use crate::codec::{self};
use crate::stdb::bindings::*;
use crate::stdb::world_index::ShardId;
use crate::stdb::world_view::*;
use crate::world::Outbound;

pub(crate) fn relay_breath_event(
    character_guid: u64,
    kind: u8,
    time_remaining_ms: u32,
    duration_ms: u32,
    damage: u32,
) -> Vec<Outbound> {
    let msg = match kind {
        0 => codec::build_breath_timer_start(time_remaining_ms, duration_ms),
        1 => codec::build_breath_timer_stop(),
        2 => codec::build_drowning_damage_log(character_guid, damage),
        3 => codec::build_fatigue_timer_start(true, time_remaining_ms, duration_ms),
        4 => codec::build_fatigue_timer_start(false, time_remaining_ms, duration_ms),
        5 => codec::build_fatigue_timer_stop(),
        6 => codec::build_exhaustion_damage_log(character_guid, damage),
        unknown => {
            log::warn!("breath relay: dropping unknown kind {unknown} for {character_guid}");
            return Vec::new();
        }
    };
    vec![Outbound::One(msg)]
}

pub(crate) fn breath_relay_appeared(view: &WorldView, shard: ShardId, row: &BreathRelayEvent) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(row.character_guid)) else {
        return;
    };
    let (guid, kind, remaining, duration, damage) = (
        row.character_guid,
        row.kind,
        row.time_remaining_ms,
        row.duration_ms,
        row.damage,
    );
    enqueue(viewer.clone(), move |_| {
        crate::codec::breath::subscriptions::relay_breath_event(
            guid, kind, remaining, duration, damage,
        )
    });
}

crate::stdb::relays::declare_relays! {
    game_breath_relay_event [false] {
        insert(Snapshot) => |context, _event, row| breath_relay_appeared(&context.view, context.shard, row);
    }
}
