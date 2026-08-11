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
    /// The cmangos spawn quaternion (issue #515) — `GAMEOBJECT_ROTATION`, the field the 5875 client
    /// actually renders a static prop's orientation from. All-zero means "no quaternion stored"
    /// (every pre-#515 row and every hand-seeded fixture): `build_gameobject_rotation_values` derives
    /// a yaw-only fallback from `orientation` in that case rather than sending a degenerate rotation.
    pub rotation_0: f32,
    pub rotation_1: f32,
    pub rotation_2: f32,
    pub rotation_3: f32,
    /// `game_gameobject_template.size` (issue #107) — the prop's render scale. 0 (or anything not
    /// positive) means "no size stored": every pre-#107 row, every hand-seeded fixture, and any dump
    /// row whose `size` column failed to parse. `object_scale_x` maps that case to 1.0.
    pub size: f32,
}

/// The `OBJECT_FIELD_SCALE_X` to send for a gameobject (issue #107): its template size, or 1.0 when
/// no size is stored. The guard is `> 0.0` rather than `!= 0.0` on purpose — a corrupt negative or a
/// NaN both fall back too, and a 0 reaching the wire renders the prop INVISIBLE (the same trap
/// `creature_template.Scale == 0` carries, see the importer's DBC scale resolution).
fn object_scale_x(size: f32) -> f32 {
    if size > 0.0 {
        size
    } else {
        1.0
    }
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

/// Build the raw `GAMEOBJECT_ROTATION` VALUES update (issue #515) — the 4-float spawn quaternion the
/// client actually orients a static prop's model from. gtker's typed builder only exposes
/// `set_gameobject_rotation(f32)`, which reaches slot 0 alone (the same descriptor-setter wall as the
/// multi-aura array), so all 4 slots (rot0..3, wire index `GAMEOBJECT_ROTATION`..+3) ride the
/// hand-rolled raw encoder instead. Not a partial UNIT/PLAYER update — `GAMEOBJECT_ROTATION` (10) never
/// collides with `OBJECT_FIELD_TYPE` (2), so `build_values_update_raw`'s debug_assert is inert here by
/// construction, same as every other GAMEOBJECT field.
///
/// Trap this exists to dodge: an all-zero stored quaternion (every row imported before this migration,
/// and every hand-seeded fixture that never set these) is NOT a valid "identity" rotation to send
/// verbatim — a real vanilla spawn's quaternion is never exactly (0,0,0,0) (that's a degenerate,
/// zero-magnitude quaternion; a true identity is (0,0,0,1)). Sending it as-is renders the client's
/// DEFAULT orientation regardless of `orientation`, which is the exact bug #515 reports. So the
/// all-zero case DERIVES a yaw-only quaternion from `orientation` (`rot2 = sin(o/2)`, `rot3 = cos(o/2)`,
/// matching vanilla's Z-axis-only yaw convention — rot0/rot1 stay 0, i.e. no terrain pitch/roll, which
/// is the best a bare `orientation` float can express) instead.
pub fn build_gameobject_rotation_values(go: &GameObjectView) -> (u16, Vec<u8>) {
    let (rot0, rot1, rot2, rot3) =
        if go.rotation_0 == 0.0 && go.rotation_1 == 0.0 && go.rotation_2 == 0.0 && go.rotation_3 == 0.0 {
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
            rotation_0: 0.0,
            rotation_1: 0.0,
            rotation_2: 0.0,
            rotation_3: 0.0,
            size: 0.7, // an authentic sub-1.0 template size (issue #107) — must reach the wire verbatim
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
                // Issue #107: the template size, NOT the old hardcoded 1.0 — this is the field the
                // client renders the prop's proportions from.
                assert_eq!(m.object_scale_x(), Some(0.7));
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
    fn object_scale_x_falls_back_to_one_for_any_non_positive_stored_size() {
        // A real imported size rides through verbatim (issue #107 — that IS the fix).
        assert_eq!(object_scale_x(0.5), 0.5);
        assert_eq!(object_scale_x(1.75), 1.75);
        // 0 = "no size stored": every pre-#107 row (default-migrated) and every hand-seeded fixture.
        // Sending the 0 verbatim would render the prop INVISIBLE, which is worse than oversized.
        assert_eq!(object_scale_x(0.0), 1.0);
        // A corrupt dump value can't reach the wire either way.
        assert_eq!(object_scale_x(-2.0), 1.0);
        assert_eq!(object_scale_x(f32::NAN), 1.0);
    }

    #[test]
    fn gameobject_create_object_sends_scale_one_when_the_template_carries_no_size() {
        // The hand-seeded / pre-#107 path end-to-end: a 0 size must reach the descriptor as 1.0, i.e.
        // byte-identical to the old hardcoded behaviour, so no existing fixture changes proportions.
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

    /// Decode a `build_gameobject_rotation_values` body: strips the fixed envelope (amount_of_objects,
    /// has_transport, update_type, packed guid — byte-identical to `build_values_update_raw`'s other
    /// callers, pinned separately by `raw_values_body_matches_gtker_envelope`) and reads the 4
    /// consecutive `GAMEOBJECT_ROTATION` float slots in ascending index order. Test-only.
    fn decode_rotation_floats(guid: u64, body: &[u8]) -> [f32; 4] {
        let mut packed_guid = Vec::new();
        super::write_packed_guid_u64(&mut packed_guid, guid);
        let mask_start = 4 + 1 + 1 + packed_guid.len(); // amount_of_objects + has_transport + update_type + guid
        let block_count = body[mask_start] as usize;
        let values_start = mask_start + 1 + block_count * 4;
        let mut out = [0.0f32; 4];
        for (i, chunk) in body[values_start..values_start + 16].chunks_exact(4).enumerate() {
            out[i] = f32::from_le_bytes(chunk.try_into().unwrap());
        }
        out
    }

    #[test]
    fn gameobject_rotation_values_passes_through_a_stored_nonzero_quaternion() {
        // A non-degenerate stored quaternion (e.g. a bench flush against a sloped wall, rot0/rot1
        // nonzero) must ride the wire UNCHANGED — no derive-from-orientation fallback applies once
        // ANY of the 4 stored slots is nonzero.
        let go = GameObjectView {
            guid: 5,
            template_entry: 446,
            x: -9450.0,
            y: -90.0,
            z: 55.0,
            orientation: 1.57,
            state: 0,
            type_id: 0, // DOOR
            display_id: 259,
            rotation_0: 0.1,
            rotation_1: 0.2,
            rotation_2: 0.70710678,
            rotation_3: 0.70710678,
            size: 0.0, // not read by the rotation VALUES update
        };
        let (opcode, body) = build_gameobject_rotation_values(&go);
        assert_eq!(opcode, 0x00A9, "SMSG_UPDATE_OBJECT opcode");
        let floats = decode_rotation_floats(go.guid, &body);
        assert_eq!(floats, [0.1, 0.2, 0.70710678, 0.70710678]);
    }

    #[test]
    fn gameobject_rotation_values_derives_yaw_only_from_orientation_when_quaternion_is_all_zero() {
        // Every pre-#515 row and every hand-seeded fixture stores an all-zero quaternion — that must
        // NOT ride verbatim (a real spawn's quaternion is never exactly (0,0,0,0); sending it as-is
        // is the exact tilted-bench bug this issue reports). Derive rot2/rot3 = sin(o/2)/cos(o/2)
        // (Z-axis-only yaw) instead, leaving rot0/rot1 (terrain pitch) at 0.
        let go = GameObjectView {
            guid: 5,
            template_entry: 446,
            x: -9450.0,
            y: -90.0,
            z: 55.0,
            orientation: std::f32::consts::FRAC_PI_2, // 90°
            state: 0,
            type_id: 0,
            display_id: 259,
            rotation_0: 0.0,
            rotation_1: 0.0,
            rotation_2: 0.0,
            rotation_3: 0.0,
            size: 0.0, // not read by the rotation VALUES update
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
