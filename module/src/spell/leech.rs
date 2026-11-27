use std::collections::HashMap;

use spacetimedb::ReducerContext;

use super::{healed_value, spell_mod};
use crate::{game_spell, Aura, WorldEntity};

// vmangos SpellModOp SPELLMOD_MULTIPLE_VALUE.
const SPELLMOD_OP_MULTIPLE_VALUE: i32 = 27;

// A leech Aura holds its multiple value in hundredths in `eff_p1`; docs/schema.md#health-leech-auras.
pub(super) fn is_leech(aura: &Aura) -> bool {
    aura.eff_p1 > 0
}

fn heal_for_damage(dealt: u32, multiplier: f32) -> u32 {
    (dealt as f32 * multiplier) as u32
}

fn multiplier(ctx: &ReducerContext, aura: &Aura, caster_guid: u64) -> f32 {
    let base = aura.eff_p1 as f32 / 100.0;
    let (flat, percent) = ctx
        .db
        .game_spell()
        .spell_id()
        .find(aura.spell_id)
        .map_or((0, 0), |header| {
            spell_mod(ctx, caster_guid, SPELLMOD_OP_MULTIPLE_VALUE, &header)
        });
    (base + flat as f32) * (100 + percent) as f32 / 100.0
}

/// vmangos `Aura::PeriodicTick` leech branch: heals the caster by `dealt` times the multiple and
/// adds the heal threat. Returns the amount healed before the health cap.
pub(super) fn heal_caster(
    ctx: &ReducerContext,
    aura: &Aura,
    caster: &WorldEntity,
    dealt: u32,
    health: &mut HashMap<u64, u32>,
) -> u32 {
    let heal = heal_for_damage(dealt, multiplier(ctx, aura, caster.guid));
    let current = health.get(&caster.guid).copied().unwrap_or(caster.health);
    let healed = healed_value(
        current,
        caster.max_health,
        i32::try_from(heal).unwrap_or(i32::MAX),
    );
    health.insert(caster.guid, healed);
    crate::threat::add_spell_heal_threat(
        ctx,
        caster.guid,
        caster.guid,
        healed.saturating_sub(current),
        aura.spell_id,
        crate::threat::HealDelivery::Periodic,
    );
    heal
}

#[cfg(test)]
mod tests {
    use super::heal_for_damage;

    #[test]
    fn a_multiple_of_one_heals_the_damage_dealt() {
        assert_eq!(heal_for_damage(30, 1.0), 30);
    }

    #[test]
    fn the_heal_truncates_a_fractional_result() {
        assert_eq!(heal_for_damage(7, 0.5), 3);
        assert_eq!(heal_for_damage(30, 1.5), 45);
    }

    #[test]
    fn no_damage_dealt_heals_nothing() {
        assert_eq!(heal_for_damage(0, 25.0), 0);
    }
}
