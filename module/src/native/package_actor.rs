use spacetimedb::{table, Identity, ReducerContext, Table};

use super::{
    directory, game_native_player, realm_party::PartyCharacter, realm_shard::game_native_away,
};
use crate::{package_account::game_package_account, peer::realm_shard};

#[table(accessor = game_package_party_authority)]
#[derive(Clone)]
pub struct PackagePartyAuthority {
    #[primary_key]
    pub guid: u64,
    pub world: Identity,
    pub revision: u64,
    pub active: bool,
}

pub(super) fn facts(ctx: &ReducerContext, guid: u64, active: bool) -> Option<PartyCharacter> {
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
            .then(|| super::member_stats::snapshot(ctx, guid))
            .flatten(),
        revision: directory::revision(ctx),
    })
}

pub(super) fn changed(
    ctx: &ReducerContext,
    guid: u64,
) -> Option<(PartyCharacter, PackagePartyAuthority)> {
    if realm_shard(ctx).is_none() {
        return None;
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
    let character = facts(ctx, guid, active)?;
    let authorities = ctx.db.game_package_party_authority();
    let authority = PackagePartyAuthority {
        guid,
        world: ctx.database_identity(),
        revision: character.revision,
        active,
    };
    authorities.guid().delete(guid);
    authorities.insert(authority.clone());
    Some((character, authority))
}

pub(super) fn local(
    ctx: &ReducerContext,
    guid: u64,
) -> Result<(PartyCharacter, PackagePartyAuthority), String> {
    crate::sessionless::group_action_gate(ctx, guid)
        .map_err(|refusal| refusal.as_tag().to_string())?;
    let authority = ctx
        .db
        .game_package_party_authority()
        .guid()
        .find(guid)
        .filter(|authority| authority.active && authority.world == ctx.database_identity())
        .ok_or("Package Character has no active social authority")?;
    let character = facts(ctx, guid, true).ok_or("Character is not owned by a Package")?;
    Ok((character, authority))
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

pub(super) fn admit(
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
        return Err("Package social authority changed".into());
    }
    authorities.guid().delete(guid);
    authorities.insert(authority);
    Ok(())
}
