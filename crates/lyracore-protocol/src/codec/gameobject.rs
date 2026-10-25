use super::*;

#[derive(Clone, Debug, Default)]
pub struct GameObjectView {
    pub guid: u64,
    pub template_entry: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub state: u8,
    pub type_id: u8,
    pub display_id: u32,
    pub rotation_0: f32,
    pub rotation_1: f32,
    pub rotation_2: f32,
    pub rotation_3: f32,
    pub size: f32,
}

fn object_scale_x(size: f32) -> f32 {
    if size > 0.0 {
        size
    } else {
        1.0
    }
}

#[derive(Clone, Debug, Default)]
pub struct GameObjectTemplateView {
    pub type_id: u8,
    pub display_id: u32,
    pub name: String,
    pub data0: u32,
    pub data1: u32,
    pub data2: u32,
}

pub fn build_gameobject_create_object(go: &GameObjectView) -> SMSG_UPDATE_OBJECT {
    let mask = UpdateGameObject::builder()
        .set_object_guid(Guid::new(go.guid))
        .set_object_entry(go.template_entry as i32)
        .set_object_scale_x(object_scale_x(go.size))
        .set_gameobject_displayid(go.display_id as i32)
        .set_gameobject_state(go.state as i32)
        .set_gameobject_type_id(go.type_id as i32)
        .set_gameobject_faction(0)
        .set_gameobject_flags(0)
        .set_gameobject_pos_x(go.x)
        .set_gameobject_pos_y(go.y)
        .set_gameobject_pos_z(go.z)
        .set_gameobject_facing(go.orientation)
        .set_gameobject_animprogress(100)
        .finalize();
    let update_flag = MovementBlock_UpdateFlag::empty()
        .set_all(MovementBlock_UpdateFlag_All { unknown1: 1 })
        .set_living(MovementBlock_UpdateFlag_Living::HasPosition {
            orientation: go.orientation,
            position: Vector3d {
                x: go.x,
                y: go.y,
                z: go.z,
            },
        });
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::CreateObject2 {
            guid3: Guid::new(go.guid),
            object_type: ObjectType::GameObject,
            movement2: MovementBlock { update_flag },
            mask2: UpdateMask::GameObject(mask),
        }],
    }
}

#[derive(Clone, Debug, Default)]
pub struct TransportView {
    pub guid: u64,
    pub entry: u32,
    pub display_id: u32,
    pub size: f32,
    pub flags: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub path_progress_ms: u32,
}

const GAMEOBJECT_TYPE_MO_TRANSPORT: i32 = 15;
const GO_STATE_READY: i32 = 1;

/// The client moves an MO_TRANSPORT along its TaxiPathNode path from `path_progress_ms` alone.
pub fn build_transport_create_object(transport: &TransportView) -> SMSG_UPDATE_OBJECT {
    let mask = UpdateGameObject::builder()
        .set_object_guid(Guid::new(transport.guid))
        .set_object_entry(transport.entry as i32)
        .set_object_scale_x(object_scale_x(transport.size))
        .set_gameobject_displayid(transport.display_id as i32)
        .set_gameobject_flags(transport.flags as i32)
        .set_gameobject_state(GO_STATE_READY)
        .set_gameobject_type_id(GAMEOBJECT_TYPE_MO_TRANSPORT)
        .set_gameobject_pos_x(transport.x)
        .set_gameobject_pos_y(transport.y)
        .set_gameobject_pos_z(transport.z)
        .set_gameobject_facing(transport.orientation)
        .set_gameobject_animprogress(100)
        .finalize();
    let update_flag = MovementBlock_UpdateFlag::empty()
        .set_transport(MovementBlock_UpdateFlag_Transport {
            transport_progress_in_ms: transport.path_progress_ms,
        })
        .set_all(MovementBlock_UpdateFlag_All { unknown1: 1 })
        .set_living(MovementBlock_UpdateFlag_Living::HasPosition {
            orientation: transport.orientation,
            position: Vector3d {
                x: transport.x,
                y: transport.y,
                z: transport.z,
            },
        });
    SMSG_UPDATE_OBJECT {
        has_transport: 1,
        objects: vec![Object::CreateObject2 {
            guid3: Guid::new(transport.guid),
            object_type: ObjectType::GameObject,
            movement2: MovementBlock { update_flag },
            mask2: UpdateMask::GameObject(mask),
        }],
    }
}

pub fn build_gameobject_values(go: &GameObjectView) -> SMSG_UPDATE_OBJECT {
    let mut mask = UpdateGameObject::builder().finalize();
    mask.dirty_reset();
    mask.set_object_entry(go.template_entry as i32);
    mask.set_object_scale_x(object_scale_x(go.size));
    mask.set_gameobject_displayid(go.display_id as i32);
    mask.set_gameobject_state(go.state as i32);
    mask.set_gameobject_type_id(go.type_id as i32);
    mask.set_gameobject_pos_x(go.x);
    mask.set_gameobject_pos_y(go.y);
    mask.set_gameobject_pos_z(go.z);
    mask.set_gameobject_facing(go.orientation);
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::Values {
            guid1: Guid::new(go.guid),
            mask1: UpdateMask::GameObject(mask),
        }],
    }
}

/// gtker's rotation setter reaches only slot 0 of the four-float quaternion.
pub fn build_gameobject_rotation_values(go: &GameObjectView) -> (u16, Vec<u8>) {
    let (rot0, rot1, rot2, rot3) = if go.rotation_0 == 0.0
        && go.rotation_1 == 0.0
        && go.rotation_2 == 0.0
        && go.rotation_3 == 0.0
    {
        let half = go.orientation * 0.5;
        (0.0, 0.0, half.sin(), half.cos())
    } else {
        (go.rotation_0, go.rotation_1, go.rotation_2, go.rotation_3)
    };
    let mut mask = update_mask::UpdateMaskValues::new();
    mask.set_f32(update_mask::idx::GAMEOBJECT_ROTATION, rot0);
    mask.set_f32(update_mask::idx::GAMEOBJECT_ROTATION + 1, rot1);
    mask.set_f32(update_mask::idx::GAMEOBJECT_ROTATION + 2, rot2);
    mask.set_f32(update_mask::idx::GAMEOBJECT_ROTATION + 3, rot3);
    build_values_update_raw(go.guid, &mask)
}

pub fn build_gameobject_query_response(
    entry: u32,
    template: Option<&GameObjectTemplateView>,
) -> SMSG_GAMEOBJECT_QUERY_RESPONSE {
    match template {
        None => SMSG_GAMEOBJECT_QUERY_RESPONSE {
            entry_id: entry | 0x8000_0000,
            found: None,
        },
        Some(t) => SMSG_GAMEOBJECT_QUERY_RESPONSE {
            entry_id: entry,
            found: Some(SMSG_GAMEOBJECT_QUERY_RESPONSE_found {
                info_type: t.type_id as u32,
                display_id: t.display_id,
                name1: t.name.clone(),
                name2: String::new(),
                name3: String::new(),
                name4: String::new(),
                name5: String::new(),
                raw_data: [t.data0, t.data1, t.data2, 0, 0, 0],
            }),
        },
    }
}

pub fn build_dynamicobject_create_object(
    guid: u64,
    caster_guid: u64,
    spell_id: u32,
    x: f32,
    y: f32,
    z: f32,
    radius: f32,
) -> SMSG_UPDATE_OBJECT {
    let mask = UpdateDynamicObject::builder()
        .set_object_guid(Guid::new(guid))
        .set_object_entry(spell_id as i32)
        .set_object_scale_x(1.0)
        .set_dynamicobject_caster(Guid::new(caster_guid))
        .set_dynamicobject_bytes(1, 0, 0, 0)
        .set_dynamicobject_spellid(spell_id as i32)
        .set_dynamicobject_radius(radius)
        .set_dynamicobject_pos_x(x)
        .set_dynamicobject_pos_y(y)
        .set_dynamicobject_pos_z(z)
        .set_dynamicobject_facing(0.0)
        .finalize();
    let update_flag = MovementBlock_UpdateFlag::empty()
        .set_all(MovementBlock_UpdateFlag_All { unknown1: 1 })
        .set_living(MovementBlock_UpdateFlag_Living::HasPosition {
            orientation: 0.0,
            position: Vector3d { x, y, z },
        });
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::CreateObject2 {
            guid3: Guid::new(guid),
            object_type: ObjectType::DynamicObject,
            movement2: MovementBlock { update_flag },
            mask2: UpdateMask::DynamicObject(mask),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_1_SQRT_2;
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

    #[test]
    fn gameobject_changes_use_values_without_resending_the_object_type() {
        let go = GameObjectView {
            guid: 1,
            state: 1,
            x: 12.0,
            y: -7.0,
            z: 3.0,
            orientation: 1.5,
            ..Default::default()
        };
        let message = build_gameobject_values(&go);
        let Object::Values {
            mask1: UpdateMask::GameObject(mask),
            ..
        } = &message.objects[0]
        else {
            panic!("GameObject change must use VALUES");
        };
        assert_eq!(mask.gameobject_state(), Some(1));
        assert_eq!(mask.gameobject_pos_x(), Some(12.0));
        assert_eq!(mask.gameobject_pos_y(), Some(-7.0));
        assert_eq!(mask.gameobject_pos_z(), Some(3.0));
        assert_eq!(mask.gameobject_facing(), Some(1.5));
        let packet =
            crate::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(message)))
                .unwrap();
        assert_eq!(&packet.body[..8], &[1, 0, 0, 0, 0, 0, 1, 1]);
        let bits = u32::from_le_bytes(packet.body[9..13].try_into().unwrap());
        assert_eq!(bits & (1 << 2), 0);
    }

    #[test]
    fn gameobject_create_object_carries_descriptor_fields_and_serializes() {
        let go = GameObjectView {
            guid: (0xF110u64 << 48) | 1,
            template_entry: 50100,
            x: -8949.0,
            y: -139.0,
            z: 83.5,
            orientation: 0.0,
            state: 1,
            type_id: 3,
            display_id: 259,
            rotation_0: 0.0,
            rotation_1: 0.0,
            rotation_2: 0.0,
            rotation_3: 0.0,
            size: 0.7,
        };
        let built = build_gameobject_create_object(&go);
        match &built.objects[0] {
            Object::CreateObject2 {
                guid3,
                object_type,
                mask2: UpdateMask::GameObject(m),
                ..
            } => {
                assert_eq!(guid3.guid(), go.guid);
                assert_eq!(*object_type, ObjectType::GameObject);
                assert_eq!(m.object_entry(), Some(50100));
                assert_eq!(m.gameobject_displayid(), Some(259));
                assert_eq!(m.gameobject_state(), Some(1));
                assert_eq!(m.gameobject_type_id(), Some(3));
                assert_eq!(m.object_scale_x(), Some(0.7));
            }
            other => panic!("expected a GameObject CreateObject2, got {other:?}"),
        }
        let msg = ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(built));
        let mut buf = Vec::new();
        msg.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn transport_create_flags_the_packet_and_ends_its_movement_block_with_path_progress() {
        let transport = TransportView {
            guid: (0x1FC0u64 << 48) | 176_244,
            entry: 176_244,
            display_id: 3015,
            size: 1.0,
            flags: 40,
            x: 8533.7,
            y: 1025.1,
            z: 0.0,
            orientation: 2.0,
            path_progress_ms: 123_456,
        };
        let message = build_transport_create_object(&transport);
        let Object::CreateObject2 {
            object_type,
            mask2: UpdateMask::GameObject(mask),
            ..
        } = &message.objects[0]
        else {
            panic!("a Transport must use CREATE_OBJECT2");
        };
        assert_eq!(*object_type, ObjectType::GameObject);
        assert_eq!(mask.gameobject_type_id(), Some(15));
        assert_eq!(mask.gameobject_flags(), Some(40));
        assert_eq!(mask.gameobject_displayid(), Some(3015));
        let packet =
            crate::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(message)))
                .unwrap();
        // Block count, has_transport and CREATE_OBJECT2, then a 6-byte packed GUID and the type.
        assert_eq!(&packet.body[..6], &[1, 0, 0, 0, 1, 3]);
        let movement = 6 + 6 + 1;
        assert_eq!(packet.body[movement - 1], 5, "GameObject object type");
        assert_eq!(packet.body[movement], 0x02 | 0x10 | 0x40);
        let progress = movement + 1 + 16 + 4;
        assert_eq!(
            u32::from_le_bytes(packet.body[progress..progress + 4].try_into().unwrap()),
            123_456
        );
    }

    #[test]
    fn object_scale_x_falls_back_to_one_for_any_non_positive_stored_size() {
        assert_eq!(object_scale_x(0.5), 0.5);
        assert_eq!(object_scale_x(1.75), 1.75);
        assert_eq!(object_scale_x(0.0), 1.0);
        assert_eq!(object_scale_x(-2.0), 1.0);
        assert_eq!(object_scale_x(f32::NAN), 1.0);
    }

    #[test]
    fn gameobject_create_object_sends_scale_one_when_the_template_carries_no_size() {
        let go = GameObjectView {
            guid: (0xF110u64 << 48) | 2,
            template_entry: 50101,
            display_id: 259,
            size: 0.0,
            ..Default::default()
        };
        match &build_gameobject_create_object(&go).objects[0] {
            Object::CreateObject2 {
                mask2: UpdateMask::GameObject(m),
                ..
            } => assert_eq!(m.object_scale_x(), Some(1.0)),
            other => panic!("expected a GameObject CreateObject2, got {other:?}"),
        }
    }

    fn decode_rotation_floats(guid: u64, body: &[u8]) -> [f32; 4] {
        let mut packed_guid = Vec::new();
        super::write_packed_guid_u64(&mut packed_guid, guid);
        let mask_start = 4 + 1 + 1 + packed_guid.len();
        let block_count = body[mask_start] as usize;
        let values_start = mask_start + 1 + block_count * 4;
        let mut out = [0.0f32; 4];
        for (i, chunk) in body[values_start..values_start + 16]
            .chunks_exact(4)
            .enumerate()
        {
            out[i] = f32::from_le_bytes(chunk.try_into().unwrap());
        }
        out
    }

    #[test]
    fn gameobject_rotation_values_passes_through_a_stored_nonzero_quaternion() {
        let go = GameObjectView {
            guid: 5,
            template_entry: 446,
            x: -9450.0,
            y: -90.0,
            z: 55.0,
            orientation: 1.57,
            state: 0,
            type_id: 0,
            display_id: 259,
            rotation_0: 0.1,
            rotation_1: 0.2,
            rotation_2: FRAC_1_SQRT_2,
            rotation_3: FRAC_1_SQRT_2,
            size: 0.0,
        };
        let (opcode, body) = build_gameobject_rotation_values(&go);
        assert_eq!(opcode, 0x00A9, "SMSG_UPDATE_OBJECT opcode");
        let floats = decode_rotation_floats(go.guid, &body);
        assert_eq!(floats, [0.1, 0.2, FRAC_1_SQRT_2, FRAC_1_SQRT_2]);
    }

    #[test]
    fn gameobject_rotation_values_derives_yaw_only_from_orientation_when_quaternion_is_all_zero() {
        let go = GameObjectView {
            guid: 5,
            template_entry: 446,
            x: -9450.0,
            y: -90.0,
            z: 55.0,
            orientation: std::f32::consts::FRAC_PI_2,
            state: 0,
            type_id: 0,
            display_id: 259,
            rotation_0: 0.0,
            rotation_1: 0.0,
            rotation_2: 0.0,
            rotation_3: 0.0,
            size: 0.0,
        };
        let (_, body) = build_gameobject_rotation_values(&go);
        let floats = decode_rotation_floats(go.guid, &body);
        let half = go.orientation * 0.5;
        assert_eq!(floats[0], 0.0, "no stored quaternion ⇒ no terrain pitch");
        assert_eq!(floats[1], 0.0);
        assert!((floats[2] - half.sin()).abs() < 1e-6, "rot2 = sin(o/2)");
        assert!((floats[3] - half.cos()).abs() < 1e-6, "rot3 = cos(o/2)");
    }

    #[test]
    fn gameobject_query_response_found_and_not_found() {
        let t = GameObjectTemplateView {
            type_id: 3,
            display_id: 259,
            name: "Reinforced Chest".to_string(),
            data0: 1,
            data1: 2,
            data2: 0,
        };
        let found = build_gameobject_query_response(50100, Some(&t));
        assert_eq!(found.entry_id, 50100);
        let f = found.found.expect("a known template must reply found");
        assert_eq!(f.info_type, 3);
        assert_eq!(f.display_id, 259);
        assert_eq!(f.name1, "Reinforced Chest");
        assert_eq!(f.raw_data, [1, 2, 0, 0, 0, 0]);

        let missing = build_gameobject_query_response(999, None);
        assert_eq!(
            missing.entry_id,
            999 | 0x8000_0000,
            "unknown entry sets the high 'not found' bit"
        );
        assert!(missing.found.is_none());
    }

    #[test]
    fn a_meeting_stone_query_response_carries_its_level_range_and_area() {
        let stone = GameObjectTemplateView {
            type_id: lyracore_shared::constants::go_type::MEETINGSTONE,
            display_id: 6973,
            name: "Meeting Stone".to_string(),
            data0: 15,
            data1: 20,
            data2: 1581,
        };
        let found = build_gameobject_query_response(179584, Some(&stone))
            .found
            .expect("a known template must reply found");
        assert_eq!(found.info_type, 23);
        assert_eq!(found.raw_data, [15, 20, 1581, 0, 0, 0]);
    }
}
