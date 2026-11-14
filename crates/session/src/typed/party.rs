//! The Party tab's view (`party` of `lua/session.lua`): its controls in
//! layout order, as `Classes/PartyTab.lua` lays them out, with the labels
//! `ParseBuffs` gives the imported buffs, or their texts in the edit boxes
//! with the advanced info shown.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedSession};
use engine::mods::{ModType, ModValue};
use engine::party::{BuffList, PartyBuff};
use serde_json::{Value as J, json};

/// The tab's controls without imported data, in layout order
const CONTROLS: &str = r##"[
 {
  "class": "LabelControl",
  "enabled": true,
  "label": "^7To import a build it must be exported with \"Export support\" enabled in the import/export tab\n\tAuras with the highest effect will take priority, your curses will take priority over a support's\n\t\n\tAll of these effects can be found in the Calcs tab",
  "path": "party.notesDesc",
  "shown": true
 },
 {
  "class": "LabelControl",
  "enabled": true,
  "label": "^7---------------------------\n",
  "path": "party.simpleEnemyCond",
  "shown": true
 },
 {
  "class": "LabelControl",
  "enabled": true,
  "label": "^7Enter a build code/URL below:",
  "path": "party.importCodeHeader",
  "shown": true
 },
 {
  "class": "LabelControl",
  "enabled": true,
  "label": "\n",
  "path": "party.simpleEnemyMods",
  "shown": true
 },
 {
  "class": "LabelControl",
  "enabled": true,
  "label": "",
  "path": "party.simpleCurses",
  "shown": true
 },
 {
  "class": "EditControl",
  "enabled": true,
  "path": "party.importCodeIn",
  "shown": true,
  "text": ""
 },
 {
  "class": "LabelControl",
  "enabled": true,
  "label": "",
  "path": "party.importCodeState",
  "shown": true
 },
 {
  "class": "DropDownControl",
  "enabled": true,
  "list": [
   "All",
   "Party Member Stats",
   "Aura",
   "Curse",
   "Warcry Skills",
   "Link Skills",
   "EnemyConditions",
   "EnemyMods"
  ],
  "path": "party.importCodeDestination",
  "sel": 1,
  "shown": true,
  "tooltip": "Destination for Import/clear\nCurrently Links Skills do not export"
 },
 {
  "class": "ButtonControl",
  "enabled": false,
  "label": "Import",
  "path": "party.importCodeGo",
  "shown": true
 },
 {
  "class": "CheckBoxControl",
  "enabled": true,
  "label": "Append",
  "path": "party.appendNotReplace",
  "shown": true,
  "state": false,
  "tooltip": "This sets the import button to append to the current party lists instead of replacing them (curses will still replace)"
 },
 {
  "class": "ButtonControl",
  "enabled": true,
  "label": "Clear",
  "path": "party.clear",
  "shown": true,
  "tooltip": "^7Clears all the party tab imported data"
 },
 {
  "class": "CheckBoxControl",
  "enabled": true,
  "label": "^7Show Advanced Info",
  "path": "party.ShowAdvanceTools",
  "shown": true,
  "state": false,
  "tooltip": "This shows the advanced info like what stats each aura/curse etc are adding, as well as enables the ability to edit them without a re-export\nDo not edit any boxes unless you know what you are doing, use copy/paste or import instead"
 },
 {
  "class": "ButtonControl",
  "enabled": true,
  "label": "Disable Party Effects",
  "path": "party.removeEffects",
  "shown": true,
  "tooltip": "^7Removes the effects of the supports, without removing the data\nUse \"rebuild all\" to apply the effects again"
 },
 {
  "class": "ButtonControl",
  "enabled": true,
  "label": "^7Rebuild All",
  "path": "party.rebuild",
  "shown": true,
  "tooltip": "^7Reparse all the inputs in case they have been disabled or they have changed since loading the build or importing"
 },
 {
  "class": "LabelControl",
  "enabled": true,
  "label": "",
  "path": "party.simpleAuras",
  "shown": true
 },
 {
  "class": "LabelControl",
  "enabled": true,
  "label": "",
  "path": "party.simpleWarcries",
  "shown": true
 },
 {
  "class": "LabelControl",
  "enabled": true,
  "label": "",
  "path": "party.simpleLinks",
  "shown": true
 }
]"##;

/// `partyDestinations`, which the import destination drop-down lists
const DESTINATIONS: [&str; 8] = [
    "All",
    "Party Member Stats",
    "Aura",
    "Curse",
    "Warcry Skills",
    "Link Skills",
    "EnemyConditions",
    "EnemyMods",
];

/// `theme.lineCounter`: 16 pixels a newline
fn line_counter(label: &str) -> i64 {
    label.bytes().filter(|&c| c == b'\n').count() as i64 * 16
}

/// A number as Lua's `tostring` writes it
fn lua_num(n: f64) -> String {
    lua::value::format_number(n)
}

/// `tostring(mod.value)` of a party label
fn value_text(v: &ModValue) -> Result<String> {
    match v {
        ModValue::Number(n) => Ok(lua_num(*n)),
        ModValue::Bool(b) => Ok(b.to_string()),
        ModValue::Str(s) => Ok(s.to_string()),
        _ => Err(format!("{NOT_PORTED}: a party label of a table value")),
    }
}

/// `"..mod.type.." "..mod.name..": "..tostring(mod.value)` lines of a label
fn mod_lines(out: &mut String, mods: &[engine::mods::Mod]) -> Result<()> {
    for m in mods {
        let typ = match m.typ {
            ModType::Flag => String::new(),
            t => format!("{} ", t.name()),
        };
        out.push_str(&format!(
            "  {typ}{}: {}\n",
            m.name.as_str(),
            value_text(&m.value)?
        ));
    }
    Ok(())
}

/// `aura..": "..auraMod.effectMult.."%\n"`
fn effect_line(name: &str, b: &PartyBuff) -> Result<String> {
    let e = match (&b.effect_text, b.effect_mult) {
        (Some(t), _) => t.clone(),
        (None, Some(n)) => lua_num(n),
        (None, None) => return Err("attempt to concatenate a nil value (effectMult)".into()),
    };
    Ok(format!("{name}: {e}%\n"))
}

/// The sorted lines framed by dividers, or `""`
fn framed(mut lines: Vec<String>) -> String {
    if lines.is_empty() {
        return String::new();
    }
    lines.sort();
    format!(
        "^7---------------------------\n{}---------------------------\n",
        lines.concat()
    )
}

/// `ParseBuffs`' label of the auras
fn aura_label(list: &BuffList) -> Result<String> {
    let mut label = "^7---------------------------\n".to_string();
    let mut lines = Vec::new();
    let aura = list.group("Aura");
    if let Some(g) = aura {
        for (n, b) in &g.buffs {
            lines.push(effect_line(n, b)?);
        }
        for (n, b) in g.vaal.iter().flatten() {
            lines.push(effect_line(n, b)?);
        }
    }
    if let Some(g) = list.group("AuraDebuff") {
        for (n, b) in &g.buffs {
            if aura.is_none_or(|a| a.get(n).is_none()) {
                lines.push(effect_line(n, b)?);
            }
        }
        // Upstream checks `list.Aura.Vaal[aura]` with `aura` nil
        for (n, b) in g.vaal.iter().flatten() {
            lines.push(effect_line(n, b)?);
        }
    }
    if !lines.is_empty() {
        lines.sort();
        label.push_str(&lines.concat());
        label.push_str("---------------------------\n");
    }
    if let Some(b) = list.group("extraAura").and_then(|g| g.get("extraAura")) {
        label.push_str("extraAuras:\n");
        mod_lines(&mut label, &b.mods)?;
        label.push_str("---------------------------\n");
    }
    if let Some(g) = list.group("otherEffects") {
        label.push_str("otherEffects:\n");
        for (_, b) in &g.buffs {
            mod_lines(&mut label, &b.mods)?;
        }
        label.push_str("---------------------------\n");
    }
    Ok(label)
}

/// `ParseBuffs`' label of warcries, links or curses
fn buff_label(list: &BuffList, typ: &str) -> Result<String> {
    let mut lines = Vec::new();
    if let Some(g) = list.group(typ) {
        for (n, b) in g.buffs.iter().chain(g.vaal.iter().flatten()) {
            lines.push(effect_line(n, b)?);
        }
    }
    Ok(framed(lines))
}

impl TypedSession<'_> {
    /// `party`: the Party tab's controls, in the order of their
    /// positions, as upstream lays them out on a tab never drawn (width 0)
    pub fn party(&self) -> Result<J> {
        let p = self
            .root
            .child("PathOfBuilding")
            .and_then(|p| p.child("Party"));
        let attr = |k: &str| p.and_then(|p| p.attr(k));
        let advanced = attr("ShowAdvanceTools") == Some("true");
        let dest = attr("destination").unwrap_or("All");
        let sel = DESTINATIONS
            .iter()
            .position(|d| *d == dest)
            .ok_or_else(|| format!("{NOT_PORTED}: a party destination {dest}"))?;
        let party = &self.build.party;
        let text = |name: &str| {
            party
                .texts
                .iter()
                .rev()
                .find(|(n, _)| n == name)
                .map(|(_, t)| t.clone())
        };
        // The labels `PartyTab:Load` gives, where it loads the buffs
        let label = |name: &str, f: &dyn Fn() -> Result<String>, default: &str| -> Result<String> {
            match text(name) {
                Some(_) => f(),
                None => Ok(default.to_string()),
            }
        };
        let auras = label("Aura", &|| aura_label(&party.aura), "")?;
        let warcries = label("Warcry Skills", &|| buff_label(&party.warcry, "Warcry"), "")?;
        let links = label("Link Skills", &|| buff_label(&party.link, "Link"), "")?;
        let curses = label("Curse", &|| buff_label(&party.curse, "Curse"), "")?;
        let enemy_mods = party
            .enemy_mods_label
            .clone()
            .unwrap_or_else(|| "\n".into());
        let enemy_cond = "^7---------------------------\n";

        let template: Vec<J> = serde_json::from_str(CONTROLS).map_err(|e| e.to_string())?;
        let mut by_path: std::collections::HashMap<String, J> = template
            .into_iter()
            .map(|c| (c["path"].as_str().unwrap_or("").to_string(), c))
            .collect();
        let mut take = |name: &str| by_path.remove(&format!("party.{name}")).unwrap_or(J::Null);
        // `notesDesc`'s width wraps its label at every space (no string
        // has a width headless). `layoutControls` positions the controls
        // by name: on the first layout `ShowAdvanceTools`, the first, sees
        // the notes' height before their width wraps them; later controls
        // and layouts see them wrapped
        let mut notes = take("notesDesc");
        let original = notes["label"].as_str().unwrap_or("").to_string();
        let wrapped = original
            .split([' ', '\t', '\n', '\r', '\u{b}', '\u{c}'])
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        notes["label"] = json!(wrapped);
        let header_wrapped = 8 + 16 + line_counter(&wrapped) + 4;
        let h = header_wrapped;
        let first = !self.party_laid_out.replace(true);
        let tools_h = if first {
            8 + 16 + line_counter(&original) + 4
        } else {
            h
        };
        let mut rows: Vec<(i64, i64, J)> = Vec::new();
        let mut put = |x: i64, y: i64, c: J| rows.push((y, x, c));
        put(8, 8, notes);
        put(8, h, take("importCodeHeader"));
        put(8, h + 20, take("importCodeIn"));
        put(-84, h + 22, take("importCodeState"));
        let mut d = take("importCodeDestination");
        d["sel"] = json!(sel + 1);
        put(8, h + 44, d);
        put(176, h + 44, take("importCodeGo"));
        let mut a = take("appendNotReplace");
        a["state"] = json!(attr("append") == Some("true"));
        put(60, h + 68, a);
        put(88, h + 68, take("clear"));
        let mut t = take("ShowAdvanceTools");
        t["state"] = json!(advanced);
        put(148, tools_h + 92, t);
        put(176, h + 92, take("removeEffects"));
        put(8, h + 116, take("rebuild"));
        let edit = |name: &str, label: &str, buff: &str| {
            json!({
                "class": "EditControl",
                "enabled": true,
                "shown": true,
                "path": format!("party.{name}"),
                "text": text(buff).unwrap_or_default(),
                "multiline": true,
                "label": label,
            })
        };
        let mut simple = |name: &str, label: &str| {
            let mut c = take(name);
            c["label"] = json!(label);
            c
        };
        // Left: auras, warcries, links under the import controls
        let auras_y = h + 152;
        // Right, beside the notes: party member stats, enemy conditions
        // and modifiers, curses
        if advanced {
            let aura_h = -378 - 24 - (header_wrapped - 24) - 60;
            let warcries_y = auras_y + 16 + aura_h + 8;
            let links_y = warcries_y + 16 + 106 + 8;
            put(8, auras_y + 18, edit("editAuras", "^7Auras", "Aura"));
            put(
                8,
                warcries_y + 18,
                edit("editWarcries", "^7Warcry Skills", "Warcry Skills"),
            );
            put(
                8,
                links_y + 18,
                edit("editLinks", "^7Link Skills", "Link Skills"),
            );
            let cond_y = 8 + 16 + 114;
            let mods_y = cond_y + 16 + 114;
            let curses_y = mods_y + 16 + 114;
            put(
                0,
                26,
                edit(
                    "editPartyMemberStats",
                    "^7Party Member Stats",
                    "PartyMemberStats",
                ),
            );
            put(
                0,
                cond_y + 18,
                edit("enemyCond", "^7Enemy Conditions", "EnemyConditions"),
            );
            put(
                0,
                mods_y + 18,
                edit("enemyMods", "^7Enemy Modifiers", "EnemyMods"),
            );
            put(0, curses_y + 18, edit("editCurses", "^7Curses", "Curse"));
        } else {
            let warcries_y = auras_y + 16 + line_counter(&auras) + 4;
            let links_y = warcries_y + 16 + line_counter(&warcries) + 4;
            put(8, auras_y + 18, simple("simpleAuras", &auras));
            put(8, warcries_y + 18, simple("simpleWarcries", &warcries));
            put(8, links_y + 18, simple("simpleLinks", &links));
            let cond_y = 8 + 16 + 4;
            let mods_y = cond_y + 16 + line_counter(enemy_cond) + 4;
            let curses_y = mods_y + 16 + line_counter(&enemy_mods) + 4;
            put(0, cond_y + 18, simple("simpleEnemyCond", enemy_cond));
            put(0, mods_y + 18, simple("simpleEnemyMods", &enemy_mods));
            put(0, curses_y + 18, simple("simpleCurses", &curses));
        }
        rows.sort_by(|a, b| {
            (a.0, a.1)
                .cmp(&(b.0, b.1))
                .then_with(|| a.2["path"].as_str().cmp(&b.2["path"].as_str()))
        });
        let controls: Vec<J> = rows.into_iter().map(|(_, _, c)| c).collect();
        Ok(json!({ "controls": controls }))
    }
}
