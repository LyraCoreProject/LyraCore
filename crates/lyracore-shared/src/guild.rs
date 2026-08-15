//! The module↔gateway GUILD wire contract — the ten-rank seed, the name rules, event-kind codes,
//! the realm-op tags, the roster-payload grammar and the reducer error strings the gateway
//! classifies into `SMSG_GUILD_COMMAND_RESULT`. Sibling of [`crate::group`], and here for the same
//! reason: one definition both crates import, so a renumber, a reworded `Err` or a delimiter change
//! is a cross-crate compile error instead of runtime drift.

/// Rank rows per guild — **exactly** this many, always.
///
/// `SMSG_GUILD_QUERY_RESPONSE.rank_names` is a fixed `[String; 10]`, so a guild with a different
/// count cannot be rendered. Treat it as an invariant of [`DEFAULT_RANK_NAMES`]'s seeding, not as
/// data a later edit may vary.
pub const GUILD_RANK_COUNT: usize = 10;

/// The rank a guild's founder holds, and the rank the master always holds.
pub const GUILD_MASTER_RANK: u32 = 0;

/// The vanilla rank names a fresh guild is seeded with, in rank order. The last five are empty:
/// vanilla ships five named ranks and five free slots, and the wire still carries ten strings.
pub const DEFAULT_RANK_NAMES: [&str; GUILD_RANK_COUNT] = [
    "Guild Master",
    "Officer",
    "Veteran",
    "Member",
    "Initiate",
    "",
    "",
    "",
    "",
    "",
];

/// The vanilla per-rank `rights` bitmask defaults, in rank order.
///
/// Written at creation and **never consulted server-side**: rank permissions are a deliberate
/// non-goal of the guild system as it stands, and the only permission check anywhere is "are you
/// the guild master". The column exists because the wire carries it.
pub const DEFAULT_RANK_RIGHTS: [u32; GUILD_RANK_COUNT] =
    [0x00FF, 0x00FF, 0x0009, 0x0009, 0x0009, 0, 0, 0, 0, 0];

/// Shortest guild name the create gate admits (vanilla's own minimum).
pub const GUILD_NAME_MIN_LEN: usize = 2;
/// Longest guild name the create gate admits (vanilla's own maximum).
pub const GUILD_NAME_MAX_LEN: usize = 24;

/// Is `name` a guild name the realm will accept? Length bounds plus a character set of letters,
/// digits and single interior spaces — no leading/trailing space, no double space, and none of the
/// roster grammar's own delimiters.
///
/// Pure, and shared: the module gates on it so a crafted `CMSG_GUILD_CREATE` cannot bypass the
/// check, and the gateway's tests read the same rule rather than a copy of it. Uniqueness is NOT
/// checked here — that is a database question, answered by the `Guild.name` unique index.
pub fn valid_guild_name(name: &str) -> bool {
    let len = name.chars().count();
    if !(GUILD_NAME_MIN_LEN..=GUILD_NAME_MAX_LEN).contains(&len) {
        return false;
    }
    if name.starts_with(' ') || name.ends_with(' ') || name.contains("  ") {
        return false;
    }
    name.chars()
        .all(|c| c.is_alphanumeric() || c == ' ' || c == '\'' || c == '-')
}

/// Guild-event kinds (`game_guild_event.kind`) — what SMSG the gateway relays. The same
/// per-recipient relay shape [`crate::group::event_kind`] uses, on the guild's own table.
pub mod event_kind {
    /// The guild roster changed → the gateway re-renders from the row's payload
    /// ([`super::encode_roster`]), built in the SAME transaction as the membership change.
    pub const ROSTER: u8 = 0;
}

/// The REALM-CORE guild ops: the `op` byte of the single operator-gated `realm_guild_op` reducer
/// the gateway drives realm-wide guild state with.
///
/// One reducer with an op byte rather than one per verb, for the reason
/// [`crate::group::realm_op`] states: every gateway-callable reducer costs a hand-maintained SDK
/// binding, and the argument SHAPE is what actually has to be pinned. It is pinned here.
///
/// Argument slots (`realm_guild_op(op, actor_guid, target_guid, arg_a, text)`), per op:
/// - [`CREATE`] — `text` is the guild name; `target_guid` and `arg_a` unused.
pub mod realm_op {
    /// `CMSG_GUILD_CREATE` — `actor_guid` founds a guild named `text` and becomes its master.
    pub const CREATE: u8 = 0;
}

/// The guild reducers' `Err` strings the gateway maps to `GuildCommandResult` variants — EXACT
/// matches on both sides, so a reword is a compile-visible edit here instead of a silent
/// classifier miss. An error not listed classifies as the gateway's internal-error default.
pub mod err {
    pub const ALREADY_IN_GUILD: &str = "already in a guild";
    pub const NAME_TAKEN: &str = "guild name taken";
    pub const NAME_INVALID: &str = "guild name invalid";
    pub const NOT_IN_GUILD: &str = "not in a guild";
    pub const NOT_GUILD_MASTER: &str = "not the guild master";
}

/// Encode a `ROSTER` payload: `motd|guid,rank,public_note,officer_note;...`.
///
/// Only what REALM-CORE owns is in it. Names, levels, classes, areas and online flags are absent
/// on purpose — the directory database holds no character rows, so the gateway fills those at
/// render time from the shards. Free-text fields get the frame delimiters stripped HERE; the
/// encoder owns the grammar, so it owns the defense.
pub fn encode_roster(motd: &str, members: &[(u64, u32, String, String)]) -> String {
    let members: Vec<String> = members
        .iter()
        .map(|(guid, rank, public_note, officer_note)| {
            format!(
                "{guid},{rank},{},{}",
                strip_frame(public_note),
                strip_frame(officer_note)
            )
        })
        .collect();
    format!("{}|{}", strip_frame(motd), members.join(";"))
}

/// Decode a `ROSTER` payload back to `(motd, [(guid, rank, public_note, officer_note)])`. `None`
/// on any malformed input — the gateway fails closed rather than rendering a corrupt roster.
#[allow(clippy::type_complexity)]
pub fn decode_roster(payload: &str) -> Option<(String, Vec<(u64, u32, String, String)>)> {
    let (motd, rest) = payload.split_once('|')?;
    let mut members = Vec::new();
    for entry in rest.split(';').filter(|e| !e.is_empty()) {
        let mut parts = entry.split(',');
        let guid: u64 = parts.next()?.parse().ok()?;
        let rank: u32 = parts.next()?.parse().ok()?;
        let public_note = parts.next()?.to_string();
        let officer_note = parts.next()?.to_string();
        members.push((guid, rank, public_note, officer_note));
    }
    (!members.is_empty()).then_some((motd.to_string(), members))
}

/// Replace the roster grammar's delimiters so a hostile MOTD or note cannot forge a member row.
fn strip_frame(text: &str) -> String {
    text.replace(['|', ';', ','], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seed is the invariant `SMSG_GUILD_QUERY_RESPONSE` depends on: ten names, ten rights,
    /// master first. A shorter seed truncates the packet's fixed `[String; 10]`.
    #[test]
    fn the_rank_seed_is_exactly_ten_rows_with_the_master_first() {
        assert_eq!(GUILD_RANK_COUNT, 10);
        assert_eq!(DEFAULT_RANK_NAMES.len(), GUILD_RANK_COUNT);
        assert_eq!(DEFAULT_RANK_RIGHTS.len(), GUILD_RANK_COUNT);
        assert_eq!(
            DEFAULT_RANK_NAMES[GUILD_MASTER_RANK as usize],
            "Guild Master"
        );
        assert_eq!(&DEFAULT_RANK_NAMES[5..], &["", "", "", "", ""]);
    }

    #[test]
    fn guild_names_are_bounded_and_restricted_to_a_readable_character_set() {
        assert!(valid_guild_name("Ab"));
        assert!(valid_guild_name("The Silver Hand"));
        assert!(valid_guild_name("Kel'Thuzad's Own"));
        assert!(valid_guild_name("Twenty Four Characters X")); // 24 chars, the maximum
        assert!(!valid_guild_name("A")); // shorter than the minimum
        assert!(!valid_guild_name("Twenty Four Characters Xy")); // 25, one over
        assert!(!valid_guild_name(" Leading"));
        assert!(!valid_guild_name("Trailing "));
        assert!(!valid_guild_name("Double  Space"));
        assert!(!valid_guild_name("Semi;colon"));
        assert!(!valid_guild_name("Pipe|Name"));
        assert!(!valid_guild_name("Comma,Name"));
        assert!(!valid_guild_name("New\nline"));
    }

    #[test]
    fn roster_round_trips_including_the_delimiter_defense() {
        let members = vec![
            (2u64, 0u32, "founder".to_string(), "trusted".to_string()),
            (3, 4, "a|b;c,d".to_string(), String::new()), // hostile note → encoder strips the frame
        ];
        let wire = encode_roster("Welcome, all", &members);
        let (motd, decoded) = decode_roster(&wire).unwrap();
        assert_eq!(motd, "Welcome_ all", "the MOTD is framed too");
        assert_eq!(
            decoded[0],
            (2, 0, "founder".to_string(), "trusted".to_string())
        );
        assert_eq!(decoded[1], (3, 4, "a_b_c_d".to_string(), String::new()));
    }

    #[test]
    fn roster_decode_fails_closed_on_garbage() {
        assert!(decode_roster("").is_none()); // no frame separator at all
        assert!(decode_roster("motd|").is_none()); // empty roster
        assert!(decode_roster("motd|x,0,,").is_none()); // non-numeric guid
        assert!(decode_roster("motd|5,x,,").is_none()); // non-numeric rank
        assert!(decode_roster("motd|5,0").is_none()); // missing both note fields
        assert!(decode_roster("motd|5,0,note").is_none()); // missing the officer note
    }

    /// The op byte is a WIRE value: the gateway sends it, the module dispatches on it, and the two
    /// are deployed separately. Pinned, not merely defined.
    #[test]
    fn realm_guild_op_codes_are_stable() {
        assert_eq!(realm_op::CREATE, 0);
    }
}
