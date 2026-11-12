//! The Calcs tab: its sections (`Modules/CalcSections.lua`, evaluated at
//! build time, and the skill selection of `Classes/CalcsTab.lua`), which
//! cells show (`CalcsTab:CheckFlag`), their text (`Modules/CalcFormat.lua`)
//! and breakdowns (`Classes/CalcBreakdownControl.lua`), on the `CALCS`
//! calculation's [`engine::calcs::CalcsView`].
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::sidebar::Drop;
use super::{NOT_PORTED, Result, TypedSession, num};
use engine::actor::OutVal;
use engine::breakdown::{Breakdowns, Cell, Row};
use engine::calcs::{BuildOutput, CalcsRef, Output, ViewActor, lua_sort};
use engine::mods::{Cfg, Mod, ModType, ModValue, Names, Sym, TagKind};
use engine::pat;
use serde_json::{Map, Value as J, json};
use std::cell::RefCell;
use std::sync::OnceLock;

/// The skill selection section of `CalcsTab:CalcsTab`, in the form of the
/// evaluated `CalcSections.lua` (a table's sequence under `#`)
const SKILL_SELECT: &str = r##"{"#": [3, "SkillSelect", 1, "^xC8C8C8", {"#": [{"defaultCollapsed": false, "label": "View Skill Details", "data": {"#": [
  {"label": "Socket Group", "#": [{"controlName": "mainSocketGroup", "control": true}]},
  {"label": "Active Skill", "#": [{"controlName": "mainSkill", "control": true}]},
  {"label": "Skill Part", "playerFlag": "multiPart", "#": [{"controlName": "mainSkillPart", "control": true}]},
  {"label": "Skill Stages", "playerFlag": "multiStage", "#": [{"controlName": "mainSkillStageCount", "control": true}]},
  {"label": "Active Mines", "playerFlag": "mine", "#": [{"controlName": "mainSkillMineCount", "control": true}]},
  {"label": "Show Minion Stats", "flag": "haveMinion", "#": [{"controlName": "showMinion", "control": true}]},
  {"label": "Minion", "flag": "minion", "#": [{"controlName": "mainSkillMinion", "control": true}]},
  {"label": "Spectre Library", "flag": "spectre", "#": [{"controlName": "mainSkillMinionLibrary", "control": true}]},
  {"label": "Minion Skill", "flag": "haveMinion", "#": [{"controlName": "mainSkillMinionSkill", "control": true}]},
  {"label": "Calculation Mode", "#": [{"controlName": "mode", "control": true}]},
  {"label": "Aura and Buff Skills", "flag": "buffs", "textSize": 12, "#": [{"format": "{output:BuffList}", "#": [{"breakdown": "SkillBuffs"}]}]},
  {"label": "Combat Buffs", "flag": "combat", "textSize": 12, "#": [{"format": "{output:CombatList}"}]},
  {"label": "Curses and Debuffs", "flag": "effective", "textSize": 12, "#": [{"format": "{output:CurseList}", "#": [{"breakdown": "SkillDebuffs"}]}]}
]}}]}]}"##;

/// `buffModeDropList`
pub(super) const BUFF_MODES: [(&str, &str); 4] = [
    ("Unbuffed", "UNBUFFED"),
    ("Buffed", "BUFFED"),
    ("In Combat", "COMBAT"),
    ("Effective DPS", "EFFECTIVE"),
];

/// The tab's sections in order (`calcsTab.sectionList`)
pub fn sections() -> &'static [J] {
    static S: OnceLock<Vec<J>> = OnceLock::new();
    S.get_or_init(|| {
        let mut list = vec![serde_json::from_str(SKILL_SELECT).expect("skill select")];
        let data: J = serde_json::from_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/calc_sections.json"
        )))
        .expect("CalcSections.lua");
        list.extend(seq(&data).iter().cloned());
        list
    })
}

/// A table's sequence
fn seq(j: &J) -> &[J] {
    j.get("#").and_then(J::as_array).map_or(&[], |v| v)
}

fn str_of<'a>(j: &'a J, k: &str) -> Option<&'a str> {
    j.get(k).and_then(J::as_str)
}

/// A string field or a list of them (`modName`)
fn names_of(j: &J, k: &str) -> Vec<String> {
    match j.get(k) {
        Some(J::String(s)) => vec![s.clone()],
        Some(o) => seq(o)
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
        None => Vec::new(),
    }
}

/// `s:match("^(%a+)%.(%a+)$")`
fn split_ns(s: &str) -> Option<(String, String)> {
    pat::captures(s, "^(%a+)%.(%a+)$").map(|c| (c[0].clone(), c[1].clone()))
}

/// The actor a cell looks at, with its calculation
struct Ctx<'a> {
    r: &'a CalcsRef<'a>,
    actor: &'a ViewActor,
    player: &'a ViewActor,
}

/// `output[k]` as Lua indexes it: `ns.var` into a table
fn lookup<'a>(out: &'a Output, k: &str) -> Result<Option<&'a OutVal>> {
    match split_ns(k) {
        Some((ns, var)) => match out.get(&Sym::new(&ns)) {
            None | Some(OutVal::Bool(false)) => Ok(None),
            Some(OutVal::Table(t)) => Ok(t.get(&var)),
            Some(_) => Err(format!("attempt to index field '{ns}'")),
        },
        None => Ok(out.get(&Sym::new(k))),
    }
}

impl<'a> Ctx<'a> {
    fn output(&self) -> &'a Output {
        &self.r.actor(self.actor.actor).output
    }

    /// `CalcsTab:CheckFlag(obj, actor, player)`
    fn check(&self, obj: &J) -> Result<bool> {
        let flags = &self.actor.skill.flags;
        let has = |f: &str| flags.contains(f);
        if str_of(obj, "flag").is_some_and(|f| !has(f)) {
            return Ok(false);
        }
        if names_of(obj, "flagList").iter().any(|f| !has(f)) {
            return Ok(false);
        }
        if str_of(obj, "playerFlag").is_some_and(|f| !self.player.skill.flags.contains(f)) {
            return Ok(false);
        }
        if str_of(obj, "notFlag").is_some_and(has) {
            return Ok(false);
        }
        if names_of(obj, "notFlagList").iter().any(|f| has(f)) {
            return Ok(false);
        }
        if let Some(k) = str_of(obj, "haveOutput") {
            let v = match split_ns(k) {
                Some((ns, var)) => match self.output().get(&Sym::new(&ns)) {
                    None | Some(OutVal::Bool(false)) => None,
                    Some(OutVal::Table(t)) => t.get(&var),
                    Some(_) => return Err(format!("attempt to index field '{ns}'")),
                },
                None => self.output().get(&Sym::new(k)),
            };
            match v {
                None | Some(OutVal::Bool(false)) => return Ok(false),
                Some(OutVal::Num(n)) if *n == 0.0 => return Ok(false),
                _ => {}
            }
        }
        Ok(true)
    }

    /// `formatCalcStr(str, actor, colData)`
    fn format(&self, s: &str, cell: Option<&J>) -> Result<String> {
        let err = RefCell::new(None);
        let fail = |e: String| {
            err.borrow_mut().get_or_insert(e);
            None
        };
        let out = self.output();
        let s = pat::gsub_fn(s, "{output:([%a%.:]+)}", |c| match lookup(out, &c[0]) {
            Err(e) => fail(e),
            Ok(None) => Some(String::new()),
            Ok(Some(v)) => match v {
                OutVal::Num(n) => Some(pat::num(*n)),
                OutVal::Str(s) => Some(s.to_string()),
                OutVal::Bool(false) => None,
                _ => fail("invalid replacement value".into()),
            },
        });
        let s = pat::gsub_fn(&s, "{(%d+):output:([%a%.:]+)}", |c| {
            let v = match lookup(out, &c[1]) {
                Err(e) => return fail(e),
                Ok(None | Some(OutVal::Bool(false))) => 0.0,
                Ok(Some(OutVal::Num(n))) => *n,
                Ok(Some(OutVal::Str(s))) => match pat::to_number(s) {
                    Some(n) => n,
                    None => return fail("attempt to perform arithmetic on a string value".into()),
                },
                Ok(Some(_)) => {
                    return fail("attempt to perform arithmetic on a table value".into());
                }
            };
            Some(calc_val(v, &c[0]))
        });
        let s = pat::gsub_fn(&s, "{(%d+):mod:([%d,]+)}", |c| {
            match self.mod_total(cell, &c[1]) {
                Ok(None) => Some("?".into()),
                Ok(Some(v)) => Some(calc_val(v, &c[0])),
                Err(e) => fail(e),
            }
        });
        match err.into_inner() {
            Some(e) => Err(e),
            None => Ok(s),
        }
    }

    /// The store and cfg an entry's mods are queried with
    fn store_cfg(&self, e: &J) -> Result<(engine::mods::StoreId, Cfg)> {
        let skill = &self.actor.skill;
        let cfg_name = str_of(e, "cfg");
        let cfg = cfg_name
            .and_then(|c| skill.cfg(c))
            .cloned()
            .unwrap_or_default();
        let store = if e.get("enemy").is_some_and(truthy) {
            let enemy = self.r.actor(self.actor.actor).enemy;
            enemy.map(|a| self.r.actor(a).store)
        } else {
            None
        };
        let store = match (store, cfg_name) {
            (Some(s), _) => s,
            (None, Some(_)) => skill
                .list
                .ok_or_else(|| format!("{NOT_PORTED}: a main skill copied from a mirage"))?,
            (None, None) => self.r.actor(self.actor.actor).store,
        };
        Ok((store, cfg))
    }

    /// The `{p:mod:1,2}` total of a cell, `None` for `?`
    fn mod_total(&self, cell: Option<&J>, list: &str) -> Result<Option<f64>> {
        let cell = cell.ok_or("attempt to index local 'colData' (a nil value)")?;
        let entries = seq(cell);
        let nums: Vec<usize> = pat::gmatch1(list, "%d+")
            .iter()
            .filter_map(|n| n.parse().ok())
            .collect();
        let entry = |n: usize| n.checked_sub(1).and_then(|i| entries.get(i));
        let Some(typ) = nums
            .first()
            .and_then(|&n| entry(n))
            .and_then(|e| str_of(e, "modType"))
        else {
            return Ok(None);
        };
        let more = typ == "MORE";
        let mut total = if more { 1.0 } else { 0.0 };
        for n in nums {
            let Some(e) = entry(n) else { break };
            let (store, mut cfg) = self.store_cfg(e)?;
            if str_of(e, "cfg").is_none() {
                if let Some(s) = str_of(e, "modSource") {
                    cfg.source = Some(s.into());
                }
                if let Some(a) = str_of(e, "actor") {
                    cfg.actor = Some(Sym::new(a));
                }
            } else if e.get("modSource").is_some() || e.get("actor").is_some() {
                return Err(format!("{NOT_PORTED}: a cell's source on a skill's cfg"));
            }
            let names: Vec<Sym> = names_of(e, "modName").iter().map(|n| Sym::new(n)).collect();
            let q = self.r.query();
            let t = str_of(e, "modType").ok_or("attempt to call Sum without a mod type")?;
            let v = match t {
                "MORE" => q.more(store, Some(&cfg), &names)?,
                "OVERRIDE" => match q.override_value(store, Some(&cfg), &names)? {
                    Some(ModValue::Number(n)) => n,
                    _ => return Err("attempt to perform arithmetic on an override".into()),
                },
                "BASE" | "INC" => q.sum(store, ModType::new(t), Some(&cfg), &names)?,
                t => return Err(format!("{NOT_PORTED}: mod type {t} in a cell")),
            };
            if more {
                total *= v;
            } else {
                total += v;
            }
        }
        if more {
            total = (total - 1.0) * 100.0;
        }
        Ok(Some(total))
    }
}

fn truthy(j: &J) -> bool {
    !matches!(j, J::Null | J::Bool(false))
}

/// `formatCalcVal(val, p)`
fn calc_val(v: f64, p: &str) -> String {
    let p: f64 = p.parse().unwrap_or(0.0);
    pat::format_num_sep(&pat::num(engine::calc::round(v, p)))
}

/// `CalcBreakdownControl:FormatModName`
fn mod_name(s: &str) -> String {
    pat::gsub(
        &pat::gsub(s, "([%l%d]:?)(%u)", "%1 %2"),
        "(%l)(%d)",
        "%1 %2",
    )
}

fn names_text(n: &Names, sep: &str) -> String {
    match n {
        Names::One(s) => s.as_str().to_string(),
        Names::List(l) => l.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(sep),
    }
}

/// `FormatVarNameOrList(var, varList)`
fn var_names(n: Option<&Names>) -> Result<String> {
    n.map(|n| mod_name(&names_text(n, " / ")))
        .ok_or_else(|| "attempt to concatenate a nil value".into())
}

/// `FormatModBase(mod, base)`
fn mod_base(m: &Mod, base: f64) -> String {
    if m.typ == ModType::Base {
        engine::breakdown::lua_format("%+g", &[engine::breakdown::Arg::Num(base.abs())])
    } else {
        format!("{}%", pat::num(base.abs()))
    }
}

/// `FormatModValue(value, modType)`
fn mod_value(v: &ModValue, typ: ModType) -> Result<String> {
    use engine::breakdown::{Arg, lua_format};
    let n = || {
        v.as_number()
            .ok_or_else(|| format!("attempt to perform arithmetic on {v:?}"))
    };
    Ok(match typ {
        ModType::Base => lua_format("%+g base", &[Arg::Num(n()?)]),
        ModType::Inc | ModType::More => {
            let n = n()?;
            let (up, down) = if typ == ModType::Inc {
                ("increased", "reduced")
            } else {
                ("more", "less")
            };
            if n >= 0.0 {
                format!("{}% {up}", pat::num(n))
            } else {
                format!("{}% {down}", pat::num(-n))
            }
        }
        ModType::Override => format!("Override: {}", value_text(v)?),
        ModType::Flag => if v.truthy() { "True" } else { "False" }.into(),
        ModType::List => match v {
            ModValue::Mod(m) => format!("Modifier: {}", mod_name(m.name.as_str())),
            ModValue::Record(r) => match r.get("mod") {
                Some(ModValue::Mod(m)) => format!("Modifier: {}", mod_name(m.name.as_str())),
                _ => "?".into(),
            },
            _ => "?".into(),
        },
        _ => value_text(v)?,
    })
}

/// `tostring` of a mod value that is a number or a string
fn value_text(v: &ModValue) -> Result<String> {
    match v {
        ModValue::Number(n) => Ok(pat::num(*n)),
        ModValue::Str(s) => Ok(s.to_string()),
        ModValue::Bool(b) => Ok(b.to_string()),
        v => Err(format!("{NOT_PORTED}: a breakdown value {v:?}")),
    }
}

/// A table section's row: its cells by column key, as `tostring` gives
/// them
type TextRow = Vec<(String, String)>;

/// A section of the breakdown
enum Section {
    Text(Vec<String>),
    Radius,
    Table {
        label: Option<String>,
        footer: Option<String>,
        cols: Vec<(&'static str, String)>,
        rows: Vec<TextRow>,
    },
}

impl Section {
    fn json(&self) -> Option<J> {
        match self {
            Section::Text(lines) => Some(json!({ "type": "text", "lines": lines })),
            Section::Radius => None,
            Section::Table {
                label,
                footer,
                cols,
                rows,
            } => {
                let mut m = Map::new();
                m.insert("type".into(), json!("table"));
                if let Some(l) = label {
                    m.insert("label".into(), json!(l));
                }
                if let Some(f) = footer {
                    m.insert("footer".into(), json!(f));
                }
                let cols: Vec<J> = cols
                    .iter()
                    .map(|(k, l)| json!({ "key": k, "label": l }))
                    .collect();
                m.insert("cols".into(), list(cols));
                let rows = rows
                    .iter()
                    .map(|r| {
                        let o: Map<String, J> =
                            r.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
                        object(o)
                    })
                    .collect();
                m.insert("rows".into(), list(rows));
                Some(J::Object(m))
            }
        }
    }
}

/// An array as `dkjson` writes it: empty tables as `[]`
fn list(v: Vec<J>) -> J {
    J::Array(v)
}

fn object(m: Map<String, J>) -> J {
    if m.is_empty() {
        json!([])
    } else {
        J::Object(m)
    }
}

fn cell_text(c: &Cell) -> Option<String> {
    match c {
        Cell::Str(s) => Some(s.clone()),
        Cell::Num(n) => Some(pat::num(*n)),
        Cell::Bool(b) => Some(b.to_string()),
        Cell::ItemTooltip(_) => None,
    }
}

/// `a > b` of two cells, as Lua compares them
fn cell_gt(a: Option<&Cell>, b: Option<&Cell>) -> Result<bool> {
    match (a, b) {
        (Some(Cell::Num(x)), Some(Cell::Num(y))) => Ok(x > y),
        (Some(Cell::Str(x)), Some(Cell::Str(y))) => Ok(x > y),
        _ => Err("attempt to compare two values of different types".into()),
    }
}

fn get<'r>(row: &'r Row, k: &str) -> Option<&'r Cell> {
    row.iter().find(|(c, _)| *c == k).map(|(_, v)| v)
}

/// `table.sort` with a comparator that may fail
fn sort_by<T>(v: &mut [T], gt: impl Fn(&T, &T) -> Result<bool>) -> Result<()> {
    let err = RefCell::new(None);
    lua_sort(v, |a, b| {
        gt(a, b).unwrap_or_else(|e| {
            err.borrow_mut().get_or_insert(e);
            false
        })
    });
    err.into_inner().map_or(Ok(()), Err)
}

/// The rows of a table with these columns
fn table_rows(rows: &[Row], cols: &[(&'static str, String)]) -> Vec<TextRow> {
    rows.iter()
        .map(|r| {
            cols.iter()
                .filter_map(|(k, _)| Some((k.to_string(), cell_text(get(r, k)?)?)))
                .collect()
        })
        .collect()
}

/// A mod of a breakdown's mod list or a tabulated store, with its value
struct ModRow {
    m: Mod,
    value: ModValue,
}

impl TypedSession<'_> {
    /// The Calcs tab's calculation, made when first asked for
    fn calcs_output(&mut self) -> Result<&mut BuildOutput> {
        if self.calcs.is_none() {
            let data = self.data.calc_data();
            let out = engine::calcs::build_output(
                &mut self.build,
                &data,
                engine::calc_env::CalcMode::Calcs,
                Some(&self.main),
            )?;
            self.calcs = Some(out);
        }
        Ok(self.calcs.as_mut().unwrap())
    }

    /// Runs `f` on the Calcs tab's calculation, with the actor it shows
    fn with_calcs<R>(
        &mut self,
        f: impl FnOnce(&Ctx, &mut Breakdowns, &TypedSession) -> Result<R>,
    ) -> Result<R> {
        self.calcs_output()?;
        let mut out = self.calcs.take().unwrap();
        let r = (|| {
            let view = out.view.as_mut().ok_or("no Calcs tab view")?;
            let show_minion = self.build.calcs.show_minion && view.minion.is_some();
            let mut bd = if show_minion {
                out.minion_breakdown.take()
            } else {
                out.breakdown.take()
            }
            .unwrap_or_default();
            let data = self.data.calc_data();
            let r = view.with(&self.build.items, &data, |r| {
                let actor = match (&r.view.minion, show_minion) {
                    (Some(m), true) => m,
                    _ => &r.view.player,
                };
                let ctx = Ctx {
                    r,
                    actor,
                    player: &r.view.player,
                };
                f(&ctx, &mut bd, self)
            });
            if show_minion {
                out.minion_breakdown = Some(bd);
            } else {
                out.breakdown = Some(bd);
            }
            r
        })();
        self.calcs = Some(out);
        r
    }

    /// `calcs`: the tab's sections with the cells shown, formatted
    pub fn calcs(&mut self) -> Result<J> {
        let show_minion = self.build.calcs.show_minion;
        let mut rows_enabled = Vec::new();
        let r = self.with_calcs(|ctx, _, _| {
            let mut shown = Vec::new();
            for (si, section) in sections().iter().enumerate() {
                let s = seq(section);
                let subs_data = s.get(4).map(seq).unwrap_or(&[]);
                let first = subs_data.first().and_then(|u| u.get("data"));
                let flags = json!({
                    "flag": first.and_then(|d| d.get("flag")),
                    "notFlag": first.and_then(|d| d.get("notFlag")),
                });
                if !ctx.check(&flags)? {
                    continue;
                }
                let mut subs = Vec::new();
                for (ui, sub) in subs_data.iter().enumerate() {
                    let data = sub.get("data").unwrap_or(&J::Null);
                    let mut rows = Vec::new();
                    for (ri, row) in seq(data).iter().enumerate() {
                        let enabled = ctx.check(row)?;
                        for c in seq(row) {
                            if let Some(name) = str_of(c, "controlName") {
                                rows_enabled.push((name, enabled));
                            }
                        }
                        if !enabled {
                            continue;
                        }
                        let mut cells = Vec::new();
                        for (ci, col) in seq(row).iter().enumerate() {
                            if let Some(name) = str_of(col, "controlName") {
                                cells.push(json!({ "control": format!("calcs.{name}") }));
                            } else if let Some(f) = str_of(col, "format")
                                && ctx.check(col)?
                            {
                                let mut c = Map::new();
                                c.insert("text".into(), json!(ctx.format(f, Some(col))?));
                                if !seq(col).is_empty() {
                                    c.insert(
                                        "breakdown".into(),
                                        json!([si + 1, ui + 1, ri + 1, ci + 1]),
                                    );
                                }
                                cells.push(J::Object(c));
                            } else {
                                cells.push(json!([]));
                            }
                        }
                        let mut r = Map::new();
                        for k in ["label", "color"] {
                            if let Some(v) = str_of(row, k) {
                                r.insert(k.into(), json!(v));
                            }
                        }
                        r.insert("cells".into(), J::Array(cells));
                        rows.push(J::Object(r));
                    }
                    if rows.is_empty() {
                        continue;
                    }
                    let mut u = Map::new();
                    if let Some(l) = str_of(sub, "label") {
                        u.insert("label".into(), json!(l));
                    }
                    if let Some(x) = str_of(data, "extra") {
                        u.insert("extra".into(), json!(ctx.format(x, None)?));
                    }
                    if sub.get("defaultCollapsed").is_some_and(truthy) {
                        u.insert("collapsed".into(), J::Bool(true));
                    }
                    u.insert("rows".into(), J::Array(rows));
                    subs.push(J::Object(u));
                }
                if subs.is_empty() {
                    continue;
                }
                let mut o = Map::new();
                for (k, i) in [("id", 1), ("group", 2), ("color", 3)] {
                    match s.get(i) {
                        Some(J::Number(n)) => o.insert(k.into(), num(n.as_f64().unwrap_or(0.0))),
                        Some(v @ J::String(_)) => o.insert(k.into(), v.clone()),
                        _ => None,
                    };
                }
                o.insert("subs".into(), J::Array(subs));
                shown.push(J::Object(o));
            }
            Ok(json!({ "sections": shown, "showMinion": show_minion }))
        })?;
        // The skill selection section's `updateFunc`
        self.calcs_rows = rows_enabled;
        let mut c = std::mem::take(&mut self.calcs_select);
        let view = self.calcs.as_ref().and_then(|o| o.view.as_ref());
        let res = match view {
            Some(v) => {
                self.refresh_skill_select(&mut c, self.build.calcs.skill_number, &v.skills, true)
            }
            None => Ok(()),
        };
        self.calcs_select = c;
        res.map(|_| r)
    }

    /// The Calcs tab's controls (`calcs.<name>`) as the translated
    /// session's `controls` describes them, after `calcs`
    pub fn calcs_control(&self, name: &str) -> Option<J> {
        let c = &self.calcs_select;
        let row = || {
            self.calcs_rows
                .iter()
                .find(|(n, _)| *n == name)
                .is_some_and(|(_, e)| *e)
        };
        Some(match name {
            "mainSocketGroup" => Drop {
                shown: row(),
                ..c.socket_group.clone()
            }
            .json(),
            "mainSkill" => c.skill.json(),
            "mainSkillPart" => c.part.json(),
            "mainSkillStageCount" => c.stage.json(),
            "mainSkillMineCount" => c.mine.json(),
            "mainSkillMinion" => c.minion.json(),
            "mainSkillMinionSkill" => c.minion_skill.json(),
            "mainSkillMinionLibrary" => json!({
                "class": "ButtonControl",
                "shown": c.library.unwrap_or_else(row),
                "enabled": true,
                "label": "Manage Spectres...",
            }),
            "showMinion" => json!({
                "class": "CheckBoxControl",
                "shown": row(),
                "enabled": true,
                "state": self.build.calcs.show_minion,
                "tooltip": "Show stats for the minion instead of the player.",
            }),
            "mode" => {
                let sel = BUFF_MODES
                    .iter()
                    .position(|(_, m)| *m == self.build.calcs.buff_mode)
                    .map_or(1, |i| i + 1);
                json!({
                    "class": "DropDownControl",
                    "shown": row(),
                    "enabled": true,
                    "list": BUFF_MODES.iter().map(|(l, _)| *l).collect::<Vec<_>>(),
                    "sel": sel,
                    "tooltip": include_str!(concat!(env!("OUT_DIR"), "/calcs_mode_tooltip.txt")),
                })
            }
            _ => return None,
        })
    }

    /// `breakdown`: `CalcBreakdownControl:SetBreakdownData` for a cell
    pub fn breakdown(&mut self, cell: &[usize]) -> Result<J> {
        let col = match cell {
            [si, ui, ri, ci] => (|| {
                let s = sections().get(si.checked_sub(1)?)?;
                let sub = seq(seq(s).get(4)?).get(ui.checked_sub(1)?)?;
                let row = seq(sub.get("data")?).get(ri.checked_sub(1)?)?;
                seq(row).get(ci.checked_sub(1)?)
            })(),
            _ => None,
        };
        let col = col.ok_or("no calcs cell")?;
        self.with_calcs(|ctx, bd, s| {
            let mut out = Vec::new();
            for e in seq(col) {
                if !ctx.check(e)? {
                    continue;
                }
                if let Some(name) = str_of(e, "breakdown") {
                    s.breakdown_section(ctx, bd, e, name, &mut out)?;
                } else if e.get("modName").is_some() {
                    s.mod_section(ctx, e, None, &mut out)?;
                }
            }
            let sections: Vec<J> = out.iter().filter_map(Section::json).collect();
            Ok(json!({ "sections": sections }))
        })
    }

    /// `AddBreakdownSection`
    fn breakdown_section(
        &self,
        ctx: &Ctx,
        bd: &mut Breakdowns,
        e: &J,
        name: &str,
        out: &mut Vec<Section>,
    ) -> Result<()> {
        let Some(b) = bd.map.get_mut(name) else {
            return Ok(());
        };
        if !b.lines.is_empty() {
            out.push(Section::Text(b.lines.clone()));
        }
        if b.radius.is_some() {
            out.push(Section::Radius);
        }
        if let (Some(rows), Some(cols)) = (&b.rows, &b.cols)
            && !rows.is_empty()
        {
            let mut rows = rows.clone();
            let key = cols.first().map_or("", |c| c.key);
            if b.source.is_some() {
                sort_by(&mut rows, |a, b| match get(a, "reqNum") {
                    Some(_) => cell_gt(get(a, "reqNum"), get(b, "reqNum")),
                    None => cell_gt(get(a, key), get(b, key)),
                })?;
            }
            let cols: Vec<_> = cols.iter().map(|c| (c.key, c.label.clone())).collect();
            out.push(Section::Table {
                label: b.label.clone(),
                footer: b.footer.clone(),
                rows: table_rows(&rows, &cols),
                cols,
            });
        }
        if let Some(res) = &b.reservations
            && !res.is_empty()
        {
            let s = |o: &Option<String>| Cell::from(o.clone());
            let rows: Vec<Row> = res
                .iter()
                .map(|r| {
                    vec![
                        ("skillName", Cell::from(r.skill_name.clone())),
                        ("base", r.base.clone()),
                        ("mult", s(&r.mult)),
                        ("more", s(&r.more)),
                        ("inc", s(&r.inc)),
                        ("efficiency", s(&r.efficiency)),
                        ("efficiencyMore", s(&r.efficiency_more)),
                        ("total", r.total.clone()),
                    ]
                })
                .collect();
            let cols = cols(&[
                ("Skill", "skillName"),
                ("Base", "base"),
                ("MCM", "mult"),
                ("More/less", "more"),
                ("Inc/red", "inc"),
                ("Efficiency", "efficiency"),
                ("Efficiency More/less", "efficiencyMore"),
                ("Reservation", "total"),
            ]);
            out.push(Section::Table {
                label: None,
                footer: None,
                rows: table_rows(&rows, &cols),
                cols,
            });
        }
        if let Some(rows) = &b.damage_types
            && !rows.is_empty()
        {
            let cols = cols(&[
                ("From", "source"),
                ("Base", "base"),
                ("Inc/red", "inc"),
                ("More/less", "more"),
                ("Converted Damage", "convSrc"),
                ("Total", "total"),
                ("Conversion", "convDst"),
                ("Gain", "gainDst"),
            ]);
            out.push(Section::Table {
                label: None,
                footer: None,
                rows: table_rows(rows, &cols),
                cols,
            });
        }
        if let Some(slots) = &mut b.slots
            && !slots.is_empty()
        {
            let gear_only = e.get("gearOnly").is_some_and(truthy);
            let tab = &self.build.items;
            let mut picked: Vec<usize> = if gear_only {
                (0..slots.len())
                    .filter(|&i| {
                        slots[i]
                            .item
                            .and_then(|id| tab.item(id))
                            .is_some_and(|it| it.armour_data.is_some())
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if gear_only {
                lua_sort(&mut picked, |&a, &b| slots[a].base > slots[b].base);
            } else {
                // Upstream sorts the breakdown's own rows
                lua_sort(slots, |a, b| a.base > b.base);
                picked = (0..slots.len()).collect();
            }
            let cols = if gear_only {
                cols(&[
                    ("Value", "base"),
                    ("Source", "source"),
                    ("Name", "sourceLabel"),
                ])
            } else {
                cols(&[
                    ("Base", "base"),
                    ("Inc/red", "inc"),
                    ("More/less", "more"),
                    ("Total", "total"),
                    ("Source", "source"),
                    ("Name", "sourceLabel"),
                ])
            };
            let rows: Vec<Row> = picked
                .iter()
                .map(|&i| {
                    let s = &slots[i];
                    let label = match s.item.and_then(|id| tab.item(id)) {
                        Some(it) => Some(format!(
                            "{}{}",
                            engine::breakdown::color::rarity(&it.rarity).unwrap_or(""),
                            it.name
                        )),
                        None => s.source_name.clone(),
                    };
                    let mut r: Row = vec![
                        ("base", Cell::Num(s.base)),
                        ("inc", Cell::from(s.inc.clone())),
                        ("more", Cell::from(s.more.clone())),
                        ("total", Cell::from(s.total.clone())),
                        ("source", Cell::from(s.source.clone())),
                    ];
                    if let Some(l) = label {
                        r.push(("sourceLabel", Cell::Str(l)));
                    }
                    r
                })
                .collect();
            out.push(Section::Table {
                label: None,
                footer: None,
                rows: table_rows(&rows, &cols),
                cols,
            });
        }
        if let Some(mods) = &b.mod_list
            && !mods.is_empty()
        {
            let rows = mods
                .iter()
                .map(|m| ModRow {
                    m: m.m.clone(),
                    value: m.value.clone(),
                })
                .collect();
            self.mod_section(ctx, e, Some(rows), out)?;
        }
        Ok(())
    }

    /// `AddModSection`: a table of mods, a store's or the given ones
    fn mod_section(
        &self,
        ctx: &Ctx,
        e: &J,
        given: Option<Vec<ModRow>>,
        out: &mut Vec<Section>,
    ) -> Result<()> {
        let mod_type = str_of(e, "modType");
        let names = names_of(e, "modName");
        let multiple = matches!(e.get("modName"), Some(J::Object(_)));
        let provided = given.is_some();
        let mut rows = match given {
            Some(r) => r,
            None => {
                let (store, mut cfg) = ctx.store_cfg(e)?;
                cfg.source = str_of(e, "modSource").map(Into::into);
                cfg.actor = str_of(e, "actor").map(Sym::new);
                let syms: Vec<Sym> = names.iter().map(|n| Sym::new(n)).collect();
                ctx.r
                    .query()
                    .tabulate(store, mod_type.map(ModType::new), Some(&cfg), &syms)?
                    .into_iter()
                    .map(|r| ModRow {
                        m: r.m.clone(),
                        value: r.value,
                    })
                    .collect::<Vec<_>>()
            }
        };
        if rows.is_empty() {
            return Ok(());
        }
        let by_value = |a: &ModRow, b: &ModRow| -> Result<bool> {
            let (x, y) = (a.m.name.as_str(), b.m.name.as_str());
            Ok(x > y
                || (x == y
                    && match (&a.value, &b.value) {
                        (ModValue::Number(p), ModValue::Number(q)) => p > q,
                        _ => false,
                    }))
        };
        if !provided && mod_type.is_none() {
            sort_by(&mut rows, |a, b| {
                if a.m.typ == b.m.typ {
                    by_value(a, b)
                } else {
                    Ok(a.m.typ.name() < b.m.typ.name())
                }
            })?;
        } else {
            sort_by(&mut rows, by_value)?;
        }
        if !provided
            && str_of(e, "modSource").is_none()
            && rows.iter().any(|r| r.m.source.is_none())
        {
            return Err("attempt to index field 'source' (a nil value)".into());
        }
        let mut text_rows = Vec::with_capacity(rows.len());
        for r in &rows {
            let mut t: TextRow = Vec::new();
            let display = match mod_type {
                None => mod_value(&r.value, r.m.typ)?,
                Some(_) => {
                    let v = r
                        .value
                        .as_number()
                        .ok_or("attempt to perform arithmetic on a value")?;
                    pat::num(engine::calc::round(v, 2.0))
                }
            };
            t.push(("displayValue".into(), display));
            if provided || multiple {
                t.push(("name".into(), mod_name(r.m.name.as_str())));
            }
            if r.m.flags != 0 || r.m.keyword_flags != 0 {
                t.push(("flags".into(), self.flag_names(&r.m)));
            }
            if let Some(tags) = self.tags_text(&r.m)? {
                t.push(("tags".into(), tags));
            }
            let source = r.m.source.as_deref().unwrap_or("");
            let source_type = pat::capture(source, "([^:]+)").unwrap_or_default();
            t.push(("source".into(), source_type.clone()));
            if let Some(n) = self.source_name(&source_type, source)? {
                t.push(("sourceName".into(), n));
            }
            text_rows.push(t);
        }
        out.push(Section::Table {
            label: str_of(e, "label").map(String::from),
            footer: None,
            cols: cols(&[
                ("Value", "displayValue"),
                ("Stat", "name"),
                ("Skill types", "flags"),
                ("Notes", "tags"),
                ("Source", "source"),
                ("Source Name", "sourceName"),
            ]),
            rows: text_rows,
        });
        Ok(())
    }

    /// The mod's flag names, sorted and joined
    fn flag_names(&self, m: &Mod) -> String {
        let setup = &self.data.game.setup;
        let mut tables = vec![(m.flags, &setup.mod_flags)];
        if m.keyword_flags == m.flags {
            tables[0].1 = &setup.keyword_flags;
        } else {
            tables.push((m.keyword_flags, &setup.keyword_flags));
        }
        let mut names: Vec<&str> = Vec::new();
        for (flags, src) in tables {
            for (name, &val) in src {
                if flags & val == val {
                    names.push(name);
                }
            }
        }
        names.sort();
        names.join(", ")
    }

    /// The row's `sourceName` for a mod from this source
    fn source_name(&self, kind: &str, source: &str) -> Result<Option<String>> {
        let rest = |p: &str| pat::capture(source, p);
        Ok(match kind {
            "Item" => rest("Item:(%d+):.+")
                .and_then(|id| self.build.items.item(id.parse().ok()?))
                .map(|it| {
                    format!(
                        "{}{}",
                        engine::breakdown::color::rarity(&it.rarity).unwrap_or(""),
                        it.name
                    )
                }),
            "Tree" => match rest("Tree:(%d+)") {
                Some(id) => {
                    let id: u32 = id.parse().map_err(|_| "bad node id")?;
                    let spec = &self.build.spec;
                    let node = match spec.index.get(&id) {
                        Some(&i) => Some(spec.tree_node(i)),
                        None => spec.tree.node(id).or_else(|| {
                            self.data
                                .tree(self.data.game.latest_tree_version())
                                .ok()?
                                .node(id)
                        }),
                    };
                    Some(node.ok_or("attempt to index local 'node'")?.dn.to_string())
                }
                None if rest("Tree:([%w_]+)").is_some() => {
                    return Err(format!("{NOT_PORTED}: tattoo and legion sources"));
                }
                None => None,
            },
            "Skill" => {
                let id = rest("Skill:(.+)").unwrap_or_default();
                let skill = self.data.game.skills.skills.get(&id);
                Some(skill.ok_or("attempt to index a nil value")?.name.clone())
            }
            "Pantheon" => rest("Pantheon:(.+)"),
            "Spectre" => rest("Spectre:(.+)"),
            "Custom" => rest("Custom:(.+)"),
            _ => None,
        })
    }

    /// The row's `tags`: each tag described
    fn tags_text(&self, m: &Mod) -> Result<Option<String>> {
        if m.tags.is_empty() {
            return Ok(None);
        }
        let mut base_val = m.value.as_number().map(|v| format!("{} ", mod_base(m, v)));
        let base_of = |b: Option<&String>| -> Result<String> {
            b.cloned()
                .ok_or_else(|| "attempt to concatenate a boolean value".into())
        };
        let mut tags: Option<String> = None;
        for tag in &m.tags {
            let cap = |s: &str| {
                let mut c = s.chars();
                c.next().map_or(String::new(), |f| {
                    f.to_uppercase().collect::<String>() + c.as_str()
                })
            };
            let neg = |n: bool| if n { "Not " } else { "" };
            let desc = match &tag.kind {
                TagKind::Condition {
                    vars,
                    neg: n,
                    actor,
                }
                | TagKind::ActorCondition {
                    actor,
                    vars: Some(vars),
                    neg: n,
                } => {
                    let a = actor
                        .map(|a| format!("{} ", cap(a.as_str())))
                        .unwrap_or_default();
                    format!("{a}Condition: {}{}", neg(*n), var_names(Some(vars))?)
                }
                TagKind::ActorCondition {
                    actor, vars: None, ..
                } => {
                    let a = actor
                        .map(|a| format!("{} ", cap(a.as_str())))
                        .unwrap_or_default();
                    format!("{a}Condition")
                }
                TagKind::Multiplier(x) => {
                    let base = match x.base {
                        Some(b) => format!("{} + {} ", mod_base(m, b), abs_value(m)?),
                        None => base_of(base_val.as_ref())?,
                    };
                    base_val = Some(String::new());
                    let div = x
                        .div
                        .map(|d| format!("{} ", pat::num(d)))
                        .unwrap_or_default();
                    format!("{base}per {div}{}", var_names(x.vars.as_ref())?)
                }
                TagKind::PerStat(x) => {
                    let base = match x.base {
                        Some(b) => format!("{} + {} ", mod_base(m, b), abs_value(m)?),
                        None => base_of(base_val.as_ref())?,
                    };
                    base_val = Some(String::new());
                    let a = x
                        .actor
                        .map(|a| format!("{} ", a.as_str()))
                        .unwrap_or_default();
                    format!(
                        "{base}per {} {a}{}",
                        pat::num(x.div.unwrap_or(1.0)),
                        var_names(x.stats.as_ref())?
                    )
                }
                TagKind::PercentStat(x) => {
                    let v = m.value.as_number().ok_or("attempt to perform arithmetic")?;
                    let fin = (v * (x.percent.unwrap_or(1.0) / 100.0)) * 100.0;
                    let base = match x.base {
                        Some(b) => format!("{} + {} ", mod_base(m, b), pat::num(fin.abs())),
                        None => mod_base(m, fin),
                    };
                    base_val = Some(String::new());
                    let a = x
                        .actor
                        .map(|a| format!("{} ", a.as_str()))
                        .unwrap_or_default();
                    let stat = match x.percent_var {
                        Some(v) => mod_name(v.as_str()),
                        None => var_names(x.stats.as_ref())?,
                    };
                    format!("{base}% of {a}{stat}")
                }
                TagKind::MultiplierThreshold(x) => {
                    let threshold = match (x.threshold, x.threshold_var) {
                        (Some(t), _) => pat::num(t),
                        (None, Some(v)) => mod_name(v.as_str()),
                        _ => return Err("attempt to index a nil value".into()),
                    };
                    let op = if x.upper { " <= " } else { " >= " };
                    format!("If {}{op}{threshold}", var_names(x.vars.as_ref())?)
                }
                TagKind::StatThreshold(x) => {
                    let threshold = match (x.threshold, x.threshold_stat) {
                        (Some(t), _) => pat::num(t),
                        (None, Some(v)) => mod_name(v.as_str()),
                        _ => return Err("attempt to index a nil value".into()),
                    };
                    let op = if x.upper { " <= " } else { " >= " };
                    let pc = x
                        .threshold_percent
                        .map(|p| format!("{}% ", pat::num(p)))
                        .unwrap_or_default();
                    format!("If {}{op}{pc}{threshold}", var_names(x.stats.as_ref())?)
                }
                TagKind::SkillName { names, .. } => match names {
                    Some(n) => format!("Skill: {}", names_text(n, " / ")),
                    None => return Err("attempt to concatenate a nil value".into()),
                },
                TagKind::SkillId(id) => {
                    let skill = id
                        .as_deref()
                        .and_then(|i| self.data.game.skills.skills.get(i));
                    format!(
                        "Skill: {}",
                        skill.ok_or("attempt to index a nil value")?.name
                    )
                }
                TagKind::SkillType {
                    skill_type, neg: n, ..
                } => {
                    let name = skill_type.and_then(|t| {
                        let types = &self.data.game.skills.skill_type;
                        let mut found: Vec<&String> = types
                            .iter()
                            .filter(|(_, v)| **v as f64 == t)
                            .map(|(k, _)| k)
                            .collect();
                        found.sort();
                        found.first().map(|n| mod_name(n))
                    });
                    format!(
                        "Skill type: {}{}",
                        neg(*n),
                        name.unwrap_or_else(|| "?".into())
                    )
                }
                TagKind::BaseFlag { flag, neg: n } => {
                    format!("Base flag: {}{}", neg(*n), mod_name(flag.as_str()))
                }
                TagKind::Limit { limit, limit_var } => match (limit_var, limit) {
                    (Some(v), _) => format!("Limited to {}", mod_name(v.as_str())),
                    (None, Some(l)) => format!("Limited to {}", mod_base(m, *l)),
                    _ => return Err("attempt to perform arithmetic on a nil value".into()),
                },
                TagKind::MonsterTag { names, .. } => match names {
                    Some(n) => format!("Monster Tag: {}", names_text(n, "/")),
                    None => return Err("attempt to concatenate a nil value".into()),
                },
                TagKind::Other { kind, fields } => match kind.as_str() {
                    "SlotNumber" => format!(
                        "When in slot #{}",
                        fields
                            .get("num")
                            .map(value_text)
                            .transpose()?
                            .unwrap_or_default()
                    ),
                    "GlobalEffect" => match fields.get("effectType") {
                        Some(ModValue::Str(s)) => mod_name(s),
                        _ => return Err("attempt to index a nil value".into()),
                    },
                    k => mod_name(k),
                },
                other => mod_name(tag_type(other)),
            };
            tags = Some(match tags {
                Some(t) => format!("{t}, {desc}"),
                None => desc,
            });
        }
        Ok(tags)
    }
}

/// `math.abs(row.mod.value)` as concatenated
fn abs_value(m: &Mod) -> Result<String> {
    m.value
        .as_number()
        .map(|v| pat::num(v.abs()))
        .ok_or_else(|| "bad argument #1 to 'abs'".into())
}

/// A tag's `type`
fn tag_type(t: &TagKind) -> &str {
    match t {
        TagKind::Multiplier(_) => "Multiplier",
        TagKind::MultiplierThreshold(_) => "MultiplierThreshold",
        TagKind::PerStat(_) => "PerStat",
        TagKind::PercentStat(_) => "PercentStat",
        TagKind::StatThreshold(_) => "StatThreshold",
        TagKind::DistanceRamp(_) => "DistanceRamp",
        TagKind::MeleeProximity(_) => "MeleeProximity",
        TagKind::Limit { .. } => "Limit",
        TagKind::Condition { .. } => "Condition",
        TagKind::ActorCondition { .. } => "ActorCondition",
        TagKind::ItemCondition(_) => "ItemCondition",
        TagKind::SocketedIn(_) => "SocketedIn",
        TagKind::SkillName { .. } => "SkillName",
        TagKind::SkillId(_) => "SkillId",
        TagKind::SkillPart { .. } => "SkillPart",
        TagKind::SkillType { .. } => "SkillType",
        TagKind::BaseFlag { .. } => "BaseFlag",
        TagKind::SlotName { .. } => "SlotName",
        TagKind::ModFlagOr(_) => "ModFlagOr",
        TagKind::KeywordFlagAnd(_) => "KeywordFlagAnd",
        TagKind::MonsterTag { .. } => "MonsterTag",
        TagKind::Other { kind, .. } => kind.as_str(),
    }
}

fn cols(c: &[(&str, &'static str)]) -> Vec<(&'static str, String)> {
    c.iter().map(|(l, k)| (*k, l.to_string())).collect()
}
