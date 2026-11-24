# Handoff: orchestrator, after the agent plan, for the parity pass

Written 2026-11-24 by the orchestrator session that ran steps 1 to 10 of
[agent.md](agent.md#steps). Branch: `handoff/orchestrator-parity-pass`.
Only this document is on it, so merge or delete the branch once you have
read it.

## How the work was run

- The orchestrator writes no code. It starts one child cloud session per
  step (`create_session`, tagged `agent-plan` and `agent-plan:step-N`).
  Each child works from the latest `main`, opens small PRs, and
  squash-merges them itself on green CI, following AGENTS.md. It then
  reports back with `send_message`.
- **Usage limits:** three children in parallel use up the 5-hour limit in
  about 15 minutes. Run one or two at a time. When a child stops on the
  limit, send it "continue" after the reset.
- **Force-pushes are refused** for children. They push under a new branch
  name and open a new PR.
- **Remote branches can't be deleted** from a cloud session (HTTP 403).
  The owner does it.
- **Oracle determinism:** LuaJIT seeds the `pairs` order per process. When
  a test is flaky against the oracle, sort both sides by a defined key
  (see #192 and #198). Never edit oracle outputs, the corpus or
  `tests/tolerances.toml`.

## State of the agent plan (docs/agent.md "Steps")

Done: steps 1 to 10 (step 9 as #212 jobs/front/dominating, #221 a
Windows test fix, #222 respec and keystone constraints), plus redo (#213) and "no
search changes the config" (#216). Step 7 landed as #217 (several open
builds) and #219 (`ui_context`, and a default grant fixed when the token
or session is created, as the owner decided).
Step 11 (trade and prices, cloud sessions, an agent in the browser, a
desktop app) is deferred and not to be started unless the owner asks.

Still in flight when this was written. Check with `list_sessions
children` from the old orchestrator, or `list_sessions` and the tag
`agent-plan`:

- The platform-agnostic local transport is finished (#218): `host::local`
  is the only per-OS code for the host, and CI runs the host and CLI
  tests on Windows.
- **Step 5 follow-ups** (`session_01W1dkjxba54CFtrTR4pqy83`), each its own
  PR:
  1. WSL mirrored networking mode in `pob agent`.
  2. PyPI (`pypi.org`, `files.pythonhosted.org`) in both default
     allowlists, with npm left off.
  3. The CRLF failure of gamedata/importer on Windows checkouts, then
     dropping CI's `core.autocrlf` workaround.

  Afterwards, remove the two answered questions from agent.md's
  "Questions for the owner".

The step 5 session stopped on the 5-hour usage limit at about 17:30 UTC
(reset 21:10 UTC), and had sent nothing since as of 22:30 UTC. It may
need a "continue".

If any of these is unmerged when you start, check its PR and nudge the
session, or take the work over.

## Owner decisions to keep

- **Config:** `best` and `improve` never change it. Config sets up a
  realistic environment, and any build can be made strong by config edits.
  An agent may still evaluate its own `set_config` variants with `eval`.
- **`improve`** proposes and doesn't apply. The owner is unsure of the
  design but wants to try it before dismissing it.
- **Gem levels** up to 21 are fine.
- **Redo** mirrors undo, whoever made the step.
- **Transport:** everything local is platform agnostic, so the host runs
  on Windows and Linux (Unix socket or named pipe, current user only).
- **`pob ctl` commits** are recorded as `agent ctl`.
- **The default container** supports WSL NAT and mirrored modes and
  allows PyPI.
- **Branch cleanup:** merged branches may be deleted (by the owner, see
  above).

## Next work, in order

1. **Import dropdown, matching upstream** (`ImportTab.lua:681`):
   - "Import to this build" commits a restorable version first. Upstream
     warns that it erases the build.
   - "Import to a new build".
   - "Import as comparison".

   Today, importing always creates a new build, which the owner called
   wrong.
2. **A written review of the git history** (`crates/history`,
   docs/history.md). Open a docs PR with proposals first, and implement
   after the owner answers. The owner thinks the spec was too light. It
   must cover:
   - **User experience**, in particular the owner's idea that loadouts
     (item, skill and tree sets) are git branches: "I feel like it could
     be nice to handle it that way". Work out what switching, comparing
     and merging loadouts would mean, and what it costs.
   - **The version shown as the commit id** when history is on, instead
     of the counter.
   - **Robustness:**
     - edits made outside PoB, two hosts on one builds directory, and
       a damaged repository;
     - git missing, and Windows paths;
     - the agent directories' keep and clean moves;
     - authentication for the backup push;
     - large histories;
     - coalescing racing an agent's step.
3. **The parity pass:** everything in docs/progress.md "Not ported yet":
   - **Calculations:** curse slot gaps, mirages and skills without a
     recalculator, party buffs, triggers outside the ported set, cell
     formatting.
   - **Tree and jewels:** timeless jewels without data, tattoos, crucible
     mods off the tree, tree links.
   - **Items:** enchant and anoint edge cases, items dragged without a
     base, deleted items in the list, the class change popup.
   - **Skills:** gem tooltips and copies, a main skill copied from a
     mirage, imbued supports.
   - **Session controls:** controls and popups not ported, drops outside a
     list, stale group and tree edits, party imports, loadouts not
     numbered 1 to n.
   - **Left by the translation:** the other tabs' `controls`, downloading
     a linked build, choosing a gem or imbued support by name
     (`GemSelectControl`), the gem slots' quality and enabled-box
     tooltips, the slot after a group's last gem, and Militant Faith with
     the devotion variant Any.
   - **Not in this pass:** trade URL, Trade for these items and Buy
     similar, which are network work deferred with step 11.

   Group the items by area into small steps, one session at a time or
   two. Expected values come from the oracle only. Record ports in
   `sync/manifest.toml`.

Not part of the oracle or GUI cleanup: no GUI Lua remains in the app.
`oracle/*.lua` and `corpus/generate.lua` are the test harness and stay.

## Loose ends

- **Remote branches the owner still has to delete:**
  ```bash
  git push origin --delete agent-front-jobs agent-respec agent-respec-2 windows-named-pipe windows-pipe agent-container pob-agent-run transport-agnostic agent-best agent-eval agents-install-2 drop-translation-delete
  ```
- **Older branches ahead of main with no merged PR head.** Investigate
  them before suggesting deletion:
  - claude/clever-euler-rt7mwc
  - claude/full-dps-threads
  - claude/node-power-converted
  - claude/sync-upstream
  - claude/tender-fermi-j9jen4
  - claude/typed-item-db
  - claude/typed-node-power
  - claude/zen-hypatia-ncd5gu
  - setup/project-skeleton
- **Open PR from before:**
  [js-pl/path-of-building-community-fork-fork#183](https://github.com/js-pl/path-of-building-community-fork-fork/pull/183)
  (claude/nice-ride-t0g45i). Check whether it is still wanted.
- **Not built in step 5:**
  - `pob agent --clean` and `pob agents clean`, which need an admin route
    on the control socket;
  - the Agents tab's mark for unsandboxed (`--no-container`)
    connections;
  - Docker support;
  - configurable container limits;
  - testing on real macOS and Windows machines.
- **Not built in step 7:** Calcs and Config selections in `ui_context`,
  hovered items or gems, and a flag for the cap on open builds.
- **Not built in steps 8 and 9:**
  - the anoint, unique, craft and rare generators, which need a `craft`
    (or anoint) verb so that each move is an `apply` change;
  - flags such as immunities as constraints;
  - repairs by crafting;
  - MCP progress notifications, since the HTTP endpoint has no SSE;
  - a UI view of jobs (the host requests exist).
- **Owner question from step 9:** should a `craft` verb, anoints first, be
  next, to unlock the item generators? Watch the timing asserts in
  `crates/agent/tests/front.rs` for flakes on slow runners.
- The old orchestrator's scheduled check-in
  (`trig_01GNVRTZ4u7HJeGUW6M1cNFw`) is deleted with this handoff.
