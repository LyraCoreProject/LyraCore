use super::*;
use crate::codec::corpse::{build_corpse_create_object, CorpseView};
use crate::codec::gameobject::{
    build_dynamicobject_create_object, build_gameobject_create_object,
    build_transport_create_object, GameObjectView, TransportView,
};
use crate::codec::item::{build_item_create_object, ItemInstanceView};
use crate::codec::test_fixtures::{proficiency, ranged_sheet_stats, worn_item};
use crate::codec::update_mask;
use crate::codec::values::build_sheet_stats_values;
use lyracore_shared::constants::speeds;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{
    ItemSlot, Month, MovementBlock_UpdateFlag_Living, Object, ObjectType, ServerMessage, Skill,
    SkillInfo, SkillInfoIndex, UpdateMask, Vector3d, VisibleItemIndex, Weekday, SMSG_UPDATE_OBJECT,
};
use wow_world_messages::Guid;

#[test]
fn a_new_map_object_beside_a_viewer_uses_create_kind_three() {
    for (name, mut message) in visibility_create_examples()
        .into_iter()
        .filter(|(name, _)| {
            matches!(
                *name,
                "creature" | "gameobject" | "corpse" | "dynamic object"
            )
        })
    {
        set_create_freshness(&mut message, CreateFreshness::New);
        assert!(
            matches!(message.objects[0], Object::CreateObject2 { .. }),
            "{name}"
        );
        assert_eq!(
            crate::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(message)))
                .unwrap()
                .body[5],
            3,
            "{name}"
        );
    }
}

#[test]
fn a_later_sighting_uses_create_kind_two_for_every_world_type() {
    for (name, mut message) in visibility_create_examples() {
        set_create_freshness(&mut message, CreateFreshness::Known);
        assert!(
            matches!(message.objects[0], Object::CreateObject { .. }),
            "{name}"
        );
        assert_eq!(
            crate::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(message)))
                .unwrap()
                .body[5],
            2,
            "{name}"
        );
    }
}

fn visibility_create_examples() -> Vec<(&'static str, SMSG_UPDATE_OBJECT)> {
    let player = warrior_entity();
    let creature = EntityView {
        type_mask: 0,
        ..warrior_entity()
    };
    vec![
        (
            "self",
            build_create_object(&player, CreateKind::SelfPlayer, &[], &[]).unwrap(),
        ),
        (
            "peer",
            build_create_object(&player, CreateKind::Peer, &[], &[]).unwrap(),
        ),
        (
            "creature",
            build_create_object(&creature, CreateKind::Peer, &[], &[]).unwrap(),
        ),
        (
            "item",
            build_item_create_object(&ItemInstanceView::default()),
        ),
        (
            "container",
            build_item_create_object(&ItemInstanceView {
                container_slots: 8,
                ..Default::default()
            }),
        ),
        (
            "transport",
            build_transport_create_object(&TransportView::default()),
        ),
        (
            "gameobject",
            build_gameobject_create_object(&GameObjectView::default()),
        ),
        (
            "elevator",
            build_gameobject_create_object(&GameObjectView {
                type_id: lyracore_shared::constants::go_type::ELEVATOR,
                ..Default::default()
            }),
        ),
        (
            "moving transport",
            build_gameobject_create_object(&GameObjectView {
                type_id: lyracore_shared::constants::go_type::MO_TRANSPORT,
                ..Default::default()
            }),
        ),
        ("corpse", build_corpse_create_object(&CorpseView::default())),
        (
            "dynamic object",
            build_dynamicobject_create_object(1, 2, 3, 0.0, 0.0, 0.0, 1.0),
        ),
    ]
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
            let (Object::CreateObject2 {
                mask2: UpdateMask::Player(fields),
                ..
            }
            | Object::CreateObject {
                mask2: UpdateMask::Player(fields),
                ..
            }) = &message.objects[0]
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
        Object::CreateObject {
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
        other => panic!("expected CreateObject, got {other:?}"),
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
        Object::CreateObject {
            mask2: UpdateMask::Unit(u),
            ..
        } => {
            assert_eq!(u.unit_npc_flags(), Some(0x1 | 0x4));
        }
        other => panic!("expected a Unit CreateObject, got {other:?}"),
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
        other => panic!("expected Character CREATE, got {other:?}"),
    }
}

#[test]
fn peer_create_uses_existing_object_type_and_unit_movement_flags() {
    for entity in [
        warrior_entity(),
        EntityView {
            type_mask: lyracore_shared::constants::type_mask::CREATURE,
            ..warrior_entity()
        },
    ] {
        let message = build_create_object(&entity, CreateKind::Peer, &[], &[]).unwrap();
        let Object::CreateObject { movement2, .. } = &message.objects[0] else {
            panic!("an existing peer must use CREATE_OBJECT");
        };
        let flags = &movement2.update_flag;
        assert!(flags.get_living().is_some());
        assert_eq!(flags.get_all().map(|all| all.unknown1), Some(1));
        assert_eq!(
            flags.get_living().and_then(|living| match living {
                MovementBlock_UpdateFlag_Living::Living { turn_rate, .. } => Some(*turn_rate),
                _ => None,
            }),
            Some(3.141_594_f32)
        );
        let packet =
            crate::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(message)))
                .unwrap();
        assert_eq!(packet.body[9], 0x70);
    }
}

#[test]
fn flying_create_carries_full_spline_and_elapsed_time_on_the_wire() {
    let mut entity = warrior_entity();
    let nodes = [
        (-100.0, -200.0, 0.0),
        (0.0, 0.0, 0.0),
        (100.0, 200.0, 30.0),
        (200.0, 0.0, 60.0),
        (200.0, 0.0, 60.0),
    ]
    .map(|(x, y, z)| Vector3d { x, y, z });
    entity.x = 75.0;
    entity.y = 150.0;
    entity.spline = Some(CreateSpline {
        nodes: nodes.to_vec(),
        time_passed_ms: 12_500,
        duration_ms: 60_000,
        id: 42,
    });

    let create = build_create_object(&entity, CreateKind::Peer, &[], &[]).unwrap();
    let mut wire = Vec::new();
    create.write_unencrypted_server(&mut wire).unwrap();
    let ServerOpcodeMessage::SMSG_UPDATE_OBJECT(decoded) =
        ServerOpcodeMessage::read_unencrypted(&mut wire.as_slice()).unwrap()
    else {
        panic!("expected CREATE update");
    };
    let Object::CreateObject { movement2, .. } = &decoded.objects[0] else {
        panic!("expected CREATE_OBJECT");
    };
    let MovementBlock_UpdateFlag_Living::Living {
        living_position,
        flags,
        ..
    } = movement2.update_flag.get_living().unwrap()
    else {
        panic!("expected living movement");
    };
    let spline = flags.get_spline_enabled().expect("missing CREATE spline");
    assert_eq!((living_position.x, living_position.y), (75.0, 150.0));
    assert!(spline.spline_flags.get_flying());
    assert!(spline.spline_flags.get_run_mode());
    assert_eq!(spline.time_passed, 12_500);
    assert_eq!(spline.duration, 60_000);
    assert_eq!(spline.id, 42);
    assert_eq!(&spline.nodes[..4], &nodes[..4]);
    assert_eq!(spline.nodes[4].z, 60.0 + 0.02);
    assert_eq!(spline.final_node, nodes[4]);
}

#[test]
fn flying_create_offsets_consecutive_identical_controls_but_not_the_destination() {
    let mut entity = warrior_entity();
    let nodes = [
        (0.0, 0.0, 0.0),
        (1.0, 1.0, 1.0),
        (1.0, 1.0, 1.0),
        (1.0, 1.0, 1.0),
        (2.0, 2.0, 2.0),
    ]
    .map(|(x, y, z)| Vector3d { x, y, z });
    entity.spline = Some(CreateSpline {
        nodes: nodes.to_vec(),
        time_passed_ms: 1,
        duration_ms: 100,
        id: 7,
    });

    let create = build_create_object(&entity, CreateKind::Peer, &[], &[]).unwrap();
    let mut wire = Vec::new();
    create.write_unencrypted_server(&mut wire).unwrap();
    let ServerOpcodeMessage::SMSG_UPDATE_OBJECT(decoded) =
        ServerOpcodeMessage::read_unencrypted(&mut wire.as_slice()).unwrap()
    else {
        panic!("expected CREATE update");
    };
    let Object::CreateObject { movement2, .. } = &decoded.objects[0] else {
        panic!("expected CREATE_OBJECT");
    };
    let MovementBlock_UpdateFlag_Living::Living { flags, .. } =
        movement2.update_flag.get_living().unwrap()
    else {
        panic!("expected living movement");
    };
    let spline = flags.get_spline_enabled().unwrap();
    assert_eq!(spline.nodes[1], nodes[1]);
    assert_eq!(spline.nodes[2].z, 1.0 + 0.02);
    assert_eq!(spline.nodes[3].z, 1.0 + 0.01);
    assert_eq!(spline.nodes[4], nodes[4]);
    assert_eq!(spline.final_node, nodes[4]);
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
        Object::CreateObject2 { movement2, .. } | Object::CreateObject { movement2, .. } => {
            movement2.update_flag.get_self()
        }
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
fn destroy_object_carries_guid() {
    let msg = build_destroy_object(42);
    let mut buf = Vec::new();
    msg.write_unencrypted_server(&mut buf).unwrap();
    match ServerOpcodeMessage::read_unencrypted(&mut buf.as_slice()).unwrap() {
        ServerOpcodeMessage::SMSG_DESTROY_OBJECT(m) => assert_eq!(m.guid.guid(), 42),
        other => panic!("expected SMSG_DESTROY_OBJECT, got {other}"),
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
        Object::CreateObject {
            mask2: UpdateMask::Unit(u),
            ..
        } => assert_eq!(
            u.unit_bytes_2(),
            Some((1, 0, 0, 0)),
            "creature CREATE must carry the sheath state too"
        ),
        other => panic!("expected a Unit CreateObject, got {other:?}"),
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
        sheet_ranged_dmg_min: 14.0,
        sheet_ranged_dmg_max: 18.0,
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
        sheet_ranged_dmg_min: 8.0,
        sheet_ranged_dmg_max: 12.0,
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
            }
            | Object::CreateObject {
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
            other => panic!("expected a Character CREATE, got {other:?}"),
        }
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
fn player_create_carries_each_worn_items_permanent_enchant_for_self_and_peer() {
    let bracer = worn_item(8, (0x4000u64 << 48) | 8, 25, 724);
    for kind in [CreateKind::SelfPlayer, CreateKind::Peer] {
        let msg = build_create_object(&warrior_entity(), kind, std::slice::from_ref(&bracer), &[])
            .unwrap();
        match &msg.objects[0] {
            Object::CreateObject2 {
                mask2: UpdateMask::Player(p),
                ..
            }
            | Object::CreateObject {
                mask2: UpdateMask::Player(p),
                ..
            } => {
                let vi = p
                    .player_visible_item(VisibleItemIndex::Index8)
                    .expect("wrist VISIBLE_ITEM set");
                assert_eq!((vi.item, vi.enchants), (25, [724, 0]), "{kind:?}");
            }
            other => panic!("expected a Character CREATE, got {other:?}"),
        }
    }
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

#[test]
fn spell_9329_and_demoralizing_shout_keep_both_attack_power_halves_in_create_and_values() {
    let modifiers = 16 | (u32::from((-35i16) as u16) << 16);
    let entity = EntityView {
        sheet_ap_modifiers: modifiers,
        sheet_ranged_ap_modifiers: 16,
        ..warrior_entity()
    };
    let create = build_create_object(&entity, CreateKind::SelfPlayer, &[], &[]).unwrap();
    let Object::CreateObject2 {
        mask2: UpdateMask::Player(player),
        ..
    } = &create.objects[0]
    else {
        panic!("expected Player CREATE");
    };
    assert_eq!(player.unit_attack_power_mods(), Some((16, (-35i16) as u16)));
    assert_eq!(player.unit_ranged_attack_power_mods(), Some((16, 0)));
    let mut stats = ranged_sheet_stats(0, 0, 0);
    stats.ap_modifiers = modifiers;
    stats.ranged_ap_modifiers = 16;
    let update = build_sheet_stats_values(1, &stats);
    let Object::Values {
        mask1: UpdateMask::Player(player),
        ..
    } = &update.objects[0]
    else {
        panic!("expected Player VALUES");
    };
    assert_eq!(player.unit_attack_power_mods(), Some((16, (-35i16) as u16)));
    assert_eq!(player.unit_ranged_attack_power_mods(), Some((16, 0)));
}
