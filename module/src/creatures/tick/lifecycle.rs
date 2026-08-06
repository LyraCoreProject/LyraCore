//! The creature lifecycle passes (issue #383 split of tick.rs): the canonical despawn checklist
//! (issue #359 — every teardown path in the crate routes through `despawn_creature_entity`), then
//! decay/respawn/GO-respawn — the due-time passes that run on schedule regardless of proximity. See
//! `tick/mod.rs`'s module doc for the pipeline's load-bearing pass ORDER; this split does not change
//! it (every pass here keeps its original name and signature, called by bare name via `tick/mod.rs`'s
//! `use lifecycle::*;`).

use spacetimedb::{ReducerContext, TimeDuration};

use crate::{game_corpse_loot, game_gameobject, game_gameobject_pool_member, game_loot_roll};

use super::*;

// ===========================================================================================
//  The canonical creature-despawn checklist (issue #359)
// ===========================================================================================

/// Delete a live creature entity AND every satellite row keyed on its guid. **This is the one
/// checklist** — every teardown path routes through it: `instance::teardown_instance_inner` (step 1,
/// the instance reap), `encounter::despawn_tracked` (wave adds on an encounter reset/wipe) and
/// [`pass_decay`] (corpse decay). Each caller adds only its OWN extras on top — the persistent
/// `game_creature_spawn` row and the encounter tracking/equip rows for the first two, the respawn-timer
/// re-arm for the third — and none of them re-implements the list below.
///
/// Issue #359: those three had each grown a private copy and DIVERGED. `despawn_tracked` deleted
/// `game_corpse_loot` by corpse guid with no `!withheld` filter — the exact delete the #50 fix removed
/// from the other two — so a mid-roll wipe on a sharded realm silently ate the winner's item (the roll
/// is authoritative on realm-core and invisible here; `settle_loot_roll`'s grant then finds no row and
/// no-ops). It also skipped the rest of the loot family and the taunt lock. Adding a satellite table
/// now means editing this fn, not three-and-hope.
///
/// NOT on the list: `game_creature_move_event`. Two of the three copies scanned that whole table for
/// the mover's pending legs; nothing has written it since perf 2.3 moved legs onto the per-creature
/// `game_creature_spline` row (#395 retired the last writers, and the tripwire below forbids new
/// ones — `gc.rs` stopped reaping it on the same grounds). A full-table scan per despawned creature,
/// to delete from a provably empty table, teaches the next reader that the table is live.
///
/// The order is the safe one: free the engagement first (`disengage` also clears threat BOTH ways via
/// `threat::clear_for_unit`, retargets the attackers and drops their combat), then the per-guid rows,
/// then the entity itself last — its `on_delete` relay is what fires `SMSG_DESTROY_OBJECT`. Idempotent:
/// every step is a keyed delete or an indexed collect-then-delete, so calling it on an already-gone
/// guid is a no-op (`pass_decay` reaching a creature `kill_creature` already disengaged is normal).
/// [entity]
pub(crate) fn despawn_creature_entity(ctx: &ReducerContext, guid: u64) {
    crate::combat::disengage(ctx, guid); // melee rows + threat (clear_for_unit) + stale selections
    crate::threat::clear_taunt_lock(ctx, guid);
    ctx.db.game_creature_spline().guid().delete(guid); // the LIVE leg row (perf 2.3)
    ctx.db.game_entity_motion().guid().delete(guid); // motion row dies with the entity (perf 2.1)
    crate::loot::reap_corpse_loot_family(ctx, guid); // item rows (withheld-safe) + eligibility + rolls + votes
    ctx.db.game_world_entity().guid().delete(guid); // last — the on_delete relay destroys the object
}


/// Pass 3 — decay (runs before respawn): a corpse whose decay window elapsed is DESTROYed and its
/// respawn timer armed to a FUTURE time (so respawn does NOT re-create it the same tick). Reaps the
/// corpse's item-loot rows.
///
/// Work-item 230 classification: STAYS GLOBAL — `despawn_at` is a due-time, not a proximity concern; a
/// corpse must decay on schedule whether or not a player is anywhere near it.
/// Work-item 229: catch-all firing only, still covering ALL instances (see
/// `TickScope::runs_global_passes`). Returns spawn rows scanned.
pub(super) fn pass_decay(ctx: &ReducerContext) -> usize {
    let now_ts = ctx.timestamp;
    let entities = ctx.db.game_world_entity();
    let spawns = ctx.db.game_creature_spawn();

    // Decay pass: a corpse whose decay window elapsed is DESTROYed (the on_delete relay
    // fires SMSG_DESTROY_OBJECT, so it vanishes) and its respawn timer armed; the respawn pass below
    // then re-creates it. Gated on the live entity being `dead` so a freshly-spawned creature (whose
    // stale despawn_at is in the past) is never decayed.
    let mut visited = 0usize;
    // RANGE SCAN, not a table scan: `by_despawn_at` walks the index from its low end and stops at
    // `now`, so it visits only corpses whose decay window has actually elapsed — normally none.
    // This works only because `despawn_at` is parked in the far future while a creature is NOT a
    // corpse (`creatures::timer_never`); when every row carried a past stamp, `<= now` matched the
    // entire table and the index would have narrowed nothing.
    let decaying: Vec<u64> = spawns
        .by_despawn_at()
        .filter(..=now_ts)
        .inspect(|_| visited += 1)
        .filter(|s| {
            entities
                .guid()
                .find(s.guid)
                .map(|e| e.dead)
                .unwrap_or(false)
        })
        .map(|s| s.guid)
        .collect();
    // Work-item 187 (review finding #2): the roll deadline and the corpse decay are stamped from
    // the SAME kill timestamp with the SAME 60s constant, and this pass ticks more often than the
    // gc sweep — without resolving due rolls FIRST, decay wins the race and silently discards an
    // unresolved roll (no winner, no ROLL_WON, item gone). Sweeping here guarantees every roll
    // whose deadline has arrived resolves (absent voters auto-pass) before its rows can be reaped.
    if !decaying.is_empty() {
        crate::loot::sweep_loot_rolls(ctx);
    }
    for guid in decaying {
        // Belt over the sweep's braces: if a roll on this corpse is somehow STILL unresolved (a
        // future constant change could put deadlines past decay), keep the corpse one more tick
        // rather than discard a live roll — the sweep resolves it next pass.
        //
        // Issue #50 fix: `sweep_loot_rolls` and the `game_loot_roll` check above only see rolls
        // whose STATE lives on THIS database. In a sharded deployment a roll promoted to realm-core
        // clears its local staging row within ~200ms of kill-time, so neither of the above can see
        // it anymore even though it is still open there — decay would otherwise reap this corpse's
        // `game_corpse_loot` row (below) before `settle_loot_roll` ever runs, silently losing the
        // winner's item (the relay's grant no-ops when it finds no row left to grant against). The
        // corpse's OWN `withheld` bit is a plane-agnostic signal for "a roll still owns this row" —
        // it is set at roll start and cleared only by a grant or an all-pass unlock, on whichever
        // database resolves the roll — so gate on it too, unconditionally, local table or not.
        if ctx
            .db
            .game_loot_roll()
            .by_corpse()
            .filter(&guid)
            .any(|r| !r.resolved)
            || ctx
                .db
                .game_corpse_loot()
                .by_corpse()
                .filter(&guid)
                .any(|l| l.withheld)
        {
            continue;
        }
        // The ONE checklist (issue #359): entity + spline/motion + the whole loot family (slice 4's
        // item rows and work-item 187's eligibility snapshot + resolved rolls + votes, so a decayed
        // corpse never orphans them — an UNRESOLVED roll blocked the decay above). This pass's own
        // extra is the respawn-timer re-arm below.
        crate::creatures::despawn_creature_entity(ctx, guid);
        if let Some(mut spawn) = spawns.guid().find(guid) {
            // Honor the per-spawn cmangos respawn timer (respawn_secs, seconds from DEATH) over the
            // flat RESPAWN_MICROS default. `respawn_secs == 0` means "not imported" and falls back to
            // the flat timer (byte-identical for every un-imported spawn). The creature's death time is
            // recovered as `despawn_at - CORPSE_DECAY_MICROS` (kill_creature arms
            // `despawn_at = death + CORPSE_DECAY_MICROS`), so a real `respawn_secs` is measured from
            // DEATH, matching cmangos, not from decay.
            spawn.respawn_at = if spawn.respawn_secs == 0 {
                now_ts
                    .checked_add(TimeDuration::from_micros(crate::combat::RESPAWN_MICROS))
                    .unwrap_or(now_ts)
            } else {
                let death_ts = spawn
                    .despawn_at
                    .checked_sub(TimeDuration::from_micros(
                        crate::combat::CORPSE_DECAY_MICROS,
                    ))
                    .unwrap_or(spawn.despawn_at);
                let armed = death_ts
                    .checked_add(TimeDuration::from_micros(
                        spawn.respawn_secs as i64 * 1_000_000,
                    ))
                    .unwrap_or(now_ts);
                // Never arm a respawn in the past (a respawn_secs shorter than the corpse-decay window
                // would otherwise instant-respawn the same tick the corpse decays) — clamp to now.
                if armed > now_ts {
                    armed
                } else {
                    now_ts
                }
            };
            // DISARM the corpse timer — read AFTER `death_ts` above, which needs the armed value.
            // The corpse is gone; leaving `despawn_at` in the past would keep this row in every
            // future decay range scan forever, which is exactly the cost this pass just stopped
            // paying.
            spawn.despawn_at = crate::creatures::timer_never(ctx);
            spawns.guid().update(spawn);
        }
    }
    visited
}


/// Pass 2 — respawn: re-create any creature whose live entity is gone and whose `respawn_at`
/// elapsed, from its persistent spawn record + template. Runs AFTER decay (decay arms a future
/// `respawn_at`, so a just-decayed creature isn't re-spawned this tick).
///
/// Work-item 230 classification: STAYS GLOBAL — `respawn_at` is a due-time; a spawn point must
/// repopulate on schedule whether or not a player is nearby to see it happen.
/// Work-item 229: catch-all firing only, still covering ALL instances. Returns spawn rows scanned.
pub(super) fn pass_respawn(ctx: &ReducerContext) -> usize {
    let now_ts = ctx.timestamp;
    let entities = ctx.db.game_world_entity();
    let spawns = ctx.db.game_creature_spawn();
    let templates = ctx.db.game_creature_template();

    let mut visited = 0usize;
    // RANGE SCAN (see `pass_decay`): only spawns whose timer has actually fired. A live creature's
    // `respawn_at` sits in the far future (`creatures::timer_never`), so it is not visited at all.
    let due: Vec<CreatureSpawn> = spawns
        .by_respawn_at()
        .filter(..=now_ts)
        .inspect(|_| visited += 1)
        .filter(|s| entities.guid().find(s.guid).is_none())
        .collect();
    for spawn in due {
        let guid = spawn.guid;
        if let Some(tmpl) = templates.entry().find(spawn.entry) {
            super::spawn::insert_creature_entity(
                ctx,
                build_creature_entity(&spawn, &tmpl, ctx.random(), 0),
            );
        }
        // DISARM: the timer has fired and the creature is alive again. This is what keeps the index
        // useful — a fired-but-still-past timer would put the row back in every subsequent scan, and
        // it is also how a freshly IMPORTED spawn (which arms `respawn_at = now` deliberately, so it
        // materialises on the first tick) leaves the scan after that first tick.
        if let Some(mut s) = spawns.guid().find(guid) {
            s.respawn_at = crate::creatures::timer_never(ctx);
            spawns.guid().update(s);
        }
    }
    visited
}

/// GATHER-node respawn (the POOL model): for every DEPLETED node (state==1)
/// whose armed respawn time has elapsed, EITHER re-roll its pool (a POOLED point — the node ROAMS to a
/// different inactive point, holding MAX_ACTIVE) OR flip it state 1→0 in place (a STANDALONE node, the
/// pool_id==0 path). Runs in the SENSE block (~4s) —
/// respawns are minutes-scale, so the slow cadence is ample and a 500ms scan would be wasteful, never
/// late by more than a sense tick. `respawn_at_micros` is a defaulted u64 (NOT a Timestamp — that table
/// is live/populated, so a Timestamp column would abort the publish; teleport-core lesson). The due
/// decision is the unit-tested `gameobject::respawn_due` predicate. Filters to depleted-only inline;
/// collect-then-mutate so we never write `game_gameobject` while iterating it.
/// Perf catalog 1.21 (was: "deliberate simplification: full-table `iter()` every sense tick,
/// defer it"): the scan is now a `by_respawn_at` range probe over armed-and-elapsed rows only. A
/// `by_state` index would still have visited every depleted node every tick; the due-time range
/// visits only the ones actually due.
///
/// Work-item 230 classification: STAYS GLOBAL — `respawn_at_micros` is a due-time (like creature
/// respawn/decay), not proximity; GAMEOBJECTS are also out of this item's creature-ticking scope (see
/// the "defer it" note above, pre-dating 230).
/// Work-item 229: catch-all firing only, still covering ALL instances. Returns GO rows scanned.
pub(super) fn pass_gameobject_respawn(ctx: &ReducerContext) -> usize {
    let now = ctx.timestamp.to_micros_since_unix_epoch() as u64;
    let gos = ctx.db.game_gameobject();
    let members = ctx.db.game_gameobject_pool_member();
    // Collect the due guids FIRST (collect-then-mutate) so a reroll's delete/insert never writes
    // game_gameobject while we iterate it, and two due rerolls can't race the same pool slot.
    // Perf catalog 1.21: `by_respawn_at` range-scans ARMED-and-elapsed rows only (`0` = not armed, so
    // the `1..=now` range skips every ready node and every non-gather GO for free). `respawn_due` stays
    // as the post-filter, so the predicate — and the result set — are unchanged. `visited` now counts
    // the index candidates rather than the whole table; that IS the 230-style evidence number.
    let mut visited = 0usize;
    let due: Vec<u64> = gos
        .by_respawn_at()
        .filter(1..=now)
        .inspect(|_| visited += 1)
        .filter(|g| crate::gameobject::respawn_due(g.state, g.respawn_at_micros, now))
        .map(|g| g.guid)
        .collect();
    for guid in due {
        // POOLED point? A guid TAGGED as a pool point (structural test, collision-proof — NOT a bare
        // low-48 lookup, which collided with standalone GO guids) → re-roll the pool (deactivate this
        // point, activate a different inactive one) so the active set ROTATES across the zone. A
        // STANDALONE node (tag bit clear → never queries the member table) falls through to the EXISTING
        // in-place state 1→0 flip — byte-identical, no regression.
        match crate::gameobject::pool_point_id_of(guid).and_then(|pid| members.point_id().find(pid))
        {
            Some(member) => crate::gameobject::reroll_pool(ctx, member.pool_id, guid),
            None => {
                if let Some(mut go) = gos.guid().find(guid) {
                    go.state = 0;
                    go.respawn_at_micros = 0;
                    gos.guid().update(go);
                }
            }
        }
    }
    visited
}

#[cfg(test)]
mod due_timer_tripwire {
    use crate::test_scan::code_of;

    /// The two due-time passes must RANGE-SCAN their index, and must DISARM the timer they fired.
    ///
    /// Both halves are load-bearing and neither is visible to a behavioural test (these are
    /// `ReducerContext` glue — playbook §7), so their shape is pinned:
    ///
    /// * Drop the range scan and they silently go back to visiting the whole spawn table — the exact
    ///   cost this work removed (measured: 5,735 rows per pass per sense tick, with nobody online).
    /// * Drop the DISARM and the range scan stops narrowing anything, because a fired timer stays in
    ///   the past and the row is revisited forever. That failure is invisible: behaviour stays
    ///   correct and only the cost comes back.
    ///
    /// Issue #383: moved from `tick.rs`'s combined `due_timer_tripwire` mod to `lifecycle.rs` with
    /// the passes it pins (`pass_decay`/`pass_respawn` now live here, not in `tick/mod.rs`).
    #[test]
    fn the_due_time_passes_range_scan_their_index_and_disarm_after_firing() {
        let src = include_str!("lifecycle.rs");

        let decay = code_of(src, "fn pass_decay(ctx: &ReducerContext) -> usize {");
        assert!(
            decay.contains("by_despawn_at()") && decay.contains("filter(..=now_ts)"),
            "`pass_decay` no longer range-scans `by_despawn_at` — it is back to a full spawn-table \
             scan every sense tick. Body was:\n{decay}"
        );
        assert!(
            decay.contains("despawn_at = crate::creatures::timer_never(ctx)"),
            "`pass_decay` no longer disarms `despawn_at` after the corpse decays, so the row stays \
             in the range scan forever and the index narrows nothing. Body was:\n{decay}"
        );

        let respawn = code_of(src, "fn pass_respawn(ctx: &ReducerContext) -> usize {");
        assert!(
            respawn.contains("by_respawn_at()") && respawn.contains("filter(..=now_ts)"),
            "`pass_respawn` no longer range-scans `by_respawn_at`. Body was:\n{respawn}"
        );
        assert!(
            respawn.contains("respawn_at = crate::creatures::timer_never(ctx)"),
            "`pass_respawn` no longer disarms `respawn_at` after materialising the creature. Every \
             imported spawn arms `respawn_at = now` deliberately, so without the disarm the ENTIRE \
             imported world sits in the range scan permanently. Body was:\n{respawn}"
        );
    }
}

#[cfg(test)]
mod despawn_checklist_tripwire {
    use crate::test_scan::code_of;

    /// **One despawn checklist** (issue #359). `despawn_creature_entity` is the only place that knows
    /// what a dying creature leaves behind; pinning its steps is how a satellite table stops being
    /// dropped from the list by a refactor that "only touched one caller".
    ///
    /// Every step below is invisible to a behavioural test (this crate has no `ReducerContext`
    /// harness — playbook §7) and every one of them fails SILENTLY: a leaked spline/motion row hands
    /// a fresh subscriber a stale leg for a creature that no longer exists, a leaked taunt lock
    /// outlives the creature it locked, and a skipped loot family orphans rows forever (a guid is
    /// unique per spawn, so nothing ever collects them).
    #[test]
    fn the_canonical_despawn_checklist_keeps_every_step() {
        let body = code_of(
            include_str!("lifecycle.rs"),
            "pub(crate) fn despawn_creature_entity(ctx: &ReducerContext, guid: u64) {",
        );
        for (needle, why) in [
            (
                "crate::combat::disengage(ctx, guid)",
                "the melee engagement + threat (via `threat::clear_for_unit`) + the attackers' \
                 stale selections",
            ),
            (
                "crate::threat::clear_taunt_lock(ctx, guid)",
                "the taunt lock (`despawn_tracked` used to skip it entirely)",
            ),
            (
                "ctx.db.game_creature_spline().guid().delete(guid)",
                "the LIVE movement leg row (perf 2.3) — the one a client actually animates from",
            ),
            (
                "ctx.db.game_entity_motion().guid().delete(guid)",
                "the per-mover motion row (perf 2.1)",
            ),
            (
                "crate::loot::reap_corpse_loot_family(ctx, guid)",
                "the whole loot family — item rows, eligibility snapshot, rolls, votes",
            ),
            (
                "ctx.db.game_world_entity().guid().delete(guid)",
                "the entity itself, whose `on_delete` relay is what fires SMSG_DESTROY_OBJECT",
            ),
        ] {
            assert!(
                body.contains(needle),
                "`despawn_creature_entity` no longer does `{needle}` — it stopped cleaning up {why}. \
                 Every teardown path routes through this fn (issue #359), so a step dropped here is \
                 dropped for the instance reap, the encounter wipe AND corpse decay at once. Body \
                 was:\n{body}"
            );
        }
    }

    /// **The three teardown paths route through the checklist and re-implement none of it.**
    ///
    /// This is the actual issue-#359 regression: `encounter::despawn_tracked` carried a private copy
    /// that deleted `game_corpse_loot` by corpse guid with NO `!withheld` filter, so a wipe mid
    /// NEED/GREED roll deleted the item out from under a roll that — on a sharded realm — is
    /// authoritative on realm-core and invisible locally, and `settle_loot_roll`'s grant then found
    /// nothing and silently no-opped. A raw `game_corpse_loot()` handle in any of these bodies is
    /// that bug growing back.
    #[test]
    fn every_creature_teardown_path_routes_through_the_checklist() {
        let tracked = code_of(
            include_str!("../../encounter.rs"),
            "fn despawn_tracked(ctx: &ReducerContext, tracked: &[EncounterSpawn]) {",
        );
        assert!(
            tracked.contains("crate::creatures::despawn_creature_entity(ctx, t.guid)"),
            "`encounter::despawn_tracked` no longer routes through the canonical checklist. Body \
             was:\n{tracked}"
        );
        assert!(
            !tracked.contains("game_corpse_loot()"),
            "`encounter::despawn_tracked` touches `game_corpse_loot` directly again — that is the \
             issue-#359 bug: an unfiltered delete there eats a WITHHELD row still owned by a live \
             cross-shard loot roll (the #50 invariant). Route it through \
             `loot::reap_corpse_loot_family`. Body was:\n{tracked}"
        );

        let teardown = code_of(
            include_str!("../../instance.rs"),
            "pub(crate) fn teardown_instance_inner(ctx: &ReducerContext, instance_id: u64, \
             delete_row: bool) {",
        );
        assert!(
            teardown.contains("crate::creatures::despawn_creature_entity(ctx, *guid)"),
            "`teardown_instance_inner`'s population sweep no longer routes through the canonical \
             checklist. Body was:\n{teardown}"
        );
        assert!(
            !teardown.contains("game_corpse_loot()") && !teardown.contains("game_threat()"),
            "`teardown_instance_inner` hand-rolls loot-family or threat cleanup again — `disengage` \
             (inside the checklist) already clears threat both ways, and the loot family belongs to \
             `loot::reap_corpse_loot_family`. Body was:\n{teardown}"
        );

        let decay = code_of(
            include_str!("lifecycle.rs"),
            "fn pass_decay(ctx: &ReducerContext) -> usize {",
        );
        assert!(
            decay.contains("crate::creatures::despawn_creature_entity(ctx, guid)"),
            "`pass_decay` no longer routes corpse decay through the canonical checklist. Body \
             was:\n{decay}"
        );
        assert!(
            decay.contains(".any(|l| l.withheld)"),
            "`pass_decay` lost its #50 withheld gate — the plane-agnostic \"a roll still owns this \
             corpse\" check that keeps a corpse one more tick instead of decaying a live cross-shard \
             roll. Body was:\n{decay}"
        );
    }

    /// The #50 invariant lives in ONE function now (`loot::reap_corpse_loot_family`, moved there from
    /// `instance.rs` by issue #359 to sit next to the tables it cleans). Losing the `!withheld`
    /// filter silently breaks the cross-shard NEED/GREED grant on every teardown path at once.
    #[test]
    fn the_canonical_loot_family_reap_never_deletes_a_withheld_row() {
        let body = code_of(
            include_str!("../../loot/mod.rs"),
            "pub(crate) fn reap_corpse_loot_family(ctx: &ReducerContext, corpse_guid: u64) {",
        );
        assert!(
            body.contains(".filter(|l| !l.withheld)"),
            "`reap_corpse_loot_family` no longer skips WITHHELD rows (issue #50): a row locked by a \
             loot roll that is authoritative on realm-core is invisible to this database, so \
             deleting it makes `settle_loot_roll`'s grant find nothing and silently no-op. Body \
             was:\n{body}"
        );
        for table in [
            "game_corpse_loot_eligible()",
            "game_loot_roll()",
            "game_loot_roll_vote()",
        ] {
            assert!(
                body.contains(table),
                "`reap_corpse_loot_family` no longer reaps `{table}` — the loot family is what every \
                 despawn path delegates here, so a missed table orphans rows on a guid nothing will \
                 ever revisit. Body was:\n{body}"
            );
        }
    }
}
