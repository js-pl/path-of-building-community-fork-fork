//! The Configuration tab's Mod Browser (`Modules/ConfigModBrowser.lua`,
//! a custom modifier group's Add Mod): every supported line of the tree's
//! nodes and mastery effects and of the item, crafted, veiled and
//! beastcraft mods, one per template (values replaced by `#`), searched
//! with its fuzzy ranking, and Add, which appends the chosen line to the
//! group's text.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::ItemPopup;
use super::{NOT_PORTED, Result, TypedSession};
use engine::item_data::Affix;
use engine::pat::{gmatch1, gsub, lower};
use engine::tree::NodeType;
use serde_json::{Value as J, json};
use std::sync::Arc;

const NO_MATCH: &str = "No matching modifiers found";

/// A line of the browser, by template
#[derive(Clone, Debug)]
struct Entry {
    text: String,
    template: String,
    sort_key: String,
    /// Where the line comes from, in the order first seen
    sources: Vec<String>,
    supported: bool,
}

/// The popup
#[derive(Clone, Debug)]
pub struct ModBrowser {
    /// The custom modifier group the line goes to
    block: usize,
    supported: Arc<Vec<Entry>>,
    search: String,
    /// `displayList`: indices into `supported`, `None` for the no-match row
    display: Vec<Option<usize>>,
    /// The list's `selIndex` and whether it has a `selValue`
    sel: Option<usize>,
    sel_value: bool,
}

/// `getModTemplate(modText)`
fn template(text: &str) -> String {
    let t = gsub(text, "([%+-]?)%((%-?%d+%.?%d*)%-(%-?%d+%.?%d*)%)", "%1#");
    lower(&gsub(&t, "%d+%.?%d*", "#"))
}

/// `addModLine(set, line, source, supported)`
fn add_line(
    set: &mut Vec<Entry>,
    index: &mut std::collections::HashMap<String, usize>,
    line: &str,
    source: &str,
    supported: Option<bool>,
) {
    let t = template(line);
    let i = match index.get(&t) {
        Some(&i) => {
            let e = &mut set[i];
            if !e.sources.iter().any(|s| s == source) {
                e.sources.push(source.to_string());
            }
            if *line < *e.text && (supported == Some(true) || !e.supported) {
                e.text = line.to_string();
            }
            i
        }
        None => {
            set.push(Entry {
                text: line.to_string(),
                template: t.clone(),
                sort_key: String::new(),
                sources: vec![source.to_string()],
                supported: false,
            });
            index.insert(t, set.len() - 1);
            set.len() - 1
        }
    };
    if let Some(s) = supported {
        let e = &mut set[i];
        if s && !e.supported {
            e.text = line.to_string();
        }
        e.supported = e.supported || s;
    }
}

/// `fuzzyScore(modText, searchStr, words)`
fn fuzzy_score(text: &str, search: &str, words: &[String]) -> Option<u8> {
    let l = lower(text);
    if l.contains(search) {
        return Some(1);
    }
    if words.len() > 1 && words.iter().all(|w| l.contains(w.as_str())) {
        return Some(2);
    }
    if words.len() == 1 && search.len() >= 3 {
        let text_words = gmatch1(&l, "%w+");
        let s = search.as_bytes();
        for (i, w) in text_words.iter().enumerate() {
            for len1 in 2..s.len() {
                let (p1, p2) = (&s[..len1], &s[len1..]);
                if w.as_bytes().starts_with(p1)
                    && text_words
                        .get(i + 1)
                        .is_some_and(|n| n.as_bytes().starts_with(p2))
                {
                    return Some(3);
                }
            }
        }
    }
    None
}

impl<'d> TypedSession<'d> {
    /// The supported lines, sorted (`OpenAddModPopup`'s `supportedList`)
    fn mod_browser_lines(&self) -> Result<Vec<Entry>> {
        let parser = &self.data.parser;
        let supported = |line: &str| {
            let (list, extra) = parser.parse(line.as_bytes());
            list.is_some() && extra.is_none()
        };
        let mut set: Vec<Entry> = Vec::new();
        let mut index = std::collections::HashMap::new();
        let tree = self.build.spec.tree;
        for n in &tree.nodes {
            if n.kind == NodeType::Mastery && !n.mastery_effects.is_empty() {
                for e in &n.mastery_effects {
                    let Some(effect) = tree.mastery_effects.get(&e.to_bits()) else {
                        continue;
                    };
                    for line in &effect.sd {
                        add_line(
                            &mut set,
                            &mut index,
                            line,
                            "Mastery Node",
                            Some(supported(line)),
                        );
                    }
                }
                continue;
            }
            let mut i = 0;
            while i < n.sd.len() {
                let mut combined = n.sd[i].to_string();
                while n.mods.get(i + 1).is_some_and(|m| m.combined) {
                    combined = format!("{combined} {}", n.sd[i + 1]);
                    i += 1;
                }
                let ok = n
                    .mods
                    .get(i)
                    .is_some_and(|m| m.list.is_some() && m.extra.is_none());
                let prefix = if n.is_bloodline {
                    "Bloodline "
                } else if n.ascendancy_name.is_some() {
                    "Ascendancy "
                } else {
                    ""
                };
                let source = format!("{prefix}{} Node", n.kind.name());
                add_line(&mut set, &mut index, &combined, &source, Some(ok));
                i += 1;
            }
        }
        let items = &self.data.game.items;
        let mut item_mod = |set: &mut Vec<Entry>, m: &Affix, source: &str| -> Result<()> {
            let lines: Vec<String> = match &m.trade_hashes {
                Some(t) => t.iter().map(|d| d.join(" ")).collect(),
                None => m.lines.clone(),
            };
            for line in lines {
                // `itemLib.applyRange(line, 0.5)`, as the mod cache does
                let ranged = engine::item_tools::apply_range(
                    items,
                    parser,
                    &line,
                    &engine::item_tools::Range::One(0.5),
                    None,
                    None,
                )?;
                let ok = supported(&ranged);
                add_line(set, &mut index, &ranged, source, Some(ok));
            }
            Ok(())
        };
        let craft = &self.data.game.craft;
        for (m, _) in &craft.master_mods {
            item_mod(&mut set, m, "Crafted mod")?;
        }
        for (group, mods) in &items.item_mods {
            if group == "Item" || group == "JewelCluster" {
                continue;
            }
            for m in &mods.list {
                item_mod(&mut set, m, "Item mod")?;
            }
        }
        for m in &craft.veiled_mods {
            item_mod(&mut set, m, "Veiled mod")?;
        }
        for m in &craft.beast_craft {
            item_mod(&mut set, m, "Item mod")?;
        }
        let mut list: Vec<Entry> = set
            .into_iter()
            .filter(|e| e.supported)
            .map(|mut e| {
                let k = gsub(&gsub(&e.template, "#", " "), "[^%a]+", " ");
                e.sort_key = k
                    .trim_matches(|c: char| c.is_ascii_whitespace())
                    .to_string();
                e
            })
            .collect();
        list.sort_by(|a, b| {
            (&a.sort_key, &a.template, &a.text).cmp(&(&b.sort_key, &b.template, &b.text))
        });
        Ok(list)
    }

    /// `updateDisplayList(controls, displayList, modList)`
    fn mod_browser_update(p: &mut ModBrowser) {
        let search = lower(&p.search);
        let search = search.trim_matches(|c: char| c.is_ascii_whitespace());
        p.display = if search.is_empty() {
            (0..p.supported.len()).map(Some).collect()
        } else {
            let words: Vec<String> = gmatch1(search, "%S+");
            let mut ranked: Vec<(u8, usize)> = p
                .supported
                .iter()
                .enumerate()
                .filter_map(|(i, e)| fuzzy_score(&e.text, search, &words).map(|r| (r, i)))
                .collect();
            // Ranked, then as the list sorts (the list is in that order)
            ranked.sort();
            ranked.into_iter().map(|(_, i)| Some(i)).collect()
        };
        if p.display.is_empty() {
            p.display.push(None);
        }
        p.sel = Some(1);
        p.sel_value = true;
    }

    /// A custom modifier group's Add Mod
    pub(super) fn open_mod_browser(&mut self, block: usize) -> Result<()> {
        let mut p = ModBrowser {
            block,
            supported: Arc::new(self.mod_browser_lines()?),
            search: String::new(),
            display: Vec::new(),
            sel: None,
            sel_value: false,
        };
        Self::mod_browser_update(&mut p);
        self.editor
            .popups
            .insert(0, ItemPopup::ModBrowser(Box::new(p)));
        self.refresh_item_popups();
        Ok(())
    }

    /// The selected line, unless it is the no-match row
    fn mod_browser_selected(p: &ModBrowser) -> Option<&Entry> {
        let i = p.sel.unwrap_or(1);
        p.display
            .get(i.wrapping_sub(1))
            .copied()
            .flatten()
            .map(|k| &p.supported[k])
    }

    pub(super) fn describe_mod_browser(&self, p: &ModBrowser) -> J {
        let magic = super::tooltip::cc("MAGIC");
        let rows: Vec<J> = p
            .display
            .iter()
            .map(|d| match d {
                Some(i) => json!(format!("{magic}{}", p.supported[*i].text)),
                None => json!(NO_MATCH),
            })
            .collect();
        let mut list = json!({
            "class": "ListControl", "shown": true, "enabled": true,
            "list": rows, "path": "popup.listControl",
        });
        if p.sel_value
            && let Some(s) = p.sel
        {
            list["sel"] = json!(s);
        }
        let enabled = Self::mod_browser_selected(p).is_some();
        json!({
            "title": "Mod Browser",
            "lines": [],
            "buttons": [
                { "name": "close", "label": "Cancel", "enabled": true },
                { "name": "save", "label": "Add", "enabled": enabled },
                { "name": "whatDoesItDo", "label": "?", "enabled": true },
            ],
            "controls": [
                list,
                {
                    "class": "EditControl", "shown": true, "enabled": true,
                    "text": p.search, "label": "^7Search:", "path": "popup.search",
                },
            ],
        })
    }

    /// Add: the line appended to the group's text; whether the popup
    /// closes (every button but `?` closes it)
    pub(super) fn mod_browser_button(&mut self, p: &ModBrowser, name: &str) -> Result<bool> {
        match name {
            "save" => {
                if let Some(e) = Self::mod_browser_selected(p) {
                    let line = e.text.clone();
                    let config = Arc::make_mut(&mut self.build.config);
                    let list = &mut config.active_mut().custom_mods;
                    let b = list
                        .get_mut(p.block)
                        .ok_or_else(|| format!("{NOT_PORTED}: Add Mod on a group that is gone"))?;
                    if !b.text.is_empty() && !b.text.ends_with('\n') {
                        b.text.push('\n');
                    }
                    b.text.push_str(&line);
                    self.update_custom_mods_controls();
                    self.custom_mods_changed()?;
                }
                Ok(true)
            }
            "close" => Ok(true),
            "whatDoesItDo" => Ok(false),
            _ => Err(format!("no popup button {name}")),
        }
    }

    pub(super) fn mod_browser_control(
        &mut self,
        p: &mut ModBrowser,
        name: &str,
        req: &J,
    ) -> Result<bool> {
        match name {
            "search" => {
                let t = req["text"].as_str().ok_or("no text")?;
                p.search = t.to_string();
                Self::mod_browser_update(p);
                Ok(false)
            }
            "listControl" => {
                let s = req["sel"]
                    .as_f64()
                    .ok_or_else(|| format!("{NOT_PORTED}: control popup.listControl"))?;
                // `SelectIndex`: no row leaves the index
                let row = (s >= 1.0 && s.fract() == 0.0 && (s as usize) <= p.display.len())
                    .then_some(s as usize);
                p.sel_value = row.is_some();
                let Some(row) = row else { return Ok(false) };
                p.sel = Some(row);
                if req["click"].as_bool() == Some(true)
                    && req["doubleClick"].as_bool() == Some(true)
                    && Self::mod_browser_selected(p).is_some()
                {
                    return self.mod_browser_button(p, "save");
                }
                Ok(false)
            }
            _ => Err(format!("{NOT_PORTED}: control popup.{name}")),
        }
    }

    /// A row's tooltip: where the line comes from
    pub(super) fn mod_browser_tooltip(
        &self,
        p: &ModBrowser,
        name: &str,
        index: Option<f64>,
    ) -> Result<J> {
        let mut tip = super::tooltip::Tooltip::default();
        if name == "listControl"
            && let Some(i) = index.filter(|i| *i >= 1.0 && i.fract() == 0.0)
            && let Some(Some(k)) = p.display.get(i as usize - 1)
        {
            for s in &p.supported[*k].sources {
                tip.add_line(14.0, &format!("^7{s}"));
            }
        }
        Ok(tip.json())
    }
}
