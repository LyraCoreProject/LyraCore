use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        CMSG_MESSAGECHAT_ChatType, PartyOperation, PartyResult, SMSG_CHAT_PLAYER_NOT_FOUND,
        SMSG_NOTIFICATION,
    },
};
use lyracore_shared::chat::{broadcast_chat, chat_kind, chat_tag_for, language, ChatRefusal};
use spacetimedb::{ConnectionId, ReducerContext, Table};

use super::{game_native_player, NativeWorldConnection, Rollback};
use crate::{
    aoi::game_aoi_viewer,
    away::game_character_away,
    chat::{
        game_character_contact, game_system_message_event, ChatEvent, EmoteEvent,
        SystemMessageEvent,
    },
    game_creature_template, game_world_entity,
    realm_chat::{
        RealmChatLine, RealmChatRequest, SpeakerFacts, WhisperRequest, WhisperTargetFacts,
    },
    WorldEntity,
};

pub(super) fn kind(chat: &CMSG_MESSAGECHAT_ChatType) -> Option<u8> {
    Some(match chat {
        CMSG_MESSAGECHAT_ChatType::Say => chat_kind::SAY,
        CMSG_MESSAGECHAT_ChatType::Yell => chat_kind::YELL,
        CMSG_MESSAGECHAT_ChatType::Emote => chat_kind::EMOTE,
        CMSG_MESSAGECHAT_ChatType::Party => chat_kind::PARTY,
        CMSG_MESSAGECHAT_ChatType::Raid => chat_kind::RAID,
        CMSG_MESSAGECHAT_ChatType::RaidLeader => chat_kind::RAID_LEADER,
        CMSG_MESSAGECHAT_ChatType::RaidWarning => chat_kind::RAID_WARNING,
        CMSG_MESSAGECHAT_ChatType::Guild => chat_kind::GUILD,
        CMSG_MESSAGECHAT_ChatType::Officer => chat_kind::OFFICER,
        CMSG_MESSAGECHAT_ChatType::Channel { .. } => chat_kind::CHANNEL,
        CMSG_MESSAGECHAT_ChatType::Whisper { .. } => chat_kind::WHISPER,
        CMSG_MESSAGECHAT_ChatType::Afk => chat_kind::AFK,
        CMSG_MESSAGECHAT_ChatType::Dnd => chat_kind::DND,
        _ => return None,
    })
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: NativeWorldConnection,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    let actor = super::available_entity(ctx, guid)?;
    let id = connection.connection_id;
    if let Some(notice) = super::chat_flood::judge(ctx, connection, &actor, &message) {
        return Ok(super::send(ctx, id, notice, false)?);
    }
    let needs_ground_actor = match &message {
        ClientOpcodeMessage::CMSG_MESSAGECHAT(chat) => matches!(
            chat.chat_type,
            CMSG_MESSAGECHAT_ChatType::Say
                | CMSG_MESSAGECHAT_ChatType::Yell
                | CMSG_MESSAGECHAT_ChatType::Emote
                | CMSG_MESSAGECHAT_ChatType::Afk
                | CMSG_MESSAGECHAT_ChatType::Dnd
        ),
        ClientOpcodeMessage::CMSG_TEXT_EMOTE(_) => true,
        _ => false,
    };
    if needs_ground_actor && crate::taxi::reject_action_while_in_flight(ctx, guid).is_err() {
        return Ok(());
    }
    let speaker = SpeakerFacts {
        race: actor.race(),
        chat_tag: chat_tag_for(actor.player_flags),
    };
    match message {
        ClientOpcodeMessage::CMSG_MESSAGECHAT(chat) => {
            let Some(kind) = kind(&chat.chat_type) else {
                return Ok(());
            };
            if kind == chat_kind::SAY && chat.message.starts_with('.') {
                if super::guild::is_dot_command(&chat.message) {
                    return super::guild::dot_command(ctx, &actor, &chat.message);
                }
                return crate::gm::apply_gm_command(ctx, actor, chat.message).map_err(|cause| {
                    Rollback::reply(
                        ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                            codec::build_gm_system_message(cause.clone()),
                        )),
                        cause,
                    )
                });
            }
            if matches!(kind, chat_kind::AFK | chat_kind::DND) {
                return Ok(crate::away::apply_set_away(ctx, actor, kind, chat.message)?);
            }
            if let CMSG_MESSAGECHAT_ChatType::Whisper { target_player } = chat.chat_type {
                return whisper(
                    ctx,
                    id,
                    guid,
                    speaker,
                    chat.language.as_int(),
                    chat.message,
                    target_player,
                );
            }
            if matches!(kind, chat_kind::SAY | chat_kind::YELL | chat_kind::EMOTE) {
                if let Err(refusal) = lyracore_shared::chat::speakable_language(
                    kind,
                    speaker.race,
                    chat.language.as_int(),
                ) {
                    return refused(ctx, id, guid, kind, String::new(), refusal);
                }
                let broadcast = match kind {
                    chat_kind::YELL => broadcast_chat::YELL,
                    chat_kind::EMOTE => broadcast_chat::EMOTE,
                    _ => broadcast_chat::SAY,
                };
                // Speech Gates run before the event or any entity change.
                let _ = crate::chat::apply_send_chat(
                    ctx,
                    actor,
                    broadcast,
                    chat.language.as_int() as u8,
                    chat.message,
                );
                return Ok(());
            }
            let channel_name = match chat.chat_type {
                CMSG_MESSAGECHAT_ChatType::Channel { channel } => channel,
                _ => String::new(),
            };
            if let Err(refusal) = crate::realm_chat::apply_chat(
                ctx,
                guid,
                RealmChatRequest {
                    kind,
                    language: chat.language.as_int(),
                    channel_name: channel_name.clone(),
                    target_guid: 0,
                    message: chat.message,
                    speaker,
                },
            ) {
                return refused(ctx, id, guid, kind, channel_name, refusal);
            }
        }
        ClientOpcodeMessage::CMSG_TEXT_EMOTE(emote) => {
            if !actor.dead {
                crate::chat::apply_send_emote(
                    ctx,
                    actor,
                    emote.text_emote.as_int(),
                    emote.emote,
                    emote.target.guid(),
                )?;
            }
        }
        ClientOpcodeMessage::CMSG_CHAT_IGNORED(ignored) => {
            if let Some(character) = crate::helpers::character_by_guid(ctx, guid) {
                let _ = crate::realm_chat::apply_chat(
                    ctx,
                    guid,
                    RealmChatRequest {
                        kind: chat_kind::IGNORED,
                        language: language::UNIVERSAL,
                        channel_name: String::new(),
                        target_guid: ignored.guid.guid(),
                        message: character.name,
                        speaker,
                    },
                );
            }
        }
        ClientOpcodeMessage::CMSG_EMOTE(_) => {}
        _ => return Err("opcode is not a chat request".into()),
    }
    Ok(())
}

fn refused(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    kind: u8,
    channel: String,
    refusal: ChatRefusal,
) -> Result<(), Rollback> {
    let response = match refusal {
        ChatRefusal::UnknownLanguage => {
            ServerOpcodeMessage::SMSG_NOTIFICATION(Box::new(SMSG_NOTIFICATION {
                notification: "You don't know that language".into(),
            }))
        }
        ChatRefusal::WrongFaction => ServerOpcodeMessage::SMSG_CHAT_WRONG_FACTION,
        ChatRefusal::NotInGroup if kind == chat_kind::PARTY => {
            ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(Box::new(
                codec::build_party_command_result(
                    PartyOperation::Leave,
                    String::new(),
                    PartyResult::NotInGroup,
                ),
            ))
        }
        ChatRefusal::Channel(refusal) => {
            let (opcode, body) = codec::channel::notify(&codec::channel::ChannelNoticeView {
                notice: refusal.notice(),
                channel_name: channel,
                subject_guid: guid,
                ..Default::default()
            });
            super::send_raw(ctx, connection, opcode, body, false);
            return Ok(());
        }
        _ => return Ok(()),
    };
    Ok(super::send(ctx, connection, response, false)?)
}

pub(super) fn ignores(ctx: &ReducerContext, listener: u64, speaker: u64) -> bool {
    ctx.db
        .game_character_contact()
        .by_owner()
        .filter(listener)
        .any(|contact| contact.is_ignore && contact.target_guid == speaker)
}

fn whisper(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    speaker: SpeakerFacts,
    language: u32,
    message: String,
    target_name: String,
) -> Result<(), Rollback> {
    let target = crate::helpers::character_by_name(ctx, &target_name)
        .filter(|target| super::is_player(ctx, target.guid));
    let Some(target) = target else {
        return Ok(super::send(
            ctx,
            connection,
            ServerOpcodeMessage::SMSG_CHAT_PLAYER_NOT_FOUND(Box::new(SMSG_CHAT_PLAYER_NOT_FOUND {
                name: target_name,
            })),
            false,
        )?);
    };
    let away = ctx
        .db
        .game_character_away()
        .character_guid()
        .find(target.guid);
    let request = WhisperRequest {
        language,
        message,
        speaker,
        target: WhisperTargetFacts {
            guid: target.guid,
            race: target.race,
            name: target.name,
            ignores_speaker: ignores(ctx, target.guid, guid),
            away_kind: away.as_ref().map_or(0, |away| away.kind),
            away_message: away.map_or_else(String::new, |away| away.message),
        },
    };
    match crate::realm_chat::whisper_lines(guid, request) {
        Ok(lines) => {
            for line in lines {
                crate::realm_chat::emit(ctx, line);
            }
            Ok(())
        }
        Err(refusal) => refused(
            ctx,
            connection,
            guid,
            chat_kind::WHISPER,
            String::new(),
            refusal,
        ),
    }
}

fn packet(message: ServerOpcodeMessage) -> lyracore_protocol::world::Packet {
    lyracore_protocol::world::encode(message).expect("bounded chat fits a world frame")
}

fn deliver(ctx: &ReducerContext, guid: u64, packet: &lyracore_protocol::world::Packet) {
    if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
        super::send_raw(
            ctx,
            owner.connection_id,
            packet.opcode,
            packet.body.clone(),
            false,
        );
    }
}

pub(crate) fn realm_chat_line(ctx: &ReducerContext, line: &RealmChatLine) {
    let Some(message) = codec::build_realm_chat_line(
        line.kind,
        line.speaker_guid,
        line.language,
        line.chat_tag,
        line.channel_name.clone(),
        line.message.clone(),
    ) else {
        return;
    };
    let packet = packet(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(message)));
    for &recipient in &line.recipients {
        if !line.ignorable || !ignores(ctx, recipient, line.speaker_guid) {
            deliver(ctx, recipient, &packet);
        }
    }
}

pub(super) fn replay_system_messages(ctx: &ReducerContext, guid: u64) {
    let Some(character) = crate::helpers::character_by_guid(ctx, guid) else {
        return;
    };
    let mut pending = ctx
        .db
        .game_system_message_event()
        .by_recipient()
        .filter(character.owner_identity)
        .filter(|event| event.recipient_guid == guid)
        .collect::<Vec<_>>();
    pending.sort_unstable_by_key(|event| event.id);
    for event in pending {
        system_message(ctx, &event);
    }
}

pub(crate) fn system_message(ctx: &ReducerContext, event: &SystemMessageEvent) {
    deliver(
        ctx,
        event.recipient_guid,
        &packet(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
            codec::build_gm_system_message(event.message.clone()),
        ))),
    );
}

fn nearby(ctx: &ReducerContext, speaker: &WorldEntity, radius: f32) -> Vec<WorldEntity> {
    // A viewer's cell can lag its position by the AOI hysteresis margin.
    let (gx0, gx1, gy0, gy1) = lyracore_shared::spatial::covering_cell_box(
        speaker.x,
        speaker.y,
        radius + lyracore_shared::spatial::AOI_HYSTERESIS_YD,
    );
    let mut listeners = Vec::new();
    for gx in gx0..=gx1 {
        for gy in gy0..=gy1 {
            let cell = lyracore_shared::spatial::grid_cell_id(gx, gy);
            for viewer in ctx.db.game_aoi_viewer().by_cell().filter((
                speaker.map_id,
                speaker.instance_id,
                cell,
            )) {
                if let Some(listener) = ctx.db.game_world_entity().guid().find(viewer.guid) {
                    if in_range(speaker, &listener, radius) {
                        listeners.push(listener);
                    }
                }
            }
        }
    }
    listeners
}

fn in_range(speaker: &WorldEntity, listener: &WorldEntity, radius: f32) -> bool {
    let dx = speaker.x - listener.x;
    let dy = speaker.y - listener.y;
    crate::helpers::in_same_partition(listener, speaker.map_id, speaker.instance_id)
        && dx * dx + dy * dy <= radius * radius
}

pub(crate) fn speech(ctx: &ReducerContext, speaker: &WorldEntity, event: &ChatEvent) {
    let name = if speaker.is_player() {
        None
    } else {
        let Some(template) = ctx.db.game_creature_template().entry().find(speaker.entry) else {
            return;
        };
        Some(template.name)
    };
    let message = packet(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
        codec::build_chat_message_to(
            speaker.guid,
            name,
            event.target_guid,
            event.chat_type,
            event.language,
            event.message.clone(),
            if speaker.is_player() {
                chat_tag_for(speaker.player_flags)
            } else {
                0
            },
        ),
    )));
    if event.chat_type == broadcast_chat::CREATURE_WHISPER {
        if ctx
            .db
            .game_world_entity()
            .guid()
            .find(event.target_guid)
            .is_some_and(|target| {
                crate::helpers::in_same_partition(&target, speaker.map_id, speaker.instance_id)
            })
        {
            deliver(ctx, event.target_guid, &message);
        }
    } else if broadcast_chat::is_zone_wide(event.chat_type) {
        if speaker.zone_id == 0 {
            return;
        }
        for owner in ctx
            .db
            .game_native_player()
            .by_zone()
            .filter(speaker.zone_id)
        {
            if ctx
                .db
                .game_world_entity()
                .guid()
                .find(owner.character_guid)
                .is_some_and(|target| {
                    crate::helpers::in_same_partition(&target, speaker.map_id, speaker.instance_id)
                })
            {
                deliver(ctx, owner.character_guid, &message);
            }
        }
    } else {
        let radius = if matches!(
            event.chat_type,
            broadcast_chat::YELL | broadcast_chat::CREATURE_BOSS_EMOTE
        ) {
            300.0
        } else {
            25.0
        };
        for listener in nearby(ctx, speaker, radius) {
            if event.chat_type != broadcast_chat::EMOTE
                || lyracore_shared::faction::same_team(speaker.race(), listener.race())
            {
                deliver(ctx, listener.guid, &message);
            }
        }
    }
}

pub(crate) fn text_emote(ctx: &ReducerContext, speaker: &WorldEntity, event: &EmoteEvent) {
    let target_name = crate::helpers::character_by_guid(ctx, event.target_guid)
        .map(|c| c.name)
        .or_else(|| {
            ctx.db
                .game_world_entity()
                .guid()
                .find(event.target_guid)
                .and_then(|e| ctx.db.game_creature_template().entry().find(e.entry))
                .map(|c| c.name)
        });
    let text = codec::build_text_emote(
        speaker.guid,
        event.text_emote,
        event.emote_anim,
        target_name,
    )
    .map(|message| packet(ServerOpcodeMessage::SMSG_TEXT_EMOTE(Box::new(message))));
    let animation = codec::build_emote_anim(speaker.guid, event.emote_anim)
        .map(|message| packet(ServerOpcodeMessage::SMSG_EMOTE(Box::new(message))));
    for listener in nearby(ctx, speaker, broadcast_chat::TEXT_EMOTE_RANGE_YD) {
        if let Some(text) = &text {
            deliver(ctx, listener.guid, text);
        }
        if let Some(animation) = &animation {
            deliver(ctx, listener.guid, animation);
        }
    }
}

pub(crate) fn creature_sound(ctx: &ReducerContext, source_guid: u64, sound_id: u32, chat_type: u8) {
    if sound_id == 0 {
        return;
    }
    let packet = packet(ServerOpcodeMessage::SMSG_PLAY_SOUND(
        lyracore_protocol::messages::vanilla::SMSG_PLAY_SOUND { sound_id },
    ));
    if !broadcast_chat::is_zone_wide(chat_type) {
        return super::visibility::broadcast(ctx, source_guid, packet.opcode, packet.body, false);
    }
    let Some(source) = ctx.db.game_world_entity().guid().find(source_guid) else {
        return;
    };
    let zone = if matches!(source.map_id, 0 | 1) {
        let Some(zone) = crate::terrain::zone_id_at(ctx, source.map_id, source.x, source.y)
            .filter(|zone| *zone != 0)
        else {
            return;
        };
        Some(zone)
    } else {
        None
    };
    for owner in ctx.db.game_native_player().iter() {
        if zone.is_some_and(|zone| zone != owner.zone_id) {
            continue;
        }
        if ctx
            .db
            .game_world_entity()
            .guid()
            .find(owner.character_guid)
            .is_some_and(|target| {
                crate::helpers::in_same_partition(&target, source.map_id, source.instance_id)
            })
        {
            deliver(ctx, owner.character_guid, &packet);
        }
    }
}
