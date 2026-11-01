#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented
    )
)]

use std::io::{self, Read};

use flate2::{Decompress, FlushDecompress, Status};
use wow_world_messages::vanilla::{
    Addon, AddonInfo, AddonType, Addon_InfoBlock, Addon_KeyVersion, Addon_UrlInfo, SMSG_ADDON_INFO,
};

pub const CMSG_AUTH_SESSION_OPCODE: u32 = 0x01ED;
const MAX_ADDON_BYTES: usize = 0xFFFFF;
// 248 entries of 264 bytes each fit in one 16-bit world reply, including its opcode.
const MAX_ADDONS: usize = 248;
const BLIZZARD_ADDON_CRC: u32 = 0x4C1C776D;
// Build-5875 addon modulus; see docs/benilla-compatibility.md for the wire contract and source.
const ADDON_PUBLIC_KEY: [u8; 256] = [
    0xC3, 0x5B, 0x50, 0x84, 0xB9, 0x3E, 0x32, 0x42, 0x8C, 0xD0, 0xC7, 0x48, 0xFA, 0x0E, 0x5D, 0x54,
    0x5A, 0xA3, 0x0E, 0x14, 0xBA, 0x9E, 0x0D, 0xB9, 0x5D, 0x8B, 0xEE, 0xB6, 0x84, 0x93, 0x45, 0x75,
    0xFF, 0x31, 0xFE, 0x2F, 0x64, 0x3F, 0x3D, 0x6D, 0x07, 0xD9, 0x44, 0x9B, 0x40, 0x85, 0x59, 0x34,
    0x4E, 0x10, 0xE1, 0xE7, 0x43, 0x69, 0xEF, 0x7C, 0x16, 0xFC, 0xB4, 0xED, 0x1B, 0x95, 0x28, 0xA8,
    0x23, 0x76, 0x51, 0x31, 0x57, 0x30, 0x2B, 0x79, 0x08, 0x50, 0x10, 0x1C, 0x4A, 0x1A, 0x2C, 0xC8,
    0x8B, 0x8F, 0x05, 0x2D, 0x22, 0x3D, 0xDB, 0x5A, 0x24, 0x7A, 0x0F, 0x13, 0x50, 0x37, 0x8F, 0x5A,
    0xCC, 0x9E, 0x04, 0x44, 0x0E, 0x87, 0x01, 0xD4, 0xA3, 0x15, 0x94, 0x16, 0x34, 0xC6, 0xC2, 0xC3,
    0xFB, 0x49, 0xFE, 0xE1, 0xF9, 0xDA, 0x8C, 0x50, 0x3C, 0xBE, 0x2C, 0xBB, 0x57, 0xED, 0x46, 0xB9,
    0xAD, 0x8B, 0xC6, 0xDF, 0x0E, 0xD6, 0x0F, 0xBE, 0x80, 0xB3, 0x8B, 0x1E, 0x77, 0xCF, 0xAD, 0x22,
    0xCF, 0xB7, 0x4B, 0xCF, 0xFB, 0xF0, 0x6B, 0x11, 0x45, 0x2D, 0x7A, 0x81, 0x18, 0xF2, 0x92, 0x7E,
    0x98, 0x56, 0x5D, 0x5E, 0x69, 0x72, 0x0A, 0x0D, 0x03, 0x0A, 0x85, 0xA2, 0x85, 0x9C, 0xCB, 0xFB,
    0x56, 0x6E, 0x8F, 0x44, 0xBB, 0x8F, 0x02, 0x22, 0x68, 0x63, 0x97, 0xBC, 0x85, 0xBA, 0xA8, 0xF7,
    0xB5, 0x40, 0x68, 0x3C, 0x77, 0x86, 0x6F, 0x4B, 0xD7, 0x88, 0xCA, 0x8A, 0xD7, 0xCE, 0x36, 0xF0,
    0x45, 0x6E, 0xD5, 0x64, 0x79, 0x0F, 0x17, 0xFC, 0x64, 0xDD, 0x10, 0x6F, 0xF3, 0xF5, 0xE0, 0xA6,
    0xC3, 0xFB, 0x1B, 0x8C, 0x29, 0xEF, 0x8E, 0xE5, 0x34, 0xCB, 0xD1, 0x2A, 0xCE, 0x79, 0xC3, 0x9A,
    0x0D, 0x36, 0xEA, 0x01, 0xE0, 0xAA, 0x91, 0x20, 0x54, 0xF0, 0x72, 0xD8, 0x1E, 0xC7, 0x89, 0xD2,
];

#[derive(Debug, PartialEq, Eq)]
pub struct AuthSessionClaim {
    pub username: String,
    pub client_seed: u32,
    pub client_proof: [u8; 20],
    pub addon_info: Vec<AddonInfo>,
}

pub fn read_auth_session<R: Read>(r: &mut R) -> io::Result<AuthSessionClaim> {
    let mut header = [0u8; 6];
    r.read_exact(&mut header)?;
    let size = u16::from_be_bytes([header[0], header[1]]) as usize;
    let opcode = u32::from_le_bytes([header[2], header[3], header[4], header[5]]);
    if opcode != CMSG_AUTH_SESSION_OPCODE {
        return Err(invalid(format!(
            "expected CMSG_AUTH_SESSION, got opcode {opcode:#06x}"
        )));
    }
    // `size` includes the u32 opcode; the u16 header bounds the body.
    let mut body = vec![0u8; size.saturating_sub(4)];
    r.read_exact(&mut body)?;
    parse_auth_session_body(&body)
}

fn parse_auth_session_body(body: &[u8]) -> io::Result<AuthSessionClaim> {
    let rest = body.get(8..).ok_or_else(short)?;
    let nul = rest.iter().position(|b| *b == 0).ok_or_else(short)?;
    let username = String::from_utf8(rest[..nul].to_vec())
        .map_err(|e| invalid(format!("CMSG_AUTH_SESSION username is not UTF-8: {e}")))?;
    let rest = &rest[nul + 1..];
    Ok(AuthSessionClaim {
        username,
        client_seed: u32::from_le_bytes(field(rest, 0)?),
        client_proof: field(rest, 4)?,
        addon_info: parse_addons(rest.get(24..).ok_or_else(short)?)?,
    })
}

fn parse_addons(block: &[u8]) -> io::Result<Vec<AddonInfo>> {
    let size = u32::from_le_bytes(field(block, 0)?) as usize;
    if size > MAX_ADDON_BYTES {
        return Err(invalid(
            "CMSG_AUTH_SESSION addon block exceeds the byte limit",
        ));
    }
    let compressed = &block[4..];
    if size == 0 && compressed.is_empty() {
        return Ok(Vec::new());
    }
    let mut inflated = vec![0; size + 1];
    let mut decoder = Decompress::new(true);
    let status = decoder
        .decompress(compressed, &mut inflated, FlushDecompress::Finish)
        .map_err(|error| invalid(format!("CMSG_AUTH_SESSION invalid addon zlib: {error}")))?;
    if status != Status::StreamEnd
        || decoder.total_out() != size as u64
        || decoder.total_in() != compressed.len() as u64
    {
        return Err(invalid(
            "CMSG_AUTH_SESSION addon zlib size or stream mismatch",
        ));
    }
    let mut rest = &inflated[..size];
    let mut addons = Vec::new();
    while !rest.is_empty() {
        if addons.len() == MAX_ADDONS {
            return Err(invalid("CMSG_AUTH_SESSION exceeds the addon count limit"));
        }
        let nul = rest
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| invalid("CMSG_AUTH_SESSION unterminated addon name"))?;
        let addon_name = String::from_utf8(rest[..nul].to_vec()).map_err(|error| {
            invalid(format!(
                "CMSG_AUTH_SESSION addon name is not UTF-8: {error}"
            ))
        })?;
        let fields = rest
            .get(nul + 1..nul + 10)
            .ok_or_else(|| invalid("CMSG_AUTH_SESSION incomplete addon entry"))?;
        addons.push(AddonInfo {
            addon_name,
            addon_has_signature: fields[0],
            addon_crc: u32::from_le_bytes(field(fields, 1)?),
            addon_extra_crc: u32::from_le_bytes(field(fields, 5)?),
        });
        rest = &rest[nul + 10..];
    }
    Ok(addons)
}

pub fn build_addon_info(addons: &[AddonInfo]) -> SMSG_ADDON_INFO {
    SMSG_ADDON_INFO {
        addons: addons
            .iter()
            .map(|addon| {
                let blizzard = addon.addon_name.contains("Blizzard");
                Addon {
                    addon_type: if blizzard {
                        AddonType::Blizzard
                    } else {
                        AddonType::Enabled
                    },
                    info_block: if blizzard {
                        Addon_InfoBlock::Available {
                            key_version: if addon.addon_crc == BLIZZARD_ADDON_CRC {
                                Addon_KeyVersion::Zero
                            } else {
                                Addon_KeyVersion::One {
                                    public_key: ADDON_PUBLIC_KEY,
                                }
                            },
                            update_available_flag: 0,
                        }
                    } else {
                        Addon_InfoBlock::Unavailable
                    },
                    url_info: Addon_UrlInfo::Unavailable,
                }
            })
            .collect(),
    }
}

fn field<const N: usize>(bytes: &[u8], at: usize) -> io::Result<[u8; N]> {
    bytes
        .get(at..at + N)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(short)
}

fn short() -> io::Error {
    invalid("CMSG_AUTH_SESSION body is shorter than its fixed fields")
}

fn invalid(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use wow_world_messages::vanilla::{ClientMessage, ServerMessage, CMSG_AUTH_SESSION};

    const PACKET_FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/auth-session-5875.bin");

    fn frame(session: &CMSG_AUTH_SESSION) -> Vec<u8> {
        let mut bytes = Vec::new();
        session
            .write_unencrypted_client(&mut std::io::Cursor::new(&mut bytes))
            .unwrap();
        bytes
    }

    fn tester() -> CMSG_AUTH_SESSION {
        CMSG_AUTH_SESSION {
            build: 5875,
            server_id: 1,
            username: "TESTER".into(),
            client_seed: 0xDEAD_BEEF,
            client_proof: [7u8; 20],
            addon_info: vec![],
        }
    }

    fn with_addon_block(block: &[u8]) -> Vec<u8> {
        let mut bytes = frame(&tester());
        bytes.truncate(6 + 8 + 7 + 24);
        bytes.extend_from_slice(block);
        let size = (bytes.len() - 2) as u16;
        bytes[..2].copy_from_slice(&size.to_be_bytes());
        bytes
    }

    fn compressed_addons(bytes: &[u8], advertised_size: u32) -> Vec<u8> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(bytes).unwrap();
        let mut block = advertised_size.to_le_bytes().to_vec();
        block.extend(encoder.finish().unwrap());
        block
    }

    fn addon_reply(addons: &[AddonInfo]) -> Vec<u8> {
        let mut bytes = Vec::new();
        build_addon_info(addons)
            .write_unencrypted_server(&mut bytes)
            .unwrap();
        bytes
    }

    #[test]
    fn decodes_the_build_5875_packet_fixture() {
        let mut packet = PACKET_FIXTURE;
        let claim = read_auth_session(&mut packet).unwrap();
        assert_eq!(claim.username, "A");
        assert_eq!(claim.client_seed, 0x49D80288);
        let names = [
            "Blizzard_AuctionUI",
            "Blizzard_BattlefieldMinimap",
            "Blizzard_BindingUI",
            "Blizzard_CombatText",
            "Blizzard_CraftUI",
            "Blizzard_GMSurveyUI",
            "Blizzard_InspectUI",
            "Blizzard_MacroUI",
            "Blizzard_RaidUI",
            "Blizzard_TalentUI",
            "Blizzard_TradeSkillUI",
            "Blizzard_TrainerUI",
        ];
        assert_eq!(
            claim.addon_info,
            names.map(|name| AddonInfo {
                addon_name: name.into(),
                addon_has_signature: 1,
                addon_crc: 0x4C1C776D,
                addon_extra_crc: 0,
            })
        );
        let mut expected = vec![0, 98, 0xef, 2];
        expected.extend([2, 1, 0, 0, 0, 0, 0, 0].repeat(12));
        assert_eq!(addon_reply(&claim.addon_info), expected);
    }

    #[test]
    fn preserves_custom_addon_fields_and_reply_order() {
        let mut session = tester();
        session.addon_info = vec![
            AddonInfo {
                addon_name: "Custom".into(),
                addon_has_signature: 1,
                addon_crc: 0x12345678,
                addon_extra_crc: 0x87654321,
            },
            AddonInfo {
                addon_name: "Blizzard_AuctionUI".into(),
                addon_has_signature: 1,
                addon_crc: 0x4C1C776D,
                addon_extra_crc: 0,
            },
            AddonInfo {
                addon_name: "Disabled".into(),
                addon_has_signature: 0,
                addon_crc: 0,
                addon_extra_crc: 0,
            },
        ];
        let claim = read_auth_session(&mut frame(&session).as_slice()).unwrap();
        assert_eq!(claim.addon_info, session.addon_info);
        assert_eq!(
            addon_reply(&claim.addon_info),
            [0, 16, 0xef, 2, 1, 0, 0, 2, 1, 0, 0, 0, 0, 0, 0, 1, 0, 0]
        );
    }

    #[test]
    fn sends_the_modulus_when_a_blizzard_addon_crc_differs() {
        let reply = addon_reply(&[AddonInfo {
            addon_name: "Blizzard_AuctionUI".into(),
            addon_crc: 1,
            ..Default::default()
        }]);
        assert_eq!(reply.len(), 268);
        assert_eq!(&reply[..7], &[1, 10, 0xef, 2, 2, 1, 1]);
        assert_eq!(&reply[7..263], &ADDON_PUBLIC_KEY);
        assert_eq!(&reply[263..], &[0; 5]);
    }

    #[test]
    fn accepts_empty_addon_lists_with_or_without_a_zlib_stream() {
        for block in [vec![0; 4], compressed_addons(&[], 0)] {
            let claim = read_auth_session(&mut with_addon_block(&block).as_slice()).unwrap();
            assert!(claim.addon_info.is_empty());
            assert_eq!(addon_reply(&claim.addon_info), [0, 2, 0xef, 2]);
        }
    }

    #[test]
    fn refuses_malformed_compressed_streams() {
        let valid = compressed_addons(b"Test\0\x01\0\0\0\0\0\0\0\0", 14);
        let mut bad_checksum = valid.clone();
        *bad_checksum.last_mut().unwrap() ^= 1;
        let mut trailing = valid.clone();
        trailing.push(0);
        for block in [
            vec![1, 0, 0, 0, 0xff],
            valid[..valid.len() - 1].to_vec(),
            bad_checksum,
            trailing,
            vec![],
            vec![0; 3],
        ] {
            let err = read_auth_session(&mut with_addon_block(&block).as_slice()).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn refuses_expansion_beyond_the_claimed_size_and_short_output() {
        for (bytes, size) in [(vec![0; MAX_ADDON_BYTES + 1], 1), (vec![0; 10], 11)] {
            let block = compressed_addons(&bytes, size);
            let err = read_auth_session(&mut with_addon_block(&block).as_slice()).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn refuses_incomplete_addon_entries() {
        for bytes in [
            b"MissingNul".as_slice(),
            b"Short\0\x01",
            b"\xff\0\0\0\0\0\0\0\0\0\0",
        ] {
            let block = compressed_addons(bytes, bytes.len() as u32);
            let err = read_auth_session(&mut with_addon_block(&block).as_slice()).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn bounds_the_addon_count_to_one_reply_frame() {
        let mut session = tester();
        session.addon_info = vec![
            AddonInfo {
                addon_name: "Blizzard_AuctionUI".into(),
                ..Default::default()
            };
            MAX_ADDONS
        ];
        let claim = read_auth_session(&mut frame(&session).as_slice()).unwrap();
        let reply = addon_reply(&claim.addon_info);
        assert_eq!(reply.len(), 65_476);
        assert_eq!(
            u16::from_be_bytes([reply[0], reply[1]]) as usize,
            reply.len() - 2
        );
        session.addon_info.push(AddonInfo::default());
        let err = read_auth_session(&mut frame(&session).as_slice()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn decodes_the_fields_the_handshake_proves_against() {
        let claim = read_auth_session(&mut frame(&tester()).as_slice()).unwrap();
        assert_eq!(
            claim,
            AuthSessionClaim {
                username: "TESTER".into(),
                client_seed: 0xDEAD_BEEF,
                client_proof: [7u8; 20],
                addon_info: vec![],
            }
        );
    }

    #[test]
    fn an_absurd_addon_size_with_garbage_zlib_is_invalid_data() {
        let mut bytes = frame(&tester());
        let fixed = 6 + 4 + 4 + "TESTER".len() + 1 + 4 + 20;
        bytes.truncate(fixed);
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0xFF; 16]);
        let size = (bytes.len() - 2) as u16;
        bytes[..2].copy_from_slice(&size.to_be_bytes());

        let err = read_auth_session(&mut bytes.as_slice()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_frame_with_another_opcode_is_invalid_data() {
        let mut bytes = frame(&tester());
        bytes[2..6].copy_from_slice(&0x0037u32.to_le_bytes());
        let err = read_auth_session(&mut bytes.as_slice()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("0x0037"), "{err}");
    }

    #[test]
    fn a_body_cut_inside_the_proof_is_invalid_data() {
        let mut bytes = frame(&tester());
        bytes.truncate(6 + 8 + 7 + 4 + 10);
        let size = (bytes.len() - 2) as u16;
        bytes[..2].copy_from_slice(&size.to_be_bytes());
        let err = read_auth_session(&mut bytes.as_slice()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_closed_socket_before_the_header_is_unexpected_eof() {
        let err = read_auth_session(&mut [0u8; 3].as_slice()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
