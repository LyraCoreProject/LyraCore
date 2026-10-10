//! Package Teardown: the Operator step that stops one Package on one Shard before the Package
//! leaves the build.
//!
//! A publish that removes a table refuses while the table holds rows. So before a Package's folder
//! moves, its tables must be empty, and its code must not write them again before the publish.
//! Teardown empties every table the Package declares, deletes its Package Config, and makes its
//! Characters Dormant Characters. It then records the Package as torn down, which stops every
//! registration the Package made (`build.rs` wraps each one in [`runs`]). The record goes away by
//! itself on the first tick of a build that no longer compiles the Package, so enabling the Package
//! again starts it fresh.

use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::package_account::game_package_account;
use crate::package_config::game_package_config;
use crate::sessionless::game_sessionless_action_consent;
use crate::transfer::{game_bot_transfer_intent, game_transfer_in, game_transfer_out};
use crate::{game_character, game_world_entity};

/// One row per Package torn down on this Shard. Private. [server]
#[table(accessor = game_package_teardown)]
pub struct PackageTeardown {
    #[primary_key]
    pub package_name: String,
}

/// One compiled Package, as `build.rs` generates it into `GAME_PACKAGES`.
pub struct InstalledPackage {
    /// The Package's Rust identifier: its folder name with `-` folded to `_`.
    pub name: &'static str,
    /// Every table the Package declares. Debug-only tables are absent from a release build.
    pub tables: &'static [&'static str],
    /// The Package's `game_package_characters!` read, when it registers one.
    pub characters: Option<fn(&ReducerContext) -> Vec<u64>>,
}

/// Whether a Package's registered code may run on this Shard.
#[cfg_attr(not(has_packages), allow(dead_code))] // package-only caller — see `build.rs`
pub(crate) fn runs(ctx: &ReducerContext, package: &str) -> bool {
    let torn_down = ctx.db.game_package_teardown();
    torn_down.count() == 0 || torn_down.package_name().find(package.to_string()).is_none()
}

/// Forget the teardown of a Package this build no longer compiles. Runs every scheduler tick.
pub(crate) fn forget_removed_packages(ctx: &ReducerContext) {
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

/// Tear down `package_name` on this Shard. Idempotent; run it on every Shard of the Realm, then
/// once more on each, so a Character that crossed into an already torn-down Shard is caught. A
/// Refusal writes nothing.
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

    // First, so the Package's own hooks stay silent while its Characters leave the world.
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

/// The Characters on Accounts the Package owns here, plus the ones it names itself.
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

/// An escrow row or a claimed Transfer Intent. The escrow blob names the Package's tables, so it
/// must settle while those tables still exist.
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

/// Offline, with no live entity, no consent and no pending Intent. The Account and Character rows
/// stay. A Character with a World Session is a player's, so it is left alone.
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

/// Empty one table by name. A table this build does not compile has nothing to empty.
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
