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
];

/// The verbs whose answers can carry the author's text under `untrusted`
pub const UNTRUSTED_TEXT: &[&str] = &["notes", "items", "export"];

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
        "undo" => (json!({ "build": build(), "expect": expect() }), &[]),
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
        for v in READING.iter().chain(UNTRUSTED_TEXT).chain(CHANGES) {
            assert!(crate::VERBS.iter().any(|(n, _)| n == v), "{v}");
        }
    }
}
