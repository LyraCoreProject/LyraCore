//! GROUP LOOT METHODS (work-item 187 slices 1-4) — round-robin / need-greed rolls / master looter.
//! Split out of `loot.rs` into its own submodule: the decision enum, the rr cursor,
//! `LootRoll`/`LootRollVote`, vote/resolve/sweep/disband, and the realm-core plane. Pure code motion
//! — every gate and grant below is byte-identical to before the split.
//!
//! Timing decision (diverges from the work item's own draft, documented here): ALL group-loot
//! stamping — round-robin/master designation AND spawning need/greed rolls — happens at KILL TIME
//! (`apply_group_loot_rules`, called once from `combat::kill_creature` right after
//! `roll_creature_loot`), not lazily "at first loot-open" as the work item's flow draft describes.
//! Folding it into the kill path (already this slice's territory) avoids a NEW gateway reducer +
//! CMSG_LOOT dispatch change just to detect "first viewer" — and the observable result is
//! equivalent: eligible members get `SMSG_LOOT_START_ROLL` immediately after the kill instead of
//! exactly-when-someone-opens-the-corpse. Round-robin designation was ALREADY spec'd for
//! KILL-time creation in the work item's own design ("corpse gets `designated_looter_guid`
//! stamped AT CREATION"), so this only extends that timing to the roll-spawn half too.
//!
//! Relay-pattern decision: rolls/master-list notifications reuse the EXISTING `game_group_event`
//! per-recipient relay (`crate::group::push_event`) instead of a new gateway-subscribed table — see
//! `lyracore_shared::loot_roll`'s module doc for the full rationale. The actual roll STATE
//! (`LootRoll`/`LootRollVote` below) is never read by the gateway for the CLIENT's sake.
//!
//! **Where the roll DECISION lives.** `LootRoll`/`LootRollVote` are authoritative on
//! REALM-CORE, alongside `game_group`/`game_group_member` — a roll's audience is who is in the
//! group, not where anyone stands, exactly like the membership it is snapshotted from. Kill-time
//! creation (`start_roll`, below) still runs on the corpse's own WORLD SHARD unconditionally — combat
//! resolution has no path to another database — so in a sharded deployment this write is a TRANSIENT
//! staging copy: the gateway's loot-roll relay (`gateway/src/world/loot.rs::relay_tick`) promotes it
//! onto realm-core (`realm_loot_op`'s `loot_op::START`, calling `insert_roll_rows` again there WITHOUT
//! re-pushing `ROLL_START` — the popup already fired locally) and clears the shard's copy
//! (`clear_promoted_loot_roll`). Voting then routes to realm-core too (`loot_op::VOTE` → `cast_vote_on`
//! on realm-core's `ctx.db`), so `remove_member`'s disband branch — unchanged, still
//! `crate::loot::force_resolve_rolls_for_disband` — resolves a live roll in the SAME transaction as
//! the membership change, on the SAME database, with no mirror and no round trip: whichever database
//! `remove_member` executes on is also the one its `game_loot_roll` rows are the truth on. Unsharded,
//! none of this promotion/routing ever runs (`WorldStore::realm_store()` answers `None`), so the roll
//! lives and dies on one database exactly as it did before this issue — byte-identical.
//!
//! **What did NOT move.** Only the roll — the group-scoped decision — travels. `game_corpse_loot`
//! (which corpse, which slot, item quality) and the actual item GRANT stay on the world shard with
//! their existing escrow guarantees (`items::grant_item`); a winner decided on realm-core is handed
//! their item by `settle_loot_roll`, an operator reducer the relay calls on the corpse's OWN shard
//! after observing the `ROLL_WON` event. `game_corpse_loot_eligible` (who is in range to roll) is
//! computed spatially at kill time and stays world-shard-local too — it is `apply_group_loot_rules`'s
//! own `recipients` snapshot, not part of the roll's realm-core state.
//!
//! **The new case.** A roll's PARTICIPANTS could not previously be on different shards —
//! `kill_reward_recipients` only ever names members who are physically near the corpse, i.e. on its
//! own shard, at the moment the roll starts. The new case is a participant LEAVING that
//! shard mid-roll (a portal, a dungeon entry) during the 60s window. Before, that voter's row was
//! simply unreachable from wherever they went — their vote auto-passed at the deadline, same as being
//! offline. Now it is reachable: voting is realm-core state, and the gateway routes a vote
//! through whichever shard the player is CURRENTLY on, so a mid-roll shard-hopper can still vote from
//! their new location. No special-casing was needed for this — it falls out of routing votes to
//! realm-core rather than to "the shard that created the roll".

use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::game_corpse_loot;
use crate::game_group; // Group row: loot_method/loot_threshold/rr_cursor/master_looter_guid
use crate::game_item_template;
use crate::game_world_entity;
use lyracore_shared::loot::LootRefusal;
use lyracore_shared::loot_roll::{event_kind as roll_event_kind, vote_kind};

use super::{refused, CorpseLoot};

/// A member of a kill's loot-eligible set, resolved from the Loot Tag at death. One row per
/// `(corpse_guid, recipient)`. Private, no
/// RLS — only the module reads it (roll-vote snapshotting, `loot_master_give`'s recipient gate).
/// `eligible_guid` (not `character_guid`) deliberately dodges the crate's `character_owned` sweep-
/// marker tripwire (`tripwires.rs`): these rows are transient, corpse-lifetime data reaped alongside
/// `game_corpse_loot` on decay (`creatures::tick::pass_decay`) — never durable per-character state, so
/// they need no delete/restamp sweep on character delete/relog (a stale row just never matches any
/// live roll again). [entity]
#[table(accessor = game_corpse_loot_eligible, index(accessor = by_corpse, btree(columns = [corpse_guid])))]
pub struct CorpseLootEligible {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub corpse_guid: u64,
    pub eligible_guid: u64,
}

/// A live NEED/GREED/NBG roll on one `game_corpse_loot` row. Private — the gateway never reads this
/// (relayed via `game_group_event` instead, see the module doc above). `resolved` is the exactly-once
/// resolution guard (work-item 187 trap): set the moment `resolve_roll` decides an outcome, checked
/// FIRST on every entry point (the deadline sweep AND a landing vote can both reach the same roll).
/// [entity]
#[derive(Clone)]
#[table(accessor = game_loot_roll, index(accessor = by_corpse, btree(columns = [corpse_guid])))]
pub struct LootRoll {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub corpse_guid: u64,
    pub slot: u8,
    pub item_entry: u32,
    pub deadline_micros: i64,
    pub resolved: bool,
}

/// One eligible member's vote on a [`LootRoll`] — snapshotted (one row per eligible guid) the moment
/// the roll starts, so a member joining the group later never joins a live roll. `voted=false` rows
/// auto-pass at resolution (deadline OR all-voted). `rolled` is assigned THE MOMENT a NEED/GREED vote
/// lands (mirrors real vanilla: `SMSG_LOOT_ROLL`'s roll number is generated at vote time, not
/// deferred to resolution) — `0` for a pass (real or auto-) and never read for one. Private, no RLS
/// (the module resolves rolls server-side; the gateway never reads vote state directly). `voter_guid`
/// (not `character_guid`) dodges the `character_owned` tripwire — same transient-data rationale as
/// [`CorpseLootEligible`]. [entity]
#[derive(Clone)]
#[table(accessor = game_loot_roll_vote, index(accessor = by_roll, btree(columns = [roll_id])))]
pub struct LootRollVote {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub roll_id: u64,
    pub voter_guid: u64,
    pub voted: bool,
    pub vote: u8,   // vote_kind::* — meaningful only if `voted`
    pub rolled: u8, // 1-100 once a NEED/GREED vote lands; 0 otherwise
}

/// Roll-window countdown (vanilla: 60s).
pub(crate) const ROLL_WINDOW_MICROS: i64 = 60_000_000;

/// Which handling applies to ONE `game_corpse_loot` row under the group's current `method`/
/// `threshold`, given the row's item `quality`? Pure — the single decision table every group-loot
/// stamping choice below reduces to; unit-tested as the "threshold split" matrix. An unrecognized
/// `method` (shouldn't happen — `group_loot_method` validates it) degrades to `Ffa` (never invents a
/// restriction from a bad value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GroupLootDecision {
    /// No restriction — behaves exactly as before this feature (FFA method, or a bad/unknown value).
    Ffa,
    /// Round-robin/below-threshold-GROUP: stamp the corpse's one pre-picked designee.
    Designate,
    /// Above-threshold under GROUP or any row under NEED_BEFORE_GREED: spawn a need/greed/pass roll.
    Roll,
    /// Above-threshold under MASTER: stamp the master + `master_only`.
    Master,
}

pub(crate) fn group_loot_decision_for_row(
    method: u8,
    quality: u8,
    threshold: u8,
) -> GroupLootDecision {
    use crate::group::loot_method::*;
    match method {
        FFA => GroupLootDecision::Ffa,
        ROUND_ROBIN => GroupLootDecision::Designate,
        MASTER => {
            if quality >= threshold {
                GroupLootDecision::Master
            } else {
                GroupLootDecision::Designate
            }
        }
        GROUP => {
            if quality >= threshold {
                GroupLootDecision::Roll
            } else {
                GroupLootDecision::Designate
            }
        }
        NEED_BEFORE_GREED => GroupLootDecision::Roll,
        _ => GroupLootDecision::Ffa,
    }
}

/// Round-robin cursor advance (work-item 187 trap: "skipping offline members"): given a STABLE
/// member ordering's online bitmap and the group's current `cursor`, return the chosen member's
/// INDEX into that same ordering plus the cursor value to persist for NEXT time (wrapping). `None`
/// if nobody is online (a fully-offline group — degrades to FFA, never panics/blocks looting). Pure.
pub(crate) fn advance_rr_cursor(online: &[bool], cursor: u32) -> Option<(usize, u32)> {
    let n = online.len();
    if n == 0 {
        return None;
    }
    let start = (cursor as usize) % n;
    for step in 0..n {
        let idx = (start + step) % n;
        if online[idx] {
            return Some((idx, (idx as u32 + 1) % n as u32));
        }
    }
    None
}

/// Given every ELIGIBLE (voted-or-auto-passed) member's vote kind, which tier contends for the item —
/// NEED beats GREED, an empty result means everyone passed (the row unlocks FFA-in-group)? Pure.
/// Returns the CONTENDING members' indices into `votes` (order-preserving).
pub(crate) fn contending_tier(votes: &[u8]) -> Vec<usize> {
    let need: Vec<usize> = votes
        .iter()
        .enumerate()
        .filter(|(_, &v)| v == vote_kind::NEED)
        .map(|(i, _)| i)
        .collect();
    if !need.is_empty() {
        return need;
    }
    votes
        .iter()
        .enumerate()
        .filter(|(_, &v)| v == vote_kind::GREED)
        .map(|(i, _)| i)
        .collect()
}

/// Pick the winner among a contending tier's already-rolled 1-100 values: the unique max, or `None`
/// on a TIE (multiple share the max — cmangos re-rolls internally and re-decides; the caller loops
/// this with fresh rolls for the tied members until it resolves, then announces ONCE). Pure.
pub(crate) fn pick_roll_winner(rolls: &[u8]) -> Option<usize> {
    if rolls.is_empty() {
        return None;
    }
    let max = *rolls.iter().max().unwrap();
    let mut winners = rolls
        .iter()
        .enumerate()
        .filter(|(_, &r)| r == max)
        .map(|(i, _)| i);
    let first = winners.next()?;
    if winners.next().is_some() {
        None // a second index shares the max — a tie
    } else {
        Some(first)
    }
}

/// KILL-TIME group-loot stamping (module doc above explains the timing choice): called once from
/// `combat::kill_creature` right after `roll_creature_loot`, for a grouped Loot Tag only. The
/// corpse-eligibility rows are the one recipient set for designation, master loot, and rolls.
///
/// Work-item 187 trap ("solo player with method GROUP set: threshold rows must NOT roll"): a solo
/// `recipients` (`len() < 2`, e.g. every other member out of XP range or dead) skips ALL group-loot
/// handling too — vanilla's "party size 1 -> direct loot" applies to loot exactly like it does to
/// the XP split, so the same recipient-count gate covers both.
pub(crate) fn apply_group_loot_rules(ctx: &ReducerContext, corpse_guid: u64, group_id: u64) {
    let recipients = super::tag::corpse_eligible_recipients(ctx, corpse_guid);
    if recipients.len() < 2 {
        return;
    }
    let Some(mut group) = ctx.db.game_group().group_id().find(group_id) else {
        return;
    };

    if group.loot_method == crate::group::loot_method::FFA {
        return;
    }

    // One STABLE member ordering (sorted by character_guid — deterministic regardless of join
    // order) so the rr_cursor index means the same thing kill after kill.
    let mut members = recipients.clone();
    members.sort_unstable();
    let online: Vec<bool> = members
        .iter()
        .map(|g| ctx.db.game_world_entity().guid().find(g).is_some())
        .collect();
    let mut rr_cursor_dirty = false;
    let designee = advance_rr_cursor(&online, group.rr_cursor).map(|(idx, next_cursor)| {
        group.rr_cursor = next_cursor;
        rr_cursor_dirty = true;
        members[idx]
    });
    let preferred_master = if group.master_looter_guid != 0 {
        group.master_looter_guid
    } else {
        group.leader_guid
    };
    let master_guid = recipients
        .contains(&preferred_master)
        .then_some(preferred_master)
        .or_else(|| recipients.first().copied())
        .expect("two eligible recipients were checked above");

    let templates = ctx.db.game_item_template();
    let rows: Vec<CorpseLoot> = ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(&corpse_guid)
        .collect();
    let mut master_rows: Vec<u8> = Vec::new(); // slots stamped master_only, for the MASTER_LIST push
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    for mut row in rows {
        if row.quest_only {
            continue; // orthogonal systems — never group-gate a quest drop (module doc invariant)
        }
        let quality = templates
            .entry()
            .find(row.item_entry)
            .map(|t| t.quality)
            .unwrap_or(0);
        match group_loot_decision_for_row(group.loot_method, quality, group.loot_threshold) {
            GroupLootDecision::Ffa => continue,
            GroupLootDecision::Designate => {
                if let Some(guid) = designee {
                    row.designated_looter_guid = guid;
                    ctx.db.game_corpse_loot().id().update(row);
                }
            }
            GroupLootDecision::Master => {
                row.designated_looter_guid = master_guid;
                row.master_only = true;
                master_rows.push(row.slot);
                ctx.db.game_corpse_loot().id().update(row);
            }
            GroupLootDecision::Roll => {
                start_roll(ctx, corpse_guid, row.slot, row.item_entry, &recipients, now);
                row.withheld = true;
                ctx.db.game_corpse_loot().id().update(row);
            }
        }
    }
    if rr_cursor_dirty {
        ctx.db.game_group().group_id().update(group);
    }
    if !master_rows.is_empty() {
        crate::group::push_event(
            ctx,
            master_guid,
            roll_event_kind::MASTER_LIST,
            0,
            lyracore_shared::loot_roll::encode_master_list(corpse_guid, &recipients),
        );
    }
}

/// Create one Loot Roll and its votes, or return the existing roll for this corpse and slot.
/// Repeated starts preserve votes, recipients and deadline. No event is emitted here.
/// Resolution deletes these rows, so a later replay after resolution is not deduplicated.
fn insert_roll_rows(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    recipients: &[u64],
    deadline_micros: i64,
) -> u64 {
    let rolls = ctx.db.game_loot_roll();
    if let Some(existing) = rolls
        .by_corpse()
        .filter(&corpse_guid)
        .find(|r| r.slot == slot)
    {
        return existing.id;
    }
    let roll = rolls.insert(LootRoll {
        id: 0,
        corpse_guid,
        slot,
        item_entry,
        deadline_micros,
        resolved: false,
    });
    let votes = ctx.db.game_loot_roll_vote();
    for &guid in recipients {
        votes.insert(LootRollVote {
            id: 0,
            roll_id: roll.id,
            voter_guid: guid,
            voted: false,
            vote: 0,
            rolled: 0,
        });
    }
    roll.id
}

/// Spawn a NEED/GREED/NBG roll at KILL TIME: [`insert_roll_rows`] + `ROLL_START` to every recipient.
///
/// This ALWAYS runs on the corpse's own WORLD SHARD — kill-time combat resolution has no
/// path to another database — so in a SHARDED deployment the row it creates is a TRANSIENT staging
/// copy: the gateway's loot-roll relay (`gateway/src/world/loot.rs::relay_tick`) promotes it onto
/// realm-core and clears this copy (`clear_promoted_loot_roll`) so voting/resolution never runs
/// twice. Unsharded, nothing promotes or clears it — it just IS the roll, byte-identical to before
/// this issue.
fn start_roll(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    recipients: &[u64],
    now_micros: i64,
) {
    insert_roll_rows(
        ctx,
        corpse_guid,
        slot,
        item_entry,
        recipients,
        now_micros + ROLL_WINDOW_MICROS,
    );
    let payload = lyracore_shared::loot_roll::encode_start(
        corpse_guid,
        slot,
        item_entry,
        (ROLL_WINDOW_MICROS / 1000) as u32,
    );
    for &guid in recipients {
        crate::group::push_event(ctx, guid, roll_event_kind::ROLL_START, 0, payload.clone());
    }
}

/// The identity-free vote core (mirrors `group.rs`'s `*_on` cores): shared by the player
/// reducer above (the world shard the voter is physically standing on) and `realm_loot_op`'s VOTE arm
/// (realm-core, once the roll has been promoted there). Same gates, same resolution, in the same
/// order — `loot_roll` is byte-identical for clients. Unlike a group op, nothing about a vote's RULES
/// differs by which database it runs on, only which one `game_loot_roll`'s `ctx.db` resolves to — the
/// caller already picked that by calling this on the right connection, so (smalls) there is
/// no `plane` parameter to thread through.
pub(crate) fn cast_vote_on(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    voter_guid: u64,
    vote: u8,
) -> Result<(), String> {
    if !matches!(vote, vote_kind::PASS | vote_kind::NEED | vote_kind::GREED) {
        return Err(refused(LootRefusal::RollUnavailable, "invalid vote"));
    }
    let roll = ctx
        .db
        .game_loot_roll()
        .by_corpse()
        .filter(&corpse_guid)
        .find(|r| r.slot == slot && !r.resolved)
        .ok_or_else(|| refused(LootRefusal::RollUnavailable, "no roll open on that item"))?;
    let votes = ctx.db.game_loot_roll_vote();
    let mut my_vote = votes
        .by_roll()
        .filter(&roll.id)
        .find(|v| v.voter_guid == voter_guid)
        .ok_or_else(|| refused(LootRefusal::RollUnavailable, "voter is not on this roll"))?;
    if my_vote.voted {
        return Err(refused(LootRefusal::RollUnavailable, "already voted"));
    }
    my_vote.voted = true;
    my_vote.vote = vote;
    my_vote.rolled = if vote == vote_kind::PASS {
        0
    } else {
        (ctx.random::<u32>() % 100 + 1) as u8
    };
    let my_rolled = my_vote.rolled; // captured before `my_vote` moves into `update` below
    votes.id().update(my_vote);

    let all: Vec<LootRollVote> = votes.by_roll().filter(&roll.id).collect();
    let recipients: Vec<u64> = all.iter().map(|v| v.voter_guid).collect();
    let payload = lyracore_shared::loot_roll::encode_vote(
        corpse_guid,
        slot,
        roll.item_entry,
        my_rolled,
        vote,
        false,
    );
    for &guid in &recipients {
        crate::group::push_event(
            ctx,
            guid,
            roll_event_kind::ROLL_VOTE,
            voter_guid,
            payload.clone(),
        );
    }
    if all.iter().all(|v| v.voted) {
        resolve_roll(ctx, &roll, &all, &recipients);
    }
    Ok(())
}

/// Grant a resolved roll's item to its winner, on WHICHEVER database holds the corpse.
///
/// Called inline by [`resolve_roll`] / [`force_resolve_rolls_for_disband`], wherever they execute —
/// a single database, or REALM-CORE in a sharded one — and by the [`settle_loot_roll`] operator
/// reducer, which the gateway's loot-roll relay calls on the corpse's OWN world shard after observing
/// the `ROLL_WON` event realm-core just pushed. All three call sites are safe to call
/// unconditionally: there is nothing to settle where no row matches at all (REALM-CORE never has
/// `game_corpse_loot` rows — no kills happen there — so the two inline callers no-op for free there),
/// and nothing to settle a second time (the row is gone after the first successful grant).
///
/// `withheld` is checked FIRST — the roll's own lock on the row, the same gate
/// [`super::group_loot_take_allowed`] uses — so a `(corpse_guid, slot)` pair that merely COINCIDES with
/// an unrelated row on another shard (`corpse_guid` is a per-shard `game_world_entity` sequence, not
/// realm-unique — the tail risk this issue's cross-shard-routing note accepts, same class as
/// character-name collisions elsewhere in this codebase) can never be mistaken for the row that just
/// resolved: a row that is not mid-roll is never touched by this function, however its numbers land.
pub(crate) fn settle_roll_grant(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    winner_guid: u64,
) {
    let loot = ctx.db.game_corpse_loot();
    let Some(mut row) = loot
        .by_corpse()
        .filter(&corpse_guid)
        .find(|l| l.slot == slot)
    else {
        // AC#4 sweep. This return is the one that ATE A WINNER'S ITEM (the review): a slow
        // voter had `CORPSE_DECAY_MICROS == ROLL_WINDOW_MICROS` reap the corpse out from under the
        // roll, settlement found no row, and it returned with no grant, no error and no log.
        //
        // It cannot be an error, because the gateway fans `settle_loot_roll` out to every shard and
        // "no row here" is the normal answer on all but one. So it is logged at INFO and worded so
        // the two cases are TELLABLE APART BY CONTEXT — exactly one shard should have held this
        // corpse, so if NO shard logs a grant for a settled roll, this line is where the item went.
        spacetimedb::log::info!(
            "settle_roll_grant: corpse {corpse_guid} slot {slot} — no loot row on this database \
             (expected on every shard but the corpse's owner; if NO shard granted, the corpse was \
             reaped before the roll settled and the winner got nothing)"
        );
        return;
    };
    if !row.withheld {
        // Not mid-roll: this row belongs to some other corpse/slot pairing on this shard, or the
        // roll already settled here. Distinct from the branch above — the row EXISTS.
        spacetimedb::log::info!(
            "settle_roll_grant: corpse {corpse_guid} slot {slot} — row is not withheld, so no roll \
             is open on it here; nothing granted"
        );
        return; // not a live roll's row on THIS database — see the doc above
    }
    match crate::items::grant_item(ctx, winner_guid, row.item_entry, row.count.max(1)) {
        Ok(()) => {
            loot.id().delete(row.id);
            super::refresh_lootable(ctx, corpse_guid);
        }
        Err(_) => {
            // Inventory-full fallback: leave the row, exclusively reserved for the winner.
            row.withheld = false;
            row.reserved_for = winner_guid;
            row.designated_looter_guid = 0;
            row.master_only = false;
            loot.id().update(row);
        }
    }
}

/// Resolve exactly once (the `resolved` flag guard — work-item 187 trap): any NEED beats any GREED;
/// a TIE within the winning tier re-rolls JUST the tied members internally (bounded — 1-100 rolls
/// collide vanishingly rarely, `MAX_TIE_REROLLS` is generous headroom) and announces once; all-pass
/// unlocks the row FFA-in-group (no `ROLL_WON` — vanilla shows no "won" line either). The winner is
/// granted via `items::grant_item`; on `Err` (inventory full) the row is LEFT, `reserved_for` stamped
/// to the winner (documented winner-locked fallback — 068's mail delivery is the eventual fix).
/// `votes`/`recipients` are the FULL snapshot (already auto-passed by the caller where needed).
fn resolve_roll(ctx: &ReducerContext, roll: &LootRoll, votes: &[LootRollVote], recipients: &[u64]) {
    if roll.resolved {
        return; // exactly-once guard
    }
    ctx.db.game_loot_roll().id().update(LootRoll {
        resolved: true,
        ..roll.clone()
    });

    let kinds: Vec<u8> = votes.iter().map(|v| v.vote).collect();
    let tier = contending_tier(&kinds);
    if tier.is_empty() {
        unlock_row_ffa(ctx, roll.corpse_guid, roll.slot);
        cleanup_roll(ctx, roll.id);
        return;
    }
    let mut rolls: Vec<u8> = tier.iter().map(|&i| votes[i].rolled).collect();
    const MAX_TIE_REROLLS: u32 = 20;
    let mut winner_in_tier = pick_roll_winner(&rolls);
    let mut attempts = 0;
    while winner_in_tier.is_none() && attempts < MAX_TIE_REROLLS {
        for r in rolls.iter_mut() {
            *r = (ctx.random::<u32>() % 100 + 1) as u8;
        }
        winner_in_tier = pick_roll_winner(&rolls);
        attempts += 1;
    }
    let Some(w) = winner_in_tier else {
        // Astronomically unlikely (20 straight all-tied re-rolls); fail safe to the first tied
        // member rather than looping forever or panicking.
        unlock_row_ffa(ctx, roll.corpse_guid, roll.slot);
        cleanup_roll(ctx, roll.id);
        return;
    };
    let winning_roll = rolls[w];
    let winner_guid = votes[tier[w]].voter_guid;
    let winning_vote = votes[tier[w]].vote; // NEED or GREED — the tier that actually won (finding #3)

    // Grant, then finalize the CorpseLoot row per the outcome (module doc: 068 mail is deferred).
    // On a single database this runs inline, right here, exactly as before. On REALM-CORE
    // (sharded) `game_corpse_loot` is always empty — no kills happen there — so this no-ops for free,
    // and the ACTUAL grant happens on the corpse's own world shard via `settle_loot_roll`, which the
    // gateway's loot-roll relay calls after observing the `ROLL_WON` event pushed below.
    settle_roll_grant(ctx, roll.corpse_guid, roll.slot, winner_guid);
    let payload = lyracore_shared::loot_roll::encode_won(
        roll.corpse_guid,
        roll.slot,
        roll.item_entry,
        winning_roll,
        winning_vote,
    );
    for &guid in recipients {
        crate::group::push_event(
            ctx,
            guid,
            roll_event_kind::ROLL_WON,
            winner_guid,
            payload.clone(),
        );
    }
    cleanup_roll(ctx, roll.id);
}

/// All-pass unlock: clear `withheld` so the row becomes an ordinary FFA-in-group row (any current
/// group member may loot it via the plain path — `designated_looter_guid`/`master_only` stay at
/// their defaults, so there is no further restriction).
fn unlock_row_ffa(ctx: &ReducerContext, corpse_guid: u64, slot: u8) {
    let loot = ctx.db.game_corpse_loot();
    if let Some(mut row) = loot
        .by_corpse()
        .filter(&corpse_guid)
        .find(|l| l.slot == slot)
    {
        row.withheld = false;
        loot.id().update(row);
    }
}

/// Delete a resolved roll's vote rows + the roll row itself — nothing left to sweep/orphan.
fn cleanup_roll(ctx: &ReducerContext, roll_id: u64) {
    let votes = ctx.db.game_loot_roll_vote();
    let stale: Vec<u64> = votes.by_roll().filter(&roll_id).map(|v| v.id).collect();
    for id in stale {
        votes.id().delete(id);
    }
    ctx.db.game_loot_roll().id().delete(roll_id);
}

/// Deadline sweep (work-item 187 trap: "disconnected member mid-roll: their vote auto-passes at
/// deadline"): scans every UNRESOLVED roll whose deadline has elapsed, auto-passes any still-`!voted`
/// member (in place — never blocks), then resolves. `pub(crate)` — exposed for the orchestrator's
/// scheduled-GC tick (`gc.rs`, NOT edited here per instructions): wire one call
/// `crate::loot::sweep_loot_rolls(ctx);` into `reap_movement_events` (it already runs every
/// `EVENT_TTL_MICROS` = 1s, ample granularity for a 60s roll window).
pub(crate) fn sweep_loot_rolls(ctx: &ReducerContext) {
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let due: Vec<LootRoll> = ctx
        .db
        .game_loot_roll()
        .iter()
        .filter(|r| !r.resolved && r.deadline_micros <= now)
        .collect();
    for roll in due {
        let votes_tab = ctx.db.game_loot_roll_vote();
        let mut all: Vec<LootRollVote> = votes_tab.by_roll().filter(&roll.id).collect();
        for v in all.iter_mut() {
            if !v.voted {
                v.voted = true;
                v.vote = vote_kind::PASS;
                v.rolled = 0;
                votes_tab.id().update(v.clone());
            }
        }
        let recipients: Vec<u64> = all.iter().map(|v| v.voter_guid).collect();
        resolve_roll(ctx, &roll, &all, &recipients);
    }
}

/// Work-item 187 trap ("disband mid-roll -> resolve to sole member"): called from
/// `group::remove_member`'s full-disband branch with the FULL former membership (`member_guids`,
/// leaver included) and the sole survivor if exactly one remains. Force-resolves every UNRESOLVED
/// roll whose ENTIRE voter set belonged to the disbanding group: with a sole survivor, grants them
/// the item directly (bypassing need/greed entirely — vanilla's behavior); with none (or more than
/// one — shouldn't happen, disband implies <2 remain), just unlocks FFA (nothing left to award to,
/// or the caller mis-invoked with a still-live group).
pub(crate) fn force_resolve_rolls_for_disband(
    ctx: &ReducerContext,
    member_guids: &[u64],
    survivor: Option<u64>,
) {
    let rolls: Vec<LootRoll> = ctx
        .db
        .game_loot_roll()
        .iter()
        .filter(|r| !r.resolved)
        .collect();
    for roll in rolls {
        let votes_tab = ctx.db.game_loot_roll_vote();
        let all: Vec<LootRollVote> = votes_tab.by_roll().filter(&roll.id).collect();
        if all.is_empty() || !all.iter().all(|v| member_guids.contains(&v.voter_guid)) {
            continue; // not this group's roll
        }
        ctx.db.game_loot_roll().id().update(LootRoll {
            resolved: true,
            ..roll.clone()
        });
        match survivor.filter(|s| all.iter().any(|v| v.voter_guid == *s)) {
            Some(winner_guid) => {
                // Inline on a single database (unchanged), a free no-op on REALM-CORE
                // (no local `game_corpse_loot` row there) — the real grant then settles onto the
                // corpse's own world shard via `settle_loot_roll`, driven by the gateway's loot-roll
                // relay off the `ROLL_WON` event pushed below. See `settle_roll_grant`'s own doc.
                settle_roll_grant(ctx, roll.corpse_guid, roll.slot, winner_guid);
                // Forced disband resolution: no contest happened — label it NEED (the sole
                // survivor takes by default; there is no honest greed/need distinction here).
                let payload = lyracore_shared::loot_roll::encode_won(
                    roll.corpse_guid,
                    roll.slot,
                    roll.item_entry,
                    0,
                    lyracore_shared::loot_roll::vote_kind::NEED,
                );
                crate::group::push_event(
                    ctx,
                    winner_guid,
                    roll_event_kind::ROLL_WON,
                    winner_guid,
                    payload,
                );
            }
            None => unlock_row_ffa(ctx, roll.corpse_guid, roll.slot),
        }
        cleanup_roll(ctx, roll.id);
    }
}

// ===========================================================================================
//  REALM-CORE plane — mirrors `group.rs`'s realm-core section for the roll itself.
// ===========================================================================================

// `#[reducer]`: SpacetimeDB reducers take their arguments FLAT off the wire, so a parameter struct is not available.
#[allow(clippy::too_many_arguments)]
/// The realm-wide loot-roll ops, as ONE operator-gated reducer keyed by
/// [`lyracore_shared::loot_roll::loot_op`] — the same one-reducer-not-several trade `realm_group_op`
/// made (`lyracore_shared::group::realm_op`'s doc), for the same reason: every gateway-callable reducer
/// needs a hand-maintained SDK binding (`docs/danger-zones.md` §1.2).
///
/// **Operator-gated, and it has to be**: both ops act on realm-core, which has no live entity to
/// derive an actor from. START's `recipients` are a snapshot the WORLD SHARD already computed
/// spatially at kill time (`kill_reward_recipients`); VOTE's `actor_guid` is the guid the gateway
/// authenticated for the calling socket (`InWorld::self_guid`), never a client-supplied literal.
#[reducer]
pub fn realm_loot_op(
    ctx: &ReducerContext,
    op: u8,
    corpse_guid: u64,
    slot: u8,
    item_entry: u32,
    actor_guid: u64,
    vote: u8,
    deadline_micros: i64,
    recipients: Vec<u64>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    use lyracore_shared::loot_roll::loot_op;
    match op {
        // The gateway's loot-roll relay PROMOTING a roll a world shard already announced locally
        // (`gateway/src/world/loot.rs::relay_tick`) — `insert_roll_rows`, never `start_roll`: the
        // `ROLL_START` popup already fired on the shard that created it, so this must not re-push it.
        loot_op::START => {
            insert_roll_rows(
                ctx,
                corpse_guid,
                slot,
                item_entry,
                &recipients,
                deadline_micros,
            );
            Ok(())
        }
        loot_op::VOTE => cast_vote_on(ctx, corpse_guid, slot, actor_guid, vote),
        other => Err(format!("unknown realm loot op {other}")),
    }
}

/// Grant a resolved roll's item on the WORLD SHARD that actually holds the corpse. The
/// gateway's loot-roll relay calls this on every connected world shard after observing realm-core's
/// `ROLL_WON` event — [`settle_roll_grant`]'s own guards make every wrong-shard call a harmless
/// no-op, so the relay never needs to know in advance WHICH shard holds the corpse.
#[reducer]
pub fn settle_loot_roll(
    ctx: &ReducerContext,
    corpse_guid: u64,
    slot: u8,
    winner_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    settle_roll_grant(ctx, corpse_guid, slot, winner_guid);
    Ok(())
}

/// Delete a STAGING roll's rows on the WORLD SHARD that created them, once the gateway's loot-roll
/// relay has promoted the roll onto realm-core — [`cleanup_roll`], the same delete a
/// resolved roll already used, called here for an UNRESOLVED one that just changed which database is
/// authoritative for it.
#[reducer]
pub fn clear_promoted_loot_roll(ctx: &ReducerContext, roll_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    cleanup_roll(ctx, roll_id);
    Ok(())
}

/// The identity-free master-give core (the `cast_vote_on` shape): everything
/// [`loot_master_give`] does after resolving WHO the master looter is. `gw::gw_loot_master_give` is
/// the other entry.
pub(crate) fn apply_master_give(
    ctx: &ReducerContext,
    master_guid: u64,
    corpse_guid: u64,
    loot_slot: u8,
    target_guid: u64,
) -> Result<(), String> {
    let loot = ctx.db.game_corpse_loot();
    let row = loot
        .by_corpse()
        .filter(&corpse_guid)
        .find(|l| l.slot == loot_slot)
        .ok_or_else(|| refused(LootRefusal::NothingToLoot, "no loot in that slot"))?;
    if !row.master_only || row.designated_looter_guid != master_guid {
        return Err(refused(
            LootRefusal::NotMasterLooter,
            "Actor does not hold the master-looter right on that row",
        ));
    }
    let is_eligible = ctx
        .db
        .game_corpse_loot_eligible()
        .by_corpse()
        .filter(&corpse_guid)
        .any(|e| e.eligible_guid == target_guid);
    if !is_eligible {
        return Err(refused(
            LootRefusal::LootTagIneligible,
            &format!("master-give target {target_guid} is not eligible"),
        ));
    }
    crate::helpers::live_entity(ctx, target_guid).map_err(|_| {
        refused(
            LootRefusal::RecipientUnavailable,
            &format!("master-loot recipient {target_guid} is not in the world"),
        )
    })?;
    crate::items::grant_item(ctx, target_guid, row.item_entry, row.count.max(1))
        .map_err(master_delivery_error)?;
    loot.id().delete(row.id);
    super::refresh_lootable(ctx, corpse_guid);
    Ok(())
}

/// Convert the stable capacity refusal. Missing templates and other grant failures remain errors.
fn master_delivery_error(error: String) -> String {
    if error == lyracore_shared::mail::INVENTORY_FULL {
        refused(LootRefusal::RecipientInventoryFull, &error)
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_delivery_classifies_only_the_stable_capacity_refusal() {
        assert_eq!(
            master_delivery_error(lyracore_shared::mail::INVENTORY_FULL.to_string()),
            LootRefusal::RecipientInventoryFull.as_tag()
        );
        assert_eq!(
            master_delivery_error("no such item 123".to_string()),
            "no such item 123"
        );
    }

    /// `group_loot_decision_for_row` — the threshold-split matrix: FFA never restricts; ROUND_ROBIN
    /// always designates regardless of quality; GROUP splits on the threshold (below → designate,
    /// at/above → roll); NEED_BEFORE_GREED always rolls regardless of quality; MASTER splits on the
    /// threshold (below → designate, at/above → master-only). An unrecognized method degrades to Ffa.
    #[test]
    fn group_loot_decision_matches_the_threshold_split_matrix() {
        use crate::group::loot_method::*;
        use GroupLootDecision::*;

        // FFA: never restricts, at any quality.
        assert_eq!(group_loot_decision_for_row(FFA, 0, 2), Ffa);
        assert_eq!(group_loot_decision_for_row(FFA, 6, 2), Ffa);

        // ROUND_ROBIN: always designate, below AND at/above the threshold.
        assert_eq!(group_loot_decision_for_row(ROUND_ROBIN, 0, 2), Designate);
        assert_eq!(group_loot_decision_for_row(ROUND_ROBIN, 6, 2), Designate);

        // GROUP: the threshold split — below designates, AT (inclusive) and above rolls.
        assert_eq!(
            group_loot_decision_for_row(GROUP, 1, 2),
            Designate,
            "Poor/Normal (1) is below Uncommon (2)"
        );
        assert_eq!(
            group_loot_decision_for_row(GROUP, 2, 2),
            Roll,
            "exactly at the threshold rolls (inclusive)"
        );
        assert_eq!(
            group_loot_decision_for_row(GROUP, 4, 2),
            Roll,
            "well above the threshold rolls"
        );

        // NEED_BEFORE_GREED: always rolls, ignoring the threshold entirely.
        assert_eq!(group_loot_decision_for_row(NEED_BEFORE_GREED, 0, 2), Roll);
        assert_eq!(group_loot_decision_for_row(NEED_BEFORE_GREED, 6, 6), Roll);

        // MASTER: the same threshold split as GROUP, but Master instead of Roll.
        assert_eq!(group_loot_decision_for_row(MASTER, 1, 2), Designate);
        assert_eq!(group_loot_decision_for_row(MASTER, 2, 2), Master);
        assert_eq!(group_loot_decision_for_row(MASTER, 6, 2), Master);

        // An unrecognized method (shouldn't happen post-validation) never invents a restriction.
        assert_eq!(group_loot_decision_for_row(255, 6, 2), Ffa);
    }

    /// `advance_rr_cursor` — round-robin trap: "skipping offline members". Wraps around a STABLE
    /// ordering, picks the next ONLINE member from the current cursor (inclusive), and returns
    /// `None` only when the whole group is offline (never panics/blocks looting).
    #[test]
    fn advance_rr_cursor_skips_offline_members_and_wraps() {
        // All online: cursor 0 picks index 0, advances to 1.
        assert_eq!(advance_rr_cursor(&[true, true, true], 0), Some((0, 1)));
        assert_eq!(advance_rr_cursor(&[true, true, true], 1), Some((1, 2)));
        // Wraps past the end back to 0.
        assert_eq!(advance_rr_cursor(&[true, true, true], 2), Some((2, 0)));
        // Member at the cursor is OFFLINE — skip forward to the next online member.
        assert_eq!(
            advance_rr_cursor(&[true, false, true], 1),
            Some((2, 0)),
            "index 1 is offline, skip to 2"
        );
        // The skip WRAPS if needed (cursor near the end, only an early member online).
        assert_eq!(
            advance_rr_cursor(&[true, false, false], 2),
            Some((0, 1)),
            "wraps around to the only online member"
        );
        // Nobody online at all → None (degrades to FFA at the call site, never panics).
        assert_eq!(advance_rr_cursor(&[false, false], 0), None);
        assert_eq!(
            advance_rr_cursor(&[], 0),
            None,
            "an empty roster never panics"
        );
    }

    /// `contending_tier` — NEED beats GREED unconditionally; an all-pass vote set contends nothing
    /// (the caller unlocks FFA-in-group). Order-preserving indices into the input slice.
    #[test]
    fn contending_tier_need_beats_greed_and_empty_means_all_passed() {
        use vote_kind::*;
        // A mix: NEED wins even though GREED/PASS are also present.
        assert_eq!(contending_tier(&[PASS, GREED, NEED, GREED]), vec![2]);
        // Multiple NEEDs all contend (the tie-break is a separate step, `pick_roll_winner`).
        assert_eq!(contending_tier(&[NEED, PASS, NEED]), vec![0, 2]);
        // No NEED at all → GREED contends.
        assert_eq!(contending_tier(&[PASS, GREED, GREED]), vec![1, 2]);
        // Everyone passed → nobody contends (empty).
        assert!(contending_tier(&[PASS, PASS, PASS]).is_empty());
        assert!(contending_tier(&[]).is_empty());
    }

    /// `pick_roll_winner` — the vote-resolution matrix's final step: the unique max roll wins; a
    /// TIE for the max (two+ share it) returns `None` so the caller re-rolls just the tied members
    /// (cmangos's internal re-roll) rather than picking arbitrarily.
    #[test]
    fn pick_roll_winner_picks_the_unique_max_or_none_on_a_tie() {
        assert_eq!(pick_roll_winner(&[50, 99, 12]), Some(1));
        assert_eq!(pick_roll_winner(&[1]), Some(0));
        assert_eq!(
            pick_roll_winner(&[100, 100]),
            None,
            "a tie for the max must re-roll, never pick arbitrarily"
        );
        assert_eq!(
            pick_roll_winner(&[50, 90, 90, 10]),
            None,
            "a tie among a subset still blocks a decision"
        );
        assert!(pick_roll_winner(&[]).is_none());
    }

    // ---- The realm-core loot-roll plane ----
    //
    // A reducer body needs a live `ReducerContext`, so none of these can be EXECUTED by a test in
    // this crate — exactly why they are scanned, the same technique `group.rs` uses for its own
    // realm-plane reducers (the review).

    use crate::test_scan::code_of;

    /// **The operator gate is the entire authorization of the realm loot-roll plane.**
    ///
    /// All three reducers below take a corpse/roll identity or an actor guid as an ARGUMENT rather
    /// than deriving it from `ctx.sender()` — realm-core has no live entity to derive one from, and
    /// the two world-shard-only ones (`settle_loot_roll`/`clear_promoted_loot_roll`) grant an item or
    /// delete roll rows outright. The gate is the only thing between an arbitrary connection and
    /// forging a roll outcome or wiping another group's live roll.
    #[test]
    fn every_realm_loot_reducer_is_operator_gated() {
        for f in [
            "pub fn realm_loot_op(",
            "pub fn settle_loot_roll(",
            "pub fn clear_promoted_loot_roll(",
        ] {
            let body = code_of(include_str!("rolls.rs"), f);
            let normalized: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.starts_with("{ crate::helpers::require_operator(ctx)?;"),
                "`{f}` no longer OPENS with the operator gate — a gate that is present but not the \
                 FIRST statement (wrapped in `if false`, `let _ =`, or preceded by an early return) \
                 is no gate. Body was:\n{body}"
            );
        }
    }

    /// The op byte is a wire value the gateway sends and `realm_loot_op` dispatches on —
    /// `lyracore_shared::loot_roll::loot_op` pins only the NUMBERS. This pins what each number DOES: a
    /// swapped arm would silently run VOTE for a START, corrupting a fresh roll's recipient snapshot.
    #[test]
    fn realm_loot_op_dispatches_start_and_vote_to_their_own_cores() {
        let body = code_of(include_str!("rolls.rs"), "pub fn realm_loot_op(");
        for (op, core) in [
            ("loot_op::START =>", "{ insert_roll_rows( ctx, corpse_guid, slot, item_entry, &recipients, deadline_micros, ); Ok(()) }"),
            ("loot_op::VOTE =>", "cast_vote_on(ctx, corpse_guid, slot, actor_guid, vote)"),
        ] {
            let arm = body
                .split(op)
                .nth(1)
                .unwrap_or_else(|| panic!("`realm_loot_op` no longer dispatches `{op}`. Body was:\n{body}"));
            let arm: String = arm.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                arm.starts_with(core),
                "`{op}` no longer runs `{core}` — the op byte is a WIRE value the gateway sends and \
                 this match dispatches on; a swapped arm runs the wrong op for every roll, silently. \
                 Arm was:\n{}",
                &arm[..arm.len().min(140)]
            );
        }
    }

    /// [`settle_roll_grant`] must check `withheld` BEFORE granting anything — without it, a
    /// wrong-shard `settle_loot_roll` call (the gateway fans a `ROLL_WON` settle to EVERY connected
    /// shard, since it does not know in advance which one holds the corpse) could steal an ordinary,
    /// un-rolled `game_corpse_loot` row that merely happens to share a `(corpse_guid, slot)` number
    /// with the resolved roll on another shard.
    #[test]
    fn settle_roll_grant_checks_withheld_before_granting() {
        let body = code_of(include_str!("rolls.rs"), "pub(crate) fn settle_roll_grant(");
        let withheld_at = body
            .find("if !row.withheld {")
            .expect("settle_roll_grant no longer gates on `row.withheld` — see this fn's own doc");
        let grant_at = body
            .find("crate::items::grant_item(")
            .expect("settle_roll_grant no longer calls grant_item at all");
        assert!(
            withheld_at < grant_at,
            "the `withheld` guard must run BEFORE `grant_item` — a guard added AFTER the grant \
             already happened is not a guard. Body was:\n{body}"
        );
    }

    /// The acceptance test for this: `group::remove_member`'s disband branch must call
    /// `force_resolve_rolls_for_disband` UNCONDITIONALLY and BEFORE it tears the group row down.
    /// That ordering — not a `Plane` flag, which does not exist on `remove_member` — is what makes a
    /// disbanding party's live rolls resolve on the SAME database, in the SAME transaction, as the
    /// membership change: whichever database `remove_member` executes on (a world shard, unsharded;
    /// REALM-CORE, sharded) is also the database its `game_loot_roll` rows are the truth on. Mutation-
    /// check: deleting the call, or moving it after the group-row delete, must turn this red.
    #[test]
    fn disband_resolves_live_rolls_before_it_tears_the_group_row_down() {
        let body = code_of(include_str!("../group.rs"), "pub(crate) fn remove_member(");
        let call = "crate::loot::force_resolve_rolls_for_disband(ctx, &all_guids, survivor);";
        let call_at = body
            .find(call)
            .unwrap_or_else(|| panic!("`remove_member` no longer force-resolves live rolls on disband (issue #50). Body was:\n{body}"));
        let delete_at = body
            .find("ctx.db.game_group().group_id().delete(group_id);")
            .expect("`remove_member` no longer deletes the group row on disband");
        assert!(
            call_at < delete_at,
            "force_resolve_rolls_for_disband must run BEFORE the group row is torn down (issue #50) \
             — resolving after the delete would still be correct data-wise, but a reviewer relying on \
             \"same transaction, no mirror\" should not have to re-derive that from execution order \
             every time this function changes. Body was:\n{body}"
        );
    }
}
