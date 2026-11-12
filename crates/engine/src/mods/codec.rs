//! Mods in the binary form of [`crate::codec`].

use super::tag::*;
use super::types::*;
use crate::codec::{Codec, Reader, Result, Writer};
use crate::codec_struct;

fn bad<T>(what: &str, k: u8) -> Result<T> {
    Err(format!("bad {what} {k} in typed data"))
}

impl Codec for ModType {
    fn put(&self, w: &mut Writer) {
        w.str(self.name());
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok(ModType::new(&r.str()?))
    }
}

impl Codec for ModValue {
    fn put(&self, w: &mut Writer) {
        match self {
            ModValue::Bool(b) => (0u8, b).put_pair(w),
            ModValue::Number(n) => (1u8, n).put_pair(w),
            ModValue::Str(s) => (2u8, s).put_pair(w),
            ModValue::Mod(m) => (3u8, m).put_pair(w),
            ModValue::Record(x) => (4u8, x).put_pair(w),
            ModValue::List(l) => (5u8, l).put_pair(w),
            ModValue::Func(f) => (6u8, f).put_pair(w),
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok(match u8::get(r)? {
            0 => ModValue::Bool(Codec::get(r)?),
            1 => ModValue::Number(Codec::get(r)?),
            2 => ModValue::Str(Codec::get(r)?),
            3 => ModValue::Mod(Codec::get(r)?),
            4 => ModValue::Record(Codec::get(r)?),
            5 => ModValue::List(Codec::get(r)?),
            6 => ModValue::Func(Codec::get(r)?),
            k => return bad("mod value", k),
        })
    }
}

/// A variant's tag and payload
trait PutPair {
    fn put_pair(self, w: &mut Writer);
}

impl<T: Codec> PutPair for (u8, &T) {
    fn put_pair(self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
    }
}

codec_struct!(Func { name, args });
codec_struct!(Record { fields });
codec_struct!(Mod {
    name,
    typ,
    value,
    flags,
    keyword_flags,
    source,
    source_slot,
    tags,
    unreached_tags,
});
codec_struct!(Tag { kind, global_limit });
codec_struct!(Multiplier {
    vars,
    actor,
    limit_actor,
    div,
    div_var,
    no_floor,
    limit,
    limit_var,
    limit_stat,
    limit_total,
    limit_neg_total,
    invert,
    base,
});
codec_struct!(MultiplierThreshold {
    vars,
    actor,
    threshold_actor,
    threshold,
    threshold_var,
    upper,
    equals,
});
codec_struct!(PerStat {
    stats,
    actor,
    div,
    div_var,
    limit,
    limit_var,
    limit_total,
    base,
});
codec_struct!(PercentStat {
    stats,
    actor,
    percent,
    percent_var,
    floor,
    limit,
    limit_var,
    limit_total,
    base,
});
codec_struct!(StatThreshold {
    stats,
    threshold,
    threshold_stat,
    threshold_percent,
    threshold_percent_var,
    upper,
});
codec_struct!(ItemCondition {
    item_slot,
    all_slots,
    both_slots,
    exclude_self,
    search_cond,
    rarity_cond,
    corrupted_cond,
    shaper_cond,
    elder_cond,
    name_cond,
    neg,
});
codec_struct!(SocketedIn {
    slot_name,
    keyword,
    socket_color,
    sockets,
    neg,
});

impl Codec for Names {
    fn put(&self, w: &mut Writer) {
        match self {
            Names::One(s) => (0u8, s).put_pair(w),
            Names::List(l) => (1u8, l).put_pair(w),
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok(match u8::get(r)? {
            0 => Names::One(Codec::get(r)?),
            1 => Names::List(Codec::get(r)?),
            k => return bad("names", k),
        })
    }
}

impl Codec for Sockets {
    fn put(&self, w: &mut Writer) {
        match self {
            Sockets::All => 0u8.put(w),
            Sockets::Numbers(n) => (1u8, n).put_pair(w),
            Sockets::Fewer(n) => (2u8, n).put_pair(w),
        }
    }
    fn get(r: &mut Reader) -> Result<Self> {
        Ok(match u8::get(r)? {
            0 => Sockets::All,
            1 => Sockets::Numbers(Codec::get(r)?),
            2 => Sockets::Fewer(Codec::get(r)?),
            k => return bad("sockets", k),
        })
    }
}

impl Codec for TagKind {
    fn put(&self, w: &mut Writer) {
        match self {
            TagKind::Multiplier(x) => (0u8, x).put_pair(w),
            TagKind::MultiplierThreshold(x) => (1u8, x).put_pair(w),
            TagKind::PerStat(x) => (2u8, x).put_pair(w),
            TagKind::PercentStat(x) => (3u8, x).put_pair(w),
            TagKind::StatThreshold(x) => (4u8, x).put_pair(w),
            TagKind::DistanceRamp(x) => (5u8, x).put_pair(w),
            TagKind::MeleeProximity(x) => (6u8, x).put_pair(w),
            TagKind::Limit { limit, limit_var } => (7u8, &(*limit, *limit_var)).put_pair(w),
            TagKind::Condition { vars, neg, actor } => {
                8u8.put(w);
                vars.put(w);
                neg.put(w);
                actor.put(w);
            }
            TagKind::ActorCondition { actor, vars, neg } => {
                9u8.put(w);
                actor.put(w);
                vars.put(w);
                neg.put(w);
            }
            TagKind::ItemCondition(x) => (10u8, x).put_pair(w),
            TagKind::SocketedIn(x) => (11u8, x).put_pair(w),
            TagKind::SkillName {
                names,
                include_transfigured,
                summon_skill,
                neg,
                skill_id,
            } => {
                12u8.put(w);
                names.put(w);
                include_transfigured.put(w);
                summon_skill.put(w);
                neg.put(w);
                skill_id.put(w);
            }
            TagKind::SkillId(x) => (13u8, x).put_pair(w),
            TagKind::SkillPart { list, part, neg } => {
                14u8.put(w);
                list.put(w);
                part.put(w);
                neg.put(w);
            }
            TagKind::SkillType {
                list,
                skill_type,
                neg,
            } => {
                15u8.put(w);
                list.put(w);
                skill_type.put(w);
                neg.put(w);
            }
            TagKind::BaseFlag { flag, neg } => (16u8, &(*flag, *neg)).put_pair(w),
            TagKind::SlotName { names, neg } => {
                17u8.put(w);
                names.put(w);
                neg.put(w);
            }
            TagKind::ModFlagOr(x) => (18u8, x).put_pair(w),
            TagKind::KeywordFlagAnd(x) => (19u8, x).put_pair(w),
            TagKind::MonsterTag { names, neg } => {
                20u8.put(w);
                names.put(w);
                neg.put(w);
            }
            TagKind::Other { kind, fields } => {
                21u8.put(w);
                kind.put(w);
                fields.put(w);
            }
        }
    }

    fn get(r: &mut Reader) -> Result<Self> {
        Ok(match u8::get(r)? {
            0 => TagKind::Multiplier(Codec::get(r)?),
            1 => TagKind::MultiplierThreshold(Codec::get(r)?),
            2 => TagKind::PerStat(Codec::get(r)?),
            3 => TagKind::PercentStat(Codec::get(r)?),
            4 => TagKind::StatThreshold(Codec::get(r)?),
            5 => TagKind::DistanceRamp(Codec::get(r)?),
            6 => TagKind::MeleeProximity(Codec::get(r)?),
            7 => TagKind::Limit {
                limit: Codec::get(r)?,
                limit_var: Codec::get(r)?,
            },
            8 => TagKind::Condition {
                vars: Codec::get(r)?,
                neg: Codec::get(r)?,
                actor: Codec::get(r)?,
            },
            9 => TagKind::ActorCondition {
                actor: Codec::get(r)?,
                vars: Codec::get(r)?,
                neg: Codec::get(r)?,
            },
            10 => TagKind::ItemCondition(Codec::get(r)?),
            11 => TagKind::SocketedIn(Codec::get(r)?),
            12 => TagKind::SkillName {
                names: Codec::get(r)?,
                include_transfigured: Codec::get(r)?,
                summon_skill: Codec::get(r)?,
                neg: Codec::get(r)?,
                skill_id: Codec::get(r)?,
            },
            13 => TagKind::SkillId(Codec::get(r)?),
            14 => TagKind::SkillPart {
                list: Codec::get(r)?,
                part: Codec::get(r)?,
                neg: Codec::get(r)?,
            },
            15 => TagKind::SkillType {
                list: Codec::get(r)?,
                skill_type: Codec::get(r)?,
                neg: Codec::get(r)?,
            },
            16 => TagKind::BaseFlag {
                flag: Codec::get(r)?,
                neg: Codec::get(r)?,
            },
            17 => TagKind::SlotName {
                names: Codec::get(r)?,
                neg: Codec::get(r)?,
            },
            18 => TagKind::ModFlagOr(Codec::get(r)?),
            19 => TagKind::KeywordFlagAnd(Codec::get(r)?),
            20 => TagKind::MonsterTag {
                names: Codec::get(r)?,
                neg: Codec::get(r)?,
            },
            21 => TagKind::Other {
                kind: Codec::get(r)?,
                fields: Codec::get(r)?,
            },
            k => return bad("tag", k),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{decode, encode};
    use crate::mods::Sym;
    use std::sync::Arc;

    #[test]
    fn mods_round_trip() {
        let inner = Mod {
            name: Sym::new("Life"),
            typ: ModType::new("BASE"),
            value: ModValue::Number(10.0),
            flags: 1,
            keyword_flags: 2,
            source: Some("Skill:X".into()),
            source_slot: None,
            tags: vec![Tag {
                kind: TagKind::Condition {
                    vars: Names::List(vec![Sym::new("A"), Sym::new("B")]),
                    neg: true,
                    actor: None,
                },
                global_limit: Some((5.0, Sym::new("k"))),
            }]
            .into(),
            unreached_tags: vec![(
                3,
                Tag {
                    kind: TagKind::SocketedIn(Arc::new(SocketedIn {
                        sockets: Some(Sockets::Numbers(vec![1.0])),
                        ..Default::default()
                    })),
                    global_limit: None,
                },
            )],
        };
        let m = Mod {
            typ: ModType::new("CHANCE"),
            value: ModValue::List(vec![
                ModValue::Mod(Box::new(inner.clone())),
                ModValue::Str("x".into()),
                ModValue::Func(Arc::new(Func {
                    name: "f".into(),
                    args: vec![None, Some(ModValue::Bool(true))],
                })),
            ]),
            tags: vec![Tag {
                kind: TagKind::Multiplier(Multiplier {
                    div: Some(2.0),
                    ..Default::default()
                }),
                global_limit: None,
            }]
            .into(),
            ..inner
        };
        assert_eq!(decode::<Mod>(&encode(&m)).unwrap(), m);
    }
}
