use std::io::Cursor;

use anyhow::{ensure, Result};
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
        opcode != crate::codec::auth_session::CMSG_AUTH_SESSION_OPCODE,
        "authentication requires the bounded auth decoder"
    );
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
    if opcode == 0x0264 {
        ensure!(
            (16..=10_240).contains(&body.len()),
            "invalid auction bidder query size"
        );
        let count = u32::from_le_bytes(body[12..16].try_into()?);
        ensure!(
            u64::from(count) * 4 == (body.len() - 16) as u64,
            "invalid auction bidder query count"
        );
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
