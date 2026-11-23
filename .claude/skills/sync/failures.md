# What a failing test means after a bump

| Fails | Means | Fix |
| --- | --- | --- |
| `importer` or `gamedata` build | a data file uses a new embedded Lua function or an unknown mod parser rule | teach the importer; never skip the file |
| `ModParser::new` reports a hash mismatch | upstream changed a closure ported by hand (`crates/engine/src/mod_parser/ported.rs`) | port the change, update the hash |
| `equivalence --test data` | a data table the importer reads changed shape | `oracle/run.py data-json` and `DATA_JSON_DIR` show the difference (see the test's header) |
| `--test mod_parser`, `--test mod_cache` | the mod parser's rules or `ModCache.lua` changed | port the rule; `portgen` (run by `sync/sync.py regen`) covers the generated closures |
| `--test game_data`, `--test item`, `--test tree` | the typed data, items or trees differ from upstream's tables (`oracleValue`) | port the data processing (`crates/gamedata`, `engine::data`, `item`, `tree`) |
| `--test build_output` | a corpus build's outputs differ from `tests/oracle` | port the calc change (`engine::calc_*`), located through the manifest |
| `--test oracle_variants`, `--test calculator` | variant builds or calculator overrides differ from the live oracle | as above; `CALCS_FILTER=<id>` narrows the run |
| the session test | a command, view or edit answers differently from upstream's | port the tab or control change (`crates/session/src/typed`); if the oracle itself fails, `oracle/api.lua` reaches a control upstream renamed or moved (`controls.<name>`) |
| `oracle/run.py check` | upstream's outputs depend on the process (string-hash order) | a finding to report, never a tolerance |
| `sync/check.py` | a file under `src/` is missing from the manifest | see [manifest.md](manifest.md) |

On an unchanged submodule `sync/sync.py regen` reproduces the committed
files exactly; if it does not, that is a finding to report before bumping.
