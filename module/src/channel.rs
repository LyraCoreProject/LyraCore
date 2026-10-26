use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table, Timestamp};

use lyracore_shared::channel::{
    channel_flag, channel_name, channel_op, check_password, classify, member_flag, notice,
    ChannelName, ChannelRefusal, GUILD_RECRUITMENT_ID,
};
use lyracore_shared::chat::ChatRefusal;
use lyracore_shared::faction::team_for_race;

use crate::realm_chat::{ChatAudience, RealmChatRequest, SpeakerFacts};

#[table(
    accessor = game_chat_channel,
    index(accessor = by_name, btree(columns = [team, name_key]))
)]
pub struct ChatChannel {
    #[primary_key]
    #[auto_inc]
    pub channel_id: u64,
    pub team: u32,
    pub name_key: String,
    pub name: String,
    pub builtin_id: u32,
    pub flags: u8,
    pub password: String,
    pub owner_guid: u64,
    pub announcements: bool,
    pub moderated: bool,
}

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
    pub member_flags: u8,
    // A nonzero generation ties membership to a Gateway Account Claim.
    pub account_id: u64,
    pub claim_generation: u64,
}

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

#[table(accessor = game_chat_channel_notice_event)]
pub struct ChatChannelNoticeEvent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub notice: u8,
    pub channel_name: String,
    pub subject_guid: u64,
    pub actor_guid: u64,
    pub old_flags: u8,
    pub new_flags: u8,
    pub channel_flags: u32,
    pub text: String,
    pub recipients: Vec<u64>,
    pub created_at: Timestamp,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct ChannelRequest {
    pub channel_name: String,
    pub password: String,
    pub target_guid: u64,
    pub target_name: String,
    pub target_race: u8,
    pub target_ignores_actor: bool,
    pub speaker: SpeakerFacts,
}

#[reducer]
pub fn realm_channel_op(
    ctx: &ReducerContext,
    request_actor: crate::account_ownership::SessionActor,
    op: u8,
    request: ChannelRequest,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let actor_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    let admission = request_actor
        .ownership
        .map_or((0, 0), |token| (token.account_id, token.generation));
    let op = match op {
        channel_op::JOIN => ChannelOp::Join,
        channel_op::LEAVE => ChannelOp::Leave,
        channel_op::PASSWORD => ChannelOp::Password,
        channel_op::SET_OWNER => ChannelOp::SetOwner,
        channel_op::MODERATOR => ChannelOp::Moderator,
        channel_op::UNMODERATOR => ChannelOp::Unmoderator,
        channel_op::MUTE => ChannelOp::Mute,
        channel_op::UNMUTE => ChannelOp::Unmute,
        channel_op::KICK => ChannelOp::Kick,
        channel_op::BAN => ChannelOp::Ban,
        channel_op::UNBAN => ChannelOp::Unban,
        channel_op::INVITE => ChannelOp::Invite,
        channel_op::ANNOUNCEMENTS => ChannelOp::Announcements,
        channel_op::MODERATE => ChannelOp::Moderate,
        other => return Err(format!("unknown channel op {other}")),
    };
    apply_op(ctx, actor_guid, admission, op, request).map_err(|refusal| refusal.as_tag().into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChannelOp {
    Join,
    Leave,
    Password,
    SetOwner,
    Moderator,
    Unmoderator,
    Mute,
    Unmute,
    Kick,
    Ban,
    Unban,
    Invite,
    Announcements,
    Moderate,
}

pub(crate) fn apply_op(
    ctx: &ReducerContext,
    actor_guid: u64,
    admission: (u64, u64),
    op: ChannelOp,
    request: ChannelRequest,
) -> Result<(), ChannelRefusal> {
    let team = team_for_race(request.speaker.race);
    match op {
        ChannelOp::Join => join(ctx, team, actor_guid, admission, &request),
        ChannelOp::Leave => leave(ctx, team, actor_guid, &request.channel_name),
        ChannelOp::Password => set_password(ctx, team, actor_guid, &request),
        ChannelOp::SetOwner => set_owner(ctx, team, actor_guid, &request),
        ChannelOp::Moderator => set_mode_flag(
            ctx,
            team,
            actor_guid,
            &request,
            member_flag::MODERATOR,
            true,
        ),
        ChannelOp::Unmoderator => set_mode_flag(
            ctx,
            team,
            actor_guid,
            &request,
            member_flag::MODERATOR,
            false,
        ),
        ChannelOp::Mute => set_mode_flag(ctx, team, actor_guid, &request, member_flag::MUTED, true),
        ChannelOp::Unmute => {
            set_mode_flag(ctx, team, actor_guid, &request, member_flag::MUTED, false)
        }
        ChannelOp::Kick => kick_or_ban(ctx, team, actor_guid, &request, false),
        ChannelOp::Ban => kick_or_ban(ctx, team, actor_guid, &request, true),
        ChannelOp::Unban => unban(ctx, team, actor_guid, &request),
        ChannelOp::Invite => invite(ctx, team, actor_guid, &request),
        ChannelOp::Announcements => toggle(ctx, team, actor_guid, &request, Toggle::Announcements),
        ChannelOp::Moderate => toggle(ctx, team, actor_guid, &request, Toggle::Moderate),
    }
    .inspect_err(|refusal| {
        spacetimedb::log::info!(
            "channel op {op:?} refused {}: actor {actor_guid}",
            refusal.as_tag()
        );
    })
}

fn join(
    ctx: &ReducerContext,
    team: u32,
    joiner: u64,
    (account_id, claim_generation): (u64, u64),
    request: &ChannelRequest,
) -> Result<(), ChannelRefusal> {
    let name = channel_name(&request.channel_name)?;
    check_password(&request.password)?;
    if classify(&name.display).is_some_and(|builtin| builtin.id == GUILD_RECRUITMENT_ID)
        && crate::guild::member(ctx, joiner).is_some()
    {
        return Ok(());
    }
    let channel = find(ctx, team, &name.key).unwrap_or_else(|| create(ctx, team, name));
    if member_of(ctx, channel.channel_id, joiner).is_some() {
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
    check_password(&request.password)?;
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

enum Toggle {
    Announcements,
    Moderate,
}

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

pub(crate) fn successor(members: &[ChatChannelMember]) -> u64 {
    members
        .iter()
        .find(|member| member.member_flags & member_flag::MODERATOR != 0)
        .or(members.first())
        .map_or(0, |member| member.character_guid)
}

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
    pub(crate) fn about(notice: u8, subject_guid: u64) -> Self {
        Self {
            notice,
            subject_guid,
            ..Self::default()
        }
    }
}

pub(crate) fn notify(
    ctx: &ReducerContext,
    channel: &ChatChannel,
    notice: Notice,
    recipients: Vec<u64>,
) {
    if recipients.is_empty() {
        return;
    }
    let event = ctx
        .db
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
    crate::native::channel_notice(ctx, &event);
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
