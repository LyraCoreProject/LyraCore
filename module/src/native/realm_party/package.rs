use spacetimedb::{reducer, sats::bsatn, table, Identity, ReducerContext, SpacetimeType, Table};

use super::{
    directory, encode, game_native_away, game_native_player, game_party_character,
    game_realm_party_command, PartyCharacter,
};
use crate::{
    group::{self, game_group_invite, GroupAction, Plane},
    package_account::game_package_account,
    peer::realm_shard,
};

#[table(accessor = game_package_party_authority)]
#[derive(Clone)]
pub struct PackagePartyAuthority {
    #[primary_key]
    pub guid: u64,
    pub world: Identity,
    pub revision: u64,
    pub active: bool,
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

fn facts(ctx: &ReducerContext, guid: u64, active: bool) -> Option<PartyCharacter> {
    let character = crate::helpers::character_by_guid(ctx, guid)?;
    ctx.db
        .game_package_account()
        .account_id()
        .find(character.account_id)?;
    if character.online
        || ctx
            .db
            .game_native_player()
            .character_guid()
            .find(guid)
            .is_some()
    {
        return None;
    }
    Some(PartyCharacter {
        guid,
        account_id: character.account_id,
        connection: 0,
        world: ctx.database_identity(),
        name: character.name,
        race: character.race,
        class: character.class,
        chat_tag: 0,
        identity: character.owner_identity,
        online: active,
        stats: active
            .then(|| super::super::member_stats::snapshot(ctx, guid))
            .flatten(),
        revision: directory::revision(ctx),
    })
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
    if realm_shard(ctx).is_none() {
        return;
    }
    let active = crate::sessionless::group_action_gate(ctx, guid).is_ok()
        && crate::helpers::character_by_guid(ctx, guid)
            .and_then(|character| {
                ctx.db
                    .game_package_account()
                    .account_id()
                    .find(character.account_id)
            })
            .is_some_and(|owner| {
                crate::package_teardown::runs(ctx, &owner.package_name.replace('-', "_"))
            });
    let Some(character) = facts(ctx, guid, active) else {
        return;
    };
    let authorities = ctx.db.game_package_party_authority();
    let authority = PackagePartyAuthority {
        guid,
        world: ctx.database_identity(),
        revision: character.revision,
        active,
    };
    authorities.guid().delete(guid);
    authorities.insert(authority.clone());
    queue(ctx, character, authority, Action::Presence);
}

pub(crate) fn submit(ctx: &ReducerContext, guid: u64, action: Action) -> Result<bool, String> {
    if realm_shard(ctx).is_none() {
        return Ok(false);
    }
    crate::sessionless::group_action_gate(ctx, guid)
        .map_err(|refusal| refusal.as_tag().to_string())?;
    let authority = ctx
        .db
        .game_package_party_authority()
        .guid()
        .find(guid)
        .filter(|authority| authority.active && authority.world == ctx.database_identity())
        .ok_or("Package Character has no active Group authority")?;
    let character = facts(ctx, guid, true).ok_or("Character is not owned by a Package")?;
    queue(ctx, character, authority, action);
    Ok(true)
}

pub(super) fn current(ctx: &ReducerContext, character: &PartyCharacter) -> bool {
    ctx.db
        .game_package_party_authority()
        .guid()
        .find(character.guid)
        .is_some_and(|authority| authority.active && authority.world == character.world)
        && ctx
            .db
            .game_package_account()
            .account_id()
            .find(character.account_id)
            .is_some()
        && ctx
            .db
            .game_native_away()
            .account_id()
            .find(character.account_id)
            .is_none()
}

fn admit(
    ctx: &ReducerContext,
    character: &PartyCharacter,
    authority: PackagePartyAuthority,
) -> Result<(), String> {
    use super::directory::game_character_directory;
    let guid = character.guid;
    let entry = ctx
        .db
        .game_character_directory()
        .guid()
        .find(guid)
        .ok_or("Package Character is not in the directory")?;
    let owner = ctx
        .db
        .game_package_account()
        .account_id()
        .find(entry.account_id)
        .ok_or("Character is not owned by a Package")?;
    if entry.world != ctx.sender()
        || character.world != ctx.sender()
        || character.account_id != entry.account_id
        || character.connection != 0
        || authority.guid != guid
        || authority.world != ctx.sender()
        || authority.revision > character.revision
        || authority.active != character.online
        || ctx
            .db
            .game_native_away()
            .account_id()
            .find(character.account_id)
            .is_some()
    {
        return Err("World Shard does not hold this Package Character".into());
    }
    if character.online
        && !crate::package_teardown::runs(ctx, &owner.package_name.replace('-', "_"))
    {
        return Err("Package is stopped on the Realm Shard".into());
    }
    let authorities = ctx.db.game_package_party_authority();
    if authorities.guid().find(guid).is_some_and(|old| {
        old.revision > authority.revision
            || (old.revision == authority.revision
                && (old.active != authority.active || old.world != authority.world))
    }) {
        return Err("Package Group authority changed".into());
    }
    authorities.guid().delete(guid);
    authorities.insert(authority);
    Ok(())
}

pub(super) fn apply(
    ctx: &ReducerContext,
    character: PartyCharacter,
    body: &[u8],
) -> Result<(), String> {
    let command: Command = bsatn::from_slice(body).map_err(|error| error.to_string())?;
    admit(ctx, &character, command.authority)?;
    let guid = character.guid;
    let mark = super::event_mark(ctx);
    let class = character.class;
    let race = character.race;
    super::store_character(ctx, character);
    if let Action::Presence = command.action {
        if let Some(member) = group::group_of(ctx, guid) {
            group::push_list_to_all(ctx, member.group_id);
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
    use crate::game_world_entity;
    let guids: std::collections::BTreeSet<_> = ctx
        .db
        .game_world_entity()
        .iter()
        .filter(|entity| entity.is_player())
        .map(|entity| entity.guid)
        .chain(
            ctx.db
                .game_package_party_authority()
                .iter()
                .map(|authority| authority.guid),
        )
        .collect();
    for guid in guids {
        presence(ctx, guid);
    }
    Ok(())
}
