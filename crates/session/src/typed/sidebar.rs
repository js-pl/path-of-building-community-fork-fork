//! The build mode's sidebar (`Modules/Build.lua`): the stat list and
//! warnings (`RefreshStatList`, `AddDisplayStatList`, `FormatStat`,
//! `InsertItemWarnings`, `EstimatePlayerProgress`), the class and skill
//! selection controls (`RefreshSkillSelectControls`, the class drop-downs)
//! and the `state` command of `lua/session.lua`.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::display_stats::{self as ds, Ctx, Stat, Suffix};
use super::{Result, TypedSession, num};
use engine::actor::OutVal;
use engine::calcs::Output;
use engine::mods::Sym;
use engine::sidebar::{Sidebar, SkillInfo};
use engine::skills::{ExplodeSource, SocketGroup};
use engine::tree::NodeType;
use serde_json::{Map, Value as J, json};
use std::sync::OnceLock;

/// A drop-down's state: labels, selection and visibility
#[derive(Clone, Debug)]
pub(super) struct Drop {
    pub list: Vec<String>,
    pub sel: f64,
    pub shown: bool,
    pub enabled: bool,
}

impl Default for Drop {
    fn default() -> Drop {
        Drop {
            list: Vec::new(),
            sel: 1.0,
            shown: true,
            enabled: true,
        }
    }
}

impl SkillSelect {
    /// The minion drop-down's entry
    pub(super) fn minion_entry(&self, i: usize) -> Option<MinionEntry> {
        self.minion_entries.get(i).cloned()
    }
}

impl Drop {
    pub(super) fn json(&self) -> J {
        json!({
            "class": "DropDownControl",
            "shown": self.shown,
            "enabled": self.enabled,
            "list": self.list,
            "sel": num(self.sel),
        })
    }
}

#[derive(Clone, Debug)]
pub(super) struct Edit {
    pub text: String,
    pub shown: bool,
}

impl Default for Edit {
    fn default() -> Edit {
        Edit {
            text: String::new(),
            shown: true,
        }
    }
}

impl Edit {
    pub(super) fn json(&self) -> J {
        json!({ "class": "EditControl", "shown": self.shown, "enabled": true, "text": self.text })
    }
}

/// An entry of the minion drop-down: an item set or a minion, or text
#[derive(Clone, Debug, PartialEq)]
pub(super) enum MinionEntry {
    ItemSet(f64),
    Minion(String),
    Text,
}

/// The sidebar's controls, which keep what they were last set to, as
/// upstream's do
#[derive(Clone, Debug, Default)]
pub struct Controls {
    pub(super) level: Edit,
    /// The level box was typed in: its text stays as typed
    pub(super) level_typed: bool,
    class: Drop,
    ascend: Drop,
    secondary: Drop,
    pub(super) skill_select: SkillSelect,
}

/// The skill selection controls of the sidebar or the Calcs tab
/// (`RefreshSkillSelectControls`)
#[derive(Clone, Debug, Default)]
pub(super) struct SkillSelect {
    pub socket_group: Drop,
    pub skill: Drop,
    pub part: Drop,
    pub stage: Edit,
    pub mine: Edit,
    pub minion: Drop,
    minion_entries: Vec<MinionEntry>,
    pub minion_skill: Drop,
    /// `mainSkillMinionLibrary.shown`, once set
    pub library: Option<bool>,
}

const MAIN_TREE_POINTS: f64 = 99.0 + 23.0;

/// `acts`: the level and quest points of each act
const ACTS: [(f64, f64); 11] = [
    (1.0, 0.0),
    (12.0, 2.0),
    (22.0, 4.0),
    (32.0, 6.0),
    (40.0, 7.0),
    (44.0, 9.0),
    (50.0, 12.0),
    (54.0, 15.0),
    (60.0, 18.0),
    (64.0, 20.0),
    (67.0, 23.0),
];

pub(super) fn display_stats() -> &'static (Vec<Stat>, Vec<Stat>) {
    static STATS: OnceLock<(Vec<Stat>, Vec<Stat>)> = OnceLock::new();
    STATS.get_or_init(|| {
        (
            ds::with_compares(ds::display_stats(), false),
            ds::with_compares(ds::minion_display_stats(), true),
        )
    })
}

/// `InsertIfNew`
fn insert_new(list: &mut Vec<String>, s: String) {
    if !list.contains(&s) {
        list.push(s);
    }
}

/// A row of the stat box: its texts and whether it is centred
enum Row {
    Gap,
    Text(String, Option<String>, bool),
}

impl Row {
    fn left(s: String) -> Row {
        Row::Text(s, None, false)
    }
    fn center(s: String) -> Row {
        Row::Text(s, None, true)
    }
    fn json(&self) -> J {
        match self {
            Row::Gap => json!({ "gap": true }),
            Row::Text(a, b, false) => match b {
                Some(b) => json!([a, b]),
                None => json!([a]),
            },
            Row::Text(a, b, true) => {
                let mut m = Map::new();
                m.insert("1".into(), json!(a));
                if let Some(b) = b {
                    m.insert("2".into(), json!(b));
                }
                m.insert("center".into(), J::Bool(true));
                J::Object(m)
            }
        }
    }
}

/// `matchFlags(flag, notFlag, skillFlags)`
fn match_flags(s: &Stat, skill: &SkillInfo) -> bool {
    s.flag.is_none_or(|f| skill.flag(f)) && s.not_flag.is_none_or(|f| !skill.flag(f))
}

/// `buildMode:FormatStat` with upstream's default settings: thousands
/// separators, no compact values
fn format_stat(s: &Stat, v: f64, over_cap: Option<f64>, color: Option<&str>) -> String {
    let val =
        v * if s.pc || s.modifier { 100.0 } else { 1.0 } - if s.modifier { 100.0 } else { 0.0 };
    let mut color = color.unwrap_or(if v >= 0.0 {
        "^7"
    } else if s.chaos_inoc {
        "^8"
    } else {
        ds::NEGATIVE
    });
    if s.label == "Unreserved Life" && v == 0.0 {
        color = ds::NEGATIVE;
    }
    let mut out = engine::sf!(&format!("%{}", s.fmt), val);
    if let Some(c) = engine::pat::captures(&out, "^([%+%-]?%d+%.%d+)(%D*)$") {
        let n = engine::pat::gsub(&engine::pat::gsub(&c[0], "0+$", ""), "%.$", "");
        out = format!("{n}{}", c[1]);
    }
    let mut out = format!("{color}{}", engine::pat::format_num_sep(&out));
    if let Some(o) = over_cap.filter(|&o| o > 0.0) {
        out.push_str(&engine::sf!("^x808080 (+%d%%)", o));
    }
    out
}

struct StatList {
    rows: Vec<Row>,
    warnings: Vec<String>,
    cap: f64,
}

impl StatList {
    /// `buildMode:AddDisplayStatList(statList, actor)`
    fn add(&mut self, stats: &[Stat], out: &Output, skill: &SkillInfo, aspects: [bool; 3]) {
        let c = Ctx {
            o: out,
            cap: self.cap,
        };
        for s in stats {
            if !match_flags(s, skill) {
                continue;
            }
            let label_color = s.color.unwrap_or("^7");
            let Some(stat) = s.stat else {
                if !matches!(self.rows.last(), Some(Row::Gap)) {
                    self.rows.push(Row::Gap);
                }
                continue;
            };
            let mut value = c.get(stat);
            if let (Some(child), Some(OutVal::Table(t))) = (s.child, value) {
                value = t.get(child);
            }
            let Some(value) = value else { continue };
            if stat == "SkillDPS" {
                if let OutVal::List(rows) = value {
                    self.skill_dps(rows);
                }
                continue;
            }
            let Some(v) = value.num() else { continue };
            let shown = match s.cond {
                Some(f) => f(v, &c),
                None => v != 0.0,
            };
            if shown && !s.hide {
                let mut over = s.over_cap.and_then(|k| c.n(k));
                if s.over_cap_cond.is_some_and(|f| !f(v, &c)) {
                    over = None;
                }
                let warned = c.t(&format!("{stat}Warning"))
                    || (s.warn_color && s.warn.and_then(|f| f(v, &c)).is_some());
                let mut text = format_stat(s, v, over, warned.then_some(ds::NEGATIVE));
                if s.suffix_cond.is_none_or(|f| f(v, &c)) {
                    let suffix = match s.suffix {
                        Suffix::None => None,
                        Suffix::Text(t) => Some(t.to_string()),
                        Suffix::Func(f) => f(v, &c),
                    };
                    if let Some(x) = suffix {
                        text.push_str(&format!("^x808080 ({x})"));
                    }
                }
                self.rows.push(Row::Text(
                    format!("{label_color}{}:", s.label),
                    Some(text),
                    false,
                ));
            }
            if let Some(w) = s.warn
                && s.cond.is_none_or(|f| f(v, &c))
                && let Some(w) = w(v, &c)
            {
                insert_new(&mut self.warnings, w);
            }
        }
        let protects = c.t("EnergyShieldProtectsMana");
        for (pool, key) in [
            ("Life", "LifeCostWarningList"),
            ("Mana", "ManaCostWarningList"),
            ("Rage", "RageCostWarningList"),
            ("Energy Shield", "ESCostWarningList"),
        ] {
            if let Some(names) = list_of(c.get(key)) {
                let pool = if protects && pool == "Mana" {
                    "Energy Shield and Mana"
                } else {
                    pool
                };
                let line = format!("You do not have enough {pool} to use: {}", names.join(", "));
                insert_new(&mut self.warnings, line);
            }
        }
        for (pool, key) in [
            ("Unreserved life", "LifePercentCostPercentCostWarningList"),
            ("Unreserved Mana", "ManaPercentCostPercentCostWarningList"),
        ] {
            if let Some(names) = list_of(c.get(key)) {
                let line = format!(
                    "You do not have enough {pool}% to use: {}",
                    names.join(", ")
                );
                insert_new(&mut self.warnings, line);
            }
        }
        if c.t("VixensTooMuchCastSpeedWarn") {
            insert_new(&mut self.warnings, "You may have too much cast speed or too little cooldown reduction to effectively use Vixen's Curse replacement".into());
        }
        if c.t("VixenModeNoVixenGlovesWarn") {
            insert_new(&mut self.warnings, "Vixen's calculation mode for Doom Blast is selected but you do not have Vixen's Entrapment Embroidered Gloves equipped".into());
        }
        let crab =
            c.n("CrabBarriersMax").unwrap_or(0.0) > 0.0 && c.n("CrabBarriers").unwrap_or(0.0) > 0.0;
        let count = usize::from(crab) + aspects.iter().filter(|&&a| a).count();
        if count > 1 {
            insert_new(
                &mut self.warnings,
                "You have more than one Aspect skill active".into(),
            );
        }
    }

    /// The Full DPS rows
    fn skill_dps(&mut self, rows: &[OutVal]) {
        for r in rows {
            let OutVal::Table(t) = r else { continue };
            let s = |k: &str| match t.get(k) {
                Some(OutVal::Str(s)) => Some(s.to_string()),
                _ => None,
            };
            let n = |k: &str| t.get(k).and_then(OutVal::num).unwrap_or(f64::NAN);
            let trigger = s("trigger")
                .filter(|t| !t.is_empty())
                .map(|t| format!("{} ({t}){}", ds::WARNING, ds::CUSTOM))
                .unwrap_or_default();
            let name = s("name").unwrap_or_default();
            let count = n("count");
            let lhs = if count >= 2.0 {
                format!(
                    "{}{}x {name}{trigger}:",
                    ds::CUSTOM,
                    engine::pat::num(count)
                )
            } else {
                format!("{}{name}{trigger}:", ds::CUSTOM)
            };
            self.rows.push(Row::Text(
                lhs,
                Some(format_stat(&ds::SKILL_DPS, n("dps") * count, None, None)),
                false,
            ));
            if let Some(p) = s("skillPart") {
                self.rows.push(Row::center(format!("^8{p}")));
            }
            if let Some(src) = s("source") {
                self.rows
                    .push(Row::center(format!("{}from {src}", ds::WARNING)));
            }
        }
    }

    /// The main skill's info messages, split where long
    fn info(&mut self, skill: &SkillInfo) {
        if let Some(m) = &skill.info_message {
            if m.len() > 40 {
                for line in engine::pat::gmatch1(m, "([^:]+)") {
                    self.rows.push(Row::center(format!("{}{line}", ds::CUSTOM)));
                }
            } else {
                self.rows.push(Row::center(format!("{}{m}", ds::CUSTOM)));
            }
        }
    }
}

fn list_of(v: Option<&OutVal>) -> Option<Vec<String>> {
    match v? {
        OutVal::List(l) => Some(
            l.iter()
                .map(|x| match x {
                    OutVal::Str(s) => s.to_string(),
                    _ => String::new(),
                })
                .collect(),
        ),
        _ => None,
    }
}

impl TypedSession<'_> {
    pub(super) fn sidebar(&self) -> Result<&Sidebar> {
        self.main
            .sidebar
            .as_ref()
            .ok_or_else(|| "no sidebar calculation".into())
    }

    pub(super) fn groups(&self) -> &[SocketGroup] {
        self.build
            .skills
            .as_ref()
            .map_or(&[], |s| s.socket_group_list())
    }

    /// `PassiveSpec:CountAllocNodes()`: points, ascendancy points and
    /// secondary ascendancy points used
    fn count_alloc_nodes(&self) -> (f64, f64, f64) {
        let spec = &self.build.spec;
        let secondary: Vec<&str> = spec
            .tree
            .alternate_ascendancies
            .iter()
            .flatten()
            .filter_map(|(_, a)| a.id.as_deref())
            .collect();
        let (mut used, mut asc, mut sec) = (0.0, 0.0, 0.0);
        for (_, &i) in spec.alloc_nodes.iter() {
            let n = spec.tree_node(i);
            if matches!(n.kind, NodeType::ClassStart | NodeType::AscendClassStart) {
                continue;
            }
            match n.ascendancy_name {
                Some(a) => {
                    if !n.is_multiple_choice_option {
                        asc += 1.0;
                        if secondary.contains(&a.as_str()) {
                            sec += 1.0;
                        }
                    }
                }
                None => used += 1.0,
            }
        }
        (used, asc, sec)
    }

    /// `buildMode:RefreshStatList()`: the rows and the warnings
    fn stat_list(&self) -> Result<(Vec<Row>, Vec<String>)> {
        let sb = self.sidebar()?;
        let cap = self
            .data
            .game
            .setup
            .misc
            .get("DotDpsCap")
            .copied()
            .unwrap_or(f64::NAN);
        let (stats, minion_stats) = display_stats();
        let mut l = StatList {
            rows: Vec::new(),
            warnings: Vec::new(),
            cap,
        };
        l.info(&sb.main_skill);
        if let Some(m) = &sb.main_skill.info_message2 {
            l.rows.push(Row::center(format!("^8{m}")));
        }
        if let (Some(out), Some(skill)) = (&self.main.minion, &sb.minion_skill) {
            l.rows.push(Row::left("^7Minion:".into()));
            if skill.info_message.is_some() {
                l.info(skill);
                if let Some(m) = &skill.info_message2 {
                    l.rows.push(Row::center(format!("^8{m}")));
                }
            }
            l.add(minion_stats, out, skill, sb.minion_aspects);
            l.rows.push(Row::Gap);
            l.rows.push(Row::left("^7Player:".into()));
        }
        if sb.main_skill.flag("disable") {
            l.rows.push(Row::left("^7Skill disabled:".into()));
            l.rows.push(match &sb.main_skill.disable_reason {
                Some(r) => Row::center(r.clone()),
                None => Row::Gap,
            });
        }
        l.add(stats, &self.main.player, &sb.main_skill, sb.aspects);
        let w = &sb.item_warnings;
        for j in &w.jewel_limit {
            insert_new(
                &mut l.warnings,
                format!("You are exceeding jewel limit with the jewel {j}"),
            );
        }
        for s in &w.socket_limit {
            insert_new(
                &mut l.warnings,
                format!("You have too many gems in your {s} slot"),
            );
        }
        if !w.missing_anoint.is_empty() {
            insert_new(
                &mut l.warnings,
                format!(
                    "You have eligible items missing an anoint: {}",
                    w.missing_anoint.join(", ")
                ),
            );
        }
        // `EstimatePlayerProgress`
        let (used, asc, sec) = self.count_alloc_nodes();
        let extra = self.extra_points().unwrap_or(0.0);
        if used > MAIN_TREE_POINTS + extra {
            insert_new(
                &mut l.warnings,
                "You have too many passive points allocated".into(),
            );
        }
        if asc > 8.0 {
            insert_new(
                &mut l.warnings,
                "You have too many ascendancy points allocated".into(),
            );
        }
        if sec > 8.0 {
            insert_new(
                &mut l.warnings,
                "You have too many secondary ascendancy points allocated".into(),
            );
        }
        Ok((l.rows, l.warnings))
    }

    fn extra_points(&self) -> Option<f64> {
        self.main
            .player
            .get(&Sym::new("ExtraPoints"))
            .and_then(OutVal::num)
    }

    /// `EstimatePlayerProgress` after a calculation: in the level's auto
    /// mode, the estimated level, with the configuration's mods built
    /// again for the next calculation
    pub(super) fn estimate_progress(&mut self) -> Result<()> {
        let level = self.estimated_level();
        if self.build.info.character_level_auto_mode && self.build.info.character_level != level {
            let data = self.data;
            self.build
                .set_character_level(level, &data.game.config, &data.parser)?;
        }
        Ok(())
    }

    /// The level `EstimatePlayerProgress` estimates from the points used
    pub fn estimated_level(&self) -> f64 {
        let (used, _, _) = self.count_alloc_nodes();
        let extra = self.extra_points().unwrap_or(0.0);
        let mut act = 0;
        loop {
            act += 1;
            let (lvl, quest) = ACTS[act - 1];
            let act_extra = if act > 2 { extra } else { 0.0 };
            let level = (used + 1.0 - quest - act_extra).max(lvl).min(100.0);
            if act == 11 || level <= ACTS[act].0 {
                return level;
            }
        }
    }

    /// `OnFrame`'s class drop-downs and `RefreshSkillSelectControls`
    pub(super) fn refresh_controls(&mut self) -> Result<()> {
        let spec = &self.build.spec;
        let tree = spec.tree;
        let c = &mut self.controls;
        if !c.level_typed {
            c.level.text = engine::pat::num(self.build.info.character_level);
        }
        // `UpdateClassDropdowns(treeVersion)`
        let mut classes: Vec<_> = tree.classes.iter().collect();
        classes.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        c.class.list = classes.iter().map(|(_, cl)| cl.name.to_string()).collect();
        if let Some(i) = classes
            .iter()
            .position(|(id, _)| Some(*id) == spec.cur_class_id)
        {
            c.class.sel = (i + 1) as f64;
            let mut asc: Vec<_> = classes[i].1.ascendancies.iter().collect();
            asc.sort_by_key(|(id, _)| *id);
            c.ascend.list = asc.iter().map(|(_, a)| a.name.to_string()).collect();
            if let Some(k) = asc
                .iter()
                .position(|(id, _)| Some(*id) == spec.cur_ascend_class_id)
            {
                c.ascend.sel = (k + 1) as f64;
            }
        }
        // `UpdateSecondaryAscendancyDropdown`
        let mut alt: Vec<(String, i64)> = tree
            .alternate_ascendancies
            .iter()
            .flatten()
            .filter(|(_, a)| a.id.is_some())
            .map(|(id, a)| (a.name.to_string(), *id))
            .collect();
        alt.sort();
        let mut entries = vec![("None".to_string(), 0)];
        entries.extend(alt);
        let selection = spec.cur_secondary_ascend_class_id.unwrap_or(0);
        if let Some(k) = entries.iter().position(|(_, id)| *id == selection) {
            c.secondary.sel = (k + 1) as f64;
        }
        c.secondary.enabled = entries.len() > 1;
        c.secondary.list = entries.into_iter().map(|(n, _)| n).collect();
        self.refresh_skill_controls()
    }

    /// `buildMode:RefreshSkillSelectControls(controls, mainSocketGroup, "")`
    fn refresh_skill_controls(&mut self) -> Result<()> {
        let main_group = self.build.info.main_socket_group;
        let sb = self.main.sidebar.as_ref().ok_or("no sidebar calculation")?;
        let mut c = std::mem::take(&mut self.controls.skill_select);
        let r = self.refresh_skill_select(&mut c, main_group, sb, false);
        self.controls.skill_select = c;
        r
    }

    /// `buildMode:RefreshSkillSelectControls(controls, mainGroup, suffix)`
    /// on a calculation's socket groups, with the suffix `Calcs` or none
    pub(super) fn refresh_skill_select(
        &self,
        c: &mut SkillSelect,
        main_group: f64,
        sb: &Sidebar,
        calcs: bool,
    ) -> Result<()> {
        let groups = self.groups();
        let env_main = sb.main_socket_group;
        c.socket_group.sel = main_group;
        c.socket_group.list = groups
            .iter()
            .map(|g| g.display_label.clone().unwrap_or_default())
            .collect();
        if groups.is_empty() {
            c.socket_group.list = vec!["<No skills added yet>".into()];
            for d in [
                &mut c.skill,
                &mut c.part,
                &mut c.minion,
                &mut c.minion_skill,
            ] {
                d.shown = false;
            }
            c.mine.shown = false;
            c.stage.shown = false;
            return Ok(());
        }
        let gi = (main_group as usize)
            .checked_sub(1)
            .filter(|&g| g < groups.len());
        let gi = gi.ok_or("attempt to index a nil value (local 'mainSocketGroup')")?;
        let group = &groups[gi];
        let display = sb.groups.get(gi).map_or(&[][..], |v| v);
        let mut main_active = if calcs {
            group.main_active_skill_calcs
        } else {
            group.main_active_skill
        }
        .unwrap_or(1.0);
        if gi + 1 == env_main && !display.is_empty() {
            main_active = main_active.min(display.len() as f64);
        }
        let labels = display
            .iter()
            .map(|d| self.skill_label(d, groups))
            .collect();
        c.skill.list = labels;
        c.skill.enabled = display.len() > 1;
        c.skill.sel = main_active;
        c.skill.shown = true;
        c.part.shown = false;
        c.mine.shown = false;
        c.stage.shown = false;
        c.minion.shown = false;
        c.minion_skill.shown = false;
        c.library = Some(false);
        if display.is_empty() {
            return Ok(());
        }
        let Some(skill) = (main_active as usize)
            .checked_sub(1)
            .and_then(|i| display.get(i))
        else {
            return Ok(());
        };
        let src = skill
            .src
            .and_then(|(g, k)| groups.get(g).and_then(|g| g.gem_list.get(k)));
        let ge = &skill.granted_effect;
        let parts = ge.parts.as_deref().unwrap_or(&[]);
        let field = |f: &engine::item::Fields, k: &str| f.get(k).cloned();
        if parts.len() > 1 {
            c.part.shown = true;
            c.part.list = parts
                .iter()
                .map(|p| match field(p, "name") {
                    Some(engine::mods::ModValue::Str(s)) => s.to_string(),
                    _ => String::new(),
                })
                .collect();
            c.part.sel = src
                .and_then(|g| {
                    if calcs {
                        g.skill_part_calcs
                    } else {
                        g.skill_part
                    }
                })
                .unwrap_or(1.0);
            let part = parts.get((c.part.sel as usize).wrapping_sub(1));
            if let Some(p) = part
                && field(p, "stages").is_some_and(|v| v.truthy())
            {
                c.stage.shown = true;
                let min = field(p, "stagesMin").and_then(|v| v.as_number());
                let n = src
                    .and_then(|g| {
                        if calcs {
                            g.skill_stage_count_calcs
                        } else {
                            g.skill_stage_count
                        }
                    })
                    .or(skill.stages_max)
                    .or(min)
                    .unwrap_or(1.0);
                c.stage.text = engine::pat::num(n);
            }
        }
        if skill.flags.contains("mine") {
            c.mine.shown = true;
            c.mine.text = src
                .and_then(|g| {
                    if calcs {
                        g.skill_mine_count_calcs
                    } else {
                        g.skill_mine_count
                    }
                })
                .map(engine::pat::num)
                .unwrap_or_default();
        }
        if skill.flags.contains("multiStage") && parts.len() <= 1 {
            c.stage.shown = true;
            let n = src
                .and_then(|g| {
                    if calcs {
                        g.skill_stage_count_calcs
                    } else {
                        g.skill_stage_count
                    }
                })
                .or(skill.stages_max)
                .or(skill.stages_min)
                .unwrap_or(1.0);
            c.stage.text = engine::pat::num(n);
        }
        if !skill.flags.contains("disable")
            && (ge.minion_list.is_some() || !skill.minion_list.is_empty())
        {
            let mut entries = Vec::new();
            let mut labels = Vec::new();
            let items = &self.build.items;
            if ge.minion_has_item_set {
                for &id in &items.item_set_order_list {
                    let title = items
                        .item_sets
                        .get(&(id as i64))
                        .and_then(|s| s.title.clone());
                    labels.push(title.unwrap_or_else(|| "Default Item Set".into()));
                    entries.push(MinionEntry::ItemSet(id));
                }
                let want = MinionEntry::ItemSet(
                    src.and_then(|g| {
                        if calcs {
                            g.skill_minion_item_set_calcs
                        } else {
                            g.skill_minion_item_set
                        }
                    })
                    .unwrap_or(1.0),
                );
                if let Some(k) = entries.iter().position(|e| *e == want) {
                    c.minion.sel = (k + 1) as f64;
                }
            } else {
                c.library = Some(ge.minion_list.as_ref().is_some_and(|l| l.is_empty()));
                for id in &skill.minion_list {
                    let name = self
                        .data
                        .game
                        .setup
                        .minions
                        .get(id)
                        .and_then(|m| m.str("name"))
                        .unwrap_or_default();
                    labels.push(name.to_string());
                    entries.push(MinionEntry::Minion(id.clone()));
                }
                if let Some(id) = src.and_then(|g| {
                    if calcs {
                        g.skill_minion_calcs.clone()
                    } else {
                        g.skill_minion.clone()
                    }
                }) && let Some(k) = entries
                    .iter()
                    .position(|e| *e == MinionEntry::Minion(id.clone()))
                {
                    c.minion.sel = (k + 1) as f64;
                }
            }
            c.minion.enabled = labels.len() > 1;
            c.minion.shown = true;
            match &skill.minion_skills {
                Some(names) => {
                    c.minion_skill.list = names.clone();
                    c.minion_skill.sel = src
                        .and_then(|g| {
                            if calcs {
                                g.skill_minion_skill_calcs
                            } else {
                                g.skill_minion_skill
                            }
                        })
                        .unwrap_or(1.0);
                    c.minion_skill.shown = true;
                    c.minion_skill.enabled = names.len() > 1;
                }
                None => {
                    c.minion_skill.list.clear();
                    labels.push("<No spectres in build>".into());
                    entries.push(MinionEntry::Text);
                }
            }
            c.minion.list = labels;
            c.minion_entries = entries;
        }
        Ok(())
    }

    /// A skill's entry in the main skill drop-down
    fn skill_label(&self, d: &engine::sidebar::DisplaySkill, groups: &[SocketGroup]) -> String {
        let source = d
            .src
            .and_then(|(g, k)| groups.get(g).and_then(|g| g.gem_list.get(k)))
            .and_then(|g| g.explode_source.clone());
        let named = match source {
            Some(ExplodeSource::Item(id)) => self
                .build
                .items
                .items
                .get(&(id as i64))
                .map(|i| (i.name.clone(), ds::rarity_color(&i.rarity))),
            Some(ExplodeSource::Node(id)) => self
                .build
                .spec
                .index
                .get(&id)
                .map(|&i| (self.build.spec.tree_node(i).dn.to_string(), ds::NORMAL)),
            None => None,
        };
        match named {
            Some((name, color)) => format!("From {color}{name}"),
            None => d.granted_effect.name.clone(),
        }
    }

    /// `state`: the build's sidebar
    pub fn state(&self) -> Result<J> {
        let (rows, warnings) = self.stat_list()?;
        let spec = &self.build.spec;
        let class = spec
            .cur_class_id
            .and_then(|id| spec.tree.classes.iter().find(|(i, _)| *i == id))
            .map(|(_, c)| c);
        let ascend = class.and_then(|c| {
            let id = spec.cur_ascend_class_id.unwrap_or(0);
            let find = |id| c.ascendancies.iter().find(|(i, _)| *i == id);
            find(id)
                .or_else(|| find(0))
                .map(|(_, a)| a.name.to_string())
        });
        let (used, asc, _) = self.count_alloc_nodes();
        let mut points = Map::new();
        points.insert("used".into(), num(used));
        points.insert("ascendancy".into(), num(asc));
        if let Some(x) = self.extra_points() {
            points.insert("max".into(), num(x));
        }
        let k = &self.controls;
        let c = &k.skill_select;
        let controls = json!({
            "build.characterLevel": k.level.json(),
            "build.classDrop": k.class.json(),
            "build.ascendDrop": k.ascend.json(),
            "build.secondaryAscendDrop": k.secondary.json(),
            "build.mainSocketGroup": c.socket_group.json(),
            "build.mainSkill": Drop { shown: c.skill.shown && c.socket_group.shown, ..c.skill.clone() }.json(),
            "build.mainSkillPart": c.part.json(),
            "build.mainSkillStageCount": c.stage.json(),
            "build.mainSkillMineCount": c.mine.json(),
            "build.mainSkillMinion": c.minion.json(),
            "build.mainSkillMinionSkill": c.minion_skill.json(),
        });
        let mut s = Map::new();
        s.insert("name".into(), json!(self.name));
        s.insert("level".into(), num(self.build.info.character_level));
        if let Some(c) = class {
            s.insert("className".into(), json!(&*c.name));
        }
        if let Some(a) = ascend {
            s.insert("ascendClassName".into(), json!(a));
        }
        s.insert("treeVersion".into(), json!(&*spec.tree.version));
        s.insert("points".into(), J::Object(points));
        s.insert(
            "stats".into(),
            J::Array(rows.iter().map(Row::json).collect()),
        );
        s.insert("warnings".into(), json!(warnings));
        s.insert("controls".into(), controls);
        s.insert("popups".into(), self.ui.popups.clone());
        s.insert("unsaved".into(), J::Bool(self.ui.unsaved));
        s.insert("outputRevision".into(), num(self.ui.output_revision));
        Ok(J::Object(s))
    }
}
