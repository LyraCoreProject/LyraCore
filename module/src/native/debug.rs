use sha1::{Digest, Sha1};
use spacetimedb::{reducer, ReducerContext, Table};

use super::{game_native_cipher, game_native_world_connection, NativeCipher, NativeWorldState};
use crate::game_account;

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
    let session_key = vec![0x5a; 40];
    crate::auth::establish_session(ctx, account.id, session_key.clone(), ctx.sender())?;
    // Keep the normal per-request proof Gate after bypassing SRP for this fixture.
    let client_proof = Sha1::new()
        .chain_update(account.username.as_bytes())
        .chain_update(0u32.to_le_bytes())
        .chain_update(0u32.to_le_bytes())
        .chain_update(connection.server_seed.to_le_bytes())
        .chain_update(&session_key)
        .finalize()
        .to_vec();
    ctx.db.game_native_cipher().insert(NativeCipher {
        connection_id,
        username: account.username,
        session_key,
        client_seed: 0,
        client_proof,
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
