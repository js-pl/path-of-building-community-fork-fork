# An agent beside the build

Status: proposal. Nothing here is built yet. The prerequisite is sub-step 8
of [the deletion](drop-translation.md#step-4-in-sub-steps), which reshapes
`session::dispatch`.

## The question

The user has PoB open and a terminal next to it. In the terminal runs their
own coding agent (Claude Code, Codex, or any other agent that speaks MCP).
The agent finds PoB's tools on its own, with nothing to configure per
session. The agent and the user work on **the same live build**: the
agent's changes show in the tree, items and sidebar as it makes them, and
the user's changes are visible to the agent.

The design is built around one question:

> **What does an agent need so that it answers questions about a build
> truthfully, changes the build the way the user asked, and finds
> improvements, tradeoffs and strictly better builds efficiently?**

Examples of what the user types in the terminal:

- "Why is my chaos resistance so low?" "What does my DPS assume about the
  enemy?"
- "Swap Elemental Damage with Attacks for Elemental Focus and tell me what
  changes."
- "Find me 10% more DPS without losing EHP." "Respec up to 8 points for
  more DPS, life and resistances must not drop."
- "Show me the tradeoffs between DPS and max hit for my last 5 points."
- "Is there anything that's strictly better than what I have?"

## Principles

1. **One build, one owner.** Exactly one engine session holds a build. The
   UI and the agent are both its clients. Neither has a copy that can
   drift.
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
4. **Applied directly, undone easily, never lost.** The agent's changes
   take effect at once. Each tool call is one labelled undo step, and a
   commit in the build's git history before the tool answers. No tool
   deletes or replaces a build, and every version can be restored, also
   after a restart (see [Build history](#build-history)).
5. **The harness is the user's.** PoB builds no chat window and no
   terminal. The agent runs in whatever terminal the user likes and finds
   PoB through MCP. Nothing depends on one agent.

## What runs where

### The constraint that decides it

The agent is a native process on the user's machine (or in a cloud
container). It can spawn programs and open sockets; it can't reach into a
browser tab. A browser tab can't spawn programs or listen on a socket. So
an agent always needs a native piece of PoB to talk to, and the question is
where the build lives.

**Option A, the tab owns the build.** The engine stays in the browser's
workers. A native bridge relays the agent's tool calls to the tab.

- The page has to connect to a local port. From a hosted https site,
  browsers gate that (Chrome's local network access permission, Safari's
  stricter mixed-content rules), and it breaks differently per browser.
- The build dies with the tab, and the agent can't work while it's closed.
- Searches run in wasm: no shared-memory threads without cross-origin
  isolation ([design.md](design.md#architecture), "Workers"), no mimalloc,
  and they compete with the UI for the workers.

**Option B, a native host owns the build (chosen).** A local `pob host`
process holds the sessions on the native engine. The browser renders from
it, and the agent's tools run in it.

- The engine runs with threads and mimalloc, the fastest it gets
  ([progress.md](progress.md), the bench).
- The host serves the web app itself on `127.0.0.1`, so the page and the
  host share an origin: no browser gating, no CORS.
- The build lives in a file the host saves after every change. It
  survives closing the tab or the agent.
- A native piece is needed anyway (A needs its bridge), and the user who
  runs a coding agent has native tools already.

The same Rust code is the engine in both places, so the web app keeps its
browser-only mode for users without an agent. That doesn't change.

### The pieces

```text
 user's terminal                            browser
   claude / codex / ...                       the web app (Preact): tabs, tree, items
     │ MCP over stdio                           │ WebSocket: the engine API
     │ (the harness spawns it)                  │ + change events
   pob mcp ───── control socket ──────┐         │
   pob ctl ───── control socket ──────┤         │
                                      ▼         ▼
                      pob host (native, local, long-lived)
                        build sessions: the typed engine, threads, warm caches
                        change log: versions, labelled undo steps, sources
                        jobs: searches on the rayon pool, progress, cancelling
                        files: the builds on disk, core.bin
                        serves the web app's static files on 127.0.0.1
```

| Piece | Runs | Does |
| --- | --- | --- |
| the web app | browser | the UI. Its engine is the host over a WebSocket (agent mode) or wasm workers (browser-only mode), behind the one `call(cmd, args)` in `web/src/engine/client.ts` |
| `pob host` | native, long-lived | owns the builds; answers the engine API for the UI and the agent API for the agent; saves; runs jobs |
| `pob mcp` | native, spawned by the harness | an MCP server over stdio; finds the host, starts one if none runs, and passes the agent's tool calls to it |
| `pob ctl` | native, run by hand or by scripts | the same agent API on the command line |
| `crates/agent` | inside the host | the agent API's verbs; no I/O, so it also builds for wasm |
| `crates/session`, `crates/engine` | inside the host, or in the browser's workers | exist today |

### The interfaces

1. **The engine API** (exists): the session's JSON commands, shaped like
   upstream's controls. The UI speaks it, over `postMessage` to a worker
   or over the WebSocket to the host. Same commands, same answers.
2. **The agent API** (new): build-level verbs with JSON schemas (below),
   implemented by `crates/agent` on top of the session. The agent speaks
   it through `pob mcp` or `pob ctl`.
3. **Change events** (new): the host tells every client when a build
   changed: its new version, the undo step's label, and its source (the
   UI, an agent, a job). The UI re-renders on them; the agent reads them
   with `changes_since`.
4. **Discovery** (new): the host writes its control socket's path and a
   random token to a file only the user can read (under
   `$XDG_RUNTIME_DIR` on Linux, the user's app data elsewhere). `pob mcp`
   and `pob ctl` read it; the browser gets the token from the URL the host
   opens.

### How the agent finds PoB

One-time, `pob agents install` registers `pob mcp` with the harnesses it
finds, through each one's own mechanism (`claude mcp add --scope user`,
`codex mcp add`, or their config files). After that, any terminal anywhere
has PoB's tools. When the agent first calls one, `pob mcp` attaches to the
running host, or starts one in the background and prints the URL to open.

The rules for using the tools truthfully go to the model as the MCP server's
`instructions`, which harnesses pass on, so there is nothing to put in a
project's `AGENTS.md`. A builds folder can still carry one for game
knowledge the user wants the agent to have.

### Security

The host runs on the user's machine and edits their files, so a web page
must never be able to talk to it. It binds to `127.0.0.1` only, the
browser connects with the token from the URL, the WebSocket checks
`Origin`, and the host sends no CORS headers. The control socket is a Unix
socket (a named pipe on Windows) only the user can open. PoB's tools need
no network.

## Live sync

- **Versions.** Every change to a build, from the UI or the agent, bumps
  its version and goes into the change log.
- **Undo.** An agent tool call is one undo step, labelled with what it did
  and what it changed (`agent: allocate Heart of Thunder, Full DPS +3.1%`).
  A multi-step tool call (a respec) is still one step. The UI lists the
  steps with their source, and undo works on them like on the user's own.
- **Conflicts.** An agent edit can carry the version it was planned on. If
  the user changed the build since, the host refuses it and the agent looks
  again. That's optimistic concurrency, which is enough for one user and
  one agent.
- **The UI's context.** `ui_context` tells the agent what the user is
  looking at: the build, the tab, the selected or hovered node, item, gem
  or socket group. "Why is this node bad?" then needs no ids.
- **Several builds.** The host can hold several open builds. Tools take an
  optional build and default to the one the UI shows.

## Build history

An agent edits a build directly, so nothing it does may lose one. The
history is the first thing built, before any verb that changes a build.

**A git repository of builds.** The host keeps its builds in one git
repository in its own data directory, one file per build
(`<name>.xml`, the XML `saveXml` writes), on one branch. A fork is a new
file, not a branch. It's an ordinary repository the user can open with
any git tool. The host writes it with `gix` (pure Rust, so no git install
is needed), or with the git CLI if `gix` falls short.

**Commits.**

- **An agent step** is committed before its tool call answers, so a crash
  can never lose an agent's change. The message is the undo step's label,
  with trailers for its source and stat change (`Source: agent`,
  `Full DPS: +3.1%`).
- **The user's own edits** are coalesced: committed after 10 seconds
  without a change, before any agent step (so the agent's commits stay
  separate), and on close. A crash loses at most those seconds of the
  user's clicks.
- **The history only grows.** The host never resets, rebases or
  force-pushes. Restoring a version is a new commit on top, so a restore
  can be undone too.

**Undo and history.** The session's undo stack covers recent steps in
memory. The history covers everything, across restarts. Undo past what
memory holds restores from the history.

**Nothing replaces a build.** No verb deletes a build. Loading a build or
starting a new one opens a new file; it never overwrites the current one.
A build closed in the UI stays in the repository.

**Verbs.** `history(build, n)` (the last versions with their labels and
stat changes), `restore(build, version)`, and `eval` across versions
(`diff` of two versions is two variants). The UI shows the history with
a restore button per version.

**What git doesn't protect against.** The user's agent can run shell
commands, and with them it could delete files PoB can't protect. PoB
lowers the risk but doesn't remove it:

- the repository is in the host's data directory, outside the folder
  the agent works in, so a harness that sandboxes writes to its working
  folder (Claude Code's sandbox, Codex's workspace-write mode) can't
  touch it;
- the MCP server's instructions tell the agent never to touch PoB's
  files outside the tools;
- `pob host --backup <remote>` pushes the history to a remote the user
  chooses (a private GitHub repository, another disk), after every commit
  or on a timer, so a copy lives off the machine. That's the only real
  protection against a deleted directory, and the doc for setting up the
  host recommends it.

The harness's own permission prompts remain the user's main guard for
shell commands.

**Browser-only mode** has no host and no git. It keeps its builds as it
does today, and snapshots in IndexedDB can come later.

## The agent API

Verbs of `crates/agent`: typed requests and answers with a JSON schema,
run in the host on the build's session. `pob mcp` exposes each as an MCP
tool; `pob ctl` prints their answers as compact text (JSON with `--json`).

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
corruptions), `set_config`, `select_set`, `undo`. `apply(changes)` makes
several edits one undo step. Each verb maps onto the session's commands
(`nodePath`, `clickNode`, `setGem`, `addItem`, `craftItem`, `control`, ...),
so the agent's edits run the same code as the UI's, tested against
upstream.

### Evaluating

The primitive every comparison is built on:

> `eval(variants, stats) -> table`: each variant a list of changes, each
> evaluated exactly on a copy of the build, all in parallel; the answer
> one row per variant with the stats asked for, their deltas, and the
> warnings each variant adds. Nothing changes in the build.

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

The verbs (none changes the build; the agent applies what the user wants):

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

**Jobs.** Searches that take more than a few seconds run as jobs in the
host: the call returns a job id, progress shows in the UI and through MCP
progress notifications, and the user can cancel from the UI. Each search
has a time budget the agent passes. A job works on a copy of the build at
its version, and its answer says which version that was.

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

## Game data

`core.bin` is made from GGG's game data: upstream exports it from the game
files and commits it in `src/Data` and `src/TreeData`, each file headed
"(c) Grinding Gear Games", and its own updater downloads those files from
upstream's GitHub.

**Decision: `core.bin` is never published.** `pob host` makes it on the
user's machine with `pob bundle`, from upstream's files at the pinned
commit: the submodule in a checkout of this repository, or, for an
installed `pob`, upstream's files fetched from upstream's GitHub the way
upstream's updater fetches them. GGG's developer docs and terms couldn't be
read from this environment. Secondary sources quote them as supporting no
access to in-game data outside GGG's APIs (the passive tree being the
stated exception) and forbidding modifying or adapting the game client or
its data without approval. Read conservatively, a new public copy of the
data is a step this project doesn't need to take. Hosting the browser-only
web app will need the same question answered for its bundle; that stays
open.

## Other ways in

- **Without the UI.** `pob host --headless` with `pob mcp` or `pob ctl`:
  an agent works on a build file with no browser at all. This is the first
  thing built.
- **From the phone.** A Claude Code session in the cloud runs `pob host
  --headless` in its container, and later serves the web app on a preview
  port.
- **An agent in the browser, later.** `crates/agent` does no I/O, so the
  same verbs can run in the browser's workers for an agent that lives in
  the page.
- **A desktop shell, later.** Tauri or similar can wrap the host and the
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
- **The history:** every restored version equals the XML committed for it
  byte for byte; the history only grows (no commit ever disappears); an
  agent step is committed before its answer, which a test that kills the
  host right after the answer checks; no verb removes or overwrites a
  build file.
- **The host:** a UI client and an agent client on one host see the same
  versions and the same answers; an edit planned on an old version is
  refused; undoing an agent's step restores the build byte for byte
  (`saveXml`); a build reopened after the host restarts is the saved one.
- **Search:** on small move sets, `front` and `dominating` equal an
  exhaustive search; every dominance claim holds in an exact evaluation;
  the same answers with any thread count.
- **MCP:** a test client spawns `pob mcp`, which starts a host, and drives
  it through load, edits, `eval`, a search job and undo.
- **End to end:** Playwright drives the web app on a host while a script
  runs `pob ctl`; the agent's edit shows in the tree, the sidebar and the
  undo list.

## Steps

Each one a PR, as usual:

1. **Build history**, `crates/history`: the git repository of builds,
   commits with labels and trailers, coalescing, restore, the backup
   push. It stands alone (XML in, versions out) and doesn't depend on
   the session, so it can start now.
2. Sub-step 8 of the deletion (the prerequisite for the rest).
3. `crates/agent` with the reading and changing verbs, on the history
   from the first edit; `pob host --headless`, the control socket and
   discovery; `pob ctl`.
4. `pob mcp` and `pob agents install`: Claude Code or Codex in any
   terminal works on a build.
5. The web app on the host: the host serves it, the WebSocket transport in
   `client.ts`, change events, the undo list with sources, the history
   with restore.
6. `ui_context` and several open builds.
7. `eval`, `assumptions`, objectives, constraints, the generators, `best`
   and `improve`.
8. `dominating` and `front` with the combination and respec search; jobs
   with progress and cancelling.
9. The port of the mod weights with its oracle test; `evaluate_item`.
10. Later: trade and prices, the cloud preview, an agent in the browser, a
    desktop shell.

## Decided

- Agent changes are applied directly; each tool call is one labelled undo
  step and one commit in the build's git history.
- The build history comes first, before any verb that changes a build.
- The harness runs in the user's own terminal and finds PoB through MCP;
  PoB embeds no terminal and no chat.
- The build lives in a native host; the browser renders from it.
- `core.bin` is never published.
- Trade and prices are deferred.

## Questions for the owner

- **design.md.** The host adds a second way to run the app (the web app on
  a native engine in a local process). Should design.md's architecture say
  so once this is accepted?
- **The backup remote.** Recommended in the docs, or asked for when the
  host first starts.
