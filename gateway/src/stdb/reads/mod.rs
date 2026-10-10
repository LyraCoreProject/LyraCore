//! Pure cache-accessor methods on `Coordinator`: read the privileged subscription cache (RLS
//! bypass) and project rows into codec views. A read only one Store family calls lives in
//! `store/<family>.rs`.
//!
//! Split by domain (pure code-motion): each submodule below is its own `impl Coordinator`
//! block. A free-fn helper used by callers in more than one submodule lives here instead of in
//! either domain file.

mod account;
mod channel;
mod guild;
mod items;
mod mail;
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

pub(crate) use account::{watch_character_names, watch_contacts, CharacterNameIndex, ContactIndex};
pub(crate) use channel::ChannelIndex;
pub(crate) use guild::{watch_guilds, GuildIndex};
pub(crate) use items::{property_enchant_ids, view_of_item_row};
pub(crate) use mail::{watch_mail_escrows, MailEscrowIndex};

// Shared with the Store family adapters in `stdb::store`.
pub(in crate::stdb) use guild::petition_view;
pub(in crate::stdb) use quest::quest_objectives_complete;
pub(in crate::stdb) use spell::spell_ranks_stack_in_book;

/// Sum a player's held quantity of item `entry` over `game_item_instance` (the coordinator reads any
/// player's items — RLS-bypassed, like the quest log). The gateway twin of the module's
/// `items::item_count`. Shared by `quest::quest_objectives_complete` and
/// `items::viewer_needs_quest_item`.
pub(in crate::stdb) fn player_item_count(db: &RemoteTables, owner_guid: u64, entry: u32) -> u32 {
    db.game_item_instance()
        .iter()
        .filter(|i| i.owner_guid == owner_guid && i.entry == entry)
        .map(|i| i.stack_count)
        .sum()
}
