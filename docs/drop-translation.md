# Dropping the translation

Status: option C accepted; step 1 done (the browser build and `pob serve`
are typed-only by default), nothing removed yet. The goal in
[design.md](design.md#mirroring-upstream) is that `crates/upstream`, `luagen`
and the Lua runtime go, the oracle stays the only reference, and Lua is left
in the importer and the oracle. This note lists what still depends on them
and how the tests keep a reference without them.

## What links them today

| Code | Uses | Shipped? |
| --- | --- | --- |
| `crates/upstream` | upstream's whole program translated by `luagen` in its build script, plus the headless host | only through the features below |
| `crates/session`, feature `translation` (default; `wasm` turns it off) | `translated.rs`, `lua/session.lua` (translated too), the dispatcher's fallback and replay (`Dispatcher::fallbacks`, `pending`) | native CLI (`pob serve --typed`, `--translated`) and the wasm build with `--translation` |
| `crates/wasm`, feature `translation` (not default) | `session/translation` | `web/scripts/build-wasm.mjs --translation` (80 MB, 15 MB gzipped, against 8.2 MB, 2 MB typed) |
| `crates/cli` | `upstream`, `lua::gc` | the native CLI (`pob serve --typed`, `--translated`, `bundle --lua`, `bench`) |
| `crates/equivalence` | `upstream`; `bridge.rs` converts the translated program's stores into typed ones | tests only |
| `crates/importer::luagen` | the translator (1.9k lines), run by `crates/upstream/build.rs` | build only |
| `crates/lua` | interpreter (parser, `env`, `rt`, `gc`, 6.4k lines) **and** library pieces: Lua patterns (`pattern`), `tostring`/`%g` formatting (`format`, `value::format_number`), LuaJIT's `pairs` order (`ljtab`) | the library pieces are in the typed engine and ship |
| `crates/importer`, `crates/gamedata` | the interpreter, to evaluate upstream's data files and `ModParser.lua` at build time | build only |
| `crates/engine` | the library pieces everywhere; `engine::install`, `mod_store.rs`, `mods/convert.rs` and a `tree.rs` helper still touch `lua::env`/`lua::rt` (the replacements installed into the running translation, and conversions) | the library pieces |

The typed session no longer falls back for anything the session and
equivalence tests exercise (`KNOWN_FALLBACKS` holds only cases where upstream
itself fails or the translation carries another build's shared state), and CI
already lints `session` and `wasm` with `--no-default-features`. So shipping
without the translation needed no engine work: `build-wasm.mjs` builds the
typed engine unless given `--translation`, and `translation` is not a
default feature of `wasm`.

Dropping the Lua runtime entirely is a different matter: the engine needs
the library pieces (they are how it reproduces upstream's string patterns,
number formatting and table order), and the importer needs the interpreter
to read upstream's data. Only the parts that run upstream's *program* can go:
`crates/upstream`, `luagen`, `lua/session.lua`, `translated.rs`, `bridge.rs`,
`engine::install` and the `lua::gc` cycle collector (only the translation
makes cyclic environments).

## Where the tests get their reference

| Tests | Reference today |
| --- | --- |
| `equivalence/tests/build_output.rs`, `calcs.rs` (whole builds, `MAIN` and `CALCS`), `mod_parser.rs`, `data.rs` | the oracle's committed outputs (`tests/oracle`, 13 MB) |
| `equivalence/tests/{calc_setup,perform,breakdown,calculator,power,mods,item,items_tab,spec,tree,skills,config,game_data}.rs` | the translation, live: intermediate state (stores, setup slices, breakdowns, calculator calls, node power) compared slice by slice through `bridge.rs` |
| `session/tests/typed.rs` (12 phases: commands, geometry, calcs, tree, skills, save, items, config, tooltips, party, new, overlays) | the translation, live: random edit sequences whose next step is chosen from the previous answers (thousands of edits and views per run), plus fresh translated sessions to align upstream's shared state |
| `session/tests/session.rs`, `web/tests` | the translation (native against wasm) |

The live, adaptive tests are the hard part. Their sequences depend on the
answers, and some comparisons need a fresh upstream session on demand (the
shared-state alignments in `docs/progress.md`, "Upstream quirks").

## Options

### A. Keep the translation as a test-only crate

Ship without it; keep `crates/upstream`, `luagen` and the interpreter as
dev-dependencies of the tests.

- Keeps every test as it is, including the intermediate-state ones that make
  porting a new upstream feature tractable (the stores and setup slices show
  *where* a difference starts, the whole-build outputs only *that* there is one).
- CI: the translated crate is the slowest part of the build: about 10 to 15
  minutes and 9 GB of memory cold (`CARGO_BUILD_JOBS=2-3` on 15 GB machines),
  a 255 MB rlib; sccache makes warm builds about 2 minutes. The translation's
  test jobs (session 3 shards, perform 2, setup and breakdowns, items and
  config) take about 5 to 7 minutes each on 8 runners. Each translated session
  takes 1.6 GB and keeps about 12 MB per build loaded (hence sharded child
  processes).
- Disk: a few GB of `target` per profile.
- Sync: every upstream bump must translate. `luagen` fails loudly on Lua it
  cannot translate, so a sync can block on translator work unrelated to any
  feature the typed engine has. The translation also differs from LuaJIT in
  details the oracle does not see (string-key `pairs` order is insertion
  order, headless stubs), so it is a second reference to keep in line with the
  first.

### B. The oracle as the only reference

Retire the translation and get every expected value from upstream on LuaJIT.

1. **Session fixtures, recorded.** Run `lua/session.lua` on the oracle's
   LuaJIT (it needs only upstream's globals and `dkjson`, which
   `oracle/headless.lua` already loads through `HeadlessWrapper.lua`) behind a
   JSON-lines driver, and record transcripts: each request with its response.
   The random sequences are generated by the driver (or by the Rust test
   against a recording run), so the requests come from upstream's answers;
   the typed test replays the requests and compares the responses, failing at
   the first difference, which is where an adaptive sequence would diverge
   anyway.
   Size: whole-build outputs alone are 13 MB for 61 builds; transcripts with
   `calcs`, breakdowns, tree geometry and tooltips would be a few hundred MB
   uncompressed. Store them compressed (zstd) and keep the big views (calcs,
   geometry, breakdowns) as hashes plus a regeneration command, or store
   them in a release artifact or LFS instead of the repository.
2. **Or a live oracle session in CI.** The same driver, but the test talks to
   a LuaJIT process instead of reading fixtures: no fixtures to store, the
   sequences stay adaptive, fresh sessions are a new process. The oracle job
   already builds LuaJIT (`oracle/setup.sh`); the session test jobs would need
   it too, and local runs need `oracle/setup.sh`.
3. **Intermediate-state tests** cannot be reproduced from whole-build outputs.
   Either the dumper writes intermediates (setup slices, stores, breakdowns:
   much larger dumps and a dumper per slice), or these tests retire once the
   typed engine is the only implementation and the whole-build, variant and
   session comparisons cover them. They were porting scaffolding: once a slice
   matches, a regression in it shows up in the outputs.

Costs: no translator to maintain and no 10-minute, 9 GB crate; the session
test becomes as fast as LuaJIT plus the typed engine (LuaJIT is likely
faster than the translation; not measured yet). New work: the JSON-lines driver,
porting the test drivers to it, fixture storage or a LuaJIT in every test job,
and these reference differences, which today the translation hides:

- LuaJIT hashes strings with a random seed (`LUAJIT_SECURITY_STRHASH`) and
  tables by address, so `pairs` over string or table keys is not reproducible
  across runs. The typed engine copies the translation's insertion order in
  places (party labels, `otherEffects`, some sidebar lists). Build the oracle's
  LuaJIT with a fixed seed and teach the typed engine LuaJIT's string order
  (as `lua::ljtab` does for numbers), or canonicalize those outputs in the
  comparison.
- The headless stubs differ: `GetScreenSize`, `DrawStringWidth` and friends in
  `HeadlessWrapper.lua` against `crates/upstream/src/host.rs`. Layout-dependent
  views (the Party tab's control order, wrapped labels) follow the stubs.
- The shared-state alignments become a fresh LuaJIT session (live) or a
  recording made from a fresh session per build (fixtures).

Sync: each bump re-runs the recorder (minutes, like `oracle/run.py dump`)
and commits or uploads the transcripts; nothing has to translate.

### C. Both, in steps (recommended)

1. **Now: ship without the translation (done).** `build-wasm.mjs` and
   `pob serve` are typed-only unless given `--translation` (`--typed`,
   `--translated` for `pob serve`); `translation` is not a default feature of
   `wasm`. It stays a default of `session`, which is not shipped on its own
   (`wasm` turns its defaults off) and whose tests and CLI need it. Nothing is
   removed; the tests keep the translation (option A). Low risk: CI checks
   the typed-only build, and the session tests record every fallback.
2. **Next: an oracle session.** Write the JSON-lines driver for `session.lua`
   on LuaJIT, run it beside the translation, and compare the two on the
   existing sequences until they agree or every difference is explained
   (string-key order, stubs). That is the test that the oracle can replace the
   translation, made before anything depends on it.
3. **Then: move the session tests** to the live oracle session in CI (B.2),
   with a small recorded set (B.1) for running without LuaJIT locally. Retire
   the intermediate-state equivalence tests one by one where whole builds,
   variants and session sequences cover them; keep the oracle dumps for whole
   builds, the mod parser and data as they are.
4. **Last: delete** `crates/upstream`, `luagen`, `lua/session.lua`,
   `translated.rs`, `bridge.rs`, `engine::install`, `lua::gc`, and split
   `crates/lua` into the library pieces the engine ships and the interpreter
   the importer keeps.

Why this order: step 1 has the user-facing benefit (a 2 MB engine, no
translated code in the browser) at no testing cost; step 2 proves the new
reference against the old one before the old one goes; the expensive parts
(CI time, 9 GB builds, translator work on every sync) go only when nothing
needs them. Until step 4, every sync still has to translate, and an upstream
change the translator cannot handle blocks it; if that starts to cost more
than the steps above, step 3 is the one to bring forward.

Live (B.2) over recorded (B.1) as the main reference because the session
tests are adaptive and need fresh sessions on demand, and because transcripts
for all views would be large and would need regenerating on every sync
anyway; the recorded set is for offline runs and as a fallback when LuaJIT is
not available.
