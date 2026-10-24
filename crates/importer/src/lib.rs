//! Reads upstream data at build time, see docs/design.md.
//!
//! [`import`] evaluates the data files upstream's `Modules/Data.lua` loads,
//! with the same arguments, and returns their raw tables. The processing
//! `Data.lua` does on top of them is ported in the engine.

pub mod eval;
pub mod luagen;
pub mod mod_parser;
pub mod portgen;

use eval::{Env, EvalError};
use lua::{TableRef, Value};
use std::path::{Path, PathBuf};

/// Raw upstream data.
pub struct Raw {
    /// Globals defined by `Data/Global.lua` (`ModFlag`, `SkillType`, ...)
    pub globals: TableRef,
    /// `data` as filled by the `LoadModule("Data/...")` calls in `Data.lua`,
    /// before `Data.lua` processes it
    pub data: TableRef,
}

#[derive(Debug)]
pub enum Error {
    Io(PathBuf, std::io::Error),
    Eval(EvalError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Error::Eval(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<EvalError> for Error {
    fn from(e: EvalError) -> Self {
        Error::Eval(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Item base and unique types, in `Data.lua`'s `itemTypes` order.
pub const ITEM_TYPES: &[&str] = &[
    "axe", "bow", "claw", "dagger", "fishing", "mace", "staff", "sword", "wand", "helmet", "body",
    "gloves", "boots", "shield", "quiver", "amulet", "ring", "belt", "jewel", "flask", "tincture",
    "graft",
];

/// Tables `Modules/Data.lua` assigns as literals.
pub const DATA_LUA_CONSTANTS: &[&str] = &[
    "data.keystones",
    "data.ailmentTypeList",
    "data.elementalAilmentTypeList",
    "data.nonDamagingAilmentTypeList",
    "data.nonElementalAilmentTypeList",
    "data.nonDamagingAilment",
    "data.jewelRadii",
];

/// Skill files, in `Data.lua`'s `skillTypes` order.
pub const SKILL_TYPES: &[&str] = &[
    "act_str", "act_dex", "act_int", "other", "glove", "minion", "spectre", "sup_str", "sup_dex",
    "sup_int",
];

pub struct Importer {
    src_dir: PathBuf,
    pub env: Env,
}

impl Importer {
    /// `src_dir` is upstream's `src/` directory.
    pub fn new(src_dir: &Path) -> Self {
        let mut env = Env::new();
        install_natives(&mut env);
        Importer {
            src_dir: src_dir.to_path_buf(),
            env,
        }
    }

    /// Reads a file relative to upstream's `src/`.
    pub fn read(&self, rel: &str) -> Result<Vec<u8>> {
        let path = self.src_dir.join(rel);
        std::fs::read(&path).map_err(|e| Error::Io(path, e))
    }

    /// `LoadModule(name, args...)`
    pub fn load(&mut self, name: &str, args: Vec<Value>) -> Result<Vec<Value>> {
        let rel = format!("{name}.lua");
        let src = self.read(&rel)?;
        Ok(self.env.eval_file(&rel, &src, args)?)
    }

    /// `LoadModule(name, args...)`, first return value
    pub fn load1(&mut self, name: &str, args: Vec<Value>) -> Result<Value> {
        Ok(self
            .load(name, args)?
            .into_iter()
            .next()
            .unwrap_or_default())
    }

    pub fn global(&self, name: &str) -> Value {
        self.env.globals.get(name).cloned().unwrap_or_default()
    }
}

/// Helpers upstream defines in Lua that data files call.
fn install_natives(env: &mut Env) {
    env.set_native("copyTable", |args| match args.first() {
        Some(Value::Table(t)) => Ok(vec![Value::Table(copy_table(
            t,
            args.get(1).is_some_and(Value::truthy),
        ))]),
        _ => Err("copyTable expects a table".into()),
    });
    let bit = TableRef::new();
    let bit_op = |f: fn(i32, i32) -> i32| {
        move |args: &[Value]| -> std::result::Result<Vec<Value>, String> {
            let mut acc = to_bit(args.first())?;
            for a in &args[1..] {
                acc = f(acc, to_bit(Some(a))?);
            }
            Ok(vec![Value::Number(acc as f64)])
        }
    };
    bit.set_str("bor", env.native("bit.bor", bit_op(|a, b| a | b)));
    bit.set_str("band", env.native("bit.band", bit_op(|a, b| a & b)));
    bit.set_str("bxor", env.native("bit.bxor", bit_op(|a, b| a ^ b)));
    bit.set_str(
        "bnot",
        env.native("bit.bnot", |args| {
            Ok(vec![Value::Number(!to_bit(args.first())? as f64)])
        }),
    );
    env.globals.insert("bit".into(), Value::Table(bit));
    let math = TableRef::new();
    math.set_str("huge", f64::INFINITY);
    math.set_str("pi", std::f64::consts::PI);
    env.globals.insert("math".into(), Value::Table(math));
}

/// LuaJIT's `bit` library works on the number modulo 2^32 as a signed 32 bit integer.
pub fn to_bit(v: Option<&Value>) -> std::result::Result<i32, String> {
    let n = v
        .and_then(lua::to_number)
        .ok_or("bit operation on a non-number")?;
    Ok((n.rem_euclid(4294967296.0) as u64 as u32) as i32)
}

/// Upstream's `copyTable`.
pub fn copy_table(t: &TableRef, no_recurse: bool) -> TableRef {
    let out = TableRef::new();
    for (k, v) in t.entries() {
        let v = match v {
            Value::Table(sub) if !no_recurse => Value::Table(copy_table(&sub, false)),
            v => v,
        };
        out.borrow_mut().set_key(k, v);
    }
    out
}

/// `makeSkillMod` from `Data.lua`.
fn make_skill_mod(args: &[Value]) -> TableRef {
    let m = TableRef::new();
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    m.set_str("name", arg(0));
    m.set_str("type", arg(1));
    m.set_str("value", arg(2));
    m.set_str(
        "flags",
        if arg(3).truthy() {
            arg(3)
        } else {
            Value::Number(0.0)
        },
    );
    m.set_str(
        "keywordFlags",
        if arg(4).truthy() {
            arg(4)
        } else {
            Value::Number(0.0)
        },
    );
    for (i, v) in args.iter().skip(5).enumerate() {
        if !v.is_nil() {
            m.set(Value::Number((i + 1) as f64), v.clone());
        }
    }
    m
}

/// The mod helpers skill, minion and stat map files get: `makeSkillMod`,
/// `makeFlagMod`, `makeSkillDataMod`.
fn skill_mod_natives(env: &mut Env) -> (Value, Value, Value) {
    let skill_mod = env.native("makeSkillMod", |args| {
        Ok(vec![Value::Table(make_skill_mod(args))])
    });
    let flag_mod = env.native("makeFlagMod", |args| {
        let mut a = vec![
            args.first().cloned().unwrap_or_default(),
            "FLAG".into(),
            true.into(),
            0.0.into(),
            0.0.into(),
        ];
        a.extend(args.iter().skip(1).cloned());
        Ok(vec![Value::Table(make_skill_mod(&a))])
    });
    let data_mod = env.native("makeSkillDataMod", |args| {
        let v = TableRef::new();
        v.set_str("key", args.first().cloned().unwrap_or_default());
        v.set_str("value", args.get(1).cloned().unwrap_or_default());
        let mut a = vec![
            "SkillData".into(),
            "LIST".into(),
            Value::Table(v),
            0.0.into(),
            0.0.into(),
        ];
        a.extend(args.iter().skip(2).cloned());
        Ok(vec![Value::Table(make_skill_mod(&a))])
    });
    (skill_mod, flag_mod, data_mod)
}

/// Imports everything `Data.lua` loads.
pub fn import(src_dir: &Path) -> Result<Raw> {
    let mut im = Importer::new(src_dir);
    let data = TableRef::new();
    im.env
        .globals
        .insert("data".into(), Value::Table(data.clone()));
    let set = |k: &str, v: Value| data.set_str(k, v);

    im.load("Data/Global", vec![])?;
    im.load("Data/Misc", vec![Value::Table(data.clone())])?;
    // Constants Modules/Data.lua defines itself
    let data_lua = im.read("Modules/Data.lua")?;
    im.env
        .eval_assigns("Modules/Data.lua", &data_lua, DATA_LUA_CONSTANTS)?;
    set("modScalability", im.load1("Data/ModScalability", vec![])?);

    let item_mods = TableRef::new();
    for (key, file) in [
        ("Explicit", "ModExplicit"),
        ("ItemExclusive", "ModItemExclusive"),
        ("Corrupted", "ModCorrupted"),
        ("Delve", "ModDelve"),
        ("Synthesis", "ModSynthesis"),
        ("Scourge", "ModScourge"),
        ("Eldritch", "ModEldritch"),
        ("Flask", "ModFlask"),
        ("Tincture", "ModTincture"),
        ("Graft", "ModGraft"),
        ("Jewel", "ModJewel"),
        ("JewelAbyss", "ModJewelAbyss"),
        ("JewelCluster", "ModJewelCluster"),
        ("JewelCharm", "ModJewelCharm"),
        ("Foulborn", "ModFoulborn"),
    ] {
        item_mods.set_str(key, im.load1(&format!("Data/{file}"), vec![])?);
    }
    set("itemMods", Value::Table(item_mods));
    set("masterMods", im.load1("Data/ModMaster", vec![])?);
    let enchantments = TableRef::new();
    for (key, file) in [
        ("Helmet", "EnchantmentHelmet"),
        ("Boots", "EnchantmentBoots"),
        ("Gloves", "EnchantmentGloves"),
        ("Belt", "EnchantmentBelt"),
        ("Body Armour", "EnchantmentBody"),
        ("Weapon", "EnchantmentWeapon"),
        ("UtilityFlask", "EnchantmentFlask"),
    ] {
        enchantments.set_str(key, im.load1(&format!("Data/{file}"), vec![])?);
    }
    set("enchantments", Value::Table(enchantments));
    for (key, file) in [
        ("essences", "Essence"),
        ("veiledMods", "ModVeiled"),
        ("beastCraft", "BeastCraft"),
        ("necropolisMods", "ModNecropolis"),
        ("crucible", "Crucible"),
        ("pantheons", "Pantheons"),
        ("costs", "Costs"),
        ("mapMods", "ModMap"),
        ("clusterJewels", "ClusterJewels"),
        ("timelessJewelTradeIDs", "TimelessJewelData/LegionTradeIds"),
        ("nodeIDList", "TimelessJewelData/NodeIndexMapping"),
        ("abyssNotableNames", "TimelessJewelData/AbyssNotableNames"),
        ("rares", "Rares"),
        ("flavourText", "FlavourText"),
    ] {
        set(key, im.load1(&format!("Data/{file}"), vec![])?);
    }
    let bosses = TableRef::new();
    im.load("Data/Bosses", vec![Value::Table(bosses.clone())])?;
    set("bosses", Value::Table(bosses));
    let mut boss_skills = im.load("Data/BossSkills", vec![])?.into_iter();
    set("bossSkills", boss_skills.next().unwrap_or_default());
    set("bossSkillsList", boss_skills.next().unwrap_or_default());

    let (skill_mod, flag_mod, data_mod) = skill_mod_natives(&mut im.env);
    set(
        "skillStatMap",
        im.load1(
            "Data/SkillStatMap",
            vec![skill_mod.clone(), flag_mod.clone(), data_mod.clone()],
        )?,
    );
    let skills = TableRef::new();
    for t in SKILL_TYPES {
        im.load(
            &format!("Data/Skills/{t}"),
            vec![
                Value::Table(skills.clone()),
                skill_mod.clone(),
                flag_mod.clone(),
                data_mod.clone(),
            ],
        )?;
    }
    set("skills", Value::Table(skills));
    set("gems", im.load1("Data/Gems", vec![])?);
    let minions = TableRef::new();
    im.load(
        "Data/Minions",
        vec![
            Value::Table(minions.clone()),
            skill_mod.clone(),
            flag_mod.clone(),
        ],
    )?;
    set("minions", Value::Table(minions));
    let spectres = TableRef::new();
    im.load(
        "Data/Spectres",
        vec![Value::Table(spectres.clone()), skill_mod, flag_mod],
    )?;
    set("spectres", Value::Table(spectres));

    let item_bases = TableRef::new();
    for t in ITEM_TYPES {
        im.load(
            &format!("Data/Bases/{t}"),
            vec![Value::Table(item_bases.clone())],
        )?;
    }
    set("itemBases", Value::Table(item_bases));
    let uniques = TableRef::new();
    for t in ITEM_TYPES {
        uniques.set_str(t, im.load1(&format!("Data/Uniques/{t}"), vec![])?);
    }
    uniques.set_str("race", im.load1("Data/Uniques/Special/race", vec![])?);
    set("uniques", Value::Table(uniques));
    set(
        "watchersEyeMods",
        im.load1("Data/Uniques/Special/WatchersEye", vec![])?,
    );
    // Data/Uniques/Special/Generated.lua builds uniques with code and is
    // ported in the engine
    im.load("Data/Uniques/Special/New", vec![])?;

    let globals = TableRef::new();
    let mut names: Vec<_> = im.env.globals.keys().cloned().collect();
    names.sort();
    for name in names {
        if name != "data" {
            globals.set_str(&name, im.env.globals[&name].clone());
        }
    }
    Ok(Raw { globals, data })
}
