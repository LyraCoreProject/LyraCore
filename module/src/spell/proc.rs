use spacetimedb::{log, ReducerContext};

use crate::game_world_entity;

use crate::combat::{Hit, HitSource};

use super::*;

// Bit values match procFlags and procEx in cmangos SpellMgr.h.

// Vanilla off-hand hits set this shared melee bit as well as the off-hand bit.
pub(crate) const PROC_FLAG_SUCCESSFUL_MELEE_HIT: u32 = 0x0000_0004;
pub(crate) const PROC_FLAG_TAKEN_MELEE_HIT: u32 = 0x0000_0008;
pub(crate) const PROC_FLAG_SUCCESSFUL_MELEE_SPELL_HIT: u32 = 0x0000_0010;
pub(crate) const PROC_FLAG_TAKEN_MELEE_SPELL_HIT: u32 = 0x0000_0020;
pub(crate) const PROC_FLAG_SUCCESSFUL_RANGED_HIT: u32 = 0x0000_0040;
pub(crate) const PROC_FLAG_TAKEN_RANGED_HIT: u32 = 0x0000_0080;
pub(crate) const PROC_FLAG_SUCCESSFUL_NEGATIVE_SPELL_HIT: u32 = 0x0001_0000;
pub(crate) const PROC_FLAG_TAKEN_NEGATIVE_SPELL_HIT: u32 = 0x0002_0000;
pub(crate) const PROC_FLAG_SUCCESSFUL_OFFHAND_HIT: u32 = 0x0080_0000;

pub(crate) const PROC_EX_NORMAL_HIT: u32 = 0x0000_0001;
pub(crate) const PROC_EX_CRITICAL_HIT: u32 = 0x0000_0002;

const SCHOOL_MASK_PHYSICAL: u8 = 0x01;

pub(crate) const BASIS_POINTS: u32 = 10_000;

// Vanilla chance_pct = ppm * attack_time_ms / 600.
const PPM_DIVISOR_PCT: f32 = 600.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ProcSide {
    Dealer,
    Victim,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProcProfile {
    pub flags: u32,
    // Percent.
    pub chance: u8,
    pub ppm: f32,
    pub proc_ex: u32,
    pub school_mask: u8,
    pub family_name: u8,
    pub family_flags: u64,
    // Zero means unlimited charges.
    pub charges: u8,
    pub icd_ms: u32,
    pub ready_micros: i64,
}

impl ProcProfile {
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct TriggeringSpell {
    pub school_mask: u8,
    pub family_name: u8,
    pub family_flags: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProcHit {
    pub dealer_bits: u32,
    pub victim_bits: u32,
    pub spell: Option<TriggeringSpell>,
    pub crit: bool,
    pub attack_time_ms: u32,
}

impl ProcHit {
    fn bits(&self, side: ProcSide) -> u32 {
        match side {
            ProcSide::Dealer => self.dealer_bits,
            ProcSide::Victim => self.victim_bits,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ProcFire {
    pub charges_left: u8,
    pub ready_micros: i64,
    pub remove_buff: bool,
}

pub(crate) fn is_proc_kind(kind: u8) -> bool {
    kind == A_PROC_TRIGGER || kind == A_PROC_DAMAGE
}

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

pub(crate) fn proc_chance_bp(profile: &ProcProfile, side: ProcSide, attack_time_ms: u32) -> u32 {
    if profile.ppm > 0.0 && side == ProcSide::Dealer {
        let pct = profile.ppm * attack_time_ms as f32 / PPM_DIVISOR_PCT;
        return ((pct * 100.0) as u32).min(BASIS_POINTS);
    }
    (profile.chance as u32 * 100).min(BASIS_POINTS)
}

fn filter_matches(profile: &ProcProfile, spell: Option<&TriggeringSpell>) -> bool {
    let Some(spell) = spell else {
        return profile.school_mask == 0 || profile.school_mask & SCHOOL_MASK_PHYSICAL != 0;
    };
    if profile.school_mask != 0 && profile.school_mask & spell.school_mask == 0 {
        return false;
    }
    if profile.family_name != 0 {
        if profile.family_name != spell.family_name {
            return false;
        }
        if profile.family_flags != 0 && profile.family_flags & spell.family_flags == 0 {
            return false;
        }
    }
    true
}

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
    if !filter_matches(profile, hit.spell.as_ref()) {
        return None;
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

pub(crate) fn frozen_profile(
    kind: u8,
    hdr: &Spell,
    overlay: Option<&SpellProcEvent>,
) -> ProcProfile {
    if !is_proc_kind(kind) {
        return ProcProfile::default();
    }
    let base = ProcProfile {
        flags: hdr.proc_flags,
        chance: hdr.proc_chance,
        charges: hdr.proc_charges,
        ..ProcProfile::default()
    };
    let Some(o) = overlay else {
        return base;
    };
    ProcProfile {
        flags: if o.proc_flags != 0 {
            o.proc_flags
        } else {
            base.flags
        },
        chance: if o.custom_chance != 0 {
            o.custom_chance
        } else {
            base.chance
        },
        ppm: o.ppm_rate,
        proc_ex: o.proc_ex,
        school_mask: o.school_mask,
        family_name: o.family_name,
        family_flags: o.family_flags,
        icd_ms: o.icd_ms,
        ..base
    }
}

pub(crate) fn freeze_profile(ctx: &ReducerContext, kind: u8, hdr: &Spell) -> ProcProfile {
    if !is_proc_kind(kind) {
        return ProcProfile::default();
    }
    let overlay = ctx.db.game_spell_proc_event().spell_id().find(hdr.spell_id);
    frozen_profile(kind, hdr, overlay.as_ref())
}

pub(crate) fn run_proc_pass(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    hit: &Hit,
    target_survived: bool,
) {
    let (dealer_bits, victim_bits) = hit_bits(hit);
    if dealer_bits == 0 && victim_bits == 0 {
        return;
    }
    fire_aura_procs(
        ctx,
        attacker_guid,
        target_guid,
        hit,
        target_survived,
        (dealer_bits, victim_bits),
    );
}

fn fire_aura_procs(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    hit: &Hit,
    target_survived: bool,
    (dealer_bits, victim_bits): (u32, u32),
) {
    // Triggered Casts can change aura rows, so snapshot all candidates before firing any.
    let mut candidates: Vec<(Aura, ProcSide, u64)> = Vec::new();
    // Guid zero targets the Carrier, so a missing Counterparty must not trigger a cast.
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
        return;
    }
    let proc_hit = ProcHit {
        dealer_bits,
        victim_bits,
        spell: triggering_spell(ctx, hit.spell_id),
        crit: hit.crit,
        // One base attack time per unit approximates spell hits and off-hand swing rates.
        attack_time_ms: ctx
            .db
            .game_world_entity()
            .guid()
            .find(attacker_guid)
            .map(|a| a.base_attack_time_ms)
            .unwrap_or(0),
    };
    let now_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    for (row, side, counterparty_guid) in candidates {
        if refuses_extra_attacks(ctx, &row) {
            continue;
        }
        let profile = ProcProfile::of(&row);
        let roll_bp = ctx.random::<u32>() % BASIS_POINTS;
        let Some(fire) = decide(&profile, side, &proc_hit, now_micros, roll_bp) else {
            continue;
        };
        if !fire_proc(ctx, &row, counterparty_guid) {
            continue;
        }
        if fire.remove_buff {
            remove_carrier_buff(ctx, row.target_guid, row.spell_id);
        } else if let Some(mut fresh) = ctx.db.game_aura().id().find(row.id) {
            // A refresh in this invocation refills charges and wins over the pre-refresh count.
            if fresh.applied_at != crate::time::now(ctx) {
                fresh.proc_charges = fire.charges_left;
            }
            fresh.proc_ready_micros = fire.ready_micros;
            fresh.update(ctx);
        }
    }
}

// vmangos fails a Proc that would grant Extra Attacks while the Carrier still holds some, so its
// cooldown and charges stay untouched.
fn refuses_extra_attacks(ctx: &ReducerContext, row: &Aura) -> bool {
    row.eff_kind == A_PROC_TRIGGER
        && super::chance_on_hit::grants_extra_attacks(ctx, row.eff_p1.max(0) as u32)
        && crate::combat::has_extra_attacks(ctx, row.target_guid)
}

fn in_world(ctx: &ReducerContext, guid: u64) -> bool {
    guid != 0 && ctx.db.game_world_entity().guid().find(guid).is_some()
}

// Skip new auras so a proc cannot fire another proc in the same invocation.
fn proc_rows(ctx: &ReducerContext, carrier_guid: u64, bits: u32) -> Vec<Aura> {
    if bits == 0 {
        return Vec::new();
    }
    auras_on(ctx, carrier_guid)
        .filter(|a| {
            is_proc_kind(a.eff_kind)
                && a.applied_at != crate::time::now(ctx)
                && a.proc_flags & bits != 0
        })
        .collect()
}

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
        A_PROC_DAMAGE => fire_proc_damage(ctx, row, counterparty_guid),
        _ => false,
    }
}

fn fire_proc_damage(ctx: &ReducerContext, row: &Aura, counterparty_guid: u64) -> bool {
    if row.amount <= 0 {
        log::info!(
            "proc: spell {} on {} is a damage Proc with no amount — nothing fired",
            row.spell_id,
            row.target_guid
        );
        return false;
    }
    let school_mask = row.eff_p0.clamp(0, u8::MAX as i32) as u8;
    let after_resist = apply_resistance(
        ctx,
        counterparty_guid,
        row.target_guid,
        school_mask,
        row.amount,
    );
    let (dealt, absorbed) = apply_target_damage(
        ctx,
        counterparty_guid,
        row.target_guid,
        after_resist,
        Hit::spell(HitSource::Triggered, row.spell_id, false),
    );
    if dealt > 0 || absorbed > 0 {
        crate::spell::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                target_guid: counterparty_guid,
                damage: dealt,
                school: school_index(school_mask),
                resisted: (row.amount as u32).saturating_sub(after_resist.max(0) as u32),
                absorbed,
                is_proc_log: true,
                ..SpellCastEvent::signal(
                    ctx,
                    row.target_guid,
                    row.spell_id,
                    SpellCastEventKind::ProcLog,
                )
            },
        );
    }
    true
}

// The client represents every effect row of a spell as one buff.
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

    #[test]
    fn a_main_hand_swing_raises_the_melee_hit_event_on_both_sides() {
        assert_eq!(hit_bits(&swing(HitSource::MainHand)), (0x4, 0x8));
    }

    #[test]
    fn an_off_hand_swing_raises_both_dealer_bits_and_the_ordinary_taken_bit() {
        let (dealer, victim) = hit_bits(&swing(HitSource::OffHand));
        assert_eq!(dealer, 0x4 | 0x0080_0000);
        assert_eq!(victim, 0x8);
    }

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

    #[test]
    fn a_triggered_hit_raises_no_event() {
        assert_eq!(
            hit_bits(&Hit::spell(HitSource::Triggered, 6136, true)),
            (0, 0)
        );
    }

    #[test]
    fn one_proc_per_minute_off_a_two_second_weapon_is_three_and_a_third_percent() {
        let profile = ProcProfile {
            ppm: 1.0,
            ..ProcProfile::default()
        };
        assert_eq!(proc_chance_bp(&profile, ProcSide::Dealer, 2000), 333);
    }

    #[test]
    fn a_slower_weapon_procs_more_per_swing_at_the_same_rate_per_minute() {
        let profile = ProcProfile {
            ppm: 1.0,
            ..ProcProfile::default()
        };
        assert_eq!(proc_chance_bp(&profile, ProcSide::Dealer, 4000), 666);
        assert_eq!(proc_chance_bp(&profile, ProcSide::Dealer, 1000), 166);
    }

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

    #[test]
    fn a_proc_fires_only_on_the_side_its_mask_names() {
        let profile = always();
        assert!(decide(&profile, ProcSide::Victim, &melee_hit(), 0, 0).is_some());
        assert!(decide(&profile, ProcSide::Dealer, &melee_hit(), 0, 0).is_none());
    }

    #[test]
    fn the_internal_cooldown_blocks_a_fire_before_the_roll() {
        let profile = ProcProfile {
            icd_ms: 5_000,
            ready_micros: 10_000_000,
            ..always()
        };
        assert!(decide(&profile, ProcSide::Victim, &melee_hit(), 9_999_999, 0).is_none());
        let fire = decide(&profile, ProcSide::Victim, &melee_hit(), 10_000_000, 0)
            .expect("the cooldown is up at its deadline");
        assert_eq!(fire.ready_micros, 10_000_000 + 5_000 * 1000);
    }

    #[test]
    fn a_proc_without_a_cooldown_stamps_no_deadline() {
        let fire = decide(&always(), ProcSide::Victim, &melee_hit(), 42, 0).expect("fires");
        assert_eq!(fire.ready_micros, 0);
    }

    #[test]
    fn the_roll_passes_strictly_below_the_chance() {
        let profile = ProcProfile {
            chance: 50,
            ..always()
        };
        assert!(decide(&profile, ProcSide::Victim, &melee_hit(), 0, 4_999).is_some());
        assert!(decide(&profile, ProcSide::Victim, &melee_hit(), 0, 5_000).is_none());
    }

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

    #[test]
    fn a_failed_roll_leaves_the_charge_alone() {
        let profile = ProcProfile {
            chance: 50,
            charges: 3,
            ..always()
        };
        assert!(decide(&profile, ProcSide::Victim, &melee_hit(), 0, 9_999).is_none());
    }

    #[test]
    fn an_unlimited_proc_never_removes_its_buff() {
        let fire = decide(&always(), ProcSide::Victim, &melee_hit(), 0, 0).expect("fires");
        assert_eq!(fire.charges_left, 0);
        assert!(!fire.remove_buff);
    }

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

    #[test]
    fn the_school_filter_admits_only_a_spell_of_the_named_school() {
        let fire_only = ProcProfile {
            flags: PROC_FLAG_TAKEN_MELEE_SPELL_HIT,
            school_mask: 0x4,
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
    }

    #[test]
    fn a_swing_is_judged_as_physical_by_a_school_filter_and_not_at_all_by_a_family_one() {
        let swing = melee_hit();
        assert!(swing.spell.is_none(), "a swing carries no spell");

        let fire_only = ProcProfile {
            school_mask: 0x4,
            ..always()
        };
        assert!(decide(&fire_only, ProcSide::Victim, &swing, 0, 0).is_none());

        let physical_only = ProcProfile {
            school_mask: SCHOOL_MASK_PHYSICAL,
            ..always()
        };
        assert!(decide(&physical_only, ProcSide::Victim, &swing, 0, 0).is_some());

        let mage_frost = ProcProfile {
            family_name: 3,
            family_flags: 0x20,
            ..always()
        };
        assert!(decide(&mage_frost, ProcSide::Victim, &swing, 0, 0).is_some());
    }

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

    fn proc_header() -> Spell {
        let mut hdr = crate::seed::base_spell(168, "Frost Armor");
        hdr.proc_flags = 0x28;
        hdr.proc_chance = 100;
        hdr.proc_charges = 3;
        hdr
    }

    fn overlay() -> SpellProcEvent {
        SpellProcEvent {
            spell_id: 168,
            proc_flags: 0,
            proc_ex: 0,
            school_mask: 0,
            family_name: 0,
            family_flags: 0,
            ppm_rate: 0.0,
            custom_chance: 0,
            icd_ms: 0,
        }
    }

    #[test]
    fn a_proc_row_freezes_its_header_and_every_other_row_freezes_zeros() {
        let hdr = proc_header();
        let frozen = frozen_profile(A_PROC_TRIGGER, &hdr, None);
        assert_eq!(frozen.flags, 0x28);
        assert_eq!(frozen.chance, 100);
        assert_eq!(frozen.charges, 3);
        assert_eq!(frozen.ppm, 0.0);
        assert_eq!(frozen.icd_ms, 0);
        assert_eq!(frozen.proc_ex, 0);
        assert_eq!(frozen.school_mask, 0);
        assert_eq!(frozen.family_name, 0);

        assert_eq!(frozen_profile(A_MOD_RESISTANCE, &hdr, None).flags, 0);
        assert_eq!(frozen_profile(A_MOD_RESISTANCE, &hdr, None).charges, 0);
    }

    #[test]
    fn the_overlay_supplies_the_rate_cooldown_hit_rule_and_filter() {
        let frozen = frozen_profile(
            A_PROC_TRIGGER,
            &proc_header(),
            Some(&SpellProcEvent {
                proc_ex: PROC_EX_CRITICAL_HIT,
                school_mask: 0x4,
                family_name: 3,
                family_flags: 0x20,
                ppm_rate: 2.0,
                icd_ms: 45_000,
                ..overlay()
            }),
        );
        assert_eq!(frozen.proc_ex, PROC_EX_CRITICAL_HIT);
        assert_eq!(frozen.school_mask, 0x4);
        assert_eq!(frozen.family_name, 3);
        assert_eq!(frozen.family_flags, 0x20);
        assert_eq!(frozen.ppm, 2.0);
        assert_eq!(frozen.icd_ms, 45_000);
        assert_eq!(frozen.charges, 3, "charges only ever come from the header");
    }

    #[test]
    fn a_non_zero_flag_override_replaces_the_header_mask_and_zero_keeps_it() {
        let overridden = frozen_profile(
            A_PROC_TRIGGER,
            &proc_header(),
            Some(&SpellProcEvent {
                proc_flags: PROC_FLAG_SUCCESSFUL_MELEE_HIT,
                ..overlay()
            }),
        );
        assert_eq!(overridden.flags, PROC_FLAG_SUCCESSFUL_MELEE_HIT);

        let kept = frozen_profile(A_PROC_TRIGGER, &proc_header(), Some(&overlay()));
        assert_eq!(kept.flags, 0x28);
    }

    #[test]
    fn a_custom_chance_replaces_the_header_chance_and_zero_keeps_it() {
        let custom = frozen_profile(
            A_PROC_TRIGGER,
            &proc_header(),
            Some(&SpellProcEvent {
                custom_chance: 15,
                ..overlay()
            }),
        );
        assert_eq!(custom.chance, 15);

        let kept = frozen_profile(A_PROC_TRIGGER, &proc_header(), Some(&overlay()));
        assert_eq!(kept.chance, 100);
    }

    #[test]
    fn the_rate_beats_the_custom_chance_on_the_dealer_side_only() {
        let frozen = frozen_profile(
            A_PROC_TRIGGER,
            &proc_header(),
            Some(&SpellProcEvent {
                custom_chance: 15,
                ppm_rate: 1.0,
                ..overlay()
            }),
        );
        assert_eq!(proc_chance_bp(&frozen, ProcSide::Dealer, 2000), 333);
        assert_eq!(proc_chance_bp(&frozen, ProcSide::Victim, 2000), 1_500);
    }
}
