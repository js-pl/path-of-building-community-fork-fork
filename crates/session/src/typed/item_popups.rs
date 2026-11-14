//! The item editor's popups: `ItemsTab:CraftItem` (Craft Item: rarity,
//! title, type and base) and `ItemsTab:EditDisplayItemText` (Edit Item
//! Text) and `ItemsTab:EnchantDisplayItem` (Enchant Item, in
//! `item_enchant.rs`), `ItemsTab:AnointDisplayItem` (Anoint Item, in
//! `item_anoint.rs`), `ItemsTab:CorruptDisplayItem` (Corrupt Item, in
//! `item_corrupt.rs`), `AddImplicitPopup` (Add Implicit to Item, in
//! `item_add_implicit.rs`), `ItemsTab:AddCustomModifierToDisplayItem`
//! (Add Modifier to Item, in `item_add_modifier.rs`),
//! `ItemsTab:AddCrucibleModifierToDisplayItem` (in `item_crucible.rs`), as `lua/session.lua` shows them (`describePopups`) and drives them
//! (`control` of `popup.*`, `popupButton`, `closePopup`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::display_stats::{MAGIC, NORMAL, RARE, RELIC, UNIQUE};
use super::{NOT_PORTED, Result, TypedSession};
use engine::item::{Item, ModLine};
use serde_json::{Value as J, json};

/// `rarityDropList`: label and rarity
const RARITIES: [(&str, &str); 5] = [
    (NORMAL, "NORMAL"),
    (MAGIC, "MAGIC"),
    (RARE, "RARE"),
    (UNIQUE, "UNIQUE"),
    (RELIC, "RELIC"),
];

fn rarity_labels() -> J {
    let names = ["Normal", "Magic", "Rare", "Unique", "Relic"];
    J::Array(
        RARITIES
            .iter()
            .zip(names)
            .map(|((c, _), n)| json!(format!("{c}{n}")))
            .collect(),
    )
}

/// A popup the item editor opened; selections are 1-based as upstream's
#[derive(Clone, Debug)]
pub enum ItemPopup {
    Craft {
        rarity: usize,
        title: String,
        typ: usize,
        base: usize,
    },
    EditText {
        rarity: usize,
        edit: String,
        /// Whether an item was in the editor when it opened
        with_item: bool,
    },
    Enchant(Box<super::item_enchant::EnchantPopup>),
    Anoint(Box<super::item_anoint::AnointPopup>),
    Corrupt(Box<super::item_corrupt::CorruptPopup>),
    AddImplicit(Box<super::item_add_implicit::AddImplicitPopup>),
    AddModifier(Box<super::item_add_modifier::AddModifierPopup>),
    Crucible(Box<super::item_crucible::CruciblePopup>),
    /// Manage Item Sets, and the name popup of a set
    ManageSets(Box<super::item_sets::ManageSets>),
    ManageSpecs(Box<super::spec_sets::ManageSpecs>),
    RenameSpec(Box<super::spec_sets::RenameSpec>),
    RenameSet(Box<super::item_sets::RenameSet>),
    /// `main:OpenConfirmPopup(title, msg, "Delete", ...)` of the item list
    Confirm {
        title: String,
        lines: Vec<String>,
        action: Deletion,
    },
}

/// What a confirmation's Delete does
#[derive(Clone, Debug)]
pub enum Deletion {
    Item(f64),
    AllItems,
    /// The tree spec at this index of the list
    Spec {
        index: usize,
    },
    /// The set at this index of its list
    Set {
        kind: super::item_sets::SetKind,
        index: usize,
        id: f64,
    },
}

/// `DropDownControl:SetSel(sel)`: the new selection, clamped to the list,
/// and whether it changed
fn set_sel(cur: &mut usize, sel: f64, len: usize) -> bool {
    let sel = sel.min(len as f64).max(1.0) as usize;
    let changed = sel != *cur;
    *cur = sel;
    changed
}

impl<'d> TypedSession<'d> {
    /// Whether the front popup is one of the item editor's
    pub fn has_item_popup(&self) -> bool {
        !self.editor.popups.is_empty() && self.class_popups_open() == 0
    }

    /// The other popups, behind the item popups
    pub(super) fn class_popups_open(&self) -> usize {
        self.ui.popups.as_array().map_or(0, Vec::len) - self.editor.described
    }

    /// `data.itemBaseLists[data.itemBaseTypeList[typ]]`
    fn base_list(&self, typ: usize) -> &[String] {
        self.data
            .game
            .item_base_lists
            .get(typ.wrapping_sub(1))
            .map_or(&[], |(_, l)| l.as_slice())
    }

    /// `ItemsTab:CraftItem()`: opens Craft Item with the last choices
    pub fn craft_item(&mut self) -> Result<J> {
        if self.class_popups_open() > 0 {
            return Err(format!(
                "{NOT_PORTED}: crafting with a class change popup open"
            ));
        }
        let (rarity, typ, base) = self.editor.last_craft.unwrap_or((3, 1, 1));
        self.editor.popups.insert(
            0,
            ItemPopup::Craft {
                rarity,
                title: String::new(),
                typ,
                base,
            },
        );
        self.refresh_item_popups();
        Ok(json!([]))
    }

    /// `main:ClosePopup()` of an item popup
    pub fn close_item_popup(&mut self) -> J {
        if !self.editor.popups.is_empty() {
            self.editor.popups.remove(0);
        }
        self.refresh_item_popups();
        json!([])
    }

    /// The `ItemsTab.lua` raw of Edit Item Text (`buildRaw`)
    fn edit_text_raw(rarity: usize, edit: &str) -> String {
        if engine::pat::is_match(edit, "^Item Class: .*\nRarity: ")
            || engine::pat::is_match(edit, "^Rarity: ")
        {
            edit.to_string()
        } else {
            let r = RARITIES.get(rarity.wrapping_sub(1)).map_or("", |r| r.1);
            format!("Rarity: {r}\n{edit}")
        }
    }

    /// `control` of an item popup's input
    pub fn item_popup_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        let name = path.strip_prefix("popup.").unwrap_or(path);
        let list_click = match self.editor.popups.first() {
            Some(ItemPopup::ManageSets(m)) => {
                name.split('.').next() == Some(m.kind_list()) || name.starts_with("sharedList")
            }
            Some(ItemPopup::ManageSpecs(_)) => name.split('.').next() == Some("1"),
            _ => false,
        };
        if req["click"].as_bool() == Some(true) && !list_click {
            return self.item_popup_button(name);
        }
        if let Some(ItemPopup::ManageSets(p)) = self.editor.popups.first() {
            let mut p = p.clone();
            let r = self.manage_sets_control(&mut p, name, req);
            // The name popup or a confirmation may have opened in front
            if let Some(i) = self
                .editor
                .popups
                .iter()
                .position(|x| matches!(x, ItemPopup::ManageSets(_)))
            {
                self.editor.popups[i] = ItemPopup::ManageSets(p);
            }
            self.refresh_item_popups();
            return r.map(|_| json!([]));
        }
        if let Some(ItemPopup::ManageSpecs(p)) = self.editor.popups.first() {
            let mut p = p.clone();
            let r = self.manage_specs_control(&mut p, name, req);
            if let Some(i) = self
                .editor
                .popups
                .iter()
                .position(|x| matches!(x, ItemPopup::ManageSpecs(_)))
            {
                self.editor.popups[i] = ItemPopup::ManageSpecs(p);
            }
            self.refresh_item_popups();
            return r.map(|_| json!([]));
        }
        if let Some(ItemPopup::RenameSpec(p)) = self.editor.popups.first() {
            let mut p = p.clone();
            let r = self.rename_spec_control(&mut p, name, req);
            self.editor.popups[0] = ItemPopup::RenameSpec(p);
            self.refresh_item_popups();
            return r.map(|_| json!([]));
        }
        if let Some(ItemPopup::RenameSet(p)) = self.editor.popups.first() {
            let mut p = p.clone();
            let r = self.rename_set_control(&mut p, name, req);
            self.editor.popups[0] = ItemPopup::RenameSet(p);
            self.refresh_item_popups();
            return r.map(|_| json!([]));
        }
        if let Some(ItemPopup::Crucible(p)) = self.editor.popups.first() {
            let mut p = p.clone();
            let r = self.crucible_popup_control(&mut p, name, req);
            self.editor.popups[0] = ItemPopup::Crucible(p);
            self.refresh_item_popups();
            return r.map(|_| json!([]));
        }
        if let Some(ItemPopup::AddModifier(p)) = self.editor.popups.first() {
            let mut p = p.clone();
            let r = self.add_modifier_popup_control(&mut p, name, req);
            self.editor.popups[0] = ItemPopup::AddModifier(p);
            self.refresh_item_popups();
            return r.map(|_| json!([]));
        }
        if let Some(ItemPopup::AddImplicit(p)) = self.editor.popups.first() {
            let mut p = p.clone();
            let r = self.add_implicit_popup_control(&mut p, name, req);
            self.editor.popups[0] = ItemPopup::AddImplicit(p);
            self.refresh_item_popups();
            return r.map(|_| json!([]));
        }
        if let Some(ItemPopup::Anoint(p)) = self.editor.popups.first() {
            let mut p = p.clone();
            let r = self.anoint_popup_control(&mut p, name, req);
            self.editor.popups[0] = ItemPopup::Anoint(p);
            self.refresh_item_popups();
            return r.map(|_| json!([]));
        }
        if let Some(ItemPopup::Corrupt(p)) = self.editor.popups.first() {
            let mut p = p.clone();
            let r = self.corrupt_popup_control(&mut p, name, req);
            self.editor.popups[0] = ItemPopup::Corrupt(p);
            self.refresh_item_popups();
            return r.map(|_| json!([]));
        }
        if let Some(ItemPopup::Enchant(p)) = self.editor.popups.first() {
            let mut p = p.clone();
            self.enchant_popup_control(&mut p, name, req)?;
            self.editor.popups[0] = ItemPopup::Enchant(p);
            self.refresh_item_popups();
            return Ok(json!([]));
        }
        let lists = self.data.game.item_base_lists.len();
        let popup = self.editor.popups.first_mut().ok_or("no popup")?;
        let sel = req["sel"].as_f64();
        let text = req["text"].as_str();
        match (popup, name) {
            (ItemPopup::Craft { rarity, .. } | ItemPopup::EditText { rarity, .. }, "rarity")
                if sel.is_some() =>
            {
                set_sel(rarity, sel.unwrap(), RARITIES.len());
            }
            (ItemPopup::Craft { title, .. }, "title") if text.is_some() => {
                *title = text.unwrap().to_string()
            }
            (ItemPopup::EditText { edit, .. }, "edit") if text.is_some() => {
                *edit = text.unwrap().to_string()
            }
            (ItemPopup::Craft { typ, base, .. }, "type") if sel.is_some() => {
                // `selFunc`: the type's bases, the first selected
                if set_sel(typ, sel.unwrap(), lists) {
                    *base = 1;
                }
            }
            (ItemPopup::Craft { typ, base, .. }, "base") if sel.is_some() => {
                let (t, b) = (*typ, *base);
                let len = self.base_list(t).len();
                let mut b = b;
                set_sel(&mut b, sel.unwrap(), len);
                if let Some(ItemPopup::Craft { base, .. }) = self.editor.popups.first_mut() {
                    *base = b;
                }
            }
            _ => return Err(format!("{NOT_PORTED}: control {path} of an item popup")),
        }
        self.refresh_item_popups();
        Ok(json!([]))
    }

    /// `popupButton` of an item popup
    pub fn item_popup_button(&mut self, name: &str) -> Result<J> {
        let popup = self.editor.popups.first().cloned().ok_or("no popup")?;
        match (popup, name) {
            (ItemPopup::Enchant(p), name) => {
                // `SetDisplayItem(...)`, then `main:ClosePopup()`
                self.enchant_popup_button(&p, name)?;
                self.close_item_popup();
            }
            (ItemPopup::Corrupt(mut p), name) => {
                let closes = self.corrupt_popup_button(&mut p, name)?;
                if closes {
                    self.close_item_popup();
                } else {
                    self.editor.popups[0] = ItemPopup::Corrupt(p);
                }
            }
            (ItemPopup::Crucible(p), "save") => {
                self.crucible_popup_save(&p)?;
                self.close_item_popup();
            }
            (ItemPopup::Crucible(_), "close") => {
                self.close_item_popup();
            }
            (ItemPopup::AddModifier(p), "save") => {
                self.add_modifier_popup_save(&p)?;
                self.close_item_popup();
            }
            (ItemPopup::AddModifier(_), "close") => {
                self.close_item_popup();
            }
            (ItemPopup::AddImplicit(p), "save") => {
                self.add_implicit_popup_save(&p)?;
                self.close_item_popup();
            }
            (ItemPopup::AddImplicit(_), "close") => {
                self.close_item_popup();
            }
            (ItemPopup::Anoint(p), "save") => {
                self.anoint_popup_save(&p)?;
                self.close_item_popup();
            }
            (ItemPopup::RenameSpec(p), "save") => {
                self.rename_spec_save(&p)?;
                self.sync_loadouts_and_rebuild()?;
                self.close_item_popup();
            }
            (ItemPopup::RenameSpec(p), "cancel") => {
                self.rename_spec_cancel(&p);
                self.close_item_popup();
            }
            (ItemPopup::RenameSet(p), "cancel") => {
                self.rename_set_cancel(&p);
                self.close_item_popup();
            }
            (ItemPopup::Anoint(_), "close") | (_, "cancel") => {
                self.close_item_popup();
            }
            (
                ItemPopup::Craft {
                    rarity,
                    title,
                    typ,
                    base,
                },
                "save",
            ) => {
                self.close_item_popup();
                let name = self
                    .base_list(typ)
                    .get(base.wrapping_sub(1))
                    .cloned()
                    .ok_or("attempt to index a nil value (base)")?;
                let item = self.make_item(&name, rarity, &title)?;
                let edit = !item.crafted && item.rarity != "NORMAL";
                self.set_display_item_value(Some(item))?;
                if edit {
                    self.edit_display_item_text()?;
                }
                self.editor.last_craft = Some((rarity, typ, base));
            }
            (ItemPopup::EditText { rarity, edit, .. }, "save") => {
                let id = self.display_item_id();
                let raw = Self::edit_text_raw(rarity, &edit);
                // `CreateDisplayItemFromRaw(raw, not self.displayItem)`
                let normalise = !self.has_display_item();
                self.display_item_from_raw(&raw, normalise)?;
                self.set_display_item_id(id);
                self.close_item_popup();
            }
            (ItemPopup::ManageSets(_), "close") => {
                self.close_item_popup();
            }
            (ItemPopup::RenameSet(p), "save") => {
                // `main:ClosePopup()` after the changes
                self.rename_set_save(&p)?;
                self.sync_loadouts_and_rebuild()?;
                self.close_item_popup();
            }
            (ItemPopup::Confirm { action, .. }, "confirm") => {
                // `main:ClosePopup()`, then `onConfirm()`
                self.close_item_popup();
                self.confirm_action(&action)?;
            }
            (_, name) => return Err(format!("no popup button {name}")),
        }
        self.refresh_item_popups();
        Ok(json!([]))
    }

    /// `makeItem(base)` of `CraftItem`
    fn make_item(&self, name: &str, rarity: usize, title: &str) -> Result<Item> {
        let data = &self.data.game.items;
        let base = data
            .base(name)
            .cloned()
            .ok_or_else(|| format!("no base {name}"))?;
        let mut item = Item {
            name: name.to_string(),
            base_name: Some(name.to_string()),
            base: Some(base.clone()),
            ..Item::default()
        };
        item.quality = match base.typ.as_str() {
            "Amulet" | "Belt" | "Jewel" | "Quiver" | "Ring" | "Graft" => None,
            _ => Some(0.0),
        };
        let mut sel = rarity;
        if (base.flask.is_some()
            || (base.typ == "Jewel" && base.sub_type.as_deref() == Some("Charm"))
            || base.typ == "Tincture")
            && sel == 3
        {
            sel = 2;
        }
        if sel == 2 || sel == 3 {
            item.crafted = true;
        }
        item.rarity = RARITIES
            .get(sel.wrapping_sub(1))
            .map(|r| r.1.to_string())
            .ok_or("attempt to index a nil value (rarity)")?;
        if sel >= 3 {
            item.title = Some(if engine::pat::is_match(title, "%S") {
                title.to_string()
            } else {
                "New Item".into()
            });
        }
        if let Some(implicit) = &base.implicit {
            for (i, line) in implicit.split('\n').filter(|l| !l.is_empty()).enumerate() {
                let tags = base
                    .implicit_mod_types
                    .as_ref()
                    .and_then(|t| t.get(i).cloned())
                    .unwrap_or_default();
                item.implicit_mod_lines.push(ModLine {
                    line: line.to_string(),
                    mod_tags: tags,
                    ..ModLine::default()
                });
            }
        }
        item.normalise_quality();
        let raw = item.build_raw();
        item.parse_raw(&self.data.item_context(), &raw, None, false)?;
        Ok(item)
    }

    /// `ItemsTab:EditDisplayItemText()`: opens Edit Item Text on the
    /// display item
    pub(super) fn edit_display_item_text(&mut self) -> Result<()> {
        let item = self.display_item.as_ref().ok_or("no item in the editor")?;
        let edit = engine::pat::gsub(&item.build_raw(), "Rarity: %w+\n", "");
        let rarity = RARITIES
            .iter()
            .position(|r| r.1 == item.rarity)
            .map_or(1, |i| i + 1);
        self.editor.popups.insert(
            0,
            ItemPopup::EditText {
                rarity,
                edit,
                with_item: true,
            },
        );
        Ok(())
    }

    /// Writes the item popups' descriptions (`describePopups`) in front of
    /// the others
    pub(super) fn refresh_item_popups(&mut self) {
        let others: Vec<J> = self
            .ui
            .popups
            .as_array()
            .map(|p| p.iter().skip(self.editor.described).cloned().collect())
            .unwrap_or_default();
        let mut all: Vec<J> = self
            .editor
            .popups
            .iter()
            .map(|p| self.describe_item_popup(p))
            .collect();
        self.editor.described = all.len();
        all.extend(others);
        self.ui.popups = J::Array(all);
    }

    /// The names of a popup's inputs and buttons
    pub(super) fn describe_item_popup_names(&self, popup: &ItemPopup) -> Vec<String> {
        let d = self.describe_item_popup(popup);
        let mut out: Vec<String> = Vec::new();
        for c in d["controls"].as_array().into_iter().flatten() {
            if let Some(p) = c["path"].as_str() {
                out.push(p.trim_start_matches("popup.").to_string());
            }
        }
        for b in d["buttons"].as_array().into_iter().flatten() {
            if let Some(n) = b["name"].as_str() {
                out.push(n.to_string());
            }
        }
        out
    }

    fn describe_item_popup(&self, popup: &ItemPopup) -> J {
        match popup {
            ItemPopup::Enchant(p) => self.describe_enchant_popup(p),
            ItemPopup::Anoint(p) => self.describe_anoint_popup(p),
            ItemPopup::Corrupt(p) => self.describe_corrupt_popup(p),
            ItemPopup::AddImplicit(p) => self.describe_add_implicit_popup(p),
            ItemPopup::AddModifier(p) => self.describe_add_modifier_popup(p),
            ItemPopup::Crucible(p) => self.describe_crucible_popup(p),
            ItemPopup::ManageSets(p) => self.describe_manage_sets(p),
            ItemPopup::RenameSet(p) => self.describe_rename_set(p),
            ItemPopup::ManageSpecs(p) => self.describe_manage_specs(p),
            ItemPopup::RenameSpec(p) => self.describe_rename_spec(p),
            ItemPopup::Confirm { title, lines, .. } => json!({
                "title": title,
                "lines": lines,
                "buttons": [{ "name": "confirm", "label": "Delete", "enabled": true }],
                "controls": [],
            }),
            ItemPopup::Craft {
                rarity,
                title,
                typ,
                base,
            } => {
                let types: Vec<J> = self
                    .data
                    .game
                    .item_base_lists
                    .iter()
                    .map(|(t, _)| json!(t))
                    .collect();
                let bases: Vec<J> = self
                    .base_list(*typ)
                    .iter()
                    .map(|b| json!(engine::pat::gsub(b, " %(.+%)", "")))
                    .collect();
                let mut controls = vec![json!({
                    "class": "DropDownControl", "enabled": true, "shown": true,
                    "label": "Rarity:", "list": rarity_labels(), "sel": rarity,
                    "path": "popup.rarity",
                })];
                if *rarity >= 3 {
                    controls.push(json!({
                        "class": "EditControl", "enabled": true, "shown": true,
                        "text": title, "path": "popup.title",
                    }));
                }
                controls.push(json!({
                    "class": "DropDownControl", "enabled": true, "shown": true,
                    "label": "Type:", "list": types, "sel": typ, "path": "popup.type",
                }));
                controls.push(json!({
                    "class": "DropDownControl", "enabled": true, "shown": true,
                    "label": "Base:", "list": bases, "sel": base, "path": "popup.base",
                }));
                json!({
                    "title": "Craft Item",
                    "lines": [],
                    "buttons": [
                        { "name": "cancel", "label": "Cancel", "enabled": true },
                        { "name": "save", "label": "Create", "enabled": true },
                    ],
                    "controls": controls,
                })
            }
            ItemPopup::EditText {
                rarity,
                edit,
                with_item,
            } => {
                let raw = Self::edit_text_raw(*rarity, edit);
                let valid = Item::new(&self.data.item_context(), &raw, None, false)
                    .is_ok_and(|i| i.base.is_some());
                let (title, save) = if *with_item {
                    ("Edit Item Text", "Save")
                } else {
                    ("Create Custom Item from Text", "Create")
                };
                json!({
                    "title": title,
                    "lines": [],
                    "buttons": [
                        { "name": "cancel", "label": "Cancel", "enabled": true },
                        { "name": "save", "label": save, "enabled": valid },
                    ],
                    "controls": [
                        {
                            "class": "DropDownControl", "enabled": true, "shown": true,
                            "list": rarity_labels(), "sel": rarity, "path": "popup.rarity",
                        },
                        {
                            "class": "EditControl", "enabled": true, "shown": true,
                            "multiline": true, "text": edit, "path": "popup.edit",
                        },
                    ],
                })
            }
        }
    }
}
