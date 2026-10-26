//! The passive tree: upstream's `PassiveTree` class (`Classes/PassiveTree.lua`)
//! without its art, as typed data. Built from a tree version's `tree.lua`, as
//! the importer reads it; node stats are parsed into typed mods.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.
//!
//! Upstream visits the tree's tables with `pairs`, and some results depend
//! on the order (which of two notables with one name is kept, the order of
//! linked nodes); the tables are visited in the order the data lists them,
//! as the translation does.

use crate::mod_parser::ModParser;
use crate::mods::convert::{Converter, get, number, text};
use crate::mods::{Mod, ModType, ModValue, Sym};
use lua::{Key, Value};
use std::collections::HashMap;
use std::rc::Rc;

pub type Result<T> = std::result::Result<T, String>;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NodeType {
    ClassStart,
    AscendClassStart,
    Mastery,
    Socket,
    Keystone,
    Notable,
    Normal,
}

impl NodeType {
    pub fn name(self) -> &'static str {
        match self {
            NodeType::ClassStart => "ClassStart",
            NodeType::AscendClassStart => "AscendClassStart",
            NodeType::Mastery => "Mastery",
            NodeType::Socket => "Socket",
            NodeType::Keystone => "Keystone",
            NodeType::Notable => "Notable",
            NodeType::Normal => "Normal",
        }
    }
}

/// The parse of one stat line of a node: `node.mods[i]`. Lines combined
/// into an earlier one have an empty list.
#[derive(Clone, Debug, PartialEq)]
pub struct StatLine {
    /// `nil` when the parser did not know the line
    pub list: Option<Vec<Mod>>,
    pub extra: Option<Rc<str>>,
}

/// A stat group of the tree data: nodes on orbits around a centre.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub x: f64,
    pub y: f64,
    pub is_proxy: bool,
    pub ascendancy_name: Option<Sym>,
    pub is_ascendancy_start: bool,
}

/// A mastery's effect choice, by effect id.
#[derive(Clone, Debug, PartialEq)]
pub struct MasteryEffect {
    pub id: f64,
    pub sd: Vec<Rc<str>>,
    pub mods: Vec<StatLine>,
    pub mod_list: Vec<Mod>,
}

/// A cluster jewel socket's `expansionJewel`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpansionJewel {
    pub size: f64,
    pub index: f64,
    pub proxy: Option<Rc<str>>,
    /// The socket this one is nested in
    pub parent: Option<Rc<str>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: u32,
    pub dn: Rc<str>,
    /// Stat lines, one per line (upstream splits multi-line stats)
    pub sd: Vec<Rc<str>>,
    pub kind: NodeType,
    pub ascendancy_name: Option<Sym>,
    pub class_start_index: Option<i64>,
    pub group: Option<i64>,
    pub orbit: Option<i64>,
    pub orbit_index: Option<i64>,
    pub angle: Option<f64>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    /// Connected nodes: `out` then `in`, in the data's order
    pub linked_id: Vec<u32>,
    pub passive_points_granted: f64,
    pub is_proxy: bool,
    pub is_blighted: bool,
    pub is_mastery: bool,
    pub is_multiple_choice: bool,
    pub is_multiple_choice_option: bool,
    pub is_ascendancy_start: bool,
    pub charm_socket: bool,
    /// Cluster jewel socket data (`expansionJewel`)
    pub expansion_jewel: Option<ExpansionJewel>,
    /// Effect ids of a mastery's choices
    pub mastery_effects: Vec<f64>,
    /// The parse of each stat line
    pub mods: Vec<StatLine>,
    /// The mods of every line the parser fully understood, sourced `Tree:<id>`
    pub mod_list: Vec<Mod>,
    pub keystone_mod: Option<Mod>,
    /// A stat line the parser did not know
    pub unknown: bool,
    /// A stat line the parser understood only partly
    pub extra: bool,
    /// For jewel sockets and keystones: the nodes within each jewel radius,
    /// in the order the data lists them
    pub nodes_in_radius: Option<Vec<Vec<u32>>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Ascendancy {
    pub id: Option<Rc<str>>,
    pub name: Rc<str>,
    pub start_node_id: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Class {
    pub name: Rc<str>,
    /// Ascendancies by index; index 0 is "None"
    pub ascendancies: Vec<(i64, Ascendancy)>,
    pub start_node_id: Option<u32>,
    /// `base_str`, `base_dex` and `base_int`, from the tree's
    /// `characterData` when it has the class, else the class's
    pub base_stats: [Option<f64>; 3],
}

#[derive(Clone, Debug)]
pub struct PassiveTree {
    pub version: Rc<str>,
    /// `treeVersions[version].num`
    pub version_num: f64,
    /// By class index (0 = Scion)
    pub classes: Vec<(i64, Class)>,
    pub groups: HashMap<i64, Group>,
    group_ids: Vec<i64>,
    pub nodes: Vec<Node>,
    pub node_index: HashMap<u32, usize>,
    /// `keystoneMap`: by name and lower case name
    pub keystone_map: HashMap<Rc<str>, u32>,
    /// The mods of the timeless jewels' keystones (`self.legion.nodes`), by
    /// name, which `keystoneMap` holds where the tree has no keystone of
    /// that name
    pub legion_keystones: HashMap<Rc<str>, Vec<Mod>>,
    /// `notableMap`: by lower case name
    pub notable_map: HashMap<Rc<str>, u32>,
    /// `ascendancyMap`: by lower case name
    pub ascendancy_map: HashMap<Rc<str>, u32>,
    /// `clusterNodeMap`: notables and keystones outside the tree, by name
    pub cluster_node_map: HashMap<Rc<str>, u32>,
    /// `classNotables`: ascendancy notable names per class name
    pub class_notables: HashMap<Rc<str>, Vec<Rc<str>>>,
    /// Jewel sockets, in the data's order
    pub sockets: Vec<u32>,
    pub mastery_effects: HashMap<u64, MasteryEffect>,
    /// `alternate_ascendancies` by index, without the legacy ones; `None`
    /// when the tree has none
    pub alternate_ascendancies: Option<Vec<(i64, Ascendancy)>>,
}

/// `(inner, outer)` of upstream's jewel radii for a tree version
/// (`data.setJewelRadiiGlobally`).
pub fn jewel_radii(jewel_radii: &Value, version: &str) -> Result<Vec<(f64, f64)>> {
    let mut nums = version.split('_').filter_map(|p| p.parse::<u32>().ok());
    let (major, minor) = (nums.next().unwrap_or(0), nums.next().unwrap_or(0));
    let set = if major <= 3 && minor <= 15 {
        "3_15"
    } else {
        "3_16"
    };
    let list = get(jewel_radii, set);
    let mut out = Vec::new();
    for r in list.as_table().map(|t| t.array()).unwrap_or_default() {
        out.push((number(&get(&r, "inner"))?, number(&get(&r, "outer"))?));
    }
    Ok(out)
}

/// `treeVersions[version].num`: `3_29` is 3.29, `3_6` is 3.06.
pub fn version_num(version: &str) -> f64 {
    let mut nums = version.split('_').filter_map(|p| p.parse::<u32>().ok());
    let (major, minor) = (nums.next().unwrap_or(0), nums.next().unwrap_or(0));
    major as f64 + minor as f64 / 100.0
}

fn truthy(v: &Value, k: &str) -> bool {
    get(v, k).truthy()
}

fn opt_int(v: &Value) -> Result<Option<i64>> {
    if v.is_nil() {
        return Ok(None);
    }
    Ok(Some(number(v)? as i64))
}

fn texts(v: &Value) -> Result<Vec<Rc<str>>> {
    v.as_table()
        .map(|t| t.array())
        .unwrap_or_default()
        .iter()
        .map(text)
        .collect()
}

/// The entries of a table in `pairs` order
fn pairs(v: &Value) -> Vec<(Key, Value)> {
    v.as_table().map(|t| t.entries()).unwrap_or_default()
}

fn lower(s: &str) -> Rc<str> {
    s.to_ascii_lowercase().into()
}

/// `modLib.createMod(name, type, value, source)`
fn create_mod(name: &str, typ: ModType, value: ModValue, source: &str) -> Mod {
    Mod {
        name: Sym::new(name),
        typ,
        value,
        flags: 0,
        keyword_flags: 0,
        source: Some(source.into()),
        source_slot: None,
        tags: vec![],
        unreached_tags: vec![],
    }
}

/// `modLib.setSource(mod, source)`
fn set_source(m: &mut Mod, source: &str) {
    m.source = Some(source.into());
    if let ModValue::Record(r) = &mut m.value
        && let Some(ModValue::Mod(inner)) = r.get_mut(Sym::new("mod"))
    {
        inner.source = Some(source.into());
    }
}

/// `PassiveTreeClass:CalcOrbitAngles(nodesInOrbit)`, in radians
fn orbit_angles(nodes_in_orbit: f64) -> Vec<f64> {
    let degrees: Vec<f64> = if nodes_in_orbit == 16.0 {
        vec![
            0.0, 30.0, 45.0, 60.0, 90.0, 120.0, 135.0, 150.0, 180.0, 210.0, 225.0, 240.0, 270.0,
            300.0, 315.0, 330.0,
        ]
    } else if nodes_in_orbit == 40.0 {
        vec![
            0.0, 10.0, 20.0, 30.0, 40.0, 45.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0,
            130.0, 135.0, 140.0, 150.0, 160.0, 170.0, 180.0, 190.0, 200.0, 210.0, 220.0, 225.0,
            230.0, 240.0, 250.0, 260.0, 270.0, 280.0, 290.0, 300.0, 310.0, 315.0, 320.0, 330.0,
            340.0, 350.0,
        ]
    } else {
        (0..=nodes_in_orbit as i64)
            .map(|i| 360.0 * i as f64 / nodes_in_orbit)
            .collect()
    };
    degrees.into_iter().map(f64::to_radians).collect()
}

/// A line's mods (nil when unknown) and the part not understood
type Parse = (Option<Vec<Mod>>, Option<Rc<str>>);

/// Parses stat lines as `PassiveTreeClass:ProcessStats` does: multi-line
/// stats are split, and a line the parser does not fully understand is
/// combined with the lines after it until the combination parses.
pub(crate) fn process_stats(
    parser: &ModParser,
    sd: &mut Vec<Rc<str>>,
    source: Option<&str>,
) -> Result<(Vec<StatLine>, Vec<Mod>, bool, bool)> {
    let mut mods = Vec::new();
    let (mod_list, unknown, extra) = process_stats_from(parser, sd, &mut mods, 0, source)?;
    Ok((mods, mod_list, unknown, extra))
}

/// `ProcessStats(node, startIndex)`: parses the lines from `start` on into
/// `mods` (the lines before are kept), giving the mods of those lines and
/// whether one was unknown or understood only partly.
pub(crate) fn process_stats_from(
    parser: &ModParser,
    sd: &mut Vec<Rc<str>>,
    out: &mut Vec<StatLine>,
    start: usize,
    source: Option<&str>,
) -> Result<(Vec<Mod>, bool, bool)> {
    let conv = Converter::default();
    let parse = |line: &str| -> Result<Parse> {
        let (list, extra) = parser.parse(line.as_bytes());
        let list = match list {
            Value::Table(t) => Some(
                t.array()
                    .iter()
                    .map(|m| conv.mod_(m))
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            ),
            _ => None,
        };
        let extra = match extra {
            Value::Str(s) => Some(s.to_str_lossy().into()),
            _ => None,
        };
        Ok((list, extra))
    };
    let mut mods: Vec<Option<StatLine>> = out.drain(..).map(Some).collect();
    let (mut unknown, mut extra_flag) = (false, false);
    let mut i = start;
    while i < sd.len() {
        if sd[i].contains('\n') {
            let lines: Vec<Rc<str>> = sd[i]
                .split('\n')
                .filter(|l| !l.is_empty())
                .map(Rc::from)
                .collect();
            sd.splice(i..=i, lines);
            if i >= sd.len() {
                break;
            }
        }
        let line = sd[i].clone();
        let (mut list, mut extra) = parse(&line)?;
        if list.is_none() || extra.is_some() {
            let mut end = i + 1;
            while end < sd.len() {
                let mut comb = line.to_string();
                for l in &sd[i + 1..=end] {
                    comb.push(' ');
                    comb.push_str(l);
                }
                (list, extra) = parse(&comb)?;
                if list.is_some() && extra.is_none() {
                    // The combined lines get empty lists
                    for ci in i + 1..=end {
                        if mods.len() <= ci {
                            mods.resize(ci + 1, None);
                        }
                        mods[ci] = Some(StatLine {
                            list: Some(vec![]),
                            extra: None,
                        });
                    }
                    break;
                }
                end += 1;
            }
        }
        if list.is_none() {
            unknown = true;
        } else if extra.is_some() {
            extra_flag = true;
        }
        if mods.len() <= i {
            mods.resize(i + 1, None);
        }
        mods[i] = Some(StatLine { list, extra });
        i += 1;
        while i < mods.len() && mods[i].is_some() {
            i += 1;
        }
    }
    let mut mods: Vec<StatLine> = mods
        .into_iter()
        .map(|m| m.ok_or("a stat line without a parse"))
        .collect::<std::result::Result<_, _>>()?;
    // Upstream sets the source on the parsed mods themselves, so the lines
    // carry it too
    let mut mod_list = Vec::new();
    for line in mods.iter_mut().skip(start) {
        if let (Some(list), None) = (&mut line.list, &line.extra) {
            for m in list {
                if let Some(s) = source {
                    set_source(m, s);
                }
                mod_list.push(m.clone());
            }
        }
    }
    *out = mods;
    Ok((mod_list, unknown, extra_flag))
}

impl PassiveTree {
    /// Adds the keystones of the timeless jewels' passives
    /// (`Data/TimelessJewelData/LegionPassives`) the tree has no keystone
    /// of the same name for
    pub fn add_legion_keystones(&mut self, legion: &Value, parser: &ModParser) -> Result<()> {
        let nodes = get(legion, "nodes");
        for (_, node) in nodes.as_table().map(|t| t.entries()).unwrap_or_default() {
            if !truthy(&node, "ks") {
                continue;
            }
            let dn: Rc<str> = lua::to_string(&get(&node, "dn")).to_str_lossy().into();
            if self.keystone_map.contains_key(&dn) || self.legion_keystones.contains_key(&dn) {
                continue;
            }
            let id = lua::to_string(&get(&node, "id"))
                .to_str_lossy()
                .into_owned();
            let mut sd = texts(&get(&node, "sd"))?;
            let (_, mod_list, _, _) = process_stats(parser, &mut sd, Some(&format!("Tree:{id}")))?;
            self.legion_keystones.insert(dn, mod_list);
        }
        Ok(())
    }

    /// The mods of a keystone by name (`keystoneMap[name].modList`)
    pub fn keystone_mods(&self, name: &str) -> Option<&[Mod]> {
        match self.keystone_map.get(name) {
            Some(&id) => self.node(id).map(|n| &n.mod_list[..]),
            None => self.legion_keystones.get(name).map(|m| &m[..]),
        }
    }

    pub fn node(&self, id: u32) -> Option<&Node> {
        self.node_index.get(&id).map(|&i| &self.nodes[i])
    }

    /// Builds the tree from its data (`tree.lua`), as `PassiveTree`'s
    /// constructor does; `radii` are the jewel radii for the version, and
    /// `latest` says it is upstream's latest tree version.
    pub fn new(
        version: &str,
        data: &Value,
        radii: &[(f64, f64)],
        parser: &ModParser,
        latest: bool,
    ) -> Result<PassiveTree> {
        let version_num = version_num(version);
        let new_format = version_num >= 3.10;

        // Classes; from 3.10 on the data is 1-based and is moved to 0-based
        let class_data = get(data, "classes");
        let mut class_tables: Vec<(i64, Value)> = Vec::new();
        for (k, v) in pairs(&class_data) {
            if let Key::Int(i) = k {
                class_tables.push((if new_format { i - 1 } else { i }, v));
            }
        }
        class_tables.sort_by_key(|(i, _)| *i);

        // Legacy alternate ascendancies upstream hides
        let legacy = ["Warden", "Warlock", "Primalist"];
        let alternate = get(data, "alternate_ascendancies");
        let mut legacy_ids: Vec<Rc<str>> = Vec::new();
        let mut alternates: Vec<(i64, Value)> = Vec::new();
        for (k, asc) in pairs(&alternate) {
            let id = text(&get(&asc, "id"))?;
            if legacy.contains(&&*id) {
                legacy_ids.push(id);
            } else if let Key::Int(i) = k {
                alternates.push((i, asc));
            }
        }

        let mut ascend_class_of: HashMap<Rc<str>, (i64, Rc<str>)> = HashMap::new();
        let mut classes = Vec::new();
        for (ci, class) in &class_tables {
            let name = text(&get(class, "name"))?;
            let asc_data = get(
                class,
                if new_format {
                    "ascendancies"
                } else {
                    "classes"
                },
            );
            let mut ascendancies = vec![(
                0,
                Ascendancy {
                    id: None,
                    name: "None".into(),
                    start_node_id: None,
                },
            )];
            for (k, asc) in pairs(&asc_data) {
                let Key::Int(ai) = k else { continue };
                let a = Ascendancy {
                    id: match get(&asc, "id") {
                        Value::Nil => None,
                        x => Some(text(&x)?),
                    },
                    name: text(&get(&asc, "name"))?,
                    start_node_id: None,
                };
                if let Some(id) = &a.id {
                    ascend_class_of.insert(id.clone(), (*ci, name.clone()));
                }
                ascend_class_of.insert(a.name.clone(), (*ci, name.clone()));
                ascendancies.push((ai, a));
            }
            let character = get(data, "characterData")
                .as_table()
                .map(|t| t.get(&Value::Number(*ci as f64)))
                .filter(Value::truthy);
            let stats_of = character.as_ref().unwrap_or(class);
            let stat = |k: &str| get(stats_of, k).as_number();
            classes.push((
                *ci,
                Class {
                    name,
                    ascendancies,
                    start_node_id: None,
                    base_stats: [stat("base_str"), stat("base_dex"), stat("base_int")],
                },
            ));
        }
        for (_, asc) in &alternates {
            let id = text(&get(asc, "id"))?;
            ascend_class_of.insert(id, (-1, "alternate_ascendancies".into()));
        }

        // Orbits
        let constants = get(data, "constants");
        let per_orbit: Vec<f64> = match get(&constants, "skillsPerOrbit") {
            Value::Table(t) => t
                .array()
                .iter()
                .map(number)
                .collect::<std::result::Result<_, _>>()?,
            _ => vec![1.0, 6.0, 12.0, 12.0, 40.0],
        };
        let radii_of_orbit: Vec<f64> = match get(&constants, "orbitRadii") {
            Value::Table(t) => t
                .array()
                .iter()
                .map(number)
                .collect::<std::result::Result<_, _>>()?,
            _ => vec![0.0, 82.0, 162.0, 335.0, 493.0],
        };
        let angles: Vec<Vec<f64>> = per_orbit.iter().map(|n| orbit_angles(*n)).collect();

        // Groups
        let mut groups = HashMap::new();
        let mut group_ids = Vec::new();
        for (k, g) in pairs(&get(data, "groups")) {
            let Key::Int(gi) = k else { continue };
            groups.insert(
                gi,
                Group {
                    x: number(&get(&g, "x"))?,
                    y: number(&get(&g, "y"))?,
                    is_proxy: truthy(&g, "isProxy"),
                    ascendancy_name: None,
                    is_ascendancy_start: false,
                },
            );
            group_ids.push(gi);
        }

        let mut tree = PassiveTree {
            version: version.into(),
            version_num,
            classes,
            groups,
            group_ids,
            nodes: Vec::new(),
            node_index: HashMap::new(),
            keystone_map: HashMap::new(),
            legion_keystones: HashMap::new(),
            notable_map: HashMap::new(),
            ascendancy_map: HashMap::new(),
            cluster_node_map: HashMap::new(),
            class_notables: HashMap::new(),
            sockets: Vec::new(),
            mastery_effects: HashMap::new(),
            alternate_ascendancies: None,
        };
        if !alternates.is_empty() {
            let mut list = Vec::new();
            for (i, asc) in &alternates {
                list.push((
                    *i,
                    Ascendancy {
                        id: Some(text(&get(asc, "id"))?),
                        name: text(&get(asc, "name"))?,
                        start_node_id: None,
                    },
                ));
            }
            tree.alternate_ascendancies = Some(list);
        }

        // Nodes, without the root and the legacy alternate ascendancies'
        let mut node_tables = Vec::new();
        for (k, node) in pairs(&get(data, "nodes")) {
            if matches!(&k, Key::Str(s) if s.as_bytes() == b"root") && new_format {
                continue;
            }
            let asc = get(&node, "ascendancyName");
            if let Value::Str(a) = &asc
                && legacy_ids.iter().any(|l| l.as_bytes() == a.as_bytes())
            {
                if let Some(g) = opt_int(&get(&node, "group"))? {
                    tree.groups.remove(&g);
                }
                continue;
            }
            node_tables.push(node);
        }
        for nt in &node_tables {
            let f = |a: &str, b: &str| get(nt, if new_format { a } else { b });
            let id = number(&f("skill", "id"))? as u32;
            let mut sd = texts(&f("stats", "sd"))?;
            let dn = text(&f("name", "dn"))?;
            let points = if new_format {
                match get(nt, "grantedPassivePoints") {
                    Value::Nil => 0.0,
                    x => number(&x)?,
                }
            } else {
                number(&get(nt, "passivePointsGranted")).unwrap_or(0.0)
            };
            if version_num <= 3.09 && points > 0.0 {
                sd.push(
                    format!(
                        "Grants {} Passive Skill Point{}",
                        lua::format::format_g(points, 14),
                        if points > 1.0 { "s" } else { "" }
                    )
                    .into(),
                );
            }
            let class_start_index = if new_format {
                opt_int(&get(nt, "classStartIndex"))?
            } else {
                opt_int(&rt_index(&get(nt, "spc"), 0))?
            };
            let ascendancy_name = match get(nt, "ascendancyName") {
                Value::Nil => None,
                x => Some(Sym::new(&text(&x)?)),
            };
            let is_mastery = truthy(nt, "isMastery");
            let kind = if class_start_index.is_some() {
                NodeType::ClassStart
            } else if truthy(nt, "isAscendancyStart") {
                NodeType::AscendClassStart
            } else if truthy(nt, "m") || is_mastery {
                NodeType::Mastery
            } else if truthy(nt, "isJewelSocket") {
                NodeType::Socket
            } else if truthy(nt, "ks") || truthy(nt, "isKeystone") {
                NodeType::Keystone
            } else if truthy(nt, "not") || truthy(nt, "isNotable") {
                NodeType::Notable
            } else {
                NodeType::Normal
            };
            let mut mastery_effects = Vec::new();
            if kind == NodeType::Mastery {
                for (_, effect) in pairs(&get(nt, "masteryEffects")) {
                    let eid = number(&get(&effect, "effect"))?;
                    mastery_effects.push(eid);
                    let key = eid.to_bits();
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        tree.mastery_effects.entry(key)
                    {
                        // Processed as a node whose id is the effect's
                        let mut esd = texts(&get(&effect, "stats"))?;
                        let source = format!("Tree:{}", lua::format::format_g(eid, 14));
                        let (mods, mod_list, _, _) =
                            process_stats(parser, &mut esd, Some(&source))?;
                        e.insert(MasteryEffect {
                            id: eid,
                            sd: esd,
                            mods,
                            mod_list,
                        });
                    }
                }
            }
            let is_multiple_choice_option = truthy(nt, "isMultipleChoiceOption");
            match kind {
                NodeType::ClassStart => {
                    let ci = class_start_index.unwrap();
                    if let Some((_, c)) = tree.classes.iter_mut().find(|(i, _)| *i == ci) {
                        c.start_node_id = Some(id);
                    }
                }
                NodeType::AscendClassStart => {
                    let a = ascendancy_name.ok_or("ascendancy start without ascendancy")?;
                    for (_, c) in tree.classes.iter_mut() {
                        for (_, asc) in c.ascendancies.iter_mut() {
                            if asc.id.as_deref() == Some(a.as_str()) || &*asc.name == a.as_str() {
                                asc.start_node_id = Some(id);
                            }
                        }
                    }
                    for (_, asc) in tree.alternate_ascendancies.iter_mut().flatten() {
                        if asc.id.as_deref() == Some(a.as_str()) {
                            asc.start_node_id = Some(id);
                        }
                    }
                }
                NodeType::Socket => tree.sockets.push(id),
                NodeType::Keystone => {
                    tree.keystone_map.insert(dn.clone(), id);
                    tree.keystone_map.insert(lower(&dn), id);
                }
                NodeType::Notable => match ascendancy_name {
                    None => {
                        let key = lower(&dn);
                        if !tree.notable_map.contains_key(&key)
                            || !get(nt, "group").is_nil() && new_format
                            || !get(nt, "g").is_nil() && !new_format
                        {
                            tree.notable_map.insert(key, id);
                        }
                    }
                    Some(a) => {
                        tree.ascendancy_map.insert(lower(&dn), id);
                        let class_name = ascend_class_of
                            .get(a.as_str())
                            .map(|(_, n)| n.clone())
                            .ok_or_else(|| format!("unknown ascendancy {a}"))?;
                        let list = tree.class_notables.entry(class_name.clone()).or_default();
                        if &*class_name != "Scion" {
                            list.push(dn.clone());
                        }
                    }
                },
                NodeType::Normal => {
                    let asc = ascendancy_name.map(|a| a.as_str());
                    if ((asc == Some("Ascendant")
                        && !is_multiple_choice_option
                        && !dn.contains("Dexterity")
                        && !dn.contains("Intelligence")
                        && !dn.contains("Strength")
                        && !dn.contains("Passive"))
                        || (is_multiple_choice_option && asc.is_some()))
                        && asc != Some("Reliquarian")
                        && asc != Some("Luminary")
                    {
                        let a = asc.unwrap();
                        let class_name = ascend_class_of
                            .get(a)
                            .map(|(_, n)| n.clone())
                            .ok_or_else(|| format!("unknown ascendancy {a}"))?;
                        tree.ascendancy_map.insert(lower(&dn), id);
                        tree.class_notables
                            .entry(class_name)
                            .or_default()
                            .push(dn.clone());
                    }
                }
                NodeType::Mastery => {}
            }
            // The node's group
            let group_id = opt_int(&get(nt, if new_format { "group" } else { "g" }))?;
            let group = group_id.filter(|g| tree.groups.contains_key(g));
            if let Some(g) = group {
                let grp = tree.groups.get_mut(&g).unwrap();
                grp.ascendancy_name = ascendancy_name;
                if truthy(nt, "isAscendancyStart") {
                    grp.is_ascendancy_start = true;
                }
            } else if matches!(kind, NodeType::Notable | NodeType::Keystone) {
                tree.cluster_node_map.insert(dn.clone(), id);
            }
            // Position
            let orbit = opt_int(&get(nt, if new_format { "orbit" } else { "o" }))?;
            let orbit_index = opt_int(&get(nt, if new_format { "orbitIndex" } else { "oidx" }))?;
            let (mut angle, mut x, mut y) = (None, None, None);
            if let Some(g) = group {
                let (o, oi) = (orbit.unwrap_or(0), orbit_index.unwrap_or(0));
                let a = angles
                    .get(o as usize)
                    .and_then(|v| v.get(oi as usize))
                    .copied()
                    .ok_or_else(|| format!("node {id}: no angle for orbit {o} index {oi}"))?;
                let r = radii_of_orbit[o as usize];
                let grp = &tree.groups[&g];
                angle = Some(a);
                x = Some(grp.x + a.sin() * r);
                y = Some(grp.y - a.cos() * r);
            }
            let source = format!("Tree:{id}");
            let (mods, mod_list, unknown, extra) = process_stats(parser, &mut sd, Some(&source))?;
            let keystone_mod = (kind == NodeType::Keystone).then(|| {
                create_mod(
                    "Keystone",
                    ModType::List,
                    ModValue::Str(dn.clone()),
                    &format!("Tree{id}"),
                )
            });
            tree.node_index.insert(id, tree.nodes.len());
            tree.nodes.push(Node {
                id,
                dn,
                sd,
                kind,
                ascendancy_name,
                class_start_index,
                group,
                orbit,
                orbit_index,
                angle,
                x,
                y,
                linked_id: Vec::new(),
                passive_points_granted: points,
                is_proxy: truthy(nt, "isProxy"),
                is_blighted: truthy(nt, "isBlighted"),
                is_mastery,
                is_multiple_choice: truthy(nt, "isMultipleChoice"),
                is_multiple_choice_option,
                is_ascendancy_start: truthy(nt, "isAscendancyStart"),
                expansion_jewel: match get(nt, "expansionJewel") {
                    Value::Nil => None,
                    ej => {
                        let t = |k: &str| match get(&ej, k) {
                            Value::Nil => Ok(None),
                            Value::Number(n) => Ok(Some(lua::format::format_g(n, 14).into())),
                            x => text(&x).map(Some),
                        };
                        Some(ExpansionJewel {
                            size: number(&get(&ej, "size"))?,
                            index: number(&get(&ej, "index"))?,
                            proxy: t("proxy")?,
                            parent: t("parent")?,
                        })
                    }
                },
                charm_socket: kind == NodeType::Socket
                    && get(nt, "name").raw_eq(&Value::str("Charm Socket")),
                mastery_effects,
                mods,
                mod_list,
                keystone_mod,
                unknown,
                extra,
                nodes_in_radius: None,
            });
        }

        // Links: `out`, then `in`
        for (ni, nt) in node_tables.iter().enumerate() {
            let mut linked = Vec::new();
            for key in ["out", "in"] {
                for (_, other) in pairs(&get(nt, key)) {
                    let other = match &other {
                        Value::Str(s) => lua::str_to_number(s.as_bytes())
                            .ok_or_else(|| format!("node link {s:?}"))?,
                        v => number(v)?,
                    };
                    linked.push(other as u32);
                }
            }
            tree.nodes[ni].linked_id = linked;
        }

        // Nodes within each jewel radius of each socket and keystone
        let max_radius = radii
            .iter()
            .fold(0.0f64, |m, (_, outer)| if *outer > m { *outer } else { m });
        let in_radius = |tree: &PassiveTree, centre: &Node| -> Vec<Vec<u32>> {
            let mut out = vec![Vec::new(); radii.len()];
            let (Some(cx), Some(cy)) = (centre.x, centre.y) else {
                return out;
            };
            let (min_x, max_x) = (cx - max_radius, cx + max_radius);
            let (min_y, max_y) = (cy - max_radius, cy + max_radius);
            for node in &tree.nodes {
                let (Some(x), Some(y)) = (node.x, node.y) else {
                    continue;
                };
                let group_proxy = node.group.is_some_and(|g| tree.groups[&g].is_proxy);
                if x >= min_x
                    && x <= max_x
                    && y >= min_y
                    && y <= max_y
                    && node.id != centre.id
                    && !node.is_blighted
                    && node.group.is_some()
                    && !node.is_proxy
                    && !group_proxy
                    && !node.is_mastery
                {
                    let (vx, vy) = (x - cx, y - cy);
                    let dist = vx * vx + vy * vy;
                    for (ri, (inner, outer)) in radii.iter().enumerate() {
                        if dist <= outer * outer && inner * inner <= dist {
                            out[ri].push(node.id);
                        }
                    }
                }
            }
            out
        };
        let sockets = tree.sockets.clone();
        for id in sockets {
            let node = tree.node(id).unwrap();
            if node.charm_socket {
                continue;
            }
            let r = in_radius(&tree, node);
            let i = tree.node_index[&id];
            tree.nodes[i].nodes_in_radius = Some(r);
        }
        let keystones: Vec<u32> = tree.keystone_map.values().copied().collect();
        for id in keystones {
            let i = tree.node_index[&id];
            if tree.nodes[i].nodes_in_radius.is_some() {
                continue;
            }
            let r = in_radius(&tree, &tree.nodes[i]);
            tree.nodes[i].nodes_in_radius = Some(r);
        }

        // Normal nodes next to a class start are connected to it
        for (_, class) in &tree.classes {
            let Some(start) = class.start_node_id.and_then(|s| tree.node(s)) else {
                continue;
            };
            let flag = format!("Condition:ConnectedTo{}Start", class.name);
            let targets: Vec<u32> = start.linked_id.clone();
            for nid in targets {
                let Some(&i) = tree.node_index.get(&nid) else {
                    continue;
                };
                if tree.nodes[i].kind == NodeType::Normal {
                    let m = create_mod(
                        &flag,
                        ModType::Flag,
                        ModValue::Bool(true),
                        &format!("Tree:{nid}"),
                    );
                    tree.nodes[i].mod_list.push(m);
                }
            }
        }
        // For the latest tree, upstream builds the Forbidden Flame and Flesh
        // uniques from the class notables, sorting each class's list
        if latest {
            for (class, list) in tree.class_notables.iter_mut() {
                if &**class != "alternate_ascendancies" {
                    list.sort();
                }
            }
        }
        Ok(tree)
    }

    /// Group ids in the data's order
    pub fn group_ids(&self) -> &[i64] {
        &self.group_ids
    }
}

fn rt_index(v: &Value, i: i64) -> Value {
    lua::rt::index(v, &Value::Number(i as f64)).unwrap_or_default()
}
