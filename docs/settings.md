# Settings

The app's own settings file, `crates/settings`. Upstream keeps its options
in `Settings.xml` in its user folder, edited through its Options dialog
(`main:OpenOptionsPopup`, `LoadSettings`, `SaveSettings` in
`Modules/Main.lua`). We keep ours apart so both apps can be installed side
by side without writing each other's file.

## The file

`settings.json` in `path-of-building-fork-fork/` in the user's config
directory:

| Platform | Directory |
| --- | --- |
| Windows | `%APPDATA%` |
| macOS | `~/Library/Application Support` |
| Linux and others | `$XDG_CONFIG_HOME`, else `~/.config` |

`POB_SETTINGS` names another file, for scripts, tests or a portable
install.

```json
{
  "history": {
    "enabled": true,
    "git": "",
    "gitConfig": ""
  }
}
```

- **A missing file** reads as the defaults; it is written on the first
  change.
- **Unknown keys are kept.** Sections and keys this version doesn't know
  survive a save, so an older and a newer version can share the file.
- **A broken file** (invalid JSON, a value of the wrong type) is an error
  that names it, and it is never overwritten.
- **Saving** goes through a file beside it, so a crash leaves the old file
  or the new one.

## The settings

| Key | Default | Meaning |
| --- | --- | --- |
| `history.enabled` | `true` | keep each build's versions in git ([history.md](history.md)) |
| `history.git` | empty | the git program, e.g. `C:\Program Files\Git\cmd\git.exe`; empty looks for `git` on the `PATH`, then for git for Windows where its installer puts it |
| `history.gitConfig` | empty | a gitconfig git reads instead of the user's global `~/.gitconfig` (`GIT_CONFIG_GLOBAL`, git 2.32 and later), e.g. for an identity of its own for the builds' commits; empty keeps the user's |

`POB_GIT` overrides `history.enabled` and `history.git` (a path, or `off`).

Turned on but unusable (a `git` that doesn't run, a `gitConfig` that isn't
there, no git found), the history is not kept, and the reason is shown
rather than swallowed.

## On the command line

```text
pob settings                    the file, every setting, and the git it gives
pob settings <key>              one setting
pob settings <key> <value>      sets it: true or false (on/off, yes/no, 1/0), a path, "" to clear
```

## The Options dialog

The web app will get an Options dialog, opened where upstream's Options
button is, with upstream's options ported over time and a Build history
section for the settings above. The browser can't start git, so that
section takes effect once the app runs on the host (`docs/agent.md` on
`claude/nice-ride-t0g45i`, step 6), which reads and writes this file; in
browser-only mode it is shown disabled.
