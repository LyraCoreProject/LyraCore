use lyracore_shared::spell_custom as custom;
use spacetimedb::ReducerContext;

use super::*;
use crate::game_world_entity;

pub(crate) fn bonus_magnitude(
    points: i32,
    power: i32,
    coefficient: f32,
    flags: u32,
    periodic: bool,
) -> i32 {
    if flags & custom::FIXED_DAMAGE != 0 {
        points
    } else if coefficient >= 0.0 {
        points.saturating_add((power as f32 * coefficient) as i32)
    } else if periodic {
        points
    } else {
        compose_magnitude(points, power, SPELL_POWER_COEFF_PCT)
    }
}

pub(crate) fn mitigate_spell(
    ctx: &ReducerContext,
    hdr: &Spell,
    caster: u64,
    target: u64,
    damage: i32,
) -> i32 {
    if hdr.school_mask & RESIST_ARMOR != 0
        && hdr.custom_flags & custom::IGNORE_ARMOR == 0
        && damage > 0
    {
        if let (Some(caster), Some(target)) = (
            ctx.db.game_world_entity().guid().find(caster),
            ctx.db.game_world_entity().guid().find(target),
        ) {
            let percent = crate::combat::armor_mitigation_pct(
                crate::combat::effective_armor(ctx, &target),
                caster.level,
            );
            return (i64::from(damage) * i64::from(100 - percent) / 100).max(1) as i32;
        }
    }
    apply_resistance(ctx, target, caster, hdr.school_mask, damage)
}

pub(crate) fn aura_casters_coexist(hdr: &Spell, kind: u8) -> bool {
    hdr.custom_flags & custom::ALLOW_STACK_BETWEEN_CASTER == 0
        && (hdr.custom_flags & custom::SEPARATE_AURA_PER_CASTER != 0
            || hdr.cast_flags & SPELL_ATTR_CHANNELED != 0
            || matches!(kind, A_PERIODIC_DAMAGE | A_PERIODIC_HEAL | A_FLAG))
}

pub(crate) fn finish_aura_apply(
    ctx: &ReducerContext,
    hdr: &Spell,
    effect: &SpellEffect,
    caster: u64,
    target: u64,
) {
    if hdr.custom_flags & custom::SINGLE_TARGET_AURA != 0 {
        let spells = ctx.db.game_spell();
        let mut displaced = std::collections::BTreeMap::<u64, Vec<Aura>>::new();
        for aura in ctx.db.game_aura().by_caster().filter(&caster) {
            if aura.target_guid != target
                && spells.spell_id().find(aura.spell_id).is_some_and(|old| {
                    old.custom_flags & custom::SINGLE_TARGET_AURA != 0
                        && custom::single_target_conflicts(
                            hdr.single_target_key,
                            old.single_target_key,
                        )
                })
            {
                displaced.entry(aura.target_guid).or_default().push(aura);
            }
        }
        for (unit, auras) in displaced {
            shed_auras(ctx, unit, auras);
        }
    }
    if hdr.custom_flags & custom::AURA_APPLY_BREAKS_STEALTH != 0 {
        let shed = ctx
            .db
            .game_aura()
            .by_target()
            .filter(&target)
            .filter(|a| a.eff_kind == A_STEALTH && a.effect_id != effect.id)
            .collect();
        shed_auras(ctx, target, shed);
    }
}

pub(crate) fn remove_auras_on_evade(ctx: &ReducerContext, unit: u64) {
    let spells = ctx.db.game_spell();
    let shed = ctx
        .db
        .game_aura()
        .by_target()
        .filter(&unit)
        .filter(|a| {
            let player_buff = a.slot < 32
                && a.expires_at.to_micros_since_unix_epoch() != i64::MAX
                && crate::helpers::character_by_guid(ctx, a.caster_guid).is_some();
            !player_buff
                && spells
                    .spell_id()
                    .find(a.spell_id)
                    .is_none_or(|s| s.custom_flags & custom::NOT_REMOVED_ON_EVADE == 0)
        })
        .collect();
    shed_auras(ctx, unit, shed);
}

pub(crate) fn channel_target_in_range(ctx: &ReducerContext, aura: &Aura) -> bool {
    let Some(hdr) = ctx.db.game_spell().spell_id().find(aura.spell_id) else {
        // Frozen effects remain valid after their catalogue row is removed.
        return true;
    };
    if hdr.cast_flags & SPELL_ATTR_CHANNELED == 0 {
        return true;
    }
    // Periodic trigger holders live on the caster and carry the channel target separately.
    if aura.channel_target != 0
        && aura.target_guid != aura.channel_target
        && aura.eff_kind != A_PERIODIC_TRIGGER
    {
        return true;
    }
    let target = if aura.channel_target != 0 {
        aura.channel_target
    } else {
        aura.target_guid
    };
    channel_in_range(ctx, &hdr, aura.caster_guid, target)
}

fn channel_in_range(ctx: &ReducerContext, hdr: &Spell, caster: u64, target: u64) -> bool {
    let entities = ctx.db.game_world_entity();
    let (Some(caster), Some(target)) = (entities.guid().find(caster), entities.guid().find(target))
    else {
        return false;
    };
    if caster.dead
        || target.dead
        || caster.map_id != target.map_id
        || caster.instance_id != target.instance_id
    {
        return false;
    }
    if hdr.custom_flags & custom::CHAN_NO_DIST_LIMIT != 0 || caster.guid == target.guid {
        return true;
    }
    let limit = if crate::combat::may_harm(ctx, &caster, &target) {
        hdr.range_yd as f32 * 1.33
    } else {
        hdr.range_yd as f32 + 1.25
    };
    distance_3d(caster.x, caster.y, caster.z, target.x, target.y, target.z) <= limit
}

pub(crate) fn check_target_level(minimum: u32, actual: u32) -> Result<(), CastGateRefusal> {
    if actual < minimum {
        Err(CastGateRefusal::new(
            CastRefusalKind::TargetLevelTooLow,
            CastRefusalReason::TargetLevelTooLow,
            format!("target requires level {minimum}, has {actual}"),
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_coefficients_scale_direct_and_periodic_effects_and_zero_is_not_a_fallback() {
        assert_eq!(bonus_magnitude(10, 100, 0.5, 0, false), 60);
        assert_eq!(bonus_magnitude(10, 100, 0.5, 0, true), 60);
        assert_eq!(bonus_magnitude(10, 100, 0.0, 0, false), 10);
        assert_eq!(
            bonus_magnitude(10, 100, -1.0, 0, false),
            compose_magnitude(10, 100, SPELL_POWER_COEFF_PCT)
        );
        assert_eq!(bonus_magnitude(10, 100, -1.0, 0, true), 10);
    }

    #[test]
    fn fixed_damage_and_healing_ignore_explicit_and_fallback_bonuses() {
        for coefficient in [-1.0, 0.75] {
            assert_eq!(
                bonus_magnitude(25, 1000, coefficient, custom::FIXED_DAMAGE, false),
                25
            );
            assert_eq!(
                bonus_magnitude(25, 1000, coefficient, custom::FIXED_DAMAGE, true),
                25
            );
        }
    }

    #[test]
    fn minimum_target_level_returns_a_typed_refusal_at_the_boundary() {
        assert!(check_target_level(0, 1).is_ok());
        assert!(check_target_level(10, 10).is_ok());
        assert_eq!(
            check_target_level(10, 9).unwrap_err().refusal.kind,
            CastRefusalKind::TargetLevelTooLow
        );
    }

    #[test]
    fn cross_caster_stacks_override_separate_slots_but_ordinary_dots_coexist() {
        let mut hdr = crate::seed::base_spell(1, "test");
        assert!(aura_casters_coexist(&hdr, A_PERIODIC_DAMAGE));
        assert!(!aura_casters_coexist(&hdr, A_MOD_STAT));
        hdr.custom_flags = custom::SEPARATE_AURA_PER_CASTER;
        assert!(aura_casters_coexist(&hdr, A_MOD_STAT));
        hdr.custom_flags |= custom::ALLOW_STACK_BETWEEN_CASTER;
        assert!(!aura_casters_coexist(&hdr, A_MOD_STAT));
        assert!(!aura_casters_coexist(&hdr, A_PERIODIC_DAMAGE));
    }
}
