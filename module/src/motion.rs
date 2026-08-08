//! Batched movement republish (#461) — the private staging table + the scheduled reducer that
//! drains it into the public `game_entity_motion` relay.
//!
//! # Why
//!
//! SpacetimeDB pays its subscription sweep — re-evaluating every distinct query plan that touches a
//! changed table — **per transaction, not per row**. `movement_update` runs one transaction per
//! movement packet, so at 500 players the sweep ran ~1000×/s. Measured 2026-08-08 on an 8-core box:
//! `movement_update` cost 7.8 ms/call of which 1.5% (56.8 µs) was our own wasm and 98.5% was
//! SpacetimeDB's subscription work — ~80% of one writer core on the hot shard.
//!
//! A PRIVATE table has no subscribers, so writing one costs a row update and **no subscription
//! evaluation at all**. So `movement_update` now writes here, and one scheduled reducer republishes
//! every queued mover into the public table in ONE transaction (Clockwork's own
//! `spacetimedb-cookbook/multiple-position-updates` shape, and Blackholio's 50 ms
//! `move_all_players`). That takes the sweep from ~1000/s to 20/s and — the part that actually
//! matters — **decouples it from player count**: 20 sweeps/s at 500 players and at 2000.
//!
//! # The queue is the dirty set
//!
//! There is no `dirty` column and no "republish everything every tick" pass. A row exists in
//! `game_entity_motion_pending` **iff** that mover has motion the tick has not published yet; the
//! tick drains (deletes) every row it publishes. An entity that did not move therefore has no row
//! and is not touched — the tick's cost is O(movers since the last firing), never O(world).
//!
//! `seq` is NOT staged here: it lives on the public row and is bumped at publish time, exactly as
//! `movement_update` used to bump it, so per-mover ordering/de-duplication on the gateway side is
//! unchanged.
//!
//! # Discrete transitions (the part that is easy to get subtly wrong)
//!
//! The relay carries `(opcode, movement_info)`, not just a position. `MSG_MOVE_JUMP`,
//! `MSG_MOVE_STOP`, `MSG_MOVE_START_FORWARD` … are **edge events**: a peer plays an animation or
//! changes extrapolation state off them. Naive "keep the latest packet" loses any transition that
//! starts and ends inside one 50 ms window. [`coalesce`] is the rule that stops that — see its doc
//! for the three cases and why each is lossless.
//!
//! # Lifecycle
//!
//! The pending row is deleted everywhere the public motion row is (logout/disconnect, cascade
//! delete, creature despawn, pet despawn, teleport), and the tick independently refuses to publish a
//! guid with no live `game_world_entity` row — so a queued packet can never resurrect a motion row
//! for an entity that left between the write and the firing.

use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::{game_entity_motion, game_world_entity, EntityMotion};

// ===========================================================================================
//  Cadence
// ===========================================================================================

/// Default republish cadence — 50 000 µs = **20 Hz** (#461's default). Peer movement gains at most
/// one tick (50 ms) of latency; vanilla clients dead-reckon between updates, and BitCraft ships far
/// coarser (one row per path segment, lerped on read).
pub const MOTION_TICK_MICROS: i64 = 50_000;

/// Cadence floor for [`set_motion_tick_ms`]. Every firing is a transaction on the one serialized
/// commit stream, so a very tight interval trades the win this whole item exists to buy back for
/// scheduler pressure (`debug_arm_instance_tick`'s floor, same reasoning).
pub const MOTION_TICK_MS_FLOOR: u64 = 10;
/// Cadence ceiling for [`set_motion_tick_ms`] — a second of staged movement is already well past
/// what peer dead-reckoning covers; anything larger is a misconfiguration, not a tuning choice.
pub const MOTION_TICK_MS_CEIL: u64 = 1_000;

// ===========================================================================================
//  The private staging table [server] — NO `public`, deliberately
// ===========================================================================================

/// One row per mover whose latest motion has not been republished yet — the whole point of the
/// item, so **do not make this table `public`**: a subscriber here reinstates exactly the
/// per-transaction subscription sweep the batching removes.
///
/// Carries the same payload `game_entity_motion` does, minus `seq` (minted at publish time from the
/// public row, so the two can never disagree) and minus any TTL/`created_at` shape (this is a work
/// queue that the tick drains, not an event table `gc.rs` reaps).
#[table(accessor = game_entity_motion_pending)]
pub struct PendingMotion {
    #[primary_key]
    pub guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub grid_x: i32,
    pub grid_y: i32,
    /// `(grid_x, grid_y)` packed — the AOI cell key, carried through to the public row verbatim.
    /// Written in the SAME statement as `grid_x`/`grid_y` from
    /// `lyracore_shared::spatial::grid_cell_id`, per #456 (see `tripwires::grid_cell_tripwire`).
    pub cell: i64,
    pub opcode: u16,
    pub movement_info: Vec<u8>,
}

/// Drives [`publish_motion`]. ONE row; its `scheduled_at` **is** the cadence knob (the
/// `CreatureMoveSchedule` precedent: no separate `tick_ms` column, which would only duplicate
/// `scheduled_at` and then drift from it). [`set_motion_tick_ms`] is the operator lever.
///
/// LOAD-BEARING: with this row absent nothing drains `game_entity_motion_pending`, so **peer
/// movement stops relaying entirely**. `seed::init` arms it on a fresh database and
/// `debug_repair_after_publish` ensures it after every publish — but `init` does NOT re-run on an
/// auto-migrate republish and the repair pass needs a claimed operator, so [`ensure_schedule_armed`]
/// is a third, unconditional net that arms it from the movement path itself. [server]
#[table(accessor = game_motion_publish_schedule, scheduled(publish_motion))]
pub struct MotionPublishSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

// ===========================================================================================
//  The coalescing rule — pure, unit-tested below
// ===========================================================================================

/// Is `opcode` a DISCRETE state transition, i.e. an edge event a peer cannot reconstruct from a
/// later packet?
///
/// Only two movement opcodes are not: `MSG_MOVE_HEARTBEAT` and `MSG_MOVE_SET_FACING`. Both carry
/// nothing but position + orientation + flags, and **every** movement packet carries all three — so
/// a superseded one is losslessly droppable. Everything else (start/stop/strafe/turn/jump/fall-land/
/// swim/run-walk-mode) is an edge the client animates off; dropping one shows a peer sliding into a
/// run with no start, or landing with no jump.
///
/// This mirrors the gateway's own rule 1 (`world/coalesce.rs`, work-item 231: "any non-heartbeat
/// opcode is ALWAYS a state change"), one layer down — it is deliberately the SAME classification,
/// only stricter about `SET_FACING`, which the gateway forwards eagerly because the module's stored
/// heading feeds server-side facing checks.
pub(crate) fn is_discrete(opcode: u16) -> bool {
    use lyracore_shared::opcodes::movement as m;
    !matches!(
        opcode as u32,
        m::MSG_MOVE_HEARTBEAT | m::MSG_MOVE_SET_FACING
    )
}

/// What to do with a newly-arrived payload given the one already queued for the same mover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Coalesce {
    /// Take the new payload and drop the queued one. Lossless: the queued one was a pure state
    /// refresh (heartbeat / facing) whose entire content — position, orientation, flags — the new
    /// payload also carries, in fresher form.
    Replace,
    /// KEEP the queued payload and drop the new one. The queued one is a discrete transition and
    /// the new one is only a state refresh, so the transition is the strictly more informative of
    /// the two. Cost: the published position is up to one extra tick stale (~50 ms, a couple of
    /// yards) — invisible against dead-reckoning, and the very next packet or tick corrects it.
    /// The caller still re-addresses the row (map/grid/cell) so it is delivered to the cell the
    /// mover is ACTUALLY in.
    KeepQueued,
    /// Two discrete transitions inside one window (e.g. START_FORWARD then JUMP). Neither may be
    /// dropped and one public row cannot express both — two writes to the same row in one
    /// transaction coalesce into one diff for subscribers, so staging both would still lose one.
    /// Publish the QUEUED one immediately (in this movement transaction, paying one sweep) and
    /// stage the new one. This is the only path that still costs a per-packet sweep, and it is
    /// reachable only when a player produces two input-state changes inside one tick window — with
    /// the gateway's own 150 ms heartbeat coalescing (work-item 231) upstream, that is rare and
    /// self-limiting, never proportional to player count.
    FlushThenReplace,
}

/// The rule. `queued` is the opcode already staged for this mover, `incoming` the one that just
/// arrived. See [`Coalesce`] for why each case is the lossless choice.
pub(crate) fn coalesce(queued: u16, incoming: u16) -> Coalesce {
    match (is_discrete(queued), is_discrete(incoming)) {
        (true, true) => Coalesce::FlushThenReplace,
        (true, false) => Coalesce::KeepQueued,
        (false, _) => Coalesce::Replace,
    }
}

/// What the tick does with one queued row. The row is deleted from the queue either way — a stale
/// one must not sit there forever, and a published one is by definition no longer pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TickAction {
    /// The mover still has a live `game_world_entity` row: republish.
    Publish,
    /// The mover left (logout, despawn, cross-map hop, cascade delete) between staging this packet
    /// and this firing. Dropping it is what stops the tick from RESURRECTING a `game_entity_motion`
    /// row for an entity that no longer exists — a resurrected row relays a ghost to every
    /// subscriber in its cell and nothing ever deletes it again.
    DropStale,
}

/// Pure per-row tick decision (this crate has no `ReducerContext` test harness by design, so the
/// decision is a function and the reducer only executes it).
pub(crate) fn tick_action(entity_alive: bool) -> TickAction {
    if entity_alive {
        TickAction::Publish
    } else {
        TickAction::DropStale
    }
}

// ===========================================================================================
//  Write path
// ===========================================================================================

/// Publish ONE staged payload into the public `game_entity_motion` relay, minting `seq` off the
/// public row (`+1`, or 0 for a mover's first ever motion) exactly as the pre-#461 inline relay did.
fn publish(ctx: &ReducerContext, p: &PendingMotion) {
    let motions = ctx.db.game_entity_motion();
    match motions.guid().find(p.guid) {
        Some(prev) => {
            motions.guid().update(EntityMotion {
                map_id: p.map_id,
                instance_id: p.instance_id,
                grid_x: p.grid_x,
                grid_y: p.grid_y,
                cell: p.cell,
                opcode: p.opcode,
                movement_info: p.movement_info.clone(),
                seq: prev.seq.wrapping_add(1),
                ..prev
            });
        }
        None => {
            motions.insert(EntityMotion {
                guid: p.guid,
                map_id: p.map_id,
                instance_id: p.instance_id,
                grid_x: p.grid_x,
                grid_y: p.grid_y,
                cell: p.cell,
                opcode: p.opcode,
                movement_info: p.movement_info.clone(),
                seq: 0,
            });
        }
    }
}

/// Stage one mover's motion for the next [`publish_motion`] firing — what `movement_update`'s
/// `plan.relay_motion` block calls instead of writing `game_entity_motion` directly.
///
/// `map_id`/`instance_id`/grid come from the entity the caller just moved, so the staged row and the
/// entity row can never disagree about which AOI box they belong to.
#[allow(clippy::too_many_arguments)]
pub(crate) fn queue_motion(
    ctx: &ReducerContext,
    guid: u64,
    map_id: u32,
    instance_id: u64,
    grid_x: i32,
    grid_y: i32,
    opcode: u16,
    movement_info: Vec<u8>,
) {
    ensure_schedule_armed(ctx);
    let pending = ctx.db.game_entity_motion_pending();
    let cell = lyracore_shared::spatial::grid_cell_id(grid_x, grid_y);
    let Some(prev) = pending.guid().find(guid) else {
        pending.insert(PendingMotion {
            guid,
            map_id,
            instance_id,
            grid_x,
            grid_y,
            cell,
            opcode,
            movement_info,
        });
        return;
    };
    match coalesce(prev.opcode, opcode) {
        // Keep the staged TRANSITION's payload; re-address the row so it lands in the cell the
        // mover is actually in now (`..prev` keeps `opcode`/`movement_info`).
        Coalesce::KeepQueued => {
            pending.guid().update(PendingMotion {
                map_id,
                instance_id,
                grid_x,
                grid_y,
                cell,
                ..prev
            });
        }
        // Two transitions in one window: neither may be lost, so the older one goes out now.
        Coalesce::FlushThenReplace => {
            publish(ctx, &prev);
            pending.guid().update(PendingMotion {
                guid,
                map_id,
                instance_id,
                grid_x,
                grid_y,
                cell,
                opcode,
                movement_info,
            });
        }
        Coalesce::Replace => {
            pending.guid().update(PendingMotion {
                guid,
                map_id,
                instance_id,
                grid_x,
                grid_y,
                cell,
                opcode,
                movement_info,
            });
        }
    }
}

/// Drop any motion staged for `guid` — called from every path that deletes the mover's
/// `game_entity_motion` row (and from the teleport primitive, so a pre-teleport packet cannot be
/// republished at the old position after the entity has been moved). Idempotent.
pub(crate) fn drop_pending(ctx: &ReducerContext, guid: u64) {
    ctx.db.game_entity_motion_pending().guid().delete(guid);
}

// ===========================================================================================
//  The tick
// ===========================================================================================

/// **The 20 Hz republish (#461).** Drains `game_entity_motion_pending` into the public
/// `game_entity_motion` in ONE transaction, so SpacetimeDB's subscription sweep runs once per
/// firing instead of once per movement packet.
///
/// Scheduler-only (the `ctx.sender()` gate every scheduled reducer in this module carries).
///
/// The `.iter()` here is deliberate and is NOT the whole-world scan `danger-zones.md` §1.7 forbids:
/// this table is a WORK QUEUE holding only the movers staged since the previous firing (typically a
/// few dozen), it is written exclusively by movers already resident on this shard, and the whole
/// point of the pass is to visit every one of them. There is no narrower read to be had — a
/// `dirty`-flag column plus an index would visit the identical rows and additionally pay for idle
/// ones.
#[reducer]
pub fn publish_motion(ctx: &ReducerContext, _schedule: MotionPublishSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let pending = ctx.db.game_entity_motion_pending();
    let entities = ctx.db.game_world_entity();
    // Collect first: the loop deletes from the table it is walking.
    let queued: Vec<PendingMotion> = pending.iter().collect();
    for row in queued {
        pending.guid().delete(row.guid);
        match tick_action(entities.guid().find(row.guid).is_some()) {
            TickAction::Publish => publish(ctx, &row),
            TickAction::DropStale => {}
        }
    }
}

/// Third net under the schedule row (see [`MotionPublishSchedule`]): checked ONCE per wasm instance
/// (the atomic), so the steady-state cost is nothing, and it self-heals the one failure mode that
/// would otherwise be silent and total — a republished database where `init` did not re-run and
/// `debug_repair_after_publish` was never called would stage movement forever and relay none of it.
static SCHEDULE_CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn ensure_schedule_armed(ctx: &ReducerContext) {
    if SCHEDULE_CHECKED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let sched = ctx.db.game_motion_publish_schedule();
    if sched.iter().next().is_none() {
        sched.insert(MotionPublishSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(MOTION_TICK_MICROS)),
        });
        log::warn!(
            "motion publish tick was NOT armed — armed it at the {MOTION_TICK_MICROS}us default. \
             A publish landed without debug_repair_after_publish running; peer movement was staged \
             but not relayed until now."
        );
    }
}

// ===========================================================================================
//  The knob
// ===========================================================================================

/// **THE CADENCE KNOB.** Retune the republish rate live, without a republish:
/// `spacetime call <db> set_motion_tick_ms 50` (20 Hz, the default), `100` (10 Hz), `25` (40 Hz).
/// Takes effect on the next firing — the row is replaced, so the scheduler re-arms at the new
/// interval; no re-publish, no re-seed, nothing else to do. Run it on EVERY shard (each database
/// has its own schedule row).
///
/// Operator-gated rather than `debug_reducers`-gated, so the lever exists in a build compiled
/// without the debug feature. It is deliberately NOT a `game_config` column: the cadence IS
/// `scheduled_at` (the `CreatureMoveSchedule` precedent), `spacetime sql` cannot write a
/// `ScheduleAt` anyway, and `game_config` is gateway-SUBSCRIBED — an END-appended column there needs
/// a hand-patched binding (`danger-zones.md` §1.2) for a value the gateway never reads.
#[reducer]
pub fn set_motion_tick_ms(ctx: &ReducerContext, tick_ms: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if tick_ms < MOTION_TICK_MS_FLOOR {
        return Err(format!(
            "tick_ms {tick_ms} below the {MOTION_TICK_MS_FLOOR}ms floor (every firing is a \
             transaction on the one serialized commit stream)"
        ));
    }
    if tick_ms > MOTION_TICK_MS_CEIL {
        return Err(format!(
            "tick_ms {tick_ms} above the {MOTION_TICK_MS_CEIL}ms ceiling (peers would visibly \
             stutter — dead-reckoning does not cover a gap that long)"
        ));
    }
    let sched = ctx.db.game_motion_publish_schedule();
    let stale: Vec<u64> = sched.iter().map(|r| r.scheduled_id).collect();
    for id in stale {
        sched.scheduled_id().delete(id);
    }
    sched.insert(MotionPublishSchedule {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(tick_ms as i64 * 1000)),
    });
    log::info!("set_motion_tick_ms: motion republish re-armed at {tick_ms}ms");
    Ok(())
}

// ===========================================================================================
//  Tests — the pure decisions above, plus a model of the queue built out of them
// ===========================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::opcodes::movement as m;
    use std::collections::{HashMap, HashSet};

    const HEARTBEAT: u16 = m::MSG_MOVE_HEARTBEAT as u16;
    const FACING: u16 = m::MSG_MOVE_SET_FACING as u16;
    const JUMP: u16 = m::MSG_MOVE_JUMP as u16;
    const START: u16 = m::MSG_MOVE_START_FORWARD as u16;
    const STOP: u16 = m::MSG_MOVE_STOP as u16;

    #[test]
    fn only_heartbeat_and_facing_are_coalescible() {
        // Every one of the 17 relayed movement opcodes is classified, not a hand-picked subset.
        for &op in m::SLICE_MOVE_OPCODES {
            let discrete = is_discrete(op as u16);
            let expected = op != m::MSG_MOVE_HEARTBEAT && op != m::MSG_MOVE_SET_FACING;
            assert_eq!(
                discrete, expected,
                "0x{op:03X}: a discrete transition is anything a peer animates off; only \
                 HEARTBEAT and SET_FACING carry nothing a later packet does not"
            );
        }
    }

    #[test]
    fn coalesce_never_drops_a_discrete_transition() {
        // A staged refresh is always superseded — nothing in it survives that the new packet lacks.
        assert_eq!(coalesce(HEARTBEAT, HEARTBEAT), Coalesce::Replace);
        assert_eq!(coalesce(HEARTBEAT, JUMP), Coalesce::Replace);
        assert_eq!(coalesce(FACING, HEARTBEAT), Coalesce::Replace);
        assert_eq!(coalesce(FACING, STOP), Coalesce::Replace);
        // A staged TRANSITION outranks a mere refresh — the refresh is dropped, not the jump.
        assert_eq!(coalesce(JUMP, HEARTBEAT), Coalesce::KeepQueued);
        assert_eq!(coalesce(START, FACING), Coalesce::KeepQueued);
        // Two transitions in one window: the older one is flushed rather than overwritten.
        assert_eq!(coalesce(START, JUMP), Coalesce::FlushThenReplace);
        assert_eq!(coalesce(JUMP, STOP), Coalesce::FlushThenReplace);
    }

    #[test]
    fn the_tick_publishes_a_live_mover_and_drops_a_departed_one() {
        assert_eq!(tick_action(true), TickAction::Publish);
        assert_eq!(
            tick_action(false),
            TickAction::DropStale,
            "publishing for a guid with no live entity row RESURRECTS a motion row nothing will \
             ever delete again"
        );
    }

    // ---- A model of the queue, built out of the SAME pure fns the reducers execute --------------
    //
    // The module crate has no `ReducerContext` harness by design (playbook §7), so the DB wiring is
    // pinned by the source scans below and the ALGORITHM is exercised here against an in-memory
    // stand-in for the two tables. Every decision in the model is a call into `coalesce` /
    // `tick_action`; the model itself only moves rows between two maps, exactly as the reducers do.

    #[derive(Default)]
    struct Model {
        pending: HashMap<u64, (u16, Vec<u8>)>,
        /// The public relay: guid -> (opcode, movement_info, seq).
        published: HashMap<u64, (u16, Vec<u8>, u32)>,
        alive: HashSet<u64>,
        /// How many times a row was written into the public table (the thing that costs a sweep).
        publishes: usize,
    }

    impl Model {
        fn publish(&mut self, guid: u64, payload: (u16, Vec<u8>)) {
            let seq = self.published.get(&guid).map_or(0, |(_, _, s)| s + 1);
            self.published.insert(guid, (payload.0, payload.1, seq));
            self.publishes += 1;
        }

        /// `movement_update`'s relay block.
        fn movement(&mut self, guid: u64, opcode: u16, info: &[u8]) {
            let incoming = (opcode, info.to_vec());
            match self.pending.get(&guid).cloned() {
                None => {
                    self.pending.insert(guid, incoming);
                }
                Some(prev) => match coalesce(prev.0, opcode) {
                    Coalesce::Replace => {
                        self.pending.insert(guid, incoming);
                    }
                    Coalesce::KeepQueued => {}
                    Coalesce::FlushThenReplace => {
                        self.publish(guid, prev);
                        self.pending.insert(guid, incoming);
                    }
                },
            }
        }

        /// `publish_motion`.
        fn tick(&mut self) {
            for (guid, payload) in std::mem::take(&mut self.pending) {
                match tick_action(self.alive.contains(&guid)) {
                    TickAction::Publish => self.publish(guid, payload),
                    TickAction::DropStale => {}
                }
            }
        }
    }

    #[test]
    fn the_tick_republishes_a_changed_row() {
        let mut w = Model::default();
        w.alive.insert(7);
        w.movement(7, HEARTBEAT, &[1, 2, 3]);
        w.tick();
        assert_eq!(
            w.published.get(&7),
            Some(&(HEARTBEAT, vec![1, 2, 3], 0)),
            "a staged heartbeat must reach the public relay on the next firing"
        );
        // ...and a second, later packet republishes with a bumped seq.
        w.movement(7, HEARTBEAT, &[4, 5, 6]);
        w.tick();
        assert_eq!(w.published.get(&7), Some(&(HEARTBEAT, vec![4, 5, 6], 1)));
    }

    #[test]
    fn the_tick_does_not_republish_an_unchanged_row() {
        let mut w = Model::default();
        w.alive.insert(7);
        w.movement(7, HEARTBEAT, &[1, 2, 3]);
        w.tick();
        let after_first = w.publishes;
        // Ten idle firings with no movement in between.
        for _ in 0..10 {
            w.tick();
        }
        assert_eq!(
            w.publishes, after_first,
            "an entity that did not move has no queued row, so the tick must not touch its public \
             row at all — rewriting every entity every tick would just move the cost, not remove it"
        );
        assert_eq!(
            w.published.get(&7).map(|(_, _, s)| *s),
            Some(0),
            "seq must not advance while nothing moved"
        );
    }

    #[test]
    fn the_tick_does_not_resurrect_a_departed_entity() {
        let mut w = Model::default();
        w.alive.insert(7);
        w.movement(7, HEARTBEAT, &[1, 2, 3]);
        // The mover logs out AFTER staging: the lifecycle path deletes both rows...
        w.alive.remove(&7);
        w.pending.remove(&7);
        w.tick();
        assert!(!w.published.contains_key(&7));
        // ...and even if a packet were staged with no lifecycle cleanup at all (a delete site
        // missed, a race), the tick's own liveness gate still refuses to write the public row.
        w.movement(7, HEARTBEAT, &[9]);
        w.tick();
        assert!(
            !w.published.contains_key(&7),
            "the liveness gate is the backstop that makes a missed delete site degrade to a \
             dropped packet instead of an immortal ghost row"
        );
        assert!(w.pending.is_empty(), "the stale row is drained either way");
    }

    #[test]
    fn a_discrete_transition_survives_a_window_it_shares_with_a_heartbeat() {
        let mut w = Model::default();
        w.alive.insert(7);
        // Jump, then a heartbeat 10ms later — both inside one 50ms window.
        w.movement(7, JUMP, &[0xAA]);
        w.movement(7, HEARTBEAT, &[0xBB]);
        w.tick();
        assert_eq!(
            w.published.get(&7).map(|(op, info, _)| (*op, info.clone())),
            Some((JUMP, vec![0xAA])),
            "the JUMP opcode AND its movement_info (which carries the jump's own flags) must \
             survive — 'latest position wins' would publish the heartbeat and the peer would never \
             play the jump"
        );
        // The heartbeat's fresher position is not lost forever: the next packet carries it.
        w.movement(7, HEARTBEAT, &[0xCC]);
        w.tick();
        assert_eq!(
            w.published.get(&7).map(|(op, info, _)| (*op, info.clone())),
            Some((HEARTBEAT, vec![0xCC]))
        );
    }

    #[test]
    fn two_discrete_transitions_in_one_window_are_both_relayed_in_order() {
        let mut w = Model::default();
        w.alive.insert(7);
        w.movement(7, START, &[0x01]);
        w.movement(7, JUMP, &[0x02]);
        // The START went out immediately (it could not be expressed by the same public row as the
        // JUMP in one transaction), so it is already published BEFORE the firing.
        assert_eq!(
            w.published.get(&7).map(|(op, _, _)| *op),
            Some(START),
            "the older transition is flushed, never overwritten"
        );
        w.tick();
        assert_eq!(
            w.published
                .get(&7)
                .map(|(op, info, s)| (*op, info.clone(), *s)),
            Some((JUMP, vec![0x02], 1)),
            "and the newer one follows on the next firing, with a bumped seq — order preserved"
        );
    }

    #[test]
    fn a_crowd_of_heartbeats_costs_one_public_write_each_per_firing() {
        let mut w = Model::default();
        for guid in 0..100u64 {
            w.alive.insert(guid);
        }
        // Three heartbeats per player inside one window (the pre-#461 cost: 300 transactions).
        for _ in 0..3 {
            for guid in 0..100u64 {
                w.movement(guid, HEARTBEAT, &[0x01]);
            }
        }
        assert_eq!(
            w.publishes, 0,
            "staging costs no public write at all — that is the whole item"
        );
        w.tick();
        assert_eq!(
            w.publishes, 100,
            "one write per MOVER per firing, all inside the single tick transaction"
        );
    }

    // ---- Source pins: the DB wiring no pure test in this crate can reach ------------------------

    #[test]
    fn the_tick_gates_every_publish_on_the_liveness_check() {
        let body = crate::test_scan::code_of(
            include_str!("motion.rs"),
            "pub fn publish_motion(ctx: &ReducerContext, _schedule: MotionPublishSchedule) {",
        );
        assert!(
            body.contains("ctx.sender() != ctx.database_identity()"),
            "the tick must stay scheduler-only. Body was:\n{body}"
        );
        assert!(
            body.contains("tick_action(entities.guid().find(row.guid).is_some())"),
            "every queued row must go through `tick_action` with a REAL liveness probe — a \
             publish that skips it resurrects motion rows for departed entities. Body was:\n{body}"
        );
        assert_eq!(
            body.matches("publish(ctx, &row)").count(),
            1,
            "exactly one publish call, and it is the `TickAction::Publish` arm. Body was:\n{body}"
        );
        assert!(
            body.contains("pending.guid().delete(row.guid)"),
            "the queue must be drained whatever the action, or a stale row lives forever. Body \
             was:\n{body}"
        );
    }

    #[test]
    fn the_write_path_routes_every_decision_through_coalesce() {
        let body =
            crate::test_scan::code_of(include_str!("motion.rs"), "pub(crate) fn queue_motion(");
        assert!(
            body.contains("coalesce(prev.opcode, opcode)"),
            "the staged-vs-incoming decision must be `coalesce`'s, not re-derived inline. Body \
             was:\n{body}"
        );
        assert_eq!(
            body.matches("publish(ctx, &prev)").count(),
            1,
            "the ONLY inline publish on the movement path is the FlushThenReplace arm — any other \
             one reinstates the per-packet subscription sweep this item removes. Body was:\n{body}"
        );
        assert!(
            !body.contains("game_entity_motion()"),
            "`movement_update`'s path must never touch the PUBLIC relay table directly — it stages \
             into the private one and lets `publish` mint `seq`. Body was:\n{body}"
        );
    }

    #[test]
    fn the_staging_table_is_private() {
        let src = include_str!("motion.rs");
        // Assembled at run time: a contiguous literal would itself look like a `#[table(` attribute
        // to `tripwires::character_owned_tripwire::extract_tables`, which text-scans this tree.
        let needle = format!("{}{}", "#[table", "(accessor = game_entity_motion_pending");
        let attr = src
            .find(&needle)
            .expect("the staging table's own attribute");
        let end = src[attr..].find(")]").expect("attribute closes") + attr;
        assert!(
            !src[attr..end].contains("public"),
            "`game_entity_motion_pending` MUST stay private — a subscriber on it reinstates the \
             per-transaction subscription sweep (#461's entire premise). Attribute was: {}",
            &src[attr..end]
        );
    }
}
