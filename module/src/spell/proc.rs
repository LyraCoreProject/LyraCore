//! The **Proc** engine: one decision, one pass, one chokepoint.
//!
//! A Proc is an aura whose row carries its own proc profile — the combat-event mask, the chance or
//! procs-per-minute rate, the charges left, the internal cooldown and its ready time, and the filter
//! on which spells may trigger it. The unit wearing the aura is the **Carrier**; the unit on the other
//! side of the hit is the **Counterparty**.
//!
//! [`run_proc_pass`] is the only `ReducerContext`-bound function here and `combat::apply_hit` — the one
//! chokepoint every damaging hit routes through — is its only caller, so a new damage path cannot
//! forget procs. Everything the pass decides ([`hit_bits`], [`proc_chance_bp`], [`decide`]) is pure and
//! unit-tested below: the pass reads rows, asks the decision, and writes the outcome.
//!
//! Firing means starting a **Triggered Cast** ([`crate::spell::cast_triggered`]) of the frozen trigger
//! spell from the Carrier at the Counterparty. A Triggered Cast pays nothing, passes no Gates, starts
//! no cooldown, and every hit it produces is marked Triggered — which fires no proc pass, so two
//! shields can never ping-pong.

use spacetimedb::{log, ReducerContext};

// The `game_world_entity` accessor trait must be in scope to read the Carrier's base attack time.
use crate::game_world_entity;

use crate::combat::{Hit, HitSource};
// The taxonomy consts, the `Aura`/`Spell` rows, the `game_aura` accessor trait and `auras_on` are all
// re-exported by `spell::mod`.
use super::*;

// ===========================================================================================
//  Vanilla `procFlags` / `procEx` bits (cmangos `SpellMgr.h`), stored VERBATIM on the row.
//  The engine names only the bits it fires; the rest import and stay inert.
// ===========================================================================================

/// Dealer, main-hand auto-hit. An off-hand swing sets this bit too (vanilla sets both).
pub(crate) const PROC_FLAG_SUCCESSFUL_MELEE_HIT: u32 = 0x0000_0004;
/// Victim, any melee auto-hit.
pub(crate) const PROC_FLAG_TAKEN_MELEE_HIT: u32 = 0x0000_0008;
/// Dealer, melee ability (weapon strike / finisher).
pub(crate) const PROC_FLAG_SUCCESSFUL_MELEE_SPELL_HIT: u32 = 0x0000_0010;
/// Victim, melee ability.
pub(crate) const PROC_FLAG_TAKEN_MELEE_SPELL_HIT: u32 = 0x0000_0020;
/// Dealer, ranged auto-shot.
pub(crate) const PROC_FLAG_SUCCESSFUL_RANGED_HIT: u32 = 0x0000_0040;
/// Victim, ranged auto-shot.
pub(crate) const PROC_FLAG_TAKEN_RANGED_HIT: u32 = 0x0000_0080;
/// Dealer, direct damaging spell.
pub(crate) const PROC_FLAG_SUCCESSFUL_NEGATIVE_SPELL_HIT: u32 = 0x0001_0000;
/// Victim, direct damaging spell.
pub(crate) const PROC_FLAG_TAKEN_NEGATIVE_SPELL_HIT: u32 = 0x0002_0000;
/// Dealer, off-hand auto-hit — its own bit, so an off-hand-only proc fires off off-hand swings alone.
pub(crate) const PROC_FLAG_SUCCESSFUL_OFFHAND_HIT: u32 = 0x0080_0000;

/// `procEx`: the hit was an ordinary (non-critical) landed hit.
pub(crate) const PROC_EX_NORMAL_HIT: u32 = 0x0000_0001;
/// `procEx`: the hit was a critical hit.
pub(crate) const PROC_EX_CRITICAL_HIT: u32 = 0x0000_0002;

/// Basis points in a whole — the roll space every chance in this module is expressed in.
const BASIS_POINTS: u32 = 10_000;

/// The vanilla procs-per-minute divisor: `chance_pct = ppm * attack_time_ms / 600`, i.e. the rate per
/// minute scaled by how much of a minute one swing takes.
const PPM_DIVISOR_PCT: f32 = 600.0;

// ===========================================================================================
//  The frozen profile + the hit facts the decision reads
// ===========================================================================================

/// Which side of the hit a Carrier is on. Decides which half of [`ProcHit`]'s event bits the profile
/// is matched against, and whether procs-per-minute applies (it is a rate per swing, so only the unit
/// that swung has an attack time to scale by).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ProcSide {
    /// The Carrier dealt the hit.
    Dealer,
    /// The Carrier took the hit.
    Victim,
}

/// One Carrier's frozen proc profile, read off its aura row. Everything the decision needs and nothing
/// else, so [`decide`] is a pure function of data rather than of a live table.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProcProfile {
    /// The combat-event mask (vanilla `procFlags`).
    pub flags: u32,
    /// Flat chance, percent.
    pub chance: u8,
    /// Procs-per-minute rate; replaces `chance` when the Carrier is the dealer.
    pub ppm: f32,
    /// Vanilla `procEx`: the normal-hit / critical-hit rule.
    pub proc_ex: u32,
    /// School filter on the triggering spell; 0 = any school.
    pub school_mask: u8,
    /// Spell-family filter (name half); 0 = any family.
    pub family_name: u8,
    /// Spell-family filter (flags half); 0 = any flag of the named family.
    pub family_flags: u64,
    /// Charges left; 0 = unlimited.
    pub charges: u8,
    /// Internal-cooldown length; 0 = none.
    pub icd_ms: u32,
    /// Micros-since-epoch the internal cooldown is up.
    pub ready_micros: i64,
}

impl ProcProfile {
    /// Read a Carrier's profile off its aura row.
    pub(crate) fn of(a: &Aura) -> Self {
        Self {
            flags: a.proc_flags,
            chance: a.proc_chance,
            ppm: a.proc_ppm,
            proc_ex: a.proc_ex,
            school_mask: a.proc_school_mask,
            family_name: a.proc_family_name,
            family_flags: a.proc_family_flags,
            charges: a.proc_charges,
            icd_ms: a.proc_icd_ms,
            ready_micros: a.proc_ready_micros,
        }
    }
}

/// The spell that carried a hit, for the school/family filter. `None` for an auto-attack swing, which
/// no filter may exclude (a swing belongs to no school and no family).
#[derive(Clone, Copy, Debug)]
pub(crate) struct TriggeringSpell {
    pub school_mask: u8,
    pub family_name: u8,
    pub family_flags: u64,
}

/// The facts of one hit the decision reads: which event bits it set on each side, what spell carried
/// it, whether it crit, and the dealer's attack time (the procs-per-minute scale).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProcHit {
    pub dealer_bits: u32,
    pub victim_bits: u32,
    pub spell: Option<TriggeringSpell>,
    pub crit: bool,
    pub attack_time_ms: u32,
}

impl ProcHit {
    /// The event bits this hit set for a Carrier on `side`.
    fn bits(&self, side: ProcSide) -> u32 {
        match side {
            ProcSide::Dealer => self.dealer_bits,
            ProcSide::Victim => self.victim_bits,
        }
    }
}

/// What a fired Proc leaves behind on its aura row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ProcFire {
    /// Charges after this fire; 0 stays 0 for an unlimited Proc.
    pub charges_left: u8,
    /// The new internal-cooldown deadline (unchanged when the Proc has no cooldown).
    pub ready_micros: i64,
    /// The last charge was spent — every aura row of this spell comes off the Carrier.
    pub remove_buff: bool,
}

// ===========================================================================================
//  The pure decision
// ===========================================================================================

/// Is `kind` one of the two Proc aura kinds?
pub(crate) fn is_proc_kind(kind: u8) -> bool {
    kind == A_PROC_TRIGGER || kind == A_PROC_DAMAGE
}

/// The event bits one hit sets: `(dealer, victim)`. An off-hand swing sets the shared melee bit AND
/// its own off-hand bit on the dealer, so both a dual-wield proc and an off-hand-only proc see it; the
/// victim reads one melee-taken bit either way. A Triggered hit sets nothing at all, which is what
/// keeps a proc from chaining off its own trigger.
pub(crate) fn hit_bits(hit: &Hit) -> (u32, u32) {
    match hit.source {
        HitSource::MainHand => (PROC_FLAG_SUCCESSFUL_MELEE_HIT, PROC_FLAG_TAKEN_MELEE_HIT),
        HitSource::OffHand => (
            PROC_FLAG_SUCCESSFUL_MELEE_HIT | PROC_FLAG_SUCCESSFUL_OFFHAND_HIT,
            PROC_FLAG_TAKEN_MELEE_HIT,
        ),
        HitSource::Ranged => (PROC_FLAG_SUCCESSFUL_RANGED_HIT, PROC_FLAG_TAKEN_RANGED_HIT),
        HitSource::MeleeSpell => (
            PROC_FLAG_SUCCESSFUL_MELEE_SPELL_HIT,
            PROC_FLAG_TAKEN_MELEE_SPELL_HIT,
        ),
        HitSource::Spell => (
            PROC_FLAG_SUCCESSFUL_NEGATIVE_SPELL_HIT,
            PROC_FLAG_TAKEN_NEGATIVE_SPELL_HIT,
        ),
        HitSource::Triggered => (0, 0),
    }
}

/// The chance this Proc fires, in basis points, capped at certainty. Procs-per-minute replaces the
/// flat chance only for a Carrier that DEALT the hit: the rate is per swing, and a taken-side Proc has
/// no swing of its own to scale by (its attacker's weapon must not decide its rate). A flat chance at
/// or above 100 percent returns the whole roll space, so it always passes.
pub(crate) fn proc_chance_bp(profile: &ProcProfile, side: ProcSide, attack_time_ms: u32) -> u32 {
    if profile.ppm > 0.0 && side == ProcSide::Dealer {
        let pct = profile.ppm * attack_time_ms as f32 / PPM_DIVISOR_PCT;
        return ((pct * 100.0) as u32).min(BASIS_POINTS);
    }
    (profile.chance as u32 * 100).min(BASIS_POINTS)
}

/// Does the triggering spell pass this Proc's school and family filter? A hit with no spell (an
/// auto-attack swing) is never filtered — the caller passes `None` and this is not consulted.
fn filter_matches(profile: &ProcProfile, spell: &TriggeringSpell) -> bool {
    if profile.school_mask != 0 && profile.school_mask & spell.school_mask == 0 {
        return false;
    }
    if profile.family_name != 0 {
        if profile.family_name != spell.family_name {
            return false;
        }
        // The same family match the spell-modifier fold applies: the name must agree, and a non-zero
        // flag mask must overlap. A named family with no flags accepts every spell of that family.
        if profile.family_flags != 0 && profile.family_flags & spell.family_flags == 0 {
            return false;
        }
    }
    true
}

/// The `procEx` hit-quality rule: the critical-hit bit alone means crit only, the normal-hit bit alone
/// means non-crit only, and neither (or both) accepts any landed hit.
fn proc_ex_allows(proc_ex: u32, crit: bool) -> bool {
    match (
        proc_ex & PROC_EX_NORMAL_HIT != 0,
        proc_ex & PROC_EX_CRITICAL_HIT != 0,
    ) {
        (true, false) => !crit,
        (false, true) => crit,
        _ => true,
    }
}

/// **The proc decision.** `None` means the Proc did not fire and nothing about it changes — its charge
/// and its cooldown are untouched, so a 50 percent Proc with 3 charges lands 3 times, not one and a
/// half.
///
/// In order: the event bit must be set for this side; a hit that carries a spell must pass the school
/// and family filter; `procEx` must accept the hit quality; the internal cooldown must be up; and only
/// then does the roll happen — so a Proc still on cooldown never consumes its roll. `roll_bp` is a
/// basis-point roll in `0..10_000`.
pub(crate) fn decide(
    profile: &ProcProfile,
    side: ProcSide,
    hit: &ProcHit,
    now_micros: i64,
    roll_bp: u32,
) -> Option<ProcFire> {
    if profile.flags & hit.bits(side) == 0 {
        return None;
    }
    if let Some(spell) = hit.spell.as_ref() {
        if !filter_matches(profile, spell) {
            return None;
        }
    }
    if !proc_ex_allows(profile.proc_ex, hit.crit) {
        return None;
    }
    if profile.ready_micros > now_micros {
        return None;
    }
    if roll_bp >= proc_chance_bp(profile, side, hit.attack_time_ms) {
        return None;
    }
    let limited = profile.charges > 0;
    let charges_left = if limited { profile.charges - 1 } else { 0 };
    Some(ProcFire {
        charges_left,
        remove_buff: limited && charges_left == 0,
        ready_micros: if profile.icd_ms > 0 {
            now_micros + (profile.icd_ms as i64) * 1000
        } else {
            profile.ready_micros
        },
    })
}

/// The profile a fresh aura row freezes at apply, from its spell header. `Spell.dbc` is the only
/// source the header has, and it carries no rate, no `procEx`, no filter and no internal cooldown —
/// those come from the classic-db `spell_proc_event` overlay. Until that overlay is loaded they freeze
/// at their neutral zero, which the decision reads as "no rate, any hit, any spell, no cooldown".
pub(crate) fn frozen_profile(kind: u8, hdr: &Spell) -> ProcProfile {
    if !is_proc_kind(kind) {
        return ProcProfile::default();
    }
    ProcProfile {
        flags: hdr.proc_flags,
        chance: hdr.proc_chance,
        charges: hdr.proc_charges,
        ..ProcProfile::default()
    }
}

// ===========================================================================================
//  The one pass [entity]
// ===========================================================================================

/// Run **the** proc pass for one landed hit. Called once per outcome branch of `combat::apply_hit`
/// (killed, duel-completed, survived) and from nowhere else.
///
/// The attacker's "dealt" Procs run first and run even on a killing blow; the target's "taken" Procs
/// run only when it survived. A Carrier whose Counterparty has left the world fires nothing at all:
/// with no Counterparty to aim at, a Triggered Cast would resolve its enemy effects back onto the
/// Carrier. Rows are snapshotted before any of them fires (collect-then-mutate), and
/// a row applied in this same reducer invocation is skipped — the same guard `break_auras_on_damage`
/// uses — so a Proc that grants another Proc cannot double up in one instant.
///
/// A fired Proc of the trigger kind starts a Triggered Cast of its frozen trigger spell from the
/// Carrier at the Counterparty; a Proc whose trigger spell is not loaded does nothing and logs, and
/// keeps its charge. Only a Proc that actually fired spends a charge or stamps its cooldown.
pub(crate) fn run_proc_pass(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    hit: &Hit,
    target_survived: bool,
) {
    let (dealer_bits, victim_bits) = hit_bits(hit);
    if dealer_bits == 0 && victim_bits == 0 {
        return; // a Triggered hit sets no event bits — it can never start a proc pass
    }
    // Snapshot every candidate row BEFORE firing any of them: a Triggered Cast writes to `game_aura`
    // (it can place, refresh or displace an aura), and a pass that read the table lazily would decide
    // later rows against a table its own earlier fire had already moved.
    let mut candidates: Vec<(Aura, ProcSide, u64)> = Vec::new();
    // A Counterparty that is not in the world at all (an environmental hit with no attacker, a caster
    // that logged out mid-flight) fires nothing: the Triggered Cast's own target resolution treats a
    // guid of 0 as "no explicit target" and would land the trigger back on the Carrier. A DEAD
    // Counterparty is still a real unit and still fires — the cast then lands only its self effects.
    if in_world(ctx, target_guid) {
        for row in proc_rows(ctx, attacker_guid, dealer_bits) {
            candidates.push((row, ProcSide::Dealer, target_guid));
        }
    }
    if target_survived && in_world(ctx, attacker_guid) {
        for row in proc_rows(ctx, target_guid, victim_bits) {
            candidates.push((row, ProcSide::Victim, attacker_guid));
        }
    }
    if candidates.is_empty() {
        return; // the common path: neither side carries a Proc that this event could fire
    }
    let proc_hit = ProcHit {
        dealer_bits,
        victim_bits,
        spell: triggering_spell(ctx, hit.spell_id),
        crit: hit.crit,
        // Procs-per-minute is a per-swing rate, and only the dealer has a swing: the Carrier's own
        // base attack time. One attack time per unit here, so an off-hand swing and a spell hit scale
        // by the same value — a documented approximation.
        attack_time_ms: ctx
            .db
            .game_world_entity()
            .guid()
            .find(attacker_guid)
            .map(|a| a.base_attack_time_ms)
            .unwrap_or(0),
    };
    let now_micros = ctx.timestamp.to_micros_since_unix_epoch();
    for (row, side, counterparty_guid) in candidates {
        let profile = ProcProfile::of(&row);
        let roll_bp = ctx.random::<u32>() % BASIS_POINTS;
        let Some(fire) = decide(&profile, side, &proc_hit, now_micros, roll_bp) else {
            continue; // no fire: charge, cooldown and buff are all untouched
        };
        if !fire_proc(ctx, &row, counterparty_guid) {
            continue; // a data gap fired nothing, so it spends nothing
        }
        if fire.remove_buff {
            remove_carrier_buff(ctx, row.target_guid, row.spell_id);
        } else if let Some(mut fresh) = ctx.db.game_aura().id().find(row.id) {
            // Re-read, because the Triggered Cast above can have written this very row. A REFRESH in
            // this same instant refilled the charges to full, and that refill wins over the count this
            // fire computed from the pre-fire snapshot; the cooldown stamp is written either way, since
            // a refresh deliberately leaves a running cooldown alone.
            if fresh.applied_at != ctx.timestamp {
                fresh.proc_charges = fire.charges_left;
            }
            fresh.proc_ready_micros = fire.ready_micros;
            ctx.db.game_aura().id().update(fresh);
        }
    }
}

/// Is `guid` a unit in the world? A guid of 0 (the "no attacker" sentinel every environmental and
/// non-melee damage path passes) is never one.
fn in_world(ctx: &ReducerContext, guid: u64) -> bool {
    guid != 0 && ctx.db.game_world_entity().guid().find(guid).is_some()
}

/// The Carrier's Proc rows this event could fire: a Proc kind, not applied in this same instant, whose
/// frozen mask names one of `bits`.
fn proc_rows(ctx: &ReducerContext, carrier_guid: u64, bits: u32) -> Vec<Aura> {
    if bits == 0 {
        return Vec::new();
    }
    auras_on(ctx, carrier_guid)
        .filter(|a| {
            is_proc_kind(a.eff_kind) && a.applied_at != ctx.timestamp && a.proc_flags & bits != 0
        })
        .collect()
}

/// The school and family of the spell that carried the hit. `None` for an auto-attack swing, and for
/// a spell whose header is not loaded: there are no facts to judge, so the filter is not consulted and
/// a data gap never silently stops a Proc that would otherwise fire.
fn triggering_spell(ctx: &ReducerContext, spell_id: u32) -> Option<TriggeringSpell> {
    if spell_id == 0 {
        return None;
    }
    ctx.db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .map(|s| TriggeringSpell {
            school_mask: s.school_mask,
            family_name: s.family_name,
            family_flags: s.family_flags,
        })
}

/// Fire one Proc row. Returns whether it actually did something — a Proc that fired nothing spends no
/// charge and stamps no cooldown.
fn fire_proc(ctx: &ReducerContext, row: &Aura, counterparty_guid: u64) -> bool {
    match row.eff_kind {
        A_PROC_TRIGGER => {
            let trigger_spell = row.eff_p1.max(0) as u32;
            if trigger_spell == 0 {
                log::info!(
                    "proc: spell {} on {} has no trigger spell — nothing fired",
                    row.spell_id,
                    row.target_guid
                );
                return false;
            }
            match crate::spell::cast_triggered(
                ctx,
                row.target_guid,
                trigger_spell,
                row.level,
                counterparty_guid,
            ) {
                Ok(()) => true,
                Err(err) => {
                    log::info!(
                        "proc: spell {} could not trigger {trigger_spell}: {err}",
                        row.spell_id
                    );
                    false
                }
            }
        }
        A_PROC_DAMAGE => {
            // The damage arm is not wired yet: the kind, its wire value and the decision that selects
            // it all exist, but nothing routes its frozen amount through the resistance and damage
            // pipeline. Until it does, the row is inert and keeps its charges.
            log::info!(
                "proc: spell {} is a damage Proc — the damage arm is not wired yet, nothing fired",
                row.spell_id
            );
            false
        }
        _ => false,
    }
}

/// The last charge is spent: EVERY aura row of `spell_id` comes off the Carrier, not just the Proc
/// effect — a proc buff is one buff in the client and its other effects must not linger as an orphan.
/// Routed through the shared [`crate::spell::shed_auras`] tail, so the projections an aura removal can
/// move are re-derived exactly as they are on any other involuntary removal.
fn remove_carrier_buff(ctx: &ReducerContext, carrier_guid: u64, spell_id: u32) {
    let spent: Vec<Aura> = auras_on(ctx, carrier_guid)
        .filter(|a| a.spell_id == spell_id)
        .collect();
    shed_auras(ctx, carrier_guid, spent);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swing(source: HitSource) -> Hit {
        Hit::weapon(source, false)
    }

    /// A profile that matches everything the tests do not specifically exercise: fires off a melee hit
    /// taken, always, unlimited charges, no cooldown, no filter.
    fn always() -> ProcProfile {
        ProcProfile {
            flags: PROC_FLAG_TAKEN_MELEE_HIT,
            chance: 100,
            ..ProcProfile::default()
        }
    }

    fn melee_hit() -> ProcHit {
        ProcHit {
            dealer_bits: PROC_FLAG_SUCCESSFUL_MELEE_HIT,
            victim_bits: PROC_FLAG_TAKEN_MELEE_HIT,
            spell: None,
            crit: false,
            attack_time_ms: 2000,
        }
    }

    // ---- hit_bits ---------------------------------------------------------------------------

    /// The event a main-hand swing raises on each side: the melee-hit bit for the swinger, the
    /// melee-hit-taken bit for whoever was hit. Values are vanilla's, read off cmangos `SpellMgr.h`.
    #[test]
    fn a_main_hand_swing_raises_the_melee_hit_event_on_both_sides() {
        assert_eq!(hit_bits(&swing(HitSource::MainHand)), (0x4, 0x8));
    }

    /// Vanilla sets BOTH dealer bits on an off-hand swing, so a dual-wield proc keyed on the shared
    /// melee bit and an off-hand-only proc keyed on its own bit both see it. The victim side is the
    /// same melee-hit-taken event either way — being hit is being hit.
    #[test]
    fn an_off_hand_swing_raises_both_dealer_bits_and_the_ordinary_taken_bit() {
        let (dealer, victim) = hit_bits(&swing(HitSource::OffHand));
        assert_eq!(dealer, 0x4 | 0x0080_0000);
        assert_eq!(victim, 0x8);
    }

    /// Ranged, melee-ability and spell hits each raise their own pair — a Frost Armor keyed on the
    /// melee pair must not see a Hunter's auto-shot or a Warlock's Shadow Bolt.
    #[test]
    fn each_remaining_source_raises_its_own_event_pair() {
        assert_eq!(hit_bits(&swing(HitSource::Ranged)), (0x40, 0x80));
        assert_eq!(
            hit_bits(&Hit::spell(HitSource::MeleeSpell, 1234, false)),
            (0x10, 0x20)
        );
        assert_eq!(
            hit_bits(&Hit::spell(HitSource::Spell, 1234, false)),
            (0x1_0000, 0x2_0000)
        );
    }

    /// A hit produced inside a Triggered Cast raises no event at all — this is what makes "a proc can
    /// never start a proc" structural rather than a per-family exception list.
    #[test]
    fn a_triggered_hit_raises_no_event() {
        assert_eq!(
            hit_bits(&Hit::spell(HitSource::Triggered, 6136, true)),
            (0, 0)
        );
    }

    // ---- proc_chance_bp ---------------------------------------------------------------------

    /// The worked vanilla example: 1 proc per minute off a 2.0 s weapon is one proc per 30 swings,
    /// i.e. 3.33 percent — 333 basis points.
    #[test]
    fn one_proc_per_minute_off_a_two_second_weapon_is_three_and_a_third_percent() {
        let profile = ProcProfile {
            ppm: 1.0,
            ..ProcProfile::default()
        };
        assert_eq!(proc_chance_bp(&profile, ProcSide::Dealer, 2000), 333);
    }

    /// A slow weapon procs more per swing and a fast one less, so the rate PER MINUTE holds: a 4.0 s
    /// weapon is twice a 2.0 s one, a 1.0 s weapon half of it.
    #[test]
    fn a_slower_weapon_procs_more_per_swing_at_the_same_rate_per_minute() {
        let profile = ProcProfile {
            ppm: 1.0,
            ..ProcProfile::default()
        };
        assert_eq!(proc_chance_bp(&profile, ProcSide::Dealer, 4000), 666);
        assert_eq!(proc_chance_bp(&profile, ProcSide::Dealer, 1000), 166);
    }

    /// A rate that would exceed certainty is capped there — 30 procs a minute off a 2.0 s weapon is
    /// 100 percent, and anything beyond it stays 100 percent.
    #[test]
    fn a_rate_above_certainty_is_capped_at_certainty() {
        let profile = ProcProfile {
            ppm: 30.0,
            ..ProcProfile::default()
        };
        assert_eq!(
            proc_chance_bp(&profile, ProcSide::Dealer, 2000),
            BASIS_POINTS
        );
        let faster = ProcProfile {
            ppm: 60.0,
            ..ProcProfile::default()
        };
        assert_eq!(
            proc_chance_bp(&faster, ProcSide::Dealer, 2000),
            BASIS_POINTS
        );
    }

    /// A Carrier on the receiving end uses its flat chance: a taken-side proc must not depend on the
    /// weapon its attacker happens to be swinging.
    #[test]
    fn the_victim_side_ignores_the_rate_and_uses_the_flat_chance() {
        let profile = ProcProfile {
            ppm: 1.0,
            chance: 50,
            ..ProcProfile::default()
        };
        assert_eq!(proc_chance_bp(&profile, ProcSide::Victim, 2000), 5_000);
        assert_eq!(proc_chance_bp(&profile, ProcSide::Dealer, 2000), 333);
    }

    /// A flat chance at or above 100 percent fills the roll space, so every roll in `0..10_000` is
    /// under it — Frost Armor's 100 always chills.
    #[test]
    fn a_flat_chance_at_or_above_certainty_always_passes() {
        for chance in [100u8, 101, 255] {
            let profile = ProcProfile {
                chance,
                ..ProcProfile::default()
            };
            assert_eq!(
                proc_chance_bp(&profile, ProcSide::Victim, 0),
                BASIS_POINTS,
                "chance {chance}"
            );
        }
    }

    // ---- decide -----------------------------------------------------------------------------

    /// The event mask decides first: a Proc keyed on "melee hit taken" fires for the Carrier that took
    /// the swing, and not for the Carrier that dealt it.
    #[test]
    fn a_proc_fires_only_on_the_side_its_mask_names() {
        let profile = always();
        assert!(decide(&profile, ProcSide::Victim, &melee_hit(), 0, 0).is_some());
        assert!(decide(&profile, ProcSide::Dealer, &melee_hit(), 0, 0).is_none());
    }

    /// The internal cooldown is checked BEFORE the roll, so a Proc inside its window cannot fire even
    /// on a roll that would otherwise have passed.
    #[test]
    fn the_internal_cooldown_blocks_a_fire_before_the_roll() {
        let profile = ProcProfile {
            icd_ms: 5_000,
            ready_micros: 10_000_000,
            ..always()
        };
        assert!(decide(&profile, ProcSide::Victim, &melee_hit(), 9_999_999, 0).is_none());
        // At the deadline it is ready again, and the fire stamps the next window.
        let fire = decide(&profile, ProcSide::Victim, &melee_hit(), 10_000_000, 0)
            .expect("the cooldown is up at its deadline");
        assert_eq!(fire.ready_micros, 10_000_000 + 5_000 * 1000);
    }

    /// A Proc with no cooldown leaves its ready time where it was — nothing to stamp.
    #[test]
    fn a_proc_without_a_cooldown_stamps_no_deadline() {
        let fire = decide(&always(), ProcSide::Victim, &melee_hit(), 42, 0).expect("fires");
        assert_eq!(fire.ready_micros, 0);
    }

    /// A roll at or above the chance fails: a 50 percent Proc passes on 4_999 and fails on 5_000.
    #[test]
    fn the_roll_passes_strictly_below_the_chance() {
        let profile = ProcProfile {
            chance: 50,
            ..always()
        };
        assert!(decide(&profile, ProcSide::Victim, &melee_hit(), 0, 4_999).is_some());
        assert!(decide(&profile, ProcSide::Victim, &melee_hit(), 0, 5_000).is_none());
    }

    /// A charged Proc spends a charge per FIRE, never per hit: three fires take it 3 → 2 → 1 → 0, and
    /// only the last one signals that the buff comes off.
    #[test]
    fn three_charges_are_spent_one_fire_at_a_time_and_the_last_removes_the_buff() {
        let mut charges = 3u8;
        for expected in [2u8, 1, 0] {
            let profile = ProcProfile {
                charges,
                ..always()
            };
            let fire = decide(&profile, ProcSide::Victim, &melee_hit(), 0, 0).expect("fires");
            assert_eq!(fire.charges_left, expected);
            assert_eq!(fire.remove_buff, expected == 0);
            charges = fire.charges_left;
        }
    }

    /// A roll that failed changes nothing — the charge is still there for the next hit, which is why a
    /// 50 percent Proc with 3 charges lands 3 times rather than one and a half.
    #[test]
    fn a_failed_roll_leaves_the_charge_alone() {
        let profile = ProcProfile {
            chance: 50,
            charges: 3,
            ..always()
        };
        assert!(decide(&profile, ProcSide::Victim, &melee_hit(), 0, 9_999).is_none());
    }

    /// An unlimited Proc (0 charges) never reports the buff should come off, however often it fires.
    #[test]
    fn an_unlimited_proc_never_removes_its_buff() {
        let fire = decide(&always(), ProcSide::Victim, &melee_hit(), 0, 0).expect("fires");
        assert_eq!(fire.charges_left, 0);
        assert!(!fire.remove_buff);
    }

    /// `procEx`: the crit bit alone fires only on a crit, the normal bit alone only on a non-crit, and
    /// neither or both accepts any landed hit.
    #[test]
    fn proc_ex_selects_the_hit_quality() {
        let crit_hit = ProcHit {
            crit: true,
            ..melee_hit()
        };
        let crit_only = ProcProfile {
            proc_ex: PROC_EX_CRITICAL_HIT,
            ..always()
        };
        assert!(decide(&crit_only, ProcSide::Victim, &crit_hit, 0, 0).is_some());
        assert!(decide(&crit_only, ProcSide::Victim, &melee_hit(), 0, 0).is_none());

        let normal_only = ProcProfile {
            proc_ex: PROC_EX_NORMAL_HIT,
            ..always()
        };
        assert!(decide(&normal_only, ProcSide::Victim, &melee_hit(), 0, 0).is_some());
        assert!(decide(&normal_only, ProcSide::Victim, &crit_hit, 0, 0).is_none());

        for proc_ex in [0, PROC_EX_NORMAL_HIT | PROC_EX_CRITICAL_HIT] {
            let any = ProcProfile {
                proc_ex,
                ..always()
            };
            assert!(decide(&any, ProcSide::Victim, &melee_hit(), 0, 0).is_some());
            assert!(decide(&any, ProcSide::Victim, &crit_hit, 0, 0).is_some());
        }
    }

    /// The school filter applies to a hit that carried a spell, and only to such a hit: an auto-attack
    /// swing belongs to no school and is never excluded by one.
    #[test]
    fn the_school_filter_judges_a_spell_hit_and_never_a_swing() {
        let fire_only = ProcProfile {
            flags: PROC_FLAG_TAKEN_MELEE_SPELL_HIT,
            school_mask: 0x4, // fire
            ..always()
        };
        let spell_hit = |school_mask| ProcHit {
            victim_bits: PROC_FLAG_TAKEN_MELEE_SPELL_HIT,
            spell: Some(TriggeringSpell {
                school_mask,
                family_name: 0,
                family_flags: 0,
            }),
            ..melee_hit()
        };
        assert!(decide(&fire_only, ProcSide::Victim, &spell_hit(0x4), 0, 0).is_some());
        assert!(decide(&fire_only, ProcSide::Victim, &spell_hit(0x10), 0, 0).is_none());

        // The same filter, on a swing that carries no spell at all: not consulted.
        let swing_hit = ProcHit {
            victim_bits: PROC_FLAG_TAKEN_MELEE_SPELL_HIT,
            spell: None,
            ..melee_hit()
        };
        assert!(decide(&fire_only, ProcSide::Victim, &swing_hit, 0, 0).is_some());
    }

    /// The family filter: the name must agree, and a non-zero flag mask must overlap. A named family
    /// with no flags accepts every spell of that family.
    #[test]
    fn the_family_filter_matches_the_name_then_the_flags() {
        let mage_frost = ProcProfile {
            family_name: 3,
            family_flags: 0x20,
            ..always()
        };
        let spell = |family_name, family_flags| ProcHit {
            spell: Some(TriggeringSpell {
                school_mask: 0,
                family_name,
                family_flags,
            }),
            ..melee_hit()
        };
        assert!(decide(&mage_frost, ProcSide::Victim, &spell(3, 0x20), 0, 0).is_some());
        assert!(decide(&mage_frost, ProcSide::Victim, &spell(3, 0x40), 0, 0).is_none());
        assert!(decide(&mage_frost, ProcSide::Victim, &spell(4, 0x20), 0, 0).is_none());

        let any_mage = ProcProfile {
            family_name: 3,
            family_flags: 0,
            ..always()
        };
        assert!(decide(&any_mage, ProcSide::Victim, &spell(3, 0x40), 0, 0).is_some());
    }

    /// The frozen profile a fresh aura row takes from its header — and the zeros every non-proc row
    /// takes, so an ordinary buff can never be mistaken for a Proc.
    #[test]
    fn a_proc_row_freezes_its_header_and_every_other_row_freezes_zeros() {
        let mut hdr = crate::seed::base_spell(168, "Frost Armor");
        hdr.proc_flags = 0x28;
        hdr.proc_chance = 100;
        hdr.proc_charges = 3;

        let frozen = frozen_profile(A_PROC_TRIGGER, &hdr);
        assert_eq!(frozen.flags, 0x28);
        assert_eq!(frozen.chance, 100);
        assert_eq!(frozen.charges, 3);
        // The `spell_proc_event` overlay fills these; the Spell.dbc header carries no source for them.
        assert_eq!(frozen.ppm, 0.0);
        assert_eq!(frozen.icd_ms, 0);
        assert_eq!(frozen.proc_ex, 0);

        assert_eq!(frozen_profile(A_MOD_RESISTANCE, &hdr).flags, 0);
        assert_eq!(frozen_profile(A_MOD_RESISTANCE, &hdr).charges, 0);
    }

    // ---- Architecture Tests -----------------------------------------------------------------

    /// ONE proc pass, called from ONE place. `apply_hit` is the chokepoint every damaging hit already
    /// routes through, so a new damage path gets procs by construction — a second call site anywhere
    /// else is how that guarantee is lost.
    #[test]
    fn the_proc_pass_is_called_only_from_apply_hit() {
        let mut offenders: Vec<String> = Vec::new();
        let mut in_death = 0usize;
        for rel in crate::test_scan::module_sources() {
            // `proc.rs` defines the pass (and its own tests name it in prose); `combat/death.rs` is
            // the one caller, whose calls are placed exactly below.
            if rel.ends_with("spell/proc.rs") {
                continue;
            }
            let src = crate::test_scan::read_scanned(&rel).expect("module/ is never optional");
            for (idx, _) in src.match_indices("run_proc_pass(") {
                if crate::test_scan::on_comment_line(&src, idx)
                    || crate::test_scan::in_string_literal(&src, idx)
                {
                    continue;
                }
                if rel.ends_with("combat/death.rs") {
                    in_death += 1;
                } else {
                    offenders.push(format!("{rel}:{}", crate::test_scan::line_of(&src, idx)));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "`run_proc_pass` is called outside `combat::apply_hit` ({offenders:?}). The pass exists \
             once, at the one chokepoint every damaging hit routes through, so a new damage path \
             cannot forget procs."
        );
        // Every call sits inside `apply_hit` itself — one per outcome branch, because each branch
        // decides for itself whether the target survived: the duel finisher and the ordinary survivor
        // fire both sides, the killing blow fires the attacker's Procs only.
        let death = crate::test_scan::read_scanned("module/src/combat/death.rs")
            .expect("module/ is never optional");
        let body = crate::test_scan::code_of(&death, "pub(crate) fn apply_hit(");
        assert_eq!(
            body.matches("run_proc_pass(").count(),
            3,
            "`apply_hit` must run the proc pass once per outcome branch — duel-completed, killed and \
             survived. A branch that writes damage and skips the pass is a damage path that forgot \
             procs. Body was:\n{body}"
        );
        assert_eq!(
            in_death, 3,
            "combat/death.rs calls `run_proc_pass` somewhere other than `apply_hit`"
        );
    }

    /// A Triggered Cast pays nothing and passes nothing: no Gate sweep, no cost charge, no global or
    /// per-spell cooldown, no stealth break, no dismount. Losing any of those exclusions turns a proc
    /// into something that can block or tax the Carrier's own casts.
    #[test]
    fn a_triggered_cast_pays_no_cost_and_passes_no_gates() {
        let src = crate::test_scan::read_scanned("module/src/spell/cast/resolve.rs")
            .expect("module/ is never optional");
        let body = crate::test_scan::code_of(&src, "pub(crate) fn cast_triggered(");
        for forbidden in [
            "check_cast_gates",
            "game_spell_cooldown",
            "game_spell_cd",
            "break_stealth",
            "dismount",
            "remove_items",
            "fire_on_cast_resolved",
        ] {
            assert!(
                !body.contains(forbidden),
                "`cast_triggered` reached `{forbidden}` — a Triggered Cast must cost nothing, pass \
                 no Gate and start no cooldown. Body was:\n{body}"
            );
        }
    }

    /// The debug damage reducer is the fast way to drive this pass: waiting on real swing timers turns
    /// a 100-hit run into minutes. It must therefore route through the SHARED pipeline as a main-hand
    /// hit, not write health by hand — a raw field write raises no combat event and fires no Proc.
    #[test]
    fn the_debug_damage_reducer_drives_the_pass_as_a_main_hand_hit() {
        let body = crate::test_scan::code_of(
            &crate::test_scan::debug_dir_src(),
            "pub fn debug_apply_damage(",
        );
        for needle in [
            "crate::combat::fold_incoming_damage(",
            "crate::combat::apply_hit(",
            "HitSource::MainHand",
        ] {
            assert!(
                body.contains(needle),
                "`debug_apply_damage` no longer reaches `{needle}` — it must drive the same \
                 fold-then-apply pipeline a real swing does, or it stops driving the proc pass. Body \
                 was:\n{body}"
            );
        }
        assert!(
            !body.contains("entities.guid().update("),
            "`debug_apply_damage` writes health by hand again instead of routing through the shared \
             pipeline. Body was:\n{body}"
        );
    }

    /// The trainer's wrapper-resolution exclusion list names the renamed Proc kind. It and the
    /// importer's `wrapper_to_rank` heuristic are the same rule on the two sides of the wire: a
    /// trigger spell on a Proc aura is an effect PAYLOAD, not a rank to learn.
    #[test]
    fn the_learn_target_resolution_excludes_the_proc_trigger_kind() {
        let src = crate::test_scan::read_scanned("module/src/trainer.rs")
            .expect("module/ is never optional");
        let body = crate::test_scan::code_of(&src, "pub(crate) fn resolve_learn_target(");
        assert!(
            body.contains("A_PROC_TRIGGER"),
            "`resolve_learn_target` no longer excludes the Proc trigger kind — buying Frost Armor \
             rank 2 would teach `Chilled` instead. Body was:\n{body}"
        );
    }
}
