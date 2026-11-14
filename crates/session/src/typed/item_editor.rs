//! The Items tab's item edits and the editor's display item (`addItem`,
//! `editItem`, `setDisplayItem`, `saveDisplayItem` of `lua/session.lua`):
//! `ItemsTab:AddItem`, `AddDisplayItem`, `CreateDisplayItemFromRaw`,
//! `CopyAnointsAndEldritchImplicits`, `SetDisplayItem`.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedSession};
use engine::item::{INFLUENCES, Item};
use engine::item_craft::{AffixSeries, AffixTable};
use engine::item_data::AffixSet;
use serde_json::{Value as J, json};
use std::sync::Arc;

/// `main.defaultItemAffixQuality`
const DEFAULT_AFFIX_QUALITY: f64 = 0.5;

/// The item editor's controls (`ItemsTab.controls.displayItem*`)
#[derive(Clone, Debug, Default)]
pub struct Editor {
    /// `displayItemAffix1` to `displayItemAffix9`
    pub affix: [AffixControl; 9],
    /// The item editor's popups, front first
    pub popups: Vec<super::item_popups::ItemPopup>,
    /// How many of `ui.popups` describe them (at the front)
    pub described: usize,
    /// `lastCraftRaritySel`, `lastCraftTypeSel`, `lastCraftBaseSel`
    pub last_craft: Option<(usize, usize, usize)>,
    /// The influence drop-downs' selections
    pub influence: [f64; 2],
    /// The quality and catalyst quality edits' text
    pub quality_buf: String,
    pub catalyst_quality_buf: String,
    /// `displayItemClusterJewelNodeCount.val`
    pub cluster_count_val: f64,
    /// `displayItemRangeLine`'s list and selection, and the range
    /// slider's value
    pub range_list: Vec<String>,
    pub range_sel: usize,
    pub range_val: Option<f64>,
    /// `craftingSorting`'s selection, 0-based (0: Default)
    pub crafting_sort: usize,
    /// The Items tab's list of items, which drawing it updates
    pub list: std::cell::RefCell<super::item_list::ItemList>,
}

/// `sortingOptions`: Default, then the power stats for items but Name
pub(super) fn crafting_sort_options() -> Vec<(String, Option<engine::power::PowerStat>)> {
    let mut list = vec![("Default".to_string(), None)];
    for p in engine::power::power_stats() {
        if !p.ignore_for_items && p.label != "Name" {
            list.push((p.label.clone(), Some(p)));
        }
    }
    list
}

/// An affix drop-down and its roll slider
#[derive(Clone, Debug)]
pub struct AffixControl {
    pub list: Vec<Option<AffixSeries>>,
    /// 0-based
    pub sel: usize,
    pub table: AffixTable,
    /// 1-based
    pub index: usize,
    pub slider_val: f64,
    pub slider_div: Option<f64>,
    pub slider_shown: bool,
}

impl Default for AffixControl {
    fn default() -> AffixControl {
        AffixControl {
            list: vec![None],
            sel: 0,
            table: AffixTable::Prefixes,
            index: 1,
            slider_val: 0.0,
            slider_div: None,
            slider_shown: false,
        }
    }
}

/// `SliderControl:GetDivVal(val)`
pub(super) fn div_val(val: f64, div: Option<f64>) -> (usize, f64) {
    match div {
        Some(d) if d > 1.0 => {
            let i = (val * d).ceil().max(1.0);
            (i as usize, val * d - i + 1.0)
        }
        _ => (1, val),
    }
}

/// The affix drop-downs' `verifyRange`: flips the roll where the tiers'
/// ranges run the other way
pub(super) fn verify_range(range: f64, index: usize, mods: &[String], affixes: &AffixSet) -> f64 {
    let min_max = |id: &str| -> Option<(f64, f64)> {
        let m = affixes.get(id)?;
        m.lines.iter().find_map(|l| {
            let c = engine::pat::captures(l, "%((%d[%d%.]*)%-(%d[%d%.]*)%)")?;
            Some((
                engine::pat::to_number(&c[0])?,
                engine::pat::to_number(&c[1])?,
            ))
        })
    };
    let flip = |a: &str, b: &str| -> bool {
        let (Some((min_a, max_a)), Some((min_b, max_b))) = (min_max(a), min_max(b)) else {
            return false;
        };
        let ints = [min_a, max_a, min_b, max_b]
            .iter()
            .all(|v| v.fract() == 0.0);
        if !ints {
            return false;
        }
        if min_a < min_b {
            min_a + 1.0 == max_b
        } else {
            min_a - 1.0 == max_b
        }
    };
    let cur = &mods[index - 1];
    if index > 1 && affixes.get(&mods[index - 2]).is_some() {
        if flip(&mods[index - 2], cur) {
            return 1.0 - range;
        }
    } else if index + 1 < mods.len()
        && affixes.get(&mods[index]).is_some()
        && flip(cur, &mods[index])
    {
        return 1.0 - range;
    }
    range
}

/// `forbiddenJewelCounterpart`
pub(super) const FORBIDDEN: [(&str, &str); 2] = [
    ("Forbidden Flame", "Forbidden Flesh"),
    ("Forbidden Flesh", "Forbidden Flame"),
];

/// `isAnointable(item)`
fn anointable(item: &Item) -> bool {
    item.base.as_ref().is_some_and(|b| {
        !b.cannot_be_anointed
            && b.sub_type.as_deref() != Some("Talisman")
            && (item.can_be_anointed || b.typ == "Amulet")
    })
}

impl<'d> TypedSession<'d> {
    /// `new("Item", raw)`, `None` when its base is not recognised
    fn new_item(&self, raw: &str) -> Result<Option<Item>> {
        let item = Item::new(&self.data.item_context(), raw, None, false)?;
        Ok(item.base.is_some().then_some(item))
    }

    /// `BuildAndParseRaw()`
    pub(super) fn build_and_parse_raw(&self, item: &mut Item) -> Result<()> {
        let raw = item.build_raw();
        item.parse_raw(&self.data.item_context(), &raw, None, false)
    }

    /// `CopyAnointsAndEldritchImplicits(newItem, copyEldritchImplicits,
    /// false)`
    fn copy_anoints_and_eldritch_implicits(&self, new: &mut Item, eldritch: bool) -> Result<()> {
        let tab = &self.build.items;
        let Some(base) = new.base.clone() else {
            return Ok(());
        };
        let typ = if base.weapon.is_some() {
            "Weapon 1"
        } else {
            base.typ.as_str()
        };
        let Some(sel) = tab.active_item_set().slots.get(typ).map(|s| s.sel_item_id) else {
            return Ok(());
        };
        let Some(current) = tab.item(sel).filter(|_| sel != 0.0) else {
            return Ok(());
        };
        if anointable(new) && new.enchant_mod_lines.is_empty() && sel > 0.0 {
            // Not an amulet with more than one anoint, as Stranglegasp
            if current.enchant_mod_lines.len() == 1 {
                new.enchant_mod_lines = current.enchant_mod_lines.clone();
            }
        }
        if new.influences[..6].iter().any(|i| *i) {
            return Ok(());
        }
        let modifiable = !(new.corrupted || new.mirrored);
        let (cleansing, tangle) = (INFLUENCES.len() - 2, INFLUENCES.len() - 1);
        if eldritch
            && ["Helmet", "Body Armour", "Gloves", "Boots"].contains(&base.typ.as_str())
            && ["NORMAL", "MAGIC", "RARE"].contains(&new.rarity.as_str())
            && new.implicit_mod_lines.is_empty()
            && modifiable
            && (current.influences[cleansing] || current.influences[tangle])
        {
            new.implicit_mod_lines = current.implicit_mod_lines.clone();
            new.influences[tangle] = current.influences[tangle];
            new.influences[cleansing] = current.influences[cleansing];
        }
        // Harvest and heist enchantments
        if (base.weapon.is_some() || base.typ == "Body Armour")
            && new.enchant_mod_lines.is_empty()
            && sel > 0.0
            && modifiable
        {
            new.enchant_mod_lines = current.enchant_mod_lines.clone();
        }
        self.build_and_parse_raw(new)
    }

    /// `CreateDisplayItemFromRaw(raw, true)`: leaves the display item as
    /// it is when the base is not recognised
    fn create_display_item_from_raw(&mut self, raw: &str) -> Result<()> {
        self.display_item_from_raw(raw, true)
    }

    /// `CreateDisplayItemFromRaw(raw, normalise)`
    pub(super) fn display_item_from_raw(&mut self, raw: &str, normalise: bool) -> Result<()> {
        let Some(mut item) = self.new_item(raw)? else {
            return Ok(());
        };
        if normalise {
            // `main.migrateEldritchImplicits`, true by default
            self.copy_anoints_and_eldritch_implicits(&mut item, true)?;
            item.normalise_quality();
            item.build_mod_list(&self.data.item_context())?;
        }
        self.set_display(Some(item))
    }

    /// `SetDisplayItem(item)`
    pub(super) fn set_display_item_value(&mut self, item: Option<Item>) -> Result<()> {
        self.set_display(item)
    }

    pub(super) fn has_display_item(&self) -> bool {
        self.display_item.is_some()
    }

    pub(super) fn display_item_id(&self) -> Option<f64> {
        self.display_item.as_ref().and_then(|i| i.id)
    }

    /// `self.displayItem.id = id`
    pub(super) fn set_display_item_id(&mut self, id: Option<f64>) {
        if let Some(item) = &mut self.display_item {
            item.id = id;
        }
    }

    /// `SetDisplayItem(item)`, the item's side of it: the affix controls
    /// of a crafted item, and a crafted cluster jewel crafted again
    fn set_display(&mut self, item: Option<Item>) -> Result<()> {
        self.display_item = item.map(|mut item| {
            if item.catalyst.is_none_or(|c| c == 0) {
                item.catalyst_quality = None;
            }
            item
        });
        let Some(item) = &self.display_item else {
            return Ok(());
        };
        let (crafted, cluster) = (item.crafted, item.cluster_jewel_size_index.is_some());
        if crafted {
            self.update_affix_controls()?;
        }
        self.set_display_controls();
        if crafted && cluster {
            self.update_cluster_jewel_controls()?;
        }
        Ok(())
    }

    /// `UpdateAffixControls()`
    pub(super) fn update_affix_controls(&mut self) -> Result<()> {
        let item = self.display_item.as_ref().ok_or("no item in the editor")?;
        let data = &self.data.game.items;
        let prefix_limit = item.affix_table_limit(AffixTable::Prefixes);
        let ignore_mod_type = item
            .rare_like_unique
            .as_ref()
            .and_then(|r| data.rare_like_uniques.get(r))
            .is_some_and(|r| r.ignore_mod_type);
        // `craftingSorting`, when shown: the series by power
        let base = item.base.as_ref().ok_or("no base")?;
        let shown =
            item.crafted && !(base.typ == "Jewel" && base.sub_type.as_deref() == Some("Cluster"));
        let sort_stat = crafting_sort_options()
            .get(self.editor.crafting_sort)
            .and_then(|o| o.1.clone())
            .filter(|_| shown);
        let mut shared: std::collections::HashMap<String, f64> = Default::default();
        let limit = item.affix_limit;
        let mut i = 1.0;
        while i <= limit {
            let item = self.display_item.as_ref().ok_or("no item in the editor")?;
            let n = i as usize;
            let (typ, table, index) = if i <= prefix_limit {
                let typ = (!ignore_mod_type).then_some("Prefix");
                (typ, AffixTable::Prefixes, n)
            } else {
                (
                    Some("Suffix"),
                    AffixTable::Suffixes,
                    (i - prefix_limit) as usize,
                )
            };
            i += 1.0;
            let (mut list, mut sel) = item.affix_options(data, typ, table, index)?;
            if let Some(stat) = &sort_stat {
                sel = self.sort_affixes_by_power(&mut list, table, index, stat, &mut shared)?;
            }
            let item = self.display_item.as_ref().ok_or("no item in the editor")?;
            let c = self.editor.affix.get_mut(n - 1).ok_or("no affix control")?;
            c.table = table;
            c.index = index;
            c.slider_shown = false;
            c.slider_val = DEFAULT_AFFIX_QUALITY;
            if let Some(Some(series)) = list.get(sel)
                && series.have_range
            {
                let div = series.mod_list.len() as f64;
                let slot = &item.affix_slots(table)[index - 1];
                let at = series
                    .mod_list
                    .iter()
                    .position(|m| *m == slot.mod_id)
                    .unwrap()
                    + 1;
                let range = match &slot.range {
                    Some(engine::item_tools::Range::One(r)) => *r,
                    Some(engine::item_tools::Range::Each(v)) => v.first().copied().unwrap_or(0.5),
                    None => 0.5,
                };
                let mut range = range.clamp(0.0, 1.0);
                if range == 0.0 && at > 1 {
                    range = 1e-4;
                }
                c.slider_div = Some(div);
                c.slider_val = (at as f64 - 1.0 + range) / div;
                if div == 1.0 {
                    c.slider_div = None;
                }
                c.slider_shown = true;
            }
            c.list = list;
            c.sel = sel;
        }
        Ok(())
    }

    /// The power sort of `UpdateAffixControl`: "None" first, then the
    /// series by the power of the stat with their middle tier added to the
    /// item (crafted without the control's own affix); the new selection
    fn sort_affixes_by_power(
        &self,
        list: &mut [Option<AffixSeries>],
        table: AffixTable,
        index: usize,
        stat: &engine::power::PowerStat,
        shared: &mut std::collections::HashMap<String, f64>,
    ) -> Result<usize> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let ctx = self.data.item_context();
        let sel_affix = display.affix_slots(table)[index - 1].mod_id.clone();
        let mut subject = Item::new(&ctx, &display.build_raw(), None, false)?;
        let mut own = std::collections::HashMap::new();
        let cache = if sel_affix != "None" {
            subject.affix_slots_mut(table)[index - 1] = engine::item::AffixSlot {
                mod_id: "None".into(),
                range: None,
                fractured: false,
            };
            subject.craft(&ctx)?;
            &mut own
        } else {
            shared
        };
        let calc_data = self.data.calc_data();
        let calc = engine::calculator::MiscCalculator::new(&self.build, &calc_data)?;
        let slot = display.primary_slot();
        let mut power = |id: &str| -> Result<f64> {
            if let Some(&p) = cache.get(id) {
                return Ok(p);
            }
            let m = subject
                .affixes
                .as_ref()
                .and_then(|a| a.get(id))
                .ok_or("attempt to get length of a nil value (local 'mod')")?;
            let mut item = subject.clone();
            for l in &m.lines {
                item.explicit_mod_lines.push(engine::item::ModLine {
                    line: l.clone(),
                    mod_tags: m.mod_tags.clone(),
                    ..Default::default()
                });
            }
            let raw = item.build_raw();
            item.parse_raw(&ctx, &raw, None, false)?;
            let ovr = engine::calc_override::Override {
                rep_item: Some((slot.clone(), Some(item))),
                ..Default::default()
            };
            // `calcFunc(override)`: Full DPS too, which only its stat reads
            let full_dps = stat.stat.as_deref() == Some("FullDPS");
            let p = stat.value(&calc.calculate(&ovr, full_dps)?);
            cache.insert(id.to_string(), p);
            Ok(p)
        };
        // `pickModifierFromList`: the middle tier
        let pick = |s: &AffixSeries| -> String {
            let n = s.mod_list.len();
            let i = if n == 1 {
                0
            } else {
                ((n - 1) as f64 * DEFAULT_AFFIX_QUALITY + 0.5).floor() as usize
            };
            s.mod_list[i].clone()
        };
        let mut powers: std::collections::HashMap<String, f64> = Default::default();
        for s in list.iter().flatten() {
            let id = pick(s);
            if let std::collections::hash_map::Entry::Vacant(e) = powers.entry(id) {
                let p = power(e.key())?;
                e.insert(p);
            }
        }
        engine::calcs::lua_sort(list, |a, b| match (a, b) {
            (None, _) => true,
            (_, None) => false,
            (Some(a), Some(b)) => powers[&pick(a)] > powers[&pick(b)],
        });
        // `findSelectedIdx()`
        let sel = list
            .iter()
            .position(|e| match e {
                Some(s) => s.mod_list.contains(&sel_affix),
                None => sel_affix == "None",
            })
            .unwrap_or(0);
        Ok(sel)
    }

    /// `UpdateClusterJewelControls()`: the skill and node count in their
    /// bounds, then `CraftClusterJewel()`
    fn update_cluster_jewel_controls(&mut self) -> Result<()> {
        let data = &self.data.game.items;
        let item = self.display_item.as_mut().ok_or("no item in the editor")?;
        let cj = item
            .base_name
            .as_ref()
            .and_then(|b| data.cluster_jewels.get(b))
            .ok_or("no cluster jewel")?;
        let unavailable = [
            "affliction_strength",
            "affliction_dexterity",
            "affliction_intelligence",
        ];
        let mut skills: Vec<(&String, &String)> = cj
            .skill_data
            .iter()
            .filter(|(id, _)| !unavailable.contains(&id.as_str()))
            .map(|(id, s)| (&s.name, id))
            .collect();
        skills.sort();
        if item
            .cluster_jewel_skill
            .as_ref()
            .is_none_or(|s| !cj.skill_data.contains_key(s))
        {
            item.cluster_jewel_skill = Some(skills.first().ok_or("no cluster skill")?.1.clone());
        }
        let count = item.cluster_jewel_node_count.unwrap_or(cj.min_nodes);
        let count = count.max(cj.min_nodes).min(cj.max_nodes);
        item.cluster_jewel_node_count = Some(count);
        self.editor.cluster_count_val = (count - cj.min_nodes) / (cj.max_nodes - cj.min_nodes);
        self.craft_cluster_jewel()
    }

    /// `CraftClusterJewel()`: the enchant lines of the jewel's size, node
    /// count and skill, then every affix selected again
    pub(super) fn craft_cluster_jewel(&mut self) -> Result<()> {
        let data = &self.data.game.items;
        let ctx = self.data.item_context();
        let item = self.display_item.as_mut().ok_or("no item in the editor")?;
        let cj = item
            .base_name
            .as_ref()
            .and_then(|b| data.cluster_jewels.get(b))
            .ok_or("no cluster jewel")?;
        let crafted = |line: String| {
            let mut ml = engine::item::ModLine {
                line,
                ..Default::default()
            };
            ml.flags.crafted = true;
            ml
        };
        let count = item.cluster_jewel_node_count.unwrap_or(cj.min_nodes);
        let mut lines = vec![crafted(format!(
            "Adds {} Passive Skills",
            engine::pat::num(count)
        ))];
        match cj.size.as_str() {
            "Large" => lines.push(crafted("2 Added Passive Skills are Jewel Sockets".into())),
            "Medium" => lines.push(crafted("1 Added Passive Skill is a Jewel Socket".into())),
            _ => {}
        }
        let skill = item
            .cluster_jewel_skill
            .as_ref()
            .and_then(|s| cj.skill_data.get(s))
            .ok_or("attempt to index local 'skill' (a nil value)")?;
        lines.push(crafted(skill.enchant.join("\n")));
        item.enchant_mod_lines = lines;
        let raw = item.build_raw();
        item.parse_raw(&ctx, &raw, None, false)?;
        // Every affix again, to force out those no longer valid
        self.update_affix_controls()?;
        // The loop's bound is read once
        let limit = self.display_item.as_ref().unwrap().affix_limit;
        let mut i = 0;
        while ((i + 1) as f64) <= limit {
            self.select_affix(i)?;
            i += 1;
        }
        Ok(())
    }

    /// The affix drop-down's `selFunc` with its selected entry: the affix
    /// of that tier and roll, crafted
    pub(super) fn select_affix(&mut self, i: usize) -> Result<()> {
        let ctx = self.data.item_context();
        let c = &self.editor.affix[i];
        let item = self.display_item.as_mut().ok_or("no item in the editor")?;
        let slots = item.affix_slots(c.table);
        let fractured = slots
            .get(c.index - 1)
            .ok_or("attempt to index field '?' (a nil value)")?
            .fractured;
        let mut affix = engine::item::AffixSlot {
            mod_id: "None".into(),
            range: None,
            fractured,
        };
        if let Some(Some(series)) = c.list.get(c.sel) {
            let div = series.mod_list.len() as f64;
            let (at, range) = div_val(c.slider_val, Some(div));
            affix.mod_id = series.mod_list[at - 1].clone();
            let affixes = item.affixes.clone().ok_or("no affixes")?;
            let range = verify_range(range, at, &series.mod_list, &affixes);
            affix.range = Some(engine::item_tools::Range::One(range));
        }
        let slots = item.affix_slots_mut(c.table);
        slots[c.index - 1] = affix;
        item.craft(&ctx)?;
        self.update_affix_controls()
    }

    /// `AddItem(item, noAutoEquip)`: the item's id
    pub(super) fn add_item(&mut self, mut item: Item, no_auto_equip: bool) -> Result<f64> {
        let new = item.id.is_none();
        if new {
            let mut id = 1.0;
            while self.build.items.item(id).is_some() {
                id += 1.0;
            }
            item.id = Some(id);
        }
        let id = item.id.unwrap();
        item.build_mod_list(&self.data.item_context())?;
        let cluster = item.cluster_jewel_size_index.is_some();
        let replacing = self.build.items.items.insert(id as i64, Arc::new(item));
        if new {
            self.build.items.item_order_list.push(id);
            self.item_keys.insert(id as u32, ());
            if !no_auto_equip {
                let ring3 = self.sidebar()?.additional_ring_slot;
                let item = self.build.items.item(id).unwrap();
                let slot = (0..self.build.items.ordered_slots.len()).find(|&i| {
                    let s = &self.build.items.ordered_slots[i];
                    s.node_id.is_none()
                        && s.sel_item_id == 0.0
                        && self.slot_shown(i, ring3)
                        && self.valid_for_slot(item, &s.slot_name)
                });
                if let Some(i) = slot {
                    self.set_sel_item(i, id)?;
                }
            }
        }
        if let Some(old) = replacing {
            let rebuild = old.cluster_jewel_size_index.is_some()
                || cluster
                || old.base_name.as_deref() == Some("Timeless Jewel");
            if rebuild && self.build.spec.jewels.values().any(|j| *j == id) {
                let b = &mut self.build;
                Arc::make_mut(&mut b.spec)
                    .rebuild_cluster_jewel_graphs(&b.items, &self.data.game.items)?;
            }
        }
        Ok(id)
    }

    /// `AddDisplayItem()`
    pub(super) fn add_display_item(&mut self) -> Result<()> {
        let item = self.display_item.take().ok_or("no item in the editor")?;
        let old = item.id.and_then(|id| self.build.items.item(id)).cloned();
        let other = FORBIDDEN
            .iter()
            .find(|(t, _)| item.title.as_deref() == Some(*t))
            .map(|(_, o)| *o);
        let id = self.add_item(item, false)?;
        if let Some(other) = other {
            let item = self.build.items.item(id).unwrap().clone();
            self.update_forbidden_counterpart(old.as_ref(), &item, other)?;
            self.add_forbidden_counterpart(&item, other)?;
        }
        self.items_changed()
    }

    /// `PopulateSlots()` and `buildFlag`
    fn items_changed(&mut self) -> Result<()> {
        self.populate_slots();
        self.rebuild()
    }

    /// `addItem`: the item from its text, equipped in the slot or where
    /// upstream would auto-equip it
    pub fn add_item_text(&mut self, text: &str, slot: Option<&str>) -> Result<J> {
        self.create_display_item_from_raw(text)?;
        let id = match slot {
            Some(slot) => {
                let item = self.display_item.take().ok_or("unrecognised item")?;
                let i = self
                    .build
                    .items
                    .ordered_slots
                    .iter()
                    .position(|s| s.slot_name == slot)
                    .ok_or_else(|| format!("{NOT_PORTED}: no slot {slot}"))?;
                let id = self.add_item(item, true)?;
                self.set_sel_item(i, id)?;
                self.items_changed()?;
                id
            }
            None => {
                let item = self.display_item.as_ref().ok_or("unrecognised item")?;
                let id = item.id;
                self.add_display_item()?;
                match id {
                    Some(id) => id,
                    // The id `AddItem` gave it: the last one listed
                    None => *self.build.items.item_order_list.last().unwrap(),
                }
            }
        };
        Ok(json!({ "id": super::num(id) }))
    }

    /// `editItem`: replaces an item's text, keeping its id and slots
    pub fn edit_item(&mut self, id: f64, text: &str) -> Result<J> {
        if self.build.items.item(id).is_none() {
            return Err(format!("no item {}", engine::pat::num(id)));
        }
        let mut item = self.new_item(text)?.ok_or("unrecognised item")?;
        item.id = Some(id);
        self.add_item(item, true)?;
        self.items_changed()?;
        Ok(json!([]))
    }

    /// `setDisplayItem`: a copy of one of the build's items, an item from
    /// its text, or none
    pub fn set_display_item(&mut self, id: Option<f64>, text: Option<&str>) -> Result<J> {
        match (id, text) {
            (Some(id), _) => {
                let item = self
                    .build
                    .items
                    .item(id)
                    .ok_or_else(|| format!("no item {}", engine::pat::num(id)))?;
                let raw = item.build_raw();
                let mut copy = self.new_item(&raw)?.ok_or("unrecognised item")?;
                copy.id = item.id;
                self.set_display(Some(copy))?;
            }
            (None, Some(text)) => {
                self.create_display_item_from_raw(text)?;
                if self.display_item.is_none() {
                    return Err("unrecognised item".into());
                }
            }
            (None, None) => self.set_display(None)?,
        }
        Ok(json!([]))
    }

    /// `saveDisplayItem`: adds the display item, or replaces the item it
    /// was opened from
    pub fn save_display_item(&mut self) -> Result<J> {
        let id = self
            .display_item
            .as_ref()
            .ok_or("no item in the editor")?
            .id;
        self.add_display_item()?;
        Ok(match id {
            Some(id) => json!({ "id": super::num(id) }),
            None => json!([]),
        })
    }
}
