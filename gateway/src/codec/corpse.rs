//! Corpse wire mapping: the `MSG_CORPSE_QUERY` reply + the corpse CREATE_OBJECT (slice 5), plus the
//! Resurrection accept-prompt (#014).

use super::*;

/// A corpse row as the gateway reads it (`game_corpse`), flattened for the CORPSE CREATE_OBJECT.
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
    /// Body-decayed-to-bones state (work-item 201: `game_corpse.is_bones`). Drives the
    /// `CORPSE_FLAG_BONE` (0x01) vs the normal body flag (0x04) in `build_corpse_create_object`.
    pub is_bones: bool,
}

/// Build the `MSG_CORPSE_QUERY` reply (slice 5): tells the client where the player's corpse is so it
/// draws the map marker and offers "Reclaim Corpse" when the ghost is near it. `None` → `NotFound`.
/// Same map for `map` (ghost map) and `corpse_map` — we have no cross-map corpses (single zone).
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

/// Build the CREATE_OBJECT for a player corpse (slice 5): a CORPSE-type object with a stationary
/// position (`HasPosition`, UPDATEFLAG 0x40) and the `UpdateCorpse` descriptors (owner, position,
/// display, bytes) so the client renders the dead body. Relayed on `game_corpse` insert; the matching
/// `SMSG_DESTROY_OBJECT` on delete (reclaim/decay) reuses `build_destroy_object`.
pub fn build_corpse_create_object(corpse: &CorpseView) -> SMSG_UPDATE_OBJECT {
    let (b1a, b1b, b1c, b1d) = unpack4(corpse.bytes_1);
    let (b2a, b2b, b2c, b2d) = unpack4(corpse.bytes_2);
    // Defensive floor against the trap-#3 null-model crash: CORPSE_FIELD_DISPLAY_ID must be a valid
    // display or the 5875 client dereferences a null model. The owner's native display drives it (49
    // for the current Human-Male fixture; race-derived once login is DBC-sourced), but never send 0.
    let display_id = if corpse.display_id == 0 {
        49
    } else {
        corpse.display_id
    };
    let mask = UpdateCorpse::builder()
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
        // CORPSE_FLAG_BONE (0x01) once the body has decayed to bones (work-item 201: no longer a
        // reclaim target — see `Corpse::is_bones`/`reclaim_corpse`); otherwise CORPSE_FLAG_UNK2
        // (0x04), what every real vanilla body carries. UNVERIFIED-until-observed: no live client has
        // confirmed the bones render actually reads this bit (flag semantics inferred from mangos'
        // `CORPSE_FLAG_BONE` naming, not from watching a real client render decayed bones).
        .set_corpse_flags(if corpse.is_bones { 0x01 } else { 0x04 })
        .finalize();
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
    SMSG_UPDATE_OBJECT {
        has_transport: 0,
        objects: vec![Object::CreateObject2 {
            guid3: Guid::new(corpse.guid),
            object_type: ObjectType::Corpse,
            movement2: MovementBlock { update_flag },
            mask2: UpdateMask::Corpse(mask),
        }],
    }
}

/// Build the `SMSG_RESURRECT_REQUEST` offer sent to a dead ally (#014): names the CASTER (so the
/// client's "<Name> requests to resurrect you" prompt reads correctly) and the guid the client echoes
/// back in `CMSG_RESURRECT_RESPONSE`. `player = true` unconditionally — E_RESURRECT only ever offers this
/// to a player target (the module gates non-player targets out before the row is even inserted), so the
/// gateway never has to special-case an NPC-cast revive here.
pub fn build_resurrect_request(caster_guid: u64, caster_name: String) -> SMSG_RESURRECT_REQUEST {
    SMSG_RESURRECT_REQUEST {
        guid: Guid::new(caster_guid),
        name: caster_name,
        player: true,
    }
}
