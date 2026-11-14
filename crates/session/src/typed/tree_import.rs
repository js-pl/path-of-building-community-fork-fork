//! Manage Passive Trees' Import Tree and Export Tree buttons
//! (`TreeTab:OpenImportPopup`, `TreeTab:OpenExportPopup`; the popup's
//! unnamed buttons `popup.2`, `popup.3` and Done, `popup.4`): a tree link
//! of the official tree's format (or poeskilltree's) decoded into a new
//! spec that becomes the active one, and the active spec's link. Links
//! that need the network (PoEURL) or poeplanner's format are not ported.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::ItemPopup;
use super::{NOT_PORTED, Result, TypedSession};
use engine::pat;
use engine::spec::PassiveSpec;
use serde_json::{Value as J, json};
use std::sync::Arc;

/// The Import Tree popup's inputs
#[derive(Clone, Debug, Default)]
pub struct ImportTree {
    /// `controls.name.buf`, `controls.edit.buf`
    name: String,
    edit: String,
    /// `controls.msg.label`, which the popup's description leaves out
    msg: String,
    /// `controls.import.enabled`: false until an input changes, then
    /// `buf:match("%S") and ...`, a string or nil, never false
    import_enabled: bool,
}

/// The Export Tree popup's link box
#[derive(Clone, Debug, Default)]
pub struct ExportTree {
    edit: String,
}

impl<'d> TypedSession<'d> {
    /// Import Tree (`popup.2`)
    pub(super) fn open_import_tree(&mut self) {
        self.editor
            .popups
            .insert(0, ItemPopup::ImportTree(Box::default()));
        self.refresh_item_popups();
    }

    /// Export Tree (`popup.3`): the active spec's link
    pub(super) fn open_export_tree(&mut self) -> Result<()> {
        let spec = &self.build.spec;
        let url = self
            .data
            .game
            .tree_version(&spec.tree.version)
            .map(|v| v.url.clone())
            .ok_or_else(|| format!("{NOT_PORTED}: a tree version without a link"))?;
        let edit = super::save::encode_url(spec, &url);
        self.editor
            .popups
            .insert(0, ItemPopup::ExportTree(Box::new(ExportTree { edit })));
        self.refresh_item_popups();
        Ok(())
    }

    pub(super) fn describe_import_tree(&self, p: &ImportTree) -> J {
        let edit = |name: &str, label: &str, text: &str| {
            json!({
                "class": "EditControl", "enabled": true, "label": label,
                "path": format!("popup.{name}"), "shown": true, "text": text,
            })
        };
        json!({
            "title": "Import Tree",
            "lines": [],
            "buttons": [
                { "name": "cancel", "label": "Cancel", "enabled": true },
                { "name": "import", "label": "Import", "enabled": p.import_enabled },
            ],
            "controls": [
                edit("name", "Enter name for this passive tree:", &p.name),
                edit("edit", "Enter passive tree link:", &p.edit),
            ],
        })
    }

    pub(super) fn describe_export_tree(&self, p: &ExportTree) -> J {
        json!({
            "title": "Export Tree",
            "lines": [],
            "buttons": [
                { "name": "copy", "label": "Copy", "enabled": true },
                { "name": "done", "label": "Done", "enabled": true },
                { "name": "shrink", "label": "Shrink with PoEURL", "enabled": true },
            ],
            "controls": [{
                "class": "EditControl", "enabled": true, "path": "popup.edit",
                "shown": true, "text": p.edit,
            }],
        })
    }

    /// `control` of the Import Tree popup's inputs: their `changeFunc`
    pub(super) fn import_tree_control(
        &self,
        p: &mut ImportTree,
        name: &str,
        req: &J,
    ) -> Result<()> {
        match (name, req["text"].as_str()) {
            ("name" | "edit", Some(t)) => {
                if name == "name" {
                    p.name = t.to_string();
                } else {
                    p.edit = t.to_string();
                }
                p.msg.clear();
                p.import_enabled = true;
                Ok(())
            }
            _ => Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
    }

    /// `control` of the Export Tree popup's link box
    pub(super) fn export_tree_control(
        &self,
        p: &mut ExportTree,
        name: &str,
        req: &J,
    ) -> Result<()> {
        match (name, req["text"].as_str()) {
            ("edit", Some(t)) => {
                p.edit = t.to_string();
                Ok(())
            }
            _ => Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
    }

    /// `validateTreeVersion(alternateType, major, minor)`: the link's
    /// version when it is a known one, else the latest, with the
    /// alternate tree's suffix
    fn validate_tree_version(
        &self,
        alt: Option<String>,
        version: Option<Vec<String>>,
    ) -> Result<String> {
        let game = &self.data.game;
        let suffix = alt.map_or_else(String::new, |a| format!("_{}", pat::gsub(&a, "-", "_")));
        if let Some(v) = version {
            let (major, minor) = match (v[0].parse::<i64>(), v[1].parse::<i64>()) {
                (Ok(a), Ok(b)) => (a, b),
                _ => {
                    return Err(format!(
                        "{NOT_PORTED}: a tree link's version {}.{}",
                        v[0], v[1]
                    ));
                }
            };
            let num = pat::to_number(&format!("{major}.{minor:02}")).unwrap_or(0.0);
            let bound = |v: &str| game.tree_version(v).map_or(0.0, |t| t.num);
            if num >= bound(game.default_tree_version()) && num <= bound(game.latest_tree_version())
            {
                return Ok(format!("{}_{}{suffix}", v[0], v[1]));
            }
        }
        Ok(format!("{}{suffix}", game.latest_tree_version()))
    }

    /// The Import button: whether the popup closed
    pub(super) fn import_tree(&mut self, p: &mut ImportTree) -> Result<bool> {
        let link = p.edit.clone();
        if link.is_empty() {
            return Ok(false);
        }
        let version_lookup = "tree/([0-9]+)%.([0-9]+)%.([0-9]+)/";
        let (link, version) = if pat::is_match(&link, "poeurl%.com/") {
            return Err(format!("{NOT_PORTED}: a PoEURL tree link"));
        } else if pat::is_match(&link, "poeplanner.com/") {
            return Err(format!("{NOT_PORTED}: a poeplanner tree link"));
        } else if pat::is_match(&link, "poeskilltree.com/") {
            let old_style = "/%?v=([0-9]+)%.([0-9]+)%.([0-9]+)%-?%w?%-?%w?#";
            let version = self.validate_tree_version(
                pat::capture(&link, "%-(%l+%-?%l*)#"),
                pat::captures(&link, old_style),
            )?;
            (pat::gsub(&link, "/%?v=.+#", "/"), version)
        } else {
            let version = self.validate_tree_version(
                pat::capture(&link, "tree/(%l+%-?%l*)"),
                pat::captures(&link, version_lookup),
            )?;
            (link, version)
        };
        // `decodeTreeLink(treeLink, newTreeVersion)`: always a new spec
        if !self.data.game.tree_versions.contains(&version) {
            return Err(format!("{NOT_PORTED}: a tree link of version {version}"));
        }
        let tree = self.data.tree(&version)?;
        let mut spec = PassiveSpec::new(tree, &self.data.parser)?;
        spec.title = Some(p.name.clone());
        if let Some(e) = spec.decode_url_message(&link)? {
            p.msg = format!("^1{e}^7");
            return Ok(false);
        }
        if self.other_specs.is_empty() {
            // The list held the active spec alone
            self.other_specs.push(None);
        }
        self.other_specs.push(Some(Arc::new(spec)));
        self.specs.1.push(p.name.clone());
        let n = self.other_specs.len();
        // `SetActiveSpec(#specList)`, `spec:AddUndoState()`, a rebuild
        self.select_tree_spec(n as f64)?;
        self.add_undo_state();
        self.ui.unsaved = true;
        Ok(true)
    }
}
