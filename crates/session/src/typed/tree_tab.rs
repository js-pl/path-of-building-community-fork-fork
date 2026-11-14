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

use super::{NOT_PORTED, Result, TypedSession};
use serde_json::{Value as J, json};

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
}

impl TreeTab {
    /// The tab as a build's `<TreeView>` loads it
    pub fn load(root: &engine::xml::Element) -> TreeTab {
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
