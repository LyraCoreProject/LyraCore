use spacetimedb::ReducerContext;

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
        crate::ItemInstance::delete(ctx, item.guid);
        crate::mail_text::drop_letter_text(ctx, item.item_text_id);
    } else {
        item.stack_count -= count;
        item.update(ctx);
    }
    if slot <= equip_slot::END {
        crate::spell::recompute_vitals(ctx, character_guid);
        crate::spell::recompute_sheet(ctx, character_guid);
    }
    Ok(())
}

pub(crate) fn apply_item_split(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
    count: u32,
    to_slot: u8,
) -> Result<(), ItemRefusal> {
    if is_bank_slot(slot) || is_bank_slot(to_slot) {
        bank_access(ctx, player_guid)?;
    }
    let mut inst = item_in_slot(ctx, player_guid, slot)
        .ok_or_else(|| refuse(ItemRefusal::ItemNotFound, format!("no item in slot {slot}")))?;
    if crate::trade::item_is_offered(ctx, player_guid, inst.guid) {
        return Err(ItemRefusal::NotRightNow);
    }
    if !valid_split_count(count, inst.stack_count) {
        return Err(refuse(ItemRefusal::WrongSlot, "invalid split count"));
    }
    if !valid_split_dest_slot(to_slot) {
        return Err(refuse(
            ItemRefusal::WrongSlot,
            format!("invalid destination slot {to_slot}"),
        ));
    }
    validate_bag_dest_slot(ctx, player_guid, to_slot)?;
    if slot_occupied(ctx, player_guid, to_slot) {
        return Err(refuse(ItemRefusal::WrongSlot, "destination slot occupied"));
    }
    let new_guid = next_item_guid(ctx).map_err(|detail| refuse(ItemRefusal::Internal, detail))?;
    inst.stack_count -= count;
    let entry = inst.entry;
    let owner_identity = inst.owner_identity;
    let durability = inst.durability;
    let random_property_id = inst.random_property_id;
    let soulbound = inst.soulbound;
    let item_text_id = inst.item_text_id;
    inst.update(ctx);
    crate::ItemInstance::insert_split(
        ctx,
        ItemInstance {
            guid: new_guid,
            entry,
            owner_identity,
            owner_guid: player_guid,
            slot: to_slot,
            stack_count: count,
            durability,
            created_at: crate::time::now(ctx),
            enchant_id: 0,
            soulbound,
            random_property_id,
            item_text_id,
        },
    );
    Ok(())
}

pub(crate) fn apply_item_move(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
    to_slot: u8,
) -> Result<(), ItemRefusal> {
    if from_slot == to_slot {
        return Ok(());
    }
    if is_bank_slot(from_slot) || is_bank_slot(to_slot) {
        bank_access(ctx, player_guid)?;
    }
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
    let displaced_off_hand = displaced_off_hand(
        ctx,
        player_guid,
        from_slot,
        to_slot,
        src.guid,
        destination.as_ref().map(|item| item.guid),
    )?;
    if let Some(mut dst) = destination {
        if dst.entry == src.entry && dst.random_property_id == src.random_property_id {
            if let Some(tmpl) = ctx.db.game_item_template().entry().find(src.entry) {
                if tmpl.max_stack > 1 {
                    let moved = merge_amount(src.stack_count, dst.stack_count, tmpl.max_stack);
                    if moved > 0 {
                        dst.stack_count += moved;
                        dst.update_merged(ctx);
                        if moved >= src.stack_count {
                            crate::ItemInstance::delete(ctx, src.guid);
                        } else {
                            src.stack_count -= moved;
                            src.update(ctx);
                        }
                    }
                    return Ok(());
                }
            }
        }
        dst.slot = from_slot;
        dst.update(ctx);
    }
    src.slot = to_slot;
    src.update(ctx);
    if let Some(off_hand) = displaced_off_hand {
        off_hand.update(ctx);
    }
    if from_slot <= equip_slot::END || to_slot <= equip_slot::END {
        crate::spell::recompute_vitals(ctx, player_guid);
        crate::spell::recompute_sheet(ctx, player_guid);
    }
    Ok(())
}

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
    if tmpl.inventory_type == invtype::BAG {
        let to_slot = first_free_bag_equip_slot(ctx, player_guid)
            .ok_or_else(|| refuse(ItemRefusal::InventoryFull, "all four bag slots are full"))?;
        return apply_item_move(ctx, player_guid, from_slot, to_slot);
    }
    let to_slot = auto_equip_slot(ctx, player_guid, tmpl.inventory_type).ok_or_else(|| {
        refuse(
            ItemRefusal::CannotEquip,
            format!("item {} is not equippable", src.entry),
        )
    })?;
    apply_item_move(ctx, player_guid, from_slot, to_slot)
}

fn displaced_off_hand(
    ctx: &ReducerContext,
    player_guid: u64,
    from_slot: u8,
    to_slot: u8,
    src_guid: u64,
    destination_guid: Option<u64>,
) -> Result<Option<ItemInstance>, ItemRefusal> {
    if ![from_slot, to_slot]
        .iter()
        .any(|slot| matches!(*slot, equip_slot::MAINHAND | equip_slot::OFFHAND))
    {
        return Ok(None);
    }
    let owned_after_move = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .map(|mut item| {
            if item.guid == src_guid {
                item.slot = to_slot;
            } else if Some(item.guid) == destination_guid {
                item.slot = from_slot;
            }
            item
        })
        .collect();
    let templates = ctx.db.game_item_template();
    off_hand_after_move(
        owned_after_move,
        &[Some(src_guid), destination_guid],
        |entry| {
            templates
                .entry()
                .find(entry)
                .is_some_and(|template| template.inventory_type == invtype::TWO_HAND_WEAPON)
        },
        |entry| {
            templates
                .entry()
                .find(entry)
                .map_or(0, |t| t.container_slots)
        },
    )
}

// The off-hand slot counts as taken while the main hand holds a two-handed weapon.
fn auto_equip_slot(ctx: &ReducerContext, player_guid: u64, inventory_type: u8) -> Option<u8> {
    let can_dual_wield = crate::spell::knows_spell(
        ctx,
        player_guid,
        lyracore_shared::constants::dual_wield::SPELL_ID,
    );
    let two_hander_equipped = item_in_slot(ctx, player_guid, equip_slot::MAINHAND)
        .and_then(|item| ctx.db.game_item_template().entry().find(item.entry))
        .is_some_and(|template| template.inventory_type == invtype::TWO_HAND_WEAPON);
    resolve_equip_slot(inventory_type, can_dual_wield, |slot| {
        slot_occupied(ctx, player_guid, slot)
            || (slot == equip_slot::OFFHAND && two_hander_equipped)
    })
}

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
        auto_equip_slot(ctx, player_guid, candidate_template.inventory_type).ok_or_else(|| {
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

const BACKPACK_SLOT_END: u8 = starter_item::BACKPACK_SLOT_0 + 16;

const BAG_SLOT_START: u8 = 19;
const BAG_SLOT_END_INCL: u8 = 22;
const BAG_SLOT_COUNT: u8 = 4;
// Keep bag-content slots above vanilla ItemSlot ordinals, whose last value is 112.
const BAG_CONTENT_OFFSET: u8 = 120;
// Each equipped bag reserves 18 flat slots, the vanilla maximum capacity.
const MAX_BAG_SIZE: u8 = 18;
const BAG_CONTENT_END: u8 = BAG_CONTENT_OFFSET + BAG_SLOT_COUNT * MAX_BAG_SIZE;

pub(crate) fn valid_dest_slot(to_slot: u8) -> bool {
    to_slot < BACKPACK_SLOT_END
        || is_bank_slot(to_slot)
        || (BAG_CONTENT_OFFSET..BAG_CONTENT_END).contains(&to_slot)
}

const BANK_SLOT_START: u8 = 39;
const BANK_SLOT_END_INCL: u8 = 62;

// Bank-bag slots 63..=68 have no content addresses in this model.
pub(crate) fn is_bank_slot(slot: u8) -> bool {
    (BANK_SLOT_START..=BANK_SLOT_END_INCL).contains(&slot)
}

pub(crate) use lyracore_shared::item::is_carried_slot;

pub(crate) fn valid_split_dest_slot(to_slot: u8) -> bool {
    valid_dest_slot(to_slot) && to_slot > BAG_SLOT_END_INCL
}

pub(crate) fn bag_content_decompose(to_slot: u8) -> (u8, u8) {
    let offset = to_slot - BAG_CONTENT_OFFSET;
    (offset / MAX_BAG_SIZE, offset % MAX_BAG_SIZE)
}

// A valid flat slot still needs an equipped bag with enough capacity.
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

fn bank_access(ctx: &ReducerContext, player_guid: u64) -> Result<(), ItemRefusal> {
    super::economy::bank_access_gate(ctx, player_guid)
        .map_err(|detail| refuse(ItemRefusal::BankUnavailable, detail))
}

pub(crate) fn valid_split_count(count: u32, stack_count: u32) -> bool {
    count != 0 && count < stack_count
}

fn occupied_slots(ctx: &ReducerContext, player_guid: u64) -> std::collections::HashSet<u8> {
    ctx.db
        .game_item_instance()
        .by_owner_guid()
        .filter(&player_guid)
        .map(|i| i.slot)
        .collect()
}

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

// A two-hander moved into the main hand sends the off-hand item to the first free carried slot.
fn off_hand_after_move(
    mut owned_after_move: Vec<ItemInstance>,
    moved: &[Option<u64>; 2],
    is_two_hander: impl Fn(u32) -> bool,
    bag_capacity: impl FnMut(u32) -> u8,
) -> Result<Option<ItemInstance>, ItemRefusal> {
    let in_slot = |slot| owned_after_move.iter().position(|item| item.slot == slot);
    let (Some(main_hand), Some(off_hand)) =
        (in_slot(equip_slot::MAINHAND), in_slot(equip_slot::OFFHAND))
    else {
        return Ok(None);
    };
    if !is_two_hander(owned_after_move[main_hand].entry) {
        return Ok(None);
    }
    let was_moved = |index: usize| moved.contains(&Some(owned_after_move[index].guid));
    match (was_moved(main_hand), was_moved(off_hand)) {
        (true, false) => {
            let free_slot = free_inventory_slots(&owned_after_move, bag_capacity)
                .first()
                .copied()
                .ok_or_else(|| {
                    refuse(
                        ItemRefusal::InventoryFull,
                        "no free slot for the off-hand item",
                    )
                })?;
            let mut off_hand = owned_after_move.swap_remove(off_hand);
            off_hand.slot = free_slot;
            Ok(Some(off_hand))
        }
        (false, true) => Err(refuse(
            ItemRefusal::TwoHandedWeaponEquipped,
            "the main hand holds a two-handed weapon",
        )),
        _ => Err(refuse(
            ItemRefusal::CannotEquip,
            "a two-handed weapon and an off-hand item cannot swap hands",
        )),
    }
}

pub(crate) fn first_free_backpack_slot(ctx: &ReducerContext, player_guid: u64) -> Option<u8> {
    let occupied = occupied_slots(ctx, player_guid);
    (starter_item::BACKPACK_SLOT_0..BACKPACK_SLOT_END).find(|slot| !occupied.contains(slot))
}

fn first_free_bag_equip_slot(ctx: &ReducerContext, player_guid: u64) -> Option<u8> {
    (BAG_SLOT_START..=BAG_SLOT_END_INCL).find(|&slot| !slot_occupied(ctx, player_guid, slot))
}

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
        bag_content_decompose, equip_slot, is_bank_slot, is_carried_slot, off_hand_after_move,
        valid_dest_slot, valid_split_count, valid_split_dest_slot, ItemInstance, ItemRefusal,
        BACKPACK_SLOT_END, BANK_SLOT_END_INCL, BANK_SLOT_START,
    };
    use lyracore_shared::constants::starter_item::BACKPACK_SLOT_0;
    use spacetimedb::{Identity, Timestamp};

    #[test]
    fn is_bank_slot_covers_the_24_base_slots_only() {
        for slot in BANK_SLOT_START..=BANK_SLOT_END_INCL {
            assert!(is_bank_slot(slot), "slot {slot} is a base bank slot");
        }
        for slot in [0u8, 38, 63, 68, 119, 120, 191, 192, 255] {
            assert!(!is_bank_slot(slot), "slot {slot} is not a base bank slot");
        }
    }

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
        assert!(!valid_split_count(0, STACK));
        assert!(!valid_split_count(STACK, STACK));
        assert!(!valid_split_count(STACK + 1, STACK));
        for count in 1..STACK {
            assert!(
                valid_split_count(count, STACK),
                "count {count} of {STACK} should be a valid split"
            );
        }
    }

    #[test]
    fn valid_dest_slot_admits_the_three_modeled_ranges_only() {
        for slot in [0u8, 18, 19, 22, 23, 38] {
            assert!(valid_dest_slot(slot), "slot {slot} is in the 0..=38 range");
        }
        for slot in BANK_SLOT_START..=BANK_SLOT_END_INCL {
            assert!(valid_dest_slot(slot), "bank slot {slot} is a valid dest");
        }
        assert!(!valid_dest_slot(63));
        assert!(!valid_dest_slot(68));
        assert!(!valid_dest_slot(119));
        assert!(valid_dest_slot(120));
        assert!(valid_dest_slot(191));
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

    const TWO_HANDER: u32 = 1;
    const SHIELD: u32 = 2;
    const ONE_HANDER: u32 = 3;
    const FILLER: u32 = 4;

    fn item(guid: u64, slot: u8, entry: u32) -> ItemInstance {
        ItemInstance {
            guid,
            entry,
            owner_identity: Identity::ZERO,
            owner_guid: 1,
            slot,
            stack_count: 1,
            durability: 50,
            created_at: Timestamp::UNIX_EPOCH,
            enchant_id: 0,
            soulbound: false,
            random_property_id: 0,
            item_text_id: 0,
        }
    }

    fn backpack_filler(except: &[u8]) -> Vec<ItemInstance> {
        (BACKPACK_SLOT_0..BACKPACK_SLOT_END)
            .filter(|slot| !except.contains(slot))
            .map(|slot| item(100 + u64::from(slot), slot, FILLER))
            .collect()
    }

    fn off_hand_after(
        owned_after_move: Vec<ItemInstance>,
        moved: [Option<u64>; 2],
    ) -> Result<Option<(u64, u8)>, ItemRefusal> {
        off_hand_after_move(owned_after_move, &moved, |entry| entry == TWO_HANDER, |_| 0)
            .map(|off_hand| off_hand.map(|item| (item.guid, item.slot)))
    }

    #[test]
    fn a_two_hander_from_a_full_backpack_frees_the_slot_the_off_hand_item_takes() {
        let mut owned = backpack_filler(&[BACKPACK_SLOT_0]);
        owned.push(item(1, equip_slot::MAINHAND, TWO_HANDER));
        owned.push(item(2, equip_slot::OFFHAND, SHIELD));

        assert_eq!(
            off_hand_after(owned, [Some(1), None]),
            Ok(Some((2, BACKPACK_SLOT_0)))
        );
    }

    #[test]
    fn a_two_hander_swapped_with_a_main_hand_weapon_needs_another_free_slot() {
        let mut owned = backpack_filler(&[BACKPACK_SLOT_0]);
        owned.push(item(1, equip_slot::MAINHAND, TWO_HANDER));
        owned.push(item(3, BACKPACK_SLOT_0, ONE_HANDER));
        owned.push(item(2, equip_slot::OFFHAND, SHIELD));

        assert_eq!(
            off_hand_after(owned, [Some(1), Some(3)]),
            Err(ItemRefusal::InventoryFull)
        );
    }

    #[test]
    fn a_two_hander_sends_the_off_hand_item_to_the_first_free_backpack_slot() {
        let mut owned = backpack_filler(&[BACKPACK_SLOT_0, 30]);
        owned.push(item(1, equip_slot::MAINHAND, TWO_HANDER));
        owned.push(item(3, BACKPACK_SLOT_0, ONE_HANDER));
        owned.push(item(2, equip_slot::OFFHAND, SHIELD));

        assert_eq!(off_hand_after(owned, [Some(1), Some(3)]), Ok(Some((2, 30))));
    }

    #[test]
    fn an_off_hand_item_cannot_join_an_equipped_two_hander() {
        let owned = vec![
            item(1, equip_slot::MAINHAND, TWO_HANDER),
            item(2, equip_slot::OFFHAND, SHIELD),
        ];

        assert_eq!(
            off_hand_after(owned, [Some(2), None]),
            Err(ItemRefusal::TwoHandedWeaponEquipped)
        );
    }

    #[test]
    fn a_one_hander_keeps_the_off_hand_item_in_place() {
        let owned = vec![
            item(3, equip_slot::MAINHAND, ONE_HANDER),
            item(2, equip_slot::OFFHAND, SHIELD),
        ];

        assert_eq!(off_hand_after(owned, [Some(3), None]), Ok(None));
    }

    #[test]
    fn bag_content_decompose_maps_flat_slot_to_bag_index_and_position() {
        assert_eq!(bag_content_decompose(120), (0, 0));
        assert_eq!(bag_content_decompose(137), (0, 17));
        assert_eq!(bag_content_decompose(138), (1, 0));
        assert_eq!(bag_content_decompose(191), (3, 17));
    }
}
