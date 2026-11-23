//! `evaluate_item`: what an item does in a slot, evaluated exactly on a
//! copy of the build, and what each mod is worth in that slot (the mod
//! weights, `engine::mod_weights`). Nothing changes in the build.

use crate::read::engine_lines;
use crate::text::{LINE_CAP, NAME_CAP, clean, untrusted};
use crate::write::{ITEM_CAP, diff, item_slots, stats};
use crate::{Agent, BuildId, R, Store, check_fields, cmd, opt_bool, opt_str, opt_u64};
use engine::calculator::MiscCalculator;
use engine::item::Item;
use engine::mod_weights::{
    Eldritch, JewelType, ModWeights, Special, StatWeight, WeightOptions, mod_weights,
};
use engine::parallel::Threads;
use serde_json::{Map, Value as J, json};
use session::Dispatcher;

/// The fields `evaluate_item` takes
pub(crate) const FIELDS: &[&str] = &[
    "build",
    "slot",
    "text",
    "item",
    "weights",
    "stat_weights",
    "corrupted",
    "eldritch",
    "jewel",
    "special",
    "limit",
];

/// What the weights' numbers mean
const GAIN_NOTE: &str = "estimates: each mod at a mid roll on a blank rare of the slot's base, in the slot instead of its item; `gain` is the weighted sum of each stat's ratio to the build's now, over the blank item's, times 1000 (at weight 1, +1% of a stat is 10); `per_point` is the gain per point of the mod's value. Linear: evaluate an item for its exact numbers";

/// The weights' options, from a request
pub(crate) fn weight_options(req: &J) -> R<WeightOptions> {
    let mut o = WeightOptions::default();
    match req.get("stat_weights") {
        None | Some(J::Null) => {}
        Some(J::Array(list)) => {
            if list.is_empty() || list.len() > 20 {
                return Err("stat_weights takes 1 to 20 stats".into());
            }
            o.stat_weights = list
                .iter()
                .map(|w| {
                    let stat = w["stat"]
                        .as_str()
                        .ok_or("each of stat_weights is {\"stat\": \"FullDPS\", \"weight\": 1}")?;
                    let weight = match w.get("weight") {
                        None | Some(J::Null) => 1.0,
                        Some(v) => v.as_f64().ok_or("a stat's weight is a number")?,
                    };
                    StatWeight::new(stat, weight).ok_or_else(|| {
                        format!("no stat {stat:?}; the stats: {}", weight_stats().join(", "))
                    })
                })
                .collect::<R<_>>()?;
        }
        Some(_) => return Err("stat_weights is a list of {\"stat\": ..., \"weight\": ...}".into()),
    }
    o.include_corrupted = opt_bool(req, "corrupted")?.unwrap_or(false);
    o.include_eldritch = match opt_str(req, "eldritch")?.unwrap_or("none") {
        "none" => Eldritch::None,
        "regular" => Eldritch::Regular,
        "presence" => Eldritch::WithPresence,
        e => return Err(format!("no eldritch {e:?}: none, regular, presence")),
    };
    o.jewel_type = match opt_str(req, "jewel")?.unwrap_or("base") {
        "base" => JewelType::Base,
        "abyss" => JewelType::Abyss,
        j => return Err(format!("no jewel {j:?}: base, abyss")),
    };
    o.special = match opt_str(req, "special")? {
        None => None,
        Some("megalomaniac") => Some(Special::Megalomaniac),
        Some("watchers_eye") => Some(Special::WatchersEye),
        Some("pearl_of_tsoatha") => Some(Special::PearlOfTsoatha),
        Some(s) => {
            return Err(format!(
                "no special {s:?}: megalomaniac, watchers_eye, pearl_of_tsoatha"
            ));
        }
    };
    Ok(o)
}

/// The stats `stat_weights` takes
fn weight_stats() -> Vec<String> {
    engine::power::power_stats()
        .into_iter()
        .filter(|p| !p.ignore_for_items)
        .filter_map(|p| p.stat)
        .collect()
}

impl<'d, S: Store> Agent<'d, S> {
    pub(crate) fn evaluate_item(&mut self, id: &BuildId, name: String, req: &J) -> R<J> {
        check_fields(req, FIELDS)?;
        let text = opt_str(req, "text")?;
        let item = opt_u64(req, "item")?;
        let slot = opt_str(req, "slot")?;
        let with_weights = opt_bool(req, "weights")?.unwrap_or(true);
        let limit = opt_u64(req, "limit")?.unwrap_or(15).clamp(1, 200) as usize;
        let opts = weight_options(req)?;
        if text.is_some() && item.is_some() {
            return Err("evaluate_item takes text or item, not both".into());
        }
        if let Some(t) = text
            && t.len() > ITEM_CAP
        {
            return Err(format!("an item's text is at most {ITEM_CAP} bytes"));
        }
        let megalomaniac = opts.special == Some(Special::Megalomaniac);
        if text.is_none() && item.is_none() && slot.is_none() && !megalomaniac {
            return Err(
                "evaluate_item takes an item (`text` as copied in game, or `item`, an id from `items`), a slot, or both"
                    .into(),
            );
        }
        let data = self.data;
        self.with_open(id, |agent, o| {
            let mut out = Map::new();
            out.insert("build".into(), json!(name));
            out.insert("version".into(), json!(o.version));
            let mut slot = slot.map(String::from);
            if let Some(s) = &slot {
                let slots = item_slots(&mut o.session)?;
                if !slots.iter().any(|x| x == s) {
                    return Err(format!("no slot {s:?}; the slots: {}", slots.join(", ")));
                }
            }
            if text.is_some() || item.is_some() {
                let uniques = agent.unique_names(&mut o.session)?.clone();
                // A copy of the build: the live one is not touched
                let mut copy = Dispatcher::typed(data);
                cmd(&mut copy, json!({ "cmd": "loadXml", "xml": o.xml, "name": "copy" }))?;
                let before = stats(&mut copy)?;
                let new_id = match (text, item) {
                    (Some(t), _) => cmd(&mut copy, json!({ "cmd": "addItem", "text": t }))
                        .map_err(|e| format!("the engine can't read this item: {e}"))?["id"]
                        .as_u64()
                        .ok_or("addItem gave no id")?,
                    (_, Some(i)) => i,
                    _ => unreachable!(),
                };
                let items = cmd(&mut copy, json!({ "cmd": "items" }))?;
                let entry = items["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|i| i["id"].as_u64() == Some(new_id))
                    .cloned()
                    .ok_or_else(|| format!("no item {new_id}; `items` lists them"))?;
                let fits: Vec<String> = items["slots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|x| {
                        x["candidates"]
                            .as_array()
                            .is_some_and(|c| c.iter().any(|c| c.as_u64() == Some(new_id)))
                    })
                    .filter_map(|x| x["name"].as_str().map(String::from))
                    .collect();
                let target = match &slot {
                    Some(s) if fits.contains(s) => s.clone(),
                    Some(s) => {
                        return Err(format!(
                            "the item doesn't go in {s}; it goes in: {}",
                            if fits.is_empty() { "no slot".into() } else { fits.join(", ") }
                        ));
                    }
                    None => fits
                        .first()
                        .cloned()
                        .ok_or("the item goes in no slot of this build")?,
                };
                let current = items["slots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|x| x["name"] == target.as_str())
                    .and_then(|x| x["item"].as_u64())
                    .filter(|i| *i != 0);
                let replaces = current.and_then(|c| {
                    items["items"]
                        .as_array()?
                        .iter()
                        .find(|i| i["id"].as_u64() == Some(c))
                        .map(|i| format!("{target}: {}", base_label(data, i["baseName"].as_str())))
                });
                cmd(
                    &mut copy,
                    json!({ "cmd": "equipItem", "slot": target, "id": new_id }),
                )?;
                let after = stats(&mut copy)?;
                let (changed, _) = diff(&before, &after);
                let base = base_label(data, entry["baseName"].as_str());
                out.insert("slot".into(), json!(target));
                out.insert("item".into(), json!(format!("{target}: {base}")));
                out.insert("rarity".into(), entry["rarity"].clone());
                if let Some(n) = entry["name"].as_str()
                    && uniques.contains(n)
                {
                    out.insert("unique".into(), entry["title"].clone());
                }
                let raw = entry["raw"].as_str().unwrap_or_default();
                match Item::new(&data.item_context(), raw, None, false) {
                    Ok(it) => {
                        let (lines, unparsed) = engine_lines(&it);
                        out.insert("lines".into(), json!(lines));
                        if unparsed > 0 {
                            out.insert("unparsed_lines".into(), json!(unparsed));
                        }
                    }
                    Err(_) => {
                        out.insert("lines".into(), json!("the engine can't read this item"));
                    }
                }
                out.insert("fits".into(), json!(fits));
                out.insert("replaces".into(), json!(replaces));
                out.insert("changed".into(), json!(changed));
                out.insert(
                    "exact".into(),
                    json!("`changed` is the engine's full calculation with the item equipped, on a copy of the build; nothing was changed"),
                );
                if text.is_some() {
                    let raw: Vec<String> = raw
                        .lines()
                        .take(100)
                        .map(|l| clean(l, LINE_CAP, false))
                        .collect();
                    let mut f = Map::new();
                    f.insert(
                        "name".into(),
                        json!(clean(entry["name"].as_str().unwrap_or_default(), NAME_CAP, false)),
                    );
                    f.insert("raw".into(), json!(raw));
                    let (k, v) = untrusted(f);
                    out.insert(k, v);
                }
                slot = Some(target);
            }
            if with_weights {
                let build = o.session.typed_build().ok_or("no build is open")?;
                let calc_data = data.calc_data();
                let calc = MiscCalculator::new(build, &calc_data)?;
                let slot = if megalomaniac { None } else { slot.as_deref() };
                let w = mod_weights(
                    &calc,
                    &data.item_context(),
                    &data.game.query_mods,
                    slot,
                    &opts,
                    Threads::available(),
                );
                let w = match w {
                    Ok(w) => weights_json(data, &w, &opts, limit),
                    Err(e) if e.starts_with("not ported") => json!({ "not_ported": e }),
                    Err(e) => return Err(e),
                };
                out.insert("weights".into(), w);
            }
            Ok(J::Object(out))
        })
    }
}

/// A base as answers name it: the game's, else not quoted
fn base_label(data: &session::typed::TypedData, base: Option<&str>) -> String {
    match base {
        Some(b) if data.game.items.base(b).is_some() => b.to_string(),
        _ => "an unknown base".into(),
    }
}

/// The weights as answers give them: the best by gain
fn weights_json(
    data: &session::typed::TypedData,
    w: &ModWeights,
    opts: &WeightOptions,
    limit: usize,
) -> J {
    let mut mods: Vec<_> = w.weights.iter().collect();
    mods.sort_by(|a, b| b.mean_stat_diff.total_cmp(&a.mean_stat_diff));
    let total = mods.len();
    let list: Vec<J> = mods
        .into_iter()
        .take(limit)
        .map(|m| {
            json!({
                "mod": clean(&m.line, LINE_CAP, false),
                "value": m.value,
                "gain": round(m.mean_stat_diff),
                "per_point": round(m.weight),
                "trade_id": m.trade_id,
            })
        })
        .collect();
    let stat_weights: Vec<J> = opts
        .stat_weights
        .iter()
        .map(|s| json!({ "stat": s.stat.stat, "label": s.stat.label, "weight": s.weight }))
        .collect();
    json!({
        "category": w.category,
        "test_base": base_label(data, Some(&w.test_base)),
        "stat_weights": stat_weights,
        "mods": list,
        "total": total,
        "untestable_lines": w.unparsed.len(),
        "estimate": GAIN_NOTE,
    })
}

/// Rounded to 4 significant decimals, for reading
fn round(x: f64) -> f64 {
    (x * 1e4).round() / 1e4
}
