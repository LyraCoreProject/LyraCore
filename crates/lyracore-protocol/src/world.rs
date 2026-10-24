use std::io::Cursor;

use anyhow::{ensure, Context, Result};
use sha1::{Digest, Sha1};
use subtle::ConstantTimeEq;
use wow_world_messages::vanilla::opcodes::{ClientOpcodeMessage, ServerOpcodeMessage};

#[derive(Debug, PartialEq, Eq)]
pub struct Packet {
    pub opcode: u16,
    pub body: Vec<u8>,
}

pub fn encode(message: ServerOpcodeMessage) -> Result<Packet> {
    let mut frame = Vec::new();
    message.write_unencrypted_server(&mut frame)?;
    if matches!(&message, ServerOpcodeMessage::SMSG_GAMEOBJECT_QUERY_RESPONSE(query) if query.found.is_some())
    {
        // Build 5875 needs 24 data words; wow_world_messages 0.3.0 emits six.
        frame.extend_from_slice(&[0; (24 - 6) * 4]);
    }
    ensure!(frame.len() >= 4, "world response is missing its header");
    Ok(Packet {
        opcode: u16::from_le_bytes([frame[2], frame[3]]),
        body: frame[4..].to_vec(),
    })
}

pub fn client_frame(opcode: u32, body: &[u8]) -> Result<Vec<u8>> {
    let size = u16::try_from(body.len().saturating_add(4))?;
    let mut frame = Vec::with_capacity(body.len() + 6);
    frame.extend_from_slice(&size.to_be_bytes());
    frame.extend_from_slice(&opcode.to_le_bytes());
    frame.extend_from_slice(body);
    Ok(frame)
}

pub fn decode(opcode: u32, body: &[u8]) -> Result<ClientOpcodeMessage> {
    ensure!(
        !matches!(
            opcode,
            crate::codec::auth_session::CMSG_AUTH_SESSION_OPCODE | 0x0205 | 0x020b
        ),
        "compressed requests require a bounded raw handler"
    );
    if let Some(message) = crate::codec::read_movement_client(opcode, body)? {
        return Ok(message);
    }
    let boolean = match opcode {
        0x0253 => body.first(),
        0x028f | 0x023e => body.get(8),
        0x02f3 => body.get(12),
        0x02ee => body.get(16),
        0x0317 => body.get(4),
        _ => None,
    };
    ensure!(
        boolean.is_none_or(|value| *value <= 1),
        "invalid world request boolean"
    );
    if opcode == 0x01ca {
        let mut input = body.get(4..).context("missing bug report kind")?;
        for _ in 0..2 {
            let size = u32::from_le_bytes(
                input
                    .get(..4)
                    .context("missing bug report string size")?
                    .try_into()?,
            ) as usize;
            input = &input[4..];
            ensure!(
                size > 0 && size <= input.len(),
                "invalid bug report string size"
            );
            ensure!(input[size - 1] == 0, "unterminated bug report string");
            input = &input[size..];
        }
        ensure!(input.is_empty(), "trailing bug report bytes");
    }
    // Build 5875 acknowledges teleport with an unpacked GUID, unlike the generated reader.
    if opcode == 0x00c7 {
        ensure!(body.len() == 16, "invalid teleport acknowledgement size");
        return Ok(ClientOpcodeMessage::MSG_MOVE_TELEPORT_ACK(Box::new(
            wow_world_messages::vanilla::MSG_MOVE_TELEPORT_ACK_Client {
                guid: wow_world_messages::Guid::new(u64::from_le_bytes(body[..8].try_into()?)),
                movement_counter: u32::from_le_bytes(body[8..12].try_into()?),
                time: std::time::Duration::from_millis(
                    u32::from_le_bytes(body[12..16].try_into()?).into(),
                ),
            },
        )));
    }
    if matches!(opcode, 0x0264 | 0x0312) {
        ensure!(
            (16..=10_240).contains(&body.len()),
            "invalid counted world request size"
        );
        let count = u32::from_le_bytes(body[12..16].try_into()?);
        ensure!(
            u64::from(count) * 4 == (body.len() - 16) as u64,
            "invalid counted world request count"
        );
    }
    if opcode == 0x0062 {
        bounded_who(body)?;
    }
    let frame = client_frame(opcode, body)?;
    let mut input = Cursor::new(&frame);
    let message = ClientOpcodeMessage::read_unencrypted(&mut input)?;
    ensure!(
        input.position() as usize == frame.len(),
        "trailing world request bytes"
    );
    Ok(message)
}

fn bounded_who(body: &[u8]) -> Result<()> {
    fn string(input: &mut &[u8]) -> Result<()> {
        let end = input
            .iter()
            .position(|byte| *byte == 0)
            .context("unterminated WHO string")?;
        *input = &input[end + 1..];
        Ok(())
    }
    fn count(input: &mut &[u8]) -> Result<usize> {
        let bytes = input.get(..4).context("missing WHO count")?;
        let count = u32::from_le_bytes(bytes.try_into()?) as usize;
        *input = &input[4..];
        Ok(count)
    }
    let mut input = body.get(8..).context("missing WHO level range")?;
    string(&mut input)?;
    string(&mut input)?;
    input = input.get(8..).context("missing WHO race and class masks")?;
    let zones = count(&mut input)?;
    ensure!(
        zones <= input.len().saturating_sub(4) / 4,
        "WHO zones exceed payload"
    );
    input = &input[zones * 4..];
    let strings = count(&mut input)?;
    ensure!(strings <= input.len(), "WHO strings exceed payload");
    for _ in 0..strings {
        string(&mut input)?;
    }
    ensure!(input.is_empty(), "trailing WHO bytes");
    Ok(())
}

pub fn matches_world_proof(
    normalized_username: &str,
    session_key: &[u8; 40],
    server_seed: u32,
    client_seed: u32,
    client_proof: &[u8; 20],
) -> bool {
    let expected: [u8; 20] = Sha1::new()
        .chain_update(normalized_username.as_bytes())
        .chain_update(0u32.to_le_bytes())
        .chain_update(client_seed.to_le_bytes())
        .chain_update(server_seed.to_le_bytes())
        .chain_update(session_key)
        .finalize()
        .into();
    bool::from(expected.ct_eq(client_proof))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::SMSG_PONG;

    #[test]
    fn malformed_boolean_and_sized_string_requests_return_errors_without_panicking() {
        let mut cases = Vec::new();
        for (opcode, offset) in [
            (0x0253, 0),
            (0x028f, 8),
            (0x02f3, 12),
            (0x02ee, 16),
            (0x0317, 4),
            (0x023e, 8),
        ] {
            let mut body = vec![0; offset + 1];
            body[offset] = 2;
            cases.push((opcode, body));
        }
        let mut bug = vec![0; 4];
        bug.extend_from_slice(&1u32.to_le_bytes());
        bug.push(0);
        bug.extend_from_slice(&1u32.to_le_bytes());
        bug.push(0);
        assert!(matches!(
            decode(0x01ca, &bug).unwrap(),
            ClientOpcodeMessage::CMSG_BUG(_)
        ));
        for offset in [4, 9] {
            for size in [0, u32::MAX] {
                let mut invalid = bug.clone();
                invalid[offset..offset + 4].copy_from_slice(&size.to_le_bytes());
                cases.push((0x01ca, invalid));
            }
            let mut invalid = bug.clone();
            invalid[offset + 4] = 2;
            cases.push((0x01ca, invalid));
        }
        let mut ticket = vec![0; 19];
        ticket[0] = 2;
        ticket.extend_from_slice(&u32::MAX.to_le_bytes());
        ticket.extend_from_slice(&[0xff; 16]);
        cases.push((0x0205, ticket));
        for (opcode, body) in cases {
            assert!(
                std::panic::catch_unwind(|| decode(opcode, &body))
                    .expect("untrusted request must not panic")
                    .is_err(),
                "{opcode:#x}"
            );
        }
    }

    #[test]
    fn who_counts_and_strings_must_fit_the_payload_before_allocation() {
        let empty = vec![0; 26];
        assert!(decode(0x0062, &empty).is_ok());
        for offset in [18, 22] {
            let mut invalid = empty.clone();
            invalid[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(decode(0x0062, &invalid).is_err());
            invalid[offset..offset + 4].copy_from_slice(&1u32.to_le_bytes());
            assert!(decode(0x0062, &invalid).is_err());
        }
        let mut query = empty[..18].to_vec();
        query.extend_from_slice(&1u32.to_le_bytes());
        query.extend_from_slice(&12u32.to_le_bytes());
        query.extend_from_slice(&2u32.to_le_bytes());
        query.extend_from_slice(b"Elwynn\0Human\0");
        assert!(matches!(
            decode(0x0062, &query).unwrap(),
            ClientOpcodeMessage::CMSG_WHO(_)
        ));
        query.pop();
        assert!(decode(0x0062, &query).is_err());
        for length in 0..26 {
            assert!(decode(0x0062, &empty[..length]).is_err());
        }
    }

    #[test]
    fn taxi_node_count_must_fit_the_packet_even_before_world_entry() {
        let mut body = vec![0; 16];
        assert!(decode(0x0312, &body).is_ok());
        body[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(0x0312, &body).is_err());
        body[12..16].copy_from_slice(&1u32.to_le_bytes());
        assert!(decode(0x0312, &body).is_err());
        body.extend_from_slice(&41u32.to_le_bytes());
        let ClientOpcodeMessage::CMSG_ACTIVATETAXIEXPRESS(query) = decode(0x0312, &body).unwrap()
        else {
            panic!("expected taxi query");
        };
        assert_eq!(query.nodes, [41]);
    }

    #[test]
    fn bidder_query_count_must_fit_the_bounded_packet_before_allocation() {
        let mut body = vec![0; 16];
        assert!(decode(0x0264, &body).is_ok());
        body[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(0x0264, &body).is_err());
        body[12..16].copy_from_slice(&1u32.to_le_bytes());
        assert!(decode(0x0264, &body).is_err());
        body.extend_from_slice(&41u32.to_le_bytes());
        let ClientOpcodeMessage::CMSG_AUCTION_LIST_BIDDER_ITEMS(query) =
            decode(0x0264, &body).unwrap()
        else {
            panic!("expected bidder query");
        };
        assert_eq!(query.outbid_item_ids, [41]);
        body.push(0);
        assert!(decode(0x0264, &body).is_err());
        assert!(decode(0x0264, &[0; 15]).is_err());
        let mut oversized = vec![0; 10_244];
        oversized[12..16].copy_from_slice(&2557u32.to_le_bytes());
        assert!(decode(0x0264, &oversized).is_err());
    }

    #[test]
    fn a_world_proof_from_wow_srp_binds_the_name_key_and_both_seeds() {
        let name = wow_srp::normalized_string::NormalizedString::new("ACCOUNT").unwrap();
        let seed = wow_srp::vanilla_header::ProofSeed::new();
        let client_seed = seed.seed();
        let key = [7; 40];
        let (proof, _) = seed.into_client_header_crypto(&name, key, 42);
        assert!(matches_world_proof(
            "ACCOUNT",
            &key,
            42,
            client_seed,
            &proof
        ));
        assert!(!matches_world_proof("OTHER", &key, 42, client_seed, &proof));
        assert!(!matches_world_proof(
            "ACCOUNT",
            &[8; 40],
            42,
            client_seed,
            &proof
        ));
        assert!(!matches_world_proof(
            "ACCOUNT",
            &key,
            43,
            client_seed,
            &proof
        ));
        assert!(!matches_world_proof(
            "ACCOUNT",
            &key,
            42,
            client_seed.wrapping_add(1),
            &proof
        ));
    }

    #[test]
    fn gameobject_query_carries_all_24_build_5875_data_words() {
        let view = crate::codec::GameObjectTemplateView {
            type_id: 3,
            display_id: 259,
            name: "Chest".into(),
            data0: 0x0102_0304,
            data1: 52,
            data2: 1,
        };
        let packet = encode(ServerOpcodeMessage::SMSG_GAMEOBJECT_QUERY_RESPONSE(
            Box::new(crate::codec::build_gameobject_query_response(
                50100,
                Some(&view),
            )),
        ))
        .unwrap();
        assert_eq!(packet.opcode, 0x005f);
        assert_eq!(
            &packet.body[..22],
            b"\xb4\xc3\0\0\x03\0\0\0\x03\x01\0\0Chest\0\0\0\0\0"
        );
        let words = packet.body[22..]
            .chunks_exact(4)
            .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(packet.body.len(), 118);
        assert_eq!(
            words,
            [
                0x0102_0304,
                52,
                1,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0
            ]
        );
    }

    #[test]
    fn missing_gameobject_query_contains_only_the_flagged_entry() {
        let packet = encode(ServerOpcodeMessage::SMSG_GAMEOBJECT_QUERY_RESPONSE(
            Box::new(crate::codec::build_gameobject_query_response(50100, None)),
        ))
        .unwrap();
        assert_eq!(packet.opcode, 0x005f);
        assert_eq!(packet.body, [0xb4, 0xc3, 0, 0x80]);
    }

    #[test]
    fn encoding_removes_only_the_transport_header() {
        let packet = encode(ServerOpcodeMessage::SMSG_PONG(SMSG_PONG {
            sequence_id: 42,
        }))
        .unwrap();
        assert_eq!(
            packet,
            Packet {
                opcode: 0x01dd,
                body: 42u32.to_le_bytes().to_vec()
            }
        );
    }

    #[test]
    fn teleport_ack_uses_the_build_5875_unpacked_guid_layout() {
        let body = [0x56, 0x34, 0x12, 0, 0, 0, 0, 0, 7, 0, 0, 0, 0xe8, 3, 0, 0];
        let ClientOpcodeMessage::MSG_MOVE_TELEPORT_ACK(ack) = decode(0xc7, &body).unwrap() else {
            panic!("expected teleport acknowledgement");
        };
        assert_eq!(ack.guid.guid(), 0x123456);
        assert_eq!(ack.movement_counter, 7);
        assert_eq!(ack.time.as_millis(), 1000);
        for size in 0..16 {
            assert!(decode(0xc7, &body[..size]).is_err());
        }
        assert!(decode(0xc7, &[0; 17]).is_err());
    }

    #[test]
    fn malformed_and_oversized_requests_do_not_reach_the_typed_decoder() {
        assert!(decode(0x01ed, &[0; 32]).is_err());
        assert!(decode(0x01dc, &[0; 7]).is_err());
        assert!(client_frame(0x01dc, &vec![0; u16::MAX as usize]).is_err());
    }
}
