//! The build's loadouts drop-down (`buildMode.controls.buildLoadouts`):
//! `buildMode:SyncLoadouts()`, which upstream calls whenever a set or spec
//! is switched, renamed, added or deleted, lists the loadouts and selects
//! the one of the active spec; a change of the selection activates the
//! spec, item, skill and config sets of that loadout (the drop-down's
//! `selFunc`), which sync the loadouts again. The calculation runs once
//! after all of it, as upstream's next frame does.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_list::Loadouts;
use super::{Result, TypedSession};

const META: [&str; 5] = [
    "^7^7Loadouts:",
    "^7^7-----",
    "^7^7New Loadout",
    "^7^7Sync",
    "^7^7Help >>",
];

/// The drop-down's state
#[derive(Clone, Debug)]
pub(super) struct LoadoutState {
    /// `selIndex`, 1-based
    pub sel: usize,
    /// The loadouts and the special links the last sync found
    pub synced: Loadouts,
    /// Nested switches wait for one calculation
    pub depth: u32,
    pub rebuild_pending: bool,
}

impl Default for LoadoutState {
    fn default() -> Self {
        LoadoutState {
            sel: 1,
            synced: Loadouts::default(),
            depth: 0,
            rebuild_pending: false,
        }
    }
}

/// `string.find(s, pattern, 1, true)`
fn plain_find(s: &str, p: &str) -> bool {
    s.contains(p)
}

impl<'d> TypedSession<'d> {
    /// The drop-down's list: the loadouts between upstream's entries
    pub(super) fn loadout_list(&self) -> Vec<String> {
        let mut list = vec![META[0].to_string()];
        list.extend(self.loadout.synced.list.iter().cloned());
        list.extend(META[1..].iter().map(|s| s.to_string()));
        list
    }

    fn item_set_count(&self) -> usize {
        self.build.items.item_set_order_list.len()
    }

    fn skill_set_count(&self) -> usize {
        self.build
            .skills
            .as_ref()
            .map_or(0, |s| s.skill_set_order_list.len())
    }

    fn config_set_count(&self) -> usize {
        self.build.config.config_set_order_list.len()
    }

    /// `buildMode:SyncLoadouts()`, its switches calculated once
    pub(super) fn sync_loadouts(&mut self) -> Result<()> {
        self.loadout.depth += 1;
        let r = self.sync_loadouts_inner();
        self.loadout.depth -= 1;
        r
    }

    /// What a request that synced the loadouts calculates afterwards
    pub(super) fn sync_loadouts_and_rebuild(&mut self) -> Result<()> {
        self.sync_loadouts()?;
        if self.loadout.rebuild_pending {
            self.rebuild()?;
        }
        Ok(())
    }

    fn sync_loadouts_inner(&mut self) -> Result<()> {
        self.loadout.synced = self.compute_loadouts();
        let list = self.loadout_list();
        // The loadout of the active spec, when its sets match
        let spec_title = self
            .specs
            .1
            .get(self.specs.0.wrapping_sub(1))
            .cloned()
            .unwrap_or_else(|| "Default".into());
        let (one_item, one_skill, one_config) = (
            self.item_set_count() == 1,
            self.skill_set_count() == 1,
            self.config_set_count() == 1,
        );
        if let Some(i) = list.iter().position(|l| *l == spec_title) {
            let link = engine::pat::capture(&spec_title, "%{(%w+)%}")
                .unwrap_or_else(|| spec_title.clone());
            let title = |t: Option<String>| t.unwrap_or_else(|| "Default".into());
            let item = title(
                self.build
                    .items
                    .item_sets
                    .get(&(self.build.items.active_item_set_id as i64))
                    .and_then(|s| s.title.clone()),
            );
            let skill = title(self.build.skills.as_ref().and_then(|s| {
                s.skill_sets
                    .get(&(s.active_skill_set_id as i64))
                    .and_then(|s| s.title.clone())
            }));
            let config = title(
                self.build
                    .config
                    .config_sets
                    .get(&(self.build.config.active_config_set_id as i64))
                    .and_then(|s| s.title.clone()),
            );
            if (one_skill || plain_find(&skill, &link))
                && (one_item || plain_find(&item, &link))
                && (one_config || plain_find(&config, &link))
            {
                return self.loadout_set_sel(i + 1);
            }
        }
        self.loadout_set_sel(1)
    }

    /// `control` of the drop-down: `SetSel(sel)`, then one calculation
    pub(super) fn select_loadout(&mut self, sel: f64) -> Result<()> {
        let n = self.loadout_list().len();
        let sel = sel.min(n as f64).max(1.0) as usize;
        self.loadout.depth += 1;
        let r = self.loadout_set_sel(sel);
        self.loadout.depth -= 1;
        r?;
        if self.loadout.rebuild_pending {
            self.rebuild()?;
        }
        Ok(())
    }

    /// `buildLoadouts:SetSel(sel)`: its `selFunc` on a change
    fn loadout_set_sel(&mut self, sel: usize) -> Result<()> {
        let list = self.loadout_list();
        let sel = sel.clamp(1, list.len());
        if sel == self.loadout.sel {
            return Ok(());
        }
        self.loadout.sel = sel;
        let value = list[sel - 1].clone();
        self.loadout_selected(&value)
    }

    /// The drop-down's `selFunc(index, value)`
    fn loadout_selected(&mut self, value: &str) -> Result<()> {
        if value == META[0] || value == META[1] {
            return self.loadout_set_sel(1);
        }
        if value == META[3] {
            self.sync_loadouts()?;
            return self.loadout_set_sel(1);
        }
        if value == META[2] {
            // New Loadout: its name popup, the drop-down back to its title
            self.open_new_loadout();
            return self.loadout_set_sel(1);
        }
        if META.contains(&value) {
            return Err(format!(
                "{}: the loadouts' {value} entry",
                super::NOT_PORTED
            ));
        }
        let link = engine::pat::capture(value, "%{(%w+)%}");
        let missing = || "attempt to index a nil value".to_string();
        let synced = self.loadout.synced.clone();
        // `findNamedSetId(treeTab:GetSpecList(), value, treeListSpecialLinks)`
        let latest = self.data.game.latest_tree_version().to_string();
        let mut spec_id = None;
        for i in 1..=self.specs.1.len() {
            let version = if i == self.specs.0 || self.other_specs.is_empty() {
                self.build.spec.tree.version.to_string()
            } else {
                self.other_specs
                    .get(i - 1)
                    .and_then(|s| s.as_ref())
                    .map_or_else(String::new, |s| s.tree.version.to_string())
            };
            let name = if version != latest {
                format!("[{}] {}", version_display(&version), self.specs.1[i - 1])
            } else {
                self.specs.1[i - 1].clone()
            };
            if value == name {
                spec_id = Some(i);
                break;
            }
            if let Some(l) = &link {
                let id = synced
                    .tree_links
                    .iter()
                    .find(|(x, _)| x == l)
                    .ok_or_else(missing)?;
                spec_id = Some(id.1);
                break;
            }
        }
        // `findSetId(setOrderList, value, sets, setSpecialLinks)`
        let find = |order: &[(f64, String)], links: &[(String, f64)]| -> Result<Option<f64>> {
            for (id, title) in order {
                if value == title {
                    return Ok(Some(*id));
                }
                if let Some(l) = &link {
                    let id = links.iter().find(|(x, _)| x == l).ok_or_else(missing)?;
                    return Ok(Some(id.1));
                }
            }
            Ok(None)
        };
        let titled = |order: &[f64], title: &dyn Fn(f64) -> Option<String>| -> Vec<(f64, String)> {
            order
                .iter()
                .map(|id| (*id, title(*id).unwrap_or_else(|| "Default".into())))
                .collect()
        };
        let items = titled(&self.build.items.item_set_order_list, &|id| {
            self.build
                .items
                .item_sets
                .get(&(id as i64))
                .and_then(|s| s.title.clone())
        });
        let skill_order = self
            .build
            .skills
            .as_ref()
            .map_or(vec![], |s| s.skill_set_order_list.clone());
        let skills = titled(&skill_order, &|id| {
            self.build
                .skills
                .as_ref()
                .and_then(|s| s.skill_sets.get(&(id as i64)))
                .and_then(|s| s.title.clone())
        });
        let configs = titled(&self.build.config.config_set_order_list, &|id| {
            self.build
                .config
                .config_sets
                .get(&(id as i64))
                .and_then(|s| s.title.clone())
        });
        // `oneItem and 1 or ...`: the first id, whatever the set's
        let item_id = if self.item_set_count() == 1 {
            Some(1.0)
        } else {
            find(&items, &synced.item_links)?
        };
        let skill_id = if self.skill_set_count() == 1 {
            Some(1.0)
        } else {
            find(&skills, &synced.skill_links)?
        };
        let config_id = if self.config_set_count() == 1 {
            Some(1.0)
        } else {
            find(&configs, &synced.config_links)?
        };
        let (Some(spec_id), Some(item_id), Some(skill_id), Some(config_id)) =
            (spec_id, item_id, skill_id, config_id)
        else {
            return Ok(());
        };
        if spec_id != self.specs.0 {
            self.select_tree_spec(spec_id as f64)?;
        }
        if item_id != self.build.items.active_item_set_id {
            self.select_item_set(item_id)?;
        }
        let active_skill = self.build.skills.as_ref().map(|s| s.active_skill_set_id);
        if active_skill.is_some_and(|a| a != skill_id) {
            self.select_skill_set(Some(skill_id))?;
        }
        if config_id != self.build.config.active_config_set_id {
            self.select_config_set(config_id)?;
        }
        // `SelByValue(value)` in the list as it is now
        if let Some(i) = self.loadout_list().iter().position(|l| l == value) {
            self.loadout.sel = i + 1;
        }
        Ok(())
    }
}

/// `treeVersions[v].display`
fn version_display(v: &str) -> String {
    let parts: Vec<&str> = v.split('_').collect();
    let base = parts.iter().take(2).copied().collect::<Vec<_>>().join(".");
    if parts.len() > 2 {
        format!("{base} ({})", parts[2..].join(" "))
    } else {
        base
    }
}
