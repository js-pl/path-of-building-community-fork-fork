//! The Tree tab's controls (`tree.*`) that open no popup, as
//! `Classes/TreeTab.lua` handles them on a tab never drawn: the search
//! (saved as the tree view's `searchStr`; a search other than the saved
//! one leaves the build unsaved), the heat map and its node power depth
//! (which `nodePower` searches within), the power report's toggle and the
//! comparison's checkbox. The spec, compare and heat map stat drop-downs
//! get their lists only when the tab draws: headless they are empty, and
//! selecting in them changes nothing.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::ItemPopup;
use super::{NOT_PORTED, Result, TypedSession};
use engine::spec::PassiveSpec;
use serde_json::{Value as J, json};
use std::sync::Arc;

/// `nodePowerMaxDepthSelect`'s list
const DEPTHS: [Option<f64>; 5] = [None, Some(5.0), Some(10.0), Some(15.0), None];

/// What the tab holds besides the specs
#[derive(Clone, Debug, Default)]
pub(super) struct TreeTab {
    /// `viewer.searchStr`, `searchStrSaved` and `searchFlag`
    pub search: String,
    pub search_saved: String,
    pub search_flag: bool,
    /// `viewer.showHeatMap`
    pub heat_map: bool,
    /// `nodePowerMaxDepthSelect.selIndex`, 1-based
    pub depth_sel: usize,
    /// `calcsTab.nodePowerMaxDepth`
    pub max_depth: Option<f64>,
    /// `powerReportList.shown`
    pub power_report: bool,
    /// `versionSelect.selIndex`, 1-based (the latest version at first)
    pub version_sel: usize,
}

impl TreeTab {
    /// The tab as a build's `<TreeView>` loads it
    pub fn load(root: &engine::xml::Element, versions: usize) -> TreeTab {
        let search = root
            .child("PathOfBuilding")
            .and_then(|p| p.child("TreeView"))
            .and_then(|v| v.attr("searchStr"))
            .unwrap_or("")
            .to_string();
        TreeTab {
            search_saved: search.clone(),
            search,
            depth_sel: 1,
            version_sel: versions,
            ..TreeTab::default()
        }
    }
}

impl TypedSession<'_> {
    /// A `tree.<name>` control changed as `api.control` changes it
    pub(super) fn tree_control(&mut self, name: &str, req: &J) -> Result<J> {
        let sel = req.get("sel").and_then(J::as_f64);
        let text = req.get("text").and_then(J::as_str);
        let state = req.get("state").and_then(J::as_bool);
        let mut power_calc = false;
        let t = &mut self.tree_tab;
        match (name, sel, text, state) {
            ("treeSearch", None, Some(s), None) => {
                t.search = s.to_string();
                t.search_flag = t.search != t.search_saved;
            }
            // Drop-downs whose lists the tab sets when it draws
            ("specSelect" | "compareSelect" | "treeHeatMapStatSelect", Some(_), None, None) => {}
            // `isComparing` and `SetCompareSpec`, which only the tab's
            // drawing reads
            ("compareCheck", None, None, Some(_)) => {}
            ("treeHeatMap", None, None, Some(s)) => {
                t.heat_map = s;
                if !s {
                    t.power_report = false;
                }
            }
            ("nodePowerMaxDepthSelect", Some(s), None, None) => {
                // `DropDownControl:SetSel`: clamped, and only a change
                // calls the handler
                let s = s.min(DEPTHS.len() as f64).max(1.0) as usize;
                if s != t.depth_sel {
                    t.depth_sel = s;
                    if s != DEPTHS.len() {
                        let old = t.max_depth;
                        t.max_depth = DEPTHS[s - 1];
                        // `oldMax ~= value`: the old depth against the
                        // entry, a number or "All"
                        let deeper =
                            old.is_some() && t.max_depth.is_none_or(|d| old.is_some_and(|o| d > o));
                        power_calc = t.heat_map && old != DEPTHS[s - 1] && deeper;
                    }
                }
            }
            ("nodePowerMaxDepthCustom", None, Some(s), None) => {
                t.max_depth = engine::pat::to_number(s);
                power_calc = t.heat_map;
            }
            ("powerReport", None, None, None) if req["click"].as_bool() == Some(true) => {
                t.power_report = !t.power_report;
            }
            ("versionSelect", Some(s), None, None) => {
                let n = self.data.game.tree_versions.len();
                let s = s.min(n as f64).max(1.0) as usize;
                if s != self.tree_tab.version_sel {
                    self.tree_tab.version_sel = s;
                    let v = self.data.game.tree_versions[s - 1].clone();
                    if *v != *self.build.spec.tree.version {
                        self.open_tree_popup(ItemPopup::ConvertVersion(v))?;
                    }
                }
            }
            ("reset", None, None, None) if req["click"].as_bool() == Some(true) => {
                self.open_tree_popup(ItemPopup::ResetTree)?;
            }
            ("specConvert", None, None, None) if req["click"].as_bool() == Some(true) => {
                if self.class_popups_open() > 0 {
                    return Err(format!(
                        "{NOT_PORTED}: converting a tree with a class change popup open"
                    ));
                }
                let latest = self.latest_for_spec();
                self.convert_to_version(&latest, false, true, false)?;
            }
            ("specConvertAll", None, None, None) if req["click"].as_bool() == Some(true) => {
                let latest = self.latest_for_spec();
                self.open_tree_popup(ItemPopup::ConvertAll(latest))?;
            }
            _ => return Err(format!("{NOT_PORTED}: control tree.{name}")),
        }
        if power_calc {
            // `SetPowerCalc`: the heat map shown, and a calculation
            self.tree_tab.heat_map = true;
            self.rebuild()?;
        }
        Ok(json!([]))
    }

    /// `controlTooltip` of a `tree.<name>` control: the spec drop-down's
    /// tooltip function has no entry on a tab never drawn; the others
    /// have none
    pub fn tree_tooltip(&self, name: &str) -> Result<J> {
        match name {
            "specSelect"
            | "compareCheck"
            | "compareSelect"
            | "reset"
            | "versionText"
            | "versionSelect"
            | "treeSearch"
            | "findTimelessJewel"
            | "treeHeatMap"
            | "nodePowerMaxDepthSelect"
            | "nodePowerMaxDepthCustom"
            | "treeHeatMapStatSelect"
            | "powerReport"
            | "specConvertText"
            | "specConvert"
            | "specConvertAll" => Ok(json!({ "lines": [] })),
            _ => Err(format!("{NOT_PORTED}: control tree.{name}")),
        }
    }
}

impl TypedSession<'_> {
    /// `treeVersions[v].display`
    pub(super) fn tree_display(&self, v: &str) -> String {
        self.data
            .game
            .tree_version(v)
            .map_or_else(|| v.to_string(), |t| t.display.clone())
    }

    /// `getLatestTreeVersion()`: the latest version, with the active
    /// spec's variant of it (`_ruthless`, ...)
    fn latest_for_spec(&self) -> String {
        let latest = self.data.game.latest_tree_version();
        let suffix = engine::pat::capture(
            &self.build.spec.tree.version,
            &format!("^{}(.*)", engine::pat::escape(latest)),
        )
        .unwrap_or_default();
        format!("{latest}{suffix}")
    }

    /// `main:OpenPopup` of one of the tab's popups, in front
    fn open_tree_popup(&mut self, popup: ItemPopup) -> Result<()> {
        if self.class_popups_open() > 0 {
            return Err(format!(
                "{NOT_PORTED}: a tree popup with a class change popup open"
            ));
        }
        self.editor.popups.insert(0, popup);
        self.refresh_item_popups();
        Ok(())
    }

    /// The version drop-down back on the active spec's version
    /// (`SelByValue`, which keeps the selection for one it does not have)
    pub(super) fn select_current_version(&mut self) {
        let v = &self.build.spec.tree.version;
        if let Some(i) = self
            .data
            .game
            .tree_versions
            .iter()
            .position(|x| x.as_str() == &**v)
        {
            self.tree_tab.version_sel = i + 1;
        }
    }

    /// Remove All Tattoos: on a spec without tattoos, only the tab's
    /// `modFlag` and `buildFlag`
    pub(super) fn remove_all_tattoos(&mut self) -> Result<()> {
        let spec = &self.build.spec;
        if (0..spec.nodes.len()).any(|i| spec.is_tattoo(i)) {
            return Err(format!("{NOT_PORTED}: removing tattoos"));
        }
        self.ui.unsaved = true;
        self.rebuild()
    }

    /// `ConvertToVersion(version, remove, success, ignoreTreeSubType)`
    pub(super) fn convert_to_version(
        &mut self,
        version: &str,
        remove: bool,
        success: bool,
        ignore_sub: bool,
    ) -> Result<()> {
        let version = self.convert_spec(version, remove, ignore_sub)?;
        self.ui.unsaved = true;
        self.rebuild()?;
        if success {
            let msg = format!(
                "The tree has been converted to {}.\nNote that some or all of the passives may have been de-allocated due to changes in the tree.\n\nYou can switch back to the old tree using the tree selector at the bottom left.",
                self.tree_display(&version)
            );
            self.open_tree_popup(ItemPopup::Message {
                title: "Tree Converted".into(),
                lines: msg.split('\n').map(String::from).collect(),
            })?;
        }
        Ok(())
    }

    /// `ConvertAllToVersion(version)`: every spec on another version
    /// converted in its place, then the active one again
    pub(super) fn convert_all_to_version(&mut self, version: &str) -> Result<()> {
        let current = self.specs.0;
        let versions: Vec<String> = self
            .spec_list()
            .iter()
            .map(|s| s.tree.version.to_string())
            .collect();
        for (i, v) in versions.iter().enumerate() {
            if v != version {
                self.switch_spec(i + 1)?;
                self.convert_spec(version, true, false)?;
            }
        }
        self.switch_spec(current)?;
        self.ui.unsaved = true;
        self.rebuild()
    }

    /// The new spec of `ConvertToVersion`, inserted after the active one
    /// and made active (the old one removed with `remove`), without the
    /// calculation; its version
    fn convert_spec(&mut self, version: &str, remove: bool, ignore_sub: bool) -> Result<String> {
        let mut version = version.to_string();
        let cur = self.build.spec.tree.version.to_string();
        if !ignore_sub
            && let Some(sub) = engine::pat::capture(&cur, "(_%l+_?%l*)")
            && !engine::pat::is_match(&version, &sub)
        {
            let v = format!("{version}{sub}");
            if self.data.game.tree_versions.contains(&v) {
                version = v;
            }
        }
        // `RestoreUndoState(CreateUndoState(), version)`: no bloodline
        let src = &self.build.spec;
        let (Some(class), Some(ascend)) = (src.cur_class_id, src.cur_ascend_class_id) else {
            return Err(format!("{NOT_PORTED}: converting a tree without a class"));
        };
        let hash_list: Vec<u32> = src.alloc_nodes.iter().map(|(k, _)| k).collect();
        let masteries: Vec<(u32, f64)> = src
            .mastery_selections
            .iter()
            .map(|(k, v)| (k, *v))
            .collect();
        let (title, jewels, overrides) = (
            src.title.clone(),
            src.jewels.clone(),
            src.hash_overrides.clone(),
        );
        if !overrides.is_empty() {
            return Err(format!(
                "{NOT_PORTED}: converting a tree with overridden nodes"
            ));
        }
        let tree = self.data.tree(&version)?;
        let mut spec = PassiveSpec::new(tree, &self.data.parser)?;
        spec.title = title;
        spec.jewels = jewels;
        spec.import_from_node_list(class, ascend, None, &hash_list, &masteries)?;
        spec.build_cluster_jewel_graphs(&self.build.items, &self.data.game.items)?;
        let active = self.specs.0;
        if self.other_specs.is_empty() {
            // The list held the active spec alone
            self.other_specs.push(None);
        }
        let label = self.specs.1.get(active - 1).cloned().unwrap_or_default();
        self.other_specs.insert(active, Some(Arc::new(spec)));
        self.specs.1.insert(active, label);
        // The undo states of the specs after the active one move down
        let undo = std::mem::take(&mut self.undo);
        self.undo = undo
            .into_iter()
            .map(|(k, v)| (if k > active { k + 1 } else { k }, v))
            .collect();
        if remove {
            // `t_remove(specList, activeSpec)`, then `SetActiveSpec` of the
            // same index: the new spec
            self.other_specs.remove(active - 1);
            self.specs.1.remove(active - 1);
            let undo = std::mem::take(&mut self.undo);
            self.undo = undo
                .into_iter()
                .filter(|(k, _)| *k != active)
                .map(|(k, v)| (if k > active { k - 1 } else { k }, v))
                .collect();
            let new = self.other_specs[active - 1].take().ok_or("no spec")?;
            self.build.spec = new;
            self.spec_activated()?;
        } else {
            self.switch_spec(active + 1)?;
        }
        Ok(version)
    }
}
