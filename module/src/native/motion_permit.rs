use lyracore_shared::constants::player_flags::GHOST;
use lyracore_shared::movement_flags::ON_TRANSPORT;
use spacetimedb::{table, ConnectionId, ReducerContext, Table};

use super::game_native_player;
use crate::world::game_world_entity;

/// A World Session whose own movement the Native Host relays to observers.
#[table(accessor = game_native_motion_permit)]
pub struct NativeMotionPermit {
    #[primary_key]
    pub connection_id: ConnectionId,
    #[unique]
    pub character_guid: u64,
}

crate::character_owned!(delete, fn sweep_delete_game_native_motion_permit(ctx, character_guid) {
    ctx.db
        .game_native_motion_permit()
        .character_guid()
        .delete(character_guid);
});

// A Motion Permit belongs to its native connection.
crate::character_owned!(not_transported, fn sweep_transfer_game_native_motion_permit());

/// Refreshes the Motion Permit of the Character's World Session, if it has one.
pub(crate) fn refresh_character(ctx: &ReducerContext, guid: u64) {
    if let Some(player) = super::world_session(ctx, guid) {
        refresh(ctx, player.connection_id, guid);
    }
}

/// Holds the permit while the Character moves freely and withdraws it otherwise.
pub(super) fn refresh(ctx: &ReducerContext, connection_id: ConnectionId, guid: u64) {
    let in_world = ctx
        .db
        .game_native_player()
        .connection_id()
        .find(connection_id)
        .is_some_and(|player| player.character_guid == guid && player.teleport.is_none());
    let permitted = in_world
        && ctx
            .db
            .game_world_entity()
            .guid()
            .find(guid)
            .is_some_and(|entity| {
                (!entity.dead || entity.player_flags & GHOST != 0)
                    && entity.movement_flags & ON_TRANSPORT == 0
                    && !crate::taxi::is_in_flight(ctx, guid)
                    && !crate::spell::is_movement_blocked(ctx, guid)
            });
    if permitted {
        // A permitted mover's Module relays go out in this transaction, not at the motion tick.
        crate::motion::flush(ctx, guid);
    }
    let permits = ctx.db.game_native_motion_permit();
    match (permits.connection_id().find(connection_id), permitted) {
        (None, true) => {
            permits.insert(NativeMotionPermit {
                connection_id,
                character_guid: guid,
            });
        }
        (Some(permit), true) if permit.character_guid != guid => {
            permits.connection_id().update(NativeMotionPermit {
                connection_id,
                character_guid: guid,
            });
        }
        (Some(_), false) => {
            permits.connection_id().delete(connection_id);
        }
        _ => {}
    }
}

pub(super) fn withdraw(ctx: &ReducerContext, connection_id: ConnectionId) {
    ctx.db
        .game_native_motion_permit()
        .connection_id()
        .delete(connection_id);
}
