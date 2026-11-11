# Phase 7 progress

Where the typed engine stands and what comes next, for picking the work up
again. The design is in `docs/design.md` (phase 7); this file is the state.

## Handoff

Last session's branch: `claude/typed-build-output`, merged as one PR. To
pick the work up:

1. `git submodule update --init --depth 50` (the upstream submodule).
2. Build the tests once: `cargo test --release -p equivalence --test
   perform --no-run` (the translated crate takes about 15 minutes the
   first time).
3. Check the state: `cargo test --release -p equivalence --test
   build_output` (the typed engine against the oracle, under a minute),
   `--test perform` (both tests in parallel, about 14 minutes) and `--test
   calc_setup` (about 4 minutes).

State at the handoff:

- `engine::calcs::build_output` is `calcs.buildOutput(build, "MAIN")`
  with `calcs.calcFullDPS`: 58 of the 61 corpus builds equal the oracle's
  `MAIN` outputs (`tests/build_output.rs`); skipped are Energy Blade
  (gen-0031) and Doom Blast's trigger (gen-0002, gen-0036).
- Skips left in `perform.rs`: Energy Blade (gen-0031) only. The variants
  test has none.

What remains for the full migration, in order:

1. Energy Blade (`initEnv` reruns with `AffectedByEnergyBlade`), Doom
   Blast's trigger sources, Tawhoa's Chosen, the rest of `configTable` in
   `CalcTriggers.lua` (only the configurations the corpus and variants
   reach are ported; a variants test per trigger support would cover
   them), overcapped lightning
   resistance's mana, PvP, Cryogenesis' added damage redirection, the
   party tab.
2. The Calcs tab's mode (`env.mode == "CALCS"`) and breakdowns
   (`CalcBreakdown.lua`, `CalcSections.lua`).
3. Calculation overrides and the power calculators (node power, item
   comparisons).
4. `Data.lua` into the importer, so no Lua runs at load.
5. The engine API (`crates/session`): replace `session.lua`'s commands,
   which drive upstream's UI classes (controls, tooltips, item crafting,
   tree, Calcs tab cells), with Rust on the typed engine; then drop the
   translation, `luagen` and the Lua runtime (`docs/design.md`, phase 7).

## Done

The whole of `calcs.initEnv` runs typed from a build file:
`engine::build::Build::load` reads the XML, `engine::calc_env::init_env`
chains the slices in upstream's order. Each slice is compared with the
translated `initEnv` for every corpus build in
`crates/equivalence/tests/calc_setup.rs` (`items_setup_matches_upstream`
covers everything from the items on; 61 builds compared).

| Upstream | Typed |
|---|---|
| `Build.lua` load | `build.rs` |
| `ConfigTab:Load`, config sets, defaults | `config_tab.rs` (options in `config.rs`) |
| `ItemsTab` slots and sets | `items_tab.rs` |
| `PassiveSpec:Load` | `spec.rs` |
| `PassiveSpec:BuildClusterJewelGraphs`, `BuildSubgraph`, legacy cluster ids | `spec_cluster.rs`: the subgraphs' nodes are the spec's own (`SpecNode::synth`) |
| base mods, bandit, pantheon, config | `calc_setup.rs` (`base_dbs`) |
| tree before items, `mergeKeystones` | `calc_setup.rs` (`build_mod_list_for_node_list`) |
| radius jewel functions | `jewel.rs` |
| items into `itemModDB`, radius jewel list | `calc_items.rs` |
| merge, granted passives, tree with jewels, `ExtraJewelFunc` rerun | `calc_tree.rs` |
| granted skill and explosion socket groups, support lists, `createActiveSkill` | `calc_skills.rs` |
| the player and enemy as query actors (`ItemCondition`, ...) | `actor.rs` |
| `buildActiveSkillModList`, `mergeSkillInstanceMods`, `buildSkillInstanceStats` | `calc_skill_mods.rs` |
| `SkillsTab:UpdateSocketGroups` (`matchesSocket`) | `skills.rs` |

`engine::calcs::build_output` gives a build's player and minion outputs
with the Full DPS table and the cost warnings, compared with the oracle's
own `MAIN` outputs (`tests/oracle/<id>.json`) in
`crates/equivalence/tests/build_output.rs`, bitwise and key for key: 58 of
61 builds equal, the rest skipped (see the handoff).

`calcs.perform` runs typed after the setup (`engine::calc_perform::perform`,
then `defence`, `defence_estimations` and `player_offence`), compared with
the translated state at `calcs.defence`, `calcs.buildDefenceEstimations`,
`calcs.triggers` and after `calcs.perform` (outputs, the player's and
enemy's databases, every skill's mod list) in
`crates/equivalence/tests/perform.rs`: 60 builds compared, 58 of them
through the offence; the gaps below are skipped. `skill_functions_match_upstream`
does the same for every gem with skill functions, in each of its parts, as
the main skill of a corpus build with fitting weapons (238 variants, about
13 minutes).

| Upstream | Typed |
|---|---|
| `calcs.perform` | `calc.rs` (stores and queries), `calc_perform.rs` |
| `calcs.defence` | `calc_defence.rs` |
| `calcs.buildDefenceEstimations` | `calc_ehp.rs` |
| `calcs.offence` | `calc_offence.rs` (setup, skill types, durations, costs), `calc_offence_hit.rs` (speed and hit passes), `calc_offence_ailments.rs`, `calc_offence_dot.rs` (DoT, combined DPS) |
| skills' `preDamageFunc`, ... | `calc_skill_funcs.rs`, by granted effect id: all of them, the minions' tested only through the corpus |
| `calcs.createMinionSkills`, `initMinionModDB`, `addMinionModifiers`, the minion's defence and offence | `calc_minion.rs`; the minion branches of perform, defence and offence in place |
| `calcs.buildActiveSkill`, `GlobalCache` | `calc_cache.rs`: a `Recalculator` reruns a copy of the build with another main skill and caches its results by `cacheSkillUUID` |
| `calcs.triggers` | `calc_triggers.rs`: `defaultTriggerHandler`, the rotation simulation and the configurations the corpus needs |
| `calcs.mirages`, `calcs.copyActiveSkill` | `calc_mirage.rs`: the copy is the main skill of a calculation of its own (`Recalc::mirage`), its mod list on snapshots of the database it comes from; General's Cry in `calc_offence.rs` |
| `calcs.buildOutput` (`MAIN`), `calcs.calcFullDPS` | `calcs.rs`: `build_output` from a loaded build, the Full DPS skills each calculated by itself and their rows sorted as the sidebar sorts them; a calculator's `Recalculator` recalculates where upstream's `CALCULATOR` mode does |

Every skill with a minion has an actor of its own after the player and the
enemy (`Perform::minions`); its skills follow the player's in
`Perform::lists` from `Perform::n_player` on, their mod lists on the
minion's database. `env.minion` is the main skill's. The test compares the
minion's output (as the player's `output.Minion`), its database and its
skills' mod lists at each stage, and after the minion's own defence and
offence.

Game data is still read from upstream's processed Lua `data` table
(`SetupData`, `ItemData`, `SkillData`, `ConfigData` `from_lua`); porting
`Data.lua` to the importer comes later.

## Not ported yet in `initEnv`

- Rebuilding cluster jewel subgraphs after an edit (they are built once,
  when the build loads); `initial_node_mods_match_upstream` skips the builds
  with subgraphs, as it loads no items
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

- The party tab, and a minion's curses on itself
- Tawhoa's Chosen, mirages of skills with minions
- Trigger configurations other than those in `calc_triggers.rs`
  (`Perform::trigger_config`), breakdowns of triggers
- Overcapped lightning resistance's
  mana, PvP scaling, added damage redirection (Cryogenesis), Energy Blade
- Breakdowns and the Calcs tab's outputs (`env.mode == "CALCS"`), and
  what `buildOutput` collects for the Configuration tab
  (`env.conditionsUsed`, `env.modsUsed`, ...)
- Doom Blast's trigger sources

## Next

See the handoff above.

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
- the sidebar (`buildMode:AddDisplayStatList`) sorts `output.SkillDPS` in
  place with `table.sort`, so the oracle has the rows in that order:
  `calcs::lua_sort` is Lua 5.1's `auxsort`, for the order of equal rows
- `calcFullDPS` reuses its first setup for each skill (`accelerate`); the
  typed one sets each skill up afresh, with equal results on the corpus
- a weapon pass's cfg sees the pass's output live as `skillStats`: take
  the cfg again (`pcfg`) after setting outputs a later query reads
- `Flag(...) and x` outputs are nil, not false, without the flag
  (`SustainableTrauma`); `output.x = output.x * a * b` must keep Lua's
  association to stay bit-exact

## Working notes

- `build_output` checks the typed engine alone against the oracle in
  under a minute: run it first
- Equivalence tests take about 5 minutes each (`cargo test --release -p
  equivalence --test <name>`); `CALCS_FILTER=<substring>` limits the builds
  (`fn-` for the skill variants, whose ids are `fn-<skill>-<part>-<template>`)
- Missing ports return errors starting with `not ported`; the tests count
  them as skips and print the reasons, the quickest list of what to do next
- Run `cargo fmt --all` last, then `cargo clippy --all-targets -- -D
  warnings` and `sync/check.py`, as CI does
- Never write into `upstream/PathOfBuilding/src`: it rebuilds the
  translated crate (about 15 minutes)
- Update `sync/manifest.toml` with every newly ported file
