use spacetimedb::{reducer, table, ConnectionId, Identity, ReducerContext, Table};

use super::{game_native_world_connection, NativeWorldState};
use crate::game_account;

#[table(accessor = game_native_conformance_connection)]
pub struct NativeConformanceConnection {
    #[primary_key]
    pub connection_id: ConnectionId,
    pub account_id: u64,
    pub owner_identity: Identity,
}

pub(super) fn account(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    account: &mut crate::Account,
) -> Result<bool, String> {
    let Some(fixture) = ctx
        .db
        .game_native_conformance_connection()
        .connection_id()
        .find(connection_id)
    else {
        return Ok(false);
    };
    if fixture.account_id != account.id || account.banned {
        return Err("conformance Account is no longer available".into());
    }
    // World entry needs an owner; this copy never changes the Account's login identity.
    account.identity = Some(fixture.owner_identity);
    Ok(true)
}

pub(super) fn close(ctx: &ReducerContext, connection_id: ConnectionId) {
    ctx.db
        .game_native_conformance_connection()
        .connection_id()
        .delete(connection_id);
}

#[reducer]
pub fn debug_native_enter_world(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let connection_id = super::connection_id(ctx)?;
    let connection = ctx
        .db
        .game_native_world_connection()
        .connection_id()
        .find(connection_id)
        .ok_or("native connection is not open")?;
    if !matches!(connection.state, NativeWorldState::AwaitingProof) {
        return Err("debug world entry requires an unauthenticated connection".into());
    }
    let character = crate::helpers::character_by_guid(ctx, character_guid)
        .ok_or("Character no longer exists")?;
    let account = ctx
        .db
        .game_account()
        .id()
        .find(character.account_id)
        .ok_or("Account no longer exists")?;
    if account.banned {
        return Err("Account is banned".into());
    }
    ctx.db
        .game_native_conformance_connection()
        .insert(NativeConformanceConnection {
            connection_id,
            account_id: account.id,
            owner_identity: account.identity.unwrap_or(ctx.sender()),
        });
    super::world::enter_character(
        ctx,
        connection,
        account.id,
        character_guid,
        crate::world::WorldEntry::Login,
        0,
    )
}
