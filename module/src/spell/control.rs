//! Crowd-control model — the composable CC predicate lattice over one shared `has_control_mechanic`
//! scanner. `mod.rs` re-exports the public predicates so every `crate::spell::<predicate>` path
//! resolves; `is_immune_to_mechanic` is `pub(crate)` (the cast pipeline reads it) and
//! `break_auras_on_damage` is `pub(crate)` (every real-damage path calls it).

use spacetimedb::ReducerContext;

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
/// concept (every direct-spell / DoT-tick call site) — it drives ONLY the `A_PROC_ON_HIT` reactive-chill
/// scan below (Frost Armor); it does not affect the break/pushback logic above. [entity]
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
    // expiry — scheduler::tick_auras — and dispel — math::dispel_target; 192 review finding #1).
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
    // Proc-on-hit: a genuine MELEE hit (`attacker_guid != 0`) on `target_guid` scans its
    // `A_PROC_ON_HIT` auras and reactively applies each one's frozen trigger spell onto the ATTACKER (Frost
    // Armor chills whoever hits its wearer in melee). No-op for the common case — every direct-spell/DoT call
    // site passes `attacker_guid == 0`, and a target with no `A_PROC_ON_HIT` aura is unaffected either way.
    if attacker_guid != 0 {
        proc_on_hit(ctx, target_guid, attacker_guid);
        // Retaliation (A_RETALIATE): a genuine incoming MELEE hit on a Retaliating `target_guid` provokes
        // ONE free main-hand counter-swing back at the attacker. Guarded against recursion inside
        // `retaliate_on_hit` (the counter routes through the spell-damage path with attacker sentinel 0, so
        // it never re-enters here). No-op for a target without an A_RETALIATE aura (the common path).
        crate::combat::retaliate_on_hit(ctx, target_guid, attacker_guid);
    }
}

/// The `A_PROC_ON_HIT` half of `break_auras_on_damage`: for every active such aura on `victim_guid` (the
/// carrier, e.g. a Frost-Armored mage), apply its frozen trigger spell (`eff_p1`, e.g. Chilled 6136) onto
/// `attacker_guid` (the melee assailant) — via `apply_linked_debuff`, the SAME "load spell X's aura effects
/// and place them on Y" machinery PW:Shield's Weakened Soul link already uses, with X = the trigger spell and
/// Y = the attacker. Attributed to the CARRIER's own `caster_guid`/`level` (frozen on the proc aura at its own
/// apply) — mirroring how a linked debuff is attributed to its parent aura's caster. Collected before
/// applying (mirrors the collect-then-mutate idiom above); a victim with no `A_PROC_ON_HIT` aura (every unit
/// without Frost Armor) is a no-op.
fn proc_on_hit(ctx: &ReducerContext, victim_guid: u64, attacker_guid: u64) {
    let procs: Vec<(u32, u64, u8)> = auras_on(ctx, victim_guid)
        .filter(|a| a.eff_kind == A_PROC_ON_HIT && a.eff_p1 != 0)
        .map(|a| (a.eff_p1 as u32, a.caster_guid, a.level))
        .collect();
    for (trigger_spell, caster_guid, level) in procs {
        crate::spell::apply_linked_debuff(ctx, trigger_spell, caster_guid, attacker_guid, level);
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

#[cfg(test)]
mod tests {
    use super::cc_blocks;

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
}
