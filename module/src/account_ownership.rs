use spacetimedb::{reducer, table, ReducerContext, SpacetimeType, Table};

use crate::game_account;

const CLAIM_MICROS: i64 = 60_000_000;
const REAP_LIMIT: usize = 64;
const STALE: &str = "STALE_WORLD_SESSION";

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldSessionToken {
    pub account_id: u64,
    pub generation: u64,
    pub request_nonce: u128,
}

#[derive(SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionActor {
    pub guid: u64,
    pub ownership: Option<WorldSessionToken>,
}

#[table(accessor = game_account_claim, index(accessor = by_character, btree(columns = [character_guid])), index(accessor = by_closed, btree(columns = [closed])))]
pub struct AccountClaim {
    #[primary_key]
    pub account_id: u64,
    pub generation: u64,
    pub request_nonce: u128,
    pub character_guid: u64,
    pub expires_micros: i64,
    pub closed: bool,
}

fn same(token: WorldSessionToken, generation: u64, nonce: u128) -> bool {
    token.generation == generation && token.request_nonce == nonce
}

fn now(ctx: &ReducerContext) -> i64 {
    crate::time::now(ctx).to_micros_since_unix_epoch()
}

fn deadline(ctx: &ReducerContext) -> Result<i64, String> {
    now(ctx)
        .checked_add(CLAIM_MICROS)
        .ok_or_else(|| "Account claim deadline overflow".into())
}

// Retrying a nonce can recover its reply but cannot reopen a closed or expired generation.
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
    // End the old generation's admitted state before installing its successor.
    if let Some(row) = &prior {
        claim_ended(ctx, row.character_guid);
    }
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
        if same(token, row.generation, row.request_nonce) && !row.closed {
            let character_guid = row.character_guid;
            row.closed = true;
            ctx.db.game_account_claim().account_id().update(row);
            claim_ended(ctx, character_guid);
        }
    }
    Ok(())
}

fn claim_ended(ctx: &ReducerContext, character_guid: u64) {
    crate::channel::leave_all(ctx, character_guid);
    crate::meeting_stone::remove_solo_seeker(ctx, character_guid);
}

// An open Account Claim makes its Character answer only to that claim's token.
pub(crate) fn require_actor(ctx: &ReducerContext, actor: SessionActor) -> Result<u64, String> {
    let live = |row: &AccountClaim| !row.closed && row.expires_micros > now(ctx);
    if let Some(token) = actor.ownership {
        return match ctx
            .db
            .game_account_claim()
            .account_id()
            .find(token.account_id)
        {
            Some(row)
                if same(token, row.generation, row.request_nonce)
                    && live(&row)
                    && row.character_guid == actor.guid =>
            {
                Ok(actor.guid)
            }
            _ => Err(STALE.into()),
        };
    }
    if ctx
        .db
        .game_account_claim()
        .by_character()
        .filter(actor.guid)
        .any(|row| live(&row))
    {
        Err(STALE.into())
    } else {
        Ok(actor.guid)
    }
}

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

pub(crate) fn reap_account_claims(ctx: &ReducerContext) {
    let cutoff = now(ctx);
    let expired: Vec<AccountClaim> = ctx
        .db
        .game_account_claim()
        .by_closed()
        .filter(false)
        .filter(|row| row.expires_micros <= cutoff)
        .take(REAP_LIMIT)
        .collect();
    for mut row in expired {
        let character_guid = row.character_guid;
        row.closed = true;
        ctx.db.game_account_claim().account_id().update(row);
        claim_ended(ctx, character_guid);
    }
}

// Retired Account Fence tables keep their declarations; see docs/danger-zones.md.
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
