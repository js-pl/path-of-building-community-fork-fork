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
  player's stay. Mirages of skills with minions are ported: the copy's
  minion is made on the databases of the calculation the skill comes
  from and is its calculation's `env.minion`, and its output goes with
  the copy's as `output.Minion`. `mirages_match_upstream` and
  `mirage_breakdowns_match_upstream` calculate The Saviour's and
  Tawhoa's Felling's mirages of Dominating Blow in each template, from
  the sidebar and the Calcs tab (8 builds each); the copy's gem level
  and quality go to the breakdown of the output it replaced, so no build
  falls back any more. `calcs_inputs_match_upstream` compares the Calcs tab's
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
  machine).
- Item, gem and anoint power (`engine::compare`): the Items tab's item
  database sort (`ItemDBControl:ListBuilder`), the Skills tab's gem
  dropdown DPS sort (`GemSelectControl:DPSBuilder`) and the anoint popup
  sort (`NotableDBControl:ListBuilder`) each run the build's calculator
  over the candidates and sort by a power stat, like node power. A gem
  swap changes the socket group, not a `CalcOverride`, so it runs on a
  build of its own (`MiscCalculator::calculate_on`, the calculator's
  cache reused). The candidates run on a thread pool (`engine::parallel`,
  which node power now shares); results keep the candidates' order, so
  they do not depend on the thread count. `tests/compare.rs` compares
  each candidate's power with the translated `calcFunc`'s over sampled
  corpus builds (6 items, 6 gems and 5 anoints a build), with the Lua
  build out of `TREE` view mode so `useFullDPS` governs; the 4-thread
  run equals the sequential one. Not wired into the typed session yet.
- Jewels that let nodes be allocated without a connection (Intuitive
  Leap, Thread of Hope, Impossible Escape: `intuitiveLeapLikesAffecting`,
  the dependencies and paths around them) and tattoos (`hashOverrides`,
  `Data/TattooPassives` in the typed tree, `allocatedTattooTypes`) are in
  the spec: `tests/spec.rs` loads generated corpus builds with such a
  jewel socketed and unconnected nodes in its radius allocated
  (`jewel_specs_match_upstream`, 28 specs) and with tattoos on a keystone,
  attribute nodes and a mastery (`tattoo_specs_match_upstream`, 10), and
  `tests/perform.rs` calculates the tattooed builds
  (`tattoos_match_upstream`).
- Timeless jewels: types 1 to 6 (Glorious Vanity, Lethal Pride, Brutal
  Restraint, Militant Faith, Elegant Hubris, Heroic Tragedy) and the
  Abyss jewels (types 7 to 11: Tecrod, Ulaman, Kurgal, Amanamu and
  Zorath's eye jewels): the legion passives and `NodeIndexMapping`
  (`engine::timeless::Timeless`, made by `gamedata::Source::timeless`),
  the look-up tables decompressed from the submodule's `.zip` files when
  a build first needs them (a loader the caller gives; without the table
  a conquered node is `not ported`, and the loader is asked again next
  time), the rest in `engine::codec`'s binary form (`Timeless::encode`,
  shipped in the bundle), and the conquered nodes of
  `BuildAllDependsAndPaths` (replacements, additions, rolled values,
  `ReconnectNodeToClassStart`). The Abyss jewels' tables are read by
  socket, Zorath's by the nodes of the allocated path from its socket to
  the class start (`GetShortestPathToClassStart`) and one node of every
  ascendancy (`DataAbyssJewelLookUpTableHelper.lua`).
  `timeless_specs_match_upstream` and `timeless_match_upstream` socket
  each jewel and conqueror with a seed of its own in generated corpus
  builds: every fifth build's socket with the most allocated nodes in
  its radius (each jewel in all its variants, the legacy ones in every
  third of those builds), and in every build each socket with allocated
  keystones in its radius, with one legacy jewel in turn (113 specs,
  3893 nodes replaced), against the translation with `Inflate` installed
  (headless upstream has none, so the oracle cannot run them). A
  conquered Necromantic Aegis no longer sends the shield's mods to the
  minions (`env.allocNodes[45175].dn` is the spec's node). The Abyss
  variants run last: a conquered node's `setSource` rewrites the mod
  parser's cached mods, which the legion keystones share, so a keystone
  conquered later in the same translation names another node as its
  mods' source; `tests/spec.rs` aligns those sources.
- Every trigger configuration of `configTable` is ported, with the
  helmet's Focus, Snipe, Manaforged Arrows, the uptime of Battlemage's and
  Infernal Cry, Kitava's Thirst (`minimumCostPerUse`) and the active
  part's cooldown of Automation and the like; so are Tawhoa's Chosen,
  overcapped lightning resistance's mana and a minion's curses on itself.
  `triggers_match_upstream` (`equivalence::variants::trigger_variants`)
  calculates every gem a configuration's key names, a support with each
  of a spell, hex, mark and warcry it supports, next to the main skill of
  a melee, bow, wand and spell build, in each of its active skills (379
  variants), and every unique a key names or that triggers skills,
  equipped, with a spell socketed in its slot (372) and as each socket
  group it grants next to a slam and a spell (257). CI runs each gem,
  item, socket group and main skill in one of its templates (287
  variants); `TRIGGERS_FULL=1` runs them in all (1008, half an hour).
  `mods_match_upstream` gives builds mod lines as custom modifiers.

What remains for the full migration, in order:

1. `CalcSections.lua` (which breakdown each cell shows) belongs to the
   session API.
2. Variants of the Calcs tab's other minion and stage selections.
3. The fast path of node power: candidates run on a base environment
   built once. Natively the candidates' calculations already run on a
   `rayon` pool (`power::node_power_with`: a recording pass lists them,
   the pool calculates them, the sequential pass combines them, so
   results do not depend on the thread count; `tests/fast_path.rs`
   compares 1 and 4 threads with the sequential path).
   Item, gem and anoint comparisons are on the calculator
   (`engine::compare`, see Done); the typed session sorts the anoint
   popup and the Skills tab's gem dropdown (`gemList` of a group's gem
   slot, `session::typed::gem_select`) and the Items tab's item databases
   (`controls` of `items.uniqueDB`, `items.rareDB` and their inputs:
   `ItemDBControl`'s filters, list and sort by a stat, a database flask
   or tincture toggled through `Override::toggle_item`, the leagues and
   item types `LoadLeaguesAndTypes` adds when the list is first used, and
   `items.selectDB`, which list shows; `session::typed::item_db_list`)
   with them; the session test's `power`
   phase compares them. The Full DPS skills of one calculation on threads were
   tried (#100, closed) and are not worth it at current pass times: each
   skill calculated ahead from the first setup and taken only where the
   skills before it left the same base databases and cache, identical
   results, but a Full DPS pass takes 5 to 45 ms and the gain was 0.85x
   to 1.7x per build and within noise over the corpus, for speculative
   runs and state matching in the Full DPS path (branch
   `claude/full-dps-threads`).
   Comparing the gem dropdown found two engine gaps, fixed: Iron Grip and
   Iron Will add the skill's own actor's strength damage bonus (a
   minion's for a minion's skill), and the typeless `mod(name, nil, ...)`
   of Corrupting Cry Support's stat map, which no query finds upstream, is
   left out instead of failing every calculation with the support.
   `tests/compare.rs` compares each gem candidate's whole player and
   minion outputs with the translation's.
4. `Data.lua` into the importer, so no Lua runs at load: `crates/gamedata`
   makes the typed data at build time (`engine::game_data::GameData` and
   one `PassiveTree` per tree version, in `engine::codec`'s binary form),
   and the equivalence tests' `TypedData` reads it from there. The mod
   parser too: `gamedata::mod_parser` makes its typed tables and upstream's
   ModCache at build time (`engine::mod_parser::rules`, 2.8 MB, 0.6 MB
   compressed, read in about 30 ms, where evaluating `ModParser.lua` and
   `ModCache.lua` at load took about 1.1 s), and parsing a line runs no Lua
   (closures compute on `mod_parser::val`, results are typed mods). Left:
   the `from_lua` constructors only run at build time now and can move
   into `gamedata`. `pob bundle` ships the binary form (see "Shipping the
   typed engine to the browser"); the wasm module built without the
   translation reads it and runs no Lua at load.
5. The engine API (`crates/session`): replace `session.lua`'s commands,
   which drive upstream's UI classes (controls, tooltips, item crafting,
   tree, Calcs tab cells), with Rust on the typed engine; then drop the
   translation, `luagen` and the Lua runtime (`docs/design.md`, phase 7).
   Started: `session::Dispatcher` answers each command with the typed
   session (`session::typed`) where it is ported, else with the
   translated one, which replays the typed requests that change the
   build before it answers (`Prefer::Translated` forces the
   translation; `pob serve --typed`, typed only `pob serve`). Typed: `loadXml`, `loadCode`,
   `state` (the sidebar: `BuildDisplayStats.lua`, the stat list,
   warnings, class and skill selection controls; the engine's side in
   `engine::sidebar`) and `outputs` (the sidebar calculation's player
   and minion outputs, new). `crates/session/tests/typed.rs`: equal JSON
   to the translation's for all 61 corpus builds (typed load 12.5 s,
   translated 74 s in total). A typed load that fails and any command only the translation has that
   changes the build hand the build to the translation until the next
   load. `nodePower` is typed (`engine::power::node_power` on a
   `MiscCalculator`), equal to the translation's on gen-0001 and gen-0002
   for combined offence/defence and Full DPS (`SESSION_POWER=<id>` runs
   others); the browser's background worker, which only loads builds and
   runs node power, no longer needs the translation.

   The Calcs tab is typed (`session::typed::calcs`): `calcs` (the
   sections of `CalcSections.lua`, evaluated by the session's build
   script, `CheckFlag`, `formatCalcStr`), `breakdown`
   (`CalcBreakdownControl:SetBreakdownData`) and `controls` for the
   tab's and the sidebar's controls, on the `CALCS` calculation made when
   first asked for; `build_output` keeps that calculation's stores and
   actors (`engine::calcs::CalcsView`) and the offence its other cfgs
   (`bleedCfg`, `dotFireCfg`, ...). Equal to the translation for all 61
   corpus builds, every cell's breakdown (14611; typed 1.3 s in total,
   translated 128 s with its loads). Upstream's tree keeps the source
   `modLib.mergeKeystones` last gave a keystone's mods, also across
   builds: a session that loaded another build with an item granting
   the keystone shows that item as the source of the tree's keystone
   mods; the test takes the typed source for such tree keystone rows (46
   rows over the corpus, counted). The session tests share one typed
   data and one translated session (`typed_session_matches_translated`,
   about 3.5 GB).

   Tree edits are typed (`session::typed::tree`): `clickNode`
   (`PassiveSpec:AllocNode` with its path, `DeallocNode` with its
   dependents, ascendancy and bloodline selection, multiple choice
   options), `selectMastery`, `resetTree`, with the cluster jewel
   subgraphs rebuilt after an edit
   (`PassiveSpec::rebuild_cluster_jewel_graphs`), then both calculations
   again as a frame does; and the views `treeState`, `nodePath`,
   `masteryOptions`, `treeSpecs`, `powerStats`. A typed edit is queued
   for the translation, which replays it when it next answers.
   `tree_edits_match_translated`: random click sequences (6 per build,
   masteries with a random effect, large cluster sockets taken off and
   put back, then a reset) on every corpus build give the translation's
   `treeState`, `state`, `outputs`, `nodePath` and `masteryOptions` after
   each step (417 edits: typed 16 s, translated 89 s with its replays).
   Builds with Intuitive Leap and the like are typed too.
   Bloodline nodes are told by their ascendancy being an alternate one
   (the typed tree has no `isBloodline`). `nodeTooltip` stays on the
   translation.

   The tree's geometry is typed (`session::typed::geometry`): `tree`
   gives the spec's nodes with positions, icons and class art, the
   connectors (`PassiveTree:BuildConnector`'s pairs, arcs on a shared
   orbit) and the classes. The typed tree imports each node's `icon`,
   `orbitRadii`, the bounds and the connector pairs; the cluster jewel
   subgraphs' nodes get their positions on the proxy's group and their
   icons, and `PassiveSpec::sub_graphs` their connectors
   (`linkNodes`). `geometry_matches` (`SESSION_PHASES=geometry`): equal
   to the translation's for every corpus build and for an empty build on
   each of the 41 tree versions; `tree_edits_match` compares it after the
   large cluster sockets are taken off and put back, and
   `trees_match_upstream` the imported icons, connectors, radii and
   bounds. Ported on the way: `BuildAllDependsAndPaths`'
   `ReplaceNode(node, self.tree.nodes[id])` where the data lists its
   nodes as an array (2_6): each node with an id up to the node count
   takes the name, lines, mods and icon of the node at that place
   (`PassiveTree::keyed_as`), in the calculations too.

   Skills edits are typed (`session::typed::skills`): `skills`,
   `gemList`, `setMainSocketGroup`, `setSocketGroup`, `addSocketGroup`,
   `removeSocketGroup`, `setGem` (with `ProcessSocketGroup`'s level reset
   when the gem's natural maximum changes), `removeGem`,
   `selectSkillSet`, and `control` of the skill selection controls of the
   sidebar (`build.mainSocketGroup`, `mainSkill`, `mainSkillPart`, stage
   and mine counts, minion and minion skill) and of the Calcs tab
   (`calcs.*`, with `mode` and `showMinion`). `displayLabel` is kept on
   the socket groups as upstream sets it (every group after loading, then
   the groups the sidebar's calculation uses). Ported on the way:
   `ProcessSocketGroup` clearing the cost of a triggered granted skill's
   level on the shared skill data, and `next(levels)` in
   `validateGemLevel` in the data's order (`GrantedEffect::first_level`).
   `skills_edits_match`: random edit sequences (6 per build) give the
   translation's `skills`, `state`, `outputs`, `calcs` and controls after
   each step (358 edits: typed 4.8 s, translated 52 s with its replays).
   After the sequence every group goes into Full DPS, one at a time.
   `calcFullDPS` sets up on `specEnv`'s cached databases, which become
   the parents of its environment's own (`Store::base`): `More` rounds
   each store's product apart (Frost Bomb's Full DPS in
   3.13-dual-wield-cospris-coc: `round(1.19 * 1.2) * 1.24` against the
   main calculation's `round(1.19 * 1.2 * 1.24)`), and a `ReplaceMod` of
   a base mod (`Multiplier:ImpaleStacks`, `MomentumStacks` from the
   config) changes them for the skills after.

   Saving is typed (`session::typed::save`): `saveXml` and `saveCode`
   are `buildMode:SaveDB` with every tab's `Save`, byte for byte the
   translation's for 60 corpus builds, after the tree and skills edit
   sequences, and in both
   round trips (a file saved by either loads in the other to the same
   file, sidebar and outputs). `Item:BuildRaw` is `engine::item_raw`;
   saving re-parses each item from it as `BuildAndParseRaw` does. What
   the typed build does not hold (notes, the Party tab's settings, import, tree
   view and timeless jewel settings, item set links, trade weights)
   comes from the loaded file as the tabs' `Load` keeps it. Ported on
   the way, as upstream leaves them nil: `includeInFullDPS` of a group
   without it, `mainActiveSkill` of a group made in the session until
   the calculation picks it (and its write-back of the clamped index;
   the minions' `skillMinionSkill` and `skillMinionSkillCalcs` likewise
   go back to the gems, the Calcs tab's from the sidebar's calculation,
   which has the same minion skills),
   `enableGlobal2` of the gems the calculations make,
   `secondaryAscendClassId="nil"`. A build that exports its buffs for its
   party (`exportParty`) saves the `ExportedBuffs` of its Calcs tab's
   calculation (`engine::party::BuffExports::texts`, upstream's
   `exportBuffs`). Typed saves take 0.2 to 1.5 s over the corpus (the first save
   re-parses every item), the translation's 2.2 s; the browser's
   background node power, which saves the build first, no longer
   replays the typed edits on the translation (its load alone took
   about 1.2 s per corpus build).

   The Party tab is typed in the engine (`engine::party`, the session's
   `party` view command is not): the `party` phase of
   `typed_session_matches_translated` gives every other corpus build
   socket groups with a link, an aura, a curse or warcry, makes it
   export its buffs and compares sidebar, outputs, Calcs tab and file;
   then each such build takes another's exported buffs as its party
   members' (and every fourth hand-written party stats and every link's
   mods) and is compared again (`PARTY_STEP=1` for all builds). Where
   the typed engine fails on a party build (upstream exports no
   `MaxLifeLeechRatePercent` for Vampiric Link, and a `varList` read
   back from text, also in what `buildOutput` collects for the
   Configuration tab), the translation fails too. Exported mods whose
   keystone source upstream's `mergeKeystones` changed on the tree's
   shared mods are aligned, as `align_keystone_sources` does.

   Items, sets and specs are typed (`session::typed::items`, `sets`):
   `items` (`UpdateSockets`' labels, which slots show, what fits each
   slot in `ItemsTab.items`' `pairs` order), `equipItem`, `deleteItem`
   (with upstream's `allocNodes[index]` quirk on other specs' jewel
   sockets), `selectItemSet`, `selectTreeSpec` (every spec loaded, the
   sockets' jewels moved between them, also to a spec on another tree
   version, which carries its own tree: phase `overlays` switches every
   eighth build to copies of its spec on 3_15 and 3_25 and back),
   `selectConfigSet`, `copySocketGroup`,
   `pasteSocketGroup` and the level box's `control`.
   `PopulateSlots` now empties abyssal sockets past their item's count.
   `items_edits_match`: random sequences of these (6 per build) give the
   translation's items, skills, sidebar, outputs, specs and controls
   after each step and its file at the end (357 edits: typed 7 s,
   translated 71 s with its replays). Saving first makes the Calcs
   tab's calculation, as a frame does, for its write-backs to the gems.

   Tooltips, notes and the party view are typed
   (`session::typed::tooltip`, `node_tooltip`, `item_tooltip`, `notes`,
   `party`): `socketGroupTooltip` (`SkillsTab:AddSocketGroupTooltip`;
   the setup now keeps each gem's `displayEffect`, whether supports are
   superseded and whom they support, and `displayGemList`),
   `nodeTooltip` (`PassiveTreeView:AddNodeTooltip` with its stat
   comparisons on the misc calculator; the tree keeps the nodes'
   `reminderText` and `flavourText`), `itemTooltip` and `previewItem`
   (`ItemsTab:AddItemTooltip` and `AddItemStatDifferences`: every slot
   the item fits, or the named slot, flasks' and tinctures' effective
   stats; the item data keeps `data.flavourText` and the bases' flavour
   text), `buildMode:CompareStatList` with `lowerIsBetter` and
   `compPercent` read from `BuildDisplayStats.lua` at build time,
   `notes`, `setNotes` (saving turns escaped colour codes back, as
   `NotesTab:Save` does) and `party`.
   `tooltips_match` (every corpus build: every socket group's and
   item's tooltip, three items in their own slot, the first item as a
   preview, 10 nodes on and around the tree and the allocated sockets,
   notes and party, then notes set and a node clicked) gives the
   translation's: 1133 item tooltips (typed 19 s, translated 54 s), 695
   node tooltips (10 s, 44 s), 379 socket group tooltips, 59 previews,
   182 notes and 61 party views; 40 requests fell back (listed by the
   test). Upstream's `ProcessSocketGroup` clears a triggered granted
   skill's level cost on the shared skill data for the rest of the
   process (gen-0010's Farrul's Chase triggers Intimidating Cry at level
   20), so a build loaded later in the same session sees that level
   without a cost: taking off Ashes of the Stars in gen-0018 drops its
   level 21 Intimidating Cry to 20, which costs 20 mana in a fresh
   translation and in the typed engine but 0 after gen-0010 was loaded.
   The test asks a request that differs again of a fresh translation
   with the same edits before it, and counts it as aligned when that one
   agrees (2 requests, both gen-0018's amulet, when gen-0010 runs
   earlier in the same shard of phase `tooltips`; with the default 3
   shards they run apart and none needs aligning). `tests/calculator.rs`
   now also takes off the amulet, and recalculates a build that differs
   on a fresh translation the same way (gen-0018 again). Comparing a jewel that changes the
   tree (timeless, Intuitive Leap, Impossible Escape) is
   `buildSpecForJewelComparison`: a copy of the spec with the jewel in
   the socket and its paths built again, which the calculator's
   calculation takes as `override.spec`
   (`MiscCalculator::calculate_on_spec`). Toggling a previewed flask
   or tincture that is not in the build (`toggleFlask = item`) adds it:
   it joins a copy of the items tab under a free negative id
   (`MiscCalculator::calculate_on`). Upstream keeps `displayEffect`,
   `displayGemList`, `finalModList` and `jewelRadiusData` on its objects
   across calculations; the typed tooltips read the sidebar's last
   calculation. A socket's threshold lines sum each radius node's
   `finalModList`: the calculation's for allocated nodes and those near
   a radius jewel (`extraRadiusNodeList`, now kept as well), else the
   node's own `modList`, which `CalcsTab:BuildOutput` sets on every
   node first. Tattooed and conquered nodes show their replacement's
   reminder text, none when it has none (`ReplaceNode` empties the
   node's table), and a tattoo's own only when the tree node has one:
   `BuildAllDependsAndPaths` puts the tree node back before each tattoo,
   which empties the tattoo's table otherwise. The emptied table can be
   the shared tree node's, read through the spec node's metatable, so
   later builds of the process lose that reminder text too: phase
   `overlays` (the tattoo and timeless jewel builds of
   `equivalence::variants`, which the corpus has none of) asks a
   tooltip that differs again of a fresh translation and aligns it when
   that one agrees (11 of 218). An allocated mastery's tooltip with a
   popup open compares reallocating it (`addNodes` of the node), as
   upstream does for the mastery popup's options; `config_edits_match`
   asks for one while a class change popup is open (and checks that
   the mastery's path is itself: masteries link to no node, so none
   depends on one). The party view with imported data shows the labels
   `ParseBuffs` gives (auras, warcries, links and curses with their
   effect, sorted; extra auras and other effects with their mods; the
   enemy modifiers by name, recorded as `Party::load` reads them), or
   the texts in edit boxes with the advanced info shown, in the order of
   their positions on a tab never drawn (width 0). `layoutControls`
   positions the controls by name: on the first layout `ShowAdvanceTools`
   sees the notes unwrapped, then the notes' width wraps them at every
   space (no string has a width headless) for every later control and
   layout; `party_match` asks for the view twice on every party build.
   The tab's controls are typed too (`typed/party_tab.rs`): the import
   code (codes, links, invalid text) to each destination, appended or
   replacing, Import, Clear, Disable Party Effects, Rebuild All, Show
   Advanced Info and the edit boxes, with upstream's split between the
   boxes' texts, the labels and the parsed actor (an edited box is parsed
   only by Rebuild All or an import; Disable Party Effects empties the
   actor alone; party member stats accumulate over imports, as
   `clearInputText` leaves `actor.modDB`). `party_controls_match`
   changes them at random on party builds (about 260 edits) and compares
   the tab, sidebar, outputs, a control's tooltip and the saved file.
   Downloading a linked build stays on the translation. The Tree tab's
   controls that open no popup are typed (`typed/tree_tab.rs`): the
   search (saved as `TreeView`'s `searchStr`; one other than the saved
   leaves the build unsaved, as `Build:OnFrame` counts
   `treeTab.searchFlag`), the heat map, its node power depth (which
   `nodePower` then searches within) and the power report's toggle; the
   spec, compare and heat map stat drop-downs get their lists only when
   the tab draws, so selecting in them headless changes nothing, and the
   compare checkbox's spec only the drawing reads. `tree_controls_match`
   (in the `tree` phase) changes them at random on every fourth build and
   compares the sidebar, outputs, tooltips, node power and the saved
   file. Reset, version conversion, spec conversion and Find Timeless
   Jewel (popups) stay on the translation. A radius jewel's
   "Attributes in Radius" is the data of its last radius function, as
   each sets `jewelRadiusData[nodeId]` in turn (Transcendent Flesh's
   40 Str in gen-0029).
   The translated runtime's `%d` of `-2^63` printed two
   minus signs; it now prints LuaJIT's.

   The Configuration tab, class changes, undo and new builds are typed
   (`session::typed::config`, `undo`): `config` (each option control's
   visibility from its `ifCond`, `ifSkill`, ... rules on the sidebar's
   calculation, whose `env.conditionsUsed`, `skillsUsed`, ... tables
   `engine::sidebar::ConfigUsed` records; values, placeholders the
   options' functions set, tooltips), `control` of the option controls
   with `BuildModList` and the recalculation, of the class, ascendancy
   and bloodline drop-downs (with `IsClassConnected` and the class change
   popup, whose Continue `popupButton` and `closePopup` are typed; the
   typed session's popups are these, so any other button gets the
   translation's "no popup button" error without falling back),
   `undo` (the active spec's `UndoHandler`, also with tattoos and
   cluster jewel subgraphs: `RestoreUndoState` keeps the subgraphs, and
   `ImportFromNodeList` allocates again every subgraph node
   `BuildClusterJewelGraphs` ever allocated anew, `allocExtendedNodes`,
   which upstream never empties; `tree_edits_match` undoes at random
   too) and `newBuild`. The options'
   metadata comes from `Modules/ConfigOptions.lua` at build time
   (`engine::config_tab::ConfigVar` in the game data), with
   `data.enemyIsBossTooltip` evaluated from `Data.lua`.
   `config_edits_match` (phase `config`): random sequences of option,
   class drop-down, popup, config set and undo edits (6 per build) give
   the translation's `config`, sidebar, outputs and tree after each step
   and its file at the end; then a new build with a class change and its
   undo. With `SESSION_STEPS=12`: 707 edits, typed 13.5 s, translated
   129 s with its replays. A build loaded after a class change popup
   opened keeps it, as upstream's popups belong to `main` (the
   dispatcher carries the typed popups over; a popup the translation
   opened while the build was on it is not carried). Saviour's mirage as
   the main skill falls back.

   A typed error starting with `UPSTREAM_ERROR` is one the translation
   answers the same request with (`no node <id>` of `clickNode`,
   `nodePath` and `nodeTooltip`): the dispatcher answers it as the
   request's error and keeps the typed build.

   The dispatcher records each request the typed engine gave to the
   translation while it held the build (`Dispatcher::fallbacks`, with
   the command and why); `typed_session_matches_translated` prints them
   per phase and fails on any not in its `KNOWN_FALLBACKS`.

   Still on the translation, before it can be dropped:
   - Read-only views: `controls` of other tabs (also `party.*`).
   - Tooltips in the cases above that fall back.
   - Items and crafting: the item editor's enchant, anoint, corrupt,
     add implicit, custom and crucible modifier popups, its modifier
     sorting by power (`craftingSorting`), the Foulborn mutation
     checkboxes.
     Typed (`session::typed::item_editor`, `item_db`, `item_popups`):
     `addItem`, `editItem`, `setDisplayItem` and `saveDisplayItem`
     (`AddItem` with its auto-equip, `CreateDisplayItemFromRaw`,
     `CopyAnointsAndEldritchImplicits`, `AddDisplayItem` with the
     Forbidden jewels' counterparts, `SetDisplayItem` with the affix
     drop-downs' lists, `UpdateAffixControl` without the sort by power,
     and `UpdateClusterJewelControls`/`CraftClusterJewel`, which craft a
     crafted cluster jewel again; `Item:Craft` is `engine::item_craft`),
     `itemDb` (`main.uniqueDB` and `main.rareDB`: `data.uniques` and
     `data.rares` with the uniques of `Generated.lua`, made in
     `gamedata::uniques` from the importer's literals of the file, and
     its tree-dependent ones from the typed tree; Pearl of Tsoatha's
     lines come from a port of `describeStats` for its one stat
     description), `craftItem` (Craft Item, and Edit Item Text for a
     unique or relic) with `control` of their `popup.*` inputs,
     `popupButton` and `closePopup`, and `displayItem`
     (`session::typed::item_display`: the tooltip of `item_tooltip.rs`
     on a copy without an id, as upstream compares by identity, and the
     editor's controls as `layoutControls` lists them, placed by their
     anchors as `Control:GetPos` does; a flask in the editor is toggled
     in a copy of the items tab under a free id), with `control` of the editor's
     controls (`session::typed::item_editor_controls`: variants and
     version, sockets and links, influences, quality, catalysts, cluster
     jewel skill and node count, affixes and their rolls, range lines,
     custom modifier removal, the Foulborn mutation checkboxes
     (`Item:MutateMod` in `engine::item_craft`), and the Save, Edit and
     Cancel buttons;
     upstream's state on the controls, such as the edits' text and the
     sliders' values, is kept in `item_editor::Editor`), the Enchant Item
     popup (`session::typed::item_enchant`, with `data.enchantments` from
     `gamedata::enchantments` and `engine::item_enchant`: skill, all
     skills, source, sort by a power stat's calculation with the
     enchanted item, enchantment; Enchant, Remove, Cancel), Corrupt Item
     (`session::typed::item_corrupt`: Corrupted, Scourge and Glimpse of
     Chaos implicits in upstream's sort order, sorted by power, Scourge
     pairs, and a unique's corrupted roll ranges) and Anoint Item
     (`session::typed::item_anoint`: the notable list of
     `NotableDBControl`, searched with `matchOrPattern`, sorted by name or
     a power stat with the anointed item; the translated session's
     `describePopups` builds and lists it, with its search and sort, and
     `control` selects a row) and Add Implicit to Item
     (`session::typed::item_add_implicit`: Searing Exarch and Eater of
     Worlds implicits grouped and sorted as `AddImplicitPopup` does, a
     unique's vestigial implicits from `data.vestigialModMappings` and
     `main.uniqueDB.byTitle`, or a custom line; sorted by power) and Add
     Modifier to Item (`session::typed::item_add_modifier`: bench,
     essence, veiled, beastcraft, Necropolis, fossil, drop-restricted,
     prefix and suffix mods from `item_data::CraftData`, or a custom
     line; sorted by power) and Add Crucible Modifier
     (`session::typed::item_crucible`: each crucible node's mods of
     `data.crucible`, the item's own selected), and the Modifier sorting
     drop-down (`craftingSorting`, which both sessions now list with the
     editor's controls: the affix lists' series by the power of a stat,
     their middle tier added to the item, re-parsed), and
     `controlTooltip` (`session::typed::item_popup_tooltips`, the
     translated session's `api.controlTooltip`): the hover tooltip of a
     popup's drop-down entry or button (`tooltipFunc`,
     `AddModComparisonTooltip`, the enchanted, anointed or saved item's
     `AddItemTooltip`, a notable's anoint comparison; their calculations
     include the Full DPS, as `calcFunc(override)` without `useFullDPS`
     does, and a power sort's only when it sorts by Full DPS). Upstream caches
     applied enemy modifiers by identity, so a flask's copy in the editor
     applies its `EnemyModifier`s again (`apply_enemy_modifiers` counts
     equal mods). A rebuild of cluster subgraphs that shared an id
     (upstream asserts, and its shared tree data stays broken for the
     builds after) falls back on the edit that makes it. The Items tab's
     item list (`session::typed::item_list`, `ItemListControl` as
     `items.itemList` and its inputs): its rows and selection as drawing
     updates them, a row's click with Ctrl (equip) or twice (edit), the
     loadout filter (with `buildMode:SyncLoadouts`'s loadouts), Sort, Del
     Unused, Del All and Delete with their confirmations, and a row's
     tooltip; the item databases' rows (`items.uniqueDB`,
     `items.rareDB`): a click with Ctrl (added and equipped) or twice
     (edited), and their tooltips with a database item's source. The
     translated session's `control` clicks any list's row
     (`SelectIndex`, then `OnSelClick` with the keys held) and
     `describeControl` lists any list's rows. Manage Item Sets
     (`session::typed::item_sets`, `items.setManage`): the sets' list with
     a double click to activate a set, Copy, New, Rename (and F2) with the
     name popup, Delete with its confirmation, and a row's tooltip
     (`AddItemSetTooltip`); the shared list beside it is empty, as only
     dragging fills it. Manage Skill Sets and Manage Config Sets
     (`skills.setManage`, `configTab.setManage`) the same, their unnamed
     list as `popup.1` (a config set's copy is deep: `copyTable` without
     `noRecurse`). The translated session lays out a popup's lists with
     their own buttons, and unnamed lists by their index. A fractional
     enemy level (the config edit "3.5") falls back: upstream's
     `monsterAccuracyTable[3.5]` is nil and its calculation fails.
     Manage Passive Trees (`session::typed::spec_sets`, the Items tab's
     `specButton`): the specs' list (unnamed, `popup.1`) with a double
     click to activate a spec, Copy (a new spec restored from the
     selected one's undo state, without its bloodline as upstream's
     `CreateUndoState` reads none), New (the latest tree with the build's
     classes), Rename and F2 with the name popup, Delete with its
     confirmation; the undo states move with the specs. A tattooed
     spec's copy keeps its tattoos (the undo state's `hashOverrides`;
     phase `overlays` copies the tattoo variants' specs). Import Tree
     and Export Tree (`session::typed::tree_import`, the popup's unnamed
     `popup.2` and `popup.3`, Done `popup.4`): a link of the official
     tree's or poeskilltree's format, its version validated as
     `validateTreeVersion` does (`treeVersions`' numbers and links are
     game data now), decoded into a new spec that becomes the active one,
     or the message `DecodeURL` returns for a broken one; poeplanner's
     format (`DecodePoePlannerURL`: little endian, `poePlannerVersions`
     as game data); the active spec's link (`EncodeURL` with its
     version's prefix). A link's masteries take their lines at once, as
     `AllocateMasteryEffects` sets the node's; one whose effect the tree
     lacks deallocates its node, and one on a node the tree lacks (an
     old tree's link) is a third failure of upstream's own: it indexes
     nil and leaves its session broken, so the typed session falls back
     with `(upstream fails)`. PoEURL links and shrinking start a
     download, which the session has none of: their button is disabled
     with its message, as upstream leaves it (`tree_links_import_and_export`,
     the build's own trees in both formats with their masteries, and the
     popup driver). Rows dragged
     within a list reorder it (`control` with `sel` and `moveTo`, which
     the translated session runs as ListControl's drop on a mutable
     list does, `session::typed::list_order`): the item list while
     unfiltered, the item, skill and config sets, the specs (the active
     spec and the loadouts follow), the shared items and sets, and the
     skills tab's socket groups (the main socket group and the Calcs
     tab's skill follow theirs; `skills_edits_match`). The build's
     loadouts (`session::typed::loadouts`): `buildMode:SyncLoadouts`
     after loading and after every switch, rename, addition or deletion
     of a set or spec, its drop-down's selection, and the sets a changed
     selection switches to (a new spec named as the active one's
     loadout activates it), calculated once per request.
     The Items tab's own controls (`session::typed::items_tab_controls`,
     `items_tab_controls_match` and a step of `items_edits_match`): the
     item set and passive tree drop-downs (`setSelect`, `specSelect`;
     their lists are what the tab's draw sets them to, which the
     translated session now sets before it reads or uses them), the set
     drop-down's tooltip (`AddItemSetTooltip`, which fails past the
     list), the weapon set buttons (the main socket group moves to the
     first group in a slot of the set used), Craft item... and Create
     custom... (Edit Item Text, empty without an item), and the Manage...
     buttons' descriptions. Trade for these items and Buy similar query
     the trade site and stay on the translation.
     The build's own controls (`session::typed::build_controls`,
     `build_controls_match`): the view mode buttons, the loadouts
     drop-down with New Loadout's name popup (a latest-tree spec and an
     item, skill and config set named after it), the level scaling
     button (auto mode re-estimates the level and rebuilds again when it
     changes), the tooltips of the level box (experience multiplier by
     area level, from `data.monsterExperienceLevelMap`, which gamedata
     now derives), the main socket group and the minion drop-down, and
     the descriptions of Back, Save, Save As and Manage Spectres....
     Their clicks (and Help >>) stay unported.
     The shared item list and shared item sets
     (`session::typed::shared_lists`, `items.sharedItemList` and Manage
     Item Sets' `popup.sharedList`): rows (the list shown while the editor
     is empty), selection, a double click to edit, Delete and Rename with
     their popups, row tooltips, and drags (`control` with `dragTo` and
     `at`, which the translated session runs as ListControl's drop does)
     between the item list, the databases and the shared list, and
     between the build's and the shared item sets. Items dropped on an
     item slot (`slots.<name>`, `session::typed::slot_drops`: the slot
     takes the build's item, or a copy of another list's with its
     quality normalised, if it fits) or on the sidebar's minion
     drop-down while it shows an item set (`EquipItemInSet` into that
     set, for the slots the main skill's minion uses;
     `items_dropped_on_slots_and_minion_sets` on gen-0006's Animate
     Guardian's Weapon). They outlive the
     build: a typed session hands them to the next build it loads, and
     after the translation held a build the next typed session reads
     them from it (`sharedLists`, a view of both sessions: each item's
     text and quality, the sets' slots in `pairs` order;
     `shared_lists_survive_a_fallback`).
     A shared set dragged into the build gives its items ids in the
     `pairs` order of its slot names, which the session's tables keep in
     the order they were set (LuaJIT's string hashes are seeded, so
     upstream's own order is not reproducible), and the slots do not
     offer its items until they are populated again (no
     `PopulateSlots`). All in `items_edits_match` (a third of the builds
     share their active set, drag it back and reorder both lists), with
     database items and Forbidden jewels in its pool. The translation's
     tooltip comparison can carry another build's change of upstream's
     shared data (gen-0010's before gen-0018's: a mana cost); the test
     asks a `displayItem` whose lines differ again of a fresh translation
     with the edits since the load. The same gap shows in outputs: upstream clears a
     triggered granted skill's level cost on the shared skill data for
     the rest of the process, the typed engine for the build only
     (`calc_offence::cost_cleared`), so gen-0018 without its amulet
     differs after gen-0010 in one translated session (CI's shards keep
     them apart). The display item
     is held typed, so `displayItem` answers on the translation after
     its replays. A load with an item popup open falls back (the
     popup's buttons act on the tab it came from). A load drops the
     requests queued for the translation, except that the popups and
     the shared lists outlive the build: queued requests that may have
     changed them (a click, a drag, a popup's input, button or close)
     are replayed first. A spec's tattoos are saved (`<Overrides>`, as
     `PassiveSpec:Save` writes them). Ported on the way:
     `initEnv`'s write-back of the clamped `build.mainSocketGroup` (an
     item granting the main socket group replaced).
   - Cluster subgraph id collision (`PassiveSpec:BuildSubgraph`): a
     subgraph's id is built from its socket's expansion index and the
     jewel's size (`0x10000 + index << 6 or << 9 + sizeIndex << 4`), so
     two cluster jewels in sockets with the same index can get the same id
     (seen after random edits that equip or edit cluster jewels). Upstream
     keeps only the second in `subGraphs`; the first stays linked to its
     socket, and the next `BuildClusterJewelGraphs` asserts (`Entrance for
     subGraph not linked to parent socket???`): in the next edit, and in
     the session test also on loading the next build, which leaves the
     translation broken for every build after. The typed spec builds both
     subgraphs fine, so it could not match what upstream gives from then
     on; it marks the collision (`PassiveSpec::sub_graph_collision`) and
     the edit that makes it falls back (`SUB_GRAPH_COLLISION`, a known
     fallback), and the test starts a fresh session after it.
   - Engine gaps that make a build fall back: none over the corpus.
     Combust (gen-0028), the character level's auto mode, tattoos and
     tree edits with Intuitive Leap no longer do; specs in the old URL
     format (`PassiveSpec:DecodeURL`) are typed, and `commands_match`
     loads every tenth build with its specs as tree links.

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
| `PartyTab:Load`, `ParseBuffs`, `exportBuffs`, `modLib.parseFormattedSourceMod`, `formatSourceMod` | `party.rs`: the build's `<Party>` section and `exportParty` (`Build::party`) |
| `env.partyMembers` in `initEnv` and `perform`: the party's enemy mods, its actor (`PARTY`, the player's `partyMembers`), auras, aura debuffs, warcries, links, curses, extra auras and other effects, the `...EqualToPartyMember` stats; `buffExports` | `calc_setup.rs`, `calc_perform.rs` (`party_buffs`), `calc_defence.rs`, `calc_ehp.rs`, `calc_offence*.rs`; `calc_party.rs` (the end of perform's exports) |
| `calcs.buildActiveSkill`, `GlobalCache` | `calc_cache.rs`: a `Recalculator` reruns a copy of the build with another main skill and caches its results by `cacheSkillUUID` |
| `calcs.triggers` | `calc_triggers.rs`: `defaultTriggerHandler`, the rotation simulation and the configurations the corpus needs |
| `calcs.mirages`, `calcs.copyActiveSkill` | `calc_mirage.rs`: the copy is the main skill of a calculation of its own in `CALCULATOR` mode (`Recalc::mirage`), its mod list on snapshots of the database it comes from; a copy of a skill with a minion has a minion of its own, made on those databases (`calc_minion.rs`), whose output the copy's carries as `output.Minion`; General's Cry in `calc_offence.rs` |
| `calcs.buildOutput` (`MAIN`, `CALCS`), `calcs.calcFullDPS` | `calcs.rs`: `build_outputs` from a loaded build, the Full DPS skills each calculated by itself and their rows sorted as the sidebar sorts them; a calculator's `Recalculator` recalculates where upstream's `CALCULATOR` mode does. The Calcs tab's calculation (`CalcMode::Calcs`: its own main skill, buff mode and gem selections, `BuffList`, `CombatList`, `CurseList`, `DisplayDamage`, ...) reuses the sidebar's Full DPS results and `env.multipliersUsed` |
| `calcs.getMiscCalculator`, `CalcOverride` in `initEnv` | `calculator.rs`, `calc_override.rs` (the override and the allocation it gives) |
| `CalcsTab:PowerBuilder`, `data.powerStatList` | `power.rs`; the candidates on a thread pool through `parallel.rs` |
| `ItemDBControl:ListBuilder`, `GemSelectControl:DPSBuilder`, `NotableDBControl:ListBuilder` (the item, gem and anoint sorts) | `compare.rs`: the calculator over the candidates on a thread pool (`parallel.rs`); a gem swap runs on a build of its own (`MiscCalculator::calculate_on`) |
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

Full node power (`--power-depth 0`, every node, combined offence and
defence) on the same sample: typed 18.4 s, the translation 361 s
(19.6x). Before Cryogenesis' added damage redirection and the
non-unique jewel functions of `ModParser.lua`
(`nonUniqueJewelsTransformDamage`, `nonUniqueJewelsGrantAttribute`)
were ported, a node at some depth failed with `not ported` on most
builds, and the session's `nodePower` answered with the translation
after the failed typed run, so it measured no speedup. The session's
`nodePower` on gen-0001: typed 4.3 s, translated 26.9 s; with Full DPS
14.1 s and 69.6 s. No corpus build falls back.

Where the time goes now (perf, node power of gen-0049): `perform` about
half (the offence a quarter), `init_env` a third (`calc_tree::setup`,
the skills' mod lists), and `Sym::new` a fifth: the queries take their
names as strings (`Calc::sum(.., &["Life"])`, often `format!`ed), so
symbols made once per call site are the next step. What calculations share (game data, the
mod parser, trees, a build's shared parts, mods' tags, sources and
values) is `Arc` and `Send + Sync`, asserted at compile time in
`engine/src/lib.rs` (`MiscCalculator` included; its base databases'
cache is a `Mutex`), so candidates and Full DPS skills can run on
threads. `Arc` costs nothing measurable on one thread: six alternating
`--typed-only` runs each, best (mean) ms, `Rc` against `Arc`: perform
10.6 (11.7) / 9.7 (11.2), build_outputs 70.0 (74.3) / 63.6 (75.9),
recalc 91.8 (95.7) / 82.8 (97.3), node power 1549 (1581) / 1368 (1564).

Node power on threads (`bench --typed-only --threads N`, two runs each,
ms, on the shared 4-core machine with another job loading it by about
2.5): without the pool 1388 / 1611, 1 thread 1638 / 1590, 2 threads
1006 / 841, 4 threads 863 / 682. Full node power (`--power-depth 0`,
load about 3.3): without the pool 17948 / 17538, 1 thread 18276, 2
threads 9895, 4 threads 9716 / 9505 (the other job takes the other
cores). The browser stays on one thread.

The mod parser (`cargo run --release -p equivalence --example
parser_bench`): its typed data is made in about 2 s at build time and
read in 25 to 45 ms (before: 1.1 s of Lua evaluation at load); the
48668 oracle lines parse at about 2000 to 2800 lines/s from a fresh
parser (before: about 2600; the scan over every pattern dominates) and
at 2 to 3 million lines/s from its cache (before: 0.6 to 0.75 million,
copying Lua tables).

## Not ported yet in `initEnv`

- `initial_node_mods_match_upstream` skips the builds with cluster jewel
  subgraphs, as it loads no items
- `override.spec` (another spec's jewels) and `override.extraJewelFuncs`
  given by the caller
- Item warnings, superseded supports, gem requirements, `nameSpec` of item
  skills, `limitDisabled` (a second
  timeless jewel over the limit still conquers nodes typed; upstream
  marks it on the shared item during a calculation and leaves it out of
  the next `BuildAllDependsAndPaths`)
- Passives granted only by the latest tree

## Not ported yet in `perform`

Each returns an error starting with `not ported`, which the tests count as
a skip:

- A gap in the curse slots, a minion's debuff without a stack count
- What `buildOutput` collects for the Configuration tab
  (`env.conditionsUsed`, `env.modsUsed`, ...; `env.multipliersUsed` is
  ported)

## Coverage

`oracle/run.py coverage` over the corpus (luacov, lines missed of
`Calc*.lua`): CalcTriggers 539 (42.8% hit), CalcOffence 1201, CalcPerform
883, CalcDefence 504, CalcSetup 301, Calcs 156, CalcMirages 94,
CalcActiveSkill 52, CalcSections 25, CalcTools 17, CalcBreakdown 15,
CalcFormat 3; `target/oracle-coverage/uncovered.txt` lists them. The
variant tests reach far more of CalcTriggers and CalcMirages than the
corpus but run on the translation, not under luacov. Of `configTable`'s
82 keys the trigger variants reach 63; not reached: `call to arms`,
`cospri's malice`, `fissure`, `focus`, `foulborn kitava's thirst`,
`ghostly artillery`, `hex on trap`, `hextouch`, `maloney's mechanism`,
`manaforged arrows`, `mjolner`, `ngamahu's flame`, `nova`, `shockwave`,
`void shockwave`, `supporttriggerfirespellonhit`, `trigger craft`,
`vixen's entrapment`, `FieryImpactHeistMaceImplicit` (their sources: an
item's crafted or enchanted trigger, a helmet enchant, a minion's
trigger, or a source skill the templates lack).

## Next

See the handoff above, and [drop-translation.md](drop-translation.md) for
how the translation can go and what the tests use as reference instead
(option C: step 1, typed-only by default, is done; step 2, the oracle
session `oracle/session.lua` with `SESSION_REFERENCE=oracle`, is under way
and its differences from the translation are listed there).

## Shipping the typed engine to the browser

`crates/wasm` and `crates/session` have a `translation` cargo feature, off
by default in `wasm` (the browser ships the typed engine) and on in
`session` (the CLI and the tests). Built with it
(`web/scripts/build-wasm.mjs --translation`), the module holds upstream's
translated program (`crates/upstream`, the Lua runtime, `lua/session.lua`)
as the fallback of commands the typed engine does not have yet. Built
without it (`web/scripts/build-wasm.mjs`, `cargo build -p wasm`), neither
the translation nor the Lua interpreter is linked, and those commands are `not ported: <cmd>` errors
(`Dispatcher::typed`, `wasm::has_translation` is 0); a `newBuild` or a
`loadXml`/`loadCode` opens a build on the typed engine, and the browser
frontend works for everything the typed session answers (sidebar,
outputs, Calcs tab and breakdowns, tree edits and geometry, skills,
items and the item editor, configuration, class drop-downs, undo, save,
node power): the dispatcher's typed commands are the same in both
builds. The module opens a new build on start (the typed `newBuild`); its
file matches the translation's also after a skill is added (the `new`
phase of `typed_session_matches_translated`).

Data no longer goes through Lua at load. `pob bundle <out>` ships the
typed engine's data in `engine::codec`'s binary form (`session::bundle`):
`core.bin` holds the game data, the mod parser (with upstream's ModCache)
and the current tree, each compressed; every tree version is a separate
file under `files/`, fetched when a build needs an older one. So are
the timeless jewels' data (`typed/timeless.bin`, 47 kB) and each jewel's
look-up table (`typed/timeless/<Name>.bin`, upstream's zlib stream, 0.9 to
30 MB): a build with a timeless jewel asks for them, the worker fetches
them and retries (`session::bundle::decode_timeless`). `tests/bundle.rs`
in `equivalence` loads a build with each of the 11 jewels on the data
read from a bundle, adding the files each asks for, and gets the same
sidebar, outputs and tree as on the data made from source. With
`pob bundle <out> --lua [build.xml ...]`, the bundle also holds the Lua
data files the translation reads starting up and loading the given builds,
and every tree version's Lua data, for an engine built with the
translation (`build-data.mjs --translation`). `pob serve` answers with
the typed engine alone too; `--typed` adds the translation as the
fallback, `--translated` answers everything with the translation.

The frontend takes `not ported` answers as such
(`isNotPorted` in `web/src/engine/client.ts`, `web/src/ui/NotPorted.tsx`): an edit the engine
does not have shows a notice instead of an error, a view it does not have
(notes, party, the item database) a note in its place, the controls of
commands that answered `not ported` are disabled, and tooltips it does
not have are left out.

### Size and load time

wasm module (release `web` profile, `strip = true`) and the `core.bin` it
loads with:

| build | wasm | wasm gzip | core.bin |
|---|---|---|---|
| with the translation | 87.7 MB | 17.1 MB | 33.7 MB |
| typed engine only | 8.2 MB | 2.0 MB | 2.6 MB |

(The 78 MB baseline in `docs/design.md` was before the typed session and
the Calcs tab data were linked; the translation build is 80 MB without
them.) The typed module's code is 7.5 MB unstripped: 5.8 MB engine, 0.5 MB
core, 0.5 MB alloc, 0.3 MB session, 0.2 MB hashbrown, 0.06 MB of the Lua
runtime (`ljtab` and `pattern`, which the typed engine uses for upstream's
table layout and item patterns); no translated program, no Lua evaluator.

Time to the first sidebar in headless Chromium (the frontend built with
Vite, served by a plain static server that sends `application/wasm` so the
module streams and compiles while it downloads, two engine workers each
loading the module and the bundle), median of 3 runs:

| build | localhost | 50 Mbit/s, 40 ms |
|---|---|---|
| with the translation | ~13.5 s | ~48 s |
| typed engine only | ~0.85 s | ~4.0 s |

Each worker logs `engine ready in <n> ms` with the module-compile and
bundle-read splits (`web/src/engine/worker.ts`). The static server does
not gzip; a real host serving the gzipped sizes above cuts the transfer
further. The translated module takes about 14 s just to compile and
instantiate per worker, so it dominates regardless of the link.

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
- `SkillsTab:ProcessGemInstance` empties the cost of the level of a
  triggered skill an item or a node grants on the shared skill data
  (`levels[level].cost = {}`), for every later calculation and build:
  typed, the costs of such a level are empty within the build
  (`Perform::cost_cleared`); `tests/perform.rs` restores the translation's
  costs before each build
- a mirage's copy is cached under the copied skill's uuid, so a later
  calculation without a fresh `GlobalCache` finds the copy's results
  (`TotalDPS`, `Speed`) for that skill: `tests/perform.rs` and
  `tests/breakdown.rs` wipe the cache before their translated
  calculation, as a frame's does
- after a mirage that replaces the main skill (the Saviour, Tawhoa's
  Chosen), `calcs.perform` writes the gem level and quality into the
  output table the copy's replaced, so they show only when no skill was
  found for the mirage
- the build's `EstimatePlayerProgress` sets the level of the auto mode
  after a calculation and rebuilds the configuration's mods without
  calculating again: the shown outputs are the previous level's
- `Flag(...) and x` outputs are nil, not false, without the flag
  (`SustainableTrauma`); `output.x = output.x * a * b` must keep Lua's
  association to stay bit-exact

## Working notes

- `build_output` checks the typed engine alone against the oracle in
  about 20 seconds, without starting upstream: run it first
- Equivalence tests take about 5 minutes each (`cargo test --release -p
  equivalence --test <name>`); `CALCS_FILTER=<substring>` limits the builds
  (`fn-` for the skill variants, whose ids are `fn-<skill>-<part>-<template>`)
- The translated program keeps about 12 MB of every build it loads (the
  cycle collector does not free it; not found yet what holds it), next to
  the 1.6 GB it starts with: a test over hundreds of builds runs them in
  processes of their own, as `perform.rs` does (`run_builds`), or runs out
  of memory on CI's 16 GB
- Missing ports return errors starting with `not ported`; the tests count
  them as skips and print the reasons, the quickest list of what to do next
- Run `cargo fmt --all` last, then `cargo clippy --all-targets -- -D
  warnings` and `sync/check.py`, as CI does
- Never write into `upstream/PathOfBuilding/src`: it rebuilds the
  translated crate (about 15 minutes)
- Update `sync/manifest.toml` with every newly ported file
