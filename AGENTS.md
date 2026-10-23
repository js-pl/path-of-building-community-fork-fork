# Path of Building Community Fork Fork

Unofficial Rust rewrite of the Path of Building Community calc engine (PoE 1).
See `docs/design.md` before making architectural decisions.

## Project overview
- `src/` holds the Rust engine
- `upstream/PathOfBuilding` is upstream PoB Community as a submodule, pinned to a release tag; never edit it
- `docs/` holds design decisions

## Coding guidelines
- code and comments are written in english
- keep ported modules aligned with upstream file and function names, and note the upstream commit they are in sync with
- never commit GGG game data or art; read data from the submodule at build time

## Git workflows
- trunk based development
- each feature gets its own feature branch
- never push directly to main, work via PRs
- always squash merge PRs; only the owner merges
- be brief in commit messages
