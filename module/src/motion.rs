use spacetimedb::{log, reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::{game_entity_motion, game_world_entity, EntityMotion};

pub const MOTION_TICK_MICROS: i64 = 50_000;

pub const MOTION_TICK_MS_FLOOR: u64 = 10;
pub const MOTION_TICK_MS_CEIL: u64 = 1_000;

// Keep staging private; subscribing here restores a subscription sweep per heartbeat.
#[table(accessor = game_entity_motion_pending)]
pub struct PendingMotion {
    #[primary_key]
    pub guid: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub grid_x: i32,
    pub grid_y: i32,
    pub cell: i64,
    pub opcode: u16,
    pub movement_info: Vec<u8>,
}

#[table(accessor = game_motion_publish_schedule, scheduled(publish_motion))]
pub struct MotionPublishSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

pub(crate) fn is_discrete(opcode: u16) -> bool {
    use lyracore_shared::opcodes::movement as m;
    !matches!(
        opcode as u32,
        m::MSG_MOVE_HEARTBEAT | m::MSG_MOVE_SET_FACING | m::MSG_MOVE_SET_PITCH
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Coalesce {
    Replace,
    KeepQueued,
    // Public updates collapse within a transaction; distinct transitions need separate firings.
    FlushThenReplace,
}

pub(crate) fn coalesce(queued: u16, incoming: u16) -> Coalesce {
    match (is_discrete(queued), is_discrete(incoming)) {
        (true, true) => Coalesce::FlushThenReplace,
        (true, false) => Coalesce::KeepQueued,
        (false, _) => Coalesce::Replace,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TickAction {
    Publish,
    DropStale,
}

pub(crate) fn tick_action(entity_alive: bool) -> TickAction {
    if entity_alive {
        TickAction::Publish
    } else {
        TickAction::DropStale
    }
}

fn publish(ctx: &ReducerContext, p: &PendingMotion) {
    crate::native::motion(ctx, p);
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
        // Keep the queued transition but update its AOI address to the current cell.
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

pub(crate) fn drop_pending(ctx: &ReducerContext, guid: u64) {
    ctx.db.game_entity_motion_pending().guid().delete(guid);
}

#[reducer]
pub fn publish_motion(ctx: &ReducerContext, _schedule: MotionPublishSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    publish_staged(ctx);
}

pub(crate) fn publish_staged(ctx: &ReducerContext) {
    let pending = ctx.db.game_entity_motion_pending();
    let entities = ctx.db.game_world_entity();
    let queued: Vec<PendingMotion> = pending.iter().collect();
    for row in queued {
        pending.guid().delete(row.guid);
        match tick_action(entities.guid().find(row.guid).is_some()) {
            TickAction::Publish => publish(ctx, &row),
            TickAction::DropStale => {}
        }
    }
}

static SCHEDULE_CHECKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn ensure_schedule_armed(ctx: &ReducerContext) {
    if SCHEDULE_CHECKED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let sched = ctx.db.game_motion_publish_schedule();
    if sched.iter().next().is_none() {
        sched.insert(MotionPublishSchedule {
            scheduled_id: 0,
            scheduled_at: crate::time::schedule_interval(
                ctx,
                TimeDuration::from_micros(MOTION_TICK_MICROS),
            ),
        });
        log::warn!(
            "motion publish tick was NOT armed — armed it at the {MOTION_TICK_MICROS}us default. \
             A publish landed without debug_repair_after_publish running; peer movement was staged \
             but not relayed until now."
        );
    }
}

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
        scheduled_at: crate::time::schedule_interval(
            ctx,
            TimeDuration::from_micros(tick_ms as i64 * 1000),
        ),
    });
    log::info!("set_motion_tick_ms: motion republish re-armed at {tick_ms}ms");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::opcodes::movement as m;

    const HEARTBEAT: u16 = m::MSG_MOVE_HEARTBEAT as u16;
    const FACING: u16 = m::MSG_MOVE_SET_FACING as u16;
    const JUMP: u16 = m::MSG_MOVE_JUMP as u16;
    const START: u16 = m::MSG_MOVE_START_FORWARD as u16;
    const STOP: u16 = m::MSG_MOVE_STOP as u16;

    #[test]
    fn only_heartbeat_facing_and_pitch_are_coalescible() {
        for &op in m::SLICE_MOVE_OPCODES
            .iter()
            .chain(m::ACKNOWLEDGED_MOVE_OPCODES)
        {
            let discrete = is_discrete(op as u16);
            let expected = ![
                m::MSG_MOVE_HEARTBEAT,
                m::MSG_MOVE_SET_FACING,
                m::MSG_MOVE_SET_PITCH,
            ]
            .contains(&op);
            assert_eq!(
                discrete, expected,
                "0x{op:03X}: a discrete transition is anything a peer animates off; only \
                 HEARTBEAT, SET_FACING and SET_PITCH carry nothing a later packet does not"
            );
        }
    }

    #[test]
    fn coalesce_never_drops_a_discrete_transition() {
        assert_eq!(coalesce(HEARTBEAT, HEARTBEAT), Coalesce::Replace);
        assert_eq!(coalesce(HEARTBEAT, JUMP), Coalesce::Replace);
        assert_eq!(coalesce(FACING, HEARTBEAT), Coalesce::Replace);
        assert_eq!(coalesce(FACING, STOP), Coalesce::Replace);
        assert_eq!(coalesce(JUMP, HEARTBEAT), Coalesce::KeepQueued);
        assert_eq!(coalesce(START, FACING), Coalesce::KeepQueued);
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
}
