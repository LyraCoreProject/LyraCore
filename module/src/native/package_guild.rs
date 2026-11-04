use std::collections::BTreeSet;

use lyracore_shared::faction::team_for_race;
use spacetimedb::{
    reducer, sats::bsatn, table, Identity, ReducerContext, ScheduleAt, SpacetimeType, Table,
    TimeDuration,
};

use super::{directory, package_actor, realm_guild, realm_party::PartyCharacter};
use crate::{
    guild::{self, membership::game_guild_invite, GuildAcceptRequest, GuildInviteRequest, GuildOp},
    package_account::game_package_account,
    peer::{realm_shard, require_sender, world_shard},
    remote_bindings::peer as remote,
};

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct GuildInvitation {
    pub guild_id: u32,
    pub inviter_guid: u64,
    pub created_micros: i64,
}

#[table(accessor = game_package_guild_invitation)]
pub struct PackageGuildInvitation {
    #[primary_key]
    pub guid: u64,
    pub revision: u64,
    pub invitation: Option<GuildInvitation>,
}

#[derive(SpacetimeType)]
enum Action {
    Invite(u64),
    Accept(GuildInvitation),
    Decline(GuildInvitation),
    Leave,
}

#[derive(SpacetimeType)]
struct Request {
    facts: realm_guild::Facts,
    authority: package_actor::PackagePartyAuthority,
    action: Option<Action>,
}

#[derive(SpacetimeType)]
enum Message {
    Request(Request),
    Invitation(PackageGuildInvitation),
}

#[table(accessor = game_package_guild_delivery, outbox(remote::idc_package_guild))]
pub struct PackageGuildDelivery {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub message: Vec<u8>,
}

#[table(accessor = game_package_guild_retry, scheduled(retry_package_guild))]
pub struct PackageGuildRetry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub call: PackageGuildDelivery,
}

fn encode<T: spacetimedb::Serialize>(value: &T) -> Vec<u8> {
    bsatn::to_vec(value).expect("Package guild message encodes")
}

fn send(ctx: &ReducerContext, database: Identity, message: Message) {
    ctx.db
        .game_package_guild_delivery()
        .insert(PackageGuildDelivery {
            id: 0,
            database: remote::Identity(database),
            message: encode(&message),
        });
}

fn pending(ctx: &ReducerContext, guid: u64) -> Option<GuildInvitation> {
    ctx.db
        .game_guild_invite()
        .target_guid()
        .find(guid)
        .map(|invite| GuildInvitation {
            guild_id: invite.guild_id,
            inviter_guid: invite.inviter_guid,
            created_micros: invite.created_at.to_micros_since_unix_epoch(),
        })
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn guild_invitation(ctx: &ReducerContext, guid: u64) -> Option<GuildInvitation> {
    let character = crate::helpers::character_by_guid(ctx, guid)?;
    ctx.db
        .game_package_account()
        .account_id()
        .find(character.account_id)?;
    if guild::member(ctx, guid).is_some() {
        return None;
    }
    if realm_shard(ctx).is_some() {
        ctx.db
            .game_package_guild_invitation()
            .guid()
            .find(guid)?
            .invitation
    } else {
        pending(ctx, guid)
    }
}

pub(crate) fn invitation_changed(ctx: &ReducerContext, guid: u64) {
    if world_shard(ctx).is_none() {
        return;
    }
    use directory::game_character_directory;
    let Some(entry) = ctx.db.game_character_directory().guid().find(guid) else {
        return;
    };
    if ctx
        .db
        .game_package_account()
        .account_id()
        .find(entry.account_id)
        .is_none()
    {
        return;
    }
    send(
        ctx,
        entry.world,
        Message::Invitation(PackageGuildInvitation {
            guid,
            revision: directory::revision(ctx),
            invitation: pending(ctx, guid),
        }),
    );
}

pub(super) fn presence(
    ctx: &ReducerContext,
    character: PartyCharacter,
    authority: package_actor::PackagePartyAuthority,
) {
    if let (Some(realm), Some(facts)) = (realm_shard(ctx), realm_guild::facts(ctx, character)) {
        send(
            ctx,
            realm,
            Message::Request(Request {
                facts,
                authority,
                action: None,
            }),
        );
    }
}

pub(crate) fn details_changed(ctx: &ReducerContext, guid: u64) {
    if realm_shard(ctx).is_some() {
        if let Ok((character, authority)) = package_actor::local(ctx, guid) {
            presence(ctx, character, authority);
        }
    }
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn invite_to_guild(ctx: &ReducerContext, guid: u64, target: u64) -> Result<(), String> {
    submit(ctx, guid, Action::Invite(target))
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn accept_guild_invite(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    let invitation = guild_invitation(ctx, guid).ok_or("no pending guild invitation")?;
    submit(ctx, guid, Action::Accept(invitation))
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn decline_guild_invite(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    let invitation = guild_invitation(ctx, guid).ok_or("no pending guild invitation")?;
    submit(ctx, guid, Action::Decline(invitation))
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn leave_guild(ctx: &ReducerContext, guid: u64) -> Result<(), String> {
    submit(ctx, guid, Action::Leave)
}

fn submit(ctx: &ReducerContext, guid: u64, action: Action) -> Result<(), String> {
    if world_shard(ctx).is_some() {
        return Err("request Package guild actions on their World Shard".into());
    }
    crate::sessionless::group_action_gate(ctx, guid)
        .map_err(|refusal| refusal.as_tag().to_string())?;
    if let Some(realm) = realm_shard(ctx) {
        if realm_guild::has_local_guilds(ctx) {
            return Err("local guild state requires migration".into());
        }
        let (character, authority) = package_actor::local(ctx, guid)?;
        let facts = realm_guild::facts(ctx, character).ok_or("Package Character is unavailable")?;
        send(
            ctx,
            realm,
            Message::Request(Request {
                facts,
                authority,
                action: Some(action),
            }),
        );
        Ok(())
    } else {
        let character =
            package_actor::facts(ctx, guid, true).ok_or("Character is not owned by a Package")?;
        apply(ctx, &character, action)
    }
}

fn apply(ctx: &ReducerContext, character: &PartyCharacter, action: Action) -> Result<(), String> {
    let guid = character.guid;
    let op = match action {
        Action::Invite(target) => {
            let invitee = if world_shard(ctx).is_some() {
                realm_guild::invitee_by_guid(ctx, guid, target)
            } else {
                crate::helpers::character_by_guid(ctx, target)
                    .and_then(|target| super::guild::invitee(ctx, guid, &target.name))
            }
            .ok_or("guild target is unavailable")?;
            GuildOp::Invite(GuildInviteRequest {
                target_guid: invitee.guid,
                actor_team: team_for_race(character.race),
                target_team: invitee.team,
                target_ignores_actor: invitee.ignores_actor,
            })
        }
        Action::Accept(expected) => {
            if pending(ctx, guid) != Some(expected) {
                return Err("guild invitation changed".into());
            }
            GuildOp::Accept(GuildAcceptRequest {
                actor_name: character.name.clone(),
                actor_team: team_for_race(character.race),
            })
        }
        Action::Decline(expected) => {
            if pending(ctx, guid) != Some(expected) {
                return Err("guild invitation changed".into());
            }
            GuildOp::Decline {
                actor_name: character.name.clone(),
            }
        }
        Action::Leave => GuildOp::Leave,
    };
    guild::apply_op(ctx, guid, character.account_id, op)
        .map_err(|refusal| refusal.as_tag().to_string())
}

#[reducer]
pub fn idc_package_guild(ctx: &ReducerContext, message: Vec<u8>) -> Result<(), String> {
    if !crate::peer::is_peer(ctx, ctx.sender()) {
        return Err("caller is not a Peer Shard".into());
    }
    let message: Message = bsatn::from_slice(&message).map_err(|error| error.to_string())?;
    match message {
        Message::Request(request) => {
            directory::require_world(ctx)?;
            let character = &request.facts.character;
            package_actor::admit(ctx, character, request.authority)?;
            let guid = character.guid;
            let mut guilds: BTreeSet<_> = guild::member(ctx, guid)
                .map(|member| member.guild_id)
                .into_iter()
                .collect();
            if let Some(action) = request.action {
                if !character.online {
                    return Err("Package Character is not active".into());
                }
                apply(ctx, character, action)?;
            }
            realm_guild::record_presence(ctx, request.facts)?;
            guilds.extend(guild::member(ctx, guid).map(|member| member.guild_id));
            realm_guild::publish(ctx, guilds);
            invitation_changed(ctx, guid);
        }
        Message::Invitation(invitation) => {
            require_sender(ctx, realm_shard(ctx))?;
            let invitations = ctx.db.game_package_guild_invitation();
            if invitations
                .guid()
                .find(invitation.guid)
                .is_none_or(|old| old.revision < invitation.revision)
            {
                directory::observe_revision(ctx, invitation.revision);
                invitations.guid().delete(invitation.guid);
                invitations.insert(invitation);
            }
        }
    }
    Ok(())
}

#[reducer(on_result(game_package_guild_delivery))]
pub fn package_guild_answered(
    ctx: &ReducerContext,
    call: PackageGuildDelivery,
    result: Result<(), String>,
) {
    let Err(error) = result else { return };
    let message: Message =
        bsatn::from_slice(&call.message).expect("stored Package guild message decodes");
    match message {
        Message::Request(request) => spacetimedb::log::warn!(
            "Package guild request for {} refused: {error}",
            request.facts.character.guid
        ),
        Message::Invitation(_) => {
            spacetimedb::log::warn!(
                "Package guild invitation to {} waits: {error}",
                call.database.0
            );
            ctx.db.game_package_guild_retry().insert(PackageGuildRetry {
                id: 0,
                scheduled_at: (ctx.timestamp + TimeDuration::from_micros(1_000_000)).into(),
                call,
            });
        }
    }
}

#[reducer]
pub fn retry_package_guild(ctx: &ReducerContext, retry: PackageGuildRetry) {
    if crate::time::scheduler_authorized(ctx) {
        ctx.db
            .game_package_guild_delivery()
            .insert(PackageGuildDelivery {
                id: 0,
                ..retry.call
            });
    }
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_package_guild_action(
    ctx: &ReducerContext,
    guid: u64,
    action: String,
    target: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    match action.as_str() {
        "invite" => crate::actor::invite_to_guild(ctx, guid, target),
        "accept" => crate::actor::accept_guild_invite(ctx, guid),
        "decline" => crate::actor::decline_guild_invite(ctx, guid),
        "leave" => crate::actor::leave_guild(ctx, guid),
        _ => Err("unknown Package guild action".into()),
    }
}
