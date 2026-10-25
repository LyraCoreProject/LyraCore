//! Build 5875 uses a different transport flag and layout from the typed codec.

use anyhow::{ensure, Result};
use lyracore_shared::env::{MOVEMENT_FLAG_ON_TRANSPORT, MOVEMENT_FLAG_SWIMMING};
use std::io::Read;
use wow_world_messages::vanilla::{
    MovementInfo, MovementInfo_MovementFlags, MovementInfo_MovementFlags_Jumping,
    MovementInfo_MovementFlags_OnTransport, MovementInfo_MovementFlags_SplineElevation,
    MovementInfo_MovementFlags_Swimming, TransportInfo, Vector3d,
};
use wow_world_messages::Guid;

// Steering and walk-mode flags, which add no fields to a MovementInfo.
pub(crate) const SIMPLE_MOVE_MASK: u32 = 0x01 | 0x02 | 0x04 | 0x08 | 0x10 | 0x20 | 0x100;
const JUMPING: u32 = 0x2000;
const SPLINE_ELEVATION: u32 = 0x0400_0000;

fn word(body: &mut &[u8]) -> Result<[u8; 4]> {
    let mut bytes = [0; 4];
    body.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn float(body: &mut &[u8]) -> Result<f32> {
    Ok(f32::from_le_bytes(word(body)?))
}

fn position(body: &mut &[u8]) -> Result<Vector3d> {
    Ok(Vector3d {
        x: float(body)?,
        y: float(body)?,
        z: float(body)?,
    })
}

pub fn bytes_to_movement_info(mut body: &[u8]) -> Result<MovementInfo> {
    let flags = u32::from_le_bytes(word(&mut body)?);
    let timestamp = u32::from_le_bytes(word(&mut body)?);
    let pos = position(&mut body)?;
    let orientation = float(&mut body)?;
    let transport = if flags & MOVEMENT_FLAG_ON_TRANSPORT != 0 {
        let mut guid = [0; 8];
        body.read_exact(&mut guid)?;
        Some(MovementInfo_MovementFlags_OnTransport {
            transport: TransportInfo {
                guid: Guid::new(u64::from_le_bytes(guid)),
                position: position(&mut body)?,
                orientation: float(&mut body)?,
                timestamp: 0,
            },
        })
    } else {
        None
    };
    let swimming = if flags & MOVEMENT_FLAG_SWIMMING != 0 {
        Some(MovementInfo_MovementFlags_Swimming {
            pitch: float(&mut body)?,
        })
    } else {
        None
    };
    let fall_time = f32::from_bits(u32::from_le_bytes(word(&mut body)?));
    let jumping = if flags & JUMPING != 0 {
        Some(MovementInfo_MovementFlags_Jumping {
            z_speed: float(&mut body)?,
            cos_angle: float(&mut body)?,
            sin_angle: float(&mut body)?,
            xy_speed: float(&mut body)?,
        })
    } else {
        None
    };
    let spline = if flags & SPLINE_ELEVATION != 0 {
        Some(MovementInfo_MovementFlags_SplineElevation {
            spline_elevation: float(&mut body)?,
        })
    } else {
        None
    };
    ensure!(body.is_empty(), "unread movement bytes: {}", body.len());
    Ok(MovementInfo {
        flags: MovementInfo_MovementFlags::new(flags, transport, jumping, swimming, spline),
        timestamp,
        position: pos,
        orientation,
        fall_time,
    })
}

pub fn movement_info_to_bytes(info: &MovementInfo) -> Result<Vec<u8>> {
    use wow_world_messages::vanilla::{ClientMessage, MSG_MOVE_HEARTBEAT_Client};
    let mut framed = Vec::new();
    MSG_MOVE_HEARTBEAT_Client { info: info.clone() }.write_unencrypted_client(&mut framed)?;
    let mut body = framed.split_off(6);
    let raw_flags = u32::from_le_bytes(word(&mut body.as_slice())?);
    let flags = &info.flags;
    for (mask, present) in [
        (
            MOVEMENT_FLAG_ON_TRANSPORT,
            flags.get_on_transport().is_some(),
        ),
        (MOVEMENT_FLAG_SWIMMING, flags.get_swimming().is_some()),
        (JUMPING, flags.get_jumping().is_some()),
        (SPLINE_ELEVATION, flags.get_spline_elevation().is_some()),
    ] {
        ensure!(
            (raw_flags & mask != 0) == present,
            "movement flag {mask:#010x} disagrees with its body"
        );
    }
    if let Some(tail) = flags.get_on_transport() {
        let guid = tail.transport.guid.guid();
        let packed_len = 1 + guid.to_le_bytes().iter().filter(|&&byte| byte != 0).count();
        let mut transport = Vec::with_capacity(24);
        transport.extend_from_slice(&guid.to_le_bytes());
        for value in [
            tail.transport.position.x,
            tail.transport.position.y,
            tail.transport.position.z,
            tail.transport.orientation,
        ] {
            transport.extend_from_slice(&value.to_le_bytes());
        }
        body.splice(24..24 + packed_len + 20, transport);
    }
    Ok(body)
}

/// A Passenger's MovementInfo: the world pose, then the full Transport GUID and the offset on it.
pub fn passenger_movement_bytes(
    flags: u32,
    timestamp: u32,
    world: [f32; 4],
    transport_guid: u64,
    offset: [f32; 4],
) -> Vec<u8> {
    let mut body = Vec::with_capacity(52);
    body.extend_from_slice(&(flags & SIMPLE_MOVE_MASK | MOVEMENT_FLAG_ON_TRANSPORT).to_le_bytes());
    body.extend_from_slice(&timestamp.to_le_bytes());
    for value in world {
        body.extend_from_slice(&value.to_le_bytes());
    }
    body.extend_from_slice(&transport_guid.to_le_bytes());
    for value in offset {
        body.extend_from_slice(&value.to_le_bytes());
    }
    body.extend_from_slice(&0u32.to_le_bytes());
    body
}

pub fn create_with_movement(
    create: &wow_world_messages::vanilla::SMSG_UPDATE_OBJECT,
    movement: &[u8],
) -> Result<Vec<u8>> {
    use wow_world_messages::vanilla::ServerMessage;
    bytes_to_movement_info(movement)?;
    let mut framed = Vec::new();
    create.write_unencrypted_server(&mut framed)?;
    let mut body = framed.split_off(4);
    ensure!(
        body.len() >= 9 && body[..4] == 1u32.to_le_bytes() && body[5] == 3,
        "expected one CREATE_OBJECT2"
    );
    let living = 9 + body[6].count_ones() as usize;
    ensure!(
        body.len() >= living + 28 && body[living - 1] & 0x20 != 0,
        "expected a living CREATE"
    );
    let flags = u32::from_le_bytes(body[living..living + 4].try_into()?);
    ensure!(
        flags & !SIMPLE_MOVE_MASK == 0,
        "initial CREATE already has conditional movement"
    );
    body.splice(living..living + 28, movement.iter().copied());
    Ok(body)
}

/// Vanilla acknowledgements: full mover guid, counter, pose, then `trailing` fixed bytes.
fn acknowledgement(body: &[u8], trailing: usize) -> Result<(Guid, u32, MovementInfo, [u8; 4])> {
    ensure!(
        body.len() >= 12 + trailing,
        "short movement acknowledgement"
    );
    let (head, rest) = body.split_at(12);
    let (pose, tail) = rest.split_at(rest.len() - trailing);
    let mut word = [0; 4];
    word[..trailing].copy_from_slice(tail);
    Ok((
        Guid::new(u64::from_le_bytes(head[..8].try_into()?)),
        u32::from_le_bytes(head[8..].try_into()?),
        bytes_to_movement_info(pose)?,
        word,
    ))
}

/// Read before the typed reader, which misreads the build-5875 transport tail.
pub fn read_movement_client(
    opcode: u32,
    body: &[u8],
) -> Result<Option<super::ClientOpcodeMessage>> {
    use super::{movement_opcodes, ClientOpcodeMessage as C};
    use wow_world_messages::vanilla::*;
    macro_rules! speed_ack {
        ($variant:ident, $counter:ident) => {{
            let (guid, $counter, info, speed) = acknowledgement(body, 4)?;
            C::$variant(Box::new($variant {
                guid,
                $counter,
                info,
                new_speed: f32::from_le_bytes(speed),
            }))
        }};
    }
    macro_rules! flag_ack {
        ($variant:ident) => {{
            let (guid, movement_counter, info, apply) = acknowledgement(body, 4)?;
            C::$variant(Box::new($variant {
                guid,
                movement_counter,
                info,
                apply: u32::from_le_bytes(apply),
            }))
        }};
    }
    macro_rules! pose_ack {
        ($variant:ident) => {{
            let (guid, movement_counter, info, _) = acknowledgement(body, 0)?;
            C::$variant(Box::new($variant {
                guid,
                movement_counter,
                info,
            }))
        }};
    }
    match opcode {
        0x00E3 => return Ok(Some(speed_ack!(CMSG_FORCE_RUN_SPEED_CHANGE_ACK, counter))),
        0x00E5 => {
            return Ok(Some(speed_ack!(
                CMSG_FORCE_RUN_BACK_SPEED_CHANGE_ACK,
                movement_counter
            )))
        }
        0x00E7 => return Ok(Some(speed_ack!(CMSG_FORCE_SWIM_SPEED_CHANGE_ACK, counter))),
        0x02DB => return Ok(Some(speed_ack!(CMSG_FORCE_WALK_SPEED_CHANGE_ACK, counter))),
        0x02DD => {
            return Ok(Some(speed_ack!(
                CMSG_FORCE_SWIM_BACK_SPEED_CHANGE_ACK,
                counter
            )))
        }
        0x02DF => return Ok(Some(speed_ack!(CMSG_FORCE_TURN_RATE_CHANGE_ACK, counter))),
        0x00E9 => return Ok(Some(pose_ack!(CMSG_FORCE_MOVE_ROOT_ACK))),
        0x00EB => return Ok(Some(pose_ack!(CMSG_FORCE_MOVE_UNROOT_ACK))),
        0x00F0 => {
            let (guid, counter, info, _) = acknowledgement(body, 0)?;
            return Ok(Some(C::CMSG_MOVE_KNOCK_BACK_ACK(Box::new(
                CMSG_MOVE_KNOCK_BACK_ACK {
                    guid,
                    counter,
                    info,
                },
            ))));
        }
        0x00F6 => {
            let (guid, counter, info, applied) = acknowledgement(body, 4)?;
            return Ok(Some(C::CMSG_MOVE_HOVER_ACK(Box::new(
                CMSG_MOVE_HOVER_ACK {
                    guid,
                    counter,
                    info,
                    is_applied: u32::from_le_bytes(applied),
                },
            ))));
        }
        0x02CF => return Ok(Some(flag_ack!(CMSG_MOVE_FEATHER_FALL_ACK))),
        0x02D0 => return Ok(Some(flag_ack!(CMSG_MOVE_WATER_WALK_ACK))),
        _ => {}
    }
    if !movement_opcodes::is_slice_move(opcode) {
        return Ok(None);
    }
    let info = bytes_to_movement_info(body)?;
    macro_rules! moves {
        ($($opcode:ident => $variant:ident),+ $(,)?) => {
            match opcode {
                $(movement_opcodes::$opcode => C::$opcode(Box::new($variant { info })),)+
                _ => return Ok(None),
            }
        };
    }
    Ok(Some(moves! {
        MSG_MOVE_START_FORWARD => MSG_MOVE_START_FORWARD_Client,
        MSG_MOVE_START_BACKWARD => MSG_MOVE_START_BACKWARD_Client,
        MSG_MOVE_STOP => MSG_MOVE_STOP_Client,
        MSG_MOVE_START_STRAFE_LEFT => MSG_MOVE_START_STRAFE_LEFT_Client,
        MSG_MOVE_START_STRAFE_RIGHT => MSG_MOVE_START_STRAFE_RIGHT_Client,
        MSG_MOVE_STOP_STRAFE => MSG_MOVE_STOP_STRAFE_Client,
        MSG_MOVE_JUMP => MSG_MOVE_JUMP_Client,
        MSG_MOVE_START_TURN_LEFT => MSG_MOVE_START_TURN_LEFT_Client,
        MSG_MOVE_START_TURN_RIGHT => MSG_MOVE_START_TURN_RIGHT_Client,
        MSG_MOVE_STOP_TURN => MSG_MOVE_STOP_TURN_Client,
        MSG_MOVE_START_PITCH_UP => MSG_MOVE_START_PITCH_UP_Client,
        MSG_MOVE_START_PITCH_DOWN => MSG_MOVE_START_PITCH_DOWN_Client,
        MSG_MOVE_STOP_PITCH => MSG_MOVE_STOP_PITCH_Client,
        MSG_MOVE_SET_RUN_MODE => MSG_MOVE_SET_RUN_MODE_Client,
        MSG_MOVE_SET_WALK_MODE => MSG_MOVE_SET_WALK_MODE_Client,
        MSG_MOVE_FALL_LAND => MSG_MOVE_FALL_LAND_Client,
        MSG_MOVE_START_SWIM => MSG_MOVE_START_SWIM_Client,
        MSG_MOVE_STOP_SWIM => MSG_MOVE_STOP_SWIM_Client,
        MSG_MOVE_SET_FACING => MSG_MOVE_SET_FACING_Client,
        MSG_MOVE_SET_PITCH => MSG_MOVE_SET_PITCH_Client,
        MSG_MOVE_HEARTBEAT => MSG_MOVE_HEARTBEAT_Client,
    }))
}
