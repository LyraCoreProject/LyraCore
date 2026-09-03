//! The plaintext `CMSG_AUTH_SESSION` frame, decoded by hand.
//!
//! `wow_world_messages` sizes a buffer from the client's `addon_info_decompressed_size` and unwraps
//! the zlib decode, so an unauthenticated peer could abort the process or unwind the session with
//! one packet. The gateway never reads the addon list, so this decoder stops before the zlib blob.
//! Layout (gtker cmsg_auth_session.wowm): `u16 BE size` (opcode + body), `u32 LE opcode`,
//! `build: u32`, `server_id: u32`, `username: CString`, `client_seed: u32`, `client_proof: u8[20]`,
//! then the compressed addon list.

use std::io::{self, Read};

/// The opcode of the one plaintext client frame in the world handshake.
pub const CMSG_AUTH_SESSION_OPCODE: u32 = 0x01ED;

/// What the world handshake proves against: the fields before the addon blob.
#[derive(Debug, PartialEq, Eq)]
pub struct AuthSessionClaim {
    pub username: String,
    pub client_seed: u32,
    pub client_proof: [u8; 20],
}

/// Read one plaintext frame and decode it as `CMSG_AUTH_SESSION`. A wrong opcode or a short body
/// is `InvalidData`; a closed or silent socket surfaces as the read's own error kind.
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
    // `size` counts the u32 opcode. The u16 header bounds the body; no client field sizes it.
    let mut body = vec![0u8; size.saturating_sub(4)];
    r.read_exact(&mut body)?;
    parse_auth_session_body(&body)
}

fn parse_auth_session_body(body: &[u8]) -> io::Result<AuthSessionClaim> {
    // build + server_id: the gateway reads neither.
    let rest = body.get(8..).ok_or_else(short)?;
    let nul = rest.iter().position(|b| *b == 0).ok_or_else(short)?;
    let username = String::from_utf8(rest[..nul].to_vec())
        .map_err(|e| invalid(format!("CMSG_AUTH_SESSION username is not UTF-8: {e}")))?;
    let rest = &rest[nul + 1..];
    Ok(AuthSessionClaim {
        username,
        client_seed: u32::from_le_bytes(field(rest, 0)?),
        client_proof: field(rest, 4)?,
    })
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
    use wow_world_messages::vanilla::{ClientMessage, CMSG_AUTH_SESSION};

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

    #[test]
    fn decodes_the_fields_the_handshake_proves_against() {
        let claim = read_auth_session(&mut frame(&tester()).as_slice()).unwrap();
        assert_eq!(
            claim,
            AuthSessionClaim {
                username: "TESTER".into(),
                client_seed: 0xDEAD_BEEF,
                client_proof: [7u8; 20],
            }
        );
    }

    /// The whole point of the hand decoder: the addon blob is never sized or inflated.
    #[test]
    fn an_absurd_addon_size_with_garbage_zlib_is_ignored() {
        let mut bytes = frame(&tester());
        // Replace the decompressed-size field and everything after it.
        let fixed = 6 + 4 + 4 + "TESTER".len() + 1 + 4 + 20;
        bytes.truncate(fixed);
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        bytes.extend_from_slice(&[0xFF; 16]);
        let size = (bytes.len() - 2) as u16;
        bytes[..2].copy_from_slice(&size.to_be_bytes());

        let claim = read_auth_session(&mut bytes.as_slice()).unwrap();
        assert_eq!(claim.username, "TESTER");
    }

    #[test]
    fn a_frame_with_another_opcode_is_invalid_data() {
        let mut bytes = frame(&tester());
        bytes[2..6].copy_from_slice(&0x0037u32.to_le_bytes()); // CMSG_CHAR_ENUM
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
