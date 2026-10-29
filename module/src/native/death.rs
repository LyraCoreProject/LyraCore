use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        SMSG_SPIRIT_HEALER_CONFIRM, SMSG_UPDATE_OBJECT,
    },
};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::{game_native_player, Rollback};
use crate::{
    game_corpse,
    spell::{game_resurrect_request, game_self_resurrect_option, ResurrectRequest},
    Corpse,
};

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    let character = super::actor(ctx, guid)?;
    let result = match message {
        ClientOpcodeMessage::CMSG_REPOP_REQUEST => crate::world::do_repop(ctx, guid),
        ClientOpcodeMessage::MSG_CORPSE_QUERY => {
            let location = ctx
                .db
                .game_corpse()
                .guid()
                .find(crate::corpse::corpse_guid_for(guid))
                .filter(|corpse| !corpse.is_bones)
                .map(|corpse| (corpse.map_id, corpse.x, corpse.y, corpse.z));
            let response =
                codec::build_corpse_query_response(location).map_err(|error| error.to_string())?;
            return Ok(super::send(
                ctx,
                connection,
                ServerOpcodeMessage::MSG_CORPSE_QUERY(Box::new(response)),
                false,
            )?);
        }
        ClientOpcodeMessage::CMSG_RECLAIM_CORPSE(_) => {
            crate::corpse::apply_reclaim_corpse(ctx, character)
        }
        ClientOpcodeMessage::CMSG_RESURRECT_RESPONSE(request) => {
            if ctx
                .db
                .game_resurrect_request()
                .target_guid()
                .find(guid)
                .is_none_or(|offer| offer.caster_guid != request.guid.guid())
            {
                return Ok(());
            }
            crate::spell::do_resurrect_response(ctx, guid, request.status != 0)
        }
        ClientOpcodeMessage::CMSG_SELF_RES => crate::spell::do_self_resurrect(ctx, guid),
        ClientOpcodeMessage::CMSG_SPIRIT_HEALER_ACTIVATE(request) => {
            crate::ghost::spirit_healer_gate(ctx, guid, request.guid.guid())?;
            crate::world::do_spirit_healer_res(ctx, guid)
        }
        _ => return Err("not a native death request".into()),
    };
    Ok(result?)
}

pub(super) fn confirm_spirit_healer(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
) -> Result<(), String> {
    super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_SPIRIT_HEALER_CONFIRM(SMSG_SPIRIT_HEALER_CONFIRM {
            guid: lyracore_protocol::messages::Guid::new(guid),
        }),
        false,
    )
}

pub(super) fn create(corpse: &Corpse) -> SMSG_UPDATE_OBJECT {
    codec::build_corpse_create_object(&codec::CorpseView {
        guid: corpse.guid,
        owner_guid: corpse.owner_guid,
        x: corpse.x,
        y: corpse.y,
        z: corpse.z,
        orientation: corpse.orientation,
        display_id: corpse.display_id,
        bytes_1: corpse.bytes_1,
        bytes_2: corpse.bytes_2,
        is_bones: corpse.is_bones,
    })
}

pub(crate) fn corpse_created(ctx: &ReducerContext, corpse: &Corpse) {
    let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(corpse.owner_guid)
    else {
        return;
    };
    if corpse.is_bones {
        return;
    }
    let elapsed = crate::time::now(ctx)
        .to_micros_since_unix_epoch()
        .saturating_sub(corpse.created_at.to_micros_since_unix_epoch())
        .max(0);
    let remaining = corpse.reclaim_delay_micros.saturating_sub(elapsed).max(0) as u64;
    let (opcode, body) =
        codec::build_corpse_reclaim_delay_raw(std::time::Duration::from_micros(remaining));
    super::send_raw(ctx, owner.connection_id, opcode, body, false);
}

pub(crate) fn corpse_changed(ctx: &ReducerContext, corpse: &Corpse) {
    if ctx.db.game_native_player().count() == 0 {
        return;
    }
    if corpse.is_bones {
        corpse_removed(ctx, corpse.owner_guid);
    }
    let packet =
        lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_corpse_flags_values(corpse.guid, corpse.is_bones),
        )))
        .expect("corpse flags fit a world frame");
    super::visibility::broadcast_aoi(
        ctx,
        super::NativeAoi {
            layer: crate::aoi::CORPSE,
            entity_guid: corpse.guid,
            exclude_connection: None,
        },
        packet.opcode,
        packet.body,
    );
}

pub(crate) fn corpse_removed(ctx: &ReducerContext, owner: u64) {
    send(
        ctx,
        owner,
        ServerOpcodeMessage::MSG_CORPSE_QUERY(Box::new(
            lyracore_protocol::messages::vanilla::MSG_CORPSE_QUERY_Server::NotFound,
        )),
    );
}

fn send(ctx: &ReducerContext, guid: u64, message: ServerOpcodeMessage) {
    if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
        if let Err(error) = super::send(ctx, owner.connection_id, message, false) {
            spacetimedb::log::error!("native resurrection message failed for {guid}: {error}");
            super::disconnect(ctx, owner.connection_id);
        }
    }
}

pub(crate) fn resurrect_offered(ctx: &ReducerContext, offer: &ResurrectRequest) {
    if let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(offer.target_guid)
    {
        let (opcode, body) =
            codec::build_resurrect_request_raw(offer.caster_guid, &offer.caster_name);
        super::send_raw(ctx, owner.connection_id, opcode, body, false);
    }
}

pub(super) fn group_event(ctx: &ReducerContext, guid: u64, kind: u8) {
    if kind == lyracore_shared::group::event_kind::DEATH_DURABILITY {
        send(ctx, guid, ServerOpcodeMessage::SMSG_DURABILITY_DAMAGE_DEATH);
    }
}

pub(crate) fn self_resurrection_changed(ctx: &ReducerContext, guid: u64, spell_id: u32) {
    send(
        ctx,
        guid,
        ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_self_res_spell_values(
            guid, spell_id,
        ))),
    );
}

pub(super) fn replay(ctx: &ReducerContext, guid: u64) {
    if let Some(corpse) = ctx
        .db
        .game_corpse()
        .guid()
        .find(crate::corpse::corpse_guid_for(guid))
    {
        corpse_created(ctx, &corpse);
    }

    if let Some(offer) = ctx.db.game_resurrect_request().target_guid().find(guid) {
        resurrect_offered(ctx, &offer);
    }
    if let Some(option) = ctx
        .db
        .game_self_resurrect_option()
        .character_guid()
        .find(guid)
    {
        self_resurrection_changed(ctx, guid, option.spell_id);
    }
}
