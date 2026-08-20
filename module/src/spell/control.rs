//! Crowd-control model — the composable CC predicate lattice over one shared `has_control_mechanic`
//! scanner, PLUS (381 split) the cast-bar teardown family (`pushback_cast` / `interrupt_cast` /
//! `break_channel` / `interrupt_cast_and_lock`), moved here from the old flat `cast.rs`: this file
//! already called `pushback_cast` from `break_auras_on_damage` below, so co-locating the teardown with
//! the rest of the crowd-control model removes a cross-module callback instead of adding one. `mod.rs`
//! re-exports the public predicates so every `crate::spell::<predicate>` path resolves;
//! `is_immune_to_mechanic` is `pub(crate)` (the cast pipeline reads it) and `break_auras_on_damage` is
//! `pub(crate)` (every real-damage path calls it). The death aura cleanup at the foot of the file joins
//! them for the same reason: it is the third way an aura is torn off a unit that did not ask for it.

use spacetimedb::{log, ReducerContext, ScheduleAt, Table, TimeDuration};

// The taxonomy consts (A_CONTROL/M_*/A_IMMUNITY/P_MECHANIC) and the generated `game_aura`/`game_spell`
// accessor traits are re-exported by `mod.rs` — pull them all into scope here.
use super::*;

/// Does `unit_guid` carry ANY active `A_CONTROL` aura whose frozen `eff_p0` names `mechanic`? The
/// crowd-control twin of `combat_field_bonus`/`stat_bonus` — it reuses the IDENTICAL aura-read idiom
/// (the `game_aura().by_target().filter(&guid)` scan) but only needs presence (`.any`), not a sum, so
/// the FIRST matching aura short-circuits. The filter mirrors `combat_field_bonus`'s `eff_kind == K &&
/// eff_p0 == V` shape exactly, swapping in the control kind + mechanic. A unit with no A_CONTROL aura
/// matches nothing → `false`, so every gate built on this is a provable no-op without a control aura
/// present. [entity]
fn has_control_mechanic(ctx: &ReducerContext, unit_guid: u64, mechanic: i32) -> bool {
    auras_on(ctx, unit_guid).any(|a| a.eff_kind == A_CONTROL && a.eff_p0 == mechanic)
}

/// Does `unit_guid` carry ANY active aura from `spell_id`? Presence check (FIRST match short-circuits)
/// over the same `by_target` scan as the CC predicates. Used by the creature spell rotation (rank 20) to
/// gate "cast this debuff only if the target lacks it" (TARGET_MISSING_AURA) and "buff self only if
/// missing" (SELF_MISSING_AURA), so a creature doesn't waste casts re-applying a live aura. A unit with
/// no aura from that spell → `false`. [entity]
pub fn has_aura(ctx: &ReducerContext, unit_guid: u64, spell_id: u32) -> bool {
    auras_on(ctx, unit_guid).any(|a| a.spell_id == spell_id)
}

/// Is `unit_guid` STUNNED — carrying an active `A_CONTROL(M_STUN)` aura? A stunned unit can neither ACT
/// (no melee swing, no creature aggro/cast) NOR MOVE — the combat melee tick and EVERY creature pass
/// gate on this. Pure read over `game_aura` (no entity write); `false` for any unit without an
/// A_CONTROL(M_STUN) aura, so an un-stunned unit's combat/AI is unaffected. [entity]
pub fn is_stunned(ctx: &ReducerContext, unit_guid: u64) -> bool {
    has_control_mechanic(ctx, unit_guid, M_STUN)
}

/// Is `unit_guid` DISARMED — carrying an active `A_DISARM` aura (Warrior Disarm)? A disarmed unit's melee
/// swing loses its main-hand weapon: the swing-range seam (`combat::swing_range_ctx`) reads this and drops a
/// PLAYER to UNARMED damage / scales a CREATURE's swing to `DISARM_SWING_RETAINED_PCT`. The plain-aura twin
/// of `is_stunned` (presence over the same `by_target` scan, FIRST match short-circuits) — A_DISARM is NOT an
/// A_CONTROL mechanic, so it scans the kind directly. `false` for any unit without an A_DISARM aura, so an
/// un-disarmed unit's swing is byte-identical (baseline-safe). [entity]
pub fn is_disarmed(ctx: &ReducerContext, unit_guid: u64) -> bool {
    auras_on(ctx, unit_guid).any(|a| a.eff_kind == A_DISARM)
}

/// Is `unit_guid` ROOTED — carrying an active `A_CONTROL(M_ROOT)` aura? A rooted unit cannot MOVE (the
/// creature MOVEMENT passes gate on this) but CAN still act (it keeps swinging/aggroing/casting), so —
/// unlike `is_stunned` — root does NOT gate the swing or the action passes. A STUN also implies no
/// movement, so the movement gates check `is_stunned(..) || is_rooted(..)`. Pure read; `false` without
/// an A_CONTROL(M_ROOT) aura, so an un-rooted unit's movement is unaffected. [entity]
pub fn is_rooted(ctx: &ReducerContext, unit_guid: u64) -> bool {
    has_control_mechanic(ctx, unit_guid, M_ROOT)
}

/// Is `unit_guid` FEARED — carrying an active `A_CONTROL(M_FEAR)` aura? A feared unit cannot ACT (the
/// ACTION gates fold it into `is_action_blocked`) and does not steer its OWN movement; instead the
/// creature fear-flee pass force-walks it AWAY from the fear source each tick ("flees in terror"). It is
/// the one mechanic that MOVES the unit: stun/poly freeze (no act, no move), root pins (act, no move),
/// fear routs (no act, forced flee). Pure read over `game_aura` (no entity write); `false` for any unit
/// without an A_CONTROL(M_FEAR) aura, so an un-feared unit is unaffected. [entity]
pub fn is_feared(ctx: &ReducerContext, unit_guid: u64) -> bool {
    has_control_mechanic(ctx, unit_guid, M_FEAR)
}

/// The GUID of whoever applied `unit_guid`'s FEAR — the `caster_guid` of its first active
/// `A_CONTROL(M_FEAR)` aura, or `None` if it is not feared. The fear-flee pass steps the unit directly
/// AWAY from this source, so it both DETECTS fear (`Some`) and supplies the bearing. Reuses the same
/// `by_target` aura scan as `is_feared` (which is `fear_source(..).is_some()` in spirit) but returns the
/// caster instead of a bool. [entity]
pub fn fear_source(ctx: &ReducerContext, unit_guid: u64) -> Option<u64> {
    auras_on(ctx, unit_guid)
        .find(|a| a.eff_kind == A_CONTROL && a.eff_p0 == M_FEAR)
        .map(|a| a.caster_guid)
}

/// Is `unit_guid` INCAPACITATED — FROZEN and unable to ACT? True under STUN *or* POLYMORPH (M_POLY): both
/// fully neutralize the unit (no act AND no move), vs root which only pins it (a rooted unit still acts)
/// and fear which routs it (no act but force-moved). It is the "no act AND no move" core: the MOVEMENT
/// gate (`is_movement_blocked`) folds it in, and the ACTION gate (`is_action_blocked`) folds it in too.
/// Baseline-safe: with no poly aura `is_incapacitated == is_stunned` (a unit without a control aura is
/// `false`). [entity]
pub fn is_incapacitated(ctx: &ReducerContext, unit_guid: u64) -> bool {
    is_stunned(ctx, unit_guid) || has_control_mechanic(ctx, unit_guid, M_POLY)
}

/// Pure CC-axis lattice: given the four independent control booleans (already resolved off a live
/// `A_CONTROL` scan), derive the three composite gates the action/movement pipeline reads — lifted
/// verbatim from `is_action_blocked`/`is_movement_blocked`/`is_self_movement_suppressed`'s own boolean
/// expressions (which each build on the same `incapacitated = stunned || poly` core `is_incapacitated`
/// computes) so the fear/root/poly axis composition is unit-tested without a live ctx. Returns
/// `(action_blocked, movement_blocked, self_movement_suppressed)`.
pub(crate) fn cc_blocks(
    stunned: bool,
    rooted: bool,
    feared: bool,
    poly: bool,
) -> (bool, bool, bool) {
    let incapacitated = stunned || poly; // is_incapacitated
    let action_blocked = incapacitated || feared; // is_action_blocked
    let movement_blocked = incapacitated || rooted; // is_movement_blocked
    let self_movement_suppressed = movement_blocked || feared; // is_self_movement_suppressed
    (action_blocked, movement_blocked, self_movement_suppressed)
}

/// Is `unit_guid` ACTION-blocked — unable to ACT (no melee swing / no aggro / no assist / no cast)? True
/// under INCAPACITATE (stun/poly) *or* FEAR: a feared unit loses control of its actions exactly like an
/// incapacitated one — the only difference is movement (incapacitate freezes it in place, fear force-flees
/// it). The ACTION gates — the melee swing, the creature aggro/assist/cast passes, and the cast chokepoint
/// — gate on THIS (not on `is_incapacitated`), so fear silences as well as routs. Baseline-safe: with no
/// poly/fear aura `is_action_blocked == is_stunned`. [entity]
pub fn is_action_blocked(ctx: &ReducerContext, unit_guid: u64) -> bool {
    cc_blocks(
        is_stunned(ctx, unit_guid),
        is_rooted(ctx, unit_guid),
        is_feared(ctx, unit_guid),
        has_control_mechanic(ctx, unit_guid, M_POLY),
    )
    .0
}

/// Is `unit_guid` MOVEMENT-blocked by crowd control — held FROZEN in place? True when INCAPACITATED
/// (stun/poly) *or* ROOTED — the three mechanics that pin a unit. (Fear is NOT here: a feared unit is not
/// frozen, it is force-fled — see `is_self_movement_suppressed`.) The fear-flee pass uses this to know a
/// stun/root/poly OUTRANKS fear and keeps the unit put. `false` for any un-CC'd unit (the common path), so
/// movement is unaffected. [entity]
pub fn is_movement_blocked(ctx: &ReducerContext, unit_guid: u64) -> bool {
    cc_blocks(
        is_stunned(ctx, unit_guid),
        is_rooted(ctx, unit_guid),
        is_feared(ctx, unit_guid),
        has_control_mechanic(ctx, unit_guid, M_POLY),
    )
    .1
}

/// Is `unit_guid`'s NORMAL self-directed movement suppressed by crowd control — so the creature MOVEMENT
/// passes (patrol / chase / return / wander / low-HP flee) must NOT run their leg this tick? True when the
/// unit is either FROZEN (`is_movement_blocked`: stun/root/poly hold it in place) or FEARED (its position
/// is driven by the dedicated fear-flee pass instead). The fear-flee pass itself gates on `is_feared &&
/// !is_movement_blocked`, so a feared-but-not-frozen creature is moved by EXACTLY one pass (fear-flee), a
/// frozen one by none, and the two never both fire. `false` for any un-CC'd creature (the common path).
/// [entity]
pub fn is_self_movement_suppressed(ctx: &ReducerContext, unit_guid: u64) -> bool {
    cc_blocks(
        is_stunned(ctx, unit_guid),
        is_rooted(ctx, unit_guid),
        is_feared(ctx, unit_guid),
        has_control_mechanic(ctx, unit_guid, M_POLY),
    )
    .2
}

/// Does a `game_spell.aura_interrupt` mask carry the BREAK_ON_DAMAGE bit (bit 0)? The pure flag-decode
/// shared by `break_auras_on_damage` (an aura with this bit drops when its target takes real damage). Pure —
/// unit-tested. Every current seed spell has `aura_interrupt == 0` → false, so an un-flagged CC is unaffected.
pub(crate) fn breaks_on_damage(aura_interrupt: u16) -> bool {
    const BREAK_ON_DAMAGE: u16 = 0x1; // game_spell.aura_interrupt bit 0
    aura_interrupt & BREAK_ON_DAMAGE != 0
}

/// On real damage to `target_guid`, BOTH (a) break its "break on damage" auras — the iconic CC-breaks-when-hit
/// mechanic (a polymorph/sap/long-stun drops the instant the target takes damage) — AND (b) PUSH BACK its
/// in-progress TIMED cast (the cast bar slides, capped; it does NOT cancel — see `pushback_cast`). An aura
/// breaks when its `game_spell` header's `aura_interrupt` carries bit 0 (`BREAK_ON_DAMAGE`); the
/// cast-pushback shares the same convergence point — this is the single helper every real-damage path
/// calls, so folding `pushback_cast` here covers all three (melee swing, direct spell, DoT tick) at once.
/// Joins each aura to its header for the flag; an aura whose spell is missing is never broken (defaults to
/// 0). Collect-then-delete (never mutate while iterating). NO-OP for a target with no break-on-damage aura
/// AND no pending cast (the common path) — baseline-safe (an un-flagged CC / a unit not mid-cast is
/// unaffected). Called from every real-damage, surviving-target, `dmg > 0` branch.
///
/// `attacker_guid` is the GENUINE MELEE assailant, or `0` when the caller has no such
/// concept (every direct-spell / DoT-tick call site) — it drives ONLY the Retaliation counter-swing
/// below; it does not affect the break/pushback logic above. Procs are NOT decided here: they run in
/// `spell::proc::run_proc_pass`, at the one damage chokepoint. [entity]
pub(crate) fn break_auras_on_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    attacker_guid: u64,
    periodic: bool,
) {
    // Package notify hook: this helper is already the single fn every real-damage
    // path calls, so it doubles as the `on_damage_taken` chokepoint. Fires BEFORE the break/interrupt
    // logic — handlers observe the damage event itself, not the CC-break side effects.
    crate::hooks::fire_on_damage_taken(
        ctx,
        &crate::hooks::DamageTakenPayload {
            target_guid,
            attacker_guid,
            periodic,
        },
    );
    let auras = ctx.db.game_aura();
    let spells = ctx.db.game_spell();
    let to_break: Vec<_> = auras
        .by_target()
        .filter(&target_guid)
        .filter(|a| {
            // SAME-TICK GUARD: an aura applied THIS reducer invocation (same frozen `ctx.timestamp`) is
            // NEVER broken by THIS tick's damage. This makes a self-damaging incapacitate (Gouge: a damage
            // effect + a break-on-damage A_CONTROL in ONE cast) order-INDEPENDENT — Gouge's own damage can't
            // delete the incapacitate it just applied, but the NEXT (later, different-timestamp) hit will.
            // No-op for the melee/DoT paths (they never apply a break-on-damage aura in the same tick they
            // deal damage), so it's baseline-safe.
            a.applied_at != ctx.timestamp
                && spells
                    .spell_id()
                    .find(a.spell_id)
                    .map(|s| breaks_on_damage(s.aura_interrupt))
                    .unwrap_or(false)
        })
        .collect();
    // CC DR: break-on-damage is the THIRD removal event that starts the 15s DR window (with natural
    // expiry — scheduler::tick_auras — and dispel — effects::dispel_target; 192 review finding #1).
    // Without this stamp the window keeps the provisional apply-time value (scheduled expiry + 15s),
    // running LONGER than vanilla's actual-break + 15s. No-op for non-CC auras / creature targets.
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch();
    for a in &to_break {
        if let Some(category) = crate::spell::stacking::dr_category_for_effect(
            ctx,
            a.eff_kind,
            a.eff_p0_kind,
            a.eff_p0,
            a.target_guid,
        ) {
            crate::spell::stacking::dr_window_on_removal(ctx, a.target_guid, category, now_micros);
        }
    }
    for a in to_break {
        auras.id().delete(a.id);
    }
    // Stealth breaks on ANY real damage to the stealthed unit (melee/direct-spell/DoT, since this
    // chokepoint is called from all three) — vanilla's "getting hit reveals you" rule, independent of
    // the BREAK_ON_DAMAGE header bit above (A_STEALTH is never flagged with it). The attacker-side
    // break (landing a swing/cast reveals the ATTACKER) is a separate call at each call site; this is
    // the VICTIM-side half. No-op on a non-stealthed target (the common path).
    super::break_stealth(ctx, target_guid);
    // Cast PUSHBACK-on-damage (work-item 039): DIRECT damage (melee swing, direct spell) PUSHES BACK the
    // victim's in-progress TIMED cast (the cast bar slides `CAST_PUSHBACK_MS`, capped at
    // `CAST_PUSHBACK_MAX` pushbacks) — vanilla 1.12 does NOT cancel a regular cast on damage; only
    // Kick/Counterspell-style interrupts (`interrupt_cast_and_lock`) and CC (`interrupt_cast` via
    // `break_channel`/the aura-apply CC teardown) do that. PERIODIC (DoT-tick) damage breaks CC above but
    // does NOT push back OR interrupt a cast in vanilla 1.12 — so skip it when `periodic`. No-op without a
    // pending cast (the common path). The same-cast self-pushback is impossible: fire_pending_cast deletes
    // the PendingCast row before resolve_cast_at runs the damage effect, so a damage spell can't push back
    // the cast that is landing it.
    if !periodic {
        crate::spell::pushback_cast(ctx, target_guid);
    }
    // Retaliation (A_RETALIATE): a genuine incoming MELEE hit (`attacker_guid != 0`) on a Retaliating
    // `target_guid` provokes ONE free main-hand counter-swing back at the attacker. Guarded against
    // recursion inside `retaliate_on_hit` (the counter is a Triggered hit, so it never re-enters here).
    // No-op for a target without an A_RETALIATE aura (the common path).
    if attacker_guid != 0 {
        crate::combat::retaliate_on_hit(ctx, target_guid, attacker_guid);
    }
}

/// Is `unit_guid` IMMUNE to crowd-control `mechanic` — carrying an active `A_IMMUNITY` aura whose frozen
/// `eff_p0` names that mechanic (with `eff_p0_kind == P_MECHANIC`)? The application-gate twin of
/// `has_control_mechanic`: it reuses the IDENTICAL `game_aura().by_target()` scan, swapping the control
/// kind for the immunity kind. When true, an incoming `A_CONTROL(mechanic)` effect is REFUSED (no aura is
/// placed), so the unit is never stunned/rooted/etc. by that mechanic while the immunity is up. A unit
/// with no `A_IMMUNITY` aura (every unit today) matches nothing → `false`, so CC application is unchanged
/// without an immunity present (baseline-safe). School-mask immunity (`eff_p0_kind == P_SCHOOL_MASK`) is a
/// separate concern handled elsewhere; this checks MECHANIC immunity only. [entity]
pub(crate) fn is_immune_to_mechanic(ctx: &ReducerContext, unit_guid: u64, mechanic: i32) -> bool {
    auras_on(ctx, unit_guid)
        .any(|a| a.eff_kind == A_IMMUNITY && a.eff_p0_kind == P_MECHANIC && a.eff_p0 == mechanic)
}

// ===========================================================================================
//  Cast-bar teardown (381 split, moved from the old flat `cast.rs`) — pushback / interrupt / channel
//  break / Kick-style interrupt-and-lock. `break_auras_on_damage` above already calls `pushback_cast`.
// ===========================================================================================

/// Per-hit cast-bar slide (vanilla 1.12 "pushback"), in milliseconds — cmangos's fixed
/// `PUSHBACK_SPELL_DELAY_TIME` (500ms). Applied to a pending cast's `scheduled_at` by `pushback_cast`,
/// once per landed direct hit, up to `CAST_PUSHBACK_MAX` pushbacks.
pub(crate) const CAST_PUSHBACK_MS: i64 = 500;

/// Max pushbacks per cast — the vanilla cap of 2 (so at most +1000ms total). The
/// 3rd+ direct hit on the same cast neither delays nor cancels it; the cast simply completes on schedule.
pub(crate) const CAST_PUSHBACK_MAX: u8 = 2;

/// Pure decision: should a landed direct hit push this pending cast back further? True while
/// `pushback_count` (the number of pushbacks already applied to THIS cast) is under the cap. Extracted
/// so the cap logic is unit-tested without a live `ReducerContext` (the module crate has no ctx harness
/// by design).
pub(crate) fn should_pushback(pushback_count: u8) -> bool {
    pushback_count < CAST_PUSHBACK_MAX
}

/// PUSH BACK `caster_guid`'s in-progress (timed) cast(s) on DIRECT damage — vanilla 1.12's actual on-hit
/// behavior for a regular cast (NOT a cancel): each landed direct hit slides the pending cast's
/// `scheduled_at` by `CAST_PUSHBACK_MS`, up to `CAST_PUSHBACK_MAX` pushbacks (cmangos's
/// two-pushbacks-per-cast convention). Past the cap, further hits are a no-op — they neither delay nor
/// cancel, so the cast completes on schedule. Updating `scheduled_at` on the scheduled row reschedules
/// the pending `fire_pending_cast` callback to the new deadline (the same table SpacetimeDB primitive
/// `interrupt_cast`'s delete already leans on to CANCEL it outright).
///
/// Only called for DIRECT damage (`break_auras_on_damage` skips this when `periodic`, matching vanilla:
/// a DoT tick does not push back a cast). Kick/Counterspell-style interrupts stay on the unconditional
/// `interrupt_cast`/`interrupt_cast_and_lock` cancel path — untouched by this function.
///
/// Emits a `game_spell_cast_event` row with `delay_ms > 0` per actual pushback (never for a capped-out
/// no-op hit) so the gateway relays `SMSG_SPELL_DELAYED{guid, delay_time}` and the client's cast bar
/// visibly shifts. No-op for a caster with no pending cast (the common path) — every real-damage call
/// site can call this unconditionally. [entity]
pub(crate) fn pushback_cast(ctx: &ReducerContext, caster_guid: u64) {
    let pending = ctx.db.game_pending_cast();
    // perf catalog 1.18 — `by_caster` probe; this runs off every landed direct hit.
    let hits: Vec<PendingCast> = pending.by_caster().filter(&caster_guid).collect();
    for mut p in hits {
        if !should_pushback(p.pushback_count) {
            continue; // cap already reached: no further delay, no cancel — the cast completes untouched
        }
        let ScheduleAt::Time(fire_at) = p.scheduled_at else {
            continue; // a cast-bar row is always a one-shot Time; defensive no-op on the Interval variant
        };
        let Some(new_fire_at) =
            fire_at.checked_add(TimeDuration::from_micros(CAST_PUSHBACK_MS * 1000))
        else {
            continue; // timestamp overflow (practically unreachable) — leave the cast untouched
        };
        let spell_id = p.spell_id;
        p.pushback_count += 1;
        let new_count = p.pushback_count;
        p.scheduled_at = ScheduleAt::Time(new_fire_at);
        pending.scheduled_id().update(p);
        // PUSHBACK signal: the gateway relays SMSG_SPELL_DELAYED so the caster's cast bar visibly shifts.
        // All other cols default/zero (this is neither a START, a GO, nor an interrupt).
        ctx.db.game_spell_cast_event().insert(SpellCastEvent {
            delay_ms: CAST_PUSHBACK_MS as u32,
            ..SpellCastEvent::signal(ctx, caster_guid, spell_id)
        });
        log::info!(
            "cast pushed back: caster {caster_guid}'s spell {spell_id} delayed {CAST_PUSHBACK_MS}ms \
             (pushback {new_count}/{CAST_PUSHBACK_MAX})"
        );
    }
}

/// Cancel `caster_guid`'s in-progress (timed) cast(s) — delete its `game_pending_cast` row(s), which also
/// cancels the scheduled `fire_pending_cast`, so the spell never resolves (rank #118 CC interrupt).
/// Reached through the `by_caster` index (perf catalog 1.18) — `break_channel` calls this on every
/// moving heartbeat, so the old full scan sat on the hottest path. Returns whether anything was interrupted; a caster with no pending cast → `false` (no-op). The
/// client-facing cast-bar cancel (an `SMSG_SPELL_FAILED`/interrupted wire push) is a gateway follow-up —
/// the server-authoritative effect (the cast simply never lands) is complete here. [entity]
pub(crate) fn interrupt_cast(ctx: &ReducerContext, caster_guid: u64) -> bool {
    let pending = ctx.db.game_pending_cast();
    // Capture (scheduled_id, spell_id) BEFORE deletion — the spell_id rides the interrupt signal row.
    let hits: Vec<(u64, u32)> = pending
        .by_caster()
        .filter(&caster_guid)
        .map(|p| (p.scheduled_id, p.spell_id))
        .collect();
    let interrupted = !hits.is_empty();
    for (id, spell_id) in hits {
        pending.scheduled_id().delete(id);
        // INTERRUPT signal: one game_spell_cast_event row (is_interrupted=true) per cancelled cast → the
        // gateway relays SMSG_SPELL_FAILURE{spell, Interrupted} to the caster. cast_time_ms 0 so it does NOT
        // take the START branch; damage 0 so no damage log. All other cols default.
        ctx.db.game_spell_cast_event().insert(SpellCastEvent {
            is_interrupted: true,
            ..SpellCastEvent::signal(ctx, caster_guid, spell_id)
        });
    }
    if interrupted {
        log::info!("cast interrupted: caster {caster_guid}'s in-progress cast cancelled");
    }
    interrupted
}

/// BREAK a channel on `caster_guid` — the ONE convergent break path for the four channel-interrupt triggers
/// (the caster MOVES / starts a NEW cast / is CC'd / its channel target dies). Deletes the caster's
/// `A_PERIODIC_TRIGGER` channel aura(s) so `tick_auras` stops firing the per-tick missile, AND cancels any
/// in-progress timed cast via `interrupt_cast` (a channel and a cast-bar are mutually exclusive in practice,
/// but covering both keeps every call site a single tear-down). Collect-then-delete (never mutate while
/// iterating), modeled on `break_stealth`. Returns whether anything was broken. NO-OP for a caster with no
/// channel/cast (the common path) — every call site can fire it unconditionally. Generic over the kind,
/// never a spell id. [entity]
pub(crate) fn break_channel(ctx: &ReducerContext, caster_guid: u64) -> bool {
    let auras = ctx.db.game_aura();
    let spells = ctx.db.game_spell();
    // A channel is a SELF-aura → it sits on the caster's own `target_guid`; scope by `by_target` then
    // filter the channel kind (the same idiom as break_stealth's A_STEALTH scope). A_PERIODIC_TRIGGER is
    // ALWAYS a channel tick (Arcane Missiles). A_PERIODIC_ENERGIZE is channel-borne ONLY when its parent
    // spell carries the CHANNELED flag (Evocation) — a NON-channeled periodic energize (Bloodrage's rage
    // trickle) must survive, so it is gated on a header join to `cast_flags & SPELL_ATTR_CHANNELED`.
    let ids: Vec<u64> = auras
        .by_target()
        .filter(&caster_guid)
        .filter(|a| {
            a.eff_kind == A_PERIODIC_TRIGGER
                || (a.eff_kind == A_PERIODIC_ENERGIZE
                    && spells
                        .spell_id()
                        .find(a.spell_id)
                        .map(|s| s.cast_flags & SPELL_ATTR_CHANNELED != 0)
                        .unwrap_or(false))
        })
        .map(|a| a.id)
        .collect();
    let broke_channel = !ids.is_empty();
    for id in ids {
        auras.id().delete(id);
    }
    // Also tear down a timed cast bar (no-op if none). `interrupt_cast` logs its own cancel.
    let broke_cast = interrupt_cast(ctx, caster_guid);
    if broke_channel {
        log::info!("channel broken: caster {caster_guid}'s channel cancelled");
    }
    broke_channel || broke_cast
}

/// KICK: cancel `target_guid`'s in-progress cast (via `interrupt_cast`) AND lock the interrupted cast's
/// SCHOOL for `lockout_ms` milliseconds — the Kick "silence". Reads each pending cast's `spell_id` BEFORE
/// deletion to resolve its `game_spell.school_mask`, then writes/refreshes a `game_school_lockout` row
/// keyed `(target, school)` so the `resolve_cast_at` gate refuses that school until the lock expires.
/// Distinct from the plain `interrupt_cast` (the CC-interrupt path stays a pure cancel — no school lock
/// when a stun/poly/fear happens to interrupt a cast). The `lockout_ms` value is read from the
/// interrupting spell's `duration_ms` at the call site (Kick=5000, Counterspell=8000, Pummel=4000).
/// No-op when the target isn't mid-cast. Generic — never a spell id. [entity]
pub(crate) fn interrupt_cast_and_lock(
    ctx: &ReducerContext,
    target_guid: u64,
    lockout_ms: i64,
) -> bool {
    let pending = ctx.db.game_pending_cast();
    // Capture the interrupted spell ids BEFORE deletion — they drive the school lockout below.
    let spell_ids: Vec<u32> = pending
        .by_caster()
        .filter(&target_guid)
        .map(|p| p.spell_id)
        .collect();
    let interrupted = interrupt_cast(ctx, target_guid);
    if interrupted {
        let until = ctx
            .timestamp
            .checked_add(TimeDuration::from_micros(lockout_ms * 1000))
            .unwrap_or(ctx.timestamp);
        let locks = ctx.db.game_school_lockout();
        for spell_id in spell_ids {
            let Some(school) = ctx
                .db
                .game_spell()
                .spell_id()
                .find(spell_id)
                .map(|h| h.school_mask)
            else {
                continue; // unknown spell row → nothing to lock (defensive; should not happen)
            };
            // Refresh in place if this caster already has a lock for the school, else insert.
            if let Some(mut row) = locks
                .by_caster()
                .filter(&target_guid)
                .find(|r| r.school == school)
            {
                row.until = until;
                locks.id().update(row);
            } else {
                locks.insert(SchoolLockout {
                    id: 0,
                    caster_guid: target_guid,
                    school,
                    until,
                });
            }
        }
        log::info!(
            "kick: caster {target_guid}'s cast cancelled + school locked for {lockout_ms}ms"
        );
    }
    interrupted
}

// ===========================================================================================
//  Death aura cleanup [entity]
// ===========================================================================================

/// Does an aura survive its carrier's DEATH? Vanilla sheds everything a unit was carrying the moment it
/// dies — cmangos `Unit::SetDeathState(JUST_DIED)` calls `Unit::RemoveAllAurasOnDeath`, whose whole body
/// is `if (!holder->IsPassive() && !holder->IsDeathPersistent()) remove`. Two exempt classes, then: a
/// PASSIVE aura (a talent bonus, a racial, a weapon specialisation — the unit *is* it rather than
/// carrying it), and a spell flagged death-persistent (`AttributesEx3` bit 20; 93 spells in 1.12.1,
/// including Ghost, Resurrection Sickness and the Warrior stances). Nothing else is spared: a mount, a
/// buff, a DoT and a crowd-control aura all come off.
///
/// This module keeps the PASSIVE half and sheds everything else. `game_spell.attributes` carries the raw
/// `Spell.dbc` Attributes column, so the passive bit is real imported data; `AttributesEx3` is not
/// imported, so the death-persistent half has no data to read. Of that vanilla exempt list only
/// Resurrection Sickness exists here as an aura at all — the Warrior stances are a `WorldEntity` field
/// and Ghost is a player flag, both of which survive death by construction — so the gap is one debuff a
/// second death clears early. Carrying `AttributesEx3` into `game_spell` is what closes it.
///
/// Pure over the attribute mask, so the policy is one testable decision rather than a filter buried in a
/// scan. A spell with no header at all reads as not-passive at the call site and is shed: unauthored
/// data losing an aura on death is the recoverable direction.
pub(crate) fn aura_survives_death(attributes: u32) -> bool {
    attributes & SPELL_ATTR_PASSIVE != 0
}

/// Shed `unit_guid`'s auras because it just DIED — the ONE death aura-cleanup path, called from the two
/// kill chokepoints (`combat::kill_player` / `combat::kill_creature`) so every lethal path leaves the
/// same corpse. Deletes every aura row [`aura_survives_death`] does not exempt, then converges the three
/// projections an aura removal can move: the vitals pools, the character sheet, and the Mount Projection
/// — the same collect-a-predicate-then-recompute tail `do_cancel_aura` and the expiry reap already use,
/// so a rider who dies is dismounted by ordinary aura removal and never by a mount-specific special case.
///
/// Collect-then-delete (never mutate while iterating). A unit carrying nothing but passives is a complete
/// no-op, and running it twice changes nothing the second time. [entity]
pub(crate) fn remove_auras_on_death(ctx: &ReducerContext, unit_guid: u64) {
    let spells = ctx.db.game_spell();
    let shed: Vec<Aura> = auras_on(ctx, unit_guid)
        .filter(|a| {
            !spells
                .spell_id()
                .find(a.spell_id)
                .map(|s| aura_survives_death(s.attributes))
                .unwrap_or(false)
        })
        .collect();
    shed_auras(ctx, unit_guid, shed);
}

/// Delete a collected set of `unit_guid`'s aura rows and converge every projection the removal can
/// move: the vitals pools, the character sheet, and the Mount Projection. THE shared tail behind an
/// involuntary aura removal — the death shed above and a Proc that spent its last charge both reduce
/// to it, so a removal path cannot leave a stale max-health or a corpse still projecting a mount.
///
/// The caller decides WHICH rows come off; this decides what that costs. Rows must already be
/// collected (never mutate the table while iterating it), and the recomputes run AFTER the deletes,
/// vitals first so a lost STA/INT aura's sheet recompute sees the already-updated Spirit. An empty set
/// is a complete no-op, and running it twice changes nothing the second time. [entity]
pub(crate) fn shed_auras(ctx: &ReducerContext, unit_guid: u64, shed: Vec<Aura>) {
    if shed.is_empty() {
        return;
    }
    let mut revitalize = false;
    let mut resheet = false;
    let mut remount = false;
    for a in &shed {
        revitalize |= aura_moves_vitals(a.eff_kind, a.eff_p0);
        resheet |= crate::spell::aura_moves_sheet(a.eff_kind, a.eff_p0);
        remount |= crate::mount::mount_aura_moves_mount(a.eff_kind, a.eff_p0);
    }
    let auras = ctx.db.game_aura();
    for a in shed {
        auras.id().delete(a.id);
    }
    if revitalize {
        recompute_vitals(ctx, unit_guid);
    }
    if resheet {
        recompute_sheet(ctx, unit_guid);
    }
    if remount {
        crate::mount::recompute_mount(ctx, unit_guid);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        aura_survives_death, cc_blocks, should_pushback, CAST_PUSHBACK_MAX, CAST_PUSHBACK_MS,
        SPELL_ATTR_PASSIVE,
    };

    /// The un-CC'd baseline: no stun/root/fear/poly → nothing is blocked on any axis.
    #[test]
    fn cc_blocks_baseline_uncontrolled_unit_blocks_nothing() {
        assert_eq!(cc_blocks(false, false, false, false), (false, false, false));
    }

    /// STUN: fully frozen — blocks ACT and MOVE (and therefore self-movement), independent of root/fear/poly.
    #[test]
    fn cc_blocks_stun_blocks_action_and_movement() {
        assert_eq!(cc_blocks(true, false, false, false), (true, true, true));
        // Stacked with every other mechanic too — stun alone is already sufficient, so OR'ing in more never
        // un-blocks anything.
        assert_eq!(cc_blocks(true, true, true, true), (true, true, true));
    }

    /// POLY (incapacitate): frozen exactly like stun — blocks ACT and MOVE. Baseline-safe equivalence:
    /// poly alone must match stun alone bit-for-bit.
    #[test]
    fn cc_blocks_poly_blocks_action_and_movement_like_stun() {
        assert_eq!(cc_blocks(false, false, false, true), (true, true, true));
        assert_eq!(
            cc_blocks(false, false, false, true),
            cc_blocks(true, false, false, false)
        );
    }

    /// ROOT alone: pins MOVEMENT (and self-movement suppression), but does NOT block ACTION — a rooted
    /// unit keeps swinging/aggroing/casting.
    #[test]
    fn cc_blocks_root_blocks_movement_only() {
        assert_eq!(cc_blocks(false, true, false, false), (false, true, true));
    }

    /// FEAR alone: blocks ACTION and self-movement (the fear-flee pass force-walks it), but does NOT set
    /// the FROZEN `movement_blocked` gate — a feared unit is force-MOVED, not pinned in place.
    #[test]
    fn cc_blocks_fear_blocks_action_and_self_movement_but_not_frozen_movement() {
        assert_eq!(cc_blocks(false, false, true, false), (true, false, true));
    }

    /// ROOT + FEAR together: action blocked (by fear), movement blocked (by root), self-movement
    /// suppressed (by either) — every axis trips, but for two DIFFERENT reasons than stun/poly.
    #[test]
    fn cc_blocks_root_and_fear_together_trip_every_axis() {
        assert_eq!(cc_blocks(false, true, true, false), (true, true, true));
    }

    /// Exhaustive sweep over all 16 (stunned, rooted, feared, poly) combinations: `action_blocked` is the
    /// OR of (stunned, feared, poly) — root plays NO part; `movement_blocked` is the OR of (stunned, rooted,
    /// poly) — fear plays NO part; `self_movement_suppressed` is the OR of all four.
    #[test]
    fn cc_blocks_matches_the_closed_form_across_every_combination() {
        for stunned in [false, true] {
            for rooted in [false, true] {
                for feared in [false, true] {
                    for poly in [false, true] {
                        let (action, movement, self_move) =
                            cc_blocks(stunned, rooted, feared, poly);
                        assert_eq!(
                            action,
                            stunned || feared || poly,
                            "action@{stunned},{rooted},{feared},{poly}"
                        );
                        assert_eq!(
                            movement,
                            stunned || rooted || poly,
                            "movement@{stunned},{rooted},{feared},{poly}"
                        );
                        assert_eq!(
                            self_move,
                            stunned || rooted || feared || poly,
                            "self_move@{stunned},{rooted},{feared},{poly}"
                        );
                    }
                }
            }
        }
    }

    /// Cast pushback (work-item 039): the per-hit slide is cmangos's fixed 500ms, and the cap is 2
    /// pushbacks per cast (the vanilla cap) — so a 3rd+ direct hit is a documented no-op.
    #[test]
    fn pushback_constants_match_the_cmangos_convention() {
        assert_eq!(CAST_PUSHBACK_MS, 500);
        assert_eq!(CAST_PUSHBACK_MAX, 2);
    }

    /// The pure cap decision: pushback is allowed strictly BELOW the cap (0 and 1 pushbacks-so-far both
    /// still allow one more), and refused AT or ABOVE it (2, 3, and beyond all refuse — the 3rd+ hit
    /// neither delays nor cancels the cast).
    #[test]
    fn should_pushback_allows_up_to_the_cap_then_refuses() {
        assert!(should_pushback(0));
        assert!(should_pushback(1));
        assert!(!should_pushback(2));
        assert!(!should_pushback(3));
        assert!(!should_pushback(u8::MAX));
    }

    /// Closed-form sweep: `should_pushback(n)` is exactly `n < CAST_PUSHBACK_MAX` for every count in a
    /// wide range — a single source of truth check that the cap boundary can't drift silently.
    #[test]
    fn should_pushback_matches_the_closed_form_across_a_range() {
        for n in 0u8..=20 {
            assert_eq!(should_pushback(n), n < CAST_PUSHBACK_MAX, "n={n}");
        }
    }

    /// THE DEATH AURA POLICY, at the one decision that carries it. Vanilla's rule
    /// (`Unit::RemoveAllAurasOnDeath`) exempts passive holders and sheds every other aura class, so a
    /// mount, a buff, a DoT and a crowd-control aura all come off a corpse — the seeded rows here are
    /// ordinary spells with `attributes == 0` and none of them survive. The exemption is read from the
    /// PASSIVE bit alone and ignores every other `Spell.dbc` Attributes bit, so an unrelated attribute
    /// can never accidentally spare an aura.
    #[test]
    fn death_sheds_every_aura_except_a_passive_one() {
        assert!(aura_survives_death(SPELL_ATTR_PASSIVE));
        // A talent passive carries other Attributes bits alongside PASSIVE; it still survives.
        assert!(aura_survives_death(SPELL_ATTR_PASSIVE | 0x4 | 0x10000));

        assert!(!aura_survives_death(0)); // every seeded spell — mount, buff, DoT, CC
        assert!(!aura_survives_death(0x4)); // an unrelated attribute never spares an aura
        assert!(!aura_survives_death(!SPELL_ATTR_PASSIVE));
    }

    /// BOTH kill chokepoints shed — vanilla's removal sits on `Unit`, so a creature drops its auras on
    /// death exactly like a player. In `kill_creature` the shed must run AFTER the reward step, which
    /// still reads the victim's live aura set to find the killer's Drain Soul channel: shedding first
    /// would silently stop soul shards dropping. Pinned by scan — both functions are
    /// `ReducerContext`-bound and the crate has no reducer harness.
    #[test]
    fn both_kill_chokepoints_shed_auras_on_death() {
        let src = include_str!("../combat/death.rs");
        for signature in ["pub(crate) fn kill_player(", "pub(crate) fn kill_creature("] {
            let body = crate::test_scan::code_of(src, signature);
            assert!(
                body.contains("crate::spell::remove_auras_on_death(ctx, "),
                "`{signature}` must shed the dying unit's auras — otherwise a corpse keeps its mount, \
                 its buffs and the crowd control that was on it. Body was:\n{body}"
            );
        }
        let creature = crate::test_scan::code_of(src, "pub(crate) fn kill_creature(");
        let rewards = creature
            .find("award_killer_rewards(ctx, &target, target_guid, killer_guid)")
            .expect("kill_creature still awards the killer before the corpse work");
        let shed = creature
            .find("crate::spell::remove_auras_on_death(ctx, target_guid)")
            .expect("checked above");
        assert!(
            rewards < shed,
            "the death shed must run AFTER `award_killer_rewards`, which reads the dying creature's \
             Drain Soul channel aura to mint the killer's soul shard. Body was:\n{creature}"
        );
    }

    /// The shed's shape: it reads the exemption through the ONE policy predicate (never an inline
    /// attribute test or a spell-id list), and hands its rows to the shared removal tail, which
    /// converges the same three recomputes every other aura removal site runs — vitals, then sheet,
    /// then the Mount Projection. Dropping any of them leaves a corpse projecting a stat pool, a
    /// paperdoll number or a mount it no longer has.
    #[test]
    fn the_death_shed_reads_the_policy_and_converges_every_projection() {
        let collect = crate::test_scan::code_of(
            include_str!("control.rs"),
            "pub(crate) fn remove_auras_on_death(",
        );
        assert!(
            collect.contains("aura_survives_death(s.attributes)"),
            "the shed must ask the one policy predicate. Body was:\n{collect}"
        );
        assert!(
            collect.contains("shed_auras(ctx, unit_guid, shed)"),
            "the shed must hand its collected rows to the shared removal tail. Body was:\n{collect}"
        );
        let body =
            crate::test_scan::code_of(include_str!("control.rs"), "pub(crate) fn shed_auras(");
        let vitals = body
            .find("recompute_vitals(ctx, unit_guid)")
            .expect("the shed must re-derive the vitals pools a lost STA/INT aura fed");
        let sheet = body
            .find("recompute_sheet(ctx, unit_guid)")
            .expect("the shed must re-derive the character sheet");
        let mount = body
            .find("crate::mount::recompute_mount(ctx, unit_guid)")
            .expect("the shed must re-derive the Mount Projection, or a corpse keeps riding");
        assert!(
            vitals < sheet && sheet < mount,
            "vitals must recompute before the sheet, so a lost stat aura's sheet recompute sees the \
             already-updated spirit. Body was:\n{body}"
        );
    }
}
