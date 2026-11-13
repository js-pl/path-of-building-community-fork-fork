//! What `calcs.perform` exports for the Party tab at its end, when the
//! build exports its buffs (`Modules/CalcPerform.lua`, "Export modifiers
//! to enemy conditions and stats for party tab"): the enemy's conditions
//! and mods, the stats party members copy, mine auras stacked.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::actor::OutVal;
use crate::calc::PLAYER;
use crate::calc_perform::Perform;
use crate::mods::store::Mods;
use crate::mods::tag::TagKind;
use crate::mods::{Mod, ModValue, Store, Sym};
use crate::party::AuraExport;
use crate::pat;

pub type Result<T> = std::result::Result<T, String>;

/// A store's own mods by name, the shared base's first, as upstream's
/// `enemyDB.mods` holds them
fn mods_by_name(store: &Store) -> Vec<(Sym, Vec<&Mod>)> {
    fn add<'s>(s: &'s Store, out: &mut Vec<(Sym, Vec<&'s Mod>)>) {
        let Mods::Db(by_name) = &s.mods else { return };
        let mut names: Vec<&Sym> = by_name.keys().collect();
        names.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        for name in names {
            let mods = by_name[name]
                .iter()
                .enumerate()
                .filter(|(i, _)| !s.replaced.contains(&(*name, *i)))
                .map(|(_, m)| m);
            match out.iter_mut().find(|(n, _)| n == name) {
                Some(e) => e.1.extend(mods),
                None => out.push((*name, mods.collect())),
            }
        }
    }
    let mut out = Vec::new();
    if let Some(b) = store.base.as_deref() {
        add(b, &mut out);
    }
    add(store, &mut out);
    out
}

/// The `GlobalEffect`-style field `k` of a tag, as text
fn tag_field<'m>(m: &'m Mod, i: usize, k: &str) -> Option<&'m ModValue> {
    match &m.tags.get(i)?.kind {
        TagKind::Other { fields, .. } => fields.get(k),
        _ => None,
    }
}

impl Perform<'_> {
    /// The end of `calcs.perform` for the Party tab: completes
    /// `buffExports` (`env.build.partyTab:setBuffExports`)
    pub(crate) fn party_exports(&mut self) -> Result<()> {
        let Some(mut x) = self.buff_exports.take() else {
            return Ok(());
        };
        let edb = self.edb();
        let by_name = mods_by_name(self.c.store(edb));
        let first_value = |name: &str| -> bool {
            by_name
                .iter()
                .find(|(n, _)| n.as_str() == name)
                .and_then(|(_, l)| l.first())
                .is_some_and(|m| m.value.truthy())
        };
        for (k, mods) in &by_name {
            let k = k.as_str();
            if k.contains("Condition") && !k.contains("Party") {
                if !x.enemy_conditions.iter().any(|c| c == k) {
                    x.enemy_conditions.push(k.to_string());
                }
                continue;
            }
            let wanted = (k.contains("Resist") && !k.contains("Totem") && !k.contains("Max"))
                || k.contains("Damage")
                || k.contains("ActionSpeed")
                || k.contains("SelfCrit")
                || (k.contains("Multiplier") && !k.contains("Max") && !k.contains("Impale"));
            if !wanted {
                continue;
            }
            for v in mods {
                let source = v
                    .source
                    .as_deref()
                    .ok_or("attempt to index a nil value (an enemy mod's source)")?;
                if v.value == ModValue::Number(0.0)
                    || source == "EnemyConfig"
                    || source == "Base"
                    || source.contains("Delirious")
                    || source.starts_with("Party")
                {
                    continue;
                }
                let skip = v.tags.iter().enumerate().any(|(i, _)| {
                    matches!(tag_field(v, i, "effectType"), Some(ModValue::Str(s)) if &**s == "Curse" || &**s == "AuraDebuff")
                });
                if skip {
                    continue;
                }
                let keep = match v.tags.first().map(|t| &t.kind) {
                    None => true,
                    Some(TagKind::Condition { vars, .. }) => {
                        let var = match vars {
                            crate::mods::Names::One(s) => Some(*s),
                            crate::mods::Names::List(l) => l.first().copied(),
                        };
                        var.is_some_and(|v| first_value(&format!("Condition:{}", v.as_str())))
                    }
                    Some(TagKind::Multiplier(m)) => match &m.vars {
                        Some(crate::mods::Names::One(var)) => {
                            first_value(&format!("Multiplier:{}", var.as_str()))
                        }
                        _ => {
                            return Err("attempt to concatenate field 'var' (a nil value)".into());
                        }
                    },
                    Some(_) => true,
                };
                if !keep {
                    continue;
                }
                match x.enemy_mods.iter_mut().find(|(n, _, _)| n == k) {
                    Some(e) => {
                        e.1.push((*v).clone());
                        e.2 = true;
                    }
                    None => x
                        .enemy_mods
                        .push((k.to_string(), vec![(*v).clone()], false)),
                }
            }
        }

        let out = |p: &Self, k: &str| p.c.out(PLAYER, k);
        let text = |v: Option<f64>| v.map_or_else(|| "nil".to_string(), pat::num);
        let pdb = self.pdb();
        let mut player = Vec::new();
        for t in ["Physical", "Lightning", "Cold", "Fire", "Chaos"] {
            let resist = out(self, &format!("{t}Resist"));
            if self
                .c
                .flag(pdb, None, &[&format!("Enemy{t}ResistEqualToYours")])?
                && let Some(r) = resist
            {
                player.push(format!("Enemy{t}ResistEqualToYours"));
                player.push(format!("{t}Resist={}", pat::num(r)));
            }
        }
        if self.c.flag(
            pdb,
            None,
            &["PartyMemberMaximumEnduranceChargesEqualToYours"],
        )? && let Some(v) = out(self, "EnduranceChargesMax")
        {
            player.push("PartyMemberMaximumEnduranceChargesEqualToYours".into());
            player.push(format!("EnduranceChargesMax={}", pat::num(v)));
        }
        let ms = out(self, "MovementSpeedMod")
            .ok_or("attempt to perform arithmetic on field 'MovementSpeedMod' (a nil value)")?;
        player.push(format!(
            "MovementSpeedMod|percent|max={}",
            pat::num(ms * 100.0)
        ));
        if let Some(AuraExport::Buff(b)) = x.aura_get("extraAura")
            && b.mods
                .iter()
                .any(|m| m.name.as_str() == "YourFortifyEqualToParent")
        {
            player.push("PartyMemberFortifyEqualToYours".into());
            let f = out(self, "FortificationStacks").unwrap_or(0.0);
            player.push(format!("FortificationStacks={}", pat::num(f)));
        }

        // Mine auras, stacked
        let names: Vec<String> = x.aura.iter().map(|(n, _)| n.clone()).collect();
        for name in names {
            if !name.contains("Mine") || name.contains(" Limit") {
                continue;
            }
            let key = format!("Multiplier:{name}Stack");
            let stack = match x.enemy_mods.iter().find(|(n, _, _)| *n == key) {
                Some((_, mods, false)) => match &mods[0].value {
                    ModValue::Number(n) => *n,
                    ModValue::Bool(false) => 0.0,
                    _ => return Err("not ported: a mine's stack count that is no number".into()),
                },
                _ => 0.0,
            };
            x.enemy_mods.retain(|(n, _, _)| *n != key);
            for aura in [name.clone(), format!("{name} Limit")] {
                if !x.aura_has(&aura) {
                    continue;
                }
                if stack == 0.0 {
                    x.remove_aura(&aura);
                    continue;
                }
                if let Some((_, AuraExport::Buff(b))) = x.aura.iter_mut().find(|(n, _)| *n == aura)
                {
                    for m in &mut b.mods {
                        stack_mine_mod(m, stack)?;
                    }
                }
            }
        }

        for (name, link) in &x.link {
            if name == "Flame Link" {
                player.push(format!("Life={}", text(out(self, "Life"))));
            }
            for m in &link.mods {
                let s = match m.name.as_str() {
                    "MaximumBlockAttackChanceIsEqualToParent" => {
                        format!("BlockChanceMax={}", text(out(self, "BlockChanceMax")))
                    }
                    "BlockAttackChanceIsEqualToParent" => {
                        format!("BlockChance={}", text(out(self, "BlockChance")))
                    }
                    "MaximumLifeLeechIsEqualToPartyMember" => format!(
                        "MaxLifeLeechRatePercent={}",
                        text(out(self, "MaxLifeLeechRatePercent"))
                    ),
                    "TakenFromParentESBeforeYou" => format!(
                        "EnergyShieldRecoveryCap={}",
                        text(out(self, "EnergyShieldRecoveryCap"))
                    ),
                    "MainHandCritIsEqualToParent" => match self.c.out_val(PLAYER, "MainHand") {
                        Some(OutVal::Table(t)) => format!(
                            "MainHand.CritChance={}",
                            text(t.get("CritChance").and_then(OutVal::num))
                        ),
                        _ => format!(
                            "MainHand.CritChance={}",
                            text(self.weapon_num(PLAYER, 0, "CritChance"))
                        ),
                    },
                    _ => continue,
                };
                player.push(s);
            }
        }
        for p in player {
            if !x.player_mods.contains(&p) {
                x.player_mods.push(p);
            }
        }
        self.buff_exports = Some(x);
        Ok(())
    }
}

/// A mine aura's mod times the mines' stack count, its first
/// `effectStackVar = "ActiveMineCount"` tag field removed
fn stack_mine_mod(m: &mut Mod, stack: f64) -> Result<()> {
    let k = Sym::new("effectStackVar");
    let found = m.tags.iter().position(|t| match &t.kind {
        TagKind::Other { fields, .. } => {
            matches!(fields.get("effectStackVar"), Some(ModValue::Str(s)) if &**s == "ActiveMineCount")
        }
        _ => false,
    });
    let Some(i) = found else { return Ok(()) };
    if let TagKind::Other { fields, .. } = &mut m.tags[i].kind {
        fields.fields.retain(|(f, _)| *f != k);
    }
    let ModValue::Number(v) = m.value else {
        return Err("attempt to perform arithmetic on a mine aura's value".into());
    };
    m.value = ModValue::Number(v * stack);
    Ok(())
}
