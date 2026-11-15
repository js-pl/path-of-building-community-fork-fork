---
name: sync
description: Bump the upstream PathOfBuilding submodule and port upstream's changes. Use only when the user explicitly asks to sync with upstream, now or at a later time ("sync", "sync with upstream", "wait 10 hours then sync"), or when a scheduled task or check-in asks for a sync. Never on your own initiative, e.g. because upstream looks outdated or a test fails.
argument-hint: "[upstream ref, default: origin/dev]"
---

# Sync with upstream

Bump `upstream/PathOfBuilding` to the given ref (default `dev`) and port what
changed into the typed engine. Upstream itself, run headless on LuaJIT, is
the only reference: its committed outputs (`tests/oracle`) and its live
session (`oracle/session.lua`) say what the port must give. See
`docs/design.md#keeping-up-with-upstream`.

Until sub-step 8 of the deletion (`docs/drop-translation.md#step-4-in-sub-steps`)
`crates/upstream` still translates upstream's program in its build script;
the notes marked *until sub-step 8* below go with it.

## 1. Triage

- The submodule is shallow: `git -C upstream/PathOfBuilding fetch --depth=200 origin dev`
  (deepen until the pinned commit is reachable; the range is needed for the log).
- List the work: `git log --reverse --format='%h %s' <old>..FETCH_HEAD -- src`.
  Go commit by commit, not through one squashed diff.
- For each commit, `git show --name-only` and look every file up in
  `sync/manifest.toml`:
  - only `src/Data`, `src/TreeData` or `src/Export`: data, nothing to port
    (the importer and `gamedata` read it at build time);
  - `ported` or `partial`: port the behaviour where the entry's `port`
    says (the typed engine is laid out freely, not by upstream's files);
  - `todo`: not ported yet; note it in the PR, nothing to do;
  - `skip`: deliberately not ported (UI-only or desktop-only); check the
    reason still holds;
  - not in the manifest: a new file, see step 4.
- Commits to the UI classes (`*Tab.lua`, `*Control.lua`) that the web UI
  ports (`partial` with a `web/` port) have no test against upstream: check
  each for new options, labels or controls by hand.

## 2. Bump and regenerate, in this order

1. `git -C upstream/PathOfBuilding checkout <new>`.
2. If upstream's `Dockerfile` changed the LuaJIT commit, rerun `oracle/setup.sh`.
3. `oracle/run.py dump` (about 2 minutes on 3 cores; `--jobs N` goes before
   the command) writes `tests/oracle/<id>.json`, `modparser.jsonl` and
   `data.jsonl`.
4. `cargo run --release -p importer --bin synthlines`; if
   `corpus/synthetic_mod_lines.txt` changed, run `dump` again (the oracle
   parses those lines too).
5. `oracle/run.py generate`, then `dump` again if `corpus/generated` changed.
   Upstream data changes reshuffle generated builds; big diffs there are normal.
6. `cargo run --release -p importer --bin portgen` regenerates
   `crates/engine/src/mod_parser/generated.rs`.

On an unchanged submodule every step reproduces the committed files exactly;
if one does not, that is a finding to report before bumping. Regenerated
`tests/oracle`, `corpus/generated` and the synthetic lines belong in the sync
PR. They are never edited by hand, and never regenerated to make a test pass
outside a bump.

## 3. Port, and let the tests find the drift

What each test says when upstream moved:

| Fails | Means | Fix |
| --- | --- | --- |
| `importer` or `gamedata` build | a data file uses a new embedded Lua function or an unknown mod parser rule | teach the importer; never skip the file |
| `ModParser::new` reports a hash mismatch | upstream changed a closure ported by hand (`crates/engine/src/mod_parser/ported.rs`) | port the change, update the hash |
| `equivalence --test data` | a data table the importer reads changed shape | `oracle/run.py data-json` and `DATA_JSON_DIR` show the difference (see the test's header) |
| `--test mod_parser`, `--test mod_cache` | the mod parser's rules or `ModCache.lua` changed | port the rule; `portgen` covers the generated closures |
| `--test game_data`, `--test item`, `--test tree` | the typed data, items or trees differ from upstream's tables (`oracleValue`) | port the data processing (`crates/gamedata`, `engine::data`, `item`, `tree`) |
| `--test build_output` | a corpus build's outputs differ from `tests/oracle` | port the calc change (`engine::calc_*`), located through the manifest |
| `--test oracle_variants`, `--test calculator` | variant builds or calculator overrides differ from the live oracle | as above; `CALCS_FILTER=<id>` narrows the run |
| the session test (`SESSION_REFERENCE=oracle`) | a command, view or edit answers differently from upstream's | port the tab or control change (`crates/session/src/typed`); if the oracle itself fails, `crates/session/lua/session.lua` reaches a control upstream renamed or moved (`controls.<name>`) |
| `oracle/run.py check` | upstream's outputs depend on the process (string-hash order) | a finding to report, never a tolerance |
| `sync/check.py` | a file under `src/` is missing from the manifest | `sync/check.py --add` lists it as `todo`; give it its real status |

Run the oracle tests with `oracle/setup.sh`'s LuaJIT installed;
`SESSION_REFERENCE=oracle` makes the session test use it (CI does).

*Until sub-step 8:* `crates/upstream` translates upstream's program in its
build script, so an upstream construct `luagen` or the `lua` runtime lacks
breaks the build: fix those, never the generated code. Locally the session
test defaults to the translation; set `SESSION_REFERENCE=oracle` to test
against upstream itself.

## 4. Record

- Set `upstream` in `sync/manifest.toml` to the new commit, and the `sync`
  of every `ported`/`partial` entry you checked.
- New upstream files: `sync/check.py --add`, then the real status and port.
- Document upstream quirks the port has to reproduce (`docs/progress.md`,
  "Upstream quirks").

## Done

`sync/check.py`, `cargo test --release` (with the oracle tests:
`SESSION_REFERENCE=oracle` and LuaJIT, see `.github/workflows/ci.yml`),
`oracle/run.py check` and the wasm tests pass. The PR lists the upstream
commit range with links, and what was ported where. If something still
fails after a few attempts, open an issue with the upstream diff and the
failing tests instead of a PR.
