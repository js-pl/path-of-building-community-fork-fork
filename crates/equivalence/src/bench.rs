//! Timings of the typed engine's calculations on corpus builds, with the
//! translation's for the same operation: the median of `reps` runs of
//! each, per build and summed over the builds.

use crate::typed_build::TypedData;
use crate::{corpus_builds, upstream_src};
use engine::calc_cache::Recalculator;
use engine::calc_env::{CalcMode, init_env};
use engine::calc_override::Override;
use engine::calc_perform::{PerformInput, perform};
use engine::calculator::MiscCalculator;
use engine::power::{PowerOptions, node_power};
use engine::tree::NodeType;
use lua::{LuaStr, TableRef, Value, rt};
use std::rc::Rc;
use std::time::Instant;
use upstream::host::Host;
use upstream::imported::Imported;

pub struct Options {
    /// Builds whose id contains this; empty for every tenth build
    pub filter: String,
    pub all: bool,
    pub reps: usize,
    pub power_reps: usize,
    /// Nodes added and removed in the calculator's overrides
    pub overrides: usize,
    pub translation: bool,
    pub power: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            filter: String::new(),
            all: false,
            reps: 5,
            power_reps: 1,
            overrides: 3,
            translation: true,
            power: true,
        }
    }
}

/// The operations timed, in the table's order
pub const OPS: [&str; 8] = [
    "load",
    "init_env",
    "perform",
    "build_outputs",
    "misc calculator",
    "recalc",
    "override",
    "node power",
];

/// A build's medians in ms by operation, typed and translated
pub struct Row {
    pub id: String,
    pub typed: [Option<f64>; OPS.len()],
    pub lua: [Option<f64>; OPS.len()],
    pub error: Option<String>,
    /// The overrides' nodes, by id, and whether each is added
    nodes: Vec<(u32, bool)>,
}

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

/// The median time of `reps` runs of `f` in ms, `f` given each run's
/// setup from `setup`, which is not timed
fn time<S, T, E>(
    reps: usize,
    mut setup: impl FnMut() -> S,
    mut f: impl FnMut(S) -> Result<T, E>,
) -> Result<f64, E> {
    let mut v = Vec::new();
    for _ in 0..reps {
        let s = setup();
        let t = Instant::now();
        let r = f(s);
        v.push(t.elapsed().as_secs_f64() * 1000.0);
        r?;
    }
    Ok(median(v).unwrap_or(0.0))
}

fn get(v: &Value, k: &str) -> Value {
    rt::index(v, &Value::str(k)).unwrap_or_default()
}

fn lua_err(e: lua::LuaError) -> String {
    lua::to_string(e.value()).to_str_lossy().into_owned()
}

/// The builds of the run
fn builds(opts: &Options) -> Vec<(String, String)> {
    corpus_builds()
        .into_iter()
        .enumerate()
        .filter(|(i, (id, _))| {
            if !opts.filter.is_empty() {
                id.contains(&opts.filter)
            } else {
                opts.all || i % 10 == 3
            }
        })
        .map(|(_, b)| b)
        .collect()
}

/// Runs the benchmark, printing each build's row as it finishes
pub fn run(opts: &Options) -> Vec<Row> {
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", lua_err(e)));
    let mut data = TypedData::load();
    let mut rows = Vec::new();
    for (id, xml) in builds(opts) {
        let root = engine::xml::parse(&xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        data.load_tree(&root);
        let mut row = Row {
            id: id.clone(),
            typed: [None; OPS.len()],
            lua: [None; OPS.len()],
            error: None,
            nodes: vec![],
        };
        if let Err(e) = typed(&data, &xml, opts, &mut row) {
            row.error = Some(e);
        }
        if opts.translation
            && let Err(e) = translated(&host, &id, &xml, opts, &mut row)
        {
            row.error.get_or_insert(format!("translation: {e}"));
        }
        eprintln!("{}", format_row(&row));
        rows.push(row);
    }
    rows
}

/// The overrides timed: nodes next to the tree added, allocated nodes
/// removed, by node id
fn overrides(b: &engine::build::Build, n: usize) -> Vec<(Override, u32, bool)> {
    let spec = &b.spec;
    let kind_ok = |i: usize| {
        let t = spec.tree_node(i);
        matches!(
            t.kind,
            NodeType::Normal | NodeType::Notable | NodeType::Keystone
        ) && t.ascendancy_name.is_none()
            && !spec.mod_list(i).is_empty()
    };
    let mut by_id: Vec<usize> = (0..spec.nodes.len()).collect();
    by_id.sort_by_key(|&i| spec.nodes[i].id);
    let adds = by_id
        .iter()
        .copied()
        .filter(|&i| !spec.nodes[i].alloc && spec.nodes[i].path_dist == 1.0 && kind_ok(i))
        .take(n)
        .map(|i| (Override::default().add_node(i), spec.nodes[i].id, true));
    let removes = by_id
        .iter()
        .copied()
        .filter(|&i| spec.nodes[i].alloc && kind_ok(i))
        .take(n)
        .map(|i| (Override::default().remove_node(i), spec.nodes[i].id, false));
    adds.chain(removes).collect()
}

fn typed(data: &TypedData, xml: &str, opts: &Options, row: &mut Row) -> Result<(), String> {
    let calc_data = data.calc_data();
    let reps = opts.reps;
    let load = || -> Result<engine::build::Build<'_>, String> {
        let root = engine::xml::parse(xml)?;
        data.load_build(&root)
    };
    row.typed[0] = Some(time(reps, || (), |_| load())?);
    let build = load()?;
    row.typed[1] = Some(time(
        reps,
        || build.clone(),
        |mut b| init_env(&mut b, &calc_data, CalcMode::Main).map(drop),
    )?);
    row.typed[2] = Some(time(
        reps,
        || {
            let mut b = build.clone();
            let env = init_env(&mut b, &calc_data, CalcMode::Main);
            (b, env)
        },
        |(b, env)| {
            let recalc = Recalculator::new(&b, &calc_data, CalcMode::Main);
            let mut input = PerformInput::new(&b, &calc_data, true)?;
            input.recalc = Some(&recalc);
            let mut p = perform(env?, input)?;
            p.finish()
        },
    )?);
    row.typed[3] = Some(time(
        reps,
        || build.clone(),
        |mut b| engine::calcs::build_outputs(&mut b, &calc_data).map(drop),
    )?);
    let mut after = build.clone();
    engine::calcs::build_outputs(&mut after, &calc_data)?;
    row.typed[4] = Some(time(
        reps,
        || (),
        |_| MiscCalculator::new(&after, &calc_data).map(drop),
    )?);
    row.typed[5] = Some(time(
        reps,
        || build.clone(),
        |mut b| {
            engine::calcs::build_outputs(&mut b, &calc_data)?;
            MiscCalculator::new(&b, &calc_data).map(drop)
        },
    )?);
    let calc = MiscCalculator::new(&after, &calc_data)?;
    let ovrs = overrides(&after, opts.overrides);
    row.nodes = ovrs.iter().map(|&(_, id, add)| (id, add)).collect();
    if !ovrs.is_empty() {
        let total = time(
            reps,
            || (),
            |_| {
                ovrs.iter()
                    .try_for_each(|(o, _, _)| calc.calculate(o, true).map(drop))
            },
        )?;
        row.typed[6] = Some(total / ovrs.len() as f64);
    }
    if opts.power {
        let p = PowerOptions {
            stat: None,
            max_depth: Some(1.0),
            cluster_nodes: vec![],
        };
        row.typed[7] = Some(time(
            opts.power_reps,
            || (),
            |_| node_power(&calc, &p).map(drop),
        )?);
    }
    Ok(())
}

fn translated(
    host: &Host,
    id: &str,
    xml: &str,
    opts: &Options,
    row: &mut Row,
) -> Result<(), String> {
    let reps = opts.reps;
    host.load_build_xml(xml.as_bytes(), id).map_err(lua_err)?;
    let build = host.build().map_err(lua_err)?;
    let tab = get(&build, "calcsTab");
    let calcs = get(&tab, "calcs");
    let call = |f: &str, args: &[Value]| rt::call(&get(&calcs, f), args).map_err(lua_err);
    let main = || [build.clone(), Value::str("MAIN")];
    row.lua[1] = Some(time(reps, || (), |_| call("initEnv", &main()))?);
    row.lua[2] = Some(time(
        reps,
        || call("initEnv", &main()).map(|r| r.into_iter().next().unwrap_or_default()),
        |env| call("perform", &[env?]),
    )?);
    row.lua[3] = Some(time(
        reps,
        || (),
        |_| {
            call("buildOutput", &main())?;
            call("buildOutput", &[build.clone(), Value::str("CALCS")])
        },
    )?);
    row.lua[4] = Some(time(
        reps,
        || (),
        |_| call("getMiscCalculator", std::slice::from_ref(&build)),
    )?);
    let method = |name: &str| rt::method(&tab, &LuaStr::from(name), &[]).map_err(lua_err);
    row.lua[5] = Some(time(reps, || (), |_| method("BuildOutput"))?);
    let misc = get(&tab, "miscCalculator");
    let calc_func = rt::index(&misc, &Value::Number(1.0)).map_err(lua_err)?;
    let lnodes = get(&get(&build, "spec"), "nodes");
    let lovrs: Vec<Value> = row
        .nodes
        .iter()
        .map(|&(nid, add)| {
            let set = TableRef::new();
            let n = rt::index(&lnodes, &Value::Number(nid as f64)).unwrap_or_default();
            set.set(n, Value::Bool(true));
            let t = TableRef::new();
            t.set_str(
                if add { "addNodes" } else { "removeNodes" },
                Value::Table(set),
            );
            Value::Table(t)
        })
        .collect();
    if !lovrs.is_empty() {
        let total = time(
            reps,
            || (),
            |_| {
                lovrs
                    .iter()
                    .try_for_each(|o| rt::call(&calc_func, std::slice::from_ref(o)).map(drop))
                    .map_err(lua_err)
            },
        )?;
        row.lua[6] = Some(total / lovrs.len() as f64);
    }
    if opts.power {
        let t = tab.as_table().ok_or("calcsTab")?;
        t.set_str("powerStat", Value::Nil);
        t.set_str("nodePowerMaxDepth", Value::Number(1.0));
        let ltree = get(&get(&build, "spec"), "tree");
        let lt = ltree.as_table().ok_or("tree")?;
        let full_map = get(&ltree, "clusterNodeMap");
        lt.set_str("clusterNodeMap", Value::Table(TableRef::new()));
        let r = time(opts.power_reps, || (), |_| method("PowerBuilder"));
        lt.set_str("clusterNodeMap", full_map);
        row.lua[7] = Some(r?);
    }
    Ok(())
}

fn ms(v: Option<f64>) -> String {
    match v {
        Some(v) if v >= 100.0 => format!("{v:.0}"),
        Some(v) => format!("{v:.1}"),
        None => "-".into(),
    }
}

fn ratio(t: Option<f64>, l: Option<f64>) -> String {
    match (t, l) {
        (Some(t), Some(l)) if t > 0.0 => format!("{:.1}x", l / t),
        _ => "-".into(),
    }
}

fn format_row(r: &Row) -> String {
    let cells: Vec<String> = (0..OPS.len())
        .map(|k| format!("{} / {}", ms(r.typed[k]), ms(r.lua[k])))
        .collect();
    let err = r
        .error
        .as_deref()
        .map_or(String::new(), |e| format!(" ({e})"));
    format!("{}: {}{err}", r.id, cells.join(", "))
}

/// The table: per build, typed / translated ms per operation, then the
/// sums over the builds every operation of which has both times, and
/// their ratio
pub fn table(rows: &[Row]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "| build | {} |\n|---|{}\n",
        OPS.join(" | "),
        "---|".repeat(OPS.len())
    ));
    for r in rows {
        let cells: Vec<String> = (0..OPS.len())
            .map(|k| format!("{} / {}", ms(r.typed[k]), ms(r.lua[k])))
            .collect();
        out.push_str(&format!("| {} | {} |\n", r.id, cells.join(" | ")));
    }
    out.push_str(
        "\n| operation | builds | typed ms | translation ms | speedup |\n|---|---|---|---|---|\n",
    );
    for (k, op) in OPS.iter().enumerate() {
        let both: Vec<&Row> = rows.iter().filter(|r| r.typed[k].is_some()).collect();
        let typed: f64 = both.iter().filter_map(|r| r.typed[k]).sum();
        let with_lua: Vec<&&Row> = both.iter().filter(|r| r.lua[k].is_some()).collect();
        let (t2, l2): (f64, f64) = with_lua
            .iter()
            .map(|r| (r.typed[k].unwrap(), r.lua[k].unwrap()))
            .fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
        let lua = (!with_lua.is_empty()).then_some(l2);
        out.push_str(&format!(
            "| {op} | {} | {} | {} | {} |\n",
            both.len(),
            ms(Some(typed)),
            ms(lua),
            ratio((!with_lua.is_empty()).then_some(t2), lua),
        ));
    }
    out
}
