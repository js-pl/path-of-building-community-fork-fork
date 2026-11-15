# Dropping the translation

Status: option C accepted; step 1 done (the browser build and `pob serve`
are typed-only by default), step 2 under way (see
[the oracle session](#the-oracle-session-step-2)), nothing removed yet. The goal in
[design.md](design.md#mirroring-upstream) is that `crates/upstream`, `luagen`
and the Lua runtime go, the oracle stays the only reference, and Lua is left
in the importer and the oracle. This note lists what still depends on them
and how the tests keep a reference without them.

## What links them today

| Code | Uses | Shipped? |
| --- | --- | --- |
| `crates/upstream` | upstream's whole program translated by `luagen` in its build script, plus the headless host | only through the features below |
| `crates/session`, feature `translation` (default; `wasm` turns it off) | `translated.rs`, `lua/session.lua` (translated too), the dispatcher's fallback and replay (`Dispatcher::fallbacks`, `pending`) | native CLI (`pob serve --typed`, `--translated`) and the wasm build with `--translation` |
| `crates/wasm`, feature `translation` (not default) | `session/translation` | `web/scripts/build-wasm.mjs --translation` (80 MB, 15 MB gzipped, against 8.2 MB, 2 MB typed) |
| `crates/cli` | `upstream`, `lua::gc` | the native CLI (`pob serve --typed`, `--translated`, `bundle --lua`, `bench`) |
| `crates/equivalence` | `upstream`; `bridge.rs` converts the translated program's stores into typed ones | tests only |
| `crates/importer::luagen` | the translator (1.9k lines), run by `crates/upstream/build.rs` | build only |
| `crates/lua` | interpreter (parser, `env`, `rt`, `gc`, 6.4k lines) **and** library pieces: Lua patterns (`pattern`), `tostring`/`%g` formatting (`format`, `value::format_number`), LuaJIT's `pairs` order (`ljtab`) | the library pieces are in the typed engine and ship |
| `crates/importer`, `crates/gamedata` | the interpreter, to evaluate upstream's data files and `ModParser.lua` at build time | build only |
| `crates/engine` | the library pieces everywhere; `engine::install`, `mod_store.rs`, `mods/convert.rs` and a `tree.rs` helper still touch `lua::env`/`lua::rt` (the replacements installed into the running translation, and conversions) | the library pieces |

The typed session no longer falls back for anything the session and
equivalence tests exercise (`KNOWN_FALLBACKS` holds only cases where upstream
itself fails or the translation carries another build's shared state), and CI
already lints `session` and `wasm` with `--no-default-features`. So shipping
without the translation needed no engine work: `build-wasm.mjs` builds the
typed engine unless given `--translation`, and `translation` is not a
default feature of `wasm`.

Dropping the Lua runtime entirely is a different matter: the engine needs
the library pieces (they are how it reproduces upstream's string patterns,
number formatting and table order), and the importer needs the interpreter
to read upstream's data. Only the parts that run upstream's *program* can go:
`crates/upstream`, `luagen`, `lua/session.lua`, `translated.rs`, `bridge.rs`,
`engine::install` and the `lua::gc` cycle collector (only the translation
makes cyclic environments).

## Where the tests get their reference

| Tests | Reference today |
| --- | --- |
| `equivalence/tests/build_output.rs`, `calcs.rs` (whole builds, `MAIN` and `CALCS`), `mod_parser.rs`, `data.rs` | the oracle's committed outputs (`tests/oracle`, 13 MB) |
| `equivalence/tests/oracle_variants.rs` (the variant families of `equivalence::variants`: skill functions, triggers, mods, tattoos, timeless jewels, mirages, unconnected-allocation jewels, Calcs inputs, every Configuration option; whole builds, `MAIN` and `CALCS`) | the oracle, live (`oracleOutputs` of `oracle/session.lua`); every trigger variant in the nightly (`TRIGGERS_FULL`) |
| `equivalence/tests/{item,tree,game_data}.rs` (typed items: every unique and rare template and every corpus build's items; the passive trees; the typed data tables) | the oracle, live (`oracleValue` of `oracle/session.lua`, see [Moved to the oracle](#moved-to-the-oracle)) |
| `equivalence/tests/calculator.rs` (calculator overrides: nodes added and removed, items replaced or taken off, flasks toggled, a condition; base and override outputs, player and minion) | the oracle, live (`oracleCalculator` of `oracle/session.lua`: `calcsTab.miscCalculator` on the loaded build), numbers bitwise |
| `equivalence/tests/{power,spec}.rs` (`mods`, `calc_setup`, `items_tab`, `skills`, `perform`, `breakdown` and `config` retired, see [Retired tests](#retired-tests)) | the translation, live: intermediate state (stores, setup slices, breakdowns, calculator calls, node power) compared slice by slice through `bridge.rs` |
| `session/tests/typed.rs` (14 phases: commands, geometry, calcs, tree, skills, save, items, config, tooltips, party, new, overlays, power, and `variants`: the Calcs tab's breakdowns of the breakdown-writing skill functions, the mirages and the Calcs input variants) | in CI the oracle, live (`SESSION_REFERENCE=oracle`, LuaJIT from `oracle/setup.sh`); locally by default the translation: random edit sequences whose next step is chosen from the previous answers (thousands of edits and views per run), plus fresh translated sessions to align upstream's shared state |
| `session/tests/session.rs`, `web/tests` | the translation (native against wasm) |

The live, adaptive tests are the hard part. Their sequences depend on the
answers, and some comparisons need a fresh upstream session on demand (the
shared-state alignments in `docs/progress.md`, "Upstream quirks").

## Options

### A. Keep the translation as a test-only crate

Ship without it; keep `crates/upstream`, `luagen` and the interpreter as
dev-dependencies of the tests.

- Keeps every test as it is, including the intermediate-state ones that make
  porting a new upstream feature tractable (the stores and setup slices show
  *where* a difference starts, the whole-build outputs only *that* there is one).
- CI: the translated crate is the slowest part of the build: about 10 to 15
  minutes and 9 GB of memory cold (`CARGO_BUILD_JOBS=2-3` on 15 GB machines),
  a 255 MB rlib; sccache makes warm builds about 2 minutes. The translation's
  test jobs (session 3 shards, perform 2, setup and breakdowns, items and
  config) take about 5 to 7 minutes each on 8 runners. Each translated session
  takes 1.6 GB and keeps about 12 MB per build loaded (hence sharded child
  processes).
- Disk: a few GB of `target` per profile.
- Sync: every upstream bump must translate. `luagen` fails loudly on Lua it
  cannot translate, so a sync can block on translator work unrelated to any
  feature the typed engine has. The translation also differs from LuaJIT in
  details the oracle does not see (string-key `pairs` order is insertion
  order, headless stubs), so it is a second reference to keep in line with the
  first.

### B. The oracle as the only reference

Retire the translation and get every expected value from upstream on LuaJIT.

1. **Session fixtures, recorded.** Run `lua/session.lua` on the oracle's
   LuaJIT (it needs only upstream's globals and `dkjson`, which
   `oracle/headless.lua` already loads through `HeadlessWrapper.lua`) behind a
   JSON-lines driver, and record transcripts: each request with its response.
   The random sequences are generated by the driver (or by the Rust test
   against a recording run), so the requests come from upstream's answers;
   the typed test replays the requests and compares the responses, failing at
   the first difference, which is where an adaptive sequence would diverge
   anyway.
   Size: whole-build outputs alone are 13 MB for 61 builds; transcripts with
   `calcs`, breakdowns, tree geometry and tooltips would be a few hundred MB
   uncompressed. Store them compressed (zstd) and keep the big views (calcs,
   geometry, breakdowns) as hashes plus a regeneration command, or store
   them in a release artifact or LFS instead of the repository.
2. **Or a live oracle session in CI.** The same driver, but the test talks to
   a LuaJIT process instead of reading fixtures: no fixtures to store, the
   sequences stay adaptive, fresh sessions are a new process. The oracle job
   already builds LuaJIT (`oracle/setup.sh`); the session test jobs would need
   it too, and local runs need `oracle/setup.sh`.
3. **Intermediate-state tests** cannot be reproduced from whole-build outputs.
   Either the dumper writes intermediates (setup slices, stores, breakdowns:
   much larger dumps and a dumper per slice), or these tests retire once the
   typed engine is the only implementation and the whole-build, variant and
   session comparisons cover them. They were porting scaffolding: once a slice
   matches, a regression in it shows up in the outputs.

Costs: no translator to maintain and no 10-minute, 9 GB crate; the session
test becomes as fast as LuaJIT plus the typed engine (LuaJIT is likely
faster than the translation; not measured yet). New work: the JSON-lines driver,
porting the test drivers to it, fixture storage or a LuaJIT in every test job,
and these reference differences, which today the translation hides:

- LuaJIT hashes strings with a random seed (`LUAJIT_SECURITY_STRHASH`) and
  tables by address, so `pairs` over string or table keys is not reproducible
  across runs. The typed engine copies the translation's insertion order in
  places (party labels, `otherEffects`, some sidebar lists). Build the oracle's
  LuaJIT with a fixed seed and teach the typed engine LuaJIT's string order
  (as `lua::ljtab` does for numbers), or canonicalize those outputs in the
  comparison.
- The headless stubs differ: `GetScreenSize`, `DrawStringWidth` and friends in
  `HeadlessWrapper.lua` against `crates/upstream/src/host.rs`. Layout-dependent
  views (the Party tab's control order, wrapped labels) follow the stubs.
- The shared-state alignments become a fresh LuaJIT session (live) or a
  recording made from a fresh session per build (fixtures).

Sync: each bump re-runs the recorder (minutes, like `oracle/run.py dump`)
and commits or uploads the transcripts; nothing has to translate.

### C. Both, in steps (recommended)

1. **Now: ship without the translation (done).** `build-wasm.mjs` and
   `pob serve` are typed-only unless given `--translation` (`--typed`,
   `--translated` for `pob serve`); `translation` is not a default feature of
   `wasm`. It stays a default of `session`, which is not shipped on its own
   (`wasm` turns its defaults off) and whose tests and CLI need it. Nothing is
   removed; the tests keep the translation (option A). Low risk: CI checks
   the typed-only build, and the session tests record every fallback.
2. **Next: an oracle session.** Write the JSON-lines driver for `session.lua`
   on LuaJIT, run it beside the translation, and compare the two on the
   existing sequences until they agree or every difference is explained
   (string-key order, stubs). That is the test that the oracle can replace the
   translation, made before anything depends on it.
3. **Then: move the session tests** to the live oracle session in CI (B.2),
   with a small recorded set (B.1) for running without LuaJIT locally. Retire
   the intermediate-state equivalence tests one by one where whole builds,
   variants and session sequences cover them; keep the oracle dumps for whole
   builds, the mod parser and data as they are.
4. **Last: delete** `crates/upstream`, `luagen`, `lua/session.lua`,
   `translated.rs`, `bridge.rs`, `engine::install`, `lua::gc`, and split
   `crates/lua` into the library pieces the engine ships and the interpreter
   the importer keeps.

Why this order: step 1 has the user-facing benefit (a 2 MB engine, no
translated code in the browser) at no testing cost; step 2 proves the new
reference against the old one before the old one goes; the expensive parts
(CI time, 9 GB builds, translator work on every sync) go only when nothing
needs them. Until step 4, every sync still has to translate, and an upstream
change the translator cannot handle blocks it; if that starts to cost more
than the steps above, step 3 is the one to bring forward.

Live (B.2) over recorded (B.1) as the main reference because the session
tests are adaptive and need fresh sessions on demand, and because transcripts
for all views would be large and would need regenerating on every sync
anyway; the recorded set is for offline runs and as a fallback when LuaJIT is
not available.

## The oracle session (step 2)

`oracle/session.lua` runs `lua/session.lua` on upstream headless on the
oracle's LuaJIT, one JSON request and response per line, as `pob serve`.
It differs from the translation's host only where headless upstream has
no implementation: `Inflate` and `Deflate` are zlib through LuaJIT's FFI,
files are read only, and source positions are stripped from errors (the
translation's carry none). `Session::oracle` runs it in a child process,
and `SESSION_REFERENCE=oracle` makes it the reference of
`typed_session_matches_translated` instead of the translation. It is fast:
a session starts and loads a corpus build in about 3.5 s, and the
`commands` phase takes 25 s, against the translation's 1.6 GB and minutes
per shard.

First run over all phases (the typed engine equals the translation on all
of them, so each difference is one between the translation and LuaJIT):

- **Agree:** `commands` (sidebar and outputs after loading every corpus
  build), `new`, most of `geometry`, half of `power`.
- **`pairs` order (not reproducible in upstream either).** LuaJIT seeds its
  string hash per process, so upstream itself gives these in a different
  order from run to run (checked: the same breakdown three times gives two
  orders). Breakdown rows (`flags` Attack/Cast/Spell, damage taken by type,
  sources of a stat), the base-type lines of a unique's raw text (`Variant`
  lines, so also saved items and builds), a tree connector, the party view's
  control order. The comparison has to treat these as unordered, or the
  oracle's LuaJIT has to run with a fixed seed and the typed engine learn
  LuaJIT's string order; unordered comparison is the practical option,
  since table keys hash by address.
- **The Party view's help label** was wrapped word by word on LuaJIT and
  not on the translation: `layoutControls` in `session.lua` iterated the
  controls with `pairs`, and positioning a control runs the size functions
  of those it is anchored to, which wrap the label. Our session script's
  bug, fixed: it goes through the controls by name and breaks ties of
  position by path. The Party view now agrees but for the enemy modifiers
  label's order.
- **Build codes:** zlib and miniz compress differently; compared decoded.
- **Compared unordered against the oracle:** the session test puts what
  upstream gives in `pairs` order into a canonical order when the oracle is
  the reference (`unordered` in `tests/typed.rs`): breakdown rows, tree
  connectors, runs of `{variant:..}` base-type lines, the party's enemy
  modifiers and exported mods. With it, 24 of 33 shards agree with the
  oracle (all of `commands`, `skills`, `save`, `items`, `new`, `overlays`,
  `power`; `tree` and `config` too).
- **Left to explain** (each a real difference, or upstream choosing between
  equals in `pairs` order):
  - with imported party members, a few builds' sidebar and `Ignite*`
    outputs (a party curse's `SelfIgniteChance` is applied on one side):
    explained, upstream's `pairs` order. `CalcPerform` slots the party's
    curses in `pairs` order over a table keyed by curse name, all of
    priority 0, so with more curses than the limit which ones apply
    follows LuaJIT's string hash (gen-0025 with gen-0029's Despair and
    Flammability under a limit of 1). The party's curses also take only
    the slots the build's own curses leave free (gen-0001 with Assassin's
    Mark and Frostbite under a limit of 2 beside its own curse). Against
    the oracle such party builds are skipped (`party_curses_ambiguous`:
    more party curses than their limit, or several beside a curse gem of
    the build's), and the curse list
    (`CurseList`, the Calcs tab's Curses and Debuffs row) is compared
    unordered;
  - gem names sharing a granted effect, `Vaal Righteous Fire` against
    `... of Arcane Devotion` as a breakdown row's source and in a saved
    gem: explained, upstream's `pairs` order. `Data.lua` makes such gems
    with one `grantedEffect`, so `data.gemForSkill[grantedEffect]` names
    whichever of them `pairs(data.gems)` sets up last; against the oracle
    every such name is compared as the group's shortest;
  - a breakdown row's source `Explosive Trap` against `... of Shrapnel`:
    explained, upstream's `pairs` order. A gem and its transfigured
    variants share mod tables, and `processMod` sets their `Skill:`
    source once per skill in `pairs(data.skills)` order, so the last
    wins; against the oracle a row's source is compared as the shortest
    skill name of its game id;
  - unique flavour text (Queen's Escape, Duskblight, a flask): explained,
    upstream's `pairs` order. A unique with several flavour texts shows the
    first `pairs(flavourTable)` gives (`ItemsTab:AddItemTooltip`); compared
    as the first in data order;
  - a connector of the 3.21 tree, 26270 to 57283 against 57283 to 26270:
    explained, upstream's `pairs` order. Both nodes list the other in
    `out`, the first `pairs(tree.nodes)` visits keeps its direction, and
    `tree.nodes` has the string key `root` among its numbers, so its
    layout follows the string hash; compared undirected;
  - a weapon that uses both hand slots (Oni-Goroshi) compared in a weapon
    slot while the other holds it: explained, upstream's `pairs` order
    (the oracle gives the comparison in two of four runs). The two slots
    disable each other, and `calcs.initEnv` breaks the cycle from the slot
    `pairs(itemDisablers)` gives first; against the oracle the weapon
    slots' comparisons of such tooltips are left out;
  - an item popup's mod list hovered on an entry tied with its neighbour
    on the sort key (gen-0055, Global Accuracy Rating (15-17)% against
    (18-20)% in the nightly): explained, upstream's `pairs` order. Tied
    entries keep the order they were gathered in, so one index names
    either mod; against the oracle such entries are not hovered (the lists
    themselves compare with ties sorted);
  - a shared item set dragged back into the build: explained, upstream's
    `pairs` order. `ItemSetListControl:ReceiveDrag` adds the set's items in
    `pairs` order of their slot names, so their ids, and with them every
    later item step, follow the string hash; that drag is left out
    against the oracle;
  - the gem dropdown's DPS of an attack candidate with The Saviour
    (3.13-dual-savior's Smite, in two of four oracle runs): explained,
    upstream's `pairs` order. `GemSelectControl` calculates the candidates
    in `pairs` order of their ids, and a mirage picks its skill from
    `GlobalCache` entries keyed by socket slot that the candidates
    calculated before left (also when the candidate is the mirage:
    gen-0020's General's Cry, 0 on one oracle run); against the oracle the
    candidates of builds with a mirage are compared without their DPS, and
    mirage candidates (General's Cry, Mirage Archer) are left out;
  - Bonespire's Intimidating Cry without cost, after a Tempest Shield
    trigger variant in the same oracle process: upstream keeps state from
    the builds loaded before (like the shared-state alignments of the
    session test); `oracle_variants.rs` asks a fresh oracle for a build
    that differs and lists it as aligned when that one agrees;
  - tied mods in the item editor's corruption and crafting lists
    (`(4-6)% increased Strength/... Dexterity` against `... Intelligence`):
    explained, upstream's `pairs` order. `ItemsTab` gathers them in
    `pairs` order and `table.sort`s by the first line with numbers as `$`;
    against the oracle such runs of ties are compared sorted;
  - an item edit that puts a non-weapon into Weapon 1 (the items
    phase's random `addItem`/`editItem`): upstream equips it, then
    `ItemsTab:PopulateSlots` goes through `pairs(self.slots)`; reaching
    Weapon 2 before Weapon 1 has dropped the item, `IsItemValidForSlot`
    indexes `weaponTypeInfo` with its type and fails, leaving the build
    broken (about half of the oracle runs). Against the oracle the rest
    of such a build is skipped, only for that failure (the oracle's
    `oracleLastError` request gives it with its source position:
    `Classes/ItemsTab.lua`, "attempt to index a nil value") and at most
    three builds per items shard (`WEAPON_SLOT_SKIPS`), so that a broader
    regression cannot pass for it;
  - node power `offence` `null` against 0 for builds without damage: a
    real difference, fixed. `0 / 0` is NaN, and LuaJIT's `math.max(0,
    NaN)` is NaN (it keeps the left operand only when strictly greater,
    as x86's `maxsd`; `math.min` alike), where the Lua runtime and the
    engine's `m_max` kept the left one, as PUC Lua does. Both now follow
    LuaJIT, a NaN or a -0 on either side giving the right operand;
  - a Calcs cell's list of debuffs (`Maim, Assassin's Mark, Frostbite`) in
    another order: explained with the party's curses above.

## Retired tests

Intermediate-state tests against the translation go once the oracle
covers what they checked; the diagnostic value (where a difference starts)
goes with them.

- `mods.rs` (`mod_queries`, corpus): the typed mod queries on every corpus
  build's stores. Every output, breakdown and Calcs cell runs on them:
  `build_output.rs` (every corpus build, `MAIN` and `CALCS`, bitwise),
  `oracle_variants.rs` (the variant builds) and the session test's calcs
  phase (every cell's breakdown), all against the oracle.
- `calc_setup.rs` (`base_dbs`, `initial_node_mods`, `items_setup`,
  corpus): the setup slices of `initEnv`, which every output stands on;
  the same coverage, plus the session's item and config edits
  recalculating after each change.
- `items_tab.rs`, `skills.rs` (corpus): the Items and Skills tabs after
  loading; the session test's items and skills views and saves, after the
  load and after random edits, on every corpus build.
- `config.rs` `config_matches_upstream` (corpus): the session test's config
  phase (views and outputs after option edits) and `build_output.rs`.
- `config.rs` `every_option_matches_upstream`: the mods each
  Configuration option builds for a few values each, against the
  translation's. `oracle_variants.rs` sets every option to the same values
  (checks on, each list entry, 3, 12 and -2) in a corpus build and
  compares the whole build with the oracle (1231 builds).
- `perform.rs` (7 tests): `calcs.perform`'s state at the defences (the
  player's output, every database's mods, conditions and multipliers), on
  the corpus and the variant families. The corpus as whole builds in
  `build_output.rs`; skill functions, triggers (a sample in CI, every one
  in the nightly), mods, tattoos, timeless jewels and mirages as whole
  builds (`MAIN` and `CALCS`) in `oracle_variants.rs`, live oracle.
- `breakdown.rs` (4 tests): the Calcs tab's breakdowns. The corpus in the
  session test's calcs phase (every cell's `breakdown` command); the
  breakdown-writing skill functions, the mirages and the Calcs inputs in
  its `variants` phase, against the oracle.

## Moved to the oracle

Tests of typed structures against upstream's tables read them from the
oracle instead of the translation. `oracle/values.lua` makes the tables on
upstream on LuaJIT and encodes them with what a Lua value has (number and
string keys, sharing, cycles, functions as `<file>:<line>`), and
`equivalence::lua_json` reads them back into Lua values, so the tests keep
their comparison code (`from_lua`, the mod `Converter`). Entries come in a
fixed order, number keys ascending, then strings, since LuaJIT's `pairs`
order over strings changes from run to run.

- `item.rs`: `new("Item"):Item(raw, rarity, true)` for every unique and
  rare template, and every item of every corpus build loaded from XML;
  same comparison as before, nothing canonicalised.
- `tree.rs`: `main:LoadTree` for each version. Upstream builds the node
  list, the connectors and the class notables in `pairs` order over
  `tree.nodes`, whose string key `root` makes it follow the string hash:
  nodes compare by id, connectors undirected, class notables sorted.
- `game_data.rs`: `from_lua` over the oracle's `data` (with the skills'
  stat map flags as after their first calculation) against `from_lua` over
  the importer's `data`, the typed data's source, rebuilt in the same
  order (`lua_json::canonical`), since the typed data keeps string-keyed
  lists (item mods and bases, record fields) in the data files' order and
  upstream in hash order. A mod table a gem shares with its transfigured
  variants takes `Skill:<id>` from whichever `pairs(data.skills)` visits
  last, so shared tables get the first id on both sides. What the typed
  data keeps in `pairs` order over number keys, the same in every run
  (`next(levels)`, pantheon soul mods, flavour texts), is compared with
  the oracle's order directly. The binary form's round trip stays as it
  was.
- `calculator.rs`: the same overrides per corpus build, sent by id
  (`oracleCalculator`: nodes of `build.spec.nodes`, items of
  `itemsTab.items`) to `calcsTab.miscCalculator` on the oracle; the base
  and each override's player and minion outputs compare bitwise, as
  before. A build that differs is asked of a fresh oracle (upstream keeps
  changed skill data within a process); one corpus build (gen-0018)
  needs it.
