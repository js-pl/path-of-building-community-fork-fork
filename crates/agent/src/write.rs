//! The changing verbs. Each call is one undo step: it runs its changes on
//! the session, saves the build, commits it through the store and only
//! then answers, with what changed. A change that fails leaves the build
//! as it was.

use crate::read::{DIFF_STATS, node_label, short};
use crate::text::{LINE_CAP, NAME_CAP, clean, plain};
use crate::{
    Agent, BuildId, Change, Grant, Open, R, Step, Store, UNDO_DEPTH, check_fields, cmd, need_str,
    opt_bool, opt_str, opt_u64,
};
use serde_json::{Value as J, json};
use session::Dispatcher;
use std::collections::HashSet;

/// The verbs `apply` takes, with their fields
const CHANGES: &[(&str, &[&str])] = &[
    ("allocate", &["nodes"]),
    ("deallocate", &["nodes"]),
    ("set_mastery", &["node", "effect"]),
    ("set_ascendancy", &["ascendancy"]),
    ("set_level", &["level"]),
    (
        "set_gem",
        &["group", "gem", "name", "level", "quality", "enabled"],
    ),
    ("remove_gem", &["group", "gem"]),
    ("add_group", &["gems", "slot", "full_dps"]),
    ("set_group", &["group", "enabled", "full_dps", "slot"]),
    ("set_main_skill", &["group", "skill"]),
    ("add_item", &["text", "slot"]),
    ("equip", &["slot", "item"]),
    ("set_config", &["option", "value"]),
    ("select_set", &["kind", "id"]),
];

/// An item's text cap
const ITEM_CAP: usize = 20_000;
/// Changes in one `apply`
const MAX_CHANGES: usize = 200;
/// A label's cap
const LABEL_CAP: usize = 200;

/// The stats whose changes go in a commit's trailers
const TRAILER_STATS: &[&str] = &["Full DPS", "EHP", "Life", "Energy Shield"];

impl<'d, S: Store> Agent<'d, S> {
    pub(crate) fn write(&mut self, grant: &Grant, id: &BuildId, verb: &str, req: &J) -> R<J> {
        let expect = opt_u64(req, "expect")?;
        let name = grant.api_name(id);
        match verb {
            "undo" => {
                check_fields(req, &["build", "expect"])?;
                self.undo(grant, id, name, expect)
            }
            "restore" => {
                check_fields(req, &["build", "expect", "version"])?;
                let version = need_str(req, "version")?.to_string();
                self.restore(grant, id, name, expect, &version)
            }
            "apply" => {
                check_fields(req, &["build", "expect", "changes"])?;
                let changes = req["changes"]
                    .as_array()
                    .ok_or("changes is a list of changes, each {\"verb\": ..., ...}")?
                    .clone();
                if changes.is_empty() || changes.len() > MAX_CHANGES {
                    return Err(format!("apply takes 1 to {MAX_CHANGES} changes"));
                }
                for c in &changes {
                    check_change(c)?;
                }
                self.step(grant, id, name, expect, &changes)
            }
            _ => {
                let fields = CHANGES
                    .iter()
                    .find(|(v, _)| *v == verb)
                    .map(|(_, f)| *f)
                    .ok_or_else(|| format!("no verb {verb}"))?;
                let mut allowed = vec!["build", "expect"];
                allowed.extend(fields);
                check_fields(req, &allowed)?;
                let mut c = req.clone();
                if let Some(o) = c.as_object_mut() {
                    o.remove("build");
                    o.remove("expect");
                }
                self.step(grant, id, name, expect, &[c])
            }
        }
    }

    /// Runs `f` on the build's open session, kept out of the map meanwhile
    pub(crate) fn with_open<T>(
        &mut self,
        id: &BuildId,
        f: impl FnOnce(&mut Self, &mut Open<'d>) -> R<T>,
    ) -> R<T> {
        self.open(id)?;
        let mut o = self.open.remove(id).unwrap();
        let r = f(self, &mut o);
        self.open.insert(id.clone(), o);
        r
    }

    /// One undo step of changes
    fn step(
        &mut self,
        grant: &Grant,
        id: &BuildId,
        name: String,
        expect: Option<u64>,
        changes: &[J],
    ) -> R<J> {
        self.with_open(id, |agent, o| {
            check_version(o, expect)?;
            let before = stats(&mut o.session)?;
            let mut labels = Vec::new();
            let mut done = Vec::new();
            for c in changes {
                match agent.change(&mut o.session, c) {
                    Ok((label, d)) => {
                        labels.push(label);
                        done.push(d);
                    }
                    Err(e) => {
                        reload(o)?;
                        let verb = c["verb"].as_str().unwrap_or("?");
                        return Err(if changes.len() > 1 {
                            format!("{verb}: {e}; nothing was changed")
                        } else {
                            format!("{e}; nothing was changed")
                        });
                    }
                }
            }
            let label = clean(&labels.join("; "), LABEL_CAP, false);
            agent.finish(grant, id, name, o, before, label, done, false)
        })
    }

    /// Saves and commits a changed build, and answers
    #[allow(clippy::too_many_arguments)]
    fn finish(
        &mut self,
        grant: &Grant,
        id: &BuildId,
        name: String,
        o: &mut Open<'d>,
        before: Vec<Option<f64>>,
        label: String,
        done: Vec<J>,
        restore: bool,
    ) -> R<J> {
        let xml = save(&mut o.session)?;
        if xml == o.xml {
            return Ok(json!({
                "build": name,
                "version": o.version,
                "unchanged": true,
                "label": label,
                "done": done,
            }));
        }
        let after = stats(&mut o.session)?;
        let (changed, trailers) = diff(&before, &after);
        let full = match trailers.first() {
            Some((k, v)) => format!("{label}, {k} {v}"),
            None => label.clone(),
        };
        let step = Step {
            label: full.clone(),
            agent: grant.connection.clone(),
            restore,
            trailers,
        };
        let commit = match self.store.commit(id, &xml, &step) {
            Ok(c) => c,
            Err(e) => {
                reload(o)?;
                return Err(format!(
                    "the build could not be saved, so nothing was changed: {e}"
                ));
            }
        };
        let prev = std::mem::replace(&mut o.xml, xml);
        o.undo.push((prev, full.clone()));
        if o.undo.len() > UNDO_DEPTH {
            o.undo.remove(0);
        }
        o.version += 1;
        o.changes.push(Change {
            version: o.version,
            label: full.clone(),
            source: format!("agent {}", grant.connection),
            commit: commit.clone(),
        });
        Ok(json!({
            "build": name,
            "version": o.version,
            "commit": commit.as_deref().map(short),
            "label": full,
            "changed": changed,
            "done": done,
        }))
    }

    fn undo(&mut self, grant: &Grant, id: &BuildId, name: String, expect: Option<u64>) -> R<J> {
        self.with_open(id, |agent, o| {
            check_version(o, expect)?;
            let Some((xml, label)) = o.undo.pop() else {
                return Err(
                    "nothing to undo since the build was opened; `history` and `restore` reach older versions"
                        .into(),
                );
            };
            let before = stats(&mut o.session)?;
            if let Err(e) = load(&mut o.session, &xml, &id.name) {
                o.undo.push((xml, label));
                reload(o)?;
                return Err(e);
            }
            let r = agent.finish(
                grant,
                id,
                name,
                o,
                before,
                format!("undo {label}"),
                vec![],
                false,
            );
            // The undone step isn't a step to undo
            if r.as_ref().is_ok_and(|v| v.get("unchanged").is_none()) {
                o.undo.pop();
            }
            r
        })
    }

    fn restore(
        &mut self,
        grant: &Grant,
        id: &BuildId,
        name: String,
        expect: Option<u64>,
        version: &str,
    ) -> R<J> {
        let xml = self.store.read_version(id, version)?;
        self.with_open(id, |agent, o| {
            check_version(o, expect)?;
            let before = stats(&mut o.session)?;
            if let Err(e) = load(&mut o.session, &xml, &id.name) {
                reload(o)?;
                return Err(format!("version {version}: {e}"));
            }
            let label = format!("restore {}", short(version));
            agent.finish(grant, id, name, o, before, label, vec![], true)
        })
    }

    /// `copy`: a build of the grant, as it is now, into the agent directory
    pub(crate) fn copy(&mut self, grant: &Grant, req: &J) -> R<J> {
        check_fields(req, &["build", "name"])?;
        let from = grant.resolve(opt_str(req, "build")?)?;
        let to = grant.resolve(Some(&format!(
            "{}{}",
            crate::AGENT_PREFIX,
            need_str(req, "name")?
        )))?;
        let xml = match self.open.get(&from) {
            Some(o) => o.xml.clone(),
            None => self
                .store
                .read(&from)?
                .ok_or_else(|| format!("no build {}", from.name))?,
        };
        let label = format!("copy of {}", grant.api_name(&from));
        self.create(grant, &to, &xml, label)
    }

    /// `import`: a build from a build code or XML into the agent directory
    pub(crate) fn import(&mut self, grant: &Grant, req: &J) -> R<J> {
        check_fields(req, &["name", "code", "xml"])?;
        let to = grant.resolve(Some(&format!(
            "{}{}",
            crate::AGENT_PREFIX,
            need_str(req, "name")?
        )))?;
        let xml = match (opt_str(req, "code")?, opt_str(req, "xml")?) {
            (Some(code), None) => {
                session::dispatch::decode_build_code(code.trim()).ok_or("not a build code")?
            }
            (None, Some(xml)) => xml.to_string(),
            _ => return Err("import takes a build code (`code`) or XML (`xml`)".into()),
        };
        // Read by the engine before it is kept
        let mut s = Dispatcher::typed(self.data);
        load(&mut s, &xml, &to.name)?;
        let xml = save(&mut s)?;
        self.create(grant, &to, &xml, "import".into())
    }

    fn create(&mut self, grant: &Grant, to: &BuildId, xml: &str, label: String) -> R<J> {
        let step = Step {
            label,
            agent: grant.connection.clone(),
            restore: false,
            trailers: Vec::new(),
        };
        let commit = self.store.create(to, xml, &step)?;
        Ok(json!({
            "build": grant.api_name(to),
            "commit": commit.as_deref().map(short),
            "note": "a build of the agent directory: edit it freely; the user keeps it or cleans it up",
        }))
    }

    /// Runs one change: its label and what it did
    fn change(&mut self, s: &mut Dispatcher<'d>, c: &J) -> R<(String, J)> {
        match c["verb"].as_str().unwrap_or_default() {
            "allocate" => allocate(s, &c["nodes"], true),
            "deallocate" => allocate(s, &c["nodes"], false),
            "set_mastery" => set_mastery(s, c),
            "set_ascendancy" => {
                let want = need_str(c, "ascendancy")?;
                let state = cmd(s, json!({ "cmd": "state" }))?;
                let list = &state["controls"]["build.ascendDrop"]["list"];
                let i = pick(list, want, "ascendancy")?;
                let name = list[i].as_str().unwrap_or_default().to_string();
                cmd(
                    s,
                    json!({ "cmd": "control", "path": "build.ascendDrop", "sel": i + 1 }),
                )?;
                Ok((format!("ascendancy {name}"), json!({ "ascendancy": name })))
            }
            "set_level" => {
                let level = opt_u64(c, "level")?.ok_or("level is missing")?;
                if !(1..=100).contains(&level) {
                    return Err("level is 1 to 100".into());
                }
                cmd(
                    s,
                    json!({ "cmd": "control", "path": "build.characterLevel", "text": level.to_string() }),
                )?;
                Ok((format!("level {level}"), json!({ "level": level })))
            }
            "set_gem" => self.set_gem(s, c),
            "remove_gem" => {
                let (g, k) = (group(c)?, need_u64(c, "gem")?);
                let skills = cmd(s, json!({ "cmd": "skills" }))?;
                let gem = gem_of(&skills, g, k)?;
                let name = self.gem_name(s, &gem)?;
                cmd(s, json!({ "cmd": "removeGem", "group": g, "gem": k }))?;
                Ok((
                    format!("remove {name} from group {g}"),
                    json!({ "removed": name }),
                ))
            }
            "add_group" => {
                let gems: Vec<String> = c["gems"]
                    .as_array()
                    .ok_or("gems is a list of gem names")?
                    .iter()
                    .map(|g| g.as_str().map(String::from).ok_or("a gem is its name"))
                    .collect::<Result<_, _>>()?;
                let known = self.gem_names(s)?;
                for g in &gems {
                    if !known.contains(g) {
                        return Err(format!(
                            "no gem {g:?}; `search` with kind `gems` finds them"
                        ));
                    }
                }
                let slot = opt_str(c, "slot")?;
                if let Some(slot) = slot {
                    check_slot(s, slot)?;
                }
                cmd(s, json!({ "cmd": "addSocketGroup" }))?;
                let skills = cmd(s, json!({ "cmd": "skills" }))?;
                let index = skills["groups"].as_array().map_or(0, Vec::len);
                for (k, g) in gems.iter().enumerate() {
                    cmd(
                        s,
                        json!({ "cmd": "setGem", "group": index, "gem": k + 1, "name": g }),
                    )?;
                }
                let mut e = json!({ "cmd": "setSocketGroup", "index": index });
                if let Some(slot) = slot {
                    e["slot"] = json!(slot);
                }
                if let Some(f) = opt_bool(c, "full_dps")? {
                    e["includeInFullDPS"] = json!(f);
                }
                cmd(s, e)?;
                Ok((
                    format!("add group {}", gems.join(", ")),
                    json!({ "group": index, "gems": gems }),
                ))
            }
            "set_group" => {
                let g = group(c)?;
                let skills = cmd(s, json!({ "cmd": "skills" }))?;
                check_group(&skills, g)?;
                let mut e = json!({ "cmd": "setSocketGroup", "index": g });
                let mut what = Vec::new();
                if let Some(b) = opt_bool(c, "enabled")? {
                    e["enabled"] = json!(b);
                    what.push(if b { "enable" } else { "disable" }.to_string());
                }
                if let Some(b) = opt_bool(c, "full_dps")? {
                    e["includeInFullDPS"] = json!(b);
                    what.push(format!("full DPS {}", if b { "on" } else { "off" }));
                }
                if let Some(slot) = opt_str(c, "slot")? {
                    if !slot.is_empty() {
                        check_slot(s, slot)?;
                    }
                    e["slot"] = json!(slot);
                    what.push(format!("slot {slot}"));
                }
                if what.is_empty() {
                    return Err("set_group takes enabled, full_dps or slot".into());
                }
                cmd(s, e)?;
                Ok((
                    format!("group {g}: {}", what.join(", ")),
                    json!({ "group": g }),
                ))
            }
            "set_main_skill" => {
                let g = group(c)?;
                let skills = cmd(s, json!({ "cmd": "skills" }))?;
                check_group(&skills, g)?;
                cmd(s, json!({ "cmd": "setMainSocketGroup", "index": g }))?;
                if let Some(k) = opt_u64(c, "skill")? {
                    cmd(
                        s,
                        json!({ "cmd": "control", "path": "build.mainSkill", "sel": k }),
                    )?;
                }
                let state = cmd(s, json!({ "cmd": "state" }))?;
                let d = &state["controls"]["build.mainSkill"];
                let sel = d["sel"].as_u64().unwrap_or(1) as usize;
                let skill = d["list"]
                    .get(sel.saturating_sub(1))
                    .and_then(J::as_str)
                    .map(plain)
                    .filter(|n| !n.starts_with("From "))
                    .unwrap_or_else(|| "a skill from an item".into());
                Ok((
                    format!("main skill {skill}"),
                    json!({ "group": g, "skill": skill }),
                ))
            }
            "add_item" => {
                let text = need_str(c, "text")?;
                if text.len() > ITEM_CAP {
                    return Err(format!("an item's text is at most {ITEM_CAP} bytes"));
                }
                let slot = opt_str(c, "slot")?;
                if let Some(slot) = slot {
                    check_slot(s, slot)?;
                }
                let mut e = json!({ "cmd": "addItem", "text": text });
                if let Some(slot) = slot {
                    e["slot"] = json!(slot);
                }
                let r = cmd(s, e)?;
                let base = item_base(s, r["id"].as_f64())?;
                let label = match slot {
                    Some(slot) => format!("add {base} in {slot}"),
                    None => format!("add {base}"),
                };
                Ok((
                    label,
                    json!({ "item": r["id"], "base": base, "slot": slot }),
                ))
            }
            "equip" => {
                let slot = need_str(c, "slot")?;
                check_slot(s, slot)?;
                let item = match c.get("item") {
                    None | Some(J::Null) => None,
                    Some(v) => Some(v.as_u64().ok_or("item is an item's id, or null")?),
                };
                if let Some(i) = item {
                    let items = cmd(s, json!({ "cmd": "items" }))?;
                    let fits = items["slots"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|x| x["name"] == slot)
                        .is_some_and(|x| {
                            x["candidates"]
                                .as_array()
                                .is_some_and(|c| c.iter().any(|c| c.as_u64() == Some(i)))
                        });
                    if !fits {
                        return Err(format!("item {i} doesn't go in {slot}"));
                    }
                }
                cmd(
                    s,
                    json!({ "cmd": "equipItem", "slot": slot, "id": item.unwrap_or(0) }),
                )?;
                let label = match item {
                    Some(i) => format!("equip {} in {slot}", item_base(s, Some(i as f64))?),
                    None => format!("empty {slot}"),
                };
                Ok((label, json!({ "slot": slot, "item": item })))
            }
            "set_config" => set_config(s, c),
            "select_set" => {
                let kind = need_str(c, "kind")?;
                let id = need_u64(c, "id")?;
                let req = match kind {
                    "tree" => json!({ "cmd": "selectTreeSpec", "index": id }),
                    "skills" => json!({ "cmd": "selectSkillSet", "id": id }),
                    "items" => json!({ "cmd": "selectItemSet", "id": id }),
                    "config" => json!({ "cmd": "selectConfigSet", "id": id }),
                    k => return Err(format!("no set kind {k:?}: tree, skills, items, config")),
                };
                cmd(s, req)?;
                Ok((
                    format!("{kind} set {id}"),
                    json!({ "kind": kind, "id": id }),
                ))
            }
            v => Err(format!("{v:?} is not a change")),
        }
    }

    fn set_gem(&mut self, s: &mut Dispatcher<'d>, c: &J) -> R<(String, J)> {
        let g = group(c)?;
        let skills = cmd(s, json!({ "cmd": "skills" }))?;
        check_group(&skills, g)?;
        let count = skills["groups"][g as usize - 1]["gems"]
            .as_array()
            .map_or(0, Vec::len) as u64;
        let k = opt_u64(c, "gem")?.unwrap_or(count + 1);
        if k == 0 || k > count + 1 {
            return Err(format!(
                "group {g} has gems 1 to {count}; {} adds one",
                count + 1
            ));
        }
        let mut e = json!({ "cmd": "setGem", "group": g, "gem": k });
        let mut what = Vec::new();
        if let Some(name) = opt_str(c, "name")? {
            if !self.gem_names(s)?.contains(name) {
                return Err(format!(
                    "no gem {name:?}; `search` with kind `gems` finds them"
                ));
            }
            e["name"] = json!(name);
            what.push(name.to_string());
        } else if k > count {
            return Err("a new gem needs its name".into());
        }
        if let Some(l) = opt_u64(c, "level")? {
            if !(1..=40).contains(&l) {
                return Err("level is 1 to 40".into());
            }
            e["level"] = json!(l);
            what.push(format!("level {l}"));
        }
        if let Some(q) = opt_u64(c, "quality")? {
            if q > 100 {
                return Err("quality is 0 to 100".into());
            }
            e["quality"] = json!(q);
            what.push(format!("quality {q}"));
        }
        if let Some(b) = opt_bool(c, "enabled")? {
            e["enabled"] = json!(b);
            what.push(if b { "enabled" } else { "disabled" }.into());
        }
        if what.is_empty() {
            return Err("set_gem takes name, level, quality or enabled".into());
        }
        let old = if k <= count {
            let gem = gem_of(&skills, g, k)?;
            Some(self.gem_name(s, &gem)?)
        } else {
            None
        };
        cmd(s, e)?;
        let label = match old {
            Some(old) => format!("group {g} gem {k} {old}: {}", what.join(", ")),
            None => format!("group {g}: add {}", what.join(", ")),
        };
        Ok((label, json!({ "group": g, "gem": k })))
    }

    /// A gem's name as answers give it
    fn gem_name(&mut self, s: &mut Dispatcher<'d>, gem: &J) -> R<String> {
        let name = gem["name"].as_str().unwrap_or_default();
        Ok(if self.gem_names(s)?.contains(name) {
            name.to_string()
        } else {
            "a gem the engine doesn't know".into()
        })
    }
}

/// Refuses a change planned on another version
fn check_version(o: &Open, expect: Option<u64>) -> R<()> {
    match expect {
        Some(v) if v != o.version => Err(format!(
            "the build changed since version {v}; it is at version {} now: look again",
            o.version
        )),
        _ => Ok(()),
    }
}

fn load(s: &mut Dispatcher, xml: &str, name: &str) -> R<()> {
    cmd(s, json!({ "cmd": "loadXml", "xml": xml, "name": name })).map(|_| ())
}

fn save(s: &mut Dispatcher) -> R<String> {
    let v = cmd(s, json!({ "cmd": "saveXml" }))?;
    v["xml"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "saveXml gave no XML".into())
}

/// Puts the build back as it was last saved
fn reload(o: &mut Open) -> R<()> {
    let name = cmd(&mut o.session, json!({ "cmd": "state" }))
        .ok()
        .and_then(|s| s["name"].as_str().map(String::from))
        .unwrap_or_default();
    load(&mut o.session, &o.xml, &name)
}

/// The stats a change is measured by, as the sidebar calculation has them
fn stats(s: &mut Dispatcher) -> R<Vec<Option<f64>>> {
    let out = cmd(s, json!({ "cmd": "outputs" }))?;
    Ok(DIFF_STATS
        .iter()
        .map(|(k, _, _)| out["player"][*k].as_f64())
        .collect())
}

/// `Full DPS 2.41M → 2.77M (+14.9%)` for each stat that changed, and the
/// commit's trailers
fn diff(before: &[Option<f64>], after: &[Option<f64>]) -> (Vec<String>, Vec<(String, String)>) {
    let mut lines = Vec::new();
    let mut trailers = Vec::new();
    for ((_, label, points), (b, a)) in DIFF_STATS.iter().zip(before.iter().zip(after)) {
        let (b, a) = (b.unwrap_or(0.0), a.unwrap_or(0.0));
        if (a - b).abs() < 1e-9 * a.abs().max(b.abs()).max(1.0) {
            continue;
        }
        let change = if *points {
            format!("{:+}", round(a - b))
        } else if b != 0.0 {
            format!("{:+.1}%", (a - b) / b.abs() * 100.0)
        } else {
            format!("{:+}", num(a - b))
        };
        lines.push(format!("{label} {} → {} ({change})", num(b), num(a)));
        if TRAILER_STATS.contains(label) {
            trailers.push((label.to_string(), change));
        }
    }
    (lines, trailers)
}

fn round(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

/// A number as answers show it: `2.41M`, `24.1k`, `312.5`
fn num(x: f64) -> String {
    let a = x.abs();
    if a >= 1e6 {
        format!("{:.2}M", x / 1e6)
    } else if a >= 1e4 {
        format!("{:.1}k", x / 1e3)
    } else {
        format!("{}", round(x))
    }
}

fn need_u64(c: &J, key: &str) -> R<u64> {
    opt_u64(c, key)?.ok_or_else(|| format!("{key} is missing"))
}

fn group(c: &J) -> R<u64> {
    let g = need_u64(c, "group")?;
    if g == 0 {
        return Err("groups count from 1, as `skills` lists them".into());
    }
    Ok(g)
}

fn check_group(skills: &J, g: u64) -> R<()> {
    let n = skills["groups"].as_array().map_or(0, Vec::len) as u64;
    if g == 0 || g > n {
        return Err(format!("no group {g}; there are {n}"));
    }
    Ok(())
}

fn gem_of(skills: &J, g: u64, k: u64) -> R<J> {
    check_group(skills, g)?;
    skills["groups"][g as usize - 1]["gems"]
        .get((k as usize).wrapping_sub(1))
        .cloned()
        .ok_or_else(|| format!("group {g} has no gem {k}"))
}

fn check_slot(s: &mut Dispatcher, slot: &str) -> R<()> {
    let items = cmd(s, json!({ "cmd": "items" }))?;
    let slots: Vec<&str> = items["slots"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| x["name"].as_str())
        .collect();
    if slots.contains(&slot) {
        Ok(())
    } else {
        Err(format!("no slot {slot:?}; the slots: {}", slots.join(", ")))
    }
}

/// An item's base, by its id
fn item_base(s: &mut Dispatcher, id: Option<f64>) -> R<String> {
    let items = cmd(s, json!({ "cmd": "items" }))?;
    let base = items["items"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["id"].as_f64() == id)
        .and_then(|i| i["baseName"].as_str())
        .unwrap_or("an item");
    Ok(clean(base, NAME_CAP, false))
}

/// The index of `want` in a list of names, by name without case
fn pick(list: &J, want: &str, what: &str) -> R<usize> {
    let names: Vec<&str> = list
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(J::as_str)
        .collect();
    names
        .iter()
        .position(|n| plain(n).eq_ignore_ascii_case(want))
        .ok_or_else(|| format!("no {what} {want:?}; the choices: {}", names.join(", ")))
}

/// A node a request names: its id, `#id`, `Name #id`, or a name only one
/// node has
fn resolve_node<'t>(tree: &'t J, r: &J) -> R<&'t J> {
    let nodes = tree["nodes"].as_array().ok_or("no tree")?;
    let by_id = |id: u64| {
        nodes
            .iter()
            .find(|n| n["id"].as_u64() == Some(id))
            .ok_or_else(|| format!("no node #{id}"))
    };
    if let Some(id) = r.as_u64() {
        return by_id(id);
    }
    let s = r.as_str().ok_or("a node is its name or its id")?.trim();
    if let Some((_, id)) = s.rsplit_once('#')
        && let Ok(id) = id.trim().parse::<u64>()
    {
        return by_id(id);
    }
    let found: Vec<&J> = nodes
        .iter()
        .filter(|n| {
            n["name"]
                .as_str()
                .is_some_and(|x| x.eq_ignore_ascii_case(s))
        })
        .collect();
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(format!(
            "no node {s:?}; `search` with kind `nodes` finds them"
        )),
        many => {
            let ids: Vec<String> = many.iter().take(8).map(|n| node_label(n)).collect();
            Err(format!(
                "{} nodes are named {s:?}; name one by its id: {}",
                many.len(),
                ids.join(", ")
            ))
        }
    }
}

fn allocated(s: &mut Dispatcher) -> R<HashSet<u64>> {
    let state = cmd(s, json!({ "cmd": "treeState" }))?;
    Ok(state["alloc"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(J::as_u64)
        .collect())
}

/// `allocate` (with each node's path) or `deallocate` (with what depends
/// on each node)
fn allocate(s: &mut Dispatcher, refs: &J, alloc: bool) -> R<(String, J)> {
    let refs = refs
        .as_array()
        .ok_or("nodes is a list of node names or ids")?;
    if refs.is_empty() {
        return Err("nodes is empty".into());
    }
    let tree = cmd(s, json!({ "cmd": "tree" }))?;
    let mut names = Vec::new();
    let mut done = Vec::new();
    let mut points = 0;
    for r in refs {
        let node = resolve_node(&tree, r)?;
        let id = node["id"].as_u64().unwrap_or(0);
        let label = node_label(node);
        let kind = node["kind"].as_str().unwrap_or_default();
        if alloc && kind == "Mastery" {
            return Err(format!(
                "{label} is a mastery: set_mastery picks its effect"
            ));
        }
        if matches!(kind, "ClassStart" | "AscendClassStart") {
            return Err(format!("{label} is a start node"));
        }
        if allocated(s)?.contains(&id) == alloc {
            done.push(json!({ "node": label, "already": true }));
            continue;
        }
        let path: Vec<J> = cmd(s, json!({ "cmd": "nodePath", "id": id }))?["path"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if alloc && path.is_empty() {
            return Err(format!("{label} can't be reached"));
        }
        cmd(s, json!({ "cmd": "clickNode", "id": id }))?;
        if allocated(s)?.contains(&id) != alloc {
            return Err(format!(
                "{label} could not be {}",
                if alloc { "allocated" } else { "deallocated" }
            ));
        }
        let path: Vec<String> = path
            .iter()
            .filter_map(|p| resolve_node(&tree, p).ok().map(node_label))
            .collect();
        points += path.len();
        names.push(clean(
            node["name"].as_str().unwrap_or_default(),
            NAME_CAP,
            false,
        ));
        let mut d = json!({ "node": label });
        d[if alloc { "path" } else { "removed" }] = json!(path);
        done.push(d);
    }
    let verb = if alloc { "allocate" } else { "deallocate" };
    let label = if names.is_empty() {
        format!("{verb} nothing new")
    } else {
        format!("{verb} {} ({points} points)", names.join(", "))
    };
    let mut d = json!({ "points": points });
    d[verb] = json!(done);
    Ok((label, d))
}

fn set_mastery(s: &mut Dispatcher, c: &J) -> R<(String, J)> {
    let tree = cmd(s, json!({ "cmd": "tree" }))?;
    let node = resolve_node(&tree, c.get("node").ok_or("node is missing")?)?;
    let id = node["id"].as_u64().unwrap_or(0);
    let label = node_label(node);
    if node["kind"] != "Mastery" {
        return Err(format!("{label} is not a mastery"));
    }
    let options = cmd(s, json!({ "cmd": "masteryOptions", "id": id }))?;
    let options = options.as_array().cloned().unwrap_or_default();
    let effect = c.get("effect").ok_or("effect is missing")?;
    let found: Vec<&J> = match effect {
        J::Number(n) => options
            .iter()
            .filter(|o| o["id"].as_u64() == n.as_u64())
            .collect(),
        J::String(t) => {
            let t = t.to_lowercase();
            let exact: Vec<&J> = options
                .iter()
                .filter(|o| o["label"].as_str().is_some_and(|l| l.to_lowercase() == t))
                .collect();
            if exact.is_empty() {
                options
                    .iter()
                    .filter(|o| {
                        o["label"]
                            .as_str()
                            .is_some_and(|l| l.to_lowercase().contains(&t))
                    })
                    .collect()
            } else {
                exact
            }
        }
        _ => return Err("effect is the effect's text or id".into()),
    };
    let effects = || {
        options
            .iter()
            .map(|o| {
                format!(
                    "{} ({})",
                    clean(o["label"].as_str().unwrap_or_default(), LINE_CAP, false),
                    o["id"]
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let [e] = found.as_slice() else {
        return Err(format!("name one effect of {label}: {}", effects()));
    };
    if !allocated(s)?.contains(&id) {
        let path = cmd(s, json!({ "cmd": "nodePath", "id": id }))?;
        if path["path"].as_array().is_none_or(|p| p.len() != 1) {
            return Err(format!(
                "{label} needs a notable next to it allocated first"
            ));
        }
    }
    cmd(
        s,
        json!({ "cmd": "selectMastery", "id": id, "effect": e["id"] }),
    )?;
    let text = clean(e["label"].as_str().unwrap_or_default(), LINE_CAP, false);
    Ok((
        format!(
            "mastery {}: {text}",
            clean(node["name"].as_str().unwrap_or_default(), NAME_CAP, false)
        ),
        json!({ "node": label, "effect": text }),
    ))
}

fn set_config(s: &mut Dispatcher, c: &J) -> R<(String, J)> {
    let want = need_str(c, "option")?;
    let value = c.get("value").ok_or("value is missing")?;
    let config = cmd(s, json!({ "cmd": "config" }))?;
    let var = config["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|sec| sec["vars"].as_array().into_iter().flatten())
        .find(|v| {
            v["var"] == want
                || plain(v["label"].as_str().unwrap_or_default())
                    .trim_end_matches(':')
                    .eq_ignore_ascii_case(want)
        })
        .ok_or_else(|| format!("no option {want:?} shown; `search` with kind `config` finds them"))?
        .clone();
    let name = var["var"].as_str().unwrap_or_default().to_string();
    let path = format!("config.{name}");
    let req = match (var["class"].as_str(), value) {
        (Some("CheckBoxControl"), J::Bool(b)) => {
            json!({ "cmd": "control", "path": path, "state": b })
        }
        (Some("DropDownControl"), J::String(v)) => {
            let i = pick(&var["list"], v, "choice")?;
            json!({ "cmd": "control", "path": path, "sel": i + 1 })
        }
        (Some("EditControl" | "ResizableEditControl"), J::Number(n)) => {
            json!({ "cmd": "control", "path": path, "text": n.to_string() })
        }
        (Some("EditControl" | "ResizableEditControl"), J::Null) => {
            json!({ "cmd": "control", "path": path, "text": "" })
        }
        (class, _) => {
            let wants = match class {
                Some("CheckBoxControl") => "true or false",
                Some("DropDownControl") => "one of its choices",
                _ => "a number, or null to clear it",
            };
            return Err(format!("{name} takes {wants}"));
        }
    };
    cmd(s, req)?;
    let shown = match value {
        J::String(v) => v.clone(),
        v => v.to_string(),
    };
    Ok((
        format!("config {name} {}", clean(&shown, NAME_CAP, false)),
        json!({ "option": name, "value": value }),
    ))
}

/// A change in `apply`: a verb it takes, with that verb's fields
fn check_change(c: &J) -> R<()> {
    let verb = c["verb"].as_str().ok_or("each change names its verb")?;
    let (_, fields) = CHANGES
        .iter()
        .find(|(v, _)| *v == verb)
        .ok_or_else(|| format!("apply doesn't take {verb:?}"))?;
    check_fields(c, fields)
}
