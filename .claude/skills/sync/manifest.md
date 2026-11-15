# The manifest's entries

`sync/manifest.toml` lists every Lua file under upstream's `src/`:

- `ported` / `partial`: `port` lists where (paths in this repository), and
  `sync` the upstream commit the port is in sync with; `note` says what a
  partial port leaves out.
- `data`: read by the importer or `gamedata` at build time.
- `todo`: not ported; the session answers `not ported` for it.
- `skip`: deliberately not ported, with a `reason` (export scripts,
  desktop-only parts, art). Check the reason still holds when the file
  changes.

New upstream files: `sync/check.py --add` appends them as `todo`. Then give
each its real status: data an importer reads is `data`; code the change
needs is ported, then `partial` or `ported` with its `port`; the rest stays
`todo` or becomes `skip` with a reason. `sync/check.py` checks that ports
exist and that every ported, partial or data entry has a `sync` commit.
