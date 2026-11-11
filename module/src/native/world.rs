use lyracore_protocol::codec;
use lyracore_protocol::messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use crate::action_bar::game_player_action;
use crate::aoi::game_aoi_grant;
use crate::character::CharacterDeparture;
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
        crate::character::WorldEntry::Login,
        0,
    )
}

pub(super) fn enter_character(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    account_id: u64,
    guid: u64,
    entry: crate::character::WorldEntry,
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
    if super::instance_shard::return_from_reaped(ctx, &connection, guid, entry, teleport_counter)? {
        return Ok(());
    }
    if !super::directory::owns_map(ctx, character.map_id) {
        return Err("World Shard does not own the Character map".into());
    }
    let cinematic = character
        .first_login
        .then(|| codec::opening_cinematic(character.race))
        .flatten();
    if let Some(previous) = ctx.db.game_native_player().account_id().find(account_id) {
        super::disconnect(ctx, previous.connection_id);
    }
    super::realm_party::restore(ctx, guid);
    crate::character::apply_player_login(ctx, &account, guid, owner, entry)?;
    super::instance_shard::arrived(ctx, guid, character.map_id, character.pending_instance_id);
    crate::aoi::enter_viewer(ctx, guid)?;
    crate::creatures::cycle::character_arrived(ctx, guid);
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
            crate::character::WorldEntry::Login => codec::WorldEntry::FreshLogin,
            crate::character::WorldEntry::WorldPort => codec::WorldEntry::WorldPort,
        },
        crate::time::now_secs(ctx),
    )
    .map_err(|error| error.to_string())?;
    for mut message in messages {
        if let ServerOpcodeMessage::SMSG_INITIAL_SPELLS(book) = &mut message {
            book.cooldowns = super::book::cooldowns(ctx, guid);
        }
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
    let mut self_create = codec::build_self_create_object(&entity, &items, &skills, &quests)
        .map_err(|error| error.to_string())?;
    codec::set_create_sheet_stats(&mut self_create, guid, &super::view::sheet_stats(&live))
        .map_err(|error| error.to_string())?;
    codec::set_create_freshness(&mut self_create, codec::CreateFreshness::Known);
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
            super::visibility::create(ctx, connection.connection_id, &grants.remove(index), false)?;
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
    replay_item_details(ctx, connection.connection_id, &items)?;
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
        teleport_started_micros: 0,
        teleport_origin: None,
    });
    for grant in grants {
        super::visibility::create(ctx, connection.connection_id, &grant, false)?;
    }
    if entry == crate::character::WorldEntry::Login {
        crate::taxi::resume_flight(ctx, guid)?;
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
    super::social::enter(ctx, account_id, guid, entry);
    super::party::enter(ctx, guid, entry);
    if entry == crate::character::WorldEntry::Login {
        super::guild::presence(ctx, guid, true);
    }
    if let Some(sequence) = cinematic {
        let (opcode, body) = codec::build_trigger_cinematic(sequence);
        super::send_raw(ctx, connection.connection_id, opcode, body, false);
    }
    Ok(())
}

fn replay_item_details(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    items: &[codec::ItemInstanceView],
) -> Result<(), String> {
    for (opcode, body) in items
        .iter()
        .filter_map(codec::build_enchantment_slot_values)
    {
        super::send_raw(ctx, connection_id, opcode, body, false);
    }
    for item in items
        .iter()
        .filter(|item| item.temporary_enchantment.enchantment != 0)
    {
        super::send(
            ctx,
            connection_id,
            ServerOpcodeMessage::SMSG_ITEM_ENCHANT_TIME_UPDATE(Box::new(
                codec::build_item_enchant_time_update(
                    item.guid,
                    item.owner_guid,
                    item.temporary_enchantment.duration_ms / 1000,
                ),
            )),
            false,
        )?;
    }
    for item in items {
        if let Some((bag_slot, position)) = codec::bag_content_parts(item.slot) {
            if let Some(bag) = items.iter().find(|bag| bag.slot == bag_slot) {
                let (opcode, body) =
                    codec::build_container_slot_values(bag.guid, position, item.guid);
                super::send_raw(ctx, connection_id, opcode, body, false);
            }
        }
    }
    Ok(())
}

pub(super) fn leave(
    ctx: &ReducerContext,
    connection_id: ConnectionId,
    departure: CharacterDeparture,
) {
    let players = ctx.db.game_native_player();
    if let Some(player) = players.connection_id().find(connection_id) {
        super::realm_party::presence(ctx, player.character_guid, false);
        super::clear_audience(ctx, connection_id);
        super::member_stats::stop(ctx, connection_id);
        super::teleport::restore_origin(ctx, player.character_guid, player.teleport_origin);
        players.connection_id().delete(connection_id);
        super::motion_permit::withdraw(ctx, connection_id);
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
            crate::character::remove_live_character(ctx, entity, departure);
        } else {
            crate::character::remove_loading_character(ctx, player.character_guid, departure);
        }
        super::party::roster(ctx, player.character_guid, true);
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "World opcode dispatch stays in one routing table"
)]
pub(super) fn receive(
    ctx: &ReducerContext,
    mut connection: NativeWorldConnection,
    guid: u64,
    relay: super::MotionRelay,
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
    let player = super::teleport::expire(ctx, player)?;
    if matches!(
        opcode,
        codec::MSG_MOVE_TELEPORT_ACK_OPCODE | codec::MSG_MOVE_WORLDPORT_ACK_OPCODE
    ) {
        return Ok(super::teleport::acknowledge(
            ctx, connection, player, opcode, body,
        )?);
    }
    if player.teleport.is_some()
        && !matches!(
            opcode,
            codec::CMSG_PING_OPCODE
                | codec::CMSG_PLAYER_LOGOUT_OPCODE
                | codec::CMSG_LOGOUT_REQUEST_OPCODE
        )
    {
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
        let relay_body = (relay == super::MotionRelay::Module).then(|| body.to_vec());
        crate::world::apply_movement_update(ctx, entity, opcode as u16, &movement, relay_body)?;
        super::motion_permit::refresh(ctx, connection.connection_id, guid);
        return Ok(());
    }
    if opcode == codec::CMSG_AUCTION_LIST_ITEMS_OPCODE {
        return super::auction::browse(ctx, connection.connection_id, guid, body);
    }
    if matches!(
        opcode,
        codec::CMSG_SET_FACTION_ATWAR_OPCODE | codec::CMSG_SET_WATCHED_FACTION_OPCODE
    ) {
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
    if super::unavailable::is_ignored(&message) {
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
    let id = connection.connection_id;
    if super::auction::claims(&message) {
        return super::auction::receive(ctx, id, guid, message);
    }
    if super::mail::claims(&message) {
        return super::mail::receive(ctx, id, guid, message);
    }
    if super::meeting_stone::claims(&message) {
        return super::meeting_stone::receive(ctx, id, guid, message);
    }
    if super::duel::claims(&message) {
        return super::duel::receive(ctx, guid, message);
    }
    if super::trade::claims(&message) {
        return super::trade::receive(ctx, guid, message);
    }
    if super::petition::claims(&message) {
        return super::petition::receive(ctx, connection, guid, message);
    }
    if super::guild::claims(&message) {
        return super::guild::receive(ctx, connection, guid, message);
    }
    if super::party::claims(&message) {
        return super::party::receive(ctx, connection, guid, message);
    }
    if super::social::claims(&message) {
        return super::social::receive(ctx, connection, guid, message);
    }
    if super::channel::claims(&message) {
        return super::channel::receive(ctx, id, guid, message);
    }
    if super::chat::claims(&message) {
        return super::chat::receive(ctx, connection, guid, message);
    }
    if super::death::claims(&message) {
        return super::death::receive(ctx, id, guid, message);
    }
    if super::gossip::claims(&message) {
        return super::gossip::receive(ctx, id, guid, message);
    }
    if super::taxi::claims(&message) {
        return super::taxi::receive(ctx, id, guid, message);
    }
    if super::loot::claims(&message) {
        return super::loot::receive(ctx, id, guid, message);
    }
    if super::trainer::claims(&message) {
        return super::trainer::receive(ctx, id, guid, message);
    }
    if super::economy::claims(&message) {
        return super::economy::receive(ctx, id, guid, message);
    }
    if super::inventory::claims(&message) {
        return super::inventory::receive(ctx, id, guid, message);
    }
    if super::quest::claims(&message) {
        return super::quest::receive(ctx, id, guid, message);
    }
    let response = match message {
        ClientOpcodeMessage::CMSG_RESET_INSTANCES => {
            return super::instance_reset::receive(ctx, connection.connection_id, guid);
        }
        ClientOpcodeMessage::CMSG_REQUEST_PARTY_MEMBER_STATS(request) => {
            return super::member_stats::request(
                ctx,
                connection.connection_id,
                guid,
                request.guid.guid(),
            );
        }
        ClientOpcodeMessage::CMSG_AREATRIGGER(request) => {
            super::actor(ctx, guid)?;
            return Ok(crate::areatrigger::request_enter_areatrigger(
                ctx,
                guid,
                request.trigger_id,
            )?);
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
            crate::inspect::apply_inspect(ctx, entity, request.guid.guid())?;
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
        ClientOpcodeMessage::CMSG_PING(ping) => {
            ServerOpcodeMessage::SMSG_PONG(lyracore_protocol::messages::vanilla::SMSG_PONG {
                sequence_id: ping.sequence_id,
            })
        }
        ClientOpcodeMessage::CMSG_LOGOUT_REQUEST | ClientOpcodeMessage::CMSG_PLAYER_LOGOUT => {
            let entity = ctx.db.game_world_entity().guid().find(guid);
            if entity.is_some_and(|entity| entity.combat_until_ms > crate::time::now_ms(ctx)) {
                ServerOpcodeMessage::SMSG_LOGOUT_RESPONSE(codec::logout_denied_in_combat())
            } else {
                leave(ctx, connection.connection_id, CharacterDeparture::Logout);
                let (connection_id, server_seed) =
                    (connection.connection_id, connection.server_seed);
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
                if crate::peer::realm_shard(ctx).is_none() {
                    for message in codec::logout_sequence() {
                        super::send(ctx, connection.connection_id, message, false)?;
                    }
                }
                super::realm_shard::return_to_selection(
                    ctx,
                    connection_id,
                    server_seed,
                    account_id,
                )?;
                return Ok(());
            }
        }
        ClientOpcodeMessage::CMSG_NAME_QUERY(query) => {
            let Some(response) = super::social::name_query(ctx, query.guid.guid())? else {
                return Ok(());
            };
            ServerOpcodeMessage::SMSG_NAME_QUERY_RESPONSE(Box::new(response))
        }
        ClientOpcodeMessage::CMSG_QUERY_TIME => ServerOpcodeMessage::SMSG_QUERY_TIME_RESPONSE(
            lyracore_protocol::messages::vanilla::SMSG_QUERY_TIME_RESPONSE {
                time: crate::time::now_secs(ctx) as u32,
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
            let mut entity = crate::helpers::live_entity(ctx, guid)?;
            entity.target_guid = selection.target.guid();
            entity.update(ctx);
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
    crate::world::apply_movement_update(ctx, entity, opcode as u16, &movement, Some(bytes))?;
    super::motion_permit::refresh_character(ctx, guid);
    Ok(())
}
