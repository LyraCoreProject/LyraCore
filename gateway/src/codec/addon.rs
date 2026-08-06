//! Addon-bridge codec (work-item 184): the LANG_ADDON escape hatch on both wire directions.
//!
//! gtker's vanilla `Language` enum has no `0xFFFFFFFF` variant, so a real `SendAddonMessage`
//! frame FAILS typed decode (and would be session-fatal), and the reply can't ride the typed
//! `SMSG_MESSAGECHAT` builder either. Both directions are therefore hand-parsed/hand-built here
//! against the byte layouts pinned from gtker's own serializers (cmsg/smsg_messagechat.rs).
//!
//! Envelope (the 184 framing protocol, versioned): the chat TEXT is `"STC\t" + "v1|<cmd>|<seq>|
//! <part>/<parts>|<payload>"`. v1 implements the single-part fast path only — a multi-part
//! message logs and drops (payloads that don't fit ~230 bytes are a design smell at this layer).

/// The addon language value `SendAddonMessage` stamps (`LANG_ADDON`).
pub const LANG_ADDON: u32 = 0xFFFF_FFFF;
/// Our bridge prefix — the addon sends/filters on exactly this.
pub const BRIDGE_PREFIX: &str = "STC";
/// `SMSG_MESSAGECHAT` / `CMSG_MESSAGECHAT` opcodes.
pub const SMSG_MESSAGECHAT_OPCODE: u16 = 0x0096;
pub const CMSG_MESSAGECHAT_OPCODE: u32 = 0x0095;

/// Client `ChatType` discriminants (u32 on the CMSG wire) this parser understands.
const CHAT_TYPE_WHISPER: u32 = 6;
const CHAT_TYPE_CHANNEL: u32 = 14;

/// Parse a raw `CMSG_MESSAGECHAT` body IF it is an addon-language message; `None` for every
/// normal-language frame (the caller re-parses those through gtker as before). Returns the chat
/// TEXT (`"<prefix>\t<message>"`). Layout (gtker cmsg_messagechat.rs): `chat_type: u32 LE`,
/// `language: u32 LE`, `[Whisper: target CString | Channel: channel CString]`, `message CString`.
pub fn parse_addon_client_chat(body: &[u8]) -> Option<String> {
    if body.len() < 9 {
        return None;
    }
    let chat_type = u32::from_le_bytes(body[0..4].try_into().ok()?);
    let language = u32::from_le_bytes(body[4..8].try_into().ok()?);
    if language != LANG_ADDON {
        return None;
    }
    let mut rest = &body[8..];
    if chat_type == CHAT_TYPE_WHISPER || chat_type == CHAT_TYPE_CHANNEL {
        // Skip the target/channel CString — the addon channel argument is cosmetic; we own the pipe.
        let nul = rest.iter().position(|b| *b == 0)?;
        rest = &rest[nul + 1..];
    }
    let nul = rest.iter().position(|b| *b == 0)?;
    Some(String::from_utf8_lossy(&rest[..nul]).into_owned())
}

/// Split `"<prefix>\t<message>"` and accept only [`BRIDGE_PREFIX`]; parse the v1 envelope
/// `v1|<cmd>|<seq>|<part>/<parts>|<payload>` → `(cmd, payload)`. Multi-part → `None` (logged by
/// the caller); foreign prefixes → `None` silently (other addons own their own airwaves).
pub fn parse_bridge_envelope(text: &str) -> Option<(String, String)> {
    let (prefix, envelope) = text.split_once('\t')?;
    if prefix != BRIDGE_PREFIX {
        return None;
    }
    let mut it = envelope.splitn(5, '|');
    let (v, cmd, _seq, parts, payload) = (
        it.next()?,
        it.next()?,
        it.next()?,
        it.next()?,
        it.next().unwrap_or(""),
    );
    if v != "v1" || parts != "1/1" || cmd.is_empty() {
        return None;
    }
    Some((cmd.to_string(), payload.to_string()))
}

/// Build the v1 single-part envelope text for a server→client message.
pub fn build_bridge_envelope(cmd: &str, payload: &str) -> String {
    format!("{BRIDGE_PREFIX}\tv1|{cmd}|0|1/1|{payload}")
}

/// Raw-build the addon-language `SMSG_MESSAGECHAT` (Whisper shape — the client surfaces it to
/// addons as `CHAT_MSG_ADDON` and does NOT print addon-language lines in the chat frame).
/// Layout (gtker smsg_messagechat.rs write_into_vec): `chat_type u8 (6)`, `language u32 LE`,
/// `sender2 u64 LE`, `msg_len u32 LE (len+1)`, msg bytes, NUL, `tag u8 (0)`.
pub fn build_addon_smsg_raw(sender_guid: u64, text: &str) -> (u16, Vec<u8>) {
    let bytes = text.as_bytes();
    let mut body = Vec::with_capacity(1 + 4 + 8 + 4 + bytes.len() + 1 + 1);
    body.push(6u8); // ChatType::Whisper
    body.extend_from_slice(&LANG_ADDON.to_le_bytes());
    body.extend_from_slice(&sender_guid.to_le_bytes());
    body.extend_from_slice(&((bytes.len() + 1) as u32).to_le_bytes());
    body.extend_from_slice(bytes);
    body.push(0);
    body.push(0); // PlayerChatTag::None
    (SMSG_MESSAGECHAT_OPCODE, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-constructed `SendAddonMessage("STC", "v1|ping|0|1/1|hi", "WHISPER", "Self")` body.
    fn addon_whisper_body(text: &str) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&CHAT_TYPE_WHISPER.to_le_bytes());
        b.extend_from_slice(&LANG_ADDON.to_le_bytes());
        b.extend_from_slice(b"Self\0");
        b.extend_from_slice(text.as_bytes());
        b.push(0);
        b
    }

    #[test]
    fn addon_whisper_parses_and_normal_language_declines() {
        let text = "STC\tv1|ping|0|1/1|hello";
        assert_eq!(
            parse_addon_client_chat(&addon_whisper_body(text)).as_deref(),
            Some(text)
        );
        // Same frame with Universal language → not ours; the typed path handles it.
        let mut normal = addon_whisper_body(text);
        normal[4..8].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(parse_addon_client_chat(&normal), None);
    }

    #[test]
    fn say_shape_addon_message_parses_without_target_skip() {
        // Party/say shapes carry no leading CString — the message is first.
        let text = "STC\tv1|ping|0|1/1|x";
        let mut b = Vec::new();
        b.extend_from_slice(&1u32.to_le_bytes()); // Party
        b.extend_from_slice(&LANG_ADDON.to_le_bytes());
        b.extend_from_slice(text.as_bytes());
        b.push(0);
        assert_eq!(parse_addon_client_chat(&b).as_deref(), Some(text));
    }

    #[test]
    fn envelope_round_trips_and_rejects_foreign_and_multipart() {
        let text = build_bridge_envelope("event.progress", "e1|55|100");
        assert_eq!(
            parse_bridge_envelope(&text),
            Some(("event.progress".to_string(), "e1|55|100".to_string()))
        );
        // Payloads keep their own pipes intact (splitn(5)).
        let t2 = build_bridge_envelope("c", "a|b|c");
        assert_eq!(parse_bridge_envelope(&t2).unwrap().1, "a|b|c");
        assert_eq!(parse_bridge_envelope("OTHER\tv1|x|0|1/1|y"), None);
        assert_eq!(parse_bridge_envelope("STC\tv1|x|0|1/2|y"), None);
        assert_eq!(parse_bridge_envelope("STC\tv2|x|0|1/1|y"), None);
    }

    #[test]
    fn smsg_raw_layout_matches_the_pinned_gtker_shape() {
        let (op, body) = build_addon_smsg_raw(9, "STC\tv1|pong|0|1/1|hi");
        assert_eq!(op, SMSG_MESSAGECHAT_OPCODE);
        assert_eq!(body[0], 6); // Whisper
        assert_eq!(&body[1..5], &LANG_ADDON.to_le_bytes());
        assert_eq!(&body[5..13], &9u64.to_le_bytes());
        let text = b"STC\tv1|pong|0|1/1|hi";
        assert_eq!(&body[13..17], &((text.len() + 1) as u32).to_le_bytes());
        assert_eq!(&body[17..17 + text.len()], text);
        assert_eq!(body[17 + text.len()], 0);
        assert_eq!(body[18 + text.len()], 0); // tag None
        assert_eq!(body.len(), 19 + text.len());
    }
}
