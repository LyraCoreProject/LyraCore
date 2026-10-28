use super::*;

use wow_world_messages::vanilla::ServerMessage;
use wow_world_messages::vanilla::{
    EnvironmentalDamageType, MSG_MOVE_HEARTBEAT_Client, SMSG_LOOT_RESPONSE_LootMethod, TimerType,
    SMSG_LOOT_RESPONSE,
};

use lyracore_shared::loot_roll::vote_kind;
use wow_world_messages::vanilla::RollVote;

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

fn warrior_entity() -> EntityView {
    EntityView {
        guid: 1,
        map_id: 0,
        zone_id: 12,
        x: -8949.95,
        y: -132.493,
        z: 83.5312,
        scale_x: 1.0,
        type_mask: 0x19,
        health: 60,
        max_health: 60,
        power: 0,
        max_power: 1000,
        level: 1,
        run_speed_mult_bp: 10_000,
        faction_template: 1,
        unit_bytes_0: 1 | (1 << 8) | (1 << 24),
        display_id: 49,
        native_display_id: 49,
        ..Default::default()
    }
}

#[test]
fn fresh_character_create_has_no_watched_faction() {
    let message = build_create_object(&warrior_entity(), CreateKind::SelfPlayer, &[], &[]).unwrap();
    let Object::CreateObject2 {
        mask2: UpdateMask::Player(fields),
        ..
    } = &message.objects[0]
    else {
        panic!("expected Character CREATE");
    };
    assert_eq!(fields.player_field_watched_faction_index(), Some(-1));
}

#[test]
fn character_create_restores_the_private_watched_faction() {
    for index in [-1, 0, 19, 63] {
        let entity = EntityView {
            watched_faction_index: Some(index),
            ..warrior_entity()
        };
        for (kind, expected) in [
            (CreateKind::SelfPlayer, Some(index)),
            (CreateKind::Peer, None),
        ] {
            let message = build_create_object(&entity, kind, &[], &[]).unwrap();
            let Object::CreateObject2 {
                mask2: UpdateMask::Player(fields),
                ..
            } = &message.objects[0]
            else {
                panic!("expected Character CREATE");
            };
            assert_eq!(fields.player_field_watched_faction_index(), expected);
        }
    }
}

#[test]
fn set_faction_standing_raw_sends_the_rep_index_not_the_faction_id() {
    let (opcode, body) =
        build_set_faction_standing_raw(19, 500).expect("a valid rep-index always builds");
    assert_eq!(opcode, 0x0124);
    assert_eq!(
        body.len(),
        12,
        "count(u32) + rep_index(u32) + standing(u32)"
    );
    assert_eq!(
        u32::from_le_bytes(body[0..4].try_into().unwrap()),
        1,
        "count = 1"
    );
    assert_eq!(
        u32::from_le_bytes(body[4..8].try_into().unwrap()),
        19,
        "the rep-index (19), NOT faction_id 72"
    );
    assert_eq!(
        u32::from_le_bytes(body[8..12].try_into().unwrap()) as i32,
        500,
        "standing survives"
    );
    let (_, neg) = build_set_faction_standing_raw(19, -42000).unwrap();
    assert_eq!(
        u32::from_le_bytes(neg[8..12].try_into().unwrap()) as i32,
        -42000
    );
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
fn movement_info_bytes_roundtrip() {
    use wow_world_messages::vanilla::{MovementInfo_MovementFlags, Vector3d};
    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 0x0102_0304,
        position: Vector3d {
            x: -8950.0,
            y: -130.0,
            z: 83.5,
        },
        orientation: 1.25,
        fall_time: 0.0,
    };
    let bytes = movement_info_to_bytes(&info).unwrap();
    assert_eq!(bytes.len(), 28);
    let back = bytes_to_movement_info(&bytes).unwrap();
    assert_eq!(back, info);
}

#[test]
fn client_movement_keeps_the_flag_word_and_the_millisecond_fall_time() {
    use lyracore_shared::movement_flags::{FORWARD, SWIMMING};
    let flags = FORWARD | 0x8000 | SWIMMING;
    let mut body = flags.to_le_bytes().to_vec();
    body.extend_from_slice(&[0; 20]);
    body.extend_from_slice(&0.5f32.to_le_bytes());
    body.extend_from_slice(&2000u32.to_le_bytes());
    let movement = ClientMovement::decode(&body).unwrap();
    assert_eq!(movement.flags(), flags);
    assert_eq!(movement.fall_time_ms(), 2000);
    assert!(movement.transport().is_none());
}

#[test]
fn name_query_response_maps_seeded_warrior() {
    let c = CharacterView {
        guid: 1,
        name: "Tester".into(),
        race: 1,
        class: 1,
        gender: 0,
        ..Default::default()
    };
    let r = build_name_query_response(&c).unwrap();
    assert_eq!(r.guid.guid(), 1);
    assert_eq!(r.character_name, "Tester");
    assert!(r.realm_name.is_empty());
    assert_eq!(r.race, Race::Human);
    assert_eq!(r.class, Class::Warrior);
    assert_eq!(r.gender, Gender::Male);
}

#[test]
fn health_values_update_is_health_only_no_object_type() {
    let guid = (0xF130u64 << 48) | (620u64 << 24) | 1;
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
    let guid = (0xF130u64 << 48) | (620u64 << 24) | 1;
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
    let guid = (0xF130u64 << 48) | (620u64 << 24) | 1;
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
fn loot_response_envelope_matches_gtker() {
    let guid = (0xF130u64 << 48) | (620u64 << 24) | 1;
    let (opcode, body) = build_loot_response_raw(guid, 13, &[]);
    let gtker = ServerOpcodeMessage::SMSG_LOOT_RESPONSE(Box::new(SMSG_LOOT_RESPONSE {
        guid: Guid::new(guid),
        loot_method: SMSG_LOOT_RESPONSE_LootMethod::Corpse,
        gold: Gold::new(13),
        items: Vec::new(),
    }));
    let mut framed = Vec::new();
    gtker.write_unencrypted_server(&mut framed).unwrap();
    let gtker_opcode = u16::from_le_bytes([framed[2], framed[3]]);
    assert_eq!(
        opcode, gtker_opcode,
        "raw opcode must match gtker's SMSG_LOOT_RESPONSE"
    );
    assert_eq!(
        body,
        framed[4..],
        "raw envelope must byte-match gtker's money-only loot"
    );
}

#[test]
fn loot_response_item_layout_is_full_vanilla() {
    let guid = 0xF130_0000_0000_0001;
    let (_op, body) = build_loot_response_raw(guid, 5, &[(0, 25, 2, 1542, 117)]);
    assert_eq!(
        body.len(),
        14 + 22,
        "one item must add 22 bytes (full vanilla layout)"
    );
    assert_eq!(body[13], 1, "amount_of_items == 1");
    let it = &body[14..];
    assert_eq!(it[0], 0, "loot slot index");
    assert_eq!(
        u32::from_le_bytes([it[1], it[2], it[3], it[4]]),
        25,
        "item id (entry)"
    );
    assert_eq!(
        u32::from_le_bytes([it[5], it[6], it[7], it[8]]),
        2,
        "stack count"
    );
    assert_eq!(
        u32::from_le_bytes([it[9], it[10], it[11], it[12]]),
        1542,
        "display id"
    );
    assert_eq!(u32::from_le_bytes(it[17..21].try_into().unwrap()), 117);
    assert_eq!(it[21], 0, "slot_type = TypeAllowLoot");
}

#[test]
fn loot_start_roll_carries_the_countdown_and_item() {
    let m = build_loot_start_roll(0xF130_0000_0000_0007, 3, 1234, 60_000, 117);
    assert_eq!(m.creature.guid(), 0xF130_0000_0000_0007);
    assert_eq!(m.loot_slot, 3);
    assert_eq!(m.item, 1234);
    assert_eq!(m.item_random_property_id, 117);
    assert_eq!(m.countdown_time, std::time::Duration::from_millis(60_000));
    let msg = ServerOpcodeMessage::SMSG_LOOT_START_ROLL(Box::new(m));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn wire_roll_number_signals_pass_as_128_and_passes_a_real_roll_through() {
    assert_eq!(wire_roll_number(vote_kind::PASS, 0), 128);
    assert_eq!(
        wire_roll_number(vote_kind::NEED, 87),
        87,
        "a real roll passes through unchanged"
    );
    assert_eq!(wire_roll_number(vote_kind::GREED, 42), 42);
}

#[test]
fn loot_roll_carries_the_voter_item_and_vote() {
    let m = build_loot_roll(0xF130_0000_0000_0007, 3, 55, 1234, 87, vote_kind::NEED, 117);
    assert_eq!(m.creature.guid(), 0xF130_0000_0000_0007);
    assert_eq!(m.loot_slot, 3);
    assert_eq!(m.player.guid(), 55);
    assert_eq!(m.item, 1234);
    assert_eq!(m.roll_number, 87);
    assert_eq!(m.item_random_property_id, 117);
    assert_eq!(m.vote, RollVote::Need);
    let pass = build_loot_roll(0xF130_0000_0000_0007, 3, 55, 1234, 0, vote_kind::PASS, 117);
    assert_eq!(pass.roll_number, 128);
    assert_eq!(pass.vote, RollVote::Pass);
}

#[test]
fn loot_roll_won_carries_the_winner_roll_and_winning_tier() {
    let m = build_loot_roll_won(0xF130_0000_0000_0007, 3, 1234, 55, 96, vote_kind::NEED, 0);
    assert_eq!(m.looted_target.guid(), 0xF130_0000_0000_0007);
    assert_eq!(m.loot_slot, 3);
    assert_eq!(m.item, 1234);
    assert_eq!(m.winning_player.guid(), 55);
    assert_eq!(m.winning_roll, 96);
    assert_eq!(m.vote, RollVote::Need);
    let g = build_loot_roll_won(0xF130_0000_0000_0007, 3, 1234, 55, 42, vote_kind::GREED, 0);
    assert_eq!(g.vote, RollVote::Greed);
}

#[test]
fn loot_master_list_carries_every_eligible_guid() {
    let m = build_loot_master_list(&[10, 20, 30]);
    assert_eq!(m.guids.len(), 3);
    assert_eq!(m.guids[0].guid(), 10);
    assert_eq!(m.guids[1].guid(), 20);
    assert_eq!(m.guids[2].guid(), 30);
    let msg = ServerOpcodeMessage::SMSG_LOOT_MASTER_LIST(Box::new(m));
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn loot_money_notify_carries_the_share_not_a_hardcoded_total() {
    let share = lyracore_shared::loot_roll::decode_money_share(
        &lyracore_shared::loot_roll::encode_money_share(17),
    )
    .unwrap();
    let m = build_loot_money_notify(share);
    assert_eq!(m.amount, 17);
    let msg = ServerOpcodeMessage::SMSG_LOOT_MONEY_NOTIFY(m);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn ghost_values_is_player_mask_without_object_type() {
    let guid = 1;
    let msg = build_player_flags_values(guid, 0x10, 0x0100_0000);
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
fn corpse_create_is_a_corpse_object_with_race_in_bytes1() {
    let corpse = CorpseView {
        guid: (0xF101u64 << 48) | 1,
        owner_guid: 1,
        x: -8949.0,
        y: -132.0,
        z: 83.0,
        orientation: 0.0,
        display_id: 49,
        bytes_1: (1u32 << 8) | (1u32 << 16) | (5u32 << 24),
        bytes_2: 0,
        is_bones: false,
    };
    let msg = build_corpse_create_object(&corpse);
    match &msg.objects[0] {
        Object::CreateObject2 {
            object_type, mask2, ..
        } => {
            assert!(
                matches!(object_type, ObjectType::Corpse),
                "must be a CORPSE object"
            );
            assert!(
                matches!(mask2, UpdateMask::Corpse(_)),
                "must carry an UpdateCorpse mask"
            );
        }
        other => panic!("expected Object::CreateObject2, got {other:?}"),
    }
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        buf.windows(4).any(|w| w == [0x00, 0x01, 0x01, 0x05]),
        "corpse BYTES_1 must carry race in byte 1 (not the player_bytes layout); buf {:02x?}",
        buf
    );
}

#[test]
fn corpse_create_object_flags_bones_state_work_item_201() {
    let live = CorpseView {
        guid: 1,
        owner_guid: 1,
        display_id: 49,
        is_bones: false,
        ..Default::default()
    };
    let msg = build_corpse_create_object(&live);
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Corpse(m),
            ..
        } => {
            assert_eq!(
                m.corpse_flags(),
                Some(0x04),
                "a live body must carry CORPSE_FLAG_UNK2 (0x04)"
            );
        }
        other => panic!("expected a Corpse CreateObject2, got {other:?}"),
    }

    let bones = CorpseView {
        is_bones: true,
        ..live
    };
    let msg2 = build_corpse_create_object(&bones);
    match &msg2.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Corpse(m),
            ..
        } => {
            assert_eq!(
                m.corpse_flags(),
                Some(0x01),
                "decayed bones must carry CORPSE_FLAG_BONE (0x01)"
            );
        }
        other => panic!("expected a Corpse CreateObject2, got {other:?}"),
    }
}

#[test]
fn corpse_query_found_carries_position_else_not_found() {
    match build_corpse_query_response(Some((0, -8949.0, -132.0, 83.0))).unwrap() {
        crate::codec::MSG_CORPSE_QUERY_Server::Found { position, .. } => {
            assert_eq!(position.x, -8949.0);
        }
        other => panic!("expected Found, got {other:?}"),
    }
    assert!(matches!(
        build_corpse_query_response(None).unwrap(),
        crate::codec::MSG_CORPSE_QUERY_Server::NotFound
    ));
}

#[test]
fn resurrect_request_names_the_caster_without_sickness_or_delay() {
    let (opcode, body) = build_resurrect_request_raw(42, "Priestly");
    assert_eq!(opcode, 0x015b);
    assert_eq!(
        body,
        [
            42, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, b'P', b'r', b'i', b'e', b's', b't', b'l', b'y', 0,
            0, 0
        ]
    );
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
fn attack_start_carries_attacker_and_victim() {
    let player = 1u64;
    let chicken = (0xF130u64 << 48) | (620u64 << 24) | 1;
    let msg = build_attack_start(player, chicken);
    assert_eq!(msg.attacker.guid(), player);
    assert_eq!(msg.victim.guid(), chicken);
}

#[test]
fn breath_mirror_timer_edges_carry_the_breath_timer() {
    match build_breath_timer_start(57_000, 60_000) {
        ServerOpcodeMessage::SMSG_START_MIRROR_TIMER(msg) => {
            assert_eq!(msg.timer, TimerType::Breath);
            assert_eq!(msg.time_remaining, 57_000);
            assert_eq!(msg.duration, 60_000);
            assert_eq!(msg.scale, u32::MAX, "wire bits for signed -1 drain scale");
            assert!(!msg.is_frozen);
        }
        other => panic!("expected SMSG_START_MIRROR_TIMER, got {other}"),
    }

    match build_breath_timer_stop() {
        ServerOpcodeMessage::SMSG_STOP_MIRROR_TIMER(msg) => {
            assert_eq!(msg.timer, TimerType::Breath)
        }
        other => panic!("expected SMSG_STOP_MIRROR_TIMER, got {other}"),
    }
}

#[test]
fn drowning_damage_log_carries_the_actual_damage() {
    match build_drowning_damage_log(0xF00D, 19) {
        ServerOpcodeMessage::SMSG_ENVIRONMENTAL_DAMAGE_LOG(log) => {
            assert_eq!(log.guid.guid(), 0xF00D);
            assert_eq!(log.damage_type, EnvironmentalDamageType::Drowning);
            assert_eq!(log.damage, 19);
        }
        other => panic!("expected SMSG_ENVIRONMENTAL_DAMAGE_LOG, got {other}"),
    }
}

#[test]
fn fatigue_mirror_timer_drains_recovers_and_stops() {
    for (in_deep_water, scale) in [(true, u32::MAX), (false, 10)] {
        match build_fatigue_timer_start(in_deep_water, 42_000, 60_000) {
            ServerOpcodeMessage::SMSG_START_MIRROR_TIMER(msg) => {
                assert_eq!(msg.timer, TimerType::Fatigue);
                assert_eq!(msg.time_remaining, 42_000);
                assert_eq!(msg.duration, 60_000);
                assert_eq!(msg.scale, scale);
            }
            other => panic!("expected SMSG_START_MIRROR_TIMER, got {other}"),
        }
    }
    match build_fatigue_timer_stop() {
        ServerOpcodeMessage::SMSG_STOP_MIRROR_TIMER(msg) => {
            assert_eq!(msg.timer, TimerType::Fatigue)
        }
        other => panic!("expected SMSG_STOP_MIRROR_TIMER, got {other}"),
    }
}

#[test]
fn exhaustion_damage_log_carries_the_actual_damage() {
    match build_exhaustion_damage_log(0xF00D, 212) {
        ServerOpcodeMessage::SMSG_ENVIRONMENTAL_DAMAGE_LOG(log) => {
            assert_eq!(log.guid.guid(), 0xF00D);
            assert_eq!(log.damage_type, EnvironmentalDamageType::Exhausted);
            assert_eq!(log.damage, 212);
        }
        other => panic!("expected SMSG_ENVIRONMENTAL_DAMAGE_LOG, got {other}"),
    }
}

#[test]
fn attacker_state_update_is_one_physical_normal_swing() {
    let player = 1u64;
    let chicken = (0xF130u64 << 48) | (620u64 << 24) | 1;
    let msg = build_attacker_state_update(player, chicken, 7, 0, 0, 0);
    assert_eq!(msg.hit_info, HitInfo::AffectsVictim);
    assert_eq!(msg.damage_state, 1);
    assert_eq!(msg.attacker.guid(), player);
    assert_eq!(msg.target.guid(), chicken);
    assert_eq!(msg.total_damage, 7);
    assert_eq!(msg.damages.len(), 1);
    assert_eq!(msg.damages[0].spell_school_mask, 0);
    assert_eq!(msg.damages[0].damage_uint, 7);
    assert_eq!(msg.damages[0].damage_float, 7.0);
    let server = ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(Box::new(msg));
    let mut buf = Vec::new();
    server.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(m) => {
            assert_eq!(m.total_damage, 7);
            assert_eq!(m.damages[0].damage_uint, 7);
        }
        other => panic!("expected SMSG_ATTACKERSTATEUPDATE, got {other}"),
    }
}

#[test]
fn attacker_state_update_maps_hit_info_discriminant() {
    let normal = build_attacker_state_update(1, 2, 7, 0, 0, 0);
    assert_eq!(normal.hit_info, HitInfo::AffectsVictim);
    assert_eq!(normal.damage_state, 1);
    assert_eq!(normal.blocked_amount, 0);
    let crit = build_attacker_state_update(1, 2, 14, 1, 0, 0);
    assert_eq!(crit.hit_info, HitInfo::CriticalHit);
    assert_eq!(crit.damage_state, 1);
    let miss = build_attacker_state_update(1, 2, 0, 2, 0, 0);
    assert_eq!(miss.hit_info, HitInfo::Miss);
    assert_eq!(miss.damage_state, 0);
    assert_eq!(miss.total_damage, 0);
    let dodge = build_attacker_state_update(1, 2, 0, 3, 0, 0);
    assert_eq!(dodge.hit_info, HitInfo::NormalSwing);
    assert_eq!(dodge.damage_state, 2);
    assert_eq!(dodge.total_damage, 0);
    let parry = build_attacker_state_update(1, 2, 0, 4, 0, 0);
    assert_eq!(parry.hit_info, HitInfo::NormalSwing);
    assert_eq!(parry.damage_state, 3);
    let glancing = build_attacker_state_update(1, 2, 4, 5, 0, 0);
    assert_eq!(glancing.hit_info, HitInfo::Glancing);
    assert_eq!(glancing.damage_state, 1);
    assert_eq!(glancing.total_damage, 4);
    let block = build_attacker_state_update(1, 2, 5, 7, 25, 0);
    assert_eq!(block.hit_info, HitInfo::AffectsVictim);
    assert_eq!(block.damage_state, 4);
    assert_eq!(block.total_damage, 5);
    assert_eq!(block.blocked_amount, 25);
    let ranged = build_attacker_state_update(1, 2, 9, 0, 0, 75);
    assert_eq!(ranged.spell_id, 75);
    assert_eq!(ranged.hit_info, HitInfo::AffectsVictim);
    assert_eq!(ranged.total_damage, 9);
    let server = ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(Box::new(block));
    let mut buf = Vec::new();
    server.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(m) => {
            assert_eq!(m.blocked_amount, 25);
            assert_eq!(m.damage_state, 4);
        }
        other => panic!("expected SMSG_ATTACKERSTATEUPDATE, got {other}"),
    }
}

#[test]
fn create_object_builds_unit_for_a_creature() {
    let chicken = EntityView {
        guid: (0xF130u64 << 48) | (620u64 << 24) | 1,
        map_id: 0,
        type_mask: lyracore_shared::constants::type_mask::CREATURE,
        entry: 620,
        scale_x: 1.0,
        health: 42,
        max_health: 42,
        level: 1,
        run_speed_mult_bp: 10_000,
        faction_template: 31,
        unit_bytes_0: 0x0101,
        display_id: 304,
        native_display_id: 304,
        ..Default::default()
    };
    let msg = build_create_object(&chicken, CreateKind::Peer, &[], &[]).unwrap();
    assert_eq!(msg.objects.len(), 1);
    match &msg.objects[0] {
        Object::CreateObject2 {
            guid3,
            object_type,
            mask2,
            ..
        } => {
            assert_eq!(guid3.guid(), chicken.guid);
            assert_eq!(*object_type, ObjectType::Unit);
            assert!(
                matches!(mask2, UpdateMask::Unit(_)),
                "expected a Unit update mask"
            );
        }
        other => panic!("expected CreateObject2, got {other:?}"),
    }
}

#[test]
fn creature_create_carries_npc_flags() {
    let vendor = EntityView {
        guid: (0xF130u64 << 48) | (6741u64 << 24) | 1,
        map_id: 0,
        type_mask: lyracore_shared::constants::type_mask::CREATURE,
        entry: 6741,
        scale_x: 1.0,
        health: 100,
        max_health: 100,
        level: 30,
        run_speed_mult_bp: 10_000,
        faction_template: 12,
        unit_bytes_0: 0x0101,
        display_id: 2068,
        native_display_id: 2068,
        npc_flags: 0x1 | 0x4,
        ..Default::default()
    };
    let msg = build_create_object(&vendor, CreateKind::Peer, &[], &[]).unwrap();
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Unit(u),
            ..
        } => {
            assert_eq!(u.unit_npc_flags(), Some(0x1 | 0x4));
        }
        other => panic!("expected a Unit CreateObject2, got {other:?}"),
    }
    let player = build_create_object(&warrior_entity(), CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &player.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.unit_npc_flags(), None);
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn create_object_carries_guid_position_speeds() {
    let e = warrior_entity();
    let msg = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    assert_eq!(msg.objects.len(), 1);
    match &msg.objects[0] {
        Object::CreateObject2 {
            guid3,
            object_type,
            movement2,
            ..
        } => {
            assert_eq!(guid3.guid(), 1);
            assert_eq!(*object_type, ObjectType::Player);
            let living = movement2.update_flag.get_living().expect("living block");
            let MovementBlock_UpdateFlag_Living::Living {
                living_position,
                turn_rate,
                running_speed,
                ..
            } = living
            else {
                panic!("expected Living variant");
            };
            assert_eq!(living_position.x, e.x);
            assert_eq!(*turn_rate, speeds::TURN_RATE);
            assert_eq!(*running_speed, speeds::RUN);
        }
        other => panic!("expected CreateObject2, got {other:?}"),
    }
}

#[test]
fn self_player_create_object_carries_player_controlled_unit_flag_on_the_wire() {
    let mut entity = warrior_entity();
    let player_controlled = lyracore_shared::constants::unit_flags::PLAYER_CONTROLLED;
    entity.unit_flags |= player_controlled;

    let update = build_create_object(&entity, CreateKind::SelfPlayer, &[], &[]).unwrap();
    let mut bytes = Vec::new();
    update.write_unencrypted_server(&mut bytes).unwrap();

    match ServerOpcodeMessage::read_unencrypted(&mut bytes.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update) => match &update.objects[0] {
            Object::CreateObject2 {
                mask2: UpdateMask::Player(player),
                ..
            } => {
                let wire_flags = player.unit_flags().expect("UNIT_FIELD_FLAGS is present");
                assert_ne!(
                    wire_flags & player_controlled as i32,
                    0,
                    "self-player CREATE_OBJECT2 must preserve PLAYER_CONTROLLED"
                );
            }
            other => panic!("expected a Player CreateObject2, got {other:?}"),
        },
        other => panic!("expected SMSG_UPDATE_OBJECT, got {other}"),
    }
}

#[test]
fn create_object_uses_entity_run_speed_multiplier() {
    let mut e = warrior_entity();
    e.run_speed_mult_bp = 30_000;
    let msg = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 { movement2, .. } => {
            let living = movement2.update_flag.get_living().expect("living block");
            let MovementBlock_UpdateFlag_Living::Living { running_speed, .. } = living else {
                panic!("expected Living variant");
            };
            assert_eq!(*running_speed, 21.0, "3× the vanilla 7 yd/s run speed");
        }
        other => panic!("expected CreateObject2, got {other:?}"),
    }

    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(
        !buf.is_empty(),
        "the multiplier-derived CREATE must serialize to the wire"
    );
}

#[test]
fn self_and_peer_create_carry_the_self_flag_on_only_the_self_side() {
    let e = warrior_entity();
    let me = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    let peer = build_create_object(&e, CreateKind::Peer, &[], &[]).unwrap();
    let self_flag = |msg: &SMSG_UPDATE_OBJECT| match &msg.objects[0] {
        Object::CreateObject2 { movement2, .. } => movement2.update_flag.get_self(),
        other => panic!("expected CreateObject2, got {other:?}"),
    };
    assert!(
        self_flag(&me),
        "a SelfPlayer create must carry the movement SELF flag"
    );
    assert!(
        !self_flag(&peer),
        "a Peer create must NOT carry the SELF flag"
    );
    assert_ne!(me, peer);
}

#[test]
fn warrior_create_carries_skills_and_serializes() {
    let mut entity = warrior_entity();
    entity.magic_resistances = [1, 2, 3, 4, 5, 6];
    let msg = build_create_object(&entity, CreateKind::SelfPlayer, &[], &[]).unwrap();
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(
                p.player_skill_info(SkillInfoIndex::Index1),
                Some(SkillInfo::new(Skill::Fury, 0, 1, 1, 0, 0)),
                "Fury (256) must anchor the Battle Shout spellbook tab"
            );
            let e = warrior_entity();
            assert_eq!(p.unit_strength(), Some(e.strength as i32));
            assert_eq!(p.unit_agility(), Some(e.agility as i32));
            assert_eq!(p.unit_stamina(), Some(e.stamina as i32));
            assert_eq!(p.unit_intellect(), Some(e.intellect as i32));
            assert_eq!(p.unit_spirit(), Some(e.spirit as i32));
            assert_eq!(p.unit_normal_resistance(), Some(e.effective_armor as i32));
            assert_eq!(p.unit_holy_resistance(), Some(1));
            assert_eq!(p.unit_fire_resistance(), Some(2));
            assert_eq!(p.unit_nature_resistance(), Some(3));
            assert_eq!(p.unit_frost_resistance(), Some(4));
            assert_eq!(p.unit_shadow_resistance(), Some(5));
            assert_eq!(p.unit_arcane_resistance(), Some(6));
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn char_enum_maps_seeded_warrior() {
    let view = CharacterView {
        guid: 1,
        name: "Tester".into(),
        race: 1,
        class: 1,
        gender: 0,
        level: 1,
        map_id: 0,
        zone_id: 12,
        ..Default::default()
    };
    let msg = build_char_enum(&[view]).unwrap();
    assert_eq!(msg.characters.len(), 1);
    assert_eq!(msg.characters[0].name, "Tester");
    assert_eq!(msg.characters[0].guid.guid(), 1);
}

#[test]
fn char_enum_orders_the_characters_by_ascending_guid() {
    let view = |guid: u64| CharacterView {
        guid,
        name: format!("Tester{guid}"),
        race: 1,
        class: 1,
        level: 1,
        ..Default::default()
    };
    let guids = |msg: &SMSG_CHAR_ENUM| -> Vec<u64> {
        msg.characters.iter().map(|c| c.guid.guid()).collect()
    };

    assert_eq!(
        guids(&build_char_enum(&[view(30), view(10), view(20)]).unwrap()),
        vec![10, 20, 30]
    );
    assert_eq!(
        guids(&build_char_enum(&[view(10), view(20), view(30)]).unwrap()),
        vec![10, 20, 30]
    );
}

#[test]
fn movement_relay_roundtrips_under_same_opcode() {
    use wow_world_messages::vanilla::MovementInfo_MovementFlags;

    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 99,
        position: Vector3d {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        orientation: 0.5,
        fall_time: 0.0,
    };
    let relay =
        build_movement_relay(movement_opcodes::MSG_MOVE_START_FORWARD, 7, info.clone()).unwrap();

    let mut buf = Vec::new();
    relay.write_unencrypted_server(&mut buf).unwrap();

    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::MSG_MOVE_START_FORWARD(m) => {
            assert_eq!(m.guid.guid(), 7);
            assert_eq!(m.info.position.x, 1.0);
            assert_eq!(m.info.timestamp, 99);
        }
        other => panic!("expected relayed MSG_MOVE_START_FORWARD, got {other}"),
    }

    assert!(build_movement_relay(0x9999, 7, info).is_none());
}

#[cfg(test)]
fn movement_info_cases() -> Vec<(&'static str, MovementInfo)> {
    use wow_world_messages::vanilla::{
        MovementInfo_MovementFlags, MovementInfo_MovementFlags_Jumping,
        MovementInfo_MovementFlags_SplineElevation, MovementInfo_MovementFlags_Swimming,
    };
    let base = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 0x1122_3344,
        position: Vector3d {
            x: -8949.95,
            y: -132.493,
            z: 83.5312,
        },
        orientation: 1.75,
        fall_time: 0.25,
    };
    vec![
        ("still", base.clone()),
        (
            "running forward",
            MovementInfo {
                flags: MovementInfo_MovementFlags::new_forward(),
                ..base.clone()
            },
        ),
        (
            "jumping (a 16-byte block AFTER fall_time)",
            MovementInfo {
                flags: MovementInfo_MovementFlags::empty()
                    .set_forward()
                    .set_jumping(MovementInfo_MovementFlags_Jumping {
                        cos_angle: 0.5,
                        sin_angle: -0.5,
                        xy_speed: 7.0,
                        z_speed: 3.0,
                    }),
                fall_time: 900.0,
                ..base.clone()
            },
        ),
        (
            "swimming (a 4-byte pitch BEFORE fall_time)",
            MovementInfo {
                flags: MovementInfo_MovementFlags::empty()
                    .set_swimming(MovementInfo_MovementFlags_Swimming { pitch: -0.3 }),
                ..base.clone()
            },
        ),
        (
            "spline elevation (a trailing 4-byte block)",
            MovementInfo {
                flags: MovementInfo_MovementFlags::empty().set_spline_elevation(
                    MovementInfo_MovementFlags_SplineElevation {
                        spline_elevation: 12.5,
                    },
                ),
                ..base.clone()
            },
        ),
        (
            "swimming + jumping + spline elevation (all three optional blocks at once)",
            MovementInfo {
                flags: MovementInfo_MovementFlags::empty()
                    .set_forward()
                    .set_swimming(MovementInfo_MovementFlags_Swimming { pitch: 0.9 })
                    .set_jumping(MovementInfo_MovementFlags_Jumping {
                        cos_angle: 1.0,
                        sin_angle: 0.0,
                        xy_speed: 4.5,
                        z_speed: -2.0,
                    })
                    .set_spline_elevation(MovementInfo_MovementFlags_SplineElevation {
                        spline_elevation: -3.25,
                    }),
                fall_time: 1234.5,
                ..base
            },
        ),
    ]
}

#[test]
fn raw_movement_relay_is_byte_identical_to_the_typed_path() {
    const GUIDS: &[u64] = &[1, 0x0100, u64::MAX, 0, 0x0000_00F3_0000_0042];
    let mut compared = 0usize;
    for (label, info) in movement_info_cases() {
        let stored = movement_info_to_bytes(&info).unwrap();
        for &opcode in movement_opcodes::SLICE_MOVE_OPCODES {
            for &guid in GUIDS {
                let typed = build_movement_relay(opcode, guid, info.clone())
                    .unwrap_or_else(|| panic!("typed relay missing for opcode 0x{opcode:03X}"));
                let mut framed = Vec::new();
                typed.write_unencrypted_server(&mut framed).unwrap();
                let typed_opcode = u16::from_le_bytes([framed[2], framed[3]]);
                let typed_body = &framed[4..];
                assert_eq!(
                    u16::from_be_bytes([framed[0], framed[1]]) as usize,
                    2 + typed_body.len(),
                    "gtker's size field is opcode(2)+body — the assumption Outbound::Raw's writer makes"
                );

                let (raw_opcode, raw_body) = build_movement_relay_raw(opcode, guid, &stored)
                    .unwrap_or_else(|| panic!("raw relay missing for opcode 0x{opcode:03X}"));
                assert_eq!(
                    raw_opcode, typed_opcode,
                    "opcode mismatch for 0x{opcode:03X} ({label}, guid {guid:#X})"
                );
                assert_eq!(
                    raw_body, typed_body,
                    "BODY MISMATCH for 0x{opcode:03X} ({label}, guid {guid:#X})\n  raw:   {raw_body:?}\n  typed: {typed_body:?}"
                );
                compared += 1;
            }
        }
    }
    assert_eq!(
        compared,
        6 * 21 * 5,
        "every case × opcode × guid must have been compared"
    );
}

/// The acknowledged relays have no vanilla library type, so only the raw path writes them.
#[test]
fn raw_and_typed_movement_relays_accept_exactly_the_same_opcodes() {
    let info = movement_info_cases()[0].1.clone();
    let stored = movement_info_to_bytes(&info).unwrap();
    for opcode in 0u32..=0xFFFF {
        assert_eq!(
            build_movement_relay(opcode, 7, info.clone()).is_some()
                || movement_opcodes::ACKNOWLEDGED_MOVE_OPCODES.contains(&opcode),
            build_movement_relay_raw(opcode, 7, &stored).is_some(),
            "the typed and raw movement relays disagree about opcode 0x{opcode:04X}"
        );
    }
}

#[test]
fn a_truncated_movement_block_is_never_framed() {
    let info = movement_info_cases()[0].1.clone();
    let stored = movement_info_to_bytes(&info).unwrap();
    assert_eq!(stored.len(), 28, "the minimum MovementInfo body");
    let op = movement_opcodes::MSG_MOVE_HEARTBEAT;
    assert!(
        build_movement_relay_raw(op, 7, &stored).is_some(),
        "a full block relays"
    );
    assert!(
        build_movement_relay_raw(op, 7, &stored[..27]).is_none(),
        "one byte short must not relay"
    );
    assert!(
        build_movement_relay_raw(op, 7, &[]).is_none(),
        "an empty block must not relay"
    );
}

#[test]
fn destroy_object_carries_guid() {
    let msg = build_destroy_object(42);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_DESTROY_OBJECT(m) => assert_eq!(m.guid.guid(), 42),
        other => panic!("expected SMSG_DESTROY_OBJECT, got {other}"),
    }
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
fn item_query_response_carries_the_work_item_213_columns() {
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
            assert_eq!(
                found.max_count, 1,
                "max_count must reach the wire (was hardcoded 0 before)"
            );
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
fn item_query_response_default_template_keeps_the_work_item_213_columns_zero() {
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
        guid: (0x4000u64 << 48) | (1 << 8) | 23,
        entry: 25,
        owner_guid: 1,
        slot: 23,
        stack_count: 1,
        durability: 20,
        max_durability: 20,
        container_slots: 0,
        random_property_id: 509_0101,
        random_property_enchant_ids: [0; 3],
        item_text_id: 0,
        enchantment: 0,
    };
    let msg = build_item_create_object(&inst);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
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
        other => panic!("expected an Item CreateObject2, got {other:?}"),
    }
}

#[test]
fn a_copied_letters_create_block_carries_its_item_text_id() {
    let inst = ItemInstanceView {
        guid: (0x4000u64 << 48) | (1 << 8) | 23,
        entry: 8383,
        owner_guid: 1,
        slot: 23,
        stack_count: 1,
        durability: 0,
        max_durability: 0,
        container_slots: 0,
        random_property_id: 0,
        random_property_enchant_ids: [0; 3],
        item_text_id: 7,
        enchantment: 0,
    };
    match &build_item_create_object(&inst).objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Item(it),
            ..
        } => assert_eq!(it.item_item_text_id(), Some(7)),
        other => panic!("expected an Item CreateObject2, got {other:?}"),
    }
    let plain = ItemInstanceView {
        item_text_id: 0,
        ..inst
    };
    match &build_item_create_object(&plain).objects[0] {
        Object::CreateObject2 {
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
fn player_create_seeds_inventory_slot_descriptor() {
    let item_guid = (0x4000u64 << 48) | (1 << 8) | 23;
    let msg = build_create_object(
        &warrior_entity(),
        CreateKind::SelfPlayer,
        &[worn_item(23, item_guid, 25, 0)],
        &[],
    )
    .unwrap();
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(
                p.player_field_inv(ItemSlot::Inventory0),
                Some(Guid::new(item_guid)),
                "the item must occupy backpack slot 0"
            );
            assert_eq!(
                p.player_visible_item(VisibleItemIndex::Index15),
                None,
                "a backpack item must NOT set any equipment VISIBLE_ITEM"
            );
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn skill_block_reads_learned_rows_override_and_append() {
    let msg = build_create_object(
        &warrior_entity(),
        CreateKind::SelfPlayer,
        &[],
        &[(95, 37, 75), (186, 12, 75), (999_999, 1, 1)],
    )
    .unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            let infos: Vec<_> = (0..=20u8)
                .filter_map(|i| SkillInfoIndex::try_from(i).ok())
                .filter_map(|idx| p.player_skill_info(idx))
                .collect();
            let find = |sk: Skill| infos.iter().find(|si| si.skill == sk);
            let def = find(Skill::Defense).expect("Defense stays in the block");
            assert_eq!(
                (def.minimum, def.maximum),
                (37, 75),
                "learned row overrides the static Defense 1/5"
            );
            let mining = find(Skill::Mining).expect("learned Mining appends");
            assert_eq!((mining.minimum, mining.maximum), (12, 75));
            assert!(
                find(Skill::Cooking).is_none(),
                "the Cooking fixture is gone unless learned"
            );
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn create_object_carries_the_sheath_state_for_player_and_creature() {
    let mut e = warrior_entity();
    e.unit_bytes_2 = 0xAB_CD_EF_01;
    let msg = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => assert_eq!(
            p.unit_bytes_2(),
            Some((0x01, 0xEF, 0xCD, 0xAB)),
            "player CREATE must carry UNIT_FIELD_BYTES_2 with the sheath state in byte 0"
        ),
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }

    let mut c = warrior_entity();
    c.type_mask = lyracore_shared::constants::type_mask::CREATURE;
    c.unit_bytes_2 = 1;
    let msg = build_create_object(&c, CreateKind::Peer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Unit(u),
            ..
        } => assert_eq!(
            u.unit_bytes_2(),
            Some((1, 0, 0, 0)),
            "creature CREATE must carry the sheath state too"
        ),
        other => panic!("expected a Unit CreateObject2, got {other:?}"),
    }
}

#[test]
fn player_create_equips_mainhand_renders_visible_item() {
    let item_guid = (0x4000u64 << 48) | (1 << 8) | 15;
    let msg = build_create_object(
        &warrior_entity(),
        CreateKind::SelfPlayer,
        &[worn_item(15, item_guid, 25, 0)],
        &[],
    )
    .unwrap();
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(
                p.player_field_inv(ItemSlot::MainHand),
                Some(Guid::new(item_guid)),
                "the weapon must occupy the main-hand equipment slot"
            );
            let vis = p
                .player_visible_item(VisibleItemIndex::Index15)
                .expect("main-hand VISIBLE_ITEM must be set so the model renders the weapon");
            assert_eq!(
                vis.item, 25,
                "VISIBLE_ITEM must carry the item entry (drives the model)"
            );
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn gossip_message_echoes_guid_with_a_greeting_option() {
    let menu = |is_vendor, is_innkeeper| gossip_menu_options(Vec::new(), is_vendor, is_innkeeper);
    let m = build_gossip_message(
        0xF130_0000_0000_0001,
        GOSSIP_GREETING_TEXT_ID,
        Vec::new(),
        &menu(false, false),
    );
    assert_eq!(m.guid.guid(), 0xF130_0000_0000_0001);
    assert_eq!(m.title_text_id, GOSSIP_GREETING_TEXT_ID);
    const {
        assert!(
            GOSSIP_GREETING_TEXT_ID <= 0xFF_FFFF,
            "title_text_id must be a low, in-range npc_text id"
        )
    };
    assert!(
        m.gossips.is_empty(),
        "an NPC with no options has a greeting-only window"
    );
    let v = build_gossip_message(
        0xF130_0000_0000_0001,
        GOSSIP_GREETING_TEXT_ID,
        Vec::new(),
        &menu(true, false),
    );
    assert_eq!(v.gossips.len(), 1);
    assert_eq!(v.gossips[0].id, 0);
    assert_eq!(v.gossips[0].item_icon, 1);
    assert!(!v.gossips[0].coded);
    assert!(m.quests.is_empty());

    use lyracore_shared::constants::npc_flags;
    assert_eq!(
        npc_flags::INNKEEPER,
        0x80,
        "cmangos 1.12 INNKEEPER is 0x80, not 0x10000"
    );
    const {
        assert!(
            135 & npc_flags::INNKEEPER != 0,
            "Farley(295) npc_flags=135 must gate as an innkeeper"
        )
    };
    assert_eq!(
        135 & npc_flags::VENDOR,
        0x4,
        "the VENDOR (0x4) bit still passes — no regression"
    );

    let inn = build_gossip_message(
        0xF130_0000_0000_0001,
        GOSSIP_GREETING_TEXT_ID,
        Vec::new(),
        &menu(false, true),
    );
    assert_eq!(inn.gossips[0].message, "Make this inn your home.");
    assert_eq!(inn.gossips[0].id, 0);
    let both = build_gossip_message(
        0xF130_0000_0000_0001,
        GOSSIP_GREETING_TEXT_ID,
        Vec::new(),
        &menu(true, true),
    );
    assert_eq!(both.gossips.len(), 2);
    assert_eq!(both.gossips[0].message, "I'd like to browse your goods.");
    assert_eq!(both.gossips[1].message, "Make this inn your home.");
    assert_eq!(both.gossips[1].id, 1);
}

#[test]
fn gossip_message_renders_imported_options_verbatim() {
    use lyracore_shared::constants::gossip_option;
    let opt = |icon, text: &str, action| GossipOptionView {
        icon,
        text: text.into(),
        action,
        ..Default::default()
    };
    let opts = vec![
        opt(0, "Care for a chat?", gossip_option::GOSSIP),
        opt(1, "Let me see your goods.", gossip_option::VENDOR),
        opt(
            0,
            "I'd like to stay here a while.",
            gossip_option::INNKEEPER,
        ),
    ];
    let m = build_gossip_message(
        1,
        GOSSIP_GREETING_TEXT_ID,
        Vec::new(),
        &gossip_menu_options(opts, true, true),
    );
    assert_eq!(m.gossips.len(), 3);
    assert_eq!(m.gossips[0].message, "Care for a chat?");
    assert_eq!(m.gossips[0].id, 0);
    assert_eq!(m.gossips[1].message, "Let me see your goods.");
    assert_eq!(m.gossips[1].item_icon, 1);
    assert_eq!(m.gossips[2].message, "I'd like to stay here a while.");
    assert_eq!(m.gossips[2].id, 2);
}

#[test]
fn gossip_menu_options_appends_only_the_missing_flag_lines() {
    use lyracore_shared::constants::gossip_option;
    let chat = GossipOptionView {
        row_id: 7,
        text: "Care for a chat?".into(),
        action: gossip_option::GOSSIP,
        ..Default::default()
    };
    let merged = gossip_menu_options(vec![chat.clone()], true, false);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].row_id, 7, "the imported row keeps its identity");
    assert_eq!(merged[1].text, "I'd like to browse your goods.");
    assert_eq!(merged[1].action, gossip_option::VENDOR);
    assert_eq!(merged[1].row_id, SYNTHESIZED_ROW_ID);
    let merged = gossip_menu_options(vec![chat.clone()], false, true);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[1].action, gossip_option::INNKEEPER);
    let has_vendor = GossipOptionView {
        row_id: 9,
        text: "Let me see your goods.".into(),
        action: gossip_option::VENDOR,
        ..Default::default()
    };
    let merged = gossip_menu_options(vec![chat, has_vendor], true, false);
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[1].text, "Let me see your goods.");
}

#[test]
fn cast_result_failed_body_carries_the_spell_status_and_reason() {
    assert_eq!(
        build_cast_result_failed(100, 0x4D),
        vec![100, 0, 0, 0, 0x02, 0x4D]
    );
}

#[test]
fn spell_damage_log_renders_crit_and_breakdown() {
    let crit = build_spell_non_melee_damage_log(0x42, 0x7, 133, 30, 4, true, 5, 3);
    assert_eq!(crit.hit_info, HitInfo::AffectsVictim);
    assert_eq!(crit.hit_info.as_int(), 0x2);
    assert_eq!(crit.damage, 30);
    assert_eq!(crit.resisted, 5);
    assert_eq!(crit.absorbed_damage, 3);
    assert_eq!(crit.blocked, 0);
    assert_eq!(crit.school, SpellSchool::Frost);
    let normal = build_spell_non_melee_damage_log(0x42, 0x7, 133, 20, 0, false, 0, 0);
    assert_eq!(normal.hit_info, HitInfo::NormalSwing);
    assert_eq!(normal.resisted, 0);
    assert_eq!(normal.absorbed_damage, 0);
    assert_eq!(normal.school, SpellSchool::Normal);
}

#[test]
fn gossip_message_body_is_exact_5875_layout() {
    let guid = 0xF130_0000_0000_0001u64;
    let msg = SMSG_GOSSIP_MESSAGE {
        guid: Guid::new(guid),
        title_text_id: GOSSIP_GREETING_TEXT_ID,
        gossips: vec![GossipItem {
            id: 0,
            item_icon: 1,
            coded: false,
            message: "Hi".to_string(),
        }],
        quests: vec![QuestItem {
            quest_id: 0x29A,
            quest_icon: 7,
            level: Level::new(3),
            title: "Q".to_string(),
        }],
    };
    let mut framed = Vec::new();
    ServerOpcodeMessage::SMSG_GOSSIP_MESSAGE(Box::new(msg))
        .write_unencrypted_server(&mut framed)
        .unwrap();
    assert_eq!(
        u16::from_le_bytes([framed[2], framed[3]]),
        0x017D,
        "opcode == SMSG_GOSSIP_MESSAGE"
    );
    let body = &framed[4..];

    let mut want: Vec<u8> = Vec::new();
    want.extend_from_slice(&guid.to_le_bytes());
    want.extend_from_slice(&1u32.to_le_bytes());
    want.extend_from_slice(&1u32.to_le_bytes());
    want.extend_from_slice(&0u32.to_le_bytes());
    want.push(1u8);
    want.push(0u8);
    want.extend_from_slice(b"Hi\0");
    want.extend_from_slice(&1u32.to_le_bytes());
    want.extend_from_slice(&0x29Au32.to_le_bytes());
    want.extend_from_slice(&7u32.to_le_bytes());
    want.extend_from_slice(&3u32.to_le_bytes());
    want.extend_from_slice(b"Q\0");

    assert_eq!(
        body,
        want.as_slice(),
        "gossip body must be the exact 5875 layout\n got {:02x?}\nwant {:02x?}",
        body,
        want
    );
}

#[test]
fn npc_text_update_carries_the_greeting_in_slot_0() {
    let t = build_npc_text_update(GOSSIP_GREETING_TEXT_ID, None);
    assert_eq!(t.text_id, GOSSIP_GREETING_TEXT_ID);
    assert_eq!(t.texts.len(), 8);
    assert!(t.texts[0].texts[0].contains("Greetings"));
    assert_eq!(t.texts[0].probability, 1.0);
    assert_eq!(t.texts[1].probability, 0.0);
    let mut view = NpcTextView::default();
    view.slots[0] = (
        "Hey, citizen!".to_string(),
        "Hey, citizen!".to_string(),
        1.0,
    );
    let t2 = build_npc_text_update(4938, Some(&view));
    assert_eq!(t2.text_id, 4938);
    assert_eq!(t2.texts[0].texts[0], "Hey, citizen!");
    assert_eq!(t2.texts[0].probability, 1.0);
}

#[test]
fn npc_text_update_ships_every_populated_slot_with_its_own_probability() {
    let mut view = NpcTextView::default();
    view.slots[0] = (
        "Well met.".to_string(),
        "Well met, traveler.".to_string(),
        0.6,
    );
    view.slots[3] = (
        "Watch yourself.".to_string(),
        "Watch yourself.".to_string(),
        0.4,
    );
    let t = build_npc_text_update(77, Some(&view));
    assert_eq!(
        t.texts[0].texts,
        ["Well met.".to_string(), "Well met, traveler.".to_string()]
    );
    assert_eq!(t.texts[0].probability, 0.6);
    assert_eq!(
        t.texts[3].texts,
        ["Watch yourself.".to_string(), "Watch yourself.".to_string()]
    );
    assert_eq!(t.texts[3].probability, 0.4);
    assert_eq!(t.texts[1].probability, 0.0);
    assert!(t.texts[1].texts[0].is_empty());
}

#[test]
fn list_inventory_raw_body_is_exact_vanilla_layout() {
    let vendor_guid = 0xF130_0000_0000_0001u64;
    let item = VendorItemView {
        item_entry: 25,
        display_id: 1542,
        buy_price: 35,
        max_durability: 20,
        max_count: 0,
        buy_count: 1,
    };
    let (opcode, body) = build_list_inventory_raw(vendor_guid, &[item]);
    assert_eq!(opcode, 0x019F, "SMSG_LIST_INVENTORY opcode (vanilla 5875)");
    assert_eq!(opcode, SMSG_LIST_INVENTORY_OPCODE);

    let expected: Vec<u8> = vec![
        0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0xF1, 0x01, 0x01, 0x00, 0x00, 0x00, 0x19, 0x00,
        0x00, 0x00, 0x06, 0x06, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x23, 0x00, 0x00, 0x00, 0x14,
        0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    ];
    assert_eq!(
        body, expected,
        "raw SMSG_LIST_INVENTORY body must match the vanilla layout byte-for-byte"
    );

    let (_op, empty) = build_list_inventory_raw(vendor_guid, &[]);
    assert_eq!(
        empty,
        vec![0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0xF1, 0x00, 0x00],
        "empty vendor body is guid + count 0 + error byte"
    );
}

#[test]
fn list_inventory_raw_truncates_stock_to_255_items() {
    let items: Vec<VendorItemView> = (0..300u32)
        .map(|i| VendorItemView {
            item_entry: i,
            display_id: 1,
            buy_price: 1,
            max_durability: 1,
            max_count: 1,
            buy_count: 1,
        })
        .collect();
    let (_op, body) = build_list_inventory_raw(1, &items);
    assert_eq!(body[8], 255, "the count byte must cap at 255");
    assert_eq!(
        body.len(),
        9 + 255 * 28,
        "envelope(9) + 255 items * 28 bytes each"
    );
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
    let item_guid = (0x4000u64 << 48) | (1 << 8) | 23;
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

#[test]
fn login_sequence_dedupes_known_spells_and_builds_the_action_bar() {
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &[6603, 100, 100, 200],
        &[],
        &[],
        WorldEntry::FreshLogin,
        1_781_524_800,
    )
    .unwrap();
    let spells = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_INITIAL_SPELLS(s) => Some(s),
            _ => None,
        })
        .expect("SMSG_INITIAL_SPELLS must be in the sequence");
    let ids: Vec<u16> = spells.initial_spells.iter().map(|s| s.spell_id).collect();
    assert_eq!(
        ids,
        vec![6603, 100, 200],
        "duplicates of ATTACK_SPELL and a learned id must be deduped"
    );

    let bar = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(b) => Some(b),
            _ => None,
        })
        .expect("SMSG_ACTION_BUTTONS must be in the sequence");
    assert_eq!(&bar.data[0..3], &[6603, 100, 200]);
    assert!(
        bar.data[3..].iter().all(|&b| b == 0),
        "unused action-bar slots stay zero"
    );
}

#[test]
fn login_sequence_sends_verify_world_at_fresh_login_only() {
    let fresh = login_sequence_messages(
        &warrior_entity(),
        &[],
        &[],
        &[],
        WorldEntry::FreshLogin,
        1_781_524_800,
    )
    .unwrap();
    assert!(
        matches!(
            fresh.first(),
            Some(ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(_))
        ),
        "a fresh login's sequence must START with SMSG_LOGIN_VERIFY_WORLD"
    );

    let port = login_sequence_messages(
        &warrior_entity(),
        &[],
        &[],
        &[],
        WorldEntry::WorldPort,
        1_781_524_800,
    )
    .unwrap();
    assert!(
        !port
            .iter()
            .any(|m| matches!(m, ServerOpcodeMessage::SMSG_LOGIN_VERIFY_WORLD(_))),
        "a world-port re-entry must never resend SMSG_LOGIN_VERIFY_WORLD"
    );
    assert_eq!(
        port.len(),
        fresh.len() - 1,
        "the ONLY difference between the two sequences is the dropped verify-world"
    );
}

#[test]
fn login_sequence_caps_the_action_bar_at_120_slots_but_not_the_spellbook() {
    let learned: Vec<u32> = (1000..1130).collect();
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &learned,
        &[],
        &[],
        WorldEntry::FreshLogin,
        1_781_524_800,
    )
    .unwrap();
    let spells = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_INITIAL_SPELLS(s) => Some(s),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        spells.initial_spells.len(),
        131,
        "ATTACK_SPELL + all 130 learned spells must be known"
    );

    let bar = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(b) => Some(b),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        bar.data.len(),
        120,
        "the action-bar array is fixed at 120 slots"
    );
    assert_eq!(bar.data[0], 6603, "slot 0 is always the Attack toggle");
    assert_eq!(
        bar.data[119], 1118,
        "slot 119 is the 120th known spell (known[119] = learned[118])"
    );
}

#[test]
fn login_sequence_folds_reputation_by_index_and_ignores_out_of_range_slots() {
    let reps = [(5, 100, false), (63, -250, true), (64, 999, false)];
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &[],
        &reps,
        &[],
        WorldEntry::FreshLogin,
        1_781_524_800,
    )
    .unwrap();
    let factions = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_INITIALIZE_FACTIONS(f) => Some(f),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        factions.factions.len(),
        64,
        "the client's fixed 64-slot rep array"
    );
    assert_eq!(factions.factions[5].standing, 100);
    assert_eq!(
        factions.factions[63].standing as i32, -250,
        "a negative standing bit-cast through u32"
    );
    assert!(
        factions.factions[63].flag.is_at_war(),
        "slot 63 carries AT_WAR"
    );
    assert!(
        factions.factions[63].flag.is_visible(),
        "AT_WAR rides with VISIBLE (the pane row must render)"
    );
    assert!(
        !factions.factions[5].flag.is_at_war(),
        "an at_war=false row leaves the flag byte empty"
    );
    for i in (0..64).filter(|&i| i != 5 && i != 63) {
        assert_eq!(
            factions.factions[i].standing, 0,
            "slot {i} must stay at the default 0 standing"
        );
    }
}

#[test]
fn login_sequence_empty_player_actions_is_byte_identical_to_the_pre_212_synth() {
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &[100, 200],
        &[],
        &[],
        WorldEntry::FreshLogin,
        1_781_524_800,
    )
    .unwrap();
    let bar = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(b) => Some(b),
            _ => None,
        })
        .expect("SMSG_ACTION_BUTTONS must be in the sequence");
    assert_eq!(
        &bar.data[0..3],
        &[6603, 100, 200],
        "Attack + known spells, unpacked (type 0 implicit)"
    );
    assert!(bar.data[3..].iter().all(|&b| b == 0));
}

#[test]
fn login_sequence_builds_the_bar_from_imported_player_actions_packing_the_type_byte() {
    let rows = [(0u8, 6603u32, 0u8), (1u8, 78u32, 0u8), (5u8, 1234u32, 4u8)];
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &[100, 200],
        &[],
        &rows,
        WorldEntry::FreshLogin,
        1_781_524_800,
    )
    .unwrap();
    let bar = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(b) => Some(b),
            _ => None,
        })
        .expect("SMSG_ACTION_BUTTONS must be in the sequence");
    assert_eq!(bar.data[0], 6603);
    assert_eq!(bar.data[1], 78);
    assert_eq!(bar.data[5], 1234 | (4 << 24));
    for slot in [2usize, 3, 4, 6, 7, 119] {
        assert_eq!(
            bar.data[slot], 0,
            "slot {slot} must stay 0 — no synth fallback fill-in"
        );
    }
}

#[test]
fn login_sequence_drops_an_out_of_range_imported_button() {
    let rows = [(255u8, 999u32, 0u8)];
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &[],
        &[],
        &rows,
        WorldEntry::FreshLogin,
        1_781_524_800,
    )
    .unwrap();
    let bar = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_ACTION_BUTTONS(b) => Some(b),
            _ => None,
        })
        .unwrap();
    assert!(
        bar.data.iter().all(|&b| b == 0),
        "an out-of-range button must be dropped silently"
    );
}

#[test]
fn create_object_carries_stored_melee_and_ranged_sheet_projection() {
    let e = EntityView {
        level: 10,
        strength: 20,
        base_attack_time_ms: 2800,
        sheet_ap_base: 50,
        sheet_dmg_min: 11,
        sheet_dmg_max: 13,
        sheet_ranged_ap: 30,
        sheet_ranged_dmg_min: 14,
        sheet_ranged_dmg_max: 18,
        ..warrior_entity()
    };
    let msg = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.unit_attack_power(), Some(50));
            assert_eq!(p.unit_mindamage(), Some(11.0));
            assert_eq!(p.unit_maxdamage(), Some(13.0));
            assert_eq!(p.unit_ranged_attack_power(), Some(30));
            assert_eq!(p.unit_ranged_attack_power_mods(), Some((0, 0)));
            assert_eq!(p.unit_minrangeddamage(), Some(14.0));
            assert_eq!(p.unit_maxrangeddamage(), Some(18.0));
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn create_object_carries_a_zero_stored_sheet_projection() {
    let msg = build_create_object(&warrior_entity(), CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.unit_attack_power(), Some(0));
            assert_eq!(p.unit_mindamage(), Some(0.0));
            assert_eq!(p.unit_maxdamage(), Some(0.0));
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn create_object_carries_a_flat_wand_projection() {
    let e = EntityView {
        sheet_ranged_dmg_min: 8,
        sheet_ranged_dmg_max: 12,
        ..warrior_entity()
    };
    let msg = build_create_object(&e, CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            assert_eq!(p.unit_ranged_attack_power(), Some(0));
            assert_eq!(p.unit_minrangeddamage(), Some(8.0));
            assert_eq!(p.unit_maxrangeddamage(), Some(12.0));
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

fn ranged_sheet_stats(
    ranged_attack_power: u32,
    ranged_dmg_min: u32,
    ranged_dmg_max: u32,
) -> SheetStatsValues {
    SheetStatsValues {
        strength: 20,
        agility: 20,
        stamina: 20,
        intellect: 20,
        spirit: 20,
        str_bonus: 0,
        agi_bonus: 0,
        sta_bonus: 0,
        int_bonus: 0,
        spi_bonus: 0,
        attack_power: 50,
        ap_mods: 0,
        dmg_min: 11,
        dmg_max: 13,
        ranged_attack_power,
        ranged_dmg_min,
        ranged_dmg_max,
        crit_pct: 5.0,
    }
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
fn sheet_values_carry_a_flat_wand_projection() {
    assert_sheet_values_projection(ranged_sheet_stats(0, 8, 12), (0, 8.0, 12.0));
}

#[test]
fn active_taxi_mount_is_present_in_the_player_create_mask() {
    let mut entity = warrior_entity();
    entity.mount_display_id = 1147;
    entity.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT;

    let msg = build_create_object(&entity, CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(player),
            ..
        } => {
            assert_eq!(player.unit_mountdisplayid(), Some(1147));
            assert_eq!(player.unit_flags(), Some(entity.unit_flags as i32));
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }

    let unmounted =
        build_create_object(&warrior_entity(), CreateKind::SelfPlayer, &[], &[]).unwrap();
    match &unmounted.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(player),
            ..
        } => assert_eq!(player.unit_mountdisplayid(), None),
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn a_land_mount_display_is_present_in_the_peer_create_mask() {
    let mut entity = warrior_entity();
    entity.mount_display_id = 1147;

    for kind in [CreateKind::Peer, CreateKind::SelfPlayer] {
        let msg = build_create_object(&entity, kind, &[], &[]).unwrap();
        match &msg.objects[0] {
            Object::CreateObject2 {
                mask2: UpdateMask::Player(player),
                ..
            } => {
                assert_eq!(player.unit_mountdisplayid(), Some(1147), "{kind:?}");
                assert_eq!(
                    player.unit_flags(),
                    Some(0),
                    "a land mount must not set TAXI_FLIGHT ({kind:?})"
                );
            }
            other => panic!("expected a Player CreateObject2, got {other:?}"),
        }
    }
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
fn visible_item_index_boundary_stops_after_equipment_slot_18() {
    let (g18, g19) = ((0x4000u64 << 48) | 18, (0x4000u64 << 48) | 19);
    let msg = build_create_object(
        &warrior_entity(),
        CreateKind::SelfPlayer,
        &[worn_item(18, g18, 100, 0), worn_item(19, g19, 200, 0)],
        &[],
    )
    .unwrap();
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Player(p),
            ..
        } => {
            let vis18 = p
                .player_visible_item(VisibleItemIndex::Index18)
                .expect("slot 18 is equipment");
            assert_eq!(
                vis18.item, 100,
                "slot 18's own entry must be the one rendered, not slot 19's"
            );
            assert_eq!(
                p.player_field_inv(ItemSlot::Bag1),
                Some(Guid::new(g19)),
                "slot 19 still gets its inv-slot pointer, just no VISIBLE_ITEM"
            );
        }
        other => panic!("expected a Player CreateObject2, got {other:?}"),
    }
}

#[test]
fn relayed_move_opcode_maps_every_relayed_client_variant() {
    use movement_opcodes as m;
    use wow_world_messages::vanilla::{
        MSG_MOVE_FALL_LAND_Client, MSG_MOVE_JUMP_Client, MSG_MOVE_SET_FACING_Client,
        MSG_MOVE_SET_PITCH_Client, MSG_MOVE_SET_RUN_MODE_Client, MSG_MOVE_SET_WALK_MODE_Client,
        MSG_MOVE_START_BACKWARD_Client, MSG_MOVE_START_FORWARD_Client,
        MSG_MOVE_START_PITCH_DOWN_Client, MSG_MOVE_START_PITCH_UP_Client,
        MSG_MOVE_START_STRAFE_LEFT_Client, MSG_MOVE_START_STRAFE_RIGHT_Client,
        MSG_MOVE_START_SWIM_Client, MSG_MOVE_START_TURN_LEFT_Client,
        MSG_MOVE_START_TURN_RIGHT_Client, MSG_MOVE_STOP_Client, MSG_MOVE_STOP_PITCH_Client,
        MSG_MOVE_STOP_STRAFE_Client, MSG_MOVE_STOP_SWIM_Client, MSG_MOVE_STOP_TURN_Client,
        MovementInfo_MovementFlags,
    };
    let info = MovementInfo {
        flags: MovementInfo_MovementFlags::empty(),
        timestamp: 0,
        position: Vector3d {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        orientation: 0.0,
        fall_time: 0.0,
    };
    let cases: Vec<(ClientOpcodeMessage, u32)> = vec![
        (
            ClientOpcodeMessage::MSG_MOVE_START_FORWARD(Box::new(MSG_MOVE_START_FORWARD_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_START_FORWARD,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_BACKWARD(Box::new(
                MSG_MOVE_START_BACKWARD_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_BACKWARD,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_STOP(Box::new(MSG_MOVE_STOP_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_STOP,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_STRAFE_LEFT(Box::new(
                MSG_MOVE_START_STRAFE_LEFT_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_STRAFE_LEFT,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_STRAFE_RIGHT(Box::new(
                MSG_MOVE_START_STRAFE_RIGHT_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_STRAFE_RIGHT,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_STOP_STRAFE(Box::new(MSG_MOVE_STOP_STRAFE_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_STOP_STRAFE,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_JUMP(Box::new(MSG_MOVE_JUMP_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_JUMP,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_TURN_LEFT(Box::new(
                MSG_MOVE_START_TURN_LEFT_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_TURN_LEFT,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_TURN_RIGHT(Box::new(
                MSG_MOVE_START_TURN_RIGHT_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_TURN_RIGHT,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_STOP_TURN(Box::new(MSG_MOVE_STOP_TURN_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_STOP_TURN,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_PITCH_UP(Box::new(
                MSG_MOVE_START_PITCH_UP_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_PITCH_UP,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_PITCH_DOWN(Box::new(
                MSG_MOVE_START_PITCH_DOWN_Client { info: info.clone() },
            )),
            m::MSG_MOVE_START_PITCH_DOWN,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_STOP_PITCH(Box::new(MSG_MOVE_STOP_PITCH_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_STOP_PITCH,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_SET_PITCH(Box::new(MSG_MOVE_SET_PITCH_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_SET_PITCH,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_SET_RUN_MODE(Box::new(MSG_MOVE_SET_RUN_MODE_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_SET_RUN_MODE,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_SET_WALK_MODE(Box::new(MSG_MOVE_SET_WALK_MODE_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_SET_WALK_MODE,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_FALL_LAND(Box::new(MSG_MOVE_FALL_LAND_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_FALL_LAND,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_START_SWIM(Box::new(MSG_MOVE_START_SWIM_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_START_SWIM,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_STOP_SWIM(Box::new(MSG_MOVE_STOP_SWIM_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_STOP_SWIM,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_SET_FACING(Box::new(MSG_MOVE_SET_FACING_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_SET_FACING,
        ),
        (
            ClientOpcodeMessage::MSG_MOVE_HEARTBEAT(Box::new(MSG_MOVE_HEARTBEAT_Client {
                info: info.clone(),
            })),
            m::MSG_MOVE_HEARTBEAT,
        ),
    ];
    assert_eq!(
        cases.len(),
        m::SLICE_MOVE_OPCODES.len(),
        "every relayed client opcode has a case"
    );
    for (msg, expected) in &cases {
        assert_eq!(
            relayed_move_opcode(msg),
            Some(*expected),
            "opcode mismatch for {msg:?}"
        );
    }
}

#[test]
fn teleport_ack_carries_target_position_with_no_movement_flags() {
    let msg = build_teleport_ack(7, 99, 1.0, 2.0, 3.0, 0.5);
    assert_eq!(msg.guid.guid(), 7);
    assert_eq!(msg.movement_counter, 99);
    assert_eq!(msg.info.position.x, 1.0);
    assert_eq!(msg.info.position.y, 2.0);
    assert_eq!(msg.info.position.z, 3.0);
    assert_eq!(msg.info.orientation, 0.5);
    assert!(
        msg.info.flags.is_empty(),
        "a teleport arrival is a clean stand-still, no movement flags"
    );
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn cross_map_teleport_sends_transfer_pending_then_new_world_carrying_target_map_and_position() {
    let [pending, new_world] = build_cross_map_teleport(36, 10.0, 20.0, 30.0, 1.5, None).unwrap();
    match pending {
        ServerOpcodeMessage::SMSG_TRANSFER_PENDING(p) => {
            assert_eq!(p.map.as_int(), 36);
            assert!(
                p.has_transport.is_none(),
                "a teleport that is not a Transport map crossing names no Transport"
            );
            let mut buf = Vec::new();
            p.write_unencrypted_server(&mut buf).unwrap();
            assert!(!buf.is_empty());
        }
        _ => panic!("expected SMSG_TRANSFER_PENDING"),
    }
    match new_world {
        ServerOpcodeMessage::SMSG_NEW_WORLD(w) => {
            assert_eq!(w.map.as_int(), 36);
            assert_eq!(w.position.x, 10.0);
            assert_eq!(w.position.y, 20.0);
            assert_eq!(w.position.z, 30.0);
            assert_eq!(w.orientation, 1.5);
            let mut buf = Vec::new();
            w.write_unencrypted_server(&mut buf).unwrap();
            assert!(!buf.is_empty());
        }
        _ => panic!("expected SMSG_NEW_WORLD"),
    }
}

#[test]
fn cross_map_teleport_rejects_an_unknown_map_id() {
    assert!(build_cross_map_teleport(u32::MAX, 0.0, 0.0, 0.0, 0.0, None).is_err());
}

#[test]
fn force_run_speed_carries_guid_speed_and_zero_move_event() {
    let msg = build_force_run_speed(7, 8.0);
    assert_eq!(msg.guid.guid(), 7);
    assert_eq!(msg.speed, 8.0);
    assert_eq!(msg.move_event, 0);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
}

#[test]
fn force_swim_speed_serializes_the_vanilla_opcode_and_speed() {
    let msg = build_force_swim_speed(7, 6.0);
    assert_eq!(msg.guid.guid(), 7);
    assert_eq!(msg.move_event, 0);
    let mut bytes = Vec::new();
    msg.write_unencrypted_server(&mut bytes).unwrap();
    assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 0x00e6);
    assert_eq!(&bytes[4..6], &[1, 7]);
    assert_eq!(&bytes[6..10], &0u32.to_le_bytes());
    assert_eq!(&bytes[10..14], &6.0f32.to_le_bytes());
}

#[test]
fn monster_move_run_flag_selects_the_run_spline_bit() {
    let start = Vector3d {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    let dest = Vector3d {
        x: 10.0,
        y: 0.0,
        z: 0.0,
    };
    let walking = build_monster_move(1, start, dest, 1000, 1, false);
    assert_eq!(
        walking.spline_flags,
        SplineFlag::new(0),
        "a walking leg carries no RUN_MODE bit"
    );
    let running = build_monster_move(1, start, dest, 1000, 2, true);
    assert_eq!(
        running.spline_flags,
        SplineFlag::new(0x100),
        "a running leg sets RUN_MODE (0x100)"
    );
    assert_eq!(
        running.splines,
        vec![dest],
        "exactly one destination point (no multi-point spline)"
    );
    assert_eq!(running.spline_id, 2);
    assert_eq!(running.move_type, SMSG_MONSTER_MOVE_MonsterMoveType::Normal);
}

#[test]
fn monster_move_facing_carries_the_angle_and_a_degenerate_single_point_spline() {
    let pos = Vector3d {
        x: 5.0,
        y: -3.0,
        z: 12.0,
    };
    let msg = build_monster_move_facing(9, pos, 1.75, 42);
    assert_eq!(msg.guid.guid(), 9);
    assert_eq!(msg.spline_point, pos);
    assert_eq!(msg.spline_id, 42);
    assert_eq!(
        msg.move_type,
        SMSG_MONSTER_MOVE_MonsterMoveType::FacingAngle { angle: 1.75 }
    );
    assert_eq!(msg.duration, 0, "facing-only — nothing to interpolate");
    assert_eq!(
        msg.splines,
        vec![pos],
        "degenerate one-point spline at the mover's own position"
    );
}

#[test]
fn taxi_move_writes_absolute_catmull_rom_points_byte_for_byte() {
    let start = Vector3d {
        x: -9465.0,
        y: -1300.0,
        z: 50.0,
    };
    let points = vec![
        Vector3d {
            x: -9460.0,
            y: -1298.0,
            z: 52.0,
        },
        Vector3d {
            x: -9450.0,
            y: -1290.0,
            z: 54.0,
        },
        Vector3d {
            x: -9440.0,
            y: -1280.0,
            z: 56.0,
        },
    ];
    let (opcode, bytes) = build_taxi_move_raw(0xF00D, start, points.clone(), 12_345, 77).unwrap();
    assert_eq!(opcode, 0x00DD);

    let mut expected = vec![0x03, 0x0D, 0xF0];
    for value in [start.x, start.y, start.z] {
        expected.extend_from_slice(&value.to_le_bytes());
    }
    expected.extend_from_slice(&77u32.to_le_bytes());
    expected.push(0);
    expected.extend_from_slice(&0x300u32.to_le_bytes());
    expected.extend_from_slice(&12_345u32.to_le_bytes());
    expected.extend_from_slice(&3u32.to_le_bytes());
    for point in points {
        for value in [point.x, point.y, point.z] {
            expected.extend_from_slice(&value.to_le_bytes());
        }
    }
    assert_eq!(bytes, expected);
}

#[test]
fn bank_result_constants_match_gtker_buy_bank_slot_result() {
    use lyracore_shared::bank::result;
    assert_eq!(
        BuyBankSlotResult::FailedTooMany.as_int(),
        result::FAILED_TOO_MANY as u32
    );
    assert_eq!(
        BuyBankSlotResult::InsufficientFunds.as_int(),
        result::INSUFFICIENT_FUNDS as u32
    );
    assert_eq!(
        BuyBankSlotResult::NotBanker.as_int(),
        result::NOT_BANKER as u32
    );
    assert_eq!(BuyBankSlotResult::Ok.as_int(), result::OK as u32);
}

#[test]
fn item_create_object_bag_slots_build_a_container_with_num_slots() {
    let inst = ItemInstanceView {
        guid: (0x4000u64 << 48) | (1 << 8) | 19,
        entry: 5000,
        owner_guid: 1,
        slot: 19,
        stack_count: 1,
        durability: 0,
        max_durability: 0,
        container_slots: 8,
        random_property_id: 509_0101,
        random_property_enchant_ids: [0; 3],
        item_text_id: 0,
        enchantment: 0,
    };
    let msg = build_item_create_object(&inst);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    assert!(!buf.is_empty());
    match &msg.objects[0] {
        Object::CreateObject2 {
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
        other => panic!("expected a Container CreateObject2, got {other:?}"),
    }
}

#[test]
fn corpse_create_object_falls_back_to_display_49_when_zero() {
    let corpse = CorpseView {
        guid: 1,
        owner_guid: 1,
        display_id: 0,
        ..Default::default()
    };
    let msg = build_corpse_create_object(&corpse);
    match &msg.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Corpse(m),
            ..
        } => {
            assert_eq!(
                m.corpse_display_id(),
                Some(49),
                "display_id 0 must fall back to 49"
            );
        }
        other => panic!("expected a Corpse CreateObject2, got {other:?}"),
    }
    let real = CorpseView {
        display_id: 304,
        ..corpse
    };
    let msg2 = build_corpse_create_object(&real);
    match &msg2.objects[0] {
        Object::CreateObject2 {
            mask2: UpdateMask::Corpse(m),
            ..
        } => {
            assert_eq!(m.corpse_display_id(), Some(304));
        }
        other => panic!("expected a Corpse CreateObject2, got {other:?}"),
    }
}

#[test]
fn creature_query_response_maps_names_and_flags_and_serializes() {
    let c = CreatureView {
        entry: 6741,
        name: "Innkeeper Farley".to_string(),
        subname: "Innkeeper".to_string(),
        display_id: 2068,
        creature_type: 7,
        creature_family: 0,
        type_flags: 0x8,
        rank: 0,
    };
    let msg = build_creature_query_response(&c);
    assert_eq!(msg.creature_entry, 6741);
    {
        let found = msg.found.as_ref().expect("a real template must be found");
        assert_eq!(found.name1, "Innkeeper Farley");
        assert_eq!(found.sub_name, "Innkeeper");
        assert_eq!(found.display_id, 2068);
        assert_eq!(found.creature_type, 7);
        assert_eq!(found.type_flags, 0x8);
    }
    let mut buf = Vec::new();
    ServerOpcodeMessage::SMSG_CREATURE_QUERY_RESPONSE(Box::new(msg))
        .write_unencrypted_server(&mut buf)
        .unwrap();
    assert!(!buf.is_empty());
}

fn worn_item(slot: u8, guid: u64, entry: u32, enchantment: u32) -> ItemInstanceView {
    ItemInstanceView {
        guid,
        entry,
        slot,
        enchantment,
        ..Default::default()
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
fn player_create_carries_each_worn_items_permanent_enchant_for_self_and_peer() {
    let bracer = worn_item(8, (0x4000u64 << 48) | 8, 25, 724);
    for kind in [CreateKind::SelfPlayer, CreateKind::Peer] {
        let msg = build_create_object(&warrior_entity(), kind, std::slice::from_ref(&bracer), &[])
            .unwrap();
        match &msg.objects[0] {
            Object::CreateObject2 {
                mask2: UpdateMask::Player(p),
                ..
            } => {
                let vi = p
                    .player_visible_item(VisibleItemIndex::Index8)
                    .expect("wrist VISIBLE_ITEM set");
                assert_eq!((vi.item, vi.enchants), (25, [724, 0]), "{kind:?}");
            }
            other => panic!("expected a Player CreateObject2, got {other:?}"),
        }
    }
}

fn compressed_move_for(i: u64) -> wow_world_messages::vanilla::CompressedMove {
    use wow_world_messages::vanilla::{
        CompressedMove, CompressedMove_CompressedMoveOpcode, MonsterMove,
        MonsterMove_MonsterMoveType, SplineFlag, Vector3d,
    };
    CompressedMove {
        opcode: CompressedMove_CompressedMoveOpcode::SmsgMonsterMove {
            monster_move: MonsterMove {
                spline_point: Vector3d {
                    x: -8938.857 + i as f32,
                    y: -131.36594,
                    z: 83.57745,
                },
                spline_id: i as u32,
                move_type: MonsterMove_MonsterMoveType::Normal {
                    duration: 500,
                    spline_flags: SplineFlag::empty(),
                    splines: vec![Vector3d {
                        x: -8900.0 + i as f32,
                        y: -100.0,
                        z: 82.4,
                    }],
                },
            },
        },
        guid: wow_world_messages::Guid::new(0x1000_0000_0000_0000 + i),
    }
}

fn compressed_moves_roundtrip(n: usize) -> Result<usize, String> {
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::SMSG_COMPRESSED_MOVES;
    use wow_world_messages::Message;

    let msg = SMSG_COMPRESSED_MOVES {
        moves: (0..n as u64).map(compressed_move_for).collect(),
    };
    let mut body = Vec::new();
    msg.write_into_vec(&mut body).map_err(|e| e.to_string())?;

    let size = 2 + body.len();
    let mut full = Vec::new();
    full.extend_from_slice(&(size as u16).to_be_bytes());
    full.extend_from_slice(&(SMSG_COMPRESSED_MOVES::OPCODE as u16).to_le_bytes());
    full.extend_from_slice(&body);

    match ServerOpcodeMessage::read_unencrypted(&mut std::io::Cursor::new(&full)) {
        Ok(ServerOpcodeMessage::SMSG_COMPRESSED_MOVES(decoded)) => Ok(decoded.moves.len()),
        Ok(other) => Err(format!("decoded as the wrong variant: {other}")),
        Err(e) => Err(e.to_string()),
    }
}

#[test]
fn compressed_moves_codec_roundtrips_cleanly_from_1_to_3000_movers() {
    for n in [1usize, 10, 50, 100, 150, 200, 300, 500, 1000, 3000] {
        match compressed_moves_roundtrip(n) {
            Ok(decoded) => assert_eq!(
                decoded, n,
                "n={n}: round-tripped to the wrong mover count ({decoded})"
            ),
            Err(e) => panic!(
                "n={n}: SMSG_COMPRESSED_MOVES failed to round-trip through wow_world_messages' own \
                 codec — this WOULD be the reported encode defect, found at n={n}: {e}"
            ),
        }
    }
}

#[test]
#[ignore = "slow (builds/compresses ~10k CompressedMove blocks); run explicitly to see the u16 wrap \
            threshold this crate's SMSG_COMPRESSED_MOVES::write_encrypted_server has — informational, \
            not a regression guard for that report (that opcode is never sent at this scale, or at all)"]
fn compressed_moves_codec_wraps_its_u16_size_field_only_far_past_reported_scale() {
    let mut last_clean = 0usize;
    for n in (1000..=20000).step_by(1000) {
        match compressed_moves_roundtrip(n) {
            Ok(decoded) if decoded == n => last_clean = n,
            _ => {
                println!(
                    "wrap threshold: clean through n={last_clean}, corrupted by n={n} — both are \
                     roughly two orders of magnitude past the reported 150-300 mover range"
                );
                assert!(
                    n > 5000,
                    "the wrap threshold moved to n={n}, uncomfortably close to a scale this repo \
                     actually benchmarks (docs/bench 300+ mover raids) — worth re-checking against the original report"
                );
                return;
            }
        }
    }
    panic!("expected the encoder to wrap its u16 size field somewhere in 1000..20000 movers");
}

fn proficiency(msg: &ServerOpcodeMessage) -> &SMSG_SET_PROFICIENCY {
    match msg {
        ServerOpcodeMessage::SMSG_SET_PROFICIENCY(p) => p,
        other => panic!("expected SMSG_SET_PROFICIENCY, got {other}"),
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
fn login_sequence_carries_both_proficiency_masks_with_the_initial_state() {
    for (learned, plate_trained) in [(&[][..], false), (&[750u32][..], true)] {
        let msgs = login_sequence_messages(
            &warrior_entity(),
            learned,
            &[],
            &[],
            WorldEntry::FreshLogin,
            1_781_524_800,
        )
        .unwrap();
        let at = |wanted: fn(&ServerOpcodeMessage) -> bool| {
            msgs.iter().position(wanted).expect("message in sequence")
        };
        let spells = at(|m| matches!(m, ServerOpcodeMessage::SMSG_INITIAL_SPELLS(_)));
        let buttons = at(|m| matches!(m, ServerOpcodeMessage::SMSG_ACTION_BUTTONS(_)));
        let first = at(|m| matches!(m, ServerOpcodeMessage::SMSG_SET_PROFICIENCY(_)));
        assert!(
            spells < first && first < buttons,
            "both masks ride the initial-state batch, after the spellbook they derive from"
        );

        let masks: Vec<(u8, u32)> = msgs
            .iter()
            .filter(|m| matches!(m, ServerOpcodeMessage::SMSG_SET_PROFICIENCY(_)))
            .map(|m| {
                (
                    proficiency(m).class.as_int(),
                    proficiency(m).item_sub_class_mask,
                )
            })
            .collect();
        assert_eq!(masks.len(), 2, "one message per item class");
        assert_eq!(masks[0].0, 2, "weapons first");
        assert_eq!(masks[1].0, 4, "then armor");
        assert!(
            masks[1].1 & (1 << 3) != 0,
            "a Warrior wears mail with nothing trained"
        );
        assert_eq!(
            masks[1].1 & (1 << 4) != 0,
            plate_trained,
            "plate follows the passive in the spellbook"
        );
    }
}

#[test]
fn realm_datetime_year_rollover() {
    let before = realm_datetime(1_798_761_540);
    assert_eq!(before.years_after_2000(), 26);
    assert_eq!(before.month(), Month::December);
    assert_eq!(
        before.month_day() + 1,
        31,
        "month_day is zero-based; +1 is the calendar day"
    );
    assert_eq!(before.weekday(), Weekday::Thursday);
    assert_eq!(before.hours(), 23);
    assert_eq!(before.minutes(), 59);

    let after = realm_datetime(1_798_761_600);
    assert_eq!(after.years_after_2000(), 27);
    assert_eq!(after.month(), Month::January);
    assert_eq!(after.month_day() + 1, 1);
    assert_eq!(after.weekday(), Weekday::Friday);
    assert_eq!(after.hours(), 0);
    assert_eq!(after.minutes(), 0);
}

#[test]
fn realm_datetime_month_rollover() {
    let before = realm_datetime(1_801_406_220);
    assert_eq!(before.month(), Month::January);
    assert_eq!(before.month_day() + 1, 31);
    assert_eq!(before.weekday(), Weekday::Sunday);
    assert_eq!(before.hours(), 14);
    assert_eq!(
        before.minutes(),
        37,
        "minute precision: 14:37 must not truncate to the hour"
    );

    let after = realm_datetime(1_801_440_300);
    assert_eq!(after.month(), Month::February);
    assert_eq!(after.month_day() + 1, 1);
    assert_eq!(after.weekday(), Weekday::Monday);
    assert_eq!(after.hours(), 0);
    assert_eq!(after.minutes(), 5);
}

#[test]
fn realm_datetime_leap_year_boundary() {
    let dt = realm_datetime(1_835_417_700);
    assert_eq!(dt.years_after_2000(), 28);
    assert_eq!(dt.month(), Month::February);
    assert_eq!(dt.month_day() + 1, 29);
    assert_eq!(dt.weekday(), Weekday::Tuesday);
    assert_eq!(dt.hours(), 6);
    assert_eq!(dt.minutes(), 15);
}

#[test]
fn realm_datetime_round_trips_through_display() {
    let dt = realm_datetime(1_781_524_800);
    assert_eq!(dt.weekday(), Weekday::Monday, "2026-06-15 is a Monday");
    assert_eq!(
        dt.month_day() + 1,
        15,
        "month_day() + 1 is the calendar day"
    );
    assert_eq!(
        dt.to_string(),
        "Monday 2026-6-15T12:0",
        "Display must show the intended calendar date"
    );
}

#[test]
fn login_sequence_uses_the_modules_clock() {
    let msgs = login_sequence_messages(
        &warrior_entity(),
        &[],
        &[],
        &[],
        WorldEntry::FreshLogin,
        1_781_524_800,
    )
    .unwrap();

    assert!(
        matches!(
            msgs.get(2),
            Some(ServerOpcodeMessage::SMSG_LOGIN_SETTIMESPEED(_))
        ),
        "SMSG_LOGIN_SETTIMESPEED stays the 3rd message in the login batch"
    );
    let settimespeed = msgs
        .iter()
        .find_map(|m| match m {
            ServerOpcodeMessage::SMSG_LOGIN_SETTIMESPEED(s) => Some(s),
            _ => None,
        })
        .expect("SMSG_LOGIN_SETTIMESPEED must be in the sequence");
    assert_eq!(settimespeed.timescale, 0.016_666_668);

    assert_eq!(settimespeed.datetime.to_string(), "Monday 2026-6-15T12:0");
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
                Object::CreateObject2 {
                    guid3,
                    object_type: ObjectType::Item,
                    mask2: UpdateMask::Item(fields),
                    ..
                } if container_slots == 0 => {
                    assert_eq!(guid3.guid(), guid);
                    assert_eq!(fields.object_guid(), Some(Guid::new(guid)));
                }
                Object::CreateObject2 {
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

#[test]
fn self_create_preserves_non_quest_fields_and_movement() {
    let entity = warrior_entity();
    let inventory = [worn_item(23, 0x4000_0000_0000_0042, 25, 0)];
    let skills = [(164, 37, 75)];
    let original =
        build_create_object(&entity, CreateKind::SelfPlayer, &inventory, &skills).unwrap();
    let quests = [update_mask::QuestLogSlot {
        slot: 19,
        quest_id: 777,
        counts: vec![3],
        state: 0,
        timer: 123456,
    }];
    let created = build_self_create_object(&entity, &inventory, &skills, &quests).unwrap();
    let descriptor = |packet: SMSG_UPDATE_OBJECT| {
        let Object::CreateObject2 {
            guid3,
            object_type,
            movement2,
            mask2,
        } = packet.objects[0].clone()
        else {
            panic!("expected a Character CREATE");
        };
        let values = SMSG_UPDATE_OBJECT {
            has_transport: 0,
            objects: vec![Object::Values {
                guid1: guid3,
                mask1: mask2,
            }],
        };
        let mut bytes = Vec::new();
        values.write_unencrypted_server(&mut bytes).unwrap();
        let mut updates = lyracore_shared::values_mask::parse_values_updates(&bytes[4..]);
        updates[0]
            .fields
            .retain(|(index, _)| !(198..258).contains(index));
        (guid3, object_type, movement2, updates)
    };
    assert_eq!(descriptor(original), descriptor(created));
}

#[test]
fn self_create_with_no_quests_matches_the_ordinary_create() {
    let entity = warrior_entity();
    assert_eq!(
        build_self_create_object(&entity, &[], &[], &[]).unwrap(),
        build_create_object(&entity, CreateKind::SelfPlayer, &[], &[]).unwrap(),
    );
}
