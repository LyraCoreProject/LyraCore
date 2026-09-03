//! The item pipeline: a hand-authored item definition (`game_item_template`) plus a per-player
//! owned-item row (`game_item_instance`), so a freshly-logged-in Warrior carries one starter weapon
//! ("Worn Shortsword") equipped in the main-hand slot (15, vanilla-authentic) so the client renders it
//! on the 3D character model via `PLAYER_VISIBLE_ITEM`. The wire format is `CREATE_OBJECT`,
//! `SMSG_ITEM_QUERY_SINGLE_RESPONSE`, and the `PLAYER_FIELD_INV_SLOT` descriptor. [entity]
//!
//! The instance row is born at login (when `owner_identity` is known — it can't be SQL-seeded in
//! `init`, where the owner is still `Identity::ZERO`, and an RLS filter on `owner_identity = :sender`
//! would hide it). See [[gtker-descriptor-setter-wall]]: the inventory slot is reachable via gtker's
//! typed `set_player_field_inv(ItemSlot, Guid)` (NOT walled like the aura array), so this needs no raw
//! encoder.
//!
//! This file is a thin facade over five by-concern submodules — `tables` (the two table structs + RLS
//! filter + guid/slot primitives), `rules` (the pure equip/economy taxonomy + arithmetic + their unit
//! tests), `ops` (the grant/use/loot mutation cores), `economy` (the vendor sell/buy/buyback/repair
//! cores + their shared NPC gate), and `inventory` (the move/split/equip/unequip cores + slot-space
//! vocabulary). The `pub use ...::*` re-exports keep every symbol reachable as
//! `crate::items::<sym>` regardless of which submodule defines it. The sender-path `#[reducer]` entry
//! points that used to live in a `reducers` submodule here are gone — the gateway's `gw_*`
//! reducers (`gw.rs`) are the only surviving callers of these cores.

mod economy;
mod inventory;
mod ops;
mod rules;
mod tables;

use lyracore_shared::item::ItemRefusal;

/// Refuse an item action. The human detail stays in the Module log and only the typed Refusal
/// travels, so neither tier reads prose.
pub(crate) fn refuse(refusal: ItemRefusal, detail: impl std::fmt::Display) -> ItemRefusal {
    spacetimedb::log::info!("item refused {}: {detail}", refusal.as_tag());
    refusal
}

/// Reducer edge: a reducer answers with a bare tag, which is the whole error text the Gateway reads.
pub(crate) fn refused(refusal: ItemRefusal) -> String {
    refusal.as_tag().to_string()
}

pub(crate) use economy::*; // the apply_* cores are pub(crate) (no pub items to re-export)
pub(crate) use inventory::*; // ditto
pub(crate) use ops::*; // ditto
pub use rules::*;
pub use tables::*;
