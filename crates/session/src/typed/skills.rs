//! The Skills tab's edits and views (`skills`, `gemList`,
//! `setMainSocketGroup`, `setSocketGroup`, `addSocketGroup`,
//! `removeSocketGroup`, `setGem`, `removeGem`, `selectSkillSet` of
//! `lua/session.lua`) and the skill selection controls' handlers
//! (`buildMode`'s and the Calcs tab's), on the typed build.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{Result, TypedSession, num};
use engine::skills::{GemInstance, Skills, SocketGroup, process_socket_group};
use serde_json::{Map, Value as J, json};

/// A gem as `describeGem` gives it
fn gem_json(g: &GemInstance) -> J {
    let mut o = Map::new();
    o.insert("name".into(), json!(g.name_spec));
    for (k, v) in [
        ("level", g.level),
        ("quality", g.quality),
        ("count", g.count),
    ] {
        if let Some(v) = v {
            o.insert(k.into(), num(v));
        }
    }
    if let Some(q) = &g.quality_id {
        o.insert("qualityId".into(), json!(q));
    }
    o.insert("enabled".into(), J::Bool(g.enabled));
    o.insert("color".into(), json!(g.color));
    if let Some(e) = &g.err_msg {
        o.insert("error".into(), json!(e));
    }
    let support = g.gem.as_ref().is_some_and(|d| d.granted_effect.support);
    o.insert("support".into(), J::Bool(support));
    J::Object(o)
}

/// What a request sets on a gem or a socket group
#[derive(Default)]
pub struct GemEdit {
    pub name: Option<String>,
    pub level: Option<f64>,
    pub quality: Option<f64>,
    pub quality_id: Option<String>,
    pub enabled: Option<bool>,
    pub count: Option<f64>,
}

#[derive(Default)]
pub struct GroupEdit {
    pub label: Option<String>,
    pub enabled: Option<bool>,
    pub include_in_full_dps: Option<bool>,
    pub slot: Option<String>,
}

impl TypedSession<'_> {
    fn skills_tab(&mut self) -> Result<&mut Skills> {
        self.build
            .skills
            .as_mut()
            .ok_or_else(|| "no skills tab".into())
    }

    fn group_mut(&mut self, index: f64) -> Result<&mut SocketGroup> {
        self.skills_tab()?
            .socket_group_list_mut()
            .and_then(|l| (index as usize).checked_sub(1).and_then(|i| l.get_mut(i)))
            .ok_or_else(|| format!("no socket group {}", engine::pat::num(index)))
    }

    /// `ProcessSocketGroup` on a group, then a frame
    fn group_changed(&mut self, index: f64) -> Result<()> {
        let data = &self.data.game.skills;
        let g = self.group_mut(index)?;
        process_socket_group(g, data);
        self.rebuild()
    }

    /// The sidebar's calculation sets `displayLabel` on the groups it
    /// uses; after loading, every group has one in upstream (`all`)
    pub(super) fn update_display_labels(&mut self, all: bool) {
        let Some(sb) = self.main.sidebar.as_ref() else {
            return;
        };
        let main = sb.main_socket_group;
        let used = sb.slot_enabled.clone();
        let counts: Vec<usize> = sb.groups.iter().map(Vec::len).collect();
        let calcs_group = self.build.calcs.skill_number as usize;
        let Some(list) = self
            .build
            .skills
            .as_mut()
            .and_then(|s| s.socket_group_list_mut())
        else {
            return;
        };
        for (gi, g) in list.iter_mut().enumerate() {
            if all || gi + 1 == main || (g.enabled && used.get(gi).copied().unwrap_or(false)) {
                g.display_label = Some(display_label(g));
            }
            // The calculations write back the main group's active skill
            let n = counts.get(gi).copied().unwrap_or(0) as f64;
            if n > 0.0 {
                if gi + 1 == main {
                    g.main_active_skill = Some(n.min(g.main_active_skill.unwrap_or(1.0)));
                }
                if gi + 1 == calcs_group {
                    let v = g.main_active_skill_calcs.unwrap_or(1.0);
                    g.main_active_skill_calcs = Some(n.min(v));
                }
            }
        }
    }

    /// `skills`: the socket groups, slots and skill sets
    pub fn skills(&self) -> Result<J> {
        let groups: Vec<J> = self
            .groups()
            .iter()
            .map(|g| {
                let mut o = Map::new();
                for (k, v) in [
                    ("label", &g.label),
                    ("displayLabel", &g.display_label),
                    ("slot", &g.slot),
                    ("source", &g.source),
                ] {
                    if let Some(v) = v {
                        o.insert(k.into(), json!(v));
                    }
                }
                o.insert("enabled".into(), J::Bool(g.enabled));
                o.insert(
                    "includeInFullDPS".into(),
                    J::Bool(g.include_in_full_dps == Some(true)),
                );
                o.insert(
                    "gems".into(),
                    J::Array(g.gem_list.iter().map(gem_json).collect()),
                );
                J::Object(o)
            })
            .collect();
        let tab = self.build.skills.as_ref().ok_or("no skills tab")?;
        let sets: Vec<J> = tab
            .skill_set_order_list
            .iter()
            .map(|&id| {
                let title = tab
                    .skill_sets
                    .get(&(id as i64))
                    .and_then(|s| s.title.clone())
                    .unwrap_or_else(|| "Default".into());
                json!({ "id": num(id), "title": title })
            })
            .collect();
        Ok(json!({
            "groups": groups,
            "mainSocketGroup": num(self.build.info.main_socket_group),
            "slots": self.shown_slots(),
            "skillSets": sets,
            "activeSkillSet": num(tab.active_skill_set_id),
        }))
    }

    /// The item slots shown, but jewel sockets (`slot:IsShown()`)
    fn shown_slots(&self) -> Vec<String> {
        let tab = &self.build.items;
        let second = tab.active_item_set().use_second_weapon_set;
        let ring3 = self
            .main
            .sidebar
            .as_ref()
            .is_some_and(|s| s.additional_ring_slot);
        let version = &*self.build.spec.tree.version;
        let abyssal_count = |parent: &str| {
            tab.ordered_slots
                .iter()
                .find(|s| s.slot_name == parent)
                .and_then(|s| tab.item(s.sel_item_id))
                .map_or(0.0, |i| i.abyssal_socket_count)
        };
        tab.ordered_slots
            .iter()
            .filter(|s| s.node_id.is_none())
            .filter(|s| {
                let name = s.slot_name.as_str();
                let active = match &s.parent_slot {
                    Some(p) => s.slot_num.unwrap_or(0.0) <= abyssal_count(p),
                    None => true,
                };
                match s.weapon_set {
                    Some(1) => active && !second,
                    Some(_) => active && second,
                    None if name == "Graft 1" || name == "Graft 2" => version.contains("3_27"),
                    None if name == "Ring 3" => ring3,
                    None => active,
                }
            })
            .map(|s| s.slot_name.clone())
            .collect()
    }

    /// `gemList`: the gems the gem picker lists
    pub fn gem_list(&self) -> J {
        let mut gems: Vec<_> = self
            .data
            .game
            .skills
            .gems()
            .iter()
            .filter(|g| !g.granted_effect.unsupported)
            .collect();
        gems.sort_by(|a, b| a.name.cmp(&b.name));
        J::Array(
            gems.iter()
                .map(|g| {
                    let mut o = Map::new();
                    o.insert("name".into(), json!(g.name));
                    o.insert("support".into(), J::Bool(g.granted_effect.support));
                    if let Some(c) = g.granted_effect.color {
                        o.insert("color".into(), num(c));
                    }
                    if let Some(t) = &g.tag_string {
                        o.insert("tags".into(), json!(t));
                    }
                    J::Object(o)
                })
                .collect(),
        )
    }

    /// `setMainSocketGroup`
    pub fn set_main_socket_group(&mut self, index: f64) -> Result<J> {
        self.group_mut(index)?;
        self.build.info.main_socket_group = index;
        self.rebuild()?;
        Ok(json!([]))
    }

    /// `setSocketGroup`
    pub fn set_socket_group(&mut self, index: f64, e: GroupEdit) -> Result<J> {
        let g = self.group_mut(index)?;
        if let Some(l) = e.label {
            g.label = Some(l);
        }
        if let Some(v) = e.enabled {
            g.enabled = v;
        }
        if let Some(v) = e.include_in_full_dps {
            g.include_in_full_dps = Some(v);
        }
        if let Some(s) = e.slot {
            g.slot = (!s.is_empty()).then_some(s);
        }
        self.group_changed(index)?;
        Ok(json!([]))
    }

    /// `addSocketGroup`
    pub fn add_socket_group(&mut self, label: Option<String>) -> Result<J> {
        let list = self
            .skills_tab()?
            .socket_group_list_mut()
            .ok_or("no skill set")?;
        list.push(SocketGroup {
            label: Some(label.unwrap_or_default()),
            enabled: true,
            ..SocketGroup::default()
        });
        let n = list.len() as f64;
        self.group_changed(n)?;
        Ok(json!({ "index": num(n) }))
    }

    /// `removeSocketGroup`
    pub fn remove_socket_group(&mut self, index: f64) -> Result<J> {
        self.group_mut(index)?;
        let list = self.skills_tab()?.socket_group_list_mut().unwrap();
        list.remove(index as usize - 1);
        let main = &mut self.build.info.main_socket_group;
        if *main >= index && *main > 1.0 {
            *main -= 1.0;
        }
        self.rebuild()?;
        Ok(json!([]))
    }

    /// `setGem`: a gem changed, or a new one added
    pub fn set_gem(&mut self, group: f64, gem: f64, e: GemEdit) -> Result<J> {
        let g = self.group_mut(group)?;
        let k = (gem as usize).checked_sub(1);
        let i = match k.filter(|&k| k < g.gem_list.len()) {
            Some(k) => k,
            None => {
                g.gem_list.push(GemInstance {
                    level: Some(20.0),
                    quality: Some(0.0),
                    enabled: true,
                    enable_global1: true,
                    enable_global2: true,
                    count: Some(1.0),
                    new: true,
                    quality_id: Some("Default".into()),
                    ..GemInstance::default()
                });
                g.gem_list.len() - 1
            }
        };
        let gi = &mut g.gem_list[i];
        if let Some(n) = e.name {
            gi.name_spec = n;
            gi.gem_id = None;
            gi.skill_id = None;
        }
        if let Some(v) = e.level {
            gi.level = Some(v);
        }
        if let Some(v) = e.quality {
            gi.quality = Some(v);
        }
        if let Some(v) = e.quality_id {
            gi.quality_id = Some(v);
        }
        if let Some(v) = e.enabled {
            gi.enabled = v;
        }
        if let Some(v) = e.count {
            gi.count = Some(v);
        }
        self.group_changed(group)?;
        Ok(json!([]))
    }

    /// `removeGem`
    pub fn remove_gem(&mut self, group: f64, gem: f64) -> Result<J> {
        let g = self.group_mut(group)?;
        let k = (gem as usize)
            .checked_sub(1)
            .filter(|&k| k < g.gem_list.len())
            .ok_or("position out of bounds")?;
        g.gem_list.remove(k);
        self.group_changed(group)?;
        Ok(json!([]))
    }

    /// `selectSkillSet`
    pub fn select_skill_set(&mut self, id: Option<f64>) -> Result<J> {
        let data = &self.data.game.skills;
        let tab = self.build.skills.as_mut().ok_or("no skills tab")?;
        let id = id.unwrap_or(tab.active_skill_set_id);
        tab.set_active_skill_set(id, data);
        self.rebuild()?;
        Ok(json!([]))
    }
}

/// `displayLabel` as the sidebar's calculation makes it
pub(super) fn display_label(g: &SocketGroup) -> String {
    if let Some(l) = &g.label
        && engine::pat::is_match(l, "%S")
    {
        return l.clone();
    }
    let names: Vec<&str> = g
        .gem_list
        .iter()
        .filter(|gem| gem.enabled)
        .filter_map(|gem| gem.effect())
        .filter(|e| !e.support)
        .map(|e| e.name.as_str())
        .collect();
    if names.is_empty() {
        "<No active skills>".into()
    } else {
        names.join(", ")
    }
}

/// Which skill selection a control belongs to
#[derive(Clone, Copy, PartialEq)]
enum Select {
    Sidebar,
    Calcs,
}

impl TypedSession<'_> {
    /// `control` for the skill selection controls of the sidebar
    /// (`build.*`) and the Calcs tab (`calcs.*`): their handlers, as a
    /// selection or a text calls them
    pub fn control(&mut self, req: &J) -> Result<J> {
        let path = req["path"].as_str().ok_or("no control path")?;
        let (host, name) = path.split_once('.').ok_or("no control")?;
        let which = match host {
            "build" => Select::Sidebar,
            "calcs" => Select::Calcs,
            _ => return Err(format!("{}: control {path}", super::NOT_PORTED)),
        };
        let sel = req.get("sel").and_then(J::as_f64);
        let text = req.get("text").and_then(J::as_str);
        let c = match which {
            Select::Sidebar => &self.controls.skill_select,
            Select::Calcs => &self.calcs_select,
        };
        // `DropDownControl:SetSel`: clamped, and only a change calls it
        let pick = |d: &super::sidebar::Drop| -> Option<f64> {
            let s = sel?.min(d.list.len() as f64).max(1.0);
            (s != d.sel && !d.list.is_empty()).then_some(s)
        };
        let calcs = which == Select::Calcs;
        match (name, sel, text) {
            ("mainSocketGroup", Some(_), _) => {
                if let Some(i) = pick(&c.socket_group) {
                    match which {
                        Select::Sidebar => self.build.info.main_socket_group = i,
                        Select::Calcs => self.build.calcs.skill_number = i,
                    }
                    self.rebuild()?;
                }
            }
            ("mainSkill", Some(_), _) => {
                if let Some(i) = pick(&c.skill) {
                    let g = self.selected_group(calcs)?;
                    let g = self.group_mut(g)?;
                    if calcs {
                        g.main_active_skill_calcs = Some(i);
                    } else {
                        g.main_active_skill = Some(i);
                    }
                    self.rebuild()?;
                }
            }
            ("mainSkillPart" | "mainSkillMinionSkill" | "mainSkillMinion", Some(_), _) => {
                let d = match name {
                    "mainSkillPart" => &c.part,
                    "mainSkillMinion" => &c.minion,
                    _ => &c.minion_skill,
                };
                if let Some(i) = pick(d) {
                    let entry = c.minion_entry(i as usize - 1);
                    let gem = self.selected_gem(calcs)?;
                    match name {
                        "mainSkillPart" => set(
                            calcs,
                            &mut gem.skill_part_calcs,
                            &mut gem.skill_part,
                            Some(i),
                        ),
                        "mainSkillMinionSkill" => set(
                            calcs,
                            &mut gem.skill_minion_skill_calcs,
                            &mut gem.skill_minion_skill,
                            Some(i),
                        ),
                        _ => match entry {
                            Some(super::sidebar::MinionEntry::ItemSet(id)) => set(
                                calcs,
                                &mut gem.skill_minion_item_set_calcs,
                                &mut gem.skill_minion_item_set,
                                Some(id),
                            ),
                            Some(super::sidebar::MinionEntry::Minion(m)) => {
                                let m = Some(m);
                                if calcs {
                                    gem.skill_minion_calcs = m;
                                } else {
                                    gem.skill_minion = m;
                                }
                            }
                            _ => {
                                if calcs {
                                    gem.skill_minion_calcs = None;
                                } else {
                                    gem.skill_minion = None;
                                }
                            }
                        },
                    }
                    self.rebuild()?;
                }
            }
            ("mainSkillStageCount" | "mainSkillMineCount", _, Some(t)) => {
                let n = engine::pat::to_number(t);
                let gem = self.selected_gem(calcs)?;
                if name == "mainSkillStageCount" {
                    set(
                        calcs,
                        &mut gem.skill_stage_count_calcs,
                        &mut gem.skill_stage_count,
                        n,
                    );
                } else {
                    set(
                        calcs,
                        &mut gem.skill_mine_count_calcs,
                        &mut gem.skill_mine_count,
                        n,
                    );
                }
                self.rebuild()?;
            }
            ("mode", Some(s), _) if calcs => {
                let i = (s.clamp(1.0, 4.0) as usize) - 1;
                let mode = super::calcs::BUFF_MODES[i].1;
                if self.build.calcs.buff_mode != mode {
                    self.build.calcs.buff_mode = mode.into();
                    self.rebuild()?;
                }
            }
            ("showMinion", _, _) if calcs && req.get("state").is_some() => {
                self.build.calcs.show_minion = req["state"].as_bool().unwrap_or(false);
                self.ui.unsaved = true;
            }
            _ => return Err(format!("{}: control {path}", super::NOT_PORTED)),
        }
        Ok(json!([]))
    }

    /// The socket group the selection shows, by index from 1
    fn selected_group(&self, calcs: bool) -> Result<f64> {
        Ok(if calcs {
            self.build.calcs.skill_number
        } else {
            self.build.info.main_socket_group
        })
    }

    /// The gem of the selected active skill
    /// (`displaySkillList[mainActiveSkill].activeEffect.srcInstance`)
    fn selected_gem(&mut self, calcs: bool) -> Result<&mut GemInstance> {
        let gi = self.selected_group(calcs)?;
        let sb = if calcs {
            self.calcs
                .as_ref()
                .and_then(|o| o.view.as_ref())
                .map(|v| &v.skills)
        } else {
            self.main.sidebar.as_ref()
        }
        .ok_or("no calculation")?;
        let group = self
            .groups()
            .get((gi as usize).wrapping_sub(1))
            .ok_or("no socket group")?;
        let active = if calcs {
            group.main_active_skill_calcs
        } else {
            group.main_active_skill
        }
        .unwrap_or(1.0);
        let src = sb
            .groups
            .get(gi as usize - 1)
            .and_then(|l| l.get((active as usize).wrapping_sub(1)))
            .and_then(|d| d.src)
            .ok_or("attempt to index a nil value (srcInstance)")?;
        let list = self
            .skills_tab()?
            .socket_group_list_mut()
            .ok_or("no skill set")?;
        list.get_mut(src.0)
            .and_then(|g| g.gem_list.get_mut(src.1))
            .ok_or_else(|| "no gem".into())
    }
}

/// Sets the field of the selection's suffix
fn set<T>(calcs: bool, c: &mut Option<T>, s: &mut Option<T>, v: Option<T>) {
    if calcs {
        *c = v;
    } else {
        *s = v;
    }
}
