//! Addon-bridge codec: the bridge's own private reply, and the raw routing that keeps addon
//! traffic off players who never asked for it.
//!
//! `wow_world_base`'s `Language` enum carries `Addon` (0xFFFFFFFF), so a typed `CMSG_MESSAGECHAT`
//! on the addon language decodes and dispatches exactly like any other chat line.
//! `gateway/src/world/mod.rs` still peeks the raw frame before typed decode, only to route it: a
//! bridge-prefixed frame goes to `handle_addon_message`; a PARTY, RAID, GUILD or OFFICER frame
//! with any other prefix falls through to the ordinary chat path, since `SendAddonMessage`
//! legitimately shares those four channels with other addons; every other chat type drops, since
//! no addon traffic reaches it. The bridge's own reply has no `chat_kind` to ride — it is a
//! private command channel between the server and one client's addon, not a broadcast chat line —
//! so it is hand-built here against the byte layout pinned from gtker's own serializer
//! (smsg_messagechat.rs).
//!
//! Envelope (the bridge's framing protocol, versioned): the chat TEXT is `"STC\t" + "v1|<cmd>|<seq>|
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
const CHAT_TYPE_PARTY: u32 = 1;
const CHAT_TYPE_RAID: u32 = 2;
const CHAT_TYPE_GUILD: u32 = 3;
const CHAT_TYPE_OFFICER: u32 = 4;
const CHAT_TYPE_WHISPER: u32 = 6;
const CHAT_TYPE_CHANNEL: u32 = 14;

/// Parse a raw `CMSG_MESSAGECHAT` body IF it is an addon-language message; `None` for every
/// normal-language frame (the caller re-parses those through gtker as before). Returns the chat
/// TYPE and the chat TEXT (`"<prefix>\t<message>"`). Layout (gtker cmsg_messagechat.rs):
/// `chat_type: u32 LE`, `language: u32 LE`, `[Whisper: target CString | Channel: channel CString]`,
/// `message CString`.
pub fn parse_addon_client_chat(body: &[u8]) -> Option<(u32, String)> {
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
    Some((
        chat_type,
        String::from_utf8_lossy(&rest[..nul]).into_owned(),
    ))
}

/// Whether `SendAddonMessage` traffic on `chat_type` must still reach real players through the
/// ordinary chat path instead of dropping. The client's own Lua API distributes AddonMessage over
/// exactly these four channels; RAID_LEADER, RAID_WARNING, WHISPER and CHANNEL never carry it.
pub const fn typed_addon_chat_type(chat_type: u32) -> bool {
    matches!(
        chat_type,
        CHAT_TYPE_PARTY | CHAT_TYPE_RAID | CHAT_TYPE_GUILD | CHAT_TYPE_OFFICER
    )
}

/// Whether `text` (the parsed `"<prefix>\t<message>"`) carries our own bridge prefix. A typed
/// chat type can carry a FOREIGN addon's traffic sharing the same wire; only our own prefix takes
/// the bridge.
pub fn is_bridge_prefixed(text: &str) -> bool {
    text.split_once('\t')
        .is_some_and(|(prefix, _)| prefix == BRIDGE_PREFIX)
}

/// Parse the v1 envelope for [`BRIDGE_PREFIX`]. Build 5875 requires escaped `||` separators in
/// `SendAddonMessage` and retains them on the wire. Unescaped envelopes remain valid for existing
/// Headless Clients. Multipart and foreign-prefix messages are declined.
pub fn parse_bridge_envelope(text: &str) -> Option<(String, String)> {
    let (prefix, envelope) = text.split_once('\t')?;
    if prefix != BRIDGE_PREFIX {
        return None;
    }
    let escaped = envelope.starts_with("v1||");
    let separator = if escaped { "||" } else { "|" };
    let mut it = envelope.splitn(5, separator);
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
    let payload = if escaped {
        payload.replace("||", "|")
    } else {
        payload.to_string()
    };
    Some((cmd.to_string(), payload))
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
            parse_addon_client_chat(&addon_whisper_body(text)),
            Some((CHAT_TYPE_WHISPER, text.to_string()))
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
        assert_eq!(
            parse_addon_client_chat(&b),
            Some((CHAT_TYPE_PARTY, text.to_string()))
        );
    }

    #[test]
    fn escaped_party_message_from_build_5875_reaches_the_bridge() {
        let mut body = vec![1, 0, 0, 0, 255, 255, 255, 255];
        body.extend_from_slice(b"STC\tv1||ping||22||1/1||PB\0");
        let (chat_type, text) = parse_addon_client_chat(&body).unwrap();
        assert_eq!(chat_type, CHAT_TYPE_PARTY);
        assert!(is_bridge_prefixed(&text));
        assert_eq!(
            parse_bridge_envelope(&text),
            Some(("ping".into(), "PB".into()))
        );
    }

    /// AC (fix #1): the client's `SendAddonMessage` API distributes over PARTY, RAID, GUILD and
    /// OFFICER only. RAID_LEADER and RAID_WARNING chat lines cannot carry addon traffic at all,
    /// and WHISPER/CHANNEL addon frames are this bridge's own private channels, not shared wire.
    #[test]
    fn typed_addon_chat_types_are_exactly_party_raid_guild_and_officer() {
        for chat_type in [
            CHAT_TYPE_PARTY,
            CHAT_TYPE_RAID,
            CHAT_TYPE_GUILD,
            CHAT_TYPE_OFFICER,
        ] {
            assert!(typed_addon_chat_type(chat_type), "{chat_type}");
        }
        for chat_type in [0, CHAT_TYPE_WHISPER, CHAT_TYPE_CHANNEL, 20, 21, 22, 87, 88] {
            assert!(!typed_addon_chat_type(chat_type), "{chat_type}");
        }
    }

    #[test]
    fn only_our_own_prefix_is_bridge_prefixed() {
        assert!(is_bridge_prefixed("STC\tv1|ping|0|1/1|hi"));
        assert!(!is_bridge_prefixed("BigWigs\thello"));
        assert!(!is_bridge_prefixed("no tab at all"));
    }

    /// AC (fix #1): a RAID addon frame from another addon's prefix is not ours, but its chat type
    /// still reaches real players — `gateway/src/world/mod.rs` lets it fall through to the typed
    /// reader instead of dropping it. This proves the premise the module doc now states: a typed
    /// `CMSG_MESSAGECHAT` on the addon language decodes cleanly, chat type and all.
    #[test]
    fn a_raid_addon_frame_from_another_prefix_decodes_through_the_typed_reader() {
        use wow_world_messages::vanilla::opcodes::ClientOpcodeMessage;
        use wow_world_messages::vanilla::{CMSG_MESSAGECHAT_ChatType, Language};

        let mut body = Vec::new();
        body.extend_from_slice(&CHAT_TYPE_RAID.to_le_bytes());
        body.extend_from_slice(&LANG_ADDON.to_le_bytes());
        body.extend_from_slice(b"BigWigs\thello");
        body.push(0);
        let (chat_type, text) = parse_addon_client_chat(&body).unwrap();
        assert!(
            !is_bridge_prefixed(&text),
            "a foreign prefix is not our bridge"
        );
        assert!(typed_addon_chat_type(chat_type), "RAID carries addon mail");

        // The same body, framed the way `gateway/src/world/mod.rs` frames it for the typed reader.
        let mut framed = Vec::new();
        framed.extend_from_slice(&((body.len() + 4) as u16).to_be_bytes());
        framed.extend_from_slice(&CMSG_MESSAGECHAT_OPCODE.to_le_bytes());
        framed.extend_from_slice(&body);
        let msg = ClientOpcodeMessage::read_unencrypted(&mut std::io::Cursor::new(framed))
            .expect("a typed CMSG_MESSAGECHAT on the addon language decodes cleanly");
        match msg {
            ClientOpcodeMessage::CMSG_MESSAGECHAT(chat) => {
                assert_eq!(chat.chat_type, CMSG_MESSAGECHAT_ChatType::Raid);
                assert_eq!(chat.language, Language::Addon);
                assert_eq!(chat.message, "BigWigs\thello");
            }
            _ => panic!("expected CMSG_MESSAGECHAT"),
        }
    }

    /// AC (fix #1): a SAY addon frame is not a typed chat type, so the caller keeps dropping it —
    /// SAY has no chat-kind route to a real player, addon-language or not.
    #[test]
    fn a_say_addon_frame_is_still_dropped() {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes()); // Say
        body.extend_from_slice(&LANG_ADDON.to_le_bytes());
        body.extend_from_slice(b"BigWigs\thello");
        body.push(0);
        let (chat_type, text) = parse_addon_client_chat(&body).unwrap();
        assert!(!is_bridge_prefixed(&text));
        assert!(
            !typed_addon_chat_type(chat_type),
            "SAY has no addon-chat route; the caller must drop it"
        );
    }

    #[test]
    fn escaped_envelope_decodes_payload_pipes_once() {
        assert_eq!(
            parse_bridge_envelope("STC\tv1||playerbots.order||21||1/1||follow||2126"),
            Some(("playerbots.order".into(), "follow|2126".into()))
        );
        assert_eq!(
            parse_bridge_envelope("STC\tv1||ping||22||1/1||a||||b"),
            Some(("ping".into(), "a||b".into()))
        );
        assert_eq!(
            parse_bridge_envelope("STC\tv1|ping|22|1/1|a||b"),
            Some(("ping".into(), "a||b".into()))
        );
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
