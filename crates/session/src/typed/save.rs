//! `saveXml` and `saveCode`: `buildMode:SaveDB` (`Modules/Build.lua`) with
//! every tab's `Save`, written as `xml.ComposeXML` writes it. What the
//! typed build does not hold (party, import and tree view settings,
//! timeless jewel search, item set links, trade weights) comes
//! from the loaded file as each tab's `Load` kept it.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::sidebar::display_stats;
use super::{NOT_PORTED, Result, TypedSession};
use engine::actor::OutVal;
use engine::config::Val;
use engine::pat::{self, num};
use engine::xml::{Element, Node};
use std::collections::BTreeMap;

/// An element under construction: attributes left out when `None`
struct X {
    elem: &'static str,
    attrib: Vec<(&'static str, String)>,
    children: Vec<C>,
}

enum C {
    E(X),
    T(String),
}

impl X {
    fn new(elem: &'static str) -> X {
        X {
            elem,
            attrib: Vec::new(),
            children: Vec::new(),
        }
    }

    fn a(mut self, k: &'static str, v: impl Into<Option<String>>) -> X {
        if let Some(v) = v.into() {
            self.attrib.push((k, v));
        }
        self
    }

    fn push(&mut self, x: X) {
        self.children.push(C::E(x));
    }

    fn text(&mut self, t: impl Into<String>) {
        self.children.push(C::T(t.into()));
    }
}

/// `encodeContent`
fn encode(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '\'' => out.push_str("&apos;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// `composeNode`: attributes sorted by name
fn compose(x: &X, lvl: usize, out: &mut String) {
    let tabs = "\t".repeat(lvl);
    out.push_str(&tabs);
    out.push('<');
    out.push_str(x.elem);
    let mut attrib: Vec<&(&str, String)> = x.attrib.iter().collect();
    attrib.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    for (k, v) in attrib {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        encode(v, out);
        out.push('"');
    }
    if x.children.is_empty() {
        out.push_str("/>\n");
        return;
    }
    out.push_str(">\n");
    for c in &x.children {
        match c {
            C::E(e) => compose(e, lvl + 1, out),
            C::T(t) => {
                out.push_str(&tabs);
                out.push('\t');
                encode(t, out);
                out.push('\n');
            }
        }
    }
    out.push_str(&tabs);
    out.push_str("</");
    out.push_str(x.elem);
    out.push_str(">\n");
}

fn b(v: bool) -> String {
    v.to_string()
}

fn n(v: f64) -> String {
    num(v)
}

/// `tostring` of an input's value, with the attribute naming its type
fn typed_value(v: &Val) -> (&'static str, String) {
    match v {
        Val::Number(x) => ("number", n(*x)),
        Val::Bool(x) => ("boolean", b(*x)),
        Val::Str(s) => ("string", s.to_string()),
    }
}

/// An `<Input>` or `<Placeholder>` element's value, as the tabs load it
fn read_value(e: &Element, boolean: bool) -> Option<Val> {
    if let Some(x) = e.attr("number") {
        return Some(Val::Number(pat::to_number(x).unwrap_or(f64::NAN)));
    }
    if let Some(s) = e.attr("string") {
        return Some(Val::Str(s.into()));
    }
    if boolean && let Some(x) = e.attr("boolean") {
        return Some(Val::Bool(x == "true"));
    }
    None
}

fn same(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Number(x), Val::Number(y)) => x == y,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Str(x), Val::Str(y)) => x == y,
        _ => false,
    }
}

/// The text children of an element, joined as upstream keeps the first
fn first_text(e: &Element) -> Option<&str> {
    e.children.iter().find_map(|c| match c {
        Node::Text(t) => Some(t.as_str()),
        Node::Element(_) => None,
    })
}

const PARTY_BUFFS: [&str; 7] = [
    "PartyMemberStats",
    "Aura",
    "Curse",
    "Warcry Skills",
    "Link Skills",
    "EnemyConditions",
    "EnemyMods",
];

impl TypedSession<'_> {
    fn section(&self, name: &str) -> Option<&Element> {
        self.root.child("PathOfBuilding")?.child(name)
    }

    /// `saveXml`: `buildMode:SaveDB("code")`
    pub fn save_xml(&mut self) -> Result<String> {
        self.check_main()?;
        // Upstream's frame made the Calcs tab's calculation, which writes
        // its selections back to the gems
        self.calcs_output()?;
        let party = self.section("Import").and_then(|i| i.attr("exportParty")) == Some("true");
        if party {
            return Err(format!("{NOT_PORTED}: exported party buffs"));
        }
        let mut root = X::new("PathOfBuilding");
        root.push(self.save_build()?);
        root.push(self.save_calcs());
        root.push(self.save_config());
        root.push(self.save_import());
        root.push(self.save_items()?);
        root.push(self.save_notes());
        root.push(self.save_party());
        root.push(self.save_skills());
        root.push(self.save_tree()?);
        root.push(self.save_tree_view());
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        compose(&root, 0, &mut out);
        Ok(out)
    }

    fn save_build(&self) -> Result<X> {
        let info = &self.build.info;
        let input = &self.build.config.active().input;
        let s = |k: &str| match input.get(k) {
            Some(Val::Str(v)) => Some(v.to_string()),
            Some(Val::Number(v)) => Some(n(*v)),
            _ => None,
        };
        let (class, ascend) = self.class_names();
        let mut x = X::new("Build")
            .a("targetVersion", info.target_version.clone())
            .a("viewMode", Some(self.view_mode.clone()))
            .a("level", n(info.character_level))
            .a("className", class)
            .a("ascendClassName", ascend)
            .a("bandit", s("bandit"))
            .a("pantheonMajorGod", s("pantheonMajorGod"))
            .a("pantheonMinorGod", s("pantheonMinorGod"))
            .a("mainSocketGroup", n(info.main_socket_group))
            .a("characterLevelAutoMode", b(info.character_level_auto_mode));
        for id in &info.spectre_list {
            x.push(X::new("Spectre").a("id", id.clone()));
        }
        let sb = self.sidebar()?;
        let out = &self.main.player;
        let (stats, minion_stats) = display_stats();
        let skill = &sb.main_skill;
        let mut added = std::collections::HashSet::new();
        for s in stats {
            let flags_match =
                s.flag.is_none_or(|f| skill.flag(f)) && s.not_flag.is_none_or(|f| !skill.flag(f));
            let Some(stat) = s.stat.filter(|_| flags_match) else {
                continue;
            };
            let name = format!("{stat}{}", s.child.unwrap_or(""));
            if added.contains(&name) {
                continue;
            }
            if stat == "SkillDPS" {
                if let Some(OutVal::List(rows)) = out.get(&engine::mods::Sym::new(stat)) {
                    for r in rows.iter() {
                        let OutVal::Table(t) = r else { continue };
                        let s = |k: &str| match t.get(k) {
                            Some(OutVal::Str(s)) => Some(s.to_string()),
                            _ => None,
                        };
                        let f = |k: &str| t.get(k).and_then(OutVal::num).unwrap_or(f64::NAN);
                        let name = s("name").unwrap_or_default();
                        let count = f("count");
                        let lhs = if count >= 2.0 {
                            format!("{}x {name}", n(count))
                        } else {
                            name
                        };
                        x.push(
                            X::new("FullDPSSkill")
                                .a("stat", lhs)
                                .a("value", n(f("dps") * count))
                                .a("skillPart", s("skillPart").unwrap_or_default())
                                .a(
                                    "source",
                                    s("source").or_else(|| s("trigger")).unwrap_or_default(),
                                ),
                        );
                    }
                }
                added.insert(name);
                continue;
            }
            let mut v = out.get(&engine::mods::Sym::new(stat));
            if let Some(c) = s.child {
                v = match v {
                    Some(OutVal::Table(t)) => t.get(c),
                    _ => None,
                };
            }
            if let Some(v) = v.and_then(stat_value) {
                x.push(X::new("PlayerStat").a("stat", name.clone()).a("value", v));
                added.insert(name);
            }
        }
        for stat in [
            "PowerCharges",
            "PowerChargesMax",
            "FrenzyCharges",
            "FrenzyChargesMax",
            "EnduranceCharges",
            "EnduranceChargesMax",
            "ActiveTotemLimit",
            "ActiveMinionLimit",
        ] {
            if let Some(v) = out.get(&engine::mods::Sym::new(stat)).and_then(stat_value) {
                x.push(
                    X::new("PlayerStat")
                        .a("stat", stat.to_string())
                        .a("value", v),
                );
            }
        }
        if let Some(m) = &self.main.minion {
            for s in minion_stats {
                let Some(stat) = s.stat else { continue };
                if let Some(v) = m.get(&engine::mods::Sym::new(stat)).and_then(stat_value) {
                    x.push(
                        X::new("MinionStat")
                            .a("stat", stat.to_string())
                            .a("value", v),
                    );
                }
            }
        }
        let t = self.section("Build").and_then(|b| b.child("TimelessData"));
        let attr = |k: &str| t.and_then(|t| t.attr(k));
        let num_attr = |k: &str| attr(k).and_then(pat::to_number);
        let mut tl = X::new("TimelessData")
            .a("jewelTypeId", num_attr("jewelTypeId").map(n))
            .a("conquerorTypeId", num_attr("conquerorTypeId").map(n))
            .a(
                "devotionVariant1",
                n(num_attr("devotionVariant1").unwrap_or(1.0)),
            )
            .a(
                "devotionVariant2",
                n(num_attr("devotionVariant2").unwrap_or(1.0)),
            )
            .a("jewelSocketId", num_attr("jewelSocketId").map(n))
            .a(
                "fallbackWeightModeIdx",
                num_attr("fallbackWeightModeIdx").map(n),
            );
        if t.is_some() {
            tl = tl
                .a(
                    "socketFilter",
                    (attr("socketFilter") == Some("true")).then(|| "true".into()),
                )
                .a(
                    "socketAllocate",
                    (attr("socketAllocate") == Some("true")).then(|| "true".into()),
                )
                .a(
                    "socketFilterDistance",
                    n(num_attr("socketFilterDistance").unwrap_or(0.0)),
                )
                .a("searchList", attr("searchList").map(String::from))
                .a(
                    "searchListFallback",
                    attr("searchListFallback").map(String::from),
                );
        } else {
            tl = tl
                .a("searchList", String::new())
                .a("searchListFallback", String::new());
        }
        x.push(tl);
        Ok(x)
    }

    fn class_names(&self) -> (Option<String>, Option<String>) {
        let spec = &self.build.spec;
        let class = spec
            .cur_class_id
            .and_then(|id| spec.tree.classes.iter().find(|(i, _)| *i == id))
            .map(|(_, c)| c);
        let ascend = class.and_then(|c| {
            let id = spec.cur_ascend_class_id.unwrap_or(0);
            let find = |id| c.ascendancies.iter().find(|(i, _)| *i == id);
            find(id)
                .or_else(|| find(0))
                .map(|(_, a)| a.name.to_string())
        });
        (class.map(|c| c.name.to_string()), ascend)
    }

    fn save_calcs(&self) -> X {
        let mut input: BTreeMap<String, Val> = BTreeMap::new();
        input.insert("skill_number".into(), Val::Number(1.0));
        input.insert("misc_buffMode".into(), Val::Str("EFFECTIVE".into()));
        let xml = self.section("Calcs");
        for e in xml
            .iter()
            .flat_map(|c| c.elements())
            .filter(|e| e.elem == "Input")
        {
            if let (Some(k), Some(v)) = (e.attr("name"), read_value(e, true)) {
                input.insert(k.into(), v);
            }
        }
        let c = &self.build.calcs;
        input.insert("skill_number".into(), Val::Number(c.skill_number));
        input.insert(
            "misc_buffMode".into(),
            Val::Str(c.buff_mode.as_str().into()),
        );
        if input.contains_key("showMinion") || self.show_minion_set {
            input.insert("showMinion".into(), Val::Bool(c.show_minion));
        }
        let mut x = X::new("Calcs");
        for (k, v) in &input {
            let (t, v) = typed_value(v);
            x.push(X::new("Input").a("name", k.clone()).a(t, v));
        }
        for section in super::calcs::sections() {
            let s = section["#"].as_array().map_or(&[][..], |v| v);
            let id = s.get(1).and_then(|v| v.as_str()).unwrap_or_default();
            let subs = s
                .get(4)
                .and_then(|v| v["#"].as_array())
                .map_or(&[][..], |v| v);
            for sub in subs {
                let label = sub["label"].as_str().unwrap_or_default();
                let sub_id = pat::gsub(label, "%W", "");
                let saved = xml.and_then(|c| {
                    c.elements().find(|e| {
                        e.elem == "Section"
                            && e.attr("id") == Some(id)
                            && e.attr("subsection") == Some(sub_id.as_str())
                    })
                });
                let collapsed = match saved {
                    Some(e) => b(e.attr("collapsed") == Some("true")),
                    None => match sub.get("defaultCollapsed") {
                        Some(serde_json::Value::Bool(v)) => b(*v),
                        _ => "nil".into(),
                    },
                };
                x.push(
                    X::new("Section")
                        .a("id", id.to_string())
                        .a("subsection", sub_id)
                        .a("collapsed", collapsed),
                );
            }
        }
        x
    }

    fn save_config(&self) -> X {
        let config = &self.build.config;
        let defaults = &self.data.game.option_defaults;
        let active = config.active();
        let default_of = |k: &str, v: &Val| -> Val {
            if let Some(p) = active.placeholder.get(k) {
                return p.clone();
            }
            if let Some(d) = defaults
                .iter()
                .rev()
                .find(|d| d.var == k)
                .and_then(|d| d.saved.clone())
            {
                return d;
            }
            match v {
                Val::Number(_) => Val::Number(0.0),
                Val::Bool(_) => Val::Bool(false),
                Val::Str(_) => Val::Str("".into()),
            }
        };
        let mut x = X::new("Config").a("activeConfigSet", n(config.active_config_set_id));
        for id in &config.config_set_order_list {
            let Some(set) = config.config_sets.get(&(*id as i64)) else {
                continue;
            };
            let mut c = X::new("ConfigSet")
                .a("id", n(*id))
                .a("title", set.title.clone());
            let mut keys: Vec<&String> = set.input.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            for k in keys {
                let v = &set.input[k];
                if !same(v, &default_of(k, v)) {
                    let (t, v) = typed_value(v);
                    c.push(X::new("Input").a("name", k.clone()).a(t, v));
                }
            }
            let mut keys: Vec<&String> = set.placeholder.keys().collect();
            keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
            for k in keys {
                let (t, v) = match &set.placeholder[k] {
                    Val::Number(x) => ("number", n(*x)),
                    v => ("string", typed_value(v).1),
                };
                c.push(X::new("Placeholder").a("name", k.clone()).a(t, v));
            }
            for block in &set.custom_mods {
                let mut e = X::new("CustomModifierBlock")
                    .a(
                        "title",
                        block.title.clone().unwrap_or_else(|| "Default".into()),
                    )
                    .a("enabled", b(block.enabled));
                e.text(block.text.clone());
                c.push(e);
            }
            x.push(c);
        }
        x
    }

    fn save_import(&self) -> X {
        let i = self.section("Import");
        let attr = |k: &str| i.and_then(|i| i.attr(k)).map(String::from);
        X::new("Import")
            .a("lastRealm", attr("lastRealm"))
            .a("lastLeague", attr("lastLeague"))
            .a("lastAccountHash", attr("lastAccountHash"))
            .a("lastCharacterHash", attr("lastCharacterHash"))
            .a(
                "exportParty",
                b(attr("exportParty").as_deref() == Some("true")),
            )
            .a("importLink", attr("importLink").filter(|l| l.len() < 100))
    }

    fn save_items(&mut self) -> Result<X> {
        let ctx = self.data.item_context();
        let tab = &mut self.build.items;
        let active = tab
            .item_sets
            .get(&(tab.active_item_set_id as i64))
            .ok_or("no active item set")?;
        let xml = self
            .root
            .child("PathOfBuilding")
            .and_then(|p| p.child("Items"));
        let show = xml
            .and_then(|i| i.attr("showStatDifferences"))
            .is_none_or(|v| v == "true");
        let mut x = X::new("Items")
            .a("activeItemSet", n(tab.active_item_set_id))
            .a("useSecondWeaponSet", b(active.use_second_weapon_set))
            .a("showStatDifferences", b(show));
        for id in tab.item_order_list.clone() {
            let Some(item) = tab.items.get_mut(&(id as i64)) else {
                continue;
            };
            // `BuildAndParseRaw`: the item as its text reads back
            let raw = item.build_raw();
            if raw != item.raw {
                std::sync::Arc::make_mut(item).parse_raw(&ctx, &raw, None, false)?;
            }
            let item = &tab.items[&(id as i64)];
            let mut c = X::new("Item")
                .a("id", n(id))
                .a("variant", item.variant.map(n));
            for (i, k) in [
                "variantAlt",
                "variantAlt2",
                "variantAlt3",
                "variantAlt4",
                "variantAlt5",
            ]
            .into_iter()
            .enumerate()
            {
                c = c.a(k, item.variant_alt[i].map(n));
            }
            c.text(item.raw.clone());
            let mut line_id = item.buff_mod_lines.len() + 1;
            for list in [
                &item.enchant_mod_lines,
                &item.scourge_mod_lines,
                &item.implicit_mod_lines,
                &item.explicit_mod_lines,
                &item.crucible_mod_lines,
            ] {
                for l in list {
                    if let Some(r) = l.range {
                        c.push(
                            X::new("ModRange")
                                .a("id", line_id.to_string())
                                .a("range", n(r)),
                        );
                    }
                    line_id += 1;
                }
            }
            x.push(c);
        }
        let tab = &self.build.items;
        let saved_sets: Vec<&Element> = xml
            .iter()
            .flat_map(|i| i.elements())
            .filter(|e| e.elem == "ItemSet")
            .collect();
        for set_id in &tab.item_set_order_list {
            let Some(set) = tab.item_sets.get(&(*set_id as i64)) else {
                continue;
            };
            let saved = saved_sets
                .iter()
                .rev()
                .find(|e| e.attr("id").and_then(pat::to_number) == Some(*set_id));
            let mut c = X::new("ItemSet")
                .a("id", n(*set_id))
                .a("title", set.title.clone())
                .a("useSecondWeaponSet", b(set.use_second_weapon_set));
            for slot in &tab.ordered_slots {
                let name = &slot.slot_name;
                match slot.node_id {
                    None => {
                        let sel = set.slots.get(name).cloned().unwrap_or_default();
                        let pb = saved
                            .and_then(|s| {
                                s.elements()
                                    .filter(|e| {
                                        e.elem == "Slot" && e.attr("name").unwrap_or("") == name
                                    })
                                    .last()
                            })
                            .map(|e| e.attr("itemPbURL").unwrap_or("").to_string())
                            .unwrap_or_default();
                        c.push(
                            X::new("Slot")
                                .a("name", name.clone())
                                .a("itemId", n(sel.sel_item_id))
                                .a("itemPbURL", pb)
                                .a("active", sel.active.then(|| "true".to_string())),
                        );
                    }
                    Some(node) => {
                        if self.build.spec.alloc_nodes.contains(node) {
                            let pb = saved
                                .and_then(|s| {
                                    s.elements()
                                        .filter(|e| {
                                            e.elem == "SocketIdURL"
                                                && e.attr("nodeId").and_then(pat::to_number)
                                                    == Some(node as f64)
                                        })
                                        .last()
                                })
                                .map(|e| e.attr("itemPbURL").unwrap_or("").to_string())
                                .unwrap_or_default();
                            c.push(
                                X::new("SocketIdURL")
                                    .a("name", name.clone())
                                    .a("nodeId", node.to_string())
                                    .a("itemPbURL", pb),
                            );
                        }
                    }
                }
            }
            x.push(c);
        }
        let mut weights = X::new("TradeSearchWeights");
        let stats = engine::power::power_stats();
        for e in xml
            .iter()
            .flat_map(|i| i.elements())
            .filter(|e| e.elem == "TradeSearchWeights")
            .flat_map(|t| t.elements())
        {
            let stat = e.attr("stat");
            let Some(entry) = stats.iter().find(|s| s.stat.as_deref() == stat) else {
                continue;
            };
            let Some(w) = e.attr("weightMult").and_then(pat::to_number) else {
                continue;
            };
            if w > 0.0 {
                weights.push(
                    X::new("Stat")
                        .a("label", entry.label.clone())
                        .a("stat", stat.map(String::from))
                        .a("weightMult", engine::sf!("%.2f", w)),
                );
            }
        }
        x.push(weights);
        Ok(x)
    }

    fn save_notes(&mut self) -> X {
        let mut x = X::new("Notes");
        x.text(self.notes_for_save());
        x
    }

    fn save_party(&self) -> X {
        let p = self.section("Party");
        let mut bufs: BTreeMap<&str, &str> = BTreeMap::new();
        for e in p.iter().flat_map(|p| p.elements()) {
            if e.elem == "ImportedBuffs"
                && let Some(name) = e.attr("name")
                && PARTY_BUFFS.contains(&name)
            {
                bufs.insert(name, first_text(e).unwrap_or(""));
            }
        }
        let attr = |k: &str| p.and_then(|p| p.attr(k));
        let destination = attr("destination")
            .filter(|d| {
                [
                    "All",
                    "Party Member Stats",
                    "Aura",
                    "Curse",
                    "Warcry Skills",
                    "Link Skills",
                    "EnemyConditions",
                    "EnemyMods",
                ]
                .contains(d)
            })
            .unwrap_or("All");
        let mut x = X::new("Party")
            .a("destination", destination.to_string())
            .a("append", b(attr("append") == Some("true")))
            .a(
                "ShowAdvanceTools",
                b(attr("ShowAdvanceTools") == Some("true")),
            );
        for name in PARTY_BUFFS {
            if let Some(t) = bufs.get(name).filter(|t| !t.is_empty()) {
                let mut c = X::new("ImportedBuffs").a("name", name.to_string());
                c.text(*t);
                x.push(c);
            }
        }
        x
    }

    fn save_skills(&self) -> X {
        let xml = self.section("Skills");
        let attr = |k: &str| xml.and_then(|s| s.attr(k));
        let Some(skills) = &self.build.skills else {
            return X::new("Skills");
        };
        let show = attr("showSupportGemTypes")
            .filter(|v| ["ALL", "NORMAL", "EXCEPTIONAL"].contains(v))
            .unwrap_or("ALL");
        let field = attr("sortGemsByDPSField")
            .filter(|v| {
                [
                    "FullDPS",
                    "CombinedDPS",
                    "TotalDPS",
                    "AverageDamage",
                    "TotalDot",
                    "BleedDPS",
                    "IgniteDPS",
                    "TotalPoisonDPS",
                    "TotalEHP",
                ]
                .contains(v)
            })
            .unwrap_or("CombinedDPS");
        let mut x = X::new("Skills")
            .a("activeSkillSet", n(skills.active_skill_set_id))
            .a(
                "defaultGemLevel",
                skills.default_gem_level.name().to_string(),
            )
            .a("defaultGemQuality", n(skills.default_gem_quality))
            .a(
                "sortGemsByDPS",
                b(attr("sortGemsByDPS").is_none_or(|v| v == "true")),
            )
            .a("showSupportGemTypes", show.to_string())
            .a("sortGemsByDPSField", field.to_string())
            .a("showLegacyGems", b(attr("showLegacyGems") == Some("true")));
        for id in &skills.skill_set_order_list {
            let Some(set) = skills.skill_sets.get(&(*id as i64)) else {
                continue;
            };
            let mut c = X::new("SkillSet")
                .a("id", n(*id))
                .a("title", set.title.clone());
            for g in &set.socket_group_list {
                let mut s = X::new("Skill")
                    .a("enabled", b(g.enabled))
                    .a(
                        "includeInFullDPS",
                        g.include_in_full_dps.map_or_else(|| "nil".into(), b),
                    )
                    .a("groupCount", g.group_count.map(n))
                    .a("label", g.label.clone())
                    .a("slot", g.slot.clone())
                    .a("source", g.source.clone())
                    .a(
                        "mainActiveSkill",
                        g.main_active_skill.map_or_else(|| "nil".into(), n),
                    )
                    .a(
                        "mainActiveSkillCalcs",
                        g.main_active_skill_calcs.map_or_else(|| "nil".into(), n),
                    )
                    .a("imbuedSupport", g.imbued_support.clone());
                for gem in &g.gem_list {
                    let opt = |v: Option<f64>| v.map(n);
                    let tostr = |v: Option<f64>| v.map_or_else(|| "nil".to_string(), n);
                    // The gems the calculations make have no `enableGlobal2`
                    // (an explosion's no `enableGlobal1` either)
                    let global = |v: bool, made: bool| if made { "nil".into() } else { b(v) };
                    s.push(
                        X::new("Gem")
                            .a("nameSpec", gem.name_spec.clone())
                            .a("skillId", gem.skill_id.clone())
                            .a("gemId", gem.gem.as_ref().map(|d| d.game_id.clone()))
                            .a("variantId", gem.gem.as_ref().map(|d| d.variant_id.clone()))
                            .a("level", tostr(gem.level))
                            .a("quality", tostr(gem.quality))
                            .a("enabled", b(gem.enabled))
                            .a(
                                "enableGlobal1",
                                global(gem.enable_global1, gem.explode_source.is_some()),
                            )
                            .a(
                                "enableGlobal2",
                                global(gem.enable_global2, gem.count.is_none()),
                            )
                            .a("count", tostr(gem.count))
                            .a("skillPart", opt(gem.skill_part))
                            .a("skillPartCalcs", opt(gem.skill_part_calcs))
                            .a("skillStageCount", opt(gem.skill_stage_count))
                            .a("skillStageCountCalcs", opt(gem.skill_stage_count_calcs))
                            .a("skillMineCount", opt(gem.skill_mine_count))
                            .a("skillMineCountCalcs", opt(gem.skill_mine_count_calcs))
                            .a("skillMinion", gem.skill_minion.clone())
                            .a("skillMinionCalcs", gem.skill_minion_calcs.clone())
                            .a("skillMinionItemSet", opt(gem.skill_minion_item_set))
                            .a(
                                "skillMinionItemSetCalcs",
                                opt(gem.skill_minion_item_set_calcs),
                            )
                            .a("skillMinionSkill", opt(gem.skill_minion_skill))
                            .a("skillMinionSkillCalcs", opt(gem.skill_minion_skill_calcs)),
                    );
                }
                c.push(s);
            }
            x.push(c);
        }
        x
    }

    fn save_tree(&self) -> Result<X> {
        let (active, _) = &self.specs;
        let mut x = X::new("Tree").a("activeSpec", active.to_string());
        if self.other_specs.is_empty() {
            x.push(save_spec(&self.build.spec));
        }
        for s in &self.other_specs {
            x.push(save_spec(s.as_deref().unwrap_or(&self.build.spec)));
        }
        Ok(x)
    }

    fn save_tree_view(&self) -> X {
        let v = self.section("TreeView");
        let attr = |k: &str| v.and_then(|v| v.attr(k));
        let num_attr = |k: &str| attr(k).and_then(pat::to_number);
        let (zx, zy) = match (attr("zoomX"), attr("zoomY")) {
            (Some(_), Some(_)) => (num_attr("zoomX"), num_attr("zoomY")),
            _ => (Some(0.0), Some(0.0)),
        };
        let tostr = |v: Option<f64>| v.map_or_else(|| "nil".to_string(), n);
        X::new("TreeView")
            .a(
                "zoomLevel",
                tostr(if attr("zoomLevel").is_some() {
                    num_attr("zoomLevel")
                } else {
                    Some(3.0)
                }),
            )
            .a("zoomX", tostr(zx))
            .a("zoomY", tostr(zy))
            .a("searchStr", attr("searchStr").unwrap_or("").to_string())
            .a(
                "showStatDifferences",
                b(attr("showStatDifferences").is_none_or(|v| v == "true")),
            )
    }

    /// `saveCode`: the build's XML as a build code
    pub fn save_code(&mut self) -> Result<String> {
        let xml = self.save_xml()?;
        let z = miniz_oxide::deflate::compress_to_vec_zlib(xml.as_bytes(), 9);
        Ok(base64(&z).replace('+', "-").replace('/', "_"))
    }
}

/// A number or a boolean stat as `tostring` writes it
fn stat_value(v: &OutVal) -> Option<String> {
    match v {
        OutVal::Num(x) => Some(n(*x)),
        OutVal::Bool(x) => Some(b(*x)),
        OutVal::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// `PassiveSpec:Save(xml)`
fn save_spec(spec: &engine::spec::PassiveSpec) -> X {
    let mut alloc: Vec<u32> = spec.alloc_nodes.iter().map(|(id, _)| id).collect();
    alloc.sort_unstable();
    let mut masteries: Vec<(u32, f64)> = spec
        .mastery_selections
        .iter()
        .map(|(k, v)| (k, *v))
        .collect();
    masteries.sort_by_key(|m| m.0);
    let effects: Vec<String> = masteries
        .iter()
        .map(|(m, e)| format!("{{{m},{}}}", n(*e)))
        .collect();
    let mut x = X::new("Spec")
        .a("title", spec.title.clone())
        .a("treeVersion", spec.tree.version.to_string())
        .a(
            "clusterHashFormatVersion",
            n(spec.cluster_hash_format_version),
        )
        .a(
            "classId",
            spec.cur_class_id
                .map_or_else(|| "nil".into(), |c| c.to_string()),
        )
        .a(
            "ascendClassId",
            spec.cur_ascend_class_id
                .map_or_else(|| "nil".into(), |c| c.to_string()),
        )
        .a(
            "secondaryAscendClassId",
            spec.cur_secondary_ascend_class_id
                .map_or_else(|| "nil".into(), |c| c.to_string()),
        )
        .a(
            "nodes",
            alloc
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(","),
        )
        .a("masteryEffects", effects.join(","));
    let mut url = X::new("URL");
    url.text(encode_url(
        spec,
        "https://www.pathofexile.com/passive-skill-tree/",
    ));
    x.push(url);
    let mut sockets = X::new("Sockets");
    for (node, item) in &spec.jewels {
        if *item > 0.0 {
            sockets.push(
                X::new("Socket")
                    .a("nodeId", node.to_string())
                    .a("itemId", n(*item)),
            );
        }
    }
    x.push(sockets);
    x.push(X::new("Overrides"));
    x
}

/// `PassiveSpec:EncodeURL(prefix)`
fn encode_url(spec: &engine::spec::PassiveSpec, prefix: &str) -> String {
    use engine::tree::NodeType;
    let class = spec.cur_class_id.unwrap_or(0);
    let ascend = spec.cur_ascend_class_id.unwrap_or(0);
    let secondary = spec.cur_secondary_ascend_class_id.unwrap_or(0);
    let mut a: Vec<i64> = vec![0, 0, 0, 6, class, (secondary << 2) | ascend];
    let (mut nodes, mut clusters, mut masteries) = (0, Vec::new(), Vec::new());
    let mut ids: Vec<(u32, usize)> = spec.alloc_nodes.iter().map(|(id, i)| (id, *i)).collect();
    ids.sort_unstable_by_key(|x| x.0);
    for (id, i) in ids {
        let kind = spec.kind(i);
        if kind != NodeType::ClassStart
            && kind != NodeType::AscendClassStart
            && id < 65536
            && nodes < 255
        {
            a.push((id / 256) as i64);
            a.push((id % 256) as i64);
            nodes += 1;
            if let Some(e) = spec.mastery_selections.get(id) {
                let e = *e as i64;
                masteries.extend([e / 256, e % 256, (id / 256) as i64, (id % 256) as i64]);
            }
        } else if id >= 65536 {
            let c = id - 65536;
            clusters.extend([(c / 256) as i64, (c % 256) as i64]);
        }
    }
    a.insert(6, nodes);
    a.push((clusters.len() / 2) as i64);
    a.extend(clusters);
    a.push((masteries.len() / 4) as i64);
    a.extend(masteries);
    let bytes: Vec<u8> = a.iter().map(|v| *v as u8).collect();
    format!(
        "{prefix}{}",
        base64(&bytes).replace('+', "-").replace('/', "_")
    )
}

/// Standard base64 with padding, as upstream's `base64.encode`
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let v = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &c)| acc | (c as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(v >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
