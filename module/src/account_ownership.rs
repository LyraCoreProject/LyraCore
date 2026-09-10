//! Account ownership shared by independent Gateways. Closed generations remain as fences.

use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table};

use crate::{game_account, game_character, game_gateway_session, game_world_entity, Account};

const CLAIM_MICROS: i64 = 60_000_000;
const REAP_LIMIT: usize = 64;
const ACCOUNT_CHARACTER_OWNER_LIMIT: usize = 4_096;
const STALE: &str = "STALE_WORLD_SESSION";

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldSessionToken {
    pub account_id: u64,
    pub generation: u64,
    pub request_nonce: u128,
}

/// A request retains its World Session's token even while queued for delivery.
/// An absent token is reserved for Operator-driven Characters without a World Session.
#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionActor {
    pub guid: u64,
    pub ownership: Option<WorldSessionToken>,
}

#[table(accessor = game_account_claim, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct AccountClaim {
    #[primary_key]
    pub account_id: u64,
    pub generation: u64,
    pub request_nonce: u128,
    pub character_guid: u64,
    pub expires_micros: i64,
    pub closed: bool,
}

#[table(accessor = game_account_fence, index(accessor = by_character, btree(columns = [character_guid])), index(accessor = by_closed, btree(columns = [closed])))]
pub struct AccountFence {
    #[primary_key]
    pub account_id: u64,
    pub account_name: String,
    pub generation: u64,
    pub request_nonce: u128,
    pub character_guid: u64,
    pub expires_micros: i64,
    pub closed: bool,
}

/// Retained Realm Account ownership for one globally unique Character guid. Every World Shard
/// keeps this row across Transfer, logout and Character deletion so a shadow Account never becomes
/// an ownership authority by itself.
#[table(
    accessor = game_account_character_owner,
    index(accessor = by_account, btree(columns = [account_id]))
)]
pub struct AccountCharacterOwner {
    #[primary_key]
    pub character_guid: u64,
    pub account_id: u64,
    pub account_name: String,
}

fn shadow_account(account: &Account) -> bool {
    account.username == format!("#{}", account.id)
        && account.salt.is_empty()
        && account.verifier.is_empty()
}

fn ownership_matches(
    owner: &AccountCharacterOwner,
    account_id: u64,
    account_name: &str,
    character_guid: u64,
) -> bool {
    owner.account_id == account_id
        && owner.account_name == account_name
        && owner.character_guid == character_guid
}

fn remember_character_owner(
    ctx: &ReducerContext,
    account_id: u64,
    account_name: &str,
    character_guid: u64,
) -> Result<(), String> {
    let owners = ctx.db.game_account_character_owner();
    if let Some(owner) = owners.character_guid().find(character_guid) {
        return if ownership_matches(&owner, account_id, account_name, character_guid) {
            Ok(())
        } else {
            Err("Character ownership changed".into())
        };
    }
    owners.insert(AccountCharacterOwner {
        character_guid,
        account_id,
        account_name: account_name.to_owned(),
    });
    Ok(())
}

fn character_belongs_to_account(
    ctx: &ReducerContext,
    account_id: u64,
    account_name: &str,
    character_guid: u64,
) -> bool {
    let Some(character) = ctx.db.game_character().guid().find(character_guid) else {
        return true;
    };
    let Some(local_account) = ctx.db.game_account().id().find(character.account_id) else {
        return false;
    };
    if local_account.username == account_name {
        return true;
    }
    shadow_account(&local_account)
        && ctx
            .db
            .game_account_character_owner()
            .character_guid()
            .find(character_guid)
            .is_some_and(|owner| {
                ownership_matches(&owner, account_id, account_name, character_guid)
            })
}

fn same(token: WorldSessionToken, generation: u64, nonce: u128) -> bool {
    token.generation == generation && token.request_nonce == nonce
}

fn now(ctx: &ReducerContext) -> i64 {
    ctx.timestamp.to_micros_since_unix_epoch()
}

fn deadline(ctx: &ReducerContext) -> Result<i64, String> {
    now(ctx)
        .checked_add(CLAIM_MICROS)
        .ok_or_else(|| "Account claim deadline overflow".into())
}

/// Retry the same request nonce to recover a lost reply. A closed or expired request never reopens.
#[reducer]
pub fn claim_account(
    ctx: &ReducerContext,
    account_id: u64,
    character_guid: u64,
    request_nonce: u128,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if request_nonce == 0 || character_guid == 0 {
        return Err("Account claim requires a Character and a nonzero request nonce".into());
    }
    let account = ctx
        .db
        .game_account()
        .id()
        .find(account_id)
        .ok_or("no such Account")?;
    if account.banned {
        return Err("Account is banned".into());
    }
    let claims = ctx.db.game_account_claim();
    let prior = claims.account_id().find(account_id);
    let generation = match &prior {
        Some(row) if row.request_nonce == request_nonce => {
            return if !row.closed
                && row.expires_micros > now(ctx)
                && row.character_guid == character_guid
            {
                Ok(())
            } else {
                Err(STALE.into())
            };
        }
        Some(row) if !row.closed && row.expires_micros > now(ctx) => {
            return Err("ACCOUNT_IN_USE".into());
        }
        Some(row) => row
            .generation
            .checked_add(1)
            .ok_or("Account generation exhausted")?,
        None => 1,
    };
    let row = AccountClaim {
        account_id,
        generation,
        request_nonce,
        character_guid,
        expires_micros: deadline(ctx)?,
        closed: false,
    };
    if prior.is_some() {
        claims.account_id().update(row);
    } else {
        claims.insert(row);
    }
    Ok(())
}

#[reducer]
pub fn renew_account_claim(ctx: &ReducerContext, token: WorldSessionToken) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let claims = ctx.db.game_account_claim();
    let mut row = claims.account_id().find(token.account_id).ok_or(STALE)?;
    if !same(token, row.generation, row.request_nonce)
        || row.closed
        || row.expires_micros <= now(ctx)
    {
        return Err(STALE.into());
    }
    row.expires_micros = deadline(ctx)?;
    claims.account_id().update(row);
    Ok(())
}

#[reducer]
pub fn release_account_claim(ctx: &ReducerContext, token: WorldSessionToken) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if let Some(mut row) = ctx
        .db
        .game_account_claim()
        .account_id()
        .find(token.account_id)
    {
        if same(token, row.generation, row.request_nonce) {
            row.closed = true;
            ctx.db.game_account_claim().account_id().update(row);
        }
    }
    Ok(())
}

/// The Gateway must obtain this generation from Realm-core and install it on every configured
/// World Shard before admission. Older deliveries cannot lower a Shard's retained generation.
#[reducer]
pub fn fence_account(
    ctx: &ReducerContext,
    token: WorldSessionToken,
    account_name: String,
    character_guid: u64,
    expires_micros: i64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if token.generation == 0
        || token.request_nonce == 0
        || character_guid == 0
        || expires_micros <= now(ctx)
    {
        return Err(STALE.into());
    }
    let fences = ctx.db.game_account_fence();
    let prior = fences.account_id().find(token.account_id);
    if let Some(row) = &prior {
        if row.account_name != account_name {
            return Err("Account fence name changed".into());
        }
        if token.generation < row.generation {
            return Err(STALE.into());
        }
        // This is the only ownership history an upgraded Shard can recover without trusting its
        // local Account id. Preserve it before a new generation replaces the fence.
        remember_character_owner(ctx, row.account_id, &row.account_name, row.character_guid)?;
    }
    if !character_belongs_to_account(ctx, token.account_id, &account_name, character_guid) {
        return Err("Character does not belong to Account".into());
    }
    remember_character_owner(ctx, token.account_id, &account_name, character_guid)?;
    if let Some(row) = &prior {
        if token.generation == row.generation {
            return if same(token, row.generation, row.request_nonce)
                && !row.closed
                && row.expires_micros > now(ctx)
                && row.character_guid == character_guid
                && row.account_name == account_name
            {
                Ok(())
            } else {
                Err(STALE.into())
            };
        }
    }
    remove_owned_characters(ctx, token.account_id, &account_name)?;
    remove_real_account_characters(ctx, token.account_id, &account_name)?;
    let row = AccountFence {
        account_id: token.account_id,
        account_name,
        generation: token.generation,
        request_nonce: token.request_nonce,
        character_guid,
        expires_micros,
        closed: false,
    };
    if prior.is_some() {
        fences.account_id().update(row);
    } else {
        fences.insert(row);
    }
    Ok(())
}

#[reducer]
pub fn renew_account_fence(
    ctx: &ReducerContext,
    token: WorldSessionToken,
    expires_micros: i64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let fences = ctx.db.game_account_fence();
    let mut row = fences.account_id().find(token.account_id).ok_or(STALE)?;
    if !same(token, row.generation, row.request_nonce)
        || row.closed
        || row.expires_micros <= now(ctx)
    {
        return Err(STALE.into());
    }
    row.expires_micros = row.expires_micros.max(expires_micros);
    fences.account_id().update(row);
    Ok(())
}

#[reducer]
pub fn close_account_fence(ctx: &ReducerContext, token: WorldSessionToken) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if let Some(mut row) = ctx
        .db
        .game_account_fence()
        .account_id()
        .find(token.account_id)
    {
        if same(token, row.generation, row.request_nonce) && !row.closed {
            remember_character_owner(ctx, row.account_id, &row.account_name, row.character_guid)?;
            remove_retained_character(ctx, row.account_id, &row.account_name, row.character_guid)?;
            row.closed = true;
            ctx.db.game_account_fence().account_id().update(row);
        }
    }
    Ok(())
}

fn retained_owner_can_remove_character(
    ctx: &ReducerContext,
    owner: &AccountCharacterOwner,
) -> bool {
    let Some(character) = ctx.db.game_character().guid().find(owner.character_guid) else {
        return true;
    };
    ctx.db
        .game_account()
        .id()
        .find(character.account_id)
        .is_some_and(|account| account.username == owner.account_name || shadow_account(&account))
}

fn retained_fence_can_remove_character(
    ctx: &ReducerContext,
    account_id: u64,
    account_name: &str,
    character_guid: u64,
) -> bool {
    let candidate = AccountCharacterOwner {
        character_guid,
        account_id,
        account_name: account_name.to_owned(),
    };
    ctx.db
        .game_account_character_owner()
        .character_guid()
        .find(character_guid)
        .is_none_or(|owner| ownership_matches(&owner, account_id, account_name, character_guid))
        && retained_owner_can_remove_character(ctx, &candidate)
}

fn remove_retained_character(
    ctx: &ReducerContext,
    account_id: u64,
    account_name: &str,
    character_guid: u64,
) -> Result<(), String> {
    let owner = ctx
        .db
        .game_account_character_owner()
        .character_guid()
        .find(character_guid)
        .filter(|owner| ownership_matches(owner, account_id, account_name, character_guid))
        .ok_or("Character ownership changed")?;
    if !retained_owner_can_remove_character(ctx, &owner) {
        return Err("Character ownership changed".into());
    }
    remove_exact_character(ctx, character_guid);
    Ok(())
}

fn remove_owned_characters(
    ctx: &ReducerContext,
    account_id: u64,
    account_name: &str,
) -> Result<(), String> {
    let owners: Vec<_> = ctx
        .db
        .game_account_character_owner()
        .by_account()
        .filter(account_id)
        .take(ACCOUNT_CHARACTER_OWNER_LIMIT + 1)
        .collect();
    if owners.len() > ACCOUNT_CHARACTER_OWNER_LIMIT {
        return Err("Account Character Owner limit exceeded".into());
    }
    if !owners.iter().all(|owner| {
        ownership_matches(owner, account_id, account_name, owner.character_guid)
            && retained_owner_can_remove_character(ctx, owner)
    }) {
        return Err("Character ownership changed".into());
    }
    for owner in owners {
        remove_exact_character(ctx, owner.character_guid);
    }
    Ok(())
}

fn remove_real_account_characters(
    ctx: &ReducerContext,
    realm_account_id: u64,
    account_name: &str,
) -> Result<(), String> {
    if let Some(account) = ctx
        .db
        .game_account()
        .username()
        .find(account_name.to_owned())
    {
        let guids: Vec<_> = ctx
            .db
            .game_character()
            .by_account()
            .filter(account.id)
            .map(|character| character.guid)
            .take(ACCOUNT_CHARACTER_OWNER_LIMIT + 1)
            .collect();
        if guids.len() > ACCOUNT_CHARACTER_OWNER_LIMIT {
            return Err("Account Character Owner limit exceeded".into());
        }
        let owners = ctx.db.game_account_character_owner();
        if guids.iter().any(|guid| {
            owners.character_guid().find(*guid).is_some_and(|owner| {
                !ownership_matches(&owner, realm_account_id, account_name, *guid)
            })
        }) {
            return Err("Character ownership changed".into());
        }
        for guid in guids {
            remove_exact_character(ctx, guid);
        }
    }
    Ok(())
}

fn remove_exact_character(ctx: &ReducerContext, guid: u64) {
    if let Some(entity) = ctx.db.game_world_entity().guid().find(guid) {
        crate::world::remove_live_character(ctx, entity);
    }
    ctx.db.game_gateway_session().entity_guid().delete(guid);
}

/// Shared Gate for gameplay, Realm-core actions and Transfer steps. It checks the token in the
/// transaction that acts on the Character; a prior Gateway read grants no authority.
pub(crate) fn require_actor(ctx: &ReducerContext, actor: SessionActor) -> Result<u64, String> {
    if let Some(token) = actor.ownership {
        if let Some(row) = ctx
            .db
            .game_account_fence()
            .account_id()
            .find(token.account_id)
        {
            if same(token, row.generation, row.request_nonce)
                && !row.closed
                && row.expires_micros > now(ctx)
                && row.character_guid == actor.guid
            {
                return Ok(actor.guid);
            }
        } else if let Some(row) = ctx
            .db
            .game_account_claim()
            .account_id()
            .find(token.account_id)
        {
            if same(token, row.generation, row.request_nonce)
                && !row.closed
                && row.expires_micros > now(ctx)
                && row.character_guid == actor.guid
            {
                return Ok(actor.guid);
            }
        }
        return Err(STALE.into());
    }
    let owned = ctx
        .db
        .game_account_fence()
        .by_character()
        .filter(actor.guid)
        .any(|row| !row.closed && row.expires_micros > now(ctx))
        || ctx
            .db
            .game_account_claim()
            .by_character()
            .filter(actor.guid)
            .any(|row| !row.closed && row.expires_micros > now(ctx));
    if owned {
        Err(STALE.into())
    } else {
        Ok(actor.guid)
    }
}

/// Completion requests must name the Character stored in the referenced durable operation.
/// Operator recovery without a token still checks that Character's current ownership.
pub(crate) fn require_actor_for(
    ctx: &ReducerContext,
    actor: SessionActor,
    character_guid: u64,
) -> Result<(), String> {
    if actor.ownership.is_some() && actor.guid != character_guid {
        return Err(STALE.into());
    }
    require_actor(
        ctx,
        SessionActor {
            guid: character_guid,
            ..actor
        },
    )
    .map(|_| ())
}

/// Called from the existing Gateway lease schedule; one surviving Gateway cannot renew another
/// World Session's Account claim or keep its Character alive.
pub(crate) fn reap_account_fences(ctx: &ReducerContext) {
    let cutoff = now(ctx);
    let expired: Vec<_> = ctx
        .db
        .game_account_fence()
        .by_closed()
        .filter(false)
        .filter(|row| row.expires_micros <= cutoff)
        .take(REAP_LIMIT)
        .collect();
    for mut row in expired {
        if !retained_fence_can_remove_character(
            ctx,
            row.account_id,
            &row.account_name,
            row.character_guid,
        ) {
            spacetimedb::log::error!("expired Account Fence cleanup ownership changed");
            row.closed = true;
            ctx.db.game_account_fence().account_id().update(row);
            continue;
        }
        if let Err(error) =
            remember_character_owner(ctx, row.account_id, &row.account_name, row.character_guid)
        {
            spacetimedb::log::error!("expired Account Fence ownership conflict: {error}");
            row.closed = true;
            ctx.db.game_account_fence().account_id().update(row);
            continue;
        }
        if let Err(error) =
            remove_retained_character(ctx, row.account_id, &row.account_name, row.character_guid)
        {
            spacetimedb::log::error!("expired Account Fence cleanup conflict: {error}");
            row.closed = true;
            ctx.db.game_account_fence().account_id().update(row);
            continue;
        }
        row.closed = true;
        ctx.db.game_account_fence().account_id().update(row);
    }
}
