//! The Party tab's view (`party` of `lua/session.lua`): its controls in
//! layout order, as `Classes/PartyTab.lua` lays them out. A build with
//! imported party data, whose labels show the parsed buffs, or with the
//! advanced info shown is left to the translation.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::{NOT_PORTED, Result, TypedSession};
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

impl TypedSession<'_> {
    /// `party`: the Party tab's controls
    pub fn party(&self) -> Result<J> {
        let p = self
            .root
            .child("PathOfBuilding")
            .and_then(|p| p.child("Party"));
        let attr = |k: &str| p.and_then(|p| p.attr(k));
        let imported = p
            .iter()
            .flat_map(|p| p.elements())
            .any(|e| e.elem == "ImportedBuffs" && e.text().is_some_and(|t| !t.is_empty()));
        if imported || attr("ShowAdvanceTools") == Some("true") {
            return Err(format!("{NOT_PORTED}: the Party tab with imported data"));
        }
        let dest = attr("destination").unwrap_or("All");
        let sel = DESTINATIONS
            .iter()
            .position(|d| *d == dest)
            .ok_or_else(|| format!("{NOT_PORTED}: a party destination {dest}"))?;
        let mut controls: J = serde_json::from_str(CONTROLS).map_err(|e| e.to_string())?;
        for c in controls.as_array_mut().into_iter().flatten() {
            match c["path"].as_str() {
                Some("party.importCodeDestination") => c["sel"] = json!(sel + 1),
                Some("party.appendNotReplace") => {
                    c["state"] = json!(attr("append") == Some("true"))
                }
                _ => {}
            }
        }
        Ok(json!({ "controls": controls }))
    }
}
