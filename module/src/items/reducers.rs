//! The thin `#[reducer]` entry points — the player-authorized item actions. Each resolves the caller's
//! live entity via `ctx.sender` then delegates to the matching `apply_*` core in `ops.rs` (the debug
//! twins drive those same cores by explicit guid for the harness, where the CLI identity owns no entity).

use spacetimedb::{reducer, ReducerContext};

use crate::helpers::entity_by_owner;

use super::ops::{
    apply_buy_item, apply_buyback_item, apply_equip_item, apply_item_move, apply_item_sell,
    apply_item_split, apply_item_use, apply_player_repair, apply_take_loot, apply_unequip_item,
};

/// Use (consume) the item in inventory `slot` — e.g. eat food to restore health. Player-authorized
/// via `ctx.sender` like every other player action; the debug `debug_use_item` drives the same
/// `apply_item_use` by explicit guid for the test harness (CLI identity owns no entity).
#[reducer]
pub fn use_item(ctx: &ReducerContext, slot: u8) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "user not in world".to_string())?;
    apply_item_use(ctx, player.guid, slot)
}

/// Sell the whole stack in inventory `slot` to a vendor for copper. Player-authorized via `ctx.sender`
/// like every other player action; `debug_sell_item` drives the same `apply_item_sell` by explicit
/// guid for the harness (CLI identity owns no entity).
#[reducer]
pub fn sell_item(ctx: &ReducerContext, vendor_guid: u64, slot: u8) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "user not in world".to_string())?;
    apply_item_sell(ctx, player.guid, vendor_guid, slot)
}

/// Re-purchase the most-recently-sold item in buyback slot `slot` (0-based; the gateway maps
/// `BuybackSlot.as_int() - 69`) from a vendor. Charges `price` copper and restores the item stack.
#[reducer]
pub fn buyback_item(ctx: &ReducerContext, vendor_guid: u64, slot: u8) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "user not in world".to_string())?;
    apply_buyback_item(ctx, player.guid, vendor_guid, slot)
}

/// Buy `count` units of `item_entry` from a vendor for copper (the inverse of `sell_item`). Player-
/// authorized via `ctx.sender` like every other player action; `debug_buy_item` drives the same
/// `apply_buy_item` by explicit guid for the harness (CLI identity owns no entity).
#[reducer]
pub fn buy_item(
    ctx: &ReducerContext,
    vendor_guid: u64,
    item_entry: u32,
    count: u32,
) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "user not in world".to_string())?;
    apply_buy_item(ctx, player.guid, vendor_guid, item_entry, count)
}

/// Split `count` units off the stack in inventory `slot` into the empty `to_slot`. Player-authorized
/// via `ctx.sender`; `debug_split_item` drives the same `apply_item_split` by explicit guid for the
/// harness.
#[reducer]
pub fn split_item(ctx: &ReducerContext, slot: u8, count: u32, to_slot: u8) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "user not in world".to_string())?;
    apply_item_split(ctx, player.guid, slot, count, to_slot)
}

/// Move (or swap) the item in inventory `from_slot` to `to_slot`. Player-authorized via `ctx.sender`;
/// `debug_move_item` drives the same `apply_item_move` by explicit guid for the harness.
#[reducer]
pub fn move_item(ctx: &ReducerContext, from_slot: u8, to_slot: u8) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "user not in world".to_string())?;
    apply_item_move(ctx, player.guid, from_slot, to_slot)
}

/// Equip the item in inventory `from_slot` into its matching equipment slot. Player-authorized via
/// `ctx.sender`; `debug_equip_item` drives the same `apply_equip_item` by explicit guid for the harness
/// (the mouse-only right-click-equip is undrivable).
#[reducer]
pub fn equip_item(ctx: &ReducerContext, from_slot: u8) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "user not in world".to_string())?;
    apply_equip_item(ctx, player.guid, from_slot)
}

/// Unequip the item in equipment `from_slot` into a free backpack slot. Player-authorized via
/// `ctx.sender`; `debug_unequip_item` drives the same `apply_unequip_item` by explicit guid.
#[reducer]
pub fn unequip_item(ctx: &ReducerContext, from_slot: u8) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "user not in world".to_string())?;
    apply_unequip_item(ctx, player.guid, from_slot)
}

/// Take one item from an open corpse's loot into the backpack (`CMSG_AUTOSTORE_LOOT_ITEM`). Player-
/// authorized via `ctx.sender` like every other player action; `debug_take_loot` drives the same
/// `apply_take_loot` by explicit guid for the harness (CLI identity owns no entity).
#[reducer]
pub fn take_loot(ctx: &ReducerContext, corpse_guid: u64, loot_slot: u8) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "user not in world".to_string())?;
    apply_take_loot(ctx, player.guid, corpse_guid, loot_slot)
}

/// Repair the item in inventory `slot` at a REPAIR-flagged NPC for copper (`CMSG_REPAIR_ITEM`).
/// Player-authorized via `ctx.sender`; `slot == ops::REPAIR_ALL` repairs the whole body (the client
/// sends one CMSG_REPAIR_ITEM per damaged item, but we also accept a repair-all for the gateway/harness).
/// `debug_repair_item` keeps driving the un-gated `apply_repair_item` by guid for the harness.
#[reducer]
pub fn repair_item(ctx: &ReducerContext, npc_guid: u64, slot: u8) -> Result<(), String> {
    let player =
        entity_by_owner(ctx, ctx.sender()).ok_or_else(|| "user not in world".to_string())?;
    apply_player_repair(ctx, player.guid, npc_guid, slot)
}
