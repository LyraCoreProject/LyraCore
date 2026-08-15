# Vanilla data ingestion — strategy & licensing

**Status:** the **strategy and licensing reasoning** are current and load-bearing. The
implementation-status statements are not — this began life as a planning document.

> **Correction (2026-08-04).** The title and opening paragraph used to say "planning, not yet
> implemented" and "no importer is built yet". **The importer is built and is the canonical way
> content reaches the databases**: `importer/` reads client DBCs through `wow-mpq` + `wow_dbc`, and
> a full starting zone (2,200+ spawns, 420 quests) has been built with it end to end. Everything
> below about the **two pipelines**, the **licensing firewall** (ship the extractor, never the data),
> and the schema decisions remains exactly the design that was implemented. See
> [`danger-zones.md`](./danger-zones.md) §2 for the traps a content import can spring.

Exact collision geometry has an additional shard-ownership and live-acceptance contract: see
[`vmap-rollout.md`](./vmap-rollout.md). Its importer preflight admits map 0 only on shards that own
both its terrain and nav data; importing never enables collision consumption.

How real vanilla 1.12.1 content reaches the SpacetimeDB backend. Captured from the 2026-06-17
data-research pass so the implementation carries the reasoning, not just the conclusion.

> ### Two worlds: the fixture, and the one you build
>
> Two things are easy to conflate. The **seeded fixture** — the realm row, start positions,
> graveyards, a test account and a small demo population — is written by the module's `init` reducer,
> ships in this repository, and is what a fresh checkout gives you. It needs no client, no MPQ or DBC
> extraction, and no third-party world database.
>
> **An imported world is a different thing, and you build it yourself.** The whole pipeline ships:
> the importer binary, the opt-in dump puller with its pinned commit and checksum, the per-family
> import manifest, and the ETL that sequences the loads and asserts the result
> (`importer/scripts/`). `lyracore import` drives it end to end behind a consent interstitial. What
> does **not** ship, and never will, is the data.
>
> #### The firewall, in user-facing terms
>
> **We ship the tools. You fetch the data. No database produced by this pipeline is ever
> distributed or hosted by this project.** Concretely:
>
> - The world-database dump comes from **cmangos' own public repository** (`classic-db`, GPL-3.0),
>   pulled by a script *you* run, to *your* disk. This project does not mirror it, vendor it, or
>   include a single row of it — no bulk table (creature templates, quests, loot, or the world's
>   scale) ships pre-populated. `.import/` is gitignored, and the pull script never *executes* any
>   cmangos SQL — it only assembles the dump for the importer to parse. (The hand-authored seeded
>   fixture is a narrower, disclosed exception — see the carve-out below.)
> - The client-side half — spells, talents, areas, factions, terrain, model geometry — is read out
>   of **your own legally obtained 1.12.1 client's `Data/` archives**. We distribute no client and
>   no client assets, and the importer only ever reads them.
> - `lyracore import` states all of this and requires an explicit `yes` (or `--accept`) before it
>   fetches or reads anything.
> - What comes out is a database on your machine. Publishing, hosting or redistributing *that* is
>   your decision and your responsibility; it is not something this project does or endorses.
>
> Provenance is recorded rather than assumed: `--source-sha` and `game_import_meta` stamps say
> exactly what was loaded, from which upstream commit.

## The core reframe: two separate problems

Mangos solves "vanilla data" with **two distinct pipelines**, and conflating them is the main
confusion to avoid:

1. **Client-derived data** — the ~138 DBC tables + terrain/maps/vmaps/mmaps. **No mangos project
   redistributes these.** The server admin *extracts* them locally from their own legally-owned
   1.12.1 client MPQs using extractor binaries. The 1.12 client already reads its own DBCs; the
   server needs only a small subset for gameplay it computes itself.
2. **Server-side world content** — `creature_template`, spawns, `item_template`, the loot-template
   family, quests, `npc_text`/vendor/trainer, `playercreateinfo`. This is the big lift, shipped as a
   curated SQL dump in a *separate* DB repo (cmangos `classic-db`), hand-authored + packet-sniff
   corrected over ~15 years. **Not in the client.**

## What OUR server actually needs

- **From DBCs (the server-computed subset):** spells, talents, areas, faction-template relations,
  creature displays/scales/model bounds, map and start-position data, stat curves, and the complete
  taxi catalogue. `TaxiNodes.dbc` supplies each node's map position, name, and faction mount
  displays; its DBC id becomes both the storage id and the bounded client/wire id. `TaxiPath.dbc`
  supplies directed endpoints and fares; `TaxiPathNode.dbc` supplies ordered geometry, raw signed
  `int32` flag containers, and non-negative signed `int32` delays.
- **Server-side world content (the road from "one chicken" to a real world):** creature templates +
  spawns + waypoints, items, gameobjects + spawns, the loot tables, quests + relations, npc/gossip
  text, vendors, trainers, `playercreateinfo_spell`, `player_levelstats`/`player_classlevelstats`
  (the per-level HP/mana curve our hardcoded `health:60` currently fakes), faction/reputation.

## Our schema is already a mangos subset

`game_*` is a deliberate, near-1:1 **denormalized subset** of the mangos shape, so transcribing is
mechanical, not a redesign. The seed already uses real cmangos ClassicDB values (Chicken entry 620,
ModelId1 304, BaseAttackTime; Human/Warrior start `(-8949.95, -132.493, 83.5312)` matches
`playercreateinfo` exactly) — proof the "copy the values" path produces client-correct data.

Clean mappings today: `game_creature_template` ← `creature_template`; `game_creature_spawn` ←
`creature`; `game_creature_waypoint` ← `creature_movement`; `game_start_position` ←
`playercreateinfo`. Gaps (future tables, not blockers): `item_template`, gameobjects, the
*_loot_template family (collapsible into ONE `game_loot_template` keyed by `(loot_type, entry)` —
all 11 mangos loot tables share the same shape), quests, npc/gossip text, vendors/trainers,
level-stats, starting spells, faction/reputation.

## SpacetimeDB strengths / tradeoffs as a world DB

**Strengths:** kill→loot-roll→grant is ONE ACID transaction (no ORM, no cache-coherency layer);
live subscriptions already *are* the visibility/broadcast layer (a creature is "just a
`game_world_entity` row"); content versions independently of code (re-import, not recompile); one
language (Rust) end-to-end reusing the gtker crate family (incl. `wow_dbc`); our schema is already a
subset so growth is append-only.

**Tradeoffs:** full-world volume is unproven for us (~1.4 GB MySQL as a size proxy); reducer
arg-size/exec limits **forbid streaming the whole dump through one call** → import MUST be externally
chunked/batched via a privileged admin/gateway SDK path; **seed-as-code (`seed.rs`) does NOT scale**
(~10k creatures / ~66k spawns / ~17k items / ~170k loot rows would wreck compile time + bloat the
wasm) → need a **data-bundle loader**, content versioned separately from code; migrations are
append-only (never reshape); we'd own an ETL + validation that mangos has 15 yrs of tooling for;
mangos column *semantics* don't always map 1:1 (e.g. multiplier-based stats vs our flat values).

**Republish & data retention (re-verified 2026-06-17, Slice 2):** a plain `spacetime publish` (no
`-c`) is **never destructive** — it either auto-migrates while RETAINING all data, or it *aborts*
with a clear error. It never silently wipes. `-c/--delete-data` is the *only* destructive path and
is explicit opt-in. The auto-migrate rule is narrower than earlier notes claimed:

- **Logic-only republishes** (new/changed reducers, new tables/indexes) auto-migrate trivially.
  Confirmed live: kept the player's level/XP intact.
- **Adding a column auto-migrates ONLY IF** (a) the new field carries a **default-value annotation**
  *and* (b) it is **appended at the END of the struct**. Inserting a column mid-struct triggers
  `Reordering table <t> requires a manual migration` and the publish **aborts** (no wipe). A column
  without a default likewise aborts with `Adding a column <c> ... requires a default value annotation`.
- When a publish aborts, the data is untouched; you then either fix the struct (default + append) or
  accept a `-c` reseed.

Slice 2 added `dynamic_flags`/`dead` to `game_world_entity` mid-struct → plain publish aborted
safely → we chose a `-c` reseed (level-3 was throwaway test data). So: content re-imports and
*end-appended, defaulted* schema growth do **not** wipe player data; mid-struct column inserts need a
manual migration or `-c`. (The earlier note here that called column-adds unconditionally safe was
wrong — corrected after this deploy.)

## Licensing — the decisive constraint (NOT technical)

cmangos `classic-db` is **GPLv3** (the mangos *core* is GPLv2; same logic applies). Its **content**
is asserted to remain Blizzard's, offered only as non-commercial "fair-use demo content."

**GPL impact, by scenario:**
- **Trigger is distribution, not use.** Private import/transform/run with no redistribution → **zero**
  copyleft obligation.
- **A. Hybrid / private (recommended): GPL impact ≈ none** — we distribute none of their data.
- **B. If we ship their data:** GPL attaches to the artifact (must be GPL-licensed, must offer
  source; no permissive/proprietary relicensing). **Copyleft-spread risk:** a separate runtime-loaded
  file is arguably mere aggregation (our code stays ours), but **compiling the data into the module**
  risks the whole module being a derivative → GPL on our code. So: never compile third-party data in.
- **C. Hosted-only SaaS:** GPLv3 has **no network clause** (unlike AGPL) → not "conveying" → no
  distribution obligation. But the Blizzard layer remains.
- Commercial use is *allowed* by GPL (you can sell it); it just forbids making it proprietary.

**The bigger, independent risk is Blizzard.** GPL is only cmangos's wrapper and can't grant rights to
Blizzard's underlying copyright. For any public/commercial future, **Blizzard — not GPL — gates.**
Same instinct as our `game_`-not-`wow_` naming discipline.

## Recommended approach: hybrid with a hard licensing firewall

Ship **zero** third-party-derived data. Three streams, each with its own provenance story:
1. **Client-owned constants via `wow_dbc`** (gtker, same vendor as our deps): read the few
   DBC-derived values we compute server-side from the **user's own** client MPQs at build/setup time.
   Cleanest provenance, never redistributed; replaces today's hardcoded `49`/`304`.
2. **Ship a tiny, wholly-ours curated starter fixture** (Elwynn / map 0 first): small enough to
   hand-author, real out of the box, zero third-party exposure. Adopt mangos table *shapes* so the
   schema is a recognized superset (append-only).
3. **Optional user-run ETL for breadth** — an operator points it at *their own* classic-db copy; it
   emits a neutral bundle the module loads. We ship the **extractor, not the data** — exactly how
   mangos ships extractors, not Blizzard MPQs.

## Decisions taken (2026-06-17)

- **Mechanism: an explicit opt-in pull-script**, not a git submodule. Same licensing posture (the user
  fetches classic-db from cmangos and transforms locally; we ship the script + our own ETL, never
  their data), but nothing auto-fetches Blizzard-derived content for a casual cloner and the
  association stays out of our default repo graph.
- **Source tracking: latest `master`**, to benefit from cmangos's ongoing data fixes — NOT pinned by
  default. Master-vs-pin is purely freshness-vs-reproducibility; it does **not** change the firewall.
- **Record the resolved commit SHA per import** (in the generated bundle / a small lockfile). Keeps
  master reproducible-after-the-fact and makes "pin if unstable" a one-line change to the recorded
  SHA. (Floating-ref + lock pattern.)
- **The ETL parses the data we use and validates + fails loudly on schema drift** — it never executes
  cmangos's SQL/scripts (that would be running GPL code) and never silently imports a changed shape.
- **Resolved (2026-06-17): the repo WILL be published eventually.** Blizzard intervention is treated
  as an accepted, inherent risk of the emulation space. This makes the firewall **mandatory, not
  optional**: a public repo must ship ZERO third-party/Blizzard-derived data — only our own code, the
  opt-in pull-script, and our hand-authored starter fixture; our own code stays permissively
  licensable. Still deferrable to importer-build time: scale target (starter zone vs whole world) and
  the client-DBC extraction UX.

## Phased roadmap (tracer-bullet)

- **P0 — Decide the firewall + lock table shapes** (no data movement). Record the licensing decision;
  lock mangos-mirrored shapes (migrations are append-only).
- **P1 — Per-(race,class) start data from the user's own client** (smallest real win): stand up the
  `wow_dbc` path; replace hardcoded start coords + `display_id` with extracted values; all races/
  classes start correctly.
- **P2 — Elwynn creature_template + spawn slice** (map 0): "one chicken" → a living starter zone;
  validates the import mechanism at small scale.
- **P3 — Level-stat curve + starting spells**: add `game_player_levelstats`/`_classlevelstats` +
  `playercreateinfo_spell`; kills the hardcoded `health:60` fiction.
- **P4 — Loot + items**: collapsed `game_loot_template` + `game_item_template` (only the columns we
  use); first content that exercises the transactional kill→roll→grant strength.
- **P5 — Quests** (+ the npc/gossip relations they need): densest behavioral content; depends on
  creatures/items/gossip.

**Per-shard scope.** Once the world is sharded by continent, the ETL's unit of work is one map into
one database — the importer takes the target database, the map id, and a bounding box, and its map
fence plus the surrounding pre- and post-flight guards keep any one database from holding two
continents' spatial rows. Non-spatial content (spells, items, quests, the DBC reference tables) is
duplicated on every shard by design.

The standalone DBC pass reads all three taxi files through the same in-memory MPQ patch chain as
the other client tables. It validates every endpoint/path reference and every `(path, node_index)`
ordinal, client id against the vanilla 1..=256 mask range, reserved wire-id collisions, and negative
delays before issuing SQL. Apply is a recoverable clear-and-reload: points are cleared before paths
and nodes, then imported rows and the reserved fixture are inserted deterministically. A database
failure during those statements can leave a partial family, but `import-world.sh` now stops on that
failure and the next run clears the family again before rebuilding it. No DBC bytes or generated
catalogue artifact is written into the repository.

## What the seeded fixture knowingly contains

The firewall above is about **bulk data**: this project never vendors, mirrors, or ships a
world-database dump, and `init` never reads one. It is not a promise that every id in the seeded
fixture is invented. The 1.12.1 client renders items, spells, and NPCs by numeric id against its
**own** local DBCs and item cache — a hand-authored fixture using fake ids would either show blank
tooltips or, worse, silently collide with a real id a later import introduces. So the fixture
deliberately reuses real vanilla ids and, for a handful of rows, real names and coordinates:

- **Items** — a few dozen entries at real ids so their names/icons/tooltips render correctly (e.g.
  25 Worn Shortsword, 6948 Hearthstone, 769 Chunk of Boar Meat, 2318 Light Leather), enumerated in
  `module/src/seed.rs`'s starter-kit and profession-item blocks.
- **Spells** — around a dozen real ids used by the starter kit and profession recipes (e.g. 772
  Rend, 133 Fireball).
- **Gameobjects** — the Goldshire-area gather-node demo, entries 1731/1732 (Copper/Tin Vein).
- **Graveyards** — five real Elwynn/Westfall world-safe-locations, with their real names and
  coordinates (`module/src/seed.rs`, mirrored as the fallback constants in `module/src/world.rs`).
- **The start position** — the real Northshire Valley coordinates a fresh Human Warrior spawns at.
- **Taxi harness** — two reserved nodes, one one-way test route with three hand-authored points and
  fare, and a nearby flight master. The 5090000+ values are storage ids; separate wire ids 255/256
  fit the client's fixed taxi mask and are rejected if imported DBC nodes already claim them. These
  synthetic nodes exist for headless protocol tests. A real client can only render flight-map nodes
  present in its own DBCs, so visual real-client testing uses the imported catalogue instead. Taxi
  activation charges the path's imported copper fare and selects the source node's Horde/Alliance
  mount display from the character's race team; a zero display is rejected safely.
- **A handful of quest/NPC display strings** exercised by the gateway's own test fixtures.

None of these came from a bulk import: they are hand-typed, one row at a time, using ids,
coordinates, and (where the numbers needed to be authentic) magnitudes that are facts about the
shipped 1.12.1 client and the public game world, not rows copied out of a third-party database.
What never ships, and never will by default, is content at the *scale* an actual `classic-db`
import produces — hundreds of creature templates, quests, or loot rows. That only exists on a
machine where **you** ran `./lyracore import` against **your own** client and **your own** fetched
dump.

## Starter aura families — provenance (reconciled 2026-08-13)

`module/src/seed.rs`'s `seed_spell_groups` hand-authors the mutually exclusive aura families. This
records what its rows were checked against, because "hand-authored" is not the same as "unverified"
and the two must not be conflated.

**Reference source.** `DBFilesClient/Spell.dbc`, read out of a locally owned 1.12.1 client install
(`Data/dbc.MPQ` + `patch*.MPQ` patch chain, build 5875), through the same `wow-mpq` + `wow_dbc` read
path `importer/src/dbc.rs` already uses. **Revision:** the client's own shipped DBC — there is no
upstream revision to pin beyond the build number, and `wow_dbc` hard-asserts the 5875 record shape, so
a wrong-version client fails the read rather than producing silent drift. The probe was a throwaway
example, run once, deleted after; it printed derived facts (id, name, rank subtext, base points) and
wrote nothing. **No client bytes entered the repository tree**, matching the firewall above.

**Not used, deliberately:** cmangos `spell_group` / `spell_group_stack_rules`. That dataset would
answer the rule questions directly, but it is GPL-licensed content this project ships zero rows of, and
transcribing its group assignments into a permissively licensed repository is exactly the copy the
firewall forbids. Operators who run the importer against their own dump get it locally; the shipped
starter set does not.

**Verified against the client (membership):** every id in all nine families, by spell name and rank
subtext. Corrections this produced:

| Family | Correction |
|---|---|
| Mark of the Wild | Rank 4 (`5234`) was missing from an otherwise complete 7-rank chain. |
| Paladin Blessings | Added Might rank 7 (`25291`), Wisdom rank 6 (`25290`), Sanctuary ranks 2-4 (`20912`/`20913`/`20914`), and the eight Greater Blessings (a Greater Blessing is the same buff as its single-target form). |
| Armor debuffs | Added Faerie Fire (Feral) ranks 1-4 (`16857`/`17390`/`17391`/`17392`) — the druid form-shifted cast of the same debuff. |
| Well Fed | Added the real food-buff ids alongside the sandbox's synthetic `50116`. |
| Fortitude, Battle Shout, Intellect, Spirit, Shadow Protection | No membership change — the existing ids matched the client exactly. |

**Verified by reasoning over the client's own magnitudes (comparability):** `rank_is_comparable` was
`true` for eight of nine families and is now `true` only for Battle Shout, the sole family built from a
single rank chain. The client makes the bug concrete: Prayer of Fortitude rank 1 grants the same
stamina as Power Word: Fortitude rank 5, and rank 2 the same as rank 6, so comparing the two chains'
rank numbers would have refused a correct Prayer of Fortitude rank 2 in favour of a Power Word rank 6
of identical strength. Gift of the Wild ranks 1-2 sit at Mark of the Wild ranks 6-7 the same way. Those
families now compare effect magnitude, with an existing aura's stack count folded in.

**Unverified, and marked as such:**

- **Rule assignment per family.** `Spell.dbc` does not encode aura exclusivity; that lives in
  server-side data. The assignments (EXCLUSIVE_STRONGER everywhere except per-caster Blessings and
  latest-wins Well Fed) rest on observed 1.12.1 behaviour, not on a reference table.
- **Whether Blessing of Freedom, Protection, and Sacrifice share the Blessing exclusivity.** They are
  omitted rather than guessed at.
- **Expose Armor's magnitude**, which the client stores as a combo-point coefficient rather than a flat
  value. The policy tests use its 5-combo-point value.

**How the families are checked on a live database:**
[`aura-stacking-probes.md`](./aura-stacking-probes.md) — replacement, refusal, per-caster Blessings,
and persisted `game_aura` rows, through the debug reducers on a development database.

## Where these questions landed

The open questions this document originally posed have all been answered, and the answers are the
reason the firewall above is mandatory rather than advisory:

- **The red line** is the strict one: ship ZERO third-party-derived data. No separately-licensed data
  pack, optional or otherwise.
- **The project is public**, so the Blizzard fair-use calculus — not the GPL — is the dominant risk,
  and it is treated as an accepted, inherent risk of the space.
- **Out of the box, the default is the hand-authored fixture.** Breadth is for operators who opt into
  the importer with their own data.
- **Client extraction happens through `wow_dbc`**, reading the operator's own client, rather than
  keeping hardcoded constants.
- **Scale target: a polished starting zone first**, not the whole vanilla world.
- **When importing, the reference source** is cmangos `classic-db` (5875-exact, ~13 MB of SQL) rather
  than the sniff-heavy, patch-ranged alternatives.

## Sources

cmangos `classic-db` (GPLv3, ClassicDB_1_12_1), mangos-classic core (GPLv2), and gtker `wow_dbc`
(a Rust 1.12 DBC reader, same family as `wow_world_messages` / `wow_srp`). The general lesson from
the codec work applies to sourcing data values too: prefer a known-good fixture as ground truth over
a value reasoned out from documentation.
