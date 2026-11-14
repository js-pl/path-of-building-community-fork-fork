//! The Skills tab's socket group list (`SkillListControl`): its rows, its
//! selection, which shows a group (`SetDisplayGroup`, the group the tab's
//! detail controls edit), New, Delete with its confirmation, Delete All,
//! and the rows' tooltips.
//!
//! Upstream holds the shown group and the selection by reference: they
//! follow their groups as rows move, and here they hold the groups'
//! [`engine::skills::GroupUid`].
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::{Deletion, ItemPopup};
use super::tooltip::cc;
use super::{NOT_PORTED, Result, TypedSession, num};
use engine::skills::{SocketGroup, process_socket_group};
use serde_json::{Value as J, json};

/// The list's selection: `selIndex`, and `selValue` by its group
#[derive(Clone, Copy, Debug, Default)]
pub struct GroupSel {
    index: Option<usize>,
    value: Option<u64>,
}

impl GroupSel {
    pub(super) fn at(index: usize, uid: u64) -> GroupSel {
        GroupSel {
            index: Some(index),
            value: Some(uid),
        }
    }
}

const PATHS: [&str; 4] = [
    "skills.groupList",
    "skills.groupList.new",
    "skills.groupList.delete",
    "skills.groupList.deleteAll",
];

impl<'d> TypedSession<'d> {
    pub fn is_group_list_control(path: &str) -> bool {
        PATHS.contains(&path)
    }

    /// The row of a group in the active set's list
    pub(super) fn group_row(&self, uid: u64) -> Option<usize> {
        self.groups().iter().position(|g| g.uid.0 == uid)
    }

    /// `SetDisplayGroup(group)`: the group is processed and shown
    pub(super) fn set_display_group(&mut self, row: Option<usize>) {
        let data = &self.data.game.skills;
        let list = self
            .build
            .skills
            .as_mut()
            .and_then(|s| s.socket_group_list_mut());
        self.display_group = match (row, list) {
            (Some(i), Some(list)) if i < list.len() => {
                process_socket_group(&mut list[i], data);
                Some(list[i].uid.0)
            }
            _ => None,
        };
    }

    /// `SetActiveSkillSet`'s `SetDisplayGroup(socketGroupList[1])`
    pub(super) fn reset_display_group(&mut self) {
        self.set_display_group(Some(0));
    }

    /// `ListControl:SelectIndex(index)`: the row's group, and `OnSelect`
    /// shows it
    fn select_group_row(&mut self, index: f64) -> bool {
        let n = self.groups().len();
        let row = (index >= 1.0 && index.fract() == 0.0 && index as usize <= n)
            .then(|| index as usize - 1);
        self.group_sel.value = row.map(|i| self.groups()[i].uid.0);
        let Some(i) = row else {
            return false;
        };
        self.group_sel.index = Some(i + 1);
        self.set_display_group(Some(i));
        true
    }

    /// `GetRowValue(1, index, socketGroup)`
    fn group_row_label(&self, i: usize, g: &SocketGroup) -> String {
        let slot_enabled = self
            .main
            .sidebar
            .as_ref()
            .and_then(|sb| sb.slot_enabled.get(i).copied())
            .unwrap_or(true);
        let mut label = g.display_label.clone().unwrap_or_else(|| "?".into());
        let main = self.build.info.main_socket_group == (i + 1) as f64;
        let disabled = !g.enabled || !slot_enabled;
        if disabled {
            let colour = if main { "" } else { "^x7F7F7F" };
            label = format!("{colour}{label} (Disabled)");
        }
        if main {
            let active = if disabled {
                " (Forced Active)"
            } else {
                " (Active)"
            };
            label = format!("{label}{}{active}", cc("RELIC"));
        }
        if g.include_in_full_dps == Some(true) {
            label = format!("{label}{} (FullDPS)", cc("CUSTOM"));
        }
        if g.source.is_none() {
            let mut colors = String::new();
            for gem in g.gem_list.iter().filter(|gem| gem.enabled) {
                let Some(effect) = gem.effect() else {
                    continue;
                };
                let c = match effect.color {
                    Some(1.0) => 'R',
                    Some(2.0) => 'G',
                    Some(3.0) => 'B',
                    _ => 'W',
                };
                if !colors.is_empty() {
                    colors += "^7-";
                }
                colors += &gem.color;
                colors.push(c);
            }
            if !colors.is_empty() {
                label = format!("{label} ^7{colors}");
            }
        }
        label
    }

    /// `controls` of the list and its buttons
    pub fn group_list_json(&self, path: &str) -> Option<J> {
        let groups = self.groups();
        let sel_value = self.group_sel.value.and_then(|u| self.group_row(u));
        let button = |label: &str, enabled: bool| json!({ "class": "ButtonControl", "shown": true, "enabled": enabled, "label": label });
        Some(match path {
            "skills.groupList" => {
                let rows: Vec<String> = groups
                    .iter()
                    .enumerate()
                    .map(|(i, g)| self.group_row_label(i, g))
                    .collect();
                let sel = match (self.group_sel.value, self.group_sel.index) {
                    (Some(_), Some(i)) => num(i as f64),
                    _ => J::Null,
                };
                json!({
                    "class": "SkillListControl", "shown": true, "enabled": true,
                    "label": "^7Socket Groups:", "list": rows, "sel": sel,
                })
            }
            "skills.groupList.new" => button("New", true),
            "skills.groupList.delete" => button(
                "Delete",
                sel_value.is_some_and(|i| groups[i].source.is_none()),
            ),
            "skills.groupList.deleteAll" => button("Delete All", !groups.is_empty()),
            _ => return None,
        })
    }

    /// `control` of the list or its buttons
    pub fn group_list_control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().unwrap_or("");
        let click = req["click"].as_bool() == Some(true);
        let sel = req["sel"].as_f64();
        match path {
            "skills.groupList" if super::list_order::move_to(req).is_some() => {
                // `SelectIndex(sel)`, then the drop
                if self.select_group_row(sel.unwrap_or(0.0)) {
                    let moved = self.move_socket_group(req)?;
                    if moved {
                        self.group_sel.value = None;
                    }
                }
            }
            "skills.groupList" if req["dragTo"].is_string() => {
                return Err(format!("no drag target {}", req["dragTo"]));
            }
            "skills.groupList" if sel.is_some() => {
                // No `OnSelClick`
                self.select_group_row(sel.unwrap_or(0.0));
            }
            "skills.groupList" if req["key"].is_string() => {
                if req["key"].as_str() == Some("DELETE") && self.group_sel.value.is_some() {
                    self.delete_selected_group()?;
                }
            }
            "skills.groupList.delete" if click => {
                // `OnSelDelete(selIndex, selValue)`, enabled or not
                if self.group_sel.value.is_none() {
                    return Err(format!("{NOT_PORTED}: Delete without a group"));
                }
                self.delete_selected_group()?;
            }
            "skills.groupList.deleteAll" if click => {
                self.editor.popups.insert(
                    0,
                    ItemPopup::Confirm {
                        title: "Delete All".into(),
                        lines: vec![
                            "Are you sure you want to delete all socket groups in this build?"
                                .into(),
                        ],
                        action: Deletion::AllGroups,
                    },
                );
                self.refresh_item_popups();
            }
            "skills.groupList.new" if click => {
                let list = self
                    .skills_tab()?
                    .socket_group_list_mut()
                    .ok_or("no skill set")?;
                list.push(SocketGroup {
                    label: Some(String::new()),
                    enabled: true,
                    ..SocketGroup::default()
                });
                let n = list.len();
                let uid = list[n - 1].uid.0;
                self.group_sel = GroupSel {
                    index: Some(n),
                    value: Some(uid),
                };
                self.set_display_group(Some(n - 1));
                self.ui.unsaved = true;
                self.rebuild()?;
            }
            _ => return Err(format!("{NOT_PORTED}: control {path}")),
        }
        Ok(json!([]))
    }

    /// `OnSelDelete(selIndex, selValue)`: a group of an item cannot go, an
    /// empty one goes, another asks first
    fn delete_selected_group(&mut self) -> Result<()> {
        let (Some(index), Some(uid)) = (self.group_sel.index, self.group_sel.value) else {
            return Ok(());
        };
        let Some(row) = self.group_row(uid) else {
            return Err(format!(
                "{NOT_PORTED}: deleting a group no longer in the list"
            ));
        };
        let g = &self.groups()[row];
        if g.source.is_some() {
            return Err(format!(
                "{NOT_PORTED}: the message that an item's group stays"
            ));
        }
        if g.gem_list.is_empty() {
            self.remove_group_at(index, uid, false)?;
        } else {
            let label = g.display_label.clone().unwrap_or_default();
            self.editor.popups.insert(
                0,
                ItemPopup::Confirm {
                    title: "Delete Socket Group".into(),
                    lines: vec![format!("Are you sure you want to delete '{label}'?")],
                    action: Deletion::Group { index, uid },
                },
            );
            self.refresh_item_popups();
        }
        Ok(())
    }

    /// `t_remove(self.list, index)`, the shown group cleared if it went
    /// (`SetDisplayGroup()` when confirmed), the main socket group and the
    /// Calcs tab's skill moved up past the selection
    pub(super) fn remove_group_at(
        &mut self,
        index: usize,
        uid: u64,
        _confirmed: bool,
    ) -> Result<()> {
        let list = self
            .skills_tab()?
            .socket_group_list_mut()
            .ok_or("no skill set")?;
        if index == 0 || index > list.len() {
            return Err(format!(
                "{NOT_PORTED}: removing row {index} of {}",
                list.len()
            ));
        }
        list.remove(index - 1);
        if self.display_group == Some(uid) {
            self.display_group = None;
        }
        if let Some(sel) = self.group_sel.index {
            let sel = sel as f64;
            let main = &mut self.build.info.main_socket_group;
            if *main > sel {
                *main -= 1.0;
            }
            let calcs = &mut self.build.calcs.skill_number;
            if *calcs > sel {
                *calcs -= 1.0;
            }
        }
        self.group_sel.value = None;
        self.ui.unsaved = true;
        self.rebuild()
    }

    /// Delete All, confirmed
    pub(super) fn remove_all_groups(&mut self) -> Result<()> {
        if let Some(list) = self.skills_tab()?.socket_group_list_mut() {
            list.clear();
        }
        self.display_group = None;
        self.group_sel = GroupSel::default();
        self.ui.unsaved = true;
        self.rebuild()
    }

    /// `controlTooltip` of a row: `AddValueTooltip`, empty for a group the
    /// sidebar's calculation never listed the skills of
    pub fn group_list_tooltip(&self, index: Option<f64>) -> Result<J> {
        let n = self.groups().len();
        let row = index
            .filter(|i| *i >= 1.0 && i.fract() == 0.0 && *i as usize <= n)
            .map(|i| i as usize - 1);
        match row {
            Some(i) if self.groups()[i].display_skill_list => {
                self.socket_group_tooltip((i + 1) as f64)
            }
            _ => Ok(super::tooltip::Tooltip::default().json()),
        }
    }
}
