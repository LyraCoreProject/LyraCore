use spacetimedb::{table, ReducerContext, Table, Timestamp};

pub const KIND_START: u8 = 0;
pub const KIND_STOP: u8 = 1;
pub const KIND_DROWNING_DAMAGE: u8 = 2;
pub const KIND_FATIGUE_DRAIN: u8 = 3;
pub const KIND_FATIGUE_RECOVER: u8 = 4;
pub const KIND_FATIGUE_STOP: u8 = 5;
pub const KIND_EXHAUSTION_DAMAGE: u8 = 6;

#[table(accessor = game_breath_relay_event, public)]
pub struct BreathRelayEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub kind: u8,
    pub time_remaining_ms: u32,
    pub duration_ms: u32,
    pub damage: u32,
    pub created_at: Timestamp,
}

crate::character_owned!(delete, fn sweep_delete_game_breath_relay_event(ctx, character_guid) {
    let events = ctx.db.game_breath_relay_event();
    for row in events.iter().filter(|row| row.character_guid == character_guid).collect::<Vec<_>>() {
        events.id().delete(row.id);
    }
});

crate::character_owned!(not_transported, fn sweep_transfer_game_breath_relay_event());

fn insert(
    ctx: &ReducerContext,
    character_guid: u64,
    kind: u8,
    time_remaining_ms: u32,
    duration_ms: u32,
    damage: u32,
) {
    let row = ctx.db.game_breath_relay_event().insert(BreathRelayEvent {
        id: 0,
        character_guid,
        kind,
        time_remaining_ms,
        duration_ms,
        damage,
        created_at: ctx.timestamp,
    });
    crate::native::breath_event(ctx, &row);
}

pub(crate) fn start(
    ctx: &ReducerContext,
    character_guid: u64,
    time_remaining_ms: u32,
    duration_ms: u32,
) {
    insert(
        ctx,
        character_guid,
        KIND_START,
        time_remaining_ms,
        duration_ms,
        0,
    );
}

pub(crate) fn stop(ctx: &ReducerContext, character_guid: u64) {
    insert(ctx, character_guid, KIND_STOP, 0, 0, 0);
}

pub(crate) fn drowning_damage(ctx: &ReducerContext, character_guid: u64, damage: u32) {
    insert(ctx, character_guid, KIND_DROWNING_DAMAGE, 0, 0, damage);
}

pub(crate) fn fatigue(
    ctx: &ReducerContext,
    character_guid: u64,
    in_deep_water: bool,
    time_remaining_ms: u32,
    duration_ms: u32,
) {
    let kind = if in_deep_water {
        KIND_FATIGUE_DRAIN
    } else {
        KIND_FATIGUE_RECOVER
    };
    insert(ctx, character_guid, kind, time_remaining_ms, duration_ms, 0);
}

pub(crate) fn fatigue_stop(ctx: &ReducerContext, character_guid: u64) {
    insert(ctx, character_guid, KIND_FATIGUE_STOP, 0, 0, 0);
}

pub(crate) fn exhaustion_damage(ctx: &ReducerContext, character_guid: u64, damage: u32) {
    insert(ctx, character_guid, KIND_EXHAUSTION_DAMAGE, 0, 0, damage);
}
