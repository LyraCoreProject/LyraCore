//! The module↔gateway GROUP wire contract — event-kind codes, the roster-payload grammar, and the
//! reducer error strings the gateway classifies into `PARTY_COMMAND_RESULT`. Cross-boundary constants
//! live HERE, both crates import: a module-side renumber, delimiter change, or reworded `Err` becomes
//! a compile error on the gateway side instead of a runtime mismatch. Same precedent as
//! `type_mask`/`npc_flags`.

/// Group-event kinds (`game_group_event.kind`) — what SMSG the gateway relays.
pub mod event_kind {
    /// You are invited (`other_*` = the inviter) → `SMSG_GROUP_INVITE`.
    pub const INVITE: u8 = 0;
    /// Your group's roster changed → the gateway sends `SMSG_GROUP_LIST` from the row's payload
    /// (payload-carry: the roster is serialized in the SAME transaction as the membership change).
    pub const LIST: u8 = 1;
    /// Your invite was declined (`other_*` = the decliner) → `SMSG_GROUP_DECLINE`.
    pub const DECLINE: u8 = 2;
    /// You are no longer in a group (left / kicked / disbanded) → `SMSG_GROUP_DESTROYED`.
    pub const DESTROYED: u8 = 3;
    // --- work-item 187 (group loot methods) / work-item 221 (money-loot split) — the roll/master-
    // loot/money-share relay REUSES this same per-recipient event table + relay (see
    // `crate::loot_roll`'s module doc for why: identical shape — one recipient, a kind byte, a small
    // payload — so a parallel new table would only add binding-checklist ceremony for zero
    // behavioral gain). Kinds/payload grammar for these five live in `crate::loot_roll`, not here
    // (loot is a distinct concern from group membership) — listed here too ONLY as the reserved
    // kind-byte range so a future group-event kind can never collide with a loot-roll kind sharing
    // the same table.
    /// Reserved range start for `crate::loot_roll::event_kind` — kinds `4..=8` are loot-roll/money-
    /// share kinds relayed through `game_group_event`, not group-membership kinds. Any FUTURE
    /// group-membership kind must start at 10+ (9 is taken — see `PARTY_CHAT` below).
    pub const LOOT_ROLL_RESERVED_START: u8 = 4;
    /// Work-item 199: a party (`/p`) chat line, one row per recipient (every OTHER group member
    /// plus an echo to the sender) → `SMSG_MESSAGECHAT` with `ChatType::Party`. `other_guid`/
    /// `other_name` (auto-resolved by `push_event`) = the SPEAKER; `payload` = the message text
    /// via `encode_party_chat`. Reuses this SAME per-recipient relay for the identical reason the
    /// roll/master-loot/money-share kinds do (one recipient, a kind byte, a small payload) — the
    /// PRODUCER is `chat.rs` (a social-tier concern), not `group.rs`, but the byte still lives here
    /// since it shares this table's one kind-byte space with membership + loot-roll. Next free slot
    /// after `crate::loot_roll::event_kind::MONEY_SHARE` (8); any FUTURE new kind on this table
    /// (membership, loot-roll, or otherwise) must start at 10+.
    pub const PARTY_CHAT: u8 = 9;
}

/// Encode a `PARTY_CHAT` payload: just the raw message text. Unlike `encode_roster`'s multi-field
/// grammar, there is nothing else to combine — the speaker's guid/name already ride
/// `GroupEvent.other_guid`/`other_name` (resolved by `push_event`) — so this is a pass-through, kept
/// as a named function (rather than writing `row.payload` directly) for symmetry with the other
/// per-kind payload helpers (`encode_money_share` etc.) and so a future second field has one call
/// site to touch. Deliberately does NOT strip delimiter characters the way `encode_roster` strips
/// `|`/`;`/`,` from names — those are reserved for THAT kind's multi-field grammar; a party chat
/// payload is the sole field, so a message containing them must survive verbatim.
pub fn encode_party_chat(message: &str) -> String {
    message.to_string()
}

/// Decode a `PARTY_CHAT` payload back to the message text. Never fails (any string is a valid
/// message) — `Option` only for call-site symmetry with the other decoders, all of which DO fail
/// closed on malformed input.
pub fn decode_party_chat(payload: &str) -> Option<String> {
    Some(payload.to_string())
}

/// The REALM-CORE party ops (issue #22, group slice): the `op` byte of the single operator-gated
/// `realm_group_op` reducer the gateway drives realm-wide membership with.
///
/// One reducer with an op byte rather than six reducers, for one reason that is not taste: every
/// gateway-callable reducer needs a HAND-MAINTAINED SDK binding (`docs/danger-zones.md` §1.2 — the
/// generated `bindings/` tree is edited by hand in this repo), and six of them is six binding files
/// plus thirty `bindings/mod.rs` edit sites for six bodies that differ by two arguments. The op byte
/// is the cheap half of that trade; the expensive half — the ARGUMENT SHAPE — is pinned here, in the
/// one file both crates import, so a renumber or an argument-slot change is a compile-visible edit
/// on both sides rather than a silent mis-dispatch.
///
/// Argument slots (`realm_group_op(op, actor_guid, target_guid, arg_a, arg_b)`), per op:
/// - [`INVITE`] / [`UNINVITE`] — `target_guid` is the invitee/kicked member; `arg_a`/`arg_b` unused.
/// - [`ACCEPT`] / [`DECLINE`] / [`LEAVE`] — `actor_guid` alone; every other slot unused.
/// - [`LOOT_METHOD`] — `arg_a` = loot setting, `target_guid` = the master looter, `arg_b` = the
///   quality threshold. (That is `CMSG_LOOT_METHOD`'s own field order, kept so the gateway hands the
///   three values straight through.)
pub mod realm_op {
    /// `CMSG_GROUP_INVITE` — `actor_guid` invites `target_guid`.
    pub const INVITE: u8 = 0;
    /// `CMSG_GROUP_ACCEPT` — `actor_guid` accepts its pending invite.
    pub const ACCEPT: u8 = 1;
    /// `CMSG_GROUP_DECLINE` — `actor_guid` declines its pending invite.
    pub const DECLINE: u8 = 2;
    /// `CMSG_GROUP_DISBAND` (the client's "Leave Party") — `actor_guid` leaves its group.
    pub const LEAVE: u8 = 3;
    /// `CMSG_GROUP_UNINVITE` — leader `actor_guid` kicks `target_guid`.
    pub const UNINVITE: u8 = 4;
    /// `CMSG_LOOT_METHOD` — leader `actor_guid` sets the party's loot rules.
    pub const LOOT_METHOD: u8 = 5;
}

/// The group op one `game_bot_invite_intent` row asks the Gateway to run.
///
/// A Package writes that row for a Character with no Session, so there is no client behind it and no
/// `ctx.sender()` to resolve; the Gateway turns the byte into the matching [`realm_op`] against the
/// party authority. Two values rather than a reuse of [`realm_op`]: those six are what a CLIENT may
/// ask for, and a Package may only ask for these two. [`INVITE`] is `0` because the column was
/// END-appended to a table that had only ever carried invites, so a pre-existing row reads correctly
/// under the migration's own default.
pub mod bot_op {
    /// `inviter_guid` invites `target_guid` into a party.
    pub const INVITE: u8 = 0;
    /// `inviter_guid` leaves its party. `target_guid` is unused.
    pub const LEAVE: u8 = 1;
}

/// The group reducers' stable `Err` strings. Most map to `PartyResult` variants. The intent claim
/// result instead tells a losing Gateway callback to stop. Exact matches keep both crates in sync.
pub mod err {
    pub const ALREADY_IN_GROUP: &str = "already in a group";
    pub const BOT_INVITE_INTENT_ALREADY_CLAIMED: &str = "bot invite intent already claimed";
    pub const GROUP_FULL: &str = "group is full";
    pub const NOT_LEADER: &str = "not the leader";
    pub const NOT_IN_GROUP: &str = "not in a group";
    pub const TARGET_NOT_IN_GROUP: &str = "not in your group";
}

/// Encode the LIST payload: `leader,loot_method,loot_threshold,master_looter_guid|guid,name,online;...`.
/// The HEAD segment (work-item 187) carries the group's current loot settings alongside the leader —
/// so a `CMSG_LOOT_METHOD` change re-renders the party frame's loot block through this SAME relay
/// (`group.rs::roster_payload` is the one place both the roster and the loot settings are guaranteed
/// consistent in the SAME transaction). Names get the frame delimiters stripped HERE (character
/// creation only admits alphabetic names today, but that invariant lives elsewhere — the encoder owns
/// the grammar, so it owns the defense too).
pub fn encode_roster(
    leader: u64,
    loot_method: u8,
    loot_threshold: u8,
    master_looter_guid: u64,
    members: &[(u64, String, bool)],
) -> String {
    let members: Vec<String> = members
        .iter()
        .map(|(guid, name, online)| {
            let name: String = name.replace(['|', ';', ','], "_");
            format!("{guid},{name},{}", if *online { 1 } else { 0 })
        })
        .collect();
    format!(
        "{leader},{loot_method},{loot_threshold},{master_looter_guid}|{}",
        members.join(";")
    )
}

// The return tuple IS the decoded roster wire payload's field list (leader, kind, count, id, members).
#[allow(clippy::type_complexity)]
/// Decode a LIST payload back to `(leader, loot_method, loot_threshold, master_looter_guid,
/// [(guid, name, online)])`. `None` on any malformed input (the gateway fails closed rather than
/// shipping a corrupt roster).
pub fn decode_roster(payload: &str) -> Option<(u64, u8, u8, u64, Vec<(u64, String, bool)>)> {
    let (head, rest) = payload.split_once('|')?;
    let mut head_parts = head.split(',');
    let leader: u64 = head_parts.next()?.parse().ok()?;
    let loot_method: u8 = head_parts.next()?.parse().ok()?;
    let loot_threshold: u8 = head_parts.next()?.parse().ok()?;
    let master_looter_guid: u64 = head_parts.next()?.parse().ok()?;
    let mut members = Vec::new();
    for entry in rest.split(';').filter(|e| !e.is_empty()) {
        let mut parts = entry.split(',');
        let guid: u64 = parts.next()?.parse().ok()?;
        let name = parts.next()?.to_string();
        let online = parts.next()? == "1";
        members.push((guid, name, online));
    }
    (!members.is_empty()).then_some((
        leader,
        loot_method,
        loot_threshold,
        master_looter_guid,
        members,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_round_trips_including_the_delimiter_defense() {
        let members = vec![
            (2u64, "Ginger".to_string(), true),
            (3u64, "df|s;d,fsd".to_string(), false), // hostile name → encoder strips the frame
        ];
        let wire = encode_roster(2, 3, 2, 0, &members);
        let (leader, loot_method, loot_threshold, master_looter_guid, decoded) =
            decode_roster(&wire).unwrap();
        assert_eq!(leader, 2);
        assert_eq!(loot_method, 3);
        assert_eq!(loot_threshold, 2);
        assert_eq!(master_looter_guid, 0);
        assert_eq!(decoded[0], (2, "Ginger".to_string(), true));
        assert_eq!(decoded[1], (3, "df_s_d_fsd".to_string(), false));
    }

    /// The HEAD segment (work-item 187) round-trips the group's current loot settings — a non-default
    /// method/threshold/master survives the encode/decode round trip untouched.
    #[test]
    fn roster_round_trips_non_default_loot_settings() {
        let members = vec![(9u64, "Bob".to_string(), true)];
        let wire = encode_roster(9, 2 /* MASTER */, 4 /* Epic */, 42, &members);
        let (leader, loot_method, loot_threshold, master_looter_guid, _) =
            decode_roster(&wire).unwrap();
        assert_eq!(leader, 9);
        assert_eq!(loot_method, 2);
        assert_eq!(loot_threshold, 4);
        assert_eq!(master_looter_guid, 42);
    }

    #[test]
    fn decode_fails_closed_on_garbage() {
        assert!(decode_roster("").is_none());
        assert!(decode_roster("x,3,2,0|1,A,1").is_none()); // non-numeric leader
        assert!(decode_roster("2,3,2,0|").is_none()); // empty roster
        assert!(decode_roster("2|1,A,1").is_none()); // missing loot-settings head fields entirely
        assert!(decode_roster("2,3|1,A,1").is_none()); // partial head (missing threshold/master)
    }

    #[test]
    fn decode_fails_closed_on_malformed_member_fields() {
        assert!(decode_roster("2,3,2,0|x,Bob,1").is_none()); // non-numeric guid
        assert!(decode_roster("2,3,2,0|5,Bob").is_none()); // missing online field
        assert!(decode_roster("2,3,2,0|5").is_none()); // missing name and online fields entirely
                                                       // An online field that isn't exactly "1" decodes successfully as offline (not a parse failure) —
                                                       // the grammar treats it as a bool flag, not an enum, so garbage there silently means "offline".
        let (.., members) = decode_roster("2,3,2,0|5,Bob,maybe").unwrap();
        assert_eq!(members[0], (5, "Bob".to_string(), false));
    }

    // ---- Party chat (work-item 199) ----

    /// Unlike `encode_roster`'s multi-field grammar, a party-chat payload has exactly one field —
    /// so, unlike a hostile roster NAME, delimiter characters in the MESSAGE survive verbatim rather
    /// than being stripped (there is no frame to protect: nothing else shares the payload string).
    #[test]
    fn party_chat_round_trips_without_stripping_delimiter_characters() {
        let message = "df|s;d,fsd — hi!".to_string();
        let wire = encode_party_chat(&message);
        assert_eq!(
            wire, message,
            "no escaping should be applied — it's the sole payload field"
        );
        assert_eq!(decode_party_chat(&wire), Some(message));
    }

    #[test]
    fn party_chat_decode_never_fails_closed() {
        // Unlike every other decoder on this table, an empty/arbitrary string is a VALID message —
        // there is no grammar to violate.
        assert_eq!(decode_party_chat(""), Some(String::new()));
        assert_eq!(
            decode_party_chat("anything at all"),
            Some("anything at all".to_string())
        );
    }

    /// Work-item 199: `PARTY_CHAT` (9) is the next free slot after the loot-roll reserved range and
    /// must not collide with any group-membership kind (0-3) or loot-roll/money-share kind (4-8).
    #[test]
    fn party_chat_kind_is_the_next_free_slot_after_the_loot_roll_range() {
        assert_eq!(
            event_kind::PARTY_CHAT,
            event_kind::LOOT_ROLL_RESERVED_START + 5
        );
        assert_eq!(event_kind::PARTY_CHAT, 9);
    }

    // ---- Realm-core party ops (issue #22, group slice) ----

    /// The intent op byte is a MIGRATION value as well as a wire one: `op` was END-appended to
    /// `game_bot_invite_intent` with a `0` default, so every row written before the column existed
    /// reads as an INVITE. Renumbering would turn those rows into leaves.
    #[test]
    fn bot_group_intent_op_codes_are_stable_and_distinct() {
        assert_eq!(bot_op::INVITE, 0, "the END-appended column defaults to 0");
        assert_eq!(bot_op::LEAVE, 1);
        assert_ne!(bot_op::INVITE, bot_op::LEAVE);
    }

    /// The op byte is a WIRE value: the gateway sends it, the module dispatches on it, and the two
    /// are deployed separately. A renumber that only one side learns about silently runs the wrong
    /// op — an ACCEPT arriving as a LEAVE — so the numbering is pinned, not merely defined.
    #[test]
    fn realm_group_op_codes_are_stable_and_distinct() {
        assert_eq!(realm_op::INVITE, 0);
        assert_eq!(realm_op::ACCEPT, 1);
        assert_eq!(realm_op::DECLINE, 2);
        assert_eq!(realm_op::LEAVE, 3);
        assert_eq!(realm_op::UNINVITE, 4);
        assert_eq!(realm_op::LOOT_METHOD, 5);
        let all = [
            realm_op::INVITE,
            realm_op::ACCEPT,
            realm_op::DECLINE,
            realm_op::LEAVE,
            realm_op::UNINVITE,
            realm_op::LOOT_METHOD,
        ];
        let mut sorted = all.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            all.len(),
            "two realm group ops share an op byte"
        );
    }
}
