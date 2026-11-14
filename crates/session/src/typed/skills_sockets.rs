//! The Skills tab's line of the sockets of the item in the slot the shown
//! group's drop-down selects (`socketsLabel`), Optimise Sockets
//! (`optimiseSockets`, which rebuilds the item's sockets for the groups
//! socketed in it) and the imbued support's label, gem box and clear
//! button (`imbuedSupportLabel`, `imbuedSupport`, `imbuedSupportClear`).
//! Choosing an imbued support by name (`GemSelectControl`) stays on the
//! translation.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::group_detail::SLOTS;
use super::tooltip::cc;
use super::{NOT_PORTED, Result, TypedSession};
use engine::item::{Item, Socket};
use engine::skills::SocketGroup;
use serde_json::{Value as J, json};
use std::sync::Arc;

const PATHS: [&str; 5] = [
    "skills.socketsLabel",
    "skills.optimiseSockets",
    "skills.imbuedSupportLabel",
    "skills.imbuedSupport",
    "skills.imbuedSupportClear",
];

/// `GetSocketDescriptionLine(item)`
fn socket_line(item: &Item) -> String {
    let mut line = String::new();
    let mut group = 0.0;
    for (i, s) in item.sockets.iter().enumerate() {
        if i > 0 {
            line += if s.group == group { "^7=" } else { "  " };
            group = s.group;
        }
        let code = match s.color {
            'R' => cc("STRENGTH"),
            'G' => cc("DEXTERITY"),
            'B' => cc("INTELLIGENCE"),
            'W' => cc("SCION"),
            _ => "^xB0B0B0",
        };
        line += code;
        line.push(s.color);
    }
    line
}

/// A gem's skill, as the sockets count it
fn has_effect(gem: &engine::skills::GemInstance) -> bool {
    gem.granted_effect.is_some() || gem.gem.is_some()
}

/// `getSocketCounts(item)`: the sockets gems can take, and the abyssal ones
fn socket_counts(item: &Item) -> (f64, usize) {
    let abyssal = item.sockets.iter().filter(|s| s.color == 'A').count();
    let limit = item
        .base
        .as_ref()
        .and_then(|b| b.socket_limit)
        .unwrap_or(0.0);
    (limit - abyssal as f64, abyssal)
}

impl<'d> TypedSession<'d> {
    pub fn is_skills_sockets_control(path: &str) -> bool {
        PATHS.contains(&path)
    }

    /// `getSelectedItem()`: the item in the slot the drop-down selects
    fn selected_socket_item(&self) -> Option<(f64, &Item, &'static str)> {
        let name = SLOTS[self.detail.slot_sel - 1].1?;
        let tab = &self.build.items;
        let slot = tab.ordered_slots.iter().find(|s| s.slot_name == name)?;
        let item = tab.item(slot.sel_item_id)?;
        Some((slot.sel_item_id, item, name))
    }

    fn shown_group_ref(&self) -> Option<&SocketGroup> {
        let uid = self.display_group?;
        self.groups().iter().find(|g| g.uid.0 == uid)
    }

    /// `isImbuedEnabled()`
    fn imbued_enabled(&self) -> bool {
        let Some(g) = self.shown_group_ref() else {
            return false;
        };
        let Some(slot) = &g.slot else {
            return false;
        };
        let map = self
            .build
            .skills
            .as_ref()
            .map(|s| &s.imbued_support_by_slot);
        match map.and_then(|m| m.get(slot)) {
            Some(_) => g.imbued_support.is_some(),
            None => true,
        }
    }

    /// `optimiseSockets.enabled`: a gem of a group socketed in the item
    /// not in a socket of its colour
    fn optimise_enabled(&self) -> bool {
        let Some((_, item, slot)) = self.selected_socket_item() else {
            return false;
        };
        if item.base.is_none() {
            return false;
        }
        let (mut max, _) = socket_counts(item);
        for g in self
            .groups()
            .iter()
            .filter(|g| g.slot.as_deref() == Some(slot))
        {
            for gem in g.gem_list.iter().filter(|gem| has_effect(gem)) {
                if max > 0.0 {
                    if !gem.matches_socket {
                        return true;
                    }
                    max -= 1.0;
                }
            }
        }
        false
    }

    pub fn skills_sockets_json(&self, path: &str) -> Option<J> {
        let g = self.shown_group_ref();
        if self.display_group.is_some() && g.is_none() {
            return None;
        }
        let item = self.selected_socket_item();
        let sockets_shown = g.is_some() && item.is_some();
        let imbued_shown = g.is_some_and(|g| g.source.is_none());
        let imbued_enabled = self.imbued_enabled();
        Some(match path {
            "skills.socketsLabel" => {
                let line = match item {
                    Some((_, i, _))
                        if i.base.as_ref().is_some_and(|b| b.socket_limit.is_none()) =>
                    {
                        "This item cannot have sockets".to_string()
                    }
                    Some((_, i, _)) => socket_line(i),
                    None => String::new(),
                };
                json!({
                    "class": "LabelControl", "shown": sockets_shown, "enabled": true,
                    "label": format!("^7Item sockets: {line}"),
                })
            }
            "skills.optimiseSockets" => {
                let own = item.is_some_and(|(_, i, _)| {
                    i.base.as_ref().is_some_and(|b| b.socket_limit.is_some())
                });
                json!({
                    "class": "ButtonControl", "shown": sockets_shown && own,
                    "enabled": self.optimise_enabled(), "label": "Optimise Sockets",
                    "tooltip": "Rebuild the item's sockets to match the groups assigned to it.",
                })
            }
            "skills.imbuedSupportLabel" => json!({
                "class": "LabelControl", "shown": imbued_shown, "enabled": true,
                "label": format!("{}Imbued Support:", cc("CRAFTED")),
            }),
            "skills.imbuedSupport" => json!({
                "class": "GemSelectControl", "shown": imbued_shown, "enabled": imbued_enabled,
            }),
            "skills.imbuedSupportClear" => json!({
                "class": "ButtonControl", "shown": imbued_shown, "enabled": imbued_enabled,
                "label": "x", "tooltip": "Remove this imbued support.",
            }),
            _ => return None,
        })
    }

    pub fn skills_sockets_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        let click = req["click"].as_bool() == Some(true);
        match path {
            "skills.optimiseSockets" if click => self.optimise_sockets()?,
            "skills.imbuedSupportClear" if click => self.clear_imbued_support()?,
            "skills.imbuedSupport" if req["text"].is_string() => {
                return self.imbued_name_control(req["text"].as_str().unwrap_or(""));
            }
            _ => return Err(format!("{NOT_PORTED}: control {path}")),
        }
        Ok(json!([]))
    }

    /// Optimise Sockets: a socket of its colour for each gem of the groups
    /// socketed in the item, linked by group, while the item has sockets
    /// for them, then its abyssal sockets
    fn optimise_sockets(&mut self) -> Result<()> {
        let Some((id, item, slot)) = self.selected_socket_item() else {
            return Ok(());
        };
        if item.base.is_none() {
            return Ok(());
        }
        let mut item = item.clone();
        let (mut max, abyssal) = socket_counts(&item);
        let mut sockets = Vec::new();
        let mut group = 0.0;
        for g in self
            .groups()
            .iter()
            .filter(|g| g.slot.as_deref() == Some(slot))
        {
            for gem in g.gem_list.iter() {
                let effect = gem
                    .granted_effect
                    .clone()
                    .or_else(|| gem.gem.as_ref().map(|d| d.granted_effect.clone()));
                if let Some(e) = effect.filter(|_| max > 0.0) {
                    let color = match e.color {
                        Some(1.0) => 'R',
                        Some(2.0) => 'G',
                        Some(3.0) => 'B',
                        _ => 'W',
                    };
                    sockets.push(Socket { color, group });
                    max -= 1.0;
                }
            }
            group += 1.0;
        }
        for _ in 0..abyssal {
            group += 1.0;
            sockets.push(Socket { color: 'A', group });
        }
        item.sockets = sockets;
        self.build_and_parse_raw(&mut item)?;
        item.build_mod_list(&self.data.item_context())?;
        self.build.items.items.insert(id as i64, Arc::new(item));
        // `itemsTab:AddUndoState()`, a calculation
        self.ui.unsaved = true;
        self.populate_slots();
        self.rebuild()
    }

    /// The clear button: the shown group's slot loses its imbued support
    fn clear_imbued_support(&mut self) -> Result<()> {
        let Some(uid) = self.display_group else {
            return Ok(());
        };
        let tab = self.build.skills.as_mut().ok_or("no skills tab")?;
        let Some(g) = tab
            .socket_group_list_mut()
            .and_then(|l| l.iter_mut().find(|g| g.uid.0 == uid))
        else {
            return Err(format!(
                "{NOT_PORTED}: clearing the imbued support of a group no longer in the list"
            ));
        };
        let Some(slot) = g.slot.clone() else {
            return Ok(());
        };
        g.imbued_support = None;
        let had = tab.imbued_support_by_slot.remove(&slot).is_some();
        // `AddUndoState()`, and a calculation if there was one
        self.ui.unsaved = true;
        if had {
            self.rebuild()?;
        }
        Ok(())
    }
}
