//! The tree view's node tooltip (`PassiveTreeView:AddNodeTooltip`, with
//! `AddNodeName`): `nodeTooltip` of `lua/session.lua`, with the stat
//! comparisons of allocating or unallocating the node and its path.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::tooltip::{Outputs, Tooltip, cc};
use super::{NOT_PORTED, Result, TypedSession};
use engine::calc_override::{Override, OverrideNode};
use engine::tree::{NodeType, StatLine};
use serde_json::Value as J;

/// `data.goldRespecPrices`, by character level
fn gold_respec_price(level: f64) -> Option<f64> {
    static PRICES: std::sync::OnceLock<Vec<f64>> = std::sync::OnceLock::new();
    let prices = PRICES.get_or_init(|| {
        let text = include_str!(concat!(env!("OUT_DIR"), "/gold_respec_prices.json"));
        serde_json::from_str(text).expect("gold_respec_prices.json")
    });
    if level.fract() != 0.0 || level < 1.0 {
        return None;
    }
    prices.get(level as usize - 1).copied()
}

const FONT_BIG: f64 = 18.0;
const NOT_SUPPORTED: &str = " ^8(Not supported in PoB yet)";

impl TypedSession<'_> {
    /// `showStatDifferences` of the tree view, as `PassiveTreeView:Load`
    /// read it
    fn tree_stat_differences(&self) -> bool {
        self.root
            .child("PathOfBuilding")
            .and_then(|p| p.child("TreeView"))
            .and_then(|t| t.attr("showStatDifferences"))
            .is_none_or(|v| v == "true")
    }

    /// The parse of a node's stat lines (`node.mods`)
    fn node_mods(&self, i: usize) -> &[StatLine] {
        let spec = &self.build.spec;
        match &spec.nodes[i].stats {
            Some(s) => &s.mods,
            None => &spec.tree_node(i).mods,
        }
    }

    /// `addModInfoToTooltip(node, i, line)`, from 0
    fn add_mod_info(&self, tip: &mut Tooltip, i: usize, k: usize, line: &str) {
        let Some(m) = self.node_mods(i).get(k) else {
            return;
        };
        if line != " " && (m.extra.is_some() || m.list.is_none()) {
            let line = format!("{}{line}{NOT_SUPPORTED}", cc("UNSUPPORTED"));
            tip.add_line(FONT_BIG, &line);
        } else {
            tip.add_line(FONT_BIG, &format!("{}{line}", cc("MAGIC")));
        }
    }

    /// `PassiveTreeView:AddNodeName`
    fn add_node_name(&self, tip: &mut Tooltip, i: usize) -> Result<()> {
        let spec = &self.build.spec;
        let node = &spec.nodes[i];
        let kind = spec.kind(i);
        // Every node has a header, so separators have an image
        tip.header = true;
        tip.add_line(24.0, &format!("^xF8E6CA{}", spec.dn(i)));
        if kind == NodeType::Socket
            && let Some(radius) = &spec.tree_node(i).nodes_in_radius
        {
            let sb = self.sidebar()?;
            let mut totals: Option<[f64; 3]> = None;
            for id in radius.get(1).into_iter().flatten() {
                // `CalcsTab:BuildOutput` sets each node's `finalModList` to
                // its `modList` before the calculation sets the ones it builds
                let a = match sb.node_attributes.get(id) {
                    Some(a) => *a,
                    None => {
                        let k = *spec
                            .index
                            .get(id)
                            .ok_or_else(|| format!("no node {id} in a socket's radius"))?;
                        let list = match &spec.nodes[k].stats {
                            Some(s) => &s.mod_list,
                            None => &spec.tree_node(k).mod_list,
                        };
                        let mut a = [0.0; 3];
                        for (k, name) in ["Str", "Dex", "Int"].iter().enumerate() {
                            a[k] =
                                engine::mods::list::sum(list, engine::mods::ModType::Base, name)?;
                        }
                        a
                    }
                };
                let t = totals.get_or_insert([0.0; 3]);
                for k in 0..3 {
                    t[k] += a[k];
                }
            }
            let t =
                totals.ok_or_else(|| format!("{NOT_PORTED}: a socket without nodes in radius"))?;
            tip.add_separator(14.0);
            for (k, (code, name)) in [
                ("STRENGTH", "Strength"),
                ("DEXTERITY", "Dexterity"),
                ("INTELLIGENCE", "Intelligence"),
            ]
            .iter()
            .enumerate()
            {
                if t[k] >= 40.0 {
                    let line = format!("^7Can support {}{name} ^7threshold jewels", cc(code));
                    tip.add_line(FONT_BIG, &line);
                }
            }
        }
        if kind == NodeType::Socket
            && node.alloc
            && let Some(d) = node.distance_to_class_start.filter(|d| *d > 0.0)
        {
            tip.add_separator(14.0);
            tip.add_line(16.0, &engine::sf!("^7Distance to start: %d", d));
        }
        Ok(())
    }

    /// `nodeTooltip`: `PassiveTreeView:AddNodeTooltip`
    pub fn node_tooltip(&mut self, id: u32) -> Result<J> {
        // A socketed jewel's comparison updates the sockets' labels
        if self.build.spec.jewels.contains_key(&id) {
            self.update_sockets();
        }
        self.node_tooltip_lines(id)
    }

    fn node_tooltip_lines(&self, id: u32) -> Result<J> {
        let spec = &self.build.spec;
        let i = self.node_or_error(id)?;
        let node = &spec.nodes[i];
        let kind = spec.kind(i);
        let mut tip = Tooltip::default();
        let tip_line = |tip: &mut Tooltip, text: &str| {
            tip.add_line(14.0, &format!("{}{text}", cc("TIP")));
        };
        if kind == NodeType::Socket && node.alloc {
            match spec.jewels.get(&id).and_then(|j| self.build.items.item(*j)) {
                Some(jewel) => self.add_item_tooltip(&mut tip, jewel, self.socket_slot(id))?,
                None => self.add_node_name(&mut tip, i)?,
            }
            if let Some(d) = node.distance_to_class_start.filter(|d| *d > 0.0) {
                tip.add_separator(14.0);
                tip.add_line(16.0, &engine::sf!("^7Distance to start: %d", d));
            }
            tip.add_separator(14.0);
            if self.socket_enabled(id) {
                tip_line(
                    &mut tip,
                    "Tip: Right click this socket to go to the items page and choose the jewel for this socket.",
                );
            }
            tip_line(&mut tip, "Tip: Hold Shift or Ctrl to hide this tooltip.");
            return Ok(tip.json());
        }
        self.add_node_name(&mut tip, i)?;
        let sd = spec.sd(i);
        let tn = spec.tree_node(i);
        let all_options = node.all_mastery_options == Some(true);
        if !sd.is_empty() && all_options {
            tip.add_separator(14.0);
            tip.add_line(14.0, "^7Available Mastery node options are:");
            tip.add_line(6.0, "");
            let mut count = 0;
            for (n, &effect) in tn.mastery_effects.iter().enumerate() {
                let existing = spec.mastery_selections.iter().any(|(_, v)| *v == effect);
                let data = (spec.tree.mastery_effects)
                    .get(&effect.to_bits())
                    .ok_or("no mastery effect")?;
                for line in &data.sd {
                    if !existing {
                        self.add_mod_info(&mut tip, i, count, line);
                    }
                    count += 1;
                }
                if !existing && n + 1 < tn.mastery_effects.len() {
                    tip.add_line(6.0, "");
                }
            }
            tip.add_separator(14.0);
        }
        if !sd.is_empty() && !all_options {
            tip.add_line(16.0, "");
            for (k, line) in sd.iter().enumerate() {
                self.add_mod_info(&mut tip, i, k, line);
            }
        }
        // Reminder text: allocated masteries have their own
        let mastery_reminder = ["Tip: Right click to select a different effect".into()];
        let reminder = if kind == NodeType::Mastery {
            (node.alloc && spec.mastery_selections.get(id).is_some())
                .then_some(&mastery_reminder[..])
        } else if let Some(o) = &node.overlay {
            // `ReplaceNode` leaves the replacement's reminder text, else
            // empties the node's. `BuildAllDependsAndPaths` puts the tree
            // node back before each tattoo: without a reminder of its own,
            // that empties the tattoo's table, which the tattoo then brings
            if o.is_tattoo && tn.reminder_text.is_none() {
                None
            } else {
                o.reminder_text.as_deref()
            }
        } else {
            tn.reminder_text.as_deref()
        };
        if let Some(r) = reminder {
            tip.add_separator(14.0);
            for line in r {
                tip.add_line(14.0, &format!("^xA0A080{line}"));
            }
        }
        if let Some(f) = &tn.flavour_text {
            tip.add_separator(14.0);
            for line in f {
                tip.add_line(FONT_BIG, &format!("{}{line}", cc("UNIQUE")));
            }
        }
        let dn = spec.dn(i);
        let tattoo = spec.is_tattoo(i)
            || (kind == NodeType::Normal
                && ["Strength", "Dexterity", "Intelligence"].contains(&dn))
            || (kind == NodeType::Notable
                && sd.first().is_some_and(|l| {
                    ["+30 to Dexterity", "+30 to Strength", "+30 to Intelligence"]
                        .iter()
                        .any(|p| engine::pat::is_match(l, p))
                }))
            || kind == NodeType::Keystone
            || kind == NodeType::Mastery;
        if tattoo && node.synth.is_none() {
            tip.add_separator(14.0);
            let what = if kind == NodeType::Mastery {
                "runegraft"
            } else {
                "tattoo"
            };
            tip_line(
                &mut tip,
                &format!("Tip: Right click to edit the {what} for this node"),
            );
        }
        if self.tree_stat_differences() {
            self.add_node_stat_differences(&mut tip, i)?;
            tip_line(
                &mut tip,
                "Tip: Press Ctrl+D to disable the display of stat differences.",
            );
        } else {
            tip.add_separator(14.0);
            tip_line(
                &mut tip,
                "Tip: Press Ctrl+D to enable the display of stat differences.",
            );
        }
        // Pathing distance
        tip.add_separator(14.0);
        let path = spec.path(i);
        if let Some(path) = path.as_ref().filter(|p| !p.is_empty()) {
            let leap = if node.intuitive.is_empty() {
                ""
            } else {
                " ^8(Can be allocated without pathing to it)"
            };
            let line = format!(
                "^7{} points to node{leap}",
                super::tooltip::n(node.path_dist)
            );
            tip.add_line(14.0, &line);
            tip.add_line(14.0, cc("TIP"));
            if path.len() > 1 {
                tip.add_line(
                    14.0,
                    "Tip: To reach this node by a different path, hold Shift, then trace the path and click this node",
                );
            }
        }
        let gold = || -> Result<f64> {
            let g = gold_respec_price(self.build.info.character_level)
                .ok_or_else(|| format!("{NOT_PORTED}: no gold price for the level"))?;
            Ok(if tn.ascendancy_name.is_some() {
                g * 5.0
            } else {
                g
            })
        };
        let depends = node.depends.len() as f64;
        let gold_line = |g: f64, what: &str| {
            let cost = engine::pat::format_num_sep(&super::tooltip::n(depends * g));
            format!("^xFFD700{cost} Gold ^7required to unallocate {what}")
        };
        if depends > 1.0 {
            let g = gold()?;
            tip.add_separator(14.0);
            let line = format!(
                "^7{} points gained from unallocating these nodes",
                node.depends.len()
            );
            tip.add_line(14.0, &line);
            tip.add_line(14.0, &gold_line(g, "these nodes"));
            tip.add_line(14.0, cc("TIP"));
        } else if node.alloc {
            let g = gold()?;
            tip.add_line(14.0, &gold_line(g, "this node"));
            tip.add_line(14.0, cc("TIP"));
        }
        if kind == NodeType::Socket {
            tip_line(&mut tip, "Tip: Hold Shift or Ctrl to hide this tooltip.");
        } else {
            tip_line(&mut tip, "Tip: Hold Ctrl to hide this tooltip.");
            tip_line(&mut tip, "Tip: Press Ctrl+C to copy this node's text.");
        }
        Ok(tip.json())
    }

    /// The stat differences of allocating or unallocating a node, and the
    /// nodes on its path or depending on it
    fn add_node_stat_differences(&self, tip: &mut Tooltip, i: usize) -> Result<()> {
        let spec = &self.build.spec;
        let node = &spec.nodes[i];
        let kind = spec.kind(i);
        let data = self.data.calc_data();
        let calc = engine::calculator::MiscCalculator::new(&self.build, &data)?;
        tip.add_separator(14.0);
        let path: Vec<usize> = if node.alloc {
            node.depends.clone()
        } else {
            spec.path(i).unwrap_or_default()
        };
        let path_len = path.len();
        let granted = self.sidebar()?.granted_passives.contains(&node.id);
        // An allocated mastery with a popup open (the mastery popup's
        // options, upstream) compares with the node added again
        let realloc = node.alloc && kind == NodeType::Mastery && self.ui.popups != J::Array(vec![]);
        // Masteries link to no node, so none depends on one
        if realloc && path_len > 1 {
            return Err(format!("mastery {} with dependent nodes", node.id));
        }
        let mut path_out = None;
        let node_out = if realloc {
            calc.calculate(&Override::default().add_node(i), true)?
        } else if node.alloc {
            if path_len > 1 {
                let ovr = Override {
                    remove_nodes: path.clone(),
                    ..Override::default()
                };
                path_out = Some(calc.calculate(&ovr, true)?);
            }
            calc.calculate(&Override::default().remove_node(i), true)?
        } else if granted {
            let ovr = Override {
                remove_granted: vec![node.id],
                ..Override::default()
            };
            calc.calculate(&ovr, true)?
        } else {
            let options = kind == NodeType::Mastery && node.all_mastery_options == Some(true);
            if path_len > 1 {
                let nodes = path.iter().filter(|&&n| !(options && n == i));
                let ovr = Override {
                    add_nodes: nodes.map(|&n| OverrideNode::Spec(n)).collect(),
                    ..Override::default()
                };
                path_out = Some(calc.calculate(&ovr, true)?);
            }
            if options {
                calc.calculate(&Override::default(), true)?
            } else {
                calc.calculate(&Override::default().add_node(i), true)?
            }
        };
        let base = calc.base();
        let outputs = |o: &engine::calculator::CalcOutput| -> (engine::calcs::Output, Option<engine::calcs::Output>) {
            (o.player.clone(), o.minion.clone())
        };
        let (bp, bm) = outputs(base);
        let base = Outputs {
            player: &bp,
            minion: bm.as_ref(),
        };
        let (np, nm) = outputs(&node_out);
        let header = if realloc {
            "^7Reallocating this node will give you:"
        } else if node.alloc {
            "^7Unallocating this node will give you:"
        } else if granted {
            "^7This node is granted by an item. Removing it will give you:"
        } else {
            "^7Allocating this node will give you:"
        };
        let compare = Outputs {
            player: &np,
            minion: nm.as_ref(),
        };
        let mut count = self.add_stat_compares(tip, &base, &compare, header, None)?;
        if path_len > 1 && !granted && (node.intuitive.is_empty() || node.alloc) {
            let p = path_out.as_ref().ok_or("no path calculation")?;
            let (pp, pm) = outputs(p);
            let compare = Outputs {
                player: &pp,
                minion: pm.as_ref(),
            };
            let header = if node.alloc {
                "^7Unallocating this node and all nodes depending on it will give you:"
            } else {
                "^7Allocating this node and all nodes leading to it will give you:"
            };
            count += self.add_stat_compares(tip, &base, &compare, header, Some(path_len as f64))?;
        }
        if count == 0 {
            if granted {
                tip.add_line(
                    14.0,
                    "^7This node is granted by an item. Removing it will cause no changes",
                );
            } else {
                let what = if node.alloc {
                    "unallocating"
                } else {
                    "allocating"
                };
                tip.add_line(14.0, &format!("^7No changes from {what} this node."));
            }
        }
        Ok(())
    }
}
