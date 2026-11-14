//! The tree's undo (`undo` of `lua/session.lua`, which undoes on the active
//! spec): `UndoHandler` with `PassiveSpec:CreateUndoState` and
//! `RestoreUndoState`, and `newBuild`.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedData, TypedSession};
use serde_json::{Value as J, json};

/// `PassiveSpec:CreateUndoState()`; `secondaryAscendClassId` reads a field
/// the spec never sets, so restoring a state takes the bloodline off
#[derive(Clone, Debug)]
pub(super) struct UndoState {
    class: Option<i64>,
    ascend: Option<i64>,
    /// `allocNodes`' ids in `pairs` order
    hash_list: Vec<u32>,
    /// `masterySelections` in `pairs` order
    masteries: Vec<(u32, f64)>,
}

impl TypedSession<'_> {
    fn undo_state(&self) -> UndoState {
        Self::state_of(&self.build.spec)
    }

    fn state_of(spec: &engine::spec::PassiveSpec) -> UndoState {
        UndoState {
            class: spec.cur_class_id,
            ascend: spec.cur_ascend_class_id,
            hash_list: spec.alloc_nodes.iter().map(|(k, _)| k).collect(),
            masteries: spec
                .mastery_selections
                .iter()
                .map(|(k, v)| (k, *v))
                .collect(),
        }
    }

    /// `spec:ResetUndo()` of each spec `PassiveSpec:Load` loaded
    pub(super) fn reset_undo(&mut self) {
        let has_specs = self
            .root
            .child("PathOfBuilding")
            .and_then(|p| p.child("Tree"))
            .is_some_and(|t| t.elements().any(|e| e.elem == "Spec"));
        if !has_specs {
            return;
        }
        let s = self.undo_state();
        self.undo.insert(self.specs.0, vec![s]);
        for (i, spec) in self.other_specs.iter().enumerate() {
            if let Some(spec) = spec {
                self.undo.insert(i + 1, vec![Self::state_of(spec)]);
            }
        }
    }

    /// `spec:AddUndoState()`
    pub(super) fn add_undo_state(&mut self) {
        let s = self.undo_state();
        let list = self.undo.entry(self.specs.0).or_default();
        list.insert(0, s);
        list.truncate(101);
    }

    /// `undo`: `spec:Undo()` when there is a state to go back to
    pub fn undo(&mut self) -> Result<J> {
        let list = self.undo.entry(self.specs.0).or_default();
        if list.len() < 2 {
            return Ok(json!([]));
        }
        let state = list[1].clone();
        // The session cannot change tattoos, so the state's are the
        // spec's; the subgraphs stay as they are (`RestoreUndoState`
        // does not rebuild them): ids they no longer have wait in
        // `allocSubgraphNodes`
        let (class, ascend) = match (state.class, state.ascend) {
            (Some(c), Some(a)) => (c, a),
            _ => return Err(format!("{NOT_PORTED}: undo to a spec without a class")),
        };
        let list = self.undo.get_mut(&self.specs.0).unwrap();
        list.drain(..2);
        let b = &mut self.build;
        let spec = std::sync::Arc::make_mut(&mut b.spec);
        spec.import_from_node_list(class, ascend, None, &state.hash_list, &state.masteries)?;
        self.add_undo_state();
        self.rebuild()?;
        Ok(json!([]))
    }

    /// `newBuild`: build mode opened without a file, as a new build
    pub fn new_build(data: &TypedData) -> Result<TypedSession<'_>> {
        let latest = data.game.latest_tree_version();
        let xml = format!(
            "<PathOfBuilding><Build level=\"1\" targetVersion=\"3_0\" bandit=\"None\" \
             pantheonMajorGod=\"None\" pantheonMinorGod=\"None\" mainSocketGroup=\"1\" \
             viewMode=\"TREE\" characterLevelAutoMode=\"true\"/>\
             <Skills activeSkillSet=\"1\" defaultGemLevel=\"normalMaximum\" \
             sortGemsByDPS=\"true\" sortGemsByDPSField=\"CombinedDPS\" \
             showSupportGemTypes=\"ALL\" showLegacyGems=\"false\"><SkillSet id=\"1\"/></Skills>\
             <Tree activeSpec=\"1\"><Spec treeVersion=\"{latest}\" classId=\"0\" \
             ascendClassId=\"0\" nodes=\"\"/></Tree></PathOfBuilding>"
        );
        let mut s = TypedSession::load(data, &xml, Some("Unnamed build"))?;
        // A new spec has no undo state yet, nor a bloodline id
        s.undo.clear();
        std::sync::Arc::make_mut(&mut s.build.spec).cur_secondary_ascend_class_id = None;
        s.ui.unsaved = false;
        s.gem_quality_nil = true;
        // Its skills tab never loaded: the quality box is empty
        s.gem_options = super::skills_controls::GemOptions::load(None, None);
        Ok(s)
    }
}
