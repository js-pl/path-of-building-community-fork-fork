# Phase 7 progress

Where the typed engine stands and what comes next, for picking the work up
again. The design is in `docs/design.md` (phase 7); this file is the state.

## Done

The whole of `calcs.initEnv` runs typed from a build file:
`engine::build::Build::load` reads the XML, `engine::calc_env::init_env`
chains the slices in upstream's order. Each slice is compared with the
translated `initEnv` for every corpus build in
`crates/equivalence/tests/calc_setup.rs` (`items_setup_matches_upstream`
covers everything from the items on; 57 builds compared, 4 skipped).

| Upstream | Typed |
|---|---|
| `Build.lua` load | `build.rs` |
| `ConfigTab:Load`, config sets, defaults | `config_tab.rs` (options in `config.rs`) |
| `ItemsTab` slots and sets | `items_tab.rs` |
| `PassiveSpec:Load` | `spec.rs` |
| base mods, bandit, pantheon, config | `calc_setup.rs` (`base_dbs`) |
| tree before items, `mergeKeystones` | `calc_setup.rs` (`build_mod_list_for_node_list`) |
| radius jewel functions | `jewel.rs` |
| items into `itemModDB`, radius jewel list | `calc_items.rs` |
| merge, granted passives, tree with jewels, `ExtraJewelFunc` rerun | `calc_tree.rs` |
| granted skill and explosion socket groups, support lists, `createActiveSkill` | `calc_skills.rs` |
| the player and enemy as query actors (`ItemCondition`, ...) | `actor.rs` |
| `buildActiveSkillModList`, `mergeSkillInstanceMods`, `buildSkillInstanceStats` | `calc_skill_mods.rs` |
| `SkillsTab:UpdateSocketGroups` (`matchesSocket`) | `skills.rs` |

Game data is still read from upstream's processed Lua `data` table
(`SetupData`, `ItemData`, `SkillData`, `ConfigData` `from_lua`); porting
`Data.lua` to the importer comes later.

## Not ported yet in `initEnv`

- Cluster jewel subgraphs (the 3.13 corpus builds are skipped for them)
- Energy Blade (`initEnv` reruns with `AffectedByEnergyBlade` and swaps in
  generated weapons; gen-0031 is skipped)
- Calculation overrides (`override.repItem`, `addNodes`, ...), used by the
  node and item power calculators
- Item warnings, superseded supports, gem requirements, `nameSpec` of item
  skills, the character level's auto mode, timeless jewel data
- Passives granted only by the latest tree

## Next

`calcs.perform` and what it calls, about 19k lines of upstream:
`CalcPerform.lua` (3.9k), `CalcOffence.lua` (6.1k), `CalcDefence.lua`
(3.8k), `CalcTriggers.lua` (1.6k), `CalcMirages.lua`, then
`calcs.createMinionSkills` (CalcActiveSkill.lua:868) and `Calcs.lua`'s
calculators. Suggested order: perform's setup (keystones, minion skills,
the per-skill multipliers at CalcPerform.lua:1252-1450), then
`doActorAttribsConditions`, `doActorLifeMana`, the buffs and auras, then
defence and offence. Compare as the setup slices do: the typed output for
the fields ported so far against the translated `env.player.output` after
`calcs.perform`, and the oracle at the end.

## Upstream quirks met so far

The typed port keeps upstream's results but not its shared mutable state;
the tests align these:

- `modLib.setSource` in `mergeKeystones` rewrites the tree's shared keystone
  mods, so their source depends on earlier calculations
- `grantedEffect.fromItem = true` marks a support on the shared skill data
  for every later calculation; typed, the mark holds within one
- `CalcPerform` writes `warcryPowerBonus` onto the tag the skill data shares
  with every copy of the mod; the typed perform should keep it local
- a few skill mods pass a tag where the flags go; such mods take flags no
  query matches (upstream fails if one is queried)
- inline `ModParser.lua` functions have no name or upvalues in the
  translation, so tests accept the typed function there

## Working notes

- Equivalence tests take about 5 minutes each (`cargo test --release -p
  equivalence --test <name>`); `CALCS_FILTER=<substring>` limits the builds
- Run `cargo fmt --all` last, then `cargo clippy --all-targets -- -D
  warnings` and `sync/check.py`, as CI does
- Never write into `upstream/PathOfBuilding/src`: it rebuilds the
  translated crate (about 15 minutes)
- Update `sync/manifest.toml` with every newly ported file
