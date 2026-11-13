//! The engine API on the typed engine: a build loaded with
//! `engine::build`, calculated with `engine::calcs`, and the commands
//! ported so far answered from it with the JSON the translated session
//! (`lua/session.lua`) gives. [`crate::Engine`] picks per command.

mod calcs;
pub mod display_stats;
mod geometry;
mod items;
mod save;
mod sets;
mod sidebar;
mod skills;
mod tree;

pub use skills::{GemEdit, GroupEdit};

use engine::build::{Build, LoadContext};
use engine::calc_env::{CalcData, CalcMode};
use engine::calcs::BuildOutput;
use engine::game_data::GameData;
use engine::mod_parser::ModParser;
use engine::tree::{NodeType, PassiveTree};
use serde_json::{Map, Value as J, json};
use std::cell::RefCell;
use std::collections::HashMap;

pub use sidebar::Controls;

/// A command the typed engine cannot answer yet; the caller falls back
/// to the translation
pub const NOT_PORTED: &str = "not ported";

pub type Result<T> = std::result::Result<T, String>;

/// Loads a tree version's passive tree
pub type TreeSource = dyn Fn(&str, &ModParser) -> Result<PassiveTree>;

/// The typed engine's game data, mod parser and passive trees
pub struct TypedData {
    pub game: GameData,
    pub parser: ModParser,
    /// The jewel sockets of the latest tree
    socket_ids: Vec<u32>,
    trees: RefCell<HashMap<String, &'static PassiveTree>>,
    source: Box<TreeSource>,
    /// Makes the timeless jewels' data, once a build needs it
    timeless_source: Option<Box<TimelessSource>>,
    timeless: std::cell::OnceCell<Option<std::sync::Arc<engine::timeless::Timeless>>>,
}

pub type TimelessSource = dyn Fn(&ModParser) -> Result<engine::timeless::Timeless>;

impl TypedData {
    pub fn new(game: GameData, parser: ModParser, source: Box<TreeSource>) -> Result<TypedData> {
        let data = TypedData {
            game,
            parser,
            socket_ids: Vec::new(),
            trees: RefCell::default(),
            source,
            timeless_source: None,
            timeless: Default::default(),
        };
        let latest = data.tree(data.game.latest_tree_version())?;
        let socket_ids = latest
            .nodes
            .iter()
            .filter(|n| n.kind == NodeType::Socket)
            .map(|n| n.id)
            .collect();
        Ok(TypedData { socket_ids, ..data })
    }

    /// Makes the data from upstream's source directory, as `gamedata`
    /// does at build time
    pub fn from_source(src_dir: &std::path::Path) -> Result<TypedData> {
        let src = gamedata::Source::load(src_dir)?;
        let game = src.game_data()?;
        let parser = src.mod_parser()?;
        let latest = game.latest_tree_version().to_string();
        let src = std::rc::Rc::new(src);
        let trees = src.clone();
        let source = move |v: &str, p: &ModParser| trees.tree(v, p, v == latest);
        let mut data = TypedData::new(game, parser, Box::new(source))?;
        data.timeless_source = Some(Box::new(move |p: &ModParser| src.timeless(p)));
        Ok(data)
    }

    /// A tree version's passive tree, loaded once. Trees live as long as
    /// the process, as builds borrow them.
    pub fn tree(&self, version: &str) -> Result<&'static PassiveTree> {
        if let Some(t) = self.trees.borrow().get(version) {
            return Ok(t);
        }
        let tree: &'static PassiveTree = Box::leak(Box::new((self.source)(version, &self.parser)?));
        self.trees.borrow_mut().insert(version.to_string(), tree);
        Ok(tree)
    }

    /// The timeless jewels' data, if the session has a source of it
    fn timeless(&self) -> Option<std::sync::Arc<engine::timeless::Timeless>> {
        self.timeless
            .get_or_init(|| {
                let f = self.timeless_source.as_ref()?;
                f(&self.parser).ok().map(std::sync::Arc::new)
            })
            .clone()
    }

    pub fn calc_data(&self) -> CalcData<'_> {
        CalcData {
            setup: &self.game.setup,
            items: &self.game.items,
            skills: &self.game.skills,
            parser: &self.parser,
            jewel_radius: &self.game.jewel_radius,
        }
    }

    pub fn item_context(&self) -> engine::item::ItemContext<'_> {
        engine::item::ItemContext {
            data: &self.game.items,
            parser: &self.parser,
            jewel_radius: &self.game.jewel_radius,
            default_affix_quality: 0.5,
        }
    }

    fn load_build<'d>(&'d self, root: &engine::xml::Element) -> Result<Build<'d>> {
        let tree = self.tree(&engine::build::tree_version(root))?;
        let one_hand = |t: &str| self.game.setup.weapon_type_info.get(t).map(|w| w.one_hand);
        let minions = |m: &str| self.game.setup.minions.contains_key(m);
        let ctx = LoadContext {
            items: engine::item::ItemContext {
                data: &self.game.items,
                parser: &self.parser,
                jewel_radius: &self.game.jewel_radius,
                default_affix_quality: 0.5,
            },
            skill_data: &self.game.skills,
            config_data: &self.game.config,
            option_defaults: &self.game.option_defaults,
            parser: &self.parser,
            socket_ids: &self.socket_ids,
            one_hand: &one_hand,
            minions: &minions,
            timeless: if engine::build::has_timeless_jewel(root) {
                self.timeless()
            } else {
                None
            },
        };
        Build::load(root, &ctx, tree, &self.parser)
    }
}

/// What the translated session's UI state holds that the build does not:
/// open popups, the unsaved flag and the output revision
#[derive(Clone, Debug)]
pub struct Ui {
    pub popups: J,
    pub unsaved: bool,
    pub output_revision: f64,
}

impl Default for Ui {
    fn default() -> Ui {
        Ui {
            popups: json!([]),
            unsaved: true,
            output_revision: 1.0,
        }
    }
}

/// A build on the typed engine
pub struct TypedSession<'d> {
    data: &'d TypedData,
    build: Build<'d>,
    name: String,
    main: BuildOutput,
    /// The Calcs tab's calculation, once asked for
    calcs: Option<BuildOutput>,
    /// The Calcs tab's skill selection, and whether each of its control
    /// rows was shown when last drawn
    calcs_select: sidebar::SkillSelect,
    calcs_rows: Vec<(&'static str, bool)>,
    /// The tree tab's active spec and the specs' titles
    specs: (usize, Vec<String>),
    pub ui: Ui,
    controls: Controls,
    /// The loaded file, for what the typed build does not hold
    root: engine::xml::Element,
    /// `buildMode.viewMode`
    view_mode: String,
    /// Whether the Calcs tab's `showMinion` input was set
    show_minion_set: bool,
    /// The tree tab's specs; the active one is the build's (`None` here)
    other_specs: Vec<Option<std::sync::Arc<engine::spec::PassiveSpec<'d>>>>,
    /// `ItemsTab.items` as upstream's table: its ids in `pairs` order
    item_keys: lua::ljtab::LjMap<()>,
    /// The jewel sockets' labels `UpdateSockets` last gave
    socket_labels: HashMap<u32, String>,
}

impl<'d> TypedSession<'d> {
    /// `loadXml`: the build, calculated
    pub fn load(data: &'d TypedData, xml: &str, name: Option<&str>) -> Result<TypedSession<'d>> {
        let root = engine::xml::parse(xml)?;
        let mut build = data.load_build(&root)?;
        let main =
            engine::calcs::build_output(&mut build, &data.calc_data(), CalcMode::Main, None)?;
        let mut s = TypedSession {
            data,
            build,
            name: name.unwrap_or("Imported build").to_string(),
            main,
            calcs: None,
            calcs_select: Default::default(),
            calcs_rows: Vec::new(),
            specs: tree::spec_titles(&root),
            ui: Ui::default(),
            controls: Controls::default(),
            view_mode: root
                .child("PathOfBuilding")
                .and_then(|p| p.child("Build"))
                .and_then(|b| b.attr("viewMode"))
                .unwrap_or("TREE")
                .to_string(),
            show_minion_set: false,
            other_specs: Vec::new(),
            item_keys: Default::default(),
            socket_labels: HashMap::new(),
            root,
        };
        s.load_specs()?;
        for id in &s.build.items.item_order_list {
            s.item_keys.insert(*id as u32, ());
        }
        s.update_display_labels(true);
        s.estimate_progress()?;
        s.refresh_controls()?;
        Ok(s)
    }

    /// `controls`: the sidebar's and the Calcs tab's controls, `None`
    /// when another is asked for
    pub fn controls(&self, paths: &[&str]) -> Option<J> {
        let state = self.state().ok()?;
        let mut out = Map::new();
        for p in paths {
            let c = match p.split_once('.')? {
                ("calcs", name) => self.calcs_control(name)?,
                ("build", _) => state["controls"].get(*p)?.clone(),
                _ => return None,
            };
            out.insert(p.to_string(), c);
        }
        Some(object(out))
    }

    /// `outputs`: the sidebar calculation's player and minion outputs
    pub fn outputs(&self) -> J {
        let mut o = Map::new();
        let mut player = output(&self.main.player);
        if let Some(m) = &self.main.minion {
            let m = output(m);
            // `output.Minion` is the minion's output
            if let J::Object(p) = &mut player {
                p.insert("Minion".into(), m.clone());
            }
            o.insert("minion".into(), m);
        }
        o.insert("player".into(), player);
        J::Object(o)
    }
}

/// A number as `dkjson` writes it: `tostring`, `%.14g`; NaN and the
/// infinities as `null`
pub fn num(n: f64) -> J {
    if !n.is_finite() {
        return J::Null;
    }
    serde_json::from_str(&lua::value::format_number(n)).unwrap_or(J::Null)
}

/// An empty table: `dkjson` writes it as an array
fn object(m: Map<String, J>) -> J {
    if m.is_empty() {
        json!([])
    } else {
        J::Object(m)
    }
}

/// The length `dkjson` writes a table with these keys as an array with,
/// if it does: keys from 1 on, without too many holes
fn array_len<'a>(keys: impl Iterator<Item = &'a String>) -> Option<usize> {
    let (mut max, mut n) = (0, 0);
    for k in keys {
        let i: usize = k
            .parse()
            .ok()
            .filter(|&i| i >= 1 && k.as_bytes()[0] != b'0')?;
        max = max.max(i);
        n += 1;
    }
    (n > 0 && !(max > 10 && max > n * 2)).then_some(max)
}

/// An actor's output as the translated session writes it: gems and items
/// by name
fn output(out: &engine::calcs::Output) -> J {
    object(
        out.iter()
            .map(|(k, v)| (k.as_str().to_string(), out_val(v)))
            .collect(),
    )
}

fn out_val(v: &engine::actor::OutVal) -> J {
    use engine::actor::OutVal;
    match v {
        OutVal::Num(n) => num(*n),
        OutVal::Bool(b) => J::Bool(*b),
        OutVal::Str(s) => J::from(&**s),
        OutVal::Table(t) => match array_len(t.keys()) {
            Some(n) => {
                let mut a = vec![J::Null; n];
                for (k, v) in t.iter() {
                    a[k.parse::<usize>().unwrap() - 1] = out_val(v);
                }
                J::Array(a)
            }
            None => object(t.iter().map(|(k, v)| (k.clone(), out_val(v))).collect()),
        },
        OutVal::List(l) => J::Array(l.iter().map(out_val).collect()),
        OutVal::Gem(n) => json!({ "$gem": &**n }),
        OutVal::Item(n) => json!({ "$item": &**n }),
    }
}

impl<'d> TypedSession<'d> {
    /// Every spec of the tree tab (`TreeTab:Load`, then `PostLoad`)
    fn load_specs(&mut self) -> Result<()> {
        let saved: Vec<engine::xml::Element> = self
            .root
            .child("PathOfBuilding")
            .and_then(|p| p.child("Tree"))
            .map(|t| t.elements().filter(|e| e.elem == "Spec").cloned().collect())
            .unwrap_or_default();
        let active = self.specs.0;
        for (i, e) in saved.iter().enumerate() {
            if i + 1 == active {
                self.other_specs.push(None);
                continue;
            }
            let tree = self.data.tree(e.attr("treeVersion").unwrap_or("2_6"))?;
            let mut spec = engine::spec::PassiveSpec::new(tree, &self.data.parser)?;
            spec.load(e).map_err(|e| format!("{NOT_PORTED}: {e}"))?;
            spec.build_cluster_jewel_graphs(&self.build.items, &self.data.game.items)?;
            self.other_specs.push(Some(std::sync::Arc::new(spec)));
        }
        Ok(())
    }

    /// `nodePower`: `CalcsTab:PowerBuilder` with the tree tab's power stat
    /// of this index (1-based), as the heat map shows it
    pub fn node_power(&self, index: f64) -> Result<J> {
        use engine::power::{PowerOptions, node_power, power_stats};
        let stat = power_stats()
            .into_iter()
            .filter(|s| !s.ignore_for_nodes)
            .nth((index as usize).wrapping_sub(1));
        let combined = stat.as_ref().is_none_or(|s| s.stat.is_none());
        let data = self.data.calc_data();
        let calc = engine::calculator::MiscCalculator::new(&self.build, &data)?;
        let opts = PowerOptions {
            stat,
            max_depth: None,
            cluster_nodes: Vec::new(),
        };
        let p = node_power(&calc, &opts)?;
        let opt = |v: Option<f64>| v.map_or(J::Null, num);
        let mut nodes = Map::new();
        for (id, n) in &p.nodes {
            let w = &n.power;
            if w.single_stat.is_none() && w.offence.is_none() && w.defence.is_none() {
                continue;
            }
            let mut o = Map::new();
            for (k, v) in [
                ("single", w.single_stat),
                ("offence", w.offence),
                ("defence", w.defence),
                ("path", w.path_power),
            ] {
                if let Some(v) = v {
                    o.insert(k.into(), num(v));
                }
            }
            nodes.insert(id.to_string(), J::Object(o));
        }
        let m = &p.max;
        let max = [
            ("single", m.single_stat),
            ("offence", m.offence),
            ("defence", m.defence),
            ("offencePerPoint", m.offence_per_point),
            ("defencePerPoint", m.defence_per_point),
        ];
        let max: Map<String, J> = max
            .iter()
            .map(|(k, v)| (k.to_string(), opt(Some(*v))))
            .collect();
        Ok(json!({ "combined": combined, "max": object(max), "nodes": object(nodes) }))
    }
}
