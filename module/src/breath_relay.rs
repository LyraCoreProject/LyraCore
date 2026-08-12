//! Client-facing breath updates: a one-shot relay row on each timer edge or drowning hit.
//!
//! The private `breath` state owns elapsed-time accounting.  This table deliberately carries only
//! the facts the client needs to mirror it: start the countdown, stop it, or display a drowning
//! damage line.  In particular, a draining second that does not cross either edge writes nothing.

use spacetimedb::{table, ReducerContext, Table, Timestamp};

/// A breath timer began; the gateway sends `SMSG_START_MIRROR_TIMER`.
pub const KIND_START: u8 = 0;
/// A breath timer ended after surfacing; the gateway sends `SMSG_STOP_MIRROR_TIMER`.
pub const KIND_STOP: u8 = 1;
/// One server-applied drowning hit; the gateway sends `SMSG_ENVIRONMENTAL_DAMAGE_LOG`.
pub const KIND_DROWNING_DAMAGE: u8 = 2;

/// A public relay row consumed by the gateway. The recipient is always `character_guid`; this is
/// public rather than RLS-scoped because coordinator subscriptions share the feed and resolve that
/// self-only audience through the owner-session lookup.
#[table(accessor = game_breath_relay_event, public)]
pub struct BreathRelayEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub kind: u8,
    /// Starting air remaining, in milliseconds. Used only by [`KIND_START`].
    pub time_remaining_ms: u32,
    /// Full breath duration, in milliseconds. Used only by [`KIND_START`].
    pub duration_ms: u32,
    /// Health actually removed by the resolver. Used only by [`KIND_DROWNING_DAMAGE`].
    pub damage: u32,
    pub created_at: Timestamp,
}

// Character-owned sweep: this is transient relay data, so it dies with the character. Public
// rows have no owner identity to re-stamp.
crate::character_owned!(delete, fn sweep_delete_game_breath_relay_event(ctx, character_guid) {
    let events = ctx.db.game_breath_relay_event();
    for row in events.iter().filter(|row| row.character_guid == character_guid).collect::<Vec<_>>() {
        events.id().delete(row.id);
    }
});

// CROSS-DATABASE transport: this one-shot relay has a GC TTL and no durable state. Carrying it
// would replay a stale breath bar or drowning line after a transfer; the destination re-arms from
// its next movement edge instead.
crate::character_owned!(not_transported, fn sweep_transfer_game_breath_relay_event());

fn insert(ctx: &ReducerContext, character_guid: u64, kind: u8, time_remaining_ms: u32, duration_ms: u32, damage: u32) {
    ctx.db.game_breath_relay_event().insert(BreathRelayEvent {
        id: 0,
        character_guid,
        kind,
        time_remaining_ms,
        duration_ms,
        damage,
        created_at: ctx.timestamp,
    });
}

/// Write the single packet-worthy edge when a player submerges.
pub(crate) fn start(ctx: &ReducerContext, character_guid: u64, time_remaining_ms: u32, duration_ms: u32) {
    insert(ctx, character_guid, KIND_START, time_remaining_ms, duration_ms, 0);
}

/// Write the single packet-worthy edge when a player surfaces.
pub(crate) fn stop(ctx: &ReducerContext, character_guid: u64) {
    insert(ctx, character_guid, KIND_STOP, 0, 0, 0);
}

/// Relay the exact damage that the server already applied to a drowning player.
pub(crate) fn drowning_damage(ctx: &ReducerContext, character_guid: u64, damage: u32) {
    insert(ctx, character_guid, KIND_DROWNING_DAMAGE, 0, 0, damage);
}
