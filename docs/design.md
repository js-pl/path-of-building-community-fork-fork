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
web/ (TypeScript)                       crates/
+-- UI: tree, items, skills, config,    +-- importer  build time: upstream Lua data -> typed data
|   calcs, notes, import/export         +-- engine    data model, mod parser, calcs, power calcs
+-- engine client (RPC)                 +-- wasm      wasm-bindgen API over engine
`-- worker pool --- engine.wasm ------> +-- cli       native runner for tests and benchmarks
                                        +-- lua       Lua values, patterns, parser and runtime
                                        +-- upstream  upstream's program translated to Rust (luagen)
                                        `-- equivalence  tests against the oracle
```

- **Engine API.** Load data, load or save a build (PoB XML and build codes,
  compatible with upstream in both directions), apply edits, get outputs and
  breakdowns, run power calculations. The engine does no I/O.
- **Data delivery.** The importer writes versioned, compressed data files. The
  site serves them as static assets and the browser caches them. They are not
  compiled into the wasm module.
- **Workers.** One engine instance per worker, communicating by messages.
  Shared-memory wasm threads would need cross-origin isolation (COOP/COEP)
  and are left as a later option.
- **Frontend.** Mirrors upstream's tabs and features, but uses web-native UI
  instead of copying the desktop UI pixel by pixel.

## Mirroring upstream

Every upstream file that holds logic maps to one Rust or TypeScript module
that keeps upstream's file and function names, so an upstream diff maps
directly to a port diff.

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
rustc's borrow checker is superlinear in function size.

The translation gives exact parity with upstream from the start, and it is
the reference the typed engine is built against: hot paths (`ModDB`/`ModStore`
lookups, `initEnv`, `perform`, Full DPS) are replaced by typed Rust one at a
time, each tested against the translation and the oracle, and the speedups
above come from those replacements, not from the translation.

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

### Catch-up

Port against one pinned tag until all equivalence tests pass. Upstream keeps
moving during that time (about 5 calc or mod parser commits a week), so the
manifest records per module which commit it is in sync with.

### Daily sync job

Once caught up, an LLM job runs every day:

1. Fetch upstream `dev` and list the new commits.
2. Data-only changes: re-run the importer. No LLM needed.
3. For each commit that touches ported files, look up the port locations in the
   manifest and port the diff, commit by commit.
4. Bump the submodule, regenerate the oracle outputs, run all equivalence
   tests.
5. Open a PR with the bump, the port and the upstream commit links. If tests
   still fail after a bounded number of attempts, open an issue with the
   upstream diff and the failing tests instead.

The deployed site follows upstream release tags; tracking `dev` daily keeps
each step small.

### Guardrails

- The job may not edit oracle outputs, the corpus, `tests/tolerances.toml` or
  the comparison rules. CI rejects PRs from the job that touch them.
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
3. **Engine API and WASM.** Build load and save, edits, incremental and parallel
   calcs, power calculations, worker pool.
4. **Frontend MVP.** Import a build code, stat sidebar, passive tree with node
   power, calcs tab with breakdowns.
5. **Full frontend.** Skills, items (including crafting), config, notes, party,
   compare and the remaining features. Character import via OAuth, then trade.
6. **Daily sync.** Starts once the first modules are at parity and is fully on
   after phase 2.

## Open questions

- Frontend framework.
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
