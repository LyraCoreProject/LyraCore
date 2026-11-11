use spacetimedb::{
    reducer, sats::bsatn, table, Identity, ReducerContext, ScheduleAt, SpacetimeType, Table,
    TimeDuration,
};

use super::{character_names, directory, realm_shard};
use crate::{
    auth::{game_account, Account},
    character::Appearance,
    package_account::{game_package_account, PackageAccount},
    peer::{realm_shard as realm, require_sender, world_shard},
    remote_bindings::peer as remote,
};

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum CharacterCreation {
    Pending,
    Created(u64),
    Refused(String),
}

#[table(accessor = game_package_character_request)]
pub struct PackageCharacterRequest {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub package_name: String,
    pub name: String,
    pub race: u8,
    pub class: u8,
    pub result: CharacterCreation,
}

#[derive(SpacetimeType, Clone)]
struct Request {
    id: u64,
    package: String,
    name: String,
    race: u8,
    class: u8,
}

#[derive(SpacetimeType)]
struct Grant {
    request: Request,
    account: Vec<u8>,
}

#[derive(SpacetimeType)]
struct Created {
    request: Request,
    profile: directory::CharacterProfile,
}

#[derive(SpacetimeType)]
struct Completion {
    id: u64,
    result: CharacterCreation,
}

#[derive(SpacetimeType)]
enum Message {
    Request(Request),
    Grant(Grant),
    Created(Created),
    Complete(Completion),
}

#[table(accessor = game_package_creation_delivery, outbox(remote::idc_package_creation))]
pub struct PackageCreationDelivery {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[target]
    pub database: remote::Identity,
    pub message: Vec<u8>,
}

#[table(accessor = game_package_creation_retry, scheduled(retry_package_creation))]
pub struct PackageCreationRetry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub call: PackageCreationDelivery,
}
crate::virtual_clock_exempt!(game_package_creation_retry);

fn encode<T: spacetimedb::Serialize>(value: &T) -> Vec<u8> {
    bsatn::to_vec(value).expect("Package creation message encodes")
}

fn send(ctx: &ReducerContext, database: Identity, message: Message) {
    ctx.db
        .game_package_creation_delivery()
        .insert(PackageCreationDelivery {
            id: 0,
            database: remote::Identity(database),
            message: encode(&message),
        });
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn request_character(
    ctx: &ReducerContext,
    package: &str,
    name: &str,
    race: u8,
    class: u8,
) -> Result<u64, String> {
    if !crate::package_teardown::runs(ctx, &package.replace('-', "_")) {
        return Err("Package is stopped".into());
    }
    if world_shard(ctx).is_some() {
        return Err("request Package Characters on their World Shard".into());
    }
    let requests = ctx.db.game_package_character_request();
    if requests.package_name().filter(package).count() >= 64 {
        return Err("consume completed Package Character requests before creating more".into());
    }
    let result = if realm(ctx).is_some() {
        CharacterCreation::Pending
    } else {
        CharacterCreation::Created(crate::package_account::create_package_character(
            ctx, package, name, race, class,
        )?)
    };
    let row = requests.insert(PackageCharacterRequest {
        id: 0,
        package_name: package.into(),
        name: name.into(),
        race,
        class,
        result,
    });
    if let Some(realm) = realm(ctx) {
        send(
            ctx,
            realm,
            Message::Request(Request {
                id: row.id,
                package: package.into(),
                name: name.into(),
                race,
                class,
            }),
        );
    }
    Ok(row.id)
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn take_character_result(
    ctx: &ReducerContext,
    package: &str,
    request_id: u64,
) -> Result<CharacterCreation, String> {
    let requests = ctx.db.game_package_character_request();
    let request = requests
        .id()
        .find(request_id)
        .ok_or("unknown Package Character request")?;
    if request.package_name != package {
        return Err("Character request belongs to another Package".into());
    }
    if request.result != CharacterCreation::Pending {
        requests.id().delete(request_id);
    }
    Ok(request.result)
}

pub(crate) fn require_settled(ctx: &ReducerContext, package: &str) -> Result<(), String> {
    let ident = package.replace('-', "_");
    if ctx
        .db
        .game_package_character_request()
        .iter()
        .any(|request| {
            request.package_name.replace('-', "_") == ident
                && request.result == CharacterCreation::Pending
        })
    {
        return Err(
            "Package Character creation is pending; wait for its result before teardown".into(),
        );
    }
    if world_shard(ctx).is_some() {
        use super::character_names::game_character_name;
        for owner in ctx
            .db
            .game_package_account()
            .iter()
            .filter(|owner| owner.package_name.replace('-', "_") == ident)
        {
            if ctx
                .db
                .game_character_name()
                .by_account()
                .filter(owner.account_id)
                .any(|name| name.guid == 0)
            {
                return Err("Package Character creation is pending on a World Shard".into());
            }
        }
    }
    Ok(())
}

pub(crate) fn clear_results(ctx: &ReducerContext, package: &str) {
    let requests = ctx.db.game_package_character_request();
    let ident = package.replace('-', "_");
    let ids: Vec<_> = requests
        .iter()
        .filter(|request| request.package_name.replace('-', "_") == ident)
        .map(|request| request.id)
        .collect();
    for id in ids {
        requests.id().delete(id);
    }
}

pub(super) fn store_account(
    ctx: &ReducerContext,
    package: &str,
    account: &[u8],
) -> Result<Account, String> {
    let decoded: Account = bsatn::from_slice(account).map_err(|error| error.to_string())?;
    let accounts = ctx.db.game_account();
    if decoded.identity.is_some()
        || !decoded.salt.is_empty()
        || !decoded.verifier.is_empty()
        || accounts.id().find(decoded.id).is_some_and(|old| {
            old.username != decoded.username
                || old.identity.is_some()
                || !old.salt.is_empty()
                || !old.verifier.is_empty()
        })
        || accounts
            .username()
            .find(&decoded.username)
            .is_some_and(|old| old.id != decoded.id)
        || ctx
            .db
            .game_package_account()
            .account_id()
            .find(decoded.id)
            .is_some_and(|owner| owner.package_name != package)
    {
        return Err("Package Account conflicts with existing World state".into());
    }
    let account = realm_shard::store_account(ctx, account)?;
    let owners = ctx.db.game_package_account();
    owners.account_id().delete(account.id);
    owners.insert(PackageAccount {
        account_id: account.id,
        package_name: package.to_string(),
    });
    Ok(account)
}

fn finish(ctx: &ReducerContext, id: u64, result: CharacterCreation) -> Result<(), String> {
    if result == CharacterCreation::Pending {
        return Err("Package Character result is still pending".into());
    }
    let requests = ctx.db.game_package_character_request();
    let Some(mut request) = requests.id().find(id) else {
        return Ok(());
    };
    if request.result != CharacterCreation::Pending && request.result != result {
        return Err("Package Character result changed".into());
    }
    request.result = result;
    requests.id().update(request);
    Ok(())
}

#[reducer]
pub fn idc_package_creation(ctx: &ReducerContext, message: Vec<u8>) -> Result<(), String> {
    if !crate::peer::is_peer(ctx, ctx.sender()) {
        return Err("caller is not a Peer Shard".into());
    }
    let message: Message = bsatn::from_slice(&message).map_err(|error| error.to_string())?;
    match message {
        Message::Request(request) => {
            directory::require_world(ctx)?;
            if !crate::package_teardown::runs(ctx, &request.package.replace('-', "_")) {
                return Err("Package is stopped on the Realm Shard".into());
            }
            let account_id =
                crate::package_account::owned_account_with_room(ctx, &request.package)?;
            character_names::reserve(ctx, &request.name, account_id, ctx.sender(), 0)?;
            let account = ctx
                .db
                .game_account()
                .id()
                .find(account_id)
                .ok_or("Package Account disappeared")?;
            send(
                ctx,
                ctx.sender(),
                Message::Grant(Grant {
                    request,
                    account: encode(&account),
                }),
            );
        }
        Message::Grant(Grant { request, account }) => {
            require_sender(ctx, realm(ctx))?;
            let pending = ctx
                .db
                .game_package_character_request()
                .id()
                .find(request.id)
                .ok_or("Package Character request disappeared")?;
            if pending.result != CharacterCreation::Pending
                || pending.package_name != request.package
                || pending.name != request.name
                || pending.race != request.race
                || pending.class != request.class
            {
                return Err("Package Character request changed".into());
            }
            if !crate::package_teardown::runs(ctx, &request.package.replace('-', "_")) {
                return Err("Package is stopped on the World Shard".into());
            }
            let account = store_account(ctx, &request.package, &account)?;
            let guid = crate::character::insert_new_character(
                ctx,
                account.id,
                request.name.clone(),
                request.race,
                request.class,
                Appearance::default(),
            )?;
            let profile = directory::CharacterProfile {
                guid,
                account_id: account.id,
                world: ctx.database_identity(),
                revision: directory::revision(ctx),
                name: request.name.clone(),
                race: request.race,
                class: request.class,
                gender: 0,
                deleted: false,
            };
            send(
                ctx,
                ctx.sender(),
                Message::Created(Created { request, profile }),
            );
        }
        Message::Created(Created { request, profile }) => {
            directory::require_world(ctx)?;
            if profile.deleted
                || profile.guid == 0
                || profile.name != request.name
                || profile.race != request.race
                || profile.class != request.class
                || ctx
                    .db
                    .game_package_account()
                    .account_id()
                    .find(profile.account_id)
                    .is_none_or(|owner| owner.package_name != request.package)
            {
                return Err("Package Character does not match its creation request".into());
            }
            let guid = profile.guid;
            directory::idc_character_profiles(ctx, encode(&vec![profile]))?;
            send(
                ctx,
                ctx.sender(),
                Message::Complete(Completion {
                    id: request.id,
                    result: CharacterCreation::Created(guid),
                }),
            );
        }
        Message::Complete(Completion { id, result }) => {
            require_sender(ctx, realm(ctx))?;
            finish(ctx, id, result)?;
        }
    }
    Ok(())
}

#[reducer(on_result(game_package_creation_delivery))]
pub fn package_creation_answered(
    ctx: &ReducerContext,
    call: PackageCreationDelivery,
    result: Result<(), String>,
) {
    let Err(error) = result else {
        return;
    };
    let message: Message =
        bsatn::from_slice(&call.message).expect("stored Package creation message decodes");
    // IDC reports a durable receiver refusal here; Transport Loss keeps the call pending.
    match message {
        Message::Request(request) => {
            finish(ctx, request.id, CharacterCreation::Refused(error))
                .expect("pending request can be refused");
        }
        Message::Grant(Grant { request, account }) => {
            let account: Account =
                bsatn::from_slice(&account).expect("stored Package Account decodes");
            character_names::release_package(ctx, &request.name, account.id, call.database.0);
            send(
                ctx,
                call.database.0,
                Message::Complete(Completion {
                    id: request.id,
                    result: CharacterCreation::Refused(error),
                }),
            );
        }
        Message::Created(_) | Message::Complete(_) => {
            spacetimedb::log::warn!(
                "Package creation result to {} waits: {error}",
                call.database.0
            );
            ctx.db
                .game_package_creation_retry()
                .insert(PackageCreationRetry {
                    id: 0,
                    scheduled_at: (ctx.timestamp + TimeDuration::from_micros(1_000_000)).into(),
                    call,
                });
        }
    }
}

#[reducer]
pub fn retry_package_creation(ctx: &ReducerContext, retry: PackageCreationRetry) {
    if crate::time::scheduler_authorized(ctx) {
        ctx.db
            .game_package_creation_delivery()
            .insert(PackageCreationDelivery {
                id: 0,
                ..retry.call
            });
    }
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_request_package_character(
    ctx: &ReducerContext,
    package: String,
    name: String,
    race: u8,
    class: u8,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::package::request_character(ctx, &package, &name, race, class).map(|_| ())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_take_package_character_result(
    ctx: &ReducerContext,
    package: String,
    request_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let result = crate::package::take_character_result(ctx, &package, request_id)?;
    spacetimedb::log::info!("Package Character request {request_id}: {result:?}");
    Ok(())
}
