//! The Items tab's item tooltip (`ItemsTab:AddItemTooltip` with
//! `AddItemStatDifferences`): `itemTooltip` and `previewItem` of
//! `lua/session.lua`, with the stat comparisons of equipping the item in
//! each slot it fits, or of toggling a flask or tincture.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::tooltip::{Outputs, Tooltip, cc, color, n};
use super::{NOT_PORTED, Result, TypedSession};
use engine::calc_override::Override;
use engine::calculator::{CalcOutput, MiscCalculator};
use engine::item::{Fields, Item, ModLine};
use engine::items_tab::Slot;
use engine::mods::ModValue;
use engine::sf;
use engine::sidebar::item_query_key;
use serde_json::Value as J;

const BIG: f64 = 18.0;
const TITLE: f64 = 22.0;
const GREY: &str = "^x7F7F7F";
const NOT_SUPPORTED: &str = " ^8(Not supported in PoB yet)";

/// `catalystQualityFormat` of `Classes/ItemsTab.lua`
const CATALYST: [&str; 12] = [
    "Attack",
    "Speed",
    "Suffix",
    "Life and Mana",
    "Caster",
    "Attribute",
    "Physical and Chaos",
    "Resistance",
    "Prefix",
    "Defence",
    "Elemental",
    "Critical",
];

/// `main:StatColor(stat, base, limit)`
fn stat_color(stat: f64, base: Option<f64>, limit: Option<f64>) -> &'static str {
    if limit.is_some_and(|l| stat > l) {
        cc("NEGATIVE")
    } else if base.is_some_and(|b| stat != b) {
        cc("MAGIC")
    } else {
        "^7"
    }
}

fn field(f: &Fields, k: &str) -> Option<f64> {
    f.get(k).and_then(ModValue::as_number)
}

fn unported(what: &str) -> String {
    format!("{NOT_PORTED}: {what}")
}

/// A field upstream formats as a number, which must be there
fn need(f: &Fields, k: &str) -> Result<f64> {
    field(f, k).ok_or_else(|| unported(&format!("an item without {k}")))
}

/// `item.baseName:gsub(" %(.+%)", "")`
fn short_base(b: &str) -> String {
    engine::pat::gsub(b, " %(.+%)", "")
}

/// `itemChangesPassiveTree(item)`
fn changes_tree(item: &Item) -> bool {
    let Some(jd) = item.jewel_data.as_ref() else {
        return false;
    };
    let t = |k: &str| jd.get(k).is_some_and(|v| v.truthy());
    item.typ.as_deref() == Some("Jewel")
        && (t("conqueredBy")
            || (item.jewel_radius_index.is_some()
                && (t("intuitiveLeapLike") || t("impossibleEscapeKeystone"))))
}

/// A slot to compare in, with its calculation
struct Compare<'a> {
    slot: &'a Slot,
    label: String,
    sel: Option<&'a Item>,
    out: CalcOutput,
    same_unique: bool,
}

impl<'d> TypedSession<'d> {
    /// `showStatDifferences` of the Items tab, as `ItemsTab:Load` read it
    fn items_stat_differences(&self) -> bool {
        self.root
            .child("PathOfBuilding")
            .and_then(|p| p.child("Items"))
            .and_then(|t| t.attr("showStatDifferences"))
            .is_none_or(|v| v == "true")
    }

    /// `ItemsTab:UpdateSockets`: the labels of the allocated sockets
    pub(super) fn update_sockets(&mut self) {
        let tab = &self.build.items;
        let spec = &self.build.spec;
        let mut active: Vec<u32> = tab
            .ordered_slots
            .iter()
            .filter_map(|s| s.node_id)
            .filter(|n| spec.alloc_nodes.contains(*n))
            .collect();
        active.sort_unstable();
        for (i, n) in active.iter().enumerate() {
            self.socket_labels.insert(*n, format!("Socket #{}", i + 1));
        }
    }

    /// A slot's `label`
    pub(super) fn slot_label(&self, s: &Slot) -> String {
        match s.node_id {
            Some(n) => self
                .socket_labels
                .get(&n)
                .cloned()
                .unwrap_or_else(|| "Socket".into()),
            None => s.label(),
        }
    }

    /// The jewel socket slot of a node
    pub(super) fn socket_slot(&self, id: u32) -> Option<&Slot> {
        let tab = &self.build.items;
        tab.ordered_slots.iter().find(|s| s.node_id == Some(id))
    }

    /// `slot:IsEnabled()`: more than the empty choice fits it
    pub(super) fn socket_enabled(&self, id: u32) -> bool {
        let tab = &self.build.items;
        self.socket_slot(id).is_some_and(|s| {
            self.item_keys
                .iter()
                .filter_map(|(i, _)| tab.item(i as f64))
                .any(|i| self.valid_for_slot(i, &s.slot_name))
        })
    }

    /// `itemTooltip`: an item of the Items tab, compared in the named
    /// slot or in every slot it fits
    pub fn item_tooltip(&mut self, id: f64, slot: Option<&str>) -> Result<J> {
        let item = self
            .build
            .items
            .item(id)
            .cloned()
            .ok_or_else(|| format!("no item {}", n(id)))?;
        self.update_sockets();
        let tab = &self.build.items;
        let slot = slot.and_then(|s| tab.ordered_slots.iter().find(|x| x.slot_name == s));
        let mut tip = Tooltip::default();
        self.add_item_tooltip(&mut tip, &item, slot)?;
        Ok(tip.json())
    }

    /// `ItemsTab:AddItemTooltip(tooltip, item, slot)`; sockets' labels as
    /// `UpdateSockets` last set them
    pub(super) fn add_item_tooltip(
        &self,
        tip: &mut Tooltip,
        item: &Item,
        slot: Option<&Slot>,
    ) -> Result<()> {
        let rarity_code = color(&item.rarity).ok_or_else(|| unported("an item rarity"))?;
        tip.header = true;
        let (Some(base), Some(base_name)) = (&item.base, &item.base_name) else {
            let name = item.title.clone().unwrap_or_else(|| item.name.clone());
            tip.add_line(TITLE, &format!("{rarity_code}{name}"));
            tip.add_separator(30.0);
            tip.add_line(
                TITLE,
                &format!(
                    "{}Item base is not supported by the current version.",
                    cc("NEGATIVE")
                ),
            );
            return Ok(());
        };
        match &item.title {
            Some(t) => {
                tip.add_line(TITLE, &format!("{rarity_code}{t}"));
                tip.add_line(TITLE, &format!("{rarity_code}{}", short_base(base_name)));
            }
            None => {
                let name = format!(
                    "{}{}{}",
                    item.name_prefix,
                    short_base(base_name),
                    item.name_suffix
                );
                tip.add_line(TITLE, &format!("{rarity_code}{name}"));
            }
        }
        tip.add_separator(10.0);
        let slot_num = slot.map_or(Some(1.0), |s| s.slot_num);
        let quality = item.quality.unwrap_or(0.0);
        let magic = cc("MAGIC");
        let quality_line = |tip: &mut Tooltip| {
            if quality > 0.0 {
                tip.add_line(BIG, &sf!(&format!("{GREY}Quality: {magic}+%d%%"), quality));
            }
        };
        if let Some(bw) = &base.weapon {
            let wd = slot_num
                .and_then(|n| item.weapon_data.as_ref()?.get(&(n as usize)))
                .ok_or_else(|| unported("the weapon data of a slot"))?;
            let info = self.data.game.setup.weapon_type_info.get(&base.typ);
            let label = info
                .and_then(|i| i.label.clone())
                .or(base.sub_type.clone())
                .unwrap_or(base.typ.clone());
            tip.add_line(BIG, &format!("{GREY}{label}"));
            quality_line(tip);
            let mut types = 0;
            if field(wd, "PhysicalDPS").is_some() {
                let line = sf!(
                    &format!("{GREY}Physical Damage: {magic}%d-%d (%.1f DPS)"),
                    need(wd, "PhysicalMin")?,
                    need(wd, "PhysicalMax")?,
                    need(wd, "PhysicalDPS")?
                );
                tip.add_line(BIG, &line);
                types += 1;
            }
            if field(wd, "ElementalDPS").is_some() {
                let mut line: Option<String> = None;
                for v in ["Fire", "Cold", "Lightning"] {
                    if field(wd, &format!("{v}DPS")).is_some() {
                        let mut l = match line {
                            Some(l) => format!("{l}{GREY}, "),
                            None => format!("{GREY}Elemental Damage: "),
                        };
                        l.push_str(&sf!(
                            "%s%d-%d",
                            cc(&v.to_uppercase()),
                            need(wd, &format!("{v}Min"))?,
                            need(wd, &format!("{v}Max"))?
                        ));
                        line = Some(l);
                    }
                }
                let line = line.ok_or_else(|| unported("elemental DPS without a type"))?;
                tip.add_line(BIG, &line);
                let line = sf!(
                    &format!("{GREY}Elemental DPS: {magic}%.1f"),
                    need(wd, "ElementalDPS")?
                );
                tip.add_line(BIG, &line);
                types += 1;
            }
            if field(wd, "ChaosDPS").is_some() {
                let line = sf!(
                    &format!("{GREY}Chaos Damage: {}%d-%d {magic}(%.1f DPS)", cc("CHAOS")),
                    need(wd, "ChaosMin")?,
                    need(wd, "ChaosMax")?,
                    need(wd, "ChaosDPS")?
                );
                tip.add_line(BIG, &line);
                types += 1;
            }
            if types > 1 {
                let line = sf!(
                    &format!("{GREY}Total DPS: {magic}%.1f"),
                    need(wd, "TotalDPS")?
                );
                tip.add_line(BIG, &line);
            }
            let crit = need(wd, "CritChance")?;
            let c = stat_color(crit, bw.get("CritChanceBase").copied(), None);
            tip.add_line(
                BIG,
                &sf!(&format!("{GREY}Critical Strike Chance: %s%.2f%%"), c, crit),
            );
            let rate = need(wd, "AttackRate")?;
            let c = stat_color(rate, bw.get("AttackRateBase").copied(), None);
            tip.add_line(
                BIG,
                &sf!(&format!("{GREY}Attacks per Second: %s%.2f"), c, rate),
            );
            let range = need(wd, "range")?;
            if range < 120.0 {
                let c = stat_color(range, bw.get("Range").copied(), None);
                tip.add_line(
                    BIG,
                    &sf!(
                        &format!("{GREY}Weapon Range: %s%.1f {GREY}metres"),
                        c,
                        range / 10.0
                    ),
                );
            }
        } else if let Some(ba) = &base.armour {
            let ad = item
                .armour_data
                .as_ref()
                .ok_or_else(|| unported("armour without armour data"))?;
            quality_line(tip);
            if let Some(b) = ba.get("BlockChance").copied() {
                let v = need(ad, "BlockChance")?;
                if v > 0.0 {
                    let line = sf!(
                        &format!("{GREY}Chance to Block: %s%d%%"),
                        stat_color(v, Some(b), None),
                        v
                    );
                    tip.add_line(BIG, &line);
                }
            }
            for (k, label) in [
                ("Armour", "Armour"),
                ("Evasion", "Evasion Rating"),
                ("EnergyShield", "Energy Shield"),
                ("Ward", "Ward"),
            ] {
                let v = need(ad, k)?;
                if v > 0.0 {
                    let b = field(ad, &format!("{k}Base"));
                    let line = sf!(&format!("{GREY}{label}: %s%d"), stat_color(v, b, None), v);
                    tip.add_line(BIG, &line);
                }
            }
        } else if let Some(bf) = &base.flask {
            let fd = item
                .flask_data
                .as_ref()
                .ok_or_else(|| unported("a flask without flask data"))?;
            quality_line(tip);
            let dur = need(fd, "duration")?;
            for (pool, total_k, base_v) in [
                ("Life", "lifeTotal", bf.life),
                ("Mana", "manaTotal", bf.mana),
            ] {
                let Some(total) = field(fd, total_k) else {
                    continue;
                };
                let lower = pool.to_lowercase();
                let gradual = need(fd, &format!("{lower}Gradual"))?;
                if gradual != 0.0 {
                    let line = sf!(
                        &format!("{GREY}Recovers %s%d {GREY}{pool} over %s%.1f0 {GREY}Seconds"),
                        stat_color(total, base_v, None),
                        gradual,
                        stat_color(dur, Some(bf.duration), None),
                        dur
                    );
                    tip.add_line(BIG, &line);
                }
                let instant = need(fd, &format!("{lower}Instant"))?;
                if instant != 0.0 {
                    let line = sf!(
                        &format!("{GREY}Recovers %s%d {GREY}{pool} instantly"),
                        stat_color(total, base_v, None),
                        instant
                    );
                    tip.add_line(BIG, &line);
                }
            }
            if field(fd, "lifeTotal").is_none() && field(fd, "manaTotal").is_none() {
                let line = sf!(
                    &format!("{GREY}Lasts %s%.2f {GREY}Seconds"),
                    stat_color(dur, Some(bf.duration), None),
                    dur
                );
                tip.add_line(BIG, &line);
            }
            let (used, max) = (need(fd, "chargesUsed")?, need(fd, "chargesMax")?);
            let line = sf!(
                &format!("{GREY}Consumes %s%d {GREY}of %s%d {GREY}Charges on use"),
                stat_color(used, Some(bf.charges_used), None),
                used,
                stat_color(max, Some(bf.charges_max), None),
                max
            );
            tip.add_line(BIG, &line);
            self.add_buff_lines(tip, item);
        } else if let Some(bt) = &base.tincture {
            let td = item
                .tincture_data
                .as_ref()
                .ok_or_else(|| unported("a tincture without tincture data"))?;
            quality_line(tip);
            let burn = need(td, "manaBurn")?;
            let line = sf!(
                &format!("{GREY}Inflicts Mana Burn every %s%.2f {GREY}Seconds"),
                stat_color(burn, Some(bt.mana_burn), None),
                burn
            );
            tip.add_line(BIG, &line);
            let cd = need(td, "cooldown")?;
            let line = sf!(
                &format!("{GREY}%s%.2f {GREY}Second Cooldown When Deactivated"),
                stat_color(cd, Some(bt.cooldown), None),
                cd
            );
            tip.add_line(BIG, &line);
            self.add_buff_lines(tip, item);
        } else if item.typ.as_deref() == Some("Jewel") {
            if let Some(l) = item.limit {
                tip.add_line(BIG, &format!("{GREY}Limited to: ^7{}", n(l)));
            }
            if let Some(c) = &item.class_restriction {
                let spec = &self.build.spec;
                let class = spec
                    .cur_class_id
                    .and_then(|id| spec.tree.classes.iter().find(|(i, _)| *i == id))
                    .map(|(_, c)| &*c.name);
                let col = cc(if class == Some(c.as_str()) {
                    "POSITIVE"
                } else {
                    "NEGATIVE"
                });
                tip.add_line(BIG, &format!("{GREY}Requires Class {col}{c}"));
            }
            if let Some(l) = &item.jewel_radius_label {
                tip.add_line(BIG, &format!("{GREY}Radius: ^7{l}"));
            }
            let radius = slot.and_then(|s| {
                let node = s.node_id?;
                let id = item.id?;
                let sb = self.sidebar().ok()?;
                sb.jewel_radius_data
                    .iter()
                    .find(|(i, n, _)| *i == id && *n == node)
                    .map(|(_, _, a)| *a)
            });
            if let Some(a) = radius {
                let mut line: Option<String> = None;
                for (k, (code, stat)) in [
                    ("STRENGTH", "Str"),
                    ("DEXTERITY", "Dex"),
                    ("INTELLIGENCE", "Int"),
                ]
                .iter()
                .enumerate()
                {
                    if let Some(v) = a[k].filter(|v| *v != 0.0) {
                        let part = sf!("%s%d %s^7", cc(code), v, *stat);
                        line = Some(match line {
                            Some(l) => format!("{l}, {part}"),
                            None => part,
                        });
                    }
                }
                if line.is_some() {
                    // Upstream keeps the table of the calculation that last
                    // set it on the item, which the typed one need not match
                    return Err(unported("a jewel's attributes in radius"));
                }
            }
        }
        if let (Some(c), Some(q)) = (item.catalyst, item.catalyst_quality)
            && c > 0
            && c <= CATALYST.len()
            && q > 0.0
        {
            let f = format!(
                "{GREY}Quality ({} Modifiers): {magic}+%d%% (augmented)",
                CATALYST[c - 1]
            );
            tip.add_line(BIG, &sf!(&f, q));
            tip.add_separator(10.0);
        }
        if !item.sockets.is_empty() {
            tip.add_line(BIG, &format!("{GREY}Sockets: {}", socket_line(item)));
        }
        if let Some(t) = item.talisman_tier {
            tip.add_line(BIG, &format!("{GREY}Talisman Tier ^xFFFFFF{}", n(t)));
            tip.add_separator(10.0);
        }
        if let Some(m) = item.memory_strands {
            tip.add_line(5.0, "");
            let line = sf!(&format!("{}Memory Strands: ^7%d", cc("MEMORY")), m);
            tip.add_line(BIG, &line);
            tip.add_line(5.0, "");
        }
        if let Some(i) = item.intangibility {
            tip.add_line(5.0, "");
            let line = sf!(&format!("{}Intangibility: ^7%d%%", cc("INTANGIBILITY")), i);
            tip.add_line(BIG, &line);
            tip.add_line(5.0, "");
        }
        tip.add_separator(10.0);
        self.add_requirements(tip, item)?;
        for list in [
            &item.enchant_mod_lines,
            &item.scourge_mod_lines,
            &item.implicit_mod_lines,
            &item.explicit_mod_lines,
            &item.crucible_mod_lines,
        ] {
            if list.is_empty() {
                continue;
            }
            for ml in list {
                let count = item.mod_line_variant_count(ml);
                if count > 0
                    && let Some(line) = self.format_mod_line(ml)?
                {
                    for _ in 0..count {
                        tip.add_line(BIG, &line);
                    }
                }
            }
            tip.add_separator(10.0);
        }
        if base_name_is_cluster(self, item) {
            self.add_cluster_lines(tip, item)?;
        }
        if item.corrupted || item.split || item.mirrored {
            if item.explicit_mod_lines.is_empty() {
                tip.add_separator(10.0);
            }
            let neg = cc("NEGATIVE");
            if item.split {
                tip.add_line(BIG, &format!("{neg}Split"));
            }
            if item.mirrored {
                tip.add_line(BIG, &format!("{neg}Mirrored"));
            }
            if item.corrupted {
                tip.add_line(BIG, &format!("{neg}Corrupted"));
            }
            tip.add_separator(14.0);
        }
        self.add_flavour_text(tip, item, base)?;
        // The granted skills' tooltips show with Shift only
        let mod_list: &[engine::mods::Mod] = match (&item.mod_list, &item.slot_mod_list) {
            (Some(l), _) => l,
            (None, Some(l)) => slot_num
                .and_then(|n| l.get((n as usize).wrapping_sub(1)))
                .ok_or_else(|| unported("the mods of a slot"))?,
            _ => return Err(unported("an item without mods")),
        };
        let supports = mod_list
            .iter()
            .filter(|m| m.name.as_str() == "ExtraSupport")
            .count();
        let skills = item.granted_skills.len() + supports;
        if skills > 0 {
            tip.add_separator(14.0);
            let s = if skills > 1 { "s" } else { "" };
            let line = format!(
                "{}Tip: Hold Shift to display a tooltip for the granted skill{s}.",
                cc("TIP")
            );
            tip.add_line(14.0, &line);
        }
        let hint = if self.view_mode == "ITEMS" {
            ""
        } else {
            " in the Items tab"
        };
        let tip_c = cc("TIP");
        if !self.items_stat_differences() {
            tip.add_separator(14.0);
            let line = format!(
                "{tip_c}Tip: Press Ctrl+D{hint} to enable the display of stat differences."
            );
            tip.add_line(14.0, &line);
            return Ok(());
        } else if base.flask.is_none() && base.tincture.is_none() {
            let line = format!(
                "{tip_c}Tip: Press Ctrl+D{hint} to disable the display of stat differences."
            );
            tip.add_line(14.0, &line);
        }
        self.add_item_stat_differences(tip, item, slot)
    }

    /// The buff lines of a flask or tincture
    fn add_buff_lines(&self, tip: &mut Tooltip, item: &Item) {
        for ml in &item.buff_mod_lines {
            if ml.extra.is_some() {
                let line = format!("{}{}{NOT_SUPPORTED}", cc("UNSUPPORTED"), ml.line);
                tip.add_line(BIG, &line);
            } else {
                tip.add_line(BIG, &format!("{}{}", cc("MAGIC"), ml.line));
            }
        }
    }

    /// `itemLib.formatModLine(modLine)`
    fn format_mod_line(&self, ml: &ModLine) -> Result<Option<String>> {
        let line = if ml.range.is_some() || ml.corrupted_range.is_some() {
            let range = engine::item_tools::Range::One(ml.range.unwrap_or(0.5));
            engine::item_tools::apply_range(
                &self.data.game.items,
                &self.data.parser,
                &ml.line,
                &range,
                ml.value_scalar,
                ml.corrupted_range,
            )?
        } else {
            ml.line.clone()
        };
        let m = |p: &str| engine::pat::is_match(&line, p);
        if m("^%+?0%%? ") || (m(" %+?0%%? ") && !m("0 to [1-9]")) || m(" 0%-0 ") || m(" 0 to 0 ") {
            return Ok(None);
        }
        let f = &ml.flags;
        if f.disabled {
            return Ok(Some(format!("{}{line}", cc("DISABLED"))));
        }
        if ml.extra.is_some() {
            return Ok(Some(format!("{}{line}{NOT_SUPPORTED}", cc("UNSUPPORTED"))));
        }
        let code = if f.fractured {
            "FRACTURED"
        } else if f.crafted {
            "CRAFTED"
        } else if f.mutated {
            "MUTATED"
        } else if f.scourge {
            "SCOURGE"
        } else if f.custom {
            "CUSTOM"
        } else if f.crucible {
            "CRUCIBLE"
        } else if f.vestigial {
            "VESTIGIAL"
        } else {
            "MAGIC"
        };
        Ok(Some(format!("{}{line}", cc(code))))
    }

    /// `buildMode:AddRequirementsToTooltip` with the item's requirements
    fn add_requirements(&self, tip: &mut Tooltip, item: &Item) -> Result<()> {
        let r = &item.requirements;
        let sb = self.sidebar()?;
        let q = |how: &str, player: bool, name: &str| -> Result<f64> {
            sb.item_queries
                .get(item_query_key(how, player, name).as_str())
                .copied()
                .ok_or_else(|| format!("no item query {name}"))
        };
        let out = |k: &str| -> Result<f64> {
            self.main
                .player
                .get(&engine::mods::Sym::new(k))
                .and_then(engine::actor::OutVal::num)
                .ok_or_else(|| unported(&format!("requirements without {k}")))
        };
        let mut req = Vec::new();
        if let Some(l) = r.level.filter(|l| *l > 0.0) {
            let c = stat_color(l, None, Some(self.build.info.character_level));
            req.push(sf!(&format!("{GREY}Level %s%d"), c, l));
        }
        if q("FLAG", false, "OmniscienceRequirements")? != 0.0 {
            let satisfy = q("INC", false, "OmniAttributeRequirements")?;
            let mut highest = 0.0;
            for s in [r.str_mod, r.dex_mod, r.int_mod] {
                if s > highest {
                    highest = s;
                }
            }
            let omni = (highest * (100.0 / satisfy)).floor();
            let have = out("Omni")?;
            if omni > 0.0 || omni > have {
                let c = stat_color(omni, Some(0.0), Some(have));
                req.push(sf!(&format!("%s%d {GREY}Omni"), c, omni));
            }
        } else {
            for (v, b, k) in [
                (r.str_mod, r.str, "Str"),
                (r.dex_mod, r.dex, "Dex"),
                (r.int_mod, r.int, "Int"),
            ] {
                let have = out(k)?;
                if v > 14.0 || v > have {
                    let c = stat_color(v, Some(b), Some(have));
                    req.push(sf!(&format!("%s%d {GREY}{k}"), c, v));
                }
            }
        }
        if !req.is_empty() {
            let line = format!("{GREY}Requires {}", req.join(&format!("{GREY}, ")));
            tip.add_line(BIG, &line);
            tip.add_separator(10.0);
        }
        Ok(())
    }

    /// A cluster jewel's notables or keystone
    fn add_cluster_lines(&self, tip: &mut Tooltip, item: &Item) -> Result<()> {
        tip.add_separator(10.0);
        let jd = item.jewel_data.as_ref();
        let tree = self.build.spec.tree;
        let node_lines = |tip: &mut Tooltip, name: &str| {
            let Some(&id) = tree.cluster_node_map.get(name) else {
                return;
            };
            let Some(node) = tree.node_index.get(&id).map(|&i| &tree.nodes[i]) else {
                return;
            };
            tip.add_line(BIG, &format!("{}{}", cc("MAGIC"), node.dn));
            for stat in &node.sd {
                tip.add_line(BIG, &format!("{GREY}{stat}"));
            }
        };
        let notables: Vec<String> = match jd.and_then(|j| j.get("clusterJewelNotables")) {
            Some(ModValue::List(l)) => l
                .iter()
                .filter_map(|v| match v {
                    ModValue::Str(s) => Some(s.to_string()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        if !notables.is_empty() {
            for name in &notables {
                node_lines(tip, name);
            }
        } else if let Some(ModValue::Str(k)) = jd.and_then(|j| j.get("clusterJewelKeystone")) {
            node_lines(tip, k);
        }
        tip.add_separator(10.0);
        Ok(())
    }

    /// The flavour text of a unique or of the base
    fn add_flavour_text(
        &self,
        tip: &mut Tooltip,
        item: &Item,
        base: &engine::item_data::Base,
    ) -> Result<()> {
        let unique = item.rarity == "UNIQUE" || item.rarity == "RELIC";
        if !unique && base.flavour_text.is_none() {
            return Ok(());
        }
        let mut flavour: Option<Vec<String>> = base.flavour_text.clone();
        if flavour.is_none() {
            let title = item
                .title
                .as_deref()
                .ok_or_else(|| unported("a unique without a title"))?;
            let key = engine::pat::gsub(title, "^Foulborn%s+", "");
            if let Some(list) = self.data.game.items.flavour_text.get(&key) {
                if engine::pat::is_match(title, "Grand Spectrum") {
                    let mut pick = None;
                    for t in tip.texts() {
                        let f = |p: &str| engine::pat::find_plain(t, p).is_some();
                        pick = if f("Power") {
                            Some("UniqueJewel170")
                        } else if f("Endurance") {
                            Some("UniqueJewel168")
                        } else if f("Frenzy") {
                            Some("UniqueJewel169")
                        } else if f("Elemental Damage") {
                            Some("UniqueJewel168")
                        } else if f("Elemental Ailments") {
                            Some("UniqueJewel166")
                        } else if f("Elemental Resistances") || f("Armour") {
                            Some("UniqueJewel76")
                        } else if f("Maximum Life") {
                            Some("UniqueJewel165")
                        } else if f("Critical Strike Multiplier") {
                            Some("UniqueJewel167")
                        } else if f("Critical Strike Chance") || f("Mana") {
                            Some("UniqueJewel75")
                        } else {
                            None
                        };
                        if pick.is_some() {
                            break;
                        }
                    }
                    if let Some(p) = pick
                        && let Some((_, t)) = list.iter().find(|(i, _)| i == p)
                    {
                        flavour = Some(t.clone());
                    }
                } else if let Some((_, t)) = list.first() {
                    // `pairs` of the ids: the translation keeps string keys
                    // in the order they were set
                    flavour = Some(t.clone());
                }
            }
        }
        if let Some(f) = flavour {
            for line in f {
                tip.add_line(BIG, &format!("{}{line}", cc("UNIQUE")));
            }
            tip.add_separator(14.0);
        }
        Ok(())
    }

    /// `buildSpecForJewelComparison`: the spec with the jewel in the socket
    /// (or none) and its paths built again, for jewels that replace
    /// passives or disconnect allocated ones. Upstream's copy starts
    /// without its subgraphs' pending allocations.
    fn spec_for_jewel_comparison(
        &self,
        socket: u32,
        rep: Option<&Item>,
    ) -> Result<std::sync::Arc<engine::spec::PassiveSpec<'d>>> {
        let mut spec = (*self.build.spec).clone();
        spec.alloc_subgraph_nodes.clear();
        spec.alloc_extended_nodes.clear();
        match rep {
            // An item not in the items tab takes a free negative id
            Some(item) => {
                let id = item
                    .id
                    .filter(|id| self.build.items.item(*id).is_some())
                    .unwrap_or_else(|| {
                        let mut id = -1.0;
                        while self.build.items.item(id).is_some() {
                            id -= 1.0;
                        }
                        id
                    });
                spec.jewels.insert(socket, id);
            }
            None => {
                spec.jewels.remove(&socket);
            }
        }
        spec.set_tree_jewels(&self.build.items);
        match rep.and_then(engine::spec::TreeJewel::of) {
            Some(j) => {
                spec.tree_jewels.insert(socket, j);
            }
            None => {
                spec.tree_jewels.remove(&socket);
            }
        }
        spec.build_all_depends_and_paths()?;
        Ok(std::sync::Arc::new(spec))
    }

    /// `ItemsTab:AddItemStatDifferences`
    fn add_item_stat_differences(
        &self,
        tip: &mut Tooltip,
        item: &Item,
        slot: Option<&Slot>,
    ) -> Result<()> {
        let base = item.base.as_ref().ok_or("no base")?;
        if base.flask.is_some() || base.tincture.is_some() {
            return self.add_flask_differences(tip, item);
        }
        let data = self.data.calc_data();
        let calc = MiscCalculator::new(&self.build, &data)?;
        let tab = &self.build.items;
        let calc_slot = |s: &'_ Slot| -> Result<(Option<&Item>, CalcOutput)> {
            let sel = tab.item(s.sel_item_id).filter(|_| s.sel_item_id != 0.0);
            let same = sel.is_some_and(|x| item.id.is_some() && x.id == item.id);
            let rep = (!same).then(|| item.clone());
            let spec = match s.node_id {
                Some(socket) if sel.is_some_and(changes_tree) || changes_tree(item) => {
                    Some(self.spec_for_jewel_comparison(socket, rep.as_ref())?)
                }
                _ => None,
            };
            let ovr = Override {
                rep_item: Some((s.slot_name.clone(), rep)),
                ..Override::default()
            };
            let out = match spec {
                Some(spec) => calc.calculate_on_spec(spec, &ovr, true)?,
                None => calc.calculate(&ovr, true)?,
            };
            Ok((sel, out))
        };
        let base_out = calc.base();
        let add = |tip: &mut Tooltip, label: &str, sel: Option<&Item>, out: &CalcOutput| {
            let same = sel.is_some_and(|x| item.id.is_some() && x.id == item.id);
            let header = if same {
                format!("^7Removing this item from {label} will give you:")
            } else {
                let replacing = match sel {
                    Some(s) => format!(
                        "\n(replacing {}{}^7)",
                        color(&s.rarity).unwrap_or(""),
                        s.name
                    ),
                    None => String::new(),
                };
                format!("^7Equipping this item in {label} will give you:{replacing}")
            };
            let b = Outputs {
                player: &base_out.player,
                minion: base_out.minion.as_ref(),
            };
            let c = Outputs {
                player: &out.player,
                minion: out.minion.as_ref(),
            };
            self.add_stat_compares(tip, &b, &c, &header, None)
        };
        // Only the named slot, as "Show tooltips only for affected slots"
        if let Some(s) = slot {
            let (sel, out) = calc_slot(s)?;
            add(tip, &self.slot_label(s), sel, &out)?;
            return Ok(());
        }
        let unique = item.rarity == "UNIQUE" || item.rarity == "RELIC";
        let mut same_count = 0.0;
        let mut slots: Vec<Compare> = Vec::new();
        let ring3 = self.sidebar()?.additional_ring_slot;
        for (i, s) in tab.ordered_slots.iter().enumerate() {
            // `slot.shown()`: active, of the weapon set in use, the graft
            // slots on 3.27 trees, the third ring with its flag
            if !self.valid_for_slot(item, &s.slot_name) || !self.slot_shown(i, ring3) {
                continue;
            }
            let (sel, out) = calc_slot(s)?;
            let same_unique = unique && sel.is_some_and(|x| x.name == item.name);
            if same_unique && item.limit.is_some() {
                same_count += 1.0;
            }
            slots.push(Compare {
                slot: s,
                label: self.slot_label(s),
                sel,
                out,
                same_unique,
            });
        }
        if Some(same_count) == item.limit {
            for c in slots.iter().filter(|c| c.same_unique) {
                add(tip, &c.label, c.sel, &c.out)?;
            }
            return Ok(());
        }
        let similar = |c: &Compare| -> f64 {
            let Some(x) = c.sel else { return 0.0 };
            let (xb, ib) = (x.base.as_ref(), item.base.as_ref());
            let same_base = !unique
                && x.rarity != "UNIQUE"
                && x.rarity != "RELIC"
                && xb.map(|b| &b.typ) == ib.map(|b| &b.typ)
                && xb.map(|b| &b.sub_type) == ib.map(|b| &b.sub_type);
            if same_base || c.same_unique { 1.0 } else { 0.0 }
        };
        let num = |c: &Compare, k: &str| c.out.num(k);
        // Descending by: empty slot, similar item, Full DPS, combined DPS,
        // EHP, label, slot name
        slots.sort_by(|a, b| {
            use std::cmp::Ordering::*;
            let nums = [
                (
                    Some(if a.slot.sel_item_id == 0.0 { 1.0 } else { 0.0 }),
                    Some(if b.slot.sel_item_id == 0.0 { 1.0 } else { 0.0 }),
                ),
                (Some(similar(a)), Some(similar(b))),
                (num(a, "FullDPS"), num(b, "FullDPS")),
                (num(a, "CombinedDPS"), num(b, "CombinedDPS")),
                (num(a, "TotalEHP"), num(b, "TotalEHP")),
            ];
            for (x, y) in nums {
                if let (Some(x), Some(y)) = (x, y) {
                    if x > y {
                        return Less;
                    } else if x < y {
                        return Greater;
                    }
                }
            }
            for (x, y) in [(&a.label, &b.label), (&a.slot.slot_name, &b.slot.slot_name)] {
                match x.as_bytes().cmp(y.as_bytes()) {
                    Equal => {}
                    o => return o.reverse(),
                }
            }
            Equal
        });
        for c in &slots {
            add(tip, &c.label, c.sel, &c.out)?;
        }
        Ok(())
    }

    /// The effective stats of a flask or tincture, and the stat
    /// differences of toggling it
    fn add_flask_differences(&self, tip: &mut Tooltip, item: &Item) -> Result<()> {
        // `toggleFlask = item` with an item outside the build adds it: it
        // joins a copy of the items tab under a free id
        let mut with_item = None;
        let id = match item.id.filter(|id| self.build.items.item(*id).is_some()) {
            Some(id) => id,
            None => {
                let mut id = -1.0;
                while self.build.items.item(id).is_some() {
                    id -= 1.0;
                }
                let mut b = self.build.clone();
                let mut it = item.clone();
                it.id = Some(id);
                b.items.items.insert(id as i64, std::sync::Arc::new(it));
                with_item = Some(b);
                id
            }
        };
        let base = item.base.as_ref().ok_or("no base")?;
        let sb = self.sidebar()?;
        let q = |how: &str, player: bool, name: &str| -> Result<f64> {
            sb.item_queries
                .get(item_query_key(how, player, name).as_str())
                .copied()
                .ok_or_else(|| format!("no item query {name}"))
        };
        let out = |k: &str| -> Result<f64> {
            self.main
                .player
                .get(&engine::mods::Sym::new(k))
                .and_then(engine::actor::OutVal::num)
                .ok_or_else(|| unported(&format!("flask stats without {k}")))
        };
        let mut stats: Vec<String> = Vec::new();
        let (header, ovr) = if let Some(bf) = &base.flask {
            let fd = item.flask_data.as_ref().ok_or("no flask data")?;
            let dur_inc = q("INC", false, "FlaskDuration")?;
            let mut effect_inc = q("INC", true, "FlaskEffect")?;
            let (mut life_dur, mut mana_dur) = (0.0, 0.0);
            if item.rarity == "MAGIC" && bf.life.is_none() && bf.mana.is_none() {
                effect_inc += q("INC", true, "MagicUtilityFlaskEffect")?;
            }
            let duration = need(fd, "duration")?;
            if bf.life.is_some() || bf.mana.is_some() {
                let rate_inc = q("INC", false, "FlaskRecoveryRate")?;
                let instant_perc = need(fd, "instantPerc")?;
                if bf.life.is_some() {
                    let life_inc = q("INC", false, "FlaskLifeRecovery")?;
                    let life_more = q("MORE", false, "FlaskLifeRecovery")?;
                    let life_rate_inc = q("INC", false, "FlaskLifeRecoveryRate")?;
                    let instant = instant_perc + q("BASE", false, "LifeFlaskInstantRecovery")?;
                    let low = q("MORE", false, "FlaskLifeRecoveryLowLife")?;
                    let low_multi = if low > 1.0 {
                        (low - 1.0) / (1.0 + effect_inc / 100.0) + 1.0
                    } else {
                        1.0
                    };
                    let life_base = need(fd, "lifeBase")?;
                    let inst = life_base * instant / 100.0
                        * (1.0 + life_inc / 100.0)
                        * life_more
                        * (1.0 + effect_inc / 100.0)
                        * low_multi;
                    let b = life_base
                        * (1.0 - instant / 100.0)
                        * (1.0 + life_inc / 100.0)
                        * life_more
                        * (1.0 + effect_inc / 100.0)
                        * (1.0 + dur_inc / 100.0)
                        * low_multi;
                    let grad = b * out("LifeRecoveryRateMod")?;
                    let es_grad = b * out("EnergyShieldRecoveryRateMod")?;
                    life_dur = duration * (1.0 + dur_inc / 100.0)
                        / (1.0 + rate_inc / 100.0)
                        / (1.0 + life_rate_inc / 100.0);
                    let additional = need(fd, "lifeAdditional")?;
                    let full_life = matches!(
                        self.build.config.active().input.get("conditionFullLife"),
                        Some(v) if v.truthy()
                    );
                    if additional > 0.0 && !full_life {
                        let total = (additional / 100.0)
                            * need(fd, "lifeTotal")?
                            * out("LifeRecoveryRateMod")?;
                        let add_grad = (life_dur / 10.0) * total;
                        let left_dur = 10.0 - life_dur;
                        let left = total - add_grad;
                        if inst > 0.0 {
                            if grad > 0.0 {
                                stats.push(sf!("^8Life recovered: ^7%d ^8(^7%d^8 instantly, plus ^7%d ^8over^7 %.2fs^8, and an additional ^7%d ^8over subsequent ^7%.2fs^8)",
                                    inst + grad + total, inst, grad + add_grad, life_dur, left, left_dur));
                            } else {
                                life_dur = 0.0;
                                stats.push(sf!("^8Life recovered: ^7%d ^8(^7%d^8 instantly, and an additional ^7%d ^8over ^7%.2fs^8)",
                                    inst + total, inst, total, 10.0));
                            }
                        } else {
                            stats.push(sf!("^8Life recovered: ^7%d ^8(^7%d ^8over ^7%.2fs^8, and an additional ^7%d ^8over subsequent ^7%.2fs^8)",
                                grad + total, grad + add_grad, life_dur, left, left_dur));
                        }
                    } else if inst > 0.0 && grad > 0.0 {
                        stats.push(sf!("^8Life recovered: ^7%d ^8(^7%d^8 instantly, plus ^7%d ^8over^7 %.2fs^8)",
                            inst + grad, inst, grad, life_dur));
                    } else if inst + grad != need(fd, "lifeTotal")?
                        || (inst == 0.0 && life_dur != duration)
                    {
                        if inst > 0.0 {
                            life_dur = 0.0;
                            stats.push(sf!("^8Life recovered: ^7%d ^8instantly", inst));
                        } else if grad > 0.0 {
                            stats.push(sf!(
                                "^8Life recovered: ^7%d ^8over ^7%.2fs",
                                grad,
                                life_dur
                            ));
                        }
                    }
                    if q("FLAG", false, "LifeFlaskAppliesToEnergyShield")? != 0.0 {
                        if inst > 0.0 && es_grad > 0.0 {
                            stats.push(sf!("^8Energy Shield recovered: ^7%d ^8(^7%d^8 instantly, plus ^7%d ^8over^7 %.2fs^8)",
                                inst + es_grad, inst, es_grad, life_dur));
                        } else if inst > 0.0 && es_grad == 0.0 {
                            stats.push(sf!("^8Energy Shield recovered: ^7%d ^8instantly", inst));
                        } else if inst == 0.0 && es_grad > 0.0 {
                            stats.push(sf!(
                                "^8Energy Shield recovered: ^7%d ^8over ^7%.2fs",
                                es_grad,
                                life_dur
                            ));
                        }
                    }
                }
                if bf.mana.is_some() {
                    let mana_inc = q("INC", false, "FlaskManaRecovery")?;
                    let mana_rate_inc = q("INC", false, "FlaskManaRecoveryRate")?;
                    let instant = instant_perc + q("BASE", false, "ManaFlaskInstantRecovery")?;
                    let mana_base = need(fd, "manaBase")?;
                    let inst = mana_base * instant / 100.0
                        * (1.0 + mana_inc / 100.0)
                        * (1.0 + effect_inc / 100.0);
                    let b = mana_base
                        * (1.0 - instant / 100.0)
                        * (1.0 + mana_inc / 100.0)
                        * (1.0 + effect_inc / 100.0)
                        * (1.0 + dur_inc / 100.0);
                    let grad = b * out("ManaRecoveryRateMod")?;
                    let life_grad = b * out("LifeRecoveryRateMod")?;
                    mana_dur = duration * (1.0 + dur_inc / 100.0)
                        / (1.0 + rate_inc / 100.0)
                        / (1.0 + mana_rate_inc / 100.0);
                    if inst > 0.0 && grad > 0.0 {
                        stats.push(sf!("^8Mana recovered: ^7%d ^8(^7%d^8 instantly, plus ^7%d ^8over^7 %.2fs^8)",
                            inst + grad, inst, grad, mana_dur));
                    } else if inst + grad != need(fd, "manaTotal")?
                        || (inst == 0.0 && mana_dur != duration)
                    {
                        if inst > 0.0 {
                            mana_dur = 0.0;
                            stats.push(sf!("^8Mana recovered: ^7%d ^8instantly", inst));
                        } else if grad > 0.0 {
                            stats.push(sf!(
                                "^8Mana recovered: ^7%d ^8over ^7%.2fs",
                                grad,
                                mana_dur
                            ));
                        }
                    }
                    if q("FLAG", false, "ManaFlaskAppliesToLife")? != 0.0 && life_grad > 0.0 {
                        stats.push(sf!(
                            "^8Life recovered: ^7%d ^8over ^7%.2fs",
                            life_grad,
                            mana_dur
                        ));
                    }
                }
            } else if dur_inc != 0.0 {
                stats.push(sf!(
                    "^8Flask effect duration: ^7%.1f0s",
                    duration * (1.0 + dur_inc / 100.0)
                ));
            }
            let effect_mod = 1.0 + (need(fd, "effectInc")? + effect_inc) / 100.0;
            if effect_mod != 1.0 {
                stats.push(sf!(
                    "^8Flask effect modifier: ^7%+d%%",
                    effect_mod * 100.0 - 100.0
                ));
            }
            let used_inc = q("INC", false, "FlaskChargesUsed")?;
            let (charges_used, charges_max) = (need(fd, "chargesUsed")?, need(fd, "chargesMax")?);
            if used_inc != 0.0 {
                let used = (charges_used * (1.0 + used_inc / 100.0)).floor();
                stats.push(sf!(
                    "^8Charges used: ^7%d ^8of ^7%d ^8(^7%d ^8uses)",
                    used,
                    charges_max,
                    (charges_max / used).floor()
                ));
            }
            let gain_mod =
                need(fd, "gainMod")? * (1.0 + q("INC", false, "FlaskChargesGained")? / 100.0);
            if gain_mod != 1.0 {
                stats.push(sf!(
                    "^8Charge gain modifier: ^7%+d%%",
                    gain_mod * 100.0 - 100.0
                ));
            }
            let mut generated = q("BASE", false, "FlaskChargesGenerated")?;
            if bf.life.is_some() {
                generated += q("BASE", false, "LifeFlaskChargesGenerated")?;
            }
            if bf.mana.is_some() {
                generated += q("BASE", false, "ManaFlaskChargesGenerated")?;
            }
            if bf.mana.is_none() && bf.life.is_none() {
                generated += q("BASE", false, "UtilityFlaskChargesGenerated")?;
            }
            let mut on_ward = 0.0;
            if item.base_name.as_deref() == Some("Iron Flask") {
                on_ward += q("BASE", false, "IronFlaskChargesGeneratedOnWardBreak")?;
            }
            let empty = self
                .build
                .items
                .ordered_slots
                .iter()
                .filter(|s| s.slot_name.starts_with("Flask") && s.sel_item_id == 0.0)
                .count() as f64;
            let per_flask =
                q("BASE", false, "FlaskChargesGeneratedPerEmptyFlask")? * empty * gain_mod;
            generated *= gain_mod;
            on_ward *= gain_mod;
            let total = generated + per_flask;
            if total > 0.0 {
                stats.push(sf!("^8Charges generated: ^7%.2f^8 per second", total));
            }
            if on_ward > 0.0 {
                stats.push(sf!("^8Charges generated on Ward Break: ^7%.2f", on_ward));
            }
            let not_consume = q("BASE", false, "FlaskChanceNotConsumeCharges")?.min(100.0);
            if not_consume != 0.0 {
                stats.push(sf!("^8Chance to not consume charges: ^7%d%%", not_consume));
            }
            let truthy = |k: &str| fd.get(k).is_some_and(|v| v.truthy());
            let mut uptime = bf.life.is_none() && bf.mana.is_none();
            let mut flask_dur = duration * (1.0 + dur_inc / 100.0);
            if bf.life.is_some()
                && (truthy("lifeEffectNotRemoved")
                    || q("FLAG", false, "LifeFlaskEffectNotRemoved")? != 0.0)
            {
                uptime = true;
                flask_dur = life_dur;
            } else if bf.mana.is_some()
                && (truthy("manaEffectNotRemoved")
                    || q("FLAG", false, "ManaFlaskEffectNotRemoved")? != 0.0)
            {
                uptime = true;
                flask_dur = mana_dur;
            }
            if uptime {
                let used = charges_used * (1.0 + used_inc / 100.0);
                if used > 0.0 && flask_dur > 0.0 {
                    let lua_mod = |a: f64, b: f64| a - (a / b).floor() * b;
                    let per3 = flask_dur - lua_mod(flask_dur, 3.0);
                    let per5 = flask_dur - lua_mod(flask_dur, 5.0);
                    let min_gen = per3 * generated + per5 * per_flask;
                    let pc_min = (min_gen / used * 100.0).min(100.0);
                    if pc_min < 100.0 && not_consume < 100.0 {
                        let avg_gen = (generated + per_flask) * flask_dur;
                        let avg_used = used * (100.0 - not_consume) / 100.0;
                        let pc_avg = (avg_gen / avg_used * 100.0).min(100.0);
                        stats.push(sf!(
                            "^8Flask uptime: ^7%d%%^8 average, ^7%d%%^8 minimum",
                            pc_avg,
                            pc_min
                        ));
                    } else {
                        stats.push("^8Flask uptime: ^7100%^8".to_string());
                    }
                }
            }
            if !stats.is_empty() {
                tip.add_line(14.0, "^7Effective flask stats:");
                for s in &stats {
                    tip.add_line(14.0, s);
                }
            }
            let header = if sb.flasks.contains(&(id as i64)) {
                "^7Deactivating this flask will give you:"
            } else {
                "^7Activating this flask will give you:"
            };
            let ovr = Override {
                toggle_flask: Some(id),
                ..Override::default()
            };
            (header, ovr)
        } else {
            let bt = base.tincture.as_ref().ok_or("no tincture base")?;
            let td = item.tincture_data.as_ref().ok_or("no tincture data")?;
            let mut effect_inc = q("INC", true, "TinctureEffect")?;
            if item.rarity == "MAGIC" {
                effect_inc += q("INC", true, "MagicTinctureEffect")?;
            }
            let effect_mod = (1.0 + (need(td, "effectInc")? + effect_inc) / 100.0)
                * (1.0 + item.quality.unwrap_or(0.0) / 100.0);
            if effect_mod != 1.0 {
                stats.push(sf!(
                    "^8Tincture effect modifier: ^7%+d%%",
                    effect_mod * 100.0 - 100.0
                ));
            }
            let burn = need(td, "manaBurn")?
                / (1.0 + q("INC", true, "TinctureManaBurnRate")? / 100.0)
                / (1.0 + q("SUMMORE", true, "TinctureManaBurnRate")? / 100.0);
            stats.push(sf!(
                "^8Mana Burn Inflicted Every Second: ^7%.2f",
                1.0 / burn
            ));
            let not_burn = q("BASE", false, "TincturesNotInflictManaBurn")?.min(100.0);
            if not_burn != 0.0 {
                stats.push(sf!("^8Chance to not inflict Mana Burn: ^7%d%%", not_burn));
            }
            let cd = bt.cooldown
                / (1.0
                    + (q("INC", true, "TinctureCooldownRecovery")? + need(td, "cooldownInc")?)
                        / 100.0);
            stats.push(sf!(
                "^8Tincture Cooldown when deactivated: ^7%.2f^8 seconds",
                cd
            ));
            tip.add_line(14.0, "^7Effective tincture stats:");
            for s in &stats {
                tip.add_line(14.0, s);
            }
            let header = if sb.tinctures.contains(&(id as i64)) {
                "^7Deactivating this tincture will give you:"
            } else {
                "^7Activating this tincture will give you:"
            };
            let ovr = Override {
                toggle_tincture: Some(id),
                ..Override::default()
            };
            (header, ovr)
        };
        let data = self.data.calc_data();
        let calc = MiscCalculator::new(&self.build, &data)?;
        let o = match &with_item {
            Some(b) => calc.calculate_on(b, &ovr, true)?,
            None => calc.calculate(&ovr, true)?,
        };
        let b = calc.base();
        let base = Outputs {
            player: &b.player,
            minion: b.minion.as_ref(),
        };
        let compare = Outputs {
            player: &o.player,
            minion: o.minion.as_ref(),
        };
        self.add_stat_compares(tip, &base, &compare, header, None)?;
        Ok(())
    }

    /// `previewItem`: the tooltip of an item from its text, not in the
    /// build
    pub fn preview_item(&mut self, text: &str) -> Result<J> {
        let ctx = self.data.item_context();
        let mut item = Item::new(&ctx, text, None, false)
            .map_err(|e| unported(&format!("an item text: {e}")))?;
        if item.base.is_none() {
            return Err(unported("an unrecognised item"));
        }
        item.normalise_quality();
        self.update_sockets();
        let mut tip = Tooltip::default();
        self.add_item_tooltip(&mut tip, &item, None)?;
        let mut out = tip.json();
        out["item"] = super::items::describe(&item);
        Ok(out)
    }
}

/// Whether the item is a cluster jewel (`item.clusterJewel`)
fn base_name_is_cluster(s: &TypedSession, item: &Item) -> bool {
    item.base_name
        .as_ref()
        .is_some_and(|b| s.data.game.items.cluster_jewels.contains_key(b))
}

/// `ItemsTab:GetSocketDescriptionLine(item)`
fn socket_line(item: &Item) -> String {
    let mut line = String::new();
    let mut group = 0.0;
    for (i, s) in item.sockets.iter().enumerate() {
        if i > 0 {
            if s.group == group {
                line.push_str("^7=");
            } else {
                line.push_str("  ");
            }
            group = s.group;
        }
        let code = match s.color {
            'R' => cc("STRENGTH"),
            'G' => cc("DEXTERITY"),
            'B' => cc("INTELLIGENCE"),
            'W' => cc("SCION"),
            'A' => "^xB0B0B0",
            _ => "",
        };
        line.push_str(code);
        line.push(s.color);
    }
    line
}
