use lyracore_protocol::codec;
use spacetimedb::{ReducerContext, ScheduleAt};

use super::game_native_player;
use crate::{game_instance_removal, InstanceRemoval};

fn remaining_ms(due: ScheduleAt, now: i64) -> u32 {
    let ScheduleAt::Time(at) = due else {
        return 0;
    };
    let micros = at.to_micros_since_unix_epoch().saturating_sub(now).max(0) as u64;
    micros.div_ceil(1000).min(u64::from(u32::MAX)) as u32
}

pub(crate) fn removal_changed(ctx: &ReducerContext, guid: u64, row: Option<&InstanceRemoval>) {
    let Some(connection) = ctx.db.game_native_player().character_guid().find(guid) else {
        return;
    };
    let timer = row.map_or(0, |row| {
        remaining_ms(
            match row.scheduled_at {
                ScheduleAt::Time(at) => ScheduleAt::Time(crate::time::scheduled_time(ctx, at)),
                interval => interval,
            },
            crate::time::now(ctx).to_micros_since_unix_epoch(),
        )
    });
    let (opcode, body) = codec::build_raid_group_only(timer);
    super::send_raw(ctx, connection.connection_id, opcode, body, false);
}

pub(super) fn replay(ctx: &ReducerContext, guid: u64) {
    let row = ctx.db.game_instance_removal().character_guid().find(guid);
    removal_changed(ctx, guid, row.as_ref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use spacetimedb::Timestamp;

    #[test]
    fn countdown_rounds_up_and_clears_at_its_deadline() {
        let due = ScheduleAt::Time(Timestamp::from_micros_since_unix_epoch(60_000_000));
        assert_eq!(remaining_ms(due, 0), 60_000);
        assert_eq!(remaining_ms(due, 59_998_999), 2);
        assert_eq!(remaining_ms(due, 59_999_999), 1);
        assert_eq!(remaining_ms(due, 60_000_000), 0);
        assert_eq!(remaining_ms(due, i64::MAX), 0);
        assert_eq!(remaining_ms(due, i64::MIN), u32::MAX);
    }
}
