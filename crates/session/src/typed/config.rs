//! The Configuration tab (`Classes/ConfigTab.lua`): its option controls,
//! which options show (`ifCond`, `ifSkill`, ... on the sidebar's
//! calculation, `ConfigVisibility.lua`'s rules as the tab applies them),
//! their tooltips, the `config` command and `control` of the options; and
//! the class, ascendancy and bloodline drop-downs of the build mode with
//! the class change popup.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{Result, TypedSession, num};
use engine::config::Val;
use engine::config_tab::ConfigVar;
use serde_json::{Map, Value as J, json};
use std::collections::HashMap;

/// What the option controls hold besides the config set: the check
/// boxes' states, the edit boxes' texts and placeholders, the drop-downs'
/// selections
#[derive(Clone, Debug, Default)]
pub(super) struct ConfigControls {
    state: HashMap<String, bool>,
    buf: HashMap<String, String>,
    placeholder: HashMap<String, J>,
    sel: HashMap<String, f64>,
    /// `enemyDamageType.enabled`, which a boss skill preset clears
    damage_type_enabled: bool,
}

fn val_json(v: &Val) -> J {
    match v {
        Val::Bool(b) => J::Bool(*b),
        Val::Number(n) => num(*n),
        Val::Str(s) => J::from(&**s),
    }
}

/// `tostring(v)`
fn val_text(v: &Val) -> String {
    match v {
        Val::Number(n) => lua::value::format_number(*n),
        v => v.text(),
    }
}

/// Lua's `a == b` on option values
fn val_eq(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Number(x), Val::Number(y)) => x == y,
        (Val::Str(x), Val::Str(y)) => x == y,
        _ => false,
    }
}

fn is_edit(kind: &str) -> bool {
    matches!(
        kind,
        "count" | "integer" | "countAllowZero" | "float" | "text"
    )
}

impl ConfigControls {
    /// The controls as `ConfigTab` makes them
    pub(super) fn new(vars: &[ConfigVar]) -> ConfigControls {
        let mut c = ConfigControls {
            damage_type_enabled: true,
            ..Default::default()
        };
        for v in vars {
            let (Some(var), Some(kind)) = (&v.var, &v.kind) else {
                continue;
            };
            match kind.as_str() {
                "check" => {
                    c.state.insert(
                        var.clone(),
                        v.default_state.as_ref().is_some_and(Val::truthy),
                    );
                }
                k if is_edit(k) => {
                    c.buf.insert(var.clone(), String::new());
                    if let Some(p) = &v.default_placeholder {
                        c.placeholder.insert(var.clone(), val_json(p));
                    }
                }
                "list" => {
                    c.sel.insert(var.clone(), v.default_index.unwrap_or(1.0));
                }
                _ => {}
            }
        }
        c
    }
}

impl TypedSession<'_> {
    fn config_vars(&self) -> &[ConfigVar] {
        &self.data.game.config_vars
    }

    fn config_var(&self, var: &str) -> Option<&ConfigVar> {
        self.config_vars()
            .iter()
            .rev()
            .find(|v| v.var.as_deref() == Some(var))
    }

    /// `ConfigTab:UpdateControls()`: the controls show the active set
    pub(super) fn update_config_controls(&mut self) {
        let set = self.build.config.active();
        let mut c = std::mem::take(&mut self.config_controls);
        for v in &self.data.game.config_vars {
            let (Some(var), Some(kind)) = (&v.var, &v.kind) else {
                continue;
            };
            let input = set.input.get(var);
            match kind.as_str() {
                "check" => {
                    c.state.insert(var.clone(), input.is_some_and(Val::truthy));
                }
                k if is_edit(k) => {
                    let text = input
                        .filter(|v| v.truthy())
                        .map_or_else(String::new, val_text);
                    c.buf.insert(var.clone(), text);
                    if let Some(p) = set.placeholder.get(var).filter(|p| p.truthy()) {
                        c.placeholder.insert(var.clone(), J::from(val_text(p)));
                    }
                }
                "list" => {
                    let value = input
                        .filter(|v| v.truthy())
                        .cloned()
                        .or_else(|| self.default_state(var, None));
                    if let Some(value) = value
                        && let Some(i) = v
                            .list
                            .iter()
                            .position(|(_, x)| x.as_ref().is_some_and(|x| val_eq(x, &value)))
                    {
                        c.sel.insert(var.clone(), (i + 1) as f64);
                    }
                }
                _ => {}
            }
        }
        self.config_controls = c;
    }

    /// What `BuildModList` did to the controls: the placeholders the
    /// options' functions set, the enemy radius a size preset shows and
    /// the damage type a boss skill preset locks
    pub(super) fn config_mods_built(&mut self) {
        let set = self.build.config.active();
        let state = &self.build.config_state;
        let c = &mut self.config_controls;
        for var in &state.placeholders_set {
            let text = set.placeholder.get(var).map_or_else(String::new, val_text);
            c.placeholder.insert(var.clone(), J::from(text));
        }
        let radius = match set.input.get("enemySizePreset") {
            Some(v) if v.is_str("Small") => Some("2"),
            Some(v) if v.is_str("Medium") => Some("3"),
            Some(v) if v.is_str("Large") => Some("5"),
            Some(v) if v.is_str("Huge") => Some("11"),
            _ => None,
        };
        if let Some(r) = radius {
            c.placeholder.insert("enemyRadius".into(), J::from(r));
        }
        let locked = state.damage_type_locked;
        if locked && let Some(v) = set.input.get("enemyDamageType") {
            let list = self
                .data
                .game
                .config_vars
                .iter()
                .find(|x| x.var.as_deref() == Some("enemyDamageType"));
            if let Some(i) = list.and_then(|l| {
                l.list
                    .iter()
                    .position(|(_, x)| x.as_ref().is_some_and(|x| val_eq(x, v)))
            }) {
                c.sel.insert("enemyDamageType".into(), (i + 1) as f64);
            }
        }
        c.damage_type_enabled = !locked;
    }

    /// `ConfigTab:GetDefaultState(var, varType)`
    fn default_state(&self, var: &str, typ: Option<&Val>) -> Option<Val> {
        let set = self.build.config.active();
        if let Some(p) = set.placeholder.get(var) {
            return Some(p.clone());
        }
        if let Some(d) = self
            .data
            .game
            .option_defaults
            .iter()
            .rev()
            .find(|d| d.var == var)
            .and_then(|d| d.saved.clone())
        {
            return Some(d);
        }
        match typ? {
            Val::Number(_) => Some(Val::Number(0.0)),
            Val::Bool(_) => Some(Val::Bool(false)),
            Val::Str(_) => Some(Val::Str("".into())),
        }
    }

    /// Whether the option's value differs from its default
    fn config_changed(&self, var: &str) -> bool {
        let Some(cur) = self.build.config.active().input.get(var) else {
            return false;
        };
        self.default_state(var, Some(cur))
            .is_none_or(|d| !val_eq(cur, &d))
    }

    /// Whether the option's rules pass on the sidebar's calculation (the
    /// control's `shownFuncs`)
    fn config_rules_pass(&self, v: &ConfigVar) -> Result<bool> {
        let used = &self.sidebar()?.used;
        let main = &self.sidebar()?.main_skill;
        let input = &self.build.config.active().input;
        let truthy = |k: &str| input.get(k).is_some_and(Val::truthy);
        let implied = || {
            let Some(var) = &v.var else {
                return false;
            };
            truthy(var)
                && (v
                    .imply_cond_list
                    .iter()
                    .any(|c| used.conditions.contains(c))
                    || v.imply_cond
                        .as_ref()
                        .is_some_and(|c| used.conditions.contains(c))
                    || v.imply_minion_cond
                        .as_ref()
                        .is_some_and(|c| used.minion_conditions.contains(c))
                    || v.imply_enemy_cond
                        .as_ref()
                        .is_some_and(|c| used.enemy_conditions.contains(c)))
        };
        let skills = &self.data.game.skills;
        for (rule, names) in &v.rules {
            let one = |n: &String| -> Result<bool> {
                let set = match rule.as_str() {
                    "ifOption" => return Ok(truthy(n)),
                    "ifCondTrue" => return Ok(used.player_conditions.contains(n)),
                    "ifFlag" => {
                        return Ok(main.flag(n) || used.main_skill_mod_flags.contains(n));
                    }
                    "ifSkill" if v.include_transfigured => {
                        return Ok(skills.game_id_from_gem_name(n, true).is_some()
                            && used
                                .skills
                                .iter()
                                .any(|s| skills.is_gem_id_same(s, n, true)));
                    }
                    "ifSkill" => return Ok(used.skills.contains(n)),
                    "ifSkillFlag" => return Ok(used.skill_flags.contains(n)),
                    "ifSkillData" => return Ok(used.skill_data.contains(n)),
                    "ifStat" => {
                        return Ok(implied()
                            || used.per_stats.contains(n)
                            || used.enemy_multipliers.contains(n));
                    }
                    "ifCond" => &used.conditions,
                    "ifMinionCond" => &used.minion_conditions,
                    "ifEnemyCond" => &used.enemy_conditions,
                    "ifMult" => &used.multipliers,
                    "ifEnemyMult" => &used.enemy_multipliers,
                    "ifEnemyStat" => &used.enemy_per_stats,
                    "ifTagType" => &used.tag_types,
                    "ifMod" => &used.mods,
                    r => return Err(format!("{}: the config rule {r}", super::NOT_PORTED)),
                };
                Ok(implied() || set.contains(n))
            };
            let mut pass = false;
            for n in names {
                if one(n)? {
                    pass = true;
                    break;
                }
            }
            if !pass {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The option control's `shown`
    fn config_shown(&self, v: &ConfigVar) -> Result<bool> {
        if !self.config_search_match(v) {
            return Ok(false);
        }
        // `isShowAllConfig`: Show All Configurations overrides the rules
        let show_all = self.config_ui.show_all && !show_all_excluded(v);
        let inner = self.config_rules_pass(v)? || show_all;
        Ok(if v.hide_if_invalid {
            inner
        } else {
            inner || v.var.as_deref().is_some_and(|var| self.config_changed(var))
        })
    }

    /// `searchMatch`: the option's label against the tab's search, its
    /// magic characters escaped (`string.matchOrPattern`)
    fn config_search_match(&self, v: &ConfigVar) -> bool {
        let search = engine::pat::gsub(
            &engine::pat::lower(&self.config_ui.search),
            "[%-%.%+%[%]%$%^%%%?%*]",
            "%%%0",
        );
        if !engine::pat::is_match(&search, "%S") {
            return true;
        }
        let label = engine::pat::lower(&strip_escapes(v.label.as_deref().unwrap_or("")));
        engine::pat::match_or_pattern(&label, &search).unwrap_or(false)
    }

    /// A `configTab.<name>` control changed as `api.control` changes it
    pub(super) fn config_tab_control(&mut self, name: &str, req: &J) -> Result<J> {
        let text = req.get("text").and_then(J::as_str);
        let click = req.get("click").and_then(J::as_bool) == Some(true);
        match (name, req.get("sel").and_then(J::as_f64), text) {
            ("search", None, Some(t)) => {
                self.config_ui.search = t.to_string();
                self.update_config_controls();
            }
            ("toggleConfigs", None, None) if click => {
                self.config_ui.show_all = !self.config_ui.show_all;
            }
            // The set drop-down gets its list when the tab draws: headless
            // it is empty, and selecting in it changes nothing
            ("setSelect", Some(_), None) => {}
            _ => return Err(format!("{}: control configTab.{name}", super::NOT_PORTED)),
        }
        Ok(json!([]))
    }

    /// The option control's `tooltipText()`
    fn config_tooltip(&self, v: &ConfigVar) -> Result<Option<String>> {
        if v.tooltip_fn {
            let out = &self.main.player;
            let get = |k: &str| {
                out.get(&engine::mods::Sym::new(k))
                    .and_then(engine::actor::OutVal::num)
                    .ok_or_else(|| format!("attempt to perform arithmetic on a nil value ({k})"))
            };
            let f = lua::value::format_number;
            return Ok(Some(match v.var.as_deref() {
                Some("conditionLowLife") => format!(
                    "You will automatically be considered to be on Low ^xE05030Life ^7if you have at least {}% ^xE05030Life ^7reserved\nbut you can use this option to force it if necessary.",
                    f(100.0 - get("LowLifePercentage")?)
                ),
                Some("conditionFullLife") => format!(
                    "You can be considered to be on Full ^xE05030Life ^7if you have at least {}% ^xE05030Life ^7left.\nYou will automatically be considered to be on Full ^xE05030Life ^7if you have Chaos Inoculation,\nbut you can use this option to force it if necessary.",
                    f(get("FullLifePercentage")?)
                ),
                o => {
                    return Err(format!(
                        "{}: the tooltip function of {o:?}",
                        super::NOT_PORTED
                    ));
                }
            }));
        }
        Ok(v.tooltip.clone())
    }

    /// `describeControl` of an option's control
    fn describe_config_control(&self, v: &ConfigVar) -> Result<J> {
        let var = v.var.as_deref().unwrap_or_default();
        let c = &self.config_controls;
        let mut d = Map::new();
        let kind = v.kind.as_deref().unwrap_or_default();
        let class = match kind {
            "check" => "CheckBoxControl",
            k if is_edit(k) && v.resizable => "ResizableEditControl",
            k if is_edit(k) => "EditControl",
            "list" => "DropDownControl",
            _ => "Control",
        };
        d.insert("class".into(), json!(class));
        d.insert("shown".into(), J::Bool(self.config_shown(v)?));
        let enabled = var != "enemyDamageType" || c.damage_type_enabled;
        d.insert("enabled".into(), J::Bool(enabled));
        match class {
            "DropDownControl" => {
                let list: Vec<&str> = v.list.iter().map(|(l, _)| l.as_str()).collect();
                d.insert("list".into(), json!(list));
                d.insert("sel".into(), num(c.sel.get(var).copied().unwrap_or(1.0)));
            }
            "EditControl" | "ResizableEditControl" => {
                d.insert(
                    "text".into(),
                    J::from(c.buf.get(var).cloned().unwrap_or_default()),
                );
                if let Some(p) = c.placeholder.get(var) {
                    d.insert("placeholder".into(), p.clone());
                }
                if kind == "text" {
                    d.insert("multiline".into(), J::Bool(true));
                }
            }
            "CheckBoxControl" => {
                d.insert(
                    "state".into(),
                    J::Bool(c.state.get(var).copied().unwrap_or(false)),
                );
            }
            _ => {}
        }
        if v.hide_if_invalid
            && class == "CheckBoxControl"
            && let Some(l) = &v.label
        {
            d.insert("label".into(), json!(l));
        }
        if let Some(t) = self.config_tooltip(v)? {
            d.insert("tooltip".into(), J::from(t));
        }
        Ok(J::Object(d))
    }

    /// `controls` of `config.<var>`
    pub(super) fn config_control_json(&self, var: &str) -> Option<J> {
        let v = self.config_var(var)?;
        self.describe_config_control(v).ok()
    }

    /// `config`: the Configuration tab's shown options by section, and the
    /// config sets
    pub fn config(&self) -> Result<J> {
        let mut sections = Vec::new();
        let mut current: Option<(&ConfigVar, Vec<J>)> = None;
        let input = &self.build.config.active().input;
        let flush = |cur: Option<(&ConfigVar, Vec<J>)>, sections: &mut Vec<J>| {
            if let Some((s, vars)) = cur
                && !vars.is_empty()
                && s.section.as_deref() != Some("Custom Modifiers")
            {
                let mut o = Map::new();
                o.insert("label".into(), json!(s.section));
                if let Some(c) = s.col {
                    o.insert("col".into(), num(c));
                }
                o.insert("vars".into(), J::Array(vars));
                sections.push(J::Object(o));
            }
        };
        for v in self.config_vars() {
            if v.section.is_some() {
                flush(current.take(), &mut sections);
                current = Some((v, Vec::new()));
                continue;
            }
            let Some(var) = v.var.as_deref() else {
                continue;
            };
            // `varControls` keeps the last control of a var
            if !std::ptr::eq(self.config_var(var).unwrap(), v) {
                continue;
            }
            let mut d = self.describe_config_control(v)?;
            if d["shown"] != J::Bool(true) {
                continue;
            }
            let o = d.as_object_mut().unwrap();
            o.insert("var".into(), json!(var));
            if let Some(x) = input.get(var) {
                o.insert("value".into(), val_json(x));
            }
            if let Some(l) = &v.label {
                o.insert("label".into(), json!(l));
            }
            if let Some((_, vars)) = &mut current {
                vars.push(d);
            }
        }
        flush(current.take(), &mut sections);
        let tab = &self.build.config;
        let sets: Vec<J> = tab
            .config_set_order_list
            .iter()
            .map(|id| {
                let title = tab
                    .config_sets
                    .get(&(*id as i64))
                    .and_then(|s| s.title.clone())
                    .unwrap_or_else(|| "Default".into());
                json!({ "id": num(*id), "title": title })
            })
            .collect();
        Ok(json!({
            "sections": sections,
            "configSets": sets,
            "activeConfigSet": num(tab.active_config_set_id),
        }))
    }

    /// `control` of an option's control: its change function, then
    /// `BuildModList` and both calculations
    pub(super) fn config_control(&mut self, var: &str, req: &J) -> Result<J> {
        let v = self
            .config_var(var)
            .cloned()
            .ok_or_else(|| format!("no control config.{var}"))?;
        let kind = v.kind.as_deref().unwrap_or_default();
        let value = if let Some(sel) = req.get("sel").and_then(J::as_f64) {
            if kind != "list" {
                return Err(format!("{}: control config.{var}", super::NOT_PORTED));
            }
            // `DropDownControl:SetSel`: clamped, and only a change calls it
            let n = v.list.len() as f64;
            let sel = sel.min(n).max(1.0);
            let cur = self.config_controls.sel.get(var).copied().unwrap_or(1.0);
            if v.list.is_empty() || sel == cur {
                return Ok(json!([]));
            }
            self.config_controls.sel.insert(var.into(), sel);
            v.list[sel as usize - 1].1.clone()
        } else if let Some(text) = req.get("text").and_then(J::as_str) {
            if !is_edit(kind) {
                return Err(format!("{}: control config.{var}", super::NOT_PORTED));
            }
            self.config_controls.buf.insert(var.into(), text.into());
            if kind == "text" {
                Some(Val::Str(text.into()))
            } else {
                engine::pat::to_number(text).map(Val::Number)
            }
        } else if let Some(state) = req.get("state").and_then(J::as_bool) {
            if kind != "check" {
                return Err(format!("{}: control config.{var}", super::NOT_PORTED));
            }
            self.config_controls.state.insert(var.into(), state);
            Some(Val::Bool(state))
        } else {
            return Err(format!("{}: control config.{var}", super::NOT_PORTED));
        };
        let config = std::sync::Arc::make_mut(&mut self.build.config);
        let input = &mut config.active_mut().input;
        match value {
            Some(x) => input.insert(var.into(), x),
            None => input.remove(var),
        };
        self.build
            .build_config_mods(&self.data.game.config, &self.data.parser)?;
        self.config_mods_built();
        self.rebuild()?;
        Ok(json!([]))
    }

    /// `control` of the class, ascendancy and bloodline drop-downs
    pub(super) fn class_control(&mut self, name: &str, sel: f64) -> Result<J> {
        // `DropDownControl:SetSel`: clamped, and only a change calls it
        let pick = |d: &super::sidebar::Drop| -> Option<usize> {
            let s = sel.min(d.list.len() as f64).max(1.0);
            (s != d.sel && !d.list.is_empty()).then_some(s as usize)
        };
        match name {
            "classDrop" => {
                let Some(i) = pick(&self.controls.class) else {
                    return Ok(json!([]));
                };
                let (id, label) = self.class_entries()[i - 1].clone();
                if Some(id) == self.build.spec.cur_class_id {
                    return Ok(json!([]));
                }
                let (used, _, _) = self.count_alloc_nodes();
                let connected = used == 0.0 || {
                    let mut spec = (*self.build.spec).clone();
                    spec.is_class_connected(id)?
                };
                if connected {
                    self.select_class(id)?;
                } else {
                    self.open_class_popup(id, &label);
                }
            }
            "ascendDrop" => {
                let Some(i) = pick(&self.controls.ascend) else {
                    return Ok(json!([]));
                };
                let class = self.build.spec.cur_class_id.ok_or("no class")?;
                let (id, _) = self.ascend_entries(class)[i - 1].clone();
                std::sync::Arc::make_mut(&mut self.build.spec).select_ascend_class(id)?;
                self.add_undo_state();
                self.rebuild()?;
            }
            "secondaryAscendDrop" => {
                let Some(i) = pick(&self.controls.secondary) else {
                    return Ok(json!([]));
                };
                let (id, _) = self.secondary_entries()[i - 1].clone();
                std::sync::Arc::make_mut(&mut self.build.spec).select_secondary_ascend_class(id)?;
                self.add_undo_state();
                self.rebuild()?;
            }
            _ => return Err(format!("{}: control build.{name}", super::NOT_PORTED)),
        }
        Ok(json!([]))
    }

    fn select_class(&mut self, id: i64) -> Result<()> {
        std::sync::Arc::make_mut(&mut self.build.spec).select_class(id)?;
        self.add_undo_state();
        self.rebuild()
    }

    /// `main:OpenConfirmPopup("Class Change", ...)` with Continue, Connect
    /// Path and Cancel; only the first has a name to press it by
    fn open_class_popup(&mut self, id: i64, label: &str) {
        let popup = json!({
            "title": "Class Change",
            "lines": [
                format!("Changing class to {label} will reset your passive tree."),
                format!("This can be avoided by connecting one of the {label} starting nodes to your tree."),
            ],
            "buttons": [{ "name": "confirm", "label": "Continue", "enabled": true }],
            "controls": [],
        });
        if let J::Array(p) = &mut self.ui.popups {
            p.insert(0, popup);
        }
        self.class_popups.insert(0, id);
    }

    /// The popups a build loaded after this one keeps open: upstream's
    /// popups belong to `main`, and the class change popup's Continue
    /// acts on whatever build is open
    pub fn keep_popups(&mut self, prev: &TypedSession) {
        self.ui.popups = prev.ui.popups.clone();
        self.class_popups = prev.class_popups.clone();
    }

    /// Whether the front popup is a class change popup
    pub fn has_class_popup(&self) -> bool {
        !self.class_popups.is_empty()
    }

    /// `popupButton`: Continue of the class change popup
    pub fn popup_button(&mut self) -> Result<J> {
        let id = *self.class_popups.first().ok_or("no popup")?;
        self.close_popup();
        self.select_class(id)?;
        Ok(json!([]))
    }

    /// `closePopup`
    pub fn close_popup(&mut self) -> J {
        if let J::Array(p) = &mut self.ui.popups
            && !p.is_empty()
        {
            p.remove(0);
            if !self.class_popups.is_empty() {
                self.class_popups.remove(0);
            }
        }
        json!([])
    }
}

/// The Configuration tab's search and Show All Configurations
#[derive(Clone, Debug, Default)]
pub(super) struct ConfigUi {
    pub search: String,
    pub show_all: bool,
}

/// `StripEscapes`: colour codes out
fn strip_escapes(s: &str) -> String {
    engine::pat::gsub(&engine::pat::gsub(s, "%^%d", ""), "%^x%x%x%x%x%x%x", "")
}

/// `configVisibility.isShowAllExcluded`: options Show All Configurations
/// leaves hidden
fn show_all_excluded(v: &ConfigVar) -> bool {
    if v.legacy
        || v.rules.iter().any(|(r, _)| {
            matches!(
                r.as_str(),
                "ifOption" | "ifSkill" | "ifSkillData" | "ifSkillFlag"
            )
        })
    {
        return true;
    }
    let label = engine::pat::lower(v.label.as_deref().unwrap_or(""));
    [
        "recently",
        "in the last",
        "in the past",
        "in last",
        "in past",
        "pvp",
    ]
    .iter()
    .any(|kw| engine::pat::find(&label, kw).is_some())
}
