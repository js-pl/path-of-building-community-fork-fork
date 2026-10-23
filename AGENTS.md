# Path of Building Community Fork Fork

Unofficial rewrite of Path of Building Community (PoE 1): Rust engine compiled to
WebAssembly plus a TypeScript web frontend.
See `docs/design.md` before making architectural decisions.

## Project overview
- `src/` holds the Rust engine
- `upstream/PathOfBuilding` is upstream PoB Community as a submodule, pinned to a release tag; never edit it
- `docs/` holds design decisions
- `oracle/` runs upstream headless as the test oracle; `tests/oracle` is its
  generated output, see `oracle/README.md`

## Coding guidelines
- code and comments are written in english
- keep ported modules aligned with upstream file and function names, and note the upstream commit they are in sync with
- never commit GGG game data or art; read data from the submodule at build time
- expected test values come from the upstream oracle only; never edit oracle
  outputs, the corpus or `tests/tolerances.toml` to make a test pass

## Git workflows
- trunk based development
- each feature gets its own feature branch
- never push directly to main, work via PRs
- always squash merge PRs; only the owner merges
- be brief in commit messages
