//! The Calcs tab's breakdowns: the typed calculation of the Calcs tab
//! (`engine::calcs::build_outputs`) writes the breakdowns the translated
//! `calcs.buildOutput(build, "CALCS")` leaves in `env.player.breakdown` and
//! `env.minion.breakdown`, value for value; a missing or extra breakdown
//! fails. Builds that reach a part not ported yet are skipped and listed.
//!
//! The breakdowns of the triggers, mirages and offence are not ported yet:
//! of the translation's, only those present when `calcs.triggers` starts
//! for the actor, and those `calcs.perform` and `calcs.buildOutput` write
//! after the offence, are compared.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::breakdown::{Breakdown, Breakdowns, Cell, Row};
use engine::items_tab::ItemsTab;
use engine::mods::convert::Converter;
use engine::mods::tag::TagKind;
use equivalence::typed_build::TypedData;
use equivalence::{corpus, typed_json, upstream_src};
use lua::rt;
use lua::{Key, LuaError, Value};
use serde_json::{Map, Value as J, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use upstream::host::Host;
use upstream::imported::Imported;

/// What the later calculations write after the offence
const LATE: [&str; 4] = ["SkillBuffs", "SkillDebuffs", "GemLevel", "GemQuality"];

fn s(x: &str) -> Value {
    Value::str(x)
}

fn get(v: &Value, k: &str) -> Value {
    rt::index(v, &s(k)).unwrap_or_default()
}

fn describe(e: &LuaError) -> String {
    format!(
        "{}\n  {}",
        lua::to_string(e.value()),
        e.traceback().join("\n  ")
    )
}

fn entries(v: &Value) -> Vec<(Key, Value)> {
    v.as_table().map(|t| t.entries()).unwrap_or_default()
}

fn number(n: f64) -> J {
    if n.is_nan() {
        json!({"$num": "nan"})
    } else if n.is_infinite() {
        json!({"$num": if n > 0.0 { "inf" } else { "-inf" }})
    } else {
        serde_json::Number::from_f64(n).map_or(J::Null, J::Number)
    }
}

fn key_str(k: &Key) -> String {
    lua::to_string(&k.to_value()).to_str_lossy().into_owned()
}

/// A Lua table as JSON: its array part under `#`, its other keys as they
/// are; functions as `$func`, items by name, mods as `typed_json` encodes
/// them
fn lua_json(v: &Value, key: &str, conv: &Converter) -> J {
    match v {
        Value::Nil => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Number(n) => number(*n),
        Value::Str(x) => J::String(x.to_str_lossy().into_owned()),
        Value::Table(_) if key == "item" => {
            json!({"$item": lua::to_string(&get(v, "name")).to_str_lossy()})
        }
        Value::Table(_) if key == "mod" => match conv.mod_(v) {
            Ok(mut m) => {
                // Upstream writes `warcryPowerBonus` on the tag the skill
                // data shares; the typed perform keeps it local
                for t in m.tags.iter_mut() {
                    if let TagKind::Other { fields, .. } = &mut t.kind {
                        fields
                            .fields
                            .retain(|(k, _)| k.as_str() != "warcryPowerBonus");
                    }
                }
                typed_json::mod_(&m)
            }
            Err(e) => json!({"$error": e}),
        },
        Value::Table(_) if key == "value" => match conv.value(v) {
            Ok(m) => typed_json::value(&m),
            Err(e) => json!({"$error": e}),
        },
        Value::Table(_) => {
            let mut m = Map::new();
            let mut list: BTreeMap<i64, J> = BTreeMap::new();
            for (k, x) in entries(v) {
                match k {
                    Key::Int(i) => {
                        list.insert(i, lua_json(&x, "", conv));
                    }
                    k => {
                        let k = key_str(&k);
                        let j = lua_json(&x, &k, conv);
                        m.insert(k, j);
                    }
                }
            }
            if !list.is_empty() {
                m.insert("#".into(), J::Array(list.into_values().collect()));
            }
            J::Object(m)
        }
        _ => json!("$func"),
    }
}

/// A translated actor's breakdowns by name, the weapons' (`MainHand`,
/// `OffHand`, breakdown modules of their own) under `<weapon>.<name>`
fn lua_breakdowns(bd: &Value, conv: &Converter) -> BTreeMap<String, J> {
    let mut out = BTreeMap::new();
    for (k, v) in entries(bd) {
        let k = key_str(&k);
        if v.as_table().is_none() {
            continue;
        }
        if matches!(get(&v, "multiChain"), Value::Function(_)) {
            for (n, x) in lua_breakdowns(&v, conv) {
                out.insert(format!("{k}.{n}"), x);
            }
        } else {
            let mut j = lua_json(&v, &k, conv);
            if let Some(slots) = j.get_mut("slots") {
                *slots = strip_false(slots.take());
            }
            out.insert(k.clone(), j);
        }
    }
    out
}

/// The rows of a list without their `false` cells: upstream leaves some
/// cells nil where others are false, which the Calcs tab shows alike
fn strip_false(mut list: J) -> J {
    if let Some(J::Array(rows)) = list.get_mut("#") {
        for r in rows {
            if let J::Object(o) = r {
                o.retain(|_, v| *v != J::Bool(false));
            }
        }
    }
    list
}

fn cell(c: &Cell) -> J {
    match c {
        Cell::Str(s) => json!(s),
        Cell::Num(n) => number(*n),
        Cell::Bool(b) => J::Bool(*b),
        Cell::ItemTooltip(_) => json!("$func"),
    }
}

fn opt_str(s: &Option<String>) -> J {
    s.as_ref().map_or(J::Bool(false), |s| json!(s))
}

/// A list as Lua's: `{}` when empty
fn list(v: Vec<J>) -> J {
    if v.is_empty() {
        json!({})
    } else {
        json!({ "#": v })
    }
}

fn rows(rows: &[Row]) -> J {
    list(
        rows.iter()
            .map(|r| J::Object(r.iter().map(|(k, c)| (k.to_string(), cell(c))).collect()))
            .collect(),
    )
}

fn typed_breakdown(b: &Breakdown, tab: &ItemsTab) -> J {
    let mut m = Map::new();
    if !b.lines.is_empty() {
        m.insert("#".into(), json!(b.lines));
    }
    if let Some(r) = b.radius {
        m.insert("radius".into(), number(r));
    }
    for (k, v) in [
        ("label", &b.label),
        ("footer", &b.footer),
        ("source", &b.source),
    ] {
        if let Some(v) = v {
            m.insert(k.into(), json!(v));
        }
    }
    if let Some(r) = &b.rows {
        m.insert("rowList".into(), rows(r));
    }
    if let Some(r) = &b.damage_types {
        m.insert("damageTypes".into(), rows(r));
    }
    if let Some(c) = &b.cols {
        let cols = c
            .iter()
            .map(|c| {
                let mut o = json!({"label": c.label, "key": c.key});
                if c.right {
                    o["right"] = J::Bool(true);
                }
                o
            })
            .collect();
        m.insert("colList".into(), list(cols));
    }
    if let Some(slots) = &b.slots {
        let v = slots
            .iter()
            .map(|s| {
                let mut o = json!({
                    "base": number(s.base),
                    "inc": opt_str(&s.inc),
                    "more": opt_str(&s.more),
                    "total": s.total,
                    "source": s.source,
                });
                if let Some(n) = &s.source_name {
                    o["sourceName"] = json!(n);
                }
                if let Some(item) = s.item.and_then(|i| tab.item(i)) {
                    o["item"] = json!({"$item": item.name});
                }
                o
            })
            .collect();
        m.insert("slots".into(), strip_false(list(v)));
    }
    if let Some(res) = &b.reservations {
        let v = res
            .iter()
            .map(|r| {
                json!({
                    "skillName": r.skill_name,
                    "base": cell(&r.base),
                    "mult": opt_str(&r.mult),
                    "more": opt_str(&r.more),
                    "inc": opt_str(&r.inc),
                    "efficiency": opt_str(&r.efficiency),
                    "efficiencyMore": opt_str(&r.efficiency_more),
                    "total": cell(&r.total),
                })
            })
            .collect();
        m.insert("reservations".into(), list(v));
    }
    if let Some(mods) = &b.mod_list {
        let v = mods
            .iter()
            .map(|e| json!({"mod": typed_json::mod_(&e.m), "value": typed_json::value(&e.value)}))
            .collect();
        m.insert("modList".into(), list(v));
    }
    J::Object(m)
}

fn typed_breakdowns(b: &Breakdowns, tab: &ItemsTab) -> BTreeMap<String, J> {
    b.map
        .iter()
        .map(|(k, v)| (k.clone(), typed_breakdown(v, tab)))
        .collect()
}

/// `false` in Lua is a missing key's value in a row: drop `false` cells
/// both sides agree on by encoding them the same; a nil cell is absent
fn compare(
    what: &str,
    got: &BTreeMap<String, J>,
    want: &BTreeMap<String, J>,
    fail: &mut Vec<String>,
) {
    for k in got.keys().chain(want.keys()).collect::<BTreeSet<_>>() {
        let (g, w) = (got.get(k), want.get(k));
        if g != w {
            let show = |v: Option<&J>| v.map_or("missing".into(), J::to_string);
            fail.push(format!("{what} {k}:\n      {}\n   != {}", show(g), show(w)));
        }
    }
}

/// The translated breakdowns the typed ones are compared with: those of
/// the actor's before `calcs.triggers`, and the late ones
fn filtered(all: BTreeMap<String, J>, before: &BTreeSet<String>) -> BTreeMap<String, J> {
    all.into_iter()
        .filter(|(k, _)| before.contains(k) || LATE.contains(&k.as_str()))
        .collect()
}

#[test]
fn breakdowns_match_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let mut data = TypedData::load(&host);
    let conv = Converter::default();
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut skips: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, path) in corpus() {
        if !id.contains(&filter) || !path.extension().is_some_and(|e| e == "xml") {
            continue;
        }
        let xml = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        let calcs = get(&get(&build, "calcsTab"), "calcs");
        // The Calcs tab's calculation again, noting the breakdowns present
        // when `calcs.triggers` starts for the player and the minion
        let main_env: Rc<RefCell<Option<Value>>> = Rc::default();
        let before: Rc<RefCell<[Option<BTreeSet<String>>; 2]>> = Rc::default();
        let (init_orig, triggers_orig) = (get(&calcs, "initEnv"), get(&calcs, "triggers"));
        let init_hook = {
            let (main_env, orig) = (main_env.clone(), init_orig.clone());
            rt::func("initEnv", move |args| {
                let r = rt::call(&orig, args)?;
                let mut m = main_env.borrow_mut();
                if m.is_none() {
                    *m = r.first().cloned();
                }
                Ok(r)
            })
        };
        let triggers_hook = {
            let (main_env, before, orig) =
                (main_env.clone(), before.clone(), triggers_orig.clone());
            rt::func("triggers", move |args| {
                if let (Some(env), Some(e), Some(actor)) =
                    (&*main_env.borrow(), args.first(), args.get(1))
                    && e.raw_eq(env)
                {
                    for (k, who) in ["player", "minion"].iter().enumerate() {
                        if actor.raw_eq(&get(env, who)) && before.borrow()[k].is_none() {
                            let keys = entries(&get(actor, "breakdown"))
                                .iter()
                                .map(|(k, _)| key_str(k))
                                .collect();
                            before.borrow_mut()[k] = Some(keys);
                        }
                    }
                }
                rt::call(&orig, args)
            })
        };
        let t = calcs.as_table().unwrap();
        t.set_str("initEnv", init_hook);
        t.set_str("triggers", triggers_hook);
        let result = rt::call(&get(&calcs, "buildOutput"), &[build.clone(), s("CALCS")]);
        t.set_str("initEnv", init_orig);
        t.set_str("triggers", triggers_orig);
        let env = match result {
            Ok(r) => r.into_iter().next().unwrap_or_default(),
            Err(e) => {
                failures.push(format!(
                    "{id}: translated buildOutput failed: {}",
                    describe(&e)
                ));
                continue;
            }
        };
        let before = std::mem::take(&mut *before.borrow_mut());
        let want_player = filtered(
            lua_breakdowns(&get(&get(&env, "player"), "breakdown"), &conv),
            before[0].as_ref().unwrap_or(&BTreeSet::new()),
        );
        let minion = get(&env, "minion");
        let want_minion = (!minion.is_nil()).then(|| {
            filtered(
                lua_breakdowns(&get(&minion, "breakdown"), &conv),
                before[1].as_ref().unwrap_or(&BTreeSet::new()),
            )
        });

        // The typed calculation
        let root = engine::xml::parse(&xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        data.load_tree(&root);
        let mut typed_build = match data.load_build(&root) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{id}: build: {e}"));
                continue;
            }
        };
        let got = match engine::calcs::build_outputs(&mut typed_build, &data.calc_data())
            .and_then(|o| o.calcs)
        {
            Ok(o) => o,
            Err(e) if e.starts_with("not ported") => {
                skips.entry(e).or_default().push(id.clone());
                continue;
            }
            Err(e) => {
                failures.push(format!("{id}: typed calculation failed: {e}"));
                continue;
            }
        };
        compared += 1;
        let tab = &typed_build.items;
        let mut fail = Vec::new();
        let empty = Breakdowns::default();
        compare(
            "player",
            &typed_breakdowns(got.breakdown.as_ref().unwrap_or(&empty), tab),
            &want_player,
            &mut fail,
        );
        match (&got.minion_breakdown, &want_minion) {
            (None, None) => {}
            (Some(g), Some(w)) => compare("minion", &typed_breakdowns(g, tab), w, &mut fail),
            (g, _) => fail.push(format!(
                "minion breakdowns: typed {}, translated differs",
                g.is_some()
            )),
        }
        if !fail.is_empty() {
            let n = fail.len();
            fail.truncate(if filter.is_empty() { 20 } else { 500 });
            failures.push(format!(
                "{id}: {n} differences\n    {}",
                fail.join("\n    ")
            ));
        }
    }
    let skipped: usize = skips.values().map(Vec::len).sum();
    eprintln!("{compared} builds compared, {skipped} skipped: {skips:?}");
    assert!(
        failures.is_empty(),
        "{} builds differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
