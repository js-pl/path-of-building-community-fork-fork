//! The Skills tab's controls of the shown socket group (`displayGroup`):
//! its label (`groupLabel`), the slot it is socketed in (`groupSlot`, with
//! its tooltip), whether it is enabled (`groupEnabled`) and in Full DPS
//! (`includeInFullDPS`), an item's group's count (`groupCount`) and the
//! note on where an item's or a node's group comes from (`sourceNote`).
//!
//! Upstream sets the controls' texts and states when it shows a group
//! (`SetDisplayGroup`) and when they are edited, so they keep what they
//! had while the group changes otherwise: [`DetailControls`] holds them.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::tooltip::{Tooltip, cc, color};
use super::{NOT_PORTED, Result, TypedSession, num};
use engine::skills::{ExplodeSource, SocketGroup, process_socket_group};
use serde_json::{Value as J, json};

/// `groupSlotDropList`: label and slot
const SLOTS: [(&str, Option<&str>); 14] = [
    ("None", None),
    ("Weapon 1", Some("Weapon 1")),
    ("Weapon 2", Some("Weapon 2")),
    ("Weapon 1 (Swap)", Some("Weapon 1 Swap")),
    ("Weapon 2 (Swap)", Some("Weapon 2 Swap")),
    ("Helmet", Some("Helmet")),
    ("Body Armour", Some("Body Armour")),
    ("Gloves", Some("Gloves")),
    ("Boots", Some("Boots")),
    ("Amulet", Some("Amulet")),
    ("Ring 1", Some("Ring 1")),
    ("Ring 2", Some("Ring 2")),
    ("Ring 3", Some("Ring 3")),
    ("Belt", Some("Belt")),
];

const PATHS: [&str; 6] = [
    "skills.groupLabel",
    "skills.groupSlot",
    "skills.groupEnabled",
    "skills.includeInFullDPS",
    "skills.groupCount",
    "skills.sourceNote",
];

/// The texts, selection and states of the controls
#[derive(Clone, Debug)]
pub struct DetailControls {
    label: String,
    slot_sel: usize,
    enabled: bool,
    full_dps: bool,
    count: String,
}

impl Default for DetailControls {
    fn default() -> Self {
        DetailControls {
            label: String::new(),
            slot_sel: 1,
            enabled: false,
            full_dps: false,
            count: String::new(),
        }
    }
}

impl DetailControls {
    /// What `SetDisplayGroup(group)` sets them to; a slot no row has
    /// keeps the row selected (`SelByValue`)
    pub fn show(&mut self, g: &SocketGroup) {
        // `SetText(socketGroup.label)`: `tostring`, "nil" without one
        self.label = g.label.clone().unwrap_or_else(|| "nil".into());
        if let Some(i) = SLOTS.iter().position(|s| s.1 == g.slot.as_deref()) {
            self.slot_sel = i + 1;
        }
        self.enabled = g.enabled;
        self.full_dps = g.include_in_full_dps == Some(true) && g.enabled;
        self.count = engine::pat::num(g.group_count.unwrap_or(1.0));
    }
}

impl<'d> TypedSession<'d> {
    pub fn is_group_detail_control(path: &str) -> bool {
        PATHS.contains(&path)
    }

    /// The shown group, while it is in the list
    fn display_group_ref(&self) -> Option<&SocketGroup> {
        let uid = self.display_group?;
        self.groups().iter().find(|g| g.uid.0 == uid)
    }

    /// `controls` of one of the controls: they show with a group;
    /// `groupSlot`'s `enabled` reads the group, which fails without one
    pub fn group_detail_json(&self, path: &str) -> Option<J> {
        let g = self.display_group_ref();
        if self.display_group.is_some() && g.is_none() {
            // The group left the list; upstream still shows it
            return None;
        }
        let shown = g.is_some();
        let c = &self.detail;
        let source = g.is_some_and(|g| g.source.is_some());
        Some(match path {
            "skills.groupLabel" => json!({
                "class": "EditControl", "shown": shown, "enabled": true,
                "text": c.label,
            }),
            "skills.groupSlot" => {
                let g = g?;
                json!({
                    "class": "DropDownControl", "shown": shown, "enabled": g.source.is_none(),
                    "list": SLOTS.map(|s| s.0), "sel": num(c.slot_sel as f64),
                })
            }
            "skills.groupEnabled" => json!({
                "class": "CheckBoxControl", "shown": shown, "enabled": true,
                "label": "Enabled:", "state": c.enabled,
            }),
            "skills.includeInFullDPS" => json!({
                "class": "CheckBoxControl", "shown": shown, "enabled": true,
                "label": "Include in Full DPS:", "state": c.full_dps,
            }),
            "skills.groupCount" => json!({
                "class": "EditControl", "shown": shown && source, "enabled": true,
                "text": c.count,
            }),
            "skills.sourceNote" => {
                // The label reads the shown group, which fails without one
                let label = self.source_note(g?).ok()?;
                // Anchored to the sockets label, shown with an item in the
                // slot the drop-down selects
                let item = SLOTS[c.slot_sel - 1].1.is_some_and(|name| {
                    let tab = &self.build.items;
                    tab.ordered_slots
                        .iter()
                        .find(|s| s.slot_name == name)
                        .is_some_and(|s| tab.item(s.sel_item_id).is_some())
                });
                json!({ "class": "LabelControl", "shown": shown && item && source, "enabled": true, "label": label })
            }
            _ => return None,
        })
    }

    /// `sourceNote.label`
    fn source_note(&self, g: &SocketGroup) -> Result<String> {
        let items = &self.build.items;
        let item_name = |id: f64| -> Result<String> {
            let item = items.item(id).ok_or("no source item")?;
            let c = color(&item.rarity).ok_or("no rarity colour")?;
            Ok(format!("{c}{}", item.name))
        };
        let node_name = |id: u32| -> Result<String> {
            let spec = &self.build.spec;
            let i = spec.index.get(&id).ok_or("no source node")?;
            Ok(spec.tree_node(*i).dn.to_string())
        };
        if !g.explode_sources.is_empty() {
            let mut label = "^7This is a special group created for the enemy explosion effect,\nwhich comes from the following sources:".to_string();
            for s in &g.explode_sources {
                let name = match s {
                    ExplodeSource::Item(id) => item_name(*id)?,
                    ExplodeSource::Node(id) => format!("{}{}", cc("NORMAL"), node_name(*id)?),
                };
                label += &format!("\n\t{name}");
            }
            label += "^7\nYou cannot delete this group, but it will disappear if you lose the above sources.";
            return Ok(label);
        }
        let gem = g
            .gem_list
            .first()
            .ok_or("attempt to index a nil value (activeGem)")?;
        let source_name = if let Some(id) = g.source_item {
            format!("'{}", item_name(id)?)
        } else if let Some(id) = g.source_node {
            format!("'{}{}", cc("NORMAL"), node_name(id)?)
        } else {
            format!("'{}?", cc("NORMAL"))
        } + "^7'";
        let skill = gem
            .effect()
            .map_or_else(|| gem.name_spec.clone(), |e| e.name.clone());
        let undo = if g.source_node.is_some() {
            "un-allocate the node."
        } else {
            "un-equip the item."
        };
        let mut label = format!(
            "^7This is a special group created for the '{}{skill}^7' skill,\nwhich is being provided by {source_name}.\nYou cannot delete this group, but it will disappear if you {undo}",
            gem.color
        );
        if !g.no_supports && g.source_node.is_none() {
            label += &format!(
                "\n\nYou cannot add support gems to this group, but support gems in\nany other group socketed into {source_name}will automatically apply to the skill."
            );
        }
        Ok(label)
    }

    /// `control` of one of the controls; without a group shown their
    /// handlers fail upstream after the control changed, which is left to
    /// the translation
    pub fn group_detail_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        let Some(uid) = self
            .display_group
            .filter(|_| self.display_group_ref().is_some())
        else {
            return Err(format!(
                "{NOT_PORTED}: control {path} without a group shown"
            ));
        };
        let text = req["text"].as_str();
        let state = req["state"].as_bool();
        let sel = req["sel"].as_f64();
        let data = &self.data.game.skills;
        let g = self
            .build
            .skills
            .as_mut()
            .and_then(|s| s.socket_group_list_mut())
            .and_then(|l| l.iter_mut().find(|g| g.uid.0 == uid))
            .ok_or("no group")?;
        match (path, text, state, sel) {
            ("skills.groupLabel", Some(t), _, _) => {
                self.detail.label = t.to_string();
                g.label = Some(t.to_string());
                process_socket_group(g, data);
            }
            ("skills.groupEnabled", _, Some(s), _) => {
                self.detail.enabled = s;
                g.enabled = s;
            }
            ("skills.includeInFullDPS", _, Some(s), _) => {
                self.detail.full_dps = s;
                g.include_in_full_dps = Some(s);
            }
            ("skills.groupCount", Some(t), _, _) => {
                self.detail.count = t.to_string();
                g.group_count = Some(engine::pat::to_number(t).unwrap_or(1.0));
            }
            ("skills.groupSlot", _, _, Some(sel)) => {
                // `SetSel`: clamped, only a change calls it
                let sel = sel.clamp(1.0, SLOTS.len() as f64) as usize;
                if sel == self.detail.slot_sel {
                    return Ok(json!([]));
                }
                self.detail.slot_sel = sel;
                let new = SLOTS[sel - 1].1.map(String::from);
                let old = g.slot.clone();
                g.slot = new.clone();
                let had_imbued = g.imbued_support.is_some();
                let map = &mut self
                    .build
                    .skills
                    .as_mut()
                    .ok_or("no skills tab")?
                    .imbued_support_by_slot;
                // The imbued support moves with the group, if the new slot
                // has none
                let old_support = old.as_ref().and_then(|o| map.get(o).cloned());
                if let Some(support) = old_support.filter(|_| had_imbued) {
                    match &new {
                        Some(n) if !map.contains_key(n) => {
                            map.insert(n.clone(), support);
                        }
                        _ => {
                            let g = self
                                .build
                                .skills
                                .as_mut()
                                .and_then(|s| s.socket_group_list_mut())
                                .and_then(|l| l.iter_mut().find(|g| g.uid.0 == uid))
                                .ok_or("no group")?;
                            g.imbued_support = None;
                        }
                    }
                    let map = &mut self
                        .build
                        .skills
                        .as_mut()
                        .ok_or("no skills tab")?
                        .imbued_support_by_slot;
                    if let Some(o) = &old {
                        map.remove(o);
                    }
                }
            }
            _ => return Err(format!("{NOT_PORTED}: control {path}")),
        }
        // `AddUndoState()`, a calculation
        self.ui.unsaved = true;
        self.rebuild()?;
        Ok(json!([]))
    }

    /// `controlTooltip` of the slot drop-down: what it is for, or the
    /// entry's equipped item (indexing a missing entry fails upstream)
    pub fn group_slot_tooltip(&mut self, index: Option<f64>) -> Result<J> {
        let mut tip = Tooltip::default();
        let row = index
            .filter(|i| *i >= 1.0 && i.fract() == 0.0)
            .and_then(|i| SLOTS.get(i as usize - 1));
        match row {
            None if index != Some(1.0) => {
                return Err(format!(
                    "{}attempt to index a nil value",
                    super::UPSTREAM_ERROR
                ));
            }
            Some((_, None)) | None => {
                tip.add_line(16.0, "Select the item in which this skill is socketed.");
                tip.add_line(
                    16.0,
                    "This will allow the skill to benefit from modifiers on the item that affect socketed gems.",
                );
            }
            Some((_, Some(slot_name))) => {
                self.update_sockets();
                let tab = &self.build.items;
                let slot = tab.ordered_slots.iter().find(|s| s.slot_name == *slot_name);
                let item = slot.and_then(|s| tab.item(s.sel_item_id)).cloned();
                match (item, slot.cloned()) {
                    (Some(item), Some(slot)) => {
                        self.add_item_tooltip(&mut tip, &item, Some(&slot))?
                    }
                    _ => tip.add_line(16.0, "No item is equipped in this slot."),
                }
            }
        }
        Ok(tip.json())
    }
}
