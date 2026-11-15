# MCP: an agent works on your build

Status: proposal. Nothing here is built yet. The prerequisite is sub-step 8
of [the deletion](drop-translation.md#step-4-in-sub-steps), which reshapes
`session::dispatch`.

## Goal

An LLM agent (Claude Code locally, or in the cloud from the Claude app on a
phone) works on a Path of Exile build with this project's engine through
the [Model Context Protocol](https://modelcontextprotocol.io). It can:

- read a build: stats, breakdowns, the tree, skills, items, config;
- edit it: allocate nodes, swap gems, equip items, set the config;
- try changes and compare them before committing to one;
- search for upgrades and rank them by impact across the tree, gems,
  anoints, uniques and crafts, and say which stats a rare should have;
- evaluate an item you paste (copied in game or from the trade site)
  exactly, as it would sit in your build;
- hand the build back as a build code.

The question the design is built around is **"what is the next most
impactful upgrade I should get?"**, asked with a build code. The answer is
a short ranked list, each entry with its exact stat change, what kind of
cost it has, and the step to do it.

**Local only.** The server makes no network calls: no trade site, no price
sources, no build sites. Trade search is deferred, see
[Deferred: trade and prices](#deferred-trade-and-prices). The layers keep
room for it.

### Why this engine fits

- **Speed.** A misc calculator evaluation (one override: a node, an item)
  takes about 2 ms natively, node power about 4 s with Full DPS off
  ([progress.md](progress.md), the bench). Evaluating every candidate for
  every slot in one tool call takes seconds on all cores.
- **Correct numbers.** Every number comes from the engine, which the
  equivalence tests hold equal to upstream. The agent reasons about the
  numbers; it does not make them up.
- **No I/O in the engine.** Nothing to audit, rate limit or switch off.

## Upstream already has the core idea

Upstream's Trade tab (`Classes/TradeQueryGenerator.lua`, with
`Data/QueryMods.lua`; `todo` in `sync/manifest.toml`) weighs a build's
stats:

1. It takes the user's stat weights (`statWeights`: Full DPS, EHP, life, ...)
   and scores an output against the base output with
   `WeightedRatioOutputs` (a weighted sum of ratios, so stats of different
   units mix).
2. For each mod that can roll on the slot's item category (`QueryMods.lua`),
   it puts a mid-roll value on a blank test item, runs the misc calculator
   with `repSlotName`/`repItem`, and keeps the gain per point as the mod's
   weight (`GenerateModWeights`). Anoints and passive nodes are weighed the
   same way (`GeneratePassiveNodeWeights`).

Upstream then turns the weights into a trade query. That last step is the
only part that needs the network. The weights alone answer "which stats do
my boots want, and what is each worth", which is half of a rare upgrade
locally.

## Layers

```text
 clients         Claude Code (local, or cloud from the phone) · Claude Desktop
                   │ MCP over stdio
 crates/mcp      `pob mcp`: tools, resources, prompts; build workspace; jobs
                   │
 crates/agent    build-level API for agents, no I/O, also builds for wasm:
                   │ handles, edits, transactions, diffs, search,
                   │ objectives, constraints, upgrade search
                   │
 crates/session  the engine API (exists): build sessions, JSON commands
 crates/engine   the typed engine (exists): calcs, misc calculator, power, compare
```

Rules:

- No crate does I/O except `mcp`, and `mcp` only reads and writes the
  workspace's files.
- `agent` drives builds through `session`, so the agent's edits and the web
  app's edits are the same code paths, already tested against upstream. It
  calls `engine` directly only for read-only bulk work (calculators, power,
  candidate evaluation) on copies of the session's build.
- The ports of upstream logic (the mod weights of `TradeQueryGenerator`,
  the power report) live in `engine` or `session` as usual and are listed
  in `sync/manifest.toml`. `agent` and `mcp` are new code with no upstream
  counterpart.

### `crates/agent`: the agent's build API

The session's commands copy upstream's controls: controls are addressed by
name, popups open and close, and `clickNode` toggles a node. That is right
for the web app and wrong for an agent. `agent` is a layer of build-level
verbs on top of it.

**Workspace and handles.** A `Workspace` holds named builds (`"main"`,
`"trial-1"`). Each is a session plus its history. `fork(handle)` copies a
build (save and load is the simple first version; a cheap copy of the
session later). Agents explore on forks and promote one to `main` when
they're done.

**Edits.** Each edit is idempotent, resolves names, and returns a
`StatDiff`:

| Verb | On the session | Notes |
| --- | --- | --- |
| `allocate(nodes)` | `nodePath`, `clickNode` along the path | by name or id; already allocated is a no-op; returns the points spent |
| `deallocate(nodes)` | `clickNode` | reports the orphaned nodes it also removes |
| `set_mastery(node, effect)` | `selectMastery` | effect by text |
| `set_ascendancy(class, asc)` | controls | |
| `set_gem(group, slot, gem, level, quality)` | `setGem` | gem by name, alternate qualities |
| `add_group`, `remove_group`, `set_main_skill` | `addSocketGroup`, ... | |
| `add_item(text)`, `equip(item, slot)` | `addItem`, `equipItem` | item text as copied in game or from the trade site |
| `craft(item, action)` | `craftItem`, popups | bench mods, anoints, eldritch implicits, enchants, corruptions |
| `set_config(key, value)` | `control` on the config tab | by the config option's name |
| `select_set(kind, name)` | `selectItemSet`, `selectTreeSpec`, ... | |
| `undo` | `undo` | |

**Reads.** All compact, with stable names:

- `summary`: the sidebar, grouped (offence, defence, resources, warnings);
  the warnings matter (uncapped resistances, unmet requirements, reservation
  over 100%).
- `explain(stat)`: the breakdown of one output, as the Calcs tab shows it.
- `tree`, `skills`, `items`, `config`: the current state, compact.
- `diff(a, b)`: the stat changes between two builds or two states.

**Search.** Lookups over the game data, so the agent never guesses ids:
nodes by name or stat text (notables, keystones, masteries, cluster
notables), gems, uniques (`itemDb`), mods for a slot (`QueryMods`), bases.

**Transactions.** `try(changes) -> StatDiff` runs a list of edits on a
fork and throws it away, or keeps it on request. An agent proposes edits
and compares them; making that cheap is the most important performance
property of the API.

### Objectives and constraints

An objective is a list of weighted power stats, the same shape as
upstream's `statWeights`, scored with `WeightedRatioOutputs` so the numbers
stay comparable to upstream's. Presets: `dps` (`FullDPS`), `ehp`
(`TotalEHP`), `balanced`, or custom weights.

Constraints are hard filters on a candidate's full output, checked after
the exact evaluation:

- resistances at their caps, attribute and level requirements met,
  reservation within 100%, no new warnings;
- minimums: life, EHP, hit chance, spell suppression, and the like;
- keep a flag: an ailment immunity, a curse slot, a keystone.

A candidate that breaks a constraint isn't simply dropped. The upgrade
search tries to **repair** it first: it finds the cheapest fix (a
resistance on another slot, a bench craft, a node) and offers the pair.
That is the difference between "this helmet is +18% DPS" and an upgrade
you can actually equip.

### Upgrade search

`next_upgrades(objective, constraints, kinds, budget)` returns a ranked
list of `Upgrade { change, diff, score, cost, how }`. Each kind of upgrade
is a generator; the runner evaluates all candidates exactly (misc
calculator overrides where they fit, a fork where they don't, such as gem
swaps), in parallel on `engine::parallel`.

| Kind | Candidates | Cost class | Engine pieces |
| --- | --- | --- | --- |
| free fixes | unspent points, mastery effects, pantheon, bandit, flask choice, gem level and quality | free | node power, `selectMastery`, config |
| passive tree | best paths by node power at depths 1 to n; 1-for-1 respecs (the weakest allocated node out, the best path in) | points, respec | `engine::power` |
| support gems | swaps per group, awakened and alternate quality versions | buy | `compare::gem_power` |
| anoints | every anoint for the amulet | buy | `compare::anoint_power` |
| uniques | every unique for each slot, and jewels | buy | `compare` item power (`ItemDBControl`) |
| crafts | bench mods, eldritch implicits, enchants on items you have | craft | `session::typed::item_add_modifier`, `engine::item_craft` |
| rares | per slot, the stats worth looking for and what each is worth (the mod weights), plus pasted items evaluated exactly | buy | the port of `GenerateModWeights` |
| jewels | cluster jewels, timeless jewels, and the stats a rare jewel should have | buy | timeless search, cluster subgraphs |

**Ranking without prices.** With no market data, costs are classes: free,
passive points, respec, craft, buy. The answer leads with the best free
changes and the best in each class. A budget is optional: you can give
prices yourself (`prices` in the workspace, e.g. an anoint's oils at 2
divines), and those upgrades are then also ranked by score per divine.

**Rares, locally.** For each slot the search lists the stats worth looking
for and what each is worth (`+70 life: +1.2% EHP`). That's a shopping list
to take to the trade site. You paste the items you find (`add_item` reads
the text the game or the trade site copies), and `evaluate_item` gives each
one's exact change in your build, constraints included.

A greedy loop on top (`plan_upgrades`) takes the best upgrade, applies it
to a fork, and searches again, for a number of steps or until the given
prices exhaust a budget. That captures interactions (a second DPS upgrade
is worth less once a resistance item is gone) that a single ranked list
misses.

Long searches run as **jobs**: the tool returns a job id at once, reports
progress through MCP progress notifications, and the result is read when
done. Node power with Full DPS and every slot's mod weights are the cases
that need it.

### The mod weights (a port, in `engine`)

`GenerateModWeights` and `GeneratePassiveNodeWeights` of
`TradeQueryGenerator`, ported like any other upstream logic:
`QueryMods.lua` read at build time by the importer as game data,
upstream's mid-roll values and its special cases (Megalomaniac, Watcher's
Eye, talisman implicits, `includeAllWEMods`). **Tested against the
oracle**: for corpus builds, slots and stat weights, the weights equal
upstream's, which keeps to the rule that expected values come from
upstream only. The query JSON and everything after it are deferred with
trade.

### `crates/mcp`: the server

`pob mcp`, on the official Rust MCP SDK (`rmcp`), over stdio.

**Threads.** A session isn't `Send` (`Rc` inside), so each build in the
workspace lives on its own thread (an actor taking requests over a
channel), the way the web app has one engine per worker. Bulk evaluations
fan out to the rayon pool on copies.

**Data.** It starts from a `core.bin` bundle that `pob bundle` makes
locally from the pinned upstream submodule (see
[Game data](#game-data)), so it doesn't evaluate Lua at start-up.

**Tools.** The `agent` verbs one-to-one, plus:

| Tool | What it does |
| --- | --- |
| `load_build(source)` | build code, XML, or a file in the workspace |
| `save_build(handle, path?)` | writes the XML; returns the build code |
| `summary`, `explain`, `tree`, `skills`, `items`, `config` | reads |
| `search(kind, query)` | nodes, gems, uniques, mods, bases |
| `allocate`, `set_gem`, `add_item`, `equip`, `craft`, `set_config`, ... | edits with their `StatDiff` |
| `try(changes)`, `fork`, `promote`, `undo` | exploration |
| `evaluate_item(text, slot?)` | a pasted item's exact change in the build, without equipping it |
| `best_nodes`, `best_gems`, `best_anoints`, `best_uniques`, `mod_weights(slot)` | one generator each |
| `next_upgrades`, `plan_upgrades` | the whole upgrade search (jobs) |
| `raw_request(json)` | the session's JSON commands, an escape hatch while the verbs are incomplete |

**Resources.** `pob://build/<handle>/summary`, `.../xml`,
`pob://data/node/<id>`, the stat weight presets: things a client can
attach to a conversation without a tool call.

**Prompts.** `/next-upgrade` (runs `next_upgrades`, answers in a fixed
format), `/audit` (defences against the usual endgame checklist: caps,
suppression, ailments, max hits), `/level-plan` (the tree's order of
points).

**Answers sized for an LLM.** Tools return summaries with numbers in
context (`"Full DPS 2.41M → 2.77M (+14.9%)"`) and a few lines each. The
full outputs are a resource the agent can read when it needs them.

**Notice.** The server's description says it is not affiliated with or
endorsed by Grinding Gear Games, as GGG's developer docs ask of
third-party tools (as secondary sources quote them).

### A skill for the craft

The tools say what the engine can compute; a skill says how to use them
well. `.claude/skills/build-coach/SKILL.md` holds the playbook: start with
`summary` and the warnings; never trade a defence for DPS without saying
so; prefer `try` before edits; check constraints after every change; say
which costs are guesses. Game knowledge the engine doesn't have (which
uniques are build-enabling, what a Watcher's Eye mod is usually worth)
belongs here too, kept short.

## Game data

`core.bin` is made from GGG's game data: upstream exports it from the game
files and commits it in `src/Data` and `src/TreeData`, each file headed
"(c) Grinding Gear Games", and its updater downloads those files from
GitHub. We read the same files from the submodule at build time.

**Decision: `core.bin` is never published.** No release asset, no hosted
copy for the MCP server. Whoever runs `pob mcp` builds it with
`pob bundle` from the pinned submodule, the same way the tests and the web
app's build do. Reasons:

- GGG's developer docs and terms couldn't be read from this environment;
  what secondary sources quote says GGG supports no access to in-game data
  outside its APIs (the passive tree being the stated exception), and that
  its terms forbid modifying or adapting the game client or its data
  without approval. Read conservatively, a new public copy of the game
  data is a step this project doesn't need to take.
- Local building costs little: a few seconds of `pob bundle` on top of
  the build of `pob`, and it keeps the data exactly in step with the
  engine's pinned upstream commit.
- It keeps to this repository's rule that GGG's data is read from the
  submodule at build time, never shipped by us.

Hosting the web app will need the same question answered for the bundle
it serves; that decision is separate and still open.

## Deployment

### Locally

`.mcp.json` in this repository starts `pob mcp --workspace builds/` for
Claude Code or Claude Desktop, after `cargo build --release -p cli` and
`pob bundle`.

### From your phone

The Claude app starts a Claude Code session in a cloud container on a
repository with the same `.mcp.json`. The environment's **setup script**
clones the submodule, builds `pob` and runs `pob bundle`. That takes
minutes on a fresh container but needs no network beyond GitHub and the
package registries the default policy allows.

```text
builds/
  main.xml          the build, one commit per accepted change
  workspace.toml    objective, constraints, optional prices
```

Git is the build's history: every accepted change is a commit with its
stat diff in the message, so "undo yesterday's respec" is a `git revert`.

The flow: paste a build code, "what next?" → `load_build` →
`next_upgrades` (a job, seconds to a minute) → a ranked list: free changes,
then points, crafts and purchases, plus each slot's shopping list → "do
the anoint, and here are three boots I found" → `evaluate_item` on each,
the edits, a commit, a new build code to paste into PoB.

### Later

- The web app's deep links: `save_build` returns a link that opens the
  build in the web app, once it is hosted.
- A remote connector for claude.ai (`pob mcp --http`): needs hosting,
  auth and compute budgets, and the game data question above.

## Deferred: trade and prices

Not built until decided otherwise. What it would add, so the layers keep
room for it:

- a `crates/market`, the only crate with network access, native only:
  upstream's trade query (`FinishQuery`, the complexity budget, pseudo
  stats), trade search and fetch with a port of `TradeQueryRateLimiter`,
  listings turned into items and evaluated exactly, prices from the
  currency exchange upstream uses and from poe.ninja, build links
  (pobb.in), character import;
- rares in `next_upgrades` ranked by real listings, and score per divine
  from market prices;
- rules from [design.md](design.md#prior-art-pobcool): never a
  `POESESSID`, an honest `User-Agent`, upstream's rate limits, caching, no
  automated buying;
- before it starts: read GGG's developer docs and terms from the source
  (the environment's network needs `www.pathofexile.com`), and decide the
  `User-Agent`'s contact.

## Testing

- `agent` verbs: each against the session commands it runs; their numbers
  are already the engine's, which the equivalence tests cover.
- The mod weights: against upstream through the oracle, like every port.
- Upgrade search: the same ranking with any thread count, and the greedy
  plan's final build equal to applying its steps one by one.
- `mcp`: a client in the test drives a stdio server through load, edits,
  `try`, `evaluate_item`, `next_upgrades` and save.

## Steps

Each one a PR, as usual:

1. Sub-step 8 of the deletion (the prerequisite).
2. `crates/agent` and `pob mcp` over stdio: load, save, `summary`,
   `search` for nodes, `allocate`, `deallocate`, `try`, `raw_request`;
   `.mcp.json` in this repository.
3. The remaining edits and reads: gems, items, crafts, config, sets,
   `explain`, `evaluate_item`.
4. Objectives, constraints, and `best_nodes`/`best_gems`/`best_anoints`/
   `best_uniques`; `next_upgrades` over those kinds; jobs.
5. The port of the mod weights with its oracle test; `mod_weights` and
   rares in `next_upgrades`.
6. `plan_upgrades`, constraint repair, the `build-coach` skill, and the
   cloud environment's setup script: working from a phone.
7. Later: trade and prices (above), the web app's deep links, a remote
   connector.

## Questions for the owner

- **Where personal builds live.** `builds/` in this repository (git
  ignored, or committed on a branch of your own), or a separate private
  repository that uses this one.
- **Default objective.** Upstream's Trade tab defaults, or a balanced
  preset of our own.
