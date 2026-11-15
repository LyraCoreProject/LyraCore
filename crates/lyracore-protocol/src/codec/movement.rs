use super::movement_info::{movement_info_to_bytes, ClientMovement};
use anyhow::{anyhow, Result};
use lyracore_shared::opcodes::movement as movement_opcodes;
use wow_world_messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};
use wow_world_messages::vanilla::{
    MSG_MOVE_TELEPORT_ACK_Server, Map, MovementInfo, MovementInfo_MovementFlags,
    SMSG_MONSTER_MOVE_MonsterMoveType, SMSG_TRANSFER_PENDING_has_transport, SplineFlag,
    TransferAbortReason, Vector3d, SMSG_FORCE_RUN_BACK_SPEED_CHANGE, SMSG_FORCE_RUN_SPEED_CHANGE,
    SMSG_FORCE_SWIM_SPEED_CHANGE, SMSG_MONSTER_MOVE, SMSG_MOVE_LAND_WALK, SMSG_MOVE_WATER_WALK,
    SMSG_NEW_WORLD, SMSG_SPLINE_SET_RUN_BACK_SPEED, SMSG_SPLINE_SET_RUN_SPEED,
    SMSG_SPLINE_SET_SWIM_SPEED, SMSG_TRANSFER_PENDING,
};
use wow_world_messages::Guid;

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

/// A Passenger's map crossing: its Transport's entry and the map the Transport leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportCrossing {
    pub entry: u32,
    pub from_map_id: u32,
}

/// Queue TRANSFER_PENDING and NEW_WORLD together; with a Transport the pose is the offset on it.
pub fn build_cross_map_teleport(
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
    transport: Option<TransportCrossing>,
) -> Result<[ServerOpcodeMessage; 2]> {
    let map = Map::try_from(map_id).map_err(|_| anyhow!("invalid map {map_id}"))?;
    let has_transport = transport
        .map(|transport| {
            Map::try_from(transport.from_map_id)
                .map(|transport_map| SMSG_TRANSFER_PENDING_has_transport {
                    transport: transport.entry,
                    transport_map,
                })
                .map_err(|_| anyhow!("invalid map {}", transport.from_map_id))
        })
        .transpose()?;
    Ok([
        ServerOpcodeMessage::SMSG_TRANSFER_PENDING(Box::new(SMSG_TRANSFER_PENDING {
            map,
            has_transport,
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
    use crate::codec::movement_info::read_movement_client;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::movement_info::movement_info_to_bytes;
    use lyracore_shared::opcodes::movement as movement_opcodes;
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::{
        MovementInfo, MovementInfo_MovementFlags, SMSG_MONSTER_MOVE_MonsterMoveType, ServerMessage,
        SplineFlag, Vector3d,
    };

    #[test]
    fn a_truncated_movement_block_is_never_framed() {
        let info = MovementInfo {
            flags: MovementInfo_MovementFlags::empty(),
            timestamp: 0x1122_3344,
            position: Vector3d {
                x: -8949.95,
                y: -132.493,
                z: 83.5312,
            },
            orientation: 1.75,
            fall_time: 0.25,
        };
        let stored = movement_info_to_bytes(&info).unwrap();
        assert_eq!(stored.len(), 28, "the minimum MovementInfo body");
        let op = movement_opcodes::MSG_MOVE_HEARTBEAT;
        assert!(
            build_movement_relay_raw(op, 7, &stored).is_some(),
            "a full block relays"
        );
        assert!(
            build_movement_relay_raw(op, 7, &stored[..27]).is_none(),
            "one byte short must not relay"
        );
        assert!(
            build_movement_relay_raw(op, 7, &[]).is_none(),
            "an empty block must not relay"
        );
    }

    #[test]
    fn teleport_ack_carries_target_position_with_no_movement_flags() {
        let msg = build_teleport_ack(7, 99, 1.0, 2.0, 3.0, 0.5);
        assert_eq!(msg.guid.guid(), 7);
        assert_eq!(msg.movement_counter, 99);
        assert_eq!(msg.info.position.x, 1.0);
        assert_eq!(msg.info.position.y, 2.0);
        assert_eq!(msg.info.position.z, 3.0);
        assert_eq!(msg.info.orientation, 0.5);
        assert!(
            msg.info.flags.is_empty(),
            "a teleport arrival is a clean stand-still, no movement flags"
        );
        let mut buf = Vec::new();
        msg.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn cross_map_teleport_sends_transfer_pending_then_new_world_carrying_target_map_and_position() {
        let [pending, new_world] =
            build_cross_map_teleport(36, 10.0, 20.0, 30.0, 1.5, None).unwrap();
        match pending {
            ServerOpcodeMessage::SMSG_TRANSFER_PENDING(p) => {
                assert_eq!(p.map.as_int(), 36);
                assert!(
                    p.has_transport.is_none(),
                    "a teleport that is not a Transport map crossing names no Transport"
                );
                let mut buf = Vec::new();
                p.write_unencrypted_server(&mut buf).unwrap();
                assert!(!buf.is_empty());
            }
            _ => panic!("expected SMSG_TRANSFER_PENDING"),
        }
        match new_world {
            ServerOpcodeMessage::SMSG_NEW_WORLD(w) => {
                assert_eq!(w.map.as_int(), 36);
                assert_eq!(w.position.x, 10.0);
                assert_eq!(w.position.y, 20.0);
                assert_eq!(w.position.z, 30.0);
                assert_eq!(w.orientation, 1.5);
                let mut buf = Vec::new();
                w.write_unencrypted_server(&mut buf).unwrap();
                assert!(!buf.is_empty());
            }
            _ => panic!("expected SMSG_NEW_WORLD"),
        }
    }

    #[test]
    fn cross_map_teleport_rejects_an_unknown_map_id() {
        assert!(build_cross_map_teleport(u32::MAX, 0.0, 0.0, 0.0, 0.0, None).is_err());
    }

    #[test]
    fn force_run_speed_carries_guid_speed_and_zero_move_event() {
        let msg = build_force_run_speed(7, 8.0);
        assert_eq!(msg.guid.guid(), 7);
        assert_eq!(msg.speed, 8.0);
        assert_eq!(msg.move_event, 0);
        let mut buf = Vec::new();
        msg.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn force_swim_speed_serializes_the_vanilla_opcode_and_speed() {
        let msg = build_force_swim_speed(7, 6.0);
        assert_eq!(msg.guid.guid(), 7);
        assert_eq!(msg.move_event, 0);
        let mut bytes = Vec::new();
        msg.write_unencrypted_server(&mut bytes).unwrap();
        assert_eq!(u16::from_le_bytes([bytes[2], bytes[3]]), 0x00e6);
        assert_eq!(&bytes[4..6], &[1, 7]);
        assert_eq!(&bytes[6..10], &0u32.to_le_bytes());
        assert_eq!(&bytes[10..14], &6.0f32.to_le_bytes());
    }

    #[test]
    fn monster_move_run_flag_selects_the_run_spline_bit() {
        let start = Vector3d {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        let dest = Vector3d {
            x: 10.0,
            y: 0.0,
            z: 0.0,
        };
        let walking = build_monster_move(1, start, dest, 1000, 1, false);
        assert_eq!(
            walking.spline_flags,
            SplineFlag::new(0),
            "a walking leg carries no RUN_MODE bit"
        );
        let running = build_monster_move(1, start, dest, 1000, 2, true);
        assert_eq!(
            running.spline_flags,
            SplineFlag::new(0x100),
            "a running leg sets RUN_MODE (0x100)"
        );
        assert_eq!(
            running.splines,
            vec![dest],
            "exactly one destination point (no multi-point spline)"
        );
        assert_eq!(running.spline_id, 2);
        assert_eq!(running.move_type, SMSG_MONSTER_MOVE_MonsterMoveType::Normal);
    }

    #[test]
    fn monster_move_facing_carries_the_angle_and_a_degenerate_single_point_spline() {
        let pos = Vector3d {
            x: 5.0,
            y: -3.0,
            z: 12.0,
        };
        let msg = build_monster_move_facing(9, pos, 1.75, 42);
        assert_eq!(msg.guid.guid(), 9);
        assert_eq!(msg.spline_point, pos);
        assert_eq!(msg.spline_id, 42);
        assert_eq!(
            msg.move_type,
            SMSG_MONSTER_MOVE_MonsterMoveType::FacingAngle { angle: 1.75 }
        );
        assert_eq!(msg.duration, 0, "facing-only — nothing to interpolate");
        assert_eq!(
            msg.splines,
            vec![pos],
            "degenerate one-point spline at the mover's own position"
        );
    }

    #[test]
    fn taxi_move_writes_absolute_catmull_rom_points_byte_for_byte() {
        let start = Vector3d {
            x: -9465.0,
            y: -1300.0,
            z: 50.0,
        };
        let points = vec![
            Vector3d {
                x: -9460.0,
                y: -1298.0,
                z: 52.0,
            },
            Vector3d {
                x: -9450.0,
                y: -1290.0,
                z: 54.0,
            },
            Vector3d {
                x: -9440.0,
                y: -1280.0,
                z: 56.0,
            },
        ];
        let (opcode, bytes) =
            build_taxi_move_raw(0xF00D, start, points.clone(), 12_345, 77).unwrap();
        assert_eq!(opcode, 0x00DD);

        let mut expected = vec![0x03, 0x0D, 0xF0];
        for value in [start.x, start.y, start.z] {
            expected.extend_from_slice(&value.to_le_bytes());
        }
        expected.extend_from_slice(&77u32.to_le_bytes());
        expected.push(0);
        expected.extend_from_slice(&0x300u32.to_le_bytes());
        expected.extend_from_slice(&12_345u32.to_le_bytes());
        expected.extend_from_slice(&3u32.to_le_bytes());
        for point in points {
            for value in [point.x, point.y, point.z] {
                expected.extend_from_slice(&value.to_le_bytes());
            }
        }
        assert_eq!(bytes, expected);
    }
}
