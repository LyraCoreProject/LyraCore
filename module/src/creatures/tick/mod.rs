//! The broadcast creature-move event + the creature tick schedule, the scheduled `tick_creatures`
//! shell, and the pass bodies the shell no longer sequences. [server]/[event]
//!
//! The pass ORDER lives in [`crate::creatures::cycle`], not here: `tick_creatures` authorizes the
//! firing, resolves coverage and cadence, and runs one behavior cycle. Each pass below is migrated
//! into that module by its own ticket (`.scratch/creature-behavior-cycle/`), so this file shrinks
//! ticket by ticket.
//!
//!   - `mod.rs` (this file) — the two tables + the schedule table, the `tick_creatures` shell, the
//!     active-cell sweep and rows-visited evidence logs, `pass_combat_drop`, the shared "one
//!     movement-leg grammar" toolkit (`PendingLeg`/`drain_legs`/`movable_creature`; the pure
//!     `leg_toward` geometry lives in `creatures::ai`), and the one spline writer
//!     (`emit_move_spline`/`emit_creature_leg`) both `movement` and `sense` depend on.
//!   - [`movement`] — the engaged legs still here: the low-HP rout and fear-flee.
//!   - [`lifecycle`] — the canonical despawn checklist (issue #359) + decay/respawn/GO-respawn, the
//!     due-time passes that run regardless of proximity.
//!   - [`sense`] — cast, threat-retarget, regen.

use lyracore_shared::spatial;
use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{game_aura, game_entity_motion, game_melee_attack, game_world_entity, WorldEntity};

use super::*;

mod lifecycle;
mod movement;
mod sense;

// The behavior cycle (`creatures::cycle`) owns WHEN each pass runs, so it needs to name them. Every
// line here disappears with the pass it exports, as the tickets in `.scratch/creature-behavior-cycle`
// migrate each body into the cycle.
pub(crate) use lifecycle::{pass_decay, pass_gameobject_respawn, pass_respawn};
pub(crate) use movement::{pass_fear_flee, pass_flee};
pub(crate) use sense::{pass_cast, pass_regen, pass_threat_retarget};

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

/// World tick (scheduled, scheduler-only): fires every `MOVE_TICK_SECS` (0.5s) on the seeded
/// catch-all row, plus once per dedicated instance row at that row's own cadence. This reducer is a
/// SHELL: it authorizes the firing, resolves which instances the firing covers and how long its
/// movement step is, then runs ONE behavior cycle. The pass list and its load-bearing order live in
/// `creatures::cycle::run_cycle` and nowhere else.
///
/// MOVEMENT EVERY FIRING, SENSING QUANTIZED: the expensive O(N)-scan sensing passes run about once
/// per 4s (`is_sense_tick_for_interval`) no matter how fast this row fires, so HP-regen rate, wander
/// frequency and respawn cadence stay vanilla; the movement step (`tick_secs_for_interval`) scales
/// with the interval so creature speed is cadence-invariant. Both are byte-identical at the seeded
/// 0.5s row. This is mangos's one-loop-with-recheck-timers model on one scheduled reducer — a single
/// tick, so no cross-scheduler `spline_id` collision.
///
/// INSTANCE SCOPE (work-item 229 — latency smoothing + work avoidance, NOT parallelism; see the
/// `CreatureMoveSchedule` doc): every firing resolves a `TickScope` from ITS OWN schedule row. The
/// catch-all row covers every instance without a dedicated row; a dedicated row covers exactly its
/// instance. Coverage is a PARTITION, so no creature is ever ticked by two rows.
#[reducer]
pub fn tick_creatures(ctx: &ReducerContext, schedule: CreatureMoveSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
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
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch();
    let tick = crate::creatures::cycle::TickContext {
        now_micros: now_micros as u64,
        now_ms: (now_micros / 1000) as u32,
        tick_secs: tick_secs_for_interval(interval_micros),
        sense: is_sense_tick_for_interval(now_micros, interval_micros),
        scope,
    };
    // Kept for the evidence lines below, which outlive the cycle the context is moved into.
    let (sense, global, scope_label) = (
        tick.sense,
        tick.scope.runs_global_passes(),
        scope_label(&tick.scope),
    );

    let outcome = crate::creatures::cycle::run(ctx, tick, interval_micros);

    if global {
        // The 230/233 evidence lines describe the WORLD tick; a dedicated row's numbers would only
        // muddy them (its scoped stats land in `log_pass_stats` below, labeled per scope).
        log_active_cell_stats(ctx, outcome.awake);
        log_narrowed_pass_stats(ctx); // work-item 233 done-when evidence (rows-visited drop)
    }
    log_pass_stats(
        ctx,
        &scope_label,
        sense,
        &outcome.rows_visited,
        interval_micros,
    );
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
/// in the current schema, so this reads every row once. That's the SAME pattern the cycle's aggro
/// phase still uses to build its own player snapshot (this fn doesn't share that one — a further,
/// independent micro-optimization) — but it's a bare bit-check per row, not the expensive per-creature
/// template/faction/stealth logic this item exists to stop running on out-of-range creatures. The
/// rows-visited reduction this item measures is that expensive-logic population, not this cheap scan.
/// Work-item 229: seeds ONLY from players in instances THIS firing's scope covers — `entities_near`
/// is already instance-gated (190 slice 1), so the returned set then contains only covered-instance
/// creatures, which scopes patrol/aggro+assist/return/wander without touching their bodies. With
/// only the seeded catch-all row, `covers()` is `true` for every player → identical set to pre-229.
pub(crate) fn active_cell_creatures(ctx: &ReducerContext, scope: &TickScope) -> TickSweep {
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
#[derive(Default)]
pub(crate) struct TickSweep {
    /// Creatures within `active_cell_radius` of at least one covered player (work-item 230).
    pub(crate) active: std::collections::HashSet<u64>,
    /// Live pets (`owner_guid != 0`), in table order — `pass_pet`'s candidate list.
    pub(crate) pets: Vec<u64>,
    /// Units carrying `UNIT_FLAG_IN_COMBAT`, in table order — `pass_combat_drop`'s candidate list.
    pub(crate) in_combat: Vec<u64>,
}

/// Work-item 230 done-when evidence: log the active-cell rows-visited/total ratio roughly once a
/// minute. `total` (a full non-player-entity count) is deliberately gated behind the SAME rare window
/// so the O(N) count itself never reintroduces the per-tick cost this item removes.
fn log_active_cell_stats(ctx: &ReducerContext, awake: usize) {
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
        "tick_creatures active-cell (work-item 230): {awake}/{total} creatures visited this tick"
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

/// How the pass rows-visited line names this firing's coverage.
fn scope_label(scope: &TickScope) -> String {
    match scope {
        TickScope::CatchAll { dedicated } => {
            format!("global(skipping {} dedicated)", dedicated.len())
        }
        TickScope::Only(n) => format!("instance {n}"),
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
    scope_label: &str,
    sense: bool,
    rows_visited: &[(&'static str, u64)],
    interval_micros: i64,
) {
    let us = ctx.timestamp.to_micros_since_unix_epoch();
    if us.rem_euclid(ACTIVE_CELL_LOG_PERIOD_MICROS) >= interval_micros.max(MOVE_TICK_MICROS) {
        return;
    }
    let body = rows_visited
        .iter()
        .map(|(pass, rows)| format!("{pass}={rows}"))
        .collect::<Vec<_>>()
        .join(" ");
    log::info!(
        "tick_creatures pass rows-visited (work-item 229): scope={scope_label} sense={sense} {body} (*=full-table scan, scales with world not instances)"
    );
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
pub(crate) fn pass_combat_drop(ctx: &ReducerContext, flagged: &[u64]) -> usize {
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

/// The ONE shared creature move-leg writer (work-item 181): every movement decision (the cycle's
/// idle and chase legs, flee, fear-flee) funnels its ALREADY-STEPPED landing point through here, so a
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
/// the cycle's chase phase) and the entity row's `orientation` write; this is purely
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
    // SAME call the cycle's spline-advance halt and its chase stop
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
///
/// `pub(crate)` for the cycle's production adapter, which opens the chase candidate list with it.
pub(crate) fn movable_creature(
    ctx: &ReducerContext,
    guid: u64,
    scope: &TickScope,
) -> Option<WorldEntity> {
    let c = ctx.db.game_world_entity().guid().find(guid)?;
    if c.is_player() || c.dead || !scope.covers(c.instance_id) {
        return None;
    }
    Some(c)
}

/// May this creature rout at all? The HP threshold (`should_flee`) plus the per-TYPE gate
/// (`flee_eligible` — only HUMANOIDS rout; BEASTS/undead/elementals fight to the death). It decides
/// whether a rout may START, never whether one is running — that is `creature_is_routing`. The two
/// non-engaged sites (the cycle's aggro and assist phases) ask THIS question: they act on creatures
/// with no engagement row, which therefore have no rout clock to read. A missing template ⇒ not eligible (safe default).
/// [server]
pub(crate) fn rout_eligible(ctx: &ReducerContext, c: &WorldEntity) -> bool {
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
