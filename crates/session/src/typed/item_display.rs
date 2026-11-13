//! `displayItem` of `lua/session.lua`: the item editor's display item, its
//! tooltip (`ItemsTab:AddItemTooltip`) and its controls as `layoutControls`
//! lists them: upstream's `ItemsTab` controls of the editor (variants,
//! sockets, enchant and anoint buttons, influences, quality, catalysts,
//! cluster jewel, affixes, custom modifiers, range lines), placed by their
//! anchors as `Control:GetPos` does and sorted top to bottom, left to right.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{Result, TypedSession, num};
use engine::item::{INFLUENCES, Item};
use serde_json::{Map, Value as J, json};

/// `colorCodes` of the socket colours
const STRENGTH: &str = "^xE05030";
const DEXTERITY: &str = "^x70FF70";
const INTELLIGENCE: &str = "^x7070FF";
const SCION: &str = "^xFFF0F0";

/// The catalyst drop-down's entries
const CATALYSTS: [&str; 13] = [
    "Catalyst",
    "Abrasive (Attack)",
    "Accelerating (Speed)",
    "Dextral (Suffix)",
    "Fertile (Life & Mana)",
    "Imbued (Caster)",
    "Intrinsic (Attribute)",
    "Noxious (Physical & Chaos Damage)",
    "Prismatic (Resistance)",
    "Sinistral (Prefix)",
    "Tempering (Defense)",
    "Turbulent (Elemental)",
    "Unstable (Critical)",
];

/// `main.showAllItemAffixes`, true by default
const SHOW_ALL_ITEM_AFFIXES: bool = true;

const INFLUENCE_TIP: &str = "Selecting an influence here will also allow the modifier dropdowns to contain influenced mods.";

#[derive(Clone, Copy, PartialEq)]
enum Point {
    TopLeft,
    TopRight,
    Left,
    Right,
    BottomLeft,
}

impl Point {
    fn factors(self) -> (f64, f64) {
        match self {
            Point::TopLeft => (0.0, 0.0),
            Point::TopRight => (1.0, 0.0),
            Point::Left => (0.0, 0.5),
            Point::Right => (1.0, 0.5),
            Point::BottomLeft => (0.0, 1.0),
        }
    }
}

/// A control: its anchor, rect and `shown` property, and what `describeControl`
/// says of it when it is listed
struct Ctl {
    name: String,
    anchor: Option<(Point, usize, Point, bool)>,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    shown: bool,
    /// The description, for the controls `displayItem` lists
    desc: Option<Map<String, J>>,
}

struct Layout {
    ctls: Vec<Ctl>,
}

impl Layout {
    fn add(
        &mut self,
        name: &str,
        anchor: Option<(Point, usize, Point, bool)>,
        rect: [f64; 4],
    ) -> usize {
        self.ctls.push(Ctl {
            name: name.to_string(),
            anchor,
            x: rect[0],
            y: rect[1],
            w: rect[2],
            h: rect[3],
            shown: true,
            desc: None,
        });
        self.ctls.len() - 1
    }

    /// `Control:GetPos()`
    fn pos(&self, i: usize) -> (f64, f64) {
        let c = &self.ctls[i];
        let Some((point, other, other_point, collapse)) = c.anchor else {
            return (c.x, c.y);
        };
        if collapse && !self.ctls[other].shown {
            return self.pos(other);
        }
        let (ox, oy) = self.pos(other);
        let (ow, oh) = if other_point == Point::TopLeft {
            (0.0, 0.0)
        } else {
            (self.ctls[other].w, self.ctls[other].h)
        };
        let (w, h) = if point == Point::TopLeft {
            (0.0, 0.0)
        } else {
            (c.w, c.h)
        };
        let (opx, opy) = other_point.factors();
        let (px, py) = point.factors();
        (
            (ox + ow * opx + c.x - w * px).floor(),
            (oy + oh * opy + c.y - h * py).floor(),
        )
    }

    /// `Control:IsShown()`
    fn is_shown(&self, i: usize) -> bool {
        let c = &self.ctls[i];
        let anchor_shown = match c.anchor {
            None => true,
            Some((_, other, _, collapse)) => collapse || self.is_shown(other),
        };
        anchor_shown && c.shown
    }
}

fn base_desc(class: &str) -> Map<String, J> {
    let mut m = Map::new();
    m.insert("class".into(), json!(class));
    m.insert("enabled".into(), J::Bool(true));
    m
}

fn drop_desc(list: Vec<J>, sel: f64) -> Map<String, J> {
    let mut m = base_desc("DropDownControl");
    m.insert("list".into(), J::Array(list));
    // A nil `selIndex` is left out
    if !sel.is_nan() {
        m.insert("sel".into(), num(sel));
    }
    m
}

fn button_desc(label: &str) -> Map<String, J> {
    let mut m = base_desc("ButtonControl");
    m.insert("label".into(), json!(label));
    m
}

fn edit_desc(text: &str) -> Map<String, J> {
    let mut m = base_desc("EditControl");
    m.insert("text".into(), json!(text));
    m
}

fn check_desc(state: bool) -> Map<String, J> {
    let mut m = base_desc("CheckBoxControl");
    m.insert("state".into(), J::Bool(state));
    m
}

fn slider_desc(val: Option<f64>) -> Map<String, J> {
    let mut m = base_desc("SliderControl");
    if let Some(v) = val {
        m.insert("val".into(), num(v));
    }
    m
}

/// `DropDownControl:SetSel(sel, true)` from a first selection of 1: the
/// selection clamped to the list
fn clamp_sel(sel: f64, len: usize) -> f64 {
    sel.min(len as f64).max(1.0)
}

/// `isAnointable(item)`
fn anointable(item: &Item) -> bool {
    item.base.as_ref().is_some_and(|b| {
        !b.cannot_be_anointed
            && b.sub_type.as_deref() != Some("Talisman")
            && (item.can_be_anointed || b.typ == "Amulet")
    })
}

impl<'d> TypedSession<'d> {
    /// `item.enchantments`: the base's enchantment table, if it has one
    fn has_enchantments(&self, item: &Item) -> bool {
        let Some(base) = &item.base else { return false };
        base.flask.is_some()
            || ["Helmet", "Boots", "Gloves", "Belt", "Body Armour"].contains(&base.typ.as_str())
            || self
                .data
                .game
                .setup
                .weapon_type_info
                .contains_key(&base.typ)
    }

    /// `displayItem`: the display item, whether the build has it, its
    /// tooltip and the editor's controls
    pub fn display_item(&mut self) -> Result<J> {
        let Some(item) = self.display_item.clone() else {
            return Ok(json!([]));
        };
        self.update_sockets();
        let mut tip = super::tooltip::Tooltip::default();
        // A copy: upstream compares items by identity, so the item it was
        // opened from is replaced by it, not removed
        let mut copy = item.clone();
        copy.id = None;
        self.add_item_tooltip(&mut tip, &copy, None)?;
        let lines = tip.json()["lines"].clone();
        let existing = item
            .id
            .is_some_and(|id| self.build.items.item(id).is_some());
        let controls = self.editor_controls(&item)?;
        Ok(json!({
            "item": super::items::describe(&item),
            "existing": existing,
            "lines": lines,
            "controls": controls,
        }))
    }

    /// The editor's controls `layoutControls` lists for the display item
    fn editor_controls(&self, item: &Item) -> Result<J> {
        let mut l = Layout { ctls: Vec::new() };
        let tl = |o: usize| Some((Point::TopLeft, o, Point::TopLeft, false));
        let below = |o: usize| Some((Point::TopLeft, o, Point::BottomLeft, false));
        let base = item.base.clone().ok_or("no base")?;

        let root = l.add("anchorDisplayItem", None, [0.0, 0.0, 0.0, 0.0]);
        let add = l.add("addDisplayItem", tl(root), [0.0, 0.0, 100.0, 20.0]);

        // Variants
        let sv = l.add(
            "displayItemSectionVariant",
            below(add),
            [0.0, 8.0, 0.0, 0.0],
        );
        let version = l.add("displayItemVersion", tl(sv), [0.0, 0.0, 300.0, 20.0]);
        let vg = item.uses_variant_groups;
        l.ctls[version].shown = vg && item.version_list.as_ref().is_some_and(|v| v.len() > 1);
        if l.ctls[version].shown {
            let list = item.version_list.clone().unwrap_or_default();
            l.ctls[version].desc = Some(drop_desc(
                list.iter().map(|v| json!(v)).collect(),
                item.selected_version.unwrap_or(1.0),
            ));
        }
        // `UpdateDisplayItemVariantControls`: the groups' drop-downs
        let mut group_drops: Vec<(Vec<J>, f64, bool)> = Vec::new();
        if vg {
            for &group in item.variant_groups.keys() {
                if item.variant_group_options(group, false).is_empty() {
                    continue;
                }
                if group_drops.len() == 6 {
                    break;
                }
                let available = item.variant_group_options(group, true);
                let mut list = Vec::new();
                let mut sel = None;
                for v in &available {
                    let label = item
                        .variant_list
                        .as_ref()
                        .and_then(|l| l.get((*v as usize).wrapping_sub(1)))
                        .cloned()
                        .unwrap_or_default();
                    list.push(json!(label));
                    if item.variant_group_selections.get(&group) == Some(&(*v as f64)) {
                        sel = Some(list.len());
                    }
                }
                if list.is_empty() {
                    list.push(json!("No available variants"));
                }
                group_drops.push((list, sel.unwrap_or(1) as f64, available.len() > 1));
            }
        }
        let variant_list: Vec<J> = item
            .variant_list
            .iter()
            .flatten()
            .map(|v| json!(v))
            .collect();
        let alt_names = [
            "displayItemAltVariant",
            "displayItemAltVariant2",
            "displayItemAltVariant3",
            "displayItemAltVariant4",
            "displayItemAltVariant5",
        ];
        let mut prev = None;
        let mut variant_drops = Vec::new();
        for k in 0..6 {
            let (name, i) = if k == 0 {
                let i = l.add("displayItemVariant", tl(sv), [0.0, 0.0, 300.0, 20.0]);
                ("displayItemVariant", i)
            } else {
                let i = l.add(
                    alt_names[k - 1],
                    below(prev.unwrap()),
                    [0.0, 4.0, 300.0, 20.0],
                );
                (alt_names[k - 1], i)
            };
            let _ = name;
            prev = Some(i);
            let legacy = if k == 0 {
                item.variant_list.as_ref().is_some_and(|v| v.len() > 1)
            } else {
                item.has_alt_variant[k - 1]
            };
            let (shown, desc) = if vg {
                match group_drops.get(k) {
                    Some((list, sel, enabled)) => {
                        let mut d = drop_desc(list.clone(), *sel);
                        d.insert("enabled".into(), J::Bool(*enabled));
                        (true, Some(d))
                    }
                    None => (false, None),
                }
            } else {
                let sel = if k == 0 {
                    item.variant
                } else {
                    item.variant_alt[k - 1]
                };
                (
                    legacy,
                    Some(drop_desc(variant_list.clone(), sel.unwrap_or(f64::NAN))),
                )
            };
            l.ctls[i].shown = shown;
            if shown {
                l.ctls[i].desc = desc;
            }
            variant_drops.push(i);
        }
        if l.is_shown(version) {
            l.ctls[variant_drops[0]].y = 24.0;
        }
        l.ctls[sv].h = if vg {
            let rows = usize::from(l.ctls[version].shown) + group_drops.len();
            if rows > 0 {
                rows as f64 * 24.0 + 4.0
            } else {
                0.0
            }
        } else if !l.is_shown(variant_drops[0]) {
            0.0
        } else {
            28.0 + 24.0 * item.has_alt_variant.iter().filter(|a| **a).count() as f64
        };

        // Sockets and links
        let sockets = &item.sockets;
        let selectable = item.selectable_socket_count;
        let ss = l.add("displayItemSectionSockets", below(sv), [0.0, 0.0, 0.0, 0.0]);
        l.ctls[ss].h = if selectable > 0.0 { 28.0 } else { 0.0 };
        let colors = [
            ("R", STRENGTH),
            ("G", DEXTERITY),
            ("B", INTELLIGENCE),
            ("W", SCION),
        ];
        let socket_list: Vec<J> = colors
            .iter()
            .map(|(c, code)| json!(format!("{code}{c}")))
            .collect();
        for i in 1..=6usize {
            let drop = l.add(
                &format!("displayItemSocket{i}"),
                tl(ss),
                [(i - 1) as f64 * 64.0, 0.0, 36.0, 20.0],
            );
            let s = sockets.get(i - 1);
            l.ctls[drop].shown = selectable >= i as f64 && s.is_some_and(|s| s.color != 'A');
            if l.ctls[drop].shown {
                let color = s.unwrap().color;
                let sel = colors
                    .iter()
                    .position(|(c, _)| c.starts_with(color))
                    .map_or(1.0, |p| (p + 1) as f64);
                l.ctls[drop].desc = Some(drop_desc(socket_list.clone(), sel));
            }
            if i < 6 {
                let link = l.add(
                    &format!("displayItemLink{i}"),
                    Some((Point::Left, drop, Point::Right, false)),
                    [4.0, 0.0, 20.0, 20.0],
                );
                let next = sockets.get(i);
                l.ctls[link].shown = selectable > i as f64 && next.is_some_and(|s| s.color != 'A');
                if l.ctls[link].shown {
                    let linked = sockets
                        .get(i - 1)
                        .is_some_and(|s| s.group == next.unwrap().group);
                    l.ctls[link].desc = Some(check_desc(linked));
                }
            }
        }
        let socket_count = sockets.len() as f64 - item.abyssal_socket_count;
        let add_socket = l.add(
            "displayItemAddSocket",
            tl(ss),
            [socket_count * 64.0 - 12.0, 0.0, 20.0, 20.0],
        );
        l.ctls[add_socket].shown = (sockets.len() as f64) < selectable + item.abyssal_socket_count;
        l.ctls[add_socket].desc = Some(button_desc("+"));

        // Enchant, anoint, corrupt
        let se = l.add("displayItemSectionEnchant", below(ss), [0.0, 0.0, 0.0, 0.0]);
        let collapse_right = |o: usize| Some((Point::TopLeft, o, Point::TopRight, true));
        let enchant = l.add("displayItemEnchant", tl(se), [0.0, 0.0, 160.0, 20.0]);
        let enchants = item.enchant_mod_lines.len();
        let has_enchantments = self.has_enchantments(item);
        l.ctls[enchant].shown = has_enchantments;
        l.ctls[enchant].desc = Some(button_desc("Change Enchantment..."));
        let enchant2 = l.add(
            "displayItemEnchant2",
            collapse_right(enchant),
            [8.0, 0.0, 160.0, 20.0],
        );
        l.ctls[enchant2].shown = has_enchantments && item.can_have_two_enchants && enchants > 0;
        l.ctls[enchant2].desc = Some(button_desc("Change Enchantment 2..."));
        let anointable = anointable(item);
        let mut prev = enchant2;
        let mut anoints = Vec::new();
        for (k, (label, shown)) in [
            ("Anoint...", anointable),
            (
                "Anoint 2...",
                anointable && item.can_have_two_enchants && enchants > 0,
            ),
            (
                "Anoint 3...",
                anointable && item.can_have_three_enchants && enchants > 1,
            ),
            (
                "Anoint 4...",
                anointable && item.can_have_four_enchants && enchants > 2,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let name = if k == 0 {
                "displayItemAnoint".to_string()
            } else {
                format!("displayItemAnoint{}", k + 1)
            };
            let i = l.add(&name, collapse_right(prev), [8.0, 0.0, 100.0, 20.0]);
            l.ctls[i].shown = shown;
            l.ctls[i].desc = Some(button_desc(label));
            anoints.push(i);
            prev = i;
        }
        let corrupt = l.add(
            "displayItemCorrupt",
            collapse_right(prev),
            [8.0, 10.0, 100.0, 20.0],
        );
        l.ctls[corrupt].shown = item.corruptible;
        l.ctls[corrupt].desc = Some(button_desc("Corrupt..."));
        let add_implicit = l.add(
            "displayItemAddImplicit",
            collapse_right(corrupt),
            [8.0, 0.0, 120.0, 20.0],
        );
        let typ = item.typ.as_deref().unwrap_or("");
        l.ctls[add_implicit].shown = typ != "Tincture"
            && typ != "Graft"
            && (item.corruptible
                || (typ != "Flask"
                    && typ != "Jewel"
                    && ["NORMAL", "MAGIC", "RARE"].contains(&item.rarity.as_str())))
            && !item.implicits_cannot_be_changed;
        l.ctls[add_implicit].desc = Some(button_desc("Add Implicit..."));
        l.ctls[se].h = if [enchant, enchant2, anoints[0], anoints[1], corrupt]
            .iter()
            .any(|&i| l.is_shown(i))
        {
            28.0
        } else {
            0.0
        };

        // Influences
        let si = l.add(
            "displayItemSectionInfluence",
            below(se),
            [0.0, 0.0, 0.0, 0.0],
        );
        l.ctls[si].h = if item.can_be_influenced { 28.0 } else { 0.0 };
        let eldritch = item.can_have_eldritch_influence
            || ["Helmet", "Body Armour", "Gloves", "Boots"].contains(&typ);
        let count = if eldritch { INFLUENCES.len() } else { 6 };
        let mut influence_list = vec![json!("Influence")];
        influence_list.extend(INFLUENCES[..count].iter().map(|(_, d)| json!(d)));
        let (mut infl1, mut infl2) = (1.0, 1.0);
        for (i, on) in item.influences.iter().enumerate() {
            if *on {
                if infl1 == 1.0 {
                    infl1 = (i + 2) as f64;
                } else if infl2 == 1.0 {
                    infl2 = (i + 2) as f64;
                    break;
                }
            }
        }
        let infl_a = l.add(
            "displayItemInfluence",
            Some((Point::TopLeft, si, Point::TopRight, false)),
            [0.0, 0.0, 100.0, 20.0],
        );
        let infl_b = l.add(
            "displayItemInfluence2",
            collapse_right(infl_a),
            [8.0, 0.0, 100.0, 20.0],
        );
        for (i, sel) in [(infl_a, infl1), (infl_b, infl2)] {
            l.ctls[i].shown = item.can_be_influenced;
            let mut d = drop_desc(influence_list.clone(), clamp_sel(sel, influence_list.len()));
            d.insert("tooltip".into(), json!(INFLUENCE_TIP));
            l.ctls[i].desc = Some(d);
        }

        // Quality
        let sq = l.add("displayItemSectionQuality", below(si), [0.0, 0.0, 0.0, 0.0]);
        let quality = l.add(
            "displayItemQuality",
            Some((Point::TopLeft, sq, Point::TopRight, false)),
            [0.0, 0.0, 0.0, 16.0],
        );
        l.ctls[quality].shown = item.quality.is_some();
        let mut qd = base_desc("LabelControl");
        qd.insert("label".into(), json!("^7Quality:"));
        l.ctls[quality].desc = Some(qd);
        let quality_edit = l.add(
            "displayItemQualityEdit",
            Some((Point::Left, quality, Point::Right, false)),
            [2.0, 0.0, 60.0, 20.0],
        );
        l.ctls[quality_edit].shown = item.quality.is_some();
        l.ctls[quality_edit].desc = Some(edit_desc(
            &item
                .quality
                .map(lua::value::format_number)
                .unwrap_or_default(),
        ));
        l.ctls[sq].h = if l.is_shown(quality) && l.is_shown(quality_edit) {
            28.0
        } else {
            0.0
        };

        // Catalysts
        let sc = l.add(
            "displayItemSectionCatalyst",
            below(sq),
            [0.0, 0.0, 0.0, 0.0],
        );
        let catalyst = l.add(
            "displayItemCatalyst",
            Some((Point::TopLeft, sc, Point::TopRight, false)),
            [0.0, 0.0, 250.0, 20.0],
        );
        let jewellery = ["Amulet", "Ring", "Belt"].contains(&base.typ.as_str());
        l.ctls[catalyst].shown = (item.crafted || item.has_mod_tags) && jewellery;
        let cat = item.catalyst.unwrap_or(0) as f64;
        l.ctls[catalyst].desc = Some(drop_desc(
            CATALYSTS.iter().map(|c| json!(c)).collect(),
            clamp_sel(cat + 1.0, CATALYSTS.len()),
        ));
        let cat_quality = l.add(
            "displayItemCatalystQualityEdit",
            Some((Point::Left, catalyst, Point::Right, false)),
            [2.0, 0.0, 60.0, 20.0],
        );
        l.ctls[cat_quality].shown = (item.crafted || item.has_mod_tags) && cat > 0.0;
        let cq = item.catalyst_quality.map_or(0.0, |q| q.max(0.0));
        l.ctls[cat_quality].desc = Some(edit_desc(&lua::value::format_number(cq)));
        l.ctls[sc].h = if l.is_shown(catalyst) || l.is_shown(cat_quality) {
            28.0
        } else {
            0.0
        };

        // Cluster jewel
        let data = &self.data.game.items;
        let cluster = item
            .base_name
            .as_ref()
            .and_then(|b| data.cluster_jewels.get(b));
        let scj = l.add(
            "displayItemSectionClusterJewel",
            below(sc),
            [0.0, 0.0, 0.0, 0.0],
        );
        let cj_skill = l.add(
            "displayItemClusterJewelSkill",
            tl(scj),
            [0.0, 0.0, 300.0, 20.0],
        );
        l.ctls[cj_skill].shown = item.crafted && cluster.is_some();
        let cj_label = l.add(
            "displayItemClusterJewelNodeCountLabel",
            below(cj_skill),
            [0.0, 7.0, 0.0, 14.0],
        );
        let cj_count = l.add(
            "displayItemClusterJewelNodeCount",
            Some((Point::Left, cj_label, Point::Right, false)),
            [2.0, 0.0, 150.0, 20.0],
        );
        if let (Some(cj), true) = (cluster, l.ctls[cj_skill].shown) {
            let unavailable = [
                "affliction_strength",
                "affliction_dexterity",
                "affliction_intelligence",
            ];
            let mut skills: Vec<(&String, &String)> = cj
                .skill_data
                .iter()
                .filter(|(id, _)| !unavailable.contains(&id.as_str()))
                .map(|(id, s)| (&s.name, id))
                .collect();
            skills.sort();
            let sel = skills
                .iter()
                .position(|(_, id)| item.cluster_jewel_skill.as_ref() == Some(*id))
                .map_or(1.0, |p| (p + 1) as f64);
            l.ctls[cj_skill].desc = Some(drop_desc(
                skills.iter().map(|(n, _)| json!(n)).collect(),
                sel,
            ));
            let count = item.cluster_jewel_node_count.unwrap_or(cj.min_nodes);
            let val = (count - cj.min_nodes) / (cj.max_nodes - cj.min_nodes);
            let mut d = slider_desc(Some(val));
            d.insert("label".into(), json!("^7Added Passives:"));
            l.ctls[cj_count].desc = Some(d);
        }
        l.ctls[scj].h = if l.is_shown(cj_skill) { 52.0 } else { 0.0 };

        // Affixes
        let sort_label = l.add("craftingSortingLabel", below(scj), [0.0, 0.0, 0.0, 16.0]);
        l.ctls[sort_label].shown =
            item.crafted && !(base.typ == "Jewel" && base.sub_type.as_deref() == Some("Cluster"));
        let sa = l.add(
            "displayItemSectionAffix",
            Some((Point::TopLeft, sort_label, Point::BottomLeft, true)),
            [
                0.0,
                if l.ctls[sort_label].shown { 8.0 } else { -16.0 },
                0.0,
                0.0,
            ],
        );
        l.ctls[sa].shown = item.crafted;
        let ignore_mod_type = item
            .rare_like_unique
            .as_ref()
            .and_then(|r| data.rare_like_uniques.get(r))
            .is_some_and(|r| r.ignore_mod_type);
        let mut prev = sa;
        let mut prev_slider: Option<usize> = None;
        let mut affix_h = 6.0;
        for i in 1..=9usize {
            let c = &self.editor.affix[i - 1];
            let y = if i == 1 {
                0.0
            } else if prev_slider.is_some_and(|s| l.is_shown(s)) {
                42.0
            } else {
                24.0
            };
            let drop = l.add(
                &format!("displayItemAffix{i}"),
                tl(prev),
                [if i == 1 { 40.0 } else { 0.0 }, y, 418.0, 20.0],
            );
            l.ctls[drop].shown = item.crafted && i as f64 <= item.affix_limit;
            let list: Vec<J> = c
                .list
                .iter()
                .map(|e| json!(e.as_ref().map_or("None", |s| s.label.as_str())))
                .collect();
            let mut d = drop_desc(list, (c.sel + 1) as f64);
            let label = if ignore_mod_type {
                "^7Explicit:"
            } else if c.table == engine::item_craft::AffixTable::Prefixes {
                "^7Prefix:"
            } else {
                "^7Suffix:"
            };
            d.insert("label".into(), json!(label));
            l.ctls[drop].desc = Some(d);
            let slider = l.add(
                &format!("displayItemAffixRange{i}"),
                below(drop),
                [
                    0.0,
                    2.0,
                    if c.slider_div.is_some() { 300.0 } else { 100.0 },
                    16.0,
                ],
            );
            l.ctls[slider].shown = c.slider_shown;
            let mut sd = slider_desc(Some(c.slider_val));
            sd.insert(
                "label".into(),
                json!(if c.sel > 0 {
                    "^7Roll:"
                } else {
                    "^x7F7F7FRoll:"
                }),
            );
            l.ctls[slider].desc = Some(sd);
            if l.is_shown(drop) {
                affix_h += 24.0;
                if l.is_shown(slider) {
                    affix_h += 18.0;
                }
            }
            prev = drop;
            prev_slider = Some(slider);
        }
        l.ctls[sa].h = if item.crafted { affix_h } else { 0.0 };

        // Custom modifiers
        let scu = l.add(
            "displayItemSectionCustom",
            Some((Point::TopLeft, sa, Point::BottomLeft, true)),
            [0.0, 0.0, 0.0, 0.0],
        );
        let add_custom = l.add("displayItemAddCustom", tl(scu), [0.0, 0.0, 120.0, 20.0]);
        let rare_like = item
            .rare_like_unique
            .as_ref()
            .and_then(|r| data.rare_like_uniques.get(r));
        l.ctls[add_custom].shown = item.rarity == "MAGIC"
            || item.rarity == "RARE"
            || rare_like.is_some_and(|r| r.supports_custom_modifiers);
        l.ctls[add_custom].desc = Some(button_desc("Add modifier..."));
        let add_crucible = l.add(
            "displayItemAddCrucible",
            tl(scu),
            [
                if l.ctls[add_custom].shown { 128.0 } else { 0.0 },
                0.0,
                150.0,
                20.0,
            ],
        );
        l.ctls[add_crucible].shown = item.primary_slot() == "Weapon 1"
            || typ == "Shield"
            || item.can_have_shield_crucible_tree;
        l.ctls[add_crucible].desc = Some(button_desc("Add Crucible mod..."));
        // `UpdateCustomControls`: a Remove button for each crafted, custom
        // or crucible line shown
        let mut custom = 0;
        if item.rare_like_unique.is_some()
            || item.rarity == "MAGIC"
            || item.rarity == "RARE"
            || !item.crucible_mod_lines.is_empty()
        {
            for ml in item
                .explicit_mod_lines
                .iter()
                .chain(&item.crucible_mod_lines)
            {
                if (ml.flags.custom || ml.flags.crafted || ml.flags.crucible)
                    && self.format_mod_line(ml)?.is_some()
                {
                    custom += 1;
                    let b = l.add(
                        &format!("displayItemCustomModifierRemove{custom}"),
                        tl(scu),
                        [0.0, custom as f64 * 22.0 + 4.0, 70.0, 20.0],
                    );
                    l.ctls[b].desc = Some(button_desc("^7Remove"));
                }
            }
        }

        l.ctls[scu].h = if l.is_shown(add_custom) || l.is_shown(add_crucible) {
            28.0 + custom as f64 * 22.0
        } else {
            0.0
        };

        // Range lines
        let sr = l.add("displayItemSectionRange", below(scu), [0.0, 0.0, 0.0, 0.0]);
        let range_lines = self.range_line_list(item);
        let unique = item.rarity == "UNIQUE";
        let range_line = l.add("displayItemRangeLine", tl(sr), [0.0, 0.0, 350.0, 18.0]);
        l.ctls[range_line].shown = !range_lines.is_empty() && !(SHOW_ALL_ITEM_AFFIXES && unique);
        let listed: Vec<J> = range_lines
            .iter()
            .filter(|ml| (ml.mod_id.is_some() && ml.new_mod_id.is_some()) || ml.range.is_some())
            .map(|ml| json!(ml.line))
            .collect();
        l.ctls[range_line].desc = Some(drop_desc(listed, 1.0));
        let mutated = l.add(
            "displayItemMutatedCheckbox",
            Some((Point::Left, range_line, Point::Right, true)),
            [8.0, 0.0, 18.0, 18.0],
        );
        l.ctls[mutated].shown = false;
        let range_slider = l.add(
            "displayItemRangeSlider",
            Some((Point::Left, mutated, Point::Right, true)),
            [4.0, 0.0, 100.0, 18.0],
        );
        let first = range_lines.first();
        l.ctls[range_slider].shown =
            !(SHOW_ALL_ITEM_AFFIXES && unique) && first.is_some_and(|ml| ml.show_slider);
        l.ctls[range_slider].desc = Some(slider_desc(first.and_then(|ml| ml.range)));
        let mut box_y = 2.0;
        for i in 1..=20usize {
            let line = range_lines.get(i - 1);
            let b = l.add(
                &format!("displayItemStackedMutatedCheckbox{i}"),
                Some((Point::TopLeft, sr, Point::TopLeft, true)),
                [0.0, box_y, 18.0, 18.0],
            );
            l.ctls[b].shown = SHOW_ALL_ITEM_AFFIXES
                && line.is_some_and(|ml| ml.mod_id.is_some() && ml.new_mod_id.is_some());
            if l.ctls[b].shown {
                l.ctls[b].x = 24.0;
            }
            l.ctls[b].desc = Some(check_desc(line.is_some_and(|ml| ml.flags.mutated)));
            let s = l.add(
                &format!("displayItemStackedRangeSlider{i}"),
                Some((Point::Left, b, Point::Right, true)),
                [4.0, 0.0, 100.0, 18.0],
            );
            l.ctls[s].shown =
                SHOW_ALL_ITEM_AFFIXES && unique && line.is_some_and(|ml| ml.show_slider);
            l.ctls[s].desc = Some(slider_desc(line.and_then(|ml| ml.range)));
            box_y += 22.0;
        }

        // `layoutControls`: the listed controls shown, by position
        let mut out: Vec<(f64, f64, J)> = Vec::new();
        for (i, c) in l.ctls.iter().enumerate() {
            let Some(d) = &c.desc else { continue };
            if !c.name.starts_with("displayItem") || !l.is_shown(i) {
                continue;
            }
            let (x, y) = l.pos(i);
            let mut d = d.clone();
            d.insert("shown".into(), J::Bool(true));
            d.insert("path".into(), json!(format!("items.{}", c.name)));
            out.push((y, x, J::Object(d)));
        }
        out.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap()
                .then(a.1.partial_cmp(&b.1).unwrap())
        });
        Ok(J::Array(out.into_iter().map(|(_, _, d)| d).collect()))
    }

    /// `item.rangeLineList`: the lines `BuildModList` gave a range slider
    /// or a Foulborn transformation, in its order
    fn range_line_list<'a>(&self, item: &'a Item) -> Vec<&'a engine::item::ModLine> {
        item.enchant_mod_lines
            .iter()
            .chain(&item.scourge_mod_lines)
            .chain(&item.class_requirement_mod_lines)
            .chain(&item.implicit_mod_lines)
            .chain(&item.explicit_mod_lines)
            .chain(&item.crucible_mod_lines)
            .filter(|ml| {
                !ml.flags.disabled
                    && item.mod_line_variant_count(ml) > 0
                    && ((ml.show_slider && ml.extra.is_none())
                        || (ml.mod_id.is_some() && ml.new_mod_id.is_some()))
            })
            .collect()
    }
}
