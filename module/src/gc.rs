use spacetimedb::{reducer, table, ReducerContext, ScheduleAt};

#[table(accessor = game_event_reaper_schedule, scheduled(reap_movement_events))]
pub struct EventReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

// Deletes the rows of an `id: u64` table whose `created_at` is older than the TTL.
macro_rules! reap_expired {
    ($ctx:expr, $accessor:ident, $ttl_micros:expr) => {{
        use spacetimedb::Table as _;
        let cutoff = $crate::time::now($ctx).to_micros_since_unix_epoch() - $ttl_micros;
        let table = $ctx.db.$accessor();
        let stale: Vec<u64> = table
            .iter()
            .filter(|row| row.created_at.to_micros_since_unix_epoch() < cutoff)
            .map(|row| row.id)
            .collect();
        for id in stale {
            table.id().delete(id);
        }
    }};
}
pub(crate) use reap_expired;

#[reducer]
pub fn reap_movement_events(ctx: &ReducerContext, _schedule: EventReaperSchedule) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    crate::xp::sweep_expired(ctx);
    crate::chat::sweep_expired(ctx);
    crate::realm_chat::sweep_expired(ctx);
    crate::channel::sweep_expired(ctx);
    crate::bridge::sweep_expired(ctx);
    crate::group::sweep_expired(ctx);
    crate::trade::sweep_expired(ctx);
    crate::duel::sweep_expired(ctx);
    crate::auction::sweep_expired(ctx);
    crate::world::sweep_expired(ctx);
    crate::rest::sweep_expired(ctx);
    crate::breath_relay::sweep_expired(ctx);
    crate::guild::sweep_expired(ctx);
    crate::guild::membership::sweep_expired(ctx);
    crate::mail_timer::sweep_expired(ctx);
    crate::spell::stacking::sweep_dr_state(ctx);
    crate::loot::sweep_loot_rolls(ctx);
    crate::corpse::sweep_corpse_decay(ctx);
}
