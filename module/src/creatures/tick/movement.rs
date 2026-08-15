//! The ENGAGED movement legs still living here: the low-HP rout and fear-flee. The behavior cycle
//! ([`crate::creatures::cycle`]) owns when each runs; its own ticket migrates each body out of here.
//! The IDLE movers — patrol, return-to-post and wander — and CHASE already live in the cycle.
//!
//! Shares the movement-leg grammar (`PendingLeg` / `drain_legs` / `movable_creature`), the one
//! spline writer (`emit_creature_leg`), and `creature_is_routing` with `tick/mod.rs` and `sense.rs`
//! — all defined in `tick/mod.rs` and reachable here as private ancestor items (Rust visibility:
//! private == visible in the defining module + every descendant), no re-export needed.

use lyracore_shared::constants;
use spacetimedb::{ReducerContext, Table};

use crate::{game_melee_attack, game_world_entity};

use super::*;

/// Pass 11 — flee: a meleeing HUMANOID below ~15% HP routs — it runs away from its target while STAYING
/// in combat (a shared combat state; both sides keep their engagement). Runs LAST (after both regen
/// passes): the still-engaged runner is skipped by regen's in-combat gate, so regen never re-writes (and
/// reverts) the fled position this tick.
///
/// The rout is BOUNDED and once per engagement: this pass stamps `MeleeAttack::rout_ends_ms` when a rout
/// starts and runs the leg only while that window is open. When it closes the creature is an ordinary
/// engaged attacker again — chase closes the gap, the swing pass resolves its blows — and it fights to
/// the death at whatever health it has, with no evade and no heal.
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — every candidate must currently be
/// the ATTACKER in a `game_melee_attack` row (combat-engaged; fleeing is a SHARED COMBAT STATE per the
/// doc above, not a disengage). Gating this on active cells would risk freezing a routing creature
/// mid-flight the instant it — or the player chasing it — stepped past the active-cell edge, which is
/// exactly the "mid-leg flee... finishes" outcome the item requires, not "freezes."
///
/// Work-item 233: outer-loops `game_melee_attack` directly (same precedent as the chase and cast
/// phases) instead of `entities.iter()` + a per-row `.find(&c.guid)` gate. VERIFIED before
/// switching: "fleeing" carries NO separate state on `WorldEntity` (no `is_fleeing`/`fleeing_since`
/// field — `WorldEntity`'s only combat-adjacent fields are `dead`/`target_guid`/`unit_flags`); a
/// creature's rout status comes from exactly two things — "is it the attacker in a melee row" and that
/// row's eligibility + rout window. Since the melee row is that same necessary condition the old per-row
/// `.find` already required, a creature whose melee row was ever deleted mid-rout could NEVER have
/// re-qualified anyway (this pass never deletes it — see the ordering note above), so outer-looping the
/// melee table instead of the entity table cannot miss a genuinely routing creature. It is also where
/// the rout clock lives, so the state is in hand with no extra lookup.
/// Work-item 229: gated per candidate on `scope.covers(c.instance_id)` (attacker's instance = the
/// pair's — same construction argument as the cast and chase phases).
///
/// Issue #383: dropped the `_tick_secs` parameter, same reasoning and same committed-leg architecture
/// as chase — the flee leg is one full `FLEE_LEG_YD` dash, re-rolled only when the previous
/// committed leg finishes (the `game_creature_spline` liveness check below), not sized per firing.
/// Returns covered candidates visited.
pub(crate) fn pass_flee(ctx: &ReducerContext, scope: &TickScope) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    // Flee pass (vanilla creature AI: a mob in melee that drops to ~15% HP routs for a bounded window).
    // For each ALIVE creature currently attacking (a `game_melee_attack` row keyed by its guid) that is
    // eligible and inside an open rout window, run one committed leg directly away from its target while
    // KEEPING the engagement live (it doesn't disengage — routing is a combat state). Moving x,y relays a
    // position change to clients, same as a patrol leg.
    //
    // ORDERING — this MUST run LAST, after both regen passes. A routing creature is still engaged, so the
    // regen passes' `in_combat` gate skips it — meaning regen never re-writes (and clobbers) the fled
    // position this tick. Were a rout ever to drop the engagement before regen ran, the health-regen pass
    // would re-write the whole row from its own snapshot and REVERT the move, pinning the runner in place
    // (it never leaves aggro range, re-engages, and grinds the target down). Snapshot the work first, then
    // mutate, so we never write the entity / melee tables while iterating them.
    let melee_flee = ctx.db.game_melee_attack();
    // Snapshot-then-mutate (never write tables mid-iteration). EVERY eligible fleeing creature re-stamps
    // combat each tick (kept alive across the long committed leg); only those whose committed leg has
    // FINISHED get a fresh leg emitted.
    let mut to_restamp: Vec<(u64, u64)> = Vec::new(); // (creature_guid, target_guid)
    let mut to_stamp: Vec<(u64, u32)> = Vec::new(); // (attacker_guid, rout window close ms)
    let mut to_flee: Vec<PendingLeg> = Vec::new();
    // Work-item 233: outer-loop the small melee-engaged table instead of every entity — `row.attacker_guid`
    // is by construction "currently a melee attacker", the exact gate the old `.find(&c.guid)` applied
    // after scanning every entity, so this visits the identical candidate set.
    for row in melee_flee.iter() {
        // Issue #383: the shared gate ladder (creature, alive, this firing's covered instance).
        let Some(c) = movable_creature(ctx, row.attacker_guid, scope) else {
            continue;
        };
        visited += 1;
        // Crowd control: a creature whose self-movement is suppressed — FROZEN (stun/root/poly) or FEARED
        // — does NOT run this low-HP flee leg. A frozen one stays engaged (we don't disengage it here) and
        // locked in place; a rooted low-HP creature keeps swinging but can't run. A FEARED low-HP creature
        // is moved by the fear-flee pass instead (which keeps it engaged, not disengaged), so excluding it
        // here avoids a double-move. Baseline-safe: `false` without a CC aura → an un-CC'd near-dead
        // creature flees exactly as before.
        if crate::spell::is_self_movement_suppressed(ctx, c.guid) {
            continue;
        }
        // Below the flee threshold AND flee-eligible. Fleeing is SELECTIVE in vanilla, not universal: only
        // HUMANOIDS rout at low HP; BEASTS (wolves/boars) etc. fight to the death. A non-eligible near-dead
        // creature is never moved here and instead keeps chasing/swinging via the earlier passes (a missing
        // template ⇒ not eligible ⇒ stands and fights — safe default).
        let eligible = rout_eligible(ctx, &c);
        // START the rout, or ride the one already running. This pass is the ONLY writer of the window, so
        // "eligible and unstamped" is the single moment a rout begins; once the window closes the creature
        // is a normal engaged attacker again (chase closes the gap, the swing pass resolves its blows) and
        // no later health drop can start a second one.
        let starting = may_start_rout(eligible, row.rout_ends_ms);
        if !starting && !(eligible && rout_window_open(now_ms, row.rout_ends_ms)) {
            continue; // not eligible, or the window is spent → it stands and fights
        }
        // Run directly away from whoever it's fighting (the melee row's target). Resolved BEFORE the
        // window is stamped, so a vanished target can never spend the rout without a leg being run.
        let Some(target) = entities.guid().find(row.target_guid) else {
            continue;
        };
        if starting {
            to_stamp.push((row.attacker_guid, rout_close_ms(now_ms)));
        }
        // SHARED COMBAT STATE: re-stamp EVERY tick (even mid-leg) so combat doesn't drop during the long
        // committed run. (Was in the emit loop, which now fires only on re-roll.)
        to_restamp.push((c.guid, row.target_guid));
        // COMMIT one long leg: re-roll ONLY when the previous leg has finished (no active spline).
        // Re-picking an away-direction every tick snap-rotated the client's facing (the "flee spin");
        // committing to one ~28yd leg (the vanilla flee-leg band) gives one stable travel tangent.
        if ctx.db.game_creature_spline().guid().find(c.guid).is_some() {
            continue;
        }
        // A single FLEE_LEG_YD leg directly away from the threat, nav-routed, at the wounded-slow run
        // speed; duration = the FULL travel time for that leg (not one
        // tick), so the client interpolates the whole committed run.
        let run = crate::combat::effective_move_speed(ctx, c.guid, constants::speeds::RUN)
            * wounded_slow_factor(c.health, c.max_health);
        let (fx, fy) = flee_step(c.x, c.y, target.x, target.y, FLEE_LEG_YD);
        let (nx, ny) =
            crate::nav::nav_step(ctx, c.map_id, (c.x, c.y), (fx, fy), FLEE_LEG_YD, 0.0, c.z);
        let Some((dx2, dy2, duration_ms)) = leg_toward((c.x, c.y), (nx, ny), run) else {
            continue;
        };
        to_flee.push(PendingLeg {
            guid: c.guid,
            dest: (dx2, dy2),
            z_fallback: c.z,
            duration_ms,
        });
    }
    // Combat re-stamp EVERY tick for every routing creature (not just re-roll ticks) — routing is a
    // SHARED COMBAT STATE; without this the 6s combat-drop fires mid committed-leg and both sides untarget
    // ("they get away or untarget each other and combat just ends"). The rout ends when its window closes,
    // or sooner if the player catches the wounded-slow mob and kills it.
    for (guid, target_guid) in to_restamp {
        crate::combat::enter_combat(ctx, guid);
        crate::combat::enter_combat(ctx, target_guid);
    }
    // Stamp the newly started rout windows (re-find the LIVE row — the table was only read above).
    for (attacker_guid, ends_ms) in to_stamp {
        if let Some(mut row) = melee_flee.attacker_guid().find(attacker_guid) {
            row.rout_ends_ms = ends_ms;
            melee_flee.attacker_guid().update(row);
        }
    }
    // Emit the committed RUN legs (only the finished-leg creatures reach here). The client interpolates
    // the whole ~28yd run; pass_advance_splines advances the authoritative position along it.
    drain_legs(ctx, to_flee, true, false, now_ms);
    visited
}

/// Pass 12 — fear-flee (crowd control): a FEARED creature is force-walked AWAY from its fear source
/// each tick with jitter. Runs LAST alongside flee for the same regen-ordering reason; unlike flee it
/// does NOT disengage (fear is temporary). The normal movement passes all skip a feared creature
/// (`is_self_movement_suppressed`), so this is its SOLE mover.
///
/// Work-item 230 classification: ALWAYS ACTIVE, no active-cell gate — a feared creature carries a live
/// CC aura (someone cast fear on it, which requires combat/targeting contact), so it's combat-adjacent
/// by construction and the population is inherently small and rare regardless of world size. Gating it
/// would risk freezing a panicked creature mid-route the instant it crossed the active-cell edge.
///
/// Work-item 233: outer-loops the `game_aura` table (filtered to `A_CONTROL(M_FEAR)` rows) instead of
/// `entities.iter()` + a per-entity `fear_source` probe. `game_aura` has no by-KIND index (only
/// `by_target`/`by_expiry`), so a target-side lookup can't narrow this the way the melee table narrows
/// cast/flee — but the aura table is itself small relative to the full entity population (fear uptime
/// is rare), so scanning IT with an `eff_kind`/`eff_p0` filter and DEDUPING the target guids is the
/// cheap, correct narrowing. `fear_source` is still called per candidate, UNCHANGED, to resolve the
/// caster — this only changes how the candidate SET is discovered (aura-table scan + dedup, vs.
/// entity-table scan + per-row probe), not which caster wins if a target somehow carries more than one
/// live fear aura (that tie-break is `fear_source`'s own `.find()` order, untouched by this pass).
/// Work-item 229: gated per candidate on `scope.covers(c.instance_id)` (the FEARED creature's own
/// instance — its sole mover must fire exactly once per its covering row). The dash distance scales
/// with `tick_secs` (`fear_step_for_tick` — verbatim `FEAR_STEP` on the default row) so a fast
/// dedicated row doesn't multiply terror speed. Returns covered candidates visited.
pub(crate) fn pass_fear_flee(ctx: &ReducerContext, scope: &TickScope, tick_secs: f32) -> usize {
    let mut visited = 0usize;
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u32;
    let entities = ctx.db.game_world_entity();
    // Fear-flee pass (crowd control: a FEARED creature "flees in terror" — it cannot act, and instead of
    // steering its own movement it is force-walked AWAY from whoever feared it each tick). For each ALIVE
    // creature carrying an `A_CONTROL(M_FEAR)` aura (detected via `fear_source`, which also yields the
    // caster to flee FROM) that is NOT otherwise FROZEN (`is_movement_blocked`: a stun/root/poly aura
    // outranks fear and pins it in place), step it `FEAR_STEP` yards away from the fear source with
    // angular jitter (zig-zag) and emit a RUN leg. Unlike the low-HP flee it does NOT disengage — fear is
    // temporary; when the aura expires the creature resumes attacking (its melee row is untouched), and
    // the action gates keep it from swinging/casting meanwhile.
    //
    // SOURCE position: the fear caster's (if still in world); failing that the creature's current melee
    // target (it still flees *something*); failing both its OWN position, which `feared_flee_step` turns
    // into a jittered +x bearing — so a feared creature always moves.
    //
    // ORDERING — like the low-HP flee it runs LAST, after both regen passes. A feared+engaged creature is
    // still `in_combat` (melee row live) so regen skips it; a feared+non-engaged one is moved here AFTER
    // regen ran, so regen never reverts the leg (this pass writes only x/y/z/grid/last_move, never
    // HP/power). The normal movement passes ALL skip feared creatures (`is_self_movement_suppressed`), so
    // this is the SOLE mover for a feared creature — one leg, one `spline_id`, per tick. Snapshot first
    // (collect-then-mutate), like every other movement pass.
    let melee_fear = ctx.db.game_melee_attack();
    let auras_fear = ctx.db.game_aura();
    let mut to_fear_flee: Vec<PendingLeg> = Vec::new();
    // Work-item 233: the small candidate SET — every distinct `target_guid` carrying a live
    // `A_CONTROL(M_FEAR)` aura — discovered by scanning the (small) aura table instead of every entity.
    // Sorted + deduped so a target with more than one live fear aura (e.g. two different casters) is
    // still visited exactly once, matching the old loop's one-decision-per-entity shape.
    let mut feared_guids: Vec<u64> = auras_fear
        .iter()
        .filter(|a| a.eff_kind == crate::spell::A_CONTROL && a.eff_p0 == crate::spell::M_FEAR)
        .map(|a| a.target_guid)
        .collect();
    feared_guids.sort_unstable();
    feared_guids.dedup();
    for guid in feared_guids {
        // Issue #383: the shared gate ladder (creature, alive, this firing's covered instance).
        let Some(c) = movable_creature(ctx, guid, scope) else {
            continue;
        };
        visited += 1;
        // Must be FEARED; `fear_source` is `Some(caster)` exactly when an A_CONTROL(M_FEAR) aura is up —
        // unchanged call, re-resolving the caster (not read off the aura row above) so the "which caster"
        // tie-break is untouched by this pass.
        let Some(src_guid) = crate::spell::fear_source(ctx, c.guid) else {
            continue;
        };
        // A stun/root/poly aura OUTRANKS fear and freezes the creature — it stays put, doesn't flee.
        if crate::spell::is_movement_blocked(ctx, c.guid) {
            continue;
        }
        // Source position: the fear caster if in world, else the current melee target, else self (→ a
        // jittered +x fallback bearing in `feared_flee_step`, so a feared creature always moves).
        let src = entities.guid().find(src_guid).or_else(|| {
            melee_fear
                .attacker_guid()
                .find(c.guid)
                .and_then(|r| entities.guid().find(r.target_guid))
        });
        let (sx, sy) = src.map(|s| (s.x, s.y)).unwrap_or((c.x, c.y));
        let (nx, ny) = feared_flee_step(
            c.x,
            c.y,
            sx,
            sy,
            fear_step_for_tick(tick_secs),
            ctx.random(),
        );
        // The writer ground-snaps the panic hop (work-item 174); off-slice stays the flat z-unchanged
        // leg (c.z fallback).
        let Some((dx2, dy2, duration_ms)) =
            leg_toward((c.x, c.y), (nx, ny), constants::speeds::RUN)
        else {
            continue; // degenerate zero-length leg (the client rejects it) — skip.
        };
        to_fear_flee.push(PendingLeg {
            guid: c.guid,
            dest: (dx2, dy2),
            z_fallback: c.z,
            duration_ms,
        });
    }
    // Feared creatures RUN in terror; no ETA gate (re-stepped each tick until the aura expires).
    drain_legs(ctx, to_fear_flee, true, false, now_ms);
    visited
}
