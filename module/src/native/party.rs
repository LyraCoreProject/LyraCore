use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        MSG_RAID_TARGET_UPDATE_Client, PartyOperation, PartyResult,
    },
};
use lyracore_shared::group::{self, event_kind, GroupRefusal, RosterPayload};
use spacetimedb::{log, ConnectionId, ReducerContext, SpacetimeType};

use super::{game_native_player, game_native_world_connection, NativeWorldConnection, Rollback};
use crate::{
    game_world_entity,
    group::{GroupAction, GroupOpError, Plane},
};

#[derive(SpacetimeType, Default)]
pub struct BroadcastCooldowns {
    ready_until_micros: i64,
    ping_until_micros: i64,
    roll_until_micros: i64,
}

impl BroadcastCooldowns {
    fn admit(&mut self, now: i64, action: GroupAction) -> bool {
        let until = match action {
            GroupAction::ReadyCheckStart => &mut self.ready_until_micros,
            GroupAction::MinimapPing { .. } => &mut self.ping_until_micros,
            GroupAction::RandomRoll { .. } => &mut self.roll_until_micros,
            _ => return true,
        };
        if now < *until {
            return false;
        }
        *until = now + 1_000_000;
        true
    }
}

enum CommandReply {
    Silent,
    Invite(String),
    Leave(String),
    Convert,
}

impl CommandReply {
    fn packet(&self, result: PartyResult) -> Option<ServerOpcodeMessage> {
        let (operation, member) = match self {
            Self::Silent => return None,
            Self::Invite(name) => (PartyOperation::Invite, name.clone()),
            Self::Leave(name) if result != PartyResult::Success => {
                (PartyOperation::Leave, name.clone())
            }
            Self::Convert if result == PartyResult::Success => {
                (PartyOperation::Invite, String::new())
            }
            Self::Leave(_) | Self::Convert => return None,
        };
        Some(ServerOpcodeMessage::SMSG_PARTY_COMMAND_RESULT(Box::new(
            codec::build_party_command_result(operation, member, result),
        )))
    }

    fn refused(&self, error: GroupOpError) -> Rollback {
        use GroupRefusal as R;
        let result = match &error {
            GroupOpError::Refused(R::AlreadyInGroup) => PartyResult::AlreadyInGroup,
            GroupOpError::Refused(R::GroupFull) => PartyResult::GroupFull,
            GroupOpError::Refused(R::NotLeader) => PartyResult::NotLeader,
            GroupOpError::Refused(R::NotInGroup) => PartyResult::NotInGroup,
            GroupOpError::Refused(R::TargetNotInGroup) => PartyResult::TargetNotInGroup,
            GroupOpError::Refused(R::WrongFaction) => PartyResult::PlayerWrongFaction,
            GroupOpError::Refused(
                R::ActorUnavailable
                | R::InviteSelf
                | R::NoSuchPlayer
                | R::TargetOffline
                | R::NoPendingInvite
                | R::InviterUnavailable
                | R::KickSelf
                | R::InvalidLootRules
                | R::IntentAlreadyClaimed
                | R::ActionSuppressed
                | R::NotRaid
                | R::TargetIsSelf
                | R::InvalidSubgroup
                | R::SubgroupFull
                | R::InvalidTargetIcon,
            )
            | GroupOpError::Invariant(_) => PartyResult::BadPlayerName,
        };
        Rollback::messages(self.packet(result), error.to_string())
    }
}

pub(super) fn receive(
    ctx: &ReducerContext,
    mut connection: NativeWorldConnection,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    let actor = super::available_entity(ctx, guid)?;
    use ClientOpcodeMessage as C;
    use GroupAction as A;
    let (action, reply) = match message {
        C::CMSG_GROUP_INVITE(request) => {
            let reply = CommandReply::Invite(request.name.clone());
            let target = named_character(ctx, &request.name)
                .ok_or_else(|| reply.refused(GroupRefusal::NoSuchPlayer.into()))?;
            (A::Invite(target), reply)
        }
        C::CMSG_GROUP_ACCEPT => (
            A::Accept {
                class: actor.class(),
            },
            CommandReply::Silent,
        ),
        C::CMSG_GROUP_DECLINE => (A::Decline, CommandReply::Silent),
        C::CMSG_GROUP_DISBAND => (
            A::Leave {
                cause: group::leave_cause::LEFT,
            },
            CommandReply::Leave(String::new()),
        ),
        C::CMSG_GROUP_UNINVITE(request) => {
            let reply = CommandReply::Leave(request.name.clone());
            let target = named_character(ctx, &request.name)
                .ok_or_else(|| reply.refused(GroupRefusal::NoSuchPlayer.into()))?;
            (A::Uninvite(target), reply)
        }
        C::CMSG_GROUP_UNINVITE_GUID(request) => {
            if request.guid.guid() == guid {
                return Ok(());
            }
            (
                A::Uninvite(request.guid.guid()),
                CommandReply::Leave(String::new()),
            )
        }
        C::CMSG_LOOT_METHOD(request) => (
            A::LootMethod {
                method: request.loot_setting.as_int(),
                master: request.loot_master.guid(),
                threshold: request.loot_threshold.as_int(),
            },
            CommandReply::Silent,
        ),
        C::CMSG_GROUP_RAID_CONVERT => (A::RaidConvert, CommandReply::Convert),
        C::CMSG_GROUP_SET_LEADER(request) => {
            (A::SetLeader(request.guid.guid()), CommandReply::Silent)
        }
        C::CMSG_GROUP_ASSISTANT_LEADER(request) => (
            A::SetAssistant {
                target: request.guid.guid(),
                promote: request.set_assistant,
            },
            CommandReply::Silent,
        ),
        C::CMSG_GROUP_CHANGE_SUB_GROUP(request) => {
            let Some(target) = named_member(ctx, guid, &request.name) else {
                return Ok(());
            };
            (
                A::ChangeSubgroup {
                    target,
                    subgroup: request.group_number,
                },
                CommandReply::Silent,
            )
        }
        C::CMSG_GROUP_SWAP_SUB_GROUP(request) => {
            let Some(first) = named_member(ctx, guid, &request.name) else {
                return Ok(());
            };
            let Some(second) = named_member(ctx, guid, &request.swap_with_name) else {
                return Ok(());
            };
            (A::SwapSubgroup { first, second }, CommandReply::Silent)
        }
        C::MSG_RAID_READY_CHECK(request) => (
            match request.answer {
                Some(answer) => A::ReadyCheckAnswer(answer.state),
                None => A::ReadyCheckStart,
            },
            CommandReply::Silent,
        ),
        C::MSG_RAID_TARGET_UPDATE(request) => (target_icon(*request), CommandReply::Silent),
        C::MSG_MINIMAP_PING(request) => {
            if !request.position_x.is_finite() || !request.position_y.is_finite() {
                return Ok(());
            }
            (
                A::MinimapPing {
                    x_bits: u64::from(request.position_x.to_bits()),
                    y_bits: u64::from(request.position_y.to_bits()),
                },
                CommandReply::Silent,
            )
        }
        C::MSG_RANDOM_ROLL(request) => (
            A::RandomRoll {
                min: u64::from(request.minimum),
                max: u64::from(request.maximum),
            },
            CommandReply::Silent,
        ),
        _ => return Err("opcode is not a party or raid request".into()),
    };
    let limited = matches!(
        action,
        A::ReadyCheckStart | A::MinimapPing { .. } | A::RandomRoll { .. }
    );
    let id = connection.connection_id;
    if limited {
        if !connection
            .group_broadcasts
            .get_or_insert_default()
            .admit(crate::time::now(ctx).to_micros_since_unix_epoch(), action)
        {
            return Ok(());
        }
        ctx.db
            .game_native_world_connection()
            .connection_id()
            .update(connection);
    }
    if let Err(error) = crate::group::apply_action(ctx, Plane::Shard, guid, action) {
        if limited && matches!(error, GroupOpError::Refused(_)) {
            return Ok(());
        }
        return Err(reply.refused(error));
    }
    if let A::Invite(target) = action {
        crate::group::answer_sessionless_invite(ctx, target)
            .map_err(|error| reply.refused(error))?;
    }
    if let Some(packet) = reply.packet(PartyResult::Success) {
        super::send(ctx, id, packet, false)?;
    }
    Ok(())
}

fn named_character(ctx: &ReducerContext, name: &str) -> Option<u64> {
    crate::helpers::character_by_name(ctx, name).map(|c| c.guid)
}

fn named_member(ctx: &ReducerContext, guid: u64, name: &str) -> Option<u64> {
    let own = crate::group::group_of(ctx, guid)?;
    crate::group::members_of(ctx, own.group_id)
        .into_iter()
        .find_map(|member| {
            crate::helpers::character_by_guid(ctx, member.character_guid)
                .filter(|c| c.name.eq_ignore_ascii_case(name))
                .map(|c| c.guid)
        })
}

fn target_icon(request: MSG_RAID_TARGET_UPDATE_Client) -> GroupAction {
    use MSG_RAID_TARGET_UPDATE_Client as U;
    let (icon, target) = match request {
        U::RequestIcons => {
            return GroupAction::TargetIcon {
                icon: group::TARGET_ICON_LIST_REQUEST,
                target: 0,
            }
        }
        U::Unknown0 { target } => (0, target),
        U::Unknown1 { target } => (1, target),
        U::Unknown2 { target } => (2, target),
        U::Unknown3 { target } => (3, target),
        U::Unknown4 { target } => (4, target),
        U::Unknown5 { target } => (5, target),
        U::Unknown6 { target } => (6, target),
        U::Unknown7 { target } => (7, target),
        U::Unknown8 { target } => (8, target),
    };
    GroupAction::TargetIcon {
        icon,
        target: target.guid(),
    }
}

fn send_roster(ctx: &ReducerContext, connection: ConnectionId, guid: u64, roster: &RosterPayload) {
    super::member_stats::start(ctx, connection, guid);
    let list = codec::build_group_list(guid, roster, |member| {
        ctx.db.game_world_entity().guid().find(member).is_some()
    });
    let (opcode, body) = codec::build_group_list_raw(&list);
    super::send_raw(ctx, connection, opcode, body, false);
    if !roster.target_icons.is_empty() {
        let (opcode, body) = codec::build_target_icon_list_raw(&roster.target_icons);
        super::send_raw(ctx, connection, opcode, body, false);
    }
}

pub(crate) fn roster(ctx: &ReducerContext, guid: u64, all_members: bool) {
    let Some(member) = crate::group::group_of(ctx, guid) else {
        return;
    };
    let Some(roster) = crate::group::roster_payload(ctx, member.group_id)
        .and_then(|payload| RosterPayload::decode(&payload))
    else {
        return;
    };
    for member in &roster.members {
        if !all_members && member.guid != guid {
            continue;
        }
        if let Some(owner) = ctx
            .db
            .game_native_player()
            .character_guid()
            .find(member.guid)
        {
            send_roster(ctx, owner.connection_id, member.guid, &roster);
        }
    }
    if super::is_player(ctx, guid) && roster.kind == group::GroupKind::Raid {
        let icons = crate::group::target_icons_of(ctx, member.group_id);
        let (opcode, body) = codec::build_target_icon_list_raw(&icons);
        if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
            super::send_raw(ctx, owner.connection_id, opcode, body, false);
        }
    }
}

pub(super) fn group_event(ctx: &ReducerContext, guid: u64, kind: u8, other: u64, payload: &str) {
    let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) else {
        return;
    };
    let name = || {
        crate::helpers::character_by_guid(ctx, other)
            .map(|c| c.name)
            .unwrap_or_default()
    };
    use lyracore_shared::meeting_stone::event_kind as stone_kind;
    use ServerOpcodeMessage as S;
    let packet = match kind {
        event_kind::INVITE => Some(S::SMSG_GROUP_INVITE(Box::new(codec::build_group_invite(
            name(),
        )))),
        event_kind::DECLINE => Some(S::SMSG_GROUP_DECLINE(Box::new(codec::build_group_decline(
            name(),
        )))),
        event_kind::SET_LEADER => Some(S::SMSG_GROUP_SET_LEADER(Box::new(
            codec::build_group_set_leader(name()),
        ))),
        event_kind::DESTROYED => {
            super::member_stats::stop(ctx, owner.connection_id);
            Some(S::SMSG_GROUP_DESTROYED)
        }
        event_kind::UNINVITED => Some(S::SMSG_GROUP_UNINVITE),
        event_kind::LIST => {
            if let Some(roster) = RosterPayload::decode(payload) {
                send_roster(ctx, owner.connection_id, guid, &roster);
            }
            return;
        }
        event_kind::READY_CHECK => Some(S::MSG_RAID_READY_CHECK(Box::new(
            codec::build_ready_check(),
        ))),
        event_kind::READY_CHECK_ANSWER => group::decode_ready_check_answer(payload).map(|state| {
            S::MSG_RAID_READY_CHECK(Box::new(codec::build_ready_check_answer(other, state)))
        }),
        event_kind::TARGET_ICON_UPDATE => group::TargetIcon::decode(payload)
            .and_then(codec::build_target_icon_update)
            .map(|packet| S::MSG_RAID_TARGET_UPDATE(Box::new(packet))),
        event_kind::TARGET_ICON_LIST => {
            if let Some(icons) = group::decode_target_icons(payload) {
                let (opcode, body) = codec::build_target_icon_list_raw(&icons);
                super::send_raw(ctx, owner.connection_id, opcode, body, false);
            }
            return;
        }
        event_kind::MINIMAP_PING => group::decode_minimap_ping(payload)
            .map(|(x, y)| S::MSG_MINIMAP_PING(Box::new(codec::build_minimap_ping(other, x, y)))),
        event_kind::RANDOM_ROLL => group::decode_random_roll(payload).map(|(min, max, result)| {
            S::MSG_RANDOM_ROLL(Box::new(codec::build_random_roll(other, min, max, result)))
        }),
        stone_kind::QUEUE => lyracore_shared::meeting_stone::decode_queue(payload)
            .and_then(|(area, status)| codec::build_meetingstone_setqueue(area, status))
            .map(S::SMSG_MEETINGSTONE_SETQUEUE),
        stone_kind::MEMBER_ADDED => Some(S::SMSG_MEETINGSTONE_MEMBER_ADDED(
            codec::build_meetingstone_member_added(other),
        )),
        stone_kind::IN_PROGRESS => Some(S::SMSG_MEETINGSTONE_IN_PROGRESS),
        stone_kind::COMPLETE => Some(S::SMSG_MEETINGSTONE_COMPLETE),
        _ => return,
    };
    if let Some(packet) = packet {
        if let Err(cause) = super::send(ctx, owner.connection_id, packet, false) {
            log::warn!("native group event {kind} for {guid}: {cause}");
        }
    } else {
        log::warn!("native group event {kind} for {guid} has an invalid payload");
    }
}
