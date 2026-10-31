use super::*;
use lyracore_shared::item::{ItemRefusal, Proficiency};
pub use wow_world_messages::vanilla::SellItemResult;
use wow_world_messages::vanilla::{
    SMSG_ITEM_ENCHANT_TIME_UPDATE, SMSG_ITEM_NAME_QUERY_RESPONSE, SMSG_SELL_ITEM,
};

pub fn inventory_slot(bag: u8, slot: u8) -> Option<u8> {
    match (bag, slot) {
        (255, 0..=62) => Some(slot),
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
        ItemRefusal::NotRightNow => SMSG_INVENTORY_CHANGE_FAILURE::CantDoRightNow {
            item1,
            item2,
            bag_type_subclass,
        },
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
            .set_item_contained(owner)
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
            .set_item_contained(owner)
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
mod tests {
    use super::*;

    fn framed(message: ServerOpcodeMessage) -> Vec<u8> {
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        wire
    }

    #[test]
    fn sell_refusal_is_a_seventeen_byte_body_under_its_size_header() {
        let reply = build_sell_item_refusal(
            0xF130_0000_0000_0001,
            0x4000_0000_0000_0007,
            SellItemResult::CantSellItem,
        );
        assert_eq!(
            framed(ServerOpcodeMessage::SMSG_SELL_ITEM(Box::new(reply))),
            [
                0x00, 0x13, 0xA1, 0x01, // size 19 (big endian), opcode 0x01A1
                0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0xF1, // vendor guid
                0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, // item guid
                0x02, // CantSellItem
            ]
        );
    }

    #[test]
    fn item_name_response_is_the_entry_then_a_terminated_name() {
        let reply = build_item_name_query_response(25, "Axe".into());
        assert_eq!(
            framed(ServerOpcodeMessage::SMSG_ITEM_NAME_QUERY_RESPONSE(
                Box::new(reply)
            )),
            [
                0x00, 0x0A, 0xC5, 0x02, // size 10, opcode 0x02C5
                0x19, 0x00, 0x00, 0x00, // item entry 25
                b'A', b'x', b'e', 0x00,
            ]
        );
    }

    #[test]
    fn read_item_failure_carries_the_guid_and_a_closing_reason() {
        let (opcode, body) = build_read_item_failed_raw(0x4000_0000_0000_0009);
        assert_eq!(opcode, 0x00AF);
        assert_eq!(body, [0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x02]);
    }

    #[test]
    fn hearthstone_query_names_the_client_use_spell() {
        let template = ItemTemplateView {
            entry: 6948,
            class: 15,
            spellid_1: 8690,
            spelltrigger_1: 0,
            ..Default::default()
        };
        let reply = build_item_query_response(6948, Some(&template));
        let message = ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(Box::new(reply));
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        let ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(reply) =
            ServerOpcodeMessage::read_unencrypted(&mut wire.as_slice()).unwrap()
        else {
            panic!("expected item query response");
        };
        let spell = reply.found.unwrap().spells[0];
        assert_eq!(spell.spell, 8690);
        assert_eq!(spell.spell_trigger, SpellTriggerType::OnUse);
    }

    fn charter(enchantment: u32) -> ItemInstanceView {
        ItemInstanceView {
            guid: 0x4000_0000_0000_0101,
            entry: lyracore_shared::guild::GUILD_CHARTER_ENTRY,
            owner_guid: 1,
            slot: 23,
            stack_count: 1,
            enchantment,
            ..ItemInstanceView::default()
        }
    }

    fn item_mask(inst: &ItemInstanceView) -> wow_world_messages::vanilla::UpdateItem {
        match build_item_create_object(inst).objects.as_slice() {
            [Object::CreateObject {
                mask2: UpdateMask::Item(item),
                ..
            }] => item.clone(),
            other => panic!("expected one Item CREATE, got {other:?}"),
        }
    }

    #[test]
    fn a_charter_create_carries_its_petition_id_in_the_first_enchantment_word() {
        assert_eq!(item_mask(&charter(42)).item_enchantment(), Some(42));
    }

    #[test]
    fn a_permanent_enchant_create_carries_the_client_enchantment_id_in_word_22() {
        let bracer = ItemInstanceView {
            entry: 25,
            slot: 8,
            enchantment: 823,
            ..charter(0)
        };
        assert_eq!(item_mask(&bracer).item_enchantment(), Some(823));
    }

    #[test]
    fn an_item_without_an_enchantment_leaves_the_word_unwritten() {
        assert_eq!(item_mask(&charter(0)).item_enchantment(), None);
    }

    #[test]
    fn a_second_charter_answers_cant_carry_more_of_this() {
        let message = ServerOpcodeMessage::SMSG_INVENTORY_CHANGE_FAILURE(Box::new(
            build_cant_carry_more_of_this(),
        ));
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        let mut expected = vec![0x11];
        expected.extend_from_slice(&[0; 16]);
        expected.push(0);
        assert_eq!(u16::from_le_bytes([wire[2], wire[3]]), 0x0112);
        assert_eq!(&wire[4..], expected.as_slice());
    }

    #[test]
    fn the_petition_values_update_sets_only_word_twenty_two_of_the_charter() {
        let (opcode, body) = build_charter_petition_values(0x4000_0000_0000_0101, 42);
        assert_eq!(opcode, 0x00A9);
        let updates = lyracore_shared::values_mask::parse_values_updates(&body);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].guid, 0x4000_0000_0000_0101);
        assert_eq!(updates[0].fields, vec![(22, 42)]);
    }

    fn suffix_item(random_property_enchant_ids: [u32; 3]) -> ItemInstanceView {
        ItemInstanceView {
            guid: 0x4000_0000_0000_0102,
            entry: 25,
            owner_guid: 1,
            slot: 23,
            stack_count: 1,
            random_property_id: 22,
            random_property_enchant_ids,
            ..ItemInstanceView::default()
        }
    }

    fn enchantment_slot_fields(inst: &ItemInstanceView) -> Vec<(u16, u32)> {
        let (opcode, body) = build_enchantment_slot_values(inst).expect("a slot update");
        assert_eq!(opcode, 0x00A9);
        let updates = lyracore_shared::values_mask::parse_values_updates(&body);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].guid, inst.guid);
        updates[0].fields.clone()
    }

    /// Enchantment slot k uses word 22 + 3k in the vanilla item descriptor.
    #[test]
    fn a_random_property_fills_enchantment_slots_three_to_five() {
        assert_eq!(
            enchantment_slot_fields(&suffix_item([73, 74, 75])),
            vec![(31, 73), (34, 74), (37, 75)]
        );
    }

    #[test]
    fn a_single_enchant_random_property_leaves_the_other_slots_unwritten() {
        assert_eq!(
            enchantment_slot_fields(&suffix_item([73, 0, 0])),
            vec![(31, 73)]
        );
    }

    #[test]
    fn an_item_without_property_or_temporary_enchantments_sends_no_slot_update() {
        assert_eq!(build_enchantment_slot_values(&suffix_item([0; 3])), None);
    }

    const INSTANT_POISON: TemporaryEnchantmentView = TemporaryEnchantmentView {
        enchantment: 323,
        duration_ms: 1_800_000,
        charges: 40,
    };

    #[test]
    fn a_temporary_enchantment_fills_slot_one_with_its_id_duration_and_charges() {
        let dagger = ItemInstanceView {
            temporary_enchantment: INSTANT_POISON,
            ..suffix_item([73, 0, 0])
        };
        assert_eq!(
            enchantment_slot_fields(&dagger),
            vec![(25, 323), (26, 1_800_000), (27, 40), (31, 73)]
        );
    }

    #[test]
    fn clearing_a_temporary_enchantment_zeroes_all_three_words() {
        let (_, body) = build_temporary_enchantment_values(7, TemporaryEnchantmentView::default());
        let updates = lyracore_shared::values_mask::parse_values_updates(&body);
        assert_eq!(updates[0].fields, vec![(25, 0), (26, 0), (27, 0)]);
    }

    #[test]
    fn the_enchant_time_update_names_slot_one_in_seconds() {
        let message = ServerOpcodeMessage::SMSG_ITEM_ENCHANT_TIME_UPDATE(Box::new(
            build_item_enchant_time_update(0x4000_0000_0000_0102, 1, 1800),
        ));
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        assert_eq!(u16::from_le_bytes([wire[2], wire[3]]), 0x01EB);
        let mut expected = 0x4000_0000_0000_0102u64.to_le_bytes().to_vec();
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend_from_slice(&1800u32.to_le_bytes());
        expected.extend_from_slice(&1u64.to_le_bytes());
        assert_eq!(&wire[4..], expected.as_slice());
    }
}
