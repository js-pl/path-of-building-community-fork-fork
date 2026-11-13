//! The texts of the unique and rare item databases (`data.uniques`,
//! `data.rares`), with the uniques `Data/Uniques/Special/Generated.lua`
//! builds from other data at load: its literal tables come from the
//! importer's evaluation of the file, the code around them is ported here.
//! The uniques it builds from the latest tree (`buildTreeDependentUniques`)
//! are made by the engine from the typed tree.
//!
//! In sync with upstream `a69391d6daaaea1aaf8054668a094aad88dd7eaa`.

use crate::{Result, Source};
use engine::pat::{self, gsub, gsub_fn, is_match};
use lua::value::FuncRef;
use lua::{Key, TableRef, Value};
use std::collections::{BTreeMap, HashMap};

const GENERATED: &str = "Data/Uniques/Special/Generated.lua";

fn text(v: &Value) -> Result<String> {
    v.as_str()
        .map(|s| s.to_str_lossy().into_owned())
        .ok_or_else(|| format!("expected a string, got {v:?}"))
}

fn table(v: &Value) -> Result<TableRef> {
    v.as_table()
        .cloned()
        .ok_or_else(|| format!("expected a table, got {v:?}"))
}

fn strings(v: &Value) -> Result<Vec<String>> {
    table(v)?.array().iter().map(text).collect()
}

/// `pairsSortByKey` of a table with string keys
fn sorted_by_key(t: &TableRef) -> Result<Vec<(String, Value)>> {
    let mut out: Vec<(String, Value)> = t
        .entries()
        .into_iter()
        .map(|(k, v)| match k {
            Key::Str(s) => Ok((s.to_str_lossy().into_owned(), v)),
            k => Err(format!("expected a string key, got {k:?}")),
        })
        .collect::<Result<_>>()?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// A number as Lua's `..` writes it
fn num(n: f64) -> String {
    pat::num(n)
}

impl Source {
    /// `data.uniques` with `generated` as `Generated.lua` leaves it before
    /// the tree loads: each type's texts, in `pairs` order
    pub fn uniques(&self) -> Result<Vec<(String, Vec<String>)>> {
        let uniques = table(&self.data.get_str("uniques"))?;
        let generated = TableRef::new();
        for raw in self.generated_uniques()? {
            generated.borrow_mut().push(Value::from(raw.as_str()));
        }
        // `data.uniques.generated = { }`, as the file does
        uniques.set_str("generated", Value::Table(generated));
        uniques
            .entries()
            .into_iter()
            .map(|(k, v)| {
                let k = match k {
                    Key::Str(s) => s.to_str_lossy().into_owned(),
                    k => return Err(format!("unique type {k:?}")),
                };
                Ok((k, strings(&v)?))
            })
            .collect()
    }

    /// `data.rares`
    pub fn rares(&self) -> Result<Vec<String>> {
        let mut im = self.importer();
        let rares = im.load1("Data/Rares", vec![]).map_err(|e| e.to_string())?;
        strings(&rares)
    }

    /// The uniques `Generated.lua` adds when it loads, in its order
    fn generated_uniques(&self) -> Result<Vec<String>> {
        let path = self.src_dir.join(GENERATED);
        let src = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut im = self.importer();
        let names = [
            "megalomaniac",
            "forbiddenShako",
            "replicaForbiddenShako",
            "excludedGems",
            "enduranceChargeMods",
            "frenzyChargeMods",
            "powerChargeMods",
            "precursorsEmblem",
            "balanceOfTerrorMods",
            "balanceOfTerror",
            "watchersEyeLegacyMods",
            "watchersEye",
            "sublimeVision",
            "voranasMarch",
            "boundByDestiny",
            "replicaDragonfangsFlight",
            "pearl",
        ];
        let l = im
            .env
            .eval_locals(GENERATED, &src, &names, HashMap::new())
            .map_err(|e| e.to_string())?;
        let local = |n: &str| -> Result<Vec<String>> { strings(&l[n]) };
        let data = &self.data;
        let mut out = Vec::new();

        // Megalomaniac
        let mut lines = local("megalomaniac")?;
        let sort_order =
            table(&table(&data.get_str("clusterJewels"))?.get_str("notableSortOrder"))?;
        let mut notables: Vec<String> = sort_order
            .entries()
            .into_iter()
            .map(|(k, _)| match k {
                Key::Str(s) => Ok(s.to_str_lossy().into_owned()),
                k => Err(format!("notable {k:?}")),
            })
            .collect::<Result<_>>()?;
        notables.sort();
        for n in &notables {
            lines.push(format!("Variant: {n}"));
        }
        lines.push("Adds 4 Passive Skills".into());
        lines.push("Added Small Passive Skills grant Nothing".into());
        for (i, n) in notables.iter().enumerate() {
            lines.push(format!("{{variant:{}}}1 Added Passive Skill is {n}", i + 1));
        }
        out.push(lines.join("\n"));

        // Forbidden Shako and its replica
        let excluded = local("excludedGems")?;
        let skills = table(&data.get_str("skills"))?;
        let mut gems: Vec<String> = Vec::new();
        for (_, gem) in table(&data.get_str("gems"))?.entries() {
            let gem = table(&gem)?;
            let effect = skills.get(&gem.get_str("grantedEffectId"));
            let Some(effect) = effect.as_table() else {
                continue;
            };
            let name = text(&effect.get_str("name"))?;
            if effect.get_str("support").truthy()
                && !effect.get_str("plusVersionOf").truthy()
                && !excluded.contains(&name)
            {
                gems.push(name);
            }
        }
        gems.sort();
        for key in ["forbiddenShako", "replicaForbiddenShako"] {
            let mut lines = local(key)?;
            for n in &gems {
                lines.push(format!("Variant: {n} (Low Level)"));
                lines.push(format!("Variant: {n} (High Level)"));
            }
            for (i, n) in gems.iter().enumerate() {
                let i = i + 1;
                lines.push(format!(
                    "{{variant:{}}}Socketed Gems are Supported by Level (1-10) {n}",
                    i * 2 - 1
                ));
                lines.push(format!(
                    "{{variant:{}}}Socketed Gems are Supported by Level (25-35) {n}",
                    i * 2
                ));
            }
            lines.push("+(25-30) to all Attributes".into());
            out.push(lines.join("\n"));
        }

        // Precursor's Emblem
        let mut lines = local("precursorsEmblem")?;
        let charge_mods: Vec<(&str, TableRef)> = vec![
            ("Endurance - ", table(&l["enduranceChargeMods"])?),
            ("Frenzy - ", table(&l["frenzyChargeMods"])?),
            ("Power - ", table(&l["powerChargeMods"])?),
        ];
        for (prefix, mods) in &charge_mods {
            for tier in mods.array() {
                for (desc, _) in sorted_by_key(&table(&tier)?)? {
                    lines.push(format!("Variant: {prefix}{desc}"));
                }
            }
        }
        lines.push(long_string_after(
            &src,
            "table.insert(precursorsEmblem, [[",
        )?);
        let mut index = 8;
        for (_, mods) in &charge_mods {
            for (t, tier_mods) in mods.array().iter().enumerate() {
                let tier = (t + 1) as f64;
                for (_, m) in sorted_by_key(&table(tier_mods)?)? {
                    let mut m = text(&m)?;
                    if is_match(&m, "[%+%-]?[%d%.]*%d+%%") {
                        m = gsub_fn(&m, "([%d%.]*%d+)", |c| {
                            let n = pat::to_number(&c[0])?;
                            Some(format!("({}-{})", c[0], num(n * tier)))
                        });
                    } else if is_match(&m, "%(%-?[%d%.]+%-%-?[%d%.]+%)%%") {
                        m = gsub_fn(&m, "(%(%-?[%d%.]+%-)(%-?[%d%.]+)%)", |c| {
                            let n = pat::to_number(&c[1])?;
                            Some(format!("{}{})", c[0], num(n * tier)))
                        });
                    } else if is_match(&m, "%(%d+%-%d+%) to %(%d+%-%d+%)") {
                        m = gsub_fn(&m, "(%(%d+%-)(%d+)(%) to %(%d+%-)(%d+)%)", |c| {
                            let a = pat::to_number(&c[1])?;
                            let b = pat::to_number(&c[3])?;
                            Some(format!(
                                "{}{}{}{})",
                                c[0],
                                num(a * tier),
                                c[2],
                                num(b * tier)
                            ))
                        });
                    }
                    lines.push(format!("{{variant:{index}}}{{range:0}}{m}"));
                    index += 1;
                }
            }
        }
        out.push(lines.join("\n"));

        // The Balance of Terror
        let mut lines = local("balanceOfTerror")?;
        lines.push("Variant: None".into());
        let mods = sorted_by_key(&table(&l["balanceOfTerrorMods"])?)?;
        for (name, _) in &mods {
            lines.push(format!("Variant: {name}"));
        }
        lines.push("+(10-15)% to all Elemental Resistances".into());
        for (i, (_, line)) in mods.iter().enumerate() {
            lines.push(format!("{{variant:{}}}{}", i + 2, text(line)?));
        }
        out.push(lines.join("\n"));

        out.extend(self.watchers_eye_and_friends(&l, &src)?);
        out.push(self.replica_dragonfangs_flight(&local("replicaDragonfangsFlight")?, &src)?);
        out.push(self.pearl(local("pearl")?)?);
        Ok(out)
    }

    /// Watcher's Eye, Sublime Vision, Vorana's March and Bound by Destiny
    fn watchers_eye_and_friends(
        &self,
        l: &HashMap<String, Value>,
        src: &[u8],
    ) -> Result<Vec<String>> {
        let legacy = table(&l["watchersEyeLegacyMods"])?;
        let mut watchers = strings(&l["watchersEye"])?;
        let mut sublime = strings(&l["sublimeVision"])?;
        let mut vorana = strings(&l["voranasMarch"])?;
        let mut bound = strings(&l["boundByDestiny"])?;
        let abbreviate = |s: &str| {
            let s = gsub(s, "Increased", "Inc");
            let s = gsub(&s, "Reduced", "Red.");
            let s = gsub(&s, "Critical", "Crit");
            let s = gsub(&s, "Physical", "Phys");
            let s = gsub(&s, "Elemental", "Ele");
            let s = gsub(&s, "Multiplier", "Mult");
            gsub(&s, "EnergyShield", "ES")
        };
        vorana.push("Variant: None".into());
        watchers.push("Variant: None".into());
        // `data.uniqueMods["Watcher's Eye"]`: the mods sorted by id
        let mods = sorted_by_key(&table(&self.data.get_str("watchersEyeMods"))?)?;
        let legacy_of = |id: &str| legacy.get_str(id).as_table().cloned();
        for (id, _) in &mods {
            if !(is_match(id, "^SublimeVision") || is_match(id, "^SummonArbalist")) {
                let name = abbreviate(id);
                let name = gsub(&name, "^[Purity Of ]*%u%l+", "%1:");
                let name = gsub(&name, "New", "");
                let name = gsub(&name, "[%u%d]", " %1");
                let name = gsub(&name, "_", "");
                let name = gsub(&name, "E S", "ES");
                match legacy_of(id) {
                    Some(lg) => {
                        if let Some(v) = lg.get_str("version").as_str() {
                            watchers.push(format!("Variant:{name} (Pre {})", v.to_str_lossy()));
                        }
                        if lg.get_str("legacyMod").truthy() {
                            watchers.push(format!("Variant:{name}"));
                        }
                        if let Some(r) = lg.get_str("rename").as_str() {
                            watchers.push(format!("Variant: {}", r.to_str_lossy()));
                        }
                    }
                    None => watchers.push(format!("Variant:{name}")),
                }
            } else if !is_match(id, "^SummonArbalist") {
                let name = gsub(&gsub(id, "SublimeVision", ""), "[%u%d]", " %1");
                sublime.push(format!("Variant:{name}"));
            } else {
                let mut name = gsub(&abbreviate(id), "SummonArbalist", "");
                for (p, r) in [
                    ("[%u%d]", " %1"),
                    ("_", ""),
                    ("Percent To ", ""),
                    ("Chance To ", ""),
                    ("Targets To ", ""),
                    ("[fF]or 4 ?[Ss]econds On Hit", ""),
                    (" Percent", ""),
                    ("Number Of ", ""),
                ] {
                    name = gsub(&name, p, r);
                }
                vorana.push(format!("Variant:{name}"));
            }
        }

        // Bound by Destiny: its mods sorted by type, then id
        let mut im = self.importer();
        let unsorted = table(
            &im.load1("Data/Uniques/Special/BoundByDestiny", vec![])
                .map_err(|e| e.to_string())?,
        )?;
        let mut sorted: Vec<(String, String, TableRef)> = Vec::new();
        for (id, m) in sorted_by_key(&unsorted)? {
            let m = table(&m)?;
            sorted.push((text(&m.get_str("type"))?, id, m));
        }
        sorted.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        for (typ, id, _) in &sorted {
            let head = gsub(typ, "(%d+)(%a+)", "%2 %1:");
            let mut name = gsub(&abbreviate(id), &format!("{typ}.*"), "");
            for (p, r) in [
                ("New", ""),
                ("[%u%d]", " %1"),
                ("_", ""),
                ("E S", "ES"),
                ("Velocity", "Speed"),
                ("Permyriad", ""),
            ] {
                name = gsub(&name, p, r);
            }
            bound.push(format!("Variant: {head}{name}"));
        }
        for (i, (_, _, m)) in sorted.iter().enumerate() {
            for line in strings(&Value::Table(m.clone()))? {
                bound.push(format!("{{variant:{}}}{line}", i + 1));
            }
        }

        watchers.push(long_string_after(src, "table.insert(watchersEye,\n[[")?);
        vorana.push(long_string_after(src, "table.insert(voranasMarch,\n[[")?);

        let (mut iw, mut is, mut iv) = (2, 1, 2);
        for (id, m) in &mods {
            let lines = strings(m)?;
            if !(is_match(id, "^SublimeVision") || is_match(id, "^SummonArbalist")) {
                match legacy_of(id) {
                    Some(lg) => {
                        if let Value::Func(f) = lg.get_str("legacyMod") {
                            let repl = legacy_replacement(src, &f)?;
                            for line in &lines {
                                let line = gsub(line, "%(.*%)", &repl);
                                watchers.push(format!("{{variant:{iw}}}{line}"));
                            }
                            iw += 1;
                        }
                        if lg.get_str("version").truthy() || lg.get_str("rename").truthy() {
                            for line in &lines {
                                watchers.push(format!("{{variant:{iw}}}{line}"));
                            }
                            iw += 1;
                        }
                    }
                    None => {
                        for line in &lines {
                            watchers.push(format!("{{variant:{iw}}}{line}"));
                        }
                        iw += 1;
                    }
                }
            } else if !is_match(id, "^SummonArbalist") {
                for line in &lines {
                    sublime.push(format!("{{variant:{is}}}{line}"));
                }
                is += 1;
            } else {
                for line in &lines {
                    vorana.push(format!("{{variant:{iv}}}{line}"));
                }
                iv += 1;
            }
        }
        Ok(vec![
            watchers.join("\n"),
            sublime.join("\n"),
            vorana.join("\n"),
            bound.join("\n"),
        ])
    }

    /// Replica Dragonfang's Flight: a variant for each active, non-Vaal gem
    fn replica_dragonfangs_flight(&self, head: &[String], src: &[u8]) -> Result<String> {
        let mut mods: BTreeMap<String, String> = BTreeMap::new();
        for (_, gem) in table(&self.data.get_str("gems"))?.entries() {
            let gem = table(&gem)?;
            let id = text(&gem.get_str("grantedEffectId"))?;
            let tags = table(&gem.get_str("tags"))?;
            let tag = |t: &str| tags.get_str(t).truthy();
            let active = tag("grants_active_skill") && !tag("support");
            if !is_match(&id, "Alt[XY]$") && active && !tag("vaal") {
                let name = text(&gem.get_str("name"))?;
                mods.insert(name.clone(), format!("+3 to Level of all {name} Gems"));
            }
        }
        let mut sorted: Vec<(String, String)> = mods.into_iter().map(|(n, l)| (l, n)).collect();
        sorted.sort();
        let mut lines = head.to_vec();
        let mut literals = long_strings_after(src, "local replicaDragonfangsFlight = {")?;
        if literals.len() < 3 {
            return Err("Replica Dragonfang's Flight: literals not found".into());
        }
        // The first literal is the head, read as a local already
        literals.remove(0);
        lines.push(literals.remove(0));
        for (_, name) in &sorted {
            lines.push(format!("Variant: {name}"));
        }
        lines.push(literals.remove(0));
        for (i, (line, _)) in sorted.iter().enumerate() {
            lines.push(format!("{{variant:{}}}{{group:1}}{line}", i + 1));
        }
        lines.push(literals.remove(0));
        Ok(lines.join("\n"))
    }

    /// Pearl of Tsoatha: a variant for each slot and support of
    /// `Data/PearlSupports.lua`, described by `stat_descriptions`
    fn pearl(&self, mut lines: Vec<String>) -> Result<String> {
        let mut im = self.importer();
        let supports = table(
            &im.load1("Data/PearlSupports", vec![])
                .map_err(|e| e.to_string())?,
        )?;
        let names: Vec<String> = supports
            .array()
            .iter()
            .map(|s| text(&table(s)?.get_str("baseItemName")))
            .collect::<Result<_>>()?;
        let describe = pearl_describer(self)?;
        let slots = ["Helmet", "Gloves", "Boots", "Passive Tree"];
        let mut mods: Vec<(String, String, String)> = Vec::new();
        for (i, name) in names.iter().enumerate() {
            for slot in 1..=4 {
                let gem = gsub(name, " Support", "");
                let description = describe(slot, i + 1, &names)?;
                mods.push((slots[slot - 1].to_string(), gem, description));
            }
        }
        mods.sort();
        for (slot, gem, _) in &mods {
            lines.push(format!("Variant: {slot} / {gem}"));
        }
        let exclusive = table(&table(&self.data.get_str("itemMods"))?.get_str("ItemExclusive"))?;
        let line_of = |id: &str| -> Result<String> {
            let m = table(&exclusive.get_str(id))?;
            let tags = strings(&m.get_str("modTags")).unwrap_or_default();
            let tags = if tags.is_empty() {
                String::new()
            } else {
                format!("{{tags:{}}}", tags.join(","))
            };
            Ok(format!("{tags}{}", strings(&Value::Table(m))?.join(" ")))
        };
        lines.push(line_of("AllResistancesImplicitRing1")?);
        for id in ["AllResistancesUniqueRing98", "ElementalDamageUnique__5"] {
            lines.push(line_of(id)?);
        }
        for (i, (_, _, d)) in mods.iter().enumerate() {
            lines.push(format!("{{variant:{}}}{d}", i + 1));
        }
        Ok(lines.join("\n"))
    }
}

/// `data.describeStats` for Pearl of Tsoatha's stats only: the support
/// of an index in a slot, at level 20
type PearlDescriber = Box<dyn Fn(usize, usize, &[String]) -> Result<String>>;

fn pearl_describer(src: &Source) -> Result<PearlDescriber> {
    let mut im = src.importer();
    let scope = table(
        &im.load1("Data/StatDescriptions/stat_descriptions", vec![])
            .map_err(|e| e.to_string())?,
    )?;
    if scope.get_str("parent").truthy() {
        return Err("stat_descriptions has a parent scope".into());
    }
    let order = scope.get_str("local_pearl_random_support_gem_2_slot_index");
    let descriptor = table(&scope.get(&order))?;
    let stats = strings(&descriptor.get_str("stats"))?;
    if stats
        != [
            "local_pearl_random_support_gem_2_slot_index",
            "local_pearl_random_support_gem_2_index",
            "local_pearl_random_support_gem_2_level",
        ]
    {
        return Err(format!("Pearl of Tsoatha's stats changed: {stats:?}"));
    }
    let lang = table(&descriptor.get_int(1))?;
    // Each description's limits, specials and text
    let mut descs = Vec::new();
    for d in lang.array() {
        let d = table(&d)?;
        if d.get_str("gem_quality").truthy() {
            continue;
        }
        let limits: Vec<(Value, Value)> = table(&d.get_str("limit"))?
            .array()
            .iter()
            .map(|l| {
                let l = table(l)?;
                Ok((l.get_int(1), l.get_int(2)))
            })
            .collect::<Result<_>>()?;
        let specials: Vec<(String, usize)> = d
            .array()
            .iter()
            .map(|s| {
                let s = table(s)?;
                let v = s.get_str("v").as_number().ok_or("special without v")? as usize;
                Ok((text(&s.get_str("k"))?, v))
            })
            .collect::<Result<_>>()?;
        descs.push((limits, specials, text(&d.get_str("text"))?));
    }
    Ok(Box::new(move |slot, index, supports| {
        let vals = [slot as f64, index as f64, 20.0];
        // `matchLimit`
        let fits = |limits: &[(Value, Value)]| {
            limits.iter().enumerate().all(|(i, (lo, hi))| {
                let v = vals[i];
                if lo.as_str().is_some_and(|s| s.to_str_lossy() == "!") {
                    return hi.as_number() != Some(v);
                }
                let above = hi.as_number().is_some_and(|h| v > h);
                let below = lo.as_number().is_some_and(|l| v < l);
                !(above || below)
            })
        };
        let Some((_, specials, text)) = descs.iter().find(|(l, _, _)| fits(l)) else {
            return Ok(String::new());
        };
        let mut shown: Vec<String> = vals.iter().map(|v| num(*v)).collect();
        for (k, v) in specials {
            match k.as_str() {
                "display_indexable_non_active_support" => {
                    let gem = supports.get(vals[v - 1] as usize - 1);
                    shown[v - 1] = gem.map(|g| gsub(g, " Support", "")).unwrap_or_default();
                }
                "reminderstring" | "canonical_line" | "canonical_stat" | "_stat" => {}
                k => return Err(format!("Pearl of Tsoatha: description function {k}")),
            }
        }
        let out = gsub_fn(text, "{(%d)}", |c| {
            let n: usize = c[0].parse().ok()?;
            shown.get(n).cloned()
        });
        let out = gsub(&out, "%%%%", "%%");
        Ok(out
            .split("\\n")
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" "))
    }))
}

/// The `[[...]]` literal of the `table.insert` that starts with `call`
fn long_string_after(src: &[u8], call: &str) -> Result<String> {
    long_strings_after(src, call)?
        .into_iter()
        .next()
        .ok_or_else(|| format!("{GENERATED}: no literal after {call}"))
}

/// The `[[...]]` literals of the file after `start`, in order, as Lua
/// reads them (a newline right after `[[` is dropped)
fn long_strings_after(src: &[u8], start: &str) -> Result<Vec<String>> {
    let s = String::from_utf8_lossy(src);
    let at = s
        .find(start)
        .ok_or_else(|| format!("{GENERATED}: {start} not found"))?;
    let mut out = Vec::new();
    let mut rest = &s[at..];
    while let Some(open) = rest.find("[[") {
        let body = &rest[open + 2..];
        let close = body.find("]]").ok_or("unclosed long string")?;
        let mut lit = &body[..close];
        if let Some(l) = lit.strip_prefix("\r\n") {
            lit = l;
        } else if let Some(l) = lit.strip_prefix('\n') {
            lit = l;
        }
        out.push(lit.to_string());
        rest = &body[close + 2..];
    }
    Ok(out)
}

/// The replacement a Watcher's Eye legacy mod function gives:
/// `currentMod:gsub("%(.*%)", "<replacement>")`
fn legacy_replacement(src: &[u8], f: &FuncRef) -> Result<String> {
    let s = String::from_utf8_lossy(src);
    let body = s.get(f.offset as usize..).ok_or("legacy mod: bad offset")?;
    let end = body.find("end").ok_or("legacy mod: no end")?;
    let c = pat::captures(&body[..end], r#"gsub%("%%%(%.%*%%%)", "([^"]*)"%)"#)
        .ok_or_else(|| format!("legacy mod: unexpected body {:?}", &body[..end]))?;
    Ok(c[0].clone())
}
