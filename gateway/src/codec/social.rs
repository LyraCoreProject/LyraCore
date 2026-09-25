//! Social-tier wire builders — `SMSG_MESSAGECHAT` (say/yell/whisper) + `SMSG_TEXT_EMOTE` /
//! `SMSG_EMOTE` (emotes) + `SMSG_WHO` (/who panel). Split out of `codec` (a 1.7k-line module) so
//! the say/yell/whisper/emote/who group lives in one focused place; re-exported from `codec`.

use lyracore_shared::chat::broadcast_chat;
use wow_world_base::shared::friend_result_vanilla_tbc::FriendResult;
use wow_world_messages::vanilla::{
    Area, Class, Emote, Friend, Friend_FriendStatus, Language, Level, MSG_RANDOM_ROLL_Server,
    PlayerChatTag, SMSG_MESSAGECHAT_ChatType, TextEmote, SMSG_EMOTE, SMSG_FRIEND_LIST,
    SMSG_IGNORE_LIST, SMSG_MESSAGECHAT, SMSG_TEXT_EMOTE,
};
use wow_world_messages::Guid;

// ─── /who panel ──────────────────────────────────────────────────────────────

/// One matched character row as `/who`'s filter (`world::who`) produces it — flat ints the codec
/// converts.
pub struct WhoPlayerView {
    pub name: String,
    /// The Guild name, empty outside a Guild.
    pub guild: String,
    pub level: u8,
    pub class: u8,
    pub race: u8,
    pub zone_id: u32,
}

/// `SMSG_WHO` opcode (vanilla 5875). RAW-encoded — see [`build_who_response_raw`].
pub const SMSG_WHO_OPCODE: u16 = 0x0063;

/// Push a NUL-terminated CString.
fn push_cstr(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

/// Build a RAW `SMSG_WHO` for `CMSG_WHO`. `players` is every match, already filtered to the
/// requester's team (`world::who::respond`); listing caps at 49, the vanilla client's display
/// limit, but `online_players` reports the full, uncapped match count.
///
/// RAW because gtker 0.3's typed `WhoPlayer` writes `class: u8`, `race: u8`, `area: u32`, then a
/// trailing `party_status: u32` — cm:MiscHandler.cpp:252-257 writes class, race AND zone as `u32`
/// each, with no `party_status` field. vm:MiscHandler.cpp:212 sends `party_status` only to clients
/// at build 1.8.4 or older, which 5875 is not. The gtker shape is 2 bytes short per entry, so a
/// real client reads every row after the first out of alignment. The body is (all little-endian):
///   - `listed_players: u32` (`players.len()`, capped at 49)
///   - `online_players: u32` (`players.len()`, the full match count, uncapped)
///   - per listed player: `name` (CString), `guild` (CString, empty outside a Guild),
///     `level: u32`, `class: u32`, `race: u32`, `zone: u32`
pub fn build_who_response_raw(players: &[WhoPlayerView]) -> (u16, Vec<u8>) {
    let listed = &players[..players.len().min(49)];
    let mut body = Vec::with_capacity(8 + listed.len() * 20);
    body.extend_from_slice(&(listed.len() as u32).to_le_bytes());
    body.extend_from_slice(&(players.len() as u32).to_le_bytes());
    for p in listed {
        push_cstr(&mut body, &p.name);
        push_cstr(&mut body, &p.guild);
        body.extend_from_slice(&u32::from(p.level).to_le_bytes());
        body.extend_from_slice(&u32::from(p.class).to_le_bytes());
        body.extend_from_slice(&u32::from(p.race).to_le_bytes());
        body.extend_from_slice(&p.zone_id.to_le_bytes());
    }
    (SMSG_WHO_OPCODE, body)
}

// ─── Friends / ignore list ─────────────────────────────────────────────────

/// One friend row as needed by `SMSG_FRIEND_LIST` — flat ints the codec converts. `online` false
/// means the row degrades to `Friend_FriendStatus::Offline` (no area/level/class carried, matching
/// the wire format — those fields are ONLY present for a non-offline status); `away` is read only
/// when `online` is true.
pub struct FriendView {
    pub guid: u64,
    pub online: bool,
    pub away: crate::world::presence::AwayStatus,
    pub level: u8,
    pub class: u8,
    pub zone_id: u32,
}

/// Build `SMSG_FRIEND_LIST` for `CMSG_FRIEND_LIST`. An online friend carries area/level/class (zone
/// ids outside the gtker `Area` enum degrade to `Area::None`, an unknown class degrades the WHOLE
/// row to `Offline` — better an inert row than a malformed one) and its status splits Online/Afk/
/// Dnd off `away`; an offline friend carries only the guid. Capped at 255 (the wire's `u8` count
/// prefix).
pub fn build_friend_list_response(friends: &[FriendView]) -> SMSG_FRIEND_LIST {
    let list = friends
        .iter()
        .take(255)
        .map(|f| {
            let status = if f.online {
                Class::try_from(f.class).ok().map(|class| {
                    let area = Area::try_from(f.zone_id).unwrap_or(Area::None);
                    let level = Level::new(f.level);
                    match f.away {
                        crate::world::presence::AwayStatus::None => {
                            Friend_FriendStatus::Online { area, class, level }
                        }
                        crate::world::presence::AwayStatus::Afk => {
                            Friend_FriendStatus::Afk { area, class, level }
                        }
                        crate::world::presence::AwayStatus::Dnd => {
                            Friend_FriendStatus::Dnd { area, class, level }
                        }
                    }
                })
            } else {
                None
            };
            Friend {
                guid: Guid::new(f.guid),
                status: status.unwrap_or(Friend_FriendStatus::Offline),
            }
        })
        .collect();
    SMSG_FRIEND_LIST { friends: list }
}

/// Build `SMSG_IGNORE_LIST` for `CMSG_FRIEND_LIST` (sent alongside `SMSG_FRIEND_LIST` — vanilla
/// replies to the one request with both lists). Capped at 255 (the wire's `u8` count prefix).
pub fn build_ignore_list_response(ignored: &[u64]) -> SMSG_IGNORE_LIST {
    SMSG_IGNORE_LIST {
        ignored: ignored.iter().take(255).copied().collect(),
    }
}

/// `SMSG_FRIEND_STATUS` opcode (vanilla 5875).
pub const SMSG_FRIEND_STATUS_OPCODE: u16 = 0x0068;

/// The trailing fields `SMSG_FRIEND_STATUS` carries for `FriendResult::Online` and
/// `FriendResult::AddedOnline` only (cm:SocialMgr.cpp:227-261, vm:SocialMgr.cpp:248-259).
#[derive(Clone)]
pub struct FriendOnline {
    pub away: crate::world::presence::AwayStatus,
    pub zone_id: u32,
    pub level: u8,
    pub class: u8,
}

impl FriendOnline {
    /// The status byte: 1 online, 2 afk, 4 dnd (cm:SocialMgr.h:31-38).
    fn status_byte(&self) -> u8 {
        match self.away {
            crate::world::presence::AwayStatus::None => 1,
            crate::world::presence::AwayStatus::Afk => 2,
            crate::world::presence::AwayStatus::Dnd => 4,
        }
    }
}

/// Build the RAW `SMSG_FRIEND_STATUS` — the add/remove confirmation (or rejection) for one
/// contact-list action. `result` is the vanilla `FriendResult` discriminant (`ADDED_ONLINE`/
/// `ADDED_OFFLINE`/`REMOVED`/`IGNORE_ADDED`/`IGNORE_REMOVED`/`NOT_FOUND`/`ENEMY`/`ALREADY`/… — the
/// client renders the right system message off it); `guid` is the OTHER party (0 when the action
/// failed to resolve a name).
///
/// RAW because gtker 0.3's typed `SMSG_FRIEND_STATUS` writes only `result` and `guid`: cm's
/// `ONLINE`/`ADDED_ONLINE` also append status u8, area u32, level u32, class u32
/// (cm:SocialMgr.cpp:227-261), which gtker 0.3 cannot encode. `online` carries those trailing
/// fields and must be `Some` for exactly `FriendResult::Online`/`FriendResult::AddedOnline`, `None`
/// otherwise — the caller decides which, from the same Realm Presence read either way.
pub fn build_friend_status_raw(
    result: FriendResult,
    guid: u64,
    online: Option<FriendOnline>,
) -> (u16, Vec<u8>) {
    let mut body = vec![result.as_int()];
    body.extend_from_slice(&guid.to_le_bytes());
    if let Some(online) = online {
        body.push(online.status_byte());
        body.extend_from_slice(&online.zone_id.to_le_bytes());
        body.extend_from_slice(&u32::from(online.level).to_le_bytes());
        body.extend_from_slice(&u32::from(online.class).to_le_bytes());
    }
    (SMSG_FRIEND_STATUS_OPCODE, body)
}

/// Build `SMSG_MESSAGECHAT` for nearby player or creature speech.
///
/// Creature packets carry the speaker name and use the matching monster variant. Unknown source
/// modes retain the existing player-say fallback.
#[cfg(test)]
fn build_chat_message(
    sender_guid: u64,
    sender_name: Option<String>,
    chat_type: u8,
    language: u8,
    message: String,
) -> SMSG_MESSAGECHAT {
    build_chat_message_to(sender_guid, sender_name, 0, chat_type, language, message)
}

/// Build creature speech while retaining the addressed target chosen by EventAI. A player EMOTE
/// row (`chat_type` 3, never a creature — EventAI has no such source) has no name, so it always
/// hits the `(EMOTE, None)` arm and names the speaker via `sender2`.
pub fn build_chat_message_to(
    sender_guid: u64,
    sender_name: Option<String>,
    target_guid: u64,
    chat_type: u8,
    language: u8,
    message: String,
) -> SMSG_MESSAGECHAT {
    let sender = Guid::new(sender_guid);
    let kind = match (chat_type, sender_name) {
        (0, Some(sender_name)) => SMSG_MESSAGECHAT_ChatType::MonsterSay {
            sender1: sender,
            sender_name,
            target: Guid::new(target_guid),
        },
        (1, Some(sender_name)) => SMSG_MESSAGECHAT_ChatType::MonsterYell {
            sender1: sender,
            sender_name,
            target: Guid::new(target_guid),
        },
        // `MonsterEmote` carries one guid and it names the SPEAKER, paired with the speaker's own
        // name. The variant has no addressee field on the wire, so an emote's addressed target is
        // not expressible here and is dropped rather than misfiled into the speaker slot.
        (broadcast_chat::CREATURE_TEXT_EMOTE, Some(monster_name)) => {
            SMSG_MESSAGECHAT_ChatType::MonsterEmote {
                monster: sender,
                monster_name,
            }
        }
        (1, None) => SMSG_MESSAGECHAT_ChatType::Yell {
            chat_credit: sender,
            speech_bubble_credit: sender,
        },
        (broadcast_chat::CREATURE_TEXT_EMOTE, None) => SMSG_MESSAGECHAT_ChatType::MonsterEmote {
            monster: sender,
            monster_name: String::new(),
        },
        // `/e` (cm:Chat.cpp:3660-3662): a Character-only broadcast type, always Universal below.
        (broadcast_chat::EMOTE, None) => SMSG_MESSAGECHAT_ChatType::Emote { sender2: sender },
        _ => SMSG_MESSAGECHAT_ChatType::Say {
            chat_credit: sender,
            speech_bubble_credit: sender,
        },
    };
    SMSG_MESSAGECHAT {
        chat_type: kind,
        language: if matches!(
            chat_type,
            broadcast_chat::CREATURE_TEXT_EMOTE | broadcast_chat::EMOTE
        ) {
            Language::Universal
        } else {
            Language::try_from(language).unwrap_or(Language::Universal)
        },
        message,
        tag: PlayerChatTag::None,
    }
}

/// Build `SMSG_MESSAGECHAT` for a whisper. `is_inform` = the sender's echo ("To X:") via the
/// `WhisperInform` type; otherwise the incoming line ("X whispers:") via `Whisper`. `other_guid` is
/// the OTHER party (the client resolves their name via NAME_QUERY). Whispers are always understood
/// (Universal language); tag None.
pub fn build_whisper(other_guid: u64, is_inform: bool, message: String) -> SMSG_MESSAGECHAT {
    let other = Guid::new(other_guid);
    let kind = if is_inform {
        SMSG_MESSAGECHAT_ChatType::WhisperInform { sender2: other }
    } else {
        SMSG_MESSAGECHAT_ChatType::Whisper { sender2: other }
    };
    SMSG_MESSAGECHAT {
        chat_type: kind,
        language: Language::Universal,
        message,
        tag: PlayerChatTag::None,
    }
}

/// Build the `SMSG_MESSAGECHAT` for one Realm Chat Line. Covers every Chat Kind in
/// `lyracore_shared::chat::chat_kind`, so a new audience family never edits the codec. PARTY names
/// the speaker in both credit fields, CHANNEL carries the channel name with rank 0, and the other
/// kinds carry one guid. An unknown language goes out as Universal. `None` for an unknown kind.
pub fn build_realm_chat_line(
    kind: u8,
    speaker_guid: u64,
    language: u32,
    chat_tag: u8,
    channel_name: String,
    message: String,
) -> Option<SMSG_MESSAGECHAT> {
    use lyracore_shared::chat::chat_kind;
    let sender2 = Guid::new(speaker_guid);
    let chat_type = match kind {
        chat_kind::PARTY => SMSG_MESSAGECHAT_ChatType::Party {
            chat_credit: sender2,
            speech_bubble_credit: sender2,
        },
        chat_kind::CHANNEL => SMSG_MESSAGECHAT_ChatType::Channel {
            channel_name,
            player: sender2,
            player_rank: 0,
        },
        chat_kind::RAID => SMSG_MESSAGECHAT_ChatType::Raid { sender2 },
        chat_kind::GUILD => SMSG_MESSAGECHAT_ChatType::Guild { sender2 },
        chat_kind::OFFICER => SMSG_MESSAGECHAT_ChatType::Officer { sender2 },
        chat_kind::WHISPER => SMSG_MESSAGECHAT_ChatType::Whisper { sender2 },
        chat_kind::WHISPER_INFORM => SMSG_MESSAGECHAT_ChatType::WhisperInform { sender2 },
        chat_kind::AFK => SMSG_MESSAGECHAT_ChatType::Afk { sender2 },
        chat_kind::DND => SMSG_MESSAGECHAT_ChatType::Dnd { sender2 },
        chat_kind::IGNORED => SMSG_MESSAGECHAT_ChatType::Ignored { sender2 },
        chat_kind::RAID_LEADER => SMSG_MESSAGECHAT_ChatType::RaidLeader { sender2 },
        chat_kind::RAID_WARNING => SMSG_MESSAGECHAT_ChatType::RaidWarning { sender2 },
        _ => return None,
    };
    Some(SMSG_MESSAGECHAT {
        chat_type,
        language: Language::try_from(language).unwrap_or(Language::Universal),
        message,
        tag: PlayerChatTag::try_from(chat_tag).unwrap_or(PlayerChatTag::None),
    })
}

/// Build `SMSG_MESSAGECHAT` System — a self-only server line (the GM dot-command
/// reducer's `Err` relayed back to the SENDER, e.g. "permission denied", "unknown command: .foo").
/// `System`'s `sender2` guid is always 0 (no meaningful sender for a server-originated line, mirroring
/// how vanilla's own system messages carry no unit credit). Always Universal + no tag, like whispers/
/// party (this line is never proximity/language-filtered — it never leaves this one connection).
pub fn build_gm_system_message(message: String) -> SMSG_MESSAGECHAT {
    SMSG_MESSAGECHAT {
        chat_type: SMSG_MESSAGECHAT_ChatType::System {
            sender2: Guid::new(0),
        },
        language: Language::Universal,
        message,
        tag: PlayerChatTag::None,
    }
}

/// Build `SMSG_TEXT_EMOTE` — the "X dances." chat-log line observers see for an emote. `text_emote`
/// is the social-emote id (drives the wording); `emote_anim` echoes the animation id; `target_name`
/// is the resolved name of the CMSG-supplied target (`None`/empty → the client renders the untargeted
/// "X dances." line; `Some(name)` → "X waves at name."). Returns `None` if `text_emote` isn't a known
/// emote (the client always sends a valid one; this just degrades safely).
pub fn build_text_emote(
    sender_guid: u64,
    text_emote: u32,
    emote_anim: u32,
    target_name: Option<String>,
) -> Option<SMSG_TEXT_EMOTE> {
    let te = TextEmote::try_from(text_emote).ok()?;
    Some(SMSG_TEXT_EMOTE {
        guid: Guid::new(sender_guid),
        text_emote: te,
        emote: emote_anim,
        name: target_name.unwrap_or_default(),
    })
}

/// Build `MSG_RANDOM_ROLL_Server`: the `/roll` result for the roller's group, or for the roller
/// alone. `minimum` / `maximum` are the clamped range from the roller's request; `actual_roll` is
/// the server-computed result; `guid` is the roller. All values echo what the module committed.
pub fn build_random_roll(
    roller_guid: u64,
    minimum: u32,
    maximum: u32,
    actual_roll: u32,
) -> MSG_RANDOM_ROLL_Server {
    MSG_RANDOM_ROLL_Server {
        minimum,
        maximum,
        actual_roll,
        guid: Guid::new(roller_guid),
    }
}

/// Build `SMSG_EMOTE` — the animation observers play for the performer. `emote_anim` is the
/// client-provided `Emote` id; `None` (text-only) when it isn't a known animation, so a missing /
/// zero id never breaks the emote (the chat line still goes out). The text line is the primary
/// social signal; the animation is an additive nice-to-have, so they relay independently by design.
pub fn build_emote_anim(sender_guid: u64, emote_anim: u32) -> Option<SMSG_EMOTE> {
    let emote = Emote::try_from(emote_anim).ok()?;
    Some(SMSG_EMOTE {
        emote,
        guid: Guid::new(sender_guid),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wow_world_messages::vanilla::ServerMessage;

    #[test]
    fn chat_message_carries_sender_type_and_serializes() {
        // Say: sender guid rides in the Say variant (both credit fields); message + language survive
        // the writer. Yell: the variant flips. Serialize each so the writer runs over the new path.
        let say = build_chat_message(7, None, 0, 0, "hello world".into());
        match &say.chat_type {
            SMSG_MESSAGECHAT_ChatType::Say {
                chat_credit,
                speech_bubble_credit,
            } => {
                assert_eq!(chat_credit.guid(), 7);
                assert_eq!(speech_bubble_credit.guid(), 7);
            }
            other => panic!("expected Say, got {other:?}"),
        }
        assert_eq!(say.message, "hello world");
        let mut buf = Vec::new();
        say.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());

        let yell = build_chat_message(9, None, 1, 0, "RUN".into());
        assert!(matches!(
            yell.chat_type,
            SMSG_MESSAGECHAT_ChatType::Yell { .. }
        ));
        // An unknown language discriminant falls back to Universal rather than panicking.
        let creature_say = build_chat_message(
            0xF130_0000_0000_000B,
            Some("Defias Thug".into()),
            0,
            0,
            "You there!".into(),
        );
        match creature_say.chat_type {
            SMSG_MESSAGECHAT_ChatType::MonsterSay {
                sender1,
                sender_name,
                target,
            } => {
                assert_eq!(sender1.guid(), 0xF130_0000_0000_000B);
                assert_eq!(sender_name, "Defias Thug");
                assert_eq!(target.guid(), 0);
            }
            other => panic!("expected MonsterSay, got {other:?}"),
        }
        let creature_yell = build_chat_message(
            0xF130_0000_0000_000B,
            Some("Defias Thug".into()),
            1,
            0,
            "Run!".into(),
        );
        match creature_yell.chat_type {
            SMSG_MESSAGECHAT_ChatType::MonsterYell {
                sender1,
                sender_name,
                target,
            } => {
                assert_eq!(sender1.guid(), 0xF130_0000_0000_000B);
                assert_eq!(sender_name, "Defias Thug");
                assert_eq!(target.guid(), 0);
            }
            other => panic!("expected MonsterYell, got {other:?}"),
        }

        let addressed = build_chat_message_to(
            0xF130_0000_0000_000B,
            Some("Defias Thug".into()),
            77,
            0,
            0,
            "You there!".into(),
        );
        match &addressed.chat_type {
            SMSG_MESSAGECHAT_ChatType::MonsterSay { target, .. } => {
                assert_eq!(target.guid(), 77);
            }
            other => panic!("expected addressed MonsterSay, got {other:?}"),
        }
        let mut buf = Vec::new();
        addressed.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());

        let emote = build_chat_message_to(
            0xF130_0000_0000_000B,
            Some("Defias Thug".into()),
            77,
            broadcast_chat::CREATURE_TEXT_EMOTE,
            0,
            "laughs.".into(),
        );
        match &emote.chat_type {
            SMSG_MESSAGECHAT_ChatType::MonsterEmote {
                monster,
                monster_name,
            } => {
                // The guid and the name must describe the same unit: the speaking creature.
                assert_eq!(monster.guid(), 0xF130_0000_0000_000B);
                assert_eq!(monster_name, "Defias Thug");
            }
            other => panic!("expected MonsterEmote, got {other:?}"),
        }
        assert_eq!(emote.language, Language::Universal);
        let mut buf = Vec::new();
        emote.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());

        let weird = build_chat_message(1, None, 0, 250, "x".into());
        assert_eq!(weird.language, Language::Universal);
    }

    /// A player `/e` row (no creature name — EventAI never emits EMOTE) builds `Emote { sender2 }`
    /// naming the speaker, in Universal whatever language byte the row carries
    /// (cm:Chat.cpp:3660-3662, cm:Player.cpp:16594).
    #[test]
    fn player_emote_row_builds_emote_in_universal_and_round_trips() {
        let emote = build_chat_message_to(
            7,
            None,
            0,
            broadcast_chat::EMOTE,
            1, // a non-Universal byte on the row must not leak onto the wire
            "waves wildly.".into(),
        );
        match &emote.chat_type {
            SMSG_MESSAGECHAT_ChatType::Emote { sender2 } => assert_eq!(sender2.guid(), 7),
            other => panic!("expected Emote, got {other:?}"),
        }
        assert_eq!(emote.language, Language::Universal);
        assert_eq!(emote.message, "waves wildly.");
        assert_eq!(read_back(&emote), emote);
    }

    #[test]
    fn whisper_picks_variant_by_direction_and_carries_other_party() {
        // Incoming whisper → Whisper { sender2 = the other party }; serialize so the writer runs.
        let incoming = build_whisper(42, false, "hi".into());
        match &incoming.chat_type {
            SMSG_MESSAGECHAT_ChatType::Whisper { sender2 } => assert_eq!(sender2.guid(), 42),
            other => panic!("expected Whisper, got {other:?}"),
        }
        assert_eq!(incoming.language, Language::Universal); // whispers are always understood
        let mut buf = Vec::new();
        incoming.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
        // The sender's echo → WhisperInform { sender2 = the target }.
        let echo = build_whisper(99, true, "hi".into());
        match &echo.chat_type {
            SMSG_MESSAGECHAT_ChatType::WhisperInform { sender2 } => assert_eq!(sender2.guid(), 99),
            other => panic!("expected WhisperInform, got {other:?}"),
        }
    }

    fn read_back(message: &SMSG_MESSAGECHAT) -> SMSG_MESSAGECHAT {
        use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
        let mut wire = Vec::new();
        message.write_unencrypted_server(&mut wire).unwrap();
        match ServerOpcodeMessage::read_unencrypted(&mut wire.as_slice()).unwrap() {
            ServerOpcodeMessage::SMSG_MESSAGECHAT(read) => *read,
            other => panic!("expected SMSG_MESSAGECHAT, got {other}"),
        }
    }

    /// Every Chat Kind, written out from the 1.12 `SMSG_MESSAGECHAT` layout
    /// (cm:Chat.cpp:3616-3669): the wire value, then the variant the vanilla reader must decode.
    #[test]
    fn every_realm_chat_kind_round_trips_through_the_vanilla_reader() {
        let speaker = Guid::new(42);
        let cases = [
            (
                0x01,
                SMSG_MESSAGECHAT_ChatType::Party {
                    chat_credit: speaker,
                    speech_bubble_credit: speaker,
                },
            ),
            (0x02, SMSG_MESSAGECHAT_ChatType::Raid { sender2: speaker }),
            (0x03, SMSG_MESSAGECHAT_ChatType::Guild { sender2: speaker }),
            (
                0x04,
                SMSG_MESSAGECHAT_ChatType::Officer { sender2: speaker },
            ),
            (
                0x06,
                SMSG_MESSAGECHAT_ChatType::Whisper { sender2: speaker },
            ),
            (
                0x07,
                SMSG_MESSAGECHAT_ChatType::WhisperInform { sender2: speaker },
            ),
            (
                0x0E,
                SMSG_MESSAGECHAT_ChatType::Channel {
                    channel_name: "Trade - City".to_string(),
                    player: speaker,
                    player_rank: 0,
                },
            ),
            (0x14, SMSG_MESSAGECHAT_ChatType::Afk { sender2: speaker }),
            (0x15, SMSG_MESSAGECHAT_ChatType::Dnd { sender2: speaker }),
            (
                0x16,
                SMSG_MESSAGECHAT_ChatType::Ignored { sender2: speaker },
            ),
            (
                0x57,
                SMSG_MESSAGECHAT_ChatType::RaidLeader { sender2: speaker },
            ),
            (
                0x58,
                SMSG_MESSAGECHAT_ChatType::RaidWarning { sender2: speaker },
            ),
        ];
        for (kind, chat_type) in cases {
            let line = build_realm_chat_line(
                kind,
                42,
                6,
                2,
                "Trade - City".to_string(),
                "form up".to_string(),
            )
            .unwrap_or_else(|| panic!("kind {kind:#x} has no packet"));
            let expected = SMSG_MESSAGECHAT {
                chat_type,
                language: Language::Dwarvish,
                message: "form up".to_string(),
                tag: PlayerChatTag::Dnd,
            };
            assert_eq!(line, expected, "kind {kind:#x}");
            assert_eq!(read_back(&line), expected, "kind {kind:#x} on the wire");
        }
    }

    #[test]
    fn an_unknown_kind_has_no_packet_and_an_unknown_language_is_universal() {
        assert_eq!(
            build_realm_chat_line(0x00, 42, 0, 0, String::new(), "x".into()),
            None,
            "say is proximity chat, never a Realm Chat Line"
        );
        assert_eq!(
            build_realm_chat_line(0xFF, 42, 0, 0, String::new(), "x".into()),
            None
        );
        let line = build_realm_chat_line(0x01, 42, 250, 9, String::new(), "x".into()).unwrap();
        assert_eq!(line.language, Language::Universal);
        assert_eq!(line.tag, PlayerChatTag::None);
        assert_eq!(
            build_realm_chat_line(0x01, 42, 0xFFFF_FFFF, 1, String::new(), "x".into())
                .unwrap()
                .language,
            Language::Addon
        );
    }

    #[test]
    fn emote_builders_carry_performer_and_degrade_safely() {
        // A known text-emote id (1 = a real TextEmote) yields the "X dances." line with the performer
        // guid; serialize so the writer runs over it. An unknown id yields None (text-only degrade).
        let te = build_text_emote(5, 1, 10, None).expect("text_emote id 1 is known");
        assert_eq!(te.guid.guid(), 5);
        assert!(te.name.is_empty());
        let mut buf = Vec::new();
        te.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
        assert!(build_text_emote(5, u32::MAX, 0, None).is_none());

        // A resolved target name flows through untouched (the "waves at <name>" case).
        let targeted =
            build_text_emote(5, 1, 10, Some("Bob".to_string())).expect("text_emote id 1 is known");
        assert_eq!(targeted.name, "Bob");

        // The animation: a valid Emote id → Some carrying the performer; an unknown id → None.
        let anim = build_emote_anim(5, 10).expect("Emote id 10 is known");
        assert_eq!(anim.guid.guid(), 5);
        assert!(build_emote_anim(5, u32::MAX).is_none());
    }

    fn online_friend(guid: u64, away: crate::world::presence::AwayStatus) -> FriendView {
        FriendView {
            guid,
            online: true,
            away,
            level: 30,
            class: 1,
            zone_id: 12,
        }
    }

    #[test]
    fn friend_list_online_carries_presence_offline_is_bare() {
        let friends = [
            online_friend(11, crate::world::presence::AwayStatus::None),
            FriendView {
                guid: 22,
                online: false,
                away: crate::world::presence::AwayStatus::None,
                level: 0,
                class: 0,
                zone_id: 0,
            },
        ];
        let resp = build_friend_list_response(&friends);
        assert_eq!(resp.friends.len(), 2);
        assert_eq!(resp.friends[0].guid.guid(), 11);
        match &resp.friends[0].status {
            Friend_FriendStatus::Online { level, class, .. } => {
                assert_eq!(level.as_int(), 30);
                assert_eq!(*class, Class::Warrior);
            }
            other => panic!("expected Online, got {other:?}"),
        }
        assert_eq!(resp.friends[1].guid.guid(), 22);
        assert!(matches!(
            resp.friends[1].status,
            Friend_FriendStatus::Offline
        ));
        let mut buf = Vec::new();
        resp.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }

    /// Away Status splits the wire status code: Online 1, Afk 2, Dnd 4 (cm:SocialMgr.h:31-38).
    #[test]
    fn friend_list_online_status_follows_away_status() {
        use crate::world::presence::AwayStatus;

        let friends = [
            online_friend(1, AwayStatus::None),
            online_friend(2, AwayStatus::Afk),
            online_friend(3, AwayStatus::Dnd),
        ];
        let resp = build_friend_list_response(&friends);
        assert!(matches!(
            resp.friends[0].status,
            Friend_FriendStatus::Online { .. }
        ));
        assert!(matches!(
            resp.friends[1].status,
            Friend_FriendStatus::Afk { .. }
        ));
        assert!(matches!(
            resp.friends[2].status,
            Friend_FriendStatus::Dnd { .. }
        ));
    }

    #[test]
    fn friend_list_unknown_class_degrades_whole_row_to_offline() {
        // An online row with an unmapped class discriminant can't build a valid Online variant —
        // degrade the WHOLE row to Offline rather than send a malformed one.
        let friends = [FriendView {
            guid: 5,
            online: true,
            away: crate::world::presence::AwayStatus::None,
            level: 10,
            class: 250,
            zone_id: 1,
        }];
        let resp = build_friend_list_response(&friends);
        assert!(matches!(
            resp.friends[0].status,
            Friend_FriendStatus::Offline
        ));
    }

    #[test]
    fn ignore_list_carries_bare_guids_and_serializes() {
        let resp = build_ignore_list_response(&[1, 2, 3]);
        assert_eq!(resp.ignored, vec![1, 2, 3]);
        let mut buf = Vec::new();
        resp.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }

    /// Bytes hand-written from the protocol table (README, cm:SocialMgr.cpp:227-261): result u8,
    /// guid u64 LE, then for `ADDED_ONLINE` only status u8 (ONLINE=1), area u32, level u32, class
    /// u32, all little-endian. Never recomputed with the builder's own field order.
    #[test]
    fn friend_status_raw_is_byte_exact_for_added_online() {
        let (opcode, body) = build_friend_status_raw(
            FriendResult::AddedOnline,
            77,
            Some(FriendOnline {
                away: crate::world::presence::AwayStatus::None,
                zone_id: 12,
                level: 30,
                class: 1,
            }),
        );
        assert_eq!(opcode, SMSG_FRIEND_STATUS_OPCODE);
        let mut expected = vec![0x06]; // ADDED_ONLINE
        expected.extend(77u64.to_le_bytes());
        expected.push(1); // status ONLINE
        expected.extend(12u32.to_le_bytes()); // area
        expected.extend(30u32.to_le_bytes()); // level
        expected.extend(1u32.to_le_bytes()); // class
        assert_eq!(body, expected);
    }

    /// An Afk friend's `ADDED_ONLINE`/`ONLINE` status byte is 2, Dnd is 4 (cm:SocialMgr.h:31-38) —
    /// distinct from the chat tag's own 1/2 numbering.
    #[test]
    fn friend_status_raw_status_byte_follows_away_status() {
        for (away, status) in [
            (crate::world::presence::AwayStatus::None, 1u8),
            (crate::world::presence::AwayStatus::Afk, 2),
            (crate::world::presence::AwayStatus::Dnd, 4),
        ] {
            let (_, body) = build_friend_status_raw(
                FriendResult::Online,
                1,
                Some(FriendOnline {
                    away,
                    zone_id: 0,
                    level: 0,
                    class: 0,
                }),
            );
            assert_eq!(body[9], status, "{away:?}");
        }
    }

    /// A rejection or an offline add/remove carries no trailing fields: result u8 then guid u64 LE,
    /// nothing after.
    #[test]
    fn friend_status_raw_carries_no_trailing_fields_without_online() {
        let (_, body) = build_friend_status_raw(FriendResult::Enemy, 5, None);
        let mut expected = vec![0x0a]; // ENEMY
        expected.extend(5u64.to_le_bytes());
        assert_eq!(body, expected);
    }

    #[test]
    fn friend_list_response_truncates_to_255_entries() {
        // The wire count is a u8; feeding more than that must cap rather than silently wrap.
        let friends: Vec<FriendView> = (0..300)
            .map(|i| FriendView {
                guid: i as u64,
                online: false,
                away: crate::world::presence::AwayStatus::None,
                level: 0,
                class: 0,
                zone_id: 0,
            })
            .collect();
        let resp = build_friend_list_response(&friends);
        assert_eq!(resp.friends.len(), 255);
    }

    #[test]
    fn ignore_list_response_truncates_to_255_entries() {
        let ignored: Vec<u64> = (0..300).collect();
        let resp = build_ignore_list_response(&ignored);
        assert_eq!(resp.ignored.len(), 255);
    }
}

// ===========================================================================================
//  Party/group
// ===========================================================================================

use lyracore_shared::group::{GroupKind, RosterPayload};
use wow_world_messages::vanilla::{
    GroupListMember, GroupLootSetting, GroupType, ItemQuality, PartyOperation, PartyResult,
    SMSG_GROUP_LIST_group_not_empty, SMSG_GROUP_DECLINE, SMSG_GROUP_INVITE, SMSG_GROUP_LIST,
    SMSG_GROUP_SET_LEADER, SMSG_PARTY_COMMAND_RESULT,
};

/// `SMSG_GROUP_INVITE` — the target's "X invites you to a group" dialog.
pub fn build_group_invite(inviter_name: String) -> SMSG_GROUP_INVITE {
    SMSG_GROUP_INVITE { name: inviter_name }
}

/// `SMSG_GROUP_DECLINE` — the inviter's "X declines your group invitation" line.
pub fn build_group_decline(decliner_name: String) -> SMSG_GROUP_DECLINE {
    SMSG_GROUP_DECLINE {
        name: decliner_name,
    }
}

/// `SMSG_GROUP_SET_LEADER`: every member's "X is now the group leader" line
/// (cm:Group.cpp:498-500).
pub fn build_group_set_leader(leader_name: String) -> SMSG_GROUP_SET_LEADER {
    SMSG_GROUP_SET_LEADER { name: leader_name }
}

/// `SMSG_PARTY_COMMAND_RESULT` — the invite/leave outcome line ("X is already in a group" etc.).
pub fn build_party_command_result(
    operation: PartyOperation,
    member: String,
    result: PartyResult,
) -> SMSG_PARTY_COMMAND_RESULT {
    SMSG_PARTY_COMMAND_RESULT {
        operation,
        member,
        result,
    }
}

/// `SMSG_GROUP_LIST` for `self_guid` (cm:Group.cpp:680-708): the group type, the viewer's own Raid
/// Slot as `flags`, then the OTHER members (the recipient is implied) with their own Raid Slots, the
/// leader, and the loot block. `roster` is the FULL roster with names already resolved; the
/// recipient is filtered out here so every member can share one roster snapshot. `online` answers
/// presence per member guid.
///
/// `loot_method`/`loot_threshold` are the module's wire-matching `group::loot_method::*`/
/// `ItemQuality` byte values, read with a direct `try_from`, never a hand-written match table. An
/// out-of-range byte (the module validates both before storing, so this should not happen) falls back to
/// FreeForAll/Uncommon rather than failing the whole packet. As in cmangos, the loot block goes out
/// only when the list names another member.
pub fn build_group_list(
    self_guid: u64,
    roster: &RosterPayload,
    online: impl Fn(u64) -> bool,
) -> SMSG_GROUP_LIST {
    let own_flags = roster
        .members
        .iter()
        .find(|member| member.guid == self_guid)
        .map_or(0, |member| member.slot.wire());
    let others: Vec<GroupListMember> = roster
        .members
        .iter()
        .filter(|member| member.guid != self_guid)
        .map(|member| GroupListMember {
            name: member.name.clone(),
            guid: Guid::new(member.guid),
            is_online: online(member.guid),
            flags: member.slot.wire(),
        })
        .collect();
    let group_not_empty = (!others.is_empty()).then(|| SMSG_GROUP_LIST_group_not_empty {
        loot_setting: GroupLootSetting::try_from(roster.loot_method)
            .unwrap_or(GroupLootSetting::FreeForAll),
        master_loot: Guid::new(roster.master_looter_guid),
        loot_threshold: ItemQuality::try_from(roster.loot_threshold)
            .unwrap_or(ItemQuality::Uncommon),
    });
    SMSG_GROUP_LIST {
        group_type: match roster.kind {
            GroupKind::Party => GroupType::Normal,
            GroupKind::Raid => GroupType::Raid,
        },
        flags: own_flags,
        members: others,
        leader: Guid::new(roster.leader),
        group_not_empty,
    }
}

/// `SMSG_GROUP_LIST` as the 1.12 servers send it: `(opcode, body)` for `Outbound::Raw`. The body
/// is gtker's encoding of `list`, then, after the loot threshold, one more byte 0 that gtker has no
/// field for. cmangos writes it as "Heroic Mod Group - unused in vanilla" (cm:Group.cpp:705), and
/// vmangos as the dungeon difficulty for client builds after 1.10.2
/// (vm:Server/Packets/Group.cpp:258-259). Both send it only with the loot block.
pub fn build_group_list_raw(list: &SMSG_GROUP_LIST) -> (u16, Vec<u8>) {
    use wow_world_messages::vanilla::ServerMessage;
    let mut framed = Vec::new();
    list.write_unencrypted_server(&mut framed)
        .expect("writing to a Vec cannot fail");
    // The frame is `[size:u16 BE][opcode:u16 LE]` and then the body.
    let opcode = u16::from_le_bytes([framed[2], framed[3]]);
    let mut body = framed.split_off(4);
    if list.group_not_empty.is_some() {
        body.push(0);
    }
    (opcode, body)
}

// ===========================================================================================
//  Group Broadcasts
// ===========================================================================================

use lyracore_shared::group::TargetIcon;
use wow_world_messages::vanilla::{
    MSG_MINIMAP_PING_Server, MSG_RAID_READY_CHECK_Server, MSG_RAID_READY_CHECK_Server_state_check,
    MSG_RAID_TARGET_UPDATE_Server, RaidTargetIndex, RaidTargetUpdate,
};

/// `MSG_RAID_READY_CHECK` with no body: a Ready Check started (cm:GroupHandler.cpp:562-563).
pub fn build_ready_check() -> MSG_RAID_READY_CHECK_Server {
    MSG_RAID_READY_CHECK_Server { state_check: None }
}

/// `MSG_RAID_READY_CHECK` for the leader: one member's guid and answer state
/// (cm:GroupHandler.cpp:576-579).
pub fn build_ready_check_answer(member_guid: u64, state: u8) -> MSG_RAID_READY_CHECK_Server {
    MSG_RAID_READY_CHECK_Server {
        state_check: Some(MSG_RAID_READY_CHECK_Server_state_check {
            guid: Guid::new(member_guid),
            state,
        }),
    }
}

/// The partial `MSG_RAID_TARGET_UPDATE`: update type 0, the icon, the unit (cm:Group.cpp:595-600).
/// Guid 0 clears the icon. `None` for an icon outside 0 to 7.
pub fn build_target_icon_update(icon: TargetIcon) -> Option<MSG_RAID_TARGET_UPDATE_Server> {
    Some(MSG_RAID_TARGET_UPDATE_Server::Partial {
        raid_target: RaidTargetUpdate {
            index: target_icon_index(icon.icon)?,
            guid: Guid::new(icon.target_guid),
        },
    })
}

/// The full `MSG_RAID_TARGET_UPDATE` as cmangos and vmangos write it: update type 1, then each
/// held icon as `icon, guid`, in icon order (cm:Group.cpp:648-666). gtker's `Full` always carries 8
/// entries, so this is `(opcode, body)` for `Outbound::Raw`.
pub fn build_target_icon_list_raw(icons: &[TargetIcon]) -> (u16, Vec<u8>) {
    use wow_world_messages::Message;
    let opcode = u16::try_from(MSG_RAID_TARGET_UPDATE_Server::OPCODE)
        .expect("a vanilla opcode fits 16 bits");
    let mut held: Vec<&TargetIcon> = icons.iter().filter(|icon| icon.target_guid != 0).collect();
    held.sort_unstable_by_key(|icon| icon.icon);
    let mut body = vec![1];
    for icon in held {
        body.push(icon.icon);
        body.extend(icon.target_guid.to_le_bytes());
    }
    (opcode, body)
}

/// gtker names the 8 icons `Unknown0` to `Unknown7`. Its `Unknown8` is not an icon.
fn target_icon_index(icon: u8) -> Option<RaidTargetIndex> {
    (icon < lyracore_shared::group::TARGET_ICON_COUNT)
        .then(|| RaidTargetIndex::try_from(icon).ok())
        .flatten()
}

/// `MSG_MINIMAP_PING`: the pinger and the point on the minimap (cm:GroupHandler.cpp:410-413).
pub fn build_minimap_ping(pinger_guid: u64, x: f32, y: f32) -> MSG_MINIMAP_PING_Server {
    MSG_MINIMAP_PING_Server {
        guid: Guid::new(pinger_guid),
        position_x: x,
        position_y: y,
    }
}

use wow_world_messages::vanilla::{RaidGroupError, SMSG_RAID_GROUP_ONLY};

/// `SMSG_RAID_GROUP_ONLY`: the Instance Removal countdown in milliseconds, or 0 to hide it
/// (cm:Player.cpp:17697-17728). The error is `Required` (1) as vmangos sends
/// (vm:Player.cpp:18561-18588). cmangos sends 0, which gtker's `RaidGroupError` cannot encode.
pub fn build_raid_group_only(timer_ms: u32) -> SMSG_RAID_GROUP_ONLY {
    SMSG_RAID_GROUP_ONLY {
        homebind_timer: timer_ms,
        error: RaidGroupError::Required,
    }
}

#[cfg(test)]
mod party_tests {
    use super::*;

    #[test]
    fn who_response_raw_caps_listed_rows_at_49_but_reports_the_full_match_count() {
        // The 1.12 client won't render more than 49 rows, so the listing caps there — but
        // `online_players` must still report the FULL match count (before any cap), matching a real
        // server's "N players online" count even when the panel itself only shows 49.
        let players: Vec<WhoPlayerView> = (0..51)
            .map(|i| WhoPlayerView {
                name: format!("P{i}"),
                guild: String::new(),
                level: 10,
                class: 1,
                race: 1,
                zone_id: 12,
            })
            .collect();
        let (opcode, body) = build_who_response_raw(&players);
        assert_eq!(opcode, SMSG_WHO_OPCODE);
        let listed_players = u32::from_le_bytes(body[0..4].try_into().unwrap());
        let online_players = u32::from_le_bytes(body[4..8].try_into().unwrap());
        assert_eq!(
            listed_players, 49,
            "the listed rows must cap at the vanilla client's 49-row limit"
        );
        assert_eq!(
            online_players, 51,
            "the online count must be the full 51-player match, uncapped"
        );
        // 8 header bytes, then one row per listed player: name CString, empty guild CString, 4
        // u32 fields. Sums the actual (varying) name lengths rather than assuming one width.
        let expected_len: usize = 8 + players[..49]
            .iter()
            .map(|p| p.name.len() + 1 + 1 + 16)
            .sum::<usize>();
        assert_eq!(body.len(), expected_len);
    }

    #[test]
    fn who_response_raw_matches_the_cmangos_byte_layout() {
        // Hand-written from cm:MiscHandler.cpp:252-257: name, guild, level/class/race/zone each a
        // u32, no party_status. Two rows, so a real client reading past the first proves the
        // layout, not just the first row's offsets.
        let players = [
            WhoPlayerView {
                name: "Ginger".into(),
                guild: "Boundary Test".into(),
                level: 10,
                class: 1,
                race: 1,
                zone_id: 12,
            },
            WhoPlayerView {
                name: "Vim".into(),
                guild: String::new(),
                level: 60,
                class: 8,
                race: 2,
                zone_id: 1,
            },
        ];
        let (opcode, body) = build_who_response_raw(&players);
        assert_eq!(opcode, SMSG_WHO_OPCODE);
        let mut expected = Vec::new();
        expected.extend_from_slice(&2u32.to_le_bytes()); // listed_players
        expected.extend_from_slice(&2u32.to_le_bytes()); // online_players
        expected.extend_from_slice(b"Ginger\0");
        expected.extend_from_slice(b"Boundary Test\0"); // guild
        expected.extend_from_slice(&10u32.to_le_bytes()); // level
        expected.extend_from_slice(&1u32.to_le_bytes()); // class
        expected.extend_from_slice(&1u32.to_le_bytes()); // race
        expected.extend_from_slice(&12u32.to_le_bytes()); // zone
        expected.extend_from_slice(b"Vim\0");
        expected.extend_from_slice(b"\0"); // guild
        expected.extend_from_slice(&60u32.to_le_bytes()); // level
        expected.extend_from_slice(&8u32.to_le_bytes()); // class
        expected.extend_from_slice(&2u32.to_le_bytes()); // race
        expected.extend_from_slice(&1u32.to_le_bytes()); // zone
        assert_eq!(body, expected);
    }

    use lyracore_shared::group::{RaidSlot, RosterMember};

    fn roster(
        kind: GroupKind,
        leader: u64,
        loot_method: u8,
        loot_threshold: u8,
        master_looter_guid: u64,
        members: &[(u64, &str, RaidSlot)],
    ) -> RosterPayload {
        RosterPayload {
            leader,
            loot_method,
            loot_threshold,
            master_looter_guid,
            kind,
            members: members
                .iter()
                .map(|(guid, name, slot)| RosterMember {
                    guid: *guid,
                    name: name.to_string(),
                    slot: *slot,
                })
                .collect(),
            target_icons: Vec::new(),
        }
    }

    fn party(members: &[(u64, &str)]) -> RosterPayload {
        let members: Vec<_> = members
            .iter()
            .map(|(guid, name)| (*guid, *name, RaidSlot::default()))
            .collect();
        roster(
            GroupKind::Party,
            1,
            3, /* GROUP */
            2, /* Uncommon */
            0,
            &members,
        )
    }

    /// The sent body bytes, pinned field by field against cm:Group.cpp:680-708 and
    /// vm:Server/Packets/Group.cpp:237-261 for a viewer in Subgroup 1 with an online leader in
    /// Subgroup 0 and an offline Assistant in Subgroup 1.
    #[test]
    fn a_raid_group_list_carries_the_raid_type_and_every_members_raid_slot() {
        let raid = roster(
            GroupKind::Raid,
            10,
            3, // GROUP_LOOT
            2, // Uncommon
            0,
            &[
                (10, "Ab", RaidSlot::new(0, false).unwrap()),
                (11, "Me", RaidSlot::new(1, false).unwrap()),
                (12, "Cd", RaidSlot::new(1, true).unwrap()),
            ],
        );
        let (opcode, body) = build_group_list_raw(&build_group_list(11, &raid, |guid| guid == 10));
        assert_eq!(opcode, 0x007D);
        let expected: Vec<u8> = [
            &[0x01][..],          // group type: GROUP_FLAG_RAID
            &[0x01],              // own flags: Subgroup 1, no Assistant
            &2u32.to_le_bytes(),  // members other than the viewer
            b"Ab\0",              // name
            &10u64.to_le_bytes(), // guid
            &[0x01],              // status: online
            &[0x00],              // flags: Subgroup 0
            b"Cd\0",              // name
            &12u64.to_le_bytes(), // guid
            &[0x00],              // status: offline
            &[0x81],              // flags: Subgroup 1 | 0x80 Assistant
            &10u64.to_le_bytes(), // leader
            &[0x03],              // loot method
            &0u64.to_le_bytes(),  // master looter
            &[0x02],              // loot threshold
            &[0x00],              // unused in 1.x: cm "Heroic Mod Group", vm "dungeonDifficulty"
        ]
        .concat();
        assert_eq!(body, expected);
    }

    /// Without another member there is no loot block, and so no trailing byte either.
    #[test]
    fn a_sent_list_naming_only_the_viewer_ends_at_the_leader() {
        let (_, body) =
            build_group_list_raw(&build_group_list(1, &party(&[(1, "Self")]), |_| true));
        let expected: Vec<u8> = [
            &[0x00][..],         // group type: GROUP_FLAG_NORMAL
            &[0x00],             // own flags
            &0u32.to_le_bytes(), // no other member
            &1u64.to_le_bytes(), // leader
        ]
        .concat();
        assert_eq!(body, expected);
    }

    #[test]
    fn a_party_group_list_stays_normal_with_every_flags_byte_zero() {
        let list = build_group_list(1, &party(&[(1, "Self"), (2, "Bob")]), |_| true);
        assert_eq!(list.group_type, GroupType::Normal);
        assert_eq!(list.flags, 0);
        assert!(list.members.iter().all(|member| member.flags == 0));
    }

    /// cm:Group.cpp:700: the loot block follows the leader only when another member is listed.
    #[test]
    fn a_list_naming_only_the_viewer_carries_no_loot_block() {
        let list = build_group_list(1, &party(&[(1, "Self")]), |_| true);
        assert!(list.members.is_empty());
        assert_eq!(list.group_not_empty, None);
    }

    #[test]
    fn group_list_excludes_self_but_keeps_the_leader_field() {
        let members = party(&[(1, "Self"), (2, "Bob"), (3, "Carol")]);
        let resp = build_group_list(1, &members, |guid| guid != 3);
        assert_eq!(
            resp.members.len(),
            2,
            "the recipient's own row must be excluded from the roster"
        );
        assert!(
            resp.members.iter().all(|m| m.guid.guid() != 1),
            "self must never appear among the members"
        );
        assert_eq!(
            resp.leader.guid(),
            1,
            "the leader field is independent of the self-exclusion filter"
        );
        let bob = resp
            .members
            .iter()
            .find(|m| m.guid.guid() == 2)
            .expect("Bob must be listed");
        assert_eq!(bob.name, "Bob");
        assert!(bob.is_online);
        let carol = resp
            .members
            .iter()
            .find(|m| m.guid.guid() == 3)
            .expect("Carol must be listed");
        assert!(!carol.is_online);
    }

    /// The loot block now carries the group's REAL current method/threshold/master
    /// (was hardcoded FreeForAll/Uncommon) — a direct `try_from` off the module's wire-matching byte
    /// values, with a safe fallback for an (unreachable, module-validated) out-of-range byte.
    #[test]
    fn group_list_loot_block_carries_the_real_method_threshold_and_master() {
        let slot = RaidSlot::default();
        let members = [(1, "Self", slot), (2, "Bob", slot)];
        let master = roster(
            GroupKind::Party,
            1,
            2, /* MASTER */
            4, /* Epic */
            2,
            &members,
        );
        let resp = build_group_list(1, &master, |_| true);
        let block = resp
            .group_not_empty
            .expect("group_not_empty must be Some for a real party");
        assert_eq!(block.loot_setting, GroupLootSetting::MasterLoot);
        assert_eq!(block.loot_threshold, ItemQuality::Epic);
        assert_eq!(block.master_loot.guid(), 2);
        // An out-of-range byte (shouldn't happen post module-validation) degrades safely rather than
        // failing the whole packet.
        let fallback = build_group_list(
            1,
            &roster(GroupKind::Party, 1, 255, 255, 0, &members),
            |_| true,
        );
        let fallback_block = fallback.group_not_empty.expect("still Some");
        assert_eq!(fallback_block.loot_setting, GroupLootSetting::FreeForAll);
        assert_eq!(fallback_block.loot_threshold, ItemQuality::Uncommon);
    }

    /// cm:Group.cpp:498-499 writes the leader's name as one CString: opcode 0x0079, then the name
    /// bytes and a 0.
    #[test]
    fn the_set_leader_packet_is_the_leader_name_as_one_cstring() {
        use wow_world_messages::vanilla::ServerMessage;
        let mut framed = Vec::new();
        build_group_set_leader("Ginger".to_string())
            .write_unencrypted_server(&mut framed)
            .unwrap();
        let body: &[u8] = b"Ginger\0";
        let expected: Vec<u8> = [
            &((body.len() + 2) as u16).to_be_bytes()[..], // size: opcode + body
            &0x0079u16.to_le_bytes(),                     // SMSG_GROUP_SET_LEADER
            body,
        ]
        .concat();
        assert_eq!(framed, expected);
    }

    /// cm:Player.h:309-314 and gtker: opcode 0x0286, then `u32 homebind_timer` and `u32 error`.
    #[test]
    fn the_raid_group_only_packet_is_the_countdown_then_the_error() {
        use wow_world_messages::vanilla::ServerMessage;
        for (timer_ms, expected) in [
            (
                60_000,
                [
                    0x00, 0x0A, 0x86, 0x02, 0x60, 0xEA, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
                ],
            ),
            (
                0,
                [
                    0x00, 0x0A, 0x86, 0x02, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
                ],
            ),
        ] {
            let mut framed = Vec::new();
            build_raid_group_only(timer_ms)
                .write_unencrypted_server(&mut framed)
                .unwrap();
            assert_eq!(framed, expected, "timer {timer_ms}");
        }
    }

    #[test]
    fn group_invite_decline_and_command_result_carry_their_payload() {
        let invite = build_group_invite("Bob".to_string());
        assert_eq!(invite.name, "Bob");
        let decline = build_group_decline("Carol".to_string());
        assert_eq!(decline.name, "Carol");
        let result = build_party_command_result(
            PartyOperation::Invite,
            "Dave".to_string(),
            PartyResult::AlreadyInGroup,
        );
        assert_eq!(result.operation, PartyOperation::Invite);
        assert_eq!(result.member, "Dave");
        assert_eq!(result.result, PartyResult::AlreadyInGroup);
    }
}
