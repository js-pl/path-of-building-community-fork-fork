---
name: sync
description: Bump the upstream PathOfBuilding submodule and port upstream's changes. Use only when the user explicitly asks to sync with upstream, now or at a later time ("sync", "sync with upstream", "wait 10 hours then sync"), or when a scheduled task or check-in asks for a sync. Never on your own initiative, e.g. because upstream looks outdated or a test fails.
argument-hint: "[upstream ref, default: origin/dev]"
---

# Sync with upstream

Bump `upstream/PathOfBuilding` to the given ref (default `dev`) and port what
changed. See `docs/design.md#keeping-up-with-upstream`.

## Bumping

- The submodule is shallow: `git -C upstream/PathOfBuilding fetch --depth=200 origin dev`
  (deepen until the old commit is reachable, the range is needed for the log).
- Work commit by commit through `git log --reverse <old>..<new> -- src`, not
  one squashed diff; commits touching only `src/Data`, `src/TreeData` or
  `src/Export` need no port.
- If upstream's `Dockerfile` changed the LuaJIT commit, rerun `oracle/setup.sh`.
- Set `upstream` in `sync/manifest.toml` to the new commit and the `sync` of
  every `ported`/`partial` entry you checked; `sync/check.py --add` lists new files.

## Regenerating, in this order

1. `oracle/run.py dump`, which also writes `modparser.jsonl` and `data.jsonl`.
2. `cargo run --release -p importer --bin synthlines`; if
   `corpus/synthetic_mod_lines.txt` changed, run `dump` again (the oracle
   parses those lines too).
3. `oracle/run.py generate`, then `dump` again if `corpus/generated` changed.
   Upstream data changes reshuffle generated builds; big diffs there are normal.
4. `cargo run --release -p importer --bin portgen`.

Regenerated `tests/oracle`, `corpus/generated` and the synthetic lines belong in
the sync PR. They are never edited, and never regenerated to make a test pass
outside a bump.

## Where it breaks, and what to fix

- `crates/upstream` fails to build: luagen or the `lua` runtime lacks a
  construct or library function upstream started using. Fix those, never the
  generated code.
- `ModParser::new` reports a hash mismatch: upstream changed a closure ported
  by hand in `crates/engine/src/mod_parser/ported.rs`. Port the change, then
  update the hash.
- The importer rejects a data file: new embedded Lua function or unknown mod
  parser rule. Teach the importer; never skip it.
- `crates/session` tests fail: `session.lua` reaches upstream's controls by
  name (`controls.<name>`), and upstream renamed or moved one.
- Typed engine modules do not follow upstream's layout. Port what the commit
  changes in behaviour, using `sync/manifest.toml` to find where, and test
  against the oracle and the translation.
- Web UI ports (`partial` UI classes in the manifest) have no test against
  upstream: check each UI commit for new options, labels or controls.
- Any order dependence `oracle/run.py check` finds is a finding to report,
  not a tolerance.

## Done

`sync/check.py`, `cargo test --release`, `oracle/run.py check`, and the wasm
tests (see `.github/workflows/ci.yml`) pass. The PR lists the upstream commit
range with links, and what was ported where. If something still fails after a
few attempts, open an issue with the upstream diff and the failing tests
instead of a PR.
