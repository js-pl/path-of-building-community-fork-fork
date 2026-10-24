//! Converts the translated program's mod stores into the engine's typed
//! ones (`engine::mods`), to test the typed queries against the translated
//! ones on the same state. Temporary: it goes with the translation.

use engine::mods::convert::{
    Converter, Originals, array, get, opt_number, opt_sym, s, text, truthy_keys,
};
use engine::mods::tag::*;
use engine::mods::*;
use lua::rt::{self, first};
use lua::{Key, LResult, LuaStr, TableRef, Value};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Converts a cfg table.
pub fn cfg(v: &Value) -> Result<Option<Cfg>, String> {
    if !v.truthy() {
        return Ok(None);
    }
    let g = |k: &str| get(v, k);
    let opt_text = |k: &str| match g(k) {
        Value::Nil => Ok(None),
        x => text(&x).map(Some),
    };
    let ge = g("skillGrantedEffect");
    let base_flags = {
        let bf = if ge.truthy() {
            get(&ge, "baseFlags")
        } else {
            Value::Nil
        };
        let bf = if bf.truthy() { bf } else { g("baseFlags") };
        if bf.truthy() {
            Some(truthy_keys(&bf)?)
        } else {
            None
        }
    };
    let skill_types = match g("skillTypes") {
        Value::Table(t) => {
            let mut set = HashSet::new();
            for (k, x) in t.entries() {
                if let Key::Int(i) = k
                    && x.truthy()
                {
                    set.insert(i);
                }
            }
            Some(set)
        }
        _ => None,
    };
    let skill_stats = match g("skillStats") {
        Value::Table(t) => {
            let mut map = HashMap::new();
            for (k, x) in t.entries() {
                if let (Key::Str(name), Some(n)) = (k, x.as_number()) {
                    map.insert(Sym::new(&name.to_str_lossy()), n);
                }
            }
            Some(map)
        }
        _ => None,
    };
    Ok(Some(Cfg {
        flags: opt_number(&g("flags"))?.map(|n| n.rem_euclid(4294967296.0) as u64 as u32),
        keyword_flags: opt_number(&g("keywordFlags"))?
            .map(|n| n.rem_euclid(4294967296.0) as u64 as u32),
        source: opt_text("source")?,
        skill_name: opt_text("skillName")?,
        summon_skill_name: opt_text("summonSkillName")?,
        skill_id: if ge.truthy() {
            Some(match get(&ge, "id") {
                Value::Nil => None,
                x => Some(text(&x)?),
            })
        } else {
            None
        },
        base_flags,
        skill_part: opt_number(&g("skillPart"))?,
        skill_types,
        skill_cond: match g("skillCond") {
            Value::Nil => None,
            sc => Some(truthy_keys(&sc)?),
        },
        skill_dist: opt_number(&g("skillDist"))?,
        slot_name: opt_sym(&g("slotName"))?,
        actor: opt_sym(&g("actor"))?,
        skill_stats,
        socket_color: opt_text("socketColor")?,
        socket_num: opt_number(&g("socketNum"))?,
        strength_gems: opt_number(&g("strengthGems"))?,
        dexterity_gems: opt_number(&g("dexterityGems"))?,
        intelligence_gems: opt_number(&g("intelligenceGems"))?,
    }))
}

/// The typed copy of the translated program's stores, and the translated
/// program behind it for what the typed core asks of [`Actors`].
pub struct Bridge {
    pub stores: Stores,
    /// The Lua store of every typed one
    pub lua: Vec<Value>,
    ids: HashMap<*const u8, StoreId>,
    pub originals: RefCell<Originals>,
    /// The Lua cfg of the query running, for what needs upstream's gem data
    pub lua_cfg: RefCell<Value>,
    env: Rc<lua::env::Env>,
}

fn ptr(v: &Value) -> Option<*const u8> {
    match v {
        Value::Table(t) => Some(Rc::as_ptr(&t.0) as *const u8),
        _ => None,
    }
}

impl Bridge {
    pub fn new(env: Rc<lua::env::Env>) -> Bridge {
        Bridge {
            stores: Stores::default(),
            lua: Vec::new(),
            ids: HashMap::new(),
            originals: Default::default(),
            lua_cfg: RefCell::new(Value::Nil),
            env,
        }
    }

    pub fn id(&self, store: &Value) -> Option<StoreId> {
        ptr(store).and_then(|p| self.ids.get(&p).copied())
    }

    /// Converts a Lua store and its parents; the store's id.
    pub fn add(&mut self, store: &Value) -> Result<StoreId, String> {
        if let Some(id) = self.id(store) {
            return Ok(id);
        }
        let conv = Converter {
            originals: Some(&self.originals),
        };
        let mods = get(store, "mods");
        let mut typed = if let Value::Table(by_name) = &mods {
            let mut db = Store::db();
            for (k, list) in by_name.entries() {
                let name = match &k {
                    Key::Str(s) => Sym::new(&s.to_str_lossy()),
                    k => return Err(format!("mod name {k:?}")),
                };
                let mut bucket = Vec::new();
                for m in array(&list) {
                    let m = conv.mod_(&m).map_err(|e| format!("{name}: {e}"))?;
                    bucket.push(m);
                }
                if let Mods::Db(map) = &mut db.mods {
                    map.insert(name, bucket);
                }
            }
            db
        } else {
            let mut list = Store::list();
            for m in array(store) {
                list.add(conv.mod_(&m)?);
            }
            list
        };
        for (field, is_cond) in [("conditions", true), ("multipliers", false)] {
            if let Value::Table(t) = get(store, field) {
                for (k, x) in t.entries() {
                    let Key::Str(name) = k else { continue };
                    let name = Sym::new(&name.to_str_lossy());
                    if is_cond {
                        typed.conditions.insert(name, x.truthy());
                    } else if let Some(n) = lua::to_number(&x) {
                        typed.multipliers.insert(name, n);
                    }
                }
            }
        }
        let id = self.stores.push(typed);
        self.lua.push(store.clone());
        self.ids.insert(ptr(store).unwrap(), id);
        let parent = get(store, "parent");
        if parent.truthy() {
            let p = self.add(&parent)?;
            self.stores.stores[id].parent = Some(p);
        }
        Ok(id)
    }

    fn calc_lib(&self, name: &str, args: &[Value]) -> Value {
        let f = get(&self.env.global("calcLib"), name);
        first(rt::call(&f, args).unwrap_or_default())
    }

    /// `getActor(store, actorType)` of `ModStore.lua`
    fn get_actor(&self, store: &Value, actor_type: &str) -> Value {
        let actor = get(store, "actor");
        if actor_type == "player" {
            let p = get(&actor, "player");
            if p.truthy() {
                return p;
            }
            let parent = get(&actor, "parent");
            let p = if parent.truthy() {
                get(&parent, "player")
            } else {
                parent
            };
            if p.truthy() {
                return p;
            }
            let enemy = get(&actor, "enemy");
            if enemy.truthy() {
                get(&enemy, "player")
            } else {
                enemy
            }
        } else {
            get(&actor, actor_type)
        }
    }
}

impl Actors for Bridge {
    fn actor_store(&self, from: StoreId, actor: Sym) -> Option<StoreId> {
        let a = self.get_actor(&self.lua[from], actor.as_str());
        if !a.truthy() {
            return None;
        }
        let db = get(&a, "modDB");
        Some(
            self.id(&db)
                .unwrap_or_else(|| panic!("the modDB of actor '{actor}' was not converted")),
        )
    }

    fn output(&self, store: StoreId, stat: Sym) -> Option<f64> {
        let out = get(&get(&self.lua[store], "actor"), "output");
        if !out.truthy() {
            return None;
        }
        let v = get(&out, stat.as_str());
        if v.truthy() {
            Some(lua::to_number(&v).unwrap_or_else(|| panic!("output {stat} is {v:?}")))
        } else {
            None
        }
    }

    fn special_stat(&self, store: StoreId, stat: Sym, _cfg: Option<&Cfg>) -> Option<f64> {
        if !matches!(
            stat.as_str(),
            "ManaReservedPercent" | "LifeReservedPercent" | "ManaUnreserved"
        ) {
            return None;
        }
        let cfg = self.lua_cfg.borrow().clone();
        let r = rt::method(
            &self.lua[store],
            &LuaStr::from("GetStat"),
            &[s(stat.as_str()), cfg],
        )
        .unwrap_or_else(|e| panic!("GetStat {stat}: {e}"));
        Some(lua::to_number(&first(r)).unwrap_or(0.0))
    }

    fn one_handed_added(&self, store: StoreId, var: Sym) -> Option<bool> {
        let actor = get(&self.lua[store], "actor");
        let pick = |w: &str| {
            let d = get(&actor, w);
            (d.truthy() && get(&d, "countsAsAll1H").truthy()).then_some(d)
        };
        let all = pick("weaponData1").or_else(|| pick("weaponData2"))?;
        let v = get(&all, &format!("Added{var}"));
        (!v.is_nil()).then(|| v.truthy())
    }

    fn monster_tags(&self, store: StoreId) -> Option<Vec<Rc<str>>> {
        let actor = get(&self.lua[store], "actor");
        let md = get(&actor, "minionData");
        let tags = if md.truthy() {
            get(&md, "monsterTags")
        } else {
            Value::Nil
        };
        let Value::Table(t) = tags else {
            return None;
        };
        Some(
            t.entries()
                .into_iter()
                .map(|(_, v)| text(&v).unwrap())
                .collect(),
        )
    }

    fn item_condition(&self, store: StoreId, tag: &ItemCondition, _cfg: Option<&Cfg>) -> bool {
        // Evaluated by the translated EvalMod on a mod with this tag alone
        let lua_tag = self.originals.borrow().item_conditions[&(tag as *const _)].clone();
        let m = TableRef::new();
        m.set_str("value", Value::Number(1.0));
        m.push(lua_tag);
        let cfg = self.lua_cfg.borrow().clone();
        let r = rt::method(
            &self.lua[store],
            &LuaStr::from("EvalMod"),
            &[Value::Table(m), cfg],
        )
        .unwrap_or_else(|e| panic!("EvalMod ItemCondition: {e}"));
        first(r).truthy()
    }

    fn gem_is_type(&self, _cfg: &Cfg, keyword: &str) -> bool {
        let lua_cfg = self.lua_cfg.borrow().clone();
        let gem = get(&lua_cfg, "skillGem");
        gem.truthy() && self.calc_lib("gemIsType", &[gem, s(keyword)]).truthy()
    }

    fn game_id(&self, gem_name: &str) -> Option<Rc<str>> {
        match self.calc_lib("getGameIdFromGemName", &[s(gem_name), Value::Bool(true)]) {
            Value::Str(x) => Some(x.to_str_lossy().into()),
            _ => None,
        }
    }

    fn high_precision(&self, name: Sym, typ: ModType) -> Option<f64> {
        let hp = get(
            &get(&self.env.global("data"), "highPrecisionMods"),
            name.as_str(),
        );
        if !hp.truthy() {
            return None;
        }
        let p = get(&hp, typ.name());
        p.truthy().then(|| lua::to_number(&p).unwrap())
    }
}

/// Calls a translated store method, `Err` with its message on a Lua error.
pub fn call(store: &Value, method: &str, args: &[Value]) -> LResult<Vec<Value>> {
    rt::method(store, &LuaStr::from(method), args)
}
