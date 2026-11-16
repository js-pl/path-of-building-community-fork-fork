# An agent beside the build

Status: proposal. Nothing here is built yet. The prerequisite is sub-step 8
of [the deletion](drop-translation.md#step-4-in-sub-steps), which reshapes
`session::dispatch`.

## The question

PoB has a console panel. In it runs the user's own coding agent (Claude
Code, Codex, or any other terminal agent). The agent and the user work on
**the same live build**: the agent's changes show in the tree, items and
sidebar as it makes them, and the user's changes are visible to the agent.

The design is built around one question:

> **What does an agent need so that it answers questions about a build
> truthfully, changes the build the way the user asked, and finds
> improvements, tradeoffs and strictly better builds efficiently?**

Examples of what the user types in the console:

- "Why is my chaos resistance so low?" "What does my DPS assume about the
  enemy?"
- "Swap Elemental Damage with Attacks for Elemental Focus and tell me what
  changes."
- "Find me 10% more DPS without losing EHP." "Respec up to 8 points for
  more DPS, life and resistances must not drop."
- "Show me the tradeoffs between DPS and max hit for my last 5 points."
- "Is there anything that's strictly better than what I have?"

## Principles

1. **One build, two editors.** One session holds the build. The UI and the
   agent are both its clients. Neither has a copy that can drift.
2. **Truthful.** Every number comes from the engine, which the equivalence
   tests hold equal to upstream. Answers state what they assume (the
   config: enemy conditions, buffs, charges), say whether a number is
   exact or an estimate, and report `not ported` as such instead of a
   guess. A search says how much it searched and never claims a proven
   optimum it didn't prove.
3. **Efficient.** The engine does the search, not the agent's loop. One
   tool call evaluates hundreds of variants on all cores and returns a
   short table. The agent spends tokens on judgement, not on evaluating
   one node per call.
4. **The user stays in control.** The agent's changes are labelled,
   undoable steps. Large changes can be proposals the user previews in the
   UI and accepts.
5. **Harness-agnostic.** Nothing depends on one agent. The tools are a
   shell command every terminal agent can run, and an MCP server for the
   agents that prefer typed tools. Both come from one schema.

## Architecture

```text
 browser: the web app                         the console panel
   tabs, tree, items, sidebar                   xterm.js
        │ WebSocket: the engine API               │ PTY stream
        │ (the JSON commands the worker takes)    │
 ───────┴──────────────── pob host ───────────────┴───────────────  local process
   build sessions (the native typed engine, threads, warm caches)
   change log: versions, labelled undo steps, proposals, the UI's context
   agent workspace: AGENTS.md, CLAUDE.md, MCP configs; the harness in a PTY
        │ control socket
 ───────┴──────────────────────────────────────────────────────────
   pob ctl <verb>   ← the agent runs it in its shell (any harness)
   pob mcp          ← MCP over stdio, a client of the same socket
```

### `pob host`

A local process that owns the build:

- **Sessions.** The native typed engine (threads, mimalloc, warm calculator
  caches). It loads `core.bin`, which `pob bundle` makes locally from the
  pinned submodule (see [Game data](#game-data)).
- **The web app.** It serves the built web app on `127.0.0.1` and answers
  its engine requests over a WebSocket. The web app already reaches its
  engine through one function, `call(cmd, args)` in
  `web/src/engine/client.ts`; in hosted mode that call goes to the host
  instead of the worker. Same commands, same answers. Without a host the
  web app keeps running on its wasm workers, with no console.
- **The console.** It spawns the user's chosen harness (`claude`, `codex`,
  or a command of their own) in a PTY, in an agent workspace, and streams
  it to an xterm.js panel. The harness logs in with the user's own
  account; the host never touches its credentials.
- **The agent workspace.** A directory the host prepares per build: an
  `AGENTS.md` (read by Codex and others) and a `CLAUDE.md` importing it,
  with the tools and the truthfulness rules; `.mcp.json` and Codex's MCP
  config pointing at `pob mcp`; the build's files. `POB_HOST` in the
  harness's environment tells `pob ctl` where the control socket is.

Security: the host spawns shells, so a web page must never be able to
talk to it. It binds to `127.0.0.1` only, the web app connects with a
random token from the URL the host opens, it checks `Origin` and sends no
CORS headers. The control socket is a Unix socket (a named pipe on
Windows), readable only by the user. The harness runs with its own
permission system; the host doesn't widen it, and PoB's tools need no
network.

### Live sync

- **Versions.** Every change to the build, from the UI or the agent, bumps
  its version and goes into the change log. The UI subscribes and
  re-renders; the agent reads `changes_since(version)` to see what the user
  did since its last look.
- **Labelled undo.** An agent's change is one undo step labelled with what
  it did (`agent: allocate Heart of Thunder`). Ctrl+Z in the UI undoes it
  like any other change.
- **Conflicts.** An agent edit can carry the version it was planned on;
  if the user changed the build since, the host refuses it and the agent
  looks again. That's optimistic concurrency, which is enough for one
  user and one agent.
- **The UI's context.** `ui_context` tells the agent what the user is
  looking at: the tab, the selected or hovered node, item, gem or socket
  group, the Calcs tab's section. "Why is this node bad?" then needs no
  ids.
- **Proposals.** `propose(changes, note)` doesn't change the build. It
  shows the change in the UI as a diff: ghost nodes on the tree, items
  side by side, the sidebar's deltas. The user accepts or rejects it there.
  A setting decides whether the agent may apply changes directly or
  proposes them.

## The tools

All tools are verbs of `crates/agent`: typed requests and answers with a
JSON schema, no I/O, run in the host on the build's session. `pob ctl`
prints their answers as compact text (JSON with `--json`), and `pob mcp`
exposes the same verbs as MCP tools.

### Reading and explaining

| Verb | Answer |
| --- | --- |
| `summary` | the sidebar, grouped (offence, defence, resources), and the warnings (uncapped resistances, unmet requirements, reservation over 100%) |
| `explain(stat)` | the stat's breakdown, as the Calcs tab shows it: sources, more and increased multipliers, conversions |
| `assumptions(stat?)` | the config the numbers rest on; for one stat, which config options change it and by how much (each toggled and evaluated exactly) |
| `tree`, `skills`, `items`, `config` | the state, compact, with names |
| `search(kind, query)` | nodes (by name or stat text), gems, uniques, mods for a slot, bases: the agent never guesses ids |
| `ui_context`, `changes_since(v)` | see [Live sync](#live-sync) |

### Changing

Each edit resolves names, is idempotent, and returns a `StatDiff`:
`allocate(nodes)` and `deallocate(nodes)` (with the path and the points
spent), `set_mastery`, `set_ascendancy`, `set_gem`, `add_group`,
`set_main_skill`, `add_item(text)` (as copied in game or from the trade
site), `equip`, `craft` (bench mods, anoints, eldritch implicits, enchants,
corruptions), `set_config`, `select_set`, `undo`. `propose(changes)` wraps
any of them. Each verb maps onto the session's commands (`nodePath`,
`clickNode`, `setGem`, `addItem`, `craftItem`, `control`, ...), so the
agent's edits run the same code as the UI's, tested against upstream.

### Evaluating

The primitive every comparison is built on:

> `eval(variants, stats) -> table`: each variant a list of changes, each
> evaluated exactly on a copy of the build, all in parallel; the answer
> one row per variant with the stats asked for, their deltas, and the
> warnings each variant adds.

A variant that fits a calculator override (nodes added or removed, an
item replaced, a flask toggled) costs one misc calculator evaluation,
about 2 ms natively ([progress.md](progress.md), the bench). Others (gem
swaps, config changes) run on a copy of the session. `evaluate_item(text,
slot?)` is the common case of one: a pasted item, in your build.

### Improving

**Objectives** are a vector of stats, each to maximise or minimise: Full
DPS, EHP, the max hit per damage type, life, movement speed, and so on.
Presets give common sets; any power stat can be one.

**Constraints** are hard: resistances at their caps, requirements met,
reservation within 100%, minimums (life, hit chance, suppression), flags
to keep (an ailment immunity, a keystone).

**Costs** are objectives too, to minimise: passive points, points
respecced, crafts, purchases (with prices only if the user gives them).
That puts "+12% DPS for nothing" and "+20% DPS for a 6-point respec" on
the same front.

**Moves** are the atomic changes the search combines, from generators
over what the engine can already rank:

| Generator | Moves | Engine pieces |
| --- | --- | --- |
| tree | add a path (node power at depth 1 to n), remove a node, swap a node for another at the same points, mastery effects | `engine::power` |
| gems | support swaps per group, gem levels and qualities, awakened and alternate quality versions | `compare::gem_power` |
| anoints | every anoint | `compare::anoint_power` |
| uniques | every unique per slot, and jewels | `compare` item power (`ItemDBControl`) |
| crafts | bench mods, eldritch implicits, enchants on the items you have | `item_add_modifier`, `engine::item_craft` |
| config | pantheon, bandit, flask choice | config |
| rares | what each stat is worth per slot (the mod weights below); pasted items | the port of `GenerateModWeights` |

The verbs:

- **`dominating(objectives, constraints, budget)`**: changes that are
  better in at least one objective and worse in none, within the budget.
  That's the answer to "is there anything strictly better?". Every claim is
  checked by an exact evaluation of the final build, not added up from
  single moves.
- **`front(objectives, constraints, budget)`**: the Pareto front, the set
  of builds where no objective can improve without another getting worse,
  as a short table of tradeoffs with each entry's changes.
- **`improve(objective, constraints, budget)`**: the best change for one
  weighted objective (upstream's `WeightedRatioOutputs`, so the scores
  match upstream's).
- **`best(generator, stat)`**: one generator's ranking (best nodes, gems,
  anoints, uniques), for questions about one kind of change.

**The search.** Single moves are evaluated exhaustively. Combinations are
searched: a beam search over moves, and for the tree a local search at a
fixed number of points (swap the weakest allocated node for the best path,
while the score improves), seeded by node power. Every candidate on a
front is evaluated exactly, constraints included. A candidate that breaks
a constraint is first **repaired** with the cheapest fix (a resistance
craft, a node) before it's dropped. Answers report the search's effort
(moves, combinations, evaluations, time) and call their result "the best
found", because combinatorial search can't promise the optimum. Searches
are deterministic: the same build gives the same answer with any thread
count.

**Jobs.** Searches that take more than a few seconds run as jobs: the
call returns a job id, progress shows in the UI and through MCP progress
notifications, and the user can cancel from the UI. Each search has a time
budget the agent passes.

### The mod weights (a port, in `engine`)

`GenerateModWeights` and `GeneratePassiveNodeWeights` of upstream's
`TradeQueryGenerator`: for each mod that can roll on a slot
(`Data/QueryMods.lua`, read by the importer), a mid-roll value on a test
item through the misc calculator, and the gain per point. That's how the
agent tells a user which stats their boots want and what each is worth.
**Tested against the oracle**: for corpus builds, slots and stat weights,
the weights equal upstream's. These weights are linear estimates, so
answers built on them say so, and a pasted item is always evaluated
exactly.

### Answers sized for an LLM

Every answer is short and self-describing: numbers with their deltas in
context (`Full DPS 2.41M → 2.77M (+14.9%)`), names instead of ids, `exact`
or `estimate`, the assumptions that matter, and `not ported: ...` where
the engine has no answer. Full outputs stay in the host; the agent asks
for more when it needs it.

### The workspace's instructions

The `AGENTS.md` the host writes holds the rules that make answers
truthful, for any harness: numbers only from the tools; state the config
assumptions; say when a number is an estimate; `try` or `eval` before
changing; never trade defence for DPS without saying so; propose large
changes; say what a search covered. Game knowledge the engine doesn't have
(which uniques enable a build, what a Watcher's Eye mod is usually worth)
goes there too, short. The project can ship and improve it like code.

## Game data

`core.bin` is made from GGG's game data: upstream exports it from the game
files and commits it in `src/Data` and `src/TreeData`, each file headed
"(c) Grinding Gear Games". We read the same files from the submodule at
build time.

**Decision: `core.bin` is never published** for the host. Whoever runs
`pob host` builds it with `pob bundle` from the pinned submodule, as the
tests and the web app's build do. GGG's developer docs and terms couldn't
be read from this environment. Secondary sources quote them as supporting
no access to in-game data outside GGG's APIs (the passive tree being the
stated exception) and forbidding modifying or adapting the game client or
its data without approval. Read conservatively, a new public copy of the
data is a step this project doesn't need to take. Hosting the web app will
need the same question answered for its own bundle; that stays open.

## Other ways in

- **Without the UI.** `pob host --headless` and `pob ctl` work in a plain
  terminal: any agent can work on a build file with no browser at all.
  This is also the first thing built.
- **From the phone.** A Claude Code session in the cloud can run
  `pob host --headless` in its container, and later serve the web app on
  a preview port.
- **A desktop shell.** Tauri or similar can later wrap the host and the
  web app into one window. [design.md](design.md#goal) keeps a native
  desktop app out of scope for now, and the host doesn't need one.

## Deferred: trade and prices

Not built until decided otherwise: trade search with upstream's query
(`TradeQueryGenerator`'s `FinishQuery`) and a port of its rate limiter,
real listings evaluated exactly, prices from the currency exchange and
poe.ninja, build links, character import. That would live in its own
crate, the only one with network access, and would follow the rules in
[design.md](design.md#prior-art-pobcool): never a `POESESSID`, an honest
`User-Agent`, upstream's rate limits, no automated buying. Before it
starts: read GGG's developer docs and terms from the source.

## Testing

- **Verbs:** each against the session commands it runs; their numbers are
  already the engine's, which the equivalence tests cover.
- **The mod weights:** against upstream through the oracle.
- **The host:** a UI client and a `pob ctl` client on one host see the
  same versions and the same answers; an agent edit planned on an old
  version is refused; undo restores the build byte for byte (`saveXml`).
- **Search:** on small move sets, `front` and `dominating` equal an
  exhaustive search; every dominance claim holds in an exact evaluation;
  the same answers with any thread count.
- **End to end:** Playwright drives the web app on a host while a script
  runs `pob ctl`; the agent's edit shows in the tree and the sidebar, and
  a proposal shows its diff.

## Steps

Each one a PR, as usual:

1. Sub-step 8 of the deletion (the prerequisite).
2. `crates/agent` with the reading and changing verbs, `pob host
   --headless` and `pob ctl`, and the workspace's `AGENTS.md`: Claude Code
   or Codex in a plain terminal can work on a build.
3. The web app on the host: the WebSocket transport in `client.ts`, live
   updates from the change log, labelled undo.
4. The console panel: the PTY, xterm.js, the harness picker, the
   workspace files.
5. `ui_context` and proposals with their diff in the UI.
6. `eval`, `assumptions`, objectives, constraints, the generators,
   `best` and `improve`.
7. `dominating` and `front` with the combination and respec search; jobs
   with progress and cancelling.
8. The port of the mod weights with its oracle test; `evaluate_item`.
9. `pob mcp`, generated from the verbs' schemas.
10. Later: trade and prices, the cloud preview, a desktop shell.

## Questions for the owner

- **The host.** It adds a mode in which the web app runs on a native engine
  in a local process. That's not the desktop app design.md rules out, but
  it is a second way to run the app. Should design.md say so?
- **Default for agent changes.** Applied directly with labelled undo, or
  proposed for the user to accept.
- **The default harness** in the picker, and whether to offer only known
  harnesses or any command.
- **Where builds live** in the workspace: the host's own directory per
  build, or a folder the user picks (and versions with git if they like).
