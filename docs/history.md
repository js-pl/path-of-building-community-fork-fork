# Build history

Every version of a build, kept in git. This is step 1 of the agent design
([agent.md](agent.md), "Build history"):
an agent edits a build directly, so nothing it does may lose one, and the
history comes before any verb that changes a build. It is built as
`crates/history` and stands alone: XML in, versions out, no session.

## Decided

- **The installed git.** The history is written by running the git the
  user has installed (git for Windows' `git.exe` under Windows), not by a
  git library. The settings ([settings.md](settings.md)) pick the program,
  a gitconfig of its own, and whether to keep a history at all;
  `POB_GIT` overrides them (a path, or `off`). Under Windows, git for Windows is also found where its
  installer puts it when it isn't on the `PATH`, and git runs without a
  console window.
- **Optional.** Turned off, or without git, there is no history and
  nothing fails:
  `History::open` gives no repository, and a `Recorder` made from that
  records nothing, so callers have one code path.
- **One repository per build.** `<root>/<build>/` is an ordinary git
  repository with `build.xml` (the XML `saveXml` writes) on `main`. A build
  can be opened, copied, backed up or deleted on its own, with any git
  tool. The directory name is the build's name, made safe on every
  platform (`dir_name`).
- **Bytes kept.** Each command runs with `core.autocrlf=false` and the
  repository commits `.gitattributes` with `* -text`, so neither our
  commands nor the user's git tools change a build's line endings. Hooks,
  commit signing and fsmonitor are off for our commands, and variables a
  parent git leaves (`GIT_DIR`, ...) are cleared.
- **Identity.** Commits carry the user's git identity; when git has none,
  the repository gets `Path of Building <pob@localhost>` in its own config.
- **Commits.** The message's subject is the undo step's label, followed by
  trailers: `Source: user`, `Source: agent <connection>` or
  `Source: restore`, then the step's own (`Full DPS: +3.1%`). An unchanged
  build makes no commit.
- **When** (`Recorder`): an agent's step is committed before the call
  returns; the user's edits are coalesced, committed once 10 seconds pass
  without another, before any agent step, and on close or drop.
- **The history only grows.** Nothing resets, rebases or force-pushes.
  `restore` commits the old version on top, so a restore can be undone too.
- **Backup.** `push(remote)` pushes `main` to a URL, a path or a remote's
  name, never forced. Off unless the caller asks for it.

## On the command line

```text
pob history <dir> commit <build.xml> [label]
pob history <dir> log [n]
pob history <dir> show <version>
pob history <dir> restore <version>
pob history <dir> push <remote>
```

A version is anything git names a commit by: a hash or its prefix,
`HEAD~2`, a tag.

## Next

The host ([agent.md](agent.md) steps 3 and 6) holds a `Recorder` per open
build, feeds it the session's saves, calls `tick` from its event loop, and
shows `log` with a restore button per version.
