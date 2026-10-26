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

`calcs.perform` runs typed after the setup (`engine::calc_perform::perform`,
then `defence`, `defence_estimations` and `player_offence`), compared with
the translated state at `calcs.defence`, `calcs.buildDefenceEstimations`,
`calcs.triggers` and after `calcs.perform` (outputs, the player's and
enemy's databases, every skill's mod list) in
`crates/equivalence/tests/perform.rs`: 40 builds compared, 36 of them
through the offence; minion builds and the gaps below are skipped.

| Upstream | Typed |
|---|---|
| `calcs.perform` | `calc.rs` (stores and queries), `calc_perform.rs` |
| `calcs.defence` | `calc_defence.rs` |
| `calcs.buildDefenceEstimations` | `calc_ehp.rs` |
| `calcs.offence` | `calc_offence.rs` (setup, skill types, durations, costs), `calc_offence_hit.rs` (speed and hit passes), `calc_offence_ailments.rs`, `calc_offence_dot.rs` (DoT, combined DPS) |
| skills' `preDamageFunc`, ... | `calc_skill_funcs.rs`, by granted effect id |
| `calcs.triggers`, `calcs.mirages` | `calc_offence.rs` (`player_offence`): detects them only |

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

## Not ported yet in `perform`

Each returns an error starting with `not ported`, which the tests count as
a skip:

- Minions (`createMinionSkills`, `initMinionModDB`, the minion branches of
  perform, defence and offence), allies' life pools (`calcTotemLife` of
  other skills), the party tab
- Triggers (`CalcTriggers.lua`'s handlers) and mirages (`CalcMirages.lua`)
- Skill functions other than the ten in `calc_skill_funcs.rs`
- Skills whose stages come from a cached calculation (Blight, Penance
  Brand's, Earthquake of Amplification), overcapped lightning resistance's
  mana, self hit damage, PvP scaling, added damage redirection
  (Cryogenesis), Energy Blade
- Breakdowns and the Calcs tab's outputs (`env.mode == "CALCS"`)

## Next

Minions, then triggers and mirages, then `Calcs.lua`'s calculators
(`buildOutput`, `calcFullDPS`) and the comparison with the oracle's JSON.
Port further skill functions as builds need them.

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
- `skillModList:More("MORE", cfg, "Accuracy")` in `calcs.offence` passes
  the string as the cfg: the typed offence queries with an empty cfg
- `Flag(...) and x` outputs are nil, not false, without the flag
  (`SustainableTrauma`); `output.x = output.x * a * b` must keep Lua's
  association to stay bit-exact

## Working notes

- Equivalence tests take about 5 minutes each (`cargo test --release -p
  equivalence --test <name>`); `CALCS_FILTER=<substring>` limits the builds
- Run `cargo fmt --all` last, then `cargo clippy --all-targets -- -D
  warnings` and `sync/check.py`, as CI does
- Never write into `upstream/PathOfBuilding/src`: it rebuilds the
  translated crate (about 15 minutes)
- Update `sync/manifest.toml` with every newly ported file
