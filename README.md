# Path of Building Community Fork Fork

An unofficial rewrite of
[Path of Building Community](https://github.com/PathOfBuildingCommunity/PathOfBuilding)
for Path of Exile 1 as a web app: a Rust engine compiled to WebAssembly and a
TypeScript frontend.

The goals are the same numbers as upstream, enforced by equivalence tests
against upstream itself, and much faster recalcs and power calculations
(node, anoint, items, gems). Daily jobs port new upstream commits.

**Status:** phase 0, the test harness. Upstream runs as an oracle in CI; the
engine is not started yet. See [docs/design.md](docs/design.md).

## Disclaimer

This project is not affiliated with or endorsed by Grinding Gear Games or the
Path of Building Community team. Path of Exile is a trademark of Grinding Gear
Games. Game data and art are (c) Grinding Gear Games; they are not stored in this
repository but generated from the upstream submodule at build time.

## Layout

| Path | Purpose |
| --- | --- |
| `src/` | Rust engine (empty for now; becomes `crates/` next to a `web/` frontend, see the design) |
| `upstream/PathOfBuilding` | Upstream PoB Community, pinned to a release tag (git submodule) |
| `oracle/` | Runs upstream headless to produce expected outputs and coverage |
| `corpus/` | Builds the oracle runs (`corpus.toml`) and the build generator |
| `tests/oracle/` | Upstream's outputs per corpus build (generated, never edited) |
| `docs/design.md` | Plan, architecture and phases |
| `docs/repository-settings.md` | How `main` is protected |

## Getting started

```sh
git clone --recurse-submodules https://github.com/js-pl/path-of-building-community-fork-fork
cargo build
oracle/setup.sh && oracle/run.py check   # optional: run the upstream oracle
```

## License

MIT, see [LICENSE](LICENSE). Contains a port of code from Path of Building
Community, Copyright (c) 2016 David Gowor, also MIT.
