mod economy;
mod exchange;
mod guids;
mod inventory;
mod ops;
mod properties;
mod rules;
mod tables;

use lyracore_shared::item::ItemRefusal;

pub(crate) fn refuse(refusal: ItemRefusal, detail: impl std::fmt::Display) -> ItemRefusal {
    spacetimedb::log::info!("item refused {}: {detail}", refusal.as_tag());
    refusal
}

pub(crate) fn refused(refusal: ItemRefusal) -> String {
    refusal.as_tag().to_string()
}

#[cfg(feature = "debug_reducers")]
pub(crate) use economy::apply_repair_item;
pub(crate) use economy::{
    apply_buy_bank_slot, apply_buy_item, apply_buyback_item, apply_item_sell, apply_player_repair,
    buy_bank_slot_result, innkeeper_access_gate, npc_interaction_gate,
};
pub(crate) use exchange::exchange_items;
pub(crate) use guids::{allocate_item_guids, next_item_guid};
pub(crate) use inventory::{
    apply_auto_bank_item, apply_equip_item, apply_equip_profile_upgrade, apply_item_destroy,
    apply_item_move, apply_item_split, apply_unequip_item, count_free_inventory_slots,
    is_carried_slot, validate_bag_dest_slot,
};
pub(crate) use ops::{
    apply_death_durability_loss, apply_item_use, apply_spirit_healer_durability_loss,
    apply_take_loot, carried_items, equipped_school_stat_bonus, equipped_stat_bonus, grant_item,
    grant_item_property, grant_letter_item, grant_starter_item, has_free_slot, item_count,
    item_stat_bonus, remove_items, request_grant_item, request_profile_item, request_take_loot,
    resistance_items, restore_carried_items, store_instance_state, ItemSnapshot,
};
pub(crate) use properties::{
    enchant_stat, enchant_stat_for_school, is_known_enchant, property_stat,
    property_stat_for_school, seed_compatibility_enchantments, select_loot_property,
    select_property,
};
pub use properties::{
    game_item_enchantment, game_item_property_weight, game_item_random_property, ItemEnchantment,
    ItemPropertyWeight, ItemRandomProperty,
};
#[cfg(feature = "debug_reducers")]
pub(crate) use rules::binds_on_equip;
pub(crate) use rules::{binds_on_grant, merge_amount, EquipStat};

pub use rules::weapon_subclass;

pub use tables::{
    game_character_buyback, game_item_instance, game_item_template, game_npc_vendor, BuybackEntry,
    ItemInstance, ItemTemplate, NpcVendor, ALL_PLAYABLE_CLASS_MASK, ALL_PLAYABLE_RACE_MASK,
};
pub(crate) use tables::{item_in_slot, item_is_broken};

pub(crate) use tables::{
    sweep_delete_game_character_buyback, sweep_delete_game_item_instance,
    sweep_restamp_game_item_instance, sweep_transfer_game_character_buyback,
    sweep_transfer_game_item_instance,
};

pub(crate) use rules::{bonding, equip_slot, invtype};

#[cfg(test)]
pub(crate) use rules::tests;
