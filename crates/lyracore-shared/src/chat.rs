//! The Realm Chat wire contract both crates import: Chat Kind codes, chat tags, language codes,
//! the language Gate and the typed [`ChatRefusal`] the Gateway maps to client feedback.

use crate::channel::ChannelRefusal;
use crate::constants::player_flags;

/// Chat Kinds: the 1.12 `ChatMsg` wire values (cm:SharedDefines.h:1545-1582). A Realm Chat Line
/// stores the wire value itself, so neither tier keeps a translation table.
pub mod chat_kind {
    pub const PARTY: u8 = 0x01;
    pub const RAID: u8 = 0x02;
    pub const GUILD: u8 = 0x03;
    pub const OFFICER: u8 = 0x04;
    pub const WHISPER: u8 = 0x06;
    pub const WHISPER_INFORM: u8 = 0x07;
    pub const CHANNEL: u8 = 0x0E;
    pub const AFK: u8 = 0x14;
    pub const DND: u8 = 0x15;
    pub const IGNORED: u8 = 0x16;
    pub const RAID_LEADER: u8 = 0x57;
    pub const RAID_WARNING: u8 = 0x58;
}

/// The last byte of `SMSG_MESSAGECHAT` (cm:Chat.h:70-74). The client prints `<AFK>` or `<DND>`
/// before the speaker's name.
pub mod chat_tag {
    pub const NONE: u8 = 0;
    pub const AFK: u8 = 1;
    pub const DND: u8 = 2;
}

/// `game_chat_event.chat_type` discriminants: the World Shard's own broadcast chat table (say,
/// yell, a creature's text emote, and a Character's `/e` custom emote). Not [`chat_kind`] above —
/// that module names the 1.12 wire values a Realm Chat Line carries; this table predates it and
/// keeps its own numbering.
pub mod broadcast_chat {
    pub const SAY: u8 = 0;
    pub const YELL: u8 = 1;
    /// Creature-authored text emote (`CHAT_TYPE_TEXT_EMOTE` on the source wire). EventAI is its
    /// only source.
    pub const CREATURE_TEXT_EMOTE: u8 = 2;
    /// A Character's `/e` custom emote (cm:ChatHandler.cpp:172-226, cm:Player.cpp:16591-16599).
    pub const EMOTE: u8 = 3;

    /// `/e` and text-emote (`/wave`, `/dance`, …) proximity range: `ListenRange.TextEmote`
    /// (cm:mangosd.conf.dist.in:1092), same as `ListenRange.Say`.
    pub const TEXT_EMOTE_RANGE_YD: f32 = 25.0;
}

/// 1.12 `Language` wire values (gtker vanilla `language.rs`).
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
    /// `SendAddonMessage` traffic. Never shown in a chat frame.
    pub const ADDON: u32 = 0xFFFF_FFFF;
}

/// The speaker's chat tag from the live entity's `PLAYER_FLAGS`. DND wins when both bits are set
/// (vm:Player.cpp:1760-1770).
pub fn chat_tag_for(flags: u32) -> u8 {
    if flags & player_flags::DND != 0 {
        chat_tag::DND
    } else if flags & player_flags::AFK != 0 {
        chat_tag::AFK
    } else {
        chat_tag::NONE
    }
}

/// The language a line from `race` goes out in, or the Refusal.
///
/// - The addon language passes only for the kinds `SendAddonMessage` can reach
///   (vm:ChatHandler.cpp:78-118). Guild and officer keep it rather than forcing Universal.
/// - Whisper, guild, officer and the notice kinds always go out in Universal
///   (cm:Player.cpp:16603-16604, cm:ChatHandler.cpp:369, cm:ChatHandler.cpp:409).
/// - Universal passes for every kind.
/// - Any other language passes only when the race knows it (cm:ChatHandler.cpp:100-111).
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

/// The languages a player race speaks: dbc:SkillRaceClassInfo.dbc rows 40, 42, 44, 46, 48, 50, 52
/// and 741 (flags 0x80), language skills from cm:ObjectMgr.cpp:89-106. Players never learn another
/// language, and Realm-core holds no skill rows, so the race is the whole answer.
fn racial_languages(race: u8) -> &'static [u32] {
    use language::*;
    match race {
        1 => &[COMMON],              // Human
        2 => &[ORCISH],              // Orc
        3 => &[COMMON, DWARVISH],    // Dwarf
        4 => &[COMMON, DARNASSIAN],  // Night Elf
        5 => &[ORCISH, GUTTERSPEAK], // Undead
        6 => &[ORCISH, TAURAHE],     // Tauren
        7 => &[COMMON, GNOMISH],     // Gnome
        8 => &[ORCISH, TROLL],       // Troll
        _ => &[],
    }
}

/// Why the Module refused a Realm Chat Line. The tag is the whole reducer error text, so neither
/// tier matches on human prose. Vanilla answers most of these with silence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRefusal {
    /// A party line from a Character in no party.
    NotInGroup,
    /// The speaker's race does not know the language.
    UnknownLanguage,
    /// The Chat Kind has no audience rule, or cannot carry the addon language.
    UnsupportedKind,
    /// Nothing is left after trimming.
    EmptyMessage,
    /// A channel line the channel refused. Answered with the channel's notice.
    Channel(ChannelRefusal),
}

impl ChatRefusal {
    pub const ALL: [Self; 17] = [
        Self::NotInGroup,
        Self::UnknownLanguage,
        Self::UnsupportedKind,
        Self::EmptyMessage,
        Self::Channel(ChannelRefusal::NotMember),
        Self::Channel(ChannelRefusal::WrongPassword),
        Self::Channel(ChannelRefusal::NotModerator),
        Self::Channel(ChannelRefusal::NotOwner),
        Self::Channel(ChannelRefusal::Muted),
        Self::Channel(ChannelRefusal::Banned),
        Self::Channel(ChannelRefusal::InvalidName),
        Self::Channel(ChannelRefusal::PlayerNotFound),
        Self::Channel(ChannelRefusal::PlayerNotBanned),
        Self::Channel(ChannelRefusal::PlayerAlreadyMember),
        Self::Channel(ChannelRefusal::InviteWrongFaction),
        Self::Channel(ChannelRefusal::PlayerInviteBanned),
        Self::Channel(ChannelRefusal::NotModerated),
    ];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::NotInGroup => "chat:not_in_group",
            Self::UnknownLanguage => "chat:unknown_language",
            Self::UnsupportedKind => "chat:unsupported_kind",
            Self::EmptyMessage => "chat:empty_message",
            Self::Channel(refusal) => refusal.as_tag(),
        }
    }

    pub fn parse_tag(tag: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|refusal| refusal.as_tag() == tag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cm:SharedDefines.h:1545-1582, `enum ChatMsg`.
    #[test]
    fn chat_kinds_are_the_vanilla_chat_msg_values() {
        assert_eq!(chat_kind::PARTY, 1);
        assert_eq!(chat_kind::RAID, 2);
        assert_eq!(chat_kind::GUILD, 3);
        assert_eq!(chat_kind::OFFICER, 4);
        assert_eq!(chat_kind::WHISPER, 6);
        assert_eq!(chat_kind::WHISPER_INFORM, 7);
        assert_eq!(chat_kind::CHANNEL, 14);
        assert_eq!(chat_kind::AFK, 20);
        assert_eq!(chat_kind::DND, 21);
        assert_eq!(chat_kind::IGNORED, 22);
        assert_eq!(chat_kind::RAID_LEADER, 87);
        assert_eq!(chat_kind::RAID_WARNING, 88);
    }

    /// `game_chat_event.chat_type` discriminants + the shared text-emote range, pinned so a
    /// later edit shows up as a failing assertion instead of a silent drift between the two crates
    /// that both read them.
    #[test]
    fn broadcast_chat_discriminants_and_text_emote_range_are_pinned() {
        assert_eq!(broadcast_chat::SAY, 0);
        assert_eq!(broadcast_chat::YELL, 1);
        assert_eq!(broadcast_chat::CREATURE_TEXT_EMOTE, 2);
        assert_eq!(broadcast_chat::EMOTE, 3);
        assert_eq!(broadcast_chat::TEXT_EMOTE_RANGE_YD, 25.0); // cm:mangosd.conf.dist.in:1092
    }

    /// Every player race against every language that a race can know, written out from the dbc
    /// rows: true where SkillRaceClassInfo grants the language skill.
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
        // Common, Orcish, Dwarvish, Darnassian, Gutterspeak, Taurahe, Gnomish, Troll
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

    /// No player race knows the NPC and GM languages.
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

    /// Whisper, guild, officer and the notice kinds go out in Universal whatever the client sent.
    #[test]
    fn forced_universal_kinds_ignore_the_requested_language() {
        for kind in [6, 7, 3, 4, 20, 21, 22] {
            assert_eq!(speakable_language(kind, 1, 1), Ok(0), "kind {kind} Orcish");
            assert_eq!(speakable_language(kind, 2, 7), Ok(0), "kind {kind} Common");
        }
    }

    /// vm:ChatHandler.cpp:78-118: party, raid, raid leader, raid warning, guild, officer and channel.
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
        for kind in [1, 2, 14, 87, 88] {
            assert_eq!(
                speakable_language(kind, 3, 6),
                Ok(6),
                "Dwarvish on kind {kind}"
            );
        }
    }

    /// cm:Player.h:370-371 bits; vm:Player.cpp:1760-1770 lets DND win.
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

    #[test]
    fn every_channel_refusal_is_a_chat_refusal() {
        for refusal in ChannelRefusal::ALL {
            assert!(
                ChatRefusal::ALL.contains(&ChatRefusal::Channel(refusal)),
                "{refusal:?}"
            );
        }
    }

    #[test]
    fn every_chat_refusal_tag_round_trips() {
        for refusal in ChatRefusal::ALL {
            assert_eq!(ChatRefusal::parse_tag(refusal.as_tag()), Some(refusal));
        }
        assert_eq!(ChatRefusal::parse_tag("chat:"), None);
        assert_eq!(
            ChatRefusal::parse_tag("chat:channel:muted"),
            Some(ChatRefusal::Channel(ChannelRefusal::Muted))
        );
        assert_eq!(
            ChatRefusal::parse_tag("realm_chat reducer timed out after 10s"),
            None
        );
    }
}
