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
