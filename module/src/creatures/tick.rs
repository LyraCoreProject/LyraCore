//! The broadcast creature-move event + the patrol schedule, and the slow world tick `tick_creatures` —
//! an ordered pipeline of named passes (patrol / respawn / decay / aggro+assist / cast / threat-retarget
//! / chase / return / wander / regen / flee / fear-flee). Each pass is a module-private free fn; the
//! pipeline order is load-bearing (see the doc comments). [server]/[event]

use lyracore_shared::{constants, spatial};
use spacetimedb::{
    log, reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp,
};

use crate::{
    game_aura, game_corpse_loot, game_corpse_loot_eligible, game_faction_template, game_gameobject,
    game_gameobject_pool_member, game_loot_roll, game_loot_roll_vote, game_melee_attack,
    game_spell, game_world_entity, MeleeAttack, WorldEntity,
};

use super::*;

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
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
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
    stats.add("chase", pass_chase(ctx, &scope, tick_secs)); // movement (step) — AFTER aggro, BEFORE regen
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
    stats.add("flee", pass_flee(ctx, &scope, tick_secs)); // movement — LAST, after regen; engaged-only
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
        spline_id,
        run,
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
    // ONE WRITE PER LEG, and it IS the relay. Was: a `game_creature_move_event` INSERT (globally
    // subscribed — `SELECT * FROM game_creature_move_event` — so every leg was delivered to EVERY
    // connected session and then discarded by most of them via the `created`-set guard) PLUS a
    // spline DELETE+INSERT. Measured at 100 dispersed players: 121.7 inserts/s and 121.6 reaps/s of
    // the event table alone. The spline row is already one-per-creature and already written on every
    // leg, so it carries the relay for free and the delivery becomes AOI-box scoped.
    //
    // SPLINE MODEL (unchanged): the client interpolates start→dest over `duration_ms`; the SERVER
    // advances the authoritative position along the SAME spline each tick in `pass_advance_splines`
    // rather than snapping to the leg END here. e.x/e.y/e.z stay at the leg START.
    let now_us = ctx.timestamp.to_micros_since_unix_epoch() as u64;
    let row = CreatureSpline {
        guid: e.guid,
        start_micros: now_us,
        dur_ms: duration_ms.max(1), // avoid /0 in the lerp; a 0-dur snap just completes next advance
        sx: e.x,
        sy: e.y,
        sz: e.z,
        dx: to.0,
        dy: to.1,
        dz: nz,
        // Grid address of the leg START — the cell the creature currently occupies, so an observer
        // whose box contains it receives the leg that moves it out.
        map_id: e.map_id,
        instance_id: e.instance_id,
        grid_x: e.grid_x,
        grid_y: e.grid_y,
        spline_id: now_ms, // must CHANGE per leg (see the field doc): the client keys interpolation
        run,               // off it, and it is what makes an in-place update read as a NEW leg
    };
    // Update in place when a leg is already active (the common case for anything patrolling);
    // insert only for a creature that was standing still. Replaces an unconditional
    // delete-then-insert — two writes and two subscription deltas per leg.
    if ctx.db.game_creature_spline().guid().find(e.guid).is_some() {
        ctx.db.game_creature_spline().guid().update(row);
    } else {
        ctx.db.game_creature_spline().insert(row);
    }
    e.last_move_ms = now_ms;
    if set_leg_ends {
        // ETA gate (patrol + wander): hold this leg until it lands, no mid-leg re-emit.
        e.leg_ends_ms = now_ms + duration_ms;
    }
    ctx.db.game_world_entity().guid().update(e);
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
        e.last_move_ms = now_ms;
        entities.guid().update(e);
        if t >= 1.0 {
            splines.guid().delete(s.guid); // arrived exactly on dest → idle
        }
        n += 1;
    }
    n
}

/// Pass 1 — patrol: an idle creature with >=2 waypoints walks its route IN ORDER, one segment per leg.
/// ETA-gated (`leg_in_flight`): while the current leg is still animating it emits nothing, so a leg
/// plays to completion instead of being re-thrown every tick. `wp_target` is the route cursor (the
/// `game_creature_waypoint.id` it's walking TO); a fresh/stale cursor re-acquires the NEAREST waypoint,
/// then advances 0→1→…→0. Skips dead, engaged, and self-movement-suppressed creatures (combat AI /
/// fear-flee own those). Work-item 230: SCOPED to `active` — a patrolling creature outside every
/// player's active-cell neighborhood is DORMANT (no leg emitted). This is always safe to freeze: the
/// authoritative row is already snapshotted at the END of its current leg the instant that leg was
/// emitted (see the position writes below — the client is still mid-ANIMATION, but the server's stored
/// x/y/z/`wp_target` are already the completed state), so skipping a dormant tick never leaves the row
/// in a half-applied state. On re-entry the route resumes from the stored `wp_target` cursor exactly as
/// if it had never paused.
/// Work-item 229: instance scope is inherited from `active` (built from covered players only) — no
/// separate gate needed. Returns candidates visited (guids past the active gate).
fn pass_patrol(ctx: &ReducerContext, active: &std::collections::HashSet<u64>) -> usize {
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    let waypoints = ctx.db.game_creature_waypoint();

    // Perf catalog 1.4: INVERTED. This used to scan + sort + dedup the ENTIRE waypoint table (a full
    // cmangos import carries 100k+ `creature_movement` rows) on every 500ms firing, only to throw away
    // everything outside the active set two lines later. Now the small, player-density-scaled `active`
    // set is the outer loop and `by_creature` probes patrol membership — the same candidate set
    // (`active` ∩ has-waypoints), discovered from the cheap side, exactly like `pass_return` does.
    // Sorted so the visit order stays PK-ascending like the old scan's (per-creature work is
    // independent, but a stable order keeps the emitted move-event ids deterministic).
    let mut guids: Vec<u64> = active
        .iter()
        .copied()
        .filter(|g| waypoints.by_creature().filter(g).next().is_some())
        .collect();
    guids.sort_unstable();

    let mut visited = 0usize;
    for guid in guids {
        visited += 1;
        let Some(mut e) = entities.guid().find(guid) else {
            continue;
        };
        // A dead creature (corpse) holds position until it decays — it must not patrol.
        if e.dead {
            continue;
        }
        // A creature engaged in melee yields its patrol to the combat AI (chase/flee pass).
        if crate::combat::is_engaged(ctx, guid) {
            continue;
        }
        // Crowd control: a FROZEN/FEARED creature doesn't patrol (fear-flee walks a feared one instead).
        if crate::spell::is_self_movement_suppressed(ctx, guid) {
            continue;
        }
        // ETA gate: the current segment leg is still animating — don't re-emit or jump. This is what
        // makes a patrol leg play to completion instead of being re-thrown every 0.5s tick (the dither).
        if leg_in_flight(now_ms, e.leg_ends_ms) {
            continue;
        }
        // The route IN ORDER (waypoint PK `id` ASC = insertion order). Carry the id so the cursor tracks
        // WHICH waypoint we're walking to, not just its coords — that's what keeps the traversal in
        // route order instead of jumping to whichever waypoint is farthest.
        let mut wps: Vec<(u64, f32, f32, f32)> = waypoints
            .by_creature() // perf catalog 1.4 — was another full waypoint scan, PER candidate
            .filter(&guid)
            .map(|w| (w.id, w.x, w.y, w.z))
            .collect();
        if wps.len() < 2 {
            continue;
        }
        wps.sort_unstable_by_key(|w| w.0);
        // Next destination: the waypoint AFTER the cursor (`wp_target`), wrapping; if the cursor is
        // unset/stale (0, or not in this route — fresh spawn), acquire the NEAREST waypoint first.
        let pts: Vec<(f32, f32)> = wps.iter().map(|w| (w.1, w.2)).collect();
        let dest_idx = match wps.iter().position(|w| w.0 == e.wp_target) {
            Some(i) => next_waypoint_idx(i, wps.len()),
            None => nearest_waypoint_idx(e.x, e.y, &pts),
        };
        let (wid, dx, dy, dz) = wps[dest_idx];
        let (ddx, ddy, ddz) = (dx - e.x, dy - e.y, dz - e.z);
        let dist = (ddx * ddx + ddy * ddy + ddz * ddz).sqrt();
        if dist < 0.5 {
            // Already on the next waypoint (e.g. just acquired the nearest) — advance the cursor and
            // skip the zero-length leg the client rejects; next tick walks to the following waypoint.
            e.wp_target = wid;
            entities.guid().update(e);
            continue;
        }
        let duration_ms = ((dist / constants::speeds::WALK) * 1000.0) as u32;
        // Advance the route cursor on the row, then funnel through the ONE shared writer (walk, arms
        // the ETA gate). 181: patrol now GROUND-SNAPS its leg like every other pass (work-item 174) —
        // the raw waypoint dz is only the OFF-SLICE fallback; on terrain-enabled slices the dest z is
        // the ground under the waypoint (== dz wherever the waypoint already sits on the ground).
        e.wp_target = wid;
        emit_creature_leg(ctx, e, (dx, dy), dz, duration_ms, false, now_ms, true);
    }
    visited
}

/// Pass 3 — decay (runs before respawn): a corpse whose decay window elapsed is DESTROYed and its
/// respawn timer armed to a FUTURE time (so respawn does NOT re-create it the same tick). Reaps the
/// corpse's item-loot rows.
///
/// Work-item 230 classification: STAYS GLOBAL — `despawn_at` is a due-time, not a proximity concern; a
/// corpse must decay on schedule whether or not a player is anywhere near it.
/// Work-item 229: catch-all firing only, still covering ALL instances (see
/// `TickScope::runs_global_passes`). Returns spawn rows scanned.
fn pass_decay(ctx: &ReducerContext) -> usize {
    let now_ts = ctx.timestamp;
    let entities = ctx.db.game_world_entity();
    let spawns = ctx.db.game_creature_spawn();

    // Decay pass: a corpse whose decay window elapsed is DESTROYed (the on_delete relay
    // fires SMSG_DESTROY_OBJECT, so it vanishes) and its respawn timer armed; the respawn pass below
    // then re-creates it. Gated on the live entity being `dead` so a freshly-spawned creature (whose
    // stale despawn_at is in the past) is never decayed.
    let mut visited = 0usize;
    // RANGE SCAN, not a table scan: `by_despawn_at` walks the index from its low end and stops at
    // `now`, so it visits only corpses whose decay window has actually elapsed — normally none.
    // This works only because `despawn_at` is parked in the far future while a creature is NOT a
    // corpse (`creatures::timer_never`); when every row carried a past stamp, `<= now` matched the
    // entire table and the index would have narrowed nothing.
    let decaying: Vec<u64> = spawns
        .by_despawn_at()
        .filter(..=now_ts)
        .inspect(|_| visited += 1)
        .filter(|s| {
            entities
                .guid()
                .find(s.guid)
                .map(|e| e.dead)
                .unwrap_or(false)
        })
        .map(|s| s.guid)
        .collect();
    // Work-item 187 (review finding #2): the roll deadline and the corpse decay are stamped from
    // the SAME kill timestamp with the SAME 60s constant, and this pass ticks more often than the
    // gc sweep — without resolving due rolls FIRST, decay wins the race and silently discards an
    // unresolved roll (no winner, no ROLL_WON, item gone). Sweeping here guarantees every roll
    // whose deadline has arrived resolves (absent voters auto-pass) before its rows can be reaped.
    if !decaying.is_empty() {
        crate::loot::sweep_loot_rolls(ctx);
    }
    for guid in decaying {
        // Belt over the sweep's braces: if a roll on this corpse is somehow STILL unresolved (a
        // future constant change could put deadlines past decay), keep the corpse one more tick
        // rather than discard a live roll — the sweep resolves it next pass.
        //
        // Issue #50 fix: `sweep_loot_rolls` and the `game_loot_roll` check above only see rolls
        // whose STATE lives on THIS database. In a sharded deployment a roll promoted to realm-core
        // clears its local staging row within ~200ms of kill-time, so neither of the above can see
        // it anymore even though it is still open there — decay would otherwise reap this corpse's
        // `game_corpse_loot` row (below) before `settle_loot_roll` ever runs, silently losing the
        // winner's item (the relay's grant no-ops when it finds no row left to grant against). The
        // corpse's OWN `withheld` bit is a plane-agnostic signal for "a roll still owns this row" —
        // it is set at roll start and cleared only by a grant or an all-pass unlock, on whichever
        // database resolves the roll — so gate on it too, unconditionally, local table or not.
        if ctx
            .db
            .game_loot_roll()
            .by_corpse()
            .filter(&guid)
            .any(|r| !r.resolved)
            || ctx
                .db
                .game_corpse_loot()
                .by_corpse()
                .filter(&guid)
                .any(|l| l.withheld)
        {
            continue;
        }
        entities.guid().delete(guid);
        // Slice 4: reap the corpse's item-loot rows so a decayed corpse doesn't orphan them (the
        // guid is unique per spawn, so a respawn never inherits stale loot). Collect-then-delete.
        let loot = ctx.db.game_corpse_loot();
        let stale: Vec<u64> = loot.by_corpse().filter(&guid).map(|l| l.id).collect();
        for id in stale {
            loot.id().delete(id);
        }
        // Work-item 187: reap this corpse's group-loot eligibility snapshot + any resolved rolls +
        // their votes, so a decayed corpse never orphans them (unresolved rolls block the decay
        // above until the sweep resolves them).
        let eligible = ctx.db.game_corpse_loot_eligible();
        let stale_eligible: Vec<u64> = eligible.by_corpse().filter(&guid).map(|e| e.id).collect();
        for id in stale_eligible {
            eligible.id().delete(id);
        }
        let rolls = ctx.db.game_loot_roll();
        let votes = ctx.db.game_loot_roll_vote();
        let stale_rolls: Vec<u64> = rolls.by_corpse().filter(&guid).map(|r| r.id).collect();
        for roll_id in stale_rolls {
            let stale_votes: Vec<u64> = votes.by_roll().filter(&roll_id).map(|v| v.id).collect();
            for id in stale_votes {
                votes.id().delete(id);
            }
            rolls.id().delete(roll_id);
        }
        if let Some(mut spawn) = spawns.guid().find(guid) {
            // Honor the per-spawn cmangos respawn timer (respawn_secs, seconds from DEATH) over the
            // flat RESPAWN_MICROS default. `respawn_secs == 0` means "not imported" and falls back to
            // the flat timer (byte-identical for every un-imported spawn). The creature's death time is
            // recovered as `despawn_at - CORPSE_DECAY_MICROS` (kill_creature arms
            // `despawn_at = death + CORPSE_DECAY_MICROS`), so a real `respawn_secs` is measured from
            // DEATH, matching cmangos, not from decay.
            spawn.respawn_at = if spawn.respawn_secs == 0 {
                now_ts
                    .checked_add(TimeDuration::from_micros(crate::combat::RESPAWN_MICROS))
                    .unwrap_or(now_ts)
            } else {
                let death_ts = spawn
                    .despawn_at
                    .checked_sub(TimeDuration::from_micros(
                        crate::combat::CORPSE_DECAY_MICROS,
                    ))
                    .unwrap_or(spawn.despawn_at);
                let armed = death_ts
                    .checked_add(TimeDuration::from_micros(
                        spawn.respawn_secs as i64 * 1_000_000,
                    ))
                    .unwrap_or(now_ts);
                // Never arm a respawn in the past (a respawn_secs shorter than the corpse-decay window
                // would otherwise instant-respawn the same tick the corpse decays) — clamp to now.
                if armed > now_ts {
                    armed
                } else {
                    now_ts
                }
            };
            // DISARM the corpse timer — read AFTER `death_ts` above, which needs the armed value.
            // The corpse is gone; leaving `despawn_at` in the past would keep this row in every
            // future decay range scan forever, which is exactly the cost this pass just stopped
            // paying.
            spawn.despawn_at = crate::creatures::timer_never(ctx);
            spawns.guid().update(spawn);
        }
    }
    visited
}

/// Pass 2 — respawn: re-create any creature whose live entity is gone and whose `respawn_at`
/// elapsed, from its persistent spawn record + template. Runs AFTER decay (decay arms a future
/// `respawn_at`, so a just-decayed creature isn't re-spawned this tick).
///
/// Work-item 230 classification: STAYS GLOBAL — `respawn_at` is a due-time; a spawn point must
/// repopulate on schedule whether or not a player is nearby to see it happen.
/// Work-item 229: catch-all firing only, still covering ALL instances. Returns spawn rows scanned.
fn pass_respawn(ctx: &ReducerContext) -> usize {
    let now_ts = ctx.timestamp;
    let entities = ctx.db.game_world_entity();
    let spawns = ctx.db.game_creature_spawn();
    let templates = ctx.db.game_creature_template();

    let mut visited = 0usize;
    // RANGE SCAN (see `pass_decay`): only spawns whose timer has actually fired. A live creature's
    // `respawn_at` sits in the far future (`creatures::timer_never`), so it is not visited at all.
    let due: Vec<CreatureSpawn> = spawns
        .by_respawn_at()
        .filter(..=now_ts)
        .inspect(|_| visited += 1)
        .filter(|s| entities.guid().find(s.guid).is_none())
        .collect();
    for spawn in due {
        let guid = spawn.guid;
        if let Some(tmpl) = templates.entry().find(spawn.entry) {
            super::spawn::insert_creature_entity(
                ctx,
                build_creature_entity(&spawn, &tmpl, ctx.random(), 0),
            );
        }
        // DISARM: the timer has fired and the creature is alive again. This is what keeps the index
        // useful — a fired-but-still-past timer would put the row back in every subsequent scan, and
        // it is also how a freshly IMPORTED spawn (which arms `respawn_at = now` deliberately, so it
        // materialises on the first tick) leaves the scan after that first tick.
        if let Some(mut s) = spawns.guid().find(guid) {
            s.respawn_at = crate::creatures::timer_never(ctx);
            spawns.guid().update(s);
        }
    }
    visited
}

/// GATHER-node respawn (the POOL model): for every DEPLETED node (state==1)
/// whose armed respawn time has elapsed, EITHER re-roll its pool (a POOLED point — the node ROAMS to a
/// different inactive point, holding MAX_ACTIVE) OR flip it state 1→0 in place (a STANDALONE node, the
/// pool_id==0 path). Runs in the SENSE block (~4s) —
/// respawns are minutes-scale, so the slow cadence is ample and a 500ms scan would be wasteful, never
/// late by more than a sense tick. `respawn_at_micros` is a defaulted u64 (NOT a Timestamp — that table
/// is live/populated, so a Timestamp column would abort the publish; teleport-core lesson). The due
/// decision is the unit-tested `gameobject::respawn_due` predicate. Filters to depleted-only inline;
/// collect-then-mutate so we never write `game_gameobject` while iterating it.
/// Perf catalog 1.21 (was: "deliberate simplification: full-table `iter()` every sense tick,
/// defer it"): the scan is now a `by_respawn_at` range probe over armed-and-elapsed rows only. A
/// `by_state` index would still have visited every depleted node every tick; the due-time range
/// visits only the ones actually due.
///
/// Work-item 230 classification: STAYS GLOBAL — `respawn_at_micros` is a due-time (like creature
/// respawn/decay), not proximity; GAMEOBJECTS are also out of this item's creature-ticking scope (see
/// the "defer it" note above, pre-dating 230).
/// Work-item 229: catch-all firing only, still covering ALL instances. Returns GO rows scanned.
fn pass_gameobject_respawn(ctx: &ReducerContext) -> usize {
    let now = ctx.timestamp.to_micros_since_unix_epoch() as u64;
    let gos = ctx.db.game_gameobject();
    let members = ctx.db.game_gameobject_pool_member();
    // Collect the due guids FIRST (collect-then-mutate) so a reroll's delete/insert never writes
    // game_gameobject while we iterate it, and two due rerolls can't race the same pool slot.
    // Perf catalog 1.21: `by_respawn_at` range-scans ARMED-and-elapsed rows only (`0` = not armed, so
    // the `1..=now` range skips every ready node and every non-gather GO for free). `respawn_due` stays
    // as the post-filter, so the predicate — and the result set — are unchanged. `visited` now counts
    // the index candidates rather than the whole table; that IS the 230-style evidence number.
    let mut visited = 0usize;
    let due: Vec<u64> = gos
        .by_respawn_at()
        .filter(1..=now)
        .inspect(|_| visited += 1)
        .filter(|g| crate::gameobject::respawn_due(g.state, g.respawn_at_micros, now))
        .map(|g| g.guid)
        .collect();
    for guid in due {
        // POOLED point? A guid TAGGED as a pool point (structural test, collision-proof — NOT a bare
        // low-48 lookup, which collided with standalone GO guids) → re-roll the pool (deactivate this
        // point, activate a different inactive one) so the active set ROTATES across the zone. A
        // STANDALONE node (tag bit clear → never queries the member table) falls through to the EXISTING
        // in-place state 1→0 flip — byte-identical, no regression.
        match crate::gameobject::pool_point_id_of(guid).and_then(|pid| members.point_id().find(pid))
        {
            Some(member) => crate::gameobject::reroll_pool(ctx, member.pool_id, guid),
            None => {
                if let Some(mut go) = gos.guid().find(guid) {
                    go.state = 0;
                    go.respawn_at_micros = 0;
                    gos.guid().update(go);
                }
            }
        }
    }
    visited
}

/// Will this creature ACTUALLY flee at low HP? Combines the HP threshold (`should_flee`) with the
/// per-TYPE eligibility gate (`flee_eligible` — only HUMANOIDS rout; BEASTS/undead/elementals fight to
/// the death). The ONE place the "who flees" rule is encoded, so every flee site agrees: the aggro-rearm
/// skip, the assist skip, the chase→flee divert, AND the flee leg. Without gating the first three too, a
/// non-eligible near-dead BEAST would be diverted out of chasing yet never actually flee — standing
/// FROZEN instead of fighting to the death. A missing template ⇒ not eligible ⇒ won't flee (safe
/// default). [server]
fn creature_will_flee(ctx: &ReducerContext, c: &WorldEntity) -> bool {
    should_flee(c.health, c.max_health)
        && ctx
            .db
            .game_creature_template()
            .entry()
            .find(c.entry)
            .is_some_and(|t| flee_eligible(t.creature_type))
}

/// Pass 4 — aggro + assist (KEPT TOGETHER — they share `aggro_events`): a hostile creature whose
/// template `aggro_range` covers a nearby player self-engages, and same-faction neighbors within
/// `ASSIST_RADIUS` of an aggroer pile onto the same player (social aggro / pack assist).
///
/// Work-item 230: SCOPED to `active` — both the aggro-candidate scan and the assist-neighbor scan
/// iterate `active` (creature guids reachable from some player's `by_grid` neighborhood) INSTEAD of
/// the full `game_world_entity` table (the "invert the loop, iterate players' neighborhoods" the item
/// calls for). This is lossless: `active`'s radius (`active_cell_radius`) is a proven superset of both
/// scans' real reach — `combat_active_radius` already folds in the assist margin on top of the aggro
/// ceiling, so a creature excluded from `active` cannot legitimately aggro OR assist this tick. Both
/// candidate sets already require "not already attacking" (an engaged creature is never a candidate),
/// so this can only dormant-ize an idle, out-of-reach creature — never an engaged one.
///
/// Work-item 229: instance scope is inherited from `active` (covered players seed it, and the
/// per-pair `instance_id` equality below keeps pairing instance-local) — no separate gate needed.
/// Returns candidates visited (aggro-scan + assist-scan iterations over `active`).
fn pass_aggro_assist(ctx: &ReducerContext, active: &std::collections::HashSet<u64>) -> usize {
    let mut visited = 0usize;
    let entities = ctx.db.game_world_entity();
    let templates = ctx.db.game_creature_template();

    // Aggro pass (vanilla creature AI: mobs aggro you on sight). For each ALIVE creature whose
    // template has `aggro_range > 0` and that is NOT already attacking (no `game_melee_attack` row as
    // attacker — so an engaged creature is never re-armed each tick; idempotent), find the nearest
    // ALIVE player on the same map within `aggro_range` yards (squared-distance compare, matching the
    // patrol/leash/loot ranges) that the creature is HOSTILE to (`compute_hostile`), and arm a
    // creature→player melee row + point the creature's `target_guid` at it. `tick_melee`'s swing pass
    // then makes the creature swing (range/timer gated there) — same arming shape as retaliation and
    // `start_attack`. Collect-then-mutate so we never insert into `game_melee_attack` while iterating
    // any table. Read the template per-creature (aggro_range lives only on the template).
    let melee = ctx.db.game_melee_attack();
    let factions = ctx.db.game_faction_template();
    // Snapshot ALIVE players (same shape as the leash/loot range checks elsewhere), each paired with its
    // stealth flag. A stealthed player is NO LONGER unconditionally invisible — the per-candidate loop
    // below applies a GRADED detect (see `stealth_detect_range`): a stealthed candidate aggroes only when
    // inside the creature's detect range, otherwise it is skipped. The flag is computed ONCE per player
    // here (not re-scanned N times per creature). A stealthed player re-enters at full aggro range the
    // instant stealth breaks (its own swing/cast clears the A_STEALTH aura).
    let players: Vec<(WorldEntity, bool)> = entities
        .iter()
        // `is_aggro_candidate` (not a hand-inlined `!e.dead`): it also excludes a GODMODED GM, who
        // would otherwise be an immortal aggro magnet — nothing ever kills them, so nothing ever
        // disengages, and creatures accumulate on them without bound. See the predicate's doc.
        .filter(|e| e.is_player() && crate::creatures::ai::is_aggro_candidate(e.dead, e.godmode))
        .map(|e| {
            let stealthed = crate::spell::is_stealthed(ctx, e.guid);
            (e, stealthed)
        })
        .collect();
    if !players.is_empty() {
        let mut to_arm: Vec<(u64, u64)> = Vec::new(); // (creature, player)
                                                      // Aggro events armed THIS tick — fuel for the assist expansion below. We capture the
                                                      // aggroer's position + faction (the assist scan measures range from the AGGROER and matches
                                                      // its faction) + instance (work-item 190 slice 1 — a neighbor only assists an aggroer in its
                                                      // OWN instance) and the target player it engaged.
        let mut aggro_events: Vec<(u64, f32, f32, u32, u32, u64, u64)> = Vec::new(); // (guid,x,y,map,faction,instance,target)
                                                                                     // ACTIVE CELLS (work-item 230): iterate the pre-computed active-cell guid set — reachable from
                                                                                     // SOME player's neighborhood via `by_grid` — instead of the full entity table. `entities.guid()`
                                                                                     // re-fetches the live row per guid (cheap: the set size scales with player density, not world
                                                                                     // size).
        for guid in active.iter().copied() {
            visited += 1;
            let Some(c) = entities.guid().find(guid) else {
                continue;
            };
            // Creatures only (no PLAYER bit — `active` never contains one anyway, belt-and-suspenders),
            // alive, not already attacking someone. A PET (owner_guid != 0) is skipped: it must NOT
            // proximity-aggro on the player's behalf — `pass_pet` arms its target off the OWNER's combat,
            // not on sight. Baseline-safe (every wild creature has owner_guid == 0).
            if c.is_player()
                || c.dead
                || c.owner_guid != 0
                || melee.attacker_guid().find(c.guid).is_some()
            {
                continue;
            }
            // Don't re-arm a near-dead creature that WILL flee — otherwise a creature that just fled (and
            // was disengaged) gets re-aggroed the very next tick before regen heals it back above the flee
            // threshold, pinning it in place. Skip it until it recovers. A non-flee-eligible BEAST never
            // fled (so it's still engaged and skipped above anyway) → `creature_will_flee` is false for it,
            // and it keeps behaving like a full-HP creature (fights to the death).
            if creature_will_flee(ctx, &c) {
                continue;
            }
            // Crowd control: an ACTION-blocked creature (stun/poly/fear) cannot ACT — it doesn't aggro on
            // sight (this is an ACTION pass; a ROOTED creature CAN still aggro, it just can't move to
            // close, so root is NOT gated here — a feared one routs and can't aggro). Baseline-safe:
            // `is_action_blocked` is `false` without a stun/poly/fear aura → an un-CC'd creature aggroes
            // exactly as before.
            if crate::spell::is_action_blocked(ctx, c.guid) {
                continue;
            }
            let Some(tmpl) = templates.entry().find(c.entry) else {
                continue;
            };
            // Nearest ALIVE same-map player within this creature's aggro radius that it is HOSTILE to.
            // The radius is computed PER (creature, player) pair: vanilla proximity aggro SCALES with the
            // level difference (`aggro_radius`) — a higher-level mob notices you from farther, and a mob
            // the player out-levels far enough (grey) returns 0 and never aggroes (so a high player isn't
            // swarmed in a low zone). An explicit `tmpl.aggro_range > 0` (e.g. the demo chicken's tuned 8)
            // OVERRIDES the scaling. NOTE: imported creatures carry `aggro_range == 0` (the cmangos ETL
            // doesn't set it), so they rely on the level-scaled default — which is what makes the whole
            // imported population aggro now that faction data is loaded. Hostility uses `compute_hostile`;
            // a missing faction row on either side ⇒ NOT hostile (safe — never aggro on missing data).
            let c_ft = factions.id().find(c.faction_template);
            // Mind Soothe (A_MOD_DETECT_RANGE): a signed additive modifier (YARDS) on a soothed creature's
            // aggro/detection radius — Mind Soothe's amount is NEGATIVE (-10), so it SHRINKS the radius.
            // Computed ONCE per creature (not per candidate player), the SAME "special-case the sensing
            // radius" shape the stealth detect below uses. 0 for an un-soothed creature → radius unchanged
            // (baseline-safe). Clamped ≥ 0 so a large soothe can't produce a negative radius.
            let detect_mod = crate::spell::detect_range_mod(ctx, c.guid);
            let nearest = players
                .iter()
                .filter(|(p, _)| p.map_id == c.map_id && p.instance_id == c.instance_id)
                .filter_map(|(p, stealthed)| {
                    let radius =
                        (aggro_radius(c.level, p.level, tmpl.aggro_range) + detect_mod).max(0.0);
                    if radius <= 0.0 {
                        return None; // grey / no proximity aggro for this (creature, player) pair (or fully soothed)
                    }
                    let (dx, dy, dz) = (p.x - c.x, p.y - c.y, p.z - c.z);
                    let d2 = dx * dx + dy * dy + dz * dz;
                    if d2 > radius * radius {
                        return None;
                    }
                    // Stealth: a stealthed candidate is detected only when CLOSE enough for THIS creature's
                    // level (graded — `stealth_detect_range`). Outside its detect range a stealthed target
                    // stays invisible (skip). A non-stealthed player skips this block (byte-identical path).
                    if *stealthed {
                        let detect = stealth_detect_range(c.level, p.level);
                        if d2 > detect * detect {
                            return None;
                        }
                    }
                    let hostile = match (&c_ft, factions.id().find(p.faction_template)) {
                        (Some(a), Some(b)) => crate::faction::compute_hostile(a, &b),
                        _ => false,
                    };
                    if !hostile {
                        return None;
                    }
                    // 243: proximity aggro requires line of sight — a hostile behind the abbey
                    // wall is not "seen". `has_los` is `true` whenever nav is off (byte-identical
                    // pre-243 behavior) and only raymarches pairs already inside aggro radius.
                    if !crate::nav::has_los(ctx, c.map_id, (c.x, c.y, c.z), (p.x, p.y, p.z)) {
                        return None;
                    }
                    Some((p.guid, d2))
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((player_guid, _)) = nearest {
                to_arm.push((c.guid, player_guid));
                // Record this aggro for the assist pass (aggroer pos+map+faction+instance, engaged
                // player). Map rides along because the neighbor loop scans the WHOLE entity table:
                // without it, same-faction creatures on DIFFERENT maps whose local coordinates
                // coincidentally overlap within ASSIST_RADIUS would cross-assist (190 slice 1
                // review finding — a latent pre-existing gap, not introduced by the instance work).
                aggro_events.push((
                    c.guid,
                    c.x,
                    c.y,
                    c.map_id,
                    c.faction_template,
                    c.instance_id,
                    player_guid,
                ));
            }
        }
        for (creature, player) in to_arm {
            // Arm the engagement — same shape as retaliation/`start_attack`. `try_insert` is a no-op
            // guard if a row appeared concurrently (it can't here, but keeps the pass idempotent).
            if melee.attacker_guid().find(creature).is_none() {
                melee.insert(MeleeAttack {
                    attacker_guid: creature,
                    target_guid: player,
                    last_swing_ms: 0,   // swing on the next melee tick
                    ranged_spell_id: 0, // creature melee aggro
                    last_offhand_swing_ms: 0,
                });
            }
            // Point the creature at its target (the established target_guid pattern), so observers see
            // it facing/targeting the player; `tick_melee` owns the actual swinging.
            if let Some(mut c) = entities.guid().find(creature) {
                if c.target_guid != player {
                    c.target_guid = player;
                    entities.guid().update(c);
                }
            }
            // Notify-hook: direct proximity aggro — "the world noticed you".
            crate::hooks::fire_on_aggro(
                ctx,
                &crate::hooks::AggroPayload {
                    creature_guid: creature,
                    target_guid: player,
                    assist: false,
                },
            );
        }

        // Assist pass (vanilla social aggro / pack behavior): for each creature that aggroed THIS
        // tick, nearby SAME-FACTION creatures pile onto the same player — even passive neighbors the
        // player never got close enough to aggro directly. Range is measured from the AGGROER (the
        // one calling for help), not from the player, so a far-flung pack-mate isn't pulled in.
        //
        // Additive + baseline-safe: a LONE aggroer has no same-faction neighbor in `ASSIST_RADIUS`,
        // so `to_assist` stays empty and behavior is identical to today (the seeded login Chicken is
        // alone → calm login unchanged). The assist deliberately does NOT gate on the neighbor's
        // `aggro_range` — a passive neighbor still answers a pack-mate's call.
        //
        // Snapshot the neighbors to arm first (collect-then-mutate), so we never write
        // `game_melee_attack` while iterating entities/the melee table — mirrors the aggro/flee passes.
        // The `already_assigned` set guards against arming the same neighbor twice when it sits in
        // assist range of two different aggroers this tick.
        if !aggro_events.is_empty() {
            // Work-item 233 (cosmetic, free): sort the small aggro-events list by aggroer guid BEFORE
            // the assist scan below picks one via `.iter().find(..)`. `active` is a `HashSet`, so the
            // order aggro events were pushed in (and therefore which same-faction aggroer a neighbor in
            // range of TWO of them would answer) was hash-iteration-order-dependent — deterministic
            // within one run, but not a meaningful tie-break and not reproducible across runs/builds.
            // Sorting here makes the tie-break the LOWEST aggroer guid, a stable and cheap fix (small
            // list — at most one entry per creature that aggroed this tick) that changes nothing when
            // there's no tie (the overwhelmingly common case: one aggroer, no assist candidates in range
            // of a second one).
            aggro_events.sort_unstable_by_key(|e| e.0);
            let mut to_assist: Vec<(u64, u64)> = Vec::new(); // (neighbor, target player)
            let mut already_assigned: std::collections::HashSet<u64> =
                std::collections::HashSet::new();
            // ACTIVE CELLS (work-item 230): same `active` set as the aggro scan above, not a re-narrowed
            // one — `active_cell_radius` already folds `ASSIST_RADIUS` on top of the aggro ceiling
            // specifically so a real assist neighbor (within `ASSIST_RADIUS` of an aggroer who is itself
            // within the aggro ceiling of a player) is always inside this same set.
            for guid in active.iter().copied() {
                visited += 1;
                let Some(n) = entities.guid().find(guid) else {
                    continue;
                };
                // Same creature predicate as the aggro pass: creatures only (no PLAYER bit), alive,
                // not already an attacker (don't double-arm / don't yank it off its current target —
                // a neighbor already engaged by the direct pass is skipped here), and not near-dead. A PET
                // (owner_guid != 0) never answers a wild-creature assist call (it follows its owner only).
                if n.is_player()
                    || n.dead
                    || n.owner_guid != 0
                    || melee.attacker_guid().find(n.guid).is_some()
                    || creature_will_flee(ctx, &n)
                {
                    continue;
                }
                // Crowd control: an ACTION-blocked neighbor (stun/poly/fear) cannot ACT — it doesn't answer
                // a pack-mate's call (an ACTION pass; a ROOTED neighbor still assists, it just can't close;
                // a FEARED one routs). Baseline-safe: `false` without a stun/poly/fear aura → unchanged assist.
                if crate::spell::is_action_blocked(ctx, n.guid) {
                    continue;
                }
                // Find an aggroer this neighbor should assist: a DIFFERENT creature, same MAP and
                // same faction (`faction_template` — a simple, safe "same kind"; real assist uses
                // faction friendship, a future refinement), same instance (work-item 190 slice 1),
                // within `ASSIST_RADIUS` of the aggroer's position.
                let assist = aggro_events.iter().find(
                    |(ag_guid, ag_x, ag_y, ag_map, ag_ft, ag_instance, _)| {
                        *ag_guid != n.guid
                        && *ag_map == n.map_id
                        && *ag_ft == n.faction_template
                        && *ag_instance == n.instance_id
                        && within_assist_radius(*ag_x, *ag_y, n.x, n.y)
                        // 243: a neighbor answers only a pack-mate it can SEE — no assist calls
                        // through the abbey wall (the second-thug-through-the-wall live find).
                        // `has_los` is `true` whenever nav is off (byte-identical pre-243).
                        && crate::nav::has_los(ctx, n.map_id, (n.x, n.y, n.z), (*ag_x, *ag_y, n.z))
                    },
                );
                if let Some((_, _, _, _, _, _, target_player)) = assist {
                    // Guard a neighbor in range of two aggroers from being queued twice this tick.
                    if already_assigned.insert(n.guid) {
                        to_assist.push((n.guid, *target_player));
                    }
                }
            }
            for (neighbor, player) in to_assist {
                // Re-check no melee row appeared (collect-then-mutate means the table was stable while
                // we scanned; this keeps the insert idempotent, matching the direct-aggro arming).
                if melee.attacker_guid().find(neighbor).is_none() {
                    melee.insert(MeleeAttack {
                        attacker_guid: neighbor,
                        target_guid: player,
                        last_swing_ms: 0,   // swing on the next melee tick
                        ranged_spell_id: 0, // assist aggro is melee
                        last_offhand_swing_ms: 0,
                    });
                }
                if let Some(mut c) = entities.guid().find(neighbor) {
                    if c.target_guid != player {
                        c.target_guid = player;
                        entities.guid().update(c);
                    }
                }
                // Notify-hook: pack-assist aggro (a neighbor answering the call).
                crate::hooks::fire_on_aggro(
                    ctx,
                    &crate::hooks::AggroPayload {
                        creature_guid: neighbor,
                        target_guid: player,
                        assist: true,
                    },
                );
            }
        }
    }
    visited
}

// The per-caster work list the pass builds: `(caster, level, ordered candidates)`, documented by the comment above it.
#[allow(clippy::type_complexity)]
/// Pass 5 — cast: an engaged caster creature casts from its spell ROTATION (rank 20) — or its single
/// `game_creature_cast` spell when it has no rotation rows. Runs after aggro/assist, before chase (a
/// caster casts rather than only closing); `resolve_cast_at` gates GCD/cost/range/cooldown internally.
/// Never moves/disengages anything.
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — every candidate here must
/// currently be the ATTACKER in a `game_melee_attack` row (combat-engaged), and the item requires an
/// engaged creature to never sleep regardless of distance ("a player could drag one far away").
///
/// Work-item 233: outer-loops `game_melee_attack` directly (the pass_chase/pass_threat_retarget
/// precedent) instead of `entities.iter()` + a per-row `melee.attacker_guid().find(..)` gate. Every
/// candidate here was ALREADY required to be a melee attacker (the `let Some(row) = ... else continue`
/// this replaces) — nothing that used to reach the cast logic is excluded, and nothing new is admitted;
/// only how the candidate set is DISCOVERED changed (small table outer loop vs full entity scan +
/// inline filter), which visits the identical set of creatures.
///
/// Work-item 229: each candidate is additionally gated on `scope.covers(c.instance_id)` — the caster
/// and its melee target share an instance by construction (arming is same-instance-gated everywhere),
/// so the ATTACKER's instance is the pair's. With only the catch-all row this admits every candidate
/// (equivalence: ai.rs `tick_scope_default_config_…`). Returns covered candidates visited.
fn pass_cast(ctx: &ReducerContext, scope: &TickScope) -> usize {
    let mut visited = 0usize;
    let entities = ctx.db.game_world_entity();

    // Cast pass (caster-type creature AI). For each ALIVE creature (no PLAYER bit, not dead) that is the
    // ATTACKER in a `game_melee_attack` row, choose an action:
    //   - ROTATION (rank 20): if its entry has `game_creature_spell` rows, evaluate them highest-priority
    //     first and collect the ones whose CONDITION holds (heal-when-low / buff-if-missing / debuff-if-
    //     missing / always-nuke), each with its derived cast target (self for heal/buff, the melee target
    //     for nuke/debuff). At cast time the survivors are attempted in priority order until one is ready.
    //   - LEGACY: no rotation rows → fall back to the single `game_creature_cast` spell at the target
    //     (existing single-spell casters are byte-identical — baseline-safe).
    //   - NEITHER → never casts (baseline-safe).
    // The cadence is each spell's OWN GCD/cooldown — `resolve_cast_at` enforces it and returns `Err` when
    // not ready, which we treat as "try the next candidate" (so a rotation whose top action is on cooldown
    // still fires a ready lower-priority one this tick), casting AT MOST ONE action per creature per tick.
    //
    // It runs AFTER aggro/assist (a creature aggroed/assisting THIS tick can already cast) and BEFORE
    // chase (a caster that CAN cast should cast, not merely close — casting is ranged). Casting never
    // moves the creature, touches its melee row, or disengages it.
    //
    // `resolve_cast_at` writes only `game_aura`/`game_world_entity`/`game_spell_cooldown` — NOT the
    // creature rows we iterate — but to stay safe against mutating `game_world_entity` while iterating it
    // we SNAPSHOT the per-creature candidate lists first (collect-then-call), then loop and cast. The
    // condition reads (HP, `game_aura`) happen in the snapshot phase (reads only).
    let casts = ctx.db.game_creature_cast();
    let rotations = ctx.db.game_creature_spell();
    let melee_cast = ctx.db.game_melee_attack();
    // (caster_guid, caster_level, ordered candidate (spell_id, cast_target) list)
    let mut to_cast: Vec<(u64, u8, Vec<(u32, u64)>)> = Vec::new();
    // Work-item 233: outer-loop the small melee-engaged table (PK `attacker_guid`, one row per
    // attacker) instead of every entity. `row.attacker_guid` is by construction "currently a melee
    // attacker" — exactly the gate the old `.find(&c.guid)` applied AFTER scanning every entity — so
    // this visits the identical candidate set via the smaller table.
    for row in melee_cast.iter() {
        let Some(c) = entities.guid().find(row.attacker_guid) else {
            continue;
        };
        // Creatures only (no PLAYER bit), alive — same predicate as the aggro/chase/flee passes. A
        // player's own melee row (they auto-attack too) is skipped here exactly as it was skipped by
        // the old `entities.iter()` loop's `is_player` check.
        if c.is_player() || c.dead {
            continue;
        }
        // Work-item 229: only this firing's covered instances (see the fn doc).
        if !scope.covers(c.instance_id) {
            continue;
        }
        visited += 1;
        // Crowd control: an ACTION-blocked creature (stun/poly/fear) cannot ACT — it doesn't cast (a
        // ROOTED caster CAN still cast — ranged, no movement; a FEARED one routs and can't). Baseline-safe:
        // `false` without a stun/poly/fear aura → unchanged casting.
        if crate::spell::is_action_blocked(ctx, c.guid) {
            continue;
        }
        // 171: MID-CAST — a creature with a live `game_pending_cast` row is busy casting; skip it.
        // Load-bearing: `begin_cast` writes no cooldown until COMPLETION, so without this guard the
        // 500ms tick would re-enter begin_cast, whose stale-row sweep deletes + restarts the pending
        // cast every tick — the cast would never finish.
        {
            use crate::spell::game_pending_cast;
            if ctx
                .db
                .game_pending_cast()
                .iter()
                .any(|p| p.caster_guid == c.guid)
            {
                continue;
            }
        }
        let target_guid = row.target_guid;
        let mut rot: Vec<CreatureSpell> = rotations.by_entry().filter(&c.entry).collect();
        let candidates: Vec<(u32, u64)> = if rot.is_empty() {
            // Legacy single-spell fallback (no rotation rows).
            match casts.creature_entry().find(c.entry) {
                Some(cast) => vec![(cast.spell_id, target_guid)],
                None => continue,
            }
        } else {
            // Rotation: highest priority first (ties by id → deterministic), keep the eligible ones.
            rot.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));
            rot.iter()
                .filter_map(|r| creature_cast_eligibility(ctx, &c, r, target_guid))
                .collect()
        };
        // 243: a hostile cast needs line of sight to its victim — drop enemy-targeted
        // candidates when the melee target is LoS-blocked (self heals/buffs stay; the creature
        // melees/chases instead, and the chase leg paths around the geometry). `has_los` is
        // `true` whenever nav is off — byte-identical pre-243 casting.
        let candidates: Vec<(u32, u64)> = if candidates.iter().any(|&(_, t)| t != c.guid) {
            let los = entities
                .guid()
                .find(target_guid)
                .map(|t| crate::nav::has_los(ctx, c.map_id, (c.x, c.y, c.z), (t.x, t.y, t.z)))
                .unwrap_or(true);
            if los {
                candidates
            } else {
                candidates
                    .into_iter()
                    .filter(|&(_, t)| t == c.guid)
                    .collect()
            }
        } else {
            candidates
        };
        if !candidates.is_empty() {
            to_cast.push((c.guid, c.level as u8, candidates));
        }
    }
    for (caster_guid, level, candidates) in to_cast {
        // Attempt the eligible actions in priority order; cast AT MOST ONE per tick. `resolve_cast_at`
        // gates GCD/cost/range/cooldown (Err = not ready) — so the first READY action fires and the rest
        // are skipped; if none are ready the creature just melees this tick.
        for (spell_id, cast_target) in candidates {
            // 171: route through `begin_cast` so a TIMED creature spell gets a real `game_pending_cast`
            // row + a START event carrying `cast_time_ms` — observers see the mob's cast bar, and the
            // player-side interrupt machinery (Kick/Counterspell/pushback, all caster-guid-agnostic)
            // gets something to hit. Instant spells self-route to `resolve_cast_at` inside — byte-
            // identical to the old direct call.
            if crate::spell::begin_cast(ctx, caster_guid, spell_id, level, cast_target, false, None)
                .is_ok()
            {
                break;
            }
        }
    }
    visited
}

/// Evaluate one rotation row's CONDITION against the live state (rank 20): `Some((spell_id, cast_target))`
/// if the row should fire, else `None`. The cast target is derived from the condition — self for a heal
/// (SELF_HP_BELOW_PCT) / buff (SELF_MISSING_AURA), the melee `target_guid` for a nuke (ALWAYS) / debuff
/// (TARGET_MISSING_AURA). Reads only HP + `game_aura` (no writes). An unknown condition never fires
/// (forward-compatible). [server]
fn creature_cast_eligibility(
    ctx: &ReducerContext,
    creature: &WorldEntity,
    row: &CreatureSpell,
    target_guid: u64,
) -> Option<(u32, u64)> {
    let cast_target = match row.condition {
        cast_condition::ALWAYS => Some(target_guid),
        cast_condition::SELF_HP_BELOW_PCT => hp_pct_below(
            creature.health,
            creature.max_health,
            row.condition_value as u32,
        )
        .then_some(creature.guid),
        cast_condition::TARGET_MISSING_AURA => {
            (!crate::spell::has_aura(ctx, target_guid, row.spell_id)).then_some(target_guid)
        }
        cast_condition::SELF_MISSING_AURA => {
            (!crate::spell::has_aura(ctx, creature.guid, row.spell_id)).then_some(creature.guid)
        }
        _ => None,
    };
    cast_target.map(|t| (row.spell_id, t))
}

/// Pass 6 — threat retarget: an engaged creature re-points at its HIGHEST-THREAT source (strict
/// hysteresis — switches only when a second source out-threats the current target). Neither moves nor
/// acts, so CC does not gate it.
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — already iterates the melee
/// table's engaged rows (not the full entity table), and threat resolution must keep working for a
/// creature a player dragged far away, per the item's engaged-always-active rule.
/// Work-item 229: gated per candidate on `scope.covers(c.instance_id)` (attacker's instance = the
/// pair's, same as pass_cast). Returns covered candidates visited.
fn pass_threat_retarget(ctx: &ReducerContext, scope: &TickScope) -> usize {
    let mut visited = 0usize;
    let entities = ctx.db.game_world_entity();

    // Threat retarget pass (vanilla aggro: an engaged creature attacks its HIGHEST-THREAT source, not
    // merely whoever it first aggroed). For each ALIVE creature that is the attacker in a melee row, pick
    // the top VALID threat source (`threat::top_threat_target` — alive, same-map, in-world); if it
    // STRICTLY out-threats the creature's CURRENT target, re-point BOTH the melee row's target and the
    // entity's `target_guid` at it. The chase pass (next) and the swing tick then follow the new target.
    //
    // The strict-greater compare is HYSTERESIS: an empty table (no damage dealt yet) or a single-source
    // table (one attacker) leaves the target untouched — byte-identical to the proximity/retaliation
    // behavior. The switch fires only when a SECOND source out-threats the first (e.g. a healer/DPS pulls
    // aggro off the puller, or a taunt tops threat) — exactly the multiplayer threat mechanic. CC does NOT
    // gate this: choosing a target is neither moving nor acting (the swing/chase passes own the stun/root
    // gates); a stunned creature still tracks who it WILL attack when the stun lifts.
    //
    // Snapshot the engaged creatures from the melee table FIRST (collect `(creature, current_target)`),
    // then resolve top-threat + mutate — never write the melee/entity tables while iterating them. The
    // melee snapshot includes player→creature rows; those are filtered out by the `is_player` attacker
    // check (a player has no threat table and is never retargeted by threat).
    let melee_threat = ctx.db.game_melee_attack();
    let engaged: Vec<(u64, u64)> = melee_threat
        .iter()
        .map(|a| (a.attacker_guid, a.target_guid))
        .collect();
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    let mut retargets: Vec<(u64, u64)> = Vec::new(); // (creature, new_target)
    for (creature, current_target) in engaged {
        // Creatures only (no PLAYER bit), alive — same predicate as the other AI passes.
        let Some(c) = entities.guid().find(creature) else {
            continue;
        };
        if c.is_player() || c.dead {
            continue;
        }
        // Work-item 229: only this firing's covered instances (see the fn doc).
        if !scope.covers(c.instance_id) {
            continue;
        }
        visited += 1;
        // TAUNT FORCED-TARGET window: a live lock PINS the creature on the taunter regardless of the
        // threat table — the vanilla taunt rule. Validity + lazy reaping of an expired/invalid lock
        // (window over, taunter dead/gone/cross-map) live in `threat::forced_target`; a `None` resumes
        // the normal top-threat compare below.
        if let Some(pinned) = crate::threat::forced_target(ctx, creature, now_ms) {
            if current_target != pinned {
                retargets.push((creature, pinned));
            }
            continue; // pinned — the threat compare is suspended for the window
        }
        // Top valid threat source; an empty/all-invalid table → keep the current target (None).
        let Some(top) = crate::threat::top_threat_target(ctx, creature) else {
            continue;
        };
        // Switch only on a STRICTLY higher threat than the current target (hysteresis — a tie never
        // flaps). A current target absent from the table reads 0, so any real damage-dealer out-threats a
        // pure proximity puller that never hit the creature.
        if top != current_target
            && crate::threat::threat_of(ctx, creature, top)
                > crate::threat::threat_of(ctx, creature, current_target)
        {
            retargets.push((creature, top));
        }
    }
    for (creature, new_target) in retargets {
        // Re-point the engagement (PK = attacker_guid, unchanged) — keep `last_swing_ms` so retargeting
        // does NOT reset the swing cadence (the creature keeps swinging on its own timer at the new foe).
        if let Some(mut row) = melee_threat.attacker_guid().find(creature) {
            row.target_guid = new_target;
            melee_threat.attacker_guid().update(row);
        }
        // Point the entity at the new target (observers see it face/target the new foe).
        if let Some(mut c) = entities.guid().find(creature) {
            if c.target_guid != new_target {
                c.target_guid = new_target;
                entities.guid().update(c);
            }
        }
    }
    visited
}

/// Pass 7 — chase: an engaged creature whose target sits OUT of melee but within leash steps toward
/// it (a run leg). Runs AFTER aggro (a creature aggroed this tick can start closing) and BEFORE regen
/// (regen's in-combat gate then skips the still-engaged chaser, so the move isn't reverted).
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — the item calls this pass out by
/// name ("make sure the movement/chase pass doesn't sleep an engaged creature"). It already iterates
/// `game_melee_attack`'s engaged rows (never the full entity table), so it was O(active combats) before
/// this item and stays that way — a player who drags a creature outside every active cell must still
/// see it keep chasing/swinging.
/// Work-item 229: gated per candidate on `scope.covers(c.instance_id)` — WITHOUT this gate a
/// dedicated instance row + the catch-all would BOTH step the same chaser each of their firings
/// (double/6× chase speed), the multiplied-work bug this item exists to prevent.
///
/// ⚠ `_tick_secs` is the firing row's own interval and is ACCEPTED BUT NOT READ here: `chase_step`
/// sizes its leg from the constant `MOVE_TICK_SECS`, not from the per-firing value, so a dedicated
/// row ticking faster than the default steps the same distance per firing and chases FASTER in
/// wall-clock terms. Only `pass_return` / `pass_fear_flee` are genuinely cadence-invariant today.
/// The parameter is kept (rather than dropped) so wiring it through stays a one-line change.
/// Returns covered candidates visited.
/// 049: the longest ENEMY-cast range (yd) creature `entry` can bring to bear — rotation
/// nukes/debuffs (conditions ALWAYS / TARGET_MISSING_AURA target the melee victim) plus the
/// legacy `game_creature_cast` spell, mapped through `game_spell.range_yd`. 0 = not an
/// offensive caster (melee-only mobs, pure healers/buff-bots) — the chase pass is then
/// byte-identical to pre-049.
fn caster_hold_range_yd(ctx: &ReducerContext, entry: u32) -> f32 {
    let spells = ctx.db.game_spell();
    let mut max_r = 0u32;
    for r in ctx.db.game_creature_spell().by_entry().filter(&entry) {
        if matches!(
            r.condition,
            cast_condition::ALWAYS | cast_condition::TARGET_MISSING_AURA
        ) {
            if let Some(h) = spells.spell_id().find(r.spell_id) {
                max_r = max_r.max(h.range_yd);
            }
        }
    }
    if let Some(c) = ctx.db.game_creature_cast().creature_entry().find(entry) {
        if let Some(h) = spells.spell_id().find(c.spell_id) {
            max_r = max_r.max(h.range_yd);
        }
    }
    max_r as f32
}

fn pass_chase(ctx: &ReducerContext, scope: &TickScope, _tick_secs: f32) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    // Chase pass (vanilla creature AI: an engaged mob closes the gap on a target that ran out of
    // melee range). For each ALIVE creature that is the ATTACKER in a `game_melee_attack` row, look up
    // its target and the squared distance. Step it toward the target only when it's OUT of melee but
    // still WITHIN leash (`CHASE_MELEE_SQ < dist² <= CHASE_LEASH_SQ`):
    //   - dist² <= melee  → already in range; `tick_melee` swings — chasing would walk onto the target.
    //   - dist²  > leash  → combat's leash pass disengages it; we don't drag it home, so leave it.
    // `chase_step` moves up to one tick of run (`RUN*MOVE_TICK_SECS` — the 0.5s movement tick) but stops
    // ~4 yd short, landing just inside the 5-yd melee band so the next swing connects; re-pathed every
    // tick toward the LIVE target (not a 4s-stale snapshot) so it stays glued to a fleeing player.
    //
    // The chaser stays ENGAGED — we do NOT touch its melee row — so the regen passes below skip it
    // (their `in_combat` gate excludes engaged guids), and they never re-write its row from a stale
    // snapshot and REVERT the move. That's why this runs AFTER aggro (so a creature aggroed THIS tick
    // can immediately start closing) and BEFORE regen (so regen sees the post-move row, not the other
    // way round). Snapshot the legs first (collect-then-mutate), so we never write the entity table
    // while iterating it — mirrors the patrol/aggro/flee passes. Emit a `CreatureMoveEvent` leg AND
    // advance the authoritative position, exactly like the patrol pass, so the client animates the
    // close-in via SMSG_MONSTER_MOVE.
    let melee_chase = ctx.db.game_melee_attack();
    // (creature_guid, dest_x, dest_y, z_fallback=target_z, duration_ms) — 181: the snap + the leg
    // start (the mover's live position) now come from `emit_creature_leg` in the drain loop below.
    let mut to_chase: Vec<(u64, f32, f32, f32, u32)> = Vec::new();
    // Iterate the ENGAGED rows (one per attacker; few) rather than scanning every entity — at the 0.5s
    // tick this keeps chase O(active combats), not O(world). The attacker must be a CREATURE (a player's
    // own attack row is skipped via `is_player`).
    for row in melee_chase.iter() {
        let Some(c) = entities.guid().find(row.attacker_guid) else {
            continue;
        };
        if c.is_player() || c.dead {
            continue;
        }
        // Work-item 229: only this firing's covered instances (see the fn doc).
        if !scope.covers(c.instance_id) {
            continue;
        }
        visited += 1;
        // CC: a FROZEN/FEARED creature doesn't chase (fear-flee walks a feared one AWAY instead). It
        // stays engaged, so a frozen one still swings if the target is already in range.
        if crate::spell::is_self_movement_suppressed(ctx, c.guid) {
            continue;
        }
        // A near-dead creature that WILL flee is diverted out of chasing — the flee pass (last) is its sole
        // mover, so it never emits both a chase leg and a flee leg (same spline_id → client rejects the
        // 2nd). A non-flee-eligible BEAST (`creature_will_flee` false) keeps chasing instead, so it runs the
        // target down to the death rather than standing frozen at low HP.
        if creature_will_flee(ctx, &c) {
            continue;
        }
        let Some(t) = entities.guid().find(row.target_guid) else {
            continue;
        };
        let (dx, dy, dz) = (t.x - c.x, t.y - c.y, t.z - c.z);
        let dist_sq = dx * dx + dy * dy + dz * dz;
        // Beyond leash → combat disengages it (don't chase).
        if dist_sq > CHASE_LEASH_SQ {
            continue;
        }
        // STAND + swing ONLY when in melee reach AND the target is STATIONARY. A MOVING (kiting) target
        // keeps the mob CHASING — vanilla mobs run a kiter down continuously and swing on the move; they
        // plant into attack stance only when the target stops. Gating the melee-skip on the target being
        // still is what removes the run→attack-stance→run flicker while chasing a moving player. The
        // melee scheduler still swings whenever in range, so a chasing mob attacks on the move.
        let target_moving = now_ms.wrapping_sub(t.last_move_ms) < CHASE_TARGET_MOVING_MS;
        if dist_sq <= CHASE_MELEE_SQ && !target_moving {
            // Cancel any in-flight leg so a mob that was lead-chasing a now-stopped target doesn't ride
            // PAST it (pass_advance would otherwise finish the leg). Stop the client at the current point.
            if ctx.db.game_creature_spline().guid().find(c.guid).is_some() {
                // 0-dur STOP where it stands — replaces the in-flight leg rather than deleting it,
                // so the client is TOLD to halt instead of being left interpolating to the old dest.
                emit_move_spline(
                    ctx,
                    c.guid,
                    (c.x, c.y, c.z),
                    (c.x, c.y, c.z),
                    0,
                    false,
                    now_ms,
                    c.map_id,
                    c.instance_id,
                    (c.grid_x, c.grid_y),
                );
            }
            continue;
        }
        // 049: an OFFENSIVE CASTER holds at spell range instead of face-tanking — vanilla casters
        // stand and cast (pass_cast already ran this tick; its range/GCD gates fire the spell).
        // Hold iff the target sits inside ~90% of its longest enemy-cast range AND is LoS-visible;
        // a wall-blocked caster keeps chasing (pass_cast drops its enemy candidates on the same
        // LoS test — the two agree, so it closes instead of standing spell-less). The 90% buffer
        // keeps small target strafes inside range so the caster doesn't yo-yo on the boundary.
        // Melee-only creatures (hold range 0) take the exact pre-049 path. CEILING: a school-locked
        // (Kicked) caster still holds through the lockout window instead of closing — rare at 1-10.
        let hold = caster_hold_range_yd(ctx, c.entry);
        if hold > 0.0 {
            let hold_r = hold * 0.9;
            if dist_sq <= hold_r * hold_r
                && crate::nav::has_los(ctx, c.map_id, (c.x, c.y, c.z), (t.x, t.y, t.z))
            {
                continue;
            }
        }
        // SPLINE chase, mangos-style CONTINUOUS follow: commit to one leg and RIDE it for several ticks,
        // re-aiming only when the target VEERS off the current heading — NOT a fresh tiny leg every 500ms
        // tick (which made the client visibly re-compute its path each tick). For a MOVING target we aim a
        // LEAD past it along the chase line, so the leg is long and a straight-running kiter keeps the mob
        // on one held heading; for a STATIONARY target we aim at melee reach and stop. `pass_advance_splines`
        // advances the authoritative position along the leg. Snare-aware.
        let run = crate::combat::effective_move_speed(ctx, c.guid, constants::speeds::RUN);
        let dist_to_t = dist_sq.sqrt();
        let (dir_x, dir_y) = ((t.x - c.x) / dist_to_t, (t.y - c.y) / dist_to_t);
        // Aim point: moving → dist + CHASE_LEAD_YD along the heading (a long, rideable leg); stationary →
        // melee reach (dist - 4), so the mob closes and the stand above stops it.
        let leg_len = if target_moving {
            dist_to_t + CHASE_LEAD_YD
        } else {
            (dist_to_t - 4.0).max(0.0)
        };
        let (ax, ay) = (c.x + dir_x * leg_len, c.y + dir_y * leg_len);
        // 243: nav-aware — nav_step returns the aim point (LoS clear) or the first detour corner (blocked).
        let (nx, ny) = crate::nav::nav_step(ctx, c.map_id, (c.x, c.y), (ax, ay), leg_len, 0.0);
        if nx == c.x && ny == c.y {
            continue; // already there → nothing to move.
        }
        let active_leg = ctx.db.game_creature_spline().guid().find(c.guid);
        // Re-aim ONLY when: no active leg (it completed), OR the target VEERED off the current leg's
        // heading by more than ~CHASE_REPATH_COS (a turn). A target running roughly straight keeps the
        // same heading → the mob rides ONE leg without re-emitting = smooth, no per-tick recompute.
        let repath = match active_leg {
            None => true,
            Some(sp) => {
                let (lx, ly) = (sp.dx - sp.sx, sp.dy - sp.sy);
                let ll = (lx * lx + ly * ly).sqrt();
                ll <= 0.001 || (lx / ll) * dir_x + (ly / ll) * dir_y < CHASE_REPATH_COS
            }
        };
        if !repath {
            continue;
        }
        let (ndx, ndy) = (nx - c.x, ny - c.y);
        let dist = (ndx * ndx + ndy * ndy).sqrt();
        let duration_ms = ((dist / run) * 1000.0) as u32;
        // 086: the writer snaps the leg's dest to GROUND at (nx, ny) — t.z is the target's height at
        // the TARGET's position, wrong on slopes and plain nonsense when (nx, ny) is a nav detour
        // corner nowhere near the target — so t.z is only the off-slice FALLBACK (pre-terrain shape).
        to_chase.push((c.guid, nx, ny, t.z, duration_ms));
    }
    for (guid, nx, ny, tz, duration_ms) in to_chase {
        // A chase is a RUN and never arms the ETA gate (re-stepped every tick toward the live target).
        if let Some(c) = entities.guid().find(guid) {
            emit_creature_leg(ctx, c, (nx, ny), tz, duration_ms, true, now_ms, false);
        }
    }
    visited
}

/// Pass 8 — return-to-spawn: a displaced, NON-engaged, waypoint-less creature beyond `RETURN_LEASH_SQ`
/// of its home walks back. WRITES `moved_this_tick` (the guids it moved) so the wander pass can skip
/// them. Runs after chase (engaged creatures already excluded) and before regen (only moves position).
///
/// Work-item 230: SCOPED to `active` — a displaced creature outside every player's active-cell
/// neighborhood is DORMANT (freezes mid-walk-home rather than continuing). Coherent by construction:
/// each step is computed fresh from the CURRENT stored `(x, y)` toward `home` (no elapsed-time or
/// step-count state), so any number of dormant ticks in between changes nothing about how the walk
/// resumes — it's exactly as if the creature paused. Never dormant-izes an engaged creature: this pass
/// already excludes them via `is_engaged` regardless of `active`.
/// Work-item 229: instance scope inherited from `active`; `tick_secs` = the firing row's interval
/// (cadence-invariant walk-home speed, exactly `MOVE_TICK_SECS` on the default row). Returns
/// candidates visited (guids past the active gate).
fn pass_return(
    ctx: &ReducerContext,
    moved_this_tick: &mut std::collections::HashSet<u64>,
    active: &std::collections::HashSet<u64>,
    tick_secs: f32,
) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    let waypoints = ctx.db.game_creature_waypoint();

    // Return-to-spawn pass (vanilla creature AI: a creature pushed off its post walks back). This is
    // how a creature that ended up displaced from its home — a future knockback, or a leash that
    // dropped it somewhere far from where it spawned — finds its way back to `game_creature_spawn`.
    // Combined with the chase pass it gives the basic loop: pull → chase → evade (leash disengages) →
    // walk home.
    //
    // SCOPE — this pass moves ONLY creatures that no other mover governs, so it can never fight the
    // patrol/chase/flee legs:
    //   - NOT engaged (`is_engaged` false): the chase/flee passes move only ENGAGED attackers, so a
    //     non-engaged creature is invisible to them — no overlap. A creature mid-pull walks toward its
    //     target (chase), not home.
    //   - NO patrol waypoints: a patrolling creature is governed by the patrol pass (it owns the
    //     creature's idle movement, walking it between waypoints — "home" for it is a waypoint, not the
    //     spawn). We reuse the SAME `by_creature` waypoint lookup the patrol pass uses to detect one.
    //   - ALIVE creature (no PLAYER bit, not dead): a corpse holds position; players move themselves.
    // Snapshot the legs first (collect-then-mutate) so we never write the entity table while iterating
    // it — mirrors the patrol/chase/flee passes.
    //
    // It runs AFTER chase (so an engaged creature is already excluded by `is_engaged`) and BEFORE regen
    // (a returning creature is out of combat, so the regen passes' `in_combat` gate does NOT skip it —
    // but this pass ONLY moves x,y,z/grid/last_move and emits a leg, never touches HP/power, so regen
    // re-writing the row from its own snapshot heals HP without reverting the move: regen reads the
    // post-move row, and even a stale read would only restore HP, not position). It also can't duplicate
    // a chase/flee leg this tick: those move only ENGAGED creatures and this one only NON-engaged ones —
    // the two sets are disjoint, so no creature gets two legs (or two `spline_id`s) in a single tick.
    let spawns_home = ctx.db.game_creature_spawn();
    // (creature_guid, dest_x, dest_y, z_fallback=home_z, duration_ms) — 181: snap + leg-start via
    // `emit_creature_leg` in the drain loop below.
    let mut to_return: Vec<(u64, f32, f32, f32, u32)> = Vec::new();
    // ACTIVE CELLS (work-item 230): iterate the pre-computed active-cell guid set instead of the full
    // entity table.
    for guid in active.iter().copied() {
        visited += 1;
        let Some(c) = entities.guid().find(guid) else {
            continue;
        };
        // Creatures only (no PLAYER bit), alive — same predicate as the chase/flee passes.
        if c.is_player() || c.dead {
            continue;
        }
        // Crowd control: a creature whose self-movement is suppressed — FROZEN (stun/root/poly) or FEARED
        // (the fear-flee pass walks it instead) — doesn't walk home (a movement pass). Baseline-safe:
        // `false` without a CC aura → an un-CC'd displaced creature returns exactly as before.
        if crate::spell::is_self_movement_suppressed(ctx, c.guid) {
            continue;
        }
        // Engaged creatures belong to chase/flee — never return-walk them.
        if crate::combat::is_engaged(ctx, c.guid) {
            continue;
        }
        // A creature with patrol waypoints is governed by the patrol pass (same `by_creature` lookup);
        // skip it so we don't drag it off its route toward its spawn.
        if waypoints.by_creature().filter(&c.guid).next().is_some() {
            continue;
        }
        // Its persistent home; no spawn row → nothing to return to.
        let Some(home) = spawns_home.guid().find(c.guid) else {
            continue;
        };
        // Within the return LEASH (squared compare, like the aggro/leash ranges) → home-enough, no
        // move. The roomy (15 yd)² leash leaves the WANDER pass space to hop within `WANDER_RADIUS`
        // (6 yd) of home without return immediately pulling it back — only a creature displaced
        // FARTHER than the leash walks home.
        let (dx, dy) = (home.x - c.x, home.y - c.y);
        if dx * dx + dy * dy <= RETURN_LEASH_SQ {
            continue;
        }
        // Step toward home up to one full tick of run, landing AT spawn if within reach (`stop_dist`
        // 0.0 — unlike chase, we want to arrive ON the home point, not short of it). Snare-aware (a snared
        // creature crawls home slower); no snare → RUN exactly.
        let run = crate::combat::effective_move_speed(ctx, c.guid, constants::speeds::RUN);
        // Step ONE TICK of run toward home (a STEP pass like chase — re-stepped every firing so the
        // walk-home chains smoothly; no ETA gate, since each short leg's duration ≈ the tick interval).
        let (nx, ny) = crate::nav::nav_step(
            ctx,
            c.map_id,
            (c.x, c.y),
            (home.x, home.y),
            run * tick_secs,
            0.0,
        );
        if nx == c.x && ny == c.y {
            continue; // no-op step (can't close the gap) — skip the zero-length leg the client rejects.
        }
        let (ndx, ndy) = (nx - c.x, ny - c.y);
        let dist = (ndx * ndx + ndy * ndy).sqrt();
        let duration_ms = ((dist / run) * 1000.0) as u32;
        // Ground-snap (in the writer) each intermediate step of the walk home (work-item 174 follow-up,
        // PR-9 review: un-snapped home.z here made a leashed creature float/clip down the slope and
        // Z-pop when the snapped wander pass took over at the post). Off-slice falls back to the spawn's
        // z — the old behavior, landing the leg at the post like patrol legs.
        to_return.push((c.guid, nx, ny, home.z, duration_ms));
    }
    // Guids that already emitted a move leg this tick (the return pass). The wander pass below must
    // skip these: a creature the return pass just walked home is now WITHIN the leash, so without this
    // guard the wander pass would re-pick it and emit a SECOND CreatureMoveEvent with the same
    // `spline_id` — and the client rejects the stale-id second spline.
    for (guid, nx, ny, hz, duration_ms) in to_return {
        moved_this_tick.insert(guid); // per-pass tail — return owns the wander-skip set
                                      // Return-to-spawn RUNS home; no ETA gate (re-stepped every tick until it arrives).
        if let Some(c) = entities.guid().find(guid) {
            emit_creature_leg(ctx, c, (nx, ny), hz, duration_ms, true, now_ms, false);
        }
    }
    visited
}

/// Pass 9 — wander: an idle, NON-engaged, waypoint-less RANDOM-movement creature WITHIN the leash of
/// its home hops to a random point near home on ~`WANDER_CHANCE_PCT` of ticks. READS `moved_this_tick`
/// and SKIPS any guid the return pass already moved (one move leg per creature per tick). Mutually
/// exclusive with return (that moves creatures OUTSIDE the leash, this inside it).
///
/// Work-item 230: SCOPED to `active` — an idle creature outside every player's active-cell
/// neighborhood is DORMANT (no hop rolled). Coherent by construction: the `leg_in_flight` ETA gate and
/// the `WANDER_CHANCE_PCT` roll are both evaluated fresh each active tick against the CURRENT stored
/// `leg_ends_ms`/position, so a creature that goes dormant mid-loiter simply resumes the same idle
/// rhythm whenever it re-enters an active cell — no state depends on how many ticks were skipped.
/// Work-item 229: instance scope inherited from `active`. The hop step stays `walk * 4.0` (the SENSE
/// cadence, ~4s for EVERY row per `is_sense_tick_for_interval`) — not the per-firing `tick_secs` —
/// because this pass runs on sense ticks only. Returns candidates visited (past the active gate).
fn pass_wander(
    ctx: &ReducerContext,
    moved_this_tick: &std::collections::HashSet<u64>,
    active: &std::collections::HashSet<u64>,
) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    let waypoints = ctx.db.game_creature_waypoint();

    // Wander pass (vanilla creature AI: an idle creature near its post loiters — hops to random
    // nearby points). For each ALIVE creature (no PLAYER bit, not dead) that is NOT engaged, has NO
    // patrol waypoints, and whose spawn home exists AND it is WITHIN `RETURN_LEASH_SQ` of that home,
    // roll a `WANDER_CHANCE_PCT` chance to pick a random point within `WANDER_RADIUS` of HOME and walk
    // toward it (one short leg, like a patrol leg). The ~1/3 chance gives a pause-then-hop loiter, not
    // a constant jog.
    //
    // MUTUALLY EXCLUSIVE with return-to-spawn: that pass moves a creature displaced FARTHER than
    // `RETURN_LEASH_SQ` from home; this one moves only a creature WITHIN it. The two ranges partition
    // the non-engaged, waypoint-less creatures, so a creature that is RETURNING this tick is outside
    // the leash and never also wanders (and a wanderer, always ≤ `WANDER_RADIUS` = 6 yd from home, is
    // far inside the 15-yd leash, so it never trips return next tick). Same disjointness from
    // chase/flee as return-to-spawn: those move only ENGAGED attackers, this one only NON-engaged
    // creatures — one leg per creature per tick. Snapshot the legs first (collect-then-mutate) so we
    // never write the entity table while iterating it — mirrors the patrol/chase/return passes.
    //
    // It runs AFTER return-to-spawn (so a creature outside the leash already returned and isn't also
    // considered here) and BEFORE regen (a wanderer is out of combat, so regen's `in_combat` gate does
    // NOT skip it — but, exactly like return-to-spawn, this pass only moves x,y,z/grid/last_move and
    // emits a leg, never HP/power, so regen re-writing the row from its own snapshot heals HP without
    // reverting the move). Flee still runs LAST and only touches ENGAGED creatures, so it can't fight a
    // wanderer.
    let spawns_wander = ctx.db.game_creature_spawn();
    // (creature_guid, dest_x, dest_y, z_fallback=home_z, duration_ms) — 181: snap + leg-start via
    // `emit_creature_leg` in the drain loop below.
    let mut to_wander: Vec<(u64, f32, f32, f32, u32)> = Vec::new();
    // ACTIVE CELLS (work-item 230): iterate the pre-computed active-cell guid set instead of the full
    // entity table.
    for guid in active.iter().copied() {
        visited += 1;
        let Some(c) = entities.guid().find(guid) else {
            continue;
        };
        // Creatures only (no PLAYER bit), alive — same predicate as the return/chase/flee passes.
        if c.is_player() || c.dead {
            continue;
        }
        // Crowd control: a creature whose self-movement is suppressed — FROZEN (stun/root/poly) or FEARED
        // (the fear-flee pass walks it instead) — doesn't wander (a movement pass). Baseline-safe:
        // `false` without a CC aura → an un-CC'd idle creature loiters exactly as before.
        if crate::spell::is_self_movement_suppressed(ctx, c.guid) {
            continue;
        }
        // Engaged creatures belong to chase/flee — never wander them.
        if crate::combat::is_engaged(ctx, c.guid) {
            continue;
        }
        // A creature the return pass already moved this tick must not also wander — one move leg per
        // creature per tick (two legs share `spline_id` and the client rejects the second).
        if moved_this_tick.contains(&c.guid) {
            continue;
        }
        // A creature with patrol waypoints is governed by the patrol pass (same `by_creature` lookup
        // the return pass uses); skip it so wander doesn't fight its route.
        if waypoints.by_creature().filter(&c.guid).next().is_some() {
            continue;
        }
        // Its persistent home; no spawn row → nowhere to wander around.
        let Some(home) = spawns_wander.guid().find(c.guid) else {
            continue;
        };
        // Only cmangos MovementType RANDOM free-wanders. IDLE creatures hold their spawn post (quest
        // givers, vendors, guards), and WAYPOINT creatures are driven by the patrol pass above — so the
        // wander pass must not move either. Un-reimported rows default to IDLE → stationary.
        if home.movement_type != MOVEMENT_RANDOM {
            continue;
        }
        // Only wander a creature that is home-enough (within the return leash). One displaced farther
        // is RETURNING this tick (handled above) and must not also wander — mutually exclusive.
        let (hdx, hdy) = (home.x - c.x, home.y - c.y);
        if hdx * hdx + hdy * hdy > RETURN_LEASH_SQ {
            continue;
        }
        // ETA gate: a wander hop is still in flight — don't roll a NEW point mid-hop (it would redirect
        // the leg). Commit to the current hop until it lands, THEN consider the next.
        if leg_in_flight(now_ms, c.leg_ends_ms) {
            continue;
        }
        // Pause between hops: ~2/3 of ticks the creature stands still (idle loiter, not a constant jog).
        // (wander runs on the ~4s SENSE cadence, so this roll keeps its original hop frequency.)
        if ctx.random::<u32>() % 100 >= WANDER_CHANCE_PCT {
            continue;
        }
        // Random destination within `WANDER_RADIUS` of HOME (two rolls → angle + radius). Anchoring on
        // home (not the current position) keeps the wander circle centred on the post, so the creature
        // never drifts away over many hops — every destination is provably ≤ `WANDER_RADIUS` from home,
        // far inside the leash.
        let (destx, desty) =
            wander_point(home.x, home.y, ctx.random(), ctx.random(), WANDER_RADIUS);
        // Walk toward it up to one tick of WALK (idle stroll, not a run), landing AT the point if within
        // reach (`stop_dist` 0.0 — arrive on it, like the return leg lands on home). Snare-aware (a snared
        // wanderer ambles slower); no snare → WALK exactly.
        let walk = crate::combat::effective_move_speed(ctx, c.guid, constants::speeds::WALK);
        let (nx, ny) =
            crate::nav::nav_step(ctx, c.map_id, (c.x, c.y), (destx, desty), walk * 4.0, 0.0);
        if nx == c.x && ny == c.y {
            continue; // no-op (already on the point / can't close) — skip the zero-length leg.
        }
        let (ndx, ndy) = (nx - c.x, ny - c.y);
        let dist = (ndx * ndx + ndy * ndy).sqrt();
        let duration_ms = ((dist / walk) * 1000.0) as u32;
        // Ground-snap the hop (in the writer, work-item 174); off-slice keeps the old behavior — the
        // home z (flat-ish ground around the post, like the return leg carries the spawn z).
        to_wander.push((c.guid, nx, ny, home.z, duration_ms));
    }
    for (guid, nx, ny, hz, duration_ms) in to_wander {
        // Wander STROLLS (walk) and DOES arm the ETA gate (hold the hop until it lands, no mid-hop
        // re-roll) — the two idle passes (patrol + wander) are the only `set_leg_ends = true` callers.
        if let Some(c) = entities.guid().find(guid) {
            emit_creature_leg(ctx, c, (nx, ny), hz, duration_ms, false, now_ms, true);
        }
    }
    visited
}

/// Pass 10 — regen (health + power TOGETHER — they share the `in_combat` snapshot): out-of-combat
/// HP recovery for any entity, then power regen/decay by power type. Runs before flee/fear-flee so a
/// still-engaged runner is skipped by the in-combat gate, not reverted by regen.
///
/// Work-item 230 classification: STAYS GLOBAL — HP/power regen isn't proximity-gated in vanilla either
/// (an out-of-view creature still heals toward full), and this pass covers PLAYERS too, so it's out of
/// this item's creature-ticking scope (mirrors `pass_combat_drop`'s reasoning).
/// Work-item 229: catch-all firing only, still covering ALL instances — the per-sense-tick regen
/// AMOUNT is cadence-quantized, so a second (faster) row running this would literally multiply
/// everyone's regen rate (see `TickScope::runs_global_passes`). Returns entity rows scanned — ONE pass
/// now feeds both the health and the power loop (perf catalog 1.6, partial), so this halved.
fn pass_regen(ctx: &ReducerContext) -> usize {
    let mut visited = 0usize;
    let entities = ctx.db.game_world_entity();
    // Derive now_ms once for the FSR (five-second rule) mana-regen gate.
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;

    // Health regen: any entity below max HP recovers each tick — out of combat at the full
    // SPIRIT+level-scaled rate, IN combat at a reduced rate if the entity carries one or more
    // `A_COMBAT_HEALTH_REGEN_PCT` auras (e.g. the Troll Regeneration racial passive). Entities in
    // combat with NO such aura are skipped (zero combat health regen — today's behaviour). The heal
    // flows to clients via the game_world_entity on_update VALUES relay.
    // Perf catalog 1.6 (partial): ONE entity pass now feeds the health loop, the power loop AND the
    // IN_COMBAT half of the combatant set. This pass used to iterate the whole table THREE times per
    // sense tick — twice for two disjoint, tiny result sets, and a third time inside
    // `combatant_guids` — so at 50k creatures it cost ~150k row visits to touch a few dozen rows.
    // Byte-identical: the same predicates over the same rows, and the flag half is harvested from
    // THESE rows (read at exactly the point `combatant_guids` would have read them), not a stale
    // snapshot. The remaining single scan is the parked half of 1.6 (a damage-driven
    // `game_regen_pending` set) — see the catalog entry.
    let mut in_combat = crate::combat::melee_combatant_guids(ctx);
    let candidates: Vec<WorldEntity> = entities
        .iter()
        .inspect(|_| visited += 1)
        .filter(|e| {
            if e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0 {
                in_combat.push(e.guid);
            }
            !e.dead && (e.health < e.max_health || e.max_power > 0)
        })
        .collect();
    for e in candidates.iter().filter(|e| e.health < e.max_health) {
        let is_in_combat = in_combat.contains(&e.guid);
        let next = if is_in_combat {
            // Sum active A_COMBAT_HEALTH_REGEN_PCT auras; skip if there are none.
            let pct = crate::spell::combat_health_regen_pct(ctx, e.guid);
            if pct <= 0 {
                continue; // no aura → zero combat regen (baseline-safe)
            }
            crate::combat::regen_health_in_combat(
                e.health,
                e.max_health,
                e.spirit,
                e.level,
                pct as u32,
            )
        } else {
            crate::combat::regen_entity_health(e)
        };
        if next != e.health {
            // Re-find the LIVE row and write ONLY health — never the snapshot's x/y/z — so a movement
            // write this tick can't be reverted by a stale full-row update (defense-in-depth atop the
            // in-combat gate, which makes the regen-vs-move ordering non-fragile).
            if let Some(mut live) = entities.guid().find(e.guid) {
                if live.health != next {
                    live.health = next;
                    entities.guid().update(live);
                }
            }
        }
    }

    // Power regen/decay by power type (only entities with a power bar — players; creatures carry
    // max_power 0 and are skipped). Mana ticks once the FSR window expires (now_ms >=
    // mana_regen_paused_until_ms), energy ticks always, rage decays out of combat. The change flows
    // to the owner via the on_update power VALUES relay (same path as combat rage). Re-uses
    // `in_combat` from the health pass above; passes `now_ms` for the FSR mana gate.
    for e in candidates.iter().filter(|e| e.max_power > 0) {
        let next = crate::combat::regen_entity_power(e, in_combat.contains(&e.guid), now_ms);
        if next != e.power {
            // Live re-find + power-only write (same reasoning as the health pass above).
            if let Some(mut live) = entities.guid().find(e.guid) {
                if live.power != next {
                    live.power = next;
                    entities.guid().update(live);
                }
            }
        }
    }
    visited
}

/// Pass 11 — flee: a meleeing HUMANOID below ~15% HP routs — it runs one RUN tick away from its target
/// each tick while STAYING in combat (a shared combat state; both sides keep their engagement). Runs LAST
/// (after both regen passes): the still-engaged runner is skipped by regen's in-combat gate, so regen never
/// re-writes (and reverts) the fled position this tick.
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — every candidate must currently be
/// the ATTACKER in a `game_melee_attack` row (combat-engaged; fleeing is a SHARED COMBAT STATE per the
/// doc above, not a disengage). Gating this on active cells would risk freezing a routing creature
/// mid-flight the instant it — or the player chasing it — stepped past the active-cell edge, which is
/// exactly the "mid-leg flee... finishes" outcome the item requires, not "freezes."
///
/// Work-item 233: outer-loops `game_melee_attack` directly (same precedent as `pass_chase`/
/// `pass_cast`) instead of `entities.iter()` + a per-row `.find(&c.guid)` gate. VERIFIED before
/// switching: "fleeing" carries NO separate state on `WorldEntity` (no `is_fleeing`/`fleeing_since`
/// field — `WorldEntity`'s only combat-adjacent fields are `dead`/`target_guid`/`unit_flags`); a
/// creature's flee status is recomputed FRESH every tick from exactly two things — "is it the attacker
/// in a melee row" and `creature_will_flee` (HP% + type). Since the melee row is that same necessary
/// condition the old per-row `.find` already required, a creature whose melee row was ever deleted
/// mid-flee could NEVER have re-qualified anyway (this pass never deletes it — see the ordering note
/// above), so outer-looping the melee table instead of the entity table cannot miss a genuinely
/// fleeing creature.
/// Work-item 229: gated per candidate on `scope.covers(c.instance_id)` (attacker's instance = the
/// pair's — same construction argument as pass_cast/pass_chase).
///
/// ⚠ `_tick_secs` is ACCEPTED BUT NOT READ, exactly as in `pass_chase` above (same gap, same
/// reason the parameter is kept): the flee leg is sized from the movement constant, so it is not
/// cadence-invariant on a non-default tick row. Returns covered candidates visited.
fn pass_flee(ctx: &ReducerContext, scope: &TickScope, _tick_secs: f32) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    // Flee pass (vanilla creature AI: a mob in melee that drops to ~15% HP runs away). For each ALIVE
    // creature currently attacking (a `game_melee_attack` row keyed by its guid) whose HP is below the
    // flee threshold AND flee-eligible, step it ONE RUN TICK directly away from its target while KEEPING the
    // engagement live (it doesn't disengage — fleeing is a combat state). Moving x,y relays a position change
    // to clients, same as a patrol leg.
    //
    // ORDERING — this MUST run LAST, after both regen passes. A fleeing creature is still engaged (its
    // melee row is live until we disengage it here), so the regen passes' `in_combat` gate skips it —
    // meaning regen never re-writes (and clobbers) the fled position this tick. If the flee ran BEFORE
    // regen, disengaging would drop the creature out of `in_combat`, the health-regen pass would then
    // re-write the whole row from its own snapshot and REVERT the move, pinning the runner in place
    // (it never leaves aggro range, re-engages, and grinds the target down). Running flee last avoids
    // that entirely. Snapshot the work first (collect (guid, nx, ny)), then mutate, so we never write
    // the entity / melee tables while iterating them.
    //
    // The aggro pass already skipped `should_flee` creatures this tick, so a near-dead creature isn't
    // re-armed before it flees. NOTE: a future regen tick heals a fled creature back above the
    // threshold, after which the aggro pass may re-engage it — faithful (an escaped mob that recovers
    // can re-engage); a persistent fear/leash state is a later refinement.
    let melee_flee = ctx.db.game_melee_attack();
    // Snapshot-then-mutate (never write tables mid-iteration). EVERY eligible fleeing creature re-stamps
    // combat each tick (kept alive across the long committed leg); only those whose committed leg has
    // FINISHED get a fresh leg emitted.
    let mut to_restamp: Vec<(u64, u64)> = Vec::new(); // (creature_guid, target_guid)
                                                      // (creature_guid, dest_x, dest_y, z_fallback=current_z, duration_ms) — snap via emit_creature_leg.
    let mut to_flee: Vec<(u64, f32, f32, f32, u32)> = Vec::new();
    // Work-item 233: outer-loop the small melee-engaged table instead of every entity — `row.attacker_guid`
    // is by construction "currently a melee attacker", the exact gate the old `.find(&c.guid)` applied
    // after scanning every entity, so this visits the identical candidate set.
    for row in melee_flee.iter() {
        let Some(c) = entities.guid().find(row.attacker_guid) else {
            continue;
        };
        // Same creature + alive predicate as the aggro/regen passes (no PLAYER bit, not dead). A
        // player's own melee row is skipped here exactly as `is_player` skipped it in the old loop.
        if c.is_player() || c.dead {
            continue;
        }
        // Work-item 229: only this firing's covered instances (see the fn doc).
        if !scope.covers(c.instance_id) {
            continue;
        }
        visited += 1;
        // Crowd control: a creature whose self-movement is suppressed — FROZEN (stun/root/poly) or FEARED
        // — does NOT run this low-HP flee leg. A frozen one stays engaged (we don't disengage it here) and
        // locked in place; a rooted low-HP creature keeps swinging but can't run. A FEARED low-HP creature
        // is moved by the fear-flee pass instead (which keeps it engaged, not disengaged), so excluding it
        // here avoids a double-move. Baseline-safe: `false` without a CC aura → an un-CC'd near-dead
        // creature flees exactly as before.
        if crate::spell::is_self_movement_suppressed(ctx, c.guid) {
            continue;
        }
        // Below the flee threshold AND flee-eligible. Fleeing is SELECTIVE in vanilla, not universal: only
        // HUMANOIDS rout at low HP; BEASTS (wolves/boars) etc. fight to the death. The old pass fled EVERY
        // near-dead creature, so a Northshire wolf (BEAST) ran away + dropped combat ("not all enemies
        // should flee"). `creature_will_flee` is the shared gate every flee site uses (see its doc), so a
        // non-eligible near-dead creature is never moved here and instead keeps chasing/swinging via the
        // earlier passes (a missing template ⇒ not eligible ⇒ stands and fights — safe default).
        if !creature_will_flee(ctx, &c) {
            continue;
        }
        // Run directly away from whoever it's fighting (the melee row's target).
        let Some(target) = entities.guid().find(row.target_guid) else {
            continue;
        };
        // SHARED COMBAT STATE: re-stamp EVERY tick (even mid-leg) so combat doesn't drop during the long
        // committed run. (Was in the emit loop, which now fires only on re-roll.)
        to_restamp.push((c.guid, row.target_guid));
        // COMMIT one long leg: re-roll ONLY when the previous leg has finished (no active spline).
        // Re-picking an away-direction every tick snap-rotated the client's facing (the "flee spin");
        // committing to one ~28yd leg (the vanilla flee-leg band) gives one stable travel tangent.
        if ctx.db.game_creature_spline().guid().find(c.guid).is_some() {
            continue;
        }
        // A single FLEE_LEG_YD leg directly away from the threat, nav-routed, at the wounded-slow run
        // speed; duration = the FULL travel time for that leg (not one
        // tick), so the client interpolates the whole committed run.
        let run = crate::combat::effective_move_speed(ctx, c.guid, constants::speeds::RUN)
            * wounded_slow_factor(c.health, c.max_health);
        let (fx, fy) = flee_step(c.x, c.y, target.x, target.y, FLEE_LEG_YD);
        let (nx, ny) = crate::nav::nav_step(ctx, c.map_id, (c.x, c.y), (fx, fy), FLEE_LEG_YD, 0.0);
        if nx == c.x && ny == c.y {
            continue;
        }
        let dist = ((nx - c.x).powi(2) + (ny - c.y).powi(2)).sqrt();
        let duration_ms = ((dist / run) * 1000.0) as u32;
        to_flee.push((c.guid, nx, ny, c.z, duration_ms));
    }
    // Combat re-stamp EVERY tick for every fleeing creature (not just re-roll ticks) — fleeing is a
    // SHARED COMBAT STATE; without this the 6s combat-drop fires mid committed-leg and both sides untarget
    // ("they get away or untarget each other and combat just ends"). The runner keeps fleeing until it
    // dies (the player catches the wounded-slow mob) or hits the leash (the leash pass evades it).
    for (guid, target_guid) in to_restamp {
        crate::combat::enter_combat(ctx, guid);
        crate::combat::enter_combat(ctx, target_guid);
    }
    // Emit the committed RUN legs (only the finished-leg creatures reach here). The client interpolates
    // the whole ~28yd run; pass_advance_splines advances the authoritative position along it.
    for (guid, nx, ny, cz, duration_ms) in to_flee {
        if let Some(c) = entities.guid().find(guid) {
            emit_creature_leg(ctx, c, (nx, ny), cz, duration_ms, true, now_ms, false);
        }
    }
    visited
}

/// Pass 12 — fear-flee (crowd control): a FEARED creature is force-walked AWAY from its fear source
/// each tick with jitter. Runs LAST alongside flee for the same regen-ordering reason; unlike flee it
/// does NOT disengage (fear is temporary). The normal movement passes all skip a feared creature
/// (`is_self_movement_suppressed`), so this is its SOLE mover.
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — a feared creature carries a live
/// CC aura (someone cast fear on it, which requires combat/targeting contact), so it's combat-adjacent
/// by construction and the population is inherently small and rare regardless of world size. Gating it
/// would risk freezing a panicked creature mid-route the instant it crossed the active-cell edge.
///
/// Work-item 233: outer-loops the `game_aura` table (filtered to `A_CONTROL(M_FEAR)` rows) instead of
/// `entities.iter()` + a per-entity `fear_source` probe. `game_aura` has no by-KIND index (only
/// `by_target`/`by_expiry`), so a target-side lookup can't narrow this the way the melee table narrows
/// cast/flee — but the aura table is itself small relative to the full entity population (fear uptime
/// is rare), so scanning IT with an `eff_kind`/`eff_p0` filter and DEDUPING the target guids is the
/// cheap, correct narrowing. `fear_source` is still called per candidate, UNCHANGED, to resolve the
/// caster — this only changes how the candidate SET is discovered (aura-table scan + dedup, vs.
/// entity-table scan + per-row probe), not which caster wins if a target somehow carries more than one
/// live fear aura (that tie-break is `fear_source`'s own `.find()` order, untouched by this pass).
/// Work-item 229: gated per candidate on `scope.covers(c.instance_id)` (the FEARED creature's own
/// instance — its sole mover must fire exactly once per its covering row). The dash distance scales
/// with `tick_secs` (`fear_step_for_tick` — verbatim `FEAR_STEP` on the default row) so a fast
/// dedicated row doesn't multiply terror speed. Returns covered candidates visited.
fn pass_fear_flee(ctx: &ReducerContext, scope: &TickScope, tick_secs: f32) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    // Fear-flee pass (crowd control: a FEARED creature "flees in terror" — it cannot act, and instead of
    // steering its own movement it is force-walked AWAY from whoever feared it each tick). For each ALIVE
    // creature carrying an `A_CONTROL(M_FEAR)` aura (detected via `fear_source`, which also yields the
    // caster to flee FROM) that is NOT otherwise FROZEN (`is_movement_blocked`: a stun/root/poly aura
    // outranks fear and pins it in place), step it `FEAR_STEP` yards away from the fear source with
    // angular jitter (zig-zag) and emit a RUN leg. Unlike the low-HP flee it does NOT disengage — fear is
    // temporary; when the aura expires the creature resumes attacking (its melee row is untouched), and
    // the action gates keep it from swinging/casting meanwhile.
    //
    // SOURCE position: the fear caster's (if still in world); failing that the creature's current melee
    // target (it still flees *something*); failing both its OWN position, which `feared_flee_step` turns
    // into a jittered +x bearing — so a feared creature always moves.
    //
    // ORDERING — like the low-HP flee it runs LAST, after both regen passes. A feared+engaged creature is
    // still `in_combat` (melee row live) so regen skips it; a feared+non-engaged one is moved here AFTER
    // regen ran, so regen never reverts the leg (this pass writes only x/y/z/grid/last_move, never
    // HP/power). The normal movement passes ALL skip feared creatures (`is_self_movement_suppressed`), so
    // this is the SOLE mover for a feared creature — one leg, one `spline_id`, per tick. Snapshot first
    // (collect-then-mutate), like every other movement pass.
    let melee_fear = ctx.db.game_melee_attack();
    let auras_fear = ctx.db.game_aura();
    // (creature_guid, dest_x, dest_y, z_fallback=current_z, duration_ms) — 181: snap + leg-start via
    // `emit_creature_leg` in the drain loop below.
    let mut to_fear_flee: Vec<(u64, f32, f32, f32, u32)> = Vec::new();
    // Work-item 233: the small candidate SET — every distinct `target_guid` carrying a live
    // `A_CONTROL(M_FEAR)` aura — discovered by scanning the (small) aura table instead of every entity.
    // Sorted + deduped so a target with more than one live fear aura (e.g. two different casters) is
    // still visited exactly once, matching the old loop's one-decision-per-entity shape.
    let mut feared_guids: Vec<u64> = auras_fear
        .iter()
        .filter(|a| a.eff_kind == crate::spell::A_CONTROL && a.eff_p0 == crate::spell::M_FEAR)
        .map(|a| a.target_guid)
        .collect();
    feared_guids.sort_unstable();
    feared_guids.dedup();
    for guid in feared_guids {
        let Some(c) = entities.guid().find(guid) else {
            continue;
        };
        // Creatures only (no PLAYER bit), alive — same predicate as the other movement passes.
        if c.is_player() || c.dead {
            continue;
        }
        // Work-item 229: only this firing's covered instances (see the fn doc).
        if !scope.covers(c.instance_id) {
            continue;
        }
        visited += 1;
        // Must be FEARED; `fear_source` is `Some(caster)` exactly when an A_CONTROL(M_FEAR) aura is up —
        // unchanged call, re-resolving the caster (not read off the aura row above) so the "which caster"
        // tie-break is untouched by this pass.
        let Some(src_guid) = crate::spell::fear_source(ctx, c.guid) else {
            continue;
        };
        // A stun/root/poly aura OUTRANKS fear and freezes the creature — it stays put, doesn't flee.
        if crate::spell::is_movement_blocked(ctx, c.guid) {
            continue;
        }
        // Source position: the fear caster if in world, else the current melee target, else self (→ a
        // jittered +x fallback bearing in `feared_flee_step`, so a feared creature always moves).
        let src = entities.guid().find(src_guid).or_else(|| {
            melee_fear
                .attacker_guid()
                .find(c.guid)
                .and_then(|r| entities.guid().find(r.target_guid))
        });
        let (sx, sy) = src.map(|s| (s.x, s.y)).unwrap_or((c.x, c.y));
        let (nx, ny) = feared_flee_step(
            c.x,
            c.y,
            sx,
            sy,
            fear_step_for_tick(tick_secs),
            ctx.random(),
        );
        if nx == c.x && ny == c.y {
            continue; // degenerate zero-length leg (the client rejects it) — skip.
        }
        let (ndx, ndy) = (nx - c.x, ny - c.y);
        let dist = (ndx * ndx + ndy * ndy).sqrt();
        let duration_ms = ((dist / constants::speeds::RUN) * 1000.0) as u32;
        // The writer ground-snaps the panic hop (work-item 174); off-slice stays the flat z-unchanged
        // leg (c.z fallback).
        to_fear_flee.push((c.guid, nx, ny, c.z, duration_ms));
    }
    for (guid, nx, ny, cz, duration_ms) in to_fear_flee {
        // Feared creatures RUN in terror; no ETA gate (re-stepped each tick until the aura expires).
        if let Some(c) = entities.guid().find(guid) {
            emit_creature_leg(ctx, c, (nx, ny), cz, duration_ms, true, now_ms, false);
        }
    }
    visited
}

#[cfg(test)]
mod due_timer_tripwire {
    use crate::test_scan::code_of;

    /// The two due-time passes must RANGE-SCAN their index, and must DISARM the timer they fired.
    ///
    /// Both halves are load-bearing and neither is visible to a behavioural test (these are
    /// `ReducerContext` glue — playbook §7), so their shape is pinned:
    ///
    /// * Drop the range scan and they silently go back to visiting the whole spawn table — the exact
    ///   cost this work removed (measured: 5,735 rows per pass per sense tick, with nobody online).
    /// * Drop the DISARM and the range scan stops narrowing anything, because a fired timer stays in
    ///   the past and the row is revisited forever. That failure is invisible: behaviour stays
    ///   correct and only the cost comes back.
    #[test]
    fn the_due_time_passes_range_scan_their_index_and_disarm_after_firing() {
        let src = include_str!("tick.rs");

        let decay = code_of(src, "fn pass_decay(ctx: &ReducerContext) -> usize {");
        assert!(
            decay.contains("by_despawn_at()") && decay.contains("filter(..=now_ts)"),
            "`pass_decay` no longer range-scans `by_despawn_at` — it is back to a full spawn-table \
             scan every sense tick. Body was:\n{decay}"
        );
        assert!(
            decay.contains("despawn_at = crate::creatures::timer_never(ctx)"),
            "`pass_decay` no longer disarms `despawn_at` after the corpse decays, so the row stays \
             in the range scan forever and the index narrows nothing. Body was:\n{decay}"
        );

        let respawn = code_of(src, "fn pass_respawn(ctx: &ReducerContext) -> usize {");
        assert!(
            respawn.contains("by_respawn_at()") && respawn.contains("filter(..=now_ts)"),
            "`pass_respawn` no longer range-scans `by_respawn_at`. Body was:\n{respawn}"
        );
        assert!(
            respawn.contains("respawn_at = crate::creatures::timer_never(ctx)"),
            "`pass_respawn` no longer disarms `respawn_at` after materialising the creature. Every \
             imported spawn arms `respawn_at = now` deliberately, so without the disarm the ENTIRE \
             imported world sits in the range scan permanently. Body was:\n{respawn}"
        );
    }

    /// The WIRING, not the predicate (playbook section 8 — and this test exists because the first
    /// version of it pinned only the pure fn, and swapping the call site back to a bare `!e.dead`
    /// left all 561 tests green while a godmoded GM went back to collecting the whole valley).
    ///
    /// `pass_aggro_assist`'s player snapshot must go through `is_aggro_candidate`, which is what
    /// excludes a GODMODED GM. An immortal target never dies, so creatures never disengage and
    /// accumulate on them without bound — 103 simultaneous attackers, observed live.
    #[test]
    fn the_aggro_pass_skips_godmoded_players() {
        let body = crate::test_scan::code_of(include_str!("tick.rs"), "fn pass_aggro_assist(");
        assert!(
            body.contains("is_aggro_candidate(e.dead, e.godmode)"),
            "the aggro pass no longer routes its player snapshot through `is_aggro_candidate`, so a \
             godmoded GM is an aggro target again. Body was:\n{body}"
        );
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
    #[test]
    fn nothing_writes_the_unsubscribed_move_event_table() {
        let mut sources: Vec<(&str, String)> = vec![
            ("creatures/tick.rs", include_str!("tick.rs").to_string()),
            ("encounter.rs", include_str!("../encounter.rs").to_string()),
            (
                "spell/cast.rs",
                include_str!("../spell/cast.rs").to_string(),
            ),
        ];
        // The playerbots half is read at RUN time, not `include_str!`d: `packages/` is an OPTIONAL
        // drop-in tree (build.rs discovers whatever is installed, and the public mirror ships none
        // of it), so a compile-time include made a checkout without it fail to BUILD. The skip is
        // scoped to `packages/playerbots/` being absent entirely — an installed package missing
        // this file still fails, which is the path-typo case this scan has to keep catching.
        if let Some(src) = crate::test_scan::read_scanned("packages/playerbots/src/mod.rs") {
            sources.push(("playerbots", src));
        }
        for (name, src) in &sources {
            let src = src.as_str();
            let code = src
                .split_once("mod tests")
                .map_or(src, |(before, _)| before);
            assert!(
                !code.contains("game_creature_move_event().insert"),
                "{name} inserts into `game_creature_move_event`, which NO subscriber reads since \
                 perf 2.3 — the movement it emits will never reach a client. Emit the leg through \
                 `creatures::tick::emit_move_spline` (or `emit_creature_leg`) instead."
            );
        }
    }
}
