//! The Calcs tab's breakdowns: the typed calculation of the Calcs tab
//! (`engine::calcs::build_outputs`) writes the breakdowns the translated
//! `calcs.buildOutput(build, "CALCS")` leaves in `env.player.breakdown` and
//! `env.minion.breakdown`, and its outputs, value for value; a missing or
//! extra breakdown or output fails. Builds that reach a part not ported yet
//! are skipped and listed.
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::breakdown::{Breakdown, Breakdowns, Cell, Row};
use engine::items_tab::ItemsTab;
use engine::mods::convert::Converter;
use engine::mods::tag::TagKind;
use engine::skill_data::Gem;
use equivalence::outputs::{compare, entries, key_str, list, lua_outputs, number, typed_outputs};
use equivalence::typed_build::TypedData;
use equivalence::{corpus_builds, gem_variants, skill_variants, typed_json, upstream_src};
use lua::rt;
use lua::{Key, LuaError, Value};
use serde_json::{Map, Value as J, json};
use std::collections::BTreeMap;
use std::rc::Rc;
use upstream::host::Host;
use upstream::imported::Imported;

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
        ("breakdownSource", &b.breakdown_source),
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

#[test]
fn breakdowns_match_upstream() {
    run(&|_| corpus_builds());
}

/// The skills whose functions write breakdowns, in each of their parts
#[test]
fn skill_function_breakdowns_match_upstream() {
    const WRITERS: [&str; 8] = [
        "BallLightning",
        "ExplosiveTrap",
        "FlamethrowerTrap",
        "ForbiddenRite",
        "KineticFusillade",
        "LightningSpireTrap",
        "MoltenStrike",
        "SeismicTrap",
    ];
    run(&|data| {
        skill_variants(&data.skills, |ge| {
            !ge.funcs.is_empty() && WRITERS.iter().any(|w| ge.id.starts_with(w))
        })
    });
}

/// The Calcs tab's inputs the corpus leaves at their defaults: its other
/// buff modes and main socket groups, and the gems' own selections of
/// skill part and active skill
#[test]
fn calcs_inputs_match_upstream() {
    run(&|data| {
        let mut out = Vec::new();
        for (id, xml) in corpus_builds()
            .into_iter()
            .filter(|(id, _)| id.starts_with("gen-"))
            .take(8)
        {
            let effective = "<Input name=\"misc_buffMode\" string=\"EFFECTIVE\"/>";
            for mode in ["COMBAT", "BUFFED", "UNBUFFED"] {
                let v = xml.replace(
                    effective,
                    &format!("<Input name=\"misc_buffMode\" string=\"{mode}\"/>"),
                );
                out.push((format!("{id}-{mode}"), v));
            }
            if xml.matches("<Skill ").count() >= 2 {
                let v = xml.replace(
                    "<Input name=\"skill_number\" number=\"1\"/>",
                    "<Input name=\"skill_number\" number=\"2\"/>",
                );
                out.push((format!("{id}-skill2"), v));
            }
        }
        let parts = |g: &Gem| g.granted_effect.parts.as_ref().map_or(0, Vec::len);
        let main = "mainActiveSkill=\"1\" mainActiveSkillCalcs=\"1\"".to_string();
        let part = gem_variants(
            &data.skills,
            |g| parts(g) >= 2,
            |g| {
                vec![(
                    format!("part-{}", g.id),
                    main.clone(),
                    format!("skillPart=\"1\" skillPartCalcs=\"{}\"", parts(g)),
                )]
            },
        );
        out.extend(part.into_iter().take(8));
        let active = gem_variants(
            &data.skills,
            |g| g.secondary_granted_effect.is_some(),
            |g| {
                vec![(
                    format!("active2-{}", g.id),
                    "mainActiveSkill=\"1\" mainActiveSkillCalcs=\"2\"".to_string(),
                    String::new(),
                )]
            },
        );
        out.extend(active.into_iter().take(6));
        out
    });
}

/// Mirages of skills with minions as the main skill
/// (`equivalence::variants::mirage_variants`)
#[test]
fn mirage_breakdowns_match_upstream() {
    run(&equivalence::variants::mirage_variants);
}

fn run(make: &dyn Fn(&mut TypedData) -> Vec<(String, String)>) {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let mut data = TypedData::load();
    let conv = Converter::default();
    let mut failures = Vec::new();
    let mut compared = 0;
    let mut skips: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, xml) in make(&mut data) {
        if !id.contains(&filter) {
            continue;
        }
        host.load_build_xml(xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        // The calculation on a cache of its own, as a frame's starts with
        // (`wipeGlobalCache` in `buildMode:OnFrame`); the typed
        // calculation makes a cache of its own
        rt::call(&host.global("wipeGlobalCache"), &[])
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let calcs = get(&get(&build, "calcsTab"), "calcs");
        let result = rt::call(&get(&calcs, "buildOutput"), &[build.clone(), s("CALCS")]);
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
        let want_player = lua_breakdowns(&get(&get(&env, "player"), "breakdown"), &conv);
        let want_out = lua_outputs(&get(&get(&env, "player"), "output"));
        let minion = get(&env, "minion");
        let want_minion =
            (!minion.is_nil()).then(|| lua_breakdowns(&get(&minion, "breakdown"), &conv));
        let want_minion_out = (!minion.is_nil()).then(|| lua_outputs(&get(&minion, "output")));

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
        compare(
            "player output",
            &typed_outputs(&got.player),
            &want_out,
            &mut fail,
        );
        match (&got.minion, &want_minion_out) {
            (None, None) => {}
            (Some(g), Some(w)) => compare("minion output", &typed_outputs(g), w, &mut fail),
            (g, _) => fail.push(format!("minion: typed {}, translated differs", g.is_some())),
        }
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
