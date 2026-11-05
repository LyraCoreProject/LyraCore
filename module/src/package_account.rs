use spacetimedb::{table, ReducerContext, Table};

use crate::auth::{game_account, Account, Appearance};

#[table(accessor = game_package_account)]
pub struct PackageAccount {
    #[primary_key]
    pub account_id: u64,
    #[index(btree)]
    pub package_name: String,
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn create_package_character(
    ctx: &ReducerContext,
    package_name: &str,
    name: &str,
    race: u8,
    class: u8,
) -> Result<u64, String> {
    crate::auth::check_new_character(ctx, name, race, class)?;
    let account_id = owned_account_with_room(ctx, package_name)?;
    Ok(crate::auth::insert_new_character(
        ctx,
        account_id,
        name.to_string(),
        race,
        class,
        Appearance::default(),
    )?)
}

pub(crate) fn owned_account_with_room(
    ctx: &ReducerContext,
    package_name: &str,
) -> Result<u64, String> {
    let owned: Vec<u64> = ctx
        .db
        .game_package_account()
        .package_name()
        .filter(package_name)
        .map(|row| row.account_id)
        .collect();
    if let Some(&id) = owned.iter().find(|&&id| {
        if crate::peer::world_shard(ctx).is_some() {
            crate::native::realm_account_has_room(ctx, id)
        } else {
            crate::auth::account_has_room(ctx, id)
        }
    }) {
        return Ok(id);
    }
    // The # separator cannot collide with uppercased alphanumeric login names.
    let accounts = ctx.db.game_account();
    let username = (owned.len()..)
        .map(|n| format!("{package_name}#{n}"))
        .find(|name| accounts.username().find(name).is_none())
        .expect("an unbounded range holds a free name");
    let account = accounts
        .try_insert(Account {
            id: 0,
            username,
            salt: Vec::new(),
            verifier: Vec::new(),
            identity: None,
            banned: false,
            alpha_test_tools: false,
            gm_level: 0,
        })
        .map_err(|error| format!("Package Account for {package_name} not created: {error}"))?;
    ctx.db.game_package_account().insert(PackageAccount {
        account_id: account.id,
        package_name: package_name.to_string(),
    });
    Ok(account.id)
}

// Propagate an error to abort the caller; logout hooks and deletion commit together.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn delete_package_character(
    ctx: &ReducerContext,
    package: &str,
    guid: u64,
) -> Result<(), String> {
    use crate::game_world_entity;
    use crate::guild::game_guild;
    use crate::native::package_relocation::{game_package_relocation, Relocation};

    if crate::peer::world_shard(ctx).is_some() {
        return Err("delete the Package Character on its World Shard".into());
    }
    crate::sessionless::action_gate(ctx, guid).map_err(|error| error.to_string())?;
    let character = crate::helpers::character_by_guid(ctx, guid).ok_or("Character unavailable")?;
    if ctx
        .db
        .game_package_account()
        .account_id()
        .find(character.account_id)
        .is_none_or(|owner| owner.package_name != package)
    {
        return Err("Character belongs to another Package".into());
    }
    if ctx
        .db
        .game_package_relocation()
        .guid()
        .filter(guid)
        .any(|request| request.result == Relocation::Pending)
    {
        return Err("Package relocation is pending".into());
    }
    if ctx
        .db
        .game_guild()
        .by_leader()
        .filter(guid)
        .next()
        .is_some()
    {
        return Err("Guild Leader cannot delete their Character".into());
    }
    crate::auth::character_deletion_gate(ctx, character.account_id, guid)?;
    if let Some(entity) = ctx.db.game_world_entity().guid().find(guid) {
        crate::world::remove_live_character(ctx, entity, crate::world::CharacterDeparture::Logout);
    }
    crate::auth::apply_delete_character(ctx, character.account_id, guid)
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_delete_package_character(
    ctx: &ReducerContext,
    package: String,
    guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::package::delete_character(ctx, &package, guid)
}
