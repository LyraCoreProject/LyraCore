//! Build-5875 MovementInfo bodies. The typed codec uses the later transport flag and layout.

use anyhow::{ensure, Result};
use lyracore_shared::env::{MOVEMENT_FLAG_ON_TRANSPORT, MOVEMENT_FLAG_SWIMMING};
use std::io::Read;
use wow_world_messages::vanilla::{
    MovementInfo, MovementInfo_MovementFlags, MovementInfo_MovementFlags_Jumping,
    MovementInfo_MovementFlags_OnTransport, MovementInfo_MovementFlags_SplineElevation,
    MovementInfo_MovementFlags_Swimming, TransportInfo, Vector3d,
};
use wow_world_messages::Guid;

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
                timestamp: 0, // The library requires this field; vanilla has no transport time.
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
    // Keep the u32 millisecond bits in the library's f32 carrier.
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
        // Replace the library's packed guid and timestamp with vanilla's full guid and pose.
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

/// Replace the initial, tail-free living pose with the Module's current vanilla movement body.
/// CREATE descriptors and speeds still come from the typed builder.
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
        flags & !0x13f == 0,
        "initial CREATE already has conditional movement"
    );
    body.splice(living..living + 28, movement.iter().copied());
    Ok(body)
}

/// Decode relayed movement before the general typed packet reader sees its transport tail.
pub fn read_movement_client(
    opcode: u32,
    body: &[u8],
) -> Result<Option<super::ClientOpcodeMessage>> {
    use super::{movement_opcodes, ClientOpcodeMessage};
    use wow_world_messages::vanilla::*;
    if !movement_opcodes::is_slice_move(opcode) {
        return Ok(None);
    }
    let info = bytes_to_movement_info(body)?;
    Ok(Some(match opcode {
        movement_opcodes::MSG_MOVE_START_FORWARD => {
            ClientOpcodeMessage::MSG_MOVE_START_FORWARD(Box::new(MSG_MOVE_START_FORWARD_Client {
                info,
            }))
        }
        movement_opcodes::MSG_MOVE_START_BACKWARD => {
            ClientOpcodeMessage::MSG_MOVE_START_BACKWARD(Box::new(MSG_MOVE_START_BACKWARD_Client {
                info,
            }))
        }
        movement_opcodes::MSG_MOVE_STOP => {
            ClientOpcodeMessage::MSG_MOVE_STOP(Box::new(MSG_MOVE_STOP_Client { info }))
        }
        movement_opcodes::MSG_MOVE_START_STRAFE_LEFT => {
            ClientOpcodeMessage::MSG_MOVE_START_STRAFE_LEFT(Box::new(
                MSG_MOVE_START_STRAFE_LEFT_Client { info },
            ))
        }
        movement_opcodes::MSG_MOVE_START_STRAFE_RIGHT => {
            ClientOpcodeMessage::MSG_MOVE_START_STRAFE_RIGHT(Box::new(
                MSG_MOVE_START_STRAFE_RIGHT_Client { info },
            ))
        }
        movement_opcodes::MSG_MOVE_STOP_STRAFE => {
            ClientOpcodeMessage::MSG_MOVE_STOP_STRAFE(Box::new(MSG_MOVE_STOP_STRAFE_Client {
                info,
            }))
        }
        movement_opcodes::MSG_MOVE_JUMP => {
            ClientOpcodeMessage::MSG_MOVE_JUMP(Box::new(MSG_MOVE_JUMP_Client { info }))
        }
        movement_opcodes::MSG_MOVE_START_TURN_LEFT => {
            ClientOpcodeMessage::MSG_MOVE_START_TURN_LEFT(Box::new(
                MSG_MOVE_START_TURN_LEFT_Client { info },
            ))
        }
        movement_opcodes::MSG_MOVE_START_TURN_RIGHT => {
            ClientOpcodeMessage::MSG_MOVE_START_TURN_RIGHT(Box::new(
                MSG_MOVE_START_TURN_RIGHT_Client { info },
            ))
        }
        movement_opcodes::MSG_MOVE_STOP_TURN => {
            ClientOpcodeMessage::MSG_MOVE_STOP_TURN(Box::new(MSG_MOVE_STOP_TURN_Client { info }))
        }
        movement_opcodes::MSG_MOVE_SET_RUN_MODE => {
            ClientOpcodeMessage::MSG_MOVE_SET_RUN_MODE(Box::new(MSG_MOVE_SET_RUN_MODE_Client {
                info,
            }))
        }
        movement_opcodes::MSG_MOVE_SET_WALK_MODE => {
            ClientOpcodeMessage::MSG_MOVE_SET_WALK_MODE(Box::new(MSG_MOVE_SET_WALK_MODE_Client {
                info,
            }))
        }
        movement_opcodes::MSG_MOVE_FALL_LAND => {
            ClientOpcodeMessage::MSG_MOVE_FALL_LAND(Box::new(MSG_MOVE_FALL_LAND_Client { info }))
        }
        movement_opcodes::MSG_MOVE_START_SWIM => {
            ClientOpcodeMessage::MSG_MOVE_START_SWIM(Box::new(MSG_MOVE_START_SWIM_Client { info }))
        }
        movement_opcodes::MSG_MOVE_STOP_SWIM => {
            ClientOpcodeMessage::MSG_MOVE_STOP_SWIM(Box::new(MSG_MOVE_STOP_SWIM_Client { info }))
        }
        movement_opcodes::MSG_MOVE_SET_FACING => {
            ClientOpcodeMessage::MSG_MOVE_SET_FACING(Box::new(MSG_MOVE_SET_FACING_Client { info }))
        }
        movement_opcodes::MSG_MOVE_HEARTBEAT => {
            ClientOpcodeMessage::MSG_MOVE_HEARTBEAT(Box::new(MSG_MOVE_HEARTBEAT_Client { info }))
        }
        _ => return Ok(None),
    }))
}
