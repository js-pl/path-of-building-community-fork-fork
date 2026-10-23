# Phase 0 baseline

What upstream costs today, measured with `oracle/run.py bench` at upstream
`v2.67.2` on the initial corpus. The performance targets in
[design.md](design.md#performance-targets) are set against these numbers.

## Upstream headless on LuaJIT

This is the calc code desktop PoB runs, on the LuaJIT commit upstream's CI uses,
with the ModCache loaded as on desktop, but without rendering. It is the
part the rewrite replaces, so it is the main baseline.

Measured on a 4-core Intel Xeon @ 2.80 GHz cloud container, CPU time, median of
3 runs. Treat the numbers as indicative: repeated runs on the same machine
varied by up to 4x for recalc. CI runs the benchmark on every push to `main`
(5 runs, artifact `oracle-bench`) for comparable numbers over time.

| Build | Load (ms) | Recalc (ms) | Node power (s) |
| --- | ---: | ---: | ---: |
| 3.13-dual-savior | 3442 | 51 | 10.1 |
| 3.13-dual-wield-cospris-coc | 1274 | 232 | 33.1 |
| 3.13-generals-perforate-zerker | 1202 | 223 | 28.4 |
| 3.13-mirage-archer-toxic-rain | 4301 | 167 | 26.9 |
| 3.13-occ-vortex | 868 | 110 | 13.5 |
| sample-character-hierophant | 1639 | 185 | 45.9 |

Startup (data, tree, ModCache) took 5.2 s.

- **Load:** XML or character import up to the first full calc. The first 3.13
  build also loads the 3.13 tree.
- **Recalc:** what `buildMode:OnFrame` does after any change: `BuildOutput`
  (`MAIN` and `CALCS`), `RefreshStatList` and `GetMiscCalculator`.
- **Node power:** `CalcsTab:PowerBuilder` to completion: one calc per
  unallocated node with a distinct mod set, plus paths, masteries and cluster
  notables. Desktop PoB spreads this over frames, so the tree shows power
  10 to 46 seconds after a change.

None of the corpus builds uses Full DPS yet, so these numbers do not include
its extra `perform` calls. A big Full DPS build is needed in the corpus.

## Not measured yet

- **Desktop PoB** end to end (Windows, with rendering). The headless numbers
  above cover its calc time; frame time on top of that still has to be
  measured by hand on Windows.
- **pob.cool.** Its own measurements are quoted in
  [design.md](design.md#prior-art-pobcool). The container this baseline ran in
  could not reach pob.cool, so it needs a browser run (Chromium and Firefox) on
  the same builds.
- **Anoint power.** Upstream computes it inside the anoint popup of the items
  tab, not in a function the benchmark can call on its own.
- **Data download size.** Depends on the importer (phase 1).
