//! The typed Skills tab (`engine::skills`) loads what the translated
//! program's loads, for every corpus build loaded from XML: the skill sets,
//! the gem defaults, and every socket group with its gems' data, levels,
//! qualities, requirements and errors. The build's XML is read with the
//! typed XML reader (`engine::xml`).
//!
//! `CALCS_FILTER=<substring>` limits the builds run.

use engine::skill_data::SkillData;
use engine::skills::{GemInstance, Skills, SocketGroup};
use equivalence::{corpus, upstream_src};
use lua::rt;
use lua::{LuaError, Value};
use std::rc::Rc;
use upstream::host::Host;
use upstream::imported::Imported;

fn s(x: &str) -> Value {
    Value::str(x)
}

fn get(v: &Value, k: &str) -> Value {
    rt::index(v, &s(k)).unwrap_or_default()
}

fn describe(e: &LuaError) -> String {
    format!(
        "{}\n  {}",
        lua::to_string(e.value()),
        e.traceback().join("\n  ")
    )
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(|s| s.to_str_lossy().into_owned())
}

fn array(v: &Value) -> Vec<Value> {
    v.as_table().map(|t| t.array()).unwrap_or_default()
}

fn compare_gem(t: &GemInstance, l: &Value, fail: &mut dyn FnMut(String)) {
    let mut check = |field: &str, ok: bool, detail: String| {
        if !ok {
            fail(format!("{field}: {detail}"));
        }
    };
    for (field, tv) in [
        ("nameSpec", Some(t.name_spec.clone())),
        ("gemId", t.gem_id.clone()),
        ("skillId", t.skill_id.clone()),
        ("errMsg", t.err_msg.clone()),
        ("color", Some(t.color.clone())),
        ("skillMinion", t.skill_minion.clone()),
        ("skillMinionCalcs", t.skill_minion_calcs.clone()),
    ] {
        let lv = text(&get(l, field));
        check(field, lv == tv, format!("{tv:?} != {lv:?}"));
    }
    for (field, tv) in [
        ("level", t.level),
        ("quality", t.quality),
        ("count", Some(t.count)),
        ("skillPart", t.skill_part),
        ("skillPartCalcs", t.skill_part_calcs),
        ("skillStageCount", t.skill_stage_count),
        ("skillStageCountCalcs", t.skill_stage_count_calcs),
        ("skillMineCount", t.skill_mine_count),
        ("skillMineCountCalcs", t.skill_mine_count_calcs),
        ("skillMinionItemSet", t.skill_minion_item_set),
        ("skillMinionItemSetCalcs", t.skill_minion_item_set_calcs),
        ("skillMinionSkill", t.skill_minion_skill),
        ("skillMinionSkillCalcs", t.skill_minion_skill_calcs),
        ("reqLevel", t.req_level),
        ("reqStr", t.req_str),
        ("reqDex", t.req_dex),
        ("reqInt", t.req_int),
        ("naturalMaxLevel", t.natural_max_level),
    ] {
        let lv = get(l, field).as_number();
        check(field, lv == tv, format!("{tv:?} != {lv:?}"));
    }
    for (field, tv) in [
        ("enabled", t.enabled),
        ("enableGlobal1", t.enable_global1),
        ("enableGlobal2", t.enable_global2),
    ] {
        let lv = get(l, field).truthy();
        check(field, lv == tv, format!("{tv} != {lv}"));
    }
    let lgem = text(&get(&get(l, "gemData"), "id"));
    check(
        "gemData",
        lgem == t.gem.as_ref().map(|g| g.id.clone()),
        format!("{lgem:?}"),
    );
    let leffect = text(&get(&get(l, "grantedEffect"), "id"));
    let teffect = t.granted_effect.as_ref().map(|g| g.id.clone());
    check(
        "grantedEffect",
        leffect == teffect,
        format!("{teffect:?} != {leffect:?}"),
    );
}

fn compare_group(t: &SocketGroup, l: &Value, fail: &mut dyn FnMut(String)) {
    let mut check = |field: &str, ok: bool, detail: String| {
        if !ok {
            fail(format!("{field}: {detail}"));
        }
    };
    for (field, tv) in [
        ("label", t.label.clone()),
        ("slot", t.slot.clone()),
        ("source", t.source.clone()),
        ("imbuedSupport", t.imbued_support.clone()),
    ] {
        let lv = text(&get(l, field));
        check(field, lv == tv, format!("{tv:?} != {lv:?}"));
    }
    for (field, tv) in [
        ("groupCount", t.group_count),
        ("mainActiveSkill", Some(t.main_active_skill)),
        ("mainActiveSkillCalcs", Some(t.main_active_skill_calcs)),
    ] {
        let lv = get(l, field).as_number();
        check(field, lv == tv, format!("{tv:?} != {lv:?}"));
    }
    for (field, tv) in [
        ("enabled", t.enabled),
        ("includeInFullDPS", t.include_in_full_dps),
    ] {
        let lv = get(l, field).truthy();
        check(field, lv == tv, format!("{tv} != {lv}"));
    }
    let lgems = array(&get(l, "gemList"));
    if lgems.len() != t.gem_list.len() {
        fail(format!(
            "gemList: {} gems != {}",
            t.gem_list.len(),
            lgems.len()
        ));
        return;
    }
    for (i, (tg, lg)) in t.gem_list.iter().zip(&lgems).enumerate() {
        compare_gem(tg, lg, &mut |f| {
            fail(format!("gem {} ({}): {f}", i + 1, tg.name_spec))
        });
    }
}

#[test]
fn skills_match_upstream() {
    let filter = std::env::var("CALCS_FILTER").unwrap_or_default();
    let host = Host::new(Rc::new(Imported::new(upstream_src())));
    host.start()
        .unwrap_or_else(|e| panic!("upstream failed to start: {}", describe(&e)));
    let data = SkillData::from_lua(&host.global("data"), &host.global("colorCodes"))
        .unwrap_or_else(|e| panic!("skill data: {e}"));
    let mut failures = Vec::new();
    let mut compared = 0;
    for (id, path) in corpus() {
        if !id.contains(&filter) || path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        let text_xml = std::fs::read_to_string(&path).unwrap();
        host.load_build_xml(text_xml.as_bytes(), &id)
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let build = host.build().unwrap();
        let tab = get(&build, "skillsTab");
        // Load again, so nothing the calculations change after loading shows
        let common = host.global("common");
        let parsed = rt::call(&get(&get(&common, "xml"), "ParseXML"), &[s(&text_xml)])
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)))
            .into_iter()
            .next()
            .unwrap();
        let pob = array(&parsed)
            .into_iter()
            .find(|n| text(&get(n, "elem")).as_deref() == Some("PathOfBuilding"))
            .unwrap();
        let Some(lskills) = array(&pob)
            .into_iter()
            .find(|n| text(&get(n, "elem")).as_deref() == Some("Skills"))
        else {
            continue;
        };
        rt::method(&tab, &"Load".into(), &[lskills, s(&id)])
            .unwrap_or_else(|e| panic!("{id}: {}", describe(&e)));
        let root = engine::xml::parse(&text_xml).unwrap_or_else(|e| panic!("{id}: {e}"));
        let Some(skills_xml) = root.child("PathOfBuilding").and_then(|p| p.child("Skills")) else {
            continue;
        };
        compared += 1;
        let skills = Skills::load(skills_xml, &data);
        let mut fail = |what: String| {
            if failures.len() < 60 {
                failures.push(format!("{id}: {what}"));
            }
        };
        let lactive = get(&tab, "activeSkillSetId").as_number();
        if lactive != Some(skills.active_skill_set_id) {
            fail(format!(
                "activeSkillSetId: {} != {lactive:?}",
                skills.active_skill_set_id
            ));
        }
        let lorder: Vec<f64> = array(&get(&tab, "skillSetOrderList"))
            .iter()
            .filter_map(Value::as_number)
            .collect();
        if lorder != skills.skill_set_order_list {
            fail(format!(
                "skillSetOrderList: {:?} != {lorder:?}",
                skills.skill_set_order_list
            ));
        }
        let lq = get(&tab, "defaultGemQuality").as_number();
        if lq != Some(skills.default_gem_quality) {
            fail(format!(
                "defaultGemQuality: {} != {lq:?}",
                skills.default_gem_quality
            ));
        }
        let lsets = get(&tab, "skillSets");
        for set in skills.skill_sets.values() {
            let lset = rt::index(&lsets, &Value::Number(set.id)).unwrap_or_default();
            let ltitle = text(&get(&lset, "title"));
            if ltitle != set.title {
                fail(format!(
                    "skill set {} title: {:?} != {ltitle:?}",
                    set.id, set.title
                ));
            }
            let lgroups = array(&get(&lset, "socketGroupList"));
            if lgroups.len() != set.socket_group_list.len() {
                fail(format!(
                    "skill set {}: {} groups != {}",
                    set.id,
                    set.socket_group_list.len(),
                    lgroups.len()
                ));
                continue;
            }
            for (i, (tg, lg)) in set.socket_group_list.iter().zip(&lgroups).enumerate() {
                compare_group(tg, lg, &mut |f| {
                    fail(format!("set {} group {}: {f}", set.id, i + 1))
                });
            }
        }
        let limbued = get(&tab, "imbuedSupportBySlot");
        for (slot, effect) in &skills.imbued_support_by_slot {
            let l = text(&get(&get(&limbued, slot), "id"));
            if l.as_ref() != Some(effect) {
                fail(format!("imbuedSupportBySlot[{slot}]: {effect} != {l:?}"));
            }
        }
    }
    eprintln!("{compared} skill tabs compared");
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
