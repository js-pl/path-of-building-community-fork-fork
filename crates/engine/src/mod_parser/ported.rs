//! ModParser closures portgen cannot translate, ported by hand. Each entry
//! records the FNV-1a hash of the Lua source it was ported from; when
//! upstream changes the closure, `ModParser::new` fails until the port is
//! updated and the hash with it.

use super::{Ctx, Port, closure};
use crate::rt::*;
use lua::Value;

pub(super) const PORTS: &[Port] = &[
    Port {
        key: "specialModList[\"non%-unique jewels cause increases and reductions to other damage types in a (%a+) radius to be transformed to apply to (%a+) damage\"]",
        hash: 0x9dfbe79a12e90895,
        f: non_unique_jewels_transform_damage,
    },
    Port {
        key: "specialModList[\"non%-unique jewels cause small and notable passive skills in a (%a+) radius to also grant %+(%d+) to (%a+)\"]",
        hash: 0x39de41df5eb1cfd8,
        f: non_unique_jewels_grant_attribute,
    },
    Port {
        key: "specialModList[\"([%+%-]%d+)%%? to (%a+) of all (.+) gems\"]",
        hash: 0xa19df17002211df2,
        f: all_gems_property,
    },
];

fn non_unique_item_conditions() -> [Value; 2] {
    let cond = |rarity: &str| {
        tbl(
            vec![
                (s("type"), s("ItemCondition")),
                (s("itemSlot"), s("{SlotName}")),
                (s("rarityCond"), s(rarity)),
                (s("neg"), Value::Bool(true)),
            ],
            vec![],
        )
    };
    [cond("UNIQUE"), cond("RELIC")]
}

/// ModParser.lua:2611. The jewel function is a closure the calcs run.
fn non_unique_jewels_transform_damage(c: &Ctx, a: &[Value]) -> Vec<Value> {
    let (radius, dmg_type) = (arg(a, 1), arg(a, 2));
    let func = closure("nonUniqueJewelsTransformDamage", vec![dmg_type]);
    let v = tbl(
        vec![
            (s("radius"), first(c.first_to_upper(vec![radius]))),
            (s("type"), s("Other")),
            (s("func"), func),
        ],
        vec![],
    );
    let [u, r] = non_unique_item_conditions();
    vec![tbl(
        vec![],
        vec![first(c.mod_(vec![s("ExtraJewelFunc"), s("LIST"), v, u, r]))],
    )]
}

/// ModParser.lua:2619. The jewel function is a closure the calcs run.
fn non_unique_jewels_grant_attribute(c: &Ctx, a: &[Value]) -> Vec<Value> {
    let (radius, val, attr) = (arg(a, 1), arg(a, 2), arg(a, 3));
    let func = closure("nonUniqueJewelsGrantAttribute", vec![val, attr]);
    let radius = first(gsub_fn(&radius, &s("^%l"), &mut upper_fn, &nil()));
    let v = tbl(
        vec![
            (s("radius"), radius),
            (s("type"), s("Other")),
            (s("func"), func),
        ],
        vec![],
    );
    let [u, r] = non_unique_item_conditions();
    vec![tbl(
        vec![],
        vec![first(c.mod_(vec![s("ExtraJewelFunc"), s("LIST"), v, u, r]))],
    )]
}

/// ModParser.lua:3228
fn all_gems_property(c: &Ctx, a: &[Value]) -> Vec<Value> {
    let (num, property, skill) = (arg(a, 0), arg(a, 2), arg(a, 3));
    if index(&c.value("gemIdLookup"), &skill).truthy() {
        let v = tbl(
            vec![
                (s("keyword"), skill),
                (s("key"), s("level")),
                (s("value"), num),
                (s("keyOfScaledMod"), s("value")),
            ],
            vec![],
        );
        return vec![tbl(
            vec![],
            vec![first(c.mod_(vec![s("GemProperty"), s("LIST"), v]))],
        )];
    }
    let word_list = tbl(vec![], vec![]);
    for caps in gmatch(&skill, &s("%S+")) {
        t_insert(&word_list, nth(&caps, 0));
    }
    let v = tbl(
        vec![
            (s("keywordList"), word_list),
            (s("key"), property),
            (s("value"), num),
            (s("keyOfScaledMod"), s("value")),
        ],
        vec![],
    );
    vec![tbl(
        vec![],
        vec![first(c.mod_(vec![s("GemProperty"), s("LIST"), v]))],
    )]
}
