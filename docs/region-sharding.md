# Region sharding — the seam menu, the assignment table, and how routing uses them

**Status: RETIRED 2026-08-08 —
[#471](https://github.com/LyraCoreProject/LyraCore/issues/471) removed the region tier from the
codebase for the alpha.** Everything below this banner is the historical design, kept for its
reasoning; none of it describes the running system anymore.

> **What was removed (#471):** `LYRACORE_REGION_SHARDS` and the gateway's region-shard connections,
> the region overlay in routing (`resolve_region_shard`, `region_db_for`, the cell-keyed resolver,
> and the region arms of `home_shard`/`settle_home_shard`), seam-crossing detection on the movement
> path, the cross-shard seam chat/emote relay, the `game_map_region`/`game_region_assignment`
> coordinator subscriptions, and per-region load sampling (the `regions=` gauge).
>
> **What the realm keeps** — the broad splits are unchanged: the continent shard map
> (`LYRACORE_SHARD_MAP`), the instance pool, realm-core (`LYRACORE_REALM_CORE`), the escrowed
> cross-database transfer (#16/#19, now driven by world ports rather than mid-walk seam crossings),
> and the #468 shared AOI index.
>
> **The module schema is untouched:** `game_map_region` and `game_region_assignment` (and their
> import reducers) stay in the module, unused — dropping a table is a destructive migration, which
> is exactly what `docs/danger-zones.md` forbids.

This was the foundation of the elastic-world-sharding
design. This document is the **content-data format** for pre-drawn seams, the operating procedure for
activating one, and the routing and visibility behaviour built on top.

> **Correction (2026-08-04).** This introduction previously said seam *mechanics* — view-merge,
> warm handoff, live region migration — were "NOT built" and gated on a capacity benchmark. That was
> true when it was written and is now contradicted by the rest of this very document. Current state:
>
> | Mechanic | State |
> |---|---|
> | Cell / region / shard hierarchy as data | built |
> | Region routing at world entry | built |
> | **Warm handoff** (a resident crosses a live seam mid-session, no loading screen) | **built, default on** (`LYRACORE_WARM_HANDOFF`), verified by a maintainer on a real client |
> | **View merge** (seeing peers across a seam) | **always on, and no longer a mechanism**: #468 replaced the per-player AOI subscriptions with ONE shared in-process cell index fed by every shard's coordinator stream, so a peer across a seam is just a peer. The §"View-merge across a seam" section below describes the RETIRED per-player implementation (`LYRACORE_VIEW_MERGE`, `split_box_by_shard`, the away-connection tier) and is kept for its reasoning. |
> | Chat and emotes across a seam | **built** |
> | Buffs / intents, melee & trade co-location | not built; AoE clips at the seam, by spec |
> | Bulk live region migration | not built — activation still retires the non-owner's creature population by hand |
> | Load sampling to decide *when* to draw a seam | built |
>
> The capacity benchmark that gated all of it has been run: 200 dispersed players at 50.5% writer
> occupancy, 200 co-located at 21.1% with p50 8 ms and no cliff. The current honest ceiling is the
> gateway's crowd egress, not the writer.
>
> **Planned direction:** the alpha realm targets at most one shard per zone, with capitals on
> separate shards, and per-zone seam menus generated from terrain area data. That work is not built
> yet; the format and procedure below are what it will generate *into*.

## The hierarchy

| Rung | What it is | Where it lives | Who reads it |
|---|---|---|---|
| **Cell** | 50yd square, `GRID_CELL_SIZE` | baked into every spatial row as `grid_x`/`grid_y` | module + gateway |
| **Region** | a contiguous set of cells, floor 10×10 | `game_map_region`, imported content data | gateway routing |
| **Shard** | a SpacetimeDB database | `game_region_assignment` on realm-core | gateway routing ONLY |

`region_of(map_id, cell)` is a **baked lookup** (`lyracore_shared::region::RegionMap::region_of`), not a
computation scattered through game code, and **no module game logic ever reads a shard id** — a
tripwire test (`partition_discipline_tripwire::no_module_game_logic_reads_a_shard_id`) fails the
build if it does. A shard is a database name; that is a gateway routing fact, and keeping it out of
the module is what lets the same wasm run on every shard and lets a region move without a code
change.

## The seam menu (content data)

A **seam menu** is a zone's pre-drawn candidate regions. Drawing one costs nothing: with every region
of a map assigned to the same shard — or with no assignment at all, the default — the seams are
inert and routing is byte-identical to the single-database gateway.

One region per line (or per `;`), `#` starts a comment:

```text
<map_id>:<region_id> = <gx_min>..<gx_max>, <gy_min>..<gy_max>
```

* `map_id` — the map the region is on (0 = Eastern Kingdoms).
* `region_id` — unique per map. **Region 0 is reserved**: it means "the rest of the map", is never
  assignable, and always routes through the ordinary `(map_id, instance_id)` shard map.
* the two ranges — an **inclusive** cell rectangle, in `grid_x`/`grid_y` cell indices
  (`lyracore_shared::spatial::grid_cell`, i.e. `floor((17066.666 - x) / 50)`; note cell indices grow as
  the world coordinate shrinks).

### Worked example — Elwynn Forest

```text
# Elwynn Forest (map 0) seam menu — three candidate regions, all dormant.
# Cell indices from grid_cell(): Goldshire (-9461, 60) is cell (530, 340),
# Northshire (-8950, -132) is cell (520, 343).
0:1 = 510..529, 330..349   # Northshire valley + the abbey approach   (20×20 cells = 1000×1000yd)
0:2 = 530..549, 330..349   # Goldshire basin + Crystal Lake shore
0:3 = 510..549, 350..369   # Southern Elwynn — Brackwell / Tower of Azora
```

Everything outside those rectangles is region 0 and routes exactly as it does today.

### The shipped fixture menu

The worked example above is illustrative. What actually ships, and what `lyracore dev up` imports
by default, is [`content/regions/fixture.regions`](../content/regions/fixture.regions) — **two**
regions, not three, and drawn only from coordinates already committed in this repository (an
earlier draft that guessed a Stormwind City box the repo holds no data for was replaced; see
issue #327's comments):

```text
0:1 = 512..524, 336..350   # Northshire Valley + the abbey approach
0:2 = 525..549, 330..369   # the rest of Elwynn — Goldshire basin, Crystal Lake, Eastvale road
```

**Region 1 — Northshire Valley.** World x `(-9183, -8533]`, y `(-483, 267]`. This is where `init`
seeds the fixture's entire spatial content (a handful of creatures and gameobjects, all within
~15 yd of cell `(520, 343)`, `module/src/seed.rs`) and where a new character spawns. It stays on
the **default** database, so a fresh login never begins with a handoff.

**Region 2 — the rest of Elwynn**, including Goldshire and its class trainers, moves to a second
database. Region 1 ends at `gx` 524 and region 2 starts at `gx` 525, so the two rectangles are
disjoint along a single line of constant world x (≈ -9183.3) and touch without overlapping —
`RegionMap::build` rejects overlap outright, so a successful parse is itself the proof. Leaving
Northshire Valley for Goldshire lowers world x, raises `gx`, and crosses 524 → 525: that is the
seam every new character walks across (docs/quickstart.md's "go and cross it" walk).

⚠ **The fixture has no content in region 2** until the contributor runs `./lyracore import` (which
needs a cmangos dump and their own client MPQs). Out of the box the crossing is real and announces
itself; the far side is empty ground. The full annotated derivation — cell math, every source
coordinate, and the activation runbook (`import_map_regions` on every world shard,
`set_region_assignment` on realm-core, `LYRACORE_REGION_SHARDS`) — is in the file's own header
comments; it is the same file, so it cannot drift from what actually ships.

### Rules the importer enforces

A row that breaks any of these is **rejected**, and its cells fall back to region 0 — a bad menu can
only ever collapse toward today's behavior, never toward a wrong route.

* **Floor: 10×10 cells** (`MIN_REGION_SIDE_CELLS`, 500×500yd). The ~100yd interaction radius is 2
  cells, so a region much smaller than this is *all seam*: every interaction inside it would cross a
  shard boundary. The spec sets this floor; do not work around it.
* **No overlap** between two regions on the same map. With overlap, which region a player is in — and
  therefore which database they land on — would depend on the order rows arrive out of a
  subscription.
* **No duplicate `(map_id, region_id)`**, no inverted bounds, no region id 0.
* Regions are **axis-aligned rectangles**. A rectangle is trivially contiguous and legible. If a zone
  needs an L shape, draw two rectangles and assign both to the same shard — *a seam between
  same-shard regions costs nothing*, which is the whole point of the design.

### Importing a menu

The whole realm's menu is one payload; the import replaces everything (a few dozen lines, well
inside one call argument). Run it against each **world** shard, alongside the world ETL that bakes
`grid_x`/`grid_y`:

```bash
spacetime call lyracore import_map_regions "$(cat elwynn.regions)"
```

## The assignment table (ops data)

```
region_assignment { map_id, region_id, shard, epoch }
```

Authoritative on **realm-core** (the same wasm published under another database name — see
`module/src/realm_core.rs`), and subscribed by every gateway. `shard` is a database name; `epoch` is
a monotonic version stamp per `(map_id, region_id)`.

```bash
# Activate: region 2 of map 0 moves to the pool-b database, at epoch 1.
spacetime call realm-core set_region_assignment 0 2 pool-b 1
# Flip back: an EMPTY shard name un-assigns the region (it falls back to the shard map).
spacetime call realm-core set_region_assignment 0 2 "" 2
```

**Creatures are single-owner: activation must retire the non-owner's population.** Regions
are region-static for creatures by design, and a hand-bootstrapped region shard (imported box)
leaves BOTH databases holding live copies of the same guids — clients swapping subscription sets at
the seam then receive CREATEs for units they already render (observed live as level-up-style
artifacts), and the non-owner pays creature tick for ground it does not own. After assigning a
region, retire the population everyone else still holds — keyed by SPAWN home, so wanderers belong
to the region of their spawn point:

```bash
# The new owner keeps its copy; every OTHER world shard holding spawns in the box runs:
spacetime call <non-owner-db> debug_retire_region_creatures <map_id> <gx_min> <gx_max> <gy_min> <gy_max>
# And if the owner's imported box is WIDER than the region, retire its out-of-region surplus too
# (decompose the complement into rectangles). Verify disjointness: the same guid must never return
# rows from two databases' game_world_entity.
```

Recoverable only by re-running the world-import ETL (maintainer tooling, not shipped in this
repository) — the reducer is `debug_reducers`-gated and operator-gated on purpose. A planned bulk
region-migration engine will subsume this manual step; it is not built yet.

**Epoch semantics.**

* A flip must carry a **strictly higher** epoch than the row it replaces; equal or lower is refused
  with `stale epoch …`. A retried or reordered operator call can therefore never resurrect a
  superseded assignment.
* An un-assign keeps the row as a **tombstone** (empty shard) at the new epoch rather than deleting
  it. Deleting would drop the epoch high-water mark with the row, and the next stale retry
  (`set_region_assignment 0 2 pool-b 1`, arriving after the un-assign at epoch 2) would be accepted
  against `current = None` and put the region straight back on `pool-b` — the exact resurrection the
  rule above forbids. A tombstone routes identically to a missing row (an empty shard name is
  "no region opinion"), so the only thing it changes is that the ordering survives.
* Where the gateway can see more than one row for a region (a delete/insert pair applied in either
  order), the **highest epoch wins**.
* **A flip re-routes new entrants at world entry, and residents at their next seam crossing.**
  The gateway resolves region → shard once per world entry (login, and the re-entry after a world
  port). A session already in the world keeps its pinned shard handle until it *moves* — at which
  point **warm handoff** (built, default on) re-resolves and migrates it mid-session with no loading
  screen. A resident who never moves out of the reassigned region is never asked again, so a flip
  under a stationary crowd still drains on their next login. ⚠ A flip does **not** move the region's
  creature population: see the `debug_retire_region_creatures` note above, and the planned bulk
  migration that will subsume it.

### The shard has to be CONNECTED — `LYRACORE_REGION_SHARDS`

`resolve_region_shard` refuses an assignment naming a database the gateway never reached (it falls
through to the shard map instead, which is always somewhere it *is* talking to). The connected world
set is `ShardMap::shards()`, so a region's target must be in it.

⚠ **A shard-map rule is the wrong way to get it there, and this is not obvious.** `shards()` is the
default database plus every rule's `db`, so before `LYRACORE_REGION_SHARDS` existed the only way to connect
a region shard was to give it a rule — and every rule routes a whole map (or map+bucket) to it. For a
map-0 region shard that means `0:*=<db>`, which hands it **every map-0 location the seam menu does not
cover** — all of Eastern Kingdoms outside the drawn regions — on a database holding none of that
content. Regions exist to take that decision *instead of* the map rule.

So declare region targets separately. Comma/semicolon/newline separated, `#` comments:

```bash
LYRACORE_REGION_SHARDS="lyracore-world-2"
```

These are ordinary world shards — they hold characters and a character-location probe walks them — but
**no rule routes to them**, so `resolve` can never return one and `check_instance_hosting` never
treats one as an instance owner. The region overlay is their only way in.

Refused, with the reason logged, and collapsing to "unconfigured":

* the **default** database, or anything a rule already names — already connected, and `shards()` must
  not list a database twice
* **realm-core** — it owns no characters, so a region assigned there would route a login onto a
  database with nothing of the player's in it

⚠ **The default database stays first in `shards()`**, because `Coordinator::region_db_for` reads the
seam menu from `world_shards().first()`. A region shard sorting ahead of it would read the menu from a
database that has none — and an empty menu makes `resolve_region_shard` answer `None`, so region
routing would quietly switch itself **off** rather than fail. Pinned by
`the_default_stays_first_so_the_seam_menu_is_still_read_from_it`.

A region shard that is unreachable at startup is **not fatal** (only the default is): the gateway
comes up, logs it, and every region naming it degrades to the shard map.

## How routing uses them

At world entry, in `WorldStore::settle_home_shard` (`gateway/src/stdb/world_store.rs:122` — the
production entry point; it overrides the simpler `home_shard` and re-applies the region overlay
after settling, which is the trap its own header documents):

1. The character-locator step settles *where the character is* — realm-core's character→shard index,
   confirmed against the shard that actually holds the row (self-healing on disagreement).
2. **The region overlay** (`config::resolve_region_shard`): definitions from the default world shard
   + assignments from realm-core + the character's position ⇒ the owning database, or `None`.
3. The static shard map's `shard_for(map_id, instance_id)` — the fall-through, unchanged.

Step 2 answers `None`, and step 3 therefore decides, in **every** case except "a seam menu is
imported AND a region containing this point is assigned to a connected world shard". In particular
it answers `None` for: no definitions, no assignment row, an empty shard name (including a
tombstone), a database that is not a connected **world shard** (realm-core holds no characters, so
an assignment naming it must route nobody), a point in region 0, and anything inside an instance —
`instance_id != 0` *or* a non-zero `pending_instance_id`, which is where a character mid-transfer
carries its destination while it has no live entity (instances are their own partition; a
region overlay on top would give one player two answers to "which database am I on").

When step 2 *does* answer, its answer is a database NAME and it decides outright — including when
it names the shard the session is already on, which means "stay", not "let the shard map try". Both
steps resolve their name through the same `Coordinator::shard_handle`, so one assignment cannot
route two ways depending on which handle the entering session happened to hold.

**Backward compatibility is the safety property.** A single-database gateway short-circuits on
`is_sharded()` before any of this runs, and does not even *subscribe* the region tables — a
subscription to a table the deployed module does not have fails to apply, which would brick a
gateway restarted before the module was republished (`coordinator_queries`, and the named tests
beside it).

## View-merge across a seam

Everything above is *routing*: which ONE database a session's writes go to. It has nothing to say
about what a player *sees* — that is `gateway/src/stdb/aoi.rs`'s `AreaOfInterestTracker`, which was
made shard-aware, gated `config::view_merge_enabled` (env `LYRACORE_VIEW_MERGE`, default **on**).
> **RETIRED by #468.** Everything in this section describes the per-player away-connection tier,
> which is gone. Cross-seam visibility is now a property of the shared cell index, not a mechanism
> of its own; the only surviving piece is the cross-shard chat/emote relay (`seam_chat_outbound` /
> `seam_emote_outbound` in `gateway/src/stdb/subscriptions.rs`), which now rides the coordinator's
> own `game_chat_event` / `game_emote_event` subscription instead of an away connection.

**This is a rebuild.** The first attempt sat on top of an earlier per-cell AOI subscription scheme
and resolved every one of a straddling box's up-to-25 cells individually, each a separate handle.
A dedicated measurement pass covered that shape twice over — a per-cell equality query costs more to
REGISTER than a whole box RANGE costs to EVALUATE — and both the per-cell subscriptions and the
view-merge attempt sitting on them were reverted together. That measurement's closing decision is
the law this rebuild follows: **a straddling box splits into per-shard RANGE
sub-boxes, never per-cell queries**, on the cheap box-replace primitive that survived every
measurement (armA: 22.6% occupancy at 3× natural churn).

### The split

Past a session's first recenter (login stays home-only, the same reason it stays box-shaped — see
the module doc at the top of `aoi.rs`), every recenter's new 5×5 box is handed to
`aoi::split_box_by_shard`: a pure boundary-line scan (no per-cell partition map is ever
materialized) that decomposes the box into the FEWEST rectangles that agree with
`config::resolve_region_shard_for_cell` at every cell. Because a region floors at 10×10 cells and the
box is only 5 wide, a box can straddle **at most one boundary per axis** — so the split is always one
of:

- **No split** — the whole box is on one shard. The result is `[(shard, [box_as_one_rect])]`, and
  that one rect's bounds are BYTE-IDENTICAL to `GridBox::bounds()`, so the query text sent is
  identical to the traffic a gateway sent before view merge existed, not merely equivalent.
- **Edge split** — one straight boundary crosses the box: one home rectangle + one away rectangle,
  each spanning the box's full height or width.
- **Corner split** — a region corner sits inside the box (up to 4 quadrants): a shard occupying two
  DIAGONAL quadrants gets exactly 2 disjoint rectangles (an "L" when the quadrants share an edge);
  no shard ever needs more than 2 for a box this size.

Each shard's share — one or two `GridRect`s — is subscribed as ONE handle carrying the SAME 4 range
queries `box_queries` always sent (`rect_queries`: `game_world_entity`, `game_gameobject`,
`game_entity_motion`, `game_creature_spline`), on THAT shard's own per-account connection. A
straddling recenter is still subscribe-new-then-unsubscribe-old-on-applied, exactly like today's
single-box recenter — just run once per shard instead of once for the whole box.

### The away connection never blocks the movement thread

> **Historical, superseded by #483.** `Coordinator::player_conn` and the per-account connection it
> named no longer exist — every gateway↔database connection is a coordinator connection now, and
> away-shard reads ride the shared coordinator caches instead of a dedicated foreign-shard connect.
> The connect-latency problem this subsection solves does not arise under that model. Kept for the
> reasoning trail; do not use it as a description of current behavior.

Opening a foreign shard's per-account connection (`Coordinator::player_conn`) is the one genuinely
slow step in this whole feature — it builds a fresh SDK connection, up to a 20s timeout. The
original attempt opened it synchronously inside the recenter that first needed it, which was raised
as a fast-follow defect: a slow or unreachable away shard would stall THAT player's own
packet processing. The rebuild kicks the connect off on a background thread the first time a shard is
ever named, and polls a shared slot on later recenters; until it resolves, that shard's rectangles
are simply not subscribed THIS recenter — no different from a per-cell subscribe that failed and
gets retried on the next crossing. The degrade is "the far side pops in late", never a stall.

### The away leg's peer-visibility gate

The away connection registers a NARROWER set of relay callbacks than home (`entity`
CREATE/DESTROY/VALUES, gameobject, peer motion, creature legs — never self-owned state: items,
quests, spells, gossip, loot). It feeds the SAME `SessionTx` and the same per-session `created` dedup
set the home registration uses, so a peer seen from either shard renders through one socket and is
never doubled.

The original attempt dropped the spirit-healer/ghost and stealth gates entirely on this leg (raised
as a second fast-follow defect — a stealthed rogue or a spirit healer on the far side of a seam would
render to a straddling viewer). The rebuild closes both:

- **Stealth** needs the CANDIDATE peer's own `A_STEALTH` aura rows, which live on whichever shard the
  candidate is resident on — i.e. the away connection's OWN cache. `register_peer_view_relays`
  subscribes that shard's `game_aura` table globally (same shape as home), so this read is
  same-shard, not cross-shard, and needs no mirroring at all.
- **Spirit-healer ghost-gating** needs the VIEWER's own GHOST player-flag, which lives only on the
  home connection. `ViewerGates` (`aoi.rs`) is the one small piece of shared state: home's existing
  ghost-flag edge-detect (the spirit-healer reveal-on-ghost callback, `subscriptions.rs`) stores the
  latest value in a plain atomic, and every away registration loads it — never a cross-shard read per
  row, never a weakened RLS check (both gates run exactly the predicate home runs, just sourced from
  whichever connection actually holds the data).

**`LYRACORE_VIEW_MERGE=0`** collapses every cell to home — the escape hatch if the away leg misbehaves
live; structurally a no-op on a single-database realm or an un-imported seam menu either way, same
as every other region knob in this document.

**What crosses a seam, as of 2026-08-04.** Movement, health, and target changes cross, because they
ride the same box-scoped tables (`game_world_entity`, `game_entity_motion`, `game_creature_spline`)
region routing already reads. **Chat and emotes now cross too**: `register_peer_view_relays`
subscribes the away shard's `game_chat_event` and `game_emote_event` globally and range-gates them
on the viewer's mirrored position (`SAY_RANGE_SQ = 625.0`, `YELL_RANGE_SQ = 90_000.0`) — this
paragraph previously said they did not, and was stale relative to the code. Still deliberately out
of scope: **buffs/intents** and **melee/trade co-location**. AoE clips at the seam, by
spec.

One counter-intuitive bug fixed live on 2026-08-04, worth knowing because it looks like the opposite
of a sharding bug: a region assigned **to the viewer's own shard by name** used to land its own
coverage in an away bucket that could never resolve — and since the home bucket only picks up
unresolved cells, a box resolving entirely to "my own shard, by name" subscribed **nothing at all**.
The fix is `config::fold_home_shard`, which folds a by-name self-reference back to "home".

### Live acceptance

The wire-protocol test suite — maintainer tooling, run against a live stack and not shipped in this
repository — carries a view-merge case: a straddling observer walks toward a live seam and expects a
CREATE plus a movement relay for a peer staged on the far shard. It is written against the same
geometry the suite's warm-handoff case already documents. A companion multi-crossing case asserts
the relays still arrive across several consecutive seam crossings; it is the mandatory net for this
and any future subscription-plane change. Both run in the maintainers' merge gate. Live wire runs
are operator-gated in attended sessions — see `docs/danger-zones.md`, "Verification".

## Known dependency

Routing consults realm-core's `game_character_shard` index as a hint. That index is **only ever
written by the login self-heal probe**: `finish_transfer` does not write it. Region
routing does not depend on the index being populated — a missing entry costs one extra cache probe
at login and then heals — but the probe is load-bearing, and making the index authoritative is
planned, not built.

## Load sampling — deciding WHEN to draw a seam

Everything above answers "how does a drawn, activated seam route traffic". This section answers
the question that has to come BEFORE that one: which shard is hot, which region within it is the
busy part, and is it worth spending a seam on. Without this, activating a seam is a guess; with it,
it is two `spacetime sql` queries.

**Who samples, and how often.** The gateway is the only component that can see the whole realm at
once — every shard's own SpacetimeDB node exposes `/v1/metrics` (the same writer-occupancy
computation the maintainers' capacity benchmark performs, read continuously here instead of
only during a deliberate ramp), every shard's coordinator connection already caches its live
player-session count, and every world shard's own subscription cache holds live positions. A
background task (`gateway/src/load_sample.rs`) samples all three every `LYRACORE_LOAD_SAMPLE_SECS`
(default **30s**) and writes them onto **realm-core** via two operator-gated reducers
(`module/src/load.rs`), the same authoritative-on-realm-core / inert-everywhere-else split
`game_region_assignment` uses:

```
game_shard_load       { id, shard, sampled_at_micros, writer_occupancy_pct, sessions, gateway_key }
game_shard_load_total { shard, sessions, updated_at_micros }
game_region_load      { id, map_id, region_id, sampled_at_micros, players }
```

Both are **ring-buffered, not TTL-reaped**: each keeps only the last 20 samples per key — per
`(shard, gateway_key)` for `game_shard_load`, per `(map_id, region_id)` for `game_region_load` —
oldest evicted at insert time, so neither table can become an
unreaped, unbounded grower. At the default cadence, 20 samples is **~10 minutes of history**, which is the
mechanism for the "sustained load, not a spike" acceptance criterion: a shard reading 90% on every
row in its ring is saturated; a shard reading 90% once and 12% on the other nineteen rows was a
momentary burst. `game_region_load` never carries a row for `DEFAULT_REGION` — "the rest of the
map" is not a candidate seam, so a count against it answers nothing actionable.

**`gateway_key` (issue #308) — why `sessions` is per-GATEWAY, not per-shard, and how to sum it.**
`sessions` is `Coordinator::session_count()`, which is **this gateway process's own** player-
connection cache for that shard — never the shard's realm-wide total. With one gateway process
that distinction is invisible; with N gateway processes connected to the same shard (the
horizontal-scaling topology of multiple gateways in front of one shard), each writes its OWN
`sessions` sample. `gateway_key` is a deterministic hash of that process's `LYRACORE_GATEWAY_ID`
(config-supplied, default `<hostname>:<world-listener-port>` — `gateway/src/config.rs`,
`load_sample::gateway_key`), so the ring is scoped per `(shard, gateway_key)` instead of per
`shard` alone: every gateway keeps its own history side-by-side rather than each insert evicting
every OTHER gateway's rows (the old "last writer wins" bug). `gateway_key` is a hash, not the raw
string, because a `String` column END-appended to an existing table cannot carry a
`#[default(...)]` (`docs/danger-zones.md` §1.2) — pre-migration rows read `gateway_key = 0`, an
"unknown gateway" bucket, not a real one.

`spacetime sql` has no `GROUP BY`/`SUM` to fold the per-gateway rows into a total on read
(`docs/danger-zones.md` §2), so `record_shard_load` folds them at WRITE time instead: every sample
recomputes the realm-wide sum (latest `sessions` row per distinct `gateway_key` for that shard) and
upserts it into `game_shard_load_total`, one row per shard, keyed by `shard`. That table — not a
manual sum over `game_shard_load` — is what a single `spacetime sql` query against realm-core
should read for the shard's session total.

**Approximate by construction — the staleness an operator should know about:**

- Both tables are snapshots taken at the sample instant, not windowed averages. A player mid-move
  between two samples is counted wherever they were AT the instant sampled.
- `sessions` is the gateway's own per-account player-connection cache size for that shard
  (`Coordinator::session_count`) — it can lag a very recent disconnect by up to one sample interval.
- **Writer occupancy needs a name→identity mapping the gateway is NOT given automatically.**
  SpacetimeDB's `/v1/metrics` labels a sample by the database's IDENTITY (a hex string), not by the
  friendly name `LYRACORE_SHARD_MAP`/`LYRACORE_REGION_SHARDS` use — exactly the same fact that
  applies to the maintainers' capacity benchmark and its own `--db` flag. Set
  `LYRACORE_METRICS_DB_IDS="<shard>=<hex-prefix>,<shard>=<hex-prefix>,..."` (comma/semicolon/newline
  separated, `#` comments) to enable occupancy sampling; each identity can be read off the node's own
  `/v1/metrics` output, which labels every database it currently exposes.
  **A shard missing from this map is not a silent zero**: its
  `game_shard_load` rows simply do not exist for that shard (sessions/regions are still sampled and
  recorded), and the gateway warns once at startup naming the gap — the "fails loudly when the
  manual config disagrees with the topology" property.

**The SHARDLOAD log line.** Same convention as QUEUESTAT/AOISTAT (`gateway/src/world/mod.rs`): one
`SHARDLOAD shard=<name> occupancy=<pct>%|unmeasured sessions=<n> regions=<n>` line per connected
world shard, every sample cycle — visible in the gateway's own log without touching SQL at all.

### The two operator queries

Both run against **realm-core** (`spacetime sql lyracore-realm "..."`) — the ring buffers are small by
construction (≤20 rows per key), so a plain dump is enough to eyeball sustained-vs-spike without
`ORDER BY`, which `spacetime sql` does not have (`docs/danger-zones.md` §2).

**Which shard is hot** — every recent sample reading over half occupied; several rows for the same
shard means SUSTAINED, one row means a spike:

```bash
spacetime sql lyracore-realm "SELECT shard, writer_occupancy_pct, sessions, gateway_key, sampled_at_micros FROM game_shard_load WHERE writer_occupancy_pct > 50"
```

Writer occupancy is a per-*node* number and identical across every gateway's rows for a shard, so it
reads correctly straight off any one row. `sessions` on `game_shard_load` is **per gateway**, per
the `gateway_key` note above — for the shard's realm-wide session total, query the materialized
total table instead:

```bash
spacetime sql lyracore-realm "SELECT shard, sessions, updated_at_micros FROM game_shard_load_total WHERE shard = 'lyracore-world-1'"
```

**Which region is the busy part of it** — pick a population threshold that means "crowded" for your
zone (a starting-zone seam menu's floor is ~10×10 cells, i.e. plenty of room for more than this many
players before it is genuinely tight):

```bash
spacetime sql lyracore-realm "SELECT map_id, region_id, players, sampled_at_micros FROM game_region_load WHERE players > 20"
```

A hot shard whose `game_region_load` rows are spread evenly across its regions says "the whole
shard is busy, consider moving a WHOLE region to a fresh shard" (bulk migration, not built yet). A
hot shard with ONE region far above the rest says "activate a seam for that one region" — the
`set_region_assignment` runbook earlier in this document is the very next step, and the planned
operator console is a visualization over exactly these two queries plus the assignment table.
