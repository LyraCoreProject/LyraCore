use lyracore_protocol::{
    codec::channel::{self as codec, ChannelNoticeView},
    messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
};
use lyracore_shared::{
    channel::{channel_op, notice, ChannelRefusal},
    chat::chat_tag_for,
    faction::team_for_race,
};
use spacetimedb::{ConnectionId, ReducerContext};

use super::game_native_player;
use crate::{
    channel::{ChannelRequest, ChatChannelNoticeEvent},
    realm_chat::SpeakerFacts,
};

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), String> {
    let actor = crate::helpers::acting_entity_by_guid(ctx, guid)
        .ok_or_else(|| super::rollback_silent("Character is not available".into()))?;
    let (op, channel_name, password, target_name) = match message {
        ClientOpcodeMessage::CMSG_JOIN_CHANNEL(c) => {
            (channel_op::JOIN, c.channel_name, c.channel_password, None)
        }
        ClientOpcodeMessage::CMSG_LEAVE_CHANNEL(c) => {
            (channel_op::LEAVE, c.channel_name, String::new(), None)
        }
        ClientOpcodeMessage::CMSG_CHANNEL_PASSWORD(c) => (
            channel_op::PASSWORD,
            c.channel_name,
            c.channel_password,
            None,
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_SET_OWNER(c) => (
            channel_op::SET_OWNER,
            c.channel_name,
            String::new(),
            Some(c.new_owner),
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_MODERATOR(c) => (
            channel_op::MODERATOR,
            c.channel_name,
            String::new(),
            Some(c.player_name),
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_UNMODERATOR(c) => (
            channel_op::UNMODERATOR,
            c.channel_name,
            String::new(),
            Some(c.player_name),
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_MUTE(c) => (
            channel_op::MUTE,
            c.channel_name,
            String::new(),
            Some(c.player_name),
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_UNMUTE(c) => (
            channel_op::UNMUTE,
            c.channel_name,
            String::new(),
            Some(c.player_name),
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_KICK(c) => (
            channel_op::KICK,
            c.channel_name,
            String::new(),
            Some(c.player_name),
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_BAN(c) => (
            channel_op::BAN,
            c.channel_name,
            String::new(),
            Some(c.player_name),
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_UNBAN(c) => (
            channel_op::UNBAN,
            c.channel_name,
            String::new(),
            Some(c.player_name),
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_INVITE(c) => (
            channel_op::INVITE,
            c.channel_name,
            String::new(),
            Some(c.player_name),
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_ANNOUNCEMENTS(c) => (
            channel_op::ANNOUNCEMENTS,
            c.channel_name,
            String::new(),
            None,
        ),
        ClientOpcodeMessage::CMSG_CHANNEL_MODERATE(c) => {
            (channel_op::MODERATE, c.channel_name, String::new(), None)
        }
        ClientOpcodeMessage::CMSG_CHANNEL_LIST(c) => {
            return roster(ctx, connection, &actor, c.channel_name, false)
        }
        ClientOpcodeMessage::CMSG_CHANNEL_OWNER(c) => {
            return roster(ctx, connection, &actor, c.channel_name, true)
        }
        _ => return Err("opcode is not a Chat Channel request".into()),
    };
    if matches!(op, channel_op::JOIN | channel_op::LEAVE) && channel_name.is_empty() {
        return Ok(());
    }
    let targeted = target_name.is_some();
    let target = target_name
        .as_deref()
        .and_then(|name| crate::helpers::character_by_name(ctx, name))
        .filter(|target| super::is_player(ctx, target.guid));
    let target_guid = target.as_ref().map_or(0, |target| target.guid);
    let target_race = target.as_ref().map_or(0, |target| target.race);
    let target_name = target.map_or_else(|| target_name.unwrap_or_default(), |target| target.name);
    let player = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(guid)
        .ok_or("native World Session is no longer current")?;
    let request = ChannelRequest {
        channel_name: channel_name.clone(),
        password,
        target_guid,
        target_race,
        target_name: target_name.clone(),
        target_ignores_actor: op == channel_op::INVITE
            && super::chat::ignores(ctx, target_guid, guid),
        speaker: SpeakerFacts {
            race: actor.race(),
            chat_tag: chat_tag_for(actor.player_flags),
        },
    };
    crate::channel::apply_op(ctx, guid, (player.account_id, 0), op, request).map_err(|cause| {
        match ChannelRefusal::parse_tag(&cause) {
            Some(refusal) => refusal_reply(
                refusal,
                channel_name,
                if targeted { target_guid } else { guid },
                target_name,
            ),
            None => super::rollback_silent(cause),
        }
    })
}

fn refusal_reply(
    refusal: ChannelRefusal,
    channel_name: String,
    subject_guid: u64,
    text: String,
) -> String {
    let (opcode, body) = codec::notify(&ChannelNoticeView {
        notice: refusal.notice(),
        channel_name,
        subject_guid,
        text,
        ..Default::default()
    });
    super::rollback_packets(
        vec![lyracore_protocol::world::Packet { opcode, body }],
        refusal.as_tag().into(),
    )
}

fn roster(
    ctx: &ReducerContext,
    connection: ConnectionId,
    actor: &crate::WorldEntity,
    channel_name: String,
    owner_only: bool,
) -> Result<(), String> {
    let (channel, _) =
        crate::channel::membership(ctx, team_for_race(actor.race()), &channel_name, actor.guid)
            .map_err(|refusal| {
                refusal_reply(refusal, channel_name.clone(), actor.guid, String::new())
            })?;
    if owner_only {
        let text = if channel.owner_guid == 0 {
            "Nobody".into()
        } else {
            crate::helpers::character_by_guid(ctx, channel.owner_guid)
                .map_or_else(|| "PLAYER_NOT_FOUND".into(), |owner| owner.name)
        };
        let (opcode, body) = codec::notify(&ChannelNoticeView {
            notice: notice::CHANNEL_OWNER,
            channel_name,
            text,
            ..Default::default()
        });
        super::send_raw(ctx, connection, opcode, body, false);
        Ok(())
    } else {
        let members = crate::channel::members_in_join_order(ctx, channel.channel_id)
            .into_iter()
            .map(|member| (member.character_guid, member.member_flags))
            .collect::<Vec<_>>();
        super::send(
            ctx,
            connection,
            ServerOpcodeMessage::SMSG_CHANNEL_LIST(Box::new(codec::build_channel_list(
                channel.name,
                channel.flags,
                &members,
            ))),
            false,
        )
    }
}

pub(crate) fn notice(ctx: &ReducerContext, event: &ChatChannelNoticeEvent) {
    let (opcode, body) = codec::notify(&ChannelNoticeView {
        notice: event.notice,
        channel_name: event.channel_name.clone(),
        subject_guid: event.subject_guid,
        actor_guid: event.actor_guid,
        old_flags: event.old_flags,
        new_flags: event.new_flags,
        channel_flags: event.channel_flags,
        text: event.text.clone(),
    });
    for &guid in &event.recipients {
        if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
            super::send_raw(ctx, owner.connection_id, opcode, body.clone(), false);
        }
    }
}
