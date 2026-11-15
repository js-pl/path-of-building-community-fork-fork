# MCP: an agent works on your build

Status: proposal. Nothing here is built yet. The prerequisite is sub-step 8
of [the deletion](drop-translation.md#step-4-in-sub-steps), which reshapes
`session::dispatch`.

## Goal

An LLM agent (Claude Code, Claude Desktop, claude.ai, the Claude app on a
phone) works on a Path of Exile build with this project's engine through
the [Model Context Protocol](https://modelcontextprotocol.io). It can:

- read a build: stats, breakdowns, the tree, skills, items, config;
- edit it: allocate nodes, swap gems, equip items, set the config;
- try changes and compare them before committing to one;
- search for upgrades and rank them by impact per cost, across the tree,
  gems, anoints, uniques and rares;
- find real trade listings for an upgrade, evaluate each one exactly in
  the engine, and hand back trade links;
- hand the build back as a build code or a pobb.in link.

The question the design is built around is **"what is the next most
impactful upgrade I should get?"**, asked from a phone with a build link and
a budget. The answer is a short ranked list, each entry with its exact
stat change, its cost, and a link to buy it or a step to do it.

### Why this engine fits

- **Speed.** A misc calculator evaluation (one override: a node, an item)
  takes about 2 ms natively, node power about 4 s with Full DPS off
  ([progress.md](progress.md), the bench). Upstream's trade query
  generator weighs a few hundred mods per slot one calculation at a time on
  the UI thread; here that is a few seconds on all cores, which is what
  makes evaluating every slot and every candidate in one tool call possible.
- **Correct numbers.** Every number comes from the engine, which the
  equivalence tests hold equal to upstream. The agent reasons about the
  numbers; it does not make them up.
- **No I/O in the engine.** The layers that talk to the network are
  separate and can be audited, rate limited and turned off.

## Upstream already has the core idea

Upstream's Trade tab (`Classes/TradeQuery.lua`,
`TradeQueryGenerator.lua`, `TradeQueryRequests.lua`,
`TradeQueryRateLimiter.lua`, `TradeHelpers.lua`, `Data/QueryMods.lua`,
`Data/TradeSiteStats.lua`; all `todo` in `sync/manifest.toml`) does this
for rares:

1. Take the user's stat weights (`statWeights`: Full DPS, EHP, life, ...)
   and score an output against the base output with
   `WeightedRatioOutputs` (a weighted sum of ratios, so stats of different
   units mix).
2. For each mod that can roll on the slot's item category (`QueryMods.lua`),
   put a mid-roll value on a blank test item, run the misc calculator with
   `repSlotName`/`repItem`, and keep the gain per point as the mod's
   weight (`GenerateModWeights`). It weighs anoints and passive nodes the
   same way (`GeneratePassiveNodeWeights`).
3. Write a trade site query with a `weight` stat group of those weights and
   a minimum of half the current item's score (`FinishQuery`), within the
   site's complexity budget of 200.
4. Search, fetch the listings, turn each one into an item, re-evaluate it
   exactly with the calculator, and sort by stat gain or by gain per price,
   with prices converted through the currency exchange
   (`web.poecdn.com/api/currency-exchange`).

The weighted sum is only a linear estimate (mods multiply off each other),
so step 4's exact re-evaluation is what makes the ranking trustworthy. This
design ports that pipeline faithfully, tests it against the oracle like
everything else, and then generalises it from "rares for one slot" to every
kind of upgrade.

## Layers

```text
 clients    Claude Code (local, or cloud from the phone) · Claude Desktop · claude.ai connector
              │ MCP over stdio                                 │ MCP over streamable HTTP
 crates/mcp ──┴── `pob mcp`: tools, resources, prompts; build workspace; job runner
              │
 crates/agent    build-level API for agents, no I/O, also builds for wasm:
              │  handles, edits, transactions, diffs, search, objectives,
              │  upgrade search, trade query generation
              │
 crates/market   all network access, native only: trade search and fetch with
              │  the rate limiter, prices, leagues, build links, character import;
              │  disk cache
              │
 crates/session  the engine API (exists): build sessions, JSON commands
 crates/engine   the typed engine (exists): calcs, misc calculator, power, compare
```

Rules:

- `agent` and `engine` do no I/O. `market` is the only crate that opens
  sockets, and `mcp` is the only one that uses it.
- `agent` drives builds through `session`, so the agent's edits and the web
  app's edits are the same code paths, already tested against upstream. It
  calls `engine` directly only for read-only bulk work (calculators, power,
  candidate evaluation) on copies of the session's build.
- The ports of upstream logic (the trade query generator, the power
  report) live in `engine` or `session` as usual and are listed in
  `sync/manifest.toml`. `agent`, `market` and `mcp` are new code with no
  upstream counterpart.

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
| `add_item(text)`, `equip(item, slot)` | `addItem`, `equipItem` | item text as copied in game or from trade |
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
upstream's `statWeights`, scored with `WeightedRatioOutputs` so the
numbers stay comparable to upstream's Trade tab. Presets: `dps`
(`FullDPS`), `ehp` (`TotalEHP`), `balanced`, or custom weights.

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

`next_upgrades(objective, constraints, budget, kinds, league)` returns a
ranked list of `Upgrade { change, diff, score, cost, how }`. Each kind of
upgrade is a generator; the runner evaluates all candidates exactly
(misc calculator overrides where they fit, a fork where they don't, such
as gem swaps), in parallel on `engine::parallel`.

| Kind | Candidates | Cost | Engine pieces |
| --- | --- | --- | --- |
| free fixes | unspent points, mastery effects, pantheon, bandit, flask choice, gem level and quality | nothing | node power, `selectMastery`, config |
| passive tree | best paths by node power at depths 1 to n; 1-for-1 respecs (the weakest allocated node out, the best path in) | points or regret orbs | `engine::power` |
| support gems | swaps per group, awakened and alternate quality versions | gem price | `compare::gem_power` |
| anoints | every anoint for the amulet | oil price | `compare::anoint_power` |
| uniques | every unique for each slot, and jewels | price | `compare` item power (`ItemDBControl`) |
| crafts | bench mods, eldritch implicits, enchants on items you have | craft cost | `session::typed::item_add_modifier`, `engine::item_craft` |
| rares | trade queries per slot (below), each listing evaluated exactly | listing price | the port of `TradeQueryGenerator` |
| jewels | rare and abyss jewels via trade, cluster jewels, timeless jewels | price | timeless search, cluster subgraphs |

Ranking: by score per divine within the budget, plus "best overall" and
"best free". Costs are kept as distributions (the cheapest few listings'
spread), so the agent can tell a firm price from one outlier.

A greedy loop on top (`plan_upgrades(budget)`) takes the best upgrade,
applies it to a fork, and searches again, until the budget runs out. That
captures interactions (a second DPS upgrade is worth less once a resistance
item is gone) that a single ranked list misses.

Long searches run as **jobs**: the tool returns a job id at once, reports
progress through MCP progress notifications, and the result is read when
done. Node power with Full DPS and a trade fetch across ten slots are the
cases that need it.

### Trade query generation (a port, in `engine`)

`TradeQueryGenerator` and the parts of `TradeQuery` that aren't UI, ported
like any other upstream logic:

- `QueryMods.lua` and `TradeSiteStats.lua` read at build time by the
  importer, as game data;
- mod weights per slot and item category, upstream's mid-roll values,
  its special cases (Megalomaniac, Watcher's Eye, talisman implicits,
  `includeAllWEMods`), its pseudo stats and its complexity budget;
- the query JSON itself.

**Tested against the oracle**: for corpus builds, slots and stat weights,
the generated weights and query JSON equal upstream's `ExecuteQuery`
output. That fits the rule that expected values come from upstream only.
The pipeline then extends upstream's: constraint-aware queries (a pseudo
resistance minimum when the current item carries resistances the build
needs), and queries for every slot in one go.

### `crates/market`: the network

Native only, behind a trait so tests run against recorded responses.

- **Trade API** (`www.pathofexile.com/api/trade`): `search` (query → id,
  results) and `fetch` (listing ids → items with prices), `data/leagues`,
  `data/stats`. The search id gives the link the user opens,
  `https://www.pathofexile.com/trade/search/<league>/<id>`. The rate
  limiter is a port of `TradeQueryRateLimiter` (it reads the
  `X-Rate-Limit-*` headers and waits); hitting a limit makes a tool call
  slower, never a failure the agent has to retry.
- **Listings to items**: a listing's item JSON to the item text `add_item`
  reads (upstream does this in `TradeQuery`), then the exact evaluation.
- **Prices**: the currency exchange endpoint upstream uses, for currency
  to divines; poe.ninja for uniques, gems, oils and cluster jewels.
  Cached for an hour.
- **Build links**: pobb.in and poe.ninja build pages to build codes, and
  pobb.in upload for handing a build back.
- **Character import**: the public profile endpoints upstream's
  `ImportTab` uses, later GGG's OAuth API (stash contents give the real
  budget).

Rules carried over from [design.md](design.md#prior-art-pobcool): never a
`POESESSID`; an honest `User-Agent` naming the tool; upstream's rate
limits respected; searches and fetches cached; no automated buying. The
agent shows listings, links and the seller's whisper text; the user buys.
Every network tool can be switched off (`--offline`), and then upgrade
search still covers everything except rares from trade.

### `crates/mcp`: the server

`pob mcp`, on the official Rust MCP SDK (`rmcp`). Over stdio for Claude
Code and Desktop, over streamable HTTP for remote clients.

**Threads.** A session isn't `Send` (`Rc` inside), so each build in the
workspace lives on its own thread (an actor taking requests over a
channel), the way the web app has one engine per worker. Bulk evaluations
fan out to the rayon pool on copies.

**Data.** It starts from the `core.bin` bundle (`pob bundle`), not from
the upstream checkout: no submodule and no Lua needed
where it runs.

**Tools.** The `agent` verbs one-to-one, plus:

| Tool | What it does |
| --- | --- |
| `load_build(source)` | build code, XML, file path, pobb.in or poe.ninja link, or a character (`account/character`) |
| `save_build(handle, path?)` | writes the XML; returns the build code and, if allowed, a pobb.in link |
| `summary`, `explain`, `tree`, `skills`, `items`, `config` | reads |
| `search(kind, query)` | nodes, gems, uniques, mods, bases |
| `allocate`, `set_gem`, `add_item`, `equip`, `craft`, `set_config`, ... | edits with their `StatDiff` |
| `try(changes)`, `fork`, `promote`, `undo` | exploration |
| `best_nodes`, `best_gems`, `best_anoints`, `best_uniques` | one generator each |
| `next_upgrades`, `plan_upgrades` | the whole upgrade search (jobs) |
| `trade_query(slot, objective)` | the query, its link, and the top listings evaluated |
| `price(item)` | market prices |
| `raw_request(json)` | the session's JSON commands, an escape hatch while the verbs are incomplete |

**Resources.** `pob://build/<handle>/summary`, `.../xml`,
`pob://data/node/<id>`, the stat weight presets: things a client can
attach to a conversation without a tool call.

**Prompts.** `/next-upgrade` (asks for league and budget, runs
`next_upgrades`, answers in a fixed format), `/audit` (defences against
the usual endgame checklist: caps, suppression, ailments, max hits),
`/level-plan` (the tree's order of points).

**Answers sized for an LLM.** Tools return summaries with numbers in
context (`"Full DPS 2.41M → 2.77M (+14.9%)"`) and a few lines each. The
full outputs are a resource the agent can read when it needs them.

### A skill for the craft

The tools say what the engine can compute; a skill says how to use them
well. `.claude/skills/build-coach/SKILL.md` (in the repo where builds are
worked on) holds the playbook: start with `summary` and the warnings; never
trade a defence for DPS without saying so; prefer `try` before edits;
check a trade result's constraints; show trade links and whisper texts and
never promise a price; state the league and the patch. Game knowledge the
engine doesn't have (what a Watcher's Eye mod is worth, which uniques are
build-enabling) belongs here too, kept short.

## Deployment: from your phone

### Claude Code in the cloud (first)

The Claude app on a phone starts a Claude Code session in a cloud
container on a repository. That repository holds your builds:

```text
my-builds/                    a private repository of yours
  .mcp.json                   starts `pob mcp --workspace builds/`
  .claude/skills/build-coach/
  builds/
    main.xml                  the build, one commit per accepted change
    league.toml               league, budget, objective, constraints
```

- The environment's **setup script** downloads a `pob` release binary and
  a `core.bin` built by this repository's CI (or builds them; slower).
- The environment's **network** allows `www.pathofexile.com`,
  `web.poecdn.com`, `poe.ninja` and `pobb.in`, and nothing else is needed.
- Git is the build's history: every accepted change is a commit with its
  stat diff in the message, so "undo yesterday's respec" is a `git revert`.

The flow: "here's my pobb.in link, 6 divines, the current league, what next?"
→ `load_build` → `next_upgrades` (a job, about a minute with trade fetches)
→ a ranked list with trade links you tap to open on the phone → "do the
anoint and the boots" → the edits, a commit, a new build code to paste into
the game's PoB or this project's web app.

### Locally

The same `.mcp.json` in Claude Code or Claude Desktop on your computer,
with `pob` on the path.

### Remote connector (later)

`pob mcp --http` behind a hosted endpoint, added to claude.ai as a custom
connector: no repository and no Claude Code needed. Open questions: compute
cost (an upgrade search is CPU minutes), auth (the connector's OAuth),
and the trade API's rate limits, which a shared server IP hits for
everyone at once (pob.cool's lesson). The engine compiles to wasm and does
no I/O, so a server on an edge runtime is possible; the trade calls are
the part that has to come from somewhere with its own limits.

### The web app (later)

`save_build` returns a deep link that opens the build in the web app
(`#code=...`), once the web app is hosted and reads it. Later the other
direction: the web app holds the live build and exposes the same tools to
an agent in the browser.

## Testing

- `agent` verbs: each against the session commands it runs; their numbers
  are already the engine's, which the equivalence tests cover.
- The trade query generator: against upstream through the oracle (mod
  weights and query JSON for corpus builds), like every port.
- Upgrade search: deterministic on recorded market data (a fixed set of
  listings and prices), the same ranking with any thread count, and the
  greedy plan's final build equal to applying its steps one by one.
- `market`: against recorded responses; no network in CI.
- `mcp`: a client in the test drives a stdio server through load, edits,
  `try`, `next_upgrades --offline` and save.

## Steps

Each one a PR, as usual:

1. Sub-step 8 of the deletion (the prerequisite).
2. `crates/agent` and `pob mcp` over stdio: load, save, `summary`,
   `search` for nodes, `allocate`, `deallocate`, `try`, `raw_request`;
   `.mcp.json` in this repository for trying it.
3. The remaining edits and reads: gems, items, crafts, config, sets,
   `explain`.
4. Objectives, constraints, and `best_nodes`/`best_gems`/`best_anoints`/
   `best_uniques`; `next_upgrades --offline` over those kinds; jobs.
5. The port of `TradeQueryGenerator` with its oracle test.
6. `crates/market`: trade search and fetch, the rate limiter, prices;
   `trade_query` and rares in `next_upgrades`.
7. Release artifacts (`pob`, `core.bin`), the builds repository template
   and the `build-coach` skill: working from a phone.
8. `plan_upgrades`, constraint repair, character import, pobb.in upload.
9. Later: the remote connector, the web app's deep links and in-browser
   tools.

## Questions for the owner

- **Release artifacts.** `core.bin` is derived from GGG's game data. The
  web app will serve the same file, but publishing it as a release asset is
  a distribution decision.
- **User-Agent.** The trade API asks automated clients for a contact; whose
  contact, or the project's.
- **Where personal builds live.** A separate private repository (proposed)
  or a directory here.
- **Default objective.** Upstream's Trade tab defaults, or a balanced
  preset of our own.
