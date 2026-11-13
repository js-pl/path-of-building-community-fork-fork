//! `control` of the item editor's controls (`items.displayItem*` and the
//! display item's Save, Edit and Cancel buttons): the handlers of
//! `ItemsTab`'s editor controls on the display item.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_display::{mod_lines, mod_lines_mut, range_line_index};
use super::item_editor::{div_val, verify_range};
use super::{NOT_PORTED, Result, TypedSession};
use engine::item::{INFLUENCES, Item, Socket};
use engine::item_tools::Range;
use serde_json::{Value as J, json};

/// What a `control` request does to a control
enum Act {
    Sel(f64),
    Text(String),
    State(bool),
    Click,
    Val(f64),
}

fn act(req: &J) -> Option<Act> {
    if let Some(s) = req["sel"].as_f64() {
        return Some(Act::Sel(s));
    }
    if let Some(t) = req["text"].as_str() {
        return Some(Act::Text(t.to_string()));
    }
    if let Some(s) = req["state"].as_bool() {
        return Some(Act::State(s));
    }
    if req["click"].as_bool() == Some(true) {
        return Some(Act::Click);
    }
    req["val"].as_f64().map(Act::Val)
}

/// `DropDownControl:SetSel(sel)`: the new selection if it changed
fn new_sel(sel: f64, len: usize, cur: Option<f64>) -> Option<usize> {
    let s = sel.min(len as f64).max(1.0) as usize;
    (Some(s as f64) != cur).then_some(s)
}

/// `SliderControl:SetVal(val)`: the new value if it changed
fn new_val(val: f64, cur: Option<f64>) -> Option<f64> {
    let v = val.clamp(0.0, 1.0);
    (Some(v) != cur).then_some(v)
}

/// The number of a trailing index (`displayItemAffix3` → 3)
fn index_of(name: &str, prefix: &str) -> Option<usize> {
    name.strip_prefix(prefix)?.parse().ok()
}

/// `UpdateDisplayItemVariantControls`' drop-downs of an item with variant
/// groups: each group, its available variants and the one selected
pub(super) fn variant_group_drops(item: &Item) -> Vec<(i64, Vec<i64>, Option<usize>)> {
    let mut out = Vec::new();
    for &group in item.variant_groups.keys() {
        if item.variant_group_options(group, false).is_empty() {
            continue;
        }
        if out.len() == 6 {
            break;
        }
        let available = item.variant_group_options(group, true);
        let sel = available
            .iter()
            .position(|v| item.variant_group_selections.get(&group) == Some(&(*v as f64)))
            .map(|p| p + 1);
        out.push((group, available, sel));
    }
    out
}

impl<'d> TypedSession<'d> {
    /// Whether a `control` path is one of the item editor's
    pub fn is_item_editor_control(path: &str) -> bool {
        path.starts_with("items.displayItem")
            || matches!(
                path,
                "items.addDisplayItem" | "items.editDisplayItem" | "items.removeDisplayItem"
            )
    }

    /// `BuildAndParseRaw()` of the display item, after a change to it
    fn reparse_display(&mut self, f: impl FnOnce(&mut Item)) -> Result<()> {
        let mut item = self.display_item.take().ok_or("no item in the editor")?;
        f(&mut item);
        let r = self.build_and_parse_raw(&mut item);
        self.display_item = Some(item);
        r
    }

    /// The affix drop-downs' `selFunc` with their selections, as a change
    /// of influence or catalyst forces
    fn reselect_affixes(&mut self) -> Result<()> {
        let item = self.display_item.as_ref().ok_or("no item in the editor")?;
        if !item.crafted {
            return Ok(());
        }
        let limit = item.affix_limit;
        let mut i = 0;
        while ((i + 1) as f64) <= limit {
            self.select_affix(i)?;
            i += 1;
        }
        Ok(())
    }

    /// `UpdateDisplayItemRangeLines()` after the item changed
    fn update_range_lines(&mut self) {
        let Some(item) = &self.display_item else {
            return;
        };
        let lines = self.range_line_list(item);
        if let Some(first) = lines.first() {
            self.editor.range_list = lines
                .iter()
                .filter(|ml| (ml.mod_id.is_some() && ml.new_mod_id.is_some()) || ml.range.is_some())
                .map(|ml| ml.line.clone())
                .collect();
            self.editor.range_sel = 1;
            self.editor.range_val = first.range;
        }
    }

    /// `control` of the item editor
    pub fn item_editor_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        let name = path.strip_prefix("items.").unwrap_or(path);
        let act = act(req).ok_or("no change")?;
        let unported = || Err(format!("{NOT_PORTED}: control {path}"));
        match (name, act) {
            ("addDisplayItem", Act::Click) => self.add_display_item()?,
            ("removeDisplayItem", Act::Click) => self.set_display_item(None, None).map(|_| ())?,
            ("editDisplayItem", Act::Click) => {
                if self.display_item.is_none() {
                    // Create Custom Item from Text
                    return unported();
                }
                self.edit_display_item_text()?;
                self.refresh_item_popups();
            }
            ("displayItemVersion", Act::Sel(sel)) => {
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                let len = item.version_list.as_ref().map_or(0, Vec::len);
                if let Some(s) = new_sel(sel, len, item.selected_version) {
                    self.reparse_display(|item| {
                        item.selected_version = Some(s as f64);
                        item.normalise_variant_selections();
                    })?;
                    self.update_range_lines();
                }
            }
            (n, Act::Sel(sel))
                if n == "displayItemVariant" || n.starts_with("displayItemAltVariant") =>
            {
                let k = match n {
                    "displayItemVariant" => 0,
                    "displayItemAltVariant" => 1,
                    n => index_of(n, "displayItemAltVariant").ok_or("no control")?,
                };
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                if item.uses_variant_groups {
                    let drops = variant_group_drops(item);
                    let Some((group, available, cur)) = drops.get(k).cloned() else {
                        return unported();
                    };
                    let len = available.len().max(1);
                    let Some(s) = new_sel(sel, len, Some(cur.unwrap_or(1) as f64)) else {
                        return Ok(json!([]));
                    };
                    // "No available variants" has no variant id
                    let Some(&variant) = available.get(s - 1) else {
                        return Ok(json!([]));
                    };
                    self.reparse_display(|item| {
                        item.variant_group_selections.insert(group, variant as f64);
                        item.normalise_variant_selections();
                    })?;
                } else {
                    let len = item.variant_list.as_ref().map_or(0, Vec::len);
                    let cur = if k == 0 {
                        item.variant
                    } else {
                        item.variant_alt[k - 1]
                    };
                    if let Some(s) = new_sel(sel, len, cur) {
                        self.reparse_display(|item| {
                            if k == 0 {
                                item.variant = Some(s as f64);
                            } else {
                                item.variant_alt[k - 1] = Some(s as f64);
                            }
                        })?;
                    } else {
                        return Ok(json!([]));
                    }
                }
                self.update_range_lines();
            }
            (n, Act::Sel(sel)) if n.starts_with("displayItemSocket") => {
                let i = index_of(n, "displayItemSocket").ok_or("no control")?;
                let colors = ['R', 'G', 'B', 'W'];
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                let s = item
                    .sockets
                    .get(i - 1)
                    .ok_or("attempt to index a nil value")?;
                let cur = colors
                    .iter()
                    .position(|c| *c == s.color)
                    .map(|p| (p + 1) as f64);
                if let Some(s) = new_sel(sel, colors.len(), cur) {
                    self.reparse_display(|item| item.sockets[i - 1].color = colors[s - 1])?;
                }
            }
            (n, Act::State(state)) if n.starts_with("displayItemLink") => {
                let i = index_of(n, "displayItemLink").ok_or("no control")?;
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                if item.sockets.len() <= i {
                    return Err("attempt to index a nil value".into());
                }
                self.reparse_display(|item| {
                    let s = &mut item.sockets;
                    let delta = if state && s[i - 1].group != s[i].group {
                        -1.0
                    } else if !state && s[i - 1].group == s[i].group {
                        1.0
                    } else {
                        0.0
                    };
                    for x in s.iter_mut().skip(i) {
                        x.group += delta;
                    }
                })?;
            }
            ("displayItemEnchant" | "displayItemEnchant2", Act::Click) => {
                if self.has_item_popup() || self.class_popups_open() > 0 {
                    return Err(format!("{NOT_PORTED}: a popup over another"));
                }
                self.enchant_display_item(if name.ends_with('2') { 2 } else { 1 })?;
                self.refresh_item_popups();
            }
            (
                "displayItemAnoint" | "displayItemAnoint2" | "displayItemAnoint3"
                | "displayItemAnoint4",
                Act::Click,
            ) => {
                if self.has_item_popup() || self.class_popups_open() > 0 {
                    return Err(format!("{NOT_PORTED}: a popup over another"));
                }
                let slot = index_of(name, "displayItemAnoint").unwrap_or(1);
                self.anoint_display_item(slot)?;
                self.refresh_item_popups();
            }
            ("displayItemCorrupt", Act::Click) => {
                if self.has_item_popup() || self.class_popups_open() > 0 {
                    return Err(format!("{NOT_PORTED}: a popup over another"));
                }
                self.corrupt_display_item()?;
                self.refresh_item_popups();
            }
            ("displayItemAddSocket", Act::Click) => {
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                let at = item.sockets.len() - item.abyssal_socket_count as usize;
                let (Some(color), Some(prev)) = (
                    item.default_socket_color,
                    at.checked_sub(1).and_then(|p| item.sockets.get(p)),
                ) else {
                    return unported();
                };
                let group = prev.group + 1.0;
                self.reparse_display(|item| {
                    item.sockets.insert(at, Socket { color, group });
                    for x in item.sockets.iter_mut().skip(at + 1) {
                        x.group += 1.0;
                    }
                })?;
            }
            (n @ ("displayItemInfluence" | "displayItemInfluence2"), Act::Sel(sel)) => {
                let j = usize::from(n == "displayItemInfluence2");
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                let typ = item.typ.as_deref().unwrap_or("");
                let eldritch = item.can_have_eldritch_influence
                    || ["Helmet", "Body Armour", "Gloves", "Boots"].contains(&typ);
                let len = 1 + if eldritch { INFLUENCES.len() } else { 6 };
                let Some(s) = new_sel(sel, len, Some(self.editor.influence[j])) else {
                    return Ok(json!([]));
                };
                self.editor.influence[j] = s as f64;
                let chosen = [s - 1, self.editor.influence[1 - j] as usize - 1];
                // `setDisplayItemInfluence`
                let item = self.display_item.as_mut().unwrap();
                item.influences = [false; 8];
                if item.has_elder_shaper_and_all_conqueror_influences {
                    for i in 0..6 {
                        item.influences[i] = true;
                    }
                } else {
                    for i in chosen {
                        if i > 0 {
                            item.influences[i - 1] = true;
                        }
                    }
                }
                self.reselect_affixes()?;
                self.reparse_display(|_| {})?;
            }
            ("displayItemQualityEdit", Act::Text(text)) => {
                let q = engine::pat::to_number(&text);
                self.editor.quality_buf = text;
                self.reparse_display(|item| item.quality = q)?;
            }
            ("displayItemCatalyst", Act::Sel(sel)) => {
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                let cur = (item.catalyst.unwrap_or(0) + 1) as f64;
                let Some(s) = new_sel(sel, 13, Some(cur.min(13.0))) else {
                    return Ok(json!([]));
                };
                let item = self.display_item.as_mut().unwrap();
                item.catalyst = Some(s - 1);
                if s > 1 {
                    if item.catalyst_quality.is_none() {
                        item.catalyst_quality = Some(20.0);
                        self.editor.catalyst_quality_buf = "20".into();
                    }
                } else {
                    item.catalyst_quality = None;
                    self.editor.catalyst_quality_buf = "0".into();
                }
                self.reselect_affixes()?;
                self.reparse_display(|_| {})?;
            }
            ("displayItemCatalystQualityEdit", Act::Text(text)) => {
                let q = engine::pat::to_number(&text);
                self.editor.catalyst_quality_buf = text;
                self.display_item
                    .as_mut()
                    .ok_or("no item in the editor")?
                    .catalyst_quality = q;
                self.reselect_affixes()?;
                self.reparse_display(|_| {})?;
            }
            ("displayItemClusterJewelSkill", Act::Sel(sel)) => {
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                let data = &self.data.game.items;
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
                let cur = skills
                    .iter()
                    .position(|(_, id)| item.cluster_jewel_skill.as_ref() == Some(*id))
                    .map(|p| (p + 1) as f64);
                if let Some(s) = new_sel(sel, skills.len(), cur) {
                    let id = skills[s - 1].1.clone();
                    self.display_item.as_mut().unwrap().cluster_jewel_skill = Some(id);
                    self.craft_cluster_jewel()?;
                }
            }
            ("displayItemClusterJewelNodeCount", Act::Val(val)) => {
                let Some(v) = new_val(val, Some(self.editor.cluster_count_val)) else {
                    return Ok(json!([]));
                };
                self.editor.cluster_count_val = v;
                let item = self.display_item.as_mut().ok_or("no item in the editor")?;
                let data = &self.data.game.items;
                let cj = item
                    .base_name
                    .as_ref()
                    .and_then(|b| data.cluster_jewels.get(b))
                    .ok_or("no cluster jewel")?;
                item.cluster_jewel_node_count =
                    Some((v * (cj.max_nodes - cj.min_nodes) + cj.min_nodes + 0.5).floor());
                self.craft_cluster_jewel()?;
            }
            (n, Act::Sel(sel)) if index_of(n, "displayItemAffix").is_some() => {
                let i = index_of(n, "displayItemAffix").unwrap();
                let c = self.editor.affix.get_mut(i - 1).ok_or("no control")?;
                if let Some(s) = new_sel(sel, c.list.len(), Some((c.sel + 1) as f64)) {
                    c.sel = s - 1;
                    self.select_affix(i - 1)?;
                }
            }
            (n, Act::Val(val)) if index_of(n, "displayItemAffixRange").is_some() => {
                let i = index_of(n, "displayItemAffixRange").unwrap();
                let c = self.editor.affix.get_mut(i - 1).ok_or("no control")?;
                let Some(v) = new_val(val, Some(c.slider_val)) else {
                    return Ok(json!([]));
                };
                c.slider_val = v;
                let c = c.clone();
                let ctx = self.data.item_context();
                let item = self.display_item.as_mut().ok_or("no item in the editor")?;
                let Some(Some(series)) = c.list.get(c.sel) else {
                    return Err("attempt to index field 'modList' (a nil value)".into());
                };
                let (at, range) = div_val(v, c.slider_div);
                let affixes = item.affixes.clone().ok_or("no affixes")?;
                let range = verify_range(range, at, &series.mod_list, &affixes);
                let slot = item
                    .affix_slots_mut(c.table)
                    .get_mut(c.index - 1)
                    .ok_or("attempt to index a nil value")?;
                slot.mod_id = series.mod_list.get(at - 1).cloned().unwrap_or_default();
                slot.range = Some(Range::One(range));
                item.craft(&ctx)?;
            }
            ("displayItemRangeLine", Act::Sel(sel)) => {
                let len = self.editor.range_list.len();
                if let Some(s) = new_sel(sel, len, Some(self.editor.range_sel as f64)) {
                    self.editor.range_sel = s;
                    // The line of the selection's index in the whole list
                    let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                    let lines = self.range_line_list(item);
                    self.editor.range_val = lines.get(s - 1).and_then(|ml| ml.range);
                }
            }
            ("displayItemRangeSlider", Act::Val(val)) => {
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                let index = range_line_index(item);
                let (k, i) = *index
                    .get(self.editor.range_sel.wrapping_sub(1))
                    .ok_or("attempt to index local 'line' (a nil value)")?;
                let cur = mod_lines(item)[k][i].range.or(self.editor.range_val);
                if let Some(v) = new_val(val, cur) {
                    self.editor.range_val = Some(v);
                    self.reparse_display(|item| mod_lines_mut(item, k)[i].range = Some(v))?;
                }
            }
            (n, Act::Val(val)) if index_of(n, "displayItemStackedRangeSlider").is_some() => {
                let j = index_of(n, "displayItemStackedRangeSlider").unwrap();
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                let index = range_line_index(item);
                let Some(&(k, i)) = index.get(j - 1) else {
                    return Ok(json!([]));
                };
                let cur = mod_lines(item)[k][i].range;
                if let Some(v) = new_val(val, cur) {
                    self.reparse_display(|item| mod_lines_mut(item, k)[i].range = Some(v))?;
                }
            }
            (n, Act::State(_))
                if n == "displayItemMutatedCheckbox"
                    || index_of(n, "displayItemStackedMutatedCheckbox").is_some() =>
            {
                // `MutateMod` of the line's mod, to Foulborn and back
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                let j = index_of(n, "displayItemStackedMutatedCheckbox")
                    .unwrap_or(self.editor.range_sel);
                let lines = self.range_line_list(item);
                let Some(line) = lines.get(j.wrapping_sub(1)) else {
                    return Ok(json!([]));
                };
                if let (Some(id), Some(new)) = (line.mod_id.clone(), line.new_mod_id.clone()) {
                    let mutated = !line.flags.mutated;
                    let data = self.data;
                    let mut item = self.display_item.take().ok_or("no item in the editor")?;
                    let r = item.mutate_mod(&data.game.items, &id, &new, mutated);
                    let r = match r {
                        Ok(true) => self.build_and_parse_raw(&mut item),
                        Ok(false) => Ok(()),
                        Err(e) => Err(e),
                    };
                    self.display_item = Some(item);
                    r?;
                    if n == "displayItemMutatedCheckbox" {
                        self.update_range_lines();
                    }
                }
            }
            (n, Act::Click) if index_of(n, "displayItemCustomModifierRemove").is_some() => {
                let j = index_of(n, "displayItemCustomModifierRemove").unwrap();
                let item = self.display_item.as_ref().ok_or("no item in the editor")?;
                // The line of the j-th Remove button `UpdateCustomControls` made
                let explicit = item.explicit_mod_lines.len();
                let mut found = None;
                let mut count = 0;
                if item.rare_like_unique.is_some()
                    || item.rarity == "MAGIC"
                    || item.rarity == "RARE"
                    || !item.crucible_mod_lines.is_empty()
                {
                    for (index, ml) in item
                        .explicit_mod_lines
                        .iter()
                        .chain(&item.crucible_mod_lines)
                        .enumerate()
                    {
                        if (ml.flags.custom || ml.flags.crafted || ml.flags.crucible)
                            && self.format_mod_line(ml)?.is_some()
                        {
                            count += 1;
                            if count == j {
                                found = Some(index);
                                break;
                            }
                        }
                    }
                }
                let index = found.ok_or_else(|| format!("{NOT_PORTED}: a hidden Remove button"))?;
                let mut item = self.display_item.take().unwrap();
                if index >= explicit {
                    item.crucible_mod_lines.remove(index - explicit);
                } else {
                    item.explicit_mod_lines.remove(index);
                }
                self.build_and_parse_raw(&mut item)?;
                let id = item.id;
                let raw = item.build_raw();
                self.display_item = Some(item);
                self.display_item_from_raw(&raw, false)?;
                self.set_display_item_id(id);
            }
            _ => return unported(),
        }
        Ok(json!([]))
    }
}
