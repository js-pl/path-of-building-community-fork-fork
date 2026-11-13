//! The item editor's Add Crucible Modifier to Item popup
//! (`ItemsTab:AddCrucibleModifierToDisplayItem`): a mod of `data.crucible`
//! for each of the five nodes of the item's crucible tree, the item's own
//! mods selected to begin with.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::ItemPopup;
use super::{NOT_PORTED, Result, TypedSession};
use engine::calcs::lua_sort;
use engine::item::{Item, ModLine};
use serde_json::{Value as J, json};
use std::collections::HashSet;

const NODES: usize = 5;

/// An entry of a node's list: "None", or a mod of `data.crucible`
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    None,
    Mod {
        /// Index in `CraftData::crucible`
        index: usize,
        label: String,
        affix_type: Option<String>,
        /// `defaultOrder`: the mod's id
        id: String,
    },
}

/// The popup's state; selections are 1-based as upstream's
#[derive(Clone, Debug)]
pub struct CruciblePopup {
    lists: Vec<Vec<Entry>>,
    sels: Vec<usize>,
}

impl<'d> TypedSession<'d> {
    /// `ItemsTab:AddCrucibleModifierToDisplayItem()`
    pub(super) fn add_crucible_modifier_to_display_item(&mut self) -> Result<()> {
        let item = self
            .display_item
            .as_ref()
            .ok_or("attempt to index a nil value")?;
        let crucible = &self.data.game.craft.crucible;
        let mut lists: Vec<Vec<Entry>> = vec![vec![Entry::None]; NODES];
        let item_mods: HashSet<&str> = item
            .crucible_mod_lines
            .iter()
            .map(|m| m.line.as_str())
            .collect();
        // nodeSelections: the item's mod at each node, by id
        let mut selections: Vec<Option<String>> = vec![None; NODES];
        for (index, c) in crucible.iter().enumerate() {
            let m = &c.m;
            if !item.can_have_mod(&self.data.game.items, m) {
                continue;
            }
            let has = |i: usize| {
                m.lines
                    .get(i)
                    .is_some_and(|l| item_mods.contains(self.line_for_allocates(l).as_str()))
            };
            if has(0) && (m.lines.len() < 2 || has(1)) {
                let loc = |i: usize| c.node_location.get(i).map(|&n| n as usize);
                match (loc(0), loc(1)) {
                    (Some(a), Some(b))
                        if (1..=NODES).contains(&a) && selections[a - 1].is_some() =>
                    {
                        if (1..=NODES).contains(&b) {
                            selections[b - 1] = Some(m.id.clone());
                        }
                    }
                    (Some(a), _) if (1..=NODES).contains(&a) => {
                        selections[a - 1] = Some(m.id.clone())
                    }
                    _ => return Err(format!("{NOT_PORTED}: a crucible mod off the tree")),
                }
            }
            let lines: Vec<String> = m.lines.iter().map(|l| self.line_for_allocates(l)).collect();
            let label = format!("{} - Tier: {}", lines.join("/"), engine::pat::num(c.tier));
            for &loc in &c.node_location {
                let loc = loc as usize;
                if !(1..=NODES).contains(&loc) {
                    return Err(format!("{NOT_PORTED}: a crucible mod off the tree"));
                }
                lists[loc - 1].push(Entry::Mod {
                    index,
                    label: label.clone(),
                    affix_type: m.typ.clone(),
                    id: m.id.clone(),
                });
            }
        }
        for list in lists.iter_mut() {
            // "None" compares as equal to every mod (`b ~= "None"`, and a
            // string's `affixType` is nil)
            lua_sort(list, |a, b| {
                let Entry::Mod {
                    affix_type: bt,
                    id: bid,
                    ..
                } = b
                else {
                    return false;
                };
                let (at, aid) = match a {
                    Entry::Mod { affix_type, id, .. } => (affix_type.as_deref(), Some(id)),
                    Entry::None => (None, None),
                };
                if at != bt.as_deref() {
                    at == Some("Spawn") && bt.as_deref() == Some("MergeOnly")
                } else {
                    aid.is_some_and(|a| a < bid)
                }
            });
        }
        let mut sels = vec![1; NODES];
        for (node, sel) in selections.iter().enumerate() {
            if let Some(id) = sel
                && let Some(i) = lists[node]
                    .iter()
                    .rposition(|e| matches!(e, Entry::Mod { id: x, .. } if x == id))
            {
                sels[node] = i + 1;
            }
        }
        let p = CruciblePopup { lists, sels };
        self.editor
            .popups
            .insert(0, ItemPopup::Crucible(Box::new(p)));
        Ok(())
    }

    /// `control` of a node's list
    pub(super) fn crucible_popup_control(
        &mut self,
        p: &mut CruciblePopup,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let node = name
            .strip_prefix("modSelectNode")
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| (1..=NODES).contains(n));
        match (node, req["sel"].as_f64()) {
            (Some(n), Some(sel)) => {
                let len = p.lists[n - 1].len();
                p.sels[n - 1] = sel.min(len as f64).max(1.0) as usize;
                Ok(())
            }
            _ => Err(format!(
                "{NOT_PORTED}: control popup.{name} of Add Crucible Modifier"
            )),
        }
    }

    /// Add: the item with the selected mods as its crucible lines
    pub(super) fn crucible_popup_save(&mut self, p: &CruciblePopup) -> Result<()> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
        item.id = display.id;
        item.crucible_mod_lines.clear();
        let crucible = &self.data.game.craft.crucible;
        for (list, &sel) in p.lists.iter().zip(&p.sels) {
            match list.get(sel - 1) {
                // `ipairs` stops at the first missing entry
                None => break,
                Some(Entry::None) => {}
                Some(Entry::Mod { index, .. }) => {
                    let m = &crucible[*index].m;
                    for l in &m.lines {
                        let mut ml = ModLine {
                            line: self.line_for_allocates(l),
                            mod_tags: m.mod_tags.clone(),
                            ..ModLine::default()
                        };
                        ml.flags.crucible = true;
                        item.crucible_mod_lines.push(ml);
                    }
                }
            }
        }
        self.build_and_parse_raw(&mut item)?;
        self.set_display_item_value(Some(item))
    }

    /// `describePopups` of the popup
    pub(super) fn describe_crucible_popup(&self, p: &CruciblePopup) -> J {
        let controls: Vec<J> = p
            .lists
            .iter()
            .zip(&p.sels)
            .enumerate()
            .map(|(i, (list, sel))| {
                let labels: Vec<J> = list
                    .iter()
                    .map(|e| match e {
                        Entry::None => json!("None"),
                        Entry::Mod { label, .. } => json!(label),
                    })
                    .collect();
                json!({
                    "class": "DropDownControl", "enabled": true, "shown": true,
                    "label": format!("^7Node {}:", i + 1), "list": labels, "sel": sel,
                    "path": format!("popup.modSelectNode{}", i + 1),
                })
            })
            .collect();
        json!({
            "title": "Add Crucible Modifier to Item",
            "lines": [],
            "buttons": [
                { "name": "close", "label": "Cancel", "enabled": true },
                { "name": "save", "label": "Add", "enabled": true },
            ],
            "controls": controls,
        })
    }
}
