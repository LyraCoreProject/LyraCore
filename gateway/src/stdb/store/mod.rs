//! `Coordinator`'s Store family adapters, one file per Protocol Family.
//!
//! Each file holds the family's trait impl and the inherent methods only that family calls.
//! Inherent methods with other callers stay in `reads`, `reducers`, `subscriptions` and
//! `connection`. Rust method resolution prefers inherent methods over trait methods, so
//! `self.characters(..)` here calls the inherent `Coordinator::characters`, not the trait method:
//! these are thin views, not recursion.

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
