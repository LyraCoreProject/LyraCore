use super::*;
use wow_world_messages::vanilla::{
    MSG_MOVE_SET_PITCH_Server, MSG_MOVE_START_PITCH_DOWN_Server, MSG_MOVE_START_PITCH_UP_Server,
    MSG_MOVE_STOP_PITCH_Server, SMSG_FORCE_RUN_BACK_SPEED_CHANGE, SMSG_FORCE_SWIM_SPEED_CHANGE,
    SMSG_MOVE_LAND_WALK, SMSG_MOVE_WATER_WALK, SMSG_SPLINE_SET_RUN_BACK_SPEED,
    SMSG_SPLINE_SET_RUN_SPEED, SMSG_SPLINE_SET_SWIM_SPEED,
};

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
        ClientOpcodeMessage::MSG_MOVE_START_PITCH_UP(_) => m::MSG_MOVE_START_PITCH_UP,
        ClientOpcodeMessage::MSG_MOVE_START_PITCH_DOWN(_) => m::MSG_MOVE_START_PITCH_DOWN,
        ClientOpcodeMessage::MSG_MOVE_STOP_PITCH(_) => m::MSG_MOVE_STOP_PITCH,
        ClientOpcodeMessage::MSG_MOVE_SET_RUN_MODE(_) => m::MSG_MOVE_SET_RUN_MODE,
        ClientOpcodeMessage::MSG_MOVE_SET_WALK_MODE(_) => m::MSG_MOVE_SET_WALK_MODE,
        ClientOpcodeMessage::MSG_MOVE_FALL_LAND(_) => m::MSG_MOVE_FALL_LAND,
        ClientOpcodeMessage::MSG_MOVE_START_SWIM(_) => m::MSG_MOVE_START_SWIM,
        ClientOpcodeMessage::MSG_MOVE_STOP_SWIM(_) => m::MSG_MOVE_STOP_SWIM,
        ClientOpcodeMessage::MSG_MOVE_SET_FACING(_) => m::MSG_MOVE_SET_FACING,
        ClientOpcodeMessage::MSG_MOVE_SET_PITCH(_) => m::MSG_MOVE_SET_PITCH,
        ClientOpcodeMessage::MSG_MOVE_HEARTBEAT(_) => m::MSG_MOVE_HEARTBEAT,
        _ => return None,
    })
}

/// Observer relay for an acknowledgement; see docs/benilla-compatibility.md#root-receipts.
pub fn acknowledged_move(msg: &ClientOpcodeMessage) -> Option<(u64, u32, ClientMovement)> {
    use lyracore_shared::movement_flags::STEERING;
    use movement_opcodes as m;
    let (guid, opcode, info) = match msg {
        ClientOpcodeMessage::CMSG_FORCE_MOVE_ROOT_ACK(ack) => {
            (ack.guid, m::MSG_MOVE_ROOT, &ack.info)
        }
        ClientOpcodeMessage::CMSG_FORCE_MOVE_UNROOT_ACK(ack) => {
            (ack.guid, m::MSG_MOVE_UNROOT, &ack.info)
        }
        ClientOpcodeMessage::CMSG_MOVE_WATER_WALK_ACK(ack) => {
            (ack.guid, m::MSG_MOVE_WATER_WALK, &ack.info)
        }
        _ => return None,
    };
    // The library clears flags with reverse_bits, so clear steering in the encoded bytes.
    let mut bytes = movement_info_to_bytes(info).ok()?;
    if opcode == m::MSG_MOVE_ROOT {
        let flags = u32::from_le_bytes(bytes[..4].try_into().ok()?) & !STEERING;
        bytes[..4].copy_from_slice(&flags.to_le_bytes());
    }
    Some((guid.guid(), opcode, ClientMovement::decode(&bytes).ok()?))
}

/// The typed transport layout belongs to later clients; use the build-5875 codec.
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
        m::MSG_MOVE_START_PITCH_UP => {
            ServerOpcodeMessage::MSG_MOVE_START_PITCH_UP(Box::new(MSG_MOVE_START_PITCH_UP_Server {
                guid,
                info,
            }))
        }
        m::MSG_MOVE_START_PITCH_DOWN => ServerOpcodeMessage::MSG_MOVE_START_PITCH_DOWN(Box::new(
            MSG_MOVE_START_PITCH_DOWN_Server { guid, info },
        )),
        m::MSG_MOVE_STOP_PITCH => {
            ServerOpcodeMessage::MSG_MOVE_STOP_PITCH(Box::new(MSG_MOVE_STOP_PITCH_Server {
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
        m::MSG_MOVE_SET_PITCH => {
            ServerOpcodeMessage::MSG_MOVE_SET_PITCH(Box::new(MSG_MOVE_SET_PITCH_Server {
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

// Fixed MovementInfo fields require 28 bytes before any optional tails.
const MIN_MOVEMENT_INFO_LEN: usize = 4 + 4 + 12 + 4 + 4;

pub fn build_movement_relay_raw(
    opcode: u32,
    mover_guid: u64,
    info_bytes: &[u8],
) -> Option<(u16, Vec<u8>)> {
    if !movement_opcodes::is_relayed_move(opcode) || info_bytes.len() < MIN_MOVEMENT_INFO_LEN {
        return None;
    }
    // 9 = the largest packed guid, including its mask byte.
    let mut body = Vec::with_capacity(9 + info_bytes.len());
    super::values::write_packed_guid_u64(&mut body, mover_guid);
    body.extend_from_slice(info_bytes);
    Some((opcode as u16, body))
}

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

/// Queue TRANSFER_PENDING and NEW_WORLD together to keep loading and arrival ordered.
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

/// Build-5875 aborts carry only a reason byte; the typed writer adds later fields.
pub fn build_transfer_aborted(reason: TransferAbortReason) -> (u16, Vec<u8>) {
    (0x0040, vec![reason.as_int()])
}

pub fn build_force_run_speed(guid: u64, speed: f32) -> SMSG_FORCE_RUN_SPEED_CHANGE {
    SMSG_FORCE_RUN_SPEED_CHANGE {
        guid: Guid::new(guid),
        move_event: 0,
        speed,
    }
}

pub fn build_force_swim_speed(guid: u64, speed: f32) -> SMSG_FORCE_SWIM_SPEED_CHANGE {
    SMSG_FORCE_SWIM_SPEED_CHANGE {
        guid: Guid::new(guid),
        move_event: 0,
        speed,
    }
}

pub fn build_force_run_back_speed(guid: u64, speed: f32) -> SMSG_FORCE_RUN_BACK_SPEED_CHANGE {
    SMSG_FORCE_RUN_BACK_SPEED_CHANGE {
        guid: Guid::new(guid),
        move_event: 0,
        speed,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservedSpeed {
    Run,
    RunBack,
    Swim,
}

/// Observers get this form for players too; see docs/benilla-compatibility.md#movement-controls.
pub fn build_spline_speed(guid: u64, kind: ObservedSpeed, speed: f32) -> ServerOpcodeMessage {
    let guid = Guid::new(guid);
    match kind {
        ObservedSpeed::Run => {
            ServerOpcodeMessage::SMSG_SPLINE_SET_RUN_SPEED(Box::new(SMSG_SPLINE_SET_RUN_SPEED {
                guid,
                speed,
            }))
        }
        ObservedSpeed::RunBack => ServerOpcodeMessage::SMSG_SPLINE_SET_RUN_BACK_SPEED(Box::new(
            SMSG_SPLINE_SET_RUN_BACK_SPEED { guid, speed },
        )),
        ObservedSpeed::Swim => {
            ServerOpcodeMessage::SMSG_SPLINE_SET_SWIM_SPEED(Box::new(SMSG_SPLINE_SET_SWIM_SPEED {
                guid,
                speed,
            }))
        }
    }
}

/// 0x00E8/0x00EA, packed guid and counter; the client misreads the library's full guid.
pub fn build_force_move_root_raw(guid: u64, rooted: bool) -> (u16, Vec<u8>) {
    let mut body = Vec::with_capacity(13);
    super::values::write_packed_guid_u64(&mut body, guid);
    body.extend_from_slice(&0u32.to_le_bytes());
    (if rooted { 0x00E8 } else { 0x00EA }, body)
}

/// 0x031A or 0x0304: packed guid only. The library writes a full guid for the root form.
pub fn build_spline_move_root_raw(guid: u64, rooted: bool) -> (u16, Vec<u8>) {
    let mut body = Vec::with_capacity(9);
    super::values::write_packed_guid_u64(&mut body, guid);
    (if rooted { 0x031A } else { 0x0304 }, body)
}

pub fn build_move_water_walk(guid: u64, water_walk: bool) -> ServerOpcodeMessage {
    let guid = Guid::new(guid);
    if water_walk {
        ServerOpcodeMessage::SMSG_MOVE_WATER_WALK(Box::new(SMSG_MOVE_WATER_WALK {
            guid,
            counter: 0,
        }))
    } else {
        ServerOpcodeMessage::SMSG_MOVE_LAND_WALK(Box::new(SMSG_MOVE_LAND_WALK { guid, counter: 0 }))
    }
}

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
        spline_flags: SplineFlag::new(if run { 0x100 } else { 0 }),
        duration: duration_ms,
        splines: vec![dest],
    }
}

// Linear paths send the destination first, then signed destination-relative points.
pub fn build_ground_path_raw(
    mover_guid: u64,
    start: Vector3d,
    points: &[(f32, f32, f32)],
    duration_ms: u32,
    spline_id: u32,
    run: bool,
) -> Option<(u16, Vec<u8>)> {
    use lyracore_shared::movement_path;
    let &destination = points.last()?;
    if points.len() > movement_path::MAX_POINTS
        || [start.x, start.y, start.z].iter().any(|v| !v.is_finite())
        || points
            .iter()
            .any(|p| !p.0.is_finite() || !p.1.is_finite() || !p.2.is_finite())
    {
        return None;
    }
    let mut body = Vec::with_capacity(46 + points.len() * 4);
    super::values::write_packed_guid_u64(&mut body, mover_guid);
    for value in [start.x, start.y, start.z] {
        body.extend_from_slice(&value.to_le_bytes());
    }
    body.extend_from_slice(&spline_id.to_le_bytes());
    body.push(0);
    body.extend_from_slice(&(if run { 0x100u32 } else { 0 }).to_le_bytes());
    body.extend_from_slice(&duration_ms.to_le_bytes());
    body.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for value in [destination.0, destination.1, destination.2] {
        body.extend_from_slice(&value.to_le_bytes());
    }
    for &point in &points[..points.len() - 1] {
        if movement_path::distance(destination, point).powi(2) < 0.5 {
            return None;
        }
        body.extend_from_slice(&movement_path::packed_offset(destination, point)?.to_le_bytes());
    }
    Some((0x00DD, body))
}

/// Taxi Catmull-Rom paths need absolute points; gtker always applies linear PackXYZ.
pub fn build_taxi_move_raw(
    passenger_guid: u64,
    start: Vector3d,
    points: Vec<Vector3d>,
    duration_ms: u32,
    spline_id: u32,
) -> Option<(u16, Vec<u8>)> {
    if points.is_empty()
        || points
            .iter()
            .chain([&start])
            .any(|p| !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite())
    {
        return None;
    }
    let mut body = Vec::with_capacity(34 + points.len() * 12);
    super::values::write_packed_guid_u64(&mut body, passenger_guid);
    for value in [start.x, start.y, start.z] {
        body.extend_from_slice(&value.to_le_bytes());
    }
    body.extend_from_slice(&spline_id.to_le_bytes());
    body.push(0);
    body.extend_from_slice(&0x0000_0300u32.to_le_bytes());
    body.extend_from_slice(&duration_ms.to_le_bytes());
    body.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for point in points {
        for value in [point.x, point.y, point.z] {
            body.extend_from_slice(&value.to_le_bytes());
        }
    }
    Some((0x00DD, body))
}

pub fn build_monster_move_facing(
    mover_guid: u64,
    pos: Vector3d,
    angle_rad: f32,
    spline_id: u32,
) -> SMSG_MONSTER_MOVE {
    SMSG_MONSTER_MOVE {
        guid: Guid::new(mover_guid),
        spline_point: pos,
        spline_id,
        move_type: SMSG_MONSTER_MOVE_MonsterMoveType::FacingAngle { angle: angle_rad },
        spline_flags: SplineFlag::new(0),
        duration: 0,
        splines: vec![pos],
    }
}

#[cfg(test)]
mod transfer_abort_tests {
    use super::*;

    #[test]
    fn vanilla_transfer_abort_contains_only_the_reason() {
        assert_eq!(
            build_transfer_aborted(TransferAbortReason::NotFound),
            (0x0040, vec![0x02])
        );
    }
}

#[cfg(test)]
mod ground_path_tests {
    use super::*;

    #[test]
    fn vanilla_ground_path_carries_destination_relative_signed_offsets() {
        let (opcode, body) = build_ground_path_raw(
            1,
            Vector3d {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            &[(9.0, 21.0, 29.5), (10.0, 20.0, 30.0)],
            2_000,
            7,
            true,
        )
        .unwrap();
        assert_eq!(opcode, 0xdd);
        assert_eq!(
            body,
            vec![
                1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0, 0, 1, 0, 0, 0xd0, 7, 0, 0,
                2, 0, 0, 0, 0, 0, 0x20, 0x41, 0, 0, 0xa0, 0x41, 0, 0, 0xf0, 0x41, 4, 0xe0, 0xbf, 0,
            ]
        );
    }

    #[test]
    fn ground_path_refuses_unrepresentable_intermediate_points() {
        let start = Vector3d {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        for points in [
            [(300.0, 0.0, 0.0), (0.0, 0.0, 0.0)],
            [(0.25, 0.0, 0.0), (0.0, 0.0, 0.0)],
            [(f32::NAN, 0.0, 0.0), (0.0, 0.0, 0.0)],
        ] {
            assert!(build_ground_path_raw(1, start, &points, 1000, 1, true).is_none());
        }
    }
}

#[cfg(test)]
mod control_tests {
    use super::*;

    /// The plaintext frame a client receives: big-endian size of opcode and body, then the opcode.
    fn framed(opcode: u16, body: &[u8]) -> Vec<u8> {
        let mut frame = ((2 + body.len()) as u16).to_be_bytes().to_vec();
        frame.extend_from_slice(&opcode.to_le_bytes());
        frame.extend_from_slice(body);
        frame
    }

    fn typed(message: ServerOpcodeMessage) -> Vec<u8> {
        let mut frame = Vec::new();
        message.write_unencrypted_server(&mut frame).unwrap();
        frame
    }

    // Packed guid (mask 0x03, then 0x02 0x01) and a zero counter.
    #[test]
    fn forced_root_and_unroot_carry_a_packed_guid_and_the_counter() {
        let body = [0x03, 0x02, 0x01, 0, 0, 0, 0];
        let (opcode, raw) = build_force_move_root_raw(0x0102, true);
        assert_eq!(
            framed(opcode, &raw),
            [0x00, 0x09, 0xE8, 0x00, 0x03, 0x02, 0x01, 0, 0, 0, 0]
        );
        let (opcode, raw) = build_force_move_root_raw(0x0102, false);
        assert_eq!((opcode, raw.as_slice()), (0x00EA, body.as_slice()));
    }

    // A creature guid packs to mask 0xC1 and its three nonzero bytes.
    #[test]
    fn spline_root_and_unroot_carry_only_the_packed_guid() {
        let (opcode, raw) = build_spline_move_root_raw(0xF130_0000_0000_0042, true);
        assert_eq!(
            framed(opcode, &raw),
            [0x00, 0x06, 0x1A, 0x03, 0xC1, 0x42, 0x30, 0xF1]
        );
        let (opcode, raw) = build_spline_move_root_raw(0xF130_0000_0000_0042, false);
        assert_eq!(
            framed(opcode, &raw),
            [0x00, 0x06, 0x04, 0x03, 0xC1, 0x42, 0x30, 0xF1]
        );
    }

    #[test]
    fn water_walk_and_land_walk_carry_a_packed_guid_and_the_counter() {
        assert_eq!(
            typed(build_move_water_walk(7, true)),
            [0x00, 0x08, 0xDE, 0x00, 0x01, 0x07, 0, 0, 0, 0]
        );
        assert_eq!(
            typed(build_move_water_walk(7, false)),
            [0x00, 0x08, 0xDF, 0x00, 0x01, 0x07, 0, 0, 0, 0]
        );
    }

    // 2.25 is 0x40100000 and 3.5 is 0x40600000.
    #[test]
    fn speed_changes_carry_the_speed_after_the_packed_guid() {
        assert_eq!(
            typed(ServerOpcodeMessage::SMSG_FORCE_RUN_BACK_SPEED_CHANGE(
                Box::new(build_force_run_back_speed(7, 2.25))
            )),
            [0x00, 0x0C, 0xE4, 0x00, 0x01, 0x07, 0, 0, 0, 0, 0x00, 0x00, 0x10, 0x40]
        );
        for (kind, opcode) in [
            (ObservedSpeed::Run, [0xFE, 0x02]),
            (ObservedSpeed::RunBack, [0xFF, 0x02]),
            (ObservedSpeed::Swim, [0x00, 0x03]),
        ] {
            assert_eq!(
                typed(build_spline_speed(7, kind, 3.5)),
                [0x00, 0x08, opcode[0], opcode[1], 0x01, 0x07, 0x00, 0x00, 0x60, 0x40]
            );
        }
    }

    /// A pose on a transport, in the build-5875 layout the library cannot read.
    fn transport_pose() -> Vec<u8> {
        let mut pose = 0x0200_0000u32.to_le_bytes().to_vec();
        pose.extend_from_slice(&5u32.to_le_bytes());
        pose.extend_from_slice(&[0; 16]);
        pose.extend_from_slice(&0xF120_0000_0000_0012u64.to_le_bytes());
        pose.extend_from_slice(&[0; 16]);
        pose.extend_from_slice(&0u32.to_le_bytes());
        pose
    }

    #[test]
    fn acknowledgements_read_the_build_5875_pose() {
        let mut root = 9u64.to_le_bytes().to_vec();
        root.extend_from_slice(&3u32.to_le_bytes());
        root.extend_from_slice(&transport_pose());
        let message = read_movement_client(0x00E9, &root).unwrap().unwrap();
        let (mover, opcode, movement) = acknowledged_move(&message).unwrap();
        assert_eq!((mover, opcode), (9, movement_opcodes::MSG_MOVE_ROOT));
        assert_eq!(
            movement_info_to_bytes(movement.info()).unwrap(),
            transport_pose()
        );

        let mut run_back = root.clone();
        run_back.extend_from_slice(&2.25f32.to_le_bytes());
        match read_movement_client(0x00E5, &run_back).unwrap().unwrap() {
            ClientOpcodeMessage::CMSG_FORCE_RUN_BACK_SPEED_CHANGE_ACK(ack) => {
                assert_eq!((ack.guid.guid(), ack.movement_counter), (9, 3));
                assert_eq!(ack.new_speed, 2.25);
            }
            other => panic!("expected the run-back acknowledgement, got {other}"),
        }

        let mut water_walk = root.clone();
        water_walk.extend_from_slice(&1u32.to_le_bytes());
        let message = read_movement_client(0x02D0, &water_walk).unwrap().unwrap();
        assert_eq!(
            acknowledged_move(&message).map(|(_, opcode, _)| opcode),
            Some(movement_opcodes::MSG_MOVE_WATER_WALK)
        );
    }

    #[test]
    fn a_truncated_or_padded_acknowledgement_is_refused() {
        let mut root = 9u64.to_le_bytes().to_vec();
        root.extend_from_slice(&3u32.to_le_bytes());
        root.extend_from_slice(&transport_pose());
        for end in 0..root.len() {
            assert!(read_movement_client(0x00E9, &root[..end]).is_err());
        }
        root.push(0);
        assert!(read_movement_client(0x00E9, &root).is_err());
    }
}
