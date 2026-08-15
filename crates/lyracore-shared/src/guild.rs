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

/// The rank a character joins at when they accept an invite: the LOWEST **named** rank of the
/// vanilla seed ("Initiate").
///
/// Not rank 9. The seed's last five names are empty, so a member parked there renders as a blank
/// line in every guild panel — the count is a wire invariant, not five usable ranks.
pub const GUILD_JOIN_RANK: u32 = 4;

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

    /// A guild invite was extended to the recipient → `SMSG_GUILD_INVITE`. `other_name` is the
    /// inviter's name and `payload` the guild's name, both filled at write time: realm-core holds
    /// no character rows, so nothing downstream can look either of them up.
    pub const INVITE: u8 = 1;

    /// A character joined the guild → `SMSG_GUILD_EVENT(Joined)`. Written for every member,
    /// including the one who just joined. `other_name` is the joiner.
    pub const JOINED: u8 = 2;

    /// The recipient's invite was declined → `SMSG_GUILD_COMMAND_RESULT` to the INVITER.
    /// `other_name` is the character who declined.
    ///
    /// Vanilla answers this with `SMSG_GUILD_DECLINE`, which `wow_world_messages` 0.3 does not
    /// carry. The command-result channel is the one alternative the guild system allows — a system
    /// chat line is never it.
    pub const DECLINED: u8 = 3;

    /// A member signed on or off → `SMSG_GUILD_EVENT(SignedOn)` / `(SignedOff)` to the rest of the
    /// guild. `other_name` is the member; `payload` is [`PRESENCE_ONLINE`] or
    /// [`PRESENCE_OFFLINE`].
    ///
    /// One kind rather than two because the reserved contract block is four kinds wide and the
    /// invite handshake needs the other three. The two directions are the same broadcast with a
    /// flipped bit, so they cost one payload byte instead of a number.
    pub const PRESENCE: u8 = 4;

    /// [`PRESENCE`]'s payload for a member who just signed on.
    pub const PRESENCE_ONLINE: &str = "1";
    /// [`PRESENCE`]'s payload for a member who just signed off.
    pub const PRESENCE_OFFLINE: &str = "0";
    /// A member left of their own accord → `SMSG_GUILD_EVENT(Left)`. `other_guid`/`other_name` name
    /// the member who left; sent to everyone who stayed.
    pub const LEFT: u8 = 5;
    /// The guild master removed a member → `SMSG_GUILD_EVENT(Removed)`. `other_guid`/`other_name`
    /// name the member who was removed.
    pub const REMOVED: u8 = 6;
    /// The guild was destroyed → `SMSG_GUILD_EVENT(Disbanded)`. Sent to every member the guild had
    /// while it still had them, because after the cascade there is no roster left to address.
    pub const DISBANDED: u8 = 7;
    /// The guild has a new master → `SMSG_GUILD_EVENT(LeaderChanged)`. `other_guid`/`other_name`
    /// name the new master.
    pub const LEADER_CHANGED: u8 = 8;
    /// A guild (`/g`) chat line, one row per recipient (every OTHER online member, plus an echo
    /// to the sender) → `SMSG_MESSAGECHAT` with `ChatType::Guild`. `other_guid`/`other_name`
    /// (resolved by the pusher, same convention [`ROSTER`] and `crate::group::event_kind` use) =
    /// the SPEAKER; `payload` = the message text via [`encode_guild_chat`]. Mirrors
    /// `crate::group::event_kind::PARTY_CHAT`.
    pub const GUILD_CHAT: u8 = 11;
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
/// - [`INVITE`] — `target_guid` is the invitee; `text` is the ACTOR's name; `arg_a` unused.
/// - [`ANSWER`] — `arg_a` is [`ANSWER_ACCEPT`] or [`ANSWER_DECLINE`]; `text` is the actor's name.
/// - [`PRESENCE`] — `arg_a` is [`PRESENCE_ON`] or [`PRESENCE_OFF`]; `text` is the actor's name.
///
/// Every op past `CREATE` puts the ACTING character's own name in `text`, because realm-core holds
/// no character rows: a notification written there can only carry a name the gateway supplied.
/// - [`LEAVE`] — `text` is the leaver's OWN name, for the departure notice; the rest unused.
/// - [`REMOVE`] — `target_guid` is the member to remove, `text` their name; `arg_a` unused.
/// - [`DISBAND`] — every slot but `actor_guid` unused.
/// - [`LEADER`] — `target_guid` is the new master, `text` their name; `arg_a` unused.
/// - [`GUILD_CHAT`] — `text` is the message; `target_guid` and `arg_a` unused.
pub mod realm_op {
    /// `CMSG_GUILD_CREATE` — `actor_guid` founds a guild named `text` and becomes its master.
    pub const CREATE: u8 = 0;

    /// `CMSG_GUILD_INVITE` — `actor_guid` (the guild master) invites `target_guid`.
    pub const INVITE: u8 = 1;

    /// `CMSG_GUILD_ACCEPT` / `CMSG_GUILD_DECLINE` — `actor_guid` answers its own pending invite.
    ///
    /// One op with a yes/no slot rather than two: both arms consume the same `GuildInvite` row, and
    /// the contract's reserved block is three ops wide with [`PRESENCE`] needing one of them. The
    /// client vocabulary keeps the two verbs apart (`world::guild::Op`); only the wire tag is shared.
    pub const ANSWER: u8 = 2;

    /// A member signed on or off — `actor_guid` tells the rest of its guild.
    pub const PRESENCE: u8 = 3;

    /// [`ANSWER`]'s `arg_a` for joining the guild.
    pub const ANSWER_ACCEPT: u32 = 1;
    /// [`ANSWER`]'s `arg_a` for refusing the invite.
    pub const ANSWER_DECLINE: u32 = 0;
    /// [`PRESENCE`]'s `arg_a` for a member who just entered the world.
    pub const PRESENCE_ON: u32 = 1;
    /// [`PRESENCE`]'s `arg_a` for a member who just left it.
    pub const PRESENCE_OFF: u32 = 0;
    /// `CMSG_GUILD_LEAVE` — `actor_guid` removes itself from its own guild.
    pub const LEAVE: u8 = 4;
    /// `CMSG_GUILD_REMOVE` — `actor_guid`, the guild master, removes `target_guid`.
    pub const REMOVE: u8 = 5;
    /// `CMSG_GUILD_DISBAND` — `actor_guid`, the guild master, destroys the guild.
    pub const DISBAND: u8 = 6;
    /// `CMSG_GUILD_LEADER` — `actor_guid`, the guild master, hands the guild to `target_guid`.
    pub const LEADER: u8 = 7;
    /// `CMSG_MESSAGECHAT` (`ChatType::Guild`) — `actor_guid` speaks `text` to its guild.
    pub const GUILD_CHAT: u8 = 10;
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
    /// The invite's TARGET already belongs to a guild. Deliberately shares no substring with
    /// [`ALREADY_IN_GUILD`]: the classifiers match by `contains`, and the two answer different
    /// `GuildCommandResult` codes (`AlreadyInGuildS` names a subject, `AlreadyInGuild` does not).
    pub const TARGET_IN_GUILD: &str = "target belongs to a guild";
    /// The invite's target is already looking at somebody else's invite dialog.
    pub const ALREADY_INVITED: &str = "target has a pending invite";
    /// No character of that name is online anywhere on the realm. Raised by the GATEWAY, which is
    /// the only party that can see every shard's characters; realm-core holds none.
    pub const TARGET_NOT_FOUND: &str = "no such player";
    /// Accept/decline with nothing to answer — never sent, already answered, or reaped by the
    /// invite GC. Dropped silently rather than shown: the dialog the client is answering is gone.
    pub const NO_PENDING_INVITE: &str = "no pending guild invite";
    /// The named character is in no guild, in a DIFFERENT guild, or names nobody at all. One
    /// refusal for all three: from the client's side they are the same typo.
    pub const TARGET_NOT_IN_GUILD: &str = "target is not in this guild";
    /// Guild-master succession is explicit. A master with members left behind must hand the guild
    /// on or disband it; the realm never promotes anybody on its own.
    pub const MASTER_MUST_TRANSFER_OR_DISBAND: &str =
        "the guild master must transfer the guild or disband it";
    /// A master removing themselves is a leave (or a disband), never a kick.
    pub const CANNOT_REMOVE_SELF: &str = "cannot remove yourself from the guild";
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

/// Encode a `GUILD_CHAT` payload: just the raw message text. Unlike [`encode_roster`]'s
/// multi-field grammar, there is nothing else to combine — the speaker's guid/name already ride
/// `GuildEvent.other_guid`/`other_name` — so this is a pass-through, mirroring
/// [`crate::group::encode_party_chat`].
pub fn encode_guild_chat(message: &str) -> String {
    message.to_string()
}

/// Decode a `GUILD_CHAT` payload back to the message text. Never fails (any string is a valid
/// message) — `Option` only for call-site symmetry with the other decoders, all of which DO fail
/// closed on malformed input.
pub fn decode_guild_chat(payload: &str) -> Option<String> {
    Some(payload.to_string())
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
        assert_eq!(realm_op::INVITE, 1);
        assert_eq!(realm_op::ANSWER, 2);
        assert_eq!(realm_op::PRESENCE, 3);
    }

    /// The event kind is the other wire value: the module writes it, the gateway's relay decodes it
    /// into a packet, and the two are deployed separately.
    #[test]
    fn guild_event_kinds_are_stable() {
        assert_eq!(event_kind::ROSTER, 0);
        assert_eq!(event_kind::INVITE, 1);
        assert_eq!(event_kind::JOINED, 2);
        assert_eq!(event_kind::DECLINED, 3);
        assert_eq!(event_kind::PRESENCE, 4);
    }

    /// A new member joins at the lowest rank that has a NAME. Rank 9 exists only so the packet's
    /// fixed `[String; 10]` can be filled, and a member sitting on it renders blank.
    #[test]
    fn a_new_member_joins_at_the_lowest_named_rank() {
        assert_eq!(DEFAULT_RANK_NAMES[GUILD_JOIN_RANK as usize], "Initiate");
        assert!(
            DEFAULT_RANK_NAMES[GUILD_JOIN_RANK as usize + 1..]
                .iter()
                .all(|n| n.is_empty()),
            "every rank below the join rank is nameless, so none of them may be the join rank"
        );
    }

    /// The invite gates are classified by `contains`, so two error strings that nest silently
    /// collapse into one wire code.
    #[test]
    fn the_guild_error_strings_do_not_nest() {
        let all = [
            err::ALREADY_IN_GUILD,
            err::NAME_TAKEN,
            err::NAME_INVALID,
            err::NOT_IN_GUILD,
            err::NOT_GUILD_MASTER,
            err::TARGET_IN_GUILD,
            err::ALREADY_INVITED,
            err::TARGET_NOT_FOUND,
            err::NO_PENDING_INVITE,
        ];
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate() {
                assert!(
                    i == j || !a.contains(b),
                    "`{a}` contains `{b}` — the gateway's classifier would answer the wrong code"
                );
            }
        }
    }

    /// The teardown block, pinned for the same reason: these numbers are on the wire between two
    /// separately deployed crates, and they are also the block this ticket owns — a renumber would
    /// silently collide with another op's.
    #[test]
    fn the_teardown_op_codes_and_event_kinds_are_stable() {
        assert_eq!(
            (
                realm_op::LEAVE,
                realm_op::REMOVE,
                realm_op::DISBAND,
                realm_op::LEADER
            ),
            (4, 5, 6, 7)
        );
        assert_eq!(
            (
                event_kind::LEFT,
                event_kind::REMOVED,
                event_kind::DISBANDED,
                event_kind::LEADER_CHANGED
            ),
            (5, 6, 7, 8)
        );
    }

    /// The gateway classifies a refusal by SUBSTRING, so one error string containing another would
    /// silently answer the wrong `GuildCommandResult` — a kick refusal rendered as "you are in no
    /// guild", say. No string may be a substring of any other.
    #[test]
    fn no_classified_error_string_shadows_another() {
        let all = [
            err::ALREADY_IN_GUILD,
            err::NAME_TAKEN,
            err::NAME_INVALID,
            err::NOT_IN_GUILD,
            err::NOT_GUILD_MASTER,
            err::TARGET_NOT_IN_GUILD,
            err::MASTER_MUST_TRANSFER_OR_DISBAND,
            err::CANNOT_REMOVE_SELF,
        ];
        for (i, outer) in all.iter().enumerate() {
            for (j, inner) in all.iter().enumerate() {
                assert!(
                    i == j || !outer.contains(inner),
                    "`{outer}` contains `{inner}` — the gateway's classifier cannot tell them apart"
                );
            }
        }
        assert_eq!(realm_op::GUILD_CHAT, 10);
        assert_eq!(event_kind::GUILD_CHAT, 11);
    }

    #[test]
    fn guild_chat_payload_round_trips_verbatim() {
        let message = "for the Alliance!".to_string();
        let wire = encode_guild_chat(&message);
        assert_eq!(decode_guild_chat(&wire), Some(message));
        // Unlike the roster grammar, a guild-chat payload has no delimiters to strip — it is the
        // sole field, so it must survive untouched (even one containing the roster's own framing
        // characters, which belong to a different kind's grammar entirely).
        assert_eq!(decode_guild_chat(""), Some(String::new()));
        assert_eq!(decode_guild_chat("a|b;c,d"), Some("a|b;c,d".to_string()));
    }
}
