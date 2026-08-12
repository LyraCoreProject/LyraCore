//! The broadcast creature-move event + the patrol schedule, and the slow world tick `tick_creatures` —
//! an ordered pipeline of named passes (patrol / respawn / decay / aggro+assist / cast / threat-retarget
//! / chase / return / wander / regen / flee / fear-flee). Each pass is a module-private free fn; the
//! pipeline order is load-bearing (see the doc comments). [server]/[event]
//!
//! Issue #383 split this file along its own comment banners, once it grew to 2,900+ lines:
//!   - `mod.rs` (this file) — the two tables + the schedule table, the `tick_creatures` pipeline
//!     itself, the active-cell/rows-visited stats infra, `pass_combat_enter`/`pass_combat_drop`, and
//!     the shared "one movement-leg grammar" toolkit (`PendingLeg`/`leg_toward`/`drain_legs`/
//!     `movable_creature`) + the one spline writer (`emit_move_spline`/`emit_creature_leg`) every
//!     submodule below calls into — kept here because BOTH `movement` and `sense` depend on them, and
//!     `pass_advance_splines` (which they feed) is itself part of the pipeline, not a "pass" any one
//!     category owns.
//!   - [`movement`] — the RUN/step legs: patrol, chase, return-to-spawn, wander, flee, fear-flee.
//!   - [`lifecycle`] — the canonical despawn checklist (issue #359) + decay/respawn/GO-respawn, the
//!     due-time passes that run regardless of proximity.
//!   - [`sense`] — aggro/assist (typed `AggroEvent`, issue #383), cast, threat-retarget, regen.
//!
//! Every `crate::creatures::tick::<sym>` path a caller outside this module used before the split
//! still resolves — see the `pub(crate) use` re-exports below the `mod` declarations.

use lyracore_shared::spatial;
use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{game_aura, game_entity_motion, game_melee_attack, game_world_entity, WorldEntity};

use super::*;

mod lifecycle;
mod movement;
mod sense;

// Bring every submodule's `pub(super)` passes into THIS module's namespace, so `tick_creatures`
// below calls them by their bare name exactly as it did before the split (the pipeline body is
// otherwise untouched — same names, same call sites, same order).
use lifecycle::*;
use movement::*;
use sense::*;

// Re-export so `crate::creatures::tick::despawn_creature_entity` (and, via `creatures::mod.rs`'s own
// `pub use tick::*`, `crate::creatures::despawn_creature_entity`) still resolves — `encounter.rs`/
// `instance.rs` call it by that exact path. `pub(crate)`, unchanged from pre-split.
pub(crate) use lifecycle::despawn_creature_entity;

// ===========================================================================================
//  Creature movement event [event] — broadcast (public, no RLS)
// ===========================================================================================

/// A server-driven creature movement leg to relay as `SMSG_MONSTER_MOVE`. Broadcast (public, no
/// RLS) — the gateway fans it out to every in-world client, matching the global `game_world_entity`
/// visibility placeholder. [event]
#[table(accessor = game_creature_move_event, public)]
pub struct CreatureMoveEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub mover_guid: u64,
    pub start_x: f32,
    pub start_y: f32,
    pub start_z: f32,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_z: f32,
    pub duration_ms: u32,
    pub spline_id: u32, // strictly increasing per creature (the client rejects a stale spline)
    pub created_at: Timestamp,
    // RUN_MODE flag for the relayed SMSG_MONSTER_MOVE: true = run animation (chase/return/flee/fear at
    // RUN speed), false = walk (patrol/wander at WALK). Without it every leg walk-animated → RUN-speed
    // legs moonwalked. `#[default(false)]` + end-appended → auto-migrates (existing rows read as walk).
    #[default(false)]
    pub run: bool,
}

/// SPLINE MODEL — the active movement leg for a creature. The client interpolates the emitted
/// `SMSG_MONSTER_MOVE` (start→dest over `dur_ms`) on its own; the SERVER advances the authoritative
/// `game_world_entity` position by lerping this SAME spline each tick in `pass_advance_splines`, instead
/// of snapping the row to the leg END at leg-start (the old "leg-lead" that made every range/melee/aggro
/// check read the creature ahead of where it renders — the "movement feels off vs vanilla" cause). One
/// row per moving creature; cleared on arrival (t≥1), on CC (halt), or when the creature is gone.
/// **PUBLIC and grid-scoped since the creature-relay change** — this row now IS the creature-movement
/// relay, replacing the per-move `game_creature_move_event` insert (the same move perf catalog 2.1
/// made for player movement, applied to creatures as 2.3 asks).
///
/// Why: `game_creature_move_event` was a globally-subscribed table (`SELECT *`), so EVERY creature
/// leg was delivered to EVERY connected player and then discarded by the gateway's `created`-set
/// guard. Measured at 100 dispersed players: 121.7 inserts/s + 121.6 reaps/s, each fanned to all 100
/// sessions. This row already existed, is already one-per-creature, and is already written on every
/// leg — so carrying the relay on it costs nothing extra and makes the delivery grid-scoped.
///
/// The grid columns mirror `game_world_entity`'s so the AOI tracker can subscribe this table with the
/// identical 5×5 box query it already builds for entities and motion.
#[table(
    accessor = game_creature_spline,
    public,
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y])),
    // #456: the AOI cell index — exactly 3 columns, all matched by equality terms, which is the
    // only shape SpacetimeDB 2.7.1's subscription planner can serve (see the `cell` column).
    index(accessor = by_cell, btree(columns = [map_id, instance_id, cell]))
)]
pub struct CreatureSpline {
    #[primary_key]
    pub guid: u64,
    pub start_micros: u64, // ctx.timestamp micros at leg-start
    pub dur_ms: u32,
    pub sx: f32,
    pub sy: f32,
    pub sz: f32, // leg START (authoritative pos at emit)
    pub dx: f32,
    pub dy: f32,
    pub dz: f32, // leg DEST (snapped landing)
    // --- END-APPENDED, all defaulted (migration rule): the relay half. ---
    /// The mover's grid address at leg-start — the AOI box predicate.
    // TYPED literals, not a bare `0`: `#[default(0)]` on a u64 encodes as 4 bytes and the publish
    // fails with "data too short for u64: Expected 8, given 4". Caught by the preflight check that
    // runs before every publish, which is the only thing that checks default ENCODINGS — the
    // compiler and the test suites are both blind to it (`docs/danger-zones.md` §2).
    #[default(0u32)]
    pub map_id: u32,
    #[default(0u64)]
    pub instance_id: u64,
    #[default(0i32)]
    pub grid_x: i32,
    #[default(0i32)]
    pub grid_y: i32,
    /// `SMSG_MONSTER_MOVE`'s spline id (the old event's `spline_id`, = `now_ms` at emit) and gait.
    /// A client keys its interpolation off the id, so it must CHANGE per leg — that is also what
    /// makes an in-place update observable as a new leg rather than a no-op.
    #[default(0u32)]
    pub spline_id: u32,
    #[default(false)]
    pub run: bool,
    /// #456: `(grid_x, grid_y)` packed into ONE indexed value — the AOI subscription's cell key.
    ///
    /// SpacetimeDB 2.7.1's subscription planner can only serve a query from an index when EVERY
    /// column of that index is matched by an equality term, and it skips any index with more than 3
    /// columns outright (`MAX_EXACT_INDEX_COLS`); range predicates are never index-served at all
    /// (`IndexProbe::Range` — "we currently never construct this variant") and an `OR` is evaluated
    /// row-by-row. So a `grid_x BETWEEN .. AND grid_y BETWEEN ..` box degrades to a full partition
    /// scan — 1.1 BILLION rows examined on `game_gameobject` in a 445-player measurement. Folding the
    /// two grid columns into one makes `by_cell` a 3-column all-equality index, which the planner CAN
    /// serve, and the AOI box becomes 25 point probes instead of a scan.
    ///
    /// ALWAYS written from `spatial::grid_cell_id(grid_x, grid_y)` in the SAME statement that writes
    /// `grid_x`/`grid_y` — a stale value here does not merely slow a query down, it puts the row in
    /// the wrong cell and shows players the wrong world. `module/src/tripwires.rs::grid_cell_tripwire`
    /// is the enforcement.
    ///
    /// `#[default(0i64)]` (typed — an i64 column needs an explicitly-typed literal) + END-appended so
    /// `publish` auto-migrates. **The default is cell (0, 0), not "unset"**, so a pre-existing row is
    /// mis-addressed until its next leg re-stamps it (sub-second for anything actually moving);
    /// `backfill_cell_ids` covers it for completeness.
    #[default(0i64)]
    pub cell: i64,
    /// #518: this leg is a FACING-ONLY packet (the mover does NOT move — `sx/sy/sz` == `dx/dy/dz`,
    /// `dur_ms` is 0) and `facing_angle` is the new heading the client should snap to
    /// (`SMSG_MONSTER_MOVE`'s `FacingAngle` variant). A stationary stand-and-swing creature never
    /// throws a normal leg (nothing to interpolate), so without this the client never learns its
    /// heading changed — the "keeps its pre-combat orientation until you move" bug. `false`/`0.0`
    /// (the pre-518 baseline) reproduces the old `Normal`-type stop exactly, so every other caller
    /// of [`emit_move_spline`] is unaffected. END-appended + defaulted (migration rule).
    #[default(false)]
    pub facing: bool,
    #[default(0.0f32)]
    pub facing_angle: f32,
}

// ===========================================================================================
//  Patrol scheduling [server]
// ===========================================================================================

/// Drives the creature tick — one row per FIRING SCOPE (work-item 229). The seeded row is the
/// GLOBAL/CATCH-ALL ticker (`instance_id == GLOBAL_TICK_INSTANCE`); an optional DEDICATED row per
/// instance makes that instance tick at its own cadence while the catch-all skips it (`TickScope` in
/// ai.rs is the coverage rule — a partition, never an overlap, so a second row DIVIDES the per-firing
/// work instead of multiplying it).
///
/// HONEST BOUND (work-item 229): SpacetimeDB serializes every reducer on ONE commit stream — this is
/// LATENCY SMOOTHING + WORK AVOIDANCE, **NOT parallelism**. Each extra row's firings preempt the
/// shared stream (10 instances at 100ms = 100 extra transactions/sec), so tight per-instance
/// cadences are a knob to use sparingly, measured via the per-pass rows-visited log below.
///
/// PER-INSTANCE `tick_ms` KNOB: the cadence IS `scheduled_at` (`ScheduleAt::Interval(tick_ms)`) on
/// the dedicated row — no separate `tick_ms` column (it would duplicate `scheduled_at`), and no
/// `game_instance` table exists yet to hang it on (190 slice 1 landed only the indexes).
/// SPLICE POINT (190 slice 2): `create_instance` inserts the dedicated row
/// `{scheduled_id: 0, scheduled_at: ScheduleAt::Interval(tick_ms), instance_id: N}` and the instance
/// reap deletes it (which automatically returns coverage of N to the catch-all — pause/slow-when-
/// empty is then "update/delete the dedicated row", also slice-2 lifecycle work). Until then the
/// operator arms one via `debug_arm_instance_tick` (debug.rs). [server]
#[table(accessor = game_creature_move_schedule, scheduled(tick_creatures))]
pub struct CreatureMoveSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    // END-APPENDED defaulted column (additive auto-migration — danger-zones §2). Which instance this
    // row ticks. The literal default MUST equal `GLOBAL_TICK_INSTANCE` (u64::MAX; the `#[default]`
    // macro wants a literal). NOTE: no test can pin this attribute literal itself — it is migration
    // metadata, not Rust `Default`; ai.rs's `global_tick_instance_sentinel_is_u64_max` pins only the
    // CONST — so editing this literal alone reds nothing: treat it as hand-synced with ai.rs so the
    // EXISTING seeded row auto-migrates into the catch-all — every live creature keeps exactly one
    // ticker, no re-seed needed. This table is NOT gateway-subscribed (no entry in connection.rs's
    // subscription list or gateway/tests/schema_parity.rs's manifest) → no binding hand-sync needed
    // (playbook failure-mode #1, the "No" branch).
    #[default(18_446_744_073_709_551_615u64)]
    pub instance_id: u64,
}

/// World tick (scheduled, scheduler-only): fires every `MOVE_TICK_SECS` (0.5s). It runs the MOVEMENT
/// passes EVERY tick (smooth, mangos-cadence motion) but the expensive O(N)-scan / tick-quantized
/// SENSING passes only every `SENSE_EVERY_N_TICKS`th tick (~4s, gated by `is_sense_tick`) — so HP-regen
/// rate, wander frequency, and respawn cadence stay vanilla and the full-entity scans stay cheap. This
/// is mangos's one-loop-with-recheck-timers model, adapted to one scheduled reducer (a single tick →
/// no cross-scheduler `spline_id` collision, and statement-order regen safety is preserved).
///
/// PASS ORDER IS LOAD-BEARING — do not reorder:
///   - **chase** runs after **aggro** (a creature aggroed on a sense tick closes the SAME tick) and
///     before **regen** (regen's in-combat gate then skips the still-engaged chaser, so the move isn't
///     reverted — and `pass_regen` also writes health/power-only, so it can never clobber a move).
///   - **wander** runs after **return** and SKIPS guids **return** already moved this tick
///     (`moved_this_tick`); both share one `spline_id`/tick and the client rejects a second leg.
///   - **flee** and **fear-flee** run LAST, after **regen**: a fleeing/feared creature is still engaged
///     so regen's in-combat gate skips it (and the health-only write means even a non-engaged feared
///     creature's position survives regen).
///   - **decay** runs before **respawn**: decay arms `respawn_at` to a FUTURE time.
///
/// ACTIVE CELLS (work-item 230 — grid activation): `active_cell_creatures` is computed ONCE per tick
/// and threaded through every pass that scopes to it. Pass-by-pass classification (verified against
/// this file's CURRENT behavior, not assumed):
///   - **SCOPED to active cells** (dormant outside them; the item's "wander/movement legs" +
///     "aggro, assist"): `pass_patrol`, `pass_aggro_assist`, `pass_return`, `pass_wander`. All four
///     already skip ENGAGED creatures internally (`is_engaged`, or "not already attacking" for aggro
///     candidates) before this item touched them, so the active-cell gate only ever puts a
///     NON-engaged, out-of-reach creature to sleep — it can never dormant-ize a creature mid-fight.
///   - **ALWAYS ACTIVE regardless of cells** (combat-engaged, per the item's explicit rule — "a player
///     could drag one far away"): `pass_chase`, `pass_cast`, `pass_threat_retarget`, `pass_flee`,
///     `pass_fear_flee`. Work-item 233 (230 review finding: these four still `entities.iter()`
///     full-scanned and filtered inside the loop) routed `pass_cast`/`pass_flee` through the SAME small
///     `game_melee_attack` table `pass_chase`/`pass_threat_retarget` already outer-loop (every candidate
///     for all four MUST already be the attacker in a melee row — that was already a hard gate inside
///     the old loop body, so outer-looping the table instead of the entity set visits the identical
///     candidates) and routed `pass_fear_flee` through the small `game_aura` table, filtered to
///     `A_CONTROL(M_FEAR)` rows (no `by_kind` index exists — `by_target`/`by_expiry` only — so a
///     target-side lookup can't narrow it, but the AURA table itself is far smaller than the entity
///     table, so scanning it with a kind/mechanic filter is the cheap, correct narrowing). All five are
///     therefore driven by a small table (or an active CC aura), never the full entity table — so they
///     were never truly O(world) in RESULT, only in SCAN, and now neither. No active-cell gate is added
///     to any of them, which is itself the correctness fix: gating an engaged/fleeing/feared creature
///     would risk freezing it mid-leg while a player is still fighting or chasing it. `pass_pet` (run in
///     the same sense block) is driven by the pet guids `active_cell_creatures` harvests from the scan
///     it already runs (perf catalog 1.10) — see its own doc comment for why an index on the entity
///     table would have been the wrong trade.
///   - **STAYS GLOBAL** (due-time based, not proximity — respawn timers and corpse decay deadlines
///     fire regardless of whether anyone is nearby): `pass_decay`, `pass_respawn`,
///     `pass_gameobject_respawn` (GO respawn; pre-existing note defers its own scaling fix separately),
///     `pass_regen` (HP/power recovery isn't proximity-gated in vanilla either), `pass_combat_drop`
///     (its due-time is `combat_until_ms`, and it scans players too, not just creatures — out of this
///     item's creature-ticking scope), `pass_combat_enter` (driven by the small melee table, already
///     cheap).
///
/// INSTANCE SCOPE (work-item 229 — latency smoothing + work avoidance, NOT parallelism; see the
/// `CreatureMoveSchedule` doc): every firing resolves a `TickScope` from ITS OWN schedule row —
/// the catch-all row covers every instance without a dedicated row; a dedicated row covers exactly
/// its instance (a partition: no creature is ever ticked by two rows). Pass-by-pass:
///   - the ACTIVE-CELL passes (patrol/aggro+assist/return/wander) inherit the scope for free:
///     `active_cell_creatures` only seeds from players in COVERED instances, and `entities_near` is
///     already instance-gated (190 slice 1), so `active` contains only covered-instance creatures.
///   - the ENGAGED/table-driven passes (cast/threat-retarget/chase/combat-enter/flee/fear-flee/pet)
///     gate each candidate on `scope.covers(creature.instance_id)` — with only the seeded catch-all
///     row this is `true` for every candidate (equivalence: ai.rs `tick_scope_default_config_…`),
///     and with a dedicated row armed it's what stops the catch-all from double-ticking (e.g.
///     double-speed-chasing) that instance's combats.
///   - the GLOBAL due-time passes (decay/respawn/GO-respawn/regen/combat-drop) + package ticks run
///     ONLY on the catch-all firing and still cover ALL instances (see
///     `TickScope::runs_global_passes` for why running them per-row would multiply their effects).
///   - the SENSE gate + the per-firing STEP length derive from the firing row's own interval
///     (`is_sense_tick_for_interval` / `tick_secs_for_interval`) so a fast dedicated row keeps the
///     ~4s sensing cadence and cadence-invariant movement speed — byte-identical at the default.
#[reducer]
pub fn tick_creatures(ctx: &ReducerContext, schedule: CreatureMoveSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    // Work-item 229: which instances THIS firing covers (see the doc above) + this row's own cadence.
    // The scope build scans only the schedule table itself (one catch-all + one row per dedicated
    // instance — a handful), never a creature/entity table.
    let scope = TickScope::from_rows(
        schedule.instance_id,
        ctx.db
            .game_creature_move_schedule()
            .iter()
            .map(|r| r.instance_id),
    );
    let interval_micros = match &schedule.scheduled_at {
        ScheduleAt::Interval(d) => d.to_micros(),
        // A one-shot Time row (nothing inserts one today) falls back to the default cadence math.
        ScheduleAt::Time(_) => MOVE_TICK_MICROS,
    };
    let tick_secs = tick_secs_for_interval(interval_micros);
    // Movement passes run EVERY firing; the sensing passes quantize to ~one per 4s regardless of this
    // row's cadence (at the seeded 0.5s interval this is byte-identical to the old 1-in-8 gate).
    let sense =
        is_sense_tick_for_interval(ctx.timestamp.to_micros_since_unix_epoch(), interval_micros);
    // The ONLY cross-pass mutable datum: guids the return pass moved this tick, which the wander pass
    // must skip (one move leg per creature per tick). Threaded return (&mut) → wander (&).
    let mut moved_this_tick: std::collections::HashSet<u64> = std::collections::HashSet::new();

    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;

    // ACTIVE CELLS (work-item 230): the union of every COVERED player's grid-cell neighborhood, read
    // through the `by_grid` index — computed once and shared by every scoped pass below (patrol/return
    // run every tick, so the set is (re)built every tick; aggro/wander are already gated by `sense`).
    let TickSweep {
        active,
        pets,
        in_combat,
    } = active_cell_creatures(ctx, &scope);
    if scope.runs_global_passes() {
        // The 230/233 evidence lines describe the WORLD tick; a dedicated row's numbers would only
        // muddy them (its scoped stats land in `log_pass_stats` below, labeled per scope).
        log_active_cell_stats(ctx, &active);
        log_narrowed_pass_stats(ctx); // work-item 233 done-when evidence (rows-visited drop)
    }
    // Work-item 229 PART B: per-pass rows-visited counters for THIS firing (cheap local increments;
    // logged once a minute — see `log_pass_stats` for the sampling-over-table-writes rationale).
    let mut stats = PassStats::default();

    // SPLINE ADVANCE runs FIRST: move every active leg to its interpolated position so the passes below
    // (and the range/melee reads) see where the creature actually renders, not a leg-end lead. Global
    // (every active spline), like the engaged table-driven passes — a mid-leg creature must advance even
    // if it drifts out of an active cell for a tick.
    let now_us = ctx.timestamp.to_micros_since_unix_epoch() as u64;
    stats.add("advance", pass_advance_splines(ctx, now_us, now_ms));
    stats.add("patrol", pass_patrol(ctx, &active)); // movement (segment, ETA-gated) — active-cell scoped
                                                    // Sensing block A — BEFORE chase so a creature aggroed this sense tick can close on the same tick.
    if sense {
        if scope.runs_global_passes() {
            stats.add("decay*", pass_decay(ctx));
            stats.add("respawn*", pass_respawn(ctx));
            stats.add("go_respawn*", pass_gameobject_respawn(ctx)); // GATHER-node respawn — minutes-scale
        }
        stats.add("aggro_assist", pass_aggro_assist(ctx, &active)); // active-cell scoped + INVERTED
                                                                    // PET AI (Tier 3b): a summoned pet engages its owner's target / follows the owner when idle. Run
                                                                    // right after aggro + before chase so a pet that ARMS a melee row this sense tick closes on the
                                                                    // SAME tick via pass_chase below (the exact aggro→chase ordering the wild creatures rely on).
        stats.add(
            "pet",
            crate::creatures::pass_pet(ctx, now_ms, &scope, interval_micros, &pets),
        );
        stats.add("cast", pass_cast(ctx, &scope)); // engaged-only — always active (no active-cell gate)
        stats.add("threat_retarget", pass_threat_retarget(ctx, &scope)); // engaged-only — always active
    }
    stats.add("chase", pass_chase(ctx, &scope)); // movement (step) — AFTER aggro, BEFORE regen
    stats.add("combat_enter", pass_combat_enter(ctx, &scope)); // flag the melee-engaged set IN_COMBAT
    stats.add(
        "return",
        pass_return(ctx, &mut moved_this_tick, &active, tick_secs),
    ); // movement (step)
       // Sensing block B — regen AFTER the movement passes (its in-combat gate + health-only write protect
       // the moves); wander AFTER return so it sees `moved_this_tick`.
    if sense {
        stats.add("wander", pass_wander(ctx, &moved_this_tick, &active)); // active-cell scoped
        if scope.runs_global_passes() {
            stats.add("regen*", pass_regen(ctx));
            stats.add("combat_drop*", pass_combat_drop(ctx, &in_combat)); // clear IN_COMBAT past the deadline
        }
    }
    stats.add("flee", pass_flee(ctx, &scope)); // movement — LAST, after regen; engaged-only
    stats.add("fear_flee", pass_fear_flee(ctx, &scope, tick_secs)); // movement — LAST; CC-only
                                                                    // Package tick passes: every registered `game_tick_pass` runs AFTER all core passes, on the
                                                                    // CATCH-ALL firing only (work-item 229: package passes take no scope and would otherwise run once
                                                                    // per schedule row — multiplied, unscoped work; on the catch-all they keep exactly their pre-229
                                                                    // every-0.5s cadence) — a package pass can observe this tick's outcome but can never wedge itself
                                                                    // between the load-bearing core orderings above. Slower cadences self-quantize (see the macro doc
                                                                    // in lib.rs).
    if scope.runs_global_passes() {
        crate::hooks::run_package_tick_passes(ctx);
    }
    log_pass_stats(ctx, &scope, sense, &stats, interval_micros);
}

// ===========================================================================================
//  Active cells [server] — work-item 230: grid-activation; only cells near players tick
// ===========================================================================================

/// Rough heartbeat period (micros) for the active-cell rows-visited log line — the work-item 230
/// done-when evidence. NOT every tick (would spam `RUST_LOG=info` at the 0.5s movement cadence);
/// roughly once a minute is plenty to eyeball the before/after ratio on a live node.
const ACTIVE_CELL_LOG_PERIOD_MICROS: i64 = 60_000_000;

/// The active-cell footprint radius (yards) for THIS tick: the larger of the combat activation radius
/// (`ai::combat_active_radius` — ~55yd at today's template data, aggro+assist) and the AOI visibility
/// radius the gateway subscribes to (`BOX_HALF_SPAN * GRID_CELL_SIZE`, 100yd guaranteed-visible). The
/// visibility floor matters for the MOVEMENT passes (patrol/return/wander): without it a creature
/// outside the (smaller) combat radius but still inside a player's view would visibly FREEZE mid-
/// route — an observable divergence the item's "byte-identical... anything a player could observe"
/// forbids. Taking the max means one active-cell set safely serves both concerns.
/// `game_creature_template` is a small reference table (tens of rows), not the ~2500-creature live
/// population this item exists to stop scanning — reading it once per tick is cheap.
fn active_cell_radius(ctx: &ReducerContext) -> f32 {
    let visible = spatial::BOX_HALF_SPAN as f32 * spatial::GRID_CELL_SIZE;
    // Perf catalog 1.19: the max-fold over every template used to run on EVERY firing (2+/s, and a
    // full cmangos import carries ~4,000 templates) even though the visibility floor dominates it at
    // every data set we ship. `by_aggro_range` answers the ONLY question that can change the outcome
    // in one indexed probe: is there an override big enough to beat the floor? If not, the floor IS
    // the answer (provably — see `aggro_override_cutoff`), so the fold is skipped entirely. When one
    // does exist (nothing imports one today) we fall back to the exact original fold.
    let templates = ctx.db.game_creature_template();
    let cutoff = crate::creatures::ai::aggro_override_cutoff(visible);
    if templates.by_aggro_range().filter(cutoff..).next().is_none() {
        return visible;
    }
    let template_aggro_max = templates
        .iter()
        .map(|t| t.aggro_range as f32)
        .fold(0.0_f32, f32::max);
    combat_active_radius(template_aggro_max).max(visible)
}

/// ACTIVE CELLS (work-item 230): the deduped guid set of every non-player creature within
/// `active_cell_radius` yards of AT LEAST ONE player entity — INVERTING the old full-table scan to
/// iterate the sparse side (players; bots included, since a bot's `game_world_entity` row carries the
/// same PLAYER type_mask bit a real client's does — `is_player()` already covers them: a bot is a
/// `game_world_entity` row with the PLAYER bit).
/// Reuses `helpers::entities_near` (the existing, already-tested `by_grid`-indexed neighborhood query
/// from work-item 190 slice 1 — until now unused) per player, so the query is instance-isolated for
/// free. A creature absent from this set is DORMANT this tick for every pass that consults it — see
/// the classification in `tick_creatures`'s doc comment. With zero players online the set is empty
/// (every creature dormant); with players online the cost scales with player density, not world size.
///
/// NOTE on the one remaining full-table touch: locating the players themselves still needs
/// `entities.iter().filter(is_player)` — there's no `by_type_mask` (or dedicated players-only) index
/// in the current schema, so this reads every row once. That's the SAME pattern `pass_aggro_assist`
/// already used pre-230 to build its own player snapshot (this fn doesn't share that one — a further,
/// independent micro-optimization) — but it's a bare bit-check per row, not the expensive per-creature
/// template/faction/stealth logic this item exists to stop running on out-of-range creatures. The
/// rows-visited reduction this item measures is that expensive-logic population, not this cheap scan.
/// Work-item 229: seeds ONLY from players in instances THIS firing's scope covers — `entities_near`
/// is already instance-gated (190 slice 1), so the returned set then contains only covered-instance
/// creatures, which scopes patrol/aggro+assist/return/wander without touching their bodies. With
/// only the seeded catch-all row, `covers()` is `true` for every player → identical set to pre-229.
fn active_cell_creatures(ctx: &ReducerContext, scope: &TickScope) -> TickSweep {
    let entities = ctx.db.game_world_entity();
    let radius = active_cell_radius(ctx);
    let mut out = std::collections::HashSet::new();
    // Perf catalog 1.10 + 1.7: `pass_pet` and `pass_combat_drop` each used to run their OWN full
    // `entities.iter()` scan per sense tick — one for `owner_guid != 0`, one for the IN_COMBAT bit.
    // This scan is already mandatory (it locates the players the active-cell set is built from) and
    // already visits every row, so both guid lists ride along for the cost of two bit tests: no new
    // table, no lifecycle hooks, and no index maintenance on the hottest write path in the tick (the
    // trade `pass_pet`'s own doc rightly rejected). Collected in table order, so both passes visit the
    // same candidates in the same order as their old dedicated scans.
    let mut pets: Vec<u64> = Vec::new();
    let mut in_combat: Vec<u64> = Vec::new();
    let players: Vec<WorldEntity> = entities
        .iter()
        .filter(|e| {
            if e.owner_guid != 0 {
                pets.push(e.guid);
            }
            if e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
                in_combat.push(e.guid);
            }
            e.is_player() && scope.covers(e.instance_id)
        })
        .collect();
    for p in players {
        for c in crate::helpers::entities_near(ctx, p.map_id, p.instance_id, p.x, p.y, radius) {
            // `cell_is_active` is a belt-and-suspenders re-check of the SAME predicate `entities_near`'s
            // `by_grid` query already encodes (mirrors `entities_near`'s own `in_same_partition`
            // re-check) — kept as a real call so the pure active-cell math it wraps stays exercised by
            // something other than its own unit tests.
            if !c.is_player()
                && cell_is_active(
                    c.map_id,
                    c.instance_id,
                    c.grid_x,
                    c.grid_y,
                    p.map_id,
                    p.instance_id,
                    p.x,
                    p.y,
                    radius,
                )
            {
                out.insert(c.guid);
            }
        }
    }
    TickSweep {
        active: out,
        pets,
        in_combat,
    }
}

/// Everything ONE pass over `game_world_entity` yields for a firing of `tick_creatures` — the
/// active-cell creature set plus the two small candidate lists that used to cost a dedicated full scan
/// each (perf catalog 1.7 / 1.10). Every field is derived from the SAME row visit, so adding a
/// consumer costs a bit test, not a scan.
struct TickSweep {
    /// Creatures within `active_cell_radius` of at least one covered player (work-item 230).
    active: std::collections::HashSet<u64>,
    /// Live pets (`owner_guid != 0`), in table order — `pass_pet`'s candidate list.
    pets: Vec<u64>,
    /// Units carrying `UNIT_FLAG_IN_COMBAT`, in table order — `pass_combat_drop`'s candidate list.
    in_combat: Vec<u64>,
}

/// Work-item 230 done-when evidence: log the active-cell rows-visited/total ratio roughly once a
/// minute. `total` (a full non-player-entity count) is deliberately gated behind the SAME rare window
/// so the O(N) count itself never reintroduces the per-tick cost this item removes.
fn log_active_cell_stats(ctx: &ReducerContext, active: &std::collections::HashSet<u64>) {
    let us = ctx.timestamp.to_micros_since_unix_epoch();
    if us.rem_euclid(ACTIVE_CELL_LOG_PERIOD_MICROS) >= MOVE_TICK_MICROS {
        return; // only the one tick per period that lands in the window logs
    }
    let total = ctx
        .db
        .game_world_entity()
        .iter()
        .filter(|e| !e.is_player())
        .count();
    log::info!(
        "tick_creatures active-cell (work-item 230): {}/{} creatures visited this tick",
        active.len(),
        total
    );
}

/// Work-item 233 done-when evidence: log the pass_cast/pass_flee/pass_fear_flee rows-visited drop, in
/// the SAME rare window `log_active_cell_stats` uses (reusing its throttle — no extra per-tick cost).
/// `melee_rows` is the candidate universe BOTH `pass_cast` and `pass_flee` now outer-loop (identical
/// gate: "currently the attacker in `game_melee_attack`"); `fear_rows` is what `pass_fear_flee` now
/// outer-loops (the `A_CONTROL(M_FEAR)` aura rows). `total_all` is a full `game_world_entity` count
/// (players included) — what EVERY ONE of the three fully `entities.iter()`-scanned before this item,
/// so it's the honest "before" denominator the rows-visited ratio is measured against.
fn log_narrowed_pass_stats(ctx: &ReducerContext) {
    let us = ctx.timestamp.to_micros_since_unix_epoch();
    if us.rem_euclid(ACTIVE_CELL_LOG_PERIOD_MICROS) >= MOVE_TICK_MICROS {
        return; // only the one tick per period that lands in the window logs
    }
    let total_all = ctx.db.game_world_entity().iter().count();
    let melee_rows = ctx.db.game_melee_attack().iter().count();
    let fear_rows = ctx
        .db
        .game_aura()
        .iter()
        .filter(|a| a.eff_kind == crate::spell::A_CONTROL && a.eff_p0 == crate::spell::M_FEAR)
        .count();
    log::info!(
        "tick_creatures narrowed passes (work-item 233): cast/flee visit {melee_rows} melee rows, \
         fear-flee visits {fear_rows} aura rows, vs {total_all} total entities each used to scan"
    );
}

/// Work-item 229 PART B (honesty addendum: "MEASURE FIRST"): per-pass rows-visited counters for one
/// firing. Each pass returns the number of candidates it did REAL per-candidate work on — i.e. rows
/// that got PAST its cheap outer gate (the active-set membership / `scope.covers()` check / due-time
/// table scan), because that post-gate population is exactly what must NOT grow with instance count
/// on the catch-all firing (the outer-scan denominators — melee/aura/entity table sizes — are already
/// in the 230/233 log lines above). Collected EVERY firing (bare integer increments inside loops that
/// already run — effectively free), so a pass's count is always current when the log window opens.
#[derive(Default)]
struct PassStats {
    entries: Vec<(&'static str, u64)>,
}

impl PassStats {
    fn add(&mut self, pass: &'static str, rows_visited: usize) {
        self.entries.push((pass, rows_visited as u64));
    }
}

/// Emit the per-pass rows-visited line for this firing, labeled with the firing's scope, throttled to
/// the SAME once-a-minute window as `log_active_cell_stats` (work-items 230/233 precedent).
///
/// WHY A SAMPLED LOG LINE AND NOT A `game_tick_stats` TABLE ROW: the counter must not itself become
/// the tax it measures — a table write per firing appends 2 rows/sec (world tick alone; +10/sec per
/// 100ms dedicated row) to the SAME serialized commit stream the honesty addendum warns about, and
/// this repo has no debug-feature-gated tick path to hide it behind (`debug_reducers` gates whole
/// reducers, not branches of a hot scheduled one). The done-when ("per-pass row-visit count does not
/// grow with instance count") needs COMPARATIVE evidence, which a once-a-minute INFO sample answers:
/// grep two samples, before and after arming a dedicated row, and compare per-pass counts. [V] the
/// live readout itself (no node in the sandbox) — runbook in work-item 229, "per-instance-ticks"
/// (archived).
///
/// COUNTER SEMANTICS (review finding — the two families are NOT comparable to each other): scoped
/// passes count POST-GATE candidates (rows this scope actually considered — these must not grow
/// when another instance is armed/populated), while the `*`-suffixed global passes count FULL
/// TABLE ROWS SCANNED — those grow with world size BY DESIGN (they cover all instances from the
/// catch-all firing) and answer 233-style scan questions, not scoping ones.
///
/// WINDOW: `max(own interval, 500ms)` — an interval-spaced firing lattice always has exactly one
/// point in any half-open window of its own interval's length, so EVERY row logs once a minute;
/// the old fixed 500ms window let a slow row (the runbook's slow-a-row-by-hand pause substitute,
/// e.g. 1000ms) miss the window FOREVER on an unlucky arm-time phase (review finding).
///
/// SENSE AT THE LOG WINDOW (500ms catch-all): now ∈ [60s·k, 60s·k + 500ms) ⇒ ⌊now/500ms⌋ = 120k
/// ≡ 0 (mod SENSE_EVERY_N_TICKS=8) — phase-independent, so the catch-all's logged line ALWAYS
/// carries the sense-gated counters (regen/decay/…). Dedicated rows can log sense=false lines;
/// the flag is printed, read accordingly.
fn log_pass_stats(
    ctx: &ReducerContext,
    scope: &TickScope,
    sense: bool,
    stats: &PassStats,
    interval_micros: i64,
) {
    let us = ctx.timestamp.to_micros_since_unix_epoch();
    if us.rem_euclid(ACTIVE_CELL_LOG_PERIOD_MICROS) >= interval_micros.max(MOVE_TICK_MICROS) {
        return;
    }
    let scope_label = match scope {
        TickScope::CatchAll { dedicated } => {
            format!("global(skipping {} dedicated)", dedicated.len())
        }
        TickScope::Only(n) => format!("instance {n}"),
    };
    let body = stats
        .entries
        .iter()
        .map(|(pass, rows)| format!("{pass}={rows}"))
        .collect::<Vec<_>>()
        .join(" ");
    log::info!(
        "tick_creatures pass rows-visited (work-item 229): scope={scope_label} sense={sense} {body} (*=full-table scan, scales with world not instances)"
    );
}

/// Movement pass — flag every unit in the melee-engaged set IN COMBAT (re-stamping its combat-drop
/// deadline via `enter_combat`). Covers melee + aggro, AND a pure caster: casting at a mob makes the mob
/// retaliate, so the caster is the TARGET of the mob's `game_melee_attack` row and gets flagged here
/// (spell casts also flag directly in `apply_target_damage`). `enter_combat` skips dead + re-stamps
/// idempotently. The engaged set is small (scales with active combats, not the world).
///
/// Work-item 229: gates each melee row on `scope.covers(<the pair's instance>)` so a dedicated row's
/// combats are re-stamped by THAT row only (the re-stamp is idempotent, but the partition discipline
/// is uniform across every scoped pass). The pair shares ONE instance by construction — arming is
/// same-instance-gated everywhere (`apply_start_attack` rejects cross-instance targets; the
/// aggro/assist/retaliation passes pair same-instance only) — so either side's entity resolves it;
/// we take the attacker's, falling back to the target's if the attacker despawned this tick. A row
/// whose BOTH entities are gone is skipped, which is observably identical to before (its
/// `enter_combat(guid)` calls were no-ops on missing entities). Returns rows visited (covered rows).
fn pass_combat_enter(ctx: &ReducerContext, scope: &TickScope) -> usize {
    let entities = ctx.db.game_world_entity();
    let mut guids: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut visited = 0usize;
    for a in ctx.db.game_melee_attack().iter() {
        let attacker = entities.guid().find(a.attacker_guid);
        let Some(instance_id) = attacker
            .as_ref()
            .map(|e| e.instance_id)
            .or_else(|| entities.guid().find(a.target_guid).map(|e| e.instance_id))
        else {
            continue; // both sides gone — enter_combat would have no-opped anyway
        };
        // Equivalence: with only the catch-all row, covers() is true for every pair (ai.rs spec).
        if !scope.covers(instance_id) {
            continue;
        }
        // 249: a PLAYER's auto-attack toggle that has never actually swung (out of range — the
        // range gate keeps last_swing_ms at 0) is NOT combat: vanilla doesn't flag you for
        // aiming at something 30 yd away, and stopping the toggle then leaves you exactly as
        // un-engaged as before. A CREATURE attacker row is combat from the instant it arms
        // (aggro IS combat), and any LANDED swing stamps last_swing_ms + the damage path calls
        // enter_combat for both sides — so real fights are untouched.
        if a.last_swing_ms == 0
            && a.last_offhand_swing_ms == 0
            && attacker.as_ref().is_some_and(|e| e.is_player())
        {
            continue;
        }
        visited += 1;
        guids.insert(a.attacker_guid);
        guids.insert(a.target_guid);
    }
    for guid in guids {
        crate::combat::enter_combat(ctx, guid);
    }
    visited
}

/// Sense pass — clear `UNIT_FLAG_IN_COMBAT` from any unit past its combat-drop deadline (no hostile
/// action for ~`COMBAT_DROP_MS`). A still-fighting unit keeps re-stamping `combat_until_ms` (via
/// `pass_combat_enter` / `apply_target_damage`) so it's never cleared mid-combat. Covers ALL entities —
/// players AND creatures — because a player's flag would otherwise stick forever (players aren't ticked
/// elsewhere). Work-item 230 classification: STAYS GLOBAL — `combat_until_ms` is a due-time (like
/// respawn/decay), not a proximity concern, and it covers PLAYERS too (not just creatures), so
/// intersecting it with the creature-only active-cell set would silently stop clearing a dormant
/// creature's flag while it's out of view. Perf catalog 1.7 kept that global reach while removing the
/// dedicated scan: the candidates come from `TickSweep::in_combat`, harvested by the entity pass
/// `active_cell_creatures` already runs every tick.
/// Work-item 229: catch-all firing only (see `TickScope::runs_global_passes`), still covering ALL
/// instances — a dedicated row never runs it, so the deadline sweep happens exactly once per sense
/// tick, as before. Returns entity rows scanned.
fn pass_combat_drop(ctx: &ReducerContext, flagged: &[u64]) -> usize {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    let entities = ctx.db.game_world_entity();
    let mut visited = 0usize;
    // Perf catalog 1.7: candidates come from `TickSweep::in_combat` — the flag bits harvested by the
    // active-cell scan that runs every tick regardless — instead of a dedicated full entity scan. The
    // predicate is re-applied to the LIVE row here (not the snapshot), which is if anything stricter:
    // `enter_combat` only ever stamps a FUTURE deadline, so a unit that gained the flag mid-tick can
    // never already be expired, and one that lost it mid-tick fails the re-check and is skipped.
    let expired: Vec<u64> = flagged
        .iter()
        .inspect(|_| visited += 1)
        .filter_map(|guid| entities.guid().find(guid))
        .filter(|e| {
            e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0
                && now_ms >= e.combat_until_ms
        })
        .map(|e| e.guid)
        .collect();
    for guid in expired {
        if let Some(mut e) = entities.guid().find(guid) {
            e.unit_flags &= !lyracore_shared::constants::unit_flags::IN_COMBAT;
            entities.guid().update(e);
        }
    }
    visited
}

/// The ONE shared creature move-leg writer (work-item 181): every movement pass (patrol / chase /
/// return / wander / flee / fear-flee) funnels its ALREADY-STEPPED landing point through here, so a
/// single ground-snap / anti-desync fix (work-item 174) applies to ALL of them at once. The per-pass
/// STEP is computed by the caller BEFORE this call (different math per pass — waypoint segment / chase
/// step / walk-home / wander hop / flee dash); this owns only what every pass shares:
///   1. ground-snap the landing point (`snap_z`) — off-slice / unimported areas fall back to the
///      caller's `z_fallback` (target z / home z / current z), byte-identical to the pre-terrain leg;
///   2. insert the `CreatureMoveEvent` (leg START = the mover's CURRENT position, DEST = the snapped
///      landing point, `spline_id = now_ms` — one leg per creature per tick);
///   3. advance the authoritative row to the leg END (x/y/z/grid/last_move), and — for the ETA-gated
///      IDLE passes only (`set_leg_ends`: patrol + wander) — arm `leg_ends_ms` so the leg plays to
///      completion instead of being re-thrown next tick (the RUN passes re-step every tick, no gate).
///
/// The mover row `e` is taken BY VALUE (every caller owns a freshly-found row and needs nothing back —
/// so no `&mut` refetch/clone dance) and written once. `run` picks the SMSG_MONSTER_MOVE walk/run
/// animation. Anything pass-specific (patrol's `wp_target`, return's `moved_this_tick`, flee's combat
/// re-stamp) stays in the caller — this writer is purely the shared leg emission.
/// Write ONE movement leg for `guid` as the AOI-scoped spline row — the single relay path since
/// perf 2.3, and the only thing a client ever sees a non-player move through.
///
/// `dur_ms == 0` with `dest == start` is a STOP (snap-and-hold): the client halts where it is.
/// Separate from [`emit_creature_leg`] because the stop/rush callers do their own entity bookkeeping
/// (a CC freeze writes the render point, Charge moves the caster) and only need the relay.
///
/// EXISTS BECAUSE 2.3 CONVERTED ONE WRITER AND LEFT FOUR: `game_creature_move_event` inserts stayed
/// in the CC freeze, the chase-stop, `encounter::move_creature_to` and Charge, while the same commit
/// removed the gateway's subscription to that table ("nothing writes this any more"). Every one of
/// them went silently undelivered — the server moved, the client did not. Route new movement here.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_move_spline(
    ctx: &ReducerContext,
    guid: u64,
    start: (f32, f32, f32),
    dest: (f32, f32, f32),
    dur_ms: u32,
    run: bool,
    spline_id: u32,
    map_id: u32,
    instance_id: u64,
    grid: (i32, i32),
) {
    let row = CreatureSpline {
        guid,
        start_micros: ctx.timestamp.to_micros_since_unix_epoch() as u64,
        dur_ms,
        sx: start.0,
        sy: start.1,
        sz: start.2,
        dx: dest.0,
        dy: dest.1,
        dz: dest.2,
        map_id,
        instance_id,
        grid_x: grid.0,
        grid_y: grid.1,
        cell: lyracore_shared::spatial::grid_cell_id(grid.0, grid.1),
        spline_id,
        run,
        facing: false,
        facing_angle: 0.0,
    };
    if ctx.db.game_creature_spline().guid().find(guid).is_some() {
        ctx.db.game_creature_spline().guid().update(row);
    } else {
        ctx.db.game_creature_spline().insert(row);
    }
}

/// #518: write a FACING-ONLY spline row — `guid` doesn't move (start == dest, `dur_ms` 0) but its
/// heading changes to `angle_rad`. Routes through the SAME `game_creature_spline` relay carrier as
/// every other creature leg (one AOI-scoped table, one gateway subscription) rather than a new one;
/// the gateway distinguishes it by the `facing` flag and emits `SMSG_MONSTER_MOVE`'s `FacingAngle`
/// variant instead of `Normal`. Callers own the epsilon gate (don't call this every tick — see
/// `pass_chase`'s stand-and-swing branch) and the entity row's `orientation` write; this is purely
/// the relay half, mirroring [`emit_move_spline`]'s split.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_facing_spline(
    ctx: &ReducerContext,
    guid: u64,
    pos: (f32, f32, f32),
    angle_rad: f32,
    spline_id: u32,
    map_id: u32,
    instance_id: u64,
    grid: (i32, i32),
) {
    let row = CreatureSpline {
        guid,
        start_micros: ctx.timestamp.to_micros_since_unix_epoch() as u64,
        dur_ms: 0,
        sx: pos.0,
        sy: pos.1,
        sz: pos.2,
        dx: pos.0,
        dy: pos.1,
        dz: pos.2,
        map_id,
        instance_id,
        grid_x: grid.0,
        grid_y: grid.1,
        cell: lyracore_shared::spatial::grid_cell_id(grid.0, grid.1),
        spline_id,
        run: false,
        facing: true,
        facing_angle: angle_rad,
    };
    if ctx.db.game_creature_spline().guid().find(guid).is_some() {
        ctx.db.game_creature_spline().guid().update(row);
    } else {
        ctx.db.game_creature_spline().insert(row);
    }
}

// A movement leg's full geometry (from/to/speed/timing); a struct built at the one call site and destructured here would be write-only.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_creature_leg(
    ctx: &ReducerContext,
    mut e: WorldEntity,
    to: (f32, f32),
    z_fallback: f32,
    duration_ms: u32,
    run: bool,
    now_ms: u32,
    set_leg_ends: bool,
) {
    // Ground-snap THIS leg's landing point (work-item 174) — one snap now covers every pass.
    let nz = crate::terrain::snap_z(ctx, e.map_id, to.0, to.1, z_fallback);
    // REFUSE a non-finite leg (see `ai::finite_point`). Writing one makes the creature invisible to
    // this very tick — its grid cell casts to `i32::MIN`, so no active cell ever contains it again —
    // while `tick_melee` keeps swinging off the melee row. It becomes an unshakeable attacker. Loud,
    // because the source of the corruption is still unknown and this log is what will name it.
    if !crate::creatures::ai::finite_point(to.0, to.1, nz) {
        spacetimedb::log::error!(
            "refused a non-finite leg for guid {} -> ({}, {}, {}) — creature left at its last good \
             position; this means some movement maths produced inf/NaN upstream",
            e.guid, to.0, to.1, nz
        );
        return;
    }
    // ONE WRITER (work-item 181/383): funnel the row build + upsert through `emit_move_spline` — the
    // SAME call `pass_advance_splines`'s CC-freeze/arrival paths and `pass_chase`'s in-place stop
    // already use, so "one spline writer" is a fact the type system enforces, not doctrine repeated at
    // each call site. Was: a `game_creature_move_event` INSERT (globally subscribed — so every leg was
    // delivered to EVERY connected session and then discarded by most of them via the `created`-set
    // guard) PLUS a hand-rolled spline DELETE+INSERT here. Measured at 100 dispersed players: 121.7
    // inserts/s and 121.6 reaps/s of the event table alone — see `emit_move_spline`'s own doc for the
    // AOI-box-scoped replacement.
    //
    // SPLINE MODEL (unchanged): the client interpolates start→dest over `duration_ms`; the SERVER
    // advances the authoritative position along the SAME spline each tick in `pass_advance_splines`
    // rather than snapping to the leg END here. e.x/e.y/e.z stay at the leg START.
    emit_move_spline(
        ctx,
        e.guid,
        (e.x, e.y, e.z),
        (to.0, to.1, nz),
        duration_ms.max(1), // avoid /0 in the lerp; a 0-dur snap just completes next advance
        run,
        now_ms, // spline id: must CHANGE per leg (see the field doc) — `now_ms` at emit time
        e.map_id,
        e.instance_id,
        (e.grid_x, e.grid_y),
    );
    e.last_move_ms = now_ms;
    if set_leg_ends {
        // ETA gate (patrol + wander): hold this leg until it lands, no mid-leg re-emit.
        e.leg_ends_ms = now_ms + duration_ms;
    }
    ctx.db.game_world_entity().guid().update(e);
}

// ===========================================================================================
//  One movement-leg grammar (issue #383) — shared by chase / return / wander / flee / fear-flee
// ===========================================================================================

/// One geometric movement leg queued by a candidate loop, ready for `drain_legs`. Replaces the
/// anonymous `(guid, dest_x, dest_y, z_fallback, duration_ms)` tuple every RUN/step pass used to
/// collect-then-mutate, re-explained by a near-identical comment at each site.
struct PendingLeg {
    guid: u64,
    dest: (f32, f32),
    z_fallback: f32,
    duration_ms: u32,
}

/// The dist→duration_ms math every movement pass ended its per-candidate loop with (six sites: patrol
/// inline, chase/return/wander/flee/fear-flee here): `to` is `speed` yards away from `from` in
/// `duration_ms` milliseconds. `speed` is yd/s (already snare/wound-adjusted by the caller, if that
/// pass applies one), so this is pure geometry — no pass-specific knowledge.
fn leg_duration_ms(from: (f32, f32), to: (f32, f32), speed: f32) -> u32 {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    ((dx * dx + dy * dy).sqrt() / speed * 1000.0) as u32
}

/// The shared tail every RUN/step movement pass repeats right after `nav::nav_step` resolves the
/// actual (possibly nav-clamped) destination: skip a degenerate zero-length step (the client rejects
/// it — `nx == c.x && ny == c.y` at five identical sites) and convert the realized distance into a leg
/// duration at `speed` yd/s. `None` collapses every site's `if nx == c.x && ny == c.y { continue; }`
/// into one check.
fn leg_toward(from: (f32, f32), to: (f32, f32), speed: f32) -> Option<(f32, f32, u32)> {
    if to == from {
        return None;
    }
    Some((to.0, to.1, leg_duration_ms(from, to, speed)))
}

/// Drain a batch of queued legs through the one shared writer (`emit_creature_leg`), re-finding each
/// mover's LIVE row first — the collect-then-mutate pattern every movement pass already followed, now
/// with one drain loop instead of five near-identical ones. `run` (walk/run animation) and
/// `set_leg_ends` (the patrol/wander-only ETA gate) are uniform across one pass's whole batch, so
/// they're parameters here rather than fields repeated on every `PendingLeg`.
fn drain_legs(
    ctx: &ReducerContext,
    legs: Vec<PendingLeg>,
    run: bool,
    set_leg_ends: bool,
    now_ms: u32,
) {
    let entities = ctx.db.game_world_entity();
    for leg in legs {
        if let Some(c) = entities.guid().find(leg.guid) {
            emit_creature_leg(
                ctx,
                c,
                leg.dest,
                leg.z_fallback,
                leg.duration_ms,
                run,
                now_ms,
                set_leg_ends,
            );
        }
    }
}

/// The shared gate ladder every ENGAGED/table-driven pass (cast / threat-retarget / chase / flee /
/// fear-flee) opens its per-candidate loop with: resolve `guid` to a live CREATURE (no PLAYER bit, not
/// dead) whose instance THIS firing's `scope` covers. `None` collapses each site's `let Some(c) = ...
/// else { continue }; if c.is_player() || c.dead { continue }; if !scope.covers(c.instance_id) {
/// continue }` into one check — every call site still increments its own `visited` counter only on
/// `Some`, matching the existing "gate first, then count" order everywhere.
fn movable_creature(ctx: &ReducerContext, guid: u64, scope: &TickScope) -> Option<WorldEntity> {
    let c = ctx.db.game_world_entity().guid().find(guid)?;
    if c.is_player() || c.dead || !scope.covers(c.instance_id) {
        return None;
    }
    Some(c)
}

/// May this creature rout at all? The HP threshold (`should_flee`) plus the per-TYPE gate
/// (`flee_eligible` — only HUMANOIDS rout; BEASTS/undead/elementals fight to the death). It decides
/// whether a rout may START, never whether one is running — that is `creature_is_routing`. The two
/// non-engaged sites (aggro re-arm, assist) ask THIS question: they act on creatures with no engagement
/// row, which therefore have no rout clock to read. A missing template ⇒ not eligible (safe default).
/// [server]
fn rout_eligible(ctx: &ReducerContext, c: &WorldEntity) -> bool {
    should_flee(c.health, c.max_health)
        && ctx
            .db
            .game_creature_template()
            .entry()
            .find(c.entry)
            .is_some_and(|t| flee_eligible(t.creature_type))
}

/// Is this creature ACTIVELY routing — eligible, inside an open rout window on its own engagement row,
/// and able to move? The ONE place that question is answered, so the three engaged sites agree: the
/// chase→rout divert, the rout leg itself, and the swing pass. If only some of them knew about the
/// window, a creature would be diverted out of chasing yet never routed — standing FROZEN instead of
/// fighting. A spent window means not routing, so a creature that has used its rout chases and swings
/// like any other attacker. CC counts as not routing for the same reason: the rout leg is suppressed for
/// a rooted/stunned/feared creature, so it must keep swinging rather than stand silent. [server]
///
/// `pub(crate)` because the swing pass in `combat::swing` is one of the three sites.
pub(crate) fn creature_is_routing(ctx: &ReducerContext, c: &WorldEntity) -> bool {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    ctx.db
        .game_melee_attack()
        .attacker_guid()
        .find(c.guid)
        .is_some_and(|row| rout_window_open(now_ms, row.rout_ends_ms))
        && rout_eligible(ctx, c)
        && !crate::spell::is_self_movement_suppressed(ctx, c.guid)
}

/// SPLINE ADVANCE — the FIRST pass each tick. Interpolates every active creature spline: moves the
/// authoritative position to `lerp(start, dest, elapsed/duration)`, recomputes its grid cell, and clears
/// the spline on arrival (t≥1). This is what replaces `emit_creature_leg`'s old snap-to-leg-end: the auth
/// position now TRACKS what the client renders (lagging by ≤ one tick) instead of LEADING it by a full
/// leg. Runs before the movement/AI passes so they read the freshly-advanced position.
///
/// CC (movement-suppressed: root/stun/poly/fear-frozen) mid-leg → HALT at the current render point: snap
/// the row there, emit a 0-duration stop leg so the CLIENT stops too (no sliding into melee while rooted),
/// and drop the spline. A gone creature's spline is reaped. Position math is absolute (recomputed from
/// start+start_micros each tick), so any number of skipped/dormant ticks never drifts.
fn pass_advance_splines(ctx: &ReducerContext, now_us: u64, now_ms: u32) -> usize {
    let splines = ctx.db.game_creature_spline();
    let entities = ctx.db.game_world_entity();
    let mut n = 0usize;
    for s in splines.iter().collect::<Vec<_>>() {
        let Some(mut e) = entities.guid().find(s.guid) else {
            splines.guid().delete(s.guid); // creature gone → reap the orphan leg
            continue;
        };
        let t = spline_t(now_us, s.start_micros, s.dur_ms);
        let (px, py, pz) = (
            s.sx + (s.dx - s.sx) * t,
            s.sy + (s.dy - s.sy) * t,
            s.sz + (s.dz - s.sz) * t,
        );
        // Same refusal as the emitter: a spline whose endpoints went bad would otherwise write the
        // corruption straight onto the entity every tick. Drop the leg and leave the creature put.
        if !crate::creatures::ai::finite_point(px, py, pz) {
            spacetimedb::log::error!(
                "refused a non-finite spline advance for guid {} — dropping the leg",
                s.guid
            );
            splines.guid().delete(s.guid);
            continue;
        }
        let (gx, gy) = spatial::grid_cell(px, py);
        // CC mid-leg: freeze at the current render point + stop the client (no root-slide into melee).
        if crate::spell::is_self_movement_suppressed(ctx, s.guid) {
            e.x = px;
            e.y = py;
            e.z = pz;
            e.grid_x = gx;
            e.grid_y = gy;
            e.cell = lyracore_shared::spatial::grid_cell_id(gx, gy);
            entities.guid().update(e);
            // 0-dur STOP at the render point (snap-and-hold), through the one relay path.
            emit_move_spline(
                ctx,
                s.guid,
                (px, py, pz),
                (px, py, pz),
                0,
                false,
                now_ms,
                s.map_id,
                s.instance_id,
                (gx, gy),
            );
            continue;
        }
        e.x = px;
        e.y = py;
        e.z = pz;
        e.grid_x = gx;
        e.grid_y = gy;
        e.cell = lyracore_shared::spatial::grid_cell_id(gx, gy);
        e.last_move_ms = now_ms;
        entities.guid().update(e);
        if t >= 1.0 {
            splines.guid().delete(s.guid); // arrived exactly on dest → idle
        }
        n += 1;
    }
    n
}

#[cfg(test)]
mod relay_tripwire {
    /// **The relay has ONE writer.** perf 2.3 moved creature legs onto the AOI-scoped
    /// `game_creature_spline` row and, in the same commit, removed the gateway's global subscription
    /// to `game_creature_move_event` on the stated grounds that "nothing writes the table any more".
    /// FIVE writers were still there — bot legs, the CC freeze, the chase stop,
    /// `encounter::move_creature_to` and Charge — and every one of them went silently undelivered:
    /// the server moved, no client ever saw it. Bots stood frozen in front of players for days.
    ///
    /// A relay whose subscriber is gone fails EXACTLY this quietly, so the invariant is pinned by
    /// scan: no module or package source may INSERT into that table. (Reads/reaps are fine — the
    /// table still exists and `gc.rs` sweeps whatever is left.)
    ///
    /// #357: this used to be a HAND-PICKED file list (`tick.rs` + `encounter.rs` + `spell/cast.rs` +
    /// playerbots), and the list omitted `creatures/pet.rs` — the Follow leg it writes leaked
    /// undelivered rows into this table every sense tick, unbounded, on a live shard, and the scan
    /// never saw it. Scan the WHOLE compiled tree instead — `character_owned_tripwire::scanned_files`
    /// already walks `module/src` plus every installed `packages/*/src`, so a new file (or a moved
    /// one) is covered for free instead of needing a second edit to add it to a list. The old
    /// hand-list also self-excluded ITS OWN body from the scan via a `.split_once("mod tests")` that
    /// only worked for `tick.rs` because that call's own text happens to contain "mod tests" earlier
    /// in the file than the needle — a coincidence, not a real test-module boundary. Build the needle
    /// at RUN time instead so it is never spelled out contiguously in source anywhere, which is
    /// self-exclusion that can't rot.
    ///
    /// Issue #383: this test used to live in `tick.rs`'s `due_timer_tripwire` mod alongside the
    /// decay/respawn and aggro tripwires below. Split with the file it pins: `emit_move_spline`
    /// (the ONE writer this test protects) stays in `tick/mod.rs`, so the test stays here too — the
    /// other two moved to `lifecycle.rs`/`sense.rs` with the passes they actually test.
    #[test]
    fn nothing_writes_the_unsubscribed_move_event_table() {
        let needle = format!("{}{}", "game_creature_move_event()", ".insert");
        for file in crate::tripwires::character_owned_tripwire::scanned_files() {
            let src = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
            assert!(
                !src.contains(&needle),
                "{} inserts into `game_creature_move_event`, which NO subscriber reads since perf \
                 2.3 — the movement it emits will never reach a client. Emit the leg through \
                 `creatures::tick::emit_move_spline` (or `emit_creature_leg`) instead.",
                file.display()
            );
        }
    }
}

