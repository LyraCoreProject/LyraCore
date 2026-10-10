//! `Coordinator`'s Store family adapters, one file per family.
//!
//! Most methods forward to `Coordinator`'s like-named INHERENT method (defined by concern in
//! `reads`/`reducers`/`subscriptions`/`connection`). Rust method resolution prefers inherent methods
//! over trait methods, so `self.characters(..)` here calls the inherent `Coordinator::characters`,
//! not the trait method: these are thin views, not recursion.

mod bank;
mod character;
mod combat;
mod death;
mod loot_roll;
mod mail;
mod npc;
mod party;
mod session;
mod shard_routing;
mod social;
mod speech;
mod trade;
mod trainer;
mod transfer;
