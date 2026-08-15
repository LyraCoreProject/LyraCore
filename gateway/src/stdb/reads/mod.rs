//! Pure cache-accessor methods on `Coordinator`: read the privileged subscription cache (RLS
//! bypass) and project rows into codec views. No reducer calls — those live in `reducers.rs`.
//!
//! Split by domain (pure code-motion): each submodule below is its own `impl Coordinator`
//! block. A free-fn helper used by callers in more than one submodule lives here instead of in
//! either domain file.

mod account;
mod guild;
mod items;
mod mail;
mod npc_vendor;
mod party;
mod quest;
mod spell;
mod talent_reputation;
mod templates;

use spacetimedb_sdk::Table;

use super::bindings::*;

// Re-exported so `subscriptions.rs` keeps resolving `super::reads::build_quest_log_slots` at the
// same path it used before the domain split.
pub(crate) use quest::build_quest_log_slots;

// Shared by `quest::quest_objectives_complete` and `items::viewer_needs_quest_item`.
fn player_item_count(db: &RemoteTables, owner_guid: u64, entry: u32) -> u32 {
    db.game_item_instance()
        .iter()
        .filter(|i| i.owner_guid == owner_guid && i.entry == entry)
        .map(|i| i.stack_count)
        .sum()
}
