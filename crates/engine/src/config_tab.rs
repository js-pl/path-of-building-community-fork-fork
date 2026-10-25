//! The Configuration tab's state (`Classes/ConfigTab.lua`) without its UI:
//! the config sets with their inputs, placeholders and custom modifier
//! blocks, as a build file loads them.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::config::{CustomBlock, Inputs, Val};
use crate::mods::convert::{array, get};
use crate::pat;
use crate::xml::Element;
use lua::Value;
use std::collections::BTreeMap;

pub type Result<T> = std::result::Result<T, String>;

/// The value a new config set gives an option (`defaultState`, the
/// `defaultIndex` entry of its list) and its placeholder
/// (`defaultPlaceholderState`)
#[derive(Clone, Debug, PartialEq)]
pub struct OptionDefault {
    pub var: String,
    pub state: Option<Val>,
    pub placeholder: Option<Val>,
}

fn val(v: &Value) -> Option<Val> {
    match v {
        Value::Bool(b) => Some(Val::Bool(*b)),
        Value::Number(n) => Some(Val::Number(*n)),
        Value::Str(s) => Some(Val::Str(s.to_str_lossy().into())),
        _ => None,
    }
}

/// The defaults of the options of `Modules/ConfigOptions.lua` (`varList`)
pub fn option_defaults(var_list: &Value) -> Vec<OptionDefault> {
    let mut out = Vec::new();
    for v in array(var_list) {
        let Value::Str(var) = get(&v, "var") else {
            continue;
        };
        let mut state = val(&get(&v, "defaultState"));
        if let Some(i) = get(&v, "defaultIndex").as_number() {
            let list = array(&get(&v, "list"));
            state = list.get(i as usize - 1).and_then(|e| val(&get(e, "val")));
        }
        out.push(OptionDefault {
            var: var.to_str_lossy().into_owned(),
            state,
            placeholder: val(&get(&v, "defaultPlaceholderState")),
        });
    }
    out
}

/// A config set: options' inputs and placeholders, and custom mods
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigSet {
    pub id: f64,
    pub title: Option<String>,
    pub input: Inputs,
    pub placeholder: Inputs,
    pub custom_mods: Vec<CustomBlock>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigTab {
    pub config_sets: BTreeMap<i64, ConfigSet>,
    pub config_set_order_list: Vec<f64>,
    pub active_config_set_id: f64,
}

fn key(id: f64) -> i64 {
    id as i64
}

fn default_block() -> CustomBlock {
    CustomBlock {
        title: Some("Default".into()),
        enabled: true,
        text: String::new(),
    }
}

impl ConfigTab {
    /// `NewConfigSet(id, title)`
    fn new_set(&mut self, id: f64, title: Option<&str>, defaults: &[OptionDefault]) {
        let mut set = ConfigSet {
            id,
            title: title.map(String::from),
            input: Inputs::new(),
            placeholder: Inputs::new(),
            custom_mods: vec![default_block()],
        };
        for d in defaults {
            if let Some(v) = &d.state {
                set.input.insert(d.var.clone(), v.clone());
            }
            if let Some(v) = &d.placeholder {
                set.placeholder.insert(d.var.clone(), v.clone());
            }
        }
        self.config_sets.insert(key(id), set);
    }

    /// A new tab: set 1, active
    pub fn new(defaults: &[OptionDefault]) -> ConfigTab {
        let mut tab = ConfigTab {
            config_sets: BTreeMap::new(),
            config_set_order_list: vec![1.0],
            active_config_set_id: 1.0,
        };
        tab.new_set(1.0, None, defaults);
        tab
    }

    pub fn active(&self) -> &ConfigSet {
        &self.config_sets[&key(self.active_config_set_id)]
    }

    pub fn active_mut(&mut self) -> &mut ConfigSet {
        self.config_sets
            .get_mut(&key(self.active_config_set_id))
            .unwrap()
    }

    /// An `<Input>` or `<Placeholder>` element of a set
    fn set_input(&mut self, node: &Element, id: f64) -> Result<()> {
        let set = self
            .config_sets
            .get_mut(&key(id))
            .ok_or_else(|| format!("no config set {id}"))?;
        let name = node.attr("name");
        match node.elem.as_str() {
            "Input" => {
                let name = name
                    .ok_or("'Input' element missing name attribute")?
                    .to_string();
                let v = if let Some(n) = node.attr("number") {
                    Val::Number(pat::to_number(n).unwrap_or(f64::NAN))
                } else if let Some(s) = node.attr("string") {
                    let s = match name.as_str() {
                        "enemyIsBoss" => {
                            let titled = pat::gsub_fn(&pat::lower(s), "(%l)(%w*)", |c| {
                                Some(c[0].to_uppercase() + &c[1])
                            });
                            let s = pat::gsub(&titled, "Uber Atziri", "Boss");
                            let s = pat::gsub(&s, "Shaper", "Pinnacle");
                            pat::gsub(&s, "Sirus", "Pinnacle")
                        }
                        "presetBossSkills" => pat::gsub(s, "^Uber ", ""),
                        _ => s.to_string(),
                    };
                    Val::Str(s.as_str().into())
                } else if let Some(b) = node.attr("boolean") {
                    Val::Bool(b == "true")
                } else {
                    return Err(
                        "'Input' element missing number, string or boolean attribute".into(),
                    );
                };
                set.input.insert(name, v);
            }
            "Placeholder" => {
                let name = name
                    .ok_or("'Placeholder' element missing name attribute")?
                    .to_string();
                if let Some(n) = node.attr("number") {
                    set.placeholder
                        .insert(name, Val::Number(pat::to_number(n).unwrap_or(f64::NAN)));
                } else if let Some(s) = node.attr("string") {
                    // Upstream puts a string placeholder in the inputs
                    set.input.insert(name, Val::Str(s.into()));
                } else {
                    return Err("'Placeholder' element missing number".into());
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn block(node: &Element) -> CustomBlock {
        CustomBlock {
            title: Some(node.attr("title").unwrap_or("Default").to_string()),
            enabled: node.attr("enabled").is_none_or(|e| e == "true"),
            text: node.text().unwrap_or_default().to_string(),
        }
    }

    /// `ConfigTabClass:Load(xml)`
    pub fn load(xml: &Element, defaults: &[OptionDefault]) -> Result<ConfigTab> {
        let mut tab = ConfigTab {
            config_sets: BTreeMap::new(),
            config_set_order_list: vec![1.0],
            active_config_set_id: 1.0,
        };
        if xml.empty {
            tab.new_set(1.0, Some("Default"), defaults);
        }
        // Places count every child, as `ipairs(xml)` does
        let children = xml
            .children
            .iter()
            .enumerate()
            .filter_map(|(i, c)| match c {
                crate::xml::Node::Element(e) => Some((i, e)),
                crate::xml::Node::Text(_) => None,
            });
        for (index, node) in children {
            if node.elem != "ConfigSet" {
                if !tab.config_sets.contains_key(&1) {
                    tab.new_set(1.0, Some("Default"), defaults);
                }
                if node.elem == "CustomModifierBlock" {
                    let b = Self::block(node);
                    tab.config_sets.get_mut(&1).unwrap().custom_mods.push(b);
                } else {
                    tab.set_input(node, 1.0)?;
                }
            } else {
                let id = node
                    .attr("id")
                    .and_then(pat::to_number)
                    .ok_or("config set without an id")?;
                tab.new_set(id, Some(node.attr("title").unwrap_or("Default")), defaults);
                // `configSetOrderList[index] = id`
                while tab.config_set_order_list.len() < index + 1 {
                    tab.config_set_order_list.push(f64::NAN);
                }
                tab.config_set_order_list[index] = id;
                tab.config_sets
                    .get_mut(&key(id))
                    .unwrap()
                    .custom_mods
                    .clear();
                for child in node.elements() {
                    if child.elem == "CustomModifierBlock" {
                        let b = Self::block(child);
                        tab.config_sets
                            .get_mut(&key(id))
                            .unwrap()
                            .custom_mods
                            .push(b);
                    } else {
                        tab.set_input(child, id)?;
                    }
                }
            }
        }
        // `ipairs` stops at the first hole
        if let Some(n) = tab.config_set_order_list.iter().position(|x| x.is_nan()) {
            tab.config_set_order_list.truncate(n);
        }
        // Legacy custom mods
        for &id in &tab.config_set_order_list.clone() {
            let Some(set) = tab.config_sets.get_mut(&key(id)) else {
                continue;
            };
            let legacy = match set.input.get("customMods") {
                Some(v) => v.text(),
                None => String::new(),
            };
            let empty = set.custom_mods.is_empty()
                || (set.custom_mods.len() == 1 && set.custom_mods[0].text.is_empty());
            if !legacy.is_empty() && empty {
                set.custom_mods = vec![CustomBlock {
                    text: legacy,
                    ..default_block()
                }];
            } else if set.custom_mods.is_empty() {
                set.custom_mods = vec![default_block()];
            }
            set.input.remove("customMods");
        }
        // `SetActiveConfigSet(id)`
        let mut id = xml
            .attr("activeConfigSet")
            .and_then(pat::to_number)
            .unwrap_or(1.0);
        if tab.config_set_order_list.is_empty() {
            tab.config_set_order_list.push(1.0);
            tab.new_set(1.0, None, defaults);
        }
        if !tab.config_sets.contains_key(&key(id)) || id.fract() != 0.0 {
            id = tab.config_set_order_list[0];
        }
        tab.active_config_set_id = id;
        Ok(tab)
    }
}
