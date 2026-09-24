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

/// Length caps for the rest of a Guild's client-supplied text, in Unicode scalars
/// (`vm:src/game/Guild/Guild.h:36-40`). Over-length input is a [`GuildRefusal::TooLong`].
pub const MAX_RANK_NAME: usize = 15;
pub const MAX_NOTE: usize = 31;
pub const MAX_INFO: usize = 500;
pub const MAX_MOTD: usize = 128;

/// Does `text` fit within `max` Unicode scalars? Shared by every guild text field length gate.
pub fn fits_length(text: &str, max: usize) -> bool {
    text.chars().count() <= max
}

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
/// are broadcast kinds that only the Gateway renders. A petition kind carries `guild_id` 0, except
/// FOUNDER, which names the new Guild.
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
    /// Addressed to an invite target: SMSG_GUILD_INVITE, not the generic SMSG_GUILD_EVENT builder.
    pub const INVITE: u8 = 0x40;
    /// Addressed to an inviter: raw SMSG_GUILD_DECLINE, not the generic SMSG_GUILD_EVENT builder.
    pub const DECLINE: u8 = 0x41;
    /// Addressed to the editor after a note edit: a fresh SMSG_GUILD_ROSTER for that viewer alone.
    pub const ROSTER_TO_ACTOR: u8 = 0x50;
    /// Broadcast after a rank edit, add or delete: SMSG_GUILD_QUERY_RESPONSE then a per-viewer
    /// SMSG_GUILD_ROSTER to every online member, each built for that viewer's own Guild Rank, so
    /// officer notes keep following each viewer's VIEWOFFNOTE right instead of going blank for
    /// everyone the way mangos' single shared broadcast roster does.
    pub const ROSTER_REFRESH: u8 = 0x80;
    /// Addressed to the offer's target: SMSG_PETITION_SHOW_SIGNATURES for the Guild Charter in
    /// `other_guid`, read when the job runs.
    pub const PETITION_OFFERED: u8 = 0x70;
    /// Addressed to the Petition owner and to the signer: SMSG_PETITION_SIGN_RESULTS OK for the
    /// Guild Charter in `other_guid`, signed by `subject_guid`.
    pub const PETITION_SIGNED: u8 = 0x71;
    /// Addressed to the Petition owner and to the signer: SMSG_PETITION_SIGN_RESULTS
    /// ALREADY_SIGNED, when the signer's Realm Account already signed.
    pub const PETITION_ALREADY_SIGNED: u8 = 0x72;
    /// Addressed to the Petition owner: MSG_PETITION_DECLINE naming the decliner, `subject_guid`.
    pub const PETITION_DECLINED: u8 = 0x73;
    /// Addressed to each signer that joined at founding: SMSG_GUILD_COMMAND_RESULT FOUNDER with the
    /// Guild name.
    pub const FOUNDER: u8 = 0x74;
    /// Addressed to the Petition owner after a Signature is lost: SMSG_PETITION_QUERY_RESPONSE for
    /// the Guild Charter in `other_guid`, read when the job runs.
    pub const PETITION_CHANGED: u8 = 0x75;

    /// Is `kind` in the petition block? Every petition kind is addressed.
    pub fn is_petition(kind: u8) -> bool {
        (0x70..=0x7F).contains(&kind)
    }
}

/// `game_guild_fee_hold.kind`: which guild operation a Fee Hold pays for.
pub mod fee_kind {
    pub const EMBLEM: u8 = 1;
    pub const CHARTER: u8 = 2;
}

/// A Guild Emblem costs 10 gold (`cm:GuildHandler.cpp:750-757`).
pub const EMBLEM_COST_COPPER: u32 = 100_000;

/// The Guild Charter item, its display and its price (`cm:PetitionsHandler.cpp:39-42`).
pub const GUILD_CHARTER_ENTRY: u32 = 5863;
pub const CHARTER_DISPLAY_ID: u32 = 16161;
pub const CHARTER_COST_COPPER: u32 = 1000;

/// Signatures a Petition needs to found a Guild, and the most it takes. Vanilla asks for nine
/// (`cm:World.cpp:589`); the client signs nine at most (`cm:PetitionsHandler.cpp:371`).
pub const MIN_PETITION_SIGNATURES: usize = 9;
pub const MAX_PETITION_SIGNATURES: usize = 9;

/// The key of the Signature in `slot` (`0..MAX_PETITION_SIGNATURES`) of Petition `petition_id`.
/// The Gateway's cache has no index on the Petition column, so it finds a Petition's Signatures by
/// these keys: at most nine keyed lookups instead of a scan of every Signature in the realm.
pub fn petition_signature_key(petition_id: u32, slot: usize) -> u64 {
    (u64::from(petition_id) << 8) | (slot as u64 & 0xFF)
}

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
    /// An Invite's team does not match the target's, or an Accept's team does not match the
    /// Guild's.
    NotAllied,
    /// The target already holds a pending Guild Invite.
    AlreadyInvited,
    /// The actor holds no pending Guild Invite, or is already a member.
    NoPendingInvite,
    /// The Guild Leader must pass leadership before leaving, or cannot be removed.
    LeaderCannotLeave,
    /// The actor's Guild Rank cannot reach the target's.
    RankTooHigh,
    /// The target already holds the Guild's lowest Guild Rank.
    RankTooLow,
    /// The op named the actor as its own target.
    TargetIsSelf,
    /// A text field (MOTD, info text, a note or a rank name) breaks its length cap.
    TooLong,
    /// Add Rank at [`MAX_RANKS`], or Delete Rank at [`MIN_RANKS`].
    RanksAtLimit,
    /// The Character already owns an open Petition.
    AlreadyHasPetition,
    /// No open Petition belongs to the named Guild Charter.
    NoSuchPetition,
    /// The actor does not own the Petition.
    NotPetitionOwner,
    /// A Petition owner cannot sign its own Petition.
    CantSignOwn,
    /// The Petition holds [`MAX_PETITION_SIGNATURES`] Signatures.
    PetitionFull,
    /// The Petition holds fewer than [`MIN_PETITION_SIGNATURES`] Signatures.
    NeedMoreSignatures,
    /// The Guild Charter item template is missing from this World Shard.
    CharterUnavailable,
    /// No free bag slot for the Guild Charter.
    BagsFull,
}

impl GuildRefusal {
    pub const ALL: [Self; 28] = [
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
        Self::NotAllied,
        Self::AlreadyInvited,
        Self::NoPendingInvite,
        Self::LeaderCannotLeave,
        Self::RankTooHigh,
        Self::RankTooLow,
        Self::TargetIsSelf,
        Self::TooLong,
        Self::RanksAtLimit,
        Self::AlreadyHasPetition,
        Self::NoSuchPetition,
        Self::NotPetitionOwner,
        Self::CantSignOwn,
        Self::PetitionFull,
        Self::NeedMoreSignatures,
        Self::CharterUnavailable,
        Self::BagsFull,
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
            Self::NotAllied => "guild:not_allied",
            Self::AlreadyInvited => "guild:already_invited",
            Self::NoPendingInvite => "guild:no_pending_invite",
            Self::LeaderCannotLeave => "guild:leader_cannot_leave",
            Self::RankTooHigh => "guild:rank_too_high",
            Self::RankTooLow => "guild:rank_too_low",
            Self::TargetIsSelf => "guild:target_is_self",
            Self::TooLong => "guild:too_long",
            Self::RanksAtLimit => "guild:ranks_at_limit",
            Self::AlreadyHasPetition => "guild:already_has_petition",
            Self::NoSuchPetition => "guild:no_such_petition",
            Self::NotPetitionOwner => "guild:not_petition_owner",
            Self::CantSignOwn => "guild:cant_sign_own",
            Self::PetitionFull => "guild:petition_full",
            Self::NeedMoreSignatures => "guild:need_more_signatures",
            Self::CharterUnavailable => "guild:charter_unavailable",
            Self::BagsFull => "guild:bags_full",
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
    fn fits_length_counts_unicode_scalars_not_bytes() {
        assert!(fits_length(&"é".repeat(31), MAX_NOTE));
        assert!(!fits_length(&"é".repeat(32), MAX_NOTE));
        assert!(fits_length("", MAX_MOTD));
    }

    #[test]
    fn each_petition_has_its_own_nine_signature_keys() {
        assert_eq!(petition_signature_key(1, 0), 0x100);
        assert_eq!(petition_signature_key(1, 8), 0x108);
        assert_eq!(petition_signature_key(2, 0), 0x200);
        assert_eq!(petition_signature_key(u32::MAX, 8), 0x00FF_FFFF_FF08);
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
