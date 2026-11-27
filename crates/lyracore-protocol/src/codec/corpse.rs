use anyhow::{anyhow, Result};
use lyracore_shared::packing::unpack4;
use wow_world_messages::vanilla::{
    MSG_CORPSE_QUERY_Server, Map, MovementBlock, MovementBlock_UpdateFlag,
    MovementBlock_UpdateFlag_All, MovementBlock_UpdateFlag_Living, Object, ObjectType,
    UpdateCorpse, UpdateMask, Vector3d, SMSG_UPDATE_OBJECT,
};
use wow_world_messages::Guid;

#[derive(Clone, Debug, Default)]
pub struct CorpseView {
    pub guid: u64,
    pub owner_guid: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub display_id: u32,
    pub bytes_1: u32,
    pub bytes_2: u32,
    pub is_bones: bool,
    pub guild_id: u32,
    pub hide_helm: bool,
    pub hide_cloak: bool,
    /// CORPSE_FIELD_ITEM values, one per equipment slot; zero leaves a slot empty.
    pub worn_items: Vec<u32>,
}

const CORPSE_FIELD_ITEM: u16 = 13;
const CORPSE_FLAG_BONES: i32 = 0x01;
const CORPSE_FLAG_UNK2: i32 = 0x04;
const CORPSE_FLAG_HIDE_HELM: i32 = 0x08;
const CORPSE_FLAG_HIDE_CLOAK: i32 = 0x10;
const EQUIPMENT_SLOTS: usize = 19;

pub fn build_corpse_query_response(
    loc: Option<(u32, f32, f32, f32)>,
) -> Result<MSG_CORPSE_QUERY_Server> {
    match loc {
        None => Ok(MSG_CORPSE_QUERY_Server::NotFound),
        Some((map_id, x, y, z)) => {
            let map = Map::try_from(map_id).map_err(|_| anyhow!("invalid corpse map {map_id}"))?;
            Ok(MSG_CORPSE_QUERY_Server::Found {
                corpse_map: map,
                map,
                position: Vector3d { x, y, z },
            })
        }
    }
}

pub fn build_corpse_create_object(corpse: &CorpseView) -> Result<SMSG_UPDATE_OBJECT> {
    let (b1a, b1b, b1c, b1d) = unpack4(corpse.bytes_1);
    let (b2a, b2b, b2c, b2d) = unpack4(corpse.bytes_2);
    // A zero display id makes build 5875 dereference a null corpse model.
    let display_id = if corpse.display_id == 0 {
        49
    } else {
        corpse.display_id
    };
    let mut flags = CORPSE_FLAG_UNK2;
    if corpse.is_bones {
        flags |= CORPSE_FLAG_BONES;
    }
    if corpse.hide_helm {
        flags |= CORPSE_FLAG_HIDE_HELM;
    }
    if corpse.hide_cloak {
        flags |= CORPSE_FLAG_HIDE_CLOAK;
    }
    let mut builder = UpdateCorpse::builder()
        .set_object_guid(Guid::new(corpse.guid))
        .set_object_scale_x(1.0)
        .set_corpse_owner(Guid::new(corpse.owner_guid))
        .set_corpse_display_id(display_id as i32)
        .set_corpse_pos_x(corpse.x)
        .set_corpse_pos_y(corpse.y)
        .set_corpse_pos_z(corpse.z)
        .set_corpse_facing(corpse.orientation)
        .set_corpse_bytes_1(b1a, b1b, b1c, b1d)
        .set_corpse_bytes_2(b2a, b2b, b2c, b2d)
        .set_corpse_flags(flags);
    // vmangos `Object::_SetCreateBits` writes only the non-zero fields into a CREATE.
    if corpse.guild_id != 0 {
        builder = builder.set_corpse_guild(corpse.guild_id as i32);
    }
    let mask = builder.finalize();
    let update_flag = MovementBlock_UpdateFlag::empty()
        .set_all(MovementBlock_UpdateFlag_All { unknown1: 1 })
        .set_living(MovementBlock_UpdateFlag_Living::HasPosition {
            orientation: corpse.orientation,
            position: Vector3d {
                x: corpse.x,
                y: corpse.y,
                z: corpse.z,
            },
        });
    let mut create = SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::CreateObject2 {
            guid3: Guid::new(corpse.guid),
            object_type: ObjectType::Corpse,
            movement2: MovementBlock { update_flag },
            mask2: UpdateMask::Corpse(mask),
        }],
    };
    if corpse.worn_items.iter().any(|&item| item != 0) {
        // gtker exposes only slot 0 of CORPSE_FIELD_ITEM.
        super::entity::add_create_fields(&mut create, |mask| {
            for (slot, &item) in corpse.worn_items.iter().take(EQUIPMENT_SLOTS).enumerate() {
                if item != 0 {
                    mask.set_u32(CORPSE_FIELD_ITEM + slot as u16, item);
                }
            }
        })?;
    }
    Ok(create)
}

// Build 5875 has separate sickness and reclaim-delay bytes after the sized caster name.
pub fn build_resurrect_request_raw(caster_guid: u64, caster_name: &str) -> (u16, Vec<u8>) {
    let mut body = caster_guid.to_le_bytes().to_vec();
    body.extend_from_slice(&((caster_name.len() + 1) as u32).to_le_bytes());
    body.extend_from_slice(caster_name.as_bytes());
    body.extend_from_slice(&[0, 0, 0]);
    (0x015b, body)
}

// Build 5875 sends milliseconds, unlike the generated Seconds field.
pub fn build_corpse_reclaim_delay_raw(delay: std::time::Duration) -> (u16, Vec<u8>) {
    let millis = delay.as_millis().min(u128::from(u32::MAX)) as u32;
    (0x0269, millis.to_le_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::{
        MSG_CORPSE_QUERY_Server, Object, ObjectType, ServerMessage, UpdateMask,
    };

    #[test]
    fn reclaim_delay_is_milliseconds() {
        assert_eq!(
            build_corpse_reclaim_delay_raw(std::time::Duration::from_secs(30)),
            (0x0269, vec![0x30, 0x75, 0, 0])
        );
    }

    #[test]
    fn corpse_create_is_a_corpse_object_with_race_in_bytes1() {
        let corpse = CorpseView {
            guid: lyracore_shared::guid::compose(lyracore_shared::guid::CORPSE, 1),
            owner_guid: 1,
            x: -8949.0,
            y: -132.0,
            z: 83.0,
            orientation: 0.0,
            display_id: 49,
            bytes_1: (1u32 << 8) | (1u32 << 16) | (5u32 << 24),
            bytes_2: 0,
            is_bones: false,
            ..Default::default()
        };
        let msg = build_corpse_create_object(&corpse).unwrap();
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
    fn corpse_create_object_flags_a_body_and_its_bones_apart() {
        let live = CorpseView {
            guid: 1,
            owner_guid: 1,
            display_id: 49,
            is_bones: false,
            ..Default::default()
        };
        let msg = build_corpse_create_object(&live).unwrap();
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
        let msg2 = build_corpse_create_object(&bones).unwrap();
        match &msg2.objects[0] {
            Object::CreateObject2 {
                mask2: UpdateMask::Corpse(m),
                ..
            } => {
                assert_eq!(
                    m.corpse_flags(),
                    Some(0x05),
                    "vmangos Map::RemoveCorpses gives bones CORPSE_FLAG_UNK2 | CORPSE_FLAG_BONES"
                );
            }
            other => panic!("expected a Corpse CreateObject2, got {other:?}"),
        }
    }

    fn created_mask(corpse: &CorpseView) -> (UpdateCorpse, Vec<u8>) {
        let msg = build_corpse_create_object(corpse).unwrap();
        let mut buf = Vec::new();
        msg.write_unencrypted_server(&mut buf).unwrap();
        match msg.objects.into_iter().next() {
            Some(Object::CreateObject2 {
                mask2: UpdateMask::Corpse(mask),
                ..
            }) => (mask, buf),
            other => panic!("expected a Corpse CreateObject2, got {other:?}"),
        }
    }

    #[test]
    fn corpse_create_carries_guild_hidden_gear_flags_and_worn_items() {
        let mut worn_items = vec![0; 19];
        worn_items[0] = 1_234 | (1 << 24);
        worn_items[18] = 0x0A0B_0C0D;
        let (mask, buf) = created_mask(&CorpseView {
            guid: 1,
            owner_guid: 1,
            display_id: 49,
            guild_id: 7,
            hide_helm: true,
            hide_cloak: true,
            worn_items,
            ..Default::default()
        });
        assert_eq!(mask.corpse_guild(), Some(7));
        assert_eq!(
            mask.corpse_flags(),
            Some(0x04 | 0x08 | 0x10),
            "CORPSE_FLAG_UNK2 | CORPSE_FLAG_HIDE_HELM | CORPSE_FLAG_HIDE_CLOAK"
        );
        assert_eq!(mask.corpse_item(), Some(1_234 | (1 << 24)));
        assert!(
            buf.windows(4).any(|w| w == 0x0A0B_0C0D_u32.to_le_bytes()),
            "CORPSE_FIELD_ITEM + 18 must reach the CREATE; buf {buf:02x?}"
        );
    }

    #[test]
    fn corpse_create_leaves_guild_and_items_out_when_empty() {
        let (mask, _) = created_mask(&CorpseView {
            guid: 1,
            owner_guid: 1,
            display_id: 49,
            worn_items: vec![0; 19],
            ..Default::default()
        });
        assert_eq!(mask.corpse_guild(), None);
        assert_eq!(mask.corpse_item(), None);
        assert_eq!(mask.corpse_flags(), Some(0x04));
    }

    #[test]
    fn corpse_query_found_carries_position_else_not_found() {
        match build_corpse_query_response(Some((0, -8949.0, -132.0, 83.0))).unwrap() {
            MSG_CORPSE_QUERY_Server::Found { position, .. } => {
                assert_eq!(position.x, -8949.0);
            }
            other => panic!("expected Found, got {other:?}"),
        }
        assert!(matches!(
            build_corpse_query_response(None).unwrap(),
            MSG_CORPSE_QUERY_Server::NotFound
        ));
    }

    #[test]
    fn resurrect_request_names_the_caster_without_sickness_or_delay() {
        let (opcode, body) = build_resurrect_request_raw(42, "Priestly");
        assert_eq!(opcode, 0x015b);
        assert_eq!(
            body,
            [
                42, 0, 0, 0, 0, 0, 0, 0, 9, 0, 0, 0, b'P', b'r', b'i', b'e', b's', b't', b'l',
                b'y', 0, 0, 0
            ]
        );
    }

    #[test]
    fn corpse_create_object_falls_back_to_display_49_when_zero() {
        let corpse = CorpseView {
            guid: 1,
            owner_guid: 1,
            display_id: 0,
            ..Default::default()
        };
        let msg = build_corpse_create_object(&corpse).unwrap();
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
        let msg2 = build_corpse_create_object(&real).unwrap();
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
}
