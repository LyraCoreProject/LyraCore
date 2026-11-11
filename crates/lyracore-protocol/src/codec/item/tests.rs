use super::*;
use crate::codec::mail::{build_mail_list, MailView};
use crate::codec::test_fixtures::proficiency;
use crate::codec::values::build_inv_slot_values;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{
    AllowedClass, AllowedRace, BagFamily, Bonding, Faction, InventoryType, ItemClassAndSubClass,
    ItemFlag, ItemQuality, Level, Object, ObjectType, ServerMessage, SheatheType, Skill,
    SpellTriggerType, UpdateMask,
};
use wow_world_messages::Guid;

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
    let updates = crate::codec::values_mask::parse_values_updates(&body);
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
    let updates = crate::codec::values_mask::parse_values_updates(&body);
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
    let updates = crate::codec::values_mask::parse_values_updates(&body);
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

fn worn_shortsword() -> ItemTemplateView {
    ItemTemplateView {
        entry: 25,
        class: 2,
        subclass: 7,
        sheath: 3,
        name: "Worn Shortsword".to_string(),
        display_id: 1542,
        quality: 0,
        inventory_type: 21,
        item_level: 2,
        required_level: 1,
        max_durability: 20,
        buy_price: 35,
        sell_price: 7,
        max_stack: 1,
        damage_min: 1.0,
        damage_max: 3.0,
        delay_ms: 1900,
        ..Default::default()
    }
}

fn blackrock_gauntlets() -> ItemTemplateView {
    ItemTemplateView {
        entry: 1448,
        class: 4,
        subclass: 1,
        name: "Blackrock Gauntlets".to_string(),
        display_id: 7836,
        quality: 1,
        inventory_type: 10,
        item_level: 20,
        required_level: 15,
        max_durability: 30,
        buy_price: 2025,
        sell_price: 405,
        max_stack: 1,
        stat_armor: 105,
        stat_strength: 3,
        bonding: 2,
        ..Default::default()
    }
}

fn bound_on_pickup_relic() -> ItemTemplateView {
    ItemTemplateView {
        entry: 90001,
        class: 12,
        subclass: 0,
        name: "Ancient Relic".to_string(),
        display_id: 1542,
        quality: 1,
        inventory_type: 0,
        item_level: 1,
        required_level: 1,
        max_durability: 0,
        buy_price: 0,
        sell_price: 0,
        max_stack: 1,
        bonding: 1,
        ..Default::default()
    }
}

#[test]
fn item_query_response_maps_worn_shortsword_and_serializes() {
    let t = worn_shortsword();
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            assert_eq!(m.item, 25);
            let found = m.found.expect("Worn Shortsword must be found");
            assert_eq!(found.name1, "Worn Shortsword");
            assert_eq!(
                found.class_and_sub_class,
                ItemClassAndSubClass::OneHandedSword
            );
            assert_eq!(found.quality, ItemQuality::Poor);
            assert_eq!(found.inventory_type, InventoryType::WeaponMainHand);
            assert_eq!(found.max_durability, 20);
            assert_eq!(found.item_level, Level::new(2));
            assert_eq!(found.sheathe_type, SheatheType::LargeWeaponLeft);
            assert_eq!(found.sheathe_type.as_int(), 3);
            assert_eq!(found.damages[0].damage_minimum, 1.0);
            assert_eq!(found.damages[0].damage_maximum, 3.0);
            assert_eq!(found.delay, 1900);
            assert_eq!(found.ranged_range_modification, 100.0);
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

#[test]
fn item_query_response_armor_and_stats_reach_wire() {
    use wow_world_messages::vanilla::ItemStatType;
    let t = blackrock_gauntlets();
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Blackrock Gauntlets must be found");
            assert_eq!(found.armor, 105, "stat_armor must reach the wire");
            assert_eq!(
                found.stats[0].stat_type,
                ItemStatType::Strength,
                "first stat pair must be Strength"
            );
            assert_eq!(found.stats[0].value, 3, "Strength value must be 3");
            assert_eq!(
                found.stats[1].value, 0,
                "second stat slot must be padded to 0"
            );
            assert_eq!(found.block, 0, "non-shield block must be 0");
            assert_eq!(
                found.bonding,
                Bonding::Equip,
                "BoE template must carry bonding=2 (Equip) over the wire"
            );
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

#[test]
fn item_query_response_bop_template_carries_bonding_pickup() {
    let t = bound_on_pickup_relic();
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Ancient Relic must be found");
            assert_eq!(
                found.bonding,
                Bonding::PickUp,
                "BoP template must carry bonding=1 (PickUp) over the wire"
            );
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

#[test]
fn item_query_response_shield_carries_its_own_sheathe_byte() {
    let t = ItemTemplateView {
        entry: 1166,
        class: 4,
        subclass: 6,
        inventory_type: 14,
        sheath: 4,
        name: "Dented Buckler".to_string(),
        ..worn_shortsword()
    };
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Dented Buckler must be found");
            assert_eq!(
                found.sheathe_type.as_int(),
                4,
                "shield sheath byte must ride through unremapped"
            );
            assert_eq!(found.sheathe_type, SheatheType::LargeWeaponRight);
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

#[test]
fn item_query_response_default_bonding_is_no_bind() {
    let t = worn_shortsword();
    assert_eq!(t.bonding, 0);
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Worn Shortsword must be found");
            assert_eq!(found.bonding, Bonding::NoBind);
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

fn ring_of_fire_resistance() -> ItemTemplateView {
    ItemTemplateView {
        entry: 90002,
        class: 4,
        subclass: 0,
        name: "Ring of Fire Resistance".to_string(),
        display_id: 5001,
        quality: 2,
        inventory_type: 11,
        item_level: 15,
        required_level: 10,
        max_durability: 0,
        buy_price: 500,
        sell_price: 100,
        max_stack: 1,
        holy_res: 0,
        fire_res: 12,
        nature_res: 0,
        frost_res: 0,
        shadow_res: 0,
        arcane_res: 0,
        spellid_3: 12345,
        spelltrigger_3: 1,
        spellid_4: 12346,
        spelltrigger_4: 2,
        spellid_5: 12347,
        spelltrigger_5: 0,
        required_skill: 165,
        required_skill_rank: 150,
        required_reputation_faction: 69,
        required_reputation_rank: 3,
        max_count: 1,
        item_flags: 0x200,
        page_text: 999,
        start_quest: 777,
        bag_family: 6,
        allowed_class: 0x8000_0002,
        allowed_race: 0x8000_0004,
        random_property: 509_0100,
        ..Default::default()
    }
}

#[test]
fn item_query_preserves_the_five_spell_category_overrides() {
    let mut item = ring_of_fire_resistance();
    item.spell_categories = [30, 94, 1141, 0, 1160];
    let response = build_item_query_response(item.entry, Some(&item));
    let found = response.found.unwrap();
    assert_eq!(
        found.spells.map(|spell| spell.spell_category),
        [30, 94, 1141, 0, 1160]
    );
}

#[test]
fn item_query_response_carries_resistances_extra_spells_and_requirements() {
    let t = ring_of_fire_resistance();
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Ring of Fire Resistance must be found");
            assert_eq!(found.random_property, 509_0100);
            assert_eq!(found.holy_resistance, 0);
            assert_eq!(found.fire_resistance, 12, "fire_res must reach the wire");
            assert_eq!(found.nature_resistance, 0);
            assert_eq!(found.frost_resistance, 0);
            assert_eq!(found.shadow_resistance, 0);
            assert_eq!(found.arcane_resistance, 0);
            assert_eq!(
                found.spells[2].spell, 12345,
                "spell slot 3 must reach the wire"
            );
            assert_eq!(found.spells[2].spell_trigger, SpellTriggerType::OnEquip);
            assert_eq!(
                found.spells[3].spell, 12346,
                "spell slot 4 must reach the wire"
            );
            assert_eq!(found.spells[3].spell_trigger, SpellTriggerType::ChanceOnHit);
            assert_eq!(
                found.spells[4].spell, 12347,
                "spell slot 5 must reach the wire"
            );
            assert_eq!(found.spells[4].spell_trigger, SpellTriggerType::OnUse);
            assert_eq!(found.required_skill, Skill::Leatherworking);
            assert_eq!(found.required_skill_rank, 150);
            assert_eq!(
                found.required_faction,
                Faction::Darnassus,
                "required_faction (69) must reach the wire"
            );
            assert_eq!(found.required_faction_rank, 3);
            assert_eq!(found.max_count, 1, "max_count must reach the wire");
            assert_eq!(
                found.flags,
                ItemFlag::new(0x200),
                "item_flags must reach the wire"
            );
            assert_eq!(found.page_text, 999);
            assert_eq!(found.start_quest, 777);
            assert_eq!(found.bag_family, BagFamily::Herbs);
            assert_eq!(found.allowed_class, AllowedClass::new(0x8000_0002));
            assert_eq!(found.allowed_race, AllowedRace::new(0x8000_0004));
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

#[test]
fn item_query_response_default_template_keeps_resistances_extra_spells_and_requirements_zero() {
    let t = worn_shortsword();
    let msg = build_item_query_response(t.entry, Some(&t));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ITEM_QUERY_SINGLE_RESPONSE(m) => {
            let found = m.found.expect("Worn Shortsword must be found");
            assert_eq!(found.holy_resistance, 0);
            assert_eq!(found.fire_resistance, 0);
            assert_eq!(found.nature_resistance, 0);
            assert_eq!(found.frost_resistance, 0);
            assert_eq!(found.shadow_resistance, 0);
            assert_eq!(found.arcane_resistance, 0);
            assert_eq!(found.spells[2].spell, 0);
            assert_eq!(found.spells[3].spell, 0);
            assert_eq!(found.spells[4].spell, 0);
            assert_eq!(found.required_skill, Skill::None);
            assert_eq!(found.required_skill_rank, 0);
            assert_eq!(found.max_count, 0);
            assert_eq!(found.flags, ItemFlag::empty());
            assert_eq!(found.page_text, 0);
            assert_eq!(found.start_quest, 0);
            assert_eq!(found.bag_family, BagFamily::None);
        }
        other => panic!("expected SMSG_ITEM_QUERY_SINGLE_RESPONSE, got {other}"),
    }
}

#[test]
fn item_query_response_unknown_entry_is_not_found() {
    let msg = build_item_query_response(9999, None);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(matches!(
        benilla_protocol::messages::parse_server(0x0058, &buf[4..]).unwrap(),
        benilla_protocol::ServerPacket::ItemQueryResponse {
            entry: 9999,
            info: None
        }
    ));
}

#[test]
fn item_create_object_is_item_typed_and_serializes() {
    let inst = ItemInstanceView {
        guid: lyracore_shared::guid::compose(lyracore_shared::guid::ITEM, (1 << 8) | 23),
        entry: 25,
        owner_guid: 1,
        contained_guid: None,
        soulbound: false,
        slot: 23,
        stack_count: 1,
        durability: 20,
        max_durability: 20,
        container_slots: 0,
        random_property_id: 509_0101,
        random_property_enchant_ids: [0; 3],
        item_text_id: 0,
        enchantment: 0,
        temporary_enchantment: TemporaryEnchantmentView::default(),
    };
    let msg = build_item_create_object(&inst);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject {
            object_type,
            mask2: UpdateMask::Item(it),
            ..
        } => {
            assert_eq!(*object_type, ObjectType::Item);
            assert_eq!(it.object_entry(), Some(25));
            assert_eq!(it.item_owner(), Some(Guid::new(1)));
            assert_eq!(it.item_contained(), Some(Guid::new(1)));
            assert_eq!(it.item_maxdurability(), Some(20));
            assert_eq!(it.item_random_properties_id(), Some(509_0101));
        }
        other => panic!("expected an Item CreateObject, got {other:?}"),
    }
}

#[test]
fn a_copied_letters_create_block_carries_its_item_text_id() {
    let inst = ItemInstanceView {
        guid: lyracore_shared::guid::compose(lyracore_shared::guid::ITEM, (1 << 8) | 23),
        entry: 8383,
        owner_guid: 1,
        contained_guid: None,
        soulbound: false,
        slot: 23,
        stack_count: 1,
        durability: 0,
        max_durability: 0,
        container_slots: 0,
        random_property_id: 0,
        random_property_enchant_ids: [0; 3],
        item_text_id: 7,
        enchantment: 0,
        temporary_enchantment: TemporaryEnchantmentView::default(),
    };
    match &build_item_create_object(&inst).objects[0] {
        Object::CreateObject {
            mask2: UpdateMask::Item(it),
            ..
        } => assert_eq!(it.item_item_text_id(), Some(7)),
        other => panic!("expected an Item CreateObject, got {other:?}"),
    }
    let plain = ItemInstanceView {
        item_text_id: 0,
        ..inst
    };
    match &build_item_create_object(&plain).objects[0] {
        Object::CreateObject {
            mask2: UpdateMask::Item(it),
..
        } => assert_eq!(
            it.item_item_text_id(),
            None,
            "a plain item must leave ITEM_FIELD_ITEM_TEXT_ID unset, byte-identical to before Letter Copy"
        ),
        other => panic!("expected an Item CreateObject2, got {other:?}"),
    }
}

#[test]
fn item_create_object_bag_slots_build_a_container_with_num_slots() {
    let inst = ItemInstanceView {
        guid: lyracore_shared::guid::compose(lyracore_shared::guid::ITEM, (1 << 8) | 19),
        entry: 5000,
        owner_guid: 1,
        contained_guid: None,
        soulbound: false,
        slot: 19,
        stack_count: 1,
        durability: 0,
        max_durability: 0,
        container_slots: 8,
        random_property_id: 509_0101,
        random_property_enchant_ids: [0; 3],
        item_text_id: 0,
        enchantment: 0,
        temporary_enchantment: TemporaryEnchantmentView::default(),
    };
    let msg = build_item_create_object(&inst);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject {
            object_type,
            mask2: UpdateMask::Container(c),
            ..
        } => {
            assert_eq!(
                *object_type,
                ObjectType::Container,
                "a bag must build a CONTAINER object"
            );
            assert_eq!(c.object_entry(), Some(5000));
            assert_eq!(c.item_random_properties_id(), Some(509_0101));
            assert_eq!(c.item_owner(), Some(Guid::new(1)));
            assert_eq!(
                c.container_num_slots(),
                Some(8),
                "the bag window needs its slot count"
            );
        }
        other => panic!("expected a Container CreateObject, got {other:?}"),
    }
}

#[test]
fn set_proficiency_pins_both_masks_on_the_wire() {
    let msgs = build_set_proficiency_msgs(1, &[750, 196, 198, 201]);
    let bytes = |msg: &ServerOpcodeMessage| {
        let mut buf = Vec::new();
        proficiency(msg).write_unencrypted_server(&mut buf).unwrap();
        buf
    };
    assert_eq!(
        bytes(&msgs[0]),
        vec![0x00, 0x07, 0x27, 0x01, 2, 0x91, 0x40, 0x10, 0x00],
        "weapon mask = axe (0), mace (4), sword (7), misc (14), fishing pole (20)"
    );
    assert_eq!(
        bytes(&msgs[1]),
        vec![0x00, 0x07, 0x27, 0x01, 4, 0x5F, 0x00, 0x00, 0x00],
        "armor mask = misc|cloth|leather|mail|plate|shield"
    );
    let armor = proficiency(&msgs[1]).item_sub_class_mask;
    for subclass in [0u8, 1, 2, 3, 4, 6] {
        assert!(armor & (1 << subclass) != 0, "armor subclass {subclass}");
    }
    assert_eq!(armor & (1 << 5), 0, "subclass 5 is not an armor tier");
}

#[test]
fn the_armor_mask_follows_the_trained_passives_per_class() {
    let mask = |player_class, learned: &[u32]| {
        let msg = build_armor_proficiency_msg(player_class, learned);
        assert_eq!(proficiency(&msg).class.as_int(), 4);
        proficiency(&msg).item_sub_class_mask
    };
    const MAIL: u32 = 1 << 3;
    const PLATE: u32 = 1 << 4;

    assert_eq!(mask(1, &[]) & MAIL, MAIL);
    assert_eq!(mask(1, &[]) & PLATE, 0);
    assert_eq!(mask(1, &[750]) & (MAIL | PLATE), MAIL | PLATE);
    assert_eq!(mask(8, &[]), 0b11);
    assert_eq!(mask(8, &[750, 8737]), 0b11);
    assert_eq!(mask(3, &[]) & MAIL, 0);
    assert_eq!(mask(3, &[8737]) & MAIL, MAIL);
}

#[test]
fn the_weapon_mask_follows_the_learned_weapon_spells() {
    let mask = |learned: &[u32]| {
        let msg = build_weapon_proficiency_msg(1, learned);
        assert_eq!(proficiency(&msg).class.as_int(), 2);
        proficiency(&msg).item_sub_class_mask
    };
    const SWORD_2H: u32 = 1 << 8;

    assert_eq!(mask(&[201]) & SWORD_2H, 0);
    assert_eq!(mask(&[201, 202]) & SWORD_2H, SWORD_2H);
}

#[test]
fn item_push_and_mail_keep_the_instance_property() {
    let push = build_item_push_result(1, 0, 23, 25, 1, false, 117);
    assert_eq!(push.item_random_property_id, 117);
    let mail = MailView {
        item_entry: 25,
        random_property_id: 117,
        ..Default::default()
    };
    assert_eq!(
        build_mail_list(&[mail], 0).mails[0].item_random_property_id,
        117
    );
}

#[test]
fn item_guid_allocation_bits_survive_item_and_container_wire_updates() {
    for guid in [
        0x4000_8000_0000_0800,
        0x4000_8000_3b9a_ca01,
        0x4000_ffff_ffff_ffff,
    ] {
        for container_slots in [0, 8] {
            let item = ItemInstanceView {
                guid,
                entry: 25,
                owner_guid: 3_000_000_001,
                slot: 23,
                stack_count: 1,
                container_slots,
                ..Default::default()
            };
            let mut bytes = Vec::new();
            build_item_create_object(&item)
                .write_unencrypted_server(&mut bytes)
                .unwrap();
            let ServerOpcodeMessage::SMSG_UPDATE_OBJECT(decoded) =
                ServerOpcodeMessage::read_unencrypted(&mut bytes.as_slice()).unwrap()
            else {
                panic!("expected item object update");
            };
            match &decoded.objects[0] {
                Object::CreateObject {
                    guid3,
                    object_type: ObjectType::Item,
                    mask2: UpdateMask::Item(fields),
                    ..
                } if container_slots == 0 => {
                    assert_eq!(guid3.guid(), guid);
                    assert_eq!(fields.object_guid(), Some(Guid::new(guid)));
                }
                Object::CreateObject {
                    guid3,
                    object_type: ObjectType::Container,
                    mask2: UpdateMask::Container(fields),
                    ..
                } if container_slots == 8 => {
                    assert_eq!(guid3.guid(), guid);
                    assert_eq!(fields.object_guid(), Some(Guid::new(guid)));
                }
                other => panic!("expected item or container, got {other:?}"),
            }
            let mut slot_bytes = Vec::new();
            build_inv_slot_values(item.owner_guid, item.slot, guid)
                .unwrap()
                .write_unencrypted_server(&mut slot_bytes)
                .unwrap();
            assert!(slot_bytes
                .windows(8)
                .any(|bytes| bytes == guid.to_le_bytes()));
        }
    }
}
