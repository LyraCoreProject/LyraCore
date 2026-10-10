//! Package-owned Accounts: the Accounts a Package creates for its own session-less Characters, and
//! the record of which Package made each one.

use spacetimedb::{table, ReducerContext, Table};

use crate::auth::{game_account, Account, Appearance};

/// The Package that created an Account. A Core table, so it outlives the Package's own tables when
/// the Package is disabled. Private. [session]
#[table(accessor = game_package_account)]
pub struct PackageAccount {
    #[primary_key]
    pub account_id: u64,
    #[index(btree)]
    pub package_name: String,
}

/// Create a Character with no Session on an Account `package_name` owns, and return its guid. The
/// Character meets the same name, race and class Refusals as a client-created one, and those
/// Refusals write nothing. The Character goes on the first owned Account with room. When every one
/// is full, a new Account without credentials is created and recorded as owned by `package_name`.
#[cfg_attr(not(has_packages), allow(dead_code))] // package-only caller — see `build.rs`
pub(crate) fn create_package_character(
    ctx: &ReducerContext,
    package_name: &str,
    name: &str,
    race: u8,
    class: u8,
) -> Result<u64, String> {
    crate::auth::check_new_character(ctx, name, race, class)?;
    let account_id = owned_account_with_room(ctx, package_name)?;
    crate::auth::insert_new_character(
        ctx,
        account_id,
        name.to_string(),
        race,
        class,
        Appearance::default(),
    )
}

fn owned_account_with_room(ctx: &ReducerContext, package_name: &str) -> Result<u64, String> {
    let owned: Vec<u64> = ctx
        .db
        .game_package_account()
        .package_name()
        .filter(package_name)
        .map(|row| row.account_id)
        .collect();
    if let Some(&id) = owned
        .iter()
        .find(|&&id| crate::auth::account_has_room(ctx, id))
    {
        return Ok(id);
    }
    // `#` keeps the name apart from every login name, which are uppercased alphanumerics. An
    // Account that lost its ownership row still holds its name, so skip taken names.
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
        })
        .map_err(|error| format!("Package Account for {package_name} not created: {error}"))?;
    ctx.db.game_package_account().insert(PackageAccount {
        account_id: account.id,
        package_name: package_name.to_string(),
    });
    Ok(account.id)
}
