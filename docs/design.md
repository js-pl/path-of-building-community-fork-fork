# Design

## Goal

A headless Rust calc engine that produces the same numbers as Path of Building
Community (PoE 1), but much faster for workloads that run many calcs per
build: node power, anoint power, item and gem comparisons.

Out of scope for now: UI, PoE 2, character import from the GGG API.

## Why upstream is slow

All numbers refer to upstream `v2.67.2`.

- Node power (`src/Classes/CalcsTab.lua`, `PowerBuilder`) uses
  `calcs.getMiscCalculator` (`src/Modules/Calcs.lua`). For every candidate node
  it runs `calcs.initEnv` and `calcs.perform` from scratch, often followed by
  `calcs.calcFullDPS`.
- It runs one node at a time, in a coroutine on the UI thread that yields
  every 100 ms.
- Anoint power and item/gem comparisons use the same calculator.
- Modifier lookups go through string-keyed tables (`ModDB`, `ModStore`).

## Where the speedup comes from

1. **Parallelism.** Candidates are independent, so they can run on all cores
   (`rayon`). LuaJIT states cannot share a build between threads.
2. **Incremental calcs.** Build the base environment once, clone it cheaply
   and apply only the diff for each candidate instead of rebuilding.
3. **Data layout.** Interned stat IDs and typed modifier storage instead of
   string lookups.

Expected: roughly 3-10x single-threaded (LuaJIT is already fast), multiplied by
core count for power calculations.

## Staying data-driven

Upstream is only partly data-driven:

| Part | Size | Nature |
| --- | --- | --- |
| `src/Data`, `src/TreeData` | ~760 MB on disk, incl. tree images | Lua tables (uniques, bases, gems, mods, tree). ~190 embedded Lua functions in skill data. |
| `src/Modules/ModParser.lua` | ~7k lines | ~4,000 pattern-to-function rules turning mod text into modifiers |
| `src/Modules/Calc*.lua` | ~20k lines | Imperative calc code |

In the 12 months before `v2.67.2`, 271 upstream commits touched the calc
modules and the mod parser (about 5 a week).

Approach:

- **Upstream as a submodule.** `upstream/PathOfBuilding` is pinned to a release
  tag. Updating means bumping the submodule.
- **Build-time importer.** Loads upstream's Lua data files with embedded Lua
  (`mlua`, LuaJIT) and writes typed, compact data for the engine. No GGG data
  is committed to this repository.
- **Fail loudly.** The importer errors on embedded Lua functions or mod parser
  rules it does not know, so upstream changes cannot drift in silently.
- **Mirror upstream structure.** Port the calc modules file by file, keeping
  upstream function names, and record the upstream commit each module is in
  sync with. Upstream diffs then map directly to Rust diffs.
- **Parity tests against real PoB.** Upstream ships `src/HeadlessWrapper.lua`
  and `spec/TestBuilds`. CI runs upstream headless on a corpus of builds and
  fails when the Rust output differs.

## Phases

0. **Baseline.** Benchmark upstream node and anoint power on endgame builds.
   Prototype parallel LuaJIT workers (one build per worker, candidates split
   between them) as a cheap, exact speedup and a reference point.
1. **Importer and parity harness.** Data import from the submodule; headless
   upstream runner producing reference outputs.
2. **Core port.** Modifier database, mod parser and a first calc slice (hits,
   defences), verified against upstream.
3. **Power calculations.** Parallel, incremental node and anoint power.
4. **Later, maybe.** A UI, or integration with
   [rusty-path-of-building](https://github.com/meehl/rusty-path-of-building).

## Legal notes

- Upstream code is MIT (Copyright (c) 2016 David Gowor). Ported code keeps that
  notice, see `LICENSE`.
- Upstream bundles libraries under other licenses (e.g. LGPL `base64.lua`,
  libcurl). None of them are copied here.
- Game data and art are (c) Grinding Gear Games. Data is read from the
  submodule at build time; no art is used.
- The project is unofficial and non-commercial, and is not published to
  crates.io under the PoB name.
- If GGG API access is ever added: registered OAuth app, identifiable
  User-Agent, documented endpoints only.
