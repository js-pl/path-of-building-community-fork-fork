# Oracle

Runs upstream at the pinned submodule commit headless on LuaJIT and records its
results. Everything the port is tested against comes from here, see
[Equivalence testing](../docs/design.md#equivalence-testing).

## Usage

```sh
git submodule update --init
oracle/setup.sh            # LuaJIT + rocks into .oracle/ (needs luarocks, make, cc)
oracle/run.py dump         # write tests/oracle/<id>.json for every corpus build
oracle/run.py check        # what CI runs: dump twice, compare with tests/oracle
oracle/run.py coverage     # line coverage, ~25 min on 4 cores, see below
oracle/run.py generate     # regenerate corpus/generated, see corpus/corpus.toml
```

Every command splits the corpus over one LuaJIT process per CPU (`--jobs N`).

Only run `generate` and `dump` when the submodule, the corpus, the generator
or the dumper changed, and commit the result in the same PR. Never edit `tests/oracle` by hand; CI fails
if the files differ from what upstream produces.

## Files

| File | Purpose |
| --- | --- |
| `setup.sh` | Builds LuaJIT at the commit upstream's `Dockerfile` names, installs `luautf8` and `luacov` |
| `headless.lua` | Starts upstream via `src/HeadlessWrapper.lua` and loads builds |
| `../corpus/generate.lua` | Generates corpus builds with upstream's build API |
| `dump.lua` | Writes calc outputs as JSON |
| `luacov.lua` | luacov config: `Modules/Calc*.lua` and `Modules/ModParser.lua` |
| `run.py` | Reads `corpus/corpus.toml` and runs the scripts above |

## Output format

One file per corpus build, `tests/oracle/<id>.json`:

```text
{
  "MAIN":  { "player": <player.output>, "minion": <minion.output, if any> },
  "CALCS": { "player": <player.output>, "minion": <minion.output, if any> }
}
```

`MAIN` and `CALCS` are `build.calcsTab.mainEnv` and `calcsEnv`, the two
`calcs.buildOutput` runs upstream does after every change. `player.output`
includes the Full DPS results (`SkillDPS`, `FullDPS`, `FullDotDPS`); its
`Minion` field is the same table as `minion.output` and is left out.

Encoding:

- Finite numbers are written with `%.17g`, which round-trips every double, so
  values compare bitwise.
- Non-finite numbers are `{"$num": "inf"}`, `{"$num": "-inf"}` and
  `{"$num": "nan"}`.
- Lua tables with keys `1..n` are arrays, all others objects with sorted keys.
  Number keys become strings. An empty table is `{}`.
- `sourceGem` and `sourceItem` (in `ReqStrItem` and friends) point at game
  objects; they are written as `{"$gem": nameSpec}` and `{"$item": name}`, the
  names upstream shows for them.
- Anything else that is not plain data (functions, cycles, class instances)
  makes the dumper fail, so new kinds of output cannot slip through unnoticed.

## How upstream is run

- **No ModCache.** Upstream skips `Data/ModCache.lua` when `CI` is set, so every
  mod line goes through `ModParser`, which is what the port has to match and
  what coverage has to see. Upstream's own CI checks that the ModCache equals
  live parsing.
- **Build import.** `.xml` builds go through `loadBuildFromXML`. `.json`
  character files go through the same `ImportTab` calls as upstream's
  `TestImport_spec`; `HeadlessWrapper`'s own `loadBuildFromJSON` does not
  handle the API's nesting under `character`.
- **Build codes** cannot be loaded headless (`Inflate` is a stub), so the corpus
  stores XML.

## Generated builds

`corpus/generate.lua` builds characters through upstream's own API (class and
ascendancy selection, `AllocNode`, socket groups, `ItemsTab:AddItem` with
uniques and rare templates from upstream's item databases, config inputs) and
saves them with upstream's `Save` functions, so every generated build is one
upstream can load. All randomness comes from a seeded xorshift PRNG over
sorted lists, and saving swaps in a sorted `pairs()` because upstream writes
XML attributes, config entries and item text in hash order. CI regenerates
the corpus and fails if it differs from `corpus/generated`.

## Determinism

LuaJIT seeds its string hash from the OS on every start, so `pairs()` visits
keys in a different order in each process. Upstream sums modifiers in
`pairs()` order in places, which could make results differ in the last bits.
Measured at upstream `v2.67.2` on the initial corpus:

- outputs are byte-identical across separate processes (5 runs),
- byte-identical whether a build is loaded alone or after other builds, in any
  order (`check` splits the corpus over a different number of processes in
  each of its two runs),
- byte-identical under the coverage hook, where LuaJIT mostly interprets
  instead of running JIT-compiled traces.

`run.py check` repeats these checks on every CI run. If a build ever turns
out to be order dependent, that is a finding for the comparison rules, not
something to paper over in `tests/tolerances.toml`.

## Coverage

`run.py coverage` reruns the dumper under luacov, one stats file per process,
merged afterwards (the pure Lua hook: `cluacov`
is built against PUC Lua internals and crashes under LuaJIT), checks that the
outputs are unchanged and writes to `target/oracle-coverage/`:

- `summary.txt`: hits, misses and coverage per file,
- `uncovered.txt`: every line the corpus never runs, as `file:line: source`,
- `luacov.report.out`: the full annotated report.

CI publishes the summary on the run page and the files as an artifact.
