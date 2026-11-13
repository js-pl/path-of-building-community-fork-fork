//! Switching sets and specs, copying socket groups and the character
//! level (`selectTreeSpec`, `selectConfigSet`, `copySocketGroup`,
//! `pasteSocketGroup` and the level's `control` of `lua/session.lua`):
//! `TreeTab:SetActiveSpec`, `ConfigTab:SetActiveConfigSet`,
//! `SkillsTab:PasteSocketGroup` and the level box's change function.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedSession};
use engine::pat;
use engine::skills::{GemInstance, SocketGroup, process_socket_group};
use serde_json::{Value as J, json};

impl TypedSession<'_> {
    /// `selectTreeSpec`: `TreeTab:SetActiveSpec(index)`
    pub fn select_tree_spec(&mut self, index: f64) -> Result<J> {
        let n = self.other_specs.len().max(1);
        let to = (index.min(n as f64)) as usize;
        if to == 0 || index.fract() != 0.0 {
            return Err(format!("{NOT_PORTED}: spec {}", pat::num(index)));
        }
        let from = self.specs.0;
        if to != from {
            let next = self.other_specs[to - 1].clone().ok_or("no spec")?;
            let mut prev = std::mem::replace(&mut self.build.spec, next);
            // The sockets' jewels stay with the spec they were in
            let prev_mut = std::sync::Arc::make_mut(&mut prev);
            for s in &self.build.items.ordered_slots {
                if let Some(node) = s.node_id {
                    prev_mut.jewels.insert(node, s.sel_item_id);
                }
            }
            self.other_specs[from - 1] = Some(prev);
            self.other_specs[to - 1] = None;
            self.specs.0 = to;
        }
        if !self.build.items.item_order_list.is_empty() {
            self.populate_slots();
        }
        self.rebuild()?;
        Ok(json!([]))
    }

    /// `selectConfigSet`: `ConfigTab:SetActiveConfigSet(id)`
    pub fn select_config_set(&mut self, id: f64) -> Result<J> {
        let config = std::sync::Arc::make_mut(&mut self.build.config);
        let id = if id.fract() == 0.0 && config.config_sets.contains_key(&(id as i64)) {
            id
        } else {
            *config
                .config_set_order_list
                .first()
                .ok_or("no config set")?
        };
        config.active_config_set_id = id;
        self.update_config_controls();
        self.build
            .build_config_mods(&self.data.game.config, &self.data.parser)?;
        self.config_mods_built();
        self.rebuild()?;
        Ok(json!([]))
    }

    /// The level box: `characterLevel = m_min(m_max(tonumber(buf) or 1, 1), 100)`
    pub(super) fn set_character_level(&mut self, text: &str) -> Result<()> {
        let level = pat::to_number(text).unwrap_or(1.0).clamp(1.0, 100.0);
        self.build.info.character_level = level;
        self.build.info.character_level_auto_mode = false;
        self.build
            .build_config_mods(&self.data.game.config, &self.data.parser)?;
        self.config_mods_built();
        self.controls.level.text = text.to_string();
        self.controls.level_typed = true;
        self.rebuild()
    }

    /// `copySocketGroup`: the text the skills tab copies
    pub fn copy_socket_group(&self, index: f64) -> Result<J> {
        let g = self
            .groups()
            .get((index as usize).wrapping_sub(1))
            .filter(|_| index.fract() == 0.0)
            .ok_or_else(|| format!("no socket group {}", pat::num(index)))?;
        let mut text = String::new();
        if let Some(l) = g.label.as_deref().filter(|l| pat::is_match(l, "%S")) {
            text.push_str(&format!("Label: {l}\r\n"));
        }
        if let Some(s) = &g.slot {
            text.push_str(&format!("Slot: {s}\r\n"));
        }
        for gem in &g.gem_list {
            // `%d` of nil fails in the translation too; `%s` writes "nil"
            let (Some(level), Some(quality)) = (gem.level, gem.quality) else {
                return Err(format!(
                    "{NOT_PORTED}: copying a gem without a level or quality"
                ));
            };
            let qid = gem.quality_id.as_deref().unwrap_or("nil");
            text.push_str(&format!(
                "{} {}/{} {} {} {}\r\n",
                gem.name_spec,
                level.trunc() as i64,
                quality.trunc() as i64,
                qid,
                if gem.enabled { "" } else { "DISABLED" },
                gem.count.unwrap_or(1.0).trunc() as i64,
            ));
        }
        Ok(json!({ "text": text }))
    }

    /// `pasteSocketGroup`: `SkillsTab:PasteSocketGroup(text)`
    pub fn paste_socket_group(&mut self, text: &str) -> Result<J> {
        let text = engine::item::sanitise_text(text);
        let mut group = SocketGroup {
            label: Some(pat::capture(&text, "Label: (%C+)").unwrap_or_default()),
            enabled: true,
            slot: pat::capture(&text, "Slot: (%C+)"),
            ..SocketGroup::default()
        };
        for c in pat::gmatch(&text, "([ %a']+) (%d+)/(%d+) ?(%a*) (%d+)") {
            let n = |s: &str| pat::to_number(s);
            group.gem_list.push(GemInstance {
                name_spec: c[0].clone(),
                level: Some(n(&c[1]).unwrap_or(20.0)),
                quality: Some(n(&c[2]).unwrap_or(0.0)),
                enabled: c[3] != "DISABLED",
                count: Some(n(&c[4]).unwrap_or(1.0)),
                enable_global1: true,
                enable_global2: true,
                ..GemInstance::default()
            });
        }
        if !group.gem_list.is_empty() {
            process_socket_group(&mut group, &self.data.game.skills);
            self.skills_tab()?
                .socket_group_list_mut()
                .ok_or("no skill set")?
                .push(group);
        }
        self.rebuild()?;
        Ok(json!([]))
    }
}
