//! The Party tab's controls (`party.*`): the import code and its
//! destination, Import, Append, Clear, Show Advanced Info, Disable Party
//! Effects, Rebuild All and the edit boxes, as `Classes/PartyTab.lua`
//! handles them. The tab keeps its edit boxes' texts, its labels and its
//! parsed buffs (the actor) apart: an edited box is parsed only by Rebuild
//! All or an import, and Disable Party Effects empties the actor alone.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use super::party::{DESTINATIONS, aura_label, buff_label};
use super::{NOT_PORTED, Result, TypedSession};
use engine::party::{BuffList, Party};
use serde_json::{Value as J, json};
use std::sync::Arc;

/// The edit boxes by the names their texts are saved under
pub(super) const BUFS: [&str; 7] = [
    "PartyMemberStats",
    "Aura",
    "Curse",
    "Warcry Skills",
    "Link Skills",
    "EnemyConditions",
    "EnemyMods",
];

/// The tab's controls (`PartyTab.controls`)
const PARTY_CONTROLS: [&str; 31] = [
    "notesDesc",
    "importCodeHeader",
    "importCodeIn",
    "importCodeState",
    "importCodeDestination",
    "importCodeGo",
    "appendNotReplace",
    "clear",
    "ShowAdvanceTools",
    "removeEffects",
    "rebuild",
    "editAurasLabel",
    "editAuras",
    "simpleAuras",
    "editWarcriesLabel",
    "editWarcries",
    "simpleWarcries",
    "editLinksLabel",
    "editLinks",
    "simpleLinks",
    "editPartyMemberStatsLabel",
    "editPartyMemberStats",
    "enemyCondLabel",
    "enemyCond",
    "simpleEnemyCond",
    "enemyModsLabel",
    "enemyMods",
    "simpleEnemyMods",
    "editCursesLabel",
    "editCurses",
    "simpleCurses",
];

const POSITIVE: &str = "^x33FF77";
const NEGATIVE: &str = "^xDD0022";
const ENEMY_COND: &str = "^7---------------------------\n";

/// `buildSites.websiteList`: label and `matchURL`
const SITES: [(&str, &str); 8] = [
    ("Maxroll", "^https://maxroll%.gg/poe/pob/.*"),
    ("pob.codes", "^https://pob%.codes/b/.+"),
    ("pobb.in", "^https://pobb%.in/.+"),
    ("PoeNinja", "^https://poe%.ninja/?p?o?e?1?/pob/%w+"),
    ("Pastebin.com", "^https://pastebin%.com/%w+"),
    ("PastebinP.com", "^https://pastebinp%.com/%w+"),
    ("Rentry.co", "^https://rentry%.co/%w+"),
    ("poedb.tw", "^https://poedb%.tw/.+"),
];

/// What the tab holds besides the actor (`build.party`)
#[derive(Clone, Debug)]
pub(super) struct PartyTab {
    /// The edit boxes' texts, as [`BUFS`]
    pub bufs: [String; 7],
    /// `simpleAuras`, `simpleWarcries`, `simpleLinks`, `simpleCurses`
    pub auras: String,
    pub warcries: String,
    pub links: String,
    pub curses: String,
    /// `simpleEnemyCond`, `simpleEnemyMods`
    pub enemy_cond: String,
    pub enemy_mods: String,
    /// `importCodeDestination`'s index (0 = All)
    pub dest: usize,
    pub append: bool,
    pub advanced: bool,
    /// `importCodeIn`'s text and what `importCodeHandle` made of it
    pub code: String,
    pub detail: String,
    pub valid: bool,
    pub xml: Option<String>,
    pub site: bool,
}

impl PartyTab {
    fn buf(&mut self, name: &str) -> &mut String {
        let i = BUFS.iter().position(|b| *b == name).unwrap();
        &mut self.bufs[i]
    }

    /// `importCodeHandle(buf)`
    fn handle_code(&mut self) -> Result<()> {
        self.site = false;
        self.detail = String::new();
        self.xml = None;
        self.valid = false;
        if self.code.is_empty() {
            return Ok(());
        }
        self.detail = format!("{NEGATIVE}Invalid input");
        let url = engine::pat::gsub(&engine::pat::gsub(&self.code, "^[%s?]+", ""), "[%s?]+$", "");
        if engine::pat::is_match(&url, "youtube%.com/redirect%?")
            || engine::pat::is_match(&url, "google%.com/url%?")
        {
            return Err(format!("{NOT_PORTED}: a redirect link as a party import"));
        }
        for (label, pattern) in SITES {
            if engine::pat::is_match(&url, pattern) {
                self.valid = true;
                self.detail = format!("{POSITIVE}URL is valid ({label})");
                self.site = true;
                self.code = url;
                return Ok(());
            }
        }
        let bytes = engine::spec::base64_decode(&self.code.replace('-', "+").replace('_', "/"));
        let Ok(xml) = miniz_oxide::inflate::decompress_to_vec_zlib(&bytes) else {
            return Ok(());
        };
        self.valid = true;
        self.detail = format!("{POSITIVE}Code is valid");
        self.xml = Some(String::from_utf8_lossy(&xml).into_owned());
        Ok(())
    }

    /// Whether an import or a clear goes to destination `k`
    fn hits(&self, k: usize) -> bool {
        self.dest == 0 || self.dest == k
    }
}

impl TypedSession<'_> {
    /// The tab as `PartyTab:Load` leaves it
    pub(super) fn initial_party_tab(&self) -> Result<PartyTab> {
        let p = self
            .root
            .child("PathOfBuilding")
            .and_then(|p| p.child("Party"));
        let attr = |k: &str| p.and_then(|p| p.attr(k));
        let party = &self.build.party;
        let text = |name: &str| {
            party
                .texts
                .iter()
                .rev()
                .find(|(n, _)| n == name)
                .map(|(_, t)| t.clone())
        };
        let label = |name: &str, f: &dyn Fn() -> Result<String>| -> Result<String> {
            match text(name) {
                Some(_) => f(),
                None => Ok(String::new()),
            }
        };
        Ok(PartyTab {
            bufs: BUFS.map(|b| text(b).unwrap_or_default()),
            auras: label("Aura", &|| aura_label(&party.aura))?,
            warcries: label("Warcry Skills", &|| buff_label(&party.warcry, "Warcry"))?,
            links: label("Link Skills", &|| buff_label(&party.link, "Link"))?,
            curses: label("Curse", &|| buff_label(&party.curse, "Curse"))?,
            enemy_cond: ENEMY_COND.into(),
            enemy_mods: party
                .enemy_mods_label
                .clone()
                .unwrap_or_else(|| "\n".into()),
            // `SelByValue` keeps the first entry for one it does not have
            dest: attr("destination")
                .and_then(|d| DESTINATIONS.iter().position(|x| *x == d))
                .unwrap_or(0),
            append: attr("append") == Some("true"),
            advanced: attr("ShowAdvanceTools") == Some("true"),
            code: String::new(),
            detail: String::new(),
            valid: false,
            xml: None,
            site: false,
        })
    }

    /// The tab, as loaded or as its controls left it
    pub(super) fn party_tab(&self) -> Result<PartyTab> {
        match &self.party_tab {
            Some(t) => Ok(t.clone()),
            None => self.initial_party_tab(),
        }
    }

    /// `ParseBuffs` of a text into the actor
    fn parse_party(&self, party: &mut Party, name: &str, text: &str) -> Result<()> {
        let setup = &self.data.game.setup;
        let skills = &self.data.game.skills;
        party.add(
            name,
            text,
            &|n| setup.mod_flags.get(n).copied(),
            &|n| setup.keyword_flags.get(n).copied(),
            &|id| skills.skills.contains_key(id),
        )?;
        Ok(())
    }

    /// `controlTooltip` of a `party.<name>` control: none of the tab's
    /// controls has a `tooltipFunc`, so an empty tooltip
    pub fn party_tooltip(&self, name: &str) -> Result<J> {
        if !PARTY_CONTROLS.contains(&name) {
            return Err(format!("{NOT_PORTED}: control party.{name}"));
        }
        Ok(json!({ "lines": [] }))
    }

    /// A `party.<name>` control changed as `api.control` changes it
    pub(super) fn party_control(&mut self, name: &str, req: &J) -> Result<J> {
        let mut tab = self.party_tab()?;
        let mut party = (*self.build.party).clone();
        let sel = req.get("sel").and_then(J::as_f64);
        let text = req.get("text").and_then(J::as_str);
        let state = req.get("state").and_then(J::as_bool);
        let click = req.get("click").and_then(J::as_bool) == Some(true);
        let edit = match name {
            "editPartyMemberStats" => Some("PartyMemberStats"),
            "editAuras" => Some("Aura"),
            "editCurses" => Some("Curse"),
            "editWarcries" => Some("Warcry Skills"),
            "editLinks" => Some("Link Skills"),
            "enemyCond" => Some("EnemyConditions"),
            "enemyMods" => Some("EnemyMods"),
            _ => None,
        };
        let mut rebuild = false;
        match (name, sel, text, state) {
            (_, None, Some(t), None) if edit.is_some() => {
                *tab.buf(edit.unwrap()) = t.to_string();
            }
            ("importCodeIn", None, Some(t), None) => {
                tab.code = t.to_string();
                tab.handle_code()?;
            }
            ("importCodeDestination", Some(s), None, None) => {
                // `SetSel`, clamped; the drop-down has no handler
                tab.dest = (s.min(DESTINATIONS.len() as f64).max(1.0) as usize) - 1;
            }
            ("appendNotReplace", None, None, Some(s)) => tab.append = s,
            ("ShowAdvanceTools", None, None, Some(s)) => tab.advanced = s,
            ("importCodeGo", None, None, None) if click => {
                if tab.site && tab.xml.is_none() {
                    return Err(format!("{NOT_PORTED}: downloading a party build"));
                }
                rebuild = self.finish_import(&mut tab, &mut party)?;
            }
            ("clear", None, None, None) if click => {
                clear_input_text(&mut tab, &mut party);
                party.enemy_mods.clear();
                rebuild = true;
            }
            ("removeEffects", None, None, None) if click => {
                wipe_actor(&mut party);
                rebuild = true;
            }
            ("rebuild", None, None, None) if click => {
                wipe_actor(&mut party);
                self.parse_party(&mut party, "PartyMemberStats", &tab.bufs[0])?;
                self.parse_party(&mut party, "Aura", &tab.bufs[1])?;
                tab.auras = aura_label(&party.aura)?;
                self.parse_party(&mut party, "Curse", &tab.bufs[2])?;
                tab.curses = buff_label(&party.curse, "Curse")?;
                self.parse_party(&mut party, "Warcry Skills", &tab.bufs[3])?;
                tab.warcries = buff_label(&party.warcry, "Warcry")?;
                self.parse_party(&mut party, "Link Skills", &tab.bufs[4])?;
                tab.links = buff_label(&party.link, "Link")?;
                self.parse_party(&mut party, "EnemyConditions", &tab.bufs[5])?;
                self.parse_party(&mut party, "EnemyMods", &tab.bufs[6])?;
                tab.enemy_mods = party.enemy_mods_label.clone().unwrap_or_default();
                rebuild = true;
            }
            _ => return Err(format!("{NOT_PORTED}: control party.{name}")),
        }
        self.party_tab = Some(tab);
        self.build.party = Arc::new(party);
        if rebuild {
            self.rebuild()?;
        }
        Ok(json!([]))
    }

    /// `finishImport`: whether it set `buildFlag`
    fn finish_import(&self, tab: &mut PartyTab, party: &mut Party) -> Result<bool> {
        if !tab.valid {
            return Ok(false);
        }
        let (mut cur_curse, mut cur_link) = (None, None);
        if !tab.append {
            clear_input_text(tab, party);
        } else {
            if tab.hits(2) {
                party.aura = BuffList::default();
            }
            if tab.hits(3) {
                cur_curse = Some(std::mem::take(tab.buf("Curse")));
                party.curse = BuffList::default();
            }
            if tab.hits(4) {
                party.warcry = BuffList::default();
            }
            if tab.hits(5) {
                cur_link = Some(std::mem::take(tab.buf("Link Skills")));
                party.link = BuffList::default();
            }
        }
        let xml = tab.xml.clone().unwrap_or_default();
        let doc = engine::xml::parse(&xml)
            .map_err(|e| format!("{NOT_PORTED}: a party import that does not parse: {e}"))?;
        let root = match doc.children.first() {
            Some(engine::xml::Node::Element(e)) if e.elem == "PathOfBuilding" => e,
            _ => {
                return Err(format!(
                    "{NOT_PORTED}: a party import without PathOfBuilding"
                ));
            }
        };
        let Some(section) = root.elements().find(|e| e.elem == "Party") else {
            return Ok(false);
        };
        for node in section.elements() {
            if node.elem != "ExportedBuffs" {
                continue;
            }
            let Some(name) = node.attr("name") else {
                continue;
            };
            let k = match name {
                "PartyMemberStats" => 1,
                "Aura" => 2,
                "Curse" => 3,
                "Warcry Skills" => 4,
                "Link Skills" => 5,
                "EnemyConditions" => 6,
                "EnemyMods" => 7,
                _ => continue,
            };
            if !tab.hits(k) {
                continue;
            }
            let mut text = match node.children.first() {
                Some(engine::xml::Node::Text(t)) => t.clone(),
                _ => String::new(),
            };
            let buf = tab.buf(name);
            if !buf.is_empty() {
                text = format!("{buf}\n{text}");
            }
            if k == 3
                && let Some(c) = &cur_curse
                && text == "--- Curse Limit ---\n1"
            {
                text = c.clone();
            }
            if k == 5
                && let Some(l) = &cur_link
                && text.is_empty()
            {
                text = l.clone();
            }
            *tab.buf(name) = text.clone();
            match k {
                1 => self.parse_party(party, name, &text)?,
                2 => {
                    self.parse_party(party, name, &text)?;
                    tab.auras = aura_label(&party.aura)?;
                }
                3 => {
                    self.parse_party(party, name, &text)?;
                    tab.curses = buff_label(&party.curse, "Curse")?;
                }
                4 => {
                    self.parse_party(party, name, &text)?;
                    tab.warcries = buff_label(&party.warcry, "Warcry")?;
                }
                5 => {
                    self.parse_party(party, name, &text)?;
                    tab.links = buff_label(&party.link, "Link")?;
                }
                _ => {}
            }
        }
        if tab.hits(6) || tab.hits(7) {
            party.enemy_mods.clear();
            self.parse_party(party, "EnemyConditions", &tab.bufs[5].clone())?;
            self.parse_party(party, "EnemyMods", &tab.bufs[6].clone())?;
            tab.enemy_mods = party.enemy_mods_label.clone().unwrap_or_default();
        }
        Ok(true)
    }
}

/// `clearInputText`: the destination's boxes and labels, and the buff
/// lists (not the party members' stats, which stay parsed)
fn clear_input_text(tab: &mut PartyTab, party: &mut Party) {
    if tab.hits(1) {
        tab.bufs[0].clear();
    }
    if tab.hits(2) {
        tab.auras.clear();
        tab.bufs[1].clear();
        party.aura = BuffList::default();
    }
    if tab.hits(3) {
        tab.curses.clear();
        tab.bufs[2].clear();
        party.curse = BuffList::default();
    }
    if tab.hits(4) {
        tab.warcries.clear();
        tab.bufs[3].clear();
        party.warcry = BuffList::default();
    }
    if tab.hits(5) {
        tab.links.clear();
        tab.bufs[4].clear();
        party.link = BuffList::default();
    }
    if tab.hits(6) {
        tab.enemy_cond = ENEMY_COND.into();
        tab.bufs[5].clear();
    }
    if tab.hits(7) {
        tab.enemy_mods = "\n".into();
        tab.bufs[6].clear();
    }
}

/// A new actor and enemy modifier list, as Disable Party Effects and
/// Rebuild All make them
fn wipe_actor(party: &mut Party) {
    party.member_mods.clear();
    party.member_output.clear();
    party.aura = BuffList::default();
    party.curse = BuffList::default();
    party.warcry = BuffList::default();
    party.link = BuffList::default();
    party.enemy_mods.clear();
}
