//! The Notes tab (`Classes/NotesTab.lua`): `notes` and `setNotes` of
//! `lua/session.lua`, on the text the tab's edit control holds.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{Result, TypedSession};
use engine::xml::{Element, Node};
use serde_json::{Value as J, json};

/// `NotesTab:Load`: the edit control takes each text of the section in
/// turn, so the last one stays
pub(super) fn load_notes(root: &Element) -> String {
    root.child("PathOfBuilding")
        .and_then(|p| p.child("Notes"))
        .and_then(|n| {
            n.children.iter().rev().find_map(|c| match c {
                Node::Text(t) => Some(t.clone()),
                Node::Element(_) => None,
            })
        })
        .unwrap_or_default()
}

impl TypedSession<'_> {
    /// `notes`: the edit control's text
    pub fn notes(&self) -> J {
        json!({ "text": self.notes })
    }

    /// `setNotes`: `EditControl:SetText`, which takes the text as it is
    pub fn set_notes(&mut self, text: &str) -> Result<J> {
        self.notes = text.to_string();
        self.ui.unsaved = true;
        Ok(json!([]))
    }

    /// `NotesTab:Save`: `SetShowColorCodes(false)` first, which turns
    /// escaped colour codes back into colour codes in the text itself
    pub(super) fn notes_for_save(&mut self) -> &str {
        let t = engine::pat::gsub(&self.notes, "%^_x(%x%x%x%x%x%x)", "^x%1");
        self.notes = engine::pat::gsub(&t, "%^_(%d)", "^%1");
        &self.notes
    }
}
