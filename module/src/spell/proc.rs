use spacetimedb::{log, ReducerContext};

use crate::game_world_entity;

use crate::combat::{AttackOutcome, Hit, HitSource};
use lyracore_shared::spell::taxonomy::{
    A_FLAG, A_IMMUNITY, A_INERT, A_MOD_DAMAGE_DONE_FLAT, A_MOD_RESISTANCE, A_MOD_SPEED,
    A_PROC_DAMAGE, A_PROC_TRIGGER, A_SPELLMOD_FLAT, A_SPELLMOD_PCT, P_MECHANIC, SPEED_CAST,
    SPEED_SWING,
};

// vmangos SpellDefines.h SpellModOp SPELLMOD_CHARGES.
const SPELLMOD_OP_CHARGES: i32 = 4;

use super::control::is_mechanic_immunity_mask;
use super::*;
use crate::{
    game_aura, game_spell, game_spell_effect, game_spell_proc_event, Aura, Spell, SpellCastEvent,
    SpellProcEvent,
};

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
pub(crate) const PROC_EX_MISS: u32 = 0x0000_0004;
pub(crate) const PROC_EX_DODGE: u32 = 0x0000_0010;
pub(crate) const PROC_EX_PARRY: u32 = 0x0000_0020;
pub(crate) const PROC_EX_BLOCK: u32 = 0x0000_0040;
pub(crate) const PROC_EX_IMMUNE: u32 = 0x0000_0100;
pub(crate) const PROC_EX_ABSORB: u32 = 0x0000_0400;
// procEx bits below this name hit results; the bits above it are conditions.
const PROC_EX_RESULTS: u32 = 0x0000_0FFF;
pub(crate) const PROC_EX_TRIGGER_ALWAYS: u32 = 0x0001_0000;

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
    pub mechanic: u8,
    pub cast_time_ms: u32,
    pub cost: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProcHit {
    pub dealer_bits: u32,
    pub victim_bits: u32,
    pub spell: Option<TriggeringSpell>,
    /// The PROC_EX_* result bits of the hit.
    pub result: u32,
    /// Whether any damage landed; a fully absorbed hit still raises PROC_EX_NORMAL_HIT.
    pub dealt_damage: bool,
    pub attack_time_ms: u32,
}

impl ProcHit {
    fn bits(&self, side: ProcSide) -> u32 {
        match side {
            ProcSide::Dealer => self.dealer_bits,
            ProcSide::Victim => self.victim_bits,
        }
    }

    fn crit(&self) -> bool {
        self.result & PROC_EX_CRITICAL_HIT != 0
    }
}

fn landed_result(hit: &Hit, absorbed: u32) -> u32 {
    let quality = if hit.crit {
        PROC_EX_CRITICAL_HIT
    } else {
        PROC_EX_NORMAL_HIT
    };
    let block = if hit.blocked { PROC_EX_BLOCK } else { 0 };
    let absorb = if absorbed > 0 { PROC_EX_ABSORB } else { 0 };
    quality | block | absorb
}

// vmangos Unit::CalculateMeleeDamage raises PROC_EX_BLOCK alone for a full block; a ranged hit is
// a Spell and keeps its quality (CreateProcExtendMask), even when a block, an absorb or a resist
// takes all of it, as does any other Spell a block takes whole. An absorb that takes the rest keeps
// the hit's quality and adds PROC_EX_ABSORB.
fn unlanded_result(outcome: AttackOutcome, hit: &Hit, dealt: u32, absorbed: u32) -> Option<u32> {
    match outcome {
        AttackOutcome::Miss => Some(PROC_EX_MISS),
        AttackOutcome::Dodge => Some(PROC_EX_DODGE),
        AttackOutcome::Parry => Some(PROC_EX_PARRY),
        AttackOutcome::Immune => Some(PROC_EX_IMMUNE),
        AttackOutcome::Evade => None,
        AttackOutcome::Normal
        | AttackOutcome::Crit
        | AttackOutcome::Glancing
        | AttackOutcome::Crushing
        | AttackOutcome::Block => match (dealt, absorbed, hit.blocked) {
            (0, _, _) if hit.source == HitSource::Ranged => Some(landed_result(hit, absorbed)),
            (0, _, true) if hit.spell_id != 0 => Some(landed_result(hit, absorbed)),
            (0, 0, true) => Some(PROC_EX_BLOCK),
            (0, 1.., _) => Some(landed_result(hit, absorbed)),
            _ => None,
        },
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

// PROC_FLAG_MAIN_HAND_WEAPON_SWING (0x0040_0000) is not modelled: vmangos notes no spell carries it.
pub(crate) fn hit_bits(hit: &Hit) -> (u32, u32) {
    match hit.source {
        HitSource::MainHand if hit.is_queued_strike() => (
            PROC_FLAG_SUCCESSFUL_MELEE_HIT | PROC_FLAG_SUCCESSFUL_MELEE_SPELL_HIT,
            PROC_FLAG_TAKEN_MELEE_HIT | PROC_FLAG_TAKEN_MELEE_SPELL_HIT,
        ),
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
        HitSource::Triggered | HitSource::DamageShield => (0, 0),
    }
}

/// vmangos `Spell::DoAllEffectOnTarget`: past the first unit, a melee ability hit gives its
/// attacker a harmful spell hit instead; the victim keeps its bits.
fn secondary_target_dealer_bits(hit: &Hit, dealer_bits: u32) -> u32 {
    if hit.secondary_target && dealer_bits & PROC_FLAG_SUCCESSFUL_MELEE_SPELL_HIT != 0 {
        (dealer_bits & !PROC_FLAG_SUCCESSFUL_MELEE_SPELL_HIT)
            | PROC_FLAG_SUCCESSFUL_NEGATIVE_SPELL_HIT
    } else {
        dealer_bits
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

// vmangos SpellMgr::IsSpellProcEventCanTriggeredBy; LyraCore has no cast-end pass, so a
// PROC_EX_CAST_END proc fires on the hit instead.
fn proc_ex_allows(proc_ex: u32, result: u32) -> bool {
    if proc_ex & PROC_EX_TRIGGER_ALWAYS != 0 {
        return true;
    }
    match proc_ex & PROC_EX_RESULTS {
        0 => result & (PROC_EX_NORMAL_HIT | PROC_EX_CRITICAL_HIT) != 0,
        wanted => wanted & result != 0,
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
    if !proc_ex_allows(profile.proc_ex, hit.result) {
        return None;
    }
    if profile.ready_micros > now_micros {
        return None;
    }
    if roll_bp >= proc_chance_bp(profile, side, hit.attack_time_ms) {
        return None;
    }
    Some(fire_of(profile, now_micros))
}

fn fire_of(profile: &ProcProfile, now_micros: i64) -> ProcFire {
    let limited = profile.charges > 0;
    let charges_left = if limited { profile.charges - 1 } else { 0 };
    ProcFire {
        charges_left,
        remove_buff: limited && charges_left == 0,
        ready_micros: if profile.icd_ms > 0 {
            now_micros + (profile.icd_ms as i64) * 1000
        } else {
            profile.ready_micros
        },
    }
}

const FEAR_WARD: u32 = 6346;
// vmangos SharedDefines.h MECHANIC_FEAR.
const MECHANIC_FEAR: u8 = 5;

// vmangos IsTriggeredAtSpellProcEvent decides Fear Ward before its proc event: it fires on a
// spell of mechanic fear taken, with no roll and whatever the result, and on nothing else.
fn fear_ward_decide(profile: &ProcProfile, hit: &ProcHit, now_micros: i64) -> Option<ProcFire> {
    hit.spell
        .is_some_and(|spell| spell.mechanic == MECHANIC_FEAR)
        .then(|| fire_of(profile, now_micros))
}

/// Every Aura Holder carries its spell's profile and base charges (vmangos SpellAuraHolder).
pub(crate) fn frozen_profile(hdr: &Spell, overlay: Option<&SpellProcEvent>) -> ProcProfile {
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

// vmangos keys a proc event on the first rank and gives it to every higher rank without its own.
// The caster's SPELLMOD_CHARGES modifiers add to the charges, as SpellAuraHolder's constructor does.
pub(crate) fn freeze_profile(ctx: &ReducerContext, hdr: &Spell, caster_guid: u64) -> ProcProfile {
    let events = ctx.db.game_spell_proc_event();
    let overlay = events
        .spell_id()
        .find(hdr.spell_id)
        .or_else(|| events.spell_id().find(super::first_rank(ctx, hdr.spell_id)));
    let profile = frozen_profile(hdr, overlay.as_ref());
    let (flat, pct) = super::spell_mod(ctx, caster_guid, SPELLMOD_OP_CHARGES, hdr);
    ProcProfile {
        charges: super::apply_spell_mod(u32::from(profile.charges), flat, pct)
            .min(u32::from(u8::MAX)) as u8,
        ..profile
    }
}

pub(crate) fn run_proc_pass(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    hit: &Hit,
    target_survived: bool,
    absorbed: u32,
) {
    fire_aura_procs(
        ctx,
        attacker_guid,
        target_guid,
        hit,
        (landed_result(hit, absorbed), true),
        target_survived,
    );
}

/// The proc pass of a weapon swing or ability that dealt no damage: missed, dodged, parried,
/// fully blocked or fully absorbed. `dealt` is the damage that landed and `absorbed` the damage
/// absorbs took; a landed hit has had its pass already.
pub(crate) fn run_unlanded_proc_pass(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    hit: &Hit,
    outcome: AttackOutcome,
    (dealt, absorbed): (u32, u32),
) {
    if let Some(result) = unlanded_result(outcome, hit, dealt, absorbed) {
        fire_aura_procs(ctx, attacker_guid, target_guid, hit, (result, false), true);
    }
}

/// The proc pass of a harmful spell that an immunity refused whole, with PROC_EX_IMMUNE.
pub(crate) fn run_immune_proc_pass(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
    spell_id: u32,
) {
    let damage_class = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .map_or(0, |spell| spell.damage_class);
    let source = match damage_class {
        DAMAGE_CLASS_MELEE => HitSource::MeleeSpell,
        DAMAGE_CLASS_RANGED => HitSource::Ranged,
        _ => HitSource::Spell,
    };
    fire_aura_procs(
        ctx,
        caster_guid,
        target_guid,
        &Hit::spell(source, spell_id, false),
        (PROC_EX_IMMUNE, false),
        true,
    );
}

pub(crate) const DAMAGE_CLASS_MELEE: u8 = 2;

pub(crate) const DAMAGE_CLASS_RANGED: u8 = 3;

/// vmangos rolls a PPM proc with the attack type of the hit, and a ranged spell's is RANGED.
fn proc_attack(source: HitSource, damage_class: Option<u8>) -> HitSource {
    if damage_class == Some(DAMAGE_CLASS_RANGED) {
        HitSource::Ranged
    } else {
        source
    }
}

fn fire_aura_procs(
    ctx: &ReducerContext,
    attacker_guid: u64,
    target_guid: u64,
    hit: &Hit,
    (result, dealt_damage): (u32, bool),
    target_survived: bool,
) {
    let (dealer_bits, victim_bits) = hit_bits(hit);
    let dealer_bits = secondary_target_dealer_bits(hit, dealer_bits);
    if dealer_bits == 0 && victim_bits == 0 {
        return;
    }
    // Triggered Casts can change aura rows, so snapshot all candidates before firing any.
    let mut candidates: Vec<(Vec<Aura>, ProcSide, u64)> = Vec::new();
    // Guid zero targets the Carrier, so a missing Counterparty must not trigger a cast.
    if in_world(ctx, target_guid) {
        for holder in proc_holders(ctx, attacker_guid, dealer_bits) {
            candidates.push((holder, ProcSide::Dealer, target_guid));
        }
    }
    if target_survived && in_world(ctx, attacker_guid) {
        for holder in proc_holders(ctx, target_guid, victim_bits) {
            candidates.push((holder, ProcSide::Victim, attacker_guid));
        }
    }
    if candidates.is_empty() {
        return;
    }
    let proc_hit = ProcHit {
        dealer_bits,
        victim_bits,
        spell: triggering_spell(ctx, hit.spell_id),
        result,
        dealt_damage,
        attack_time_ms: ctx
            .db
            .game_world_entity()
            .guid()
            .find(attacker_guid)
            .map_or(0, |a| {
                let damage_class = ctx
                    .db
                    .game_spell()
                    .spell_id()
                    .find(hit.spell_id)
                    .map(|spell| spell.damage_class);
                crate::combat::attack_time(ctx, &a, proc_attack(hit.source, damage_class))
            }),
    };
    let now_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    for (holder, side, counterparty_guid) in candidates {
        fire_holder(ctx, &holder, side, counterparty_guid, &proc_hit, now_micros);
    }
}

/// vmangos SpellAuraProcResult: whether one effect of a triggered Aura Holder fired.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AuraProc {
    Fired,
    Failed,
}

/// The carried spell's fields that the aura-type handlers read.
#[derive(Clone, Copy, Debug, Default)]
struct HolderSpell {
    spell_id: u32,
    family_name: u8,
    family_flags: u64,
    attributes: u32,
    visual_id: u32,
}

impl HolderSpell {
    fn of(ctx: &ReducerContext, spell_id: u32) -> Self {
        ctx.db
            .game_spell()
            .spell_id()
            .find(spell_id)
            .map_or(Self::default(), |hdr| Self {
                spell_id,
                family_name: hdr.family_name,
                family_flags: hdr.family_flags,
                attributes: hdr.attributes,
                visual_id: hdr.visual_id,
            })
    }
}

// vmangos Unit::IsTriggeredAtSpellProcEvent, called from ProcDamageAndSpellFor, rolls each
// Aura Holder once; Unit::HandleTriggers then runs every effect and spends at most one charge.
fn fire_holder(
    ctx: &ReducerContext,
    holder: &[Aura],
    side: ProcSide,
    counterparty_guid: u64,
    hit: &ProcHit,
    now_micros: i64,
) {
    let profile = holder_profile(holder);
    let roll_bp = ctx.random::<u32>() % BASIS_POINTS;
    let decision = if holder[0].spell_id == FEAR_WARD && side == ProcSide::Victim {
        fear_ward_decide(&profile, hit, now_micros)
    } else {
        decide(&profile, side, hit, now_micros, roll_bp)
    };
    let Some(fire) = decision else {
        return;
    };
    let spell = HolderSpell::of(ctx, holder[0].spell_id);
    let results: Vec<AuraProc> = holder
        .iter()
        .map(|row| aura_proc(ctx, row, &spell, profile.charges, counterparty_guid, hit))
        .collect();
    let fired = results.contains(&AuraProc::Fired);
    let spends = profile.charges > 0 && spends_charge(spell.attributes, &results);
    if !fired && !spends {
        return;
    }
    let fresh: Vec<Aura> = holder
        .iter()
        .filter_map(|row| ctx.db.game_aura().id().find(row.id))
        .collect();
    // A refresh in this invocation refills charges and wins over the pre-refresh count.
    let spends = spends
        && !fresh
            .iter()
            .any(|row| row.applied_at == crate::time::now(ctx));
    if spends && fire.remove_buff {
        remove_carrier_buff(ctx, holder[0].target_guid, holder[0].spell_id);
        return;
    }
    let cooling_down = starts_cooldown(holder, &results);
    for mut row in fresh {
        if spends {
            row.proc_charges = fire.charges_left;
        }
        if cooling_down {
            row.proc_ready_micros = fire.ready_micros;
        }
        row.update(ctx);
    }
}

// vmangos starts the internal cooldown in TriggerProccedSpell: only a Proc row that fired starts it.
fn starts_cooldown(holder: &[Aura], results: &[AuraProc]) -> bool {
    holder
        .iter()
        .zip(results)
        .any(|(row, result)| is_proc_kind(row.eff_kind) && *result == AuraProc::Fired)
}

const SPELL_ATTR_PROC_FAILURE_BURNS_CHARGE: u32 = 0x1;

// vmangos HandleTriggers: an Aura Holder with PROC_FAILURE_BURNS_CHARGE spends only when every
// effect failed, and any other only when none did.
fn spends_charge(attributes: u32, results: &[AuraProc]) -> bool {
    let burns_on_failure = attributes & SPELL_ATTR_PROC_FAILURE_BURNS_CHARGE != 0;
    results
        .iter()
        .all(|result| (*result == AuraProc::Failed) == burns_on_failure)
}

fn holder_profile(holder: &[Aura]) -> ProcProfile {
    let mut profile = ProcProfile::of(&holder[0]);
    for row in &holder[1..] {
        profile.charges = profile.charges.max(row.proc_charges);
        profile.ready_micros = profile.ready_micros.max(row.proc_ready_micros);
    }
    profile
}

// vmangos AuraProcHandler by aura type, and the spell's AuraScript where LyraCore has one.
fn aura_proc(
    ctx: &ReducerContext,
    row: &Aura,
    spell: &HolderSpell,
    charges: u8,
    counterparty_guid: u64,
    hit: &ProcHit,
) -> AuraProc {
    match row.eff_kind {
        A_PROC_TRIGGER | A_PROC_DAMAGE => {
            if !refuses_extra_attacks(ctx, row) && fire_proc(ctx, row, counterparty_guid) {
                AuraProc::Fired
            } else {
                AuraProc::Failed
            }
        }
        A_FLAG => dummy_proc(ctx, row, charges, hit),
        _ => effect_proc(row, spell, charges, hit),
    }
}

const SPELL_FAMILY_PRIEST: u8 = 6;
// vmangos SpellClassMask.h CF_PRIEST_INNER_FIRE.
const FAMILY_FLAG_PRIEST_INNER_FIRE: u64 = 0x2;
// vmangos Unit::HandleHasteAuraProc names Flurry by SpellIconID 108 and SpellVisual 2759.
const FLURRY_VISUAL: u32 = 2759;
// The importer keeps Insight's aura 73, SPELL_AURA_MOD_POWER_COST_SCHOOL, as A_INERT.
const INSIGHT: u32 = 18820;
// Insight's EffectMiscValue: every school but physical.
const INSIGHT_SCHOOLS: u8 = 0x7E;

/// The vmangos aura-type handler of a non-Proc effect; `Failed` keeps the charge.
fn effect_proc(row: &Aura, spell: &HolderSpell, charges: u8, hit: &ProcHit) -> AuraProc {
    let school_mask = hit
        .spell
        .map_or(SCHOOL_MASK_PHYSICAL, |spell| spell.school_mask);
    let admitted = match row.eff_kind {
        // HandleModDamageAuraProc.
        A_MOD_DAMAGE_DONE_FLAT => row.eff_p0 & i32::from(school_mask) != 0,
        // HandleModResistanceAuraProc: Inner Fire counts only a hit that dealt damage.
        A_MOD_RESISTANCE
            if spell.family_name == SPELL_FAMILY_PRIEST
                && spell.family_flags & FAMILY_FLAG_PRIEST_INNER_FIRE != 0 =>
        {
            hit.dealt_damage
        }
        // HandleHasteAuraProc: Flurry's talent applies it again on a crit, so the last charge stays.
        A_MOD_SPEED if row.eff_p0 == i32::from(SPEED_SWING) && spell.visual_id == FLURRY_VISUAL => {
            !(charges <= 1 && hit.crit())
        }
        // HandleModCastingSpeedNotStackAuraProc: only a spell with a cast time.
        A_MOD_SPEED if row.eff_p0 == i32::from(SPEED_CAST) => {
            hit.spell.is_some_and(|spell| spell.cast_time_ms > 0)
        }
        // HandleMechanicImmuneResistanceAuraProc: only a spell of the immune mechanic. Aura 147
        // has HandleNULLProc, which always fires.
        A_IMMUNITY if row.eff_p0_kind == P_MECHANIC && !is_mechanic_immunity_mask(row.eff_p1) => {
            hit.spell
                .is_some_and(|spell| i32::from(spell.mechanic) == row.eff_p0)
        }
        // HandleModPowerCostSchoolAuraProc: only a spell of the school that costs power.
        A_INERT if spell.spell_id == INSIGHT => hit
            .spell
            .is_some_and(|spell| spell.cost > 0 && spell.school_mask & INSIGHT_SCHOOLS != 0),
        _ => true,
    };
    if admitted {
        AuraProc::Fired
    } else {
        AuraProc::Failed
    }
}

const SWEEPING_STRIKES: [u32; 2] = [12292, 18765];
const COMBUSTION: u32 = 11129;
const COMBUSTION_CRIT_BUFF: u32 = 28682;

// vmangos HandleDummyAuraProc and the spell's AuraScript, where LyraCore has one.
fn dummy_proc(ctx: &ReducerContext, row: &Aura, charges: u8, hit: &ProcHit) -> AuraProc {
    if SWEEPING_STRIKES.contains(&row.spell_id) {
        return AuraProc::Failed;
    }
    if row.spell_id != COMBUSTION {
        return AuraProc::Fired;
    }
    // vmangos spell_mage.cpp MageCombustionProcScript.
    let owner = row.target_guid;
    if !auras_on(ctx, owner).any(|a| a.spell_id == COMBUSTION_CRIT_BUFF) {
        remove_carrier_buff(ctx, owner, COMBUSTION);
        return AuraProc::Failed;
    }
    if charges <= 1 && hit.crit() {
        remove_carrier_buff(ctx, owner, COMBUSTION_CRIT_BUFF);
        return AuraProc::Fired;
    }
    if let Err(err) =
        crate::spell::cast_triggered(ctx, owner, COMBUSTION_CRIT_BUFF, row.level, owner)
    {
        log::info!("proc: Combustion on {owner} could not stack its crit buff: {err}");
    }
    if hit.crit() {
        AuraProc::Fired
    } else {
        AuraProc::Failed
    }
}

// vmangos fails, without spending, a Proc granting Extra Attacks while the Carrier holds some.
fn refuses_extra_attacks(ctx: &ReducerContext, row: &Aura) -> bool {
    row.eff_kind == A_PROC_TRIGGER
        && super::chance_on_hit::grants_extra_attacks(ctx, row.eff_p1.max(0) as u32)
        && crate::combat::has_extra_attacks(ctx, row.target_guid)
}

fn in_world(ctx: &ReducerContext, guid: u64) -> bool {
    guid != 0 && ctx.db.game_world_entity().guid().find(guid).is_some()
}

// Skip new auras so a proc cannot fire another proc in the same invocation.
fn proc_holders(ctx: &ReducerContext, carrier_guid: u64, bits: u32) -> Vec<Vec<Aura>> {
    if bits == 0 {
        return Vec::new();
    }
    let rows = auras_on(ctx, carrier_guid)
        .filter(|a| a.applied_at != crate::time::now(ctx) && a.proc_flags & bits != 0)
        .map(|a| {
            let item_guid = a.item_guid(ctx);
            (a, item_guid)
        });
    let mut holders = group_holders(rows);
    holders.retain(|holder| takes_part(holder));
    holders
}

// An Aura Holder is one caster's spell on one unit, and each equipped item applies its own.
fn group_holders(rows: impl IntoIterator<Item = (Aura, u64)>) -> Vec<Vec<Aura>> {
    let mut holders: Vec<((u32, u64, u64), Vec<Aura>)> = Vec::new();
    for (row, item_guid) in rows {
        let key = (row.spell_id, row.caster_guid, item_guid);
        match holders
            .iter_mut()
            .find(|(holder_key, _)| *holder_key == key)
        {
            Some((_, holder)) => holder.push(row),
            None => holders.push((key, vec![row])),
        }
    }
    holders.into_iter().map(|(_, holder)| holder).collect()
}

// vmangos skips an Aura Holder whose spell modifier carries charges; the modified spell spends them.
fn takes_part(holder: &[Aura]) -> bool {
    let charged = holder.iter().any(|row| row.proc_charges > 0);
    let spell_modifier = holder
        .iter()
        .any(|row| matches!(row.eff_kind, A_SPELLMOD_FLAT | A_SPELLMOD_PCT));
    if charged {
        !spell_modifier
    } else {
        holder.iter().any(|row| is_proc_kind(row.eff_kind))
    }
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
            mechanic: s.mechanic,
            cast_time_ms: s.cast_time_ms,
            cost: s.cost,
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
            match crate::spell::cast_aura_triggered(
                ctx,
                row.spell_id,
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
                    // vmangos TriggerProccedSpell ignores the cast result, so a reagent refusal still fires.
                    err.reason == crate::spell::CastRefusalReason::MissingReagent
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
    let points = ctx
        .db
        .game_spell()
        .spell_id()
        .find(row.spell_id)
        .zip(ctx.db.game_spell_effect().id().find(row.effect_id))
        .map_or(row.amount, |(hdr, effect)| {
            let damage = super::bonuses::spell_damage_done_float(
                ctx,
                &hdr,
                &effect,
                row.target_guid,
                row.amount as f32,
                false,
            );
            let damage = super::bonuses::spell_damage_taken(
                ctx,
                &hdr,
                &effect,
                row.target_guid,
                counterparty_guid,
                damage,
                1,
            );
            super::bonuses::round_damage(ctx, counterparty_guid, damage)
        });
    let after_resist = apply_resistance(
        ctx,
        counterparty_guid,
        row.target_guid,
        school_mask,
        points,
        row.spell_id,
    );
    let (dealt, absorbed) = crate::combat::deal_damage(
        ctx,
        row.target_guid,
        counterparty_guid,
        after_resist,
        Hit::spell(HitSource::Triggered, row.spell_id, false),
    );
    if dealt > 0 || absorbed > 0 || points > after_resist {
        crate::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                target_guid: counterparty_guid,
                damage: dealt,
                school: school_index(school_mask),
                resisted: (points.max(0) as u32).saturating_sub(after_resist.max(0) as u32),
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
    shed_auras(ctx, spent);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swing(source: HitSource) -> Hit {
        Hit::weapon(source, false)
    }

    #[test]
    fn a_ranged_spells_proc_rolls_with_the_ranged_weapon() {
        assert!(matches!(
            proc_attack(HitSource::MeleeSpell, Some(DAMAGE_CLASS_RANGED)),
            HitSource::Ranged
        ));
        assert!(matches!(
            proc_attack(HitSource::MeleeSpell, Some(2)),
            HitSource::MeleeSpell
        ));
        assert!(matches!(
            proc_attack(HitSource::OffHand, None),
            HitSource::OffHand
        ));
    }

    fn spell_facts() -> TriggeringSpell {
        TriggeringSpell {
            school_mask: 0,
            family_name: 0,
            family_flags: 0,
            mechanic: 0,
            cast_time_ms: 0,
            cost: 0,
        }
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
            result: PROC_EX_NORMAL_HIT,
            dealt_damage: true,
            attack_time_ms: 2000,
        }
    }

    #[test]
    fn a_main_hand_swing_raises_the_melee_hit_event_on_both_sides() {
        assert_eq!(hit_bits(&swing(HitSource::MainHand)), (0x4, 0x8));
    }

    #[test]
    fn a_queued_strike_raises_the_swing_and_the_melee_ability_events_on_both_sides() {
        assert_eq!(
            hit_bits(&Hit::spell(HitSource::MainHand, 78, false)),
            (0x4 | 0x10, 0x8 | 0x20)
        );
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
    fn a_secondary_target_gives_its_attacker_a_harmful_spell_hit_for_a_melee_ability() {
        let first = Hit::spell(HitSource::MeleeSpell, 845, false);
        let secondary = first.as_secondary_target();
        assert_eq!(secondary_target_dealer_bits(&first, 0x4 | 0x10), 0x4 | 0x10);
        assert_eq!(
            secondary_target_dealer_bits(&secondary, 0x4 | 0x10),
            0x4 | 0x1_0000,
            "vmangos keeps the main-hand swing bit and swaps 0x10 for 0x10000"
        );
        assert_eq!(
            secondary_target_dealer_bits(&secondary, 0x40),
            0x40,
            "a hit without the melee ability bit keeps its bits"
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
            result: PROC_EX_CRITICAL_HIT,
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
    fn a_proc_that_triggers_always_fires_on_an_avoided_swing_and_any_other_does_not() {
        let missed = ProcHit {
            result: PROC_EX_MISS,
            ..melee_hit()
        };
        let flurry = ProcProfile {
            proc_ex: PROC_EX_TRIGGER_ALWAYS,
            charges: 3,
            ..always()
        };
        assert!(decide(&flurry, ProcSide::Victim, &missed, 0, 0).is_some());
        assert!(decide(&always(), ProcSide::Victim, &missed, 0, 0).is_none());
    }

    #[test]
    fn a_proc_that_names_a_result_fires_only_on_that_result() {
        let on_block = ProcProfile {
            proc_ex: PROC_EX_BLOCK,
            ..always()
        };
        let blocked = ProcHit {
            result: PROC_EX_NORMAL_HIT | PROC_EX_BLOCK,
            ..melee_hit()
        };
        assert!(decide(&on_block, ProcSide::Victim, &blocked, 0, 0).is_some());
        assert!(decide(&on_block, ProcSide::Victim, &melee_hit(), 0, 0).is_none());

        let on_dodge = ProcProfile {
            proc_ex: PROC_EX_DODGE,
            ..always()
        };
        let dodged = ProcHit {
            result: PROC_EX_DODGE,
            ..melee_hit()
        };
        assert!(decide(&on_dodge, ProcSide::Victim, &dodged, 0, 0).is_some());
        assert!(decide(&on_dodge, ProcSide::Victim, &melee_hit(), 0, 0).is_none());
    }

    #[test]
    fn a_blocked_hit_still_fires_a_proc_on_hits() {
        let blocked = ProcHit {
            result: PROC_EX_NORMAL_HIT | PROC_EX_BLOCK,
            ..melee_hit()
        };
        assert!(decide(&always(), ProcSide::Victim, &blocked, 0, 0).is_some());
    }

    fn aura(spell_id: u32, eff_kind: u8, proc_charges: u8) -> Aura {
        Aura {
            id: 0,
            target_guid: 1,
            caster_guid: 1,
            spell_id,
            slot: 0,
            level: 60,
            flags: 0,
            applied_at: spacetimedb::Timestamp::UNIX_EPOCH,
            expires_at: spacetimedb::Timestamp::UNIX_EPOCH,
            effect_id: 0,
            eff_kind,
            amount: 0,
            eff_p0: 0,
            eff_p0_kind: 0,
            eff_p1: 0,
            period_ms: 0,
            amount_remaining: 0,
            stacks: 1,
            next_tick_micros: 0,
            channel_target: 0,
            enters_combat: false,
            proc_flags: PROC_FLAG_TAKEN_MELEE_HIT,
            proc_chance: 100,
            proc_ppm: 0.0,
            proc_ex: 0,
            proc_school_mask: 0,
            proc_family_name: 0,
            proc_family_flags: 0,
            proc_charges,
            proc_icd_ms: 0,
            proc_ready_micros: 0,
        }
    }

    #[test]
    fn a_charged_aura_takes_part_in_the_proc_pass_whatever_its_effect() {
        assert!(takes_part(&[aura(588, A_MOD_RESISTANCE, 20)]));
        assert!(takes_part(&[aura(324, A_PROC_DAMAGE, 0)]));
        assert!(!takes_part(&[aura(1243, A_MOD_STAT, 0)]));
    }

    #[test]
    fn a_charged_spell_modifier_is_left_to_the_spell_that_uses_it() {
        assert!(!takes_part(&[aura(16870, A_SPELLMOD_PCT, 1)]));
        assert!(!takes_part(&[
            aura(16166, A_SPELLMOD_FLAT, 1),
            aura(16166, A_SPELLMOD_PCT, 1)
        ]));
    }

    fn spell_hit(spell: TriggeringSpell, result: u32) -> ProcHit {
        ProcHit {
            spell: Some(spell),
            result,
            ..melee_hit()
        }
    }

    #[test]
    fn inner_fire_spends_only_on_a_hit_that_dealt_damage() {
        let inner_fire = HolderSpell {
            family_name: SPELL_FAMILY_PRIEST,
            family_flags: FAMILY_FLAG_PRIEST_INNER_FIRE,
            ..HolderSpell::default()
        };
        let row = aura(588, A_MOD_RESISTANCE, 20);
        let absorbed = ProcHit {
            result: PROC_EX_NORMAL_HIT | PROC_EX_ABSORB,
            dealt_damage: false,
            ..melee_hit()
        };
        assert_eq!(
            effect_proc(&row, &inner_fire, 20, &melee_hit()),
            AuraProc::Fired
        );
        assert_eq!(
            effect_proc(&row, &inner_fire, 20, &absorbed),
            AuraProc::Failed
        );
    }

    #[test]
    fn flurry_keeps_its_last_charge_on_a_crit() {
        let flurry = HolderSpell {
            visual_id: FLURRY_VISUAL,
            ..HolderSpell::default()
        };
        let mut row = aura(12970, A_MOD_SPEED, 1);
        row.eff_p0 = i32::from(SPEED_SWING);
        let crit = ProcHit {
            result: PROC_EX_CRITICAL_HIT,
            ..melee_hit()
        };
        assert_eq!(effect_proc(&row, &flurry, 1, &crit), AuraProc::Failed);
        assert_eq!(effect_proc(&row, &flurry, 2, &crit), AuraProc::Fired);
        assert_eq!(effect_proc(&row, &flurry, 1, &melee_hit()), AuraProc::Fired);
        assert_eq!(
            effect_proc(&row, &HolderSpell::default(), 1, &crit),
            AuraProc::Fired,
            "another melee haste Aura has no such rule"
        );
    }

    #[test]
    fn a_cast_speed_charge_is_spent_only_by_a_spell_with_a_cast_time() {
        let mut focus = aura(18803, A_MOD_SPEED, 1);
        focus.eff_p0 = i32::from(SPEED_CAST);
        let spell = HolderSpell::default();
        let cast = spell_hit(
            TriggeringSpell {
                cast_time_ms: 1500,
                ..spell_facts()
            },
            PROC_EX_NORMAL_HIT,
        );
        let instant = spell_hit(spell_facts(), PROC_EX_NORMAL_HIT);
        assert_eq!(effect_proc(&focus, &spell, 1, &cast), AuraProc::Fired);
        assert_eq!(effect_proc(&focus, &spell, 1, &instant), AuraProc::Failed);
        assert_eq!(
            effect_proc(&focus, &spell, 1, &melee_hit()),
            AuraProc::Failed
        );
    }

    #[test]
    fn fear_ward_fires_on_a_fear_whatever_its_result_and_on_no_other_spell() {
        let profile = ProcProfile {
            flags: PROC_FLAG_TAKEN_NEGATIVE_SPELL_HIT,
            chance: 100,
            proc_ex: PROC_EX_NORMAL_HIT,
            charges: 1,
            ..ProcProfile::default()
        };
        let fear = TriggeringSpell {
            mechanic: MECHANIC_FEAR,
            ..spell_facts()
        };
        let refused_fear = spell_hit(fear, PROC_EX_IMMUNE);
        let fire = fear_ward_decide(&profile, &refused_fear, 0).expect("a refused fear fires");
        assert!(fire.remove_buff);
        assert_eq!(
            decide(&profile, ProcSide::Victim, &refused_fear, 0, 0),
            None,
            "its proc event names a normal hit, which a refused fear is not"
        );
        assert_eq!(
            fear_ward_decide(&profile, &spell_hit(spell_facts(), PROC_EX_NORMAL_HIT), 0),
            None
        );
    }

    #[test]
    fn a_mechanic_immunity_charge_is_spent_only_by_a_spell_of_that_mechanic() {
        let mut fear_ward = aura(6346, A_IMMUNITY, 1);
        fear_ward.eff_p0 = i32::from(MECHANIC_FEAR);
        fear_ward.eff_p0_kind = P_MECHANIC;
        let spell = HolderSpell::default();
        let fear = spell_hit(
            TriggeringSpell {
                mechanic: MECHANIC_FEAR,
                ..spell_facts()
            },
            PROC_EX_NORMAL_HIT,
        );
        let frostbolt = spell_hit(spell_facts(), PROC_EX_NORMAL_HIT);
        assert_eq!(effect_proc(&fear_ward, &spell, 1, &fear), AuraProc::Fired);
        assert_eq!(
            effect_proc(&fear_ward, &spell, 1, &frostbolt),
            AuraProc::Failed
        );
        let mut mask = fear_ward;
        mask.eff_p1 = 147;
        mask.eff_p0 = 817;
        assert_eq!(
            effect_proc(&mask, &spell, 1, &frostbolt),
            AuraProc::Fired,
            "aura 147 has HandleNULLProc, which fires on any spell"
        );
    }

    #[test]
    fn insight_is_spent_only_by_a_magic_spell_that_costs_power() {
        let insight = HolderSpell {
            spell_id: INSIGHT,
            ..HolderSpell::default()
        };
        let row = aura(INSIGHT, A_INERT, 1);
        let costly = |school_mask| {
            spell_hit(
                TriggeringSpell {
                    school_mask,
                    cost: 50,
                    ..spell_facts()
                },
                PROC_EX_NORMAL_HIT,
            )
        };
        let free = spell_hit(
            TriggeringSpell {
                school_mask: 0x4,
                ..spell_facts()
            },
            PROC_EX_NORMAL_HIT,
        );
        assert_eq!(
            effect_proc(&row, &insight, 1, &costly(0x4)),
            AuraProc::Fired
        );
        assert_eq!(
            effect_proc(&row, &insight, 1, &costly(SCHOOL_MASK_PHYSICAL)),
            AuraProc::Failed
        );
        assert_eq!(effect_proc(&row, &insight, 1, &free), AuraProc::Failed);
    }

    #[test]
    fn a_flat_damage_charge_is_spent_only_by_a_hit_of_its_school() {
        let mut row = aura(24389, A_MOD_DAMAGE_DONE_FLAT, 1);
        row.eff_p0 = 0x4;
        let spell = HolderSpell::default();
        let fire = spell_hit(
            TriggeringSpell {
                school_mask: 0x4,
                ..spell_facts()
            },
            PROC_EX_NORMAL_HIT,
        );
        assert_eq!(effect_proc(&row, &spell, 1, &fire), AuraProc::Fired);
        assert_eq!(effect_proc(&row, &spell, 1, &melee_hit()), AuraProc::Failed);
    }

    #[test]
    fn a_spell_that_burns_charges_on_failure_spends_only_when_every_effect_failed() {
        let burns = SPELL_ATTR_PROC_FAILURE_BURNS_CHARGE;
        assert!(!spends_charge(burns, &[AuraProc::Fired]));
        assert!(spends_charge(burns, &[AuraProc::Failed]));
        assert!(spends_charge(0, &[AuraProc::Fired]));
        assert!(!spends_charge(0, &[AuraProc::Fired, AuraProc::Failed]));
    }

    #[test]
    fn each_equipped_item_of_one_spell_is_its_own_holder() {
        let mut first = aura(16511, A_MOD_STAT, 3);
        first.id = 1;
        let mut second = aura(16511, A_MOD_STAT, 3);
        second.id = 2;
        let mut sibling = aura(16511, A_MOD_RESISTANCE, 3);
        sibling.id = 3;
        let holders = group_holders([(first, 10), (second, 11), (sibling, 10)]);
        let ids: Vec<Vec<u64>> = holders
            .iter()
            .map(|holder| holder.iter().map(|row| row.id).collect())
            .collect();
        assert_eq!(ids, [vec![1, 3], vec![2]]);
    }

    #[test]
    fn an_absorbed_swing_keeps_its_quality_and_adds_the_absorb() {
        let swing = Hit::weapon(HitSource::MainHand, false);
        let blocked = swing.with_block(10);
        let crit = Hit::weapon(HitSource::MainHand, true);
        assert_eq!(
            unlanded_result(AttackOutcome::Normal, &swing, 0, 40),
            Some(PROC_EX_NORMAL_HIT | PROC_EX_ABSORB)
        );
        assert_eq!(
            unlanded_result(AttackOutcome::Crit, &crit, 0, 40),
            Some(PROC_EX_CRITICAL_HIT | PROC_EX_ABSORB)
        );
        assert_eq!(
            unlanded_result(AttackOutcome::Block, &blocked, 0, 30),
            Some(PROC_EX_NORMAL_HIT | PROC_EX_BLOCK | PROC_EX_ABSORB)
        );
        assert_eq!(
            unlanded_result(AttackOutcome::Block, &blocked, 0, 0),
            Some(PROC_EX_BLOCK),
            "a full block raises the block alone"
        );
        assert_eq!(unlanded_result(AttackOutcome::Normal, &swing, 5, 40), None);
    }

    #[test]
    fn a_landed_hit_an_absorb_took_part_of_adds_the_absorb() {
        let swing = Hit::weapon(HitSource::MainHand, false);
        let crit = Hit::spell(HitSource::Spell, 133, true);
        let blocked = swing.with_block(10);
        assert_eq!(landed_result(&swing, 0), PROC_EX_NORMAL_HIT);
        assert_eq!(
            landed_result(&swing, 7),
            PROC_EX_NORMAL_HIT | PROC_EX_ABSORB
        );
        assert_eq!(
            landed_result(&crit, 7),
            PROC_EX_CRITICAL_HIT | PROC_EX_ABSORB
        );
        assert_eq!(
            landed_result(&blocked, 7),
            PROC_EX_NORMAL_HIT | PROC_EX_BLOCK | PROC_EX_ABSORB
        );
    }

    #[test]
    fn a_fully_blocked_ranged_hit_keeps_its_quality_beside_the_block() {
        let shot = Hit::weapon(HitSource::Ranged, false).with_block(10);
        let crit = Hit::spell(HitSource::Ranged, 2643, true).with_block(10);
        assert_eq!(
            unlanded_result(AttackOutcome::Normal, &shot, 0, 0),
            Some(PROC_EX_NORMAL_HIT | PROC_EX_BLOCK)
        );
        assert_eq!(
            unlanded_result(AttackOutcome::Crit, &crit, 0, 0),
            Some(PROC_EX_CRITICAL_HIT | PROC_EX_BLOCK)
        );
    }

    #[test]
    fn a_fully_blocked_cast_spell_keeps_its_quality_beside_the_block() {
        let strike = Hit::spell(HitSource::MeleeSpell, 12294, false).with_block(10);
        let crit = Hit::spell(HitSource::MeleeSpell, 12294, true).with_block(10);
        let bloodthirst = Hit::spell(HitSource::Spell, 23881, false).with_block(10);
        assert_eq!(
            unlanded_result(AttackOutcome::Normal, &strike, 0, 0),
            Some(PROC_EX_NORMAL_HIT | PROC_EX_BLOCK)
        );
        assert_eq!(
            unlanded_result(AttackOutcome::Normal, &bloodthirst, 0, 0),
            Some(PROC_EX_NORMAL_HIT | PROC_EX_BLOCK)
        );
        assert_eq!(
            unlanded_result(AttackOutcome::Crit, &crit, 0, 0),
            Some(PROC_EX_CRITICAL_HIT | PROC_EX_BLOCK)
        );
        let landed = Hit::spell(HitSource::MeleeSpell, 12294, false);
        assert_eq!(unlanded_result(AttackOutcome::Normal, &landed, 0, 0), None);
    }

    #[test]
    fn a_fully_blocked_queued_strike_keeps_its_quality_beside_the_block() {
        let strike = Hit::spell(HitSource::MainHand, 78, false).with_block(10);
        let crit = Hit::spell(HitSource::MainHand, 78, true).with_block(10);
        assert_eq!(
            unlanded_result(AttackOutcome::Normal, &strike, 0, 0),
            Some(PROC_EX_NORMAL_HIT | PROC_EX_BLOCK)
        );
        assert_eq!(
            unlanded_result(AttackOutcome::Crit, &crit, 0, 0),
            Some(PROC_EX_CRITICAL_HIT | PROC_EX_BLOCK)
        );
        let white = Hit::weapon(HitSource::MainHand, false).with_block(10);
        assert_eq!(
            unlanded_result(AttackOutcome::Block, &white, 0, 0),
            Some(PROC_EX_BLOCK)
        );
    }

    #[test]
    fn a_fully_resisted_ranged_hit_still_raises_a_normal_hit() {
        let shot = Hit::weapon(HitSource::Ranged, false);
        assert_eq!(
            unlanded_result(AttackOutcome::Normal, &shot, 0, 0),
            Some(PROC_EX_NORMAL_HIT)
        );
        assert_eq!(unlanded_result(AttackOutcome::Normal, &shot, 4, 0), None);
    }

    #[test]
    fn only_a_proc_row_that_fired_starts_the_internal_cooldown() {
        let holder = [aura(20925, A_PROC_DAMAGE, 4), aura(20925, A_MOD_STAT, 4)];
        assert!(!starts_cooldown(
            &holder,
            &[AuraProc::Failed, AuraProc::Fired]
        ));
        assert!(starts_cooldown(
            &holder,
            &[AuraProc::Fired, AuraProc::Fired]
        ));
    }

    #[test]
    fn a_holder_rolls_with_the_most_charges_and_the_latest_cooldown_of_its_effects() {
        let mut first = aura(20925, A_PROC_DAMAGE, 4);
        first.proc_ready_micros = 5;
        let mut second = aura(20925, A_MOD_COMBAT, 0);
        second.proc_ready_micros = 9;
        let profile = holder_profile(&[first, second]);
        assert_eq!((profile.charges, profile.ready_micros), (4, 9));
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
                ..spell_facts()
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
                ..spell_facts()
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
        let mut hdr = crate::Spell::blank(168, "Frost Armor");
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
    fn an_aura_freezes_its_spells_header() {
        let hdr = proc_header();
        let frozen = frozen_profile(&hdr, None);
        assert_eq!(frozen.flags, 0x28);
        assert_eq!(frozen.chance, 100);
        assert_eq!(frozen.charges, 3);
        assert_eq!(frozen.ppm, 0.0);
        assert_eq!(frozen.icd_ms, 0);
        assert_eq!(frozen.proc_ex, 0);
        assert_eq!(frozen.school_mask, 0);
        assert_eq!(frozen.family_name, 0);
    }

    #[test]
    fn the_overlay_supplies_the_rate_cooldown_hit_rule_and_filter() {
        let frozen = frozen_profile(
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
            &proc_header(),
            Some(&SpellProcEvent {
                proc_flags: PROC_FLAG_SUCCESSFUL_MELEE_HIT,
                ..overlay()
            }),
        );
        assert_eq!(overridden.flags, PROC_FLAG_SUCCESSFUL_MELEE_HIT);

        let kept = frozen_profile(&proc_header(), Some(&overlay()));
        assert_eq!(kept.flags, 0x28);
    }

    #[test]
    fn a_custom_chance_replaces_the_header_chance_and_zero_keeps_it() {
        let custom = frozen_profile(
            &proc_header(),
            Some(&SpellProcEvent {
                custom_chance: 15,
                ..overlay()
            }),
        );
        assert_eq!(custom.chance, 15);

        let kept = frozen_profile(&proc_header(), Some(&overlay()));
        assert_eq!(kept.chance, 100);
    }

    #[test]
    fn the_rate_beats_the_custom_chance_on_the_dealer_side_only() {
        let frozen = frozen_profile(
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
