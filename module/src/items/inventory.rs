//! Move / split / equip / unequip + the slot-space vocabulary and free-slot search (split off
//! `ops.rs`, pure code-motion, no behavior change except the free-slot search's index-scan count — see
//! `first_free_backpack_slot`/`first_free_bag_slot`). Each `apply_*` is the shared core behind a thin
//! player reducer and its debug twin (see `reducers.rs`).

use spacetimedb::{ReducerContext, Table};

use lyracore_shared::constants::starter_item;
use lyracore_shared::item::ItemRefusal;

use super::{next_item_guid, refuse, refused};

use super::rules::{
    binds_on_equip, can_equip_into, eligibility_mask_allows, equip_slot, invtype,
    meets_required_level, meets_required_reputation, meets_required_skill, merge_amount,
    resolve_equip_slot, Proficiency,
};
use super::tables::{
    game_item_instance, game_item_template, item_in_slot, slot_occupied, ItemInstance,
};
use crate::{game_player_reputation, game_player_skill};

fn bag_has_contents(ctx: &ReducerContext, character_guid: u64, slot: u8) -> bool {
    if !(BAG_SLOT_START..=BAG_SLOT_END_INCL).contains(&slot) {
        return false;
    }
    let start = BAG_CONTENT_OFFSET + (slot - BAG_SLOT_START) * MAX_BAG_SIZE;
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&character_guid)
        .any(|item| (start..start + MAX_BAG_SIZE).contains(&item.slot))
}

/// Destroy only the addressed owned stack. Zero means the whole stack.
pub(crate) fn apply_item_destroy(
    ctx: &ReducerContext,
    character_guid: u64,
    slot: u8,
    count: u32,
) -> Result<(), ItemRefusal> {
    let character = crate::helpers::live_entity(ctx, character_guid)
        .map_err(|_| refuse(ItemRefusal::Internal, "Character is not in world"))?;
    if character.health == 0 {
        return Err(ItemRefusal::PlayerDead);
    }
    if is_bank_slot(slot) {
        bank_access(ctx, character_guid)?;
    }
    let mut item = item_in_slot(ctx, character_guid, slot).ok_or(ItemRefusal::ItemNotFound)?;
    if crate::trade::item_is_offered(ctx, character_guid, item.guid) {
        return Err(ItemRefusal::NotRightNow);
    }
    let template = ctx
        .db
        .game_item_template()
        .entry()
        .find(item.entry)
        .ok_or(ItemRefusal::ItemNotFound)?;
    if template.item_flags & 0x20 != 0 {
        return Err(ItemRefusal::Indestructible);
    }
    if bag_has_contents(ctx, character_guid, slot) {
        return Err(ItemRefusal::BagNotEmpty);
    }
    if count == 0 || count >= item.stack_count {
        ctx.db.game_item_instance().guid().delete(item.guid);
    } else {
        item.stack_count -= count;
        ctx.db.game_item_instance().guid().update(item);
    }
    if slot <= equip_slot::END {
        crate::spell::recompute_vitals(ctx, character_guid);
        crate::spell::recompute_sheet(ctx, character_guid);
    }
    Ok(())
}

/// Split a proper subset into empty storage, preserving ownership and binding state.
/// Equipment and bag equipment slots cannot receive a split stack.
pub(crate) fn apply_item_split(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
    count: u32,
    to_slot: u8,
) -> Result<(), ItemRefusal> {
    // The bank is a place, not a portable bag: either endpoint in bank space needs an open bank.
    if is_bank_slot(slot) || is_bank_slot(to_slot) {
        bank_access(ctx, player_guid)?;
    }
    let instances = ctx.db.game_item_instance();
    let mut inst = item_in_slot(ctx, player_guid, slot)
        .ok_or_else(|| refuse(ItemRefusal::ItemNotFound, format!("no item in slot {slot}")))?;
    if crate::trade::item_is_offered(ctx, player_guid, inst.guid) {
        return Err(ItemRefusal::NotRightNow);
    }
    // A split must leave at least one unit in BOTH slots — splitting off none or the whole stack isn't
    // a split (the latter is a move).
    if !valid_split_count(count, inst.stack_count) {
        return Err(refuse(ItemRefusal::WrongSlot, "invalid split count"));
    }
    if !valid_split_dest_slot(to_slot) {
        return Err(refuse(
            ItemRefusal::WrongSlot,
            format!("invalid destination slot {to_slot}"),
        ));
    }
    // Bag-content destination: validate that the corresponding bag is equipped and the slot is
    // within its capacity — same phantom-slot dupe vector as apply_item_move.
    validate_bag_dest_slot(ctx, player_guid, to_slot)?;
    // The destination slot must be free; we never merge/swap on a split.
    if slot_occupied(ctx, player_guid, to_slot) {
        return Err(refuse(ItemRefusal::WrongSlot, "destination slot occupied"));
    }
    let new_guid = next_item_guid(ctx).map_err(|detail| refuse(ItemRefusal::Internal, detail))?;
    inst.stack_count -= count;
    let entry = inst.entry;
    let owner_identity = inst.owner_identity;
    let durability = inst.durability;
    let random_property_id = inst.random_property_id;
    let soulbound = inst.soulbound; // the split half carries the SAME binding state as its source stack
    let item_text_id = inst.item_text_id; // ditto — a split half stays as readable as its source
    instances.guid().update(inst);
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
        random_property_id,
        item_text_id,
    });
    Ok(())
}

/// Move, swap or merge two owned stacks after both destinations pass their Gates.
/// Nonempty equipped bags stay in place until contents can follow a bag-slot change.
pub(crate) fn apply_item_move(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
    to_slot: u8,
) -> Result<(), ItemRefusal> {
    if from_slot == to_slot {
        return Ok(());
    }
    // Bank access applies to either endpoint.
    if is_bank_slot(from_slot) || is_bank_slot(to_slot) {
        bank_access(ctx, player_guid)?;
    }
    let instances = ctx.db.game_item_instance();
    let mut src = item_in_slot(ctx, player_guid, from_slot).ok_or_else(|| {
        refuse(
            ItemRefusal::ItemNotFound,
            format!("no item in slot {from_slot}"),
        )
    })?;
    let mut destination = item_in_slot(ctx, player_guid, to_slot);
    // Check both placements before storing either item.
    for (src, to_slot) in
        std::iter::once((&mut src, to_slot)).chain(destination.as_mut().map(|dst| (dst, from_slot)))
    {
        if crate::trade::item_is_offered(ctx, player_guid, src.guid) {
            return Err(ItemRefusal::NotRightNow);
        }
        if !valid_dest_slot(to_slot) {
            return Err(refuse(
                ItemRefusal::WrongSlot,
                format!("invalid destination slot {to_slot}"),
            ));
        }
        validate_bag_dest_slot(ctx, player_guid, to_slot)?;
        if bag_has_contents(ctx, player_guid, src.slot) {
            return Err(ItemRefusal::BagNotEmpty);
        }
        if (BAG_SLOT_START..=BAG_SLOT_END_INCL).contains(&src.slot) {
            let start = BAG_CONTENT_OFFSET + (src.slot - BAG_SLOT_START) * MAX_BAG_SIZE;
            if (start..start + MAX_BAG_SIZE).contains(&to_slot) {
                return Err(ItemRefusal::WrongSlot);
            }
        }
        if to_slot <= BAG_SLOT_END_INCL {
            let can_dual_wield = crate::spell::knows_spell(
                ctx,
                player_guid,
                lyracore_shared::constants::dual_wield::SPELL_ID,
            );
            let tmpl = ctx
                .db
                .game_item_template()
                .entry()
                .find(src.entry)
                .ok_or(ItemRefusal::CannotEquip)?;
            let fits = if to_slot >= BAG_SLOT_START {
                matches!(tmpl.class, 1 | 11)
                    && tmpl.inventory_type == invtype::BAG
                    && tmpl.container_slots > 0
            } else {
                can_equip_into(tmpl.class, tmpl.inventory_type, to_slot, can_dual_wield)
            };
            if !fits {
                return Err(ItemRefusal::CannotEquip);
            }
            let player = crate::helpers::live_entity(ctx, player_guid)
                .map_err(|_| refuse(ItemRefusal::Internal, "user not in world"))?;
            if !meets_required_level(player.level, tmpl.required_level) {
                return Err(refuse(
                    ItemRefusal::CannotEquip,
                    format!("requires level {}", tmpl.required_level),
                ));
            }
            let player_class = player.class();
            if !eligibility_mask_allows(tmpl.allowed_class, player_class) {
                return Err(refuse(
                    ItemRefusal::NoProficiency,
                    format!("class {player_class} is not allowed to equip this item"),
                ));
            }
            let player_race = player.race();
            if !eligibility_mask_allows(tmpl.allowed_race, player_race) {
                return Err(refuse(
                    ItemRefusal::NoProficiency,
                    format!("race {player_race} is not allowed to equip this item"),
                ));
            }
            let current_skill = ctx
                .db
                .game_player_skill()
                .by_character()
                .filter(&player_guid)
                .find(|skill| skill.skill_line == tmpl.required_skill)
                .map(|skill| skill.current);
            if !meets_required_skill(tmpl.required_skill, tmpl.required_skill_rank, current_skill) {
                return Err(refuse(
                    ItemRefusal::RequiredSkill,
                    format!(
                        "requires skill {} at rank {}",
                        tmpl.required_skill, tmpl.required_skill_rank
                    ),
                ));
            }
            let reputation_standing = ctx
                .db
                .game_player_reputation()
                .by_character()
                .filter(&player_guid)
                .find(|reputation| reputation.faction_id == tmpl.required_reputation_faction)
                .map(|reputation| reputation.standing);
            if !meets_required_reputation(
                tmpl.required_reputation_faction,
                tmpl.required_reputation_rank,
                reputation_standing,
            ) {
                return Err(refuse(
                    ItemRefusal::RequiredReputation,
                    format!(
                        "requires reputation faction {} at rank {}",
                        tmpl.required_reputation_faction, tmpl.required_reputation_rank
                    ),
                ));
            }
            let proficiency = Proficiency::from_spellbook(player_class, |spell_id| {
                crate::spell::knows_spell(ctx, player_guid, spell_id)
            });
            if !proficiency.can_equip(tmpl.class, tmpl.subclass) {
                return Err(refuse(
                    ItemRefusal::NoProficiency,
                    format!(
                        "class {} lacks proficiency for item class {}/subclass {}",
                        player_class, tmpl.class, tmpl.subclass
                    ),
                ));
            }
            if binds_on_equip(tmpl.bonding) {
                src.soulbound = true;
            }
        }
    }
    if [from_slot, to_slot]
        .iter()
        .any(|slot| matches!(*slot, equip_slot::MAINHAND | equip_slot::OFFHAND))
    {
        let entry_after_move = |slot| {
            if to_slot == slot {
                Some(src.entry)
            } else if from_slot == slot {
                destination.as_ref().map(|item| item.entry)
            } else {
                item_in_slot(ctx, player_guid, slot).map(|item| item.entry)
            }
        };
        if entry_after_move(equip_slot::OFFHAND).is_some()
            && entry_after_move(equip_slot::MAINHAND)
                .and_then(|entry| ctx.db.game_item_template().entry().find(entry))
                .is_some_and(|template| template.inventory_type == invtype::TWO_HAND_WEAPON)
        {
            return Err(ItemRefusal::CannotEquip);
        }
    }
    if let Some(mut dst) = destination {
        if dst.entry == src.entry && dst.random_property_id == src.random_property_id {
            if let Some(tmpl) = ctx.db.game_item_template().entry().find(src.entry) {
                if tmpl.max_stack > 1 {
                    let moved = merge_amount(src.stack_count, dst.stack_count, tmpl.max_stack);
                    if moved > 0 {
                        dst.stack_count += moved;
                        instances.guid().update(dst);
                        if moved >= src.stack_count {
                            instances.guid().delete(src.guid);
                        } else {
                            src.stack_count -= moved;
                            instances.guid().update(src);
                        }
                    }
                    return Ok(());
                }
            }
        }
        dst.slot = from_slot;
        instances.guid().update(dst);
    }
    src.slot = to_slot;
    instances.guid().update(src);
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
) -> Result<(), ItemRefusal> {
    let src = item_in_slot(ctx, player_guid, from_slot).ok_or_else(|| {
        refuse(
            ItemRefusal::ItemNotFound,
            format!("no item in slot {from_slot}"),
        )
    })?;
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(src.entry)
        .ok_or_else(|| {
            refuse(
                ItemRefusal::ItemNotFound,
                format!("no template for item entry {}", src.entry),
            )
        })?;
    // Bags (INVTYPE_BAG) equip into bag-equip slots 19..=22, not the equipment region 0..=18.
    // Route them separately: find the first free bag-equip slot and move the bag there. A bag
    // already in the bag slots (dragged manually) won't come through here, but autoequip does.
    if tmpl.inventory_type == invtype::BAG {
        let to_slot = first_free_bag_equip_slot(ctx, player_guid)
            .ok_or_else(|| refuse(ItemRefusal::InventoryFull, "all four bag slots are full"))?;
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
    .ok_or_else(|| {
        refuse(
            ItemRefusal::CannotEquip,
            format!("item {} is not equippable", src.entry),
        )
    })?;
    // Equip == a validated move into the resolved equip slot (reuses the equip-validation + swap there).
    apply_item_move(ctx, player_guid, from_slot, to_slot)
}

/// Equip a profile item only when its destination is empty or holds lower item-level gear. The
/// existing equip operation still owns every slot, level, proficiency, skill, reputation, and bind
/// Gate. Full bag slots and missing equipped templates preserve what the Character already wears.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn apply_equip_profile_upgrade(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
) -> Result<bool, ItemRefusal> {
    if from_slot <= BAG_SLOT_END_INCL || !is_carried_slot(from_slot) {
        return Err(refuse(
            ItemRefusal::WrongSlot,
            "profile equipment must come from carried storage",
        ));
    }
    let candidate = item_in_slot(ctx, player_guid, from_slot).ok_or_else(|| {
        refuse(
            ItemRefusal::ItemNotFound,
            format!("no item in slot {from_slot}"),
        )
    })?;
    let candidate_template = ctx
        .db
        .game_item_template()
        .entry()
        .find(candidate.entry)
        .ok_or_else(|| {
            refuse(
                ItemRefusal::ItemNotFound,
                format!("no template for item entry {}", candidate.entry),
            )
        })?;

    let destination = if candidate_template.inventory_type == invtype::BAG {
        let Some(slot) = first_free_bag_equip_slot(ctx, player_guid) else {
            return Ok(false);
        };
        slot
    } else {
        let can_dual_wield = crate::spell::knows_spell(
            ctx,
            player_guid,
            lyracore_shared::constants::dual_wield::SPELL_ID,
        );
        resolve_equip_slot(candidate_template.inventory_type, can_dual_wield, |slot| {
            slot_occupied(ctx, player_guid, slot)
        })
        .ok_or_else(|| {
            refuse(
                ItemRefusal::CannotEquip,
                format!("item {} is not equippable", candidate.entry),
            )
        })?
    };

    if let Some(equipped) = item_in_slot(ctx, player_guid, destination) {
        let Some(equipped_template) = ctx.db.game_item_template().entry().find(equipped.entry)
        else {
            return Ok(false);
        };
        if equipped_template.item_level >= candidate_template.item_level {
            return Ok(false);
        }
    }
    apply_equip_item(ctx, player_guid, from_slot)?;
    Ok(true)
}

/// Shared UNEQUIP logic for the player + debug paths: take the item in equipment `from_slot` (0..=18)
/// and move it to the first free backpack slot. Rejects a non-equipment source slot (it's not equipped)
/// or a full backpack. Delegates to `apply_item_move` so the placement is the exact same move primitive;
/// the destination is a backpack slot, so the equip-validation branch there is a no-op for it. Additive.
pub(crate) fn apply_unequip_item(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
) -> Result<(), ItemRefusal> {
    if from_slot > equip_slot::END {
        return Err(refuse(
            ItemRefusal::WrongSlot,
            format!("slot {from_slot} is not an equipment slot"),
        ));
    }
    // Must actually hold an equipped item to unequip.
    if !slot_occupied(ctx, player_guid, from_slot) {
        return Err(refuse(
            ItemRefusal::ItemNotFound,
            format!("no item equipped in slot {from_slot}"),
        ));
    }
    let free = first_free_backpack_slot(ctx, player_guid)
        .ok_or_else(|| refuse(ItemRefusal::InventoryFull, "inventory full"))?;
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

/// Splits target storage slots only. Equipment requires the move path's Gates.
pub(crate) fn valid_split_dest_slot(to_slot: u8) -> bool {
    valid_dest_slot(to_slot) && to_slot > BAG_SLOT_END_INCL
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

/// The bag-content CAPACITY gate shared by `apply_item_move` and `apply_item_split`: once
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
) -> Result<(), ItemRefusal> {
    if to_slot < BAG_CONTENT_OFFSET {
        return Ok(());
    }
    let (bag_idx, slot_in_bag) = bag_content_decompose(to_slot);
    let bag_equip_slot = BAG_SLOT_START + bag_idx;
    let bag_inst = item_in_slot(ctx, player_guid, bag_equip_slot).ok_or_else(|| {
        refuse(
            ItemRefusal::WrongSlot,
            format!("no bag equipped in slot {bag_equip_slot}"),
        )
    })?;
    let bag_tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(bag_inst.entry)
        .ok_or_else(|| refuse(ItemRefusal::WrongSlot, "equipped bag has no template"))?;
    if slot_in_bag >= bag_tmpl.container_slots.min(MAX_BAG_SIZE) {
        return Err(refuse(
            ItemRefusal::WrongSlot,
            format!(
                "slot {} out of range for bag with {} slots",
                slot_in_bag, bag_tmpl.container_slots
            ),
        ));
    }
    Ok(())
}

/// The banker-proximity Gate as an item Refusal: its own prose stays in the Module log.
fn bank_access(ctx: &ReducerContext, player_guid: u64) -> Result<(), ItemRefusal> {
    super::economy::bank_access_gate(ctx, player_guid)
        .map_err(|detail| refuse(ItemRefusal::BankUnavailable, detail))
}

/// A split must leave at least one unit in each stack.
pub(crate) fn valid_split_count(count: u32, stack_count: u32) -> bool {
    count != 0 && count < stack_count
}

/// First backpack slot (23..39) not occupied by any of the player's items, or `None` if the backpack
/// is full. Collects the owner's occupied slots into a set ONCE (smalls: was one `by_owner_guid`
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
    let owned: Vec<ItemInstance> = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .collect();
    free_inventory_slots(&owned, |entry| {
        ctx.db
            .game_item_template()
            .entry()
            .find(entry)
            .map_or(0, |t| t.container_slots)
    })
    .len() as u32
}

/// Free storage slots after the planned item consumption. Empty stacks no longer occupy a slot
/// or provide bag capacity. Backpack slots precede equipped bags in their normal storage order.
pub(super) fn free_inventory_slots(
    owned: &[ItemInstance],
    mut bag_capacity: impl FnMut(u32) -> u8,
) -> Vec<u8> {
    let occupied: std::collections::HashSet<u8> = owned
        .iter()
        .filter(|item| item.stack_count > 0)
        .map(|item| item.slot)
        .collect();
    let mut free: Vec<u8> = (starter_item::BACKPACK_SLOT_0..BACKPACK_SLOT_END)
        .filter(|slot| !occupied.contains(slot))
        .collect();
    for bag_idx in 0..BAG_SLOT_COUNT {
        let Some(bag) = owned
            .iter()
            .find(|item| item.slot == BAG_SLOT_START + bag_idx && item.stack_count > 0)
        else {
            continue;
        };
        let capacity = bag_capacity(bag.entry).min(MAX_BAG_SIZE);
        let base = BAG_CONTENT_OFFSET + bag_idx * MAX_BAG_SIZE;
        free.extend(
            (0..capacity)
                .map(|offset| base + offset)
                .filter(|slot| !occupied.contains(slot)),
        );
    }
    free
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
/// (smalls, same fix as `first_free_backpack_slot`) instead of one index scan per candidate
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
            .ok_or_else(|| refused(refuse(ItemRefusal::InventoryFull, "inventory full")))?
    } else {
        first_free_bank_slot(ctx, player_guid)
            .ok_or_else(|| refused(refuse(ItemRefusal::InventoryFull, "bank full")))?
    };
    apply_item_move(ctx, player_guid, slot, to_slot).map_err(refused)
}

#[cfg(test)]
mod tests {
    use super::{
        bag_content_decompose, is_bank_slot, is_carried_slot, valid_dest_slot, valid_split_count,
        valid_split_dest_slot, BANK_SLOT_END_INCL, BANK_SLOT_START,
    };

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

    #[test]
    fn valid_split_dest_slot_accepts_only_storage() {
        for slot in 0..=22 {
            assert!(valid_dest_slot(slot));
            assert!(!valid_split_dest_slot(slot));
        }
        for slot in [23, 38, 39, 62, 120, 191] {
            assert!(valid_split_dest_slot(slot));
        }
        for slot in [63, 119, 192, 255] {
            assert!(!valid_split_dest_slot(slot));
        }
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
