//! Move / split / equip / unequip + the slot-space vocabulary and free-slot search (#387: split off
//! `ops.rs`, pure code-motion, no behavior change except the free-slot search's index-scan count — see
//! `first_free_backpack_slot`/`first_free_bag_slot`). Each `apply_*` is the shared core behind a thin
//! player reducer and its debug twin (see `reducers.rs`).

use spacetimedb::{ReducerContext, Table};

use lyracore_shared::constants::starter_item;

use super::rules::{
    binds_on_equip, can_equip_into, can_equip_proficiency, equip_slot, invtype,
    meets_required_level, merge_amount, resolve_equip_slot,
};
use super::tables::{
    game_item_instance, game_item_template, item_in_slot, next_item_guid, slot_occupied,
    ItemInstance,
};

#[allow(dead_code)] // core kept for a future gw_split_item twin (#483 deleted the sender-path reducer)
/// Shared stack-split logic for the player + debug paths: split `count` units off the stack in `slot`
/// into the empty `to_slot`, leaving the remainder in the source. Vanilla only splits a STRICT subset
/// (you can't split off the whole stack — that's a move), so `count == 0` or `count >= stack_count` is
/// rejected; the destination must be empty AND outside the equipment region (`valid_split_dest_slot`,
/// #360) — a split can never legitimately land on the body, and unlike `apply_item_move` this path runs
/// no equip-validation at all, so admitting 0..=18 here bypassed `can_equip_into`/proficiency/
/// required-level/BoE entirely. The new partial-stack row reuses the source's entry / owner /
/// durability and takes a fresh per-slot guid (`item_guid_for`) + the current timestamp. Errors if the
/// source slot is empty, the count is invalid, the destination is an equipment slot, or the destination
/// is occupied. Additive — decrements the source row and inserts one new item row. [entity]
pub(crate) fn apply_item_split(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
    count: u32,
    to_slot: u8,
) -> Result<(), String> {
    // The bank is a place, not a portable bag: either endpoint in bank space needs an open bank.
    if is_bank_slot(slot) || is_bank_slot(to_slot) {
        super::economy::bank_access_gate(ctx, player_guid)?;
    }
    let instances = ctx.db.game_item_instance();
    let mut inst =
        item_in_slot(ctx, player_guid, slot).ok_or_else(|| format!("no item in slot {slot}"))?;
    // A split must leave at least one unit in BOTH slots — splitting off none or the whole stack isn't
    // a split (the latter is a move).
    if !valid_split_count(count, inst.stack_count) {
        return Err("invalid split count".to_string());
    }
    // Reject an out-of-range destination (anti-overflow; same phantom-slot dupe vector as apply_item_move)
    // AND reject an equipment-region destination (0..=18): a split can never legitimately target the
    // body — unlike apply_item_move it runs no can_equip_into/proficiency/required-level/BoE check, so
    // admitting 0..=18 here let a modified client CMSG_SPLIT_ITEM a stack straight into an empty head or
    // off-hand slot, bypassing every gate the move path enforces for that region.
    if !valid_split_dest_slot(to_slot) {
        return Err(format!("invalid destination slot {to_slot}"));
    }
    // Bag-content destination: validate that the corresponding bag is equipped and the slot is
    // within its capacity — same phantom-slot dupe vector as apply_item_move.
    validate_bag_dest_slot(ctx, player_guid, to_slot)?;
    // The destination slot must be free; we never merge/swap on a split.
    if slot_occupied(ctx, player_guid, to_slot) {
        return Err("destination slot occupied".to_string());
    }
    inst.stack_count -= count;
    let entry = inst.entry;
    let owner_identity = inst.owner_identity;
    let durability = inst.durability;
    let soulbound = inst.soulbound; // the split half carries the SAME binding state as its source stack
    instances.guid().update(inst);
    let new_guid = next_item_guid(ctx, player_guid, to_slot);
    instances.insert(ItemInstance {
        guid: new_guid,
        entry,
        owner_identity,
        owner_guid: player_guid,
        slot: to_slot,
        stack_count: count,
        durability,
        created_at: ctx.timestamp,
        enchant_id: 0, // a split is only ever on a stackable (non-equippable) item → never enchanted
        soulbound,
    });
    Ok(())
}

/// Shared move/swap logic for the player + debug paths: move the item in `from_slot` to `to_slot`.
/// If `to_slot` holds an item too, the two SWAP slots; if it's empty, the item just moves. The item
/// GUID is its stable identity (only `slot` changes) — `item_guid_for`'s guid↔slot derivation is a
/// grant-time convenience, not an invariant the client relies on after that (it tracks items by guid).
/// One EXCEPTION to the swap (FEATURE B): if the destination holds the SAME stackable item
/// (`dst.entry == src.entry` and the template's `max_stack > 1`), the stacks MERGE instead of swapping
/// — `merge_amount` units flow from src into dst (capped by dst's headroom); src is deleted if drained,
/// else left with the remainder. A non-matching or non-stackable destination keeps the SWAP byte-for-byte.
/// Errors if `from_slot` is empty. A no-op when the slots are equal. Additive — touches only the two
/// item rows' `slot`/`stack_count` (and may delete the drained src on a full merge). [entity]
pub(crate) fn apply_item_move(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
    to_slot: u8,
) -> Result<(), String> {
    if from_slot == to_slot {
        return Ok(());
    }
    // The bank is a place, not a portable bag: either endpoint in bank space needs an open bank.
    if is_bank_slot(from_slot) || is_bank_slot(to_slot) {
        super::economy::bank_access_gate(ctx, player_guid)?;
    }
    let instances = ctx.db.game_item_instance();
    let mut src = item_in_slot(ctx, player_guid, from_slot)
        .ok_or_else(|| format!("no item in slot {from_slot}"))?;
    // Reject an out-of-range destination. Valid slots: equipment 0..=18, bag-equip 19..=22,
    // backpack 23..=38, or a bag-content slot (120..=191) for items landing inside an equipped bag.
    // Anything outside these ranges (e.g. 39..=119 = bank/keyring we don't model, or 192..255) is
    // an inventory-overflow dupe vector from a modified client and is rejected.
    if !valid_dest_slot(to_slot) {
        return Err(format!("invalid destination slot {to_slot}"));
    }
    // If the destination is in the bag-content region, validate that the corresponding bag is
    // equipped and the slot is within its capacity (prevents stashing items in unequipped bags).
    validate_bag_dest_slot(ctx, player_guid, to_slot)?;
    // Equip-validation: moving INTO any EQUIPMENT slot (0..=18) requires the SOURCE item's
    // `inventory_type` map to that slot (`can_equip_into`). A missing template fails closed (we can't
    // prove it's wearable). Slot 15 (main-hand) defers to the stricter `can_equip_mainhand` rule inside
    // `can_equip_into`; the other 18 slots use the general resolver. Non-equipment destinations (>18:
    // bag/backpack/bank) don't reach this branch — the move/swap/merge below run unconditionally.
    if to_slot <= equip_slot::END {
        // Dual Wield: a caster who has LEARNED spell 674 may equip a second one-hander into OFFHAND —
        // `can_equip_into` only accepts that combination when this is true.
        let can_dual_wield = crate::spell::knows_spell(
            ctx,
            player_guid,
            lyracore_shared::constants::dual_wield::SPELL_ID,
        );
        let tmpl = match ctx.db.game_item_template().entry().find(src.entry) {
            Some(tmpl)
                if can_equip_into(tmpl.class, tmpl.inventory_type, to_slot, can_dual_wield) =>
            {
                tmpl
            }
            _ => return Err(format!("cannot equip that item in slot {to_slot}")),
        };
        // Required-level gate: you can carry a too-high item in the bag, but can't EQUIP it. Read the
        // player entity just for its level here (the move path doesn't otherwise need it); a missing
        // entity fails closed. Seeded items are required_level 1, so this never trips for the loadout.
        let player = crate::helpers::live_entity(ctx, player_guid)
            .map_err(|_| "user not in world".to_string())?;
        if !meets_required_level(player.level, tmpl.required_level) {
            return Err(format!("requires level {}", tmpl.required_level));
        }
        // Proficiency gate: enforce class armor/weapon restrictions (e.g. a Mage can't equip
        // plate). The class is byte 1 of unit_bytes_0 (race | class<<8 | gender<<16 | power<<24).
        // Creatures (class 0) never call equip_item; fail closed for unknown classes.
        let player_class = player.class();
        if !can_equip_proficiency(player_class, tmpl.class, tmpl.subclass) {
            return Err(format!(
                "class {} lacks proficiency for item class {}/subclass {}",
                player_class, tmpl.class, tmpl.subclass
            ));
        }
        // BoE: a Bind-on-Equip item binds the FIRST time it lands on the body — not on pickup. Every
        // equip lands here (apply_equip_item and a direct manual move both route through this branch),
        // so this is the single BoE binding trigger. Idempotent: an already-bound item just stays
        // bound (no-op re-equip / re-swap).
        if binds_on_equip(tmpl.bonding) {
            src.soulbound = true;
        }
    }
    // If the destination is occupied, either MERGE (same stackable item) or SWAP (anything else).
    if let Some(mut dst) = item_in_slot(ctx, player_guid, to_slot) {
        // MERGE only when dropping onto the SAME entry AND the item is stackable (max_stack > 1).
        // A missing template can't be merged (we can't know max_stack) — fall through to SWAP, which
        // needs no template, so the move never wedges on unseeded item data.
        if dst.entry == src.entry {
            if let Some(tmpl) = ctx.db.game_item_template().entry().find(src.entry) {
                if tmpl.max_stack > 1 {
                    let moved = merge_amount(src.stack_count, dst.stack_count, tmpl.max_stack);
                    if moved > 0 {
                        dst.stack_count += moved;
                        instances.guid().update(dst);
                        if moved >= src.stack_count {
                            // The whole source stack flowed into dst → remove the now-empty src row.
                            instances.guid().delete(src.guid);
                        } else {
                            // Partial merge (dst hit max_stack) → leave the remainder in src, in place.
                            src.stack_count -= moved;
                            instances.guid().update(src);
                        }
                    }
                    // `moved == 0` (dst already full) is a no-op: both stacks stay exactly as they were,
                    // which matches vanilla refusing to merge into a full stack (no swap, no error).
                    return Ok(());
                }
            }
        }
        // Not a same-item stackable pair → the original SWAP: dst falls back into the source slot.
        dst.slot = from_slot;
        instances.guid().update(dst);
    }
    src.slot = to_slot;
    instances.guid().update(src);
    // Parity #8: if either endpoint is an EQUIPMENT slot (0..=18), gear just changed on the body, so
    // re-derive the owner's max HP/mana (recompute_vitals now folds equipped Stamina/Intellect). The
    // health bar grows when you equip a +Sta piece and shrinks when you take it off. A pure bag↔bag move
    // touches no equip slot → skipped, so loose-inventory shuffles are byte-identical. (recompute_vitals
    // is a no-op for non-players + when the derived max is unchanged.)
    if from_slot <= equip_slot::END || to_slot <= equip_slot::END {
        crate::spell::recompute_vitals(ctx, player_guid);
        crate::spell::recompute_sheet(ctx, player_guid);
    }
    Ok(())
}

/// Shared EQUIP logic for the player + debug paths: take the item in backpack/inventory `from_slot` and
/// equip it into the correct `EQUIPMENT_SLOT_*` for its `inventory_type` (auto-resolved, including the
/// first-free of a finger/trinket pair). Rejects an item whose type isn't equippable (`inventory_type`
/// has no slot — e.g. food/junk). Delegates the actual placement/swap (and the required-level gate) to
/// the shared `apply_item_move`, so equipping is exactly "a move into the resolved equip slot": an empty
/// target just receives the item; an occupied target SWAPS its resident back into `from_slot`. Additive
/// — touches only the item rows' `slot` (no new fields). Errors if `from_slot` is empty, the template is
/// missing, or the item isn't equippable. [entity]
pub(crate) fn apply_equip_item(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
) -> Result<(), String> {
    let src = item_in_slot(ctx, player_guid, from_slot)
        .ok_or_else(|| format!("no item in slot {from_slot}"))?;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(src.entry)
        .ok_or_else(|| format!("no template for item entry {}", src.entry))?;
    // Bags (INVTYPE_BAG) equip into bag-equip slots 19..=22, not the equipment region 0..=18.
    // Route them separately: find the first free bag-equip slot and move the bag there. A bag
    // already in the bag slots (dragged manually) won't come through here, but autoequip does.
    if tmpl.inventory_type == invtype::BAG {
        let to_slot = first_free_bag_equip_slot(ctx, player_guid)
            .ok_or_else(|| "all four bag slots are full".to_string())?;
        return apply_item_move(ctx, player_guid, from_slot, to_slot);
    }
    // Dual Wield: redirect a second one-hander to OFFHAND (instead of swapping MAINHAND) when the
    // caster has learned spell 674 — see `resolve_equip_slot`'s doc comment.
    let can_dual_wield = crate::spell::knows_spell(
        ctx,
        player_guid,
        lyracore_shared::constants::dual_wield::SPELL_ID,
    );
    // Snapshot which equip slots are occupied so the pair-resolver can prefer a free finger/trinket.
    let to_slot = resolve_equip_slot(tmpl.inventory_type, can_dual_wield, |s| {
        slot_occupied(ctx, player_guid, s)
    })
    .ok_or_else(|| format!("item {} is not equippable", src.entry))?;
    // Equip == a validated move into the resolved equip slot (reuses the equip-validation + swap there).
    apply_item_move(ctx, player_guid, from_slot, to_slot)
}

/// Shared UNEQUIP logic for the player + debug paths: take the item in equipment `from_slot` (0..=18)
/// and move it to the first free backpack slot. Rejects a non-equipment source slot (it's not equipped)
/// or a full backpack. Delegates to `apply_item_move` so the placement is the exact same move primitive;
/// the destination is a backpack slot, so the equip-validation branch there is a no-op for it. Additive.
pub(crate) fn apply_unequip_item(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
) -> Result<(), String> {
    if from_slot > equip_slot::END {
        return Err(format!("slot {from_slot} is not an equipment slot"));
    }
    // Must actually hold an equipped item to unequip.
    if !slot_occupied(ctx, player_guid, from_slot) {
        return Err(format!("no item equipped in slot {from_slot}"));
    }
    let free =
        first_free_backpack_slot(ctx, player_guid).ok_or_else(|| "inventory full".to_string())?;
    apply_item_move(ctx, player_guid, from_slot, free)
}

/// The loose-inventory ("backpack") slot range in this minimal model: 16 slots, 23..=38, mirroring
/// `starter_item::BACKPACK_SLOT_0` (23). Equip slots (0..=18) and bag-container slots aren't loose
/// storage, so auto-store only ever lands an item in the backpack. Expressed as an exclusive 23..39.
const BACKPACK_SLOT_END: u8 = starter_item::BACKPACK_SLOT_0 + 16; // 39

/// First of the four bag-equip slots (the bags themselves live here; NOT their contents).
const BAG_SLOT_START: u8 = 19;
/// Last bag-equip slot (inclusive). Vanilla has four: 19..=22.
const BAG_SLOT_END_INCL: u8 = 22;
/// Number of bag-equip slots.
const BAG_SLOT_COUNT: u8 = 4;
/// Base of the flat bag-content slot space. All values from here to `BAG_CONTENT_END - 1` are
/// used exclusively for items stored INSIDE equipped bags. The range starts above the largest
/// vanilla `ItemSlot` ordinal (Keyring32 = 112), so `ItemSlot::try_from(slot)` returns `Err`
/// for bag-content slots — the gateway correctly sends no `PLAYER_FIELD_INV_SLOT` pointer for
/// them (items in bags are tracked via the container object, not the player descriptor).
const BAG_CONTENT_OFFSET: u8 = 120;
/// Maximum container slots any bag can hold — vanilla's largest (Portable Hole) is 18 slots.
/// Each bag-equip position (19..=22) is assigned this many flat content slots.
const MAX_BAG_SIZE: u8 = 18;
/// Exclusive upper bound of the bag-content region: 120 + 4 × 18 = 192. Slots 120..=191.
const BAG_CONTENT_END: u8 = BAG_CONTENT_OFFSET + BAG_SLOT_COUNT * MAX_BAG_SIZE; // 192

/// The anti-dupe destination-slot gate shared by `apply_item_move` and `apply_item_split`: a valid
/// destination is the flat equip+bag-equip+backpack range (0..=38), the base bank range (39..=62), or
/// a bag-content slot (120..=191). Anything else (the bank-bag/buyback/keyring ordinals 63..=119 we
/// don't model, or 192..255) is an inventory-overflow dupe vector from a modified client and is
/// rejected. Extracted (pure code-motion, deduplicating the two identical inline expressions) so the
/// slot-range boundaries are unit-tested without a live module.
pub(crate) fn valid_dest_slot(to_slot: u8) -> bool {
    to_slot < BACKPACK_SLOT_END // 0..=38 (equip + bag-equip + backpack)
        || is_bank_slot(to_slot) // 39..=62 (the 24 base bank slots)
        || (BAG_CONTENT_OFFSET..BAG_CONTENT_END).contains(&to_slot) // 120..=191
}

/// First base bank slot (vanilla `ItemSlot::Bank1`).
const BANK_SLOT_START: u8 = 39;
/// Last base bank slot (`ItemSlot::Bank24`), 24 slots in total.
const BANK_SLOT_END_INCL: u8 = 62;

/// Whether a slot is one of the 24 base bank slots. Bank bag slots (63..=68) are NOT bank storage
/// here — their contents have no slot addresses, so they stay refused everywhere.
pub(crate) fn is_bank_slot(slot: u8) -> bool {
    (BANK_SLOT_START..=BANK_SLOT_END_INCL).contains(&slot)
}

/// Whether a slot is CARRIED — equipment, bag-equip, backpack, or bag content — as opposed to merely
/// owned. Bank slots are owned but not carried: they don't count for collect quests, aren't consumed
/// on turn-in, aren't fired as ammo, and never receive auto-stored loot. Stated as the carried ranges
/// rather than "not the bank", so the bank-bag region stays uncarried when it opens.
pub(crate) fn is_carried_slot(slot: u8) -> bool {
    slot < BACKPACK_SLOT_END // 0..=38 (equip + bag-equip + backpack)
        || (BAG_CONTENT_OFFSET..BAG_CONTENT_END).contains(&slot) // 120..=191
}

#[allow(dead_code)] // core kept for a future gw_split_item twin (#483 deleted the sender-path reducer)
/// The destination-slot gate for `apply_item_split` ONLY: everything `valid_dest_slot` admits, MINUS
/// the equipment region (0..=`equip_slot::END`, i.e. 0..=18). A split can never legitimately place an
/// item on the body — `apply_item_move` is the only path that runs equip-validation
/// (`can_equip_into`/proficiency/required-level/BoE), and a split has no source-template check at all,
/// so admitting 0..=18 here was a total bypass of that region's gates. Splits only ever land in
/// bag-equip/backpack/bag-content space.
pub(crate) fn valid_split_dest_slot(to_slot: u8) -> bool {
    valid_dest_slot(to_slot) && to_slot > equip_slot::END
}

/// Decompose a bag-content slot (120..=191) into `(bag_idx, slot_in_bag)`: which of the four equipped
/// bags (0..=3) and which position within it. Extracted from `apply_item_move` / `apply_item_split`
/// (pure code-motion, deduplicating the two identical inline expressions) — the caller adds `bag_idx` to
/// `BAG_SLOT_START` to find the bag's equip slot. Only meaningful for `to_slot >= BAG_CONTENT_OFFSET`
/// (the caller guards that); this does not itself validate the range.
pub(crate) fn bag_content_decompose(to_slot: u8) -> (u8, u8) {
    let offset = to_slot - BAG_CONTENT_OFFSET;
    (offset / MAX_BAG_SIZE, offset % MAX_BAG_SIZE)
}

/// The bag-content CAPACITY gate shared by `apply_item_move` and `apply_item_split` (issue #372): once
/// `to_slot` has already passed `valid_dest_slot`/`valid_split_dest_slot`, a bag-content destination
/// (120..=191) additionally needs the corresponding bag-equip slot to actually hold an equipped bag
/// whose template covers `slot_in_bag`. A no-op (`Ok(())`) for a non-bag-content destination — the
/// caller only reaches here after its own range gate already accepted `to_slot`. Anti-dupe: a modified
/// client can otherwise `CMSG_SPLIT_ITEM`/`CMSG_MOVE_ITEM` straight to slot 120 with no bag equipped in
/// slot 19, creating an orphaned item row that is invisible and can never be freed by normal play.
/// Extracted (pure code-motion, deduplicating the two byte-identical inline blocks) — every error
/// string below is preserved verbatim from both call sites.
pub(crate) fn validate_bag_dest_slot(
    ctx: &ReducerContext,
    player_guid: u64,
    to_slot: u8,
) -> Result<(), String> {
    if to_slot < BAG_CONTENT_OFFSET {
        return Ok(());
    }
    let (bag_idx, slot_in_bag) = bag_content_decompose(to_slot);
    let bag_equip_slot = BAG_SLOT_START + bag_idx;
    let bag_inst = item_in_slot(ctx, player_guid, bag_equip_slot)
        .ok_or_else(|| format!("no bag equipped in slot {bag_equip_slot}"))?;
    let bag_tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(bag_inst.entry)
        .ok_or_else(|| "equipped bag has no template".to_string())?;
    if slot_in_bag >= bag_tmpl.container_slots.min(MAX_BAG_SIZE) {
        return Err(format!(
            "slot {} out of range for bag with {} slots",
            slot_in_bag, bag_tmpl.container_slots
        ));
    }
    Ok(())
}

#[allow(dead_code)] // core kept for a future gw_split_item twin (#483 deleted the sender-path reducer)
/// A split must leave at least one unit in BOTH the source and the new stack — splitting off none
/// (`count == 0`) or the whole stack (`count >= stack_count`, that's a move) is rejected. Extracted from
/// `apply_item_split` (pure code-motion) so the count-gate boundaries are unit-tested without a live
/// module.
pub(crate) fn valid_split_count(count: u32, stack_count: u32) -> bool {
    count != 0 && count < stack_count
}

/// First backpack slot (23..39) not occupied by any of the player's items, or `None` if the backpack
/// is full. Collects the owner's occupied slots into a set ONCE (#387 smalls: was one `by_owner_guid`
/// index scan PER CANDIDATE slot — up to 16 full scans for a nearly-full backpack) then does a plain
/// membership check per candidate. Vanilla auto-store fills the first free bag slot.
/// The occupied-slot set behind the two backpack probes below — one spelling of the scan.
fn occupied_slots(ctx: &ReducerContext, player_guid: u64) -> std::collections::HashSet<u8> {
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .map(|i| i.slot)
        .collect()
}

/// Number of free carry slots across the backpack and all equipped bags.
pub(crate) fn count_free_inventory_slots(ctx: &ReducerContext, player_guid: u64) -> u32 {
    let templates = ctx.db.game_item_template();
    let owned: Vec<ItemInstance> = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .collect();
    let occupied: std::collections::HashSet<u8> = owned.iter().map(|i| i.slot).collect();
    let backpack = (starter_item::BACKPACK_SLOT_0..BACKPACK_SLOT_END)
        .filter(|slot| !occupied.contains(slot))
        .count() as u32;
    let bags = (0..BAG_SLOT_COUNT)
        .filter_map(|bag_idx| {
            let bag = owned.iter().find(|i| i.slot == BAG_SLOT_START + bag_idx)?;
            let capacity = templates
                .entry()
                .find(bag.entry)?
                .container_slots
                .min(MAX_BAG_SIZE);
            let base = BAG_CONTENT_OFFSET + bag_idx * MAX_BAG_SIZE;
            Some(
                (0..capacity)
                    .filter(|offset| !occupied.contains(&(base + offset)))
                    .count() as u32,
            )
        })
        .sum::<u32>();
    backpack + bags
}

pub(crate) fn first_free_backpack_slot(ctx: &ReducerContext, player_guid: u64) -> Option<u8> {
    let occupied = occupied_slots(ctx, player_guid);
    (starter_item::BACKPACK_SLOT_0..BACKPACK_SLOT_END).find(|slot| !occupied.contains(slot))
}

/// First free bag-equip slot (19..=22) not currently holding a bag, or `None` if all four are
/// occupied. Used by `apply_equip_item` to auto-resolve where to drop a bag. Pure scan.
fn first_free_bag_equip_slot(ctx: &ReducerContext, player_guid: u64) -> Option<u8> {
    (BAG_SLOT_START..=BAG_SLOT_END_INCL).find(|&slot| !slot_occupied(ctx, player_guid, slot))
}

/// First free content slot across all equipped bags (19..=22), scanning in bag-equip order (bag 1
/// first). Returns `None` if every equipped bag is full or no bags are equipped. Used by
/// `store_item` as the fallback after the 16-slot backpack is exhausted — bags thus act as
/// overflow storage. A bag equip slot with no item is skipped; a bag whose template is missing
/// or has `container_slots == 0` (not a real bag) is also skipped. Collects the owner's rows ONCE
/// (#387 smalls, same fix as `first_free_backpack_slot`) instead of one index scan per candidate
/// content slot — up to 18 scans per bag before this. [entity]
pub(crate) fn first_free_bag_slot(ctx: &ReducerContext, player_guid: u64) -> Option<u8> {
    let templates = ctx.db.game_item_template();
    let owned: Vec<ItemInstance> = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .collect();
    let occupied: std::collections::HashSet<u8> = owned.iter().map(|i| i.slot).collect();
    for bag_idx in 0..BAG_SLOT_COUNT {
        let bag_equip_slot = BAG_SLOT_START + bag_idx;
        let Some(bag_inst) = owned.iter().find(|i| i.slot == bag_equip_slot) else {
            continue;
        };
        let Some(bag_tmpl) = templates.entry().find(bag_inst.entry) else {
            continue;
        };
        let slots_in_bag = bag_tmpl.container_slots.min(MAX_BAG_SIZE);
        if slots_in_bag == 0 {
            continue;
        }
        let base = BAG_CONTENT_OFFSET + bag_idx * MAX_BAG_SIZE;
        for si in 0..slots_in_bag {
            let content_slot = base + si;
            if !occupied.contains(&content_slot) {
                return Some(content_slot);
            }
        }
    }
    None
}

/// First bank slot (39..=62) not occupied by any of the player's items, or `None` if the bank is
/// full. Same one-scan shape as `first_free_backpack_slot` — collect the owner's occupied slots
/// once, then a plain membership check per candidate.
pub(crate) fn first_free_bank_slot(ctx: &ReducerContext, player_guid: u64) -> Option<u8> {
    let occupied: std::collections::HashSet<u8> = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .map(|i| i.slot)
        .collect();
    (BANK_SLOT_START..=BANK_SLOT_END_INCL).find(|slot| !occupied.contains(slot))
}

/// Shared auto-bank/auto-store-bank core for the player + debug paths (right-click to bank, right-
/// click to withdraw): the direction is inferred from `slot` itself rather than taken as an argument
/// — a bank slot withdraws to the first free carry slot (backpack, then bag space, the same fallback
/// `store_item` uses for loot); anything else deposits to the first free bank slot. Delegates the
/// placement to `apply_item_move`, so both directions get the banker-proximity gate for free.
pub(crate) fn apply_auto_bank_item(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
) -> Result<(), String> {
    let to_slot = if is_bank_slot(slot) {
        first_free_backpack_slot(ctx, player_guid)
            .or_else(|| first_free_bag_slot(ctx, player_guid))
            .ok_or_else(|| "inventory full".to_string())?
    } else {
        first_free_bank_slot(ctx, player_guid).ok_or_else(|| "bank full".to_string())?
    };
    apply_item_move(ctx, player_guid, slot, to_slot)
}

#[cfg(test)]
mod tests {
    use super::{
        bag_content_decompose, equip_slot, is_bank_slot, is_carried_slot, valid_dest_slot,
        valid_split_count, valid_split_dest_slot, BANK_SLOT_END_INCL, BANK_SLOT_START,
    };
    use crate::test_scan::code_of;

    /// BANK SLOT RANGE: exactly the 24 base bank slots (39..=62). The bank-bag ordinals just past them
    /// (63..=68) are not bank storage in this model.
    #[test]
    fn is_bank_slot_covers_the_24_base_slots_only() {
        for slot in BANK_SLOT_START..=BANK_SLOT_END_INCL {
            assert!(is_bank_slot(slot), "slot {slot} is a base bank slot");
        }
        for slot in [0u8, 38, 63, 68, 119, 120, 191, 192, 255] {
            assert!(!is_bank_slot(slot), "slot {slot} is not a base bank slot");
        }
    }

    /// BANK ACCESS GATE WIRING: both mutation paths must consult `bank_access_gate` when either
    /// endpoint is a bank slot, or the bank becomes a portable 24-slot bag. There is no
    /// `ReducerContext` harness in this crate, so the call's PRESENCE is pinned by a source scan; the
    /// gate's own decision is asserted directly on `economy::banker_in_reach`.
    #[test]
    fn both_mutation_paths_gate_bank_endpoints_on_an_open_bank() {
        let src = include_str!("inventory.rs");
        for signature in [
            "pub(crate) fn apply_item_move(",
            "pub(crate) fn apply_item_split(",
        ] {
            let body = code_of(src, signature);
            assert!(
                body.contains("bank_access_gate(ctx, player_guid)?"),
                "`{signature}` must refuse a bank endpoint without an open bank"
            );
            assert!(
                body.contains("is_bank_slot(to_slot)"),
                "`{signature}` must test its destination slot for bank space"
            );
        }
    }

    /// FREE-BANK-SLOT SEARCH SHAPE: like `first_free_backpack_slot`, `first_free_bank_slot` must
    /// collect the owner's occupied slots into a set ONCE rather than re-scanning per candidate slot.
    /// There is no `ReducerContext` harness in this crate, so the shape is pinned by a source scan.
    #[test]
    fn first_free_bank_slot_scans_the_owners_rows_once() {
        let src = include_str!("inventory.rs");
        let body = code_of(src, "pub(crate) fn first_free_bank_slot(");
        assert_eq!(
            body.matches("by_owner_guid()").count(),
            1,
            "first_free_bank_slot must collect the owner's rows once, not scan per candidate slot"
        );
    }

    /// AUTO-BANK DIRECTION INFERENCE: `apply_auto_bank_item` decides deposit vs. withdraw from the
    /// SOURCE slot alone. Pinned by source scan (no `ReducerContext` harness): a bank source resolves
    /// against the carry-space search pair; anything else resolves against the bank search.
    #[test]
    fn apply_auto_bank_item_infers_direction_from_the_source_slot() {
        let src = include_str!("inventory.rs");
        let body = code_of(src, "pub(crate) fn apply_auto_bank_item(");
        assert!(
            body.contains("is_bank_slot(slot)"),
            "must branch on whether the source is a bank slot"
        );
        assert!(
            body.contains("first_free_backpack_slot(ctx, player_guid)")
                && body.contains("first_free_bag_slot(ctx, player_guid)"),
            "withdraw must fall back from the backpack to bag space, like loot auto-store"
        );
        assert!(
            body.contains("first_free_bank_slot(ctx, player_guid)"),
            "deposit must resolve against the free-bank-slot search"
        );
    }

    /// CARRIED-SLOT PREDICATE: equipment, bag-equip, backpack, and bag-content slots are carried; the
    /// base bank range is owned but not carried, and so is every slot past it (the bank-bag region and
    /// the unaddressed tail).
    #[test]
    fn is_carried_slot_admits_the_two_carry_regions_only() {
        for slot in [0u8, 18, 19, 22, 23, 38, 120, 191] {
            assert!(is_carried_slot(slot), "slot {slot} is carried");
        }
        for slot in BANK_SLOT_START..=BANK_SLOT_END_INCL {
            assert!(!is_carried_slot(slot), "bank slot {slot} is not carried");
        }
        for slot in [63u8, 68, 119, 192, 255] {
            assert!(!is_carried_slot(slot), "slot {slot} is not carry space");
        }
    }

    /// SPLIT COUNT GATE: `count == 0` and `count == stack_count` (splitting off nothing, or the whole
    /// stack — that's a move) are rejected; every count strictly between 0 and the stack passes.
    #[test]
    fn valid_split_count_rejects_zero_and_the_whole_stack_only() {
        const STACK: u32 = 5;
        assert!(!valid_split_count(0, STACK)); // splitting off nothing isn't a split
        assert!(!valid_split_count(STACK, STACK)); // splitting off the whole stack is a move, not a split
        assert!(!valid_split_count(STACK + 1, STACK)); // over the stack is never valid either
        for count in 1..STACK {
            assert!(
                valid_split_count(count, STACK),
                "count {count} of {STACK} should be a valid split"
            );
        }
    }

    /// ANTI-DUPE DESTINATION-SLOT GATE (`apply_item_move` / `apply_item_split`): the equip+bag-equip+
    /// backpack range (0..=38), the base bank range (39..=62), and the bag-content range (120..=191) are
    /// valid; the unmodeled bank-bag/buyback/keyring gap (63..=119) and anything past the bag-content
    /// region (192+) are rejected.
    #[test]
    fn valid_dest_slot_admits_the_three_modeled_ranges_only() {
        // Inside 0..=38 (equip 0..=18, bag-equip 19..=22, backpack 23..=38).
        for slot in [0u8, 18, 19, 22, 23, 38] {
            assert!(valid_dest_slot(slot), "slot {slot} is in the 0..=38 range");
        }
        // The 24 base bank slots.
        for slot in BANK_SLOT_START..=BANK_SLOT_END_INCL {
            assert!(valid_dest_slot(slot), "bank slot {slot} is a valid dest");
        }
        // Bank bag slots and the rest of the unmodeled gap stay refused.
        assert!(!valid_dest_slot(63));
        assert!(!valid_dest_slot(68));
        assert!(!valid_dest_slot(119));
        // Inside the bag-content region 120..=191.
        assert!(valid_dest_slot(120));
        assert!(valid_dest_slot(191));
        // Just past the bag-content region.
        assert!(!valid_dest_slot(192));
        assert!(!valid_dest_slot(255));
    }

    /// SPLIT DESTINATION GATE (#360 regression): a split can never legitimately target the body, so
    /// `valid_split_dest_slot` must refuse every equipment-region slot (0..=18) even though
    /// `valid_dest_slot` alone admits it (0..=38 is the modeled equip+bag-equip+backpack range). Every
    /// non-equipment slot `valid_dest_slot` admits stays admitted.
    #[test]
    fn valid_split_dest_slot_refuses_the_equipment_region() {
        // The whole equipment region (0..=18, e.g. HEAD=0..=TABARD=18) is valid for a plain move/dest
        // check but MUST be refused for a split — this is the exact bypass #360 reports.
        for slot in 0u8..=equip_slot::END {
            assert!(
                valid_dest_slot(slot),
                "slot {slot} should still be in the general 0..=38 range"
            );
            assert!(
                !valid_split_dest_slot(slot),
                "slot {slot} is in the equipment region and must be refused as a split destination"
            );
        }
        // Bag-equip, backpack, bank, and bag-content slots (non-equipment) stay valid split destinations.
        for slot in [19u8, 22, 23, 38, 39, 62, 120, 191] {
            assert!(
                valid_split_dest_slot(slot),
                "slot {slot} is outside the equipment region and should remain a valid split destination"
            );
        }
        // The already-invalid ranges stay invalid.
        assert!(!valid_split_dest_slot(63));
        assert!(!valid_split_dest_slot(119));
        assert!(!valid_split_dest_slot(192));
    }

    /// BAG-CONTENT SLOT DECOMPOSITION: a flat bag-content slot (120..=191) decomposes into
    /// `(bag_idx, slot_in_bag)` — 18 slots per bag, in bag-equip order.
    #[test]
    fn bag_content_decompose_maps_flat_slot_to_bag_index_and_position() {
        assert_eq!(bag_content_decompose(120), (0, 0)); // first slot of the first bag
        assert_eq!(bag_content_decompose(137), (0, 17)); // last slot of the first bag (18 slots: 120..=137)
        assert_eq!(bag_content_decompose(138), (1, 0)); // first slot of the second bag
        assert_eq!(bag_content_decompose(191), (3, 17)); // last slot of the fourth (last) bag
    }
}
