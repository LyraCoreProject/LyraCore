//! Movement wire mapping: the inbound `MSG_MOVE_*` relay (peer animation), the server-driven
//! `SMSG_MONSTER_MOVE` leg, and the `MovementInfo` ↔ raw-bytes carrier used to ferry a movement
//! block across the DB. Pure code-motion out of `mod.rs`.

use super::*;

/// The opcode of an inbound movement message, if it's one of the slice's relayed `MSG_MOVE_*`
/// (Phase 5). Used to record + relay the move under the *same* opcode.
pub fn relayed_move_opcode(msg: &ClientOpcodeMessage) -> Option<u32> {
    use movement_opcodes as m;
    Some(match msg {
        ClientOpcodeMessage::MSG_MOVE_START_FORWARD(_) => m::MSG_MOVE_START_FORWARD,
        ClientOpcodeMessage::MSG_MOVE_START_BACKWARD(_) => m::MSG_MOVE_START_BACKWARD,
        ClientOpcodeMessage::MSG_MOVE_STOP(_) => m::MSG_MOVE_STOP,
        ClientOpcodeMessage::MSG_MOVE_START_STRAFE_LEFT(_) => m::MSG_MOVE_START_STRAFE_LEFT,
        ClientOpcodeMessage::MSG_MOVE_START_STRAFE_RIGHT(_) => m::MSG_MOVE_START_STRAFE_RIGHT,
        ClientOpcodeMessage::MSG_MOVE_STOP_STRAFE(_) => m::MSG_MOVE_STOP_STRAFE,
        ClientOpcodeMessage::MSG_MOVE_JUMP(_) => m::MSG_MOVE_JUMP,
        ClientOpcodeMessage::MSG_MOVE_START_TURN_LEFT(_) => m::MSG_MOVE_START_TURN_LEFT,
        ClientOpcodeMessage::MSG_MOVE_START_TURN_RIGHT(_) => m::MSG_MOVE_START_TURN_RIGHT,
        ClientOpcodeMessage::MSG_MOVE_STOP_TURN(_) => m::MSG_MOVE_STOP_TURN,
        ClientOpcodeMessage::MSG_MOVE_SET_RUN_MODE(_) => m::MSG_MOVE_SET_RUN_MODE,
        ClientOpcodeMessage::MSG_MOVE_SET_WALK_MODE(_) => m::MSG_MOVE_SET_WALK_MODE,
        ClientOpcodeMessage::MSG_MOVE_FALL_LAND(_) => m::MSG_MOVE_FALL_LAND,
        ClientOpcodeMessage::MSG_MOVE_START_SWIM(_) => m::MSG_MOVE_START_SWIM,
        ClientOpcodeMessage::MSG_MOVE_STOP_SWIM(_) => m::MSG_MOVE_STOP_SWIM,
        ClientOpcodeMessage::MSG_MOVE_SET_FACING(_) => m::MSG_MOVE_SET_FACING,
        ClientOpcodeMessage::MSG_MOVE_HEARTBEAT(_) => m::MSG_MOVE_HEARTBEAT,
        _ => return None,
    })
}

/// Re-emit a relayed `MSG_MOVE_*` to an observer (Phase 6): the **same** opcode, the mover's
/// (packed) guid, and the carried `MovementInfo` verbatim — which is what makes the peer client
/// animate run/walk/turn rather than teleport. Returns `None` for a non-relayed opcode. The
/// crate packs the guid and frames the header on write (`ServerOpcodeMessage::write_*_server`).
///
/// **No longer on the hot path** (work-item 286): the peer relay ships
/// [`build_movement_relay_raw`]'s pre-serialized body instead of paying a decode + re-encode per
/// observer. This stays as the typed REFERENCE implementation the raw builder is pinned against —
/// `codec::tests::raw_movement_relay_is_byte_identical_to_the_typed_path` serializes it and demands
/// the raw bytes match, which is the only thing standing between a memcpy relay and a silent
/// peer-position desync. Deleting it would delete that proof, so it is deliberately kept.
#[cfg_attr(not(test), allow(dead_code))]
pub fn build_movement_relay(
    opcode: u32,
    mover_guid: u64,
    info: MovementInfo,
) -> Option<ServerOpcodeMessage> {
    use movement_opcodes as m;
    let guid = Guid::new(mover_guid);
    Some(match opcode {
        m::MSG_MOVE_START_FORWARD => {
            ServerOpcodeMessage::MSG_MOVE_START_FORWARD(Box::new(MSG_MOVE_START_FORWARD_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_START_BACKWARD => {
            ServerOpcodeMessage::MSG_MOVE_START_BACKWARD(Box::new(MSG_MOVE_START_BACKWARD_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_STOP => {
            ServerOpcodeMessage::MSG_MOVE_STOP(Box::new(MSG_MOVE_STOP_Server { guid, info }))
        }
        m::MSG_MOVE_START_STRAFE_LEFT => ServerOpcodeMessage::MSG_MOVE_START_STRAFE_LEFT(Box::new(
            MSG_MOVE_START_STRAFE_LEFT_Server { guid, info },
        )),
        m::MSG_MOVE_START_STRAFE_RIGHT => ServerOpcodeMessage::MSG_MOVE_START_STRAFE_RIGHT(
            Box::new(MSG_MOVE_START_STRAFE_RIGHT_Server { guid, info }),
        ),
        m::MSG_MOVE_STOP_STRAFE => {
            ServerOpcodeMessage::MSG_MOVE_STOP_STRAFE(Box::new(MSG_MOVE_STOP_STRAFE_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_JUMP => {
            ServerOpcodeMessage::MSG_MOVE_JUMP(Box::new(MSG_MOVE_JUMP_Server { guid, info }))
        }
        m::MSG_MOVE_START_TURN_LEFT => ServerOpcodeMessage::MSG_MOVE_START_TURN_LEFT(Box::new(
            MSG_MOVE_START_TURN_LEFT_Server { guid, info },
        )),
        m::MSG_MOVE_START_TURN_RIGHT => ServerOpcodeMessage::MSG_MOVE_START_TURN_RIGHT(Box::new(
            MSG_MOVE_START_TURN_RIGHT_Server { guid, info },
        )),
        m::MSG_MOVE_STOP_TURN => {
            ServerOpcodeMessage::MSG_MOVE_STOP_TURN(Box::new(MSG_MOVE_STOP_TURN_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_SET_RUN_MODE => {
            ServerOpcodeMessage::MSG_MOVE_SET_RUN_MODE(Box::new(MSG_MOVE_SET_RUN_MODE_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_SET_WALK_MODE => {
            ServerOpcodeMessage::MSG_MOVE_SET_WALK_MODE(Box::new(MSG_MOVE_SET_WALK_MODE_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_FALL_LAND => {
            ServerOpcodeMessage::MSG_MOVE_FALL_LAND(Box::new(MSG_MOVE_FALL_LAND_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_START_SWIM => {
            ServerOpcodeMessage::MSG_MOVE_START_SWIM(Box::new(MSG_MOVE_START_SWIM_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_STOP_SWIM => {
            ServerOpcodeMessage::MSG_MOVE_STOP_SWIM(Box::new(MSG_MOVE_STOP_SWIM_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_SET_FACING => {
            ServerOpcodeMessage::MSG_MOVE_SET_FACING(Box::new(MSG_MOVE_SET_FACING_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_HEARTBEAT => {
            ServerOpcodeMessage::MSG_MOVE_HEARTBEAT(Box::new(MSG_MOVE_HEARTBEAT_Server {
                guid,
                info,
            }))
        }
        _ => return None,
    })
}

/// The shortest legal `MovementInfo` body: `flags u32 + timestamp u32 + position 12 +
/// orientation f32 + fall_time f32`. Every optional block (transport / pitch / jump / spline
/// elevation) only makes it longer, so this is a floor, not an exact size — it exists so a truncated
/// row can never be framed as a movement packet (the client would parse past the end of the body and
/// desync its stream).
const MIN_MOVEMENT_INFO_LEN: usize = 4 + 4 + 12 + 4 + 4;

/// The RAW twin of [`build_movement_relay`] (work-item 286): the same `MSG_MOVE_*` packet, built as
/// `(opcode, body)` for [`Outbound::Raw`](crate::world::Outbound::Raw) **without decoding
/// `info_bytes`**. `game_entity_motion.movement_info` already holds exactly the bytes gtker would
/// serialize a `MovementInfo` into (the gateway put them there via [`movement_info_to_bytes`]), and
/// the server-side body is `PackedGuid guid` followed by that block verbatim — so the relay is a
/// packed guid plus a memcpy, not a decode and a re-encode.
///
/// Why it matters: this is called once per OBSERVER, per mover, per heartbeat. At 371 co-located
/// clients that is ~275 k/s, and the typed path paid a full parse + a fresh `ServerOpcodeMessage`
/// for each one to produce an identical ~30-byte body. Only the header differs per observer, and the
/// writer already frames and encrypts that per session.
///
/// Returns `None` for a non-relayed opcode (matching `build_movement_relay`'s `None`, pinned
/// exhaustively by `codec::tests`) or for an `info_bytes` too short to be a `MovementInfo`.
/// **The body is byte-identical to what `build_movement_relay` serializes** — that is the whole
/// premise, and it is asserted, not assumed, in `raw_movement_relay_is_byte_identical_to_the_typed_path`.
pub fn build_movement_relay_raw(
    opcode: u32,
    mover_guid: u64,
    info_bytes: &[u8],
) -> Option<(u16, Vec<u8>)> {
    if !movement_opcodes::is_slice_move(opcode) || info_bytes.len() < MIN_MOVEMENT_INFO_LEN {
        return None;
    }
    // 9 = the largest packed guid (pattern byte + 8 non-zero bytes).
    let mut body = Vec::with_capacity(9 + info_bytes.len());
    super::values::write_packed_guid_u64(&mut body, mover_guid);
    body.extend_from_slice(info_bytes);
    // `MSG_MOVE_*` opcodes are bidirectional: the server re-emits under the SAME number the client
    // sent (the `_Server` message's `OPCODE` equals the `_Client` one), so no mapping is needed.
    Some((opcode as u16, body))
}

/// Build `MSG_MOVE_TELEPORT_ACK_Server` (same-map teleport, opcode 0x00C7) moving `mover_guid` to
/// `(x, y, z)` facing `o`. The 5875 client applies the position on receipt and echoes a CMSG
/// `MSG_MOVE_TELEPORT_ACK` we accept-and-ignore, so `counter` (the movement counter it echoes) can be 0 —
/// we don't gate on its reply. The `MovementInfo` carries no movement flags (a clean stand-still arrival).
/// This is the reusable teleport leg: graveyard release, `debug_teleport`, and future `.tele`/hearthstone.
pub fn build_teleport_ack(
    mover_guid: u64,
    counter: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) -> MSG_MOVE_TELEPORT_ACK_Server {
    MSG_MOVE_TELEPORT_ACK_Server {
        guid: Guid::new(mover_guid),
        movement_counter: counter,
        info: MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: 0,
            position: Vector3d { x, y, z },
            orientation: o,
            fall_time: 0.0,
        },
    }
}

/// Build the cross-map teleport-ARRIVAL pair (work-item 224): `SMSG_TRANSFER_PENDING(map)` (opcode
/// 0x003F — starts the client's loading screen) immediately followed by `SMSG_NEW_WORLD(map, x, y, z, o)`
/// (opcode 0x003E — the position on the new map). Sent back-to-back as ONE batch: both go out BEFORE the client even starts
/// loading — the client only replies `MSG_MOVE_WORLDPORT_ACK` (opcode 0x00DC, empty body — [V]ed against
/// the vendored gtker crate, `wow_world_messages` 0.3.0) once ITS OWN load completes, at which point the
/// gateway drives the entity rebuild + full subscription reset (`world::mod.rs`'s `enter_world`).
/// `map_id` must resolve to a client-known `Map` variant — `Err` in the pathological case (an unmapped id).
pub fn build_cross_map_teleport(
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) -> Result<[ServerOpcodeMessage; 2]> {
    let map = Map::try_from(map_id).map_err(|_| anyhow!("invalid map {map_id}"))?;
    Ok([
        ServerOpcodeMessage::SMSG_TRANSFER_PENDING(Box::new(SMSG_TRANSFER_PENDING {
            map,
            has_transport: None,
        })),
        ServerOpcodeMessage::SMSG_NEW_WORLD(Box::new(SMSG_NEW_WORLD {
            map,
            position: Vector3d { x, y, z },
            orientation: o,
        })),
    ])
}

/// Build `SMSG_TRANSFER_ABORTED` (opcode 0x0040) — "the transfer you are loading for is not going to
/// happen". Issue #39: the ONLY thing worse than failing a dungeon entry is not telling the client,
/// because a client that has already been sent `SMSG_TRANSFER_PENDING` sits on its loading screen
/// forever with no error and no way out. This is the packet vanilla servers send on exactly this
/// path (mangos `SendTransferAborted`, e.g. "instance not found" / "instance is full"), so the
/// client's own handler surfaces it as a dialog and stops loading.
///
/// `[V]`: the wire shape is pinned against the vendored gtker crate (`Map` u32 + `TransferAbortReason`
/// u8 + a trailing `argument` u8, body 6 bytes); the client-side DIALOG has not been eyeballed on a
/// live 1.12.1 client yet — the server-side property (the client is TOLD, and the socket is not
/// silently dropped mid-load) is what this fixes and what the tests pin.
///
/// `map_id` must resolve to a client-known `Map` variant — `Err` for an unmapped id, which the
/// caller treats as "no abort packet", never as a reason to skip the rest of its failure handling.
pub fn build_transfer_aborted(
    map_id: u32,
    reason: TransferAbortReason,
) -> Result<SMSG_TRANSFER_ABORTED> {
    Ok(SMSG_TRANSFER_ABORTED {
        map: Map::try_from(map_id).map_err(|_| anyhow!("invalid map {map_id}"))?,
        reason,
        argument: 0,
    })
}

/// Build `SMSG_FORCE_RUN_SPEED_CHANGE` (opcode 0x00E2): tell the client `guid` now runs at `speed`
/// yards/sec. Sprint (and any future move-speed aura) pushes this so the client actually moves faster —
/// player movement is client-authoritative, so the server-side `A_MOD_SPEED(MOVE)` aura alone never
/// changes the client's speed. `move_event` is 0 (cmangos/vmangos convention). The client echoes
/// `CMSG_FORCE_RUN_SPEED_CHANGE_ACK`, which we accept-and-ignore (we don't gate on its reply).
pub fn build_force_run_speed(guid: u64, speed: f32) -> SMSG_FORCE_RUN_SPEED_CHANGE {
    SMSG_FORCE_RUN_SPEED_CHANGE {
        guid: Guid::new(guid),
        move_event: 0,
        speed,
    }
}

/// Build `SMSG_MONSTER_MOVE` for one server-driven creature leg (Phase N2): a straight walk from
/// `start` to `dest` over `duration_ms`. EXACTLY ONE destination point — the crate writes a single
/// spline point absolute, avoiding its multi-point packed-offset encoding (not 1.12-compatible).
/// `spline_id` must strictly increase per creature or the client ignores the new spline.
pub fn build_monster_move(
    mover_guid: u64,
    start: Vector3d,
    dest: Vector3d,
    duration_ms: u32,
    spline_id: u32,
    run: bool,
) -> SMSG_MONSTER_MOVE {
    SMSG_MONSTER_MOVE {
        guid: Guid::new(mover_guid),
        spline_point: start,
        spline_id,
        move_type: SMSG_MONSTER_MOVE_MonsterMoveType::Normal,
        // RUN_MODE (0x100) = run animation; 0 = walk. Set from the leg's `run` flag so a RUN-speed
        // chase/return/flee/fear leg run-animates (without it every leg walk-animated → moonwalk).
        spline_flags: SplineFlag::new(if run { 0x100 } else { 0 }),
        duration: duration_ms,
        splines: vec![dest],
    }
}

/// Carry a `MovementInfo` across the DB as opaque bytes (`game_movement_event.movement_info`).
/// The crate's `MovementInfo` (de)serializer is `pub(crate)`, but `MSG_MOVE_HEARTBEAT_Client`'s
/// whole body *is* exactly one `MovementInfo` and its codec is public — so we round-trip through
/// it. The vanilla client header is 6 bytes: `[(body_len+4) as u16 BE][opcode as u32 LE]`.
const MOVE_HEADER_LEN: usize = 6;

/// Serialize a `MovementInfo` to its raw body bytes (what travels in the event row).
pub fn movement_info_to_bytes(info: &MovementInfo) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    MSG_MOVE_HEARTBEAT_Client { info: info.clone() }.write_unencrypted_client(&mut buf)?;
    if buf.len() < MOVE_HEADER_LEN {
        return Err(anyhow!(
            "movement body shorter than header ({} bytes)",
            buf.len()
        ));
    }
    Ok(buf.split_off(MOVE_HEADER_LEN))
}

/// Reconstruct a `MovementInfo` from raw body bytes by framing them as a `MSG_MOVE_HEARTBEAT`
/// client packet and decoding. All `MSG_MOVE_*` client bodies are an identical `MovementInfo`, so
/// HEARTBEAT works as the carrier regardless of the event's real opcode.
///
/// The relay stopped calling this in work-item 286 (it memcpys the stored block instead), so its
/// only remaining callers are tests — kept as the decode half of the `movement_info_to_bytes`
/// round-trip that pins the carrier format the module's rows are written in.
#[cfg_attr(not(test), allow(dead_code))]
pub fn bytes_to_movement_info(body: &[u8]) -> Result<MovementInfo> {
    let size_field = (body.len() as u16)
        .checked_add(4)
        .ok_or_else(|| anyhow!("movement body too large"))?;
    let mut framed = Vec::with_capacity(MOVE_HEADER_LEN + body.len());
    framed.extend_from_slice(&size_field.to_be_bytes());
    framed.extend_from_slice(&(movement_opcodes::MSG_MOVE_HEARTBEAT).to_le_bytes());
    framed.extend_from_slice(body);
    match ClientOpcodeMessage::read_unencrypted(&mut framed.as_slice()) {
        Ok(ClientOpcodeMessage::MSG_MOVE_HEARTBEAT(c)) => Ok(c.info),
        Ok(other) => Err(anyhow!("movement carrier decoded to unexpected {other}")),
        Err(e) => Err(anyhow!("movement carrier decode failed: {e}")),
    }
}
