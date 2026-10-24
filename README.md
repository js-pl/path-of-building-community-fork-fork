# Path of Building Community Fork Fork

An unofficial rewrite of
[Path of Building Community](https://github.com/PathOfBuildingCommunity/PathOfBuilding)
for Path of Exile 1 as a web app: a Rust engine compiled to WebAssembly and a
TypeScript frontend.

The goals are the same numbers as upstream, enforced by equivalence tests
against upstream itself, and much faster recalcs and power calculations
(node, anoint, items, gems). Daily jobs port new upstream commits.

**Status:** phases 2 to 4 done and much of 5: upstream's program, translated
to Rust, calculates every corpus build exactly as upstream does, natively and
in the browser, and the web app has the tree, skills, items, calcs, config,
notes and import/export. See [docs/design.md](docs/design.md) for what is open.

## Disclaimer

This project is not affiliated with or endorsed by Grinding Gear Games or the
Path of Building Community team. Path of Exile is a trademark of Grinding Gear
Games. Game data and art are (c) Grinding Gear Games; they are not stored in this
repository but generated from the upstream submodule at build time.

## Layout

| Path | Purpose |
| --- | --- |
| `crates/` | Rust: `engine`, `importer`, `lua` (Lua semantics, parser and runtime), `upstream` (upstream's program translated to Rust), `session` (engine API), `wasm` (the engine for the browser), `cli` (`pob`, native runner), `equivalence` (oracle tests) |
| `web/` | The web app (TypeScript, Preact, Vite) |
| `upstream/PathOfBuilding` | Upstream PoB Community, pinned to a release tag (git submodule) |
| `oracle/` | Runs upstream headless to produce expected outputs and coverage |
| `corpus/` | Builds the oracle runs (`corpus.toml`) and the build generator |
| `tests/oracle/` | Upstream's outputs per corpus build, mod line and data table (generated, never edited) |
| `sync/manifest.toml` | Every upstream file, where it is ported and the commit it is in sync with |
| `docs/design.md` | Plan, architecture and phases |
| `docs/repository-settings.md` | How `main` is protected |

## Getting started

```sh
git clone --recurse-submodules https://github.com/js-pl/path-of-building-community-fork-fork
cargo build
oracle/setup.sh && oracle/run.py check   # optional: run the upstream oracle
```

The web app (needs `rustup target add wasm32-unknown-unknown` and Node 22):

```sh
cd web
npm install
npm run build:wasm   # public/engine.wasm
npm run build:data   # public/data, generated from the submodule, never committed
npm run dev
npm test             # the wasm engine against the native one (cargo build --release -p cli first)
```

## License

MIT, see [LICENSE](LICENSE). Contains a port of code from Path of Building
Community, Copyright (c) 2016 David Gowor, also MIT.
