//! Find a Timeless Jewel (`TreeTab:FindTimelessJewel`): the build's
//! `timelessData` (loaded and saved with the build), and the popup's
//! settings: jewel type, conqueror, socket, the socket filter with its
//! node distance and protected notables, the devotion and Abyss ascendancy
//! choices, the node weight sliders and the node drop-down that write the
//! desired and fallback node lists, and the trade settings the tab
//! remembers. Searching, generating fallback weights and the trade URL are
//! not ported yet.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::item_popups::ItemPopup;
use super::tooltip::Tooltip;
use super::{NOT_PORTED, Result, TypedSession, UPSTREAM_ERROR, num};
use engine::breakdown::{Arg, lua_format};
use engine::pat::{gmatch1, is_match, to_number};
use engine::tree::NodeType;
use serde_json::{Value as J, json};

/// `jewelTypes`: label and name, by id from 1
pub(super) const JEWEL_TYPES: [(&str, &str); 11] = [
    ("Glorious Vanity", "vaal"),
    ("Lethal Pride", "karui"),
    ("Brutal Restraint", "maraketh"),
    ("Militant Faith", "templar"),
    ("Elegant Hubris", "eternal"),
    ("Heroic Tragedy", "kalguur"),
    ("Festering Vengeance", "abyss_murderous"),
    ("Extinguishing Grasp", "abyss_searching"),
    ("Baleful Dominion", "abyss_hypnotic"),
    ("Destructive Aspiration", "abyss_ghastly"),
    ("Reclaimed Malevolence", "abyss_special"),
];

/// `conquerorTypes` by jewel type: labels, by id from 1
const CONQUERORS: [&[&str]; 11] = [
    &[
        "Any",
        "Doryani (Corrupted Soul)",
        "Xibaqua (Divine Flesh)",
        "Ahuana (Immortal Ambition)",
    ],
    &[
        "Any",
        "Kaom (Strength of Blood)",
        "Rakiata (Tempered by War)",
        "Akoya (Chainbreaker)",
    ],
    &[
        "Any",
        "Asenath (Dance with Death)",
        "Nasima (Second Sight)",
        "Balbala (The Traitor)",
    ],
    &[
        "Any",
        "Avarius (Power of Purpose)",
        "Dominus (Inner Conviction)",
        "Maxarius (Transcendence)",
    ],
    &[
        "Any",
        "Cadiro (Supreme Decadence)",
        "Victario (Supreme Grandstanding)",
        "Caspiro (Supreme Ostentation)",
    ],
    &[
        "Any",
        "Vorana (Black Scythe Training)",
        "Uhtred (Celestial Mathematics)",
        "Medved (The Unbreaking Circle)",
    ],
    &["Tecrod (Overwhelming Hate)"],
    &["Ulaman (Weighted Exchange)"],
    &["Kurgal (Reconstructed Essence)"],
    &["Amanamu (The Loyal Few)"],
    &["Zorath"],
];

/// `devotionVariants`' labels, in list order
const DEVOTIONS: [&str; 17] = [
    "Any",
    "Totem Damage",
    "Brand Damage",
    "Channelling Damage",
    "Area Damage",
    "Elemental Damage",
    "Elemental Resistances",
    "Effect of non-Damaging Ailments",
    "Elemental Ailment Duration",
    "Duration of Curses",
    "Minion Attack and Cast Speed",
    "Minions Accuracy Rating",
    "Mana Regen",
    "Mana Cost (legacy)",
    "Mana Cost Efficiency",
    "Non-Curse Aura Effect",
    "Defences from Shield",
];

/// `ignoredMods`
const IGNORED: [&str; 9] = [
    "Might of the Vaal",
    "Legacy of the Vaal",
    "Strength",
    "Add Strength",
    "Dex",
    "Add Dexterity",
    "Devotion",
    "Price of Glory",
    "Ward",
];

/// `totalMods[jewelType]`
pub(super) fn total_mod(jewel: usize) -> Option<&'static str> {
    match jewel {
        2 => Some("Strength"),
        3 => Some("Dexterity"),
        4 => Some("Devotion"),
        _ => None,
    }
}

/// `build.timelessData`: what the build saves, and the total minimum
/// weight the popup keeps for the build
#[derive(Clone, Debug, Default)]
pub(super) struct TimelessData {
    /// The ids of `jewelType`, `conquerorType`, `jewelSocket` and
    /// `fallbackWeightMode.idx`; `None` for an empty table
    pub jewel_type: Option<f64>,
    pub conqueror: Option<f64>,
    pub devotion: [f64; 2],
    pub socket: Option<f64>,
    pub fallback_mode: Option<f64>,
    pub socket_filter: bool,
    pub socket_allocate: bool,
    pub socket_filter_distance: Option<f64>,
    pub search_list: Option<String>,
    pub search_list_fallback: Option<String>,
    pub total_minimum_weight: Option<f64>,
    /// `searchResults` and `sharedResults`, which the build keeps until
    /// it loads again
    pub results: Vec<super::timeless_seeds::SeedResult>,
    pub shared: Option<super::timeless_seeds::Shared>,
}

impl TimelessData {
    /// `buildMode:Init`'s table, then the build's `<TimelessData>`
    pub fn load(root: &engine::xml::Element) -> TimelessData {
        let t = root
            .child("PathOfBuilding")
            .and_then(|p| p.child("Build"))
            .and_then(|b| b.child("TimelessData"));
        let Some(t) = t else {
            return TimelessData {
                devotion: [1.0, 1.0],
                search_list: Some(String::new()),
                search_list_fallback: Some(String::new()),
                ..TimelessData::default()
            };
        };
        let n = |k: &str| t.attr(k).and_then(to_number);
        TimelessData {
            jewel_type: n("jewelTypeId"),
            conqueror: n("conquerorTypeId"),
            devotion: [
                n("devotionVariant1").unwrap_or(1.0),
                n("devotionVariant2").unwrap_or(1.0),
            ],
            socket: n("jewelSocketId"),
            fallback_mode: n("fallbackWeightModeIdx"),
            socket_filter: t.attr("socketFilter") == Some("true"),
            socket_allocate: t.attr("socketAllocate") == Some("true"),
            socket_filter_distance: Some(n("socketFilterDistance").unwrap_or(0.0)),
            search_list: t.attr("searchList").map(String::from),
            search_list_fallback: t.attr("searchListFallback").map(String::from),
            ..TimelessData::default()
        }
    }
}

/// A `jewelSockets` entry
#[derive(Clone, Debug)]
struct Socket {
    label: String,
    keystone: String,
    /// -1 for All Sockets
    id: f64,
}

/// A `modData` entry
#[derive(Clone, Debug)]
struct ModEntry {
    label: String,
    descriptions: Vec<String>,
    id: Option<String>,
}

/// A protection checkbox (`protectAllocatedBox<i>`)
#[derive(Clone, Debug)]
struct ProtectBox {
    label: String,
    /// The tree's node (`nodeInfo.node`)
    id: u32,
    state: bool,
}

/// The popup's controls
#[derive(Clone, Debug)]
pub struct FindTimeless {
    sockets: Vec<Socket>,
    socket_sel: usize,
    conqueror_sel: usize,
    jewel_sel: usize,
    devotion_sel: [usize; 2],
    /// `abyssAscendancyOptions` after Any: label and legion node id
    abyss: Vec<(String, String)>,
    abyss_sel: usize,
    protect: Vec<ProtectBox>,
    /// `protectedNodes` by label, and `protectedNodesCount`
    protected: Vec<(String, bool)>,
    protected_count: i64,
    socket_filter: bool,
    socket_allocate: bool,
    distance_shown: bool,
    distance: f64,
    mod_data: Vec<ModEntry>,
    node_sel: usize,
    /// The sliders' values and their value labels
    slider: [f64; 3],
    slider_label: [String; 3],
    slider2_enabled: bool,
    slider2_title: String,
    /// `nodeSliderStatLabel` and `nodeSlider2StatLabel`
    stat_label: [String; 2],
    /// `searchListTbl` and `searchListFallbackTbl`
    tbl: [Vec<Vec<String>>; 2],
    /// The two lists' edit boxes
    buf: [String; 2],
    fallback_shown: bool,
    fallback_button: String,
    fallback_sel: f64,
    total_minimum: String,
    search_more: bool,
    realm_sel: usize,
    trade_type_sel: usize,
    /// The results list's `selIndex`, whether it has a `selValue`, and
    /// `highlightIndex`
    results_sel: Option<usize>,
    results_value: bool,
    trade_enabled: bool,
    trade_label: String,
}

/// `DropDownControl:SetSel(sel)` on a list of `len`: the new selection if
/// it changed
fn set_sel(cur: &mut usize, sel: f64, len: usize) -> bool {
    let s = sel.min(len as f64).max(1.0);
    // `DropIndexToListIndex` of a fraction finds no row
    if s.fract() != 0.0 {
        return false;
    }
    let s = s as usize;
    let changed = s != *cur;
    *cur = s;
    changed
}

/// `SliderControl:SetVal(val)`: whether the value changed
fn set_val(cur: &mut f64, val: f64) -> bool {
    let v = val.clamp(0.0, 1.0);
    let changed = v != *cur;
    *cur = v;
    changed
}

fn fmt(f: &str, v: f64) -> String {
    lua_format(f, &[Arg::Num(v)])
}

/// A list's text as rows of values (`parseSearchList(0, ...)`)
fn parse_rows(text: &str) -> Vec<Vec<String>> {
    gmatch1(text, "[^\r\n]+")
        .iter()
        .map(|line| gmatch1(line, "([^,%s]+)"))
        .collect()
}

/// Lua's `round(val, dec)`
fn round(v: f64, dec: i32) -> f64 {
    let f = 10f64.powi(dec);
    (v * f + 0.5).floor() / f
}

fn tostring(v: f64) -> String {
    lua::value::format_number(v)
}

impl FindTimeless {
    pub(super) fn jewel(&self) -> usize {
        self.jewel_sel
    }

    /// `searchListTbl` and `searchListFallbackTbl`
    pub(super) fn tables(&self) -> &[Vec<Vec<String>>; 2] {
        &self.tbl
    }

    pub(super) fn protected_count(&self) -> i64 {
        self.protected_count
    }

    pub(super) fn is_protected(&self, label: &str) -> bool {
        self.protected.iter().any(|(l, s)| l == label && *s)
    }

    pub(super) fn socket_filter_state(&self) -> bool {
        self.socket_filter
    }

    /// `jewelSockets`: id, label and keystone
    pub(super) fn socket_list(&self) -> Vec<(f64, String, String)> {
        self.sockets
            .iter()
            .map(|s| (s.id, s.label.clone(), s.keystone.clone()))
            .collect()
    }

    pub(super) fn conqueror(&self) -> usize {
        self.conqueror_sel
    }

    /// `modData`'s ids, and whether each is a total (`totalMod`)
    pub(super) fn mod_ids(&self) -> Vec<(String, bool)> {
        self.mod_data
            .iter()
            .filter_map(|m| {
                let id = m.id.clone()?;
                let total = id.starts_with("total_");
                Some((id, total))
            })
            .collect()
    }

    /// `fallbackWeightsList.selIndex`
    pub(super) fn fallback_mode(&self) -> f64 {
        self.fallback_sel
    }

    /// What Generate sets on the fallback list's button
    pub(super) fn generated(&mut self) {
        self.fallback_button = "^4Fallback Nodes".into();
    }

    /// What a search sets on the results list and the trade button
    pub(super) fn searched(&mut self) {
        self.results_sel = Some(1);
        self.trade_label = "Open Trade URL".into();
    }

    pub(super) fn set_trade_enabled(&mut self, enabled: bool) {
        self.trade_enabled = enabled;
    }

    fn sel_id(&self) -> Option<&str> {
        self.mod_data
            .get(self.node_sel.wrapping_sub(1))
            .and_then(|m| m.id.as_deref())
    }

    /// `getNodeWeights()`: the value labels after their colour code
    fn node_weights(&self) -> [String; 3] {
        let w = |l: &str| {
            let v = l.get(2..).unwrap_or("").to_lowercase();
            match to_number(&v) {
                Some(n) => tostring(round(n, 3)),
                None => v,
            }
        };
        [
            w(&self.slider_label[0]),
            w(&self.slider_label[1]),
            w(&self.slider_label[2]),
        ]
    }

    /// `controls.protectAllocatedLabel.shown`
    fn protect_shown(&self, socket: Option<f64>) -> bool {
        matches!(self.jewel(), 4 | 11) && self.socket_filter && socket != Some(-1.0)
    }
}

impl<'d> TypedSession<'d> {
    fn timeless_data(&self) -> &TimelessData {
        &self.tree_tab.timeless
    }

    /// `buildMods()`: the node drop-down's entries for the jewel type
    fn timeless_mods(&self, jewel: usize) -> Result<Vec<ModEntry>> {
        let t = self
            .data
            .timeless()
            .ok_or_else(|| format!("{NOT_PORTED}: timeless jewels without their data"))?;
        let name = JEWEL_TYPES[jewel - 1].1;
        let prefix = format!("^{name}_.+");
        let mut mods = Vec::new();
        let mut small = Vec::new();
        for l in &t.nodes {
            let n = &l.node;
            if is_match(&n.id, &prefix)
                && !is_match(&n.id, "^abyss_special_ascendancy_notable_")
                && !IGNORED.contains(&&*n.dn)
                && n.kind != NodeType::Keystone
            {
                let descriptions: Vec<String> = n.sd.iter().map(|s| s.to_string()).collect();
                if n.kind == NodeType::Notable {
                    let mut label = format!(
                        "{}{}{}",
                        n.dn,
                        " ".repeat(48),
                        descriptions.first().map_or("", |s| s.as_str())
                    );
                    if let Some(s) = descriptions.get(1) {
                        label = format!("{label} {s}");
                    }
                    mods.push(ModEntry {
                        label,
                        descriptions,
                        id: Some(n.id.to_string()),
                    });
                } else {
                    small.push(ModEntry {
                        label: n.dn.to_string(),
                        descriptions,
                        id: Some(n.id.to_string()),
                    });
                }
            }
        }
        if name != "vaal" {
            for l in &t.additions {
                let n = &l.node;
                if is_match(&n.id, &prefix) && !IGNORED.contains(&&*n.dn) {
                    mods.push(ModEntry {
                        label: n.dn.to_string(),
                        descriptions: n.sd.iter().map(|s| s.to_string()).collect(),
                        id: Some(n.id.to_string()),
                    });
                }
            }
        }
        engine::calcs::lua_sort(&mut mods, |a, b| a.label < b.label);
        engine::calcs::lua_sort(&mut small, |a, b| a.label < b.label);
        if let Some(total) = total_mod(jewel) {
            mods.insert(
                0,
                ModEntry {
                    label: format!("Total {total}"),
                    descriptions: vec![format!(
                        "This is a hybrid node containing all additions to {total}"
                    )],
                    id: Some(format!("total_{}", total.to_lowercase())),
                },
            );
        }
        mods.insert(
            0,
            ModEntry {
                label: "...".into(),
                descriptions: Vec::new(),
                id: None,
            },
        );
        mods.extend(small);
        Ok(mods)
    }

    /// `jewelSockets`: All Sockets, then the tree's jewel sockets by label
    fn timeless_sockets(&self) -> Result<Vec<Socket>> {
        let spec = &self.build.spec;
        let mut out = Vec::new();
        for (_, &i) in spec.order.iter() {
            if spec.nodes[i].synth.is_some() {
                continue;
            }
            let node = spec.tree_node(i);
            if node.kind != NodeType::Socket || node.charm_socket {
                continue;
            }
            let id = node.id;
            let keystone = match id {
                26725 => "Marauder".to_string(),
                54127 => "Duelist".to_string(),
                7960 => "Templar/Witch".to_string(),
                _ => {
                    let tree_node = spec.tree.node(id).ok_or_else(|| {
                        format!("{UPSTREAM_ERROR}attempt to index a nil value (field '?')")
                    })?;
                    let mut keystone = "Unknown".to_string();
                    let mut min = f64::INFINITY;
                    let radius = tree_node
                        .nodes_in_radius
                        .as_ref()
                        .and_then(|r| r.get(2))
                        .ok_or_else(|| {
                            format!("{UPSTREAM_ERROR}attempt to index a nil value (field '?')")
                        })?;
                    for &r in radius {
                        let Some(n) = spec.tree.node(r) else { continue };
                        if n.kind == NodeType::Keystone {
                            let d = ((n.x.unwrap_or(0.0) - node.x.unwrap_or(0.0)).powi(2)
                                + (n.y.unwrap_or(0.0) - node.y.unwrap_or(0.0)).powi(2))
                            .sqrt();
                            if d < min {
                                keystone = n.dn.to_string();
                                min = d;
                            }
                        }
                    }
                    keystone
                }
            };
            let mut label = format!("{keystone}: {id}");
            if spec.alloc_nodes.contains(id) {
                label = format!("# {label}");
            }
            out.push(Socket {
                label,
                keystone,
                id: id as f64,
            });
        }
        engine::calcs::lua_sort(&mut out, |a, b| a.label < b.label);
        out.insert(
            0,
            Socket {
                label: "All Sockets".into(),
                keystone: "Multi-Socket Search".into(),
                id: -1.0,
            },
        );
        Ok(out)
    }

    /// `abyssAscendancyOptions` after Any
    fn timeless_abyss_options(&self) -> Result<Vec<(String, String)>> {
        let t = self
            .data
            .timeless()
            .ok_or_else(|| format!("{NOT_PORTED}: timeless jewels without their data"))?;
        let mut out: Vec<(String, String)> = t
            .nodes
            .iter()
            .filter(|l| is_match(&l.node.id, "^abyss_special_ascendancy_notable_%d+$"))
            .map(|l| (l.node.dn.to_string(), l.node.id.to_string()))
            .collect();
        engine::calcs::lua_sort(&mut out, |a, b| a.0 < b.0);
        Ok(out)
    }

    /// `setAllocatedNodes()`: the protection checkboxes of the socket's
    /// allocated notables
    fn timeless_set_allocated(&self, p: &mut FindTimeless) -> Result<()> {
        p.protect.clear();
        p.protected.clear();
        p.protected_count = 0;
        self.tree_tab_mut_allocated(0);
        let spec = &self.build.spec;
        let socket = self.timeless_data().socket;
        let Some(socket) = socket.filter(|s| *s != -1.0) else {
            return Ok(());
        };
        let Some(tree_socket) = spec.tree.node(socket as u32) else {
            return Ok(());
        };
        let mut options: Vec<(String, u32)> = Vec::new();
        if p.jewel() == 11 {
            let class = spec.ascend_class_name();
            for (id, _) in spec.alloc_nodes.iter() {
                if let Some(n) = spec.tree.node(id)
                    && n.ascendancy_name.as_ref().map(|s| s.as_str()) == class.as_deref()
                    && n.kind == NodeType::Notable
                {
                    options.push((n.dn.to_string(), id));
                }
            }
        } else {
            let granted = &self.sidebar()?.granted_passives;
            let radius = tree_socket
                .nodes_in_radius
                .as_ref()
                .and_then(|r| r.get(2))
                .cloned()
                .unwrap_or_default();
            for id in radius {
                if (granted.contains(&id) || spec.alloc_nodes.contains(id))
                    && let Some(n) = spec.tree.node(id)
                    && n.kind == NodeType::Notable
                {
                    options.push((n.dn.to_string(), id));
                }
            }
        }
        engine::calcs::lua_sort(&mut options, |a, b| a.0 < b.0);
        self.tree_tab_mut_allocated(options.len());
        p.protect = options
            .into_iter()
            .map(|(label, id)| ProtectBox {
                label,
                id,
                state: false,
            })
            .collect();
        Ok(())
    }

    /// `self.allocatedNodesInRadiusCount`, which only the search reads
    fn tree_tab_mut_allocated(&self, n: usize) {
        self.tree_tab.allocated_in_radius.set(n);
    }

    /// Find Timeless Jewel: the popup with the build's choices
    pub(super) fn open_find_timeless(&mut self) -> Result<()> {
        let mut td = self.tree_tab.timeless.clone();
        let jewel = match td.jewel_type {
            None => 1,
            Some(j) if (1.0..=11.0).contains(&j) && j.fract() == 0.0 => j as usize,
            Some(_) => return Err(format!("{NOT_PORTED}: an unknown timeless jewel type")),
        };
        td.jewel_type = Some(jewel as f64);
        let conquerors = CONQUERORS[jewel - 1].len();
        let conqueror = match td.conqueror {
            None => 1,
            Some(c) if c >= 1.0 && c <= conquerors as f64 && c.fract() == 0.0 => c as usize,
            Some(_) => return Err(format!("{NOT_PORTED}: an unknown conqueror")),
        };
        td.conqueror = Some(conqueror as f64);
        let sockets = self.timeless_sockets()?;
        let socket_sel =
            match td.socket {
                None => 1,
                Some(id) => {
                    sockets.iter().position(|s| s.id == id).ok_or_else(|| {
                        format!("{NOT_PORTED}: a timeless jewel socket not listed")
                    })? + 1
                }
            };
        td.socket = Some(sockets[socket_sel - 1].id);
        let devotion_sel = td.devotion.map(|d| d as usize);
        if td
            .devotion
            .iter()
            .any(|d| d.fract() != 0.0 || *d < 1.0 || *d > DEVOTIONS.len() as f64)
        {
            return Err(format!("{NOT_PORTED}: an unknown devotion variant"));
        }
        let mod_data = self.timeless_mods(jewel)?;
        let abyss = self.timeless_abyss_options()?;
        let tbl = [
            parse_rows(td.search_list.as_deref().unwrap_or("")),
            parse_rows(td.search_list_fallback.as_deref().unwrap_or("")),
        ];
        let tt = &self.tree_tab;
        let realm_sel = tt.realm.unwrap_or(1);
        let trade_type_sel = tt.trade_type.unwrap_or(1);
        let search_more = tt.search_more;
        let mut p = FindTimeless {
            sockets,
            socket_sel,
            conqueror_sel: conqueror,
            jewel_sel: jewel,
            devotion_sel,
            abyss,
            abyss_sel: 1,
            protect: Vec::new(),
            protected: Vec::new(),
            protected_count: 0,
            socket_filter: td.socket_filter,
            socket_allocate: td.socket_allocate,
            distance_shown: td.socket_filter,
            distance: 0.0,
            mod_data,
            node_sel: 1,
            slider: [0.0; 3],
            slider_label: ["^71.000".into(), "^71.000".into(), "^70".into()],
            slider2_enabled: true,
            slider2_title: "^7Secondary Node Weight:".into(),
            stat_label: ["None".into(), "None".into()],
            tbl,
            buf: [
                td.search_list.clone().unwrap_or_default(),
                td.search_list_fallback.clone().unwrap_or_default(),
            ],
            fallback_shown: false,
            fallback_button: "^7Fallback Nodes".into(),
            fallback_sel: td.fallback_mode.unwrap_or(1.0),
            total_minimum: String::new(),
            search_more,
            realm_sel,
            trade_type_sel,
            results_sel: None,
            results_value: false,
            trade_enabled: !self.tree_tab.timeless.results.is_empty(),
            trade_label: "Open Trade URL".into(),
        };
        self.tree_tab.timeless = td;
        self.tree_tab.realm = Some(realm_sel);
        self.tree_tab.trade_type = Some(trade_type_sel);
        self.tree_tab.allocated_in_radius.set(0);
        if p.socket_filter {
            self.timeless_set_allocated(&mut p)?;
        }
        for row in &p.tbl[0] {
            for (i, (_, id)) in p.abyss.iter().enumerate() {
                if row.first() == Some(id) {
                    p.abyss_sel = i + 2;
                    break;
                }
            }
        }
        // `socketFilterAdditionalDistance:SetVal(distance / 10)`
        let d = self.tree_tab.timeless.socket_filter_distance.unwrap_or(0.0) / 10.0;
        if set_val(&mut p.distance, d) {
            self.timeless_distance_changed(&p);
        }
        p.slider = [0.1, 0.1, 0.0];
        self.open_tree_popup(ItemPopup::FindTimeless(Box::new(p)))
    }

    fn timeless_distance_changed(&mut self, p: &FindTimeless) {
        self.tree_tab.timeless.socket_filter_distance = Some((p.distance * 10.0 + 0.01).floor());
    }

    /// `updateSearchList(text, fallback)`
    pub(super) fn timeless_update_list(
        &mut self,
        p: &mut FindTimeless,
        text: &str,
        fallback: bool,
    ) {
        let i = fallback as usize;
        if fallback {
            self.tree_tab.timeless.search_list_fallback = Some(text.to_string());
        } else {
            self.tree_tab.timeless.search_list = Some(text.to_string());
        }
        p.buf[i] = text.to_string();
        p.tbl[i] = parse_rows(text);
        self.ui.unsaved = true;
    }

    /// `parseSearchList(1, fallback)`: the selected node's row takes the
    /// sliders' weights, and the list is written back
    fn timeless_write_list(&mut self, p: &mut FindTimeless, fallback: bool) {
        let i = fallback as usize;
        let id = p.sel_id().map(String::from);
        let weights = p.node_weights();
        let mut text = String::new();
        for row in p.tbl[i].iter_mut() {
            if id.is_some() && row.first().map(String::as_str) == id.as_deref() {
                row.resize(row.len().max(4), String::new());
                row[1..4].clone_from_slice(&weights);
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&row.join(", "));
        }
        let data = &mut self.tree_tab.timeless;
        let cur = if fallback {
            &mut data.search_list_fallback
        } else {
            &mut data.search_list
        };
        if cur.as_deref() != Some(&text) {
            *cur = Some(text.clone());
            p.buf[i] = text;
            self.ui.unsaved = true;
        }
    }

    /// `updateSliders(sliderData)`
    fn timeless_update_sliders(p: &mut FindTimeless, row: &[String]) {
        let cell = |i: usize| row.get(i).map(String::as_str);
        let val =
            |i: usize, div: f64| (cell(i).and_then(to_number).unwrap_or(0.0) / div).clamp(0.0, 1.0);
        if cell(1) == Some("required") {
            p.slider[0] = 1.0;
            p.slider_label[0] = fmt("^7%.3f", 10.0);
        } else {
            p.slider[0] = val(1, 10.0);
            p.slider_label[0] = fmt("^7%.3f", p.slider[0] * 10.0);
        }
        if p.slider2_enabled {
            if cell(2) == Some("required") {
                p.slider[1] = 1.0;
                p.slider_label[1] = fmt("^7%.3f", 10.0);
            } else {
                p.slider[1] = val(2, 10.0);
                p.slider_label[1] = fmt("^7%.3f", p.slider[1] * 10.0);
            }
        }
        if cell(3) == Some("required") {
            p.slider[2] = 1.0;
            p.slider_label[2] = "^7Required".into();
        } else {
            p.slider[2] = val(3, 500.0);
            p.slider_label[2] = fmt("^7%.f", p.slider[2] * 500.0);
        }
    }

    /// `getLegionStatLabels(legionPassive)` of the node with this id, in
    /// the legion nodes then the additions
    fn timeless_stat_labels(&self, jewel: usize, id: &str) -> (usize, String, String) {
        let Some(t) = self.data.timeless() else {
            return (0, "None".into(), "None".into());
        };
        let labels = |l: &engine::timeless::LegionNode| {
            let sd = &l.node.sd;
            let count = if jewel >= 7 {
                l.sorted_stats.len()
            } else {
                sd.len()
            };
            if count > sd.len() {
                let first = sd.first().map_or("", |s| &**s);
                return (
                    count,
                    format!("Minimum value: {first}"),
                    format!("Maximum value: {first}"),
                );
            }
            let first = if count == 1 {
                sd.iter().map(|s| &**s).collect::<Vec<_>>().join(" + ")
            } else {
                sd.first().map_or("None".into(), |s| s.to_string())
            };
            (
                count,
                first,
                sd.get(1).map_or("None".into(), |s| s.to_string()),
            )
        };
        // The labels of the node found, from the additions when the
        // legion node has no stat
        let mut out = None;
        if let Some(l) = t.nodes.iter().find(|l| &*l.node.id == id) {
            out = Some(labels(l));
        }
        if out.as_ref().is_none_or(|o| o.0 == 0)
            && let Some(l) = t.additions.iter().find(|l| &*l.node.id == id)
        {
            out = Some(labels(l));
        }
        out.unwrap_or((0, "None".into(), "None".into()))
    }

    /// `nodeSelect`'s `selFunc`
    fn timeless_node_selected(&mut self, p: &mut FindTimeless) {
        p.stat_label = ["None".into(), "None".into()];
        let Some(id) = p.sel_id().map(String::from) else {
            return;
        };
        let (count, l1, l2) = self.timeless_stat_labels(p.jewel(), &id);
        p.stat_label = [l1, l2];
        if count <= 1 {
            p.slider2_title = "^9Secondary Node Weight:".into();
            p.slider[1] = 0.0;
            p.slider_label[1] = fmt("^9%.3f", 0.0);
        } else {
            p.slider2_title = "^7Secondary Node Weight:".into();
            p.slider_label[1] = fmt("^7%.3f", p.slider[1] * 10.0);
        }
        p.slider2_enabled = count > 1;
        let w = p.node_weights();
        let new_node = format!("{id}, {}, {}, {}", w[0], w[1], w[2]);
        let i = p.fallback_shown as usize;
        if let Some(row) = p.tbl[i].iter().find(|r| r.first() == Some(&id)).cloned() {
            Self::timeless_update_sliders(p, &row);
            return;
        }
        // `Insert` at the end, then the edit box's `changeFunc`
        let sep = if p.buf[i].is_empty() { "" } else { "\n" };
        let text = format!("{}{sep}{new_node}", p.buf[i]);
        if i == 1 {
            self.tree_tab.timeless.search_list_fallback = Some(text.clone());
        } else {
            self.tree_tab.timeless.search_list = Some(text.clone());
        }
        p.tbl[i] = parse_rows(&text);
        p.buf[i] = text;
        self.ui.unsaved = true;
    }

    /// A `popup.<name>` control of Find a Timeless Jewel
    pub(super) fn find_timeless_control(
        &mut self,
        p: &mut FindTimeless,
        name: &str,
        req: &J,
    ) -> Result<()> {
        let sel = req["sel"].as_f64();
        let text = req["text"].as_str();
        let state = req["state"].as_bool();
        let val = req["val"].as_f64();
        let unported = || Err(format!("{NOT_PORTED}: control popup.{name}"));
        match name {
            "jewelSelect" => {
                let Some(s) = sel else { return unported() };
                if set_sel(&mut p.jewel_sel, s, JEWEL_TYPES.len()) {
                    let j = p.jewel_sel;
                    self.tree_tab.timeless.jewel_type = Some(j as f64);
                    p.abyss_sel = 1;
                    p.conqueror_sel = 1;
                    self.tree_tab.timeless.conqueror = Some(1.0);
                    p.node_sel = 1;
                    p.mod_data = self.timeless_mods(j)?;
                    self.timeless_update_list(p, "", false);
                    self.timeless_update_list(p, "", true);
                    if p.socket_filter {
                        self.timeless_set_allocated(p)?;
                    }
                }
            }
            "conquerorSelect" => {
                let Some(s) = sel else { return unported() };
                let len = CONQUERORS[p.jewel() - 1].len();
                if set_sel(&mut p.conqueror_sel, s, len) {
                    self.tree_tab.timeless.conqueror = Some(p.conqueror_sel as f64);
                    self.ui.unsaved = true;
                }
            }
            "socketSelect" => {
                let Some(s) = sel else { return unported() };
                if set_sel(&mut p.socket_sel, s, p.sockets.len()) {
                    self.tree_tab.timeless.socket = Some(p.sockets[p.socket_sel - 1].id);
                    self.timeless_set_allocated(p)?;
                    self.ui.unsaved = true;
                }
            }
            "devotionSelect1" | "devotionSelect2" => {
                let Some(s) = sel else { return unported() };
                let i = (name == "devotionSelect2") as usize;
                if set_sel(&mut p.devotion_sel[i], s, DEVOTIONS.len()) {
                    self.tree_tab.timeless.devotion[i] = p.devotion_sel[i] as f64;
                }
            }
            "socketFilter" => {
                let Some(s) = state else { return unported() };
                p.socket_filter = s;
                self.tree_tab.timeless.socket_filter = s;
                self.ui.unsaved = true;
                p.distance_shown = s;
                if s {
                    self.timeless_set_allocated(p)?;
                } else {
                    p.protect.clear();
                    p.protected.clear();
                    p.protected_count = 0;
                }
            }
            "socketAllocate" => {
                let Some(s) = state else { return unported() };
                p.socket_allocate = s;
                self.tree_tab.timeless.socket_allocate = s;
                self.ui.unsaved = true;
            }
            "socketFilterAdditionalDistance" => {
                let Some(v) = val else { return unported() };
                if set_val(&mut p.distance, v) {
                    self.timeless_distance_changed(p);
                }
            }
            "nodeSlider" | "nodeSlider2" | "nodeSlider3" => {
                let Some(v) = val else { return unported() };
                let i = match name {
                    "nodeSlider" => 0,
                    "nodeSlider2" => 1,
                    _ => 2,
                };
                if set_val(&mut p.slider[i], v) {
                    let v = p.slider[i];
                    p.slider_label[i] = match i {
                        2 if v == 1.0 => "^7Required".into(),
                        2 => fmt("^7%.f", v * 500.0),
                        _ => fmt("^7%.3f", v * 10.0),
                    };
                    let fallback = p.fallback_shown;
                    self.timeless_write_list(p, fallback);
                }
            }
            "nodeSelect" => {
                let Some(s) = sel else { return unported() };
                if set_sel(&mut p.node_sel, s, p.mod_data.len()) {
                    self.timeless_node_selected(p);
                }
            }
            "fallbackWeightsList" => {
                let Some(s) = sel else { return unported() };
                let len = engine::power::power_stats()
                    .iter()
                    .filter(|s| !s.ignore_for_items && s.label != "Name")
                    .count();
                let mut cur = p.fallback_sel as usize;
                let was = p.fallback_sel;
                set_sel(&mut cur, s, len);
                if cur as f64 != was {
                    p.fallback_sel = cur as f64;
                    self.tree_tab.timeless.fallback_mode = Some(cur as f64);
                }
            }
            "totalMinimumWeight" => {
                let Some(t) = text else { return unported() };
                p.total_minimum = t.to_string();
                self.tree_tab.timeless.total_minimum_weight = to_number(t);
                self.ui.unsaved = true;
            }
            "searchList" | "searchListFallback" => {
                let Some(t) = text else { return unported() };
                let i = (name == "searchListFallback") as usize;
                p.buf[i] = t.to_string();
                if i == 1 {
                    self.tree_tab.timeless.search_list_fallback = Some(t.to_string());
                } else {
                    self.tree_tab.timeless.search_list = Some(t.to_string());
                }
                p.tbl[i] = parse_rows(t);
                self.ui.unsaved = true;
            }
            "abyssAscendancySelect" => {
                let Some(s) = sel else { return unported() };
                if set_sel(&mut p.abyss_sel, s, p.abyss.len() + 1) {
                    let mut lines: Vec<String> = p.tbl[0]
                        .iter()
                        .filter(|r| {
                            !is_match(
                                r.first().map_or("", String::as_str),
                                "^abyss_special_ascendancy_notable_",
                            )
                        })
                        .map(|r| r.join(", "))
                        .collect();
                    if let Some((_, id)) = p.abyss.get(p.abyss_sel.wrapping_sub(2)) {
                        lines.push(format!("{id}, 1, 0, 1"));
                    }
                    let text = lines.join("\n");
                    self.timeless_update_list(p, &text, false);
                }
            }
            "realmSelection" => {
                let Some(s) = sel else { return unported() };
                if set_sel(&mut p.realm_sel, s, 3) {
                    self.tree_tab.realm = Some(p.realm_sel);
                }
            }
            "tradeTypeSelection" => {
                let Some(s) = sel else { return unported() };
                if set_sel(&mut p.trade_type_sel, s, 5) {
                    self.tree_tab.trade_type = Some(p.trade_type_sel);
                }
            }
            // An empty list: nothing to select
            "searchTradeLeagueSelect" if sel.is_some() => {}
            "searchMore" => {
                let Some(s) = state else { return unported() };
                p.search_more = s;
                self.tree_tab.search_more = s;
            }
            "searchResults" if sel.is_some() && req.get("key").is_none() => {
                // `SelectIndex`: no row leaves the selection's index
                let s = sel.unwrap_or(0.0);
                let rows = self.tree_tab.timeless.results.len();
                let row =
                    (s >= 1.0 && s.fract() == 0.0 && s as usize <= rows).then_some(s as usize);
                p.results_value = row.is_some();
                let Some(row) = row else { return Ok(()) };
                p.results_sel = Some(row);
                if req["doubleClick"].as_bool() == Some(true)
                    && req["click"].as_bool() == Some(true)
                    && !self.tree_tab.timeless.results[row - 1]
                        .label
                        .contains("B2B2B2")
                {
                    self.timeless_add_jewel(row)?;
                }
            }
            n if n.starts_with("protectAllocatedBox") => {
                let Some(s) = state else { return unported() };
                let i: usize = n["protectAllocatedBox".len()..]
                    .parse()
                    .map_err(|_| format!("{NOT_PORTED}: control popup.{name}"))?;
                let Some(b) = p.protect.get_mut(i.wrapping_sub(1)) else {
                    return Err(format!("no control popup.{name}"));
                };
                b.state = s;
                let label = b.label.clone();
                match p.protected.iter_mut().find(|(l, _)| *l == label) {
                    Some(e) => e.1 = s,
                    None => p.protected.push((label, s)),
                }
                p.protected_count += if s { 1 } else { -1 };
            }
            _ => return unported(),
        }
        Ok(())
    }

    /// A button of Find a Timeless Jewel: whether it closes the popup
    pub(super) fn find_timeless_button(
        &mut self,
        p: &mut FindTimeless,
        name: &str,
    ) -> Result<bool> {
        match name {
            "searchListButton" => {
                p.fallback_shown = false;
            }
            "searchListFallbackButton" => {
                p.fallback_shown = true;
                p.fallback_button = "^7Fallback Nodes".into();
            }
            "resetButton" => {
                self.timeless_update_list(p, "", true);
                self.timeless_update_list(p, "", false);
                p.abyss_sel = 1;
                self.tree_tab.timeless.results.clear();
                p.trade_enabled = false;
                self.timeless_set_allocated(p)?;
            }
            "searchButton" => self.timeless_search(p)?,
            "fallbackWeightsButton" => {
                self.timeless_generate(p)?;
                p.generated();
            }
            "closeButton" => return Ok(true),
            _ => return Err(format!("{NOT_PORTED}: popup button {name}")),
        }
        Ok(false)
    }

    /// The popup as `describePopups` gives it
    pub(super) fn describe_find_timeless(&self, p: &FindTimeless) -> J {
        let td = self.timeless_data();
        let jewel = p.jewel();
        // (y, x, path, control)
        let mut controls: Vec<(f64, f64, String, J)> = Vec::new();
        let mut add = |y: f64, x: f64, name: &str, mut c: J| {
            c["path"] = json!(format!("popup.{name}"));
            c["shown"] = json!(true);
            if c.get("enabled").is_none() {
                c["enabled"] = json!(true);
            }
            controls.push((y, x, format!("popup.{name}"), c));
        };
        let drop = |list: Vec<J>, sel: J, label: Option<&str>| {
            let mut c = json!({ "class": "DropDownControl", "list": list, "sel": sel });
            if let Some(l) = label {
                c["label"] = json!(l);
            }
            c
        };
        let check = |state: bool, label: &str| json!({ "class": "CheckBoxControl", "state": state, "label": label });
        let slider = |v: f64, label: &str| json!({ "class": "SliderControl", "val": num(v), "label": label });
        let labels = |l: &[&str]| l.iter().map(|s| json!(s)).collect::<Vec<J>>();
        add(
            25.0,
            520.0,
            "jewelSelect",
            drop(
                JEWEL_TYPES.iter().map(|t| json!(t.0)).collect(),
                json!(p.jewel_sel),
                Some("^7Jewel Type:"),
            ),
        );
        add(
            48.0,
            520.0,
            "conquerorSelect",
            drop(
                labels(CONQUERORS[jewel - 1]),
                json!(p.conqueror_sel),
                Some("^7Conqueror:"),
            ),
        );
        if jewel == 4 {
            for (i, y) in [49.0, 74.0].into_iter().enumerate() {
                add(
                    y,
                    819.0,
                    &format!("devotionSelect{}", i + 1),
                    drop(labels(&DEVOTIONS), json!(p.devotion_sel[i]), None),
                );
            }
        }
        if jewel == 11 {
            let mut list = vec![json!("Any")];
            list.extend(p.abyss.iter().map(|(l, _)| json!(l)));
            add(
                49.0,
                15.0,
                "abyssAscendancySelect",
                drop(list, json!(p.abyss_sel), None),
            );
        }
        if p.protect_shown(td.socket) {
            let top = if jewel == 11 { 85.0 } else { 25.0 };
            for (i, b) in p.protect.iter().enumerate() {
                add(
                    top + 20.0 + 22.0 * i as f64,
                    15.0,
                    &format!("protectAllocatedBox{}", i + 1),
                    check(b.state, &b.label),
                );
            }
        }
        if p.distance_shown {
            add(
                93.0,
                558.0,
                "socketFilterAdditionalDistance",
                slider(p.distance, "^7Node Distance:"),
            );
        }
        add(
            94.0,
            520.0,
            "socketFilter",
            check(p.socket_filter, "^7Filter Nodes:"),
        );
        add(
            117.0,
            520.0,
            "socketAllocate",
            check(p.socket_allocate, "^7Socket Jewel:"),
        );
        add(
            140.0,
            520.0,
            "nodeSlider",
            slider(p.slider[0], "^7Primary Node Weight:"),
        );
        let mut s2 = slider(p.slider[1], &p.slider2_title);
        s2["enabled"] = json!(p.slider2_enabled);
        add(163.0, 520.0, "nodeSlider2", s2);
        add(
            186.0,
            520.0,
            "nodeSlider3",
            slider(p.slider[2], "^7Minimum Node Weight:"),
        );
        add(
            209.0,
            520.0,
            "nodeSelect",
            drop(
                p.mod_data.iter().map(|m| json!(m.label)).collect(),
                json!(p.node_sel),
                Some("^7Search for Node:"),
            ),
        );
        let weights: Vec<J> = engine::power::power_stats()
            .iter()
            .filter(|s| !s.ignore_for_items && s.label != "Name")
            .map(|s| json!(format!("Sort by {}", s.label)))
            .collect();
        add(
            232.0,
            520.0,
            "fallbackWeightsList",
            drop(weights, num(p.fallback_sel), None),
        );
        add(
            255.0,
            520.0,
            "totalMinimumWeight",
            json!({ "class": "EditControl", "text": p.total_minimum, "label": "^7Total Minimum Weight:" }),
        );
        let mut more = check(p.search_more, "^7Search Maximum Amount:");
        more["tooltip"] = json!(
            "By default, this tool only searches for 4 or 10 (depending on conqueror) seeds at once to preserve sorting.\nThis option searches up to 180 or 60 seeds at once if you don't care about the sorting."
        );
        add(267.0, 1005.0, "searchMore", more);
        add(
            292.0,
            710.0,
            "realmSelection",
            drop(labels(&["PC", "Sony", "Xbox"]), json!(p.realm_sel), None),
        );
        add(
            292.0,
            819.0,
            "tradeTypeSelection",
            drop(
                labels(&[
                    "Instant buyout",
                    "Instant buyout and in person",
                    "In person (online in league)",
                    "In person (online)",
                    "Any (includes offline)",
                ]),
                json!(p.trade_type_sel),
                None,
            ),
        );
        add(
            318.0,
            710.0,
            "searchTradeLeagueSelect",
            drop(Vec::new(), json!(1), None),
        );
        let (list_name, i) = if p.fallback_shown {
            ("searchListFallback", 1)
        } else {
            ("searchList", 0)
        };
        add(
            344.0,
            12.0,
            list_name,
            json!({ "class": "EditControl", "text": p.buf[i], "multiline": true }),
        );
        let rows: Vec<J> = td.results.iter().map(|r| json!(r.label)).collect();
        let mut results = json!({ "class": "TimelessJewelListControl", "list": rows });
        if p.results_value
            && let Some(s) = p.results_sel
        {
            results["sel"] = json!(s);
        }
        add(344.0, 524.0, "searchResults", results);
        controls.sort_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then(a.1.total_cmp(&b.1))
                .then(a.2.cmp(&b.2))
        });
        let button = |name: &str, label: &str, enabled: bool| json!({ "name": name, "label": label, "enabled": enabled });
        json!({
            "title": "Find a Timeless Jewel",
            "lines": [],
            "buttons": [
                button("closeButton", "Cancel", true),
                button("fallbackWeightsButton", "Generate", true),
                button("resetButton", "Reset", true),
                button("searchButton", "Search", true),
                button("searchListButton", "^7Desired Nodes", true),
                button("searchListFallbackButton", &p.fallback_button, true),
                button("searchTradeButton", &p.trade_label, p.trade_enabled),
            ],
            "controls": controls.into_iter().map(|c| c.3).collect::<Vec<J>>(),
        })
    }

    /// `controlTooltip` of a control of Find a Timeless Jewel
    pub(super) fn find_timeless_tooltip(
        &mut self,
        p: &FindTimeless,
        name: &str,
        index: Option<f64>,
    ) -> Result<J> {
        let mut tip = Tooltip::default();
        let idx = index
            .filter(|i| *i >= 1.0 && i.fract() == 0.0)
            .map(|i| i as usize);
        // Upstream's message, without the local LuaJIT names
        let nil = || Err(format!("{UPSTREAM_ERROR}attempt to index a nil value"));
        let mut lines = |l: &[&str]| {
            for s in l {
                tip.add_line(16.0, s);
            }
        };
        match name {
            "conquerorSelect" => {
                let Some(label) = idx.and_then(|i| CONQUERORS[p.jewel() - 1].get(i - 1)) else {
                    return nil();
                };
                // `spec.tree.keystoneMap`: the tree's keystone, else the
                // legion keystone of that name
                let tree = self.build.spec.tree;
                let name = engine::pat::capture(label, "%((.+)%)$");
                if let Some(name) = name {
                    if let Some(n) = tree
                        .keystone_map
                        .get(name.as_str())
                        .and_then(|&id| tree.node(id))
                    {
                        self.base_node_tooltip(
                            &mut tip,
                            &n.dn,
                            &n.sd,
                            &n.mods,
                            n.reminder_text.as_deref(),
                            n.flavour_text.as_deref(),
                        );
                    } else if let Some(t) = self.data.timeless()
                        && let Some(l) = t
                            .nodes
                            .iter()
                            .find(|l| l.node.kind == NodeType::Keystone && *l.node.dn == *name)
                    {
                        let n = &l.node;
                        self.base_node_tooltip(
                            &mut tip,
                            &n.dn,
                            &n.sd,
                            &n.mods,
                            n.reminder_text.as_deref(),
                            None,
                        );
                    }
                }
            }
            "socketFilter" => lines(&[
                "^7Enable this option to exclude nodes that you do not have allocated on your active passive skill tree.",
                "^7This can be useful if you're never going to path towards those excluded nodes and don't care what happens to them.",
            ]),
            "socketAllocate" => lines(&[
                "^7Double clicking a result also equips the jewel in its jewel socket.",
                "^7The socket must be allocated on your current tree; if it isn't, the jewel is only added to your item list.",
                "^7A jewel already in that socket is replaced.",
            ]),
            "socketFilterAdditionalDistance" => lines(&[
                "^7This controls the maximum amount of points that need to be spent to grab a node before its filtered out",
            ]),
            "nodeSlider" | "nodeSlider2" => {
                let i = (name == "nodeSlider2") as usize;
                if p.stat_label[i] == "None" {
                    let which = if i == 0 { "first" } else { "second" };
                    let l = format!(
                        "^7For nodes with multiple stats this slider controls the weight of the {which} stat listed."
                    );
                    lines(&[&l]);
                } else {
                    let l = format!("^7        {}", p.stat_label[i]);
                    lines(&[
                        "^7This slider controls the weight of the following stat:",
                        &l,
                    ]);
                }
            }
            "nodeSlider3" => lines(&[
                "^7Seeds that do not meet the minimum weight threshold for a desired node are excluded from the search results.",
            ]),
            "nodeSelect" => {
                let Some(m) = idx.and_then(|i| p.mod_data.get(i - 1)) else {
                    return nil();
                };
                let l: Vec<String> = m.descriptions.iter().map(|d| format!("^7{d}")).collect();
                lines(&l.iter().map(String::as_str).collect::<Vec<_>>());
            }
            "fallbackWeightsButton" => lines(&[
                "^7Click this button to generate new fallback node weights, replacing your old ones.",
            ]),
            "totalMinimumWeight" => lines(&[
                "^7Optional: Only show results where total weight meets or exceeds this value.",
            ]),
            "searchListButton" => lines(&[
                "^7This contains a list of your desired nodes along with their primary, secondary, and minimum weights.",
                "^7This list can be updated manually or by selecting the node you want to update via the search dropdown list and then moving the node weight sliders.",
            ]),
            "searchListFallbackButton" => lines(&[
                "^7This contains a list of your fallback nodes along with their primary, secondary, and minimum weights.",
                "^7This list can be updated manually or by selecting the node you want to update via the search dropdown list and then moving the node weight sliders.",
                "^7Fallback node weights are only used when no matching entry exists in the desired nodes list, allowing you to override or disable specific automatic weights.",
                "^7Fallback node weights typically contain automatically generated stat weights based on your current build.",
                "^7Any manual changes made to your fallback nodes are lost when you click the generate button, as it completely replaces them.",
            ]),
            "searchTradeButton" => lines(&[
                "^7Click to generate and open a trade URL for searching for jewels in this list.",
                "^7Paste the URL in a web browser to search.",
                "",
                "^7You can click to select a row so that search begins from there.",
                "^7After selecting a row You can also shift+click on another row to select a range of rows to search.",
            ]),
            "abyssAscendancySelect" => match idx {
                None => return nil(),
                Some(i) if i > p.abyss.len() + 1 => return nil(),
                Some(1) => {}
                Some(i) => {
                    // The legion node (`value.node`)
                    let (_, id) = &self.timeless_abyss_options()?[i - 2];
                    let t = self.data.timeless().ok_or_else(|| {
                        format!("{NOT_PORTED}: timeless jewels without their data")
                    })?;
                    let l = t
                        .nodes
                        .iter()
                        .find(|l| &*l.node.id == id.as_str())
                        .ok_or("no legion node")?;
                    let n = &l.node;
                    self.base_node_tooltip(
                        &mut tip,
                        &n.dn,
                        &n.sd,
                        &n.mods,
                        n.reminder_text.as_deref(),
                        None,
                    );
                }
            },
            n if n.starts_with("protectAllocatedBox") => {
                // The tree's node (`nodeInfo.node`)
                let Some(b) = n["protectAllocatedBox".len()..]
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| p.protect.get(i.wrapping_sub(1)))
                else {
                    return Err(format!("{NOT_PORTED}: the tooltip of popup.{name}"));
                };
                let tree = self.build.spec.tree;
                let n = tree.node(b.id).ok_or("no protected node")?;
                self.base_node_tooltip(
                    &mut tip,
                    &n.dn,
                    &n.sd,
                    &n.mods,
                    n.reminder_text.as_deref(),
                    n.flavour_text.as_deref(),
                );
            }
            "searchResults" => {
                let rows = self.tree_tab.timeless.results.len();
                if let Some(i) = idx.filter(|i| *i <= rows) {
                    return self.timeless_row_tooltip(i);
                }
            }
            "jewelSelect"
            | "socketSelect"
            | "devotionSelect1"
            | "devotionSelect2"
            | "fallbackWeightsList"
            | "searchList"
            | "searchListFallback"
            | "searchMore"
            | "realmSelection"
            | "tradeTypeSelection"
            | "searchTradeLeagueSelect"
            | "resetButton"
            | "closeButton"
            | "searchButton" => {}
            _ => return Err(format!("{NOT_PORTED}: the tooltip of popup.{name}")),
        }
        Ok(tip.json())
    }
}
