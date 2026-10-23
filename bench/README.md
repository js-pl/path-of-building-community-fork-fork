# Phase 0 baseline

Times upstream node power and anoint power on real builds, single-threaded and
with parallel LuaJIT worker processes, and checks that both give bit-identical
results. Results are in [docs/phase0-baseline.md](../docs/phase0-baseline.md).

## Requirements

- the upstream submodule: `git submodule update --init`
- LuaJIT with the `lua-utf8` C module and zlib, e.g. on Debian/Ubuntu:

  ```sh
  apt-get install luajit libluajit-5.1-dev luarocks
  luarocks --lua-version=5.1 install luautf8
  ```

## Running

```sh
bench/run.sh                                   # upstream spec/TestBuilds, all cores
bench/run.sh --workers 8 my-build.xml code.txt # own builds (XML or share code)
bench/run.sh --tasks node:FullDPS,anoint       # pick tasks
```

Tasks are `node:<power stat>` (`combined` is the tree tab's default
"Offence/Defence", otherwise a `stat` from `data.powerStatList`, e.g.
`FullDPS`) and `anoint`. Output goes to `target/bench/`: `report.md`,
`report.json` and the raw per-calc results of every worker.

## How it works

- `lua/pob.lua` boots upstream through `src/HeadlessWrapper.lua`, with a real
  clock and zlib for share codes. `CI` is unset so the mod cache is used like
  in the app.
- `lua/power.lua` turns power calculations into independent calc jobs.
  - Node power is not reimplemented. A plan pass runs upstream
    `CalcsTabClass:PowerBuilder` with a stub calculator that records the
    overrides it asks for, so the candidates always match upstream.
  - Anoint power mirrors `NotableDBClass:ListBuilder`: one calc per anointable
    notable on the equipped amulet, plus the base calc without anoint.
  - Every calc result is reduced to a fingerprint: all power stats plus the
    fields the combined offence/defence score reads, printed with `%.17g`.
- `lua/worker.lua` is one upstream instance. It loads the build, plans the
  jobs, sorts them by key and calculates every N-th job. With `--upstream` it
  instead runs `PowerBuilder` unchanged and records each calc it makes.
- `lua/run.lua` runs the single-threaded baseline and then N workers at once,
  merges their results and compares them with the baseline job by job.

Each worker is a separate process because LuaJIT states cannot share a build
between threads. "Parallel" in the report is the slowest worker's plan and
calc time, i.e. what long-lived workers that already hold the build would
need. "Incl. startup" adds booting upstream and loading the build in every
worker.
