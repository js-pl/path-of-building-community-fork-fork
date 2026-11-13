//! The item editor's Anoint Item popup (`ItemsTab:AnointDisplayItem`) and
//! its notable list (`Classes/NotableDBControl.lua`): the anointable
//! notables of the spec's tree, filtered by a search, sorted by name or by
//! the power of a stat with the anointed item in its slot; Anoint with the
//! selected notable, or remove the slot's anoint.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::ItemPopup;
use super::{NOT_PORTED, Result, TypedSession};
use engine::calc_override::Override;
use engine::calculator::MiscCalculator;
use engine::compare;
use engine::item::{Item, ModLine};
use engine::parallel::Threads;
use engine::pat::{gsub, is_match, lower, match_or_pattern};
use engine::power::{PowerStat, mod_key, power_stats};
use engine::tree::Node;
use serde_json::{Value as J, json};

/// The sort list (`BuildSortOrder`): label and power stat, `None` for Name
fn sort_list() -> Vec<(String, Option<PowerStat>)> {
    power_stats()
        .into_iter()
        .filter(|p| !p.ignore_for_items)
        .map(|p| {
            let label = format!("Sort by {}", p.label);
            (label, p.stat.is_some().then_some(p))
        })
        .collect()
}

const SEARCH_MODES: [&str; 3] = ["Anywhere", "Names", "Modifiers"];

/// The popup's state; selections are 1-based as upstream's
#[derive(Clone, Debug)]
pub struct AnointPopup {
    /// `self.anointEnchantSlot`
    slot: usize,
    search: String,
    search_mode: usize,
    sort_sel: usize,
    /// The list as last built: node ids in order
    list: Vec<u32>,
    /// `selValue`: the selected notable
    sel: Option<u32>,
}

/// `ItemsTab:getAnoint(item)`: the names of the nodes the item allocates
pub(super) fn anoints(item: &Item) -> Vec<String> {
    let mut out = Vec::new();
    for list in [
        &item.enchant_mod_lines,
        &item.scourge_mod_lines,
        &item.implicit_mod_lines,
        &item.explicit_mod_lines,
        &item.crucible_mod_lines,
    ] {
        for m in list {
            if let Some((start, _)) = engine::pat::find(&m.line, "Allocates ([a-zA-Z ]+)") {
                out.push(m.line[start + "Allocates ".len()..].to_string());
            }
        }
    }
    out
}

impl<'d> TypedSession<'d> {
    /// `ItemsTab:AnointDisplayItem(enchantSlot)`
    pub(super) fn anoint_display_item(&mut self, slot: usize) -> Result<()> {
        if self.display_item.is_none() {
            return Err(format!("{NOT_PORTED}: anointing without an item"));
        }
        let mut p = AnointPopup {
            slot,
            search: String::new(),
            search_mode: 1,
            sort_sel: 1,
            list: Vec::new(),
            sel: None,
        };
        self.build_notable_list(&mut p)?;
        self.editor.popups.insert(0, ItemPopup::Anoint(Box::new(p)));
        Ok(())
    }

    /// `anointItem(node)`: the display item with the slot's enchantment
    /// replaced by the node's, or removed
    pub(super) fn anoint_item(&self, slot: usize, node: Option<&Node>) -> Result<Item> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
        item.id = display.id;
        if item.enchant_mod_lines.len() >= slot {
            item.enchant_mod_lines.remove(slot - 1);
        }
        if let Some(node) = node {
            if slot > item.enchant_mod_lines.len() + 1 {
                return Err(format!(
                    "{NOT_PORTED}: an anoint slot past the item's enchantments"
                ));
            }
            let mut line = ModLine {
                line: format!("Allocates {}", node.dn),
                ..ModLine::default()
            };
            line.flags.crafted = true;
            item.enchant_mod_lines.insert(slot - 1, line);
        }
        self.build_and_parse_raw(&mut item)?;
        Ok(item)
    }

    /// `DoesNotableMatchFilters(node)`
    fn notable_matches(&self, p: &AnointPopup, node: &Node) -> Result<bool> {
        if node.recipe.is_empty() {
            return Ok(false);
        }
        let search = gsub(&lower(&p.search), "[%-%.%+%[%]%$%^%%%?%*]", "%%%0");
        if !is_match(&search, "%S") {
            return Ok(true);
        }
        // A malformed pattern matches nothing (`PCall`)
        let found = |s: &str| match_or_pattern(&lower(s), &search).unwrap_or(false);
        let mut ok = false;
        if (p.search_mode == 1 || p.search_mode == 2) && found(&node.dn) {
            ok = true;
        }
        if !ok && (p.search_mode == 1 || p.search_mode == 3) {
            ok = node.sd.iter().any(|l| found(l));
        }
        Ok(ok)
    }

    /// `ListBuilder()`: the matching notables, sorted
    fn build_notable_list(&self, p: &mut AnointPopup) -> Result<()> {
        let tree = self.build.spec.tree;
        let mut list: Vec<(&Node, f64)> = Vec::new();
        for node in &tree.nodes {
            if self.notable_matches(p, node)? {
                list.push((node, 0.0));
            }
        }
        let sort = sort_list();
        let stat = sort
            .get(p.sort_sel.wrapping_sub(1))
            .and_then(|s| s.1.clone());
        if let Some(stat) = &stat {
            let display = self.display_item.as_ref().ok_or("no item in the editor")?;
            let slot = display.base.as_ref().ok_or("no base")?.typ.clone();
            let data = self.data.calc_data();
            let calc = MiscCalculator::new(&self.build, &data)?;
            let ovr = |item: Item| Override {
                rep_item: Some((slot.clone(), Some(item))),
                ..Override::default()
            };
            // The notables with mods; the others keep a power of 0
            let mut measured = Vec::new();
            let mut nodes = Vec::new();
            for (i, (node, _)) in list.iter().enumerate() {
                if !mod_key(&node.mods).is_empty() {
                    measured.push(i);
                    nodes.push(ovr(self.anoint_item(p.slot, Some(node))?));
                }
            }
            let base = ovr(self.anoint_item(p.slot, None)?);
            let powers = compare::anoint_power(&calc, base, nodes, stat, Threads::available())?;
            let mut max: f64 = 0.0;
            let mut infinites = Vec::new();
            for (i, power) in measured.into_iter().zip(powers) {
                let power = power?;
                list[i].1 = power;
                if power == f64::INFINITY {
                    infinites.push(i);
                } else {
                    max = max.max(power);
                }
            }
            for i in infinites {
                list[i].1 = max * 2.0;
            }
        }
        let by_stat = stat.is_some();
        engine::calcs::lua_sort(&mut list, |a, b| {
            if by_stat && a.1 != b.1 {
                return b.1 < a.1;
            }
            a.0.dn < b.0.dn
        });
        p.list = list.iter().map(|(n, _)| n.id).collect();
        Ok(())
    }

    /// `control` of an input of the popup
    pub(super) fn anoint_popup_control(
        &mut self,
        p: &mut AnointPopup,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let sel = req["sel"].as_f64();
        let new_sel = |cur: usize, sel: f64, len: usize| {
            let s = sel.min(len as f64).max(1.0) as usize;
            (s != cur).then_some(s)
        };
        match name {
            "notableDB" if sel.is_some() => {
                // `SelectIndex(index)`: a row, or no selection past the list
                let i = sel.unwrap();
                p.sel = (i >= 1.0 && i.fract() == 0.0)
                    .then(|| p.list.get(i as usize - 1).copied())
                    .flatten();
                return Ok(());
            }
            "notableDB.search" if req["text"].is_string() => {
                p.search = req["text"].as_str().unwrap_or("").to_string();
            }
            "notableDB.searchMode" if sel.is_some() => {
                match new_sel(p.search_mode, sel.unwrap(), SEARCH_MODES.len()) {
                    Some(s) => p.search_mode = s,
                    None => return Ok(()),
                }
            }
            "notableDB.sort" if sel.is_some() => {
                match new_sel(p.sort_sel, sel.unwrap(), sort_list().len()) {
                    Some(s) => p.sort_sel = s,
                    None => return Ok(()),
                }
            }
            _ => return Err(format!("{NOT_PORTED}: control popup.{name} of Anoint Item")),
        }
        self.build_notable_list(p)
    }

    /// The node of the selection
    fn anoint_node(&self, p: &AnointPopup) -> Option<&'d Node> {
        let tree = self.build.spec.tree;
        p.sel
            .and_then(|id| tree.node_index.get(&id).map(|&i| &tree.nodes[i]))
    }

    /// Anoint (or remove the slot's anoint)
    pub(super) fn anoint_popup_save(&mut self, p: &AnointPopup) -> Result<()> {
        let item = self.anoint_item(p.slot, self.anoint_node(p))?;
        self.set_display_item_value(Some(item))
    }

    /// `describePopups` of Anoint Item
    pub(super) fn describe_anoint_popup(&self, p: &AnointPopup) -> J {
        let save = match self.anoint_node(p) {
            Some(node) => format!("Anoint {}", node.dn),
            None => {
                let current = self.display_item.as_ref().map(anoints).unwrap_or_default();
                match current.get(p.slot.wrapping_sub(1)) {
                    Some(name) => format!("Remove {name}"),
                    None => "No Anoint".to_string(),
                }
            }
        };
        let tree = self.build.spec.tree;
        let names: Vec<J> = p
            .list
            .iter()
            .filter_map(|id| tree.node_index.get(id).map(|&i| json!(&*tree.nodes[i].dn)))
            .collect();
        let mut list = json!({
            "class": "NotableDBControl", "enabled": true, "shown": true,
            "list": names, "path": "popup.notableDB",
        });
        if let Some(i) = p.sel.and_then(|id| p.list.iter().position(|&x| x == id)) {
            list["sel"] = json!(i + 1);
        }
        let sorts: Vec<J> = sort_list().into_iter().map(|(l, _)| json!(l)).collect();
        json!({
            "title": "Anoint Item",
            "lines": [],
            "buttons": [
                { "name": "close", "label": "Cancel", "enabled": true },
                { "name": "save", "label": save, "enabled": true },
            ],
            "controls": [
                {
                    "class": "DropDownControl", "enabled": true, "shown": true,
                    "list": sorts, "sel": p.sort_sel, "path": "popup.notableDB.sort",
                },
                {
                    "class": "EditControl", "enabled": true, "shown": true,
                    "text": p.search, "path": "popup.notableDB.search",
                },
                {
                    "class": "DropDownControl", "enabled": true, "shown": true,
                    "list": SEARCH_MODES, "sel": p.search_mode,
                    "path": "popup.notableDB.searchMode",
                },
                list,
            ],
        })
    }
}

impl<'d> TypedSession<'d> {
    /// `ItemsTab:AppendAnointTooltip(tooltip, node)`: the stats anointing
    /// the node (or nothing) gives
    fn append_anoint_tooltip(
        &self,
        tip: &mut super::tooltip::Tooltip,
        p: &AnointPopup,
        node: Option<&Node>,
    ) -> Result<()> {
        let Some(display) = self.display_item.as_ref() else {
            return Ok(());
        };
        let header = match node {
            Some(n) => {
                if self.build.spec.alloc_nodes.get(n.id).is_some() {
                    tip.add_line(
                        14.0,
                        &format!("^7Anointing {} changes nothing because this node is already allocated on the tree.", n.dn),
                    );
                    return Ok(());
                }
                if anoints(display).iter().any(|a| *a == *n.dn) {
                    tip.add_line(
                        14.0,
                        &format!(
                            "^7Anointing {} changes nothing because this node is already anointed.",
                            n.dn
                        ),
                    );
                    return Ok(());
                }
                format!("^7Anointing {} will give you: ", n.dn)
            }
            None => "^7Anointing nothing will give you: ".to_string(),
        };
        let slot = display
            .base
            .as_ref()
            .map_or("Amulet".to_string(), |b| b.typ.clone());
        let data = self.data.calc_data();
        let calc = MiscCalculator::new(&self.build, &data)?;
        let rep = |item: Item| Override {
            rep_item: Some((slot.clone(), Some(item))),
            ..Override::default()
        };
        let base = calc.calculate(&rep(display.clone()), false)?;
        let new = calc.calculate(&rep(self.anoint_item(p.slot, node)?), false)?;
        let changes = self.add_stat_compares(
            tip,
            &super::tooltip::Outputs {
                player: &base.player,
                minion: base.minion.as_ref(),
            },
            &super::tooltip::Outputs {
                player: &new.player,
                minion: new.minion.as_ref(),
            },
            &header,
            None,
        )?;
        if let Some(n) = node
            && changes == 0
        {
            tip.add_line(14.0, &format!("^7Anointing {} changes nothing.", n.dn));
        }
        Ok(())
    }

    /// The tooltips of Anoint Item: the save button's, a notable's row
    /// (`NotableDBControl:AddValueTooltip`)
    pub(super) fn anoint_tooltip(
        &self,
        tip: &mut super::tooltip::Tooltip,
        p: &AnointPopup,
        name: &str,
        idx: Option<usize>,
    ) -> Result<()> {
        if name == "save" {
            return self.append_anoint_tooltip(tip, p, self.anoint_node(p));
        }
        let tree = self.build.spec.tree;
        let Some(node) = idx
            .and_then(|i| p.list.get(i.wrapping_sub(1)))
            .and_then(|id| tree.node_index.get(id))
            .map(|&i| &tree.nodes[i])
        else {
            return Ok(());
        };
        // `AddNodeName`
        tip.header = true;
        tip.add_line(24.0, &format!("^xF8E6CA{}", node.dn));
        if !node.sd.is_empty() {
            tip.add_line(16.0, "");
            for (i, line) in node.sd.iter().enumerate() {
                let m = node.mods.get(i);
                let unsupported = m.is_none_or(|m| m.extra.is_some() || m.list.is_none());
                if &**line != " " && unsupported {
                    tip.add_line(
                        16.0,
                        &format!("^xF05050{line} ^8(Not supported in PoB yet)"),
                    );
                } else {
                    tip.add_line(16.0, &format!("^x8888FF{line}"));
                }
            }
        }
        if let Some(reminder) = &node.reminder_text {
            tip.add_separator(14.0);
            for l in reminder {
                tip.add_line(14.0, &format!("^xA0A080{l}"));
            }
        }
        tip.add_separator(14.0);
        self.append_anoint_tooltip(tip, p, Some(node))
    }
}
