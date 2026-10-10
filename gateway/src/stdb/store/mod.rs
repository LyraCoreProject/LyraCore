//! `Coordinator`'s Store family adapters, one file per Protocol Family.
//!
//! Each file holds the family's trait impl and the inherent methods only that family calls.
//! Inherent methods with other callers stay in `reads`, `reducers`, `subscriptions` and
//! `connection`. Rust method resolution prefers inherent methods over trait methods, so
//! `self.characters(..)` here calls the inherent `Coordinator::characters`, not the trait method:
//! these are thin views, not recursion.

use anyhow::{anyhow, Result};
use std::time::Duration;

mod auction;
mod bank;
mod cast;
mod channel;
mod character;
mod chat;
mod combat;
mod death;
mod duel;
mod guild;
mod guild_fee;
mod item;
mod loot_roll;
mod loot_window;
mod mail;
mod meeting_stone;
mod melee;
mod member_stats;
mod npc;
mod party;
mod quest;
mod session;
mod shard_routing;
mod social;
mod speech;
mod taxi;
mod trade;
mod trainer;
mod transfer;
mod vendor;
mod weather;

/// Wait up to two seconds for a row a value-flow reducer just committed to reach this handle's
/// cache.
fn wait_for_cache_row<T>(
    operation_id: u64,
    row_name: &str,
    mut read: impl FnMut() -> Option<T>,
) -> Result<T> {
    for _ in 0..100 {
        if let Some(row) = read() {
            return Ok(row);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err(anyhow!(
        "{row_name} {operation_id} committed but is not visible in the coordinator cache"
    ))
}

/// A random nonzero operation id for one auction or guild fee value flow.
fn next_operation_id() -> Result<u64> {
    loop {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes)
            .map_err(|error| anyhow!("OS randomness unavailable: {error}"))?;
        let operation_id = u64::from_le_bytes(bytes);
        if operation_id != 0 {
            return Ok(operation_id);
        }
    }
}
