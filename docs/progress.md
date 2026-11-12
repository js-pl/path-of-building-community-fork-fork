# Phase 7 progress

Where the typed engine stands and what comes next, for picking the work up
again. The design is in `docs/design.md` (phase 7); this file is the state.

## Handoff

Last session's branch: `claude/typed-power-calcs` (calculation overrides
and the calculators, then node power). To pick the work up:

1. `git submodule update --init --depth 50` (the upstream submodule).
2. Build the tests once: `cargo test --release -p equivalence --test
   perform --no-run` (the translated crate takes about 15 minutes the
   first time).
3. Check the state: `cargo test --release -p equivalence --test
   build_output` (the typed engine against the oracle, under a minute),
   `--test perform` (both tests in parallel, about 14 minutes), `--test
   calc_setup` (about 4 minutes) and `--test breakdown` (about 3
   minutes).

State at the handoff:

- `engine::calcs::build_outputs` is `CalcsTab:BuildOutput`:
  `calcs.buildOutput(build, "MAIN")` with `calcs.calcFullDPS`, then
  `calcs.buildOutput(build, "CALCS")` without breakdowns: all 61 corpus
  builds equal the oracle's `MAIN` and `CALCS` outputs
  (`tests/build_output.rs`). Energy Blade (gen-0031) reruns the setup with
  `AffectedByEnergyBlade` and swaps in generated weapons, which the items
  tab holds under reserved negative ids (`ItemsTab::generated`). Doom
  Blast's trigger sources are ported (all four `doomBlastSource` modes);
  the corpus reaches only the default, Vixen's.
- `perform.rs` and its variants test skip no build.
- Breakdowns (`engine::breakdown`, `Actor::breakdown`, only with
  `env.buildBreakdown`): all of `CalcBreakdown.lua`, and every breakdown of
  `calcs.perform`, `calcs.defence`, `calcs.buildDefenceEstimations`, the
  triggers, the offence (a weapon's pass under `MainHand.`/`OffHand.`, the
  `damageTypes` tables, `applyDmgTakenConversion`), the skills' functions
  and `calcs.buildOutput`, for the player and the minion: equal to the
  translated `calcs.buildOutput(build, "CALCS")` for every corpus build,
  key for key (`tests/breakdown.rs`), and for the skills whose functions
  write breakdowns as the main skill of a corpus build, in each of their
  parts (61 variants, `skill_function_breakdowns_match_upstream`).
  Mirages calculate in `CALCULATOR` mode, without breakdowns, so the
  player's stay. `calcs_inputs_match_upstream` compares the Calcs tab's
  outputs and breakdowns for its other inputs: the buff modes `COMBAT`,
  `BUFFED` and `UNBUFFED` (whose Full DPS sets up on base databases with
  that mode's conditions, `calc_env::init_env_on`), another main socket
  group, a gem's own skill part and second active skill (46 variants).
  PvP scaling is ported, offence and defence.
- Calculation overrides (`engine::calc_override::Override`: nodes added
  and removed, an item replaced or taken off, a flask or tincture
  toggled, conditions) run through the typed `initEnv`
  (`calc_env::init_env_with`), and `engine::calculator::MiscCalculator`
  is `calcs.getMiscCalculator`: its base and override calculations equal
  the translated `calcFunc`'s for every corpus build, over a sample of
  overrides each (`tests/calculator.rs`, 1063 overrides, about 2
  minutes).
- Node power (`engine::power::node_power`, `CalcsTab:PowerBuilder` with
  `data.powerStatList`, `CalculatePowerStat` and the combined offence and
  defence): every spec node's power, the masteries' by effect, cluster
  notables and `powerMax` equal the translated `PowerBuilder`'s
  (`tests/power.rs`, a build in ten with a depth of one point and three
  cluster notables; `POWER_FULL=1` runs every node). Full node power of
  gen-0004 (2618 nodes with power): typed 14 to 22 s with the build's
  own calculations, the translation 42 to 51 s (one thread, a loaded
  machine). Builds with Intuitive Leap and the
  like return `not ported` (the spec does not port them).

What remains for the full migration, in order:

1. Tawhoa's Chosen, the rest of `configTable` in
   `CalcTriggers.lua` (only the configurations the corpus and variants
   reach are ported; a variants test per trigger support would cover
   them), overcapped lightning
   resistance's mana, Cryogenesis' added damage redirection, the party
   tab. `CalcSections.lua` (which breakdown each cell shows) belongs to
   the session API.
2. A variants test per trigger support and per helmet focus, and of
   the Calcs tab's other minion and stage selections.
3. The fast path of node power: candidates as a job list run on a base
   environment built once (`node_power` keeps its API); parallelism
   needs the engine's `Rc` data shared or rebuilt per worker. Then item,
   gem and anoint comparisons on the calculator.
4. `Data.lua` into the importer, so no Lua runs at load: `crates/gamedata`
   makes the typed data at build time (`engine::game_data::GameData` and
   one `PassiveTree` per tree version, in `engine::codec`'s binary form),
   and the equivalence tests' `TypedData` reads it from there. Left: the
   mod parser still takes the importer's tables of `ModParser.lua` and
   `ModCache.lua` at load; the `from_lua` constructors only run at build
   time now and can move into `gamedata`; `pob bundle` should ship the
   binary once the engine API uses the typed engine.
5. The engine API (`crates/session`): replace `session.lua`'s commands,
   which drive upstream's UI classes (controls, tooltips, item crafting,
   tree, Calcs tab cells), with Rust on the typed engine; then drop the
   translation, `luagen` and the Lua runtime (`docs/design.md`, phase 7).
   Started: `session::Dispatcher` answers each command with the typed
   session (`session::typed`) where it is ported, else with the
   translated one, which replays the typed requests that change the
   build before it answers (`Prefer::Translated` forces the
   translation; `pob serve --typed`). Typed: `loadXml`, `loadCode`,
   `state` (the sidebar: `BuildDisplayStats.lua`, the stat list,
   warnings, class and skill selection controls; the engine's side in
   `engine::sidebar`) and `outputs` (the sidebar calculation's player
   and minion outputs, new). `crates/session/tests/typed.rs`: equal JSON
   to the translation's for all 61 corpus builds (typed load 12.5 s,
   translated 74 s in total). A typed load that fails (or the character
   level's auto mode) and any command only the translation has that
   changes the build hand the build to the translation until the next
   load. Next: Calcs tab cells and breakdowns, node power, the simple
   edits, save and export.

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

`engine::calcs::build_outputs` gives a build's player and minion outputs
of the sidebar's and the Calcs tab's calculations, with the Full DPS table
and the cost warnings, compared with the oracle's own `MAIN` and `CALCS`
outputs (`tests/oracle/<id>.json`) in
`crates/equivalence/tests/build_output.rs`, bitwise and key for key: all 61
builds equal in both.

`calcs.perform` runs typed after the setup (`engine::calc_perform::perform`,
then `defence`, `defence_estimations` and `player_offence`), compared with
the translated state at `calcs.defence`, `calcs.buildDefenceEstimations`,
`calcs.triggers` and after `calcs.perform` (outputs, the player's and
enemy's databases, every skill's mod list) in
`crates/equivalence/tests/perform.rs`: all 61 builds compared through
the offence; the gaps below would be skipped. `skill_functions_match_upstream`
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
| `calcs.buildOutput` (`MAIN`, `CALCS`), `calcs.calcFullDPS` | `calcs.rs`: `build_outputs` from a loaded build, the Full DPS skills each calculated by itself and their rows sorted as the sidebar sorts them; a calculator's `Recalculator` recalculates where upstream's `CALCULATOR` mode does. The Calcs tab's calculation (`CalcMode::Calcs`: its own main skill, buff mode and gem selections, `BuffList`, `CombatList`, `CurseList`, `DisplayDamage`, ...) reuses the sidebar's Full DPS results and `env.multipliersUsed` |
| `calcs.getMiscCalculator`, `CalcOverride` in `initEnv` | `calculator.rs`, `calc_override.rs` (the override and the allocation it gives) |
| `CalcsTab:PowerBuilder`, `data.powerStatList` | `power.rs` |
| `CalcBreakdown.lua`, the calculations' `breakdown.*` | `breakdown.rs` (types and helpers); the breakdowns where each calculation writes them, into `Actor::breakdown` |

Every skill with a minion has an actor of its own after the player and the
enemy (`Perform::minions`); its skills follow the player's in
`Perform::lists` from `Perform::n_player` on, their mod lists on the
minion's database. `env.minion` is the main skill's. The test compares the
minion's output (as the player's `output.Minion`), its database and its
skills' mod lists at each stage, and after the minion's own defence and
offence.

The typed engine's game data comes from `crates/gamedata`, through its
binary form (`TypedData::load`, `equivalence::game_data`); no upstream
host is needed for it. `tests/game_data.rs` checks it against
`from_lua` on a started host's `data` (items, skills, setup and config:
6.2 MB, 1.6 MB compressed, read in about 0.15 s natively, where starting
upstream's host takes about 10 s; the 41 trees: 31 MB, 13.6 MB
compressed, about 12 ms each to read, 0.5 s each to make).

## Performance

`cargo run --release -p equivalence --bin bench` (see `docs/design.md`,
"Performance targets"): the sum over the default sample (6 builds) of each
operation's median of 5 runs, in ms, typed against the translation, on a
shared 4-core machine (expect noise of 10 to 30 %):

Each column is typed ms (speedup over the translation in the same run,
which is steadier than the ms on a shared machine), after each change:

| operation | translation | baseline | shared build | cheap copies | names | setup reuse | mimalloc |
|---|---|---|---|---|---|---|---|
| load | - | 594 | 486 | 475 | 526 | 553 | 356 |
| init_env | 50.4 | 25.1 (2.0x) | 13.8 (3.3x) | 11.3 (4.1x) | 8.8 (5.4x) | 8.8 (6.1x) | 2.9 (18.2x) |
| perform | 108 | 55.5 (1.9x) | 27.6 (3.8x) | 24.3 (4.4x) | 25.6 (4.0x) | 29.9 (4.1x) | 9.4 (11.0x) |
| build_outputs | 653 | 705 (0.9x) | 290 (2.0x) | 235 (2.5x) | 249 (2.6x) | 168 (3.8x) | 63.3 (9.2x) |
| misc calculator | 298 | 230 (1.3x) | 84.6 (3.1x) | 60.9 (4.2x) | 62.4 (4.4x) | 47.8 (5.8x) | 19.2 (14.5x) |
| recalc | 1069 | 793 (1.3x) | 371 (2.4x) | 286 (3.0x) | 302 (2.9x) | 221 (4.2x) | 81.8 (11.1x) |
| override | 211 | 99.8 (2.1x) | 47.3 (3.9x) | 34.6 (5.1x) | 33.0 (5.6x) | 30.6 (6.5x) | 13.0 (13.9x) |
| node power (depth 1) | 35836 | 8014 (4.5x) | 4341 (6.8x) | 3265 (8.6x) | 3407 (8.5x) | 2884 (10.9x) | 1413 (19.1x) |

- shared build: a build's copies share its spec and config (`Rc`); every
  calculation copies the build, and copying the spec's nodes and the
  config sets was a third of `build_outputs`.
- cheap copies: a mod's tags (`Tags`, copy on write), a build's items
  and the spec nodes' stats are shared between copies (`Rc`): copying
  mods into each calculation's databases was a fifth of it.
- names: symbol maps and the interner hash with FxHash, a per-thread
  cache of recent names skips the interner's lock and its names'
  scattered bytes, and a list query no mod's name matches gives the empty
  result without copying the list (each node's mods were copied for
  five queries); `Sym::new` went from 14 % to 8 % of node power in the
  profile (the bench gain is within its noise).
- setup reuse: the setup's databases move into the calculation and the
  main skill's cache instead of being copied; calculations that differ
  only in their main skill (the Full DPS skills, the cost warnings' and
  other lookups' recalculations) copy one setup instead of each running
  `init_env` (`calc_env::reuse_setup`), and a build's base databases are
  set up once per set of inputs (`calc_env::BaseDbs`).
  `tests/fast_path.rs` checks both against setting each up afresh.
- mimalloc: the native binaries (`pob`, the tests and the bench) use
  mimalloc as the global allocator; glibc's malloc (`_int_malloc`,
  `malloc_consolidate`) was a third of the calculations' time. The
  browser keeps Rust's own allocator, so fewer allocations still count
  there.

Where the time goes now (perf, node power of gen-0049): `perform` about
half (the offence a quarter), `init_env` a third (`calc_tree::setup`,
the skills' mod lists), and `Sym::new` a fifth: the queries take their
names as strings (`Calc::sum(.., &["Life"])`, often `format!`ed), so
symbols made once per call site are the next step. Running candidates
and Full DPS skills on threads needs the build, the calculation data and
the mods `Send + Sync`: the engine shares them with `Rc`, and
`ModParser` (which `base_dbs` and mastery nodes call) holds Lua tables.

## Not ported yet in `initEnv`

- Rebuilding cluster jewel subgraphs after an edit (they are built once,
  when the build loads); `initial_node_mods_match_upstream` skips the builds
  with subgraphs, as it loads no items
- `override.spec` (another spec's jewels) and `override.extraJewelFuncs`
  given by the caller; tattoo counts
- Item warnings, superseded supports, gem requirements, `nameSpec` of item
  skills, the character level's auto mode, timeless jewel data
- Passives granted only by the latest tree

## Not ported yet in `perform`

Each returns an error starting with `not ported`, which the tests count as
a skip:

- The party tab, and a minion's curses on itself
- Tawhoa's Chosen, mirages of skills with minions
- Trigger configurations other than those in `calc_triggers.rs`
  (`Perform::trigger_config`)
- Overcapped lightning resistance's
  mana, added damage redirection (Cryogenesis)
- What `buildOutput` collects for the Configuration tab
  (`env.conditionsUsed`, `env.modsUsed`, ...; `env.multipliersUsed` is
  ported)

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
- the Calcs tab's `SkillDPS` keeps `calcFullDPS`'s order: only the
  sidebar's table is sorted
- `pairs` over number-keyed tables (allocated nodes, the spec's nodes,
  mastery selections) visits keys in LuaJIT's hash order, which decides
  the order float sums are taken in: keep such tables in
  `lua::ljtab::LjMap`, filled in upstream's order, including `t[k] = nil`
  on missing keys (`ResetNodes`), which gives the key a slot
- a breakdown that upstream assigns from another (`breakdown.HitChance =
  handBreakdown.HitChance`) shares the table: setting `breakdownSource`
  on the actor's crit chance sets it on the hand's too
- the self hit sources' breakdowns join in `pairs` order over upstream's
  handler table; typed they join in a fixed order, equal while a build has
  one source
- only the sidebar's calculation (`MAIN`) adds and removes the socket
  groups of granted skills; a calculator with an item taken off keeps
  the skills it granted
- a calculator's lookups of other skills' results share `GlobalCache`
  with every calculation before in `CALCULATOR` mode, and Saviour's
  mirages take a cached result without calculating it again: typed,
  each calculator calculation starts from the cache of the calculator's
  base calculation, so calculations stay independent of their order
- `Flag(...) and x` outputs are nil, not false, without the flag
  (`SustainableTrauma`); `output.x = output.x * a * b` must keep Lua's
  association to stay bit-exact

## Working notes

- `build_output` checks the typed engine alone against the oracle in
  about 20 seconds, without starting upstream: run it first
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
