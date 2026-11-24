//! The verbs' fields as JSON schemas, for transports that describe them
//! (MCP's `tools/list`). The verbs check their fields themselves; these
//! schemas say the same.

use serde_json::{Value as J, json};

/// The reading verbs: they change no build
pub const READING: &[&str] = &[
    "builds",
    "summary",
    "tree",
    "skills",
    "items",
    "config",
    "notes",
    "export",
    "search",
    "history",
    "changes_since",
    "evaluate_item",
    "eval",
    "assumptions",
    "best",
    "improve",
    "front",
    "dominating",
    "job",
    "cancel",
];

/// The verbs whose answers can carry the author's text under `untrusted`
pub const UNTRUSTED_TEXT: &[&str] = &["notes", "items", "export", "evaluate_item"];

fn build() -> J {
    json!({
        "type": "string",
        "description": "a build of the grant by name, or agent/<name> in the agent directory; default: the grant's first build",
    })
}

fn expect() -> J {
    json!({
        "type": "integer",
        "minimum": 0,
        "description": "the version the change was planned on; refused if the build changed since",
    })
}

fn node() -> J {
    json!({
        "description": "a node's id, \"#id\", \"Name #id\" or a name only one node has",
        "type": ["integer", "string"],
    })
}

fn stats() -> J {
    json!({
        "type": "array",
        "minItems": 1,
        "maxItems": 12,
        "items": { "type": "string" },
        "description": "the stats to show, as the engine's outputs (FullDPS, TotalEHP, Life, EnergyShield, FireResist, ...); default Full DPS, EHP and the sidebar's stats that changed",
    })
}

fn objective() -> J {
    json!({
        "description": "what to maximise: a preset (default: Full DPS 1, EHP 0.5, the trade weights; dps, ehp, balanced, max_hit, life), a power stat, or a list of {\"stat\", \"weight\"}; a negative weight minimises. Scored as upstream's WeightedRatioOutputs",
        "anyOf": [
            { "type": "string" },
            { "type": "object", "required": ["stat"], "additionalProperties": false, "properties": { "stat": { "type": "string" }, "weight": { "type": "number" } } },
            { "type": "array", "minItems": 1, "maxItems": 10, "items": { "type": "object", "required": ["stat"], "additionalProperties": false, "properties": { "stat": { "type": "string" }, "weight": { "type": "number" } } } },
        ],
    })
}

fn constraints() -> J {
    json!({
        "type": "array",
        "maxItems": 30,
        "description": "hard constraints: \"res_capped\", \"requirements\", \"reservation\", \"no_new_warnings\", {\"stat\": \"Life\", \"min\": 5000}, {\"stat\": ..., \"max\": ...}, {\"keep\": \"TotalEHP\"} (no lower than now), {\"points\": 5} (at most 5 more passive points)",
        "items": { "type": ["string", "object"] },
    })
}

fn search_fields() -> J {
    json!({
        "depth": { "type": "number", "minimum": 1, "maximum": 12, "description": "tree: how many points away a node may be; default 3" },
        "candidates": { "type": "integer", "minimum": 1, "maximum": 500, "description": "how many moves of a ranked or long generator are evaluated exactly; default 40" },
        "group": index("supports: the socket group; default the main one"),
        "gem": index("supports: one gem of the group to replace"),
    })
}

fn job_fields() -> J {
    json!({
        "seconds": { "type": "number", "exclusiveMinimum": 0, "maximum": 600, "description": "the search's time budget; it stops then and answers with the best found so far; default 30" },
        "wait": { "type": "number", "minimum": 0, "maximum": 120, "description": "seconds to wait for the answer; a search still running then answers with its job's id and progress, which `job` asks for again; default 20" },
        "threads": { "type": "integer", "minimum": 1, "maximum": 256, "description": "the most threads to evaluate on; default all cores. The answer is the same with any number" },
    })
}

fn generators(what: &str) -> J {
    let searched = || crate::generate::GENERATORS.iter();
    let names: Vec<&str> = searched().map(|(g, _)| *g).collect();
    let about = searched()
        .map(|(g, d)| format!("{g}: {d}"))
        .collect::<Vec<_>>()
        .join("; ");
    json!({
        "type": "array",
        "minItems": 1,
        "items": { "type": "string", "enum": names },
        "description": format!("{what}. {about}"),
    })
}

fn index(what: &str) -> J {
    json!({ "type": "integer", "minimum": 1, "description": what })
}

/// A change's own fields, without `build` and `expect`
fn change(verb: &str) -> Option<(J, &'static [&'static str])> {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let b = |d: &str| json!({ "type": "boolean", "description": d });
    Some(match verb {
        "allocate" | "deallocate" => (
            json!({ "nodes": { "type": "array", "minItems": 1, "items": node() } }),
            &["nodes"],
        ),
        "set_mastery" => (
            json!({
                "node": node(),
                "effect": {
                    "type": ["integer", "string"],
                    "description": "the effect's id or text",
                },
            }),
            &["node", "effect"],
        ),
        "set_ascendancy" => (
            json!({ "ascendancy": s("the ascendancy's name") }),
            &["ascendancy"],
        ),
        "set_level" => (
            json!({ "level": { "type": "integer", "minimum": 1, "maximum": 100 } }),
            &["level"],
        ),
        "set_gem" => (
            json!({
                "group": index("the socket group, as `skills` counts them"),
                "gem": index("the gem in the group; without it a gem is appended"),
                "name": s("the gem's name"),
                "level": { "type": "integer", "minimum": 1 },
                "quality": { "type": "integer", "minimum": 0 },
                "enabled": b("whether the gem is enabled"),
            }),
            &["group"],
        ),
        "remove_gem" => (
            json!({
                "group": index("the socket group, as `skills` counts them"),
                "gem": index("the gem in the group"),
            }),
            &["group", "gem"],
        ),
        "add_group" => (
            json!({
                "gems": { "type": "array", "items": { "type": "string" }, "description": "gem names" },
                "slot": s("the slot the gems are socketed in"),
                "full_dps": b("whether the group counts in Full DPS"),
            }),
            &["gems"],
        ),
        "set_group" => (
            json!({
                "group": index("the socket group, as `skills` counts them"),
                "enabled": b("whether the group is enabled"),
                "full_dps": b("whether the group counts in Full DPS"),
                "slot": s("the slot, or \"\" for none"),
            }),
            &["group"],
        ),
        "set_main_skill" => (
            json!({
                "group": index("the socket group, as `skills` counts them"),
                "skill": index("the skill of the group, for groups with several"),
            }),
            &["group"],
        ),
        "add_item" => (
            json!({
                "text": s("the item's text, as copied in game or from the trade site"),
                "slot": s("the slot to equip it in"),
            }),
            &["text"],
        ),
        "equip" => (
            json!({
                "slot": s("the slot"),
                "item": {
                    "type": ["integer", "null"],
                    "description": "an item's id as `items` gives it, or null to empty the slot",
                },
            }),
            &["slot"],
        ),
        "set_config" => (
            json!({
                "option": s("the option's variable or label"),
                "value": {
                    "type": ["boolean", "string", "number", "null"],
                    "description": "true or false, a choice, a number, or null to clear it",
                },
            }),
            &["option", "value"],
        ),
        "select_set" => (
            json!({
                "kind": { "type": "string", "enum": ["tree", "skills", "items", "config"] },
                "id": index("the set's id"),
            }),
            &["kind", "id"],
        ),
        _ => return None,
    })
}

/// The verbs `apply` takes
const CHANGES: &[&str] = &[
    "allocate",
    "deallocate",
    "set_mastery",
    "set_ascendancy",
    "set_level",
    "set_gem",
    "remove_gem",
    "add_group",
    "set_group",
    "set_main_skill",
    "add_item",
    "equip",
    "set_config",
    "select_set",
];

/// A verb's request fields as a JSON schema (without `verb`); `None` for a
/// verb this module doesn't describe yet
pub fn input_schema(verb: &str) -> Option<J> {
    let (props, required): (J, &[&str]) = match verb {
        "builds" => (json!({}), &[]),
        "summary" | "tree" | "skills" | "config" | "notes" => (json!({ "build": build() }), &[]),
        "items" => (
            json!({
                "build": build(),
                "text": { "type": "boolean", "description": "adds the author's text (names, raw lines), untrusted" },
            }),
            &[],
        ),
        "export" => (
            json!({ "build": build(), "format": { "type": "string", "enum": ["xml"] } }),
            &[],
        ),
        "search" => (
            json!({
                "build": build(),
                "kind": { "type": "string", "enum": ["nodes", "gems", "uniques", "bases", "config"] },
                "query": { "type": "string", "description": "a name or text to look for" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
            }),
            &["kind"],
        ),
        "history" => (
            json!({ "build": build(), "n": { "type": "integer", "minimum": 1, "maximum": 200 } }),
            &[],
        ),
        "changes_since" => (
            json!({ "build": build(), "version": { "type": "integer", "minimum": 0 } }),
            &[],
        ),
        "evaluate_item" => (
            json!({
                "build": build(),
                "slot": { "type": "string", "description": "the slot to evaluate in; default: the first slot the item fits. A slot alone gives only its mod weights" },
                "text": { "type": "string", "description": "an item's text, as copied in game or from the trade site" },
                "item": { "type": "integer", "minimum": 0, "description": "an item of the build by its id as `items` gives it, instead of text" },
                "weights": { "type": "boolean", "description": "whether to add the slot's mod weights; default true" },
                "stat_weights": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 20,
                    "description": "the stats the weights are worth, as power stats; default Full DPS 1, EHP 0.5",
                    "items": {
                        "type": "object",
                        "required": ["stat"],
                        "additionalProperties": false,
                        "properties": {
                            "stat": { "type": "string", "description": "a power stat (FullDPS, TotalEHP, Life, FireResist, ...)" },
                            "weight": { "type": "number", "description": "default 1" },
                        },
                    },
                },
                "corrupted": { "type": "boolean", "description": "weigh corrupted implicits too" },
                "eldritch": { "type": "string", "enum": ["none", "regular", "presence"], "description": "eldritch implicits to weigh" },
                "jewel": { "type": "string", "enum": ["base", "abyss"], "description": "the jewel type for a jewel socket" },
                "special": { "type": "string", "enum": ["megalomaniac", "watchers_eye", "pearl_of_tsoatha"], "description": "a unique weighed its own way" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200, "description": "how many mods to list; default 15" },
            }),
            &[],
        ),
        "eval" => (
            json!({
                "build": build(),
                "variants": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 500,
                    "description": "the variants, each a list of changes as `apply` takes them (or one change); each is evaluated on its own copy of the build",
                    "items": { "type": ["array", "object"] },
                },
                "stats": stats(),
                "objective": objective(),
                "constraints": constraints(),
            }),
            &["variants"],
        ),
        "best" | "improve" => {
            let mut props = json!({
                "build": build(),
                "objective": objective(),
                "constraints": constraints(),
                "stats": stats(),
            });
            for (k, v) in search_fields().as_object().unwrap() {
                props[k] = v.clone();
            }
            let generators: Vec<&str> = crate::generate::GENERATORS
                .iter()
                .map(|(g, _)| *g)
                .collect();
            let about = crate::generate::GENERATORS
                .iter()
                .map(|(g, d)| format!("{g}: {d}"))
                .collect::<Vec<_>>()
                .join("; ");
            if verb == "best" {
                props["generator"] =
                    json!({ "type": "string", "enum": generators, "description": about });
                props["limit"] = json!({ "type": "integer", "minimum": 1, "maximum": 100, "description": "how many rows; default 10" });
                (props, &["generator"])
            } else {
                props["generators"] = json!({
                    "type": "array",
                    "minItems": 1,
                    "items": { "type": "string", "enum": generators },
                    "description": format!("the generators each step takes moves from; default masteries; tree needs a points constraint. {about}"),
                });
                props["steps"] = json!({ "type": "integer", "minimum": 1, "maximum": 10, "description": "the most steps; default 3" });
                (props, &[])
            }
        }
        "front" | "dominating" => {
            let mut props = json!({
                "build": build(),
                "objectives": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 8,
                    "description": "two or more objectives, each a stat to maximise (\"FullDPS\"; one lower-is-better is minimised), {\"stat\", \"goal\": \"max\" | \"min\"}, or a cost to minimise: \"points\" (passive points added), \"moves\"; default Full DPS and EHP, and the points added when the tree is searched without a points constraint",
                    "items": {
                        "anyOf": [
                            { "type": "string" },
                            { "type": "object", "required": ["stat"], "additionalProperties": false, "properties": { "stat": { "type": "string" }, "goal": { "type": "string", "enum": ["max", "min"] } } },
                        ],
                    },
                },
                "constraints": constraints(),
                "stats": stats(),
                "generators": generators(&format!("the generators whose moves are combined; default {}", crate::front::DEFAULT_GENERATORS.join(", "))),
                "combine": { "type": "integer", "minimum": 1, "maximum": 6, "description": "the most moves a candidate combines; default 3" },
                "beam": { "type": "integer", "minimum": 1, "maximum": 100, "description": "the candidates each round of the beam search extends; default 12" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "how many rows; default 12" },
            });
            for (k, v) in search_fields()
                .as_object()
                .unwrap()
                .iter()
                .chain(job_fields().as_object().unwrap())
            {
                props[k] = v.clone();
            }
            (props, &[])
        }
        "job" => (
            json!({
                "job": { "type": "string", "description": "the job's id, as a search answered it; without it, the jobs of this connection's builds" },
                "wait": { "type": "number", "minimum": 0, "maximum": 120, "description": "seconds to wait for the answer; default 0" },
            }),
            &[],
        ),
        "cancel" => (
            json!({
                "job": { "type": "string", "description": "the job's id" },
                "wait": { "type": "number", "minimum": 0, "maximum": 120, "description": "seconds to wait for its answer, the best found so far; default 10" },
            }),
            &["job"],
        ),
        "assumptions" => (
            json!({
                "build": build(),
                "stat": { "type": "string", "description": "a stat (FullDPS, TotalEHP, Life, ...): adds the options that change it, each evaluated exactly" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "how many options to list; default 15" },
            }),
            &[],
        ),
        "undo" | "redo" => (json!({ "build": build(), "expect": expect() }), &[]),
        "restore" => (
            json!({
                "build": build(),
                "expect": expect(),
                "version": { "type": "string", "description": "a version as `history` gives it" },
            }),
            &["version"],
        ),
        "apply" => (
            json!({
                "build": build(),
                "expect": expect(),
                "changes": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 200,
                    "description": "changes, each {\"verb\": ..., ...} with that verb's fields",
                    "items": {
                        "type": "object",
                        "required": ["verb"],
                        "properties": { "verb": { "type": "string", "enum": CHANGES } },
                    },
                },
            }),
            &["changes"],
        ),
        "copy" => (
            json!({
                "build": build(),
                "name": { "type": "string", "description": "the copy's name in the agent directory" },
            }),
            &["name"],
        ),
        "import" => (
            json!({
                "name": { "type": "string", "description": "the build's name in the agent directory" },
                "code": { "type": "string", "description": "a build code" },
                "xml": { "type": "string", "description": "a build's XML" },
            }),
            &["name"],
        ),
        v => {
            let (fields, required) = change(v)?;
            let mut props = json!({ "build": build(), "expect": expect() });
            for (k, f) in fields.as_object().into_iter().flatten() {
                props[k] = f.clone();
            }
            (props, required)
        }
    };
    Some(json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verb_described() {
        for (v, _) in crate::VERBS {
            let s = input_schema(v).unwrap_or_else(|| panic!("no schema for {v}"));
            for r in s["required"].as_array().unwrap() {
                assert!(s["properties"].get(r.as_str().unwrap()).is_some(), "{v}");
            }
        }
        let s = input_schema("evaluate_item").unwrap();
        let mut props: Vec<&str> = s["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut fields = crate::evaluate::FIELDS.to_vec();
        props.sort();
        fields.sort();
        assert_eq!(props, fields);
        for (v, fields) in [
            ("eval", crate::eval::FIELDS),
            ("assumptions", crate::assume::FIELDS),
            ("best", crate::search::BEST_FIELDS),
            ("improve", crate::search::IMPROVE_FIELDS),
            ("front", crate::front::FIELDS),
            ("dominating", crate::front::FIELDS),
        ] {
            let s = input_schema(v).unwrap();
            let mut props: Vec<&str> = s["properties"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            let mut fields = fields.to_vec();
            props.sort();
            fields.sort();
            assert_eq!(props, fields, "{v}");
        }
        for v in READING.iter().chain(UNTRUSTED_TEXT).chain(CHANGES) {
            assert!(crate::VERBS.iter().any(|(n, _)| n == v), "{v}");
        }
    }
}
