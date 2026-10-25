use lyracore_protocol::codec;
use lyracore_protocol::messages::vanilla::{opcodes::ServerOpcodeMessage, CMSG_CHAR_CREATE};
use spacetimedb::ReducerContext;

use super::Rollback;
use crate::guild::game_guild;

pub(super) fn create(
    ctx: &ReducerContext,
    account_id: u64,
    request: CMSG_CHAR_CREATE,
) -> Result<ServerOpcodeMessage, Rollback> {
    let appearance = crate::auth::Appearance {
        gender: request.gender.as_int(),
        skin: request.skin_color,
        face: request.face,
        hair_style: request.hair_style,
        hair_color: request.hair_color,
        facial_hair: request.facial_hair,
    };
    crate::auth::insert_new_character(
        ctx,
        account_id,
        request.name,
        request.race.as_int(),
        request.class.as_int(),
        appearance,
    )
    .map_err(|cause| {
        let outcome = match cause.as_str() {
            "NAME_IN_USE" => codec::CharCreateOutcome::NameInUse,
            "SERVER_LIMIT" => codec::CharCreateOutcome::ServerLimit,
            _ => codec::CharCreateOutcome::Failed,
        };
        Rollback::reply(
            ServerOpcodeMessage::SMSG_CHAR_CREATE(codec::build_char_create_response(outcome)),
            cause,
        )
    })?;
    Ok(ServerOpcodeMessage::SMSG_CHAR_CREATE(
        codec::build_char_create_response(codec::CharCreateOutcome::Success),
    ))
}

pub(super) fn delete(
    ctx: &ReducerContext,
    account_id: u64,
    guid: u64,
) -> Result<ServerOpcodeMessage, Rollback> {
    let apply = || {
        if ctx
            .db
            .game_guild()
            .by_leader()
            .filter(guid)
            .next()
            .is_some()
        {
            return Err("Guild Leader cannot delete their Character".into());
        }
        crate::auth::apply_delete_character(ctx, account_id, guid)
    };
    apply().map_err(|cause| {
        Rollback::reply(
            ServerOpcodeMessage::SMSG_CHAR_DELETE(codec::build_char_delete_response(
                codec::CharDeleteOutcome::Failed,
            )),
            cause,
        )
    })?;
    Ok(ServerOpcodeMessage::SMSG_CHAR_DELETE(
        codec::build_char_delete_response(codec::CharDeleteOutcome::Success),
    ))
}
