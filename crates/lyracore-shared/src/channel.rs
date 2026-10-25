pub mod notice {
    pub const JOINED: u8 = 0x00;
    pub const LEFT: u8 = 0x01;
    pub const YOU_JOINED: u8 = 0x02;
    pub const YOU_LEFT: u8 = 0x03;
    pub const WRONG_PASSWORD: u8 = 0x04;
    pub const NOT_MEMBER: u8 = 0x05;
    pub const NOT_MODERATOR: u8 = 0x06;
    pub const PASSWORD_CHANGED: u8 = 0x07;
    pub const OWNER_CHANGED: u8 = 0x08;
    pub const PLAYER_NOT_FOUND: u8 = 0x09;
    pub const NOT_OWNER: u8 = 0x0A;
    pub const CHANNEL_OWNER: u8 = 0x0B;
    pub const MODE_CHANGE: u8 = 0x0C;
    pub const ANNOUNCEMENTS_ON: u8 = 0x0D;
    pub const ANNOUNCEMENTS_OFF: u8 = 0x0E;
    pub const MODERATION_ON: u8 = 0x0F;
    pub const MODERATION_OFF: u8 = 0x10;
    pub const MUTED: u8 = 0x11;
    pub const PLAYER_KICKED: u8 = 0x12;
    pub const BANNED: u8 = 0x13;
    pub const PLAYER_BANNED: u8 = 0x14;
    pub const PLAYER_UNBANNED: u8 = 0x15;
    pub const PLAYER_NOT_BANNED: u8 = 0x16;
    pub const PLAYER_ALREADY_MEMBER: u8 = 0x17;
    pub const INVITE: u8 = 0x18;
    pub const INVITE_WRONG_FACTION: u8 = 0x19;
    pub const WRONG_FACTION: u8 = 0x1A;
    pub const INVALID_NAME: u8 = 0x1B;
    pub const NOT_MODERATED: u8 = 0x1C;
    pub const PLAYER_INVITED: u8 = 0x1D;
    pub const PLAYER_INVITE_BANNED: u8 = 0x1E;
    pub const THROTTLED: u8 = 0x1F;
}

// gtker names 0x04 MODERATOR; vanilla uses 0x04 for VOICED.
pub mod member_flag {
    pub const OWNER: u8 = 0x01;
    pub const MODERATOR: u8 = 0x02;
    pub const VOICED: u8 = 0x04;
    pub const MUTED: u8 = 0x08;
}

pub mod channel_flag {
    pub const CUSTOM: u8 = 0x01;
    pub const TRADE: u8 = 0x04;
    pub const NOT_LFG: u8 = 0x08;
    pub const GENERAL: u8 = 0x10;
    pub const CITY: u8 = 0x20;
    pub const LFG: u8 = 0x40;
}

pub mod channel_op {
    pub const JOIN: u8 = 0;
    pub const LEAVE: u8 = 1;
    pub const PASSWORD: u8 = 2;
    pub const SET_OWNER: u8 = 3;
    pub const MODERATOR: u8 = 4;
    pub const UNMODERATOR: u8 = 5;
    pub const MUTE: u8 = 6;
    pub const UNMUTE: u8 = 7;
    pub const KICK: u8 = 8;
    pub const BAN: u8 = 9;
    pub const UNBAN: u8 = 10;
    pub const INVITE: u8 = 11;
    pub const ANNOUNCEMENTS: u8 = 12;
    pub const MODERATE: u8 = 13;
}

mod dbc_flag {
    pub const TRADE: u32 = 0x0_0008;
    pub const CITY_ONLY2: u32 = 0x0_0020;
    pub const LFG: u32 = 0x4_0000;
}

// Build 5875 carries no channel id on join; localized names become custom channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuiltinChannel {
    pub id: u32,
    pub dbc_flags: u32,
    // Pattern prefix before %s, or the complete unzoned name.
    pub prefix: &'static str,
    pub zoned: bool,
}

pub const WORLD_DEFENSE_ID: u32 = 23;
pub const GUILD_RECRUITMENT_ID: u32 = 25;

pub const BUILTIN_CHANNELS: [BuiltinChannel; 6] = [
    BuiltinChannel {
        id: 1,
        dbc_flags: 0x3,
        prefix: "General - ",
        zoned: true,
    },
    BuiltinChannel {
        id: 2,
        dbc_flags: 0x3B,
        prefix: "Trade - ",
        zoned: true,
    },
    BuiltinChannel {
        id: 22,
        dbc_flags: 0x1_0003,
        prefix: "LocalDefense - ",
        zoned: true,
    },
    BuiltinChannel {
        id: WORLD_DEFENSE_ID,
        dbc_flags: 0x1_0004,
        prefix: "WorldDefense",
        zoned: false,
    },
    BuiltinChannel {
        id: 24,
        dbc_flags: 0x0,
        prefix: "LookingForGroup",
        zoned: false,
    },
    BuiltinChannel {
        id: GUILD_RECRUITMENT_ID,
        dbc_flags: 0x2_0032,
        prefix: "GuildRecruitment - ",
        zoned: true,
    },
];

impl BuiltinChannel {
    pub fn wire_flags(self) -> u8 {
        let mut flags = channel_flag::GENERAL;
        if self.dbc_flags & dbc_flag::TRADE != 0 {
            flags |= channel_flag::TRADE;
        }
        if self.dbc_flags & dbc_flag::CITY_ONLY2 != 0 {
            flags |= channel_flag::CITY;
        }
        if self.dbc_flags & dbc_flag::LFG != 0 {
            flags |= channel_flag::LFG;
        } else {
            flags |= channel_flag::NOT_LFG;
        }
        flags
    }

    // WorldDefense requires honor ranks, which this realm does not implement.
    pub fn players_may_speak(self) -> bool {
        self.id != WORLD_DEFENSE_ID
    }
}

pub fn classify(name: &str) -> Option<BuiltinChannel> {
    BUILTIN_CHANNELS.into_iter().find(|builtin| {
        if builtin.zoned {
            name.get(..builtin.prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(builtin.prefix))
        } else {
            name.eq_ignore_ascii_case(builtin.prefix)
        }
    })
}

pub const MAX_NAME_CHARS: usize = 128;
pub const MAX_PASSWORD_CHARS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelName {
    pub key: String,
    pub display: String,
}

impl ChannelName {
    pub fn normalize(raw: &str) -> Self {
        let mut display = String::with_capacity(raw.len());
        let mut in_gap = false;
        for c in raw.chars().take(MAX_NAME_CHARS) {
            if matches!(c, ' ' | '\t' | '\u{7}' | '\n') {
                if !in_gap {
                    display.push(' ');
                }
                in_gap = true;
            } else {
                display.push(c);
                in_gap = false;
            }
        }
        Self {
            key: display.to_lowercase(),
            display,
        }
    }
}

pub fn channel_name(raw: &str) -> Result<ChannelName, ChannelRefusal> {
    let name = ChannelName::normalize(raw);
    match name.display.chars().next() {
        Some(first) if !first.is_ascii() || first.is_ascii_alphabetic() => Ok(name),
        _ => Err(ChannelRefusal::InvalidName),
    }
}

pub fn check_password(password: &str) -> Result<(), ChannelRefusal> {
    if password.chars().count() > MAX_PASSWORD_CHARS {
        Err(ChannelRefusal::WrongPassword)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelRefusal {
    NotMember,
    WrongPassword,
    NotModerator,
    NotOwner,
    Muted,
    Banned,
    InvalidName,
    PlayerNotFound,
    PlayerNotBanned,
    PlayerAlreadyMember,
    InviteWrongFaction,
    PlayerInviteBanned,
    NotModerated,
}

impl ChannelRefusal {
    pub const ALL: [Self; 13] = [
        Self::NotMember,
        Self::WrongPassword,
        Self::NotModerator,
        Self::NotOwner,
        Self::Muted,
        Self::Banned,
        Self::InvalidName,
        Self::PlayerNotFound,
        Self::PlayerNotBanned,
        Self::PlayerAlreadyMember,
        Self::InviteWrongFaction,
        Self::PlayerInviteBanned,
        Self::NotModerated,
    ];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::NotMember => "chat:channel:not_member",
            Self::WrongPassword => "chat:channel:wrong_password",
            Self::NotModerator => "chat:channel:not_moderator",
            Self::NotOwner => "chat:channel:not_owner",
            Self::Muted => "chat:channel:muted",
            Self::Banned => "chat:channel:banned",
            Self::InvalidName => "chat:channel:invalid_name",
            Self::PlayerNotFound => "chat:channel:player_not_found",
            Self::PlayerNotBanned => "chat:channel:player_not_banned",
            Self::PlayerAlreadyMember => "chat:channel:player_already_member",
            Self::InviteWrongFaction => "chat:channel:invite_wrong_faction",
            Self::PlayerInviteBanned => "chat:channel:player_invite_banned",
            Self::NotModerated => "chat:channel:not_moderated",
        }
    }

    pub fn notice(self) -> u8 {
        match self {
            Self::NotMember => notice::NOT_MEMBER,
            Self::WrongPassword => notice::WRONG_PASSWORD,
            Self::NotModerator => notice::NOT_MODERATOR,
            Self::NotOwner => notice::NOT_OWNER,
            Self::Muted => notice::MUTED,
            Self::Banned => notice::BANNED,
            Self::InvalidName => notice::INVALID_NAME,
            Self::PlayerNotFound => notice::PLAYER_NOT_FOUND,
            Self::PlayerNotBanned => notice::PLAYER_NOT_BANNED,
            Self::PlayerAlreadyMember => notice::PLAYER_ALREADY_MEMBER,
            Self::InviteWrongFaction => notice::INVITE_WRONG_FACTION,
            Self::PlayerInviteBanned => notice::PLAYER_INVITE_BANNED,
            Self::NotModerated => notice::NOT_MODERATED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_codes_are_the_vanilla_chat_notify_values() {
        let pinned = [
            (notice::JOINED, 0x00),
            (notice::LEFT, 0x01),
            (notice::YOU_JOINED, 0x02),
            (notice::YOU_LEFT, 0x03),
            (notice::WRONG_PASSWORD, 0x04),
            (notice::NOT_MEMBER, 0x05),
            (notice::NOT_MODERATOR, 0x06),
            (notice::PASSWORD_CHANGED, 0x07),
            (notice::OWNER_CHANGED, 0x08),
            (notice::PLAYER_NOT_FOUND, 0x09),
            (notice::NOT_OWNER, 0x0A),
            (notice::CHANNEL_OWNER, 0x0B),
            (notice::MODE_CHANGE, 0x0C),
            (notice::ANNOUNCEMENTS_ON, 0x0D),
            (notice::ANNOUNCEMENTS_OFF, 0x0E),
            (notice::MODERATION_ON, 0x0F),
            (notice::MODERATION_OFF, 0x10),
            (notice::MUTED, 0x11),
            (notice::PLAYER_KICKED, 0x12),
            (notice::BANNED, 0x13),
            (notice::PLAYER_BANNED, 0x14),
            (notice::PLAYER_UNBANNED, 0x15),
            (notice::PLAYER_NOT_BANNED, 0x16),
            (notice::PLAYER_ALREADY_MEMBER, 0x17),
            (notice::INVITE, 0x18),
            (notice::INVITE_WRONG_FACTION, 0x19),
            (notice::WRONG_FACTION, 0x1A),
            (notice::INVALID_NAME, 0x1B),
            (notice::NOT_MODERATED, 0x1C),
            (notice::PLAYER_INVITED, 0x1D),
            (notice::PLAYER_INVITE_BANNED, 0x1E),
            (notice::THROTTLED, 0x1F),
        ];
        for (code, wire) in pinned {
            assert_eq!(code, wire);
        }
    }

    #[test]
    fn member_flags_are_the_cores_values() {
        assert_eq!(member_flag::OWNER, 0x01);
        assert_eq!(member_flag::MODERATOR, 0x02);
        assert_eq!(member_flag::VOICED, 0x04);
        assert_eq!(member_flag::MUTED, 0x08);
    }

    #[test]
    fn channel_flags_are_the_cores_values() {
        assert_eq!(channel_flag::CUSTOM, 0x01);
        assert_eq!(channel_flag::TRADE, 0x04);
        assert_eq!(channel_flag::NOT_LFG, 0x08);
        assert_eq!(channel_flag::GENERAL, 0x10);
        assert_eq!(channel_flag::CITY, 0x20);
        assert_eq!(channel_flag::LFG, 0x40);
    }

    #[test]
    fn channel_op_codes_are_reserved_in_order() {
        let ops = [
            channel_op::JOIN,
            channel_op::LEAVE,
            channel_op::PASSWORD,
            channel_op::SET_OWNER,
            channel_op::MODERATOR,
            channel_op::UNMODERATOR,
            channel_op::MUTE,
            channel_op::UNMUTE,
            channel_op::KICK,
            channel_op::BAN,
            channel_op::UNBAN,
            channel_op::INVITE,
            channel_op::ANNOUNCEMENTS,
            channel_op::MODERATE,
        ];
        assert_eq!(ops, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
    }

    #[test]
    fn every_builtin_channel_classifies_with_its_wire_flags() {
        let rows = [
            ("General - Elwynn Forest", 1, 0x18),
            ("Trade - City", 2, 0x3C),
            ("LocalDefense - Durotar", 22, 0x18),
            ("WorldDefense", 23, 0x18),
            ("LookingForGroup", 24, 0x18),
            ("GuildRecruitment - City", 25, 0x38),
        ];
        for (name, id, wire_flags) in rows {
            let builtin = classify(name).unwrap_or_else(|| panic!("{name} is built-in"));
            assert_eq!(builtin.id, id, "{name}");
            assert_eq!(builtin.wire_flags(), wire_flags, "{name}");
        }
    }

    #[test]
    fn classification_ignores_case() {
        assert_eq!(classify("trade - city").map(|b| b.id), Some(2));
        assert_eq!(classify("GENERAL - Westfall").map(|b| b.id), Some(1));
        assert_eq!(classify("lookingforgroup").map(|b| b.id), Some(24));
    }

    #[test]
    fn near_misses_stay_custom() {
        for name in [
            "My General - x",
            "General",
            "Trade-City",
            "WorldDefense - Ashenvale",
            "LookingForGroups",
            "LocalDefense",
            "Guild",
            "",
        ] {
            assert_eq!(classify(name), None, "{name:?}");
        }
    }

    #[test]
    fn only_world_defense_is_read_only() {
        for builtin in BUILTIN_CHANNELS {
            assert_eq!(builtin.players_may_speak(), builtin.id != 23, "{builtin:?}");
        }
    }

    #[test]
    fn a_name_keys_by_its_lowercase_spelling() {
        assert_eq!(
            channel_name("Trade - City"),
            Ok(ChannelName {
                key: "trade - city".to_string(),
                display: "Trade - City".to_string(),
            })
        );
    }

    #[test]
    fn invisible_runs_collapse_to_one_space() {
        assert_eq!(
            channel_name("Night\t\t Watch\u{7}\nCrew").map(|name| name.display),
            Ok("Night Watch Crew".to_string())
        );
    }

    #[test]
    fn a_name_is_capped_at_128_characters() {
        let long = "a".repeat(130);
        assert_eq!(
            channel_name(&long).map(|name| name.display.chars().count()),
            Ok(128)
        );
        let accented = "é".repeat(129);
        assert_eq!(
            ChannelName::normalize(&accented).display.chars().count(),
            128
        );
    }

    #[test]
    fn a_name_must_start_with_a_letter() {
        for name in ["1abc", " abc", "_abc", "-", ""] {
            assert_eq!(
                channel_name(name),
                Err(ChannelRefusal::InvalidName),
                "{name:?}"
            );
        }
        assert!(channel_name("Zed").is_ok());
        assert!(channel_name("Ärger").is_ok(), "non-ASCII passes");
    }

    #[test]
    fn a_password_over_128_characters_is_wrong() {
        assert_eq!(check_password(&"p".repeat(128)), Ok(()));
        assert_eq!(
            check_password(&"p".repeat(129)),
            Err(ChannelRefusal::WrongPassword)
        );
        assert_eq!(check_password(""), Ok(()));
    }

    #[test]
    fn each_refusal_renders_its_notice() {
        let expected = [
            (ChannelRefusal::NotMember, 0x05),
            (ChannelRefusal::WrongPassword, 0x04),
            (ChannelRefusal::NotModerator, 0x06),
            (ChannelRefusal::NotOwner, 0x0A),
            (ChannelRefusal::Muted, 0x11),
            (ChannelRefusal::Banned, 0x13),
            (ChannelRefusal::InvalidName, 0x1B),
            (ChannelRefusal::PlayerNotFound, 0x09),
            (ChannelRefusal::PlayerNotBanned, 0x16),
            (ChannelRefusal::PlayerAlreadyMember, 0x17),
            (ChannelRefusal::InviteWrongFaction, 0x19),
            (ChannelRefusal::PlayerInviteBanned, 0x1E),
            (ChannelRefusal::NotModerated, 0x1C),
        ];
        assert_eq!(expected.len(), ChannelRefusal::ALL.len());
        for (refusal, wire) in expected {
            assert_eq!(refusal.notice(), wire, "{refusal:?}");
        }
    }
}
