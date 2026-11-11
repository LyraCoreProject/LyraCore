use super::update_mask;
use super::values::build_values_update_raw;
use lyracore_shared::item::{ItemRefusal, Proficiency};
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
pub use wow_world_messages::vanilla::SellItemResult;
use wow_world_messages::vanilla::{
    AllowedClass, AllowedRace, BagFamily, Bonding, BuyBankSlotResult, BuyResult, Faction, Gold,
    InventoryType, ItemClass, ItemClassAndSubClass, ItemDamageType, ItemFlag, ItemQuality,
    ItemSpells, ItemStat, ItemStatType, Level, MovementBlock, MovementBlock_UpdateFlag,
    MovementBlock_UpdateFlag_All, Object, ObjectType, SMSG_ITEM_QUERY_SINGLE_RESPONSE_found,
    SheatheType, Skill, SpellSchool, SpellTriggerType, UpdateContainer, UpdateItem, UpdateMask,
    SMSG_BUY_BANK_SLOT_RESULT, SMSG_BUY_FAILED, SMSG_INVENTORY_CHANGE_FAILURE,
    SMSG_ITEM_ENCHANT_TIME_UPDATE, SMSG_ITEM_NAME_QUERY_RESPONSE, SMSG_ITEM_QUERY_SINGLE_RESPONSE,
    SMSG_SELL_ITEM, SMSG_SET_PROFICIENCY, SMSG_SHOW_BANK, SMSG_UPDATE_OBJECT,
};
use wow_world_messages::Guid;

pub fn inventory_slot(bag: u8, slot: u8) -> Option<u8> {
    match (bag, slot) {
        (255, 0..=62 | 81..=112) => Some(slot),
        (19..=22, 0..=17) => Some(120 + (bag - 19) * 18 + slot),
        _ => None,
    }
}

pub fn bag_content_parts(slot: u8) -> Option<(u8, u8)> {
    (120..192)
        .contains(&slot)
        .then(|| (19 + (slot - 120) / 18, (slot - 120) % 18))
}

#[derive(Clone, Debug, Default)]
pub struct ItemTemplateView {
    pub entry: u32,
    pub class: u8,
    pub subclass: u8,
    pub name: String,
    pub display_id: u32,
    pub quality: u8,
    pub inventory_type: u8,
    pub item_level: u8,
    pub required_level: u8,
    pub max_durability: u32,
    pub buy_price: u32,
    pub sell_price: u32,
    pub max_stack: u32,
    pub damage_min: f32,
    pub damage_max: f32,
    pub delay_ms: u32,
    pub stat_strength: i32,
    pub stat_agility: i32,
    pub stat_stamina: i32,
    pub stat_intellect: i32,
    pub stat_spirit: i32,
    pub stat_armor: i32,
    pub block_value: i32,
    pub spellid_1: u32,
    pub spelltrigger_1: u8,
    pub spellid_2: u32,
    pub spelltrigger_2: u8,
    pub bonding: u8,
    /// Preserve raw sheath values; gtker's SheatheType names differ from cmangos.
    pub sheath: u8,
    pub holy_res: i32,
    pub fire_res: i32,
    pub nature_res: i32,
    pub frost_res: i32,
    pub shadow_res: i32,
    pub arcane_res: i32,
    pub spellid_3: u32,
    pub spelltrigger_3: u8,
    pub spellid_4: u32,
    pub spelltrigger_4: u8,
    pub spellid_5: u32,
    pub spelltrigger_5: u8,
    pub spell_categories: [u32; 5],
    pub required_skill: u32,
    pub required_skill_rank: u32,
    pub required_reputation_faction: u32,
    pub required_reputation_rank: u32,
    pub max_count: u32,
    pub item_flags: u32,
    pub page_text: u32,
    pub start_quest: u32,
    pub bag_family: u32,
    pub allowed_class: u32,
    pub allowed_race: u32,
    pub random_property: u32,
}

#[derive(Clone, Debug, Default)]
pub struct ItemInstanceView {
    pub guid: u64,
    pub entry: u32,
    pub owner_guid: u64,
    pub contained_guid: Option<u64>,
    pub soulbound: bool,
    pub slot: u8,
    pub stack_count: u32,
    pub durability: u32,
    pub max_durability: u32,
    pub container_slots: u8,
    pub random_property_id: u32,
    pub random_property_enchant_ids: [u32; 3],
    pub item_text_id: u32,
    pub enchantment: u32,
    pub temporary_enchantment: TemporaryEnchantmentView,
}

/// A zero `enchantment` is an empty slot. `duration_ms` is the time left as the item is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TemporaryEnchantmentView {
    pub enchantment: u32,
    pub duration_ms: u32,
    pub charges: u32,
}

pub const ITEM_FIELD_ENCHANTMENT: u16 = 22;
// vmangos TEMP_ENCHANTMENT_SLOT; each slot holds id, duration and charges words.
pub const TEMPORARY_ENCHANTMENT_SLOT: u16 = 1;
const TEMPORARY_ENCHANTMENT_FIELD: u16 = ITEM_FIELD_ENCHANTMENT + TEMPORARY_ENCHANTMENT_SLOT * 3;

pub fn build_charter_petition_values(charter_item_guid: u64, petition_id: u32) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    mask.set_u32(ITEM_FIELD_ENCHANTMENT, petition_id);
    build_values_update_raw(charter_item_guid, &mask)
}

const RANDOM_PROPERTY_ENCHANT_ID_FIELDS: [u16; 3] = [
    ITEM_FIELD_ENCHANTMENT + 3 * 3,
    ITEM_FIELD_ENCHANTMENT + 4 * 3,
    ITEM_FIELD_ENCHANTMENT + 5 * 3,
];

/// The typed CREATE reaches only enchantment slot 0; later slots follow in VALUES.
pub fn build_enchantment_slot_values(inst: &ItemInstanceView) -> Option<(u16, Vec<u8>)> {
    if inst.random_property_enchant_ids == [0; 3] && inst.temporary_enchantment.enchantment == 0 {
        return None;
    }
    let mut mask = update_mask::UpdateMaskValues::new();
    if inst.temporary_enchantment.enchantment != 0 {
        set_temporary_enchantment(&mut mask, inst.temporary_enchantment);
    }
    for (field, enchant_id) in RANDOM_PROPERTY_ENCHANT_ID_FIELDS
        .into_iter()
        .zip(inst.random_property_enchant_ids)
        .filter(|&(_, enchant_id)| enchant_id != 0)
    {
        mask.set_u32(field, enchant_id);
    }
    Some(build_values_update_raw(inst.guid, &mask))
}

/// Writes all three words, so an empty view clears the slot.
pub fn build_temporary_enchantment_values(
    item_guid: u64,
    temporary: TemporaryEnchantmentView,
) -> (u16, Vec<u8>) {
    let mut mask = update_mask::UpdateMaskValues::new();
    set_temporary_enchantment(&mut mask, temporary);
    build_values_update_raw(item_guid, &mask)
}

fn set_temporary_enchantment(
    mask: &mut update_mask::UpdateMaskValues,
    temporary: TemporaryEnchantmentView,
) {
    mask.set_u32(TEMPORARY_ENCHANTMENT_FIELD, temporary.enchantment);
    mask.set_u32(TEMPORARY_ENCHANTMENT_FIELD + 1, temporary.duration_ms);
    mask.set_u32(TEMPORARY_ENCHANTMENT_FIELD + 2, temporary.charges);
}

/// The client counts the tooltip's time left down from `duration_s`.
pub fn build_item_enchant_time_update(
    item_guid: u64,
    owner_guid: u64,
    duration_s: u32,
) -> SMSG_ITEM_ENCHANT_TIME_UPDATE {
    SMSG_ITEM_ENCHANT_TIME_UPDATE {
        item: Guid::new(item_guid),
        slot: u32::from(TEMPORARY_ENCHANTMENT_SLOT),
        duration: duration_s,
        player: Guid::new(owner_guid),
    }
}

pub fn build_item_query_response(
    item: u32,
    t: Option<&ItemTemplateView>,
) -> SMSG_ITEM_QUERY_SINGLE_RESPONSE {
    let Some(t) = t else {
        return SMSG_ITEM_QUERY_SINGLE_RESPONSE {
            item: item | 0x8000_0000,
            found: None,
        };
    };
    // ItemClassAndSubClass packs subclass in the high u32 and class in the low u32.
    let class_and_sub_class =
        ItemClassAndSubClass::try_from((u64::from(t.subclass) << 32) | u64::from(t.class))
            .unwrap_or_default();
    let stats: [ItemStat; 10] = {
        let pairs: [(ItemStatType, i32); 5] = [
            (ItemStatType::Strength, t.stat_strength),
            (ItemStatType::Agility, t.stat_agility),
            (ItemStatType::Stamina, t.stat_stamina),
            (ItemStatType::Intellect, t.stat_intellect),
            (ItemStatType::Spirit, t.stat_spirit),
        ];
        let mut out = [ItemStat {
            stat_type: ItemStatType::Mana,
            value: 0,
        }; 10];
        let mut slot = 0usize;
        for (ty, val) in pairs {
            if val != 0 && slot < 10 {
                out[slot] = ItemStat {
                    stat_type: ty,
                    value: val,
                };
                slot += 1;
            }
        }
        out
    };
    let spell_slot = |id: u32, trig: u8| ItemSpells {
        spell: id,
        spell_trigger: SpellTriggerType::try_from(u32::from(trig)).unwrap_or_default(),
        ..Default::default()
    };
    let mut spells = [ItemSpells::default(); 5];
    spells[0] = spell_slot(t.spellid_1, t.spelltrigger_1);
    spells[1] = spell_slot(t.spellid_2, t.spelltrigger_2);
    spells[2] = spell_slot(t.spellid_3, t.spelltrigger_3);
    spells[3] = spell_slot(t.spellid_4, t.spelltrigger_4);
    spells[4] = spell_slot(t.spellid_5, t.spelltrigger_5);
    for (spell, category) in spells.iter_mut().zip(t.spell_categories) {
        spell.spell_category = category;
    }
    SMSG_ITEM_QUERY_SINGLE_RESPONSE {
        item: t.entry,
        found: Some(SMSG_ITEM_QUERY_SINGLE_RESPONSE_found {
            class_and_sub_class,
            name1: t.name.clone(),
            display_id: t.display_id,
            quality: ItemQuality::try_from(u32::from(t.quality)).unwrap_or_default(),
            flags: ItemFlag::new(t.item_flags),
            buy_price: Gold::new(t.buy_price),
            sell_price: Gold::new(t.sell_price),
            inventory_type: InventoryType::try_from(u32::from(t.inventory_type))
                .unwrap_or_default(),
            item_level: Level::new(t.item_level),
            required_level: Level::new(t.required_level),
            allowed_class: AllowedClass::new(t.allowed_class),
            allowed_race: AllowedRace::new(t.allowed_race),
            required_skill: Skill::try_from(t.required_skill).unwrap_or_default(),
            required_skill_rank: t.required_skill_rank,
            required_faction: Faction::try_from(t.required_reputation_faction).unwrap_or_default(),
            required_faction_rank: t.required_reputation_rank,
            stackable: t.max_stack.max(1),
            max_count: t.max_count,
            stats,
            armor: t.stat_armor,
            block: t.block_value.max(0) as u32,
            damages: {
                let mut d = [ItemDamageType::default(); 5];
                d[0] = ItemDamageType {
                    damage_minimum: t.damage_min,
                    damage_maximum: t.damage_max,
                    school: SpellSchool::Normal,
                };
                d
            },
            holy_resistance: t.holy_res,
            fire_resistance: t.fire_res,
            nature_resistance: t.nature_res,
            frost_resistance: t.frost_res,
            shadow_resistance: t.shadow_res,
            arcane_resistance: t.arcane_res,
            delay: t.delay_ms,
            sheathe_type: SheatheType::try_from(t.sheath).unwrap_or_default(),
            spells,
            bonding: Bonding::try_from(u32::from(t.bonding)).unwrap_or_default(),
            page_text: t.page_text,
            start_quest: t.start_quest,
            max_durability: t.max_durability,
            bag_family: BagFamily::try_from(t.bag_family).unwrap_or_default(),
            // RangedModRange is percent of base range; zero makes every ranged target out of range.
            ranged_range_modification: 100.0,
            random_property: t.random_property,
            ..Default::default()
        }),
    }
}

pub fn build_inventory_change_failure() -> SMSG_INVENTORY_CHANGE_FAILURE {
    SMSG_INVENTORY_CHANGE_FAILURE::ItemCantBeEquipped {
        item1: Guid::new(0),
        item2: Guid::new(0),
        bag_type_subclass: 0,
    }
}

// The client unlocks item1 and item2; a zero guid leaves a locked item grey until relog.
pub fn build_inventory_refusal(
    refusal: ItemRefusal,
    item1: u64,
    item2: u64,
) -> SMSG_INVENTORY_CHANGE_FAILURE {
    let (item1, item2, bag_type_subclass) = (Guid::new(item1), Guid::new(item2), 0);
    match refusal {
        ItemRefusal::Indestructible => SMSG_INVENTORY_CHANGE_FAILURE::CantDropSoulbound {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::BagNotEmpty => SMSG_INVENTORY_CHANGE_FAILURE::CanOnlyDoWithEmptyBags {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::ItemNotFound => SMSG_INVENTORY_CHANGE_FAILURE::ItemNotFound {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::InventoryFull => SMSG_INVENTORY_CHANGE_FAILURE::InventoryFull {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::WrongSlot => SMSG_INVENTORY_CHANGE_FAILURE::ItemDoesntGoToSlot {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::CannotEquip => SMSG_INVENTORY_CHANGE_FAILURE::ItemCantBeEquipped {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::TwoHandedWeaponEquipped => {
            SMSG_INVENTORY_CHANGE_FAILURE::CantEquipWithTwohanded {
                item1,
                item2,
                bag_type_subclass,
            }
        }
        ItemRefusal::NoProficiency => SMSG_INVENTORY_CHANGE_FAILURE::NoRequiredProficiency {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::RequiredSkill => SMSG_INVENTORY_CHANGE_FAILURE::CantEquipSkill {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::RequiredReputation => SMSG_INVENTORY_CHANGE_FAILURE::CantEquipReputation {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::PlayerDead => SMSG_INVENTORY_CHANGE_FAILURE::YouAreDead {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::BankUnavailable => SMSG_INVENTORY_CHANGE_FAILURE::TooFarAwayFromBank {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::ItemNotUsable => SMSG_INVENTORY_CHANGE_FAILURE::YouCanNeverUseThatItem {
            item1,
            item2,
            bag_type_subclass,
        },
        ItemRefusal::NotRightNow | ItemRefusal::SpellCooldown(_) => {
            SMSG_INVENTORY_CHANGE_FAILURE::CantDoRightNow {
                item1,
                item2,
                bag_type_subclass,
            }
        }
        ItemRefusal::Internal => SMSG_INVENTORY_CHANGE_FAILURE::IntBagError {
            item1,
            item2,
            bag_type_subclass,
        },
    }
}

pub fn build_cant_carry_more_of_this() -> SMSG_INVENTORY_CHANGE_FAILURE {
    SMSG_INVENTORY_CHANGE_FAILURE::CantCarryMoreOfThis {
        item1: Guid::new(0),
        item2: Guid::new(0),
        bag_type_subclass: 0,
    }
}

pub fn build_buy_failed(vendor_guid: u64, item_entry: u32, result: BuyResult) -> SMSG_BUY_FAILED {
    SMSG_BUY_FAILED {
        guid: Guid::new(vendor_guid),
        item: item_entry,
        result,
    }
}

pub fn build_sell_item_refusal(
    vendor_guid: u64,
    item_guid: u64,
    result: SellItemResult,
) -> SMSG_SELL_ITEM {
    SMSG_SELL_ITEM {
        guid: Guid::new(vendor_guid),
        item: Guid::new(item_guid),
        result,
    }
}

pub fn build_item_name_query_response(
    item: u32,
    item_name: String,
) -> SMSG_ITEM_NAME_QUERY_RESPONSE {
    SMSG_ITEM_NAME_QUERY_RESPONSE { item, item_name }
}

pub const SMSG_READ_ITEM_FAILED_OPCODE: u16 = 0x00AF;

/// 5875 body: item guid, then a reason byte; gtker omits the byte. Reasons above 1 close the text.
pub fn build_read_item_failed_raw(item_guid: u64) -> (u16, Vec<u8>) {
    let mut body = item_guid.to_le_bytes().to_vec();
    body.push(2);
    (SMSG_READ_ITEM_FAILED_OPCODE, body)
}

pub fn build_show_bank(banker_guid: u64) -> SMSG_SHOW_BANK {
    SMSG_SHOW_BANK {
        guid: Guid::new(banker_guid),
    }
}

pub fn build_buy_bank_slot_result(result: BuyBankSlotResult) -> SMSG_BUY_BANK_SLOT_RESULT {
    SMSG_BUY_BANK_SLOT_RESULT { result }
}

pub fn build_item_create_object(inst: &ItemInstanceView) -> SMSG_UPDATE_OBJECT {
    let owner = Guid::new(inst.owner_guid);
    let contained = Guid::new(inst.contained_guid.unwrap_or(inst.owner_guid));
    let update_flag =
        MovementBlock_UpdateFlag::empty().set_all(MovementBlock_UpdateFlag_All { unknown1: 1 });
    let guid3 = Guid::new(inst.guid);
    if inst.container_slots > 0 {
        let mut container = UpdateContainer::builder()
            .set_object_guid(guid3)
            .set_object_entry(inst.entry as i32)
            .set_object_scale_x(1.0)
            .set_item_random_properties_id(inst.random_property_id as i32)
            .set_item_owner(owner)
            .set_item_contained(contained)
            .set_item_flags(i32::from(inst.soulbound))
            .set_item_stack_count(inst.stack_count.max(1) as i32)
            .set_item_durability(inst.durability as i32)
            .set_item_maxdurability(inst.max_durability as i32)
            .set_container_num_slots(inst.container_slots as i32)
            .finalize();
        if inst.item_text_id != 0 {
            container.set_item_item_text_id(inst.item_text_id as i32);
        }
        if inst.enchantment != 0 {
            container.set_item_enchantment(inst.enchantment as i32);
        }
        SMSG_UPDATE_OBJECT {
            has_transport: 0,
            objects: vec![Object::CreateObject {
                guid3,
                object_type: ObjectType::Container,
                movement2: MovementBlock { update_flag },
                mask2: UpdateMask::Container(container),
            }],
        }
    } else {
        let mut item = UpdateItem::builder()
            .set_object_guid(guid3)
            .set_object_entry(inst.entry as i32)
            .set_object_scale_x(1.0)
            .set_item_random_properties_id(inst.random_property_id as i32)
            .set_item_owner(owner)
            .set_item_contained(contained)
            .set_item_flags(i32::from(inst.soulbound))
            .set_item_stack_count(inst.stack_count.max(1) as i32)
            .set_item_durability(inst.durability as i32)
            .set_item_maxdurability(inst.max_durability as i32)
            .finalize();
        if inst.item_text_id != 0 {
            item.set_item_item_text_id(inst.item_text_id as i32);
        }
        if inst.enchantment != 0 {
            item.set_item_enchantment(inst.enchantment as i32);
        }
        SMSG_UPDATE_OBJECT {
            has_transport: 0,
            objects: vec![Object::CreateObject {
                guid3,
                object_type: ObjectType::Item,
                movement2: MovementBlock { update_flag },
                mask2: UpdateMask::Item(item),
            }],
        }
    }
}

use wow_world_messages::vanilla::{
    NewItemChatAlert, NewItemCreationType, NewItemSource, SMSG_ITEM_PUSH_RESULT,
};

pub fn build_item_push_result(
    player_guid: u64,
    bag_slot: u8,
    item_slot: u32,
    entry: u32,
    count: u32,
    stack_add: bool,
    random_property_id: u32,
) -> SMSG_ITEM_PUSH_RESULT {
    SMSG_ITEM_PUSH_RESULT {
        guid: Guid::new(player_guid),
        source: NewItemSource::Looted,
        creation_type: NewItemCreationType::Received,
        alert_chat: NewItemChatAlert::Show,
        bag_slot,
        item_slot: if stack_add { 0xFFFF_FFFF } else { item_slot },
        item: entry,
        item_suffix_factor: 0,
        item_random_property_id: random_property_id,
        item_count: count,
    }
}

pub fn learned_proficiency(player_class: u8, learned: &[u32]) -> Proficiency {
    Proficiency::from_spellbook(player_class, |spell_id| learned.contains(&spell_id))
}

pub fn build_set_proficiency_msgs(player_class: u8, learned: &[u32]) -> [ServerOpcodeMessage; 2] {
    [
        build_weapon_proficiency_msg(player_class, learned),
        build_armor_proficiency_msg(player_class, learned),
    ]
}

pub fn build_weapon_proficiency_msg(player_class: u8, learned: &[u32]) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_SET_PROFICIENCY(SMSG_SET_PROFICIENCY {
        class: ItemClass::Weapon,
        item_sub_class_mask: learned_proficiency(player_class, learned).weapon_subclass_mask(),
    })
}

pub fn build_armor_proficiency_msg(player_class: u8, learned: &[u32]) -> ServerOpcodeMessage {
    ServerOpcodeMessage::SMSG_SET_PROFICIENCY(SMSG_SET_PROFICIENCY {
        class: ItemClass::Armor,
        item_sub_class_mask: learned_proficiency(player_class, learned).armor_subclass_mask(),
    })
}

#[cfg(test)]
mod tests;
