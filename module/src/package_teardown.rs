use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::package_account::game_package_account;
use crate::package_config::game_package_config;
use crate::sessionless::game_sessionless_action_consent;
use crate::transfer::{game_bot_transfer_intent, game_transfer_in, game_transfer_out};
use crate::{game_character, game_world_entity};

#[table(accessor = game_package_teardown)]
pub struct PackageTeardown {
    #[primary_key]
    pub package_name: String,
}

pub struct InstalledPackage {
    pub name: &'static str,
    pub tables: &'static [&'static str],
    pub characters: Option<fn(&ReducerContext) -> Vec<u64>>,
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn runs(ctx: &ReducerContext, package: &str) -> bool {
    let torn_down = ctx.db.game_package_teardown();
    torn_down.count() == 0 || torn_down.package_name().find(package.to_string()).is_none()
}

pub(crate) fn forget_removed_packages(ctx: &ReducerContext) {
    crate::package_combat::clear_removed_packages(ctx);
    let torn_down = ctx.db.game_package_teardown();
    if torn_down.count() == 0 {
        return;
    }
    for row in torn_down.iter().collect::<Vec<_>>() {
        if !crate::GAME_PACKAGES
            .iter()
            .any(|package| package.name == row.package_name)
        {
            torn_down.package_name().delete(row.package_name);
        }
    }
}

#[reducer]
pub fn teardown_package(ctx: &ReducerContext, package_name: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let ident = package_name.replace('-', "_");
    let package = crate::GAME_PACKAGES
        .iter()
        .find(|package| package.name == ident)
        .ok_or_else(|| format!("{package_name} is not a Package this build compiles"))?;
    let characters = package_characters(ctx, package, &package_name);
    if let Some(guid) = characters.iter().find(|&&guid| crossing(ctx, guid)) {
        return Err(format!(
            "Character {guid} is crossing between Shards; retry the teardown once it arrives"
        ));
    }

    // Silence Package hooks before its Characters leave the world.
    if ctx
        .db
        .game_package_teardown()
        .package_name()
        .find(ident.clone())
        .is_none()
    {
        ctx.db.game_package_teardown().insert(PackageTeardown {
            package_name: ident.clone(),
        });
    }
    crate::package_combat::clear_package(ctx, &package_name);
    for guid in characters {
        make_dormant(ctx, guid);
    }
    for table in package.tables {
        clear_table(table)?;
    }
    let config = ctx.db.game_package_config();
    for row in config
        .by_package_key()
        .filter(package_name.as_str())
        .collect::<Vec<_>>()
    {
        config.id().delete(row.id);
    }
    spacetimedb::log::info!("teardown_package: {package_name} torn down");
    Ok(())
}

fn package_characters(
    ctx: &ReducerContext,
    package: &InstalledPackage,
    package_name: &str,
) -> Vec<u64> {
    let mut guids: Vec<u64> = package.characters.map_or_else(Vec::new, |read| read(ctx));
    for account in ctx
        .db
        .game_package_account()
        .package_name()
        .filter(package_name)
    {
        guids.extend(
            ctx.db
                .game_character()
                .by_account()
                .filter(account.account_id)
                .map(|character| character.guid),
        );
    }
    guids.sort_unstable();
    guids.dedup();
    guids
}

// Settle Escrow before removing the Package tables named in its blob.
fn crossing(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db
        .game_transfer_out()
        .by_character()
        .filter(guid)
        .next()
        .is_some()
        || ctx
            .db
            .game_transfer_in()
            .by_character()
            .filter(guid)
            .next()
            .is_some()
        || ctx
            .db
            .game_bot_transfer_intent()
            .by_bot()
            .filter(guid)
            .any(|intent| intent.claim_token != 0)
}

fn make_dormant(ctx: &ReducerContext, guid: u64) {
    if crate::helpers::character_by_guid(ctx, guid).is_none_or(|character| character.online) {
        return;
    }
    let intents = ctx.db.game_bot_transfer_intent();
    for intent in intents.by_bot().filter(guid).collect::<Vec<_>>() {
        intents.id().delete(intent.id);
    }
    crate::group::clear_unclaimed_group_intents(ctx, guid);
    ctx.db
        .game_sessionless_action_consent()
        .character_guid()
        .delete(guid);
    if let Some(entity) = ctx.db.game_world_entity().guid().find(guid) {
        crate::world::remove_live_character(ctx, entity);
    }
}

fn clear_table(name: &str) -> Result<(), String> {
    use spacetimedb::sys::{datastore_clear, table_id_from_name, Errno};
    match table_id_from_name(name) {
        Ok(id) => datastore_clear(id)
            .map(|_| ())
            .map_err(|error| format!("could not empty {name}: {error}")),
        Err(error) if error == Errno::NO_SUCH_TABLE => Ok(()),
        Err(error) => Err(format!("could not find {name}: {error}")),
    }
}
