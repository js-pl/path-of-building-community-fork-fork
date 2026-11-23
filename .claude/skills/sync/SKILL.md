---
name: sync
description: Bump the upstream PathOfBuilding submodule and port upstream's changes. Use only when the user explicitly asks to sync with upstream, now or at a later time ("sync", "sync with upstream", "wait 10 hours then sync"), or when a scheduled task or check-in asks for a sync. Never on your own initiative, e.g. because upstream looks outdated or a test fails.
argument-hint: "[upstream ref, default: origin/dev]"
---

# Sync with upstream

Bump `upstream/PathOfBuilding` to the given ref (default `origin/dev`) and
port what changed into the typed engine. Upstream itself, run headless on
LuaJIT (the oracle), is the only reference.

`sync/sync.py` does the mechanical parts and prints only what the next step
needs. Keep it that way: read the files this skill names when a step sends
you there, not before, and don't read what the tests compare for you.

If `crates/upstream` still exists, the translation has not been deleted yet
(sub-step 8 of `docs/drop-translation.md`): stop and say so rather than
syncing through it.

## 1. Triage

`sync/sync.py triage [ref]` fetches upstream and lists the commits since the
manifest's `upstream`, each with the files that may need a port, their
status and where they are ported. Commits with nothing to port are one line
(`[data only]`, `[todo only: ...]`). For each remaining commit,
`sync/sync.py show <sha>` gives its diff without the data files.

- `ported` / `partial`: port the behaviour where the port says (the typed
  engine is laid out freely, not by upstream's files).
- `todo`: not ported; list it in the PR, nothing to do.
- `new`: a file not in the manifest; see [manifest.md](manifest.md).
- Files the triage lists as ported in `web/` have no test against upstream:
  check those diffs by hand for new options, labels or controls.

Data (`src/Data`, `src/TreeData`) reaches the engine through the importer at
build time; the tests catch what it changes.

## 2. Bump and regenerate

1. `git -C upstream/PathOfBuilding checkout <new>` (the triage prints it).
2. `oracle/setup.sh` if `.oracle/` is missing (once per machine; `regen`
   reruns it when upstream's `Dockerfile` changes LuaJIT).
3. `sync/sync.py regen` regenerates `corpus/generated`, `tests/oracle`, the
   synthetic mod lines and `crates/engine/src/mod_parser/generated.rs`, in
   order (about 2 minutes). It prints a line per step and a diff summary; on a failure, the
   last lines of `target/sync/regen.log`.

Never open the regenerated files' diffs (megabytes of JSON; big diffs are
normal). They belong in the PR as they are, and are never edited by hand or
regenerated outside a bump.

## 3. Port

Port each commit from step 1. Then run the tests narrowly first, the ones
for what you ported, and the full set once at the end:

- `cargo test --release -p equivalence --test <name>`; `CALCS_FILTER=<id>`
  narrows to one build;
- `SESSION_PHASES=<phases> cargo test --release -p
  session --test typed`.

When a test fails and the cause isn't plain from its output, look the test
up in [failures.md](failures.md). If upstream's behaviour is a quirk the
port has to reproduce, record it in
`docs/progress.md#upstream-quirks-met-so-far`.

## 4. Record

`sync/sync.py record` sets the manifest's `upstream` and every entry's
`sync` to the new commit. Pass `--except <file> ...` for entries whose
upstream change you did not port, and say why in the PR.

## Done

`sync/check.py`, `cargo test --release` (on `oracle/setup.sh`'s LuaJIT, as
`.github/workflows/ci.yml`), `oracle/run.py check` and the wasm tests
pass. The PR lists the upstream commit range with links, what was ported
where, and the `todo` files left out. If something still fails after a few
attempts, open an issue with the upstream commit and the failing tests
instead of a PR.
