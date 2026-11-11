use spacetimedb::ReducerContext;

use crate::spell::SpellCastEventKind;
use crate::{game_spell, game_spell_effect, game_world_entity, WorldEntity};
use crate::{Spell, SpellCastEvent, SpellEffect};

#[derive(Clone, Copy, Default)]
pub(crate) struct QueuedStrikeDamage {
    pub(crate) bonus: u32,
    pub(crate) mod_flat: i32,
    pub(crate) mod_pct: i32,
}

pub(crate) fn queue(ctx: &ReducerContext, unit_guid: u64, spell_id: u32) {
    if let Some(mut unit) = ctx.db.game_world_entity().guid().find(unit_guid) {
        unit.next_swing_spell = spell_id;
        unit.update(ctx);
    }
}

/// Drops the queued strike on an entity the caller is about to write.
pub(crate) fn forget(unit: &mut WorldEntity) {
    unit.next_swing_spell = 0;
}

pub(crate) fn clear(ctx: &ReducerContext, guid: u64) {
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(guid) {
        if e.next_swing_spell != 0 {
            forget(&mut e);
            e.update(ctx);
        }
    }
}

// A Character pays when the swing fires, even on a miss; short of the cost, it swings white.
pub(crate) fn charge(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    queued_spell: u32,
    strike: Option<&Spell>,
) -> bool {
    if !attacker.is_player() {
        return true;
    }
    let Some(mut payer) = ctx.db.game_world_entity().guid().find(attacker.guid) else {
        return false;
    };
    if strike.is_some_and(|strike| payer.power < strike.cost) {
        SpellCastEvent::insert(
            ctx,
            SpellCastEvent {
                is_interrupted: true,
                failure_reason: crate::spell::CAST_FAIL_NO_POWER,
                ..SpellCastEvent::signal_at(
                    ctx,
                    attacker,
                    queued_spell,
                    SpellCastEventKind::Interrupt,
                )
            },
        );
        return false;
    }
    if let Some(strike) = strike {
        crate::spell::charge_spell_cost(ctx, &mut payer, strike);
    }
    payer.update(ctx);
    true
}

pub(crate) fn damage(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    strike: &Spell,
) -> QueuedStrikeDamage {
    let (mod_flat, mod_pct) =
        crate::spell::spell_mod(ctx, attacker.guid, crate::spell::SPELLMOD_OP_DAMAGE, strike);
    QueuedStrikeDamage {
        bonus: bonus(ctx, attacker, strike),
        mod_flat,
        mod_pct,
    }
}

// vmangos interrupts a form-bound queued strike whenever its form is lost, even for a sibling form.
pub(crate) fn interrupt_form_bound(ctx: &ReducerContext, unit: &WorldEntity) {
    let Some(strike) = ctx.db.game_spell().spell_id().find(unit.next_swing_spell) else {
        return;
    };
    if strike.stances != 0 {
        interrupt(ctx, unit);
    }
}

// AttackStop and CMSG_CANCEL_CAST interrupt it; vmangos `Spell::cancel` reports the interruption.
pub(crate) fn interrupt_of(ctx: &ReducerContext, guid: u64) {
    if let Some(unit) = ctx.db.game_world_entity().guid().find(guid) {
        interrupt(ctx, &unit);
    }
}

fn interrupt(ctx: &ReducerContext, unit: &WorldEntity) {
    if unit.next_swing_spell == 0 {
        return;
    }
    clear(ctx, unit.guid);
    SpellCastEvent::insert(
        ctx,
        SpellCastEvent {
            is_interrupted: true,
            failure_reason: crate::spell::CAST_FAIL_INTERRUPTED,
            ..SpellCastEvent::signal_at(
                ctx,
                unit,
                unit.next_swing_spell,
                SpellCastEventKind::Interrupt,
            )
        },
    );
}

fn bonus(ctx: &ReducerContext, attacker: &WorldEntity, strike: &Spell) -> u32 {
    let Some(effect) = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&strike.spell_id)
        .find(|e| e.kind == crate::spell::E_NEXT_SWING)
    else {
        return 0;
    };
    bonus_points(&effect, strike, attacker.level, ctx.random::<u32>())
}

fn bonus_points(effect: &SpellEffect, strike: &Spell, attacker_level: u32, roll: u32) -> u32 {
    crate::spell::effect_amount(
        effect.base_points,
        effect.die_sides,
        effect.per_level,
        attacker_level,
        strike.spell_level,
        strike.max_level,
        roll,
    )
    .max(0) as u32
}

#[cfg(test)]
mod tests {
    use super::bonus_points;
    use crate::{Spell, SpellEffect};

    fn bonus(base_points: i32, die_sides: i32, per_level: f32) -> crate::SpellEffect {
        crate::SpellEffect {
            base_points,
            die_sides,
            per_level,
            ..SpellEffect::blank(1, 0)
        }
    }

    fn strike(spell_level: u8) -> crate::Spell {
        crate::Spell {
            spell_level,
            ..Spell::blank(1, "Strike")
        }
    }

    #[test]
    fn heroic_strike_rank_one_adds_its_flat_eleven() {
        assert_eq!(bonus_points(&bonus(11, 1, 0.0), &strike(1), 60, 7), 11);
    }

    #[test]
    fn a_creature_strike_scales_with_the_attacker_level() {
        // Strike 11976: 5 points plus 1.5 per level above 1; level 60 adds 5 + 88.
        assert_eq!(bonus_points(&bonus(5, 1, 1.5), &strike(1), 60, 0), 93);
    }
}
