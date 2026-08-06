//! The module↔gateway NEED/GREED-ROLL + MASTER-LOOTER wire contract (work-item 187, slices 2-4).
//!
//! **Relay-pattern decision**: rolls/master-loot notifications ride the SAME per-recipient
//! `game_group_event` table + relay the group system already uses for INVITE/LIST/DECLINE/DESTROYED
//! (`crate::group`) — they are NOT a new gateway-subscribed table. The shape every one of these
//! needs is identical: one recipient (RLS-scoped by identity), a small kind byte, and a compact
//! payload the recipient's own connection decodes. Standing up a parallel `game_loot_roll_event`
//! table would require the FULL new-table gateway-binding checklist (binding file pair + ~13
//! `bindings/mod.rs` splice sites) for zero behavioral gain over reusing the existing relay — the
//! `game_group_event.kind` byte already discriminates payload shape per-row, which is exactly what a
//! second table would also need to do internally. `crate::group::event_kind::LOOT_ROLL_RESERVED_START`
//! documents the kind-byte range reserved here so the two producers (group.rs, loot.rs, both
//! module-side) can never collide on a kind number.
//!
//! The actual per-item roll STATE (`game_loot_roll` / `game_loot_roll_vote`) lives in
//! `module/src/loot.rs`. Client-visible transitions (start / a vote lands / the winner is decided /
//! the master's eligible-recipient list) ride the SAME `game_group_event` relay this doc describes —
//! the gateway never needs a wire binding to `game_loot_roll` for the CLIENT'S sake, and that rule is
//! unchanged by issue #50 below.
//!
//! **Issue #50** moves `game_loot_roll` / `game_loot_roll_vote` to be authoritative on realm-core,
//! alongside `game_group` / `game_group_member` (#22) — a roll's audience is defined by who is in the
//! group, not by where anyone stands, exactly like the membership it depends on. Unlike #22's
//! membership tables, nothing else in the module reads a roll's internal vote state (only the
//! lifecycle functions in `loot.rs` do), so there is no world-shard MIRROR to keep in sync — instead
//! the gateway's coordinator DOES subscribe `game_loot_roll` / `game_loot_roll_vote` now (a first for
//! this pair), purely so its loot-roll relay (`gateway/src/world/loot.rs`) can see a freshly-created
//! roll on a world shard and PROMOTE it onto realm-core (`realm_loot_op`'s `loot_op::START`), then
//! clear the shard's transient staging copy. Voting, once promoted, is routed the same way
//! (`loot_op::VOTE`); the winning GRANT settles back onto the corpse's own world shard through a
//! separate operator reducer (`settle_loot_roll`) the relay calls after observing the `ROLL_WON`
//! event realm-core pushes — items and their escrow guarantees never leave the world shard.

/// Kinds carried in `game_group_event.kind` for a loot-roll/master-loot/money-share notification
/// (work-item 187 slices 2-4, work-item 221). Reserved range `4..=8` — see
/// `crate::group::event_kind::LOOT_ROLL_RESERVED_START`.
pub mod event_kind {
    /// A need/greed/pass window opened for one item → `SMSG_LOOT_START_ROLL`.
    pub const ROLL_START: u8 = 4;
    /// A member's vote landed (server already rolled 1-100 for a need/greed vote) → `SMSG_LOOT_ROLL`,
    /// relayed to every eligible member so the live vote/roll numbers are visible party-wide.
    pub const ROLL_VOTE: u8 = 5;
    /// The roll resolved to a winner → `SMSG_LOOT_ROLL_WON`. Never fired for an all-pass resolution
    /// (the row just unlocks FFA-in-group; vanilla shows no "won" line when everyone passes).
    pub const ROLL_WON: u8 = 6;
    /// The master looter's eligible-recipient list for a just-appeared above-threshold row →
    /// `SMSG_LOOT_MASTER_LIST`. Sent to the master only.
    pub const MASTER_LIST: u8 = 7;
    /// Work-item 221: a grouped money-loot split resolved — this recipient's copper share →
    /// `SMSG_LOOT_MONEY_NOTIFY(share)`. Next free kind after `MASTER_LIST`; reuses the SAME
    /// per-recipient relay (one recipient, a kind byte, a small payload) rather than a new gateway-
    /// subscribed table, for the identical reasons the roll/master-loot kinds above do. Pushed to
    /// EVERY recipient of a grouped split, including the looter itself (vanilla shows the looter its
    /// own "Your share of the loot is X" line too, not the solo "You loot X copper" line) — a SOLO
    /// money loot never pushes this kind at all (no `game_corpse_loot_eligible` snapshot for that
    /// corpse), so the gateway sends no notify and the client's own local "You loot" line is the only
    /// feedback (work-item 221's whole fix).
    pub const MONEY_SHARE: u8 = 8;
}

/// The REALM-CORE loot-roll ops (issue #50): the `op` byte of the operator-gated `realm_loot_op`
/// reducer, mirroring `crate::group::realm_op`'s one-reducer-not-several trade (see that module's
/// doc for why: a hand-maintained gateway binding per reducer, `docs/danger-zones.md` §1.2).
///
/// Argument slots (`realm_loot_op(op, corpse_guid, slot, item_entry, actor_guid, vote,
/// deadline_micros, recipients)`), per op:
/// - [`START`] — `corpse_guid`/`slot`/`item_entry`/`deadline_micros`/`recipients` carry the roll a
///   world shard already announced locally (`item_entry`/`deadline_micros` preserved so promotion
///   never restarts the 60s clock); `actor_guid`/`vote` unused.
/// - [`VOTE`] — `corpse_guid`/`slot`/`vote` as `CMSG_LOOT_ROLL` sends them; `actor_guid` is the
///   voter (the gateway's own `InWorld::self_guid`, never a client-supplied literal); `item_entry`/
///   `deadline_micros`/`recipients` unused.
pub mod loot_op {
    /// Promote a world shard's freshly-created roll onto realm-core.
    pub const START: u8 = 0;
    /// `CMSG_LOOT_ROLL` — `actor_guid` casts `vote` on the roll open at `(corpse_guid, slot)`.
    pub const VOTE: u8 = 1;
}

/// Vote kinds (`game_loot_roll_vote.vote`) — matches the vanilla wire `RollVote` enum EXACTLY
/// (`Pass=0, Need=1, Greed=2`; `Disenchant=3` unused here) so a vote byte from `CMSG_LOOT_ROLL`
/// (gateway-decoded via `RollVote::as_int()`) round-trips to the module with ZERO translation table —
/// avoiding the #1 silent-bug shape in this codebase: an invented enum ordering that doesn't match
/// the wire.
pub mod vote_kind {
    pub const PASS: u8 = 0;
    pub const NEED: u8 = 1;
    pub const GREED: u8 = 2;
}

/// Encode a `ROLL_START` payload: `corpse_guid,slot,item_entry,countdown_ms`.
pub fn encode_start(corpse_guid: u64, slot: u8, item_entry: u32, countdown_ms: u32) -> String {
    format!("{corpse_guid},{slot},{item_entry},{countdown_ms}")
}

/// Decode a `ROLL_START` payload back to `(corpse_guid, slot, item_entry, countdown_ms)`.
pub fn decode_start(payload: &str) -> Option<(u64, u8, u32, u32)> {
    let mut p = payload.split(',');
    let corpse_guid: u64 = p.next()?.parse().ok()?;
    let slot: u8 = p.next()?.parse().ok()?;
    let item_entry: u32 = p.next()?.parse().ok()?;
    let countdown_ms: u32 = p.next()?.parse().ok()?;
    if p.next().is_some() {
        return None; // trailing garbage — fail closed
    }
    Some((corpse_guid, slot, item_entry, countdown_ms))
}

/// Encode a `ROLL_VOTE` payload: `corpse_guid,slot,item_entry,roll_number,vote_kind,auto_pass`.
/// The voter's own guid rides `GroupEvent.other_guid` (already resolved/looked-up by `push_event`),
/// not the payload — mirrors INVITE/DECLINE's convention.
pub fn encode_vote(
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    roll_number: u8,
    vote: u8,
    auto_pass: bool,
) -> String {
    format!(
        "{corpse_guid},{slot},{item_entry},{roll_number},{vote},{}",
        auto_pass as u8
    )
}

/// Decode a `ROLL_VOTE` payload back to `(corpse_guid, slot, item_entry, roll_number, vote_kind, auto_pass)`.
pub fn decode_vote(payload: &str) -> Option<(u64, u8, u32, u8, u8, bool)> {
    let mut p = payload.split(',');
    let corpse_guid: u64 = p.next()?.parse().ok()?;
    let slot: u8 = p.next()?.parse().ok()?;
    let item_entry: u32 = p.next()?.parse().ok()?;
    let roll_number: u8 = p.next()?.parse().ok()?;
    let vote: u8 = p.next()?.parse().ok()?;
    let auto_pass = p.next()? == "1";
    if p.next().is_some() {
        return None;
    }
    Some((corpse_guid, slot, item_entry, roll_number, vote, auto_pass))
}

/// Encode a `ROLL_WON` payload: `corpse_guid,slot,item_entry,winning_roll,winning_vote`. The
/// winner's guid rides `GroupEvent.other_guid`, matching `encode_vote`'s convention.
/// `winning_vote` is the tier that won (`vote_kind::NEED` or `GREED`) — without it the wire's
/// won-line misreports every greed-only win as a Need win (187 review finding #3).
pub fn encode_won(
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    winning_roll: u8,
    winning_vote: u8,
) -> String {
    format!("{corpse_guid},{slot},{item_entry},{winning_roll},{winning_vote}")
}

/// Decode a `ROLL_WON` payload back to `(corpse_guid, slot, item_entry, winning_roll, winning_vote)`.
pub fn decode_won(payload: &str) -> Option<(u64, u8, u32, u8, u8)> {
    let mut p = payload.split(',');
    let corpse_guid: u64 = p.next()?.parse().ok()?;
    let slot: u8 = p.next()?.parse().ok()?;
    let item_entry: u32 = p.next()?.parse().ok()?;
    let winning_roll: u8 = p.next()?.parse().ok()?;
    let winning_vote: u8 = p.next()?.parse().ok()?;
    if p.next().is_some() {
        return None;
    }
    Some((corpse_guid, slot, item_entry, winning_roll, winning_vote))
}

/// Encode a `MASTER_LIST` payload: `corpse_guid|guid,guid,...` (the eligible-recipient set).
pub fn encode_master_list(corpse_guid: u64, eligible: &[u64]) -> String {
    let guids: Vec<String> = eligible.iter().map(|g| g.to_string()).collect();
    format!("{corpse_guid}|{}", guids.join(","))
}

/// Decode a `MASTER_LIST` payload back to `(corpse_guid, [eligible guids])`.
pub fn decode_master_list(payload: &str) -> Option<(u64, Vec<u64>)> {
    let (corpse_guid, rest) = payload.split_once('|')?;
    let corpse_guid: u64 = corpse_guid.parse().ok()?;
    let mut guids = Vec::new();
    for g in rest.split(',').filter(|s| !s.is_empty()) {
        guids.push(g.parse().ok()?);
    }
    (!guids.is_empty()).then_some((corpse_guid, guids))
}

/// Encode a `MONEY_SHARE` payload: just the recipient's copper share — `SMSG_LOOT_MONEY_NOTIFY`
/// carries no other field, unlike the roll payloads above (no corpse/slot/item to round-trip).
pub fn encode_money_share(amount: u32) -> String {
    amount.to_string()
}

/// Decode a `MONEY_SHARE` payload back to the recipient's copper share.
pub fn decode_money_share(payload: &str) -> Option<u32> {
    payload.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_round_trips() {
        let wire = encode_start(555, 2, 1234, 60_000);
        assert_eq!(decode_start(&wire), Some((555, 2, 1234, 60_000)));
    }

    #[test]
    fn start_fails_closed_on_garbage() {
        assert!(decode_start("").is_none());
        assert!(decode_start("1,2,3").is_none()); // missing countdown
        assert!(decode_start("1,2,3,4,5").is_none()); // trailing garbage
        assert!(decode_start("x,2,3,4").is_none());
    }

    #[test]
    fn vote_round_trips_including_auto_pass() {
        let wire = encode_vote(555, 2, 1234, 87, vote_kind::NEED, false);
        assert_eq!(
            decode_vote(&wire),
            Some((555, 2, 1234, 87, vote_kind::NEED, false))
        );
        let auto = encode_vote(555, 2, 1234, 0, vote_kind::PASS, true);
        assert_eq!(
            decode_vote(&auto),
            Some((555, 2, 1234, 0, vote_kind::PASS, true))
        );
    }

    #[test]
    fn vote_kind_matches_the_real_wire_rollvote_ordering() {
        // RollVote::Pass=0, Need=1, Greed=2 (wow_world_base) — a mismatch here would silently
        // misclassify every vote decoded from CMSG_LOOT_ROLL.
        assert_eq!(vote_kind::PASS, 0);
        assert_eq!(vote_kind::NEED, 1);
        assert_eq!(vote_kind::GREED, 2);
    }

    #[test]
    fn won_round_trips() {
        let wire = encode_won(555, 2, 1234, 96, vote_kind::GREED);
        assert_eq!(
            decode_won(&wire),
            Some((555, 2, 1234, 96, vote_kind::GREED))
        );
    }

    #[test]
    fn master_list_round_trips() {
        let wire = encode_master_list(555, &[10, 20, 30]);
        assert_eq!(decode_master_list(&wire), Some((555, vec![10, 20, 30])));
    }

    #[test]
    fn master_list_fails_closed_on_garbage() {
        assert!(decode_master_list("").is_none());
        assert!(decode_master_list("555|").is_none()); // no eligible guids at all
        assert!(decode_master_list("555|1,x,3").is_none());
    }

    #[test]
    fn reserved_kind_range_does_not_collide_with_group_event_kinds() {
        use crate::group::event_kind as g;
        assert_eq!(g::LOOT_ROLL_RESERVED_START, event_kind::ROLL_START);
        const { assert!(event_kind::ROLL_START > crate::group::event_kind::DESTROYED) };
        assert_eq!(event_kind::ROLL_VOTE, event_kind::ROLL_START + 1);
        assert_eq!(event_kind::ROLL_WON, event_kind::ROLL_VOTE + 1);
        assert_eq!(event_kind::MASTER_LIST, event_kind::ROLL_WON + 1);
        assert_eq!(event_kind::MONEY_SHARE, event_kind::MASTER_LIST + 1);
    }

    /// Work-item 199: every group-membership kind (0-3), loot-roll/money-share kind (4-8), and the
    /// party-chat kind (9) is pairwise distinct — a future renumber that silently collides two kinds
    /// would corrupt the relay (the gateway match would resolve the wrong SMSG for a row).
    #[test]
    fn all_group_and_loot_event_kinds_are_distinct_0_through_9() {
        use crate::group::event_kind as g;
        let mut kinds = vec![
            g::INVITE,
            g::LIST,
            g::DECLINE,
            g::DESTROYED,
            event_kind::ROLL_START,
            event_kind::ROLL_VOTE,
            event_kind::ROLL_WON,
            event_kind::MASTER_LIST,
            event_kind::MONEY_SHARE,
            g::PARTY_CHAT,
        ];
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds, (0u8..=9).collect::<Vec<u8>>());
    }

    #[test]
    fn money_share_round_trips() {
        let wire = encode_money_share(37);
        assert_eq!(decode_money_share(&wire), Some(37));
        let zero = encode_money_share(0);
        assert_eq!(decode_money_share(&zero), Some(0));
    }

    #[test]
    fn money_share_fails_closed_on_garbage() {
        assert!(decode_money_share("").is_none());
        assert!(decode_money_share("12x").is_none());
        assert!(decode_money_share("-1").is_none()); // u32, no sign
    }

    // ---- Realm-core loot-roll ops (issue #50) ----

    /// The op byte is a WIRE value the gateway sends and the module dispatches on, deployed
    /// separately — a renumber only one side learns about silently runs the wrong op (a VOTE
    /// arriving as a START). Mirrors `realm_group_op_codes_are_stable_and_distinct` in `group.rs`.
    #[test]
    fn realm_loot_op_codes_are_stable_and_distinct() {
        assert_eq!(loot_op::START, 0);
        assert_eq!(loot_op::VOTE, 1);
    }
}
