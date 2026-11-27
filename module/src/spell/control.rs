use spacetimedb::{log, table, ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp};

use super::tables::DamageResponse;
use super::*;
use crate::{
    game_aura, game_creature_template, game_global_cooldown, game_ground_area, game_pending_cast,
    game_school_lockout, game_spell, game_spell_effect, game_world_entity, Aura, GroundArea,
    PendingCast, SchoolLockout, SpellCastEvent,
};

/// The delays damage has made to a caster's channel, as vmangos `Spell::m_delayAtDamageCount`.
#[table(accessor = game_channel_delay)]
pub struct ChannelDelay {
    #[primary_key]
    pub caster_guid: u64,
    pub spell_id: u32,
    // A channel that no longer ends at this time starts again from no delays.
    pub ends_at: Timestamp,
    pub delays: u8,
}

crate::outside_table_catalog!(game_channel_delay);

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
        || channel_auras(ctx, unit_guid).next().is_some()
        || channelled_areas(ctx, unit_guid).next().is_some()
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
fn break_flagged_auras(ctx: &ReducerContext, target_guid: u64, breaks: fn(&crate::Spell) -> bool) {
    let spells = ctx.db.game_spell();
    let to_break: Vec<_> = auras_on(ctx, target_guid)
        .filter(|a| {
            a.applied_at != crate::time::now(ctx)
                && spells
                    .spell_id()
                    .find(a.spell_id)
                    .is_some_and(|s| breaks(&s))
        })
        .collect();
    shed_auras(ctx, to_break);
}

pub(crate) fn break_auras_on_move(ctx: &ReducerContext, mover_guid: u64) {
    break_flagged_auras(ctx, mover_guid, |spell| {
        breaks_on_move(spell.aura_interrupt)
    });
}

// attacker_guid is zero for direct spells and periodic damage, which cannot trigger Retaliation.
pub(crate) fn break_auras_on_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    attacker_guid: u64,
    periodic: bool,
) {
    react_to_damage(ctx, target_guid, attacker_guid, periodic, |spell| {
        breaks_on_damage(spell.aura_interrupt)
    });
}

/// For a hit absorbed whole; vmangos checkProcFlags keeps an Aura whose spell has proc flags.
pub(crate) fn break_auras_on_absorbed_hit(
    ctx: &ReducerContext,
    target_guid: u64,
    attacker_guid: u64,
    periodic: bool,
) {
    react_to_damage(ctx, target_guid, attacker_guid, periodic, |spell| {
        breaks_on_damage(spell.aura_interrupt) && spell.proc_flags == 0
    });
}

fn react_to_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    attacker_guid: u64,
    periodic: bool,
    breaks: fn(&crate::Spell) -> bool,
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
    break_flagged_auras(ctx, target_guid, breaks);
    super::break_stealth(ctx, target_guid);
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

// vmangos SpellModOp SPELLMOD_NOT_LOSE_CASTING_TIME.
const SPELLMOD_OP_NOT_LOSE_CASTING_TIME: i32 = 9;
// vmangos AuraType SPELL_AURA_RESIST_PUSHBACK, kept in an A_FLAG effect's p1.
const AURA_RESIST_PUSHBACK: i32 = 149;

/// vmangos `Unit::DealDamage`: a direct hit cancels or delays the victim's cast and channel.
pub(crate) fn pushback_on_damage(ctx: &ReducerContext, victim_guid: u64, attacker_guid: u64) {
    let Some(victim) = ctx.db.game_world_entity().guid().find(victim_guid) else {
        return;
    };
    if victim.is_player() {
        pushback_cast(ctx, victim_guid);
    }
    // vmangos delays only a Character's channel, and not for damage it deals itself.
    let may_delay = victim.is_player() && attacker_guid != victim_guid;
    for spell_id in channel_spell_ids(ctx, victim_guid) {
        let Some(spell) = ctx.db.game_spell().spell_id().find(spell_id) else {
            continue;
        };
        match spell.channel_damage_response() {
            DamageResponse::Delayed if may_delay => delay_channel(ctx, victim_guid, &spell),
            DamageResponse::Cancelled => {
                end_channel(ctx, victim_guid, spell_id);
            }
            DamageResponse::Delayed | DamageResponse::Unaffected => {}
        }
    }
}

/// vmangos `Unit::DealDamage` for a non-DOT hit a shield absorbs whole: it cancels a Character's
/// cast and channel whose spell says damage cancels it, and delays nothing.
pub(crate) fn interrupt_on_absorbed_hit(ctx: &ReducerContext, victim_guid: u64) {
    if !ctx
        .db
        .game_world_entity()
        .guid()
        .find(victim_guid)
        .is_some_and(|victim| victim.is_player())
    {
        return;
    }
    let cancels = |spell_id: u32| {
        ctx.db
            .game_spell()
            .spell_id()
            .find(spell_id)
            .is_some_and(|spell| spell.cancelled_by_absorbed_damage())
    };
    if ctx
        .db
        .game_pending_cast()
        .by_caster()
        .filter(&victim_guid)
        .any(|cast| cancels(cast.spell_id))
    {
        interrupt_cast(ctx, victim_guid);
    }
    for spell_id in channel_spell_ids(ctx, victim_guid) {
        if cancels(spell_id) {
            end_channel(ctx, victim_guid, spell_id);
        }
    }
}

fn pushback_cast(ctx: &ReducerContext, caster_guid: u64) {
    let pending = ctx.db.game_pending_cast();
    let hits: Vec<PendingCast> = pending.by_caster().filter(&caster_guid).collect();
    let now_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    for mut p in hits {
        let Some(spell) = ctx.db.game_spell().spell_id().find(p.spell_id) else {
            continue;
        };
        match spell.cast_damage_response() {
            DamageResponse::Cancelled => {
                interrupt_cast(ctx, caster_guid);
                return;
            }
            DamageResponse::Unaffected => continue,
            DamageResponse::Delayed => {}
        }
        if resists_pushback(ctx, caster_guid, &spell) {
            continue;
        }
        let ScheduleAt::Time(fire_at) = p.scheduled_at else {
            continue;
        };
        let fire_at = crate::time::scheduled_time(ctx, fire_at);
        let delay = cast_pushback_micros(
            p.pushback_count,
            fire_at.to_micros_since_unix_epoch() - now_micros,
            i64::from(p.cast_time_ms) * 1000,
        );
        let Some(new_fire_at) = fire_at.checked_add(TimeDuration::from_micros(delay)) else {
            continue;
        };
        let spell_id = p.spell_id;
        p.pushback_count = p.pushback_count.saturating_add(1);
        let new_count = p.pushback_count;
        p.scheduled_at = crate::time::schedule_time(ctx, new_fire_at);
        pending.scheduled_id().update(p);
        let delay_ms = (delay / 1000) as u32;
        crate::SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                delay_ms,
                ..SpellCastEvent::signal(ctx, caster_guid, spell_id, SpellCastEventKind::Pushback)
            },
        );
        log::info!(
            "cast pushed back: caster {caster_guid}'s spell {spell_id} delayed {delay_ms}ms \
             (pushback {new_count})"
        );
    }
}

/// vmangos `Spell::Delayed` and `Spell::DelayedChannel` resist roll.
fn resists_pushback(ctx: &ReducerContext, caster_guid: u64, spell: &crate::Spell) -> bool {
    let (flat, pct) = spell_mod(ctx, caster_guid, SPELLMOD_OP_NOT_LOSE_CASTING_TIME, spell);
    let from_auras: i64 = auras_on(ctx, caster_guid)
        .filter(|aura| aura.eff_kind == A_FLAG && aura.eff_p1 == AURA_RESIST_PUSHBACK)
        .map(|aura| i64::from(crate::modifiers::stacked_amount(aura.amount, aura.stacks)))
        .sum();
    let chance = i64::from(apply_spell_mod(100, flat, pct)) - 100 + from_auras;
    chance >= 100
        || (chance > 0 && i64::from(super::rolls::integer_roll(ctx, caster_guid, 99)) < chance)
}

/// vmangos `Spell::GetNextDelayAtDamageMsTime`, in microseconds.
fn delay_at_damage_micros(delays: u8) -> i64 {
    1_000_000 - 200_000 * i64::from(delays.min(4))
}

/// vmangos `Spell::Delayed`: a cast's time left never grows past its full cast time. A cast time
/// of 0 is unknown, from a cast in flight at the publish that added it, and caps nothing.
fn cast_pushback_micros(delays: u8, remaining_micros: i64, cast_time_micros: i64) -> i64 {
    let delay = delay_at_damage_micros(delays);
    if cast_time_micros == 0 {
        delay
    } else {
        delay.min((cast_time_micros - remaining_micros).max(0))
    }
}

/// vmangos `Aura::UpdatePeriodicTimer`: the next tick falls so that the last one ends the channel.
fn realigned_next_tick(now_micros: i64, remaining_micros: i64, period_micros: i64) -> i64 {
    let first = if remaining_micros > period_micros {
        remaining_micros % period_micros
    } else {
        remaining_micros
    };
    now_micros + if first == 0 { period_micros } else { first }
}

/// vmangos `Spell::DelayedChannel`: shortens the channel's Auras and ground areas, and ends the
/// channel when no time remains.
fn delay_channel(ctx: &ReducerContext, caster_guid: u64, spell: &crate::Spell) {
    let spell_id = spell.spell_id;
    let auras: Vec<Aura> = channel_auras(ctx, caster_guid)
        .filter(|aura| aura.spell_id == spell_id)
        .collect();
    let areas: Vec<GroundArea> = channelled_areas(ctx, caster_guid)
        .filter(|area| area.spell_id == spell_id)
        .collect();
    let Some(ends_at) = auras
        .iter()
        .map(|aura| aura.expires_at)
        .chain(areas.iter().map(|area| area.expires_at))
        .max()
    else {
        return;
    };
    let end_micros = ends_at.to_micros_since_unix_epoch();
    // i64::MAX marks a channel with no end.
    if end_micros == i64::MAX || resists_pushback(ctx, caster_guid, spell) {
        return;
    }
    let now_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    let remaining = end_micros.saturating_sub(now_micros).max(0);
    let delays = ctx
        .db
        .game_channel_delay()
        .caster_guid()
        .find(caster_guid)
        .filter(|row| row.spell_id == spell_id && row.ends_at == ends_at)
        .map_or(0, |row| row.delays);
    let delay = delay_at_damage_micros(delays).min(remaining);
    if delay == remaining {
        end_channel(ctx, caster_guid, spell_id);
        return;
    }
    let shortened = |expires_at: Timestamp| {
        let expires = expires_at.to_micros_since_unix_epoch();
        expires.saturating_sub(delay).max(now_micros)
    };
    for mut aura in auras {
        let expires = shortened(aura.expires_at);
        aura.expires_at = Timestamp::from_micros_since_unix_epoch(expires);
        if aura.period_ms > 0 && aura.next_tick_micros != 0 {
            aura.next_tick_micros = realigned_next_tick(
                now_micros,
                expires - now_micros,
                i64::from(aura.period_ms) * 1000,
            );
        }
        aura.update(ctx);
    }
    for mut area in areas {
        let expires = shortened(area.expires_at);
        area.expires_at = Timestamp::from_micros_since_unix_epoch(expires);
        if area.period_ms > 0 {
            area.next_tick_micros = realigned_next_tick(
                now_micros,
                expires - now_micros,
                i64::from(area.period_ms) * 1000,
            );
        }
        ctx.db.game_ground_area().id().update(area);
    }
    let channel_delays = ctx.db.game_channel_delay();
    channel_delays.caster_guid().delete(caster_guid);
    channel_delays.insert(ChannelDelay {
        caster_guid,
        spell_id,
        ends_at: Timestamp::from_micros_since_unix_epoch(end_micros - delay),
        delays: delays.saturating_add(1),
    });
    let remaining_ms = (remaining - delay) / 1000;
    crate::native::aura::channel_updated(
        ctx,
        caster_guid,
        remaining_ms.min(i64::from(u32::MAX)) as u32,
    );
    log::info!(
        "channel delayed: caster {caster_guid}'s spell {spell_id} shortened {}ms, {remaining_ms}ms left",
        delay / 1000
    );
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

fn channel_auras(ctx: &ReducerContext, caster_guid: u64) -> impl Iterator<Item = Aura> + '_ {
    ctx.db
        .game_aura()
        .by_caster()
        .filter(caster_guid)
        .filter(move |aura| is_channel_aura(ctx, aura))
}

fn channelled_areas(
    ctx: &ReducerContext,
    caster_guid: u64,
) -> impl Iterator<Item = GroundArea> + '_ {
    ctx.db
        .game_ground_area()
        .by_caster()
        .filter(caster_guid)
        .filter(move |area| super::cast::is_channelled_area(ctx, area))
}

pub(crate) fn break_channel(ctx: &ReducerContext, caster_guid: u64) -> bool {
    let removed: Vec<Aura> = channel_auras(ctx, caster_guid).collect();
    let channelled_areas: Vec<GroundArea> = channelled_areas(ctx, caster_guid).collect();
    let broke_channel = !removed.is_empty() || !channelled_areas.is_empty();
    ctx.db
        .game_channel_delay()
        .caster_guid()
        .delete(caster_guid);
    shed_auras(ctx, removed);
    for area in &channelled_areas {
        super::cast::end_ground_area(ctx, area);
    }
    let broke_cast = interrupt_cast(ctx, caster_guid);
    let left_ritual = crate::summoning_ritual::leave(ctx, caster_guid);
    if broke_channel {
        log::info!("channel broken: caster {caster_guid}'s channel cancelled");
    }
    broke_channel || broke_cast || left_ritual
}

/// Ends the caster's channel of one spell, leaving any cast in progress; returns whether it had one.
pub(crate) fn end_channel(ctx: &ReducerContext, caster_guid: u64, spell_id: u32) -> bool {
    let auras: Vec<Aura> = channel_auras(ctx, caster_guid)
        .filter(|aura| aura.spell_id == spell_id)
        .collect();
    let areas: Vec<GroundArea> = channelled_areas(ctx, caster_guid)
        .filter(|area| area.spell_id == spell_id)
        .collect();
    let ended = !auras.is_empty() || !areas.is_empty();
    ctx.db
        .game_channel_delay()
        .caster_guid()
        .delete(caster_guid);
    shed_auras(ctx, auras);
    for area in &areas {
        super::cast::end_ground_area(ctx, area);
    }
    let left_ritual = crate::summoning_ritual::channel_spell(ctx, caster_guid) == Some(spell_id)
        && crate::summoning_ritual::leave(ctx, caster_guid);
    if ended {
        log::info!("channel ended: caster {caster_guid}'s spell {spell_id} channel cancelled");
    }
    ended || left_ritual
}

/// Spell ids of a unit's channel Auras, channelled ground areas and Summoning Ritual channel.
fn channel_spell_ids(ctx: &ReducerContext, unit_guid: u64) -> Vec<u32> {
    let mut spell_ids: Vec<u32> = channel_auras(ctx, unit_guid)
        .map(|aura| aura.spell_id)
        .chain(channelled_areas(ctx, unit_guid).map(|area| area.spell_id))
        .chain(crate::summoning_ritual::channel_spell(ctx, unit_guid))
        .collect();
    spell_ids.sort_unstable();
    spell_ids.dedup();
    spell_ids
}

/// vmangos `Spell::EffectInterruptCast`: ends the target's cast and channel that yield to an
/// interrupt effect and locks the school of each.
pub(crate) fn interrupt_cast_and_lock(
    ctx: &ReducerContext,
    target_guid: u64,
    lockout_ms: i64,
) -> bool {
    let yields = |spell_id: &u32| {
        ctx.db
            .game_spell()
            .spell_id()
            .find(*spell_id)
            .is_some_and(|spell| spell.yields_to_interrupt_effect())
    };
    let cast_ids: Vec<u32> = ctx
        .db
        .game_pending_cast()
        .by_caster()
        .filter(&target_guid)
        .map(|cast| cast.spell_id)
        .filter(|spell_id| yields(spell_id))
        .collect();
    let mut stopped: Vec<u32> = Vec::new();
    if !cast_ids.is_empty() && interrupt_cast(ctx, target_guid) {
        stopped.extend(cast_ids);
    }
    for spell_id in channel_spell_ids(ctx, target_guid) {
        if yields(&spell_id) && end_channel(ctx, target_guid, spell_id) {
            stopped.push(spell_id);
        }
    }
    let interrupted = !stopped.is_empty();
    // vmangos `Creature::LockOutSpells` skips a template immune to silence.
    if interrupted && !template_names_mechanic(ctx, target_guid, MECHANIC_SILENCE) {
        let until = crate::time::now(ctx)
            .checked_add(TimeDuration::from_micros(lockout_ms * 1000))
            .unwrap_or(crate::time::now(ctx));
        let locks = ctx.db.game_school_lockout();
        for spell_id in stopped {
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
            "kick: caster {target_guid}'s cast or channel ended + school locked for {lockout_ms}ms"
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
        aura_survives_death, cast_pushback_micros, cc_blocks, delay_at_damage_micros,
        immunity_names_mechanic, realigned_next_tick, SPELL_ATTR_PASSIVE,
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
    fn each_hit_delays_a_cast_or_channel_two_hundred_ms_less_down_to_two_hundred() {
        let delays: Vec<i64> = (0..7).map(delay_at_damage_micros).collect();
        assert_eq!(
            delays,
            [1_000_000, 800_000, 600_000, 400_000, 200_000, 200_000, 200_000]
        );
    }

    #[test]
    fn each_hit_on_a_long_cast_pushes_it_back_by_the_next_delay_with_no_limit_on_hits() {
        const CAST_TIME: i64 = 10_000_000;
        let pushbacks: Vec<i64> = (0..7)
            .map(|hits| cast_pushback_micros(hits, 1_000_000, CAST_TIME))
            .collect();
        assert_eq!(
            pushbacks,
            [1_000_000, 800_000, 600_000, 400_000, 200_000, 200_000, 200_000]
        );
    }

    #[test]
    fn a_pushback_never_leaves_more_than_the_full_cast_time() {
        // Fireball: a 3500 ms cast hit at 2900 ms left can grow only 600 ms.
        assert_eq!(cast_pushback_micros(0, 2_900_000, 3_500_000), 600_000);
        assert_eq!(cast_pushback_micros(1, 3_500_000, 3_500_000), 0);
        assert_eq!(cast_pushback_micros(0, 2_000_000, 3_500_000), 1_000_000);
    }

    #[test]
    fn a_cast_without_a_known_cast_time_takes_the_full_pushback() {
        assert_eq!(cast_pushback_micros(0, 2_900_000, 0), 1_000_000);
        assert_eq!(cast_pushback_micros(2, 2_900_000, 0), 600_000);
    }

    #[test]
    fn a_delayed_channel_ticks_so_that_its_last_tick_falls_on_its_new_end() {
        const NOW: i64 = 5_000_000;
        const PERIOD: i64 = 1_000_000;
        assert_eq!(realigned_next_tick(NOW, 2_500_000, PERIOD), NOW + 500_000);
        assert_eq!(realigned_next_tick(NOW, 3_000_000, PERIOD), NOW + PERIOD);
        assert_eq!(realigned_next_tick(NOW, 400_000, PERIOD), NOW + 400_000);
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
