//! The hover tooltips of the item editor's popups (`controlTooltip`): a
//! drop-down entry's mod with its stat comparison
//! (`ItemsTab:AddModComparisonTooltip`), an enchanted item, or the item a
//! save button would make (`AddItemTooltip`).
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::ItemPopup;
use super::tooltip::{Outputs, Tooltip};
use super::{NOT_PORTED, Result, TypedSession};
use engine::calc_override::Override;
use engine::calculator::MiscCalculator;
use engine::item::{Item, ModLine};
use engine::item_data::Affix;
use serde_json::Value as J;

/// A new item of the popup: upstream compares items by identity, so it is
/// none of the build's
fn unowned(mut item: Item) -> Item {
    item.id = None;
    item
}

impl<'d> TypedSession<'d> {
    /// `ItemsTab:AddModComparisonTooltip(tooltip, mod, replaceImplicits)`
    pub(super) fn add_mod_comparison_tooltip(
        &self,
        tip: &mut Tooltip,
        m: &Affix,
        replace_implicits: bool,
    ) -> Result<()> {
        let display = self.display_item.as_ref().ok_or("no item in the editor")?;
        let slot = display.primary_slot();
        let mut item = Item::new(&self.data.item_context(), &display.build_raw(), None, false)?;
        let target = if replace_implicits {
            item.implicit_mod_lines.clear();
            &mut item.implicit_mod_lines
        } else {
            &mut item.explicit_mod_lines
        };
        for l in &m.lines {
            target.push(ModLine {
                line: self.line_for_allocates(l),
                mod_tags: m.mod_tags.clone(),
                ..ModLine::default()
            });
        }
        self.build_and_parse_raw(&mut item)?;
        let data = self.data.calc_data();
        let calc = MiscCalculator::new(&self.build, &data)?;
        let rep = |item: Item| Override {
            rep_item: Some((slot.clone(), Some(item))),
            ..Override::default()
        };
        // `calcFunc(override)`: `useFullDPS` nil calculates the Full DPS
        let base = calc.calculate(&rep(display.clone()), true)?;
        let new = calc.calculate(&rep(item), true)?;
        self.add_stat_compares(
            tip,
            &Outputs {
                player: &base.player,
                minion: base.minion.as_ref(),
            },
            &Outputs {
                player: &new.player,
                minion: new.minion.as_ref(),
            },
            "\nAdding this mod will give: ",
            None,
        )?;
        Ok(())
    }

    /// The mod's lines, then its comparison
    fn mod_tooltip(
        &self,
        tip: &mut Tooltip,
        m: &Affix,
        allocates: bool,
        replace: bool,
    ) -> Result<()> {
        for l in &m.lines {
            let l = if allocates {
                self.line_for_allocates(l)
            } else {
                l.clone()
            };
            tip.add_line(16.0, &format!("^7{l}"));
        }
        self.add_mod_comparison_tooltip(tip, m, replace)
    }

    /// `controlTooltip` of an input or button of the front item popup
    pub fn item_popup_tooltip(&mut self, path: &str, index: Option<f64>) -> Result<J> {
        let name = path.strip_prefix("popup.").unwrap_or(path).to_string();
        let popup = self.editor.popups.first().cloned().ok_or("no popup")?;
        let mut tip = Tooltip::default();
        let idx = index
            .filter(|i| *i >= 1.0 && i.fract() == 0.0)
            .map(|i| i as usize);
        let unported = || Err(format!("{NOT_PORTED}: the tooltip of popup.{name}"));
        match (&popup, name.as_str()) {
            (ItemPopup::Enchant(p), "enchantment") => {
                if let Some(item) = self.enchant_entry_item(p, idx)? {
                    self.add_item_tooltip_mode(&mut tip, &unowned(item), None, true)?;
                }
            }
            (ItemPopup::Corrupt(p), n) if n.starts_with("implicit") => {
                if let Some(m) = self.corrupt_entry(p, n, idx) {
                    self.mod_tooltip(&mut tip, &m, false, true)?;
                }
            }
            (ItemPopup::Corrupt(p), "save") => {
                let item = self.corrupt_save_item(p)?;
                self.add_item_tooltip_mode(&mut tip, &unowned(item), None, false)?;
            }
            (ItemPopup::AddImplicit(p), "modGroupSelect") => {
                if let Some((m, title, vestigial)) = self.implicit_group_entry(p, idx) {
                    for l in &m.lines {
                        tip.add_line(16.0, &format!("^7{l}"));
                    }
                    if let Some(t) = title {
                        tip.add_line(
                            16.0,
                            &format!("^7Source: {}{t}^7", super::display_stats::UNIQUE),
                        );
                    }
                    self.add_mod_comparison_tooltip(&mut tip, &m, vestigial)?;
                }
            }
            (ItemPopup::AddImplicit(p), "modSelect") => {
                if let Some((m, title, _)) = self.implicit_mod_entry(p, idx) {
                    for l in &m.lines {
                        tip.add_line(16.0, &format!("^7{l}"));
                    }
                    if let Some(t) = title {
                        tip.add_line(
                            16.0,
                            &format!("^7Source: {}{t}^7", super::display_stats::UNIQUE),
                        );
                    }
                    self.add_mod_comparison_tooltip(&mut tip, &m, false)?;
                }
            }
            (ItemPopup::AddImplicit(p), "save") => {
                let item = self.add_implicit_save_item(p)?;
                self.add_item_tooltip_mode(&mut tip, &unowned(item), None, false)?;
            }
            (ItemPopup::AddModifier(p), "modSelect") => {
                if let Some(m) = self.modifier_entry(p, idx) {
                    self.mod_tooltip(&mut tip, &m, false, false)?;
                }
            }
            (ItemPopup::AddModifier(p), "save") => {
                let item = self.add_modifier_save_item(p)?;
                self.add_item_tooltip_mode(&mut tip, &unowned(item), None, false)?;
            }
            (ItemPopup::Crucible(p), n) if n.starts_with("modSelectNode") => {
                if let Some(m) = self.crucible_entry(p, n, idx) {
                    self.mod_tooltip(&mut tip, &m, true, false)?;
                }
            }
            (ItemPopup::Crucible(p), "save") => {
                let item = self.crucible_save_item(p)?;
                self.add_item_tooltip_mode(&mut tip, &unowned(item), None, false)?;
            }
            (ItemPopup::Anoint(p), "save" | "notableDB") => {
                self.anoint_tooltip(&mut tip, p, &name, idx)?;
            }
            (ItemPopup::ManageSets(_), "sharedList") => {
                return self.shared_set_tooltip(index);
            }
            (ItemPopup::ManageSets(m), n) if n == m.kind_list() => {
                return self.set_list_tooltip(m.kind, index);
            }
            // The other inputs and buttons have no tooltip function
            (_, _) => {
                let known = self.describe_item_popup_names(&popup);
                if !known.contains(&name) {
                    return unported();
                }
            }
        }
        Ok(tip.json())
    }
}
