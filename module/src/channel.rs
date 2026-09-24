//! Chat Channels on Realm-core: one copy of every channel per team, whichever World Shard its
//! members play on.
//!
//! Every op runs through [`realm_channel_op`]. A successful op writes its Channel Notices as
//! [`ChatChannelNoticeEvent`] rows with explicit recipients, which the Gateway delivers like a
//! Realm Chat Line. A Refusal rolls the transaction back, so the Gateway answers it from the tag.
//! Channel speech is a Realm Chat Line of kind CHANNEL; [`chat_audience`] is its audience rule.
//!
//! Channel Membership lasts as long as the Account Claim generation that admitted it. The claim
//! code calls [`leave_all`] when it releases, replaces or reaps a claim.

use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table, Timestamp};

use lyracore_shared::channel::{
    channel_flag, channel_name, channel_op, check_password, classify, member_flag, notice,
    ChannelName, ChannelRefusal,
};
use lyracore_shared::chat::ChatRefusal;
use lyracore_shared::faction::team_for_race;

use crate::realm_chat::{ChatAudience, RealmChatRequest, SpeakerFacts};

/// One Chat Channel. Private: the owner-token Coordinator is its only reader. Deleted with its
/// bans when its last member leaves. [entity]
#[table(
    accessor = game_chat_channel,
    index(accessor = by_name, btree(columns = [team, name_key]))
)]
pub struct ChatChannel {
    #[primary_key]
    #[auto_inc]
    pub channel_id: u64,
    /// `lyracore_shared::faction` team id. Each team has its own channels.
    pub team: u32,
    /// The lowercase name. With `team`, the channel's identity.
    pub name_key: String,
    /// The creator's spelling, echoed on the wire.
    pub name: String,
    /// The ChatChannels id. 0 for a custom channel.
    pub builtin_id: u32,
    /// `lyracore_shared::channel::channel_flag` bits.
    pub flags: u8,
    pub password: String,
    /// 0 for none. Built-in channels never have one.
    pub owner_guid: u64,
    /// JOINED and LEFT go to the other members.
    pub announcements: bool,
    /// Only moderators may speak.
    pub moderated: bool,
}

/// One Channel Membership. The row id is the join order that owner succession reads. [entity]
#[table(
    accessor = game_chat_channel_member,
    index(accessor = by_channel, btree(columns = [channel_id])),
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct ChatChannelMember {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel_id: u64,
    pub character_guid: u64,
    /// `lyracore_shared::channel::member_flag` bits.
    pub member_flags: u8,
    /// The Account Claim that admitted the member. 0 for an actor without a World Session.
    pub account_id: u64,
    pub claim_generation: u64,
}

/// A Character banned from one channel. Deleted with its channel. [entity]
#[table(
    accessor = game_chat_channel_ban,
    index(accessor = by_channel, btree(columns = [channel_id]))
)]
pub struct ChatChannelBan {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel_id: u64,
    pub character_guid: u64,
}

/// One Channel Notice with its complete recipient list. Private, reaped by the shared event GC.
/// Which fields a notice fills follows cm:Channel.cpp:748-946. [event]
#[table(accessor = game_chat_channel_notice_event)]
pub struct ChatChannelNoticeEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    /// `lyracore_shared::channel::notice`.
    pub notice: u8,
    pub channel_name: String,
    /// JOINED, LEFT, MODE_CHANGE, PLAYER_ALREADY_MEMBER, OWNER_CHANGED, the target of
    /// PLAYER_KICKED, PLAYER_BANNED and PLAYER_UNBANNED, and the inviter of INVITE.
    pub subject_guid: u64,
    /// PASSWORD_CHANGED, the announcement and moderation toggles, and the source of
    /// PLAYER_KICKED, PLAYER_BANNED and PLAYER_UNBANNED.
    pub actor_guid: u64,
    /// MODE_CHANGE only.
    pub old_flags: u8,
    pub new_flags: u8,
    /// YOU_JOINED only: the wire channel flags.
    pub channel_flags: u32,
    /// The name PLAYER_INVITED and PLAYER_INVITE_BANNED carry.
    pub text: String,
    pub recipients: Vec<u64>,
    pub created_at: Timestamp,
}

/// One channel op as the client asked for it, with the facts only the Gateway can read. Every op
/// uses the same shape, so adding an op never changes the reducer signature.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct ChannelRequest {
    /// As the client typed it.
    pub channel_name: String,
    /// JOIN and PASSWORD.
    pub password: String,
    /// Ops that name a player. The Gateway resolves the name realm-wide.
    pub target_guid: u64,
    /// The target's name, for notices that carry one.
    pub target_name: String,
    /// INVITE: the target's team check.
    pub target_race: u8,
    /// INVITE: read from the target's own contact rows.
    pub target_ignores_actor: bool,
    /// The actor. Its race decides the team.
    pub speaker: SpeakerFacts,
}

/// Run one channel op for `request_actor`.
///
/// Operator-gated because the actor is an argument: Realm-core has no live entity to derive it
/// from. A Refusal rolls the transaction back and returns its stable tag. An op byte this Module
/// does not know is an untagged error.
#[reducer]
pub fn realm_channel_op(
    ctx: &ReducerContext,
    request_actor: crate::SessionActor,
    op: u8,
    request: ChannelRequest,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let team = team_for_race(request.speaker.race);
    let outcome = match op {
        channel_op::JOIN => {
            let admission = request_actor
                .ownership
                .map_or((0, 0), |token| (token.account_id, token.generation));
            join(ctx, team, actor_guid, admission, &request)
        }
        channel_op::LEAVE => leave(ctx, team, actor_guid, &request.channel_name),
        channel_op::PASSWORD => set_password(ctx, team, actor_guid, &request),
        channel_op::SET_OWNER => set_owner(ctx, team, actor_guid, &request),
        channel_op::MODERATOR => set_mode_flag(
            ctx,
            team,
            actor_guid,
            &request,
            member_flag::MODERATOR,
            true,
        ),
        channel_op::UNMODERATOR => set_mode_flag(
            ctx,
            team,
            actor_guid,
            &request,
            member_flag::MODERATOR,
            false,
        ),
        channel_op::MUTE => {
            set_mode_flag(ctx, team, actor_guid, &request, member_flag::MUTED, true)
        }
        channel_op::UNMUTE => {
            set_mode_flag(ctx, team, actor_guid, &request, member_flag::MUTED, false)
        }
        channel_op::KICK => kick_or_ban(ctx, team, actor_guid, &request, false),
        channel_op::BAN => kick_or_ban(ctx, team, actor_guid, &request, true),
        channel_op::UNBAN => unban(ctx, team, actor_guid, &request),
        channel_op::INVITE => invite(ctx, team, actor_guid, &request),
        channel_op::ANNOUNCEMENTS => toggle(ctx, team, actor_guid, &request, Toggle::Announcements),
        channel_op::MODERATE => toggle(ctx, team, actor_guid, &request, Toggle::Moderate),
        other => return Err(format!("unknown channel op {other}")),
    };
    outcome.map_err(|refusal| {
        let tag = refusal.as_tag();
        spacetimedb::log::info!("channel op {op} refused {tag}: actor {actor_guid}");
        tag.to_string()
    })
}

/// cm:Channel.cpp:59-124. The channel is created on the first join; a refused join rolls the
/// creation back with it.
fn join(
    ctx: &ReducerContext,
    team: u32,
    joiner: u64,
    (account_id, claim_generation): (u64, u64),
    request: &ChannelRequest,
) -> Result<(), ChannelRefusal> {
    let name = channel_name(&request.channel_name)?;
    check_password(&request.password)?;
    let channel = find(ctx, team, &name.key).unwrap_or_else(|| create(ctx, team, name));
    if member_of(ctx, channel.channel_id, joiner).is_some() {
        // Built-in channels answer a repeat join with nothing (cm:Channel.cpp:64-72).
        return if channel.builtin_id != 0 {
            Ok(())
        } else {
            Err(ChannelRefusal::PlayerAlreadyMember)
        };
    }
    if is_banned(ctx, channel.channel_id, joiner) {
        return Err(ChannelRefusal::Banned);
    }
    if !channel.password.is_empty() && channel.password != request.password {
        return Err(ChannelRefusal::WrongPassword);
    }
    // A built-in channel can hold every player of a team, so its member list is read only when an
    // announcement or a first owner needs it. Built-in channels need neither.
    let takes_ownership = channel.builtin_id == 0 && channel.owner_guid == 0;
    let others = if channel.announcements || takes_ownership {
        members_in_join_order(ctx, channel.channel_id)
    } else {
        Vec::new()
    };
    if channel.announcements {
        notify(
            ctx,
            &channel,
            Notice::about(notice::JOINED, joiner),
            guids(&others),
        );
    }
    ctx.db.game_chat_channel_member().insert(ChatChannelMember {
        id: 0,
        channel_id: channel.channel_id,
        character_guid: joiner,
        member_flags: 0,
        account_id,
        claim_generation,
    });
    notify(
        ctx,
        &channel,
        Notice {
            notice: notice::YOU_JOINED,
            channel_flags: u32::from(channel.flags),
            ..Notice::default()
        },
        vec![joiner],
    );
    if takes_ownership {
        let exclaim = !others.is_empty();
        hand_ownership(ctx, channel, joiner, exclaim);
    }
    Ok(())
}

/// cm:Channel.cpp:126-167.
fn leave(
    ctx: &ReducerContext,
    team: u32,
    leaver: u64,
    raw_name: &str,
) -> Result<(), ChannelRefusal> {
    let (channel, member) = membership(ctx, team, raw_name, leaver)?;
    notify(
        ctx,
        &channel,
        Notice::about(notice::YOU_LEFT, 0),
        vec![leaver],
    );
    let announced = channel.announcements;
    depart(ctx, channel, member, announced);
    Ok(())
}

/// cm:Channel.cpp:288-317.
fn set_password(
    ctx: &ReducerContext,
    team: u32,
    actor: u64,
    request: &ChannelRequest,
) -> Result<(), ChannelRefusal> {
    let (mut channel, member) = membership(ctx, team, &request.channel_name, actor)?;
    if member.member_flags & member_flag::MODERATOR == 0 {
        return Err(ChannelRefusal::NotModerator);
    }
    channel.password = request.password.clone();
    let channel = ctx.db.game_chat_channel().channel_id().update(channel);
    notify(
        ctx,
        &channel,
        Notice {
            notice: notice::PASSWORD_CHANGED,
            actor_guid: actor,
            ..Notice::default()
        },
        guids(&members_in_join_order(ctx, channel.channel_id)),
    );
    Ok(())
}

/// SET_OWNER (cm:Channel.cpp:393-444, cm:Channel.cpp:976-1017). The actor must be the owner
/// (`NotOwner`) and the target must be a member (`PlayerNotFound`). A built-in channel has no
/// owner, so its `owner_guid` is always 0 and every actor fails the owner check.
fn set_owner(
    ctx: &ReducerContext,
    team: u32,
    actor: u64,
    request: &ChannelRequest,
) -> Result<(), ChannelRefusal> {
    let (channel, _) = membership(ctx, team, &request.channel_name, actor)?;
    if actor != channel.owner_guid {
        return Err(ChannelRefusal::NotOwner);
    }
    if member_of(ctx, channel.channel_id, request.target_guid).is_none() {
        return Err(ChannelRefusal::PlayerNotFound);
    }
    let exclaim = members_in_join_order(ctx, channel.channel_id).len() > 1;
    hand_ownership(ctx, channel, request.target_guid, exclaim);
    Ok(())
}

/// MODERATOR, UNMODERATOR, MUTE and UNMUTE (cm:Channel.cpp:319-391, cm:Channel.cpp:960-974). The
/// actor must be a moderator (`NotModerator`) and the target must be a member (`PlayerNotFound`).
/// Naming the owner refuses `NotOwner`, except that the owner's own MODERATOR bit never changes
/// (cm:Channel.cpp:359-360): a silent no-op, not a mute exemption. `change_member_flags` already
/// silently skips a flag that is already at the requested state.
fn set_mode_flag(
    ctx: &ReducerContext,
    team: u32,
    actor: u64,
    request: &ChannelRequest,
    flag: u8,
    set: bool,
) -> Result<(), ChannelRefusal> {
    let (channel, actor_member) = membership(ctx, team, &request.channel_name, actor)?;
    if actor_member.member_flags & member_flag::MODERATOR == 0 {
        return Err(ChannelRefusal::NotModerator);
    }
    let target = request.target_guid;
    if member_of(ctx, channel.channel_id, target).is_none() {
        return Err(ChannelRefusal::PlayerNotFound);
    }
    if target == channel.owner_guid {
        if actor != channel.owner_guid {
            return Err(ChannelRefusal::NotOwner);
        }
        if flag == member_flag::MODERATOR {
            return Ok(());
        }
    }
    change_member_flags(ctx, &channel, target, |flags| {
        if set {
            flags | flag
        } else {
            flags & !flag
        }
    });
    Ok(())
}

/// KICK and BAN (cm:Channel.cpp:169-237). The actor must be a moderator; only the owner may remove
/// the owner. A BAN of a Character not yet banned records the ban and writes PLAYER_BANNED;
/// otherwise, and for KICK, write PLAYER_KICKED. Either notice names target and actor and goes to
/// every member, target included, before the target departs (cm:Channel.cpp:231-233). `depart`
/// sends no LEFT of its own here, because this notice already covers the departure.
fn kick_or_ban(
    ctx: &ReducerContext,
    team: u32,
    actor: u64,
    request: &ChannelRequest,
    ban: bool,
) -> Result<(), ChannelRefusal> {
    let (channel, actor_member) = membership(ctx, team, &request.channel_name, actor)?;
    if actor_member.member_flags & member_flag::MODERATOR == 0 {
        return Err(ChannelRefusal::NotModerator);
    }
    let target = request.target_guid;
    let Some(target_member) = member_of(ctx, channel.channel_id, target) else {
        return Err(ChannelRefusal::PlayerNotFound);
    };
    if channel.owner_guid == target && actor != channel.owner_guid {
        return Err(ChannelRefusal::NotOwner);
    }
    let fresh_ban = ban && !is_banned(ctx, channel.channel_id, target);
    if fresh_ban {
        ctx.db.game_chat_channel_ban().insert(ChatChannelBan {
            id: 0,
            channel_id: channel.channel_id,
            character_guid: target,
        });
    }
    notify(
        ctx,
        &channel,
        Notice {
            notice: if fresh_ban {
                notice::PLAYER_BANNED
            } else {
                notice::PLAYER_KICKED
            },
            subject_guid: target,
            actor_guid: actor,
            ..Notice::default()
        },
        guids(&members_in_join_order(ctx, channel.channel_id)),
    );
    depart(ctx, channel, target_member, false);
    Ok(())
}

/// UNBAN (cm:Channel.cpp:239-286). The actor must be a moderator. An unresolved target (guid 0,
/// the Gateway's sentinel for a name that did not resolve to an online Character) refuses
/// `PlayerNotFound`, the same as cmangos's own `GetPlayer` miss, checked before the ban table,
/// never after: no real Character ever holds guid 0. A Character not banned refuses
/// `PlayerNotBanned`. Otherwise remove the ban and write PLAYER_UNBANNED naming target and actor.
fn unban(
    ctx: &ReducerContext,
    team: u32,
    actor: u64,
    request: &ChannelRequest,
) -> Result<(), ChannelRefusal> {
    let (channel, actor_member) = membership(ctx, team, &request.channel_name, actor)?;
    if actor_member.member_flags & member_flag::MODERATOR == 0 {
        return Err(ChannelRefusal::NotModerator);
    }
    let target = request.target_guid;
    if target == 0 {
        return Err(ChannelRefusal::PlayerNotFound);
    }
    let ban_id = ctx
        .db
        .game_chat_channel_ban()
        .by_channel()
        .filter(channel.channel_id)
        .find(|ban| ban.character_guid == target)
        .map(|ban| ban.id);
    let Some(ban_id) = ban_id else {
        return Err(ChannelRefusal::PlayerNotBanned);
    };
    ctx.db.game_chat_channel_ban().id().delete(ban_id);
    notify(
        ctx,
        &channel,
        Notice {
            notice: notice::PLAYER_UNBANNED,
            subject_guid: target,
            actor_guid: actor,
            ..Notice::default()
        },
        guids(&members_in_join_order(ctx, channel.channel_id)),
    );
    Ok(())
}

/// INVITE (cm:Channel.cpp:666-726). Needs no moderator right. An unresolved target (guid 0, the
/// Gateway's sentinel for a name that did not resolve to an online Character) refuses
/// `PlayerNotFound`, cmangos's own `GetPlayer` miss, checked first: race 0 would otherwise pass
/// the team check below as Alliance and falsely succeed. A target already a member refuses
/// `PlayerAlreadyMember`; a banned target refuses `PlayerInviteBanned`; another team refuses
/// `InviteWrongFaction`. Otherwise write INVITE naming the actor to the target, unless the target
/// ignores the actor, and write PLAYER_INVITED with the target's name to the actor.
fn invite(
    ctx: &ReducerContext,
    team: u32,
    actor: u64,
    request: &ChannelRequest,
) -> Result<(), ChannelRefusal> {
    let (channel, _) = membership(ctx, team, &request.channel_name, actor)?;
    let target = request.target_guid;
    if target == 0 {
        return Err(ChannelRefusal::PlayerNotFound);
    }
    if member_of(ctx, channel.channel_id, target).is_some() {
        return Err(ChannelRefusal::PlayerAlreadyMember);
    }
    if is_banned(ctx, channel.channel_id, target) {
        return Err(ChannelRefusal::PlayerInviteBanned);
    }
    if team_for_race(request.target_race) != team {
        return Err(ChannelRefusal::InviteWrongFaction);
    }
    if !request.target_ignores_actor {
        notify(
            ctx,
            &channel,
            Notice::about(notice::INVITE, actor),
            vec![target],
        );
    }
    notify(
        ctx,
        &channel,
        Notice {
            notice: notice::PLAYER_INVITED,
            text: request.target_name.clone(),
            ..Notice::default()
        },
        vec![actor],
    );
    Ok(())
}

/// What [`toggle`] flips.
enum Toggle {
    Announcements,
    Moderate,
}

/// ANNOUNCEMENTS and MODERATE (cm:Channel.cpp:523-591). The actor must be a moderator. Flip the
/// flag and tell every member.
fn toggle(
    ctx: &ReducerContext,
    team: u32,
    actor: u64,
    request: &ChannelRequest,
    which: Toggle,
) -> Result<(), ChannelRefusal> {
    let (mut channel, member) = membership(ctx, team, &request.channel_name, actor)?;
    if member.member_flags & member_flag::MODERATOR == 0 {
        return Err(ChannelRefusal::NotModerator);
    }
    let (on, notice_on, notice_off) = match which {
        Toggle::Announcements => {
            channel.announcements = !channel.announcements;
            (
                channel.announcements,
                notice::ANNOUNCEMENTS_ON,
                notice::ANNOUNCEMENTS_OFF,
            )
        }
        Toggle::Moderate => {
            channel.moderated = !channel.moderated;
            (
                channel.moderated,
                notice::MODERATION_ON,
                notice::MODERATION_OFF,
            )
        }
    };
    let channel = ctx.db.game_chat_channel().channel_id().update(channel);
    notify(
        ctx,
        &channel,
        Notice {
            notice: if on { notice_on } else { notice_off },
            actor_guid: actor,
            ..Notice::default()
        },
        guids(&members_in_join_order(ctx, channel.channel_id)),
    );
    Ok(())
}

/// Leave every channel without YOU_LEFT, as vanilla does at logout (cm:Player.cpp:4739-4750).
/// The other members still see LEFT where announced, and ownership passes on.
pub(crate) fn leave_all(ctx: &ReducerContext, character_guid: u64) {
    let memberships: Vec<ChatChannelMember> = ctx
        .db
        .game_chat_channel_member()
        .by_character()
        .filter(character_guid)
        .collect();
    for member in memberships {
        if let Some(channel) = ctx
            .db
            .game_chat_channel()
            .channel_id()
            .find(member.channel_id)
        {
            let announced = channel.announcements;
            depart(ctx, channel, member, announced);
        }
    }
}

/// Remove one member: LEFT to the rest when `notify_left`, owner succession, and the empty channel
/// deleted with its password and bans (cm:ChannelMgr.cpp:85-103). Built-in channels keep no state
/// worth an empty row either. `notify_left` is `channel.announcements` for a plain leave; KICK and
/// BAN pass `false`, because their own PLAYER_KICKED or PLAYER_BANNED notice already covers the
/// departure and vanilla does not double-announce it. The remaining members are read only when a
/// notice or a new owner needs them, so leaving a built-in channel never reads its member list.
fn depart(
    ctx: &ReducerContext,
    channel: ChatChannel,
    member: ChatChannelMember,
    notify_left: bool,
) {
    ctx.db.game_chat_channel_member().id().delete(member.id);
    let empty = ctx
        .db
        .game_chat_channel_member()
        .by_channel()
        .filter(channel.channel_id)
        .next()
        .is_none();
    if empty {
        delete_channel(ctx, channel.channel_id);
        return;
    }
    let hands_over = channel.owner_guid == member.character_guid && channel.builtin_id == 0;
    if !notify_left && !hands_over {
        return;
    }
    let rest = members_in_join_order(ctx, channel.channel_id);
    if notify_left {
        notify(
            ctx,
            &channel,
            Notice::about(notice::LEFT, member.character_guid),
            guids(&rest),
        );
    }
    if hands_over {
        let successor = successor(&rest);
        hand_ownership(ctx, channel, successor, rest.len() > 1);
    }
}

/// Owner succession: the earliest-joined moderator, else the earliest joiner. mangos
/// `SelectNewOwner` does the same with guid order (cm:Channel.cpp:948-958); join order is the
/// fairer tie-break. `members` is in join order and not empty.
pub(crate) fn successor(members: &[ChatChannelMember]) -> u64 {
    members
        .iter()
        .find(|member| member.member_flags & member_flag::MODERATOR != 0)
        .or(members.first())
        .map_or(0, |member| member.character_guid)
}

/// Make `new_owner` the owner of a custom channel (cm:Channel.cpp:976-1016). A previous owner who
/// is still a member keeps MODERATOR and loses OWNER. The new owner gains both. Each change is one
/// MODE_CHANGE to every member, and OWNER_CHANGED follows when `exclaim` is set.
pub(crate) fn hand_ownership(
    ctx: &ReducerContext,
    mut channel: ChatChannel,
    new_owner: u64,
    exclaim: bool,
) {
    let previous = channel.owner_guid;
    if previous != 0 && previous != new_owner {
        change_member_flags(ctx, &channel, previous, |flags| flags & !member_flag::OWNER);
    }
    channel.owner_guid = new_owner;
    let channel = ctx.db.game_chat_channel().channel_id().update(channel);
    change_member_flags(ctx, &channel, new_owner, |flags| {
        flags | member_flag::OWNER | member_flag::MODERATOR
    });
    if exclaim {
        notify(
            ctx,
            &channel,
            Notice::about(notice::OWNER_CHANGED, new_owner),
            guids(&members_in_join_order(ctx, channel.channel_id)),
        );
    }
}

/// Rewrite one member's flags and tell every member with MODE_CHANGE. No row, no notice when the
/// character is not a member or the flags do not change.
pub(crate) fn change_member_flags(
    ctx: &ReducerContext,
    channel: &ChatChannel,
    character_guid: u64,
    change: impl FnOnce(u8) -> u8,
) {
    let members = members_in_join_order(ctx, channel.channel_id);
    let recipients = guids(&members);
    let Some(mut member) = members
        .into_iter()
        .find(|member| member.character_guid == character_guid)
    else {
        return;
    };
    let old_flags = member.member_flags;
    let new_flags = change(old_flags);
    if new_flags == old_flags {
        return;
    }
    member.member_flags = new_flags;
    ctx.db.game_chat_channel_member().id().update(member);
    notify(
        ctx,
        channel,
        Notice {
            notice: notice::MODE_CHANGE,
            subject_guid: character_guid,
            old_flags,
            new_flags,
            ..Notice::default()
        },
        recipients,
    );
}

/// Who hears a CHANNEL line (cm:Channel.cpp:593-664): every member, speaker included. A listener
/// who ignores the speaker skips the line unless the speaker is a moderator (cm:Channel.cpp:663,
/// vm:Channel.cpp:677). Vanilla has no death check here.
pub(crate) fn chat_audience(
    ctx: &ReducerContext,
    speaker_guid: u64,
    request: &RealmChatRequest,
) -> Result<ChatAudience, ChatRefusal> {
    let team = team_for_race(request.speaker.race);
    let (channel, speaker) =
        membership(ctx, team, &request.channel_name, speaker_guid).map_err(ChatRefusal::Channel)?;
    let read_only = classify(&channel.name).is_some_and(|builtin| !builtin.players_may_speak());
    if speaker.member_flags & member_flag::MUTED != 0 || read_only {
        return Err(ChatRefusal::Channel(ChannelRefusal::Muted));
    }
    let moderator = speaker.member_flags & member_flag::MODERATOR != 0;
    if channel.moderated && !moderator {
        return Err(ChatRefusal::Channel(ChannelRefusal::NotModerator));
    }
    Ok(ChatAudience {
        recipients: guids(&members_in_join_order(ctx, channel.channel_id)),
        ignorable: !moderator,
        channel_name: channel.name,
    })
}

/// The channel `raw_name` names for `team`, and `character_guid`'s membership in it.
pub(crate) fn membership(
    ctx: &ReducerContext,
    team: u32,
    raw_name: &str,
    character_guid: u64,
) -> Result<(ChatChannel, ChatChannelMember), ChannelRefusal> {
    let key = ChannelName::normalize(raw_name).key;
    let channel = find(ctx, team, &key).ok_or(ChannelRefusal::NotMember)?;
    let member =
        member_of(ctx, channel.channel_id, character_guid).ok_or(ChannelRefusal::NotMember)?;
    Ok((channel, member))
}

/// `character_guid`'s membership in one channel, read through the Character's own few
/// memberships rather than the channel's member list.
fn member_of(
    ctx: &ReducerContext,
    channel_id: u64,
    character_guid: u64,
) -> Option<ChatChannelMember> {
    ctx.db
        .game_chat_channel_member()
        .by_character()
        .filter(character_guid)
        .find(|member| member.channel_id == channel_id)
}

fn find(ctx: &ReducerContext, team: u32, key: &str) -> Option<ChatChannel> {
    ctx.db
        .game_chat_channel()
        .by_name()
        .filter((team, key))
        .next()
}

/// A built-in channel gets its wire flags, no announcements and never an owner. A custom channel
/// gets CUSTOM and announcements (cm:Channel.cpp:26-57).
fn create(ctx: &ReducerContext, team: u32, name: ChannelName) -> ChatChannel {
    let builtin = classify(&name.display);
    ctx.db.game_chat_channel().insert(ChatChannel {
        channel_id: 0,
        team,
        name_key: name.key,
        name: name.display,
        builtin_id: builtin.map_or(0, |builtin| builtin.id),
        flags: builtin.map_or(channel_flag::CUSTOM, |builtin| builtin.wire_flags()),
        password: String::new(),
        owner_guid: 0,
        announcements: builtin.is_none(),
        moderated: false,
    })
}

fn delete_channel(ctx: &ReducerContext, channel_id: u64) {
    let bans: Vec<u64> = ctx
        .db
        .game_chat_channel_ban()
        .by_channel()
        .filter(channel_id)
        .map(|ban| ban.id)
        .collect();
    for id in bans {
        ctx.db.game_chat_channel_ban().id().delete(id);
    }
    ctx.db.game_chat_channel().channel_id().delete(channel_id);
}

pub(crate) fn is_banned(ctx: &ReducerContext, channel_id: u64, character_guid: u64) -> bool {
    ctx.db
        .game_chat_channel_ban()
        .by_channel()
        .filter(channel_id)
        .any(|ban| ban.character_guid == character_guid)
}

pub(crate) fn members_in_join_order(
    ctx: &ReducerContext,
    channel_id: u64,
) -> Vec<ChatChannelMember> {
    let mut members: Vec<ChatChannelMember> = ctx
        .db
        .game_chat_channel_member()
        .by_channel()
        .filter(channel_id)
        .collect();
    members.sort_unstable_by_key(|member| member.id);
    members
}

pub(crate) fn guids(members: &[ChatChannelMember]) -> Vec<u64> {
    members.iter().map(|member| member.character_guid).collect()
}

/// A [`ChatChannelNoticeEvent`] without the channel, recipients, id and time. Unused fields stay
/// zero.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Notice {
    pub(crate) notice: u8,
    pub(crate) subject_guid: u64,
    pub(crate) actor_guid: u64,
    pub(crate) old_flags: u8,
    pub(crate) new_flags: u8,
    pub(crate) channel_flags: u32,
    pub(crate) text: String,
}

impl Notice {
    /// A notice that names one member, or none when `subject_guid` is 0.
    pub(crate) fn about(notice: u8, subject_guid: u64) -> Self {
        Self {
            notice,
            subject_guid,
            ..Self::default()
        }
    }
}

/// The only writer of `game_chat_channel_notice_event`. Nothing is written for an empty audience.
pub(crate) fn notify(
    ctx: &ReducerContext,
    channel: &ChatChannel,
    notice: Notice,
    recipients: Vec<u64>,
) {
    if recipients.is_empty() {
        return;
    }
    ctx.db
        .game_chat_channel_notice_event()
        .insert(ChatChannelNoticeEvent {
            id: 0,
            notice: notice.notice,
            channel_name: channel.name.clone(),
            subject_guid: notice.subject_guid,
            actor_guid: notice.actor_guid,
            old_flags: notice.old_flags,
            new_flags: notice.new_flags,
            channel_flags: notice.channel_flags,
            text: notice.text,
            recipients,
            created_at: ctx.timestamp,
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_scan::code_of;

    fn member(id: u64, character_guid: u64, member_flags: u8) -> ChatChannelMember {
        ChatChannelMember {
            id,
            channel_id: 1,
            character_guid,
            member_flags,
            account_id: 0,
            claim_generation: 0,
        }
    }

    #[test]
    fn the_earliest_moderator_succeeds_before_an_earlier_joiner() {
        let members = [member(3, 30, 0), member(5, 50, 0x02), member(7, 70, 0x02)];
        assert_eq!(successor(&members), 50);
    }

    #[test]
    fn without_a_moderator_the_earliest_joiner_succeeds() {
        let members = [member(3, 30, 0x08), member(5, 50, 0)];
        assert_eq!(successor(&members), 30);
    }

    /// The actor is an argument, so the operator gate is the entire authorization. The scan
    /// anchors to the opening brace, so a neutralized gate fails it.
    #[test]
    fn the_realm_channel_op_reducer_is_operator_gated() {
        let body = code_of(include_str!("channel.rs"), "pub fn realm_channel_op(");
        let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
            "`realm_channel_op` no longer OPENS with the operator gate. Body was:\n{body}"
        );
    }
}
