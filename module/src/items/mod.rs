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
//! This file is a thin facade over four by-concern submodules — `tables` (the two table structs + RLS
//! filter + guid/slot primitives), `rules` (the pure equip/economy taxonomy + arithmetic + their unit
//! tests), `ops` (the `apply_*` mutation cores), and `reducers` (the thin `#[reducer]` entry points).
//! The `pub use ...::*` re-exports keep every symbol reachable as `crate::items::<sym>` regardless of
//! which submodule defines it.

mod ops;
mod reducers;
mod rules;
mod tables;

pub(crate) use ops::*; // the apply_* cores are pub(crate) (no pub items to re-export)
pub use reducers::*;
pub use rules::*;
pub use tables::*;
