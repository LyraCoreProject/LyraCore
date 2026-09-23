//! Social-tier wire builders — `SMSG_MESSAGECHAT` (say/yell/whisper) + `SMSG_TEXT_EMOTE` /
//! `SMSG_EMOTE` (emotes) + `SMSG_WHO` (/who panel). Split out of `codec` (a 1.7k-line module) so
//! the say/yell/whisper/emote/who group lives in one focused place; re-exported from `codec`.

use wow_world_base::shared::friend_result_vanilla_tbc::FriendResult;
use wow_world_messages::vanilla::{
    Area, Class, Emote, Friend, Friend_FriendStatus, Language, Level, MSG_RANDOM_ROLL_Server,
    PlayerChatTag, Race, SMSG_MESSAGECHAT_ChatType, TextEmote, WhoPlayer, SMSG_EMOTE,
    SMSG_FRIEND_LIST, SMSG_FRIEND_STATUS, SMSG_IGNORE_LIST, SMSG_MESSAGECHAT, SMSG_TEXT_EMOTE,
    SMSG_WHO,
};
use wow_world_messages::Guid;

/// Internal `game_chat_event` mode for a creature-authored text emote.
pub(crate) const CHAT_TEXT_EMOTE: u8 = 2;

// ─── /who panel ──────────────────────────────────────────────────────────────

/// One online character row as needed by the WHO response — flat ints the codec converts.
pub struct WhoPlayerView {
    pub name: String,
    pub level: u8,
    pub class: u8,
    pub race: u8,
    pub zone_id: u32,
}

/// Build `SMSG_WHO` for `CMSG_WHO`. Filters are ignored for the first pass — every currently-online
/// character is listed (up to 49, the vanilla client cap). `online_count` is the total roster size
/// (before any cap). Zone ids that aren't in the gtker `Area` enum degrade to `Area::None`.
pub fn build_who_response(players: &[WhoPlayerView]) -> SMSG_WHO {
    let listed: Vec<WhoPlayer> = players
        .iter()
        .take(49) // vanilla client won't render more than 49 rows
        .filter_map(|p| {
            let class = Class::try_from(p.class).ok()?;
            let race = Race::try_from(p.race).ok()?;
            Some(WhoPlayer {
                name: p.name.clone(),
                guild: String::new(), // no guild system yet
                level: Level::new(p.level),
                class,
                race,
                area: Area::try_from(p.zone_id).unwrap_or(Area::None),
                party_status: 0, // not in a party
            })
        })
        .collect();
    SMSG_WHO {
        online_players: players.len() as u32,
        players: listed,
    }
}

// ─── Friends / ignore list ─────────────────────────────────────────────────

/// One friend row as needed by `SMSG_FRIEND_LIST` — flat ints the codec converts. `online` false
/// means the row degrades to `Friend_FriendStatus::Offline` (no area/level/class carried, matching
/// the wire format — those fields are ONLY present for a non-offline status).
pub struct FriendView {
    pub guid: u64,
    pub online: bool,
    pub level: u8,
    pub class: u8,
    pub zone_id: u32,
}

/// Build `SMSG_FRIEND_LIST` for `CMSG_FRIEND_LIST`. An online friend carries area/level/class (zone
/// ids outside the gtker `Area` enum degrade to `Area::None`, an unknown class degrades the WHOLE
/// row to `Offline` — better an inert row than a malformed one); an offline friend carries only the
/// guid. Capped at 255 (the wire's `u8` count prefix).
pub fn build_friend_list_response(friends: &[FriendView]) -> SMSG_FRIEND_LIST {
    let list = friends
        .iter()
        .take(255)
        .map(|f| {
            let status = if f.online {
                Class::try_from(f.class)
                    .ok()
                    .map(|class| Friend_FriendStatus::Online {
                        area: Area::try_from(f.zone_id).unwrap_or(Area::None),
                        class,
                        level: Level::new(f.level),
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

/// Build `SMSG_FRIEND_STATUS` — the add/remove confirmation (or rejection) for one contact-list
/// action. `result` is the vanilla `FriendResult` discriminant (`ADDED_ONLINE`/`ADDED_OFFLINE`/
/// `REMOVED`/`IGNORE_ADDED`/`IGNORE_REMOVED`/`NOT_FOUND`/`ALREADY`/… — the client renders the right
/// system message off it); `guid` is the OTHER party (0 when the action failed to resolve a name).
pub fn build_friend_status(result: FriendResult, guid: u64) -> SMSG_FRIEND_STATUS {
    SMSG_FRIEND_STATUS {
        result,
        guid: Guid::new(guid),
    }
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

/// Build creature speech while retaining the addressed target chosen by EventAI.
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
        (CHAT_TEXT_EMOTE, Some(monster_name)) => SMSG_MESSAGECHAT_ChatType::MonsterEmote {
            monster: sender,
            monster_name,
        },
        (1, None) => SMSG_MESSAGECHAT_ChatType::Yell {
            chat_credit: sender,
            speech_bubble_credit: sender,
        },
        (CHAT_TEXT_EMOTE, None) => SMSG_MESSAGECHAT_ChatType::MonsterEmote {
            monster: sender,
            monster_name: String::new(),
        },
        _ => SMSG_MESSAGECHAT_ChatType::Say {
            chat_credit: sender,
            speech_bubble_credit: sender,
        },
    };
    SMSG_MESSAGECHAT {
        chat_type: kind,
        language: if chat_type == CHAT_TEXT_EMOTE {
            Language::Universal
        } else {
            Language::try_from(language).unwrap_or(Language::Universal)
        },
        message,
        tag: PlayerChatTag::None,
    }
}

/// Build `SMSG_MESSAGECHAT` for a CHANNEL line: ChatType::Channel carries the channel's
/// display name (as the sender's client spelled it — the client matches it to the joined tab),
/// the speaking player's guid (name via NAME_QUERY), and rank 0 (moderator ranks are out of scope).
pub fn build_channel_message(
    sender_guid: u64,
    channel_display: String,
    message: String,
) -> SMSG_MESSAGECHAT {
    SMSG_MESSAGECHAT {
        chat_type: SMSG_MESSAGECHAT_ChatType::Channel {
            channel_name: channel_display,
            player: Guid::new(sender_guid),
            player_rank: 0,
        },
        language: Language::try_from(0u8).unwrap_or(Language::Universal),
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

/// Build `MSG_RANDOM_ROLL_Server` — the `/roll` result broadcast to all nearby players.
/// `minimum` / `maximum` are the clamped range from the roller's request; `actual_roll` is the
/// server-computed result; `guid` is the roller. All values echo what the module committed.
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
            CHAT_TEXT_EMOTE,
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

    #[test]
    fn friend_list_online_carries_presence_offline_is_bare() {
        let friends = [
            FriendView {
                guid: 11,
                online: true,
                level: 30,
                class: 1,
                zone_id: 12,
            },
            FriendView {
                guid: 22,
                online: false,
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

    #[test]
    fn friend_list_unknown_class_degrades_whole_row_to_offline() {
        // An online row with an unmapped class discriminant can't build a valid Online variant —
        // degrade the WHOLE row to Offline rather than send a malformed one.
        let friends = [FriendView {
            guid: 5,
            online: true,
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

    #[test]
    fn friend_status_carries_result_and_guid() {
        let resp = build_friend_status(FriendResult::AddedOnline, 77);
        assert_eq!(resp.result, FriendResult::AddedOnline);
        assert_eq!(resp.guid.guid(), 77);
        let mut buf = Vec::new();
        resp.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
    }

    #[test]
    fn friend_list_response_truncates_to_255_entries() {
        // The wire count is a u8; feeding more than that must cap rather than silently wrap.
        let friends: Vec<FriendView> = (0..300)
            .map(|i| FriendView {
                guid: i as u64,
                online: false,
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
    SMSG_PARTY_COMMAND_RESULT,
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
/// `ItemQuality` byte values — a direct `try_from`, never a hand-written match table. An
/// out-of-range byte (shouldn't happen — the module validates both before storing) falls back to
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

#[cfg(test)]
mod party_tests {
    use super::*;

    #[test]
    fn who_response_caps_listed_rows_at_49_but_reports_the_full_roster_count() {
        // The 1.12 client won't render more than 49 rows, so `players` is capped there — but
        // `online_players` must still report the FULL roster size (before any cap), matching a real
        // server's "N players online" count even when the panel itself only shows 49.
        let players: Vec<WhoPlayerView> = (0..51)
            .map(|i| WhoPlayerView {
                name: format!("P{i}"),
                level: 10,
                class: 1,
                race: 1,
                zone_id: 12,
            })
            .collect();
        let resp = build_who_response(&players);
        assert_eq!(
            resp.online_players, 51,
            "the online count must be the full 51-player roster"
        );
        assert_eq!(
            resp.players.len(),
            49,
            "the listed rows must cap at the vanilla client's 49-row limit"
        );
    }

    #[test]
    fn who_response_skips_rows_with_an_unmapped_class_or_race_without_shrinking_the_count() {
        let players = [
            WhoPlayerView {
                name: "Good".into(),
                level: 10,
                class: 1,
                race: 1,
                zone_id: 12,
            },
            WhoPlayerView {
                name: "BadClass".into(),
                level: 10,
                class: 250,
                race: 1,
                zone_id: 12,
            },
            WhoPlayerView {
                name: "BadRace".into(),
                level: 10,
                class: 1,
                race: 250,
                zone_id: 12,
            },
        ];
        let resp = build_who_response(&players);
        assert_eq!(
            resp.players.len(),
            1,
            "rows with an unmapped class/race must be skipped"
        );
        assert_eq!(resp.players[0].name, "Good");
        assert_eq!(
            resp.online_players, 3,
            "the count reflects the full roster, unaffected by the skip"
        );
    }

    use lyracore_shared::group::{RaidSlot, RosterMember};
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

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

    /// The body bytes, pinned field by field against cm:Group.cpp:680-708 for a viewer in
    /// Subgroup 1 with an online leader in Subgroup 0 and an offline Assistant in Subgroup 1.
    /// cmangos appends one more byte, commented "Heroic Mod Group - unused in vanilla"; gtker does
    /// not encode it and LyraCore has never sent it.
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
        let list = build_group_list(11, &raid, |guid| guid == 10);
        let mut framed = Vec::new();
        ServerOpcodeMessage::SMSG_GROUP_LIST(Box::new(list))
            .write_unencrypted_server(&mut framed)
            .unwrap();
        assert_eq!(u16::from_le_bytes([framed[2], framed[3]]), 0x007D);
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
        ]
        .concat();
        assert_eq!(&framed[4..], expected.as_slice());
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
