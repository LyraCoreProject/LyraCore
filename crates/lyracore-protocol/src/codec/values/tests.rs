use super::*;
use crate::codec::item::ItemInstanceView;
use crate::codec::test_fixtures::{ranged_sheet_stats, worn_item};
use wow_world_messages::vanilla::{
    ItemSlot, Object, ServerMessage, UpdateMask, UpdatePlayer, VisibleItemIndex, SMSG_UPDATE_OBJECT,
};
use wow_world_messages::Guid;

#[test]
fn every_values_builder_serializes_without_the_object_type_bit() {
    let g = 0x9u64;
    let msgs: Vec<(&str, SMSG_UPDATE_OBJECT)> = vec![
        ("health", build_health_values(g, 41)),
        (
            "player_flags",
            build_player_flags_values(g, Some(0x10), None),
        ),
        ("dynamic_flags", build_dynamic_flags_values(g, 1)),
        ("unit_flags", build_unit_flags_values(g, 0x80000)),
        (
            "taxi_presentation",
            build_taxi_presentation_values(g, 1147, 0x0010_0000),
        ),
        ("mount_display", build_mount_display_values(g, 1147)),
        ("display", build_display_values(g, 1825)),
        ("power", build_power_values(g, 0, 55)),
        ("target", build_target_values(g, 0xF130_0000_0000_0001)),
        ("max_vitals", build_max_vitals_values(g, 100, 0, 200)),
        ("armor", build_armor_values(g, [60, 0, 0, 0, 0, 0, 0], 20)),
        (
            "resistance",
            build_resistance_values(g, [40, 0, 0, 0, 0, 0, 0]),
        ),
        ("coinage", build_coinage_values(g, 900)),
        ("ammo", build_player_ammo_id_values(g, 2512)),
        (
            "owner_summon",
            build_owner_summon_values(g, 0xF130_0000_0000_0009),
        ),
        (
            "skill",
            build_skill_values(g, 3, Skill::Fishing, 100, 150).unwrap(),
        ),
        ("talent_points", build_talent_points_values(g, 2)),
        ("xp", build_player_xp_values(g, 400, 1000)),
        (
            "inv_slot",
            build_inv_slot_values(g, 15, 0x4000_0000_0000_0001).unwrap(),
        ),
        (
            "visible_item",
            build_visible_item_values(
                g,
                15,
                &ItemInstanceView {
                    entry: 25,
                    random_property_id: 117,
                    enchantment: 823,
                    ..Default::default()
                },
            )
            .unwrap(),
        ),
        ("item", build_item_values(0x4000_0000_0000_0001, 5, 70)),
        (
            "item_enchantment",
            build_item_enchantment_values(0x4000_0000_0000_0001, 823),
        ),
    ];
    for (name, m) in msgs {
        let mut buf = Vec::new();
        m.write_unencrypted_server(&mut buf).unwrap();
        let body = &buf[4..];
        let updates = lyracore_shared::values_mask::parse_values_updates(body);
        assert!(
            !updates.is_empty(),
            "{name}: no VALUES block decoded — framing drift?"
        );
        for u in &updates {
            assert!(
                !u.fields.iter().any(|&(idx, _)| idx == 2),
                "{name}: OBJECT_FIELD_TYPE (bit 2) leaked into a VALUES partial — dirty_reset missing (5875 crash class)"
            );
            assert!(!u.fields.is_empty(), "{name}: empty VALUES mask");
        }
    }
}

/// OBJECT_FIELD_TYPE is field 2: bit 2 of the first mask word in a one-object VALUES body.
fn values_mask_has_object_type(body: &[u8]) -> bool {
    assert_eq!(
        &body[..4],
        &1u32.to_le_bytes(),
        "one object; body {body:02x?}"
    );
    assert_eq!(body[5], 0, "VALUES update type; body {body:02x?}");
    let blocks = 7 + body[6].count_ones() as usize;
    assert!(body[blocks] > 0, "the mask has a block; body {body:02x?}");
    let first = u32::from_le_bytes(body[blocks + 1..blocks + 5].try_into().unwrap());
    first & (1 << 2) != 0
}

fn assert_no_object_type(body: &[u8]) {
    assert!(
        !values_mask_has_object_type(body),
        "OBJECT_FIELD_TYPE must not be re-sent in a partial VALUES update; body {body:02x?}"
    );
}

#[test]
fn the_object_type_probe_detects_a_mask_that_keeps_field_2() {
    let msg = SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(1),
            mask1: UpdateMask::Player(UpdatePlayer::builder().set_player_xp(450).finalize()),
        }],
    };
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(values_mask_has_object_type(&buf[4..]));
}

#[test]
fn raw_values_body_matches_gtker_envelope() {
    use super::update_mask::{self, idx, AuraSlot};
    let (guid, spell_id, level, flags) = (0x42u64, 6673u32, 1u8, 0x09u8);

    let mut player = UpdatePlayer::builder()
        .set_unit_aura(spell_id as i32)
        .set_unit_auraflags(flags, 0, 0, 0)
        .set_unit_auralevels(level, 0, 0, 0)
        .set_unit_auraapplications(0, 0, 0, 0)
        .finalize();
    player.dirty_reset();
    player.set_unit_aura(spell_id as i32);
    player.set_unit_auraflags(flags, 0, 0, 0);
    player.set_unit_auralevels(level, 0, 0, 0);
    player.set_unit_auraapplications(0, 0, 0, 0);
    let mut gtker = Vec::new();
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(guid),
            mask1: UpdateMask::Player(player),
        }],
    }
    .write_unencrypted_server(&mut gtker)
    .unwrap();
    let gtker_opcode = u16::from_le_bytes([gtker[2], gtker[3]]);
    let gtker_body = &gtker[4..];

    let mut mask = update_mask::UpdateMaskValues::new();
    update_mask::write_auras(
        &mut mask,
        &[AuraSlot {
            slot: 0,
            spell_id,
            flags,
            level,
        }],
    );
    mask.set_u32(idx::UNIT_AURAAPPLICATIONS, 0);
    let (our_opcode, our_body) = build_values_update_raw(guid, &mask);

    assert_eq!(
        our_opcode, gtker_opcode,
        "SMSG_UPDATE_OBJECT opcode mismatch"
    );
    assert_eq!(
        our_body, gtker_body,
        "raw VALUES body must byte-match gtker's envelope+mask\n  ours:  {our_body:?}\n  gtker: {gtker_body:?}"
    );
}

#[test]
fn creature_virtual_items_use_all_three_display_fields_without_object_type() {
    let guid = 0xF130_0000_0000_002Au64;
    let (opcode, body) = build_virtual_item_values(guid, 2196, 2716, 0);
    assert_eq!(opcode, 0x00A9);

    // Two mask blocks; fields 37..=39 are bits 5..=7 of the second, with no OBJECT_FIELD_TYPE bit.
    let expected = [
        2, 0, 0, 0, 0, 0xE0, 0, 0, 0, 0x94, 0x08, 0, 0, 0x9C, 0x0A, 0, 0, 0, 0, 0, 0,
    ];
    assert!(body.ends_with(&expected), "{body:02X?}");
}

#[test]
fn health_values_update_is_health_only_no_object_type() {
    let guid = lyracore_shared::guid::creature(620, 1);
    let msg = build_health_values(guid, 20);
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), guid);
            assert!(
                matches!(mask1, UpdateMask::Unit(_)),
                "expected a Unit values mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        buf.ends_with(&[0x01, 0x00, 0x00, 0x40, 0x00, 20, 0, 0, 0]),
        "VALUES mask must be health-only (bit 22) with no OBJECT_FIELD_TYPE; tail was {:02x?}",
        &buf[buf.len().saturating_sub(16)..]
    );
}

#[test]
fn resistance_values_is_unit_only_no_object_type() {
    let guid = lyracore_shared::guid::creature(620, 1);
    let resistances = [0x1234u32, 1, 2, 3, 4, 5, 6];
    let msg = build_resistance_values(guid, resistances);
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), guid);
            assert!(
                matches!(mask1, UpdateMask::Unit(_)),
                "expected a Unit values mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        buf.ends_with(&[
            0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x00, 0x00, 0xf8, 0x03,
            0x00, 0x00, 0x00, 0x34, 0x12, 0x00, 0x00, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0,
            0, 5, 0, 0, 0, 6, 0, 0, 0,
        ]),
        "resistance VALUES must carry only indices 155..=161; tail was {:02x?}",
        &buf[buf.len().saturating_sub(56)..]
    );
    assert_no_object_type(&buf[4..]);
}

#[test]
fn dynamic_flags_values_is_unit_only_no_object_type() {
    let guid = lyracore_shared::guid::creature(620, 1);
    let msg = build_dynamic_flags_values(guid, 0x20);
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), guid);
            assert!(
                matches!(mask1, UpdateMask::Unit(_)),
                "expected a Unit values mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let contains = |needle: &[u8]| buf.windows(needle.len()).any(|w| w == needle);
    assert!(
        contains(&[0x20, 0x00, 0x00, 0x00]),
        "expected dynamic_flags value; buf {:02x?}",
        buf
    );
    assert_no_object_type(&buf[4..]);
}

#[test]
fn coinage_values_is_player_mask_without_object_type() {
    let msg = build_coinage_values(1, 0x111);
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), 1);
            assert!(
                matches!(mask1, UpdateMask::Player(_)),
                "coinage update must be a Player mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let contains = |needle: &[u8]| buf.windows(needle.len()).any(|w| w == needle);
    assert!(
        contains(&[0x11, 0x01, 0x00, 0x00]),
        "expected coinage value; buf {:02x?}",
        buf
    );
    assert_no_object_type(&buf[4..]);
}

#[test]
fn bank_purchase_values_combine_coinage_and_bank_slots_without_object_type() {
    let update = build_bank_purchase_values(5, 9_000, 0x0201_0000);
    let [Object::Values {
        guid1,
        mask1: UpdateMask::Player(fields),
    }] = update.objects.as_slice()
    else {
        panic!("expected one Character VALUES update: {update:?}");
    };
    assert_eq!(guid1.guid(), 5);
    assert_eq!(fields.player_field_coinage(), Some(9_000));
    assert_eq!(fields.player_bytes_2(), Some((0, 0xee, 1, 2)));
    let mut bytes = Vec::new();
    update.write_unencrypted_server(&mut bytes).unwrap();
    assert_no_object_type(&bytes[4..]);
}

#[test]
fn combo_points_values_set_the_combo_target_and_byte_1_of_player_field_bytes() {
    let (opcode, body) = build_combo_points_values(0x10, 0xF130_0000_0000_0042, 3);
    assert_eq!(opcode, SMSG_UPDATE_OBJECT_OPCODE);
    let mut expected = vec![1, 0, 0, 0, 0, 0, 0x01, 0x10, 39];
    let mut mask = [0u32; 39];
    mask[714 / 32] |= 1 << (714 % 32) | 1 << (715 % 32);
    mask[1222 / 32] |= 1 << (1222 % 32);
    for word in mask {
        expected.extend_from_slice(&word.to_le_bytes());
    }
    expected.extend_from_slice(&0x0000_0042u32.to_le_bytes());
    expected.extend_from_slice(&0xF130_0000u32.to_le_bytes());
    expected.extend_from_slice(&[0, 3, 0, 0]);
    assert_eq!(body, expected);
}

#[test]
fn aura_values_is_player_mask_without_object_type() {
    use super::update_mask::{self, idx, AuraSlot};
    let win = |b: &[u8], needle: &[u8]| b.windows(needle.len()).any(|w| w == needle);

    let mask = update_mask::full_aura_mask(&[AuraSlot {
        slot: 0,
        spell_id: 6673,
        flags: 0x09,
        level: 1,
    }]);
    assert!(
        mask.get(idx::OBJECT_TYPE).is_none(),
        "live aura mask must never carry OBJECT_FIELD_TYPE"
    );
    let (_op, body) = build_values_update_raw(1, &mask);
    assert!(
        win(&body, &[0x11, 0x1a, 0x00, 0x00]),
        "expected aura spell-id 6673; body {:02x?}",
        body
    );

    let clear = update_mask::full_aura_mask(&[]);
    assert!(
        clear.get(idx::OBJECT_TYPE).is_none(),
        "cleared aura mask must never carry OBJECT_FIELD_TYPE"
    );
    let (_op, clear_body) = build_values_update_raw(1, &clear);
    assert_no_object_type(&clear_body);
}

#[test]
fn ghost_values_is_player_mask_without_object_type() {
    let guid = 1;
    let msg = build_player_flags_values(guid, Some(0x10), Some(0x0100_0000));
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), guid);
            assert!(
                matches!(mask1, UpdateMask::Player(_)),
                "ghost update must be a Player mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let contains = |n: &[u8]| buf.windows(n.len()).any(|w| w == n);
    assert!(
        contains(&[0x10, 0x00, 0x00, 0x00]),
        "expected PLAYER_FLAGS_GHOST; buf {:02x?}",
        buf
    );
    assert!(
        contains(&[0x00, 0x00, 0x00, 0x01]),
        "expected unit_bytes_1 vis-ghost (byte 3); buf {:02x?}",
        buf
    );
    assert_no_object_type(&buf[4..]);
}

#[test]
fn player_flags_values_only_write_changed_fields() {
    let guid = 0x1234;
    for (flags, bytes, expected) in [
        (Some(0x20), None, vec![(190, 0x20)]),
        (None, Some(0x0100_0000), vec![(138, 0x0100_0000)]),
        (
            Some(0x20),
            Some(0x0100_0000),
            vec![(138, 0x0100_0000), (190, 0x20)],
        ),
    ] {
        let msg = build_player_flags_values(guid, flags, bytes);
        let mut packet = Vec::new();
        msg.write_unencrypted_server(&mut packet).unwrap();
        let updates = lyracore_shared::values_mask::parse_values_updates(&packet[4..]);
        assert_eq!(updates[0].fields, expected);
    }
}

#[test]
fn level_values_is_player_mask_without_object_type() {
    let msg = build_level_values(1, 2);
    match &msg.objects[0] {
        Object::Values {
            guid1,
            mask1: UpdateMask::Player(p),
        } => {
            assert_eq!(guid1.guid(), 1);
            assert_eq!(p.unit_level(), Some(2));
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert_no_object_type(&buf[4..]);
}

#[test]
fn unit_flags_values_is_unit_only_no_object_type() {
    let guid = 7u64;
    let flags = 0x0008_0000u32;
    let msg = build_unit_flags_values(guid, flags);
    match &msg.objects[0] {
        Object::Values { guid1, mask1 } => {
            assert_eq!(guid1.guid(), guid);
            assert!(
                matches!(mask1, UpdateMask::Unit(_)),
                "expected a Unit values mask"
            );
        }
        other => panic!("expected Object::Values, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let contains = |n: &[u8]| buf.windows(n.len()).any(|w| w == n);
    assert!(
        contains(&[0x00, 0x00, 0x08, 0x00]),
        "expected the unit_flags value; buf {:02x?}",
        buf
    );
    assert_no_object_type(&buf[4..]);
}

#[test]
fn power_values_selects_the_descriptor_by_power_byte() {
    let cases: [(u8, i32); 5] = [(0, 111), (1, 222), (2, 333), (3, 444), (4, 555)];
    for (power_b, cur) in cases {
        let msg = build_power_values(1, power_b, cur as u32);
        match &msg.objects[0] {
            Object::Values {
                mask1: UpdateMask::Player(p),
                ..
            } => {
                let got = match power_b {
                    1 => p.unit_power2(),
                    2 => p.unit_power3(),
                    3 => p.unit_power4(),
                    4 => p.unit_power5(),
                    _ => p.unit_power1(),
                };
                assert_eq!(
                    got,
                    Some(cur),
                    "power_b={power_b} must land on its own descriptor"
                );
            }
            other => panic!("expected a Player Values update, got {other:?}"),
        }
    }
    let msg = build_power_values(1, 1, 222);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert_no_object_type(&buf[4..]);
}

#[test]
fn target_values_is_unit_only_no_object_type() {
    let (guid, target) = (1u64, 555u64);
    let msg = build_target_values(guid, target);
    match &msg.objects[0] {
        Object::Values {
            guid1,
            mask1: UpdateMask::Unit(u),
        } => {
            assert_eq!(guid1.guid(), guid);
            assert_eq!(u.unit_target(), Some(Guid::new(target)));
        }
        other => panic!("expected a Unit Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert_no_object_type(&buf[4..]);
}

#[test]
fn max_vitals_values_sets_maxhealth_and_selects_max_power_descriptor() {
    let cases: [(u8, i32); 5] = [(0, 1001), (1, 1002), (2, 1003), (3, 1004), (4, 1005)];
    for (power_b, max_power) in cases {
        let msg = build_max_vitals_values(1, 777, power_b, max_power as u32);
        match &msg.objects[0] {
            Object::Values {
                mask1: UpdateMask::Unit(u),
                ..
            } => {
                assert_eq!(u.unit_maxhealth(), Some(777));
                let got = match power_b {
                    1 => u.unit_maxpower2(),
                    2 => u.unit_maxpower3(),
                    3 => u.unit_maxpower4(),
                    4 => u.unit_maxpower5(),
                    _ => u.unit_maxpower1(),
                };
                assert_eq!(
                    got,
                    Some(max_power),
                    "power_b={power_b} must land on its own max-power descriptor"
                );
            }
            other => panic!("expected a Unit Values update, got {other:?}"),
        }
    }
    let msg = build_max_vitals_values(1, 777, 0, 1001);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert_no_object_type(&buf[4..]);
}

#[test]
fn player_ammo_id_values_is_player_only_no_object_type() {
    let msg = build_player_ammo_id_values(1, 2512);
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.player_ammo_id(), Some(2512));
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert_no_object_type(&buf[4..]);
}

#[test]
fn self_res_spell_values_sets_only_field_1224() {
    let msg = build_self_res_spell_values(1, 3026);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let mut tail = vec![39u8];
    tail.extend(std::iter::repeat_n(0u8, 38 * 4));
    tail.extend([0x00, 0x01, 0x00, 0x00]);
    tail.extend([0xD2, 0x0B, 0x00, 0x00]);
    assert!(
        buf.ends_with(&tail),
        "the VALUES mask must carry field 1224 alone; tail was {:02x?}",
        &buf[buf.len().saturating_sub(tail.len())..]
    );

    let cleared = build_self_res_spell_values(1, 0);
    let mut buf = Vec::new();
    cleared.write_unencrypted_server(&mut buf).unwrap();
    tail.truncate(tail.len() - 4);
    tail.extend([0x00; 4]);
    assert!(buf.ends_with(&tail), "spell 0 clears the field");
}

#[test]
fn player_xp_values_carries_xp_and_next_level_no_object_type() {
    let msg = build_player_xp_values(1, 450, 1000);
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.player_xp(), Some(450));
            assert_eq!(p.player_next_level_xp(), Some(1000));
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert_no_object_type(&buf[4..]);
}

#[test]
fn item_values_clamps_zero_stack_count_to_one_and_carries_durability() {
    let msg = build_item_values(1, 0, 15);
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Item(it),
            ..
        } => {
            assert_eq!(
                it.item_stack_count(),
                Some(1),
                "a zero stack count must clamp to 1"
            );
            assert_eq!(it.item_durability(), Some(15));
        }
        other => panic!("expected an Item Values update, got {other:?}"),
    }
    let stacked = build_item_values(1, 5, 15);
    match &stacked.objects[0] {
        Object::Values {
            mask1: UpdateMask::Item(it),
            ..
        } => assert_eq!(it.item_stack_count(), Some(5)),
        other => panic!("expected an Item Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert_no_object_type(&buf[4..]);
}

#[test]
fn container_slot_values_computes_the_field_index_from_the_bag_slot() {
    use super::update_mask::{self, idx};
    let (bag_guid, item_guid) = (0x4000_0000_0000_0001u64, 0x4000_0000_0000_0099u64);
    let (opcode, body) = build_container_slot_values(bag_guid, 2, item_guid);
    assert_eq!(opcode, 0x00A9, "SMSG_UPDATE_OBJECT opcode");

    let mut want_mask = update_mask::UpdateMaskValues::new();
    want_mask.set_u64(idx::CONTAINER_FIELD_SLOT_1 + 2 * 2, item_guid);
    let (_op, want_body) = build_values_update_raw(bag_guid, &want_mask);
    assert_eq!(
        body, want_body,
        "slot 2 must set field index 54 (CONTAINER_FIELD_SLOT_1 + 2*2)"
    );

    let (_op2, cleared) = build_container_slot_values(bag_guid, 2, 0);
    let mut clear_mask = update_mask::UpdateMaskValues::new();
    clear_mask.set_u64(idx::CONTAINER_FIELD_SLOT_1 + 2 * 2, 0);
    let (_op3, want_cleared) = build_values_update_raw(bag_guid, &clear_mask);
    assert_eq!(cleared, want_cleared);
}

#[test]
fn inv_slot_values_sets_the_slot_pointer_or_none_for_an_invalid_slot() {
    let item_guid = lyracore_shared::guid::compose(lyracore_shared::guid::ITEM, (1 << 8) | 23);
    let msg = build_inv_slot_values(1, 23, item_guid).expect("slot 23 (Inventory0) is valid");
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(
                p.player_field_inv(ItemSlot::Inventory0),
                Some(Guid::new(item_guid))
            );
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    let cleared = build_inv_slot_values(1, 23, 0).unwrap();
    match &cleared.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.player_field_inv(ItemSlot::Inventory0), Some(Guid::new(0)));
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    assert!(build_inv_slot_values(1, 255, item_guid).is_none());
}

#[test]
fn aura_duration_carries_the_slot_and_remaining_window() {
    let msg = build_aura_duration(3, 12_000);
    assert_eq!(msg.aura_slot, 3);
    assert_eq!(msg.aura_duration, 12_000);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

fn assert_sheet_values_projection(stats: SheetStatsValues, expected: (i32, f32, f32)) {
    let msg = build_sheet_stats_values(1, &stats);
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.unit_attack_power(), Some(50));
            assert_eq!(p.unit_mindamage(), Some(11.0));
            assert_eq!(p.unit_maxdamage(), Some(13.0));
            assert_eq!(p.unit_ranged_attack_power(), Some(expected.0));
            assert_eq!(p.unit_ranged_attack_power_mods(), Some((0, 0)));
            assert_eq!(p.unit_minrangeddamage(), Some(expected.1));
            assert_eq!(p.unit_maxrangeddamage(), Some(expected.2));
        }
        other => panic!("expected Player Values, got {other:?}"),
    }
}

#[test]
fn sheet_values_carry_the_same_melee_and_ranged_projection() {
    assert_sheet_values_projection(ranged_sheet_stats(30, 14, 18), (30, 14.0, 18.0));
}

#[test]
fn spell_7598_updates_both_melee_and_ranged_crit_fields() {
    let mut stats = ranged_sheet_stats(180, 48, 68);
    stats.crit_pct = 7.0;
    stats.ranged_crit_pct = 7.0;
    let msg = build_sheet_stats_values(1, &stats);
    let Object::Values {
        mask1: UpdateMask::Player(player),
        ..
    } = &msg.objects[0]
    else {
        panic!("expected Player Values")
    };
    assert_eq!(player.player_crit_percentage(), Some(7.0));
    assert_eq!(player.player_ranged_crit_percentage(), Some(7.0));
}

#[test]
fn sheet_values_carry_a_flat_wand_projection() {
    assert_sheet_values_projection(ranged_sheet_stats(0, 8, 12), (0, 8.0, 12.0));
}

#[test]
fn mount_display_values_carries_the_display_it_is_given() {
    for display in [1147u32, 0] {
        let msg = crate::codec::build_mount_display_values(0x1234, display);
        match &msg.objects[0] {
            Object::Values {
                mask1: UpdateMask::Unit(unit),
                ..
            } => assert_eq!(unit.unit_mountdisplayid(), Some(display as i32)),
            other => panic!("expected a Unit Values object, got {other:?}"),
        }
    }
}

#[test]
fn display_values_carries_only_the_current_appearance() {
    for display in [1825u32, 49] {
        let msg = build_display_values(0x1234, display);
        let mut bytes = Vec::new();
        msg.write_unencrypted_server(&mut bytes).unwrap();
        let updates = lyracore_shared::values_mask::parse_values_updates(&bytes[4..]);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].guid, 0x1234);
        assert_eq!(updates[0].fields, vec![(131, display)]);
    }
}

#[test]
fn visible_item_values_render_and_clear_equipment_slots_only_work_item_087() {
    let sword = ItemInstanceView {
        random_property_id: 117,
        ..worn_item(15, 1, 25, 0)
    };
    let msg = build_visible_item_values(1, 15, &sword).expect("slot 15 (mainhand) is equipment");
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            let vi = p
                .player_visible_item(VisibleItemIndex::Index15)
                .expect("visible item set");
            assert_eq!(vi.random_property_id, 117);
            assert_eq!(
                vi.item, 25,
                "the mainhand VISIBLE_ITEM must carry the equipped entry"
            );
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    let cleared = build_visible_item_values(1, 15, &ItemInstanceView::default()).unwrap();
    match &cleared.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            let vi = p
                .player_visible_item(VisibleItemIndex::Index15)
                .expect("cleared but present");
            assert_eq!(vi.item, 0);
            assert_eq!(
                vi.enchants,
                [0, 0],
                "an unequip clears the enchant words too"
            );
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    assert!(build_visible_item_values(1, 23, &sword).is_none());
    assert!(build_visible_item_values(1, 19, &sword).is_none());
    assert!(build_visible_item_values(1, 120, &sword).is_none());
}

#[test]
fn visible_item_values_carry_the_permanent_enchant_in_the_first_enchant_word() {
    let bracer = worn_item(8, 1, 25, 823);
    let msg = build_visible_item_values(1, 8, &bracer).expect("slot 8 (wrist) is equipment");
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Player(p),
            ..
        } => {
            let vi = p
                .player_visible_item(VisibleItemIndex::Index8)
                .expect("visible item set");
            assert_eq!(vi.item, 25);
            assert_eq!(
                vi.enchants,
                [823, 0],
                "slot 0 is permanent, slot 1 temporary"
            );
        }
        other => panic!("expected a Player Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let updates = lyracore_shared::values_mask::parse_values_updates(&buf[4..]);
    let fields = &updates[0].fields;
    assert!(
        !fields.iter().any(|&(idx, _)| idx == 2),
        "OBJECT_FIELD_TYPE must stay off the partial update"
    );
    assert_eq!(fields.iter().filter(|&&(_, word)| word == 823).count(), 1);
}

#[test]
fn item_enchantment_values_set_only_the_first_enchantment_word() {
    let msg = build_item_enchantment_values(0x4000_0000_0000_0007, 823);
    match &msg.objects[0] {
        Object::Values {
            mask1: UpdateMask::Item(item),
            ..
        } => assert_eq!(item.item_enchantment(), Some(823)),
        other => panic!("expected an Item Values update, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    let updates = lyracore_shared::values_mask::parse_values_updates(&buf[4..]);
    assert_eq!(
        updates[0].fields,
        vec![(22, 823)],
        "only ITEM_FIELD_ENCHANTMENT is written, never OBJECT_FIELD_TYPE"
    );
}

#[test]
fn demoralizing_shouts_negative_attack_power_is_signed_on_the_wire() {
    let mut stats = ranged_sheet_stats(0, 0, 0);
    stats.ap_modifiers = u32::from((-35i16) as u16) << 16;
    let update = build_sheet_stats_values(1, &stats);
    let Object::Values {
        mask1: UpdateMask::Player(player),
        ..
    } = &update.objects[0]
    else {
        panic!("expected Player values");
    };
    assert_eq!(player.unit_attack_power_mods(), Some((0, (-35i16) as u16)));
}
