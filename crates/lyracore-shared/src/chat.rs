use crate::channel::ChannelRefusal;
use crate::constants::player_flags;

// ChatMsg wire values, cm:SharedDefines.h:1545-1582.
pub mod chat_kind {
    pub const SAY: u8 = 0x00;
    pub const PARTY: u8 = 0x01;
    pub const RAID: u8 = 0x02;
    pub const GUILD: u8 = 0x03;
    pub const OFFICER: u8 = 0x04;
    pub const YELL: u8 = 0x05;
    pub const WHISPER: u8 = 0x06;
    pub const WHISPER_INFORM: u8 = 0x07;
    pub const EMOTE: u8 = 0x08;
    pub const CHANNEL: u8 = 0x0E;
    pub const AFK: u8 = 0x14;
    pub const DND: u8 = 0x15;
    pub const IGNORED: u8 = 0x16;
    pub const RAID_LEADER: u8 = 0x57;
    pub const RAID_WARNING: u8 = 0x58;
}

// Last byte of SMSG_MESSAGECHAT, cm:Chat.h:70-74.
pub mod chat_tag {
    pub const NONE: u8 = 0;
    pub const AFK: u8 = 1;
    pub const DND: u8 = 2;
}

// game_chat_event uses its own numbering, distinct from ChatMsg wire values.
pub mod broadcast_chat {
    pub const SAY: u8 = 0;
    pub const YELL: u8 = 1;
    pub const CREATURE_TEXT_EMOTE: u8 = 2;
    pub const EMOTE: u8 = 3;
    pub const CREATURE_BOSS_EMOTE: u8 = 4;
    pub const CREATURE_WHISPER: u8 = 5;
    pub const CREATURE_ZONE_YELL: u8 = 6;
    pub const CREATURE_ZONE_EMOTE: u8 = 7;

    pub fn is_zone_wide(chat_type: u8) -> bool {
        matches!(chat_type, CREATURE_ZONE_YELL | CREATURE_ZONE_EMOTE)
    }

    // Text emotes use ListenRange.TextEmote, equal to ListenRange.Say.
    pub const TEXT_EMOTE_RANGE_YD: f32 = 25.0;
}

// Build-5875 Language wire values.
pub mod language {
    pub const UNIVERSAL: u32 = 0;
    pub const ORCISH: u32 = 1;
    pub const DARNASSIAN: u32 = 2;
    pub const TAURAHE: u32 = 3;
    pub const DWARVISH: u32 = 6;
    pub const COMMON: u32 = 7;
    pub const DEMONIC: u32 = 8;
    pub const TITAN: u32 = 9;
    pub const THALASSIAN: u32 = 10;
    pub const DRACONIC: u32 = 11;
    pub const KALIMAG: u32 = 12;
    pub const GNOMISH: u32 = 13;
    pub const TROLL: u32 = 14;
    pub const GUTTERSPEAK: u32 = 33;
    pub const ADDON: u32 = 0xFFFF_FFFF;
}

pub fn chat_tag_for(flags: u32) -> u8 {
    if flags & player_flags::DND != 0 {
        chat_tag::DND
    } else if flags & player_flags::AFK != 0 {
        chat_tag::AFK
    } else {
        chat_tag::NONE
    }
}

pub fn speakable_language(kind: u8, race: u8, language: u32) -> Result<u32, ChatRefusal> {
    use chat_kind::*;
    if language == language::ADDON {
        return match kind {
            PARTY | RAID | RAID_LEADER | RAID_WARNING | GUILD | OFFICER | CHANNEL => Ok(language),
            _ => Err(ChatRefusal::UnsupportedKind),
        };
    }
    if matches!(
        kind,
        WHISPER | WHISPER_INFORM | GUILD | OFFICER | AFK | DND | IGNORED
    ) || language == language::UNIVERSAL
    {
        return Ok(language::UNIVERSAL);
    }
    if racial_languages(race).contains(&language) {
        Ok(language)
    } else {
        Err(ChatRefusal::UnknownLanguage)
    }
}

// Racial languages derive from SkillRaceClassInfo.dbc language skill rows.
fn racial_languages(race: u8) -> &'static [u32] {
    use language::*;
    match race {
        1 => &[COMMON],
        2 => &[ORCISH],
        3 => &[COMMON, DWARVISH],
        4 => &[COMMON, DARNASSIAN],
        5 => &[ORCISH, GUTTERSPEAK],
        6 => &[ORCISH, TAURAHE],
        7 => &[COMMON, GNOMISH],
        8 => &[ORCISH, TROLL],
        _ => &[],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRefusal {
    NotInGroup,
    NotRaid,
    NotRaidLeader,
    NotRaidLeaderOrAssistant,
    UnknownLanguage,
    UnsupportedKind,
    EmptyMessage,
    NotInGuild,
    NoGuildChatRight,
    Channel(ChannelRefusal),
    WrongFaction,
}

impl ChatRefusal {
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::NotInGroup => "chat:not_in_group",
            Self::NotRaid => "chat:not_raid",
            Self::NotRaidLeader => "chat:not_raid_leader",
            Self::NotRaidLeaderOrAssistant => "chat:not_raid_leader_or_assistant",
            Self::UnknownLanguage => "chat:unknown_language",
            Self::UnsupportedKind => "chat:unsupported_kind",
            Self::EmptyMessage => "chat:empty_message",
            Self::NotInGuild => "chat:not_in_guild",
            Self::NoGuildChatRight => "chat:no_guild_chat_right",
            Self::Channel(refusal) => refusal.as_tag(),
            Self::WrongFaction => "chat:wrong_faction",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_kinds_are_the_vanilla_chat_msg_values() {
        assert_eq!(chat_kind::SAY, 0);
        assert_eq!(chat_kind::PARTY, 1);
        assert_eq!(chat_kind::RAID, 2);
        assert_eq!(chat_kind::GUILD, 3);
        assert_eq!(chat_kind::OFFICER, 4);
        assert_eq!(chat_kind::YELL, 5);
        assert_eq!(chat_kind::WHISPER, 6);
        assert_eq!(chat_kind::WHISPER_INFORM, 7);
        assert_eq!(chat_kind::EMOTE, 8);
        assert_eq!(chat_kind::CHANNEL, 14);
        assert_eq!(chat_kind::AFK, 20);
        assert_eq!(chat_kind::DND, 21);
        assert_eq!(chat_kind::IGNORED, 22);
        assert_eq!(chat_kind::RAID_LEADER, 87);
        assert_eq!(chat_kind::RAID_WARNING, 88);
    }

    #[test]
    fn broadcast_chat_discriminants_and_text_emote_range_are_pinned() {
        assert_eq!(broadcast_chat::SAY, 0);
        assert_eq!(broadcast_chat::YELL, 1);
        assert_eq!(broadcast_chat::CREATURE_TEXT_EMOTE, 2);
        assert_eq!(broadcast_chat::EMOTE, 3);
        assert_eq!(broadcast_chat::TEXT_EMOTE_RANGE_YD, 25.0);
    }

    #[test]
    fn a_race_speaks_exactly_its_racial_languages() {
        const COMMON: u32 = 7;
        const ORCISH: u32 = 1;
        const DWARVISH: u32 = 6;
        const DARNASSIAN: u32 = 2;
        const GUTTERSPEAK: u32 = 33;
        const TAURAHE: u32 = 3;
        const GNOMISH: u32 = 13;
        const TROLL: u32 = 14;
        let columns = [
            COMMON,
            ORCISH,
            DWARVISH,
            DARNASSIAN,
            GUTTERSPEAK,
            TAURAHE,
            GNOMISH,
            TROLL,
        ];
        let rows: [(&str, u8, [bool; 8]); 8] = [
            (
                "Human",
                1,
                [true, false, false, false, false, false, false, false],
            ),
            (
                "Orc",
                2,
                [false, true, false, false, false, false, false, false],
            ),
            (
                "Dwarf",
                3,
                [true, false, true, false, false, false, false, false],
            ),
            (
                "Night Elf",
                4,
                [true, false, false, true, false, false, false, false],
            ),
            (
                "Undead",
                5,
                [false, true, false, false, true, false, false, false],
            ),
            (
                "Tauren",
                6,
                [false, true, false, false, false, true, false, false],
            ),
            (
                "Gnome",
                7,
                [true, false, false, false, false, false, true, false],
            ),
            (
                "Troll",
                8,
                [false, true, false, false, false, false, false, true],
            ),
        ];
        for (name, race, known) in rows {
            for (language, known) in columns.into_iter().zip(known) {
                let expected = if known {
                    Ok(language)
                } else {
                    Err(ChatRefusal::UnknownLanguage)
                };
                assert_eq!(
                    speakable_language(chat_kind::PARTY, race, language),
                    expected,
                    "{name} speaking language {language}"
                );
            }
        }
    }

    #[test]
    fn nobody_speaks_the_languages_no_race_learns() {
        for race in 1..=8 {
            for language in [8, 9, 10, 11, 12] {
                assert_eq!(
                    speakable_language(chat_kind::PARTY, race, language),
                    Err(ChatRefusal::UnknownLanguage),
                    "race {race} language {language}"
                );
            }
        }
        assert_eq!(
            speakable_language(chat_kind::PARTY, 0, 7),
            Err(ChatRefusal::UnknownLanguage),
            "an unknown race knows nothing but Universal"
        );
    }

    #[test]
    fn universal_passes_for_every_kind_and_race() {
        for kind in [1, 2, 3, 4, 6, 7, 14, 20, 21, 22, 87, 88] {
            for race in [0, 1, 2, 5, 8] {
                assert_eq!(speakable_language(kind, race, 0), Ok(0));
            }
        }
    }

    #[test]
    fn forced_universal_kinds_ignore_the_requested_language() {
        for kind in [6, 7, 3, 4, 20, 21, 22] {
            assert_eq!(speakable_language(kind, 1, 1), Ok(0), "kind {kind} Orcish");
            assert_eq!(speakable_language(kind, 2, 7), Ok(0), "kind {kind} Common");
        }
    }

    #[test]
    fn the_addon_language_passes_only_for_addon_kinds() {
        for kind in [1, 2, 87, 88, 3, 4, 14] {
            assert_eq!(
                speakable_language(kind, 1, 0xFFFF_FFFF),
                Ok(0xFFFF_FFFF),
                "kind {kind}"
            );
        }
        for kind in [6, 7, 20, 21, 22] {
            assert_eq!(
                speakable_language(kind, 1, 0xFFFF_FFFF),
                Err(ChatRefusal::UnsupportedKind),
                "kind {kind}"
            );
        }
    }

    #[test]
    fn a_racial_language_passes_for_every_carrying_kind() {
        for kind in [0, 1, 2, 5, 8, 14, 87, 88] {
            assert_eq!(
                speakable_language(kind, 3, 6),
                Ok(6),
                "Dwarvish on kind {kind}"
            );
        }
    }

    #[test]
    fn proximity_kinds_refuse_a_language_the_race_does_not_know() {
        for kind in [0, 5, 8] {
            assert_eq!(
                speakable_language(kind, 1, 1),
                Err(ChatRefusal::UnknownLanguage),
                "Human Orcish on kind {kind}"
            );
            assert_eq!(speakable_language(kind, 2, 1), Ok(1), "Orc Orcish");
            assert_eq!(speakable_language(kind, 1, 0), Ok(0), "Universal");
            assert_eq!(
                speakable_language(kind, 1, 0xFFFF_FFFF),
                Err(ChatRefusal::UnsupportedKind),
                "addon on kind {kind}"
            );
        }
    }

    #[test]
    fn the_chat_tag_follows_the_away_bits() {
        assert_eq!(chat_tag_for(0), 0);
        assert_eq!(chat_tag_for(0x02), 1);
        assert_eq!(chat_tag_for(0x04), 2);
        assert_eq!(chat_tag_for(0x06), 2);
        assert_eq!(
            chat_tag_for(0x10 | 0x02),
            1,
            "GHOST does not change the tag"
        );
        assert_eq!(chat_tag_for(0x10), 0);
    }
}
