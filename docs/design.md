# Design

## Goal

A full rewrite of Path of Building Community (PoE 1) as a web app:

- **Engine** in Rust, compiled to WebAssembly for the browser and natively for
  tools and tests.
- **Frontend** in TypeScript.
- **Same numbers as upstream.** For every build, the engine produces the same
  outputs as upstream at the pinned commit. This is enforced by tests, see
  [Equivalence testing](#equivalence-testing).
- **Fast.** Recalcs after a build change and power calculations (node, anoint,
  item, gem) must feel instant on big endgame builds.
- **Kept up to date** by daily LLM jobs that port upstream commits, see
  [Keeping up with upstream](#keeping-up-with-upstream).

Out of scope for now: PoE 2, a native desktop app.

## Prior art: pob.cool

[pob-web](https://github.com/atty303/pob-web) (MIT, hosted at pob.cool) runs
unmodified upstream Lua in a Lua 5.2 interpreter compiled to WebAssembly, with
a reimplementation of SimpleGraphic and a WebGL renderer, all inside a Web
Worker. It tracks upstream hourly and supports PoE 1, PoE 2 and Last Epoch.

Where it falls short, and where this project has to be better:

- **Speed.** An interpreter instead of LuaJIT, and the same algorithms as
  upstream. Its own measurements put Lua at ~12.5 ms (Chromium) and ~34.5 ms
  (Firefox) per frame just for dragging the tree
  (`docs/rust-lua-vm-experiment-plan.md` in pob-web).
- **UI.** The desktop UI drawn pixel by pixel on a canvas: no text selection,
  accessibility, deep links or native mobile layout.

Lessons to take over: one shared CORS proxy IP gets rate limited; `POESESSID`
must never be used; OAuth character import works from the browser.

## Why upstream is slow

Numbers refer to upstream `v2.67.2`.

- **Build changes.** Every change runs synchronously on the UI thread
  (`buildMode:OnFrame` in `src/Modules/Build.lua`): `calcs.buildOutput` for
  `MAIN` and `CALCS`, plus `getNodeCalculator` and `getMiscCalculator`. Each
  runs `initEnv`, `perform` and `calcFullDPS`, and `calcFullDPS` runs another
  `initEnv` plus one full `perform` per Full DPS skill. With N Full DPS skills
  that is ~8 `initEnv` and ~4(N+1) `perform` calls per change.
- **Power calculations.** `CalcsTab:PowerBuilder` runs `initEnv` and `perform`
  from scratch per candidate node, often followed by `calcFullDPS`, one node at
  a time in a coroutine on the UI thread.
- **Data layout.** Modifier lookups go through string-keyed tables (`ModDB`,
  `ModStore`).

## Where the speedup comes from

1. **Incremental calcs.** Build the base environment once, clone it cheaply and
   apply only the diff (a node, an item, a Full DPS skill).
2. **No redundant work.** Compute shared results such as Full DPS once per
   change, not four times.
3. **Data layout.** Interned stat IDs and typed modifier storage.
4. **Parallelism.** Independent calcs (candidate nodes, Full DPS skills) run on
   a pool of Web Workers in the browser and on threads (`rayon`) natively.
5. **Off the UI thread.** The engine always runs in a worker, so the UI never
   freezes.

None of these may change results; the optimised paths are tested against the
straightforward path, see [Equivalence testing](#equivalence-testing).

## Architecture

```text
web/ (TypeScript, Preact)               crates/
+-- UI: tree, items, skills, config,    +-- importer  build time: upstream Lua data -> typed data
|   calcs, notes, import/export         +-- engine    data model, mod parser, calcs, power calcs
+-- engine client (RPC)                 +-- session   engine API: build sessions, JSON commands
`-- worker pool --- engine.wasm ------> +-- wasm      C ABI over session for the browser
                                        +-- cli       `pob`: native runner, data bundles, benchmarks
                                        +-- lua       Lua values, patterns, parser and runtime
                                        +-- upstream  upstream's program translated to Rust (luagen)
                                        `-- equivalence  tests against the oracle
```

- **Engine API** (`crates/session`). Load or save a build (PoB XML and build
  codes, compatible with upstream in both directions), apply edits, get the
  sidebar, calcs tab cells and breakdowns, tooltips, run power calculations.
  A session runs upstream's translated program and takes JSON requests
  (`{"cmd": ...}`), answered by a Lua bridge (`crates/session/lua/session.lua`,
  translated by `luagen` like upstream) that calls the same functions as
  upstream's controls and tabs do, so every result is upstream's. Upstream's
  own controls (sidebar skill selection, config options, calcs tab options)
  are addressed by name and changed through their handlers. The engine does
  no I/O.
- **Data delivery.** `pob bundle` writes the data files the engine reads when
  starting and loading a build on the current tree into one compressed bundle
  (`core.bin`), and every tree version as separate files. The site serves them
  as static assets; files the engine finds missing (older trees) are fetched
  and the request retried. They are not compiled into the wasm module.
- **WASM.** `crates/wasm` is a plain C ABI (`alloc`, `request`,
  `result_ptr`/`result_len`, ...), so no generated bindings are needed.
- **Workers.** One engine instance per worker, communicating by messages: a
  main worker holds the build being edited, a background worker runs slow
  calculations (node power) on a copy, so edits never wait for them.
  Shared-memory wasm threads would need cross-origin isolation (COOP/COEP)
  and are left as a later option.
- **Frontend** (`web/`, Preact and Vite). Mirrors upstream's tabs and features,
  but uses web-native UI instead of copying the desktop UI pixel by pixel.

## Mirroring upstream

Every upstream file that holds logic maps to the Rust or TypeScript modules
that port it. The typed engine is free-form: it does not keep upstream's file
and function names, so it can be refactored and laid out for speed. Upstream
diffs are ported by behaviour, located through the manifest and checked by the
equivalence tests.

| Upstream | Port |
| --- | --- |
| `src/Data`, `src/TreeData` | `crates/importer` (no port, read at build time) |
| `src/Modules/Calc*.lua`, `ModParser.lua`, `ModTools.lua`, `ItemTools.lua` | `crates/engine` |
| Logic classes: `ModDB`, `ModList`, `ModStore`, `Item`, `PassiveSpec`, `PassiveTree`, ... | `crates/engine` |
| UI classes: `*Tab.lua`, `*Control.lua`, `PassiveTreeView.lua`, ... | `web/` |
| `src/Modules/Build.lua`, `Main.lua` | split between `crates/engine` (state) and `web/` (UI) |

`sync/manifest.toml` lists every upstream file with its port location and the
upstream commit it is in sync with. Files that are deliberately not ported are
listed too, with a reason. CI fails if an upstream file is missing from the
manifest (`sync/check.py`).

**Data from source.** Where upstream mixes data with code, the data is read
from upstream's source rather than copied: the importer evaluates the literal
tables of `ModParser.lua`, and its closures are translated to Rust by
`portgen` (`cargo run -p importer --bin portgen`) or ported by hand. Every
ported closure records the hash of the Lua it came from, and the engine
refuses to start when upstream's source no longer matches.

**Whole-program translation.** `crates/upstream` is upstream's entire
program (`src/Modules`, `src/Classes`, `Launch.lua`, the libraries in
`runtime/lua`) translated statement by statement to Rust by `luagen`
(`crates/importer/src/luagen.rs`) in the crate's build script, so it is always
in sync with the submodule and never edited by hand. The generated code runs on
a dynamic runtime with LuaJIT's semantics (`lua::rt`, `lua::env`: values,
metatables, the standard library with LuaJIT's `table.sort`, `%.14g` number
formatting, LuaJIT's table constructor order for duplicate keys). A headless
host (`crates/upstream/src/host.rs`, a port of `HeadlessWrapper.lua`) runs it
like the oracle does. Data files are not translated: the importer evaluates
them at run time, and the functions they contain are translated by `luagen`
like the rest. Generated functions are split into segment functions, since
rustc's borrow checker is superlinear in function size. Values are reference
counted; a cycle collector (`lua::gc`) frees the cyclic environments upstream's
calcs build on every calculation, following tables, closures and the cells
they capture.

The translation gives exact parity with upstream from the start, and it is
the reference the typed engine is built against. It is scaffolding: the goal
is to replace all of it with typed Rust, hot paths first (`ModDB`/`ModStore`
lookups, `initEnv`, `perform`, Full DPS), each replacement tested against the
translation and the oracle. Once nothing shipped runs on it, the engine is
refactored and optimised freely; the speedups above come from that work, not
from the translation. Long term the translation is dropped: `crates/upstream`,
`luagen` and the Lua runtime go, the oracle is the only reference, and Lua is
left only in the importer (reading upstream's data at build time) and in the
oracle itself.

**Fail loudly.** The importer errors on embedded Lua functions or mod parser
rules it does not know, so upstream changes cannot drift in silently.

## Equivalence testing

### Oracle

Upstream itself, at the pinned commit, run headless with
`src/HeadlessWrapper.lua` on LuaJIT in CI. A dumper script writes upstream's
results as JSON to `tests/oracle/`, and CI checks that upstream reproduces
them exactly. Expected values always come from the oracle and are never
written by hand. Details, the JSON format and the determinism checks are in
[oracle/README.md](../oracle/README.md).

### Layers

1. **Data.** Every imported table matches the Lua table it came from.
2. **Mod parser.** Every mod line upstream knows (tree nodes, uniques, base and
   crafted mods, cluster and timeless jewels, ...) parses to the same modifier
   list as `modLib.parseMod`. This covers all of it, not a sample.
3. **Upstream specs.** `spec/System/*_spec.lua` is ported to Rust tests.
4. **Whole builds.** For every build in the corpus: all of `player.output`,
   `minion.output` and the Full DPS table, in both `MAIN` and `CALCS` mode, plus
   breakdowns once the calcs tab is ported.
5. **Variants.** Each corpus build is mutated the way users mutate builds:
   - allocate or deallocate each node,
   - toggle each config option,
   - pick each skill as the main skill,
   - add each skill to Full DPS,
   - change gem levels and qualities,
   - swap items.

   Random mutation sequences are fuzzed against the oracle, and failures are
   shrunk to a minimal reproduction.
6. **Power calculations.** Node, anoint, item and gem power match upstream's
   results.
7. **Fast path vs simple path.** Incremental, deduplicated and parallel
   results equal a from-scratch single-threaded calc. Engine-only, so it also
   runs on inputs the oracle cannot handle quickly.
8. **Round trip.** Builds saved by the engine load in upstream with identical
   outputs, and the other way round.

### Corpus

- Listed in `corpus/corpus.toml`.
- The 5 builds in upstream's `spec/TestBuilds`, which are old (3.13), and
  upstream's `spec/System/SampleCharacter.json` on the current tree.
- Generated builds on the current tree (`corpus/generate.lua`), cycling
  through every class, ascendancy and kind of main skill, with random trees,
  items, jewels, flasks, configs, minions and Full DPS. Grow the count, or add
  targeted builds, wherever coverage shows gaps.
- A curated set of current builds covering every class, ascendancy, main skill
  type and major unique interaction, plus generated builds for gaps.
- Build sources and licensing are recorded. The corpus may live outside this
  repository.

### Coverage gates

- The oracle runs with Lua line coverage (`luacov`) on `Calc*.lua` and
  `ModParser.lua`. Lines the corpus never reaches are listed, and the corpus
  grows until the list is empty or every remaining line is justified.
- Every mod parser rule must be hit at least once.

### Comparison rules

- Integers, strings and flags compare exactly.
- Numbers compare bitwise. Differences are only allowed for results of
  transcendental functions (`pow`, `log`, ...), which may differ by a few ULP
  between LuaJIT and Rust's libm. Each allowed case is listed in
  `tests/tolerances.toml` with a reason.
- A missing or extra output key is a failure.

## Keeping up with upstream

Upstream moves about 5 calc or mod parser commits a week. Syncing starts
now, while most of the program still runs translated: those parts follow
upstream for free on every bump, and each typed replacement only has to catch
up with the commits since it was written. The manifest records per file which
upstream commit its port is in sync with.

### Daily sync job

An LLM job runs every day with the `sync` skill
(`.claude/skills/sync/SKILL.md`):

1. Fetch upstream `dev` and list the new commits.
2. Data-only changes: bump and regenerate. No porting needed.
3. For each commit that touches ported files, look up the port locations in the
   manifest and port the change, commit by commit.
4. Bump the submodule, regenerate the oracle outputs, the corpus and the
   generated ports, run all equivalence tests.
5. Open a PR with the bump, the port and the upstream commit links. If tests
   still fail after a bounded number of attempts, open an issue with the
   upstream diff and the failing tests instead.

The deployed site follows upstream release tags; tracking `dev` daily keeps
each step small.

### Guardrails

- The job never edits oracle outputs, the corpus, `tests/tolerances.toml` or
  the comparison rules by hand. Oracle outputs and the generated corpus are
  only regenerated with their tools, in the PR that bumps the submodule, and
  CI fails if they differ from what upstream produces.
- Every PR needs green CI and is merged by the owner.
- Commit messages and port headers reference the upstream commit.

## Performance targets

Upstream takes 50 to 230 ms per recalc and 10 to 46 s for full node power on
the initial corpus (LuaJIT, headless). Targets, to be set with the engine:

- recalc after a build change on a big Full DPS build,
- full node power,
- anoint power,
- data download and startup time.

## Phases

0. **Harness first.** Oracle runner and JSON dumper in CI, initial corpus,
   coverage reporting. Done.
1. **Data and mod parser.** Importer and mod parser at full parity.
   Done: every table `Data.lua` loads imports byte-identical to upstream's;
   all 41601 mod lines (every line upstream ships plus synthetic lines for
   rules no real line reaches) parse identically, and the engine preloads
   upstream's ModCache as upstream does. Tree data, stat descriptions and
   timeless jewel data are imported with the phases that need them.
2. **Core calcs.** Whole builds match. Done by translating upstream's whole
   program (see [Mirroring upstream](#mirroring-upstream)): every corpus build
   calculates to the oracle's outputs exactly, in `MAIN` and `CALCS` mode
   (`crates/equivalence/tests/calcs.rs`). Variants follow with the engine API.
3. **Engine API and WASM.** Done on the translation: build load and save
   (XML and build codes), edits, outputs, sidebar, calcs tab and breakdowns,
   tooltips and node power, natively and in the browser
   (`crates/session/tests`, `web/tests` compares the wasm engine with the
   native one). Open: incremental and parallel calcs and the performance
   targets, which come with the typed engine, and the wasm size (78 MB, 15 MB
   gzipped).
4. **Frontend MVP.** Done: import a build code or XML, stat sidebar, passive
   tree with node power, calcs tab with breakdowns.
5. **Full frontend.** Skills, items (including crafting), config, notes, party,
   compare and the remaining features. Character import via OAuth, then trade.
   Done: skills (socket groups, gems, main skill, Full DPS), items (slots,
   item sets, items from text, unique and rare templates, tooltips, and
   crafting with upstream's item editor: bases, affixes, variants, quality,
   influences, catalysts, cluster jewels, enchants, anoints, corruption),
   config, notes, party, tree, skill and config sets, export (code, link, XML)
   and builds saved in the browser. Open: compare, character import via
   OAuth, trade, art for the tree, node power split over several workers.
6. **Daily sync.** Starts now, with the `sync` skill, alongside phase 7.
7. **Typed engine.** Replace the whole translation with typed Rust, hot paths
   first, each step at parity with the translation and the oracle. Then
   refactor and optimise freely toward the performance targets, and drop the
   translation, `luagen` and the Lua runtime. The engine API keeps its JSON
   commands, with `session.lua` replaced by Rust.
   Replacements are installed into the running translation
   (`engine::install`) and replace its methods; `POB_TRANSLATED=1` turns
   them off to compare. Native code that still works on the program's Lua
   tables is only faster where it saves the translation's dispatch: the
   queries of `ModStore`, `ModDB` and `ModList`
   (`crates/engine/src/mod_store.rs`) took about 25% off edits and node
   power, a native `PassiveSpec:BuildPathFromNode` nothing. Speedups to the
   translation's runtime (generational cycle collection, hashed strings)
   go with it; the lasting work is the typed engine.

   The typed engine starts with the mod core (`crates/engine/src/mods`):
   interned names, typed mods, tags and values, ModDB and ModList stores in
   one arena, and their queries, in plain Rust. `crates/equivalence/tests/mods.rs`
   converts the stores of every corpus build's calculation and runs every
   query for every mod name and type present, with no cfg and with each
   active skill's, against the translated stores (565k queries, all equal).
   What the queries need from the rest of the engine (actors, outputs,
   items and gems) is the `Actors` trait, which the test answers from the
   translation until those parts are typed. `engine::mods::convert` turns
   upstream's mods as Lua values (the importer's data, the mod parser's
   output) into typed ones; every one of the 41601 parsed lines converts
   and encodes back to upstream's mods (`tests/mod_parser.rs`).

   The build model follows, bottom up: first the passive tree
   (`crates/engine/src/tree.rs`), typed nodes with their stat parses and
   mods, positions, links, jewel radii and lookup tables, equal to the
   translated tree for every tree version the corpus uses
   (`tests/tree.rs`). Then the passive spec (`crates/engine/src/spec.rs`):
   class and ascendancy, allocation, mastery selections, dependencies and
   paths, equal to the translated spec of every corpus build
   (`tests/spec.rs`); jewels that change the tree, tattoos and cluster
   jewel subgraphs follow. Then items (`crates/engine/src/item.rs`, with
   `item_data.rs` and `item_tools.rs`): an item's text parsed into its
   base, properties, sockets, variants, affixes and mod lines, rolled and
   scaled as upstream does, and the mods it grants in each slot with local
   mods applied and its weapon, armour, flask, tincture and jewel data,
   equal to the translated item for every corpus build item and every
   unique and rare template (`tests/item.rs`); crafting and text building
   follow with the UI. Then the configuration (`crates/engine/src/config.rs`):
   every option's apply function and the map mods' as plain Rust, the boss
   presets with the placeholders they set, and the custom modifier lines,
   equal to the translated Configuration tab for every corpus build and for
   every option set alone to each of its values (`tests/config.rs`). Then
   the Skills tab (`crates/engine/src/skills.rs`, with typed gem and skill
   data in `skill_data.rs`): skill sets and socket groups loaded from a
   build, each gem found and its level and requirements settled, equal to
   the translated tab for every corpus build (`tests/skills.rs`). Builds are
   read with `engine::xml`, upstream's XML reader. Then `initEnv`
   (`crates/engine/src/calc_setup.rs`) building typed stores, in slices
   each compared with the translated `calcs.initEnv`'s databases on the
   mods of the sources ported so far (`tests/calc_setup.rs`): first the
   base mods of the player and the enemy, the bandit, pantheon and
   configuration mods; then the tree before the items with its keystones;
   the items in their slots (`calc_items.rs`) with the radius jewels; the
   tree with the items, their granted passives and jewel functions
   (`calc_tree.rs`); the socket groups of granted skills and explosions
   (`calc_skills.rs`); then the active skills.

   Item text and data lines are matched with upstream's own patterns
   (`engine::pat`, Lua patterns on Rust strings), so the parse is
   upstream's. The item data is read from upstream's processed `data`
   table for now; porting `Data.lua`'s item processing makes it typed data
   of the importer.

   Upstream mutates shared data in a few places, and the typed engine does
   not copy that by accident: `modLib.mergeKeystones` sets the source of a
   tree keystone's mods to whatever else grants the keystone, so the tree
   itself changes after a calculation.

## Open questions

- Worker pool vs wasm threads, and memory per engine instance.
- Corpus sources and where to store them.
- Hosting, and the site's name.
- PoE 2 later.

## Legal notes

- Upstream code is MIT (Copyright (c) 2016 David Gowor). Ported code keeps that
  notice, see `LICENSE`. pob-web is MIT as well.
- Upstream bundles libraries under other licenses (e.g. LGPL `base64.lua`,
  libcurl). None of them are copied here.
- Game data and art are (c) Grinding Gear Games and are never committed to this
  repository; they are generated from the submodule at build time.
- **Change from the original plan:** the deployed site has to serve game data
  and passive tree art, as PoB and pob.cool do. It stays unofficial and
  non-commercial, shows a GGG disclaimer and complies with takedown requests.
- The site and packages do not use the Path of Building name as their own.
- GGG API access: registered OAuth app, identifiable User-Agent, documented
  endpoints only, respect rate limits, never `POESESSID`.
