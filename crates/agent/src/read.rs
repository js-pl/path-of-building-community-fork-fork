//! The reading verbs: compact views of a build, with names, and the
//! author's text only under `untrusted`.

use crate::text::{self, LINE_CAP, NAME_CAP, NOTES_CAP, clean, plain, untrusted};
use crate::{Agent, BuildId, Grant, R, Store, check_fields, cmd, opt_bool, opt_str, opt_u64};
use engine::item::{Item, ModLine};
use serde_json::{Map, Value as J, json};
use session::Dispatcher;

/// The stats a change reports, by output key: label, and whether it is in
/// points (resistances, chances) rather than a ratio
pub(crate) const DIFF_STATS: &[(&str, &str, bool)] = &[
    ("FullDPS", "Full DPS", false),
    ("CombinedDPS", "Combined DPS", false),
    ("TotalEHP", "EHP", false),
    ("Life", "Life", false),
    ("EnergyShield", "Energy Shield", false),
    ("Mana", "Mana", false),
    ("Armour", "Armour", false),
    ("Evasion", "Evasion", false),
    ("PhysicalMaximumHitTaken", "Phys Max Hit", false),
    ("FireMaximumHitTaken", "Fire Max Hit", false),
    ("ColdMaximumHitTaken", "Cold Max Hit", false),
    ("LightningMaximumHitTaken", "Lightning Max Hit", false),
    ("ChaosMaximumHitTaken", "Chaos Max Hit", false),
    ("FireResist", "Fire Res", true),
    ("ColdResist", "Cold Res", true),
    ("LightningResist", "Lightning Res", true),
    ("ChaosResist", "Chaos Res", true),
    ("SpellSuppressionChance", "Suppression", true),
    ("BlockChance", "Block", true),
    ("SpellBlockChance", "Spell Block", true),
    ("MovementSpeedMod", "Movement Speed", false),
];

impl<'d, S: Store> Agent<'d, S> {
    pub(crate) fn read(&mut self, grant: &Grant, id: &BuildId, verb: &str, req: &J) -> R<J> {
        let name = grant.api_name(id);
        match verb {
            "summary" => {
                check_fields(req, &["build"])?;
                self.summary(id, name)
            }
            "tree" => {
                check_fields(req, &["build"])?;
                self.tree(id, name)
            }
            "skills" => {
                check_fields(req, &["build"])?;
                self.skills(id, name)
            }
            "items" => {
                check_fields(req, &["build", "text"])?;
                let with_text = opt_bool(req, "text")?.unwrap_or(false);
                self.items(id, name, with_text)
            }
            "config" => {
                check_fields(req, &["build"])?;
                self.config(id, name)
            }
            "notes" => {
                check_fields(req, &["build"])?;
                self.notes(id, name)
            }
            "export" => {
                check_fields(req, &["build", "format"])?;
                match opt_str(req, "format")?.unwrap_or("xml") {
                    "xml" => {}
                    f => return Err(format!("not ported: export as {f}; xml is")),
                }
                let o = self.open(id)?;
                let xml = cmd(&mut o.session, json!({ "cmd": "saveXml" }))?["xml"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                if xml.len() > text::EXPORT_CAP {
                    return Err(format!(
                        "the build's XML is over {} bytes",
                        text::EXPORT_CAP
                    ));
                }
                let mut f = Map::new();
                // Kept as written: an export is for the agent's scripts, so
                // only control characters XML can't hold anyway are absent
                f.insert("xml".into(), json!(xml));
                let (k, v) = untrusted(f);
                Ok(json!({ "build": name, "version": o.version, "format": "xml", k: v }))
            }
            "search" => {
                check_fields(req, &["build", "kind", "query", "limit"])?;
                self.search(id, req)
            }
            "history" => {
                check_fields(req, &["build", "n"])?;
                let n = opt_u64(req, "n")?.unwrap_or(20).min(200) as usize;
                let log = self.store.log(id, n)?;
                let versions: Vec<J> = log
                    .iter()
                    .map(|e| {
                        let mut o = Map::new();
                        o.insert("version".into(), json!(short(&e.version)));
                        o.insert("time".into(), json!(e.time));
                        o.insert("label".into(), json!(clean(&e.label, LINE_CAP, false)));
                        o.insert("source".into(), json!(clean(&e.source, NAME_CAP, false)));
                        for (k, v) in &e.trailers {
                            o.insert(clean(k, NAME_CAP, false), json!(clean(v, NAME_CAP, false)));
                        }
                        J::Object(o)
                    })
                    .collect();
                Ok(json!({ "build": name, "versions": versions }))
            }
            "changes_since" => {
                check_fields(req, &["build", "version"])?;
                let since = opt_u64(req, "version")?.unwrap_or(0);
                let o = self.open(id)?;
                let changes: Vec<J> = o
                    .changes
                    .iter()
                    .filter(|c| c.version > since)
                    .map(|c| {
                        json!({
                            "version": c.version,
                            "label": c.label,
                            "source": c.source,
                            "commit": c.commit.as_deref().map(short),
                        })
                    })
                    .collect();
                Ok(json!({ "build": name, "version": o.version, "changes": changes }))
            }
            _ => unreachable!("{verb}"),
        }
    }

    fn summary(&mut self, id: &BuildId, name: String) -> R<J> {
        self.with_open(id, |a, o| a.summary_of(o, name))
    }

    fn summary_of(&mut self, o: &mut crate::Open<'d>, name: String) -> R<J> {
        let s = &mut o.session;
        let gems = self.gem_names(s)?.clone();
        let uniques = self.unique_names(s)?.clone();
        let state = cmd(s, json!({ "cmd": "state" }))?;
        let skills = cmd(s, json!({ "cmd": "skills" }))?;
        let out = cmd(s, json!({ "cmd": "outputs" }))?;
        let p = &out["player"];
        let mut groups: Vec<Vec<String>> = vec![Vec::new()];
        for row in state["stats"].as_array().into_iter().flatten() {
            match row.as_array() {
                Some(r) if r.len() >= 2 => {
                    let label = plain(r[0].as_str().unwrap_or_default());
                    let label = label.trim().trim_end_matches(':');
                    let value = plain(r[1].as_str().unwrap_or_default());
                    groups.last_mut().unwrap().push(format!("{label}: {value}"));
                }
                _ => {
                    if !groups.last().unwrap().is_empty() {
                        groups.push(Vec::new());
                    }
                }
            }
        }
        groups.retain(|g| !g.is_empty());
        let mut warnings: Vec<String> = state["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(J::as_str)
            .map(|w| {
                // The jewel's name is the author's
                if w.starts_with("You are exceeding jewel limit with the jewel") {
                    "You are exceeding the jewel limit of a jewel".to_string()
                } else if let Some((head, what)) = w.split_once(" requirement of ") {
                    // A gem's or a known unique's name, else an item's the
                    // author named
                    let what = if gems.contains(what) || uniques.contains(what) {
                        what
                    } else {
                        "an item"
                    };
                    clean(&format!("{head} requirement of {what}"), LINE_CAP, false)
                } else {
                    clean(w, LINE_CAP, false)
                }
            })
            .collect();
        for res in ["Fire", "Cold", "Lightning", "Chaos"] {
            let missing = p[format!("Missing{res}Resist")].as_f64().unwrap_or(0.0);
            if missing > 0.0 && res != "Chaos" {
                warnings.push(format!("{res} resistance {missing} below its cap"));
            }
        }
        for pool in ["Life", "Mana"] {
            if p[format!("{pool}Unreserved")]
                .as_f64()
                .is_some_and(|v| v < 0.0)
            {
                warnings.push(format!("{pool} reserved over 100%"));
            }
        }
        let controls = &state["controls"];
        let main_skill = main_skill(controls, &skills);
        Ok(json!({
            "build": name,
            "version": o.version,
            "class": state["className"],
            "ascendancy": state["ascendClassName"],
            "level": state["level"],
            "points": {
                "used": state["points"]["used"],
                "ascendancy": state["points"]["ascendancy"],
            },
            "main_skill": main_skill,
            "stats": groups,
            "warnings": warnings,
            "exact": "every number is the engine's, on the configuration `config` shows",
        }))
    }

    fn tree(&mut self, id: &BuildId, name: String) -> R<J> {
        let o = self.open(id)?;
        let s = &mut o.session;
        let state = cmd(s, json!({ "cmd": "treeState" }))?;
        let tree = cmd(s, json!({ "cmd": "tree" }))?;
        let sidebar = cmd(s, json!({ "cmd": "state" }))?;
        let alloc: std::collections::HashSet<u64> = state["alloc"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(J::as_u64)
            .collect();
        let mut keystones = Vec::new();
        let mut notables = Vec::new();
        let mut ascendancy = Vec::new();
        let mut masteries = Vec::new();
        let (mut small, mut sockets) = (0, 0);
        for n in tree["nodes"].as_array().into_iter().flatten() {
            let nid = n["id"].as_u64().unwrap_or(0);
            if !alloc.contains(&nid) {
                continue;
            }
            let label = node_label(n);
            match n["kind"].as_str().unwrap_or_default() {
                "ClassStart" | "AscendClassStart" => {}
                _ if n.get("ascendancy").is_some() => ascendancy.push(label),
                "Keystone" => keystones.push(label),
                "Notable" => notables.push(label),
                "Mastery" => {
                    let sel = state["masteries"][nid.to_string()].as_f64();
                    let options = cmd(s, json!({ "cmd": "masteryOptions", "id": nid }))?;
                    let effect = options
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|e| e["id"].as_f64() == sel)
                        .and_then(|e| e["label"].as_str())
                        .map(|l| clean(l, LINE_CAP, false));
                    masteries.push(json!({ "node": label, "effect": effect }));
                }
                "Socket" | "ClusterSocket" => sockets += 1,
                _ => small += 1,
            }
        }
        Ok(json!({
            "build": name,
            "version": o.version,
            "class": sidebar["className"],
            "ascendancy": sidebar["ascendClassName"],
            "tree_version": sidebar["treeVersion"],
            "points": {
                "used": sidebar["points"]["used"],
                "ascendancy": sidebar["points"]["ascendancy"],
            },
            "keystones": keystones,
            "notables": notables,
            "masteries": masteries,
            "ascendancy_nodes": ascendancy,
            "jewel_sockets": sockets,
            "small_nodes": small,
            "names": "nodes are `Name #id`; changes take a name or the id",
        }))
    }

    fn skills(&mut self, id: &BuildId, name: String) -> R<J> {
        let (mut v, version) =
            self.with_open(id, |a, o| Ok((a.skills_of(&mut o.session)?, o.version)))?;
        v["build"] = json!(name);
        v["version"] = json!(version);
        Ok(v)
    }

    fn skills_of(&mut self, s: &mut Dispatcher<'d>) -> R<J> {
        let skills = cmd(s, json!({ "cmd": "skills" }))?;
        let gems = self.gem_names(s)?;
        let groups: Vec<J> = skills["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, g)| {
                let list: Vec<String> = g["gems"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|gem| gem_label(gem, gems))
                    .collect();
                let mut o = Map::new();
                o.insert("group".into(), json!(i + 1));
                o.insert("enabled".into(), g["enabled"].clone());
                o.insert("full_dps".into(), g["includeInFullDPS"].clone());
                if let Some(slot) = g["slot"].as_str() {
                    o.insert("slot".into(), json!(slot));
                }
                match g["source"].as_str() {
                    Some(s) if s.starts_with("Item:") => {
                        o.insert("granted_by".into(), json!("an item"));
                    }
                    Some(_) => {
                        o.insert("granted_by".into(), json!("the build"));
                    }
                    None => {}
                }
                o.insert("gems".into(), json!(list));
                J::Object(o)
            })
            .collect();
        Ok(json!({
            "main_group": skills["mainSocketGroup"],
            "groups": groups,
            "gems": "`Name level/quality`, then `support` or `disabled`",
            "skill_sets": skills["skillSets"].as_array().map(|a| a.iter().map(|s| s["id"].clone()).collect::<Vec<_>>()),
            "active_skill_set": skills["activeSkillSet"],
        }))
    }

    fn items(&mut self, id: &BuildId, name: String, with_text: bool) -> R<J> {
        let data = self.data;
        let (mut v, version) = self.with_open(id, |a, o| {
            let items = cmd(&mut o.session, json!({ "cmd": "items" }))?;
            let uniques = a.unique_names(&mut o.session)?;
            Ok((describe_items(data, &items, uniques, with_text), o.version))
        })?;
        v["build"] = json!(name);
        v["version"] = json!(version);
        Ok(v)
    }

    fn config(&mut self, id: &BuildId, name: String) -> R<J> {
        let o = self.open(id)?;
        let s = &mut o.session;
        let config = cmd(s, json!({ "cmd": "config" }))?;
        let mut set = Vec::new();
        for sec in config["sections"].as_array().into_iter().flatten() {
            for v in sec["vars"].as_array().into_iter().flatten() {
                if v.get("value").is_none() {
                    continue;
                }
                set.push(config_option(v, sec));
            }
        }
        let custom = custom_mods(s)?;
        let lines: usize = custom
            .iter()
            .map(|(_, t)| t.lines().filter(|l| !l.trim().is_empty()).count())
            .sum();
        Ok(json!({
            "build": name,
            "version": o.version,
            "set": set,
            "custom_mods": format!("{} groups, {lines} lines; their text is in `notes`", custom.len()),
            "config_sets": config["configSets"].as_array().map(|a| a.iter().map(|s| s["id"].clone()).collect::<Vec<_>>()),
            "active_config_set": config["activeConfigSet"],
            "options": "only the options set are listed; `search` with kind `config` finds the others",
        }))
    }

    fn notes(&mut self, id: &BuildId, name: String) -> R<J> {
        let o = self.open(id)?;
        let s = &mut o.session;
        let notes = cmd(s, json!({ "cmd": "notes" }))?;
        let skills = cmd(s, json!({ "cmd": "skills" }))?;
        let items = cmd(s, json!({ "cmd": "items" }))?;
        let config = cmd(s, json!({ "cmd": "config" }))?;
        let specs = cmd(s, json!({ "cmd": "treeSpecs" }))?;
        let titles = |list: &J, key: &str| -> J {
            list[key]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| json!({ "id": s["id"], "title": clean(s["title"].as_str().unwrap_or_default(), NAME_CAP, false) }))
                .collect()
        };
        let labels: Vec<J> = skills["groups"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(i, g)| {
                let l = g["label"].as_str().filter(|l| !l.is_empty())?;
                Some(json!({ "group": i + 1, "label": clean(l, NAME_CAP, false) }))
            })
            .collect();
        let tree_specs: Vec<J> = specs["list"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(i, t)| json!({ "index": i + 1, "title": clean(t.as_str().unwrap_or_default(), NAME_CAP, false) }))
            .collect();
        let custom: Vec<J> = custom_mods(s)?
            .iter()
            .take(50)
            .map(|(title, text)| {
                let lines: Vec<String> = text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .take(200)
                    .map(|l| clean(l, LINE_CAP, false))
                    .collect();
                json!({ "title": clean(title, NAME_CAP, false), "lines": lines })
            })
            .collect();
        let mut f = Map::new();
        f.insert(
            "notes".into(),
            json!(clean(
                notes["text"].as_str().unwrap_or_default(),
                NOTES_CAP,
                true
            )),
        );
        f.insert("group_labels".into(), json!(labels));
        f.insert("tree_sets".into(), json!(tree_specs));
        f.insert("skill_sets".into(), titles(&skills, "skillSets"));
        f.insert("item_sets".into(), titles(&items, "itemSets"));
        f.insert("config_sets".into(), titles(&config, "configSets"));
        f.insert("custom_mods".into(), json!(custom));
        let (k, v) = untrusted(f);
        Ok(json!({ "build": name, "version": o.version, k: v }))
    }

    fn search(&mut self, id: &BuildId, req: &J) -> R<J> {
        let kind =
            opt_str(req, "kind")?.ok_or("kind is one of nodes, gems, uniques, bases, config")?;
        let query = opt_str(req, "query")?.unwrap_or("").to_lowercase();
        let limit = opt_u64(req, "limit")?.unwrap_or(20).clamp(1, 200) as usize;
        let matches = |s: &str| s.to_lowercase().contains(&query);
        let data = self.data;
        // Exact names first, then names starting with the query, then
        // names holding it, then the rest
        let rank = |name: &str| {
            let n = name.to_lowercase();
            if n == query {
                0
            } else if n.starts_with(&query) {
                1
            } else if n.contains(&query) {
                2
            } else {
                3
            }
        };
        let mut found: Vec<(u8, J)> = Vec::new();
        let mut push = |name: &str, v: J| found.push((rank(name), v));
        match kind {
            "nodes" => {
                let o = self.open(id)?;
                let tree = cmd(&mut o.session, json!({ "cmd": "tree" }))?;
                let state = cmd(&mut o.session, json!({ "cmd": "treeState" }))?;
                let alloc: std::collections::HashSet<u64> = state["alloc"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(J::as_u64)
                    .collect();
                for n in tree["nodes"].as_array().into_iter().flatten() {
                    let stats: Vec<String> = n["stats"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(J::as_str)
                        .map(|l| clean(l, LINE_CAP, false))
                        .collect();
                    let name = n["name"].as_str().unwrap_or_default();
                    if matches(name) || stats.iter().any(|l| matches(l)) {
                        let nid = n["id"].as_u64().unwrap_or(0);
                        let mut o = json!({
                            "node": node_label(n),
                            "kind": n["kind"],
                            "stats": stats,
                            "allocated": alloc.contains(&nid),
                        });
                        if let Some(a) = n["ascendancy"].as_str() {
                            o["ascendancy"] = json!(a);
                        }
                        push(name, o);
                    }
                }
            }
            "gems" => {
                let o = self.open(id)?;
                let list = cmd(&mut o.session, json!({ "cmd": "gemList" }))?;
                for g in list.as_array().into_iter().flatten() {
                    let name = g["name"].as_str().unwrap_or_default();
                    let tags = g["tags"].as_str().unwrap_or_default();
                    if matches(name) || matches(tags) {
                        push(
                            name,
                            json!({ "gem": name, "support": g["support"], "tags": tags }),
                        );
                    }
                }
            }
            "uniques" => {
                let o = self.open(id)?;
                let list = cmd(&mut o.session, json!({ "cmd": "itemDb", "kind": "unique" }))?;
                for u in list.as_array().into_iter().flatten() {
                    let name = u["name"].as_str().unwrap_or_default();
                    let raw = u["raw"].as_str().unwrap_or_default();
                    if matches(name) || matches(raw) {
                        push(
                            name,
                            json!({ "unique": name, "type": u["type"], "text": raw }),
                        );
                    }
                }
            }
            "bases" => {
                for b in data.game.items.bases() {
                    if matches(&b.name) || matches(&b.typ) {
                        push(
                            &b.name,
                            json!({ "base": b.name, "type": b.typ, "sub_type": b.sub_type }),
                        );
                    }
                }
            }
            "config" => {
                let o = self.open(id)?;
                let config = cmd(&mut o.session, json!({ "cmd": "config" }))?;
                for sec in config["sections"].as_array().into_iter().flatten() {
                    for v in sec["vars"].as_array().into_iter().flatten() {
                        let label = plain(v["label"].as_str().unwrap_or_default());
                        if matches(&label) || matches(v["var"].as_str().unwrap_or_default()) {
                            push(&label, config_option(v, sec));
                        }
                    }
                }
            }
            k => {
                return Err(format!(
                    "no search kind {k:?}: nodes, gems, uniques, bases, config"
                ));
            }
        }
        found.sort_by_key(|(r, _)| *r);
        let total = found.len();
        let found: Vec<J> = found.into_iter().take(limit).map(|(_, v)| v).collect();
        Ok(json!({ "kind": kind, "found": found, "total": total }))
    }
}

/// A version as shown: its first 10 characters
pub(crate) fn short(v: &str) -> String {
    v.chars().take(10).collect()
}

/// A node as answers name it, `Name #id`
pub(crate) fn node_label(n: &J) -> String {
    format!(
        "{} #{}",
        clean(n["name"].as_str().unwrap_or_default(), NAME_CAP, false),
        n["id"]
    )
}

/// A gem as `skills` lists it; a name the engine doesn't know is the
/// author's and isn't quoted
fn gem_label(gem: &J, known: &std::collections::HashSet<String>) -> String {
    let name = gem["name"].as_str().unwrap_or_default();
    let name = if name.is_empty() {
        "(empty)"
    } else if known.contains(name) {
        name
    } else {
        "(a gem the engine doesn't know)"
    };
    let mut s = format!(
        "{name} {}/{}",
        gem["level"].as_f64().unwrap_or(0.0),
        gem["quality"].as_f64().unwrap_or(0.0)
    );
    if gem["support"] == true {
        s.push_str(" support");
    }
    if gem["enabled"] == false {
        s.push_str(" disabled");
    }
    if gem["count"].as_f64().is_some_and(|c| c != 1.0) {
        s.push_str(&format!(" x{}", gem["count"]));
    }
    s
}

/// The main skill, by its gem's name: the sidebar's entry, or for one
/// from an item (whose entry names the item) the main group's first skill
fn main_skill(controls: &J, skills: &J) -> J {
    let d = &controls["build.mainSkill"];
    let sel = d["sel"].as_u64().unwrap_or(1) as usize;
    if let Some(e) = d["list"].get(sel.saturating_sub(1)).and_then(J::as_str) {
        let e = plain(e);
        if !e.starts_with("From ") {
            return json!(e);
        }
    }
    let main = skills["mainSocketGroup"].as_u64().unwrap_or(1) as usize;
    skills["groups"]
        .get(main.saturating_sub(1))
        .and_then(|g| g["gems"].as_array()?.iter().find(|g| g["support"] == false))
        .and_then(|g| g["name"].as_str())
        .map(|n| json!(n))
        .unwrap_or(J::Null)
}

/// A config option as `config` and `search` give it
fn config_option(v: &J, sec: &J) -> J {
    let mut o = Map::new();
    o.insert("option".into(), v["var"].clone());
    o.insert(
        "label".into(),
        json!(plain(v["label"].as_str().unwrap_or_default()).trim_end_matches(':')),
    );
    o.insert("section".into(), sec["label"].clone());
    match v["class"].as_str() {
        Some("DropDownControl") => {
            let sel = v["sel"].as_u64().unwrap_or(1) as usize;
            o.insert("value".into(), v["list"][sel.saturating_sub(1)].clone());
            o.insert("choices".into(), v["list"].clone());
        }
        Some("CheckBoxControl") => {
            o.insert("value".into(), v["state"].clone());
        }
        _ => {
            let t = v["text"].as_str().unwrap_or_default();
            o.insert(
                "value".into(),
                match t.parse::<f64>() {
                    Ok(n) => json!(n),
                    Err(_) if t.is_empty() => J::Null,
                    Err(_) => json!(clean(t, NAME_CAP, false)),
                },
            );
        }
    }
    J::Object(o)
}

/// The config's custom modifier groups, as the author wrote them: each
/// one's title and text. The groups' controls, `configTab.<i>`, follow the
/// options' (which answer as not controls of a group).
fn custom_mods(s: &mut Dispatcher) -> R<Vec<(String, String)>> {
    let mut out = Vec::new();
    for i in 0..4000 {
        let (title, text) = (
            format!("configTab.{i}.titleEdit"),
            format!("configTab.{i}.textEdit"),
        );
        let Ok(c) = cmd(s, json!({ "cmd": "controls", "paths": [&title, &text] })) else {
            continue;
        };
        if c.as_object().is_none_or(|o| o.is_empty()) {
            break;
        }
        let get = |k: &str| c[k]["text"].as_str().unwrap_or_default().to_string();
        out.push((get(&title), get(&text)));
    }
    Ok(out)
}

/// The lines of an item the engine understood, as it reads them; how many
/// it didn't
fn engine_lines(item: &Item) -> (Vec<String>, usize) {
    let mut lines = Vec::new();
    let mut unparsed = 0;
    let groups: [(&str, &Vec<ModLine>); 5] = [
        ("enchant", &item.enchant_mod_lines),
        ("implicit", &item.implicit_mod_lines),
        ("explicit", &item.explicit_mod_lines),
        ("crucible", &item.crucible_mod_lines),
        ("scourge", &item.scourge_mod_lines),
    ];
    for (_, list) in groups {
        for l in list {
            if !item.check_mod_line_variant(l) {
                continue;
            }
            if l.extra.is_some() || l.mod_list.is_empty() {
                unparsed += 1;
            } else {
                lines.push(clean(&l.line, LINE_CAP, false));
            }
        }
    }
    (lines, unparsed)
}

fn describe_items(
    data: &session::typed::TypedData,
    items: &J,
    uniques: &std::collections::HashSet<String>,
    with_text: bool,
) -> J {
    let ctx = data.item_context();
    let by_id: std::collections::BTreeMap<u64, &J> = items["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| Some((i["id"].as_u64()?, i)))
        .collect();
    let mut equipped = std::collections::HashSet::new();
    let describe = |i: &J| -> Map<String, J> {
        let mut o = Map::new();
        o.insert("id".into(), i["id"].clone());
        let base = i["baseName"].as_str().unwrap_or_default();
        o.insert("base".into(), json!(clean(base, NAME_CAP, false)));
        o.insert("rarity".into(), i["rarity"].clone());
        // A unique's name is the game's when the engine knows it
        if let Some(n) = i["name"].as_str()
            && uniques.contains(n)
        {
            o.insert("unique".into(), json!(i["title"]));
        }
        match Item::new(&ctx, i["raw"].as_str().unwrap_or_default(), None, false) {
            Ok(item) => {
                let (lines, unparsed) = engine_lines(&item);
                o.insert("lines".into(), json!(lines));
                if unparsed > 0 {
                    o.insert("unparsed_lines".into(), json!(unparsed));
                }
            }
            Err(_) => {
                o.insert("lines".into(), json!("the engine can't read this item"));
            }
        }
        o
    };
    let mut slots = Vec::new();
    for s in items["slots"].as_array().into_iter().flatten() {
        if s["shown"] != true {
            continue;
        }
        let Some(item) = s["item"].as_u64().and_then(|i| by_id.get(&i)) else {
            continue;
        };
        equipped.insert(s["item"].as_u64().unwrap_or(0));
        let mut o = Map::new();
        let slot = s["name"].as_str().unwrap_or_default();
        o.insert("slot".into(), json!(slot));
        o.insert(
            "item".into(),
            json!(format!(
                "{slot}: {}",
                clean(
                    item["baseName"].as_str().unwrap_or_default(),
                    NAME_CAP,
                    false
                )
            )),
        );
        o.extend(describe(item));
        slots.push(J::Object(o));
    }
    let others: Vec<J> = by_id
        .iter()
        .filter(|(id, _)| !equipped.contains(*id))
        .map(|(_, i)| {
            let d = describe(i);
            json!({ "id": d["id"], "base": d["base"], "rarity": d["rarity"] })
        })
        .collect();
    let mut out = json!({
        "equipped": slots,
        "unequipped": others,
        "item_sets": items["itemSets"].as_array().map(|a| a.iter().map(|s| s["id"].clone()).collect::<Vec<_>>()),
        "active_item_set": items["activeItemSet"],
        "names": "items are named by slot and base; `lines` are the engine's reading of the mods it understood",
    });
    if with_text {
        let texts: Vec<J> = by_id
            .values()
            .map(|i| {
                let raw: Vec<String> = i["raw"]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .take(100)
                    .map(|l| clean(l, LINE_CAP, false))
                    .collect();
                json!({
                    "id": i["id"],
                    "name": clean(i["name"].as_str().unwrap_or_default(), NAME_CAP, false),
                    "raw": raw,
                })
            })
            .collect();
        let mut f = Map::new();
        f.insert("items".into(), json!(texts));
        let (k, v) = untrusted(f);
        out[k] = v;
    }
    out
}
