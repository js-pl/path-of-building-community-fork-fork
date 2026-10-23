# Path of Building Community Fork Fork

An unofficial Rust rewrite of the calculation engine of
[Path of Building Community](https://github.com/PathOfBuildingCommunity/PathOfBuilding)
for Path of Exile 1.

The goal is fast calcs for endgame characters, especially node power and
anoint power, while staying as data-driven as upstream so new PoB releases can
be picked up with little work.

**Status:** planning. Nothing is implemented yet. See [docs/design.md](docs/design.md).

## Disclaimer

This project is not affiliated with or endorsed by Grinding Gear Games or the
Path of Building Community team. Path of Exile is a trademark of Grinding Gear
Games. Game data is (c) Grinding Gear Games; it is not stored in this
repository but read from the upstream submodule.

## Layout

| Path | Purpose |
| --- | --- |
| `src/` | Rust engine (empty for now) |
| `upstream/PathOfBuilding` | Upstream PoB Community, pinned to a release tag (git submodule) |
| `docs/design.md` | Plan, architecture and phases |
| `docs/repository-settings.md` | How `main` is protected |

## Getting started

```sh
git clone --recurse-submodules https://github.com/js-pl/path-of-building-community-fork-fork
cargo build
```

## License

MIT, see [LICENSE](LICENSE). Contains a port of code from Path of Building
Community, Copyright (c) 2016 David Gowor, also MIT.
