use lyracore_protocol::codec;
use lyracore_protocol::messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use crate::action_bar::game_player_action;
use crate::aoi::game_aoi_grant;
use crate::game_world_entity;
use crate::reputation::game_player_reputation;
use crate::skill::game_player_skill;
use crate::spell::spellbook::game_player_spell;

use super::{
    game_native_player, game_native_world_connection, NativeActive, NativePlayer,
    NativeWorldConnection, NativeWorldState, Rollback,
};

pub(super) fn login(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    body: &[u8],
) -> Result<(), Rollback> {
    enter(ctx, connection, body).map_err(|cause| {
        Rollback::reply(
            ServerOpcodeMessage::SMSG_CHARACTER_LOGIN_FAILED(codec::build_character_login_failed()),
            cause,
        )
    })
}

fn enter(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    body: &[u8],
) -> Result<(), String> {
    let NativeWorldState::CharacterSelection { account_id } = connection.state else {
        return Err("character login requires character selection".into());
    };
    let guid = u64::from_le_bytes(
        body.try_into()
            .map_err(|_| "invalid character login packet")?,
    );
    enter_character(
        ctx,
        connection,
        account_id,
        guid,
        crate::world::WorldEntry::Login,
        0,
    )
}

pub(super) fn enter_character(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    account_id: u64,
    guid: u64,
    entry: crate::world::WorldEntry,
    teleport_counter: u32,
) -> Result<(), String> {
    let account = super::current_account(ctx, &connection, account_id)?;
    let owner = account
        .identity
        .ok_or("Account has no authenticated identity")?;
    let character =
        crate::helpers::character_by_guid(ctx, guid).ok_or("Character no longer exists")?;
    if character.account_id != account_id || account.banned {
        return Err("Account cannot log in this Character".into());
    }
    let cinematic = character
        .first_login
        .then(|| codec::opening_cinematic(character.race))
        .flatten();
    if let Some(previous) = ctx.db.game_native_player().account_id().find(account_id) {
        super::disconnect(ctx, previous.connection_id);
    }
    crate::world::apply_player_login(ctx, &account, guid, owner, entry)?;
    let live = ctx
        .db
        .game_world_entity()
        .guid()
        .find(guid)
        .ok_or("world entry did not create a player")?;
    let mut entity = super::view::entity(ctx, &live);
    super::taxi::create_spline(ctx, guid, &mut entity);
    let items = super::view::items(ctx, guid);
    let learned = ctx
        .db
        .game_player_spell()
        .by_character()
        .filter(guid)
        .map(|spell| spell.spell_id)
        .collect::<Vec<_>>();
    let skills = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(guid)
        .map(|skill| (skill.skill_line, skill.current, skill.max_rank))
        .collect::<Vec<_>>();
    let reputations = ctx
        .db
        .game_player_reputation()
        .by_character()
        .filter(guid)
        .map(|rep| (rep.reputation_index, rep.standing, rep.at_war))
        .collect::<Vec<_>>();
    let actions = ctx
        .db
        .game_player_action()
        .by_character()
        .filter(guid)
        .map(|action| (action.button, action.action, action.action_type))
        .collect::<Vec<_>>();
    let messages = codec::login_sequence_messages(
        &entity,
        &learned,
        &reputations,
        &actions,
        match entry {
            crate::world::WorldEntry::Login => codec::WorldEntry::FreshLogin,
            crate::world::WorldEntry::WorldPort => codec::WorldEntry::WorldPort,
        },
        crate::time::now(ctx).to_micros_since_unix_epoch().max(0) as u64 / 1_000_000,
    )
    .map_err(|error| error.to_string())?;
    for message in messages {
        super::send(ctx, connection.connection_id, message, false)?;
    }
    for item in &items {
        super::send(
            ctx,
            connection.connection_id,
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_item_create_object(
                item,
            ))),
            false,
        )?;
    }
    let quests = super::view::quests(ctx, guid);
    let self_create = codec::build_self_create_object(&entity, &items, &skills, &quests)
        .map_err(|error| error.to_string())?;
    crate::aoi::enter_viewer(ctx, guid)?;
    let mut grants: Vec<_> = ctx.db.game_aoi_grant().by_viewer().filter(guid).collect();
    grants.sort_by_key(|grant| grant.layer != crate::aoi::TRANSPORT);
    // A Passenger's self CREATE names its Transport, so the client must already have it.
    let carrier = crate::transport::passenger(ctx, guid).and_then(|passenger| {
        let carrier = crate::transport::guid(passenger.transport_entry);
        grants
            .iter()
            .position(|grant| grant.layer == crate::aoi::TRANSPORT && grant.entity_guid == carrier)
    });
    let movement = carrier.and_then(|_| super::transport::passenger_movement(ctx, &live));
    match (carrier, movement) {
        (Some(index), Some(movement)) => {
            super::visibility::create(ctx, connection.connection_id, &grants.remove(index))?;
            let body = codec::create_with_movement(&self_create, &movement)
                .map_err(|error| error.to_string())?;
            super::send_raw(
                ctx,
                connection.connection_id,
                codec::SMSG_UPDATE_OBJECT_OPCODE,
                body,
                false,
            );
        }
        _ => super::send(
            ctx,
            connection.connection_id,
            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(self_create)),
            false,
        )?,
    }
    super::environment::enter_zone(ctx, connection.connection_id, entity.map_id, entity.zone_id);
    for (opcode, body) in items.iter().filter_map(codec::build_random_property_values) {
        super::send_raw(ctx, connection.connection_id, opcode, body, false);
    }
    for item in &items {
        if let Some((bag_slot, position)) = codec::bag_content_parts(item.slot) {
            if let Some(bag) = items.iter().find(|bag| bag.slot == bag_slot) {
                let (opcode, body) =
                    codec::build_container_slot_values(bag.guid, position, item.guid);
                super::send_raw(ctx, connection.connection_id, opcode, body, false);
            }
        }
    }
    ctx.db
        .game_native_world_connection()
        .connection_id()
        .update(NativeWorldConnection {
            state: NativeWorldState::InWorld(NativeActive {
                account_id,
                character_guid: guid,
            }),
            ..connection
        });
    super::admission::entered(ctx, connection.connection_id);
    ctx.db.game_native_player().insert(NativePlayer {
        connection_id: connection.connection_id,
        character_guid: guid,
        account_id,
        open_loot: 0,
        gossip_menu: None,
        gossip_root_menu_id: 0,
        teleport: None,
        teleport_counter,
        zone_id: entity.zone_id,
    });
    for grant in grants {
        super::visibility::create(ctx, connection.connection_id, &grant)?;
    }
    if carrier.is_none() {
        crate::transport::rejoin(ctx, guid);
    }
    super::economy::replay_buyback(ctx, connection.connection_id, guid)?;
    super::progression::replay_exploration(ctx, connection.connection_id, guid);
    super::combat::replay_combo_points(ctx, connection.connection_id, guid);
    super::death::replay(ctx, guid);
    super::instance::replay(ctx, guid);
    super::petition::replay(ctx, guid, connection.connection_id);
    super::chat::replay_system_messages(ctx, guid);
    super::party::roster(ctx, guid, entry == crate::world::WorldEntry::Login);
    if entry == crate::world::WorldEntry::Login {
        super::social::presence(ctx, guid, true);
        super::guild::presence(ctx, guid, true);
    }
    if let Some(sequence) = cinematic {
        let (opcode, body) = codec::build_trigger_cinematic(sequence);
        super::send_raw(ctx, connection.connection_id, opcode, body, false);
    }
    Ok(())
}

pub(super) fn leave(ctx: &ReducerContext, connection_id: ConnectionId) {
    let players = ctx.db.game_native_player();
    if let Some(player) = players.connection_id().find(connection_id) {
        super::clear_audience(ctx, connection_id);
        super::member_stats::stop(ctx, connection_id);
        players.connection_id().delete(connection_id);
        crate::meeting_stone::remove_solo_seeker(ctx, player.character_guid);
        crate::channel::leave_all(ctx, player.character_guid);
        super::social::presence(ctx, player.character_guid, false);
        super::guild::presence(ctx, player.character_guid, false);
        if let Some(entity) = ctx
            .db
            .game_world_entity()
            .guid()
            .find(player.character_guid)
        {
            crate::world::remove_live_character(ctx, entity);
        } else {
            crate::world::remove_loading_character(ctx, player.character_guid);
        }
        super::party::roster(ctx, player.character_guid, true);
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "World opcode dispatch stays in one match"
)]
pub(super) fn receive(
    ctx: &ReducerContext,
    mut connection: NativeWorldConnection,
    guid: u64,
    opcode: u32,
    body: &[u8],
) -> Result<(), Rollback> {
    let player = ctx
        .db
        .game_native_player()
        .connection_id()
        .find(connection.connection_id)
        .filter(|player| player.character_guid == guid)
        .ok_or("native World Session is no longer current")?;
    if matches!(opcode, 0x00c7 | 0x00dc) {
        return Ok(super::teleport::acknowledge(
            ctx, connection, player, opcode, body,
        )?);
    }
    if player.teleport.is_some() && !matches!(opcode, 0x01dc | 0x004a | 0x004b) {
        return Ok(());
    }
    if opcode == codec::addon::CMSG_MESSAGECHAT_OPCODE {
        if let Some((chat_type, text)) = codec::addon::parse_addon_client_chat(body) {
            if codec::addon::is_bridge_prefixed(&text) {
                return super::addon::receive(ctx, connection, guid, &text);
            }
            if !codec::addon::typed_addon_chat_type(chat_type) {
                return Ok(());
            }
        }
    }
    if lyracore_shared::opcodes::movement::is_slice_move(opcode) {
        let movement = codec::ClientMovement::decode(body).map_err(|error| error.to_string())?;
        let entity = ctx
            .db
            .game_world_entity()
            .guid()
            .find(guid)
            .ok_or("Character is not in the world")?;
        return Ok(crate::world::apply_movement_update(
            ctx,
            entity,
            opcode as u16,
            &movement,
            body.to_vec(),
        )?);
    }
    if opcode == codec::CMSG_AUCTION_LIST_ITEMS_OPCODE {
        return super::auction::browse(ctx, connection.connection_id, guid, body);
    }
    if matches!(opcode, 0x0125 | 0x0318) {
        return super::reputation::receive(ctx, guid, opcode, body);
    }
    let message =
        lyracore_protocol::world::decode(opcode, body).map_err(|error| error.to_string())?;
    if matches!(
        &message,
        ClientOpcodeMessage::CMSG_FORCE_MOVE_ROOT_ACK(_)
            | ClientOpcodeMessage::CMSG_FORCE_MOVE_UNROOT_ACK(_)
            | ClientOpcodeMessage::CMSG_MOVE_WATER_WALK_ACK(_)
    ) {
        return movement_receipt(ctx, guid, &message);
    }
    if super::unavailable::is_control_receipt(&message) {
        return Ok(());
    }
    let previous_notices = connection.unavailable_notices;
    if let Some(packets) =
        super::unavailable::responses(&message, &mut connection.unavailable_notices)
    {
        for packet in packets {
            super::send_raw(
                ctx,
                connection.connection_id,
                packet.opcode,
                packet.body,
                false,
            );
        }
        if previous_notices != connection.unavailable_notices {
            ctx.db
                .game_native_world_connection()
                .connection_id()
                .update(connection);
        }
        return Ok(());
    }
    let response = match message {
        message @ (ClientOpcodeMessage::MSG_AUCTION_HELLO(_)
        | ClientOpcodeMessage::CMSG_AUCTION_LIST_OWNER_ITEMS(_)
        | ClientOpcodeMessage::CMSG_AUCTION_LIST_BIDDER_ITEMS(_)
        | ClientOpcodeMessage::CMSG_AUCTION_SELL_ITEM(_)
        | ClientOpcodeMessage::CMSG_AUCTION_PLACE_BID(_)
        | ClientOpcodeMessage::CMSG_AUCTION_REMOVE_ITEM(_)) => {
            return super::auction::receive(ctx, connection.connection_id, guid, message);
        }

        message @ (ClientOpcodeMessage::CMSG_GET_MAIL_LIST(_)
        | ClientOpcodeMessage::MSG_QUERY_NEXT_MAIL_TIME
        | ClientOpcodeMessage::CMSG_ITEM_TEXT_QUERY(_)
        | ClientOpcodeMessage::CMSG_MAIL_MARK_AS_READ(_)
        | ClientOpcodeMessage::CMSG_MAIL_DELETE(_)
        | ClientOpcodeMessage::CMSG_MAIL_RETURN_TO_SENDER(_)
        | ClientOpcodeMessage::CMSG_MAIL_TAKE_MONEY(_)
        | ClientOpcodeMessage::CMSG_MAIL_TAKE_ITEM(_)
        | ClientOpcodeMessage::CMSG_SEND_MAIL(_)
        | ClientOpcodeMessage::CMSG_MAIL_CREATE_TEXT_ITEM(_)) => {
            return super::mail::receive(ctx, connection.connection_id, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_MEETINGSTONE_JOIN(_)
        | ClientOpcodeMessage::CMSG_MEETINGSTONE_LEAVE
        | ClientOpcodeMessage::CMSG_MEETINGSTONE_INFO
        | ClientOpcodeMessage::MSG_LOOKING_FOR_GROUP) => {
            return super::meeting_stone::receive(ctx, connection.connection_id, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_DUEL_ACCEPTED(_)
        | ClientOpcodeMessage::CMSG_DUEL_CANCELLED(_)) => {
            return super::duel::receive(ctx, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_INITIATE_TRADE(_)
        | ClientOpcodeMessage::CMSG_BEGIN_TRADE
        | ClientOpcodeMessage::CMSG_CANCEL_TRADE
        | ClientOpcodeMessage::CMSG_SET_TRADE_ITEM(_)
        | ClientOpcodeMessage::CMSG_CLEAR_TRADE_ITEM(_)
        | ClientOpcodeMessage::CMSG_SET_TRADE_GOLD(_)
        | ClientOpcodeMessage::CMSG_ACCEPT_TRADE(_)
        | ClientOpcodeMessage::CMSG_UNACCEPT_TRADE
        | ClientOpcodeMessage::CMSG_BUSY_TRADE
        | ClientOpcodeMessage::CMSG_IGNORE_TRADE) => {
            return super::trade::receive(ctx, guid, message);
        }
        message @ (ClientOpcodeMessage::MSG_TABARDVENDOR_ACTIVATE(_)
        | ClientOpcodeMessage::MSG_SAVE_GUILD_EMBLEM(_)
        | ClientOpcodeMessage::CMSG_PETITION_SHOWLIST(_)
        | ClientOpcodeMessage::CMSG_PETITION_BUY(_)
        | ClientOpcodeMessage::CMSG_PETITION_QUERY(_)
        | ClientOpcodeMessage::CMSG_PETITION_SHOW_SIGNATURES(_)
        | ClientOpcodeMessage::MSG_PETITION_RENAME(_)
        | ClientOpcodeMessage::CMSG_OFFER_PETITION(_)
        | ClientOpcodeMessage::CMSG_PETITION_SIGN(_)
        | ClientOpcodeMessage::MSG_PETITION_DECLINE(_)
        | ClientOpcodeMessage::CMSG_TURN_IN_PETITION(_)) => {
            return super::petition::receive(ctx, connection, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_GUILD_QUERY(_)
        | ClientOpcodeMessage::CMSG_GUILD_ROSTER
        | ClientOpcodeMessage::CMSG_GUILD_INFO
        | ClientOpcodeMessage::CMSG_GUILD_CREATE(_)
        | ClientOpcodeMessage::CMSG_GUILD_INVITE(_)
        | ClientOpcodeMessage::CMSG_GUILD_ACCEPT
        | ClientOpcodeMessage::CMSG_GUILD_DECLINE
        | ClientOpcodeMessage::CMSG_GUILD_LEAVE
        | ClientOpcodeMessage::CMSG_GUILD_REMOVE(_)
        | ClientOpcodeMessage::CMSG_GUILD_PROMOTE(_)
        | ClientOpcodeMessage::CMSG_GUILD_DEMOTE(_)
        | ClientOpcodeMessage::CMSG_GUILD_LEADER(_)
        | ClientOpcodeMessage::CMSG_GUILD_DISBAND
        | ClientOpcodeMessage::CMSG_GUILD_MOTD(_)
        | ClientOpcodeMessage::CMSG_GUILD_INFO_TEXT(_)
        | ClientOpcodeMessage::CMSG_GUILD_SET_PUBLIC_NOTE(_)
        | ClientOpcodeMessage::CMSG_GUILD_SET_OFFICER_NOTE(_)
        | ClientOpcodeMessage::CMSG_GUILD_RANK(_)
        | ClientOpcodeMessage::CMSG_GUILD_ADD_RANK(_)
        | ClientOpcodeMessage::CMSG_GUILD_DEL_RANK) => {
            return super::guild::receive(ctx, connection, guid, message);
        }
        ClientOpcodeMessage::CMSG_REQUEST_PARTY_MEMBER_STATS(request) => {
            return super::member_stats::request(
                ctx,
                connection.connection_id,
                guid,
                request.guid.guid(),
            );
        }
        message @ (ClientOpcodeMessage::CMSG_GROUP_INVITE(_)
        | ClientOpcodeMessage::CMSG_GROUP_ACCEPT
        | ClientOpcodeMessage::CMSG_GROUP_DECLINE
        | ClientOpcodeMessage::CMSG_GROUP_DISBAND
        | ClientOpcodeMessage::CMSG_GROUP_UNINVITE(_)
        | ClientOpcodeMessage::CMSG_GROUP_UNINVITE_GUID(_)
        | ClientOpcodeMessage::CMSG_LOOT_METHOD(_)
        | ClientOpcodeMessage::CMSG_GROUP_RAID_CONVERT
        | ClientOpcodeMessage::CMSG_GROUP_SET_LEADER(_)
        | ClientOpcodeMessage::CMSG_GROUP_ASSISTANT_LEADER(_)
        | ClientOpcodeMessage::CMSG_GROUP_CHANGE_SUB_GROUP(_)
        | ClientOpcodeMessage::CMSG_GROUP_SWAP_SUB_GROUP(_)
        | ClientOpcodeMessage::MSG_RAID_READY_CHECK(_)
        | ClientOpcodeMessage::MSG_RAID_TARGET_UPDATE(_)
        | ClientOpcodeMessage::MSG_MINIMAP_PING(_)
        | ClientOpcodeMessage::MSG_RANDOM_ROLL(_)) => {
            return super::party::receive(ctx, connection, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_WHO(_)
        | ClientOpcodeMessage::CMSG_FRIEND_LIST
        | ClientOpcodeMessage::CMSG_ADD_FRIEND(_)
        | ClientOpcodeMessage::CMSG_DEL_FRIEND(_)
        | ClientOpcodeMessage::CMSG_ADD_IGNORE(_)
        | ClientOpcodeMessage::CMSG_DEL_IGNORE(_)) => {
            return super::social::receive(ctx, connection, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_JOIN_CHANNEL(_)
        | ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_LIST(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_PASSWORD(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_SET_OWNER(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_OWNER(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_MODERATOR(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_UNMODERATOR(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_MUTE(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_UNMUTE(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_INVITE(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_KICK(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_BAN(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_UNBAN(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_ANNOUNCEMENTS(_)
        | ClientOpcodeMessage::CMSG_CHANNEL_MODERATE(_)) => {
            return super::channel::receive(ctx, connection.connection_id, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_MESSAGECHAT(_)
        | ClientOpcodeMessage::CMSG_TEXT_EMOTE(_)
        | ClientOpcodeMessage::CMSG_EMOTE(_)
        | ClientOpcodeMessage::CMSG_CHAT_IGNORED(_)) => {
            return super::chat::receive(ctx, connection, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_REPOP_REQUEST
        | ClientOpcodeMessage::MSG_CORPSE_QUERY
        | ClientOpcodeMessage::CMSG_RECLAIM_CORPSE(_)
        | ClientOpcodeMessage::CMSG_RESURRECT_RESPONSE(_)
        | ClientOpcodeMessage::CMSG_SELF_RES
        | ClientOpcodeMessage::CMSG_SPIRIT_HEALER_ACTIVATE(_)) => {
            return super::death::receive(ctx, connection.connection_id, guid, message);
        }

        message @ (ClientOpcodeMessage::CMSG_GOSSIP_HELLO(_)
        | ClientOpcodeMessage::CMSG_GOSSIP_SELECT_OPTION(_)
        | ClientOpcodeMessage::CMSG_BINDER_ACTIVATE(_)
        | ClientOpcodeMessage::MSG_TALENT_WIPE_CONFIRM(_)) => {
            return super::gossip::receive(ctx, connection.connection_id, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_TAXINODE_STATUS_QUERY(_)
        | ClientOpcodeMessage::CMSG_TAXIQUERYAVAILABLENODES(_)
        | ClientOpcodeMessage::CMSG_ACTIVATETAXI(_)) => {
            return super::taxi::receive(ctx, connection.connection_id, guid, message);
        }
        ClientOpcodeMessage::CMSG_AREATRIGGER(request) => {
            super::actor(ctx, guid)?;
            return Ok(crate::quest::request_enter_areatrigger(
                ctx,
                guid,
                request.trigger_id,
            )?);
        }
        message @ (ClientOpcodeMessage::CMSG_LOOT(_)
        | ClientOpcodeMessage::CMSG_LOOT_MONEY
        | ClientOpcodeMessage::CMSG_LOOT_RELEASE(_)
        | ClientOpcodeMessage::CMSG_AUTOSTORE_LOOT_ITEM(_)
        | ClientOpcodeMessage::CMSG_GAMEOBJ_USE(_)
        | ClientOpcodeMessage::CMSG_LOOT_ROLL(_)
        | ClientOpcodeMessage::CMSG_LOOT_MASTER_GIVE(_)) => {
            return super::loot::receive(ctx, connection.connection_id, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_TRAINER_LIST(_)
        | ClientOpcodeMessage::CMSG_TRAINER_BUY_SPELL(_)
        | ClientOpcodeMessage::CMSG_LEARN_TALENT(_)
        | ClientOpcodeMessage::CMSG_SET_ACTION_BUTTON(_)) => {
            return super::trainer::receive(ctx, connection.connection_id, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_LIST_INVENTORY(_)
        | ClientOpcodeMessage::CMSG_BUY_ITEM(_)
        | ClientOpcodeMessage::CMSG_SELL_ITEM(_)
        | ClientOpcodeMessage::CMSG_BUYBACK_ITEM(_)
        | ClientOpcodeMessage::CMSG_REPAIR_ITEM(_)
        | ClientOpcodeMessage::CMSG_BANKER_ACTIVATE(_)
        | ClientOpcodeMessage::CMSG_BUY_BANK_SLOT(_)
        | ClientOpcodeMessage::CMSG_AUTOBANK_ITEM(_)
        | ClientOpcodeMessage::CMSG_AUTOSTORE_BANK_ITEM(_)) => {
            return super::economy::receive(ctx, connection.connection_id, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_DESTROYITEM(_)
        | ClientOpcodeMessage::CMSG_SPLIT_ITEM(_)
        | ClientOpcodeMessage::CMSG_AUTOEQUIP_ITEM(_)
        | ClientOpcodeMessage::CMSG_AUTOSTORE_BAG_ITEM(_)
        | ClientOpcodeMessage::CMSG_SWAP_INV_ITEM(_)
        | ClientOpcodeMessage::CMSG_SWAP_ITEM(_)
        | ClientOpcodeMessage::CMSG_USE_ITEM(_)
        | ClientOpcodeMessage::CMSG_READ_ITEM(_)
        | ClientOpcodeMessage::CMSG_ITEM_NAME_QUERY(_)) => {
            return super::inventory::receive(ctx, connection.connection_id, guid, message);
        }
        message @ (ClientOpcodeMessage::CMSG_QUESTGIVER_STATUS_QUERY(_)
        | ClientOpcodeMessage::CMSG_QUESTGIVER_HELLO(_)
        | ClientOpcodeMessage::CMSG_QUESTGIVER_QUERY_QUEST(_)
        | ClientOpcodeMessage::CMSG_QUEST_QUERY(_)
        | ClientOpcodeMessage::CMSG_QUESTGIVER_ACCEPT_QUEST(_)
        | ClientOpcodeMessage::CMSG_QUESTGIVER_CANCEL
        | ClientOpcodeMessage::CMSG_QUESTGIVER_COMPLETE_QUEST(_)
        | ClientOpcodeMessage::CMSG_QUESTGIVER_REQUEST_REWARD(_)
        | ClientOpcodeMessage::CMSG_QUESTGIVER_CHOOSE_REWARD(_)
        | ClientOpcodeMessage::CMSG_QUESTLOG_REMOVE_QUEST(_)
        | ClientOpcodeMessage::CMSG_PUSHQUESTTOPARTY(_)) => {
            return super::quest::receive(ctx, connection.connection_id, guid, message);
        }
        ClientOpcodeMessage::CMSG_CAST_SPELL(request) => {
            return super::cast::receive(ctx, connection.connection_id, guid, &request)
        }
        ClientOpcodeMessage::CMSG_CANCEL_AUTO_REPEAT_SPELL => {
            super::cast::cancel_repeat(ctx, guid);
            return Ok(());
        }
        ClientOpcodeMessage::CMSG_CANCEL_CAST(_)
        | ClientOpcodeMessage::CMSG_CANCEL_CHANNELLING(_) => {
            return Ok(crate::spell::do_cancel_cast(ctx, super::actor(ctx, guid)?)?);
        }
        ClientOpcodeMessage::CMSG_CANCEL_AURA(request) => {
            super::actor(ctx, guid)?;
            return Ok(crate::spell::do_cancel_aura(ctx, guid, request.id)?);
        }
        ClientOpcodeMessage::CMSG_SET_ACTIVE_MOVER(request) => {
            if request.guid.guid() == guid {
                return Ok(());
            }
            ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(codec::build_gm_system_message(
                "Control of another mover is not available on this realm.".into(),
            )))
        }
        ClientOpcodeMessage::CMSG_CREATURE_QUERY(request) => {
            let Some(response) = super::query::creature(ctx, request.creature) else {
                return Ok(());
            };
            response
        }
        ClientOpcodeMessage::CMSG_GAMEOBJECT_QUERY(request) => {
            super::query::gameobject(ctx, request.entry_id)
        }
        ClientOpcodeMessage::CMSG_ITEM_QUERY_SINGLE(request) => {
            super::query::item(ctx, request.item)
        }
        ClientOpcodeMessage::CMSG_NPC_TEXT_QUERY(request) => {
            super::query::npc_text(ctx, request.text_id)
        }
        ClientOpcodeMessage::CMSG_PET_NAME_QUERY(request) => {
            let Some(response) =
                super::query::pet_name(ctx, request.guid.guid(), request.pet_number)
            else {
                return Ok(());
            };
            response
        }
        ClientOpcodeMessage::CMSG_INSPECT(request) => {
            let entity = crate::helpers::live_entity(ctx, guid)?;
            crate::world::apply_inspect(ctx, entity, request.guid.guid())?;
            ServerOpcodeMessage::SMSG_INSPECT(codec::build_inspect_response(request.guid.guid()))
        }
        ClientOpcodeMessage::CMSG_ATTACKSWING(request) => {
            return super::combat::attack(ctx, guid, request.guid.guid())
        }
        ClientOpcodeMessage::CMSG_ATTACKSTOP => return super::combat::stop(ctx, guid),
        ClientOpcodeMessage::CMSG_SETSHEATHED(request) => {
            let entity = crate::helpers::live_entity(ctx, guid)?;
            return Ok(crate::world::apply_set_sheathed(
                ctx,
                entity,
                request.sheathed.as_int(),
            )?);
        }
        ClientOpcodeMessage::CMSG_PET_RENAME(request) => {
            return super::pet::rename(ctx, guid, request.pet.guid(), &request.name);
        }
        ClientOpcodeMessage::CMSG_PET_ACTION(request) => {
            return Ok(crate::creatures::apply_pet_command(
                ctx,
                guid,
                request.data,
                request.target.guid(),
            )?);
        }
        ClientOpcodeMessage::CMSG_FORCE_RUN_SPEED_CHANGE_ACK(_)
        | ClientOpcodeMessage::CMSG_CANCEL_GROWTH_AURA
        | ClientOpcodeMessage::CMSG_REQUEST_PET_INFO => return Ok(()),
        ClientOpcodeMessage::CMSG_PING(ping) => {
            ServerOpcodeMessage::SMSG_PONG(lyracore_protocol::messages::vanilla::SMSG_PONG {
                sequence_id: ping.sequence_id,
            })
        }
        ClientOpcodeMessage::CMSG_LOGOUT_REQUEST | ClientOpcodeMessage::CMSG_PLAYER_LOGOUT => {
            let entity = ctx.db.game_world_entity().guid().find(guid);
            if entity.is_some_and(|entity| {
                entity.combat_until_ms
                    > crate::time::now(ctx).to_micros_since_unix_epoch().max(0) as u64 / 1_000
            }) {
                ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(codec::logout_denied_in_combat())
            } else {
                leave(ctx, connection.connection_id);
                let NativeWorldState::InWorld(NativeActive { account_id, .. }) = connection.state
                else {
                    return Err("invalid world state".into());
                };
                ctx.db
                    .game_native_world_connection()
                    .connection_id()
                    .update(NativeWorldConnection {
                        state: NativeWorldState::CharacterSelection { account_id },
                        ..connection
                    });
                for message in codec::logout_sequence() {
                    super::send(ctx, connection.connection_id, message, false)?;
                }
                return Ok(());
            }
        }
        ClientOpcodeMessage::CMSG_NAME_QUERY(query) => {
            let Some(character) = crate::helpers::character_by_guid(ctx, query.guid.guid()) else {
                return Ok(());
            };
            ServerOpcodeMessage::SMSG_NAME_QUERY_RESPONSE(Box::new(
                codec::build_name_query_response(&super::view::character(ctx, character))
                    .map_err(|error| error.to_string())?,
            ))
        }
        ClientOpcodeMessage::CMSG_QUERY_TIME => ServerOpcodeMessage::SMSG_QUERY_TIME_RESPONSE(
            lyracore_protocol::messages::vanilla::SMSG_QUERY_TIME_RESPONSE {
                time: (crate::time::now(ctx).to_micros_since_unix_epoch() / 1_000_000) as u32,
            },
        ),
        ClientOpcodeMessage::CMSG_PLAYED_TIME => {
            let character =
                crate::helpers::character_by_guid(ctx, guid).ok_or("Character no longer exists")?;
            ServerOpcodeMessage::SMSG_PLAYED_TIME(codec::build_played_time(
                character.played_total_secs,
                character.session_start_micros,
                crate::time::now(ctx).to_micros_since_unix_epoch().max(0) as u64,
            ))
        }
        ClientOpcodeMessage::CMSG_LOGOUT_CANCEL => ServerOpcodeMessage::SMSG_LOGOUT_CANCEL_ACK,
        ClientOpcodeMessage::CMSG_STANDSTATECHANGE(request) => {
            let entity = crate::helpers::live_entity(ctx, guid)?;
            let previous = (entity.unit_bytes_1 & 0xff) as u8;
            crate::world::apply_set_stand_state(ctx, entity, request.animation_state.as_int())
                .map_err(|cause| {
                    let state =
                        lyracore_protocol::messages::vanilla::UnitStandState::try_from(previous)
                            .unwrap_or_default();
                    Rollback::reply(
                        ServerOpcodeMessage::SMSG_STANDSTATE_UPDATE(
                            codec::build_stand_state_update(state),
                        ),
                        cause,
                    )
                })?;
            return Ok(());
        }
        ClientOpcodeMessage::CMSG_SET_SELECTION(selection) => {
            crate::world::apply_set_target(
                ctx,
                crate::helpers::live_entity(ctx, guid)?,
                selection.target.guid(),
            )?;
            return Ok(());
        }
        ClientOpcodeMessage::CMSG_REQUEST_ACCOUNT_DATA(request) => {
            if let Some((opcode, body)) = codec::build_empty_account_data(request.data_type) {
                super::send_raw(ctx, connection.connection_id, opcode, body, false);
            }
            return Ok(());
        }
        _ => return Ok(()),
    };
    Ok(super::send(ctx, connection.connection_id, response, false)?)
}

fn movement_receipt(
    ctx: &ReducerContext,
    guid: u64,
    message: &ClientOpcodeMessage,
) -> Result<(), Rollback> {
    let Some((mover, opcode, movement)) = codec::acknowledged_move(message) else {
        return Ok(());
    };
    if mover != guid {
        return Ok(());
    }
    let entity = crate::helpers::live_entity(ctx, guid)?;
    if entity.dead && entity.player_flags & lyracore_shared::constants::player_flags::GHOST == 0 {
        let mut info = movement.info().clone();
        info.position.x = entity.x;
        info.position.y = entity.y;
        info.position.z = entity.z;
        info.orientation = entity.orientation;
        let bytes = codec::movement_info_to_bytes(&info).map_err(|error| error.to_string())?;
        if let Some((opcode, body)) = codec::build_movement_relay_raw(opcode, guid, &bytes) {
            super::visibility::broadcast(ctx, guid, opcode, body, true);
        }
        return Ok(());
    }
    let bytes =
        codec::movement_info_to_bytes(movement.info()).map_err(|error| error.to_string())?;
    Ok(crate::world::apply_movement_update(
        ctx,
        entity,
        opcode as u16,
        &movement,
        bytes,
    )?)
}
