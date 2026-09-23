//! The guild contract both tiers share: Rank Rights bits, the default Guild Ranks, the guild name
//! rule, Guild Event kinds and the typed [`GuildRefusal`]. The Module applies these rules on
//! Realm-core; the Gateway reads the same functions for its advisory checks and renders.

/// Rank Rights bits as the 1.12 client reads them (`cm:Guild.h:45-63`). Every right except
/// [`EMPTY`] carries the `EMPTY` bit, so test a right with [`super::has_right`], never a bare `&`.
pub mod rights {
    pub const EMPTY: u32 = 0x40;
    pub const GCHATLISTEN: u32 = 0x41;
    pub const GCHATSPEAK: u32 = 0x42;
    pub const OFFCHATLISTEN: u32 = 0x44;
    pub const OFFCHATSPEAK: u32 = 0x48;
    pub const PROMOTE: u32 = 0xC0;
    pub const DEMOTE: u32 = 0x140;
    pub const INVITE: u32 = 0x50;
    pub const REMOVE: u32 = 0x60;
    pub const SETMOTD: u32 = 0x1040;
    pub const EPNOTE: u32 = 0x2040;
    pub const VIEWOFFNOTE: u32 = 0x4040;
    pub const EOFFNOTE: u32 = 0x8040;
    pub const MODIFY_GUILD_INFO: u32 = 0x10040;
    pub const ALL: u32 = 0xF_F1FF;
}

/// Does a Guild Rank with `rank_rights` hold `right`? The shared `EMPTY` bit is masked off first,
/// so a zero mask grants nothing. mangos tests `rights & right` and would grant every right to a
/// mask that carries only `EMPTY`; the client never sends one without other bits.
pub fn has_right(rank_rights: u32, right: u32) -> bool {
    rank_rights & (right & !rights::EMPTY) != 0
}

/// A Guild has five to ten Guild Ranks (`cm:Guild.h:30-31`).
pub const MIN_RANKS: usize = 5;
pub const MAX_RANKS: usize = 10;

/// The five Guild Ranks a new Guild starts with, highest first (`cm:Guild.cpp:156-165`). The names
/// are the English `mangos_string` rows 811 to 815.
pub const DEFAULT_RANKS: [(&str, u32); MIN_RANKS] = [
    ("Guild Master", rights::ALL),
    ("Officer", rights::ALL),
    ("Veteran", rights::GCHATLISTEN | rights::GCHATSPEAK),
    ("Member", rights::GCHATLISTEN | rights::GCHATSPEAK),
    ("Initiate", rights::GCHATLISTEN | rights::GCHATSPEAK),
];

/// The Guild Leader's rank id.
pub const LEADER_RANK: u32 = 0;

/// A new Guild's message of the day (`cm:Guild.cpp:123`).
pub const DEFAULT_MOTD: &str = "No message set.";

/// Guild name length in Unicode scalars.
pub const MIN_GUILD_NAME: usize = 2;
pub const MAX_GUILD_NAME: usize = 24;

/// Guild name rule (`cm:ObjectMgr.cpp:8128-8139,8224-8240`): 2 to 24 Unicode scalars of digits,
/// spaces and letters of ONE script, as mangos `isValidString` checks a Charter name, so a
/// look-alike such as a Cyrillic "К" in a Latin name is refused. Spaces only separate words: no
/// leading, trailing or repeated space, so "Knights" and "Knights " cannot both exist.
pub fn validate_guild_name(name: &str) -> Result<(), GuildRefusal> {
    let length = name.chars().count();
    let spaced_once = !name.starts_with(' ') && !name.ends_with(' ') && !name.contains("  ");
    let one_script = [Script::Latin, Script::Cyrillic, Script::EastAsian]
        .into_iter()
        .any(|script| {
            name.chars()
                .all(|c| c.is_ascii_digit() || c == ' ' || script.holds(c))
        });
    if (MIN_GUILD_NAME..=MAX_GUILD_NAME).contains(&length) && spaced_once && one_script {
        Ok(())
    } else {
        Err(GuildRefusal::NameInvalid)
    }
}

/// The letter sets mangos accepts in a name (`cm:Util.h:172-227`).
#[derive(Clone, Copy)]
enum Script {
    /// Basic Latin plus the Latin-1 and Latin Extended-A letters mangos lists.
    Latin,
    Cyrillic,
    /// Hangul, kana, CJK ideographs and the halfwidth and fullwidth forms.
    EastAsian,
}

impl Script {
    fn holds(self, c: char) -> bool {
        let c = u32::from(c);
        let ranges: &[(u32, u32)] = match self {
            Self::Latin => &[
                (0x41, 0x5A),
                (0x61, 0x7A),
                (0xC0, 0xD6),
                (0xD8, 0xDF),
                (0xE0, 0xF6),
                (0xF8, 0xFE),
                (0x100, 0x12F),
                (0x1E9E, 0x1E9E),
            ],
            Self::Cyrillic => &[(0x401, 0x401), (0x410, 0x44F), (0x451, 0x451)],
            Self::EastAsian => &[
                (0x1100, 0x11F9),
                (0x3041, 0x30FF),
                (0x3131, 0x318E),
                (0x31F0, 0x31FF),
                (0x3400, 0x4DB5),
                (0x4E00, 0x9FC3),
                (0xAC00, 0xD7A3),
                (0xFF01, 0xFFEE),
            ],
        };
        ranges
            .iter()
            .any(|(first, last)| (*first..=*last).contains(&c))
    }
}

/// The founding Gates, in the order `.guild create` reports them: a valid name, then a free name,
/// then a leader outside every Guild. Answers the [`name_key`] to store. The Module runs this on
/// Realm-core; a Gateway Fake runs the same function, so neither can drift from the other.
pub fn founding_gate(
    name: &str,
    name_taken: impl FnOnce(&str) -> bool,
    leader_in_guild: bool,
) -> Result<String, GuildRefusal> {
    validate_guild_name(name)?;
    let key = name_key(name);
    if name_taken(&key) {
        return Err(GuildRefusal::NameExists);
    }
    if leader_in_guild {
        return Err(GuildRefusal::AlreadyInGuild);
    }
    Ok(key)
}

/// The uniqueness key of a guild name. Two names that differ only by case collide, so "Knights"
/// and "knights" cannot both exist.
pub fn name_key(name: &str) -> String {
    name.to_lowercase()
}

/// `game_guild_event.kind`: what the Gateway renders for one Guild Event row.
///
/// `0..=13` are broadcast kinds and equal the wire `GuildEvent` byte of SMSG_GUILD_EVENT
/// (`cm:Guild.h:100-116`). Addressed kinds start at `0x40` and are reserved in blocks: `0x40..=0x4F`
/// membership, `0x50..=0x5F` settings, `0x60..=0x6F` fees, `0x70..=0x7F` petitions. `0x80..=0x8F`
/// are broadcast kinds that only the Gateway renders.
pub mod event_kind {
    pub const PROMOTION: u8 = 0;
    pub const DEMOTION: u8 = 1;
    pub const MOTD: u8 = 2;
    pub const JOINED: u8 = 3;
    pub const LEFT: u8 = 4;
    pub const REMOVED: u8 = 5;
    pub const LEADER_IS: u8 = 6;
    pub const LEADER_CHANGED: u8 = 7;
    pub const DISBANDED: u8 = 8;
    pub const TABARD_CHANGED: u8 = 9;
    pub const SIGNED_ON: u8 = 12;
    pub const SIGNED_OFF: u8 = 13;
}

/// `game_guild_fee_hold.kind`: which guild operation a Fee Hold pays for.
pub mod fee_kind {
    pub const EMBLEM: u8 = 1;
}

/// A Guild Emblem costs 10 gold (`cm:GuildHandler.cpp:750-757`).
pub const EMBLEM_COST_COPPER: u32 = 100_000;

/// Why the Module refused a guild Durable Request. The tag is the whole reducer error text, so
/// neither tier matches on human prose. A Refusal leaves every guild row unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuildRefusal {
    /// The acting Character has GM level 0.
    NotGameMaster,
    /// The guild name breaks [`validate_guild_name`].
    NameInvalid,
    /// Another Guild holds the same [`name_key`].
    NameExists,
    /// The Character is already a member of a Guild.
    AlreadyInGuild,
    /// The acting Character is in no Guild.
    NotInGuild,
    /// The request named no Character.
    NoSuchCharacter,
    /// The actor's Guild Rank lacks the Rank Right the op needs.
    NoPermission,
    /// The named Character is not a member of the actor's Guild.
    TargetNotInGuild,
    /// The op needs the Guild Leader.
    NotLeader,
    /// The purse holds less than the guild fee.
    NotEnoughMoney,
    /// The named NPC does not serve this guild operation, or not to this Character here.
    NpcRefused,
}

impl GuildRefusal {
    pub const ALL: [Self; 11] = [
        Self::NotGameMaster,
        Self::NameInvalid,
        Self::NameExists,
        Self::AlreadyInGuild,
        Self::NotInGuild,
        Self::NoSuchCharacter,
        Self::NoPermission,
        Self::TargetNotInGuild,
        Self::NotLeader,
        Self::NotEnoughMoney,
        Self::NpcRefused,
    ];

    pub fn as_tag(self) -> &'static str {
        match self {
            Self::NotGameMaster => "guild:not_game_master",
            Self::NameInvalid => "guild:name_invalid",
            Self::NameExists => "guild:name_exists",
            Self::AlreadyInGuild => "guild:already_in_guild",
            Self::NotInGuild => "guild:not_in_guild",
            Self::NoSuchCharacter => "guild:no_such_character",
            Self::NoPermission => "guild:no_permission",
            Self::TargetNotInGuild => "guild:target_not_in_guild",
            Self::NotLeader => "guild:not_leader",
            Self::NotEnoughMoney => "guild:not_enough_money",
            Self::NpcRefused => "guild:npc_refused",
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

    #[test]
    fn guild_names_accept_one_script_with_digits_and_single_spaces() {
        for accepted in [
            "Ab",
            "Knights of Silver Hand",
            "abcdefghijklmnopqrstuvwx",
            "Guild 7",
            "Ordre du Cèdre",
            "Рыцари Света",
            "銀の騎士団",
        ] {
            assert_eq!(validate_guild_name(accepted), Ok(()), "{accepted:?}");
        }
        for refused in [
            "",
            "A",
            "abcdefghijklmnopqrstuvwxy",
            "Knights!",
            "Tracer-Guild",
            "The_Guild",
            "Guild\tName",
            // A Cyrillic capital Ka in front of Latin letters.
            "\u{41A}nights",
            "Knights ",
            " Knights",
            "Knights  Two",
            // The oe ligature is past the Latin letters mangos lists.
            "Ordre du Cœur",
            "Ιππότες",
        ] {
            assert_eq!(
                validate_guild_name(refused),
                Err(GuildRefusal::NameInvalid),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn name_length_counts_unicode_scalars_not_bytes() {
        let twenty_four = "é".repeat(24);
        assert_eq!(twenty_four.len(), 48);
        assert_eq!(validate_guild_name(&twenty_four), Ok(()));
        assert_eq!(
            validate_guild_name(&"é".repeat(25)),
            Err(GuildRefusal::NameInvalid)
        );
    }

    #[test]
    fn founding_gates_run_name_then_uniqueness_then_membership() {
        let taken = |key: &str| key == "tracer guild";
        assert_eq!(
            founding_gate("Tracer Guild!", taken, true),
            Err(GuildRefusal::NameInvalid)
        );
        assert_eq!(
            founding_gate("TRACER GUILD", taken, true),
            Err(GuildRefusal::NameExists)
        );
        assert_eq!(
            founding_gate("Knights", taken, true),
            Err(GuildRefusal::AlreadyInGuild)
        );
        assert_eq!(founding_gate("Knights", taken, false), Ok("knights".into()));
    }

    #[test]
    fn names_that_differ_only_by_case_share_one_key() {
        assert_eq!(name_key("Tracer Guild"), name_key("tracer guild"));
        assert_eq!(name_key("ÉCLAT"), name_key("éclat"));
        assert_ne!(name_key("Tracer Guild"), name_key("TracerGuild"));
    }

    #[test]
    fn has_right_ignores_the_shared_empty_bit() {
        assert!(has_right(rights::ALL, rights::VIEWOFFNOTE));
        assert!(has_right(
            rights::GCHATLISTEN | rights::GCHATSPEAK,
            rights::GCHATSPEAK
        ));
        assert!(!has_right(
            rights::GCHATLISTEN | rights::GCHATSPEAK,
            rights::VIEWOFFNOTE
        ));
        assert!(!has_right(0, rights::GCHATLISTEN));
        assert!(!has_right(rights::EMPTY, rights::INVITE));
        assert!(!has_right(rights::ALL, rights::EMPTY));
    }

    #[test]
    fn default_ranks_give_the_leader_and_officer_every_right() {
        assert_eq!(DEFAULT_RANKS[LEADER_RANK as usize].0, "Guild Master");
        assert_eq!(DEFAULT_RANKS[0].1, 0xF_F1FF);
        assert_eq!(DEFAULT_RANKS[1].1, 0xF_F1FF);
        for (_, initiate_rights) in &DEFAULT_RANKS[2..] {
            assert_eq!(*initiate_rights, 0x43);
        }
    }

    #[test]
    fn every_guild_refusal_tag_round_trips() {
        for refusal in GuildRefusal::ALL {
            assert_eq!(GuildRefusal::parse_tag(refusal.as_tag()), Some(refusal));
        }
        assert_eq!(GuildRefusal::parse_tag("guild:"), None);
        assert_eq!(
            GuildRefusal::parse_tag("realm_guild_op reducer timed out after 10s"),
            None
        );
    }
}
