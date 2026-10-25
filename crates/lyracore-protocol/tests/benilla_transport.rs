use benilla_protocol::messages::{self, opcode, Object, ServerPacket, TransportPose};
use benilla_protocol::wire::Vector3d;
use lyracore_protocol::codec;
use lyracore_protocol::messages::vanilla::opcodes::ServerOpcodeMessage;

const MOONSPRAY: u64 = (0x1FC0 << 48) | 176_244;

fn created(opcode: u16, body: &[u8]) -> (u64, benilla_protocol::messages::MovementBlock) {
    let (packet, tail) = messages::parse_server_with_tail(opcode, body).unwrap();
    assert_eq!(tail, 0, "unread bytes for {}", packet.name());
    let ServerPacket::UpdateObject { objects } = packet else {
        panic!("expected SMSG_UPDATE_OBJECT");
    };
    match objects.into_iter().next() {
        Some(Object::Create { guid, movement, .. }) => (guid, movement),
        _ => panic!("expected one CREATE"),
    }
}

#[test]
fn benilla_reads_the_transport_path_progress_anchor() {
    let message = codec::build_transport_create_object(&codec::TransportView {
        guid: MOONSPRAY,
        entry: 176_244,
        display_id: 3015,
        size: 1.0,
        flags: 40,
        x: 8533.7,
        y: 1025.1,
        z: 0.0,
        orientation: 2.0,
        path_progress_ms: 4_000_000_000,
    });
    let packet = lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(message),
    ))
    .unwrap();
    let (guid, movement) = created(packet.opcode, &packet.body);
    assert_eq!(guid, MOONSPRAY);
    assert_eq!(movement.transport_progress, Some(4_000_000_000));
}

#[test]
fn benilla_places_a_late_seen_passenger_on_its_transport() {
    let passenger = codec::EntityView {
        guid: 7,
        map_id: 1,
        x: 8530.0,
        y: 1020.0,
        z: 5.0,
        scale_x: 1.0,
        type_mask: 0x19,
        health: 60,
        max_health: 60,
        level: 1,
        run_speed_mult_bp: 10_000,
        faction_template: 1,
        unit_bytes_0: 1 | (1 << 8) | (1 << 24),
        display_id: 49,
        native_display_id: 49,
        ..Default::default()
    };
    let create = codec::build_create_object(&passenger, codec::CreateKind::Peer, &[], &[]).unwrap();
    let movement = codec::passenger_movement_bytes(
        0x1,
        99,
        [8530.0, 1020.0, 5.0, 1.0],
        MOONSPRAY,
        [1.5, -2.0, 6.25, 0.5],
    );
    let body = codec::create_with_movement(&create, &movement).unwrap();
    let (guid, movement) = created(opcode::SMSG_UPDATE_OBJECT, &body);
    assert_eq!(guid, 7);
    assert_eq!(
        movement.transport,
        Some(TransportPose {
            guid: MOONSPRAY,
            pos: Vector3d {
                x: 1.5,
                y: -2.0,
                z: 6.25,
            },
            orientation: 0.5,
        })
    );
    assert_eq!(
        movement.mover.map(|mover| mover.flags),
        Some(0x0200_0001),
        "ON_TRANSPORT with the steering flags"
    );
}
