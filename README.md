# Path of Building Community Fork Fork

An unofficial rewrite of
[Path of Building Community](https://github.com/PathOfBuildingCommunity/PathOfBuilding)
for Path of Exile 1 as a web app: a Rust engine compiled to WebAssembly and a
TypeScript frontend.

The goals are the same numbers as upstream, enforced by equivalence tests
against upstream itself, and much faster recalcs and power calculations
(node, anoint, items, gems). Daily jobs port new upstream commits.

**Status:** phases 2 to 4 done and much of 5: the typed Rust engine
calculates every corpus build exactly as upstream does, natively and in the
browser, and the web app has the tree, skills, items, calcs, config,
notes and import/export. See [docs/design.md](docs/design.md) for what is open.

## Disclaimer

This project is not affiliated with or endorsed by Grinding Gear Games or the
Path of Building Community team. Path of Exile is a trademark of Grinding Gear
Games. Game data and art are (c) Grinding Gear Games; they are not stored in this
repository but generated from the upstream submodule at build time.

## Layout

| Path | Purpose |
| --- | --- |
| `crates/` | Rust: `engine`, `importer`, `lua` (Lua values, patterns, formatting), `luai` (the Lua interpreter the importer runs), `session` (engine API), `wasm` (the engine for the browser), `cli` (`pob`, native runner), `history` (builds' versions in git), `settings` (the app's settings file), `equivalence` (oracle tests) |
| `web/` | The web app (TypeScript, Preact, Vite) |
| `upstream/PathOfBuilding` | Upstream PoB Community, pinned to a commit on `dev` (git submodule) |
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
npm run build:wasm   # public/engine.wasm, the typed engine
npm run build:data   # public/data, generated from the submodule, never committed
npm run build:art    # public/art, the tree's sprite sheets from the submodule, never committed (optional)
npm run dev
npm test             # the wasm engine against the native one, `pob serve` (cargo build --release -p cli first)
```

The web app on `pob host`, with the builds and their history on disk and
agents beside you (see [docs/agent.md](docs/agent.md)):

```sh
(cd web && npm install && npm run build)   # web/dist; build:art first for the tree's art
cargo run --release -p cli -- host          # serves it on 127.0.0.1:8730 and opens the browser
target/release/pob agents install --build <build>   # Claude Code or Codex beside you
```

The build's name in the top bar picks another of your builds; the link
beside it opens the build in a page of its own, so several can be open.

## License

MIT, see [LICENSE](LICENSE). Contains a port of code from Path of Building
Community, Copyright (c) 2016 David Gowor, also MIT.
