//! The broadcast creature-move event + the creature tick schedule and the scheduled `tick_creatures`
//! shell. [server]/[event]
//!
//! No behavior pass lives here. `tick_creatures` authorizes the firing, resolves coverage and
//! cadence, and runs ONE behavior cycle; the pass list and its load-bearing order are
//! [`crate::creatures::cycle`]'s alone.
//!
//!   - `mod.rs` (this file) — the two tables + the schedule table, the `tick_creatures` shell, the
//!     active-cell sweep and rows-visited evidence logs, the shared candidate gate
//!     `movable_creature`, the rout predicates, and the one spline writer
//!     (`emit_move_spline`/`emit_creature_leg`) every movement decision funnels through, with
//!     the stop between firings (`stop_where_rendered`) and the persist gate for a leg advance
//!     (`advance_needs_persist`).
//!   - [`lifecycle`] — the canonical despawn checklist + decay/respawn/GO-respawn, the
//!     due-time passes that run regardless of proximity.

use lyracore_shared::spatial;
use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{
    game_aura, game_creature_ai_state, game_entity_motion, game_melee_attack,
    game_sessionless_action_consent, game_world_entity, WorldEntity,
};

use super::*;

mod lifecycle;

// The behavior cycle (`creatures::cycle`) owns WHEN each pass runs, so it needs to name them. These
// three are not behavior — they keep their owner here and the cycle only sequences them.
pub(crate) use lifecycle::{pass_decay, pass_gameobject_respawn, pass_respawn};

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
    // The AOI cell index — exactly 3 columns, all matched by equality terms, which is the
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
    /// `(grid_x, grid_y)` packed into ONE indexed value — the AOI subscription's cell key.
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
    /// This leg is a FACING-ONLY packet (the mover does NOT move — `sx/sy/sz` == `dx/dy/dz`,
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
    /// Intermediate points and destination for one linear ground path. None retains one-leg behavior.
    #[default(None::<CreaturePath>)]
    pub path: Option<CreaturePath>,
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
        sense_secs: sense_period_secs_for_interval(interval_micros),
        scope,
    };
    // Kept for the evidence lines below, which outlive the cycle the context is moved into.
    let (sense, global, scope_label) = (
        tick.sense,
        tick.scope.runs_global_passes(),
        scope_label(&tick.scope),
    );

    let outcome = crate::creatures::cycle::run(ctx, tick);

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

/// Read each occupied neighborhood once, preserving map and instance isolation.
/// Authored active objects stay awake without a nearby Character. Pets and combat
/// candidates retain their separate sense-firing discovery. An Idle Bot wakes no cell.
pub(crate) fn active_cell_creatures(
    ctx: &ReducerContext,
    scope: &TickScope,
    sense: bool,
) -> TickSweep {
    let entities = ctx.db.game_world_entity();
    let consents = ctx.db.game_sessionless_action_consent();
    let package_controlled = |guid: u64| consents.character_guid().find(guid).is_some();
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let radius = active_cell_radius(ctx);
    let mut out = std::collections::HashSet::new();
    let mut pets: Vec<u64> = Vec::new();
    let mut in_combat: Vec<u64> = Vec::new();

    // Pet behavior consumes its candidates only on sense firings. The global combat-drop and regen
    // passes consume IN_COMBAT candidates on global sense firings. Keep their shared table-order scan
    // on that cadence; non-sense movement firings no longer read the world to build unused lists.
    if sense {
        let global = scope.runs_global_passes();
        for e in entities.iter() {
            if e.owner_guid != 0 {
                pets.push(e.guid);
            }
            if global && e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
                in_combat.push(e.guid);
            }
        }
    }

    let players: Vec<WorldEntity> = entities
        .by_entry()
        .filter(&0u32)
        .filter(|e| e.is_player() && scope.covers(e.instance_id))
        .filter(|e| !is_idle_bot(e, now_ms, || package_controlled(e.guid)))
        .collect();
    for state in ctx
        .db
        .game_creature_ai_state()
        .by_active_object()
        .filter(&true)
    {
        let Some(e) = entities.guid().find(state.creature_guid) else {
            continue;
        };
        if active_object_enters_scope(e.is_player(), scope.covers(e.instance_id), true) {
            out.insert(e.guid);
        }
    }
    let cells = active_cells(
        players
            .into_iter()
            .map(|p| (p.map_id, p.instance_id, p.x, p.y)),
        radius,
    );
    for cell in cells {
        for creature in entities.by_grid().filter(cell) {
            if !creature.is_player() {
                out.insert(creature.guid);
            }
        }
    }
    TickSweep {
        active: out,
        pets,
        in_combat,
    }
}

fn active_object_enters_scope(is_player: bool, partition_covered: bool, active: bool) -> bool {
    !is_player && partition_covered && active
}

/// How long a Package-controlled Character stands still out of combat before it is an Idle Bot.
const IDLE_BOT_AFTER_MS: u32 = 30_000;

/// Is this Character an Idle Bot? `package_controlled` reports a Sessionless Action Consent row and
/// runs last because it is a lookup. A leg advance stamps the move clock every firing, so a
/// Character on a movement leg is never idle.
fn is_idle_bot(
    character: &WorldEntity,
    now_ms: u32,
    package_controlled: impl FnOnce() -> bool,
) -> bool {
    character.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT == 0
        && now_ms.wrapping_sub(character.last_move_ms) >= IDLE_BOT_AFTER_MS
        && package_controlled()
}

/// One firing's active-cell creature set plus the sense-cadence pet and in-combat candidate lists.
#[derive(Default)]
pub(crate) struct TickSweep {
    /// Creatures within `active_cell_radius` of at least one covered Character that is not an Idle
    /// Bot (work-item 230).
    pub(crate) active: std::collections::HashSet<u64>,
    /// Live pets (`owner_guid != 0`), in table order — the cycle's pet-phase candidate list.
    pub(crate) pets: Vec<u64>,
    /// Units carrying `UNIT_FLAG_IN_COMBAT`, in table order — the candidate list the cycle's combat
    /// exit sweeps, and the flag half of regeneration's in-combat verdict.
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

/// Work-item 233 done-when evidence: log the cast/rout/fear rows-visited drop, in
/// the SAME rare window `log_active_cell_stats` uses (reusing its throttle — no extra per-tick cost).
/// `melee_rows` is the candidate universe BOTH the cycle's cast and rout phases outer-loop (identical
/// gate: "currently the attacker in `game_melee_attack`"); `fear_rows` is what the fear phase
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
        "tick_creatures narrowed passes (work-item 233): cast/rout visit {melee_rows} melee rows, \
         fear visits {fear_rows} aura rows, vs {total_all} total entities each used to scan"
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
/// when another instance is armed/populated), while the `*`-suffixed global passes count what they
/// visited across ALL instances (full table rows for decay and respawn, the harvested candidate
/// list for regen and combat exit) — those grow with world size BY DESIGN and answer 233-style scan
/// questions, not scoping ones.
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
        path: None,
    };
    if ctx.db.game_creature_spline().guid().find(guid).is_some() {
        ctx.db.game_creature_spline().guid().update(row);
    } else {
        ctx.db.game_creature_spline().insert(row);
    }
}

/// Write a FACING-ONLY spline row — `guid` doesn't move (start == dest, `dur_ms` 0) but its
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
        path: None,
    };
    if ctx.db.game_creature_spline().guid().find(guid).is_some() {
        ctx.db.game_creature_spline().guid().update(row);
    } else {
        ctx.db.game_creature_spline().insert(row);
    }
}

#[derive(spacetimedb::SpacetimeType, Clone)]
pub struct CreaturePath {
    pub points: Vec<CreaturePathPoint>,
    pub navigation: crate::nav::NavigationInputs,
}

#[derive(spacetimedb::SpacetimeType, Clone)]
pub struct CreaturePathPoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Publish the already checked path as one movement, shared by server advance and client relay.
/// A mover on a leg starts the path where [`stop_where_rendered`] would stop it, so a renewal
/// between two advance firings does not move the client back.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn emit_creature_path(
    ctx: &ReducerContext,
    mut mover: WorldEntity,
    points: Vec<(f32, f32, f32)>,
    run: bool,
) {
    use lyracore_shared::{constants::speeds, movement_path};
    let Some(&destination) = points.last() else {
        return;
    };
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch() as u64;
    let now_ms = (now_micros / 1000) as u32;
    let spline_id = place_where_rendered(ctx, &mut mover)
        .map_or(now_ms, |previous| next_spline_id(now_micros, previous));
    let start = (mover.x, mover.y, mover.z);
    let length = movement_path::length(start, &points);
    if !length.is_finite() || length <= 0.0 {
        return;
    }
    let speed = if run { speeds::RUN } else { speeds::WALK };
    let duration = (length / speed * 1000.0).ceil().max(1.0) as u32;
    emit_move_spline(
        ctx,
        mover.guid,
        start,
        destination,
        duration,
        run,
        spline_id,
        mover.map_id,
        mover.instance_id,
        (mover.grid_x, mover.grid_y),
    );
    if let Some(mut spline) = ctx.db.game_creature_spline().guid().find(mover.guid) {
        spline.path = Some(CreaturePath {
            navigation: crate::nav::inputs(ctx, mover.map_id),
            points: points
                .iter()
                .map(|&(x, y, z)| CreaturePathPoint { x, y, z })
                .collect(),
        });
        ctx.db.game_creature_spline().guid().update(spline);
    }
    mover.orientation = (points[0].1 - mover.y).atan2(points[0].0 - mover.x);
    mover.last_move_ms = now_ms;
    ctx.db.game_world_entity().guid().update(mover);
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
    let nz = crate::terrain::snap_z(ctx, e.map_id, e.instance_id, to.0, to.1, z_fallback);
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

/// Stop `mover`'s leg where the client renders it now, facing along its Route Path. The row moves
/// to that point and a zero-duration stop there replaces the leg, so the stored position and the
/// stop packet agree with the client. Without this, a stop between two advance firings holds the
/// position the last firing wrote, behind the client. A blocked Route Path segment or changed
/// navigation inputs halt the stop where a leg advance would halt, so it never lands past an
/// obstruction. The caller writes `mover`. A mover with no leg is unchanged.
pub(crate) fn stop_where_rendered(ctx: &ReducerContext, mover: &mut WorldEntity) {
    let Some(previous) = place_where_rendered(ctx, mover) else {
        return;
    };
    let spline_id = next_spline_id(ctx.timestamp.to_micros_since_unix_epoch() as u64, previous);
    let at = (mover.x, mover.y, mover.z);
    emit_move_spline(
        ctx,
        mover.guid,
        at,
        at,
        0,
        false,
        spline_id,
        mover.map_id,
        mover.instance_id,
        (mover.grid_x, mover.grid_y),
    );
}

/// Move `mover` to where a stop now would leave it on its leg, and return that leg's spline id.
/// `None` for a mover with no leg, which stays unchanged. The caller writes `mover`.
fn place_where_rendered(ctx: &ReducerContext, mover: &mut WorldEntity) -> Option<u32> {
    let leg = ctx.db.game_creature_spline().guid().find(mover.guid)?;
    let spline_id = leg.spline_id;
    if let Some(stop) = super::cycle::stop_on_stored_leg(ctx, leg) {
        place_stopped(mover, stop);
    }
    Some(spline_id)
}

/// The id for a leg that replaces one with id `previous`. The client ignores an id that does not
/// exceed the one it replaces, so a second leg in the same millisecond takes `previous + 1`.
fn next_spline_id(now_micros: u64, previous: u32) -> u32 {
    ((now_micros / 1000) as u32).max(previous.wrapping_add(1))
}

/// Move `mover` to `stop`: position, grid address and packed cell together, and the stop's heading
/// when it has one.
fn place_stopped(mover: &mut WorldEntity, stop: super::cycle::Stop) {
    let (grid_x, grid_y) = spatial::grid_cell(stop.at.x, stop.at.y);
    mover.x = stop.at.x;
    mover.y = stop.at.y;
    mover.z = stop.at.z;
    mover.grid_x = grid_x;
    mover.grid_y = grid_y;
    mover.cell = spatial::grid_cell_id(grid_x, grid_y);
    if let Some(heading) = stop.heading {
        mover.orientation = heading;
    }
}

/// Must a leg advance stay in the creature's stored row once the firing ends? The cycle's passes
/// always read the advanced row; only the commit log may skip it. `stored` is the row before the
/// advance, `advanced` the row it wrote, `settled` the row at the end of the firing, and `leg` the
/// mover's spline row then. Arrival, a new leg, a halt, any other write, a cell change, a turn,
/// combat and a Character all keep the advance.
pub(crate) fn advance_needs_persist(
    stored: &WorldEntity,
    advanced: &WorldEntity,
    settled: &WorldEntity,
    leg: Option<&CreatureSpline>,
    now_micros: u64,
) -> bool {
    // Range and line-of-sight checks outside the cycle may read a walking creature up to this far
    // behind, the trade Characters' own heartbeat gate makes.
    let max_drift = crate::world::PERSIST_MAX_DRIFT_YD;
    let drift_sq = (advanced.x - stored.x).powi(2)
        + (advanced.y - stored.y).powi(2)
        + (advanced.z - stored.z).powi(2);
    let same_leg_in_flight = leg.is_some_and(|leg| leg.start_micros < now_micros);
    !same_leg_in_flight
        || settled != advanced
        || stored.is_player()
        || stored.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0
        || (stored.grid_x, stored.grid_y) != (advanced.grid_x, advanced.grid_y)
        || stored.orientation != advanced.orientation
        || drift_sq > max_drift * max_drift
}

/// The shared gate ladder every ENGAGED/table-driven phase (cast, threat retarget, chase, rout and
/// fear) opens its per-candidate loop with: resolve `guid` to a live CREATURE (no PLAYER bit, not
/// dead) whose instance THIS firing's `scope` covers. `None` collapses each site's `let Some(c) = ...
/// else { continue }; if c.is_player() || c.dead { continue }; if !scope.covers(c.instance_id) {
/// continue }` into one check — every call site still increments its own `visited` counter only on
/// `Some`, matching the existing "gate first, then count" order everywhere.
///
/// `pub(crate)` for the cycle's production adapter, which opens the cast, threat-retarget and chase
/// candidate lists with it.
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

/// Is this creature ACTIVELY routing — inside an open rout window on its own engagement row, entitled
/// to that window, and able to move? The ONE place that question is answered, so the three engaged
/// sites agree: the chase→rout divert, the rout leg itself, and the swing pass. If only some of them
/// knew about the window, a creature would be diverted out of chasing yet never routed — standing
/// FROZEN instead of fighting. A spent window means not routing, so a creature that has used its rout
/// chases and swings like any other attacker. CC counts as not routing for the same reason: the rout
/// leg is suppressed for a rooted/stunned/feared creature, so it must keep swinging rather than stand
/// silent. [server]
///
/// Two things entitle a creature to the window. The fixed rout's own gate (`rout_eligible`: below the
/// flee threshold AND of a kind that runs) opens it for the ordinary near-death humanoid. An authored
/// flee opens it for anyone the script says flees, a beast at 30% health included, because the
/// script's window is the only thing that stamped it, and the fixed gate is switched OFF for such a
/// creature. Reading only the fixed gate left an authored flee with an open window and nothing that
/// would move it.
///
/// `pub(crate)` because the swing pass in `combat::swing` is one of the three sites.
pub(crate) fn creature_is_routing(ctx: &ReducerContext, c: &WorldEntity) -> bool {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    ctx.db
        .game_melee_attack()
        .attacker_guid()
        .find(c.guid)
        .is_some_and(|row| rout_window_open(now_ms, row.rout_ends_ms))
        && (rout_eligible(ctx, c) || crate::creatures::eventai::authored_combat(ctx, c.guid).flee)
        && !crate::spell::is_self_movement_suppressed(ctx, c.guid)
}

fn active_cells(
    positions: impl IntoIterator<Item = (u32, u64, f32, f32)>,
    radius: f32,
) -> std::collections::BTreeSet<(u32, u64, i32, i32)> {
    let mut cells = std::collections::BTreeSet::new();
    for (map, instance, x, y) in positions {
        let (gx0, gx1, gy0, gy1) = spatial::covering_cell_box(x, y, radius);
        for gx in gx0..=gx1 {
            for gy in gy0..=gy1 {
                cells.insert((map, instance, gx, gy));
            }
        }
    }
    cells
}

#[cfg(test)]
mod relay_tripwire {
    #[test]
    fn overlapping_characters_read_each_active_cell_once() {
        let cells = super::active_cells([(0, 0, 0.0, 0.0); 1_000], 100.0);
        assert_eq!(cells.len(), 25);
        assert!(cells.contains(&(0, 0, 339, 339)));
        assert!(cells.contains(&(0, 0, 343, 343)));
        assert!(!cells.contains(&(0, 0, 344, 343)));
    }

    #[test]
    fn active_cells_keep_maps_and_instances_separate() {
        let cells = super::active_cells(
            [(0, 0, 0.0, 0.0), (1, 0, 0.0, 0.0), (0, 7, 0.0, 0.0)],
            100.0,
        );
        assert_eq!(cells.len(), 75);
        for partition in [(0, 0), (1, 0), (0, 7)] {
            assert!(cells.contains(&(partition.0, partition.1, 339, 339)));
        }
        assert!(!cells.contains(&(1, 7, 339, 339)));
    }

    #[test]
    fn adjacent_neighborhoods_preserve_their_outer_cells() {
        let cells = super::active_cells([(0, 0, 0.0, 0.0), (0, 0, 50.0, 0.0)], 100.0);
        assert_eq!(cells.len(), 30);
        assert!(cells.contains(&(0, 0, 338, 339)));
        assert!(cells.contains(&(0, 0, 343, 343)));
    }

    #[test]
    fn active_objects_enter_only_their_creature_partition_sweep() {
        assert!(super::active_object_enters_scope(false, true, true));
        assert!(!super::active_object_enters_scope(true, true, true));
        assert!(!super::active_object_enters_scope(false, false, true));
        assert!(!super::active_object_enters_scope(false, true, false));
    }

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
    /// This used to be a HAND-PICKED file list (`tick.rs` + `encounter.rs` + `spell/cast.rs` +
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
    /// This test used to live in `tick.rs`'s `due_timer_tripwire` mod alongside the
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

#[cfg(test)]
mod advance_persist_gate {
    use super::{advance_needs_persist, CreatureSpline};
    use crate::WorldEntity;
    use lyracore_shared::constants::{type_mask, unit_flags};
    use lyracore_shared::spatial;

    const NOW_MICROS: u64 = 60_000_000;
    const NOW_MS: u32 = 60_000;

    /// An out-of-combat creature at `(x, 0)` whose move clock reads `moved_ms`.
    fn creature_at(x: f32, moved_ms: u32) -> WorldEntity {
        let (gx, gy) = spatial::grid_cell(x, 0.0);
        let mut creature = crate::helpers::tests::entity(7, 0, 0, gx, gy);
        creature.x = x;
        creature.last_move_ms = moved_ms;
        creature
    }

    fn leg_started_at(start_micros: u64, dur_ms: u32) -> CreatureSpline {
        CreatureSpline {
            guid: 7,
            start_micros,
            dur_ms,
            sx: 0.0,
            sy: 0.0,
            sz: 0.0,
            dx: 20.0,
            dy: 0.0,
            dz: 0.0,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            cell: 0,
            spline_id: (start_micros / 1000) as u32,
            run: false,
            facing: false,
            facing_angle: 0.0,
            path: None,
        }
    }

    /// One advance to judge: stored at `from_x`, advanced to `to_x` one second into an eight-second
    /// leg, with nothing else writing the row this firing.
    struct Walk {
        stored: WorldEntity,
        advanced: WorldEntity,
        settled: WorldEntity,
        leg: Option<CreatureSpline>,
    }

    impl Walk {
        fn between(from_x: f32, to_x: f32) -> Self {
            Walk {
                stored: creature_at(from_x, NOW_MS - 1_000),
                advanced: creature_at(to_x, NOW_MS),
                settled: creature_at(to_x, NOW_MS),
                leg: Some(leg_started_at(NOW_MICROS - 1_000_000, 8_000)),
            }
        }

        fn rows(&mut self) -> [&mut WorldEntity; 3] {
            [&mut self.stored, &mut self.advanced, &mut self.settled]
        }

        fn persists(&self) -> bool {
            advance_needs_persist(
                &self.stored,
                &self.advanced,
                &self.settled,
                self.leg.as_ref(),
                NOW_MICROS,
            )
        }
    }

    #[test]
    fn a_walk_within_four_yards_leaves_the_stored_row_behind() {
        assert!(!Walk::between(1.0, 4.0).persists());
        assert!(!Walk::between(1.0, 5.0).persists(), "exactly four yards");
    }

    #[test]
    fn a_walk_past_four_yards_is_written() {
        assert!(Walk::between(1.0, 5.5).persists());
    }

    #[test]
    fn a_step_into_another_cell_is_written() {
        let walk = Walk::between(15.0, 18.0);
        assert_ne!(
            walk.stored.grid_x, walk.advanced.grid_x,
            "x=16.67 is a cell edge"
        );
        assert!(walk.persists());
    }

    #[test]
    fn arrival_is_written() {
        let mut walk = Walk::between(1.0, 2.0);
        walk.leg = None;
        assert!(walk.persists());
    }

    #[test]
    fn a_leg_started_or_halted_this_firing_is_written() {
        let mut walk = Walk::between(1.0, 2.0);
        walk.leg = Some(leg_started_at(NOW_MICROS, 3_000));
        assert!(walk.persists(), "a new leg");
        walk.leg = Some(leg_started_at(NOW_MICROS, 0));
        assert!(walk.persists(), "a halt");
    }

    #[test]
    fn another_write_to_the_row_this_firing_keeps_the_advance() {
        let mut walk = Walk::between(1.0, 2.0);
        walk.settled.wp_target = 3;
        assert!(walk.persists());
    }

    #[test]
    fn a_turn_along_the_path_is_written() {
        let mut walk = Walk::between(1.0, 2.0);
        walk.advanced.orientation = 1.5;
        walk.settled.orientation = 1.5;
        assert!(walk.persists());
    }

    #[test]
    fn a_creature_in_combat_is_written() {
        let mut walk = Walk::between(1.0, 2.0);
        for row in walk.rows() {
            row.unit_flags = unit_flags::IN_COMBAT;
        }
        assert!(walk.persists());
    }

    #[test]
    fn a_character_on_a_leg_is_written() {
        let mut walk = Walk::between(1.0, 2.0);
        for row in walk.rows() {
            row.type_mask = type_mask::PLAYER_BIT;
        }
        assert!(walk.persists());
    }
}

#[cfg(test)]
mod stop_between_firings {
    use super::{next_spline_id, place_stopped};
    use crate::creatures::cycle::{Point, Stop};
    use crate::WorldEntity;
    use lyracore_shared::spatial;

    /// A creature the last firing stored at (`x`, 0, 0), in the cell that point lies in.
    fn creature_stored_at(x: f32) -> WorldEntity {
        let (grid_x, grid_y) = spatial::grid_cell(x, 0.0);
        let mut creature = crate::helpers::tests::entity(42, 0, 0, grid_x, grid_y);
        creature.x = x;
        creature.cell = spatial::grid_cell_id(grid_x, grid_y);
        creature
    }

    #[test]
    fn a_stop_past_a_cell_edge_moves_the_creature_into_the_next_cell() {
        // x = 16.67 is a cell edge. The last firing stored the creature short of it.
        let mut creature = creature_stored_at(15.0);
        let before = (creature.grid_x, creature.cell);
        place_stopped(
            &mut creature,
            Stop {
                at: Point {
                    x: 20.0,
                    y: 0.0,
                    z: 0.0,
                },
                heading: Some(1.5),
            },
        );
        let (grid_x, grid_y) = spatial::grid_cell(20.0, 0.0);
        assert_ne!(before.0, grid_x, "the stop must cross the cell edge");
        assert_eq!((creature.x, creature.y, creature.z), (20.0, 0.0, 0.0));
        assert_eq!((creature.grid_x, creature.grid_y), (grid_x, grid_y));
        assert_eq!(creature.cell, spatial::grid_cell_id(grid_x, grid_y));
        assert_ne!(creature.cell, before.1);
        assert_eq!(creature.orientation, 1.5);
    }

    #[test]
    fn a_stop_without_a_heading_keeps_the_creatures_facing() {
        let mut creature = creature_stored_at(15.0);
        creature.orientation = 0.7;
        place_stopped(
            &mut creature,
            Stop {
                at: Point {
                    x: 15.0,
                    y: 0.0,
                    z: 0.0,
                },
                heading: None,
            },
        );
        assert_eq!(creature.orientation, 0.7);
    }

    #[test]
    fn a_stop_in_the_same_millisecond_as_its_leg_still_gets_a_newer_spline_id() {
        assert_eq!(next_spline_id(60_000_400, 60_000), 60_001);
        assert_eq!(next_spline_id(60_000_400, 59_000), 60_000);
    }
}

#[cfg(test)]
mod idle_bot {
    use super::is_idle_bot;
    use crate::WorldEntity;
    use lyracore_shared::constants::{type_mask, unit_flags};

    const NOW_MS: u32 = 3_600_000;

    /// An out-of-combat Character whose move clock last ticked `still_ms` ago.
    fn character_still_for(still_ms: u32) -> WorldEntity {
        let mut character = crate::helpers::tests::entity(42, 0, 0, 0, 0);
        character.type_mask = type_mask::PLAYER_BIT;
        character.last_move_ms = NOW_MS - still_ms;
        character
    }

    #[test]
    fn a_package_controlled_character_still_for_thirty_seconds_is_an_idle_bot() {
        assert!(is_idle_bot(&character_still_for(30_000), NOW_MS, || true));
        assert!(is_idle_bot(&character_still_for(600_000), NOW_MS, || true));
    }

    #[test]
    fn a_character_without_consent_is_never_an_idle_bot() {
        // Humans and test fixture Characters have no Sessionless Action Consent row.
        let human = character_still_for(600_000);
        assert!(!is_idle_bot(&human, NOW_MS, || false));
    }

    #[test]
    fn a_bot_in_combat_is_not_idle() {
        let mut bot = character_still_for(600_000);
        bot.unit_flags = unit_flags::IN_COMBAT;
        assert!(!is_idle_bot(&bot, NOW_MS, || true));
    }

    #[test]
    fn a_bot_on_a_movement_leg_is_not_idle() {
        // The leg advance stamped the move clock on the last firing.
        assert!(!is_idle_bot(&character_still_for(500), NOW_MS, || true));
    }

    #[test]
    fn a_bot_that_moved_within_thirty_seconds_is_not_idle() {
        assert!(!is_idle_bot(&character_still_for(29_999), NOW_MS, || true));
        let mut across_the_wrap = character_still_for(0);
        across_the_wrap.last_move_ms = u32::MAX - 4_999;
        assert!(!is_idle_bot(&across_the_wrap, 5_000, || true));
    }
}
