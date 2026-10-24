use benilla_protocol::messages::{self, opcode, ServerPacket};
use lyracore_protocol::codec;

fn decode(opcode: u16, body: &[u8]) -> ServerPacket {
    let (packet, tail) = messages::parse_server_with_tail(opcode, body).unwrap();
    assert_eq!(tail, 0, "unread bytes for {}", packet.name());
    packet
}

#[test]
fn benilla_transport_movement_survives_decode_and_relay() {
    for flags in [0x0200_0000, 0x0220_2001, 0x200] {
        for guid in [0, 1, 0x0100_0000_0000_0000, u64::MAX] {
            let mut info = movement_info(flags);
            if let Some(transport) = &mut info.transport {
                transport.guid = guid;
            }
            assert_movement_relay(info);
        }
    }
}

#[test]
fn benilla_movement_refuses_truncated_and_trailing_bytes() {
    let body = messages::movement(&movement_info(0x0220_2001));
    for end in 0..body.len() {
        assert!(crate::codec::read_movement_client(
            u32::from(opcode::MSG_MOVE_HEARTBEAT),
            &body[..end]
        )
        .is_err());
    }
    let mut trailing = body.clone();
    trailing.push(0);
    assert!(
        crate::codec::read_movement_client(u32::from(opcode::MSG_MOVE_HEARTBEAT), &trailing)
            .is_err()
    );
    assert_eq!(
        lyracore_shared::env::fall_time_from_movement_info(&body),
        None
    );
}

fn assert_movement_relay(info: messages::MovementInfo) {
    use crate::codec;
    let sent = messages::movement(&info);
    for &opcode in lyracore_shared::opcodes::movement::SLICE_MOVE_OPCODES {
        let decoded = codec::read_movement_client(opcode, &sent).unwrap().unwrap();
        let stored = codec::movement_info_to_bytes(decoded.movement_info().unwrap()).unwrap();
        assert_eq!(stored, sent, "movement flags {:#010x}", info.flags);
        let (opcode, body) =
            codec::build_movement_relay_raw(opcode, 0x0102_0000_0000_0304, &stored).unwrap();
        assert_movement_packet(decode(opcode, &body), opcode, 0x0102_0000_0000_0304, &info);
    }
}

fn movement_info(flags: u32) -> messages::MovementInfo {
    use benilla_protocol::messages::{JumpInfo, MovementInfo, TransportPose};
    use benilla_protocol::wire::Vector3d;
    MovementInfo {
        flags,
        timestamp: 123_456,
        position: Vector3d {
            x: -100.5,
            y: 20.25,
            z: 83.5,
        },
        orientation: 1.5,
        transport: (flags & 0x0200_0000 != 0).then_some(TransportPose {
            guid: 0xf120_0000_0000_0012,
            pos: Vector3d {
                x: 1.0,
                y: -2.5,
                z: 3.25,
            },
            orientation: 0.5,
        }),
        pitch: if flags & 0x20_0000 != 0 { 0.25 } else { 0.0 },
        fall_time: 2345,
        jump: (flags & 0x2000 != 0).then_some(JumpInfo {
            zspeed: -7.5,
            cos_angle: 0.6,
            sin_angle: 0.8,
            xy_speed: 7.0,
        }),
    }
}

fn assert_movement_packet(
    packet: ServerPacket,
    expected_opcode: u16,
    expected_guid: u64,
    info: &messages::MovementInfo,
) {
    let ServerPacket::PlayerMove {
        guid,
        opcode,
        flags: actual_flags,
        position: actual_pos,
        orientation,
        pitch,
        time,
        fall_time,
        jump,
        transport,
        ..
    } = packet
    else {
        panic!("expected a movement relay");
    };
    assert_eq!(guid, expected_guid);
    assert_eq!(opcode, expected_opcode);
    assert_eq!(actual_flags, info.flags);
    assert_eq!(actual_pos, info.position);
    assert_eq!(orientation, info.orientation);
    assert_eq!(time, info.timestamp);
    assert_eq!(fall_time, info.fall_time);
    assert_eq!(pitch, info.pitch);
    assert_eq!(jump, info.jump);
    assert_eq!(transport, info.transport);
}
