//! Gameobject wire mapping: the GameObject CREATE_OBJECT (a stationary world prop) + the
//! `SMSG_GAMEOBJECT_QUERY_RESPONSE` template reply. The CREATE_OBJECT mirrors `corpse.rs` exactly — a
//! stationary `HasPosition` object — but with the `UpdateGameObject` descriptor. The descriptor is a
//! FULL build via `finalize()` (not a partial VALUES), so there is no OBJECT_FIELD_TYPE crash risk.

use super::*;

/// A `game_gameobject` row joined with its template, flattened for the CREATE_OBJECT.
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
}

/// A `game_gameobject_template` row, for the `CMSG_GAMEOBJECT_QUERY` reply.
#[derive(Clone, Debug, Default)]
pub struct GameObjectTemplateView {
    pub type_id: u8,
    pub display_id: u32,
    pub name: String,
    pub data0: u32,
    pub data1: u32,
}

/// Build the CREATE_OBJECT for a gameobject: a GAMEOBJECT-type object with a stationary position
/// (`HasPosition`, like a corpse) carrying the `UpdateGameObject` descriptors (display, state, type,
/// position) so the client renders the prop. Relayed on `game_gameobject` insert; the matching
/// `SMSG_DESTROY_OBJECT` on delete reuses `build_destroy_object`. Full descriptor → no VALUES crash risk.
pub fn build_gameobject_create_object(go: &GameObjectView) -> SMSG_UPDATE_OBJECT {
    let mask = UpdateGameObject::builder()
        .set_object_guid(Guid::new(go.guid))
        .set_object_entry(go.template_entry as i32)
        .set_object_scale_x(1.0)
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

/// Build `SMSG_GAMEOBJECT_QUERY_RESPONSE` (the client asks for a GO template's name/type/display before
/// it can render/interact). `None` → the not-found form (`entry_id` with the high bit set, per protocol).
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
                raw_data: [t.data0, t.data1, 0, 0, 0, 0],
            }),
        },
    }
}

/// A ground-area spell's DYNAMICOBJECT CREATE (118, Consecration's swirl): the 5875 client renders
/// the persistent ground effect from `DYNAMICOBJECT_SPELLID`'s SpellVisual, sized by RADIUS at
/// POS — it never draws it from the cast packets alone (live find). Same stationary
/// `CreateObject2` + `HasPosition` shape as the gameobject CREATE above; entry = the spell id and
/// bytes = (1,0,0,0) = DYNAMIC_OBJECT_AREA_SPELL — the value the 5875 client expects here.
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
        .set_dynamicobject_bytes(1, 0, 0, 0) // DYNAMIC_OBJECT_AREA_SPELL
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
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

    #[test]
    fn gameobject_create_object_carries_descriptor_fields_and_serializes() {
        // Was WEAK (only asserted non-empty serialization); now pins the actual descriptor fields
        // the client renders the prop from, plus the object-type/mask shape, before serializing.
        let go = GameObjectView {
            guid: (0xF110u64 << 48) | 1,
            template_entry: 50100,
            x: -8949.0,
            y: -139.0,
            z: 83.5,
            orientation: 0.0,
            state: 1, // open — distinct from the enum's 0 default, so a dropped setter call is caught
            type_id: 3, // CHEST
            display_id: 259,
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
            }
            other => panic!("expected a GameObject CreateObject2, got {other:?}"),
        }
        // The full GAMEOBJECT CREATE_OBJECT must still serialize without panic (stationary
        // descriptor like a corpse).
        let msg = ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(built));
        let mut buf = Vec::new();
        msg.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn gameobject_query_response_found_and_not_found() {
        let t = GameObjectTemplateView {
            type_id: 3,
            display_id: 259,
            name: "Reinforced Chest".to_string(),
            data0: 1,
            data1: 2,
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
}
