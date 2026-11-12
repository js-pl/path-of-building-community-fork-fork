//! The tree tab's edits and views (`clickNode`, `selectMastery`,
//! `resetTree`, `treeState`, `nodePath`, `masteryOptions`, `treeSpecs`,
//! `powerStats` of `lua/session.lua`) on the typed spec
//! (`PassiveSpec:AllocNode`, `DeallocNode`, ...).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedSession, num, object};
use engine::spec::PassiveSpec;
use engine::tree::NodeType;
use serde_json::{Map, Value as J, json};
use std::rc::Rc;

impl<'d> TypedSession<'d> {
    fn spec_mut(&mut self) -> &mut PassiveSpec<'d> {
        Rc::make_mut(&mut self.build.spec)
    }

    /// The spec's index of a node id
    fn node(&self, id: u32) -> Result<usize> {
        self.build
            .spec
            .index
            .get(&id)
            .copied()
            .ok_or_else(|| format!("no node {id}"))
    }

    /// What a frame does after `buildFlag` is set: both calculations again
    pub(super) fn rebuild(&mut self) -> Result<()> {
        let data = self.data.calc_data();
        self.main = engine::calcs::build_output(
            &mut self.build,
            &data,
            engine::calc_env::CalcMode::Main,
            None,
        )?;
        self.calcs = None;
        self.ui.output_revision += 1.0;
        self.ui.unsaved = true;
        self.refresh_controls()
    }

    /// Edits of the tree are not ported where jewels change its paths
    fn check_tree(&self) -> Result<()> {
        engine::power::check_tree_jewels(&self.build)
            .map_err(|_| format!("{NOT_PORTED}: tree edits with Intuitive Leap and the like"))
    }

    fn alloc(&mut self, i: usize) -> Result<()> {
        let b = &mut self.build;
        Rc::make_mut(&mut b.spec).alloc_node(i, &b.items, &self.data.game.items)
    }

    /// Whether the node belongs to a bloodline (an alternate ascendancy)
    fn is_bloodline(&self, asc: &str) -> bool {
        self.build
            .spec
            .tree
            .alternate_ascendancies
            .iter()
            .flatten()
            .any(|(_, a)| a.id.as_deref() == Some(asc))
    }

    fn secondary_id(&self) -> Option<Rc<str>> {
        let spec = &self.build.spec;
        let id = spec.cur_secondary_ascend_class_id?;
        spec.tree
            .alternate_ascendancies
            .as_ref()?
            .iter()
            .find(|(i, _)| *i == id)
            .and_then(|(_, a)| a.id.clone())
    }

    /// `clickNode`: a left click on a node in the tree view
    pub fn click_node(&mut self, id: u32) -> Result<J> {
        self.check_tree()?;
        let i = self.node(id)?;
        let spec = &self.build.spec;
        if spec.nodes[i].alloc {
            let b = &mut self.build;
            Rc::make_mut(&mut b.spec).dealloc_node(i, &b.items, &self.data.game.items)?;
            self.rebuild()?;
            return Ok(json!([]));
        }
        let tn = spec.tree_node(i);
        let mut changed = false;
        if let Some(asc) = tn.ascendancy_name.map(|a| a.as_str().to_string()) {
            if !self.is_bloodline(&asc) {
                let base = spec.ascend_class_base_name();
                if (spec.cur_ascend_class_id == Some(0) || base.as_deref() != Some(&*asc))
                    && self.secondary_id().as_deref() != Some(&*asc)
                    && let Some((aid, _)) = spec
                        .ascend_classes()
                        .into_iter()
                        .find(|(_, a)| a.id.as_deref() == Some(&*asc))
                {
                    self.spec_mut().select_ascend_class(aid)?;
                    changed = true;
                }
            } else if self.secondary_id().as_deref() != Some(&*asc)
                && let Some(aid) = spec
                    .tree
                    .alternate_ascendancies
                    .iter()
                    .flatten()
                    .find(|(_, a)| a.id.as_deref() == Some(&*asc))
                    .map(|(i, _)| *i)
            {
                self.spec_mut().select_secondary_ascend_class(aid)?;
                changed = true;
            }
        }
        let spec = &self.build.spec;
        let tn = spec.tree_node(i);
        if spec.nodes[i].path.is_some() && !spec.nodes[i].alloc {
            if tn.kind == NodeType::Mastery && !tn.mastery_effects.is_empty() {
                let options = self.mastery_options(id)?;
                if changed {
                    self.rebuild()?;
                }
                return Ok(json!({ "mastery": options }));
            }
            self.alloc(i)?;
            changed = true;
        }
        if changed {
            self.rebuild()?;
        }
        Ok(json!([]))
    }

    /// `masteryOptions`: the effects a mastery may take
    /// (`TreeTab:OpenMasteryPopup`)
    pub fn mastery_options(&self, id: u32) -> Result<J> {
        let i = self.node(id)?;
        let spec = &self.build.spec;
        let mut out = Vec::new();
        for &e in &spec.tree_node(i).mastery_effects {
            let assigned = spec
                .mastery_selections
                .iter()
                .find(|(_, v)| **v == e)
                .map(|(k, _)| k);
            if assigned.is_none_or(|a| a == id) {
                let effect = spec
                    .tree
                    .mastery_effects
                    .get(&e.to_bits())
                    .ok_or("no mastery effect")?;
                let label: Vec<&str> = effect.sd.iter().map(|s| &**s).collect();
                out.push(json!({ "label": label.join(" / "), "id": num(e) }));
            }
        }
        Ok(J::Array(out))
    }

    /// `selectMastery`: `TreeTab:SaveMasteryPopup`
    pub fn select_mastery(&mut self, id: u32, effect: f64) -> Result<J> {
        self.check_tree()?;
        let i = self.node(id)?;
        if !self
            .build
            .spec
            .tree
            .mastery_effects
            .contains_key(&effect.to_bits())
        {
            return Err(format!("no mastery effect {effect} on {id}"));
        }
        let spec = self.spec_mut();
        spec.mastery_selections.insert(id, effect);
        if spec.nodes[i].alloc {
            spec.build_all_depends_and_paths()?;
        } else {
            self.alloc(i)?;
        }
        self.rebuild()?;
        Ok(json!([]))
    }

    /// `resetTree`
    pub fn reset_tree(&mut self) -> Result<J> {
        self.check_tree()?;
        let spec = self.spec_mut();
        spec.reset_nodes();
        spec.build_all_depends_and_paths()?;
        self.rebuild()?;
        Ok(json!([]))
    }

    /// `treeState`: allocation, granted passives and masteries
    pub fn tree_state(&self) -> Result<J> {
        let spec = &self.build.spec;
        let mut alloc: Vec<u32> = spec.alloc_nodes.iter().map(|(k, _)| k).collect();
        alloc.sort_unstable();
        let granted: Vec<u32> = self
            .main
            .sidebar
            .as_ref()
            .map(|s| s.granted_passives.iter().copied().collect())
            .unwrap_or_default();
        let masteries: Map<String, J> = spec
            .mastery_selections
            .iter()
            .map(|(k, v)| (k.to_string(), num(*v)))
            .collect();
        let mut o = Map::new();
        o.insert("alloc".into(), json!(alloc));
        o.insert("granted".into(), json!(granted));
        o.insert("masteries".into(), object(masteries));
        if let Some(c) = spec.cur_class_id {
            o.insert("classId".into(), num(c as f64));
        }
        if let Some(c) = spec.cur_ascend_class_id {
            o.insert("ascendClassId".into(), num(c as f64));
        }
        if let Some(a) = spec.ascend_class_base_name() {
            o.insert("ascendancy".into(), json!(&*a));
        }
        if let Some(a) = self.secondary_id() {
            o.insert("secondaryAscendancy".into(), json!(&*a));
        }
        Ok(J::Object(o))
    }

    /// `nodePath`: the nodes allocating (or deallocating) a node changes
    pub fn node_path(&self, id: u32) -> Result<J> {
        let i = self.node(id)?;
        let spec = &self.build.spec;
        let path = if spec.nodes[i].alloc {
            spec.nodes[i].depends.clone()
        } else {
            spec.path(i).unwrap_or_default()
        };
        let ids: Vec<u32> = path.iter().map(|&n| spec.nodes[n].id).collect();
        Ok(json!({ "path": ids }))
    }

    /// `treeSpecs`: the tree tab's specs
    pub fn tree_specs(&self) -> J {
        json!({ "active": num(self.specs.0 as f64), "list": self.specs.1 })
    }

    /// `powerStats`: the tree tab's power stats
    pub fn power_stats() -> J {
        let list: Vec<J> = engine::power::power_stats()
            .into_iter()
            .filter(|s| !s.ignore_for_nodes)
            .map(|s| {
                let mut o = Map::new();
                if let Some(st) = s.stat {
                    o.insert("stat".into(), json!(st));
                }
                o.insert("label".into(), json!(s.label));
                J::Object(o)
            })
            .collect();
        J::Array(list)
    }
}

/// The tree tab's specs of a build file: the active one, and each one's
/// title (`TreeTab:Load`, `SetActiveSpec`)
pub fn spec_titles(root: &engine::xml::Element) -> (usize, Vec<String>) {
    let tree = root.child("PathOfBuilding").and_then(|p| p.child("Tree"));
    let titles: Vec<String> = tree
        .map(|t| {
            t.elements()
                .filter(|e| e.elem == "Spec")
                .map(|e| e.attr("title").unwrap_or("Default").to_string())
                .collect()
        })
        .unwrap_or_default();
    let titles = if titles.is_empty() {
        vec!["Default".to_string()]
    } else {
        titles
    };
    let active = tree
        .and_then(|t| t.attr("activeSpec"))
        .and_then(engine::pat::to_number)
        .unwrap_or(1.0) as usize;
    (active.min(titles.len()), titles)
}
