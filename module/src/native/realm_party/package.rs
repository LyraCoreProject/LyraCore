use spacetimedb::{
    reducer, sats::bsatn, table, ReducerContext, ScheduleAt, SpacetimeType, Table, TimeDuration,
};

use super::super::package_actor::{facts, game_package_party_authority, PackagePartyAuthority};
use super::{encode, game_party_character, game_realm_party_command, PartyCharacter};
use crate::{
    group::{self, game_group_invite, GroupAction, Plane},
    package_account::game_package_account,
    peer::realm_shard,
};

#[table(accessor = game_package_party_refresh, scheduled(refresh_package_party))]
pub struct PackagePartyRefresh {
    #[primary_key]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
}

pub(crate) fn membership_changed(ctx: &ReducerContext, guid: u64) {
    if realm_shard(ctx).is_none()
        || group::group_of(ctx, guid).is_none()
        || !ctx
            .db
            .game_package_party_authority()
            .guid()
            .find(guid)
            .is_some_and(|authority| authority.active && authority.world == ctx.database_identity())
    {
        return;
    }
    let refresh = ctx.db.game_package_party_refresh();
    if refresh.id().find(0).is_none() {
        refresh.insert(PackagePartyRefresh {
            id: 0,
            scheduled_at: crate::time::schedule_interval(ctx, TimeDuration::from_micros(5_000_000)),
        });
    }
}

#[reducer]
pub fn refresh_package_party(ctx: &ReducerContext, refresh: PackagePartyRefresh) {
    if !crate::time::scheduler_authorized(ctx)
        || ctx
            .db
            .game_package_party_refresh()
            .id()
            .find(refresh.id)
            .is_none()
    {
        return;
    }
    let grouped: Vec<_> = ctx
        .db
        .game_package_party_authority()
        .iter()
        .filter(|authority| {
            authority.active
                && authority.world == ctx.database_identity()
                && group::group_of(ctx, authority.guid).is_some()
        })
        .collect();
    if realm_shard(ctx).is_none() || grouped.is_empty() {
        ctx.db.game_package_party_refresh().id().delete(refresh.id);
        return;
    }
    // Optional statistics wait for outstanding Package commands instead of growing the outbox.
    if ctx
        .db
        .game_realm_party_command()
        .iter()
        .any(|call| call.opcode == super::PACKAGE_COMMAND)
    {
        return;
    }
    for authority in grouped {
        if crate::sessionless::group_action_gate(ctx, authority.guid).is_err() {
            presence(ctx, authority.guid);
        } else if let Some(character) = facts(ctx, authority.guid, true) {
            queue(ctx, character, authority, Action::Presence);
        }
    }
}

#[derive(SpacetimeType, Clone, Copy)]
pub(crate) enum Action {
    Presence,
    Invite(u64),
    Accept,
    Answer(Invitation),
    Leave,
}

#[derive(SpacetimeType, Clone, Copy)]
pub(crate) struct Invitation {
    pub id: u64,
    pub inviter: u64,
}

#[derive(SpacetimeType)]
struct Command {
    authority: PackagePartyAuthority,
    action: Action,
}

fn queue(
    ctx: &ReducerContext,
    character: PartyCharacter,
    authority: PackagePartyAuthority,
    action: Action,
) {
    let Some(realm) = realm_shard(ctx) else {
        return;
    };
    ctx.db
        .game_realm_party_command()
        .insert(super::RealmPartyCommand {
            id: 0,
            database: crate::remote_bindings::peer::Identity(realm),
            character: encode(&character),
            opcode: super::PACKAGE_COMMAND,
            body: encode(&Command { authority, action }),
        });
}

pub(crate) fn presence(ctx: &ReducerContext, guid: u64) {
    let Some((character, authority)) = super::super::package_actor::changed(ctx, guid) else {
        return;
    };
    membership_changed(ctx, guid);
    queue(ctx, character.clone(), authority.clone(), Action::Presence);
    super::super::package_guild::presence(ctx, character, authority);
}

pub(crate) fn submit(ctx: &ReducerContext, guid: u64, action: Action) -> Result<bool, String> {
    if realm_shard(ctx).is_none() {
        return Ok(false);
    }
    let (character, authority) = super::super::package_actor::local(ctx, guid)?;
    queue(ctx, character, authority, action);
    Ok(true)
}

pub(super) fn apply(
    ctx: &ReducerContext,
    character: PartyCharacter,
    body: &[u8],
) -> Result<(), String> {
    let command: Command = bsatn::from_slice(body).map_err(|error| error.to_string())?;
    super::super::package_actor::admit(ctx, &character, command.authority)?;
    let guid = character.guid;
    let mark = super::event_mark(ctx);
    let class = character.class;
    let race = character.race;
    let presence_changed = ctx
        .db
        .game_party_character()
        .guid()
        .find(guid)
        .is_none_or(|previous| {
            previous.online != character.online
                || previous.world != character.world
                || previous.connection != character.connection
        });
    super::store_character(ctx, character);
    if let Action::Presence = command.action {
        if presence_changed {
            if let Some(member) = group::group_of(ctx, guid) {
                group::push_list_to_all(ctx, member.group_id);
            }
        }
        if super::online(ctx, guid) {
            if let Some(invite) = ctx.db.game_group_invite().by_target().filter(guid).next() {
                group::push_event(
                    ctx,
                    guid,
                    lyracore_shared::group::event_kind::INVITE,
                    invite.inviter_guid,
                    String::new(),
                );
            }
        }
        super::publish(ctx, Default::default(), &[guid], mark);
        return Ok(());
    }
    if !super::online(ctx, guid) {
        return Err("Package Character is not active".into());
    }
    let action = match command.action {
        Action::Invite(target) => GroupAction::Invite(target),
        Action::Accept | Action::Answer(_) => {
            let invite = ctx
                .db
                .game_group_invite()
                .by_target()
                .filter(guid)
                .next()
                .ok_or("Group invitation expired")?;
            if matches!(command.action, Action::Answer(expected) if invite.inviter_guid != expected.inviter || invite.id != expected.id)
            {
                return Err("Group invitation changed".into());
            }
            GroupAction::Accept { class }
        }
        Action::Leave => GroupAction::Leave {
            cause: lyracore_shared::group::leave_cause::LEFT,
        },
        Action::Presence => unreachable!(),
    };
    let target = match action {
        GroupAction::Invite(target) => Some(target),
        GroupAction::Accept { .. } => ctx
            .db
            .game_group_invite()
            .by_target()
            .filter(guid)
            .next()
            .map(|invite| invite.inviter_guid),
        _ => None,
    };
    if let Some(target) = target {
        if !super::online(ctx, target) {
            return Err("Group target is offline".into());
        }
        if ctx
            .db
            .game_party_character()
            .guid()
            .find(target)
            .is_none_or(|target| !lyracore_shared::faction::same_team(race, target.race))
        {
            return Err("Group target belongs to another faction".into());
        }
    }
    let mut groups = group::action_groups(ctx, guid, action);
    group::apply_action(ctx, Plane::RealmCore, guid, action).map_err(|error| error.to_string())?;
    groups.extend(group::action_groups(ctx, guid, action));
    let mut characters = vec![guid];
    characters.extend(target);
    super::publish(ctx, groups, &characters, mark);
    Ok(())
}

#[reducer]
pub fn operator_report_package_groups(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if realm_shard(ctx).is_none() {
        return Err("Package Group presence must be reported by a World Shard".into());
    }
    use crate::game_character;
    let guids: Vec<_> = ctx
        .db
        .game_package_account()
        .iter()
        .flat_map(|owner| {
            ctx.db
                .game_character()
                .by_account()
                .filter(owner.account_id)
                .map(|character| character.guid)
        })
        .collect();
    for guid in guids {
        presence(ctx, guid);
    }
    Ok(())
}
