//! The actors of a calculation as the mod queries see them: each actor's
//! database, equipment, outputs and relations (`env.player`, `env.enemy`,
//! `env.minion`), the [`Actors`] of the typed stores.
//!
//! In sync with upstream `b32759ab0f31a1c8499a0d420cb0f0633d4fe478`.

use crate::item::{Fields, Item};
use crate::item_data::ItemData;
use crate::items_tab::ItemsTab;
use crate::mods::store::{Actors, Cfg, StoreId};
use crate::mods::tag::ItemCondition;
use crate::mods::{ModType, ModValue, Sym};
use crate::pat;
use crate::skill_data::SkillData;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

/// A value of an actor's output: mostly numbers
#[derive(Clone, Debug, PartialEq)]
pub enum OutVal {
    Num(f64),
    Bool(bool),
    Str(Rc<str>),
    /// A table: a weapon's output, a requirement's source, ...
    Table(Rc<BTreeMap<String, OutVal>>),
    /// A gem or an item in a table, by name (`oracle/json.lua` writes them
    /// as `{"$gem": name}` and `{"$item": name}`)
    Gem(Rc<str>),
    Item(Rc<str>),
}

impl OutVal {
    pub fn num(&self) -> Option<f64> {
        match self {
            OutVal::Num(n) => Some(*n),
            _ => None,
        }
    }
}

/// An active skill as `GetStat` sees it: a skill with a reservation that
/// is not disabled, with its buffs' names and the base it reserves
#[derive(Clone, Debug, Default)]
pub struct ReservingSkill {
    pub buff_names: Vec<String>,
    pub mana_reserved_base: Option<f64>,
    pub life_reserved_base: Option<f64>,
}

/// An actor: the player, the enemy or a minion
#[derive(Clone, Debug, Default)]
pub struct Actor {
    /// The actor's database (`actor.modDB`)
    pub store: StoreId,
    /// `actor.itemList`: the item in each slot, by id
    pub item_list: BTreeMap<String, f64>,
    /// `actor.output`
    pub output: HashMap<Sym, OutVal>,
    /// `actor.weaponData1` and `weaponData2`
    pub weapon_data: [Option<Fields>; 2],
    /// `actor.minionData.monsterTags`
    pub monster_tags: Option<Vec<Rc<str>>>,
    /// What `GetStat` reads of the actor's active skills
    pub reserving_skills: Vec<ReservingSkill>,
    /// `actor.ManaEfficiency`
    pub mana_efficiency: Option<f64>,
    /// `actor.player`, `actor.enemy` and `actor.parent`, by index
    pub player: Option<usize>,
    pub enemy: Option<usize>,
    pub parent: Option<usize>,
}

/// The actors of a calculation and what their queries read
pub struct EnvActors<'a> {
    pub actors: Vec<Actor>,
    /// The actor each store belongs to (`store.actor`)
    pub store_actor: HashMap<StoreId, usize>,
    pub tab: &'a ItemsTab,
    pub item_data: &'a ItemData,
    pub skill_data: &'a SkillData,
}

impl EnvActors<'_> {
    fn actor_of(&self, store: StoreId) -> Option<&Actor> {
        self.store_actor.get(&store).map(|&i| &self.actors[i])
    }

    fn item(&self, id: f64) -> Option<&Item> {
        self.tab.item(id)
    }

    /// `getActor(store, actorType)` of `ModStore.lua`
    fn get_actor(&self, store: StoreId, kind: &str) -> Option<usize> {
        let a = self.actor_of(store)?;
        let player_of = |i: usize| self.actors[i].player;
        match kind {
            "player" => a
                .player
                .or_else(|| a.parent.and_then(player_of))
                .or_else(|| a.enemy.and_then(player_of)),
            "enemy" => a.enemy,
            "parent" => a.parent,
            _ => None,
        }
    }

    /// The items an `ItemCondition` tag looks at, by slot
    fn condition_items(
        &self,
        actor: &Actor,
        slot: &str,
        tag: &ItemCondition,
    ) -> Vec<(String, f64)> {
        let get = |s: &str| actor.item_list.get(s).copied();
        let kalandra = |id: f64| {
            self.item(id)
                .is_some_and(|i| i.name.contains("Kalandra's Touch"))
        };
        if tag.all_slots {
            return actor
                .item_list
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
        }
        if tag.both_slots {
            let (s1, s2) = (format!("{slot} 1"), format!("{slot} 2"));
            let mut i1 = get(&s1);
            let mut i2 = get(&s2);
            if i1.is_some_and(kalandra) {
                i1 = i2;
            }
            if i2.is_some_and(kalandra) {
                i2 = i1;
            }
            return match (i1, i2) {
                (Some(a), Some(b)) => vec![(s1, a), (s2, b)],
                _ => vec![],
            };
        }
        let mut item = get(slot);
        if item.is_some_and(kalandra) {
            let other = pat::gsub_fn(slot, "%d$", |c| match c[0].as_str() {
                "1" => Some("2".into()),
                "2" => Some("1".into()),
                _ => None,
            });
            item = get(&other);
        }
        item.map(|i| vec![(slot.to_string(), i)])
            .unwrap_or_default()
    }
}

/// `itemSlot:lower():gsub("(%l)(%w*)", ...)`, trimmed: each word capitalised
fn slot_key(slot: &str) -> String {
    let lower = pat::lower(slot);
    let titled = pat::gsub_fn(&lower, "(%l)(%w*)", |c| Some(c[0].to_uppercase() + &c[1]));
    titled.trim().to_string()
}

impl Actors for EnvActors<'_> {
    fn actor_store(&self, from: StoreId, actor: Sym) -> Option<StoreId> {
        self.get_actor(from, actor.as_str())
            .map(|i| self.actors[i].store)
    }

    fn output(&self, store: StoreId, stat: Sym) -> Option<f64> {
        match self.actor_of(store)?.output.get(&stat)? {
            OutVal::Num(n) => Some(*n),
            _ => None,
        }
    }

    fn special_stat(&self, store: StoreId, stat: Sym, cfg: Option<&Cfg>) -> Option<f64> {
        let actor = self.actor_of(store)?;
        let out = |k: &str| actor.output.get(&Sym::new(k)).and_then(OutVal::num);
        match stat.as_str() {
            s @ ("ManaReservedPercent" | "LifeReservedPercent") => {
                let mana = s == "ManaReservedPercent";
                let total = out(if mana { "Mana" } else { "Life" }).unwrap_or(f64::NAN);
                if total == 0.0 {
                    return Some(0.0);
                }
                let mut reserved = 0.0;
                if let Some(cfg) = cfg {
                    for skill in &actor.reserving_skills {
                        let has = |n: &Option<Rc<str>>| {
                            n.as_deref()
                                .is_some_and(|n| skill.buff_names.iter().any(|b| b == n))
                        };
                        if has(&cfg.skill_name) || has(&cfg.summon_skill_name) {
                            let base = if mana {
                                skill.mana_reserved_base
                            } else {
                                skill.life_reserved_base
                            };
                            reserved = (base.unwrap_or(0.0) / total * 100.0).floor();
                            break;
                        }
                    }
                }
                Some(if 100.0 < reserved { 100.0 } else { reserved })
            }
            "ManaUnreserved" => {
                let v = out("ManaUnreserved")?;
                if v.is_nan() {
                    out("Mana")
                } else if v < 0.0 {
                    let pct = out("ManaUnreservedPercent").unwrap_or(f64::NAN);
                    let eff = actor.mana_efficiency.unwrap_or(f64::NAN);
                    let before = (pct.abs() + 100.0) * ((100.0 + eff) / 100.0);
                    Some(out("Mana").unwrap_or(f64::NAN) * (before.ceil() / 100.0))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn one_handed_added(&self, store: StoreId, var: Sym) -> Option<bool> {
        let actor = self.actor_of(store)?;
        let all = actor
            .weapon_data
            .iter()
            .flatten()
            .find(|d| d.get("countsAsAll1H").is_some_and(ModValue::truthy))?;
        all.get(&format!("Added{var}")).map(ModValue::truthy)
    }

    fn monster_tags(&self, store: StoreId) -> Option<Vec<Rc<str>>> {
        self.actor_of(store)?.monster_tags.clone()
    }

    fn item_condition(&self, store: StoreId, tag: &ItemCondition, _: Option<&Cfg>) -> bool {
        let Some(actor) = self.actor_of(store) else {
            return tag.neg;
        };
        let slot = slot_key(&tag.item_slot);
        let items: Vec<(String, &Item)> = self
            .condition_items(actor, &slot, tag)
            .into_iter()
            .filter_map(|(s, id)| Some((s, self.item(id)?)))
            .collect();
        let mut matches = Vec::new();
        if let Some(search) = &tag.search_cond {
            let search = pat::lower(search);
            for (s, item) in &items {
                let typ = item.typ.as_deref().unwrap_or("");
                if ((!tag.all_slots || (typ != "Jewel" && typ != "Graft")) && *s != slot)
                    || !tag.exclude_self
                {
                    matches.push(item.find_modifier_substring(
                        &search,
                        &pat::lower(s),
                        self.item_data,
                    ));
                }
            }
        }
        if let Some(r) = &tag.rarity_cond {
            matches.extend(items.iter().map(|(_, i)| i.rarity == **r));
        }
        if let Some(c) = tag.corrupted_cond {
            matches.extend(items.iter().map(|(_, i)| i.corrupted == c));
        }
        if let Some(c) = tag.shaper_cond {
            matches.extend(items.iter().map(|(_, i)| i.influences[0] == c));
        }
        if let Some(c) = tag.elder_cond {
            matches.extend(items.iter().map(|(_, i)| i.influences[1] == c));
        }
        if let Some(n) = &tag.name_cond {
            let n = pat::lower(n);
            matches.extend(items.iter().map(|(_, i)| pat::lower(&i.name) == n));
        }
        let all = matches.iter().all(|&m| m != tag.neg);
        all && (!items.is_empty() || tag.neg)
    }

    fn gem_is_type(&self, cfg: &Cfg, keyword: &str) -> bool {
        cfg.skill_gem
            .as_ref()
            .is_some_and(|g| self.skill_data.gem_is_type(g, keyword, false))
    }

    fn game_id(&self, gem_name: &str) -> Option<Rc<str>> {
        self.skill_data
            .game_id_from_gem_name(gem_name, true)
            .map(Rc::from)
    }

    fn high_precision(&self, name: Sym, typ: ModType) -> Option<f64> {
        self.item_data
            .high_precision_mods
            .get(&(name.as_str().to_string(), typ.name().to_string()))
            .copied()
    }
}
