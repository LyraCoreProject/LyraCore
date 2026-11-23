use spacetimedb::{log, ReducerContext, ScheduleAt, Table, TimeDuration};

use super::*;
use crate::{
    game_aura, game_creature_template, game_global_cooldown, game_pending_cast,
    game_school_lockout, game_spell, game_spell_effect, game_world_entity, Aura, PendingCast,
    SchoolLockout, SpellCastEvent,
};

fn has_control_mechanic(ctx: &ReducerContext, unit_guid: u64, mechanic: i32) -> bool {
    auras_on(ctx, unit_guid).any(|a| a.eff_kind == A_CONTROL && a.eff_p0 == mechanic)
}

pub(crate) fn has_aura(ctx: &ReducerContext, unit_guid: u64, spell_id: u32) -> bool {
    auras_on(ctx, unit_guid).any(|a| a.spell_id == spell_id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuffStatus {
    Missing,
    Satisfied,
    Unavailable(BuffUnavailableReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuffUnavailableReason {
    AuraLimit,
    Family,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitControl {
    Incapacitated,
    Feared,
    Rooted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlReadError {
    AuraLimit,
    EffectLimit,
}

pub(crate) fn control_status(
    ctx: &ReducerContext,
    unit_guid: u64,
    limit: usize,
) -> Result<Option<UnitControl>, ControlReadError> {
    let auras: Vec<_> = auras_on(ctx, unit_guid)
        .take(limit.saturating_add(1))
        .collect();
    if auras.len() > limit {
        return Err(ControlReadError::AuraLimit);
    }
    let has = |mechanic| {
        auras
            .iter()
            .any(|aura| aura.eff_kind == A_CONTROL && aura.eff_p0 == mechanic)
    };
    Ok(if has(M_STUN) || has(M_POLY) {
        Some(UnitControl::Incapacitated)
    } else if has(M_FEAR) {
        Some(UnitControl::Feared)
    } else if has(M_ROOT) {
        Some(UnitControl::Rooted)
    } else {
        None
    })
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn spell_control(
    ctx: &ReducerContext,
    spell_id: u32,
) -> Result<Option<UnitControl>, ControlReadError> {
    let effects: Vec<_> = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .take(4)
        .collect();
    if effects.len() > 3 {
        return Err(ControlReadError::EffectLimit);
    }
    let has = |mechanic| {
        effects.iter().any(|effect| {
            effect.kind == A_CONTROL && effect.p0_kind == P_MECHANIC && effect.p0 == mechanic
        })
    };
    Ok(if has(M_STUN) || has(M_POLY) {
        Some(UnitControl::Incapacitated)
    } else if has(M_FEAR) {
        Some(UnitControl::Feared)
    } else if has(M_ROOT) {
        Some(UnitControl::Rooted)
    } else {
        None
    })
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn buff_status(
    ctx: &ReducerContext,
    target_guid: u64,
    spell_id: u32,
    caster_level: u8,
) -> BuffStatus {
    const AURA_LIMIT: usize = 64;
    const FAMILY_LIMIT: usize = 64;
    let auras: Vec<_> = auras_on(ctx, target_guid).take(AURA_LIMIT + 1).collect();
    if auras.len() > AURA_LIMIT {
        return BuffStatus::Unavailable(BuffUnavailableReason::AuraLimit);
    }
    if auras.iter().any(|aura| aura.spell_id == spell_id) {
        return BuffStatus::Satisfied;
    }
    match super::stacking::buff_group_status(ctx, spell_id, caster_level, &auras, FAMILY_LIMIT) {
        super::stacking::BuffGroupStatus::NoGroup | super::stacking::BuffGroupStatus::Missing => {
            BuffStatus::Missing
        }
        super::stacking::BuffGroupStatus::Satisfied => BuffStatus::Satisfied,
        super::stacking::BuffGroupStatus::Unavailable => {
            BuffStatus::Unavailable(BuffUnavailableReason::Family)
        }
    }
}

fn is_channel_aura(ctx: &ReducerContext, aura: &Aura) -> bool {
    let flags = ctx
        .db
        .game_spell()
        .spell_id()
        .find(aura.spell_id)
        .map_or(0, |spell| spell.cast_flags);
    lyracore_shared::spell::is_channel_aura(aura.eff_kind, flags)
}

pub(crate) fn is_non_melee_spell_casting(ctx: &ReducerContext, unit_guid: u64) -> bool {
    ctx.db
        .game_pending_cast()
        .by_caster()
        .filter(&unit_guid)
        .next()
        .is_some()
        || ctx
            .db
            .game_aura()
            .by_caster()
            .filter(&unit_guid)
            .any(|aura| is_channel_aura(ctx, &aura))
}

pub(crate) fn is_stunned(ctx: &ReducerContext, unit_guid: u64) -> bool {
    has_control_mechanic(ctx, unit_guid, M_STUN)
}

pub(crate) fn is_disarmed(ctx: &ReducerContext, unit_guid: u64) -> bool {
    auras_on(ctx, unit_guid).any(|a| a.eff_kind == A_DISARM)
}

pub(crate) fn is_rooted(ctx: &ReducerContext, unit_guid: u64) -> bool {
    has_control_mechanic(ctx, unit_guid, M_ROOT)
}

pub(crate) fn is_feared(ctx: &ReducerContext, unit_guid: u64) -> bool {
    has_control_mechanic(ctx, unit_guid, M_FEAR)
}

pub(crate) fn fear_source(ctx: &ReducerContext, unit_guid: u64) -> Option<u64> {
    auras_on(ctx, unit_guid)
        .find(|a| a.eff_kind == A_CONTROL && a.eff_p0 == M_FEAR)
        .map(|a| a.caster_guid)
}

pub(crate) fn cc_blocks(
    stunned: bool,
    rooted: bool,
    feared: bool,
    poly: bool,
) -> (bool, bool, bool) {
    let incapacitated = stunned || poly;
    let action_blocked = incapacitated || feared;
    let movement_blocked = incapacitated || rooted;
    let self_movement_suppressed = movement_blocked || feared;
    (action_blocked, movement_blocked, self_movement_suppressed)
}

pub(crate) fn is_action_blocked(ctx: &ReducerContext, unit_guid: u64) -> bool {
    cc_blocks(
        is_stunned(ctx, unit_guid),
        is_rooted(ctx, unit_guid),
        is_feared(ctx, unit_guid),
        has_control_mechanic(ctx, unit_guid, M_POLY),
    )
    .0
}

pub(crate) fn is_movement_blocked(ctx: &ReducerContext, unit_guid: u64) -> bool {
    auras_on(ctx, unit_guid).any(|a| crate::modifiers::holds_in_place(a.eff_kind, a.eff_p0))
}

pub(crate) fn is_self_movement_suppressed(ctx: &ReducerContext, unit_guid: u64) -> bool {
    cc_blocks(
        is_stunned(ctx, unit_guid),
        is_rooted(ctx, unit_guid),
        is_feared(ctx, unit_guid),
        has_control_mechanic(ctx, unit_guid, M_POLY),
    )
    .2
}

pub(crate) fn breaks_on_damage(aura_interrupt: u16) -> bool {
    const BREAK_ON_DAMAGE: u16 = 0x1;
    aura_interrupt & BREAK_ON_DAMAGE != 0
}

pub(crate) fn breaks_on_move(aura_interrupt: u16) -> bool {
    const BREAK_ON_MOVE: u16 = 0x2; // game_spell.aura_interrupt bit 1
    aura_interrupt & BREAK_ON_MOVE != 0
}

/// Auras applied in this Durable Request survive its interruption effects.
fn break_flagged_auras(ctx: &ReducerContext, target_guid: u64, breaks: fn(u16) -> bool) {
    let spells = ctx.db.game_spell();
    let to_break: Vec<_> = auras_on(ctx, target_guid)
        .filter(|a| {
            a.applied_at != crate::time::now(ctx)
                && spells
                    .spell_id()
                    .find(a.spell_id)
                    .is_some_and(|s| breaks(s.aura_interrupt))
        })
        .collect();
    shed_auras(ctx, to_break);
}

pub(crate) fn break_auras_on_move(ctx: &ReducerContext, mover_guid: u64) {
    break_flagged_auras(ctx, mover_guid, breaks_on_move);
}

// attacker_guid is zero for direct spells and periodic damage, which cannot trigger Retaliation.
pub(crate) fn break_auras_on_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    attacker_guid: u64,
    periodic: bool,
) {
    // Hooks must observe damage before its crowd-control break effects.
    crate::hooks::fire_on_damage_taken(
        ctx,
        &crate::hooks::DamageTakenPayload {
            target_guid,
            attacker_guid,
            periodic,
        },
    );
    crate::world::stand_up_on_damage(ctx, target_guid);
    break_flagged_auras(ctx, target_guid, breaks_on_damage);
    super::break_stealth(ctx, target_guid);
    // Vanilla 1.12 periodic damage neither interrupts nor pushes back a cast.
    if !periodic {
        crate::spell::pushback_cast(ctx, target_guid);
    }
    if attacker_guid != 0 {
        crate::combat::retaliate_on_hit(ctx, target_guid, attacker_guid);
    }
}

// vmangos SharedDefines.h MECHANIC_SILENCE.
const MECHANIC_SILENCE: u8 = 9;
const SOURCE_MECHANIC_IMMUNITY_MASK: i32 = 147;
pub(super) const SOURCE_AURA_MASK: i32 = 0xff;
pub(super) const IMMUNE_TO_HOSTILE_AND_FRIENDLY: i32 = 0x100;
// vmangos SPELL_ATTR_NO_IMMUNITIES, kept in `attributes`.
pub(super) const SPELL_ATTR_NO_IMMUNITIES: u32 = 0x2000_0000;
// vmangos SPELL_ATTR_EX3_IGNORE_CASTER_AND_TARGET_RESTRICTIONS, kept in `attributes_ex_3`.
pub(super) const SPELL_ATTR_EX3_IGNORE_RESTRICTIONS: u32 = 0x1000_0000;

pub(crate) fn is_mechanic_immunity_mask(p1: i32) -> bool {
    p1 & SOURCE_AURA_MASK == SOURCE_MECHANIC_IMMUNITY_MASK
}

fn immunity_names_mechanic(source_aura: i32, p0: i32, mechanic: u8) -> bool {
    if source_aura == SOURCE_MECHANIC_IMMUNITY_MASK {
        (1..=32).contains(&mechanic) && p0 & (1 << (mechanic - 1)) != 0
    } else {
        p0 == i32::from(mechanic)
    }
}

fn aims_at_enemy(effect: &crate::SpellEffect) -> bool {
    matches!(effect.target, T_TARGET_ENEMY | T_AREA_ENEMY | T_CHAIN_ENEMY)
}

fn is_custom_positive(spell: &crate::Spell) -> bool {
    spell.custom_flags & lyracore_shared::spell_custom::POSITIVE != 0
}

// Approximates vmangos IsPositiveSpell, without SPELL_ATTR_AURA_IS_DEBUFF: docs/schema.md#mechanic-numbering.
pub(crate) fn spell_is_harmful(spell: &crate::Spell, effects: &[crate::SpellEffect]) -> bool {
    spell.negative() || (!is_custom_positive(spell) && effects.iter().any(aims_at_enemy))
}

pub(crate) fn effect_is_harmful(spell: &crate::Spell, effect: &crate::SpellEffect) -> bool {
    spell.negative() || (!is_custom_positive(spell) && aims_at_enemy(effect))
}

fn is_immune_to_mechanic(
    ctx: &ReducerContext,
    unit_guid: u64,
    mechanic: u8,
    harmful: bool,
) -> bool {
    mechanic != 0
        && auras_on(ctx, unit_guid).any(|aura| {
            if aura.eff_kind != A_IMMUNITY
                || aura.eff_p0_kind != P_MECHANIC
                || !immunity_names_mechanic(aura.eff_p1 & SOURCE_AURA_MASK, aura.eff_p0, mechanic)
            {
                return false;
            }
            let source = ctx.db.game_spell().spell_id().find(aura.spell_id);
            aura.eff_p1 & IMMUNE_TO_HOSTILE_AND_FRIENDLY != 0
                || match source.map(|source| !source.negative()) {
                    Some(helpful) => helpful == harmful,
                    None => !is_mechanic_immunity_mask(aura.eff_p1) || !harmful,
                }
        })
}

fn template_names_mechanic(ctx: &ReducerContext, unit_guid: u64, mechanic: u8) -> bool {
    (1..=32).contains(&mechanic)
        && ctx
            .db
            .game_world_entity()
            .guid()
            .find(unit_guid)
            .filter(|unit| !unit.is_player())
            .and_then(|unit| ctx.db.game_creature_template().entry().find(unit.entry))
            .is_some_and(|template| template.mechanic_immune_mask & (1 << (mechanic - 1)) != 0)
}

// vmangos `Creature::IsImmuneToSpell`: a creature's own cast skips its template mask.
fn template_is_immune_to_mechanic(
    ctx: &ReducerContext,
    caster_guid: u64,
    unit_guid: u64,
    mechanic: u8,
) -> bool {
    caster_guid != unit_guid && template_names_mechanic(ctx, unit_guid, mechanic)
}

/// vmangos `IsImmuneToSpell`, by header mechanic: docs/schema.md#mechanic-numbering.
pub(crate) fn is_immune_to_spell(
    ctx: &ReducerContext,
    caster_guid: u64,
    unit_guid: u64,
    spell: &crate::Spell,
    effects: &[crate::SpellEffect],
) -> bool {
    spell.attributes & SPELL_ATTR_NO_IMMUNITIES == 0
        && spell.attributes_ex_3 & SPELL_ATTR_EX3_IGNORE_RESTRICTIONS == 0
        && (template_is_immune_to_mechanic(ctx, caster_guid, unit_guid, spell.mechanic)
            || is_immune_to_mechanic(
                ctx,
                unit_guid,
                spell.mechanic,
                spell_is_harmful(spell, effects),
            ))
}

/// vmangos `Spell::CheckCast` refuses a helpful spell on a target that is immune to its header.
pub(crate) fn refuses_helpful_spell(
    ctx: &ReducerContext,
    caster_guid: u64,
    unit_guid: u64,
    spell: &crate::Spell,
    effects: &[crate::SpellEffect],
) -> bool {
    !spell_is_harmful(spell, effects)
        && is_immune_to_spell(ctx, caster_guid, unit_guid, spell, effects)
}

/// vmangos `IsImmuneToSpellEffect`, by effect mechanic: docs/schema.md#mechanic-numbering.
pub(crate) fn is_immune_to_effect(
    ctx: &ReducerContext,
    caster_guid: u64,
    unit_guid: u64,
    spell: &crate::Spell,
    effect: &crate::SpellEffect,
) -> bool {
    spell.attributes_ex_3 & SPELL_ATTR_EX3_IGNORE_RESTRICTIONS == 0
        && (template_is_immune_to_mechanic(ctx, caster_guid, unit_guid, effect.effect_mechanic)
            || is_immune_to_mechanic(
                ctx,
                unit_guid,
                effect.effect_mechanic,
                effect_is_harmful(spell, effect),
            ))
}

// Matches cmangos PUSHBACK_SPELL_DELAY_TIME.
pub(crate) const CAST_PUSHBACK_MS: i64 = 500;

pub(crate) const CAST_PUSHBACK_MAX: u8 = 2;

pub(crate) fn should_pushback(pushback_count: u8) -> bool {
    pushback_count < CAST_PUSHBACK_MAX
}

pub(crate) fn pushback_cast(ctx: &ReducerContext, caster_guid: u64) {
    let pending = ctx.db.game_pending_cast();
    let hits: Vec<PendingCast> = pending.by_caster().filter(&caster_guid).collect();
    for mut p in hits {
        if !should_pushback(p.pushback_count) {
            continue;
        }
        let ScheduleAt::Time(fire_at) = p.scheduled_at else {
            continue;
        };
        let Some(new_fire_at) = crate::time::scheduled_time(ctx, fire_at)
            .checked_add(TimeDuration::from_micros(CAST_PUSHBACK_MS * 1000))
        else {
            continue;
        };
        let spell_id = p.spell_id;
        p.pushback_count += 1;
        let new_count = p.pushback_count;
        p.scheduled_at = crate::time::schedule_time(ctx, new_fire_at);
        pending.scheduled_id().update(p);
        crate::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                delay_ms: CAST_PUSHBACK_MS as u32,
                ..SpellCastEvent::signal(ctx, caster_guid, spell_id, SpellCastEventKind::Pushback)
            },
        );
        log::info!(
            "cast pushed back: caster {caster_guid}'s spell {spell_id} delayed {CAST_PUSHBACK_MS}ms \
             (pushback {new_count}/{CAST_PUSHBACK_MAX})"
        );
    }
}

pub(crate) fn interrupt_cast(ctx: &ReducerContext, caster_guid: u64) -> bool {
    interrupt_cast_with_outcome(ctx, caster_guid, CastFinish::Cancelled)
}

fn interrupt_cast_with_outcome(
    ctx: &ReducerContext,
    caster_guid: u64,
    outcome: CastFinish,
) -> bool {
    let pending = ctx.db.game_pending_cast();
    let hits: Vec<(u64, u32, u64)> = pending
        .by_caster()
        .filter(&caster_guid)
        .map(|p| (p.scheduled_id, p.spell_id, p.target_guid))
        .collect();
    let interrupted = !hits.is_empty();
    // Clear the cancelled cast's GCD before hooks can start another cast.
    for (_, spell_id, _) in &hits {
        let Some(spell) = ctx.db.game_spell().spell_id().find(*spell_id) else {
            continue;
        };
        if spell.gcd_ms > 0 && spell.cast_flags & CAST_FLAG_CHANNELED == 0 {
            ctx.db
                .game_global_cooldown()
                .by_caster_category()
                .delete((caster_guid, spell.gcd_category));
        }
    }
    for (id, spell_id, target_guid) in hits {
        pending.scheduled_id().delete(id);
        clear_dead_callback_cast_admission(ctx, caster_guid, spell_id);
        crate::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                is_interrupted: true,
                failure_reason: CAST_FAIL_INTERRUPTED,
                ..SpellCastEvent::signal(ctx, caster_guid, spell_id, SpellCastEventKind::Interrupt)
            },
        );
        crate::hooks::fire_on_cast_finished(
            ctx,
            &crate::hooks::CastFinishedPayload {
                caster_guid,
                target_guid,
                scheduled_id: id,
                outcome: outcome.clone(),
            },
        );
    }
    if interrupted {
        log::info!("cast interrupted: caster {caster_guid}'s in-progress cast cancelled");
    }
    interrupted
}

pub(crate) fn cancel_cast_attempt(
    ctx: &ReducerContext,
    caster_guid: u64,
    scheduled_id: u64,
) -> bool {
    if pending_cast(ctx, caster_guid).is_none_or(|cast| cast.scheduled_id != scheduled_id) {
        return false;
    }
    interrupt_cast(ctx, caster_guid)
}

#[cfg_attr(not(all(has_packages, feature = "debug_reducers")), allow(dead_code))]
pub(crate) fn expire_cast_attempt(
    ctx: &ReducerContext,
    caster_guid: u64,
    scheduled_id: u64,
    deadline_micros: i64,
) -> bool {
    if crate::time::now(ctx).to_micros_since_unix_epoch() < deadline_micros
        || pending_cast(ctx, caster_guid).is_none_or(|cast| cast.scheduled_id != scheduled_id)
    {
        return false;
    }
    interrupt_cast_with_outcome(ctx, caster_guid, CastFinish::Expired)
}

pub(crate) fn break_channel(ctx: &ReducerContext, caster_guid: u64) -> bool {
    let auras = ctx.db.game_aura();
    let removed: Vec<Aura> = auras
        .by_caster()
        .filter(&caster_guid)
        .filter(|aura| is_channel_aura(ctx, aura))
        .collect();
    let broke_channel = !removed.is_empty();
    shed_auras(ctx, removed);
    let broke_cast = interrupt_cast(ctx, caster_guid);
    if broke_channel {
        log::info!("channel broken: caster {caster_guid}'s channel cancelled");
    }
    broke_channel || broke_cast
}

pub(crate) fn interrupt_cast_and_lock(
    ctx: &ReducerContext,
    target_guid: u64,
    lockout_ms: i64,
) -> bool {
    let pending = ctx.db.game_pending_cast();
    // Keep spell ids before deleting casts so the lockout can read their school headers.
    let spell_ids: Vec<u32> = pending
        .by_caster()
        .filter(&target_guid)
        .map(|p| p.spell_id)
        .collect();
    let interrupted = interrupt_cast(ctx, target_guid);
    // vmangos `Creature::LockOutSpells` skips a template immune to silence.
    if interrupted && !template_names_mechanic(ctx, target_guid, MECHANIC_SILENCE) {
        let until = crate::time::now(ctx)
            .checked_add(TimeDuration::from_micros(lockout_ms * 1000))
            .unwrap_or(crate::time::now(ctx));
        let locks = ctx.db.game_school_lockout();
        for spell_id in spell_ids {
            let Some(school) = ctx
                .db
                .game_spell()
                .spell_id()
                .find(spell_id)
                .map(|h| h.school_mask)
            else {
                continue;
            };
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

// vmangos IsDeathPersistentSpell reads SPELL_ATTR_EX3_ALLOW_AURA_WHILE_DEAD, 0x0010_0000.
pub(crate) fn aura_survives_death(spell: &crate::Spell) -> bool {
    spell.attributes & SPELL_ATTR_PASSIVE != 0 || spell.attributes_ex_3 & 0x0010_0000 != 0
}

pub(crate) fn remove_auras_on_death(ctx: &ReducerContext, unit_guid: u64) {
    let spells = ctx.db.game_spell();
    let shed: Vec<Aura> = auras_on(ctx, unit_guid)
        .filter(|a| {
            !spells
                .spell_id()
                .find(a.spell_id)
                .map(|s| aura_survives_death(&s) || (a.item_guid(ctx) != 0 && s.equip_passive()))
                .unwrap_or(false)
        })
        .collect();
    shed_auras(ctx, shed);
}

#[cfg(test)]
mod tests {
    use super::{
        aura_survives_death, cc_blocks, immunity_names_mechanic, CAST_PUSHBACK_MAX,
        CAST_PUSHBACK_MS, SPELL_ATTR_PASSIVE,
    };

    #[test]
    fn cc_blocks_baseline_uncontrolled_unit_blocks_nothing() {
        assert_eq!(cc_blocks(false, false, false, false), (false, false, false));
    }

    #[test]
    fn cc_blocks_stun_blocks_action_and_movement() {
        assert_eq!(cc_blocks(true, false, false, false), (true, true, true));
        assert_eq!(cc_blocks(true, true, true, true), (true, true, true));
    }

    #[test]
    fn cc_blocks_poly_blocks_action_and_movement_like_stun() {
        assert_eq!(cc_blocks(false, false, false, true), (true, true, true));
        assert_eq!(
            cc_blocks(false, false, false, true),
            cc_blocks(true, false, false, false)
        );
    }

    #[test]
    fn cc_blocks_root_blocks_movement_only() {
        assert_eq!(cc_blocks(false, true, false, false), (false, true, true));
    }

    #[test]
    fn cc_blocks_fear_blocks_action_and_self_movement_but_not_frozen_movement() {
        assert_eq!(cc_blocks(false, false, true, false), (true, false, true));
    }

    #[test]
    fn cc_blocks_root_and_fear_together_trip_every_axis() {
        assert_eq!(cc_blocks(false, true, true, false), (true, true, true));
    }

    #[test]
    fn pushback_constants_match_the_cmangos_convention() {
        assert_eq!(CAST_PUSHBACK_MS, 500);
        assert_eq!(CAST_PUSHBACK_MAX, 2);
    }

    #[test]
    fn death_sheds_every_aura_except_a_passive_or_death_persistent_one() {
        let spell = |attributes, attributes_ex_3| crate::Spell {
            attributes,
            attributes_ex_3,
            ..crate::Spell::blank(2457, "Battle Stance")
        };
        assert!(aura_survives_death(&spell(SPELL_ATTR_PASSIVE, 0)));
        assert!(aura_survives_death(&spell(
            SPELL_ATTR_PASSIVE | 0x4 | 0x10000,
            0
        )));
        assert!(aura_survives_death(&spell(0x0905_0010, 0x0010_0000)));

        assert!(!aura_survives_death(&spell(0, 0)));
        assert!(!aura_survives_death(&spell(0x4, 0x0008_0000)));
        assert!(!aura_survives_death(&spell(!SPELL_ATTR_PASSIVE, 0)));
    }

    #[test]
    fn a_mechanic_immunity_names_its_mechanic_and_a_mask_names_each_bit() {
        const MECHANIC_IMMUNITY: i32 = 77;
        const MECHANIC_IMMUNITY_MASK: i32 = 147;
        const CHARM: u8 = 1;
        const FEAR: u8 = 5;
        const SLEEP: u8 = 10;
        assert!(immunity_names_mechanic(MECHANIC_IMMUNITY, 5, FEAR));
        assert!(!immunity_names_mechanic(MECHANIC_IMMUNITY, 5, CHARM));
        let blue_dragon = 817;
        assert!(immunity_names_mechanic(
            MECHANIC_IMMUNITY_MASK,
            blue_dragon,
            CHARM
        ));
        assert!(immunity_names_mechanic(
            MECHANIC_IMMUNITY_MASK,
            blue_dragon,
            FEAR
        ));
        assert!(immunity_names_mechanic(
            MECHANIC_IMMUNITY_MASK,
            blue_dragon,
            SLEEP
        ));
        assert!(!immunity_names_mechanic(
            MECHANIC_IMMUNITY_MASK,
            blue_dragon,
            12
        ));
        assert!(!immunity_names_mechanic(MECHANIC_IMMUNITY_MASK, 1, 33));
    }
}
