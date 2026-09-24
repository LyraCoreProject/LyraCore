//! Chat Channel packets: every Channel Notice (`SMSG_CHANNEL_NOTIFY`) and the member list
//! (`SMSG_CHANNEL_LIST`).
//!
//! Channel Notices are hand-encoded, because gtker 0.3's `SMSG_CHANNEL_NOTIFY` carries only the
//! notice and the channel name, never the payload that follows (cm:Channel.cpp:748-946).

use lyracore_shared::channel::notice;
use wow_world_messages::vanilla::{
    ChannelFlags, ChannelMember, ChannelMemberFlags, SMSG_CHANNEL_LIST,
};
use wow_world_messages::Guid;

/// `SMSG_CHANNEL_NOTIFY` (vanilla 5875).
pub const SMSG_CHANNEL_NOTIFY_OPCODE: u16 = 0x0099;

/// One Channel Notice: a Module row, or a Refusal the Gateway answers. Which fields a notice reads
/// is [`notify`]'s business; the others stay zero.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChannelNoticeView {
    pub notice: u8,
    pub channel_name: String,
    pub subject_guid: u64,
    pub actor_guid: u64,
    pub old_flags: u8,
    pub new_flags: u8,
    pub channel_flags: u32,
    pub text: String,
}

/// Encode one Channel Notice. The body is the notice byte and the channel name as a CString, then
/// the notice's payload (all little-endian, guids as the full u64):
/// - one guid: `subject_guid` for JOINED, LEFT, OWNER_CHANGED, PLAYER_ALREADY_MEMBER and INVITE;
///   `actor_guid` for PASSWORD_CHANGED and the announcement and moderation toggles;
/// - YOU_JOINED: the channel flags u32, then a channel index u32 that is always 0;
/// - MODE_CHANGE: `subject_guid`, old flags u8, new flags u8;
/// - PLAYER_KICKED, PLAYER_BANNED, PLAYER_UNBANNED: `subject_guid` (the target) then
///   `actor_guid` (the source);
/// - a CString `text` for PLAYER_NOT_FOUND, CHANNEL_OWNER, PLAYER_NOT_BANNED, PLAYER_INVITED and
///   PLAYER_INVITE_BANNED;
/// - nothing for the rest.
pub fn notify(view: &ChannelNoticeView) -> (u16, Vec<u8>) {
    let mut body = Vec::with_capacity(2 + view.channel_name.len() + 18);
    body.push(view.notice);
    push_cstr(&mut body, &view.channel_name);
    match view.notice {
        notice::JOINED
        | notice::LEFT
        | notice::OWNER_CHANGED
        | notice::PLAYER_ALREADY_MEMBER
        | notice::INVITE => push_guid(&mut body, view.subject_guid),
        notice::PASSWORD_CHANGED
        | notice::ANNOUNCEMENTS_ON
        | notice::ANNOUNCEMENTS_OFF
        | notice::MODERATION_ON
        | notice::MODERATION_OFF => push_guid(&mut body, view.actor_guid),
        notice::YOU_JOINED => {
            body.extend_from_slice(&view.channel_flags.to_le_bytes());
            body.extend_from_slice(&0u32.to_le_bytes());
        }
        notice::MODE_CHANGE => {
            push_guid(&mut body, view.subject_guid);
            body.push(view.old_flags);
            body.push(view.new_flags);
        }
        notice::PLAYER_KICKED | notice::PLAYER_BANNED | notice::PLAYER_UNBANNED => {
            push_guid(&mut body, view.subject_guid);
            push_guid(&mut body, view.actor_guid);
        }
        notice::PLAYER_NOT_FOUND
        | notice::CHANNEL_OWNER
        | notice::PLAYER_NOT_BANNED
        | notice::PLAYER_INVITED
        | notice::PLAYER_INVITE_BANNED => push_cstr(&mut body, &view.text),
        _ => {}
    }
    (SMSG_CHANNEL_NOTIFY_OPCODE, body)
}

/// `SMSG_CHANNEL_LIST` (cm:Channel.cpp:479-521): the stored name, the wire channel flags, and each
/// member's guid with its raw member flags. gtker's `ChannelMemberFlags` constants use other bit
/// names than the cores, so the flags pass through as raw bits.
pub fn build_channel_list(
    channel_name: String,
    channel_flags: u8,
    members: &[(u64, u8)],
) -> SMSG_CHANNEL_LIST {
    SMSG_CHANNEL_LIST {
        channel_name,
        channel_flags: ChannelFlags::new(channel_flags),
        members: members
            .iter()
            .map(|&(guid, flags)| ChannelMember {
                guid: Guid::new(guid),
                member_flags: ChannelMemberFlags::new(flags),
            })
            .collect(),
    }
}

fn push_cstr(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

fn push_guid(body: &mut Vec<u8>, guid: u64) {
    body.extend_from_slice(&guid.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

    const GUID_A: u64 = 0x0102_0304_0506_0708;
    const GUID_B: u64 = 0x1112_1314_1516_1718;
    const A_BYTES: [u8; 8] = [0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01];
    const B_BYTES: [u8; 8] = [0x18, 0x17, 0x16, 0x15, 0x14, 0x13, 0x12, 0x11];

    fn view(notice: u8) -> ChannelNoticeView {
        ChannelNoticeView {
            notice,
            channel_name: "Tx".to_string(),
            subject_guid: GUID_A,
            actor_guid: GUID_B,
            old_flags: 0x00,
            new_flags: 0x03,
            channel_flags: 0x3C,
            text: "Bob".to_string(),
        }
    }

    fn body(notice: u8) -> Vec<u8> {
        let (opcode, body) = notify(&view(notice));
        assert_eq!(opcode, 0x0099);
        body
    }

    fn head(notice: u8) -> Vec<u8> {
        vec![notice, b'T', b'x', 0]
    }

    #[test]
    fn subject_guid_notices_carry_the_subject() {
        for code in [0x00, 0x01, 0x08, 0x17, 0x18] {
            let mut expected = head(code);
            expected.extend_from_slice(&A_BYTES);
            assert_eq!(body(code), expected, "notice {code:#04x}");
        }
    }

    #[test]
    fn actor_guid_notices_carry_the_actor() {
        for code in [0x07, 0x0D, 0x0E, 0x0F, 0x10] {
            let mut expected = head(code);
            expected.extend_from_slice(&B_BYTES);
            assert_eq!(body(code), expected, "notice {code:#04x}");
        }
    }

    /// cm:Channel.cpp:767-772: the flags, then a channel index of 0.
    #[test]
    fn you_joined_carries_the_channel_flags_and_a_zero_index() {
        assert_eq!(body(0x02), [0x02, b'T', b'x', 0, 0x3C, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn mode_change_carries_the_member_and_both_flag_bytes() {
        let mut expected = head(0x0C);
        expected.extend_from_slice(&A_BYTES);
        expected.extend_from_slice(&[0x00, 0x03]);
        assert_eq!(body(0x0C), expected);
    }

    #[test]
    fn kick_and_ban_notices_carry_target_then_source() {
        for code in [0x12, 0x14, 0x15] {
            let mut expected = head(code);
            expected.extend_from_slice(&A_BYTES);
            expected.extend_from_slice(&B_BYTES);
            assert_eq!(body(code), expected, "notice {code:#04x}");
        }
    }

    #[test]
    fn name_notices_carry_the_text() {
        for code in [0x09, 0x0B, 0x16, 0x1D, 0x1E] {
            let mut expected = head(code);
            expected.extend_from_slice(b"Bob\0");
            assert_eq!(body(code), expected, "notice {code:#04x}");
        }
    }

    #[test]
    fn the_other_notices_carry_only_the_channel() {
        for code in [
            0x03, 0x04, 0x05, 0x06, 0x0A, 0x11, 0x13, 0x19, 0x1A, 0x1B, 0x1C, 0x1F,
        ] {
            assert_eq!(body(code), head(code), "notice {code:#04x}");
        }
    }

    /// cm:Channel.cpp:490-515: name, flags u8, count u32, then guid u64 and flags u8 per member.
    #[test]
    fn the_channel_list_writes_raw_member_flags() {
        let message = ServerOpcodeMessage::SMSG_CHANNEL_LIST(Box::new(build_channel_list(
            "Tx".to_string(),
            0x01,
            &[(GUID_A, 0x03), (GUID_B, 0x08)],
        )));
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        let mut expected = vec![b'T', b'x', 0, 0x01, 2, 0, 0, 0];
        expected.extend_from_slice(&A_BYTES);
        expected.push(0x03);
        expected.extend_from_slice(&B_BYTES);
        expected.push(0x08);
        assert_eq!(u16::from_le_bytes([wire[2], wire[3]]), 0x009B);
        assert_eq!(&wire[4..], expected.as_slice());
    }
}
