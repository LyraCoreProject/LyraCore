use lyracore_shared::chat::broadcast_chat;
use wow_world_base::shared::friend_result_vanilla_tbc::FriendResult;
use wow_world_messages::vanilla::{
    Area, Class, Emote, Friend, Friend_FriendStatus, Language, Level, MSG_RANDOM_ROLL_Server,
    PlayerChatTag, SMSG_MESSAGECHAT_ChatType, TextEmote, SMSG_EMOTE, SMSG_FRIEND_LIST,
    SMSG_IGNORE_LIST, SMSG_MESSAGECHAT, SMSG_TEXT_EMOTE,
};
use wow_world_messages::Guid;

pub struct WhoPlayerView {
    pub name: String,
    pub guild: String,
    pub level: u8,
    pub class: u8,
    pub race: u8,
    pub zone_id: u32,
}

pub const SMSG_WHO_OPCODE: u16 = 0x0063;

fn push_cstr(body: &mut Vec<u8>, s: &str) {
    body.extend_from_slice(s.as_bytes());
    body.push(0);
}

/// Build 5875 uses u32 class/race/zone and no party_status; gtker's row is two bytes short.
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

pub struct FriendView {
    pub guid: u64,
    pub online: bool,
    pub away: crate::AwayStatus,
    pub level: u8,
    pub class: u8,
    pub zone_id: u32,
}

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
                        crate::AwayStatus::None => {
                            Friend_FriendStatus::Online { area, class, level }
                        }
                        crate::AwayStatus::Afk => Friend_FriendStatus::Afk { area, class, level },
                        crate::AwayStatus::Dnd => Friend_FriendStatus::Dnd { area, class, level },
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

pub fn build_ignore_list_response(ignored: &[u64]) -> SMSG_IGNORE_LIST {
    SMSG_IGNORE_LIST {
        ignored: ignored.iter().take(255).copied().collect(),
    }
}

pub const SMSG_FRIEND_STATUS_OPCODE: u16 = 0x0068;

#[derive(Clone)]
pub struct FriendOnline {
    pub away: crate::AwayStatus,
    pub zone_id: u32,
    pub level: u8,
    pub class: u8,
}

impl FriendOnline {
    fn status_byte(&self) -> u8 {
        match self.away {
            crate::AwayStatus::None => 1,
            crate::AwayStatus::Afk => 2,
            crate::AwayStatus::Dnd => 4,
        }
    }
}

/// gtker omits the status/area/level/class tail required for ONLINE and ADDED_ONLINE.
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

#[cfg(test)]
fn build_chat_message(
    sender_guid: u64,
    sender_name: Option<String>,
    chat_type: u8,
    language: u8,
    message: String,
) -> SMSG_MESSAGECHAT {
    build_chat_message_to(sender_guid, sender_name, 0, chat_type, language, message, 0)
}

pub fn build_chat_message_to(
    sender_guid: u64,
    sender_name: Option<String>,
    target_guid: u64,
    chat_type: u8,
    language: u8,
    message: String,
    chat_tag: u8,
) -> SMSG_MESSAGECHAT {
    let sender = Guid::new(sender_guid);
    let kind = match (chat_type, sender_name) {
        (0, Some(sender_name)) => SMSG_MESSAGECHAT_ChatType::MonsterSay {
            sender1: sender,
            sender_name,
            target: Guid::new(target_guid),
        },
        (1 | broadcast_chat::CREATURE_ZONE_YELL, Some(sender_name)) => {
            SMSG_MESSAGECHAT_ChatType::MonsterYell {
                sender1: sender,
                sender_name,
                target: Guid::new(target_guid),
            }
        }
        // MonsterEmote's sole guid names the speaker; it has no addressee field.
        (broadcast_chat::CREATURE_WHISPER, Some(monster_name)) => {
            SMSG_MESSAGECHAT_ChatType::MonsterWhisper {
                monster: sender,
                monster_name,
            }
        }
        (broadcast_chat::CREATURE_BOSS_EMOTE, Some(monster_name)) => {
            SMSG_MESSAGECHAT_ChatType::RaidBossEmote {
                monster: sender,
                monster_name,
            }
        }
        (
            broadcast_chat::CREATURE_TEXT_EMOTE | broadcast_chat::CREATURE_ZONE_EMOTE,
            Some(monster_name),
        ) => SMSG_MESSAGECHAT_ChatType::MonsterEmote {
            monster: sender,
            monster_name,
        },
        (1, None) => SMSG_MESSAGECHAT_ChatType::Yell {
            chat_credit: sender,
            speech_bubble_credit: sender,
        },
        (broadcast_chat::CREATURE_TEXT_EMOTE, None) => SMSG_MESSAGECHAT_ChatType::MonsterEmote {
            monster: sender,
            monster_name: String::new(),
        },
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
            broadcast_chat::CREATURE_TEXT_EMOTE
                | broadcast_chat::CREATURE_BOSS_EMOTE
                | broadcast_chat::CREATURE_WHISPER
                | broadcast_chat::EMOTE
        ) {
            Language::Universal
        } else {
            Language::try_from(language).unwrap_or(Language::Universal)
        },
        message,
        tag: PlayerChatTag::try_from(chat_tag).unwrap_or(PlayerChatTag::None),
    }
}

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
            0,
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
            0,
        );
        match &emote.chat_type {
            SMSG_MESSAGECHAT_ChatType::MonsterEmote {
                monster,
                monster_name,
            } => {
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
    fn creature_speech_keeps_its_wire_kinds_and_languages() {
        let make = |kind| {
            build_chat_message_to(
                11,
                Some("Herald".into()),
                77,
                kind,
                7,
                "Attention".into(),
                0,
            )
        };
        let whisper = make(broadcast_chat::CREATURE_WHISPER);
        assert!(
            matches!(whisper.chat_type, SMSG_MESSAGECHAT_ChatType::MonsterWhisper { monster, .. } if monster.guid() == 11)
        );
        assert_eq!(whisper.language, Language::Universal);
        let boss = make(broadcast_chat::CREATURE_BOSS_EMOTE);
        assert!(matches!(
            boss.chat_type,
            SMSG_MESSAGECHAT_ChatType::RaidBossEmote { .. }
        ));
        assert_eq!(boss.language, Language::Universal);
        let yell = make(broadcast_chat::CREATURE_ZONE_YELL);
        assert!(
            matches!(yell.chat_type, SMSG_MESSAGECHAT_ChatType::MonsterYell { target, .. } if target.guid() == 77)
        );
        assert_eq!(yell.language, Language::Common);
        let emote = make(broadcast_chat::CREATURE_ZONE_EMOTE);
        assert!(matches!(
            emote.chat_type,
            SMSG_MESSAGECHAT_ChatType::MonsterEmote { .. }
        ));
        assert_eq!(emote.language, Language::Common);
    }

    #[test]
    fn player_emote_row_builds_emote_in_universal_and_round_trips() {
        let emote = build_chat_message_to(
            7,
            None,
            0,
            broadcast_chat::EMOTE,
            1,
            "waves wildly.".into(),
            0,
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
    fn an_afk_players_say_ends_with_the_afk_tag() {
        let say = build_chat_message_to(7, None, 0, 0, 7, "hi".into(), 1);
        let mut wire = Vec::new();
        say.write_unencrypted_server(&mut wire).unwrap();
        let body = [
            0x00, 0x07, 0x00, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, b'h', b'i', 0x00,
            0x01,
        ];
        assert!(wire.ends_with(&body), "wire {wire:02x?}");
        assert_eq!(say.tag, PlayerChatTag::Afk);
        let dnd = build_chat_message_to(7, None, 0, 1, 7, "hi".into(), 2);
        assert_eq!(dnd.tag, PlayerChatTag::Dnd);
        let creature = build_chat_message_to(7, Some("Thug".into()), 0, 0, 0, "hi".into(), 9);
        assert_eq!(creature.tag, PlayerChatTag::None);
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
        let te = build_text_emote(5, 1, 10, None).expect("text_emote id 1 is known");
        assert_eq!(te.guid.guid(), 5);
        assert!(te.name.is_empty());
        let mut buf = Vec::new();
        te.write_unencrypted_server(&mut buf).unwrap();
        assert!(!buf.is_empty());
        assert!(build_text_emote(5, u32::MAX, 0, None).is_none());

        let targeted =
            build_text_emote(5, 1, 10, Some("Bob".to_string())).expect("text_emote id 1 is known");
        assert_eq!(targeted.name, "Bob");

        let anim = build_emote_anim(5, 10).expect("Emote id 10 is known");
        assert_eq!(anim.guid.guid(), 5);
        assert!(build_emote_anim(5, u32::MAX).is_none());
    }

    fn online_friend(guid: u64, away: crate::AwayStatus) -> FriendView {
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
            online_friend(11, crate::AwayStatus::None),
            FriendView {
                guid: 22,
                online: false,
                away: crate::AwayStatus::None,
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
    fn friend_list_online_status_follows_away_status() {
        use crate::AwayStatus;

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
        let friends = [FriendView {
            guid: 5,
            online: true,
            away: crate::AwayStatus::None,
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
    fn friend_status_raw_is_byte_exact_for_added_online() {
        let (opcode, body) = build_friend_status_raw(
            FriendResult::AddedOnline,
            77,
            Some(FriendOnline {
                away: crate::AwayStatus::None,
                zone_id: 12,
                level: 30,
                class: 1,
            }),
        );
        assert_eq!(opcode, SMSG_FRIEND_STATUS_OPCODE);
        let mut expected = vec![0x06];
        expected.extend(77u64.to_le_bytes());
        expected.push(1);
        expected.extend(12u32.to_le_bytes());
        expected.extend(30u32.to_le_bytes());
        expected.extend(1u32.to_le_bytes());
        assert_eq!(body, expected);
    }

    #[test]
    fn friend_status_raw_status_byte_follows_away_status() {
        for (away, status) in [
            (crate::AwayStatus::None, 1u8),
            (crate::AwayStatus::Afk, 2),
            (crate::AwayStatus::Dnd, 4),
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

    #[test]
    fn friend_status_raw_carries_no_trailing_fields_without_online() {
        let (_, body) = build_friend_status_raw(FriendResult::Enemy, 5, None);
        let mut expected = vec![0x0a];
        expected.extend(5u64.to_le_bytes());
        assert_eq!(body, expected);
    }

    #[test]
    fn friend_list_response_truncates_to_255_entries() {
        let friends: Vec<FriendView> = (0..300)
            .map(|i| FriendView {
                guid: i as u64,
                online: false,
                away: crate::AwayStatus::None,
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

use lyracore_shared::group::{GroupKind, RosterPayload};
use wow_world_messages::vanilla::{
    GroupListMember, GroupLootSetting, GroupType, ItemQuality, PartyOperation, PartyResult,
    SMSG_GROUP_LIST_group_not_empty, SMSG_GROUP_DECLINE, SMSG_GROUP_INVITE, SMSG_GROUP_LIST,
    SMSG_GROUP_SET_LEADER, SMSG_PARTY_COMMAND_RESULT,
};

pub fn build_group_invite(inviter_name: String) -> SMSG_GROUP_INVITE {
    SMSG_GROUP_INVITE { name: inviter_name }
}

pub fn build_group_decline(decliner_name: String) -> SMSG_GROUP_DECLINE {
    SMSG_GROUP_DECLINE {
        name: decliner_name,
    }
}

pub fn build_group_set_leader(leader_name: String) -> SMSG_GROUP_SET_LEADER {
    SMSG_GROUP_SET_LEADER { name: leader_name }
}

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

/// The loot block needs a trailing zero difficulty byte that gtker omits.
pub fn build_group_list_raw(list: &SMSG_GROUP_LIST) -> (u16, Vec<u8>) {
    use wow_world_messages::vanilla::ServerMessage;
    let mut framed = Vec::new();
    list.write_unencrypted_server(&mut framed)
        .expect("writing to a Vec cannot fail");
    let opcode = u16::from_le_bytes([framed[2], framed[3]]);
    let mut body = framed.split_off(4);
    if list.group_not_empty.is_some() {
        body.push(0);
    }
    (opcode, body)
}

use lyracore_shared::group::TargetIcon;
use wow_world_messages::vanilla::{
    MSG_MINIMAP_PING_Server, MSG_RAID_READY_CHECK_Server, MSG_RAID_READY_CHECK_Server_state_check,
    MSG_RAID_TARGET_UPDATE_Server, RaidTargetIndex, RaidTargetUpdate,
};

pub fn build_ready_check() -> MSG_RAID_READY_CHECK_Server {
    MSG_RAID_READY_CHECK_Server { state_check: None }
}

pub fn build_ready_check_answer(member_guid: u64, state: u8) -> MSG_RAID_READY_CHECK_Server {
    MSG_RAID_READY_CHECK_Server {
        state_check: Some(MSG_RAID_READY_CHECK_Server_state_check {
            guid: Guid::new(member_guid),
            state,
        }),
    }
}

pub fn build_target_icon_update(icon: TargetIcon) -> Option<MSG_RAID_TARGET_UPDATE_Server> {
    Some(MSG_RAID_TARGET_UPDATE_Server::Partial {
        raid_target: RaidTargetUpdate {
            index: target_icon_index(icon.icon)?,
            guid: Guid::new(icon.target_guid),
        },
    })
}

/// Send only held raid icons; gtker's Full type always emits eight entries.
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

fn target_icon_index(icon: u8) -> Option<RaidTargetIndex> {
    (icon < lyracore_shared::group::TARGET_ICON_COUNT)
        .then(|| RaidTargetIndex::try_from(icon).ok())
        .flatten()
}

pub fn build_minimap_ping(pinger_guid: u64, x: f32, y: f32) -> MSG_MINIMAP_PING_Server {
    MSG_MINIMAP_PING_Server {
        guid: Guid::new(pinger_guid),
        position_x: x,
        position_y: y,
    }
}

use wow_world_messages::vanilla::{RaidGroupError, SMSG_RAID_GROUP_ONLY};

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
        let expected_len: usize = 8 + players[..49]
            .iter()
            .map(|p| p.name.len() + 1 + 1 + 16)
            .sum::<usize>();
        assert_eq!(body.len(), expected_len);
    }

    #[test]
    fn who_response_raw_matches_the_cmangos_byte_layout() {
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
        expected.extend_from_slice(&2u32.to_le_bytes());
        expected.extend_from_slice(&2u32.to_le_bytes());
        expected.extend_from_slice(b"Ginger\0");
        expected.extend_from_slice(b"Boundary Test\0");
        expected.extend_from_slice(&10u32.to_le_bytes());
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend_from_slice(&1u32.to_le_bytes());
        expected.extend_from_slice(&12u32.to_le_bytes());
        expected.extend_from_slice(b"Vim\0");
        expected.extend_from_slice(b"\0");
        expected.extend_from_slice(&60u32.to_le_bytes());
        expected.extend_from_slice(&8u32.to_le_bytes());
        expected.extend_from_slice(&2u32.to_le_bytes());
        expected.extend_from_slice(&1u32.to_le_bytes());
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
        roster(GroupKind::Party, 1, 3, 2, 0, &members)
    }

    #[test]
    fn a_raid_group_list_carries_the_raid_type_and_every_members_raid_slot() {
        let raid = roster(
            GroupKind::Raid,
            10,
            3,
            2,
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
            &[0x01][..],
            &[0x01],
            &2u32.to_le_bytes(),
            b"Ab\0",
            &10u64.to_le_bytes(),
            &[0x01],
            &[0x00],
            b"Cd\0",
            &12u64.to_le_bytes(),
            &[0x00],
            &[0x81],
            &10u64.to_le_bytes(),
            &[0x03],
            &0u64.to_le_bytes(),
            &[0x02],
            &[0x00],
        ]
        .concat();
        assert_eq!(body, expected);
    }

    #[test]
    fn a_sent_list_naming_only_the_viewer_ends_at_the_leader() {
        let (_, body) =
            build_group_list_raw(&build_group_list(1, &party(&[(1, "Self")]), |_| true));
        let expected: Vec<u8> = [
            &[0x00][..],
            &[0x00],
            &0u32.to_le_bytes(),
            &1u64.to_le_bytes(),
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

    #[test]
    fn group_list_loot_block_carries_the_real_method_threshold_and_master() {
        let slot = RaidSlot::default();
        let members = [(1, "Self", slot), (2, "Bob", slot)];
        let master = roster(GroupKind::Party, 1, 2, 4, 2, &members);
        let resp = build_group_list(1, &master, |_| true);
        let block = resp
            .group_not_empty
            .expect("group_not_empty must be Some for a real party");
        assert_eq!(block.loot_setting, GroupLootSetting::MasterLoot);
        assert_eq!(block.loot_threshold, ItemQuality::Epic);
        assert_eq!(block.master_loot.guid(), 2);
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
    fn the_set_leader_packet_is_the_leader_name_as_one_cstring() {
        use wow_world_messages::vanilla::ServerMessage;
        let mut framed = Vec::new();
        build_group_set_leader("Ginger".to_string())
            .write_unencrypted_server(&mut framed)
            .unwrap();
        let body: &[u8] = b"Ginger\0";
        let expected: Vec<u8> = [
            &((body.len() + 2) as u16).to_be_bytes()[..],
            &0x0079u16.to_le_bytes(),
            body,
        ]
        .concat();
        assert_eq!(framed, expected);
    }

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
