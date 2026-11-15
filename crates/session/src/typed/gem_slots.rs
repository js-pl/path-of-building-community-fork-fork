//! The Skills tab's gem slots of the shown socket group
//! (`SkillsTab:CreateGemSlot`): each slot's delete button, gem name, level,
//! quality, enabled box, count, error message and the boxes of a Vaal
//! gem's two skills (`skills.gemSlot<i>Delete`, `...Name`, `...Level`,
//! `...Quality`, `...Enable`, `...Count`, `...ErrMsg`, `...EnableGlobal1`,
//! `...EnableGlobal2`), and the count's tooltip.
//!
//! Upstream makes the slots as a group needs them and keeps them; their
//! texts and states are set when a group is shown and when they are
//! edited ([`SlotTexts`]). Choosing a gem by name (`GemSelectControl`),
//! the edits of the slot after the last gem, which add one, and the
//! quality and enabled boxes' tooltips stay on the translation.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedSession};
use engine::pat::{num as lua_num, to_number};
use engine::skills::{GemInstance, SocketGroup, process_gem_level, process_socket_group};
use serde_json::{Value as J, json};

/// A slot's texts and states
#[derive(Clone, Debug, Default)]
pub struct SlotTexts {
    name: String,
    level: String,
    quality: String,
    enabled: bool,
    global1: bool,
    global2: bool,
    count: String,
    /// The name box's list was dropped (`dropped`), and its sort cache
    dropped: bool,
    cache: Option<SortCache>,
}

/// What the name box's sort cache was made for (`UpdateSortCache` keeps it
/// while they stay)
#[derive(Clone, Debug, PartialEq)]
pub struct CacheKey {
    group: u64,
    gem: Option<(Option<String>, String)>,
    revision: f64,
    show: &'static str,
    legacy: bool,
    quality: Option<f64>,
    level: engine::skills::DefaultGemLevel,
    char_level: Option<f64>,
    field: &'static str,
}

/// The name box's sort cache: its gems in `BuildList("")`'s order
#[derive(Clone, Debug)]
pub struct SortCache {
    key: CacheKey,
    order: Vec<String>,
}

/// The slots made so far: the tab makes the first
#[derive(Clone, Debug)]
pub struct GemSlots(Vec<SlotTexts>);

impl Default for GemSlots {
    fn default() -> Self {
        GemSlots(vec![SlotTexts::default()])
    }
}

/// `tostring` of a number upstream may not have
fn text(v: Option<f64>) -> String {
    v.map_or_else(|| "nil".into(), lua_num)
}

impl GemSlots {
    /// `UpdateGemSlots()`, then the texts of the group's gems
    pub fn show(&mut self, g: &SocketGroup) {
        let n = g.gem_list.len();
        while self.0.len() < n + 1 {
            self.0.push(SlotTexts::default());
        }
        let last = &mut self.0[n];
        last.name = String::new();
        last.level = String::new();
        last.quality = String::new();
        last.enabled = false;
        last.count = "1".into();
        for (i, gem) in g.gem_list.iter().enumerate() {
            self.set_from_gem(i, gem);
        }
    }

    fn set_from_gem(&mut self, i: usize, gem: &GemInstance) {
        let s = &mut self.0[i];
        s.name = gem.name_spec.clone();
        s.level = text(gem.level);
        s.quality = text(gem.quality);
        s.enabled = gem.enabled;
        s.global1 = gem.enable_global1;
        s.global2 = gem.enable_global2;
        s.count = lua_num(gem.count.unwrap_or(1.0));
    }
}

const PARTS: [&str; 9] = [
    "Delete",
    "Name",
    "Level",
    "Quality",
    "Enable",
    "Count",
    "ErrMsg",
    "EnableGlobal1",
    "EnableGlobal2",
];

/// A slot control's path: its slot (1-based) and part
pub fn parse(path: &str) -> Option<(usize, &'static str)> {
    let rest = path.strip_prefix("skills.gemSlot")?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let i: usize = rest[..digits].parse().ok()?;
    let part = PARTS.iter().find(|p| **p == &rest[digits..])?;
    (i >= 1).then_some((i, *part))
}

/// The effects of a gem the count goes with: its gem's skills, or the
/// item's skill
fn count_shown(gem: &GemInstance) -> bool {
    let effects: Vec<_> = match &gem.gem {
        Some(d) => std::iter::once(&d.granted_effect)
            .chain(d.secondary_granted_effect.as_ref())
            .collect(),
        None => gem.granted_effect.iter().collect(),
    };
    effects.iter().enumerate().any(|(k, e)| {
        let global = if k == 0 {
            gem.enable_global1
        } else {
            gem.enable_global2
        };
        !e.support && !e.unsupported && (!e.has_global_effect || global)
    })
}

/// A Vaal gem's skill `k` (0 or 1), which has a box
fn global_shown(gem: &GemInstance, k: usize) -> bool {
    let Some(d) = gem.gem.as_ref().filter(|d| d.vaal_gem) else {
        return false;
    };
    let effect = if k == 0 {
        Some(&d.granted_effect)
    } else {
        d.secondary_granted_effect.as_ref()
    };
    effect.is_some_and(|e| !e.support)
}

impl<'d> TypedSession<'d> {
    pub fn is_gem_slot_control(path: &str) -> bool {
        parse(path).is_some()
    }

    /// Whether the tab has made the slot (a control it has not is left
    /// out of `controls`)
    pub fn gem_slot_exists(&self, path: &str) -> bool {
        parse(path).is_some_and(|(i, _)| i <= self.detail.gem_slots.0.len())
    }

    /// The shown group, while it is in the list
    fn shown_group(&self) -> Option<&SocketGroup> {
        let uid = self.display_group?;
        self.groups().iter().find(|g| g.uid.0 == uid)
    }

    /// `controls` of a slot's control; most read the shown group for
    /// `enabled`, which fails without one
    pub fn gem_slot_json(&self, path: &str) -> Option<J> {
        let (i, part) = parse(path)?;
        let s = self.detail.gem_slots.0.get(i - 1)?;
        let g = self.shown_group();
        if self.display_group.is_some() && g.is_none() {
            return None;
        }
        let gem = g.and_then(|g| g.gem_list.get(i - 1));
        let n = g.map(|g| g.gem_list.len());
        // The delete button's anchors: the slots below the image label,
        // which shows with a group no item gives
        let base = g.is_some_and(|g| g.source.is_none() && i <= g.gem_list.len() + 1);
        let used = || -> Option<bool> { Some(i <= n?) };
        let count = base && gem.is_some_and(count_shown);
        Some(match part {
            "Delete" => json!({
                "class": "ButtonControl", "shown": base, "enabled": used()?, "label": "x",
                "tooltip": "Remove this gem.",
            }),
            "Name" => json!({ "class": "GemSelectControl", "shown": base, "enabled": true }),
            "Level" | "Quality" => json!({
                "class": "EditControl", "shown": base, "enabled": used()?,
                "text": if part == "Level" { &s.level } else { &s.quality },
            }),
            "Enable" => json!({
                "class": "CheckBoxControl", "shown": base, "enabled": used()?, "state": s.enabled,
            }),
            "Count" => json!({
                "class": "EditControl", "shown": count, "enabled": used()?, "text": s.count,
            }),
            "ErrMsg" => {
                let err = gem.and_then(|g| g.err_msg.clone()).unwrap_or_default();
                json!({ "class": "LabelControl", "shown": count, "enabled": true, "label": format!("^1{err}") })
            }
            _ => {
                let k = usize::from(part == "EnableGlobal2");
                let own = gem.is_some_and(|gem| global_shown(gem, k));
                let state = if k == 0 { s.global1 } else { s.global2 };
                json!({
                    "class": "CheckBoxControl", "shown": base && own, "enabled": true, "state": state,
                })
            }
        })
    }

    /// `control` of a slot's control, for a gem the shown group has
    pub fn gem_slot_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        let (i, part) = parse(path).ok_or("no gem slot")?;
        let unported = || Err(format!("{NOT_PORTED}: control {path}"));
        let Some(uid) = self.shown_group().map(|g| g.uid.0) else {
            return unported();
        };
        if i > self.detail.gem_slots.0.len() {
            return unported();
        }
        if part == "Name" {
            return self.gem_name_control(uid, i, req);
        }
        let text = req["text"].as_str();
        let state = req["state"].as_bool();
        let click = req["click"].as_bool() == Some(true);
        let data = &self.data.game.skills;
        let char_level = self.build.info.character_level;
        let tab = self.build.skills.as_mut().ok_or("no skills tab")?;
        let default_level = tab.default_gem_level;
        let default_quality = if self.gem_quality_nil {
            0.0
        } else {
            tab.default_gem_quality
        };
        let g = tab
            .socket_group_list_mut()
            .and_then(|l| l.iter_mut().find(|g| g.uid.0 == uid))
            .ok_or("no group")?;
        let slots = &mut self.detail.gem_slots;
        if part == "Delete" && click {
            // `deleteGem()`: a slot past the gems removes nothing; the
            // slots from it on show the gems that moved up
            if i <= g.gem_list.len() {
                g.gem_list.remove(i - 1);
            }
            for k in i - 1..g.gem_list.len() {
                slots.set_from_gem(k, &g.gem_list[k]);
            }
        } else {
            // Typing in the slot after the last gem adds one
            let Some(gem) = g.gem_list.get_mut(i - 1) else {
                return unported();
            };
            let s = &mut slots.0[i - 1];
            match (part, text, state) {
                ("Level", Some(t), _) => {
                    let level = match to_number(t).or(gem.natural_max_level) {
                        Some(l) => l,
                        None => match &gem.gem {
                            Some(d) => process_gem_level(d, false, default_level, char_level),
                            None => return unported(),
                        },
                    };
                    s.level = t.to_string();
                    gem.level = Some(level);
                    process_socket_group(g, data);
                }
                ("Quality", Some(t), _) => {
                    s.quality = t.to_string();
                    gem.quality = Some(to_number(t).unwrap_or(default_quality));
                    process_socket_group(g, data);
                }
                ("Enable", _, Some(on)) => {
                    // `gemInstance.gemData.vaalGem` fails without data
                    let Some(d) = &gem.gem else {
                        return unported();
                    };
                    if !d.vaal_gem {
                        s.global1 = true;
                        s.global2 = true;
                        gem.enable_global1 = true;
                        gem.enable_global2 = true;
                    }
                    s.enabled = on;
                    gem.enabled = on;
                    process_socket_group(g, data);
                }
                ("Count", Some(t), _) => {
                    let count = to_number(t).unwrap_or(1.0);
                    s.count = lua_num(count);
                    gem.count = Some(count);
                    process_socket_group(g, data);
                }
                ("EnableGlobal1", _, Some(on)) => {
                    s.global1 = on;
                    gem.enable_global1 = on;
                }
                ("EnableGlobal2", _, Some(on)) => {
                    s.global2 = on;
                    gem.enable_global2 = on;
                }
                _ => return unported(),
            }
        }
        // `AddUndoState()`, a calculation
        self.ui.unsaved = true;
        self.rebuild()?;
        Ok(json!([]))
    }

    /// `controlTooltip` of a slot's enabled box: the stats with the gem
    /// enabled or disabled
    pub fn gem_slot_enable_tooltip(&self, path: &str) -> Result<J> {
        let (i, _) = parse(path).ok_or("no gem slot")?;
        let mut tip = super::tooltip::Tooltip::default();
        let Some(uid) = self.display_group else {
            return Err(format!(
                "{}attempt to index a nil value",
                super::UPSTREAM_ERROR
            ));
        };
        let groups = self.groups();
        let Some(row) = groups.iter().position(|g| g.uid.0 == uid) else {
            return Err(format!(
                "{NOT_PORTED}: the tooltip of a group no longer in the list"
            ));
        };
        let Some(gem) = groups[row].gem_list.get(i - 1) else {
            return Ok(tip.json());
        };
        let enabled = gem.enabled;
        let mut b = self.build.clone();
        if let Some(g) = b
            .skills
            .as_mut()
            .and_then(|s| s.socket_group_list_mut())
            .and_then(|l| l.get_mut(row))
        {
            g.gem_list[i - 1].enabled = !enabled;
        }
        let data = self.data.calc_data();
        let none = engine::calc_override::Override::default();
        let calc = engine::calculator::MiscCalculator::new(&self.build, &data)?;
        // `calcFunc()`: the Full DPS too
        let o = calc.calculate_on(&b, &none, true)?;
        let base = calc.base();
        let header = if enabled {
            "^7Disabling this gem will give you:"
        } else {
            "^7Enabling this gem will give you:"
        };
        self.add_stat_compares(
            &mut tip,
            &super::tooltip::Outputs {
                player: &base.player,
                minion: base.minion.as_ref(),
            },
            &super::tooltip::Outputs {
                player: &o.player,
                minion: o.minion.as_ref(),
            },
            header,
            None,
        )?;
        Ok(tip.json())
    }

    /// `controlTooltip` of a slot's quality box: what 20% quality gives
    /// the gem's skills (their quality stats' descriptions), and the
    /// stats with the gem at 20 quality
    pub fn gem_slot_quality_tooltip(&self, path: &str) -> Result<J> {
        let (i, _) = parse(path).ok_or("no gem slot")?;
        let mut tip = super::tooltip::Tooltip::default();
        let Some(uid) = self.display_group else {
            return Err(format!(
                "{}attempt to index a nil value",
                super::UPSTREAM_ERROR
            ));
        };
        let groups = self.groups();
        let Some(row) = groups.iter().position(|g| g.uid.0 == uid) else {
            return Err(format!(
                "{NOT_PORTED}: the tooltip of a group no longer in the list"
            ));
        };
        let Some(gem) = groups[row].gem_list.get(i - 1) else {
            return Ok(tip.json());
        };
        let cc = super::tooltip::cc;
        let data = &self.data.game.skills;
        let descriptions = &self.data.game.stat_descriptions;
        let add_quality = |tip: &mut super::tooltip::Tooltip,
                           e: &engine::skill_data::GrantedEffect|
         -> Result<()> {
            tip.add_line(18.0, &format!("{}{}", cc("GEM"), e.name));
            tip.add_line(16.0, &format!("{}At +20% Quality:", cc("NORMAL")));
            let scope = e.stat_description_scope.as_deref().unwrap_or("");
            for (stat, per) in &e.quality_stats {
                let lines = descriptions
                    .describe(stat, per * 20.0, scope)
                    .map_err(|why| format!("{NOT_PORTED}: {why}"))?;
                for line in lines {
                    if e.stat_map.contains_key(stat) || data.skill_stat_map.contains_key(stat) {
                        tip.add_line(16.0, &format!("{}{line}", cc("MAGIC")));
                    } else {
                        tip.add_line(
                            16.0,
                            &format!("{}{line} ^8(Not supported in PoB yet)", cc("UNSUPPORTED")),
                        );
                    }
                }
            }
            tip.add_separator(10.0);
            Ok(())
        };
        if let Some(d) = &gem.gem {
            if !d.granted_effect.quality_stats.is_empty() {
                add_quality(&mut tip, &d.granted_effect)?;
            }
            if let Some(e) = d
                .secondary_granted_effect
                .as_ref()
                .filter(|e| !e.quality_stats.is_empty())
            {
                add_quality(&mut tip, e)?;
            }
        }
        let mut b = self.build.clone();
        if let Some(g) = b
            .skills
            .as_mut()
            .and_then(|s| s.socket_group_list_mut())
            .and_then(|l| l.get_mut(row))
        {
            g.gem_list[i - 1].quality = Some(20.0);
        }
        let calc_data = self.data.calc_data();
        let none = engine::calc_override::Override::default();
        let calc = engine::calculator::MiscCalculator::new(&self.build, &calc_data)?;
        // `calcFunc()`: the Full DPS too
        let o = calc.calculate_on(&b, &none, true)?;
        let base = calc.base();
        self.add_stat_compares(
            &mut tip,
            &super::tooltip::Outputs {
                player: &base.player,
                minion: base.minion.as_ref(),
            },
            &super::tooltip::Outputs {
                player: &o.player,
                minion: o.minion.as_ref(),
            },
            "^7Setting to 20 quality will give you:",
            None,
        )?;
        Ok(tip.json())
    }

    /// `controlTooltip` of a slot's count: what it is for
    pub fn gem_slot_count_tooltip(&self) -> J {
        let mut tip = super::tooltip::Tooltip::default();
        for l in [
            "^8Note: `count` integer value scales the DPS of associated skill by a scalar.",
            "^8To be used with totems, minions, shot-gunning of projectiles (e.g., VD, magma-orbs),",
            "^8multi-hit projectiles (e.g. ball-lightning), traps, mines.",
        ] {
            tip.add_line(16.0, l);
        }
        tip.json()
    }
}

/// `string:split(":")`: the non-empty pieces, with an empty first one
/// when the text starts with the separator
fn split_tags(s: &str) -> Vec<String> {
    let mut fields: Vec<String> = Vec::new();
    if s.starts_with(':') {
        fields.push(String::new());
    }
    fields.extend(s.split(':').filter(|p| !p.is_empty()).map(String::from));
    fields
}

/// `BuildList(buf)`'s first gem, of the gems in `BuildList("")`'s order:
/// names that are the search, are its initials, start with it, contain
/// it, which have the tags after `:` (or not, after `:-`), each round with
/// the gems of the search's tag; `None` when nothing matches, `Err` for a
/// search that is no pattern
fn first_match(
    data: &engine::skill_data::SkillData,
    order: &[String],
    buf: &str,
) -> std::result::Result<Option<String>, String> {
    if buf.is_empty() {
        return Ok(order.first().cloned());
    }
    let fields = split_tags(buf);
    let term = fields.first().cloned().unwrap_or_default();
    // The tags: an alias's tag, and whether it must be absent
    let tags: Vec<(String, bool)> = fields[1.min(fields.len())..]
        .iter()
        .map(|t| {
            let t = engine::pat::lower(&engine::pat::gsub(t, "%s+", ""));
            let (negate, t) = match t.strip_prefix('-') {
                Some(r) => (true, r.to_string()),
                None => (false, t),
            };
            let t = match t.as_str() {
                "active" => "grants_active_skill".to_string(),
                "int" => "intelligence".to_string(),
                "str" => "strength".to_string(),
                "dex" => "dexterity".to_string(),
                "aoe" => "area".to_string(),
                _ => t,
            };
            (t, negate)
        })
        .collect();
    let lower = engine::pat::lower(&term);
    let tag = engine::pat::lower(match term.as_str() {
        "active" => "grants_active_skill",
        "int" => "intelligence",
        "str" => "strength",
        "dex" => "dexterity",
        t => t,
    });
    let patterns = [
        format!("^ {lower}$"),
        format!("^{}$", engine::pat::gsub(&lower, "%a", " %0%%l+")),
        format!("^ {lower}"),
        lower.clone(),
    ];
    for p in &patterns {
        for id in order {
            let Some(gem) = data.gem(id) else {
                continue;
            };
            let name = format!(" {}", engine::pat::lower(&gem.name));
            if engine::pat::try_find(&name, p)?.is_some() {
                let ok = tags
                    .iter()
                    .all(|(t, negate)| gem.tags.contains(t) != *negate);
                if ok {
                    return Ok(Some(id.clone()));
                }
            } else if gem.tags.contains(&tag) {
                return Ok(Some(id.clone()));
            }
        }
    }
    Ok(None)
}

impl<'d> TypedSession<'d> {
    /// The name box's sort cache key for the shown group's slot
    fn gem_cache_key(&self, uid: u64, i: usize) -> Option<CacheKey> {
        let g = self.groups().iter().find(|g| g.uid.0 == uid)?;
        let tab = self.build.skills.as_ref()?;
        let o = &self.gem_options;
        let char_level = (tab.default_gem_level == engine::skills::DefaultGemLevel::CharacterLevel)
            .then_some(self.build.info.character_level);
        Some(CacheKey {
            group: uid,
            gem: g
                .gem_list
                .get(i - 1)
                .map(|gem| (gem.gem_id.clone(), gem.name_spec.clone())),
            revision: self.ui.output_revision,
            show: o.show,
            legacy: o.show_legacy,
            quality: (!self.gem_quality_nil).then_some(tab.default_gem_quality),
            level: tab.default_gem_level,
            char_level,
            field: o.field,
        })
    }

    /// `gemList` on a slot: `SetDisplayGroup(group)`, the slot's box gets
    /// a new sort cache with the gems' DPS, then the shown group again
    pub(super) fn gem_list_shown(&mut self, group: f64, slot: f64, order: Vec<String>) {
        let prev = self.display_group;
        let row = group as usize - 1;
        self.set_display_group(Some(row));
        let uid = self.display_group;
        let i = slot as usize;
        if let Some(key) = uid.and_then(|u| self.gem_cache_key(u, i))
            && let Some(s) = self.detail.gem_slots.0.get_mut(i - 1)
        {
            s.cache = Some(SortCache { key, order });
        }
        match prev.and_then(|u| self.group_row(u)) {
            Some(r) => self.set_display_group(Some(r)),
            None => self.display_group = prev,
        }
    }

    /// The name box's text: the list drops (a new sort cache unless the
    /// one it has still holds), the first gem the text finds is chosen,
    /// and the slot's gem changes to it (`gemChangeFunc`, which
    /// calculates only when the text is the gem's name)
    fn gem_name_control(&mut self, uid: u64, i: usize, req: &J) -> Result<J> {
        let unported = |why: &str| Err(format!("{NOT_PORTED}: a gem name {why}"));
        let Some(text) = req["text"].as_str() else {
            return unported("without a text");
        };
        let key = self.gem_cache_key(uid, i).ok_or("no group")?;
        let slot = &self.detail.gem_slots.0[i - 1];
        let cache = match (&slot.cache, slot.dropped) {
            (Some(c), true) => c.clone(),
            (None, true) => return unported("in a list dropped without a cache"),
            (Some(c), false) if c.key == key => c.clone(),
            _ => {
                let row = self.group_row(uid).ok_or("no group")?;
                let (_, _, order) =
                    self.gem_select_rows((row + 1) as f64, i as f64, false, false)?;
                SortCache { key, order }
            }
        };
        let data = &self.data.game.skills;
        let first = match first_match(data, &cache.order, text) {
            Ok(f) => f,
            Err(_) => return unported("that is no pattern"),
        };
        let s = &mut self.detail.gem_slots.0[i - 1];
        s.name = text.to_string();
        s.dropped = true;
        s.cache = Some(cache);
        let gem = first.as_ref().and_then(|id| data.gem(id));
        let matches = gem.is_some_and(|g| engine::pat::lower(&g.name) == engine::pat::lower(text));
        let gem_id = gem
            .filter(|_| engine::pat::is_match(text, "%S"))
            .map(|g| g.id.clone());
        // `gemChangeFunc(gemId, addUndo, focusLost, bufMatchesGem)`
        let char_level = self.build.info.character_level;
        let tab = self.build.skills.as_mut().ok_or("no skills tab")?;
        let default_level = tab.default_gem_level;
        let quality = if self.gem_quality_nil {
            0.0
        } else {
            tab.default_gem_quality
        };
        let g = tab
            .socket_group_list_mut()
            .and_then(|l| l.iter_mut().find(|g| g.uid.0 == uid))
            .ok_or("no group")?;
        let s = &mut self.detail.gem_slots.0[i - 1];
        if i > g.gem_list.len() {
            let Some(_) = &gem_id else {
                return Ok(json!([]));
            };
            if i > g.gem_list.len() + 1 {
                return unported("past the slot after the last gem");
            }
            g.gem_list.push(GemInstance {
                name_spec: String::new(),
                level: Some(1.0),
                quality: Some(quality),
                enabled: true,
                enable_global1: true,
                enable_global2: true,
                count: Some(1.0),
                new: true,
                ..GemInstance::default()
            });
            s.level = "1".into();
            s.quality = lua_num(quality);
            s.enabled = true;
            s.global1 = true;
            s.global2 = true;
            s.count = "1".into();
        } else if gem_id == g.gem_list[i - 1].gem_id {
            if matches {
                self.rebuild()?;
            }
            return Ok(json!([]));
        }
        let gem = &mut g.gem_list[i - 1];
        gem.gem_id = gem_id;
        gem.skill_id = None;
        process_socket_group(g, data);
        let gem = &mut g.gem_list[i - 1];
        // `ProcessGemLevel(gemInstance.gemData)` fails without data
        let Some(d) = gem.gem.clone() else {
            return unported("without gem data");
        };
        let level = process_gem_level(&d, false, default_level, char_level);
        gem.level = Some(level);
        gem.natural_max_level = Some(level);
        s.level = lua_num(level);
        s.count = lua_num(gem.count.unwrap_or(1.0));
        if matches {
            self.rebuild()?;
        }
        Ok(json!([]))
    }
}

impl<'d> TypedSession<'d> {
    /// The imbued support box's text: its list (the supports that can
    /// support the group's skills by their own types) drops, the first
    /// gem the text finds is chosen, and only a text that is its name
    /// imbues it into the shown group's slot; another text clears it
    pub(super) fn imbued_name_control(&mut self, text: &str) -> Result<J> {
        let unported = |why: &str| Err(format!("{NOT_PORTED}: an imbued support {why}"));
        let Some(uid) = self.display_group.filter(|u| self.group_row(*u).is_some()) else {
            return unported("without a group shown");
        };
        let key = self.gem_cache_key(uid, 1).ok_or("no group")?;
        let b = &self.detail.imbued_box;
        let cache = match (&b.cache, b.dropped) {
            (Some(c), true) => c.clone(),
            (None, true) => return unported("in a list dropped without a cache"),
            (Some(c), false) if c.key == key => c.clone(),
            _ => {
                let row = self.group_row(uid).ok_or("no group")?;
                let (_, _, order) = self.gem_select_rows((row + 1) as f64, 1.0, false, true)?;
                SortCache { key, order }
            }
        };
        let data = &self.data.game.skills;
        let first = match first_match(data, &cache.order, text) {
            Ok(f) => f,
            Err(_) => return unported("that is no pattern"),
        };
        let b = &mut self.detail.imbued_box;
        b.dropped = true;
        b.cache = Some(cache);
        let gem = first
            .as_ref()
            .and_then(|id| data.gem(id))
            .filter(|_| engine::pat::is_match(text, "%S"));
        let matches = gem.is_some_and(|g| engine::pat::lower(&g.name) == engine::pat::lower(text));
        // `gemChangeFunc(gemId, ...)`: the shown group's slot
        let tab = self.build.skills.as_mut().ok_or("no skills tab")?;
        let Some(g) = tab
            .socket_group_list_mut()
            .and_then(|l| l.iter_mut().find(|g| g.uid.0 == uid))
        else {
            return Err("no group".into());
        };
        let Some(slot) = g.slot.clone() else {
            return Ok(json!([]));
        };
        // `AddUndoState()`
        self.ui.unsaved = true;
        match gem.filter(|_| matches) {
            Some(gem) => {
                g.imbued_support = Some(gem.name.clone());
                tab.imbued_support_by_slot
                    .insert(slot, gem.granted_effect.id.clone());
                self.rebuild()?;
            }
            None => {
                g.imbued_support = None;
                if tab.imbued_support_by_slot.remove(&slot).is_some() {
                    self.rebuild()?;
                }
            }
        }
        Ok(json!([]))
    }
}
