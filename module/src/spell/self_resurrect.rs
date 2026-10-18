use lyracore_shared::packing::power_type;
use spacetimedb::{ReducerContext, Table};

use super::{
    auras_on, game_resurrect_request, game_self_resurrect_option, game_spell_effect,
    SelfResurrectOption, A_SELF_RESURRECT, E_SELF_RESURRECT,
};
use crate::game_world_entity;

// Record the option before death removes the Soulstone aura.
pub(crate) fn record_self_resurrect_option(ctx: &ReducerContext, character_guid: u64) {
    let chosen = auras_on(ctx, character_guid)
        .filter(|a| a.eff_kind == A_SELF_RESURRECT && a.eff_p0 > 0)
        .map(|a| a.eff_p0 as u32)
        .max()
        .or_else(|| package_reincarnation(ctx, character_guid));
    let options = ctx.db.game_self_resurrect_option();
    options.character_guid().delete(character_guid);
    if let Some(spell_id) = chosen {
        options.insert(SelfResurrectOption {
            character_guid,
            spell_id,
        });
    }
}

fn package_reincarnation(ctx: &ReducerContext, character_guid: u64) -> Option<u32> {
    const REINCARNATION_PASSIVE: u32 = 20608;
    const REINCARNATION_EFFECT: u32 = 21169;
    (crate::package_combat::essential_spell(ctx, character_guid, REINCARNATION_PASSIVE)
        && crate::spell::knows_spell(ctx, character_guid, REINCARNATION_PASSIVE)
        && ctx
            .db
            .game_spell_effect()
            .by_spell()
            .filter(REINCARNATION_EFFECT)
            .any(|effect| effect.kind == E_SELF_RESURRECT))
    .then_some(REINCARNATION_EFFECT)
}

pub(crate) fn do_self_resurrect(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut character = entities
        .guid()
        .find(character_guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !character.dead {
        return Err("caller is not dead".to_string());
    }
    crate::package_combat::resurrection_gate(ctx, character_guid)?;
    let option = ctx
        .db
        .game_self_resurrect_option()
        .character_guid()
        .find(character_guid)
        .ok_or_else(|| "no Self-Resurrection Option".to_string())?;
    let effect = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&option.spell_id)
        .find(|e| e.kind == E_SELF_RESURRECT)
        .ok_or_else(|| format!("spell {} has no self-resurrect effect", option.spell_id))?;

    let (health, power) = self_resurrect_vitals(
        character.max_health,
        character.max_power,
        power_type::for_class(character.class()),
        effect.base_points,
        effect.p0,
    );
    character.health = health;
    character.power = power;
    character.update(ctx);
    crate::ghost::finish_resurrection(ctx, character_guid);
    crate::package_combat::commit_resurrection(ctx, character_guid);
    Ok(())
}

pub(crate) fn self_resurrect_vitals(
    max_health: u32,
    max_power: u32,
    power_kind: u8,
    base_points: i32,
    flat_mana: i32,
) -> (u32, u32) {
    let percent_of =
        |max: u32| (u64::from(max) * u64::from(base_points.unsigned_abs()) / 100) as u32;
    let (health, mana) = if base_points < 0 {
        (base_points.unsigned_abs(), flat_mana.max(0) as u32)
    } else {
        (percent_of(max_health), percent_of(max_power))
    };
    let power = match power_kind {
        power_type::MANA => mana.min(max_power),
        power_type::ENERGY => max_power,
        _ => 0,
    };
    (health.min(max_health).max(1), power)
}

pub(crate) fn clear_resurrect_request(ctx: &ReducerContext, character_guid: u64) {
    ctx.db
        .game_resurrect_request()
        .target_guid()
        .delete(character_guid);
}

pub(crate) fn clear_resurrect_request_and_option(ctx: &ReducerContext, character_guid: u64) {
    clear_resurrect_request(ctx, character_guid);
    ctx.db
        .game_self_resurrect_option()
        .character_guid()
        .delete(character_guid);
}

#[cfg(test)]
mod tests {
    use super::self_resurrect_vitals;
    use lyracore_shared::packing::power_type::{ENERGY, MANA, RAGE};

    // Minor Soulstone 3026 stores raw base -401 for 400 health and 700 mana.
    #[test]
    fn a_negative_base_is_flat_health_and_flat_mana() {
        assert_eq!(
            self_resurrect_vitals(1000, 600, MANA, -400, 700),
            (400, 600)
        );
        assert_eq!(
            self_resurrect_vitals(1000, 2000, MANA, -400, 700),
            (400, 700)
        );
    }

    // Reincarnation 21169 stores raw base 19 for 20 percent of each maximum.
    #[test]
    fn a_positive_base_is_a_percent_of_max() {
        assert_eq!(self_resurrect_vitals(1000, 600, MANA, 20, 0), (200, 120));
    }

    #[test]
    fn health_stays_within_one_and_max() {
        assert_eq!(self_resurrect_vitals(0, 0, MANA, -400, 700), (1, 0));
        assert_eq!(self_resurrect_vitals(300, 0, MANA, -400, 700), (300, 0));
        assert_eq!(self_resurrect_vitals(1000, 600, MANA, 0, 0), (1, 0));
    }

    #[test]
    fn rage_comes_back_empty_and_energy_full() {
        assert_eq!(self_resurrect_vitals(1000, 1000, RAGE, -400, 700), (400, 0));
        assert_eq!(
            self_resurrect_vitals(1000, 100, ENERGY, -400, 700),
            (400, 100)
        );
        assert_eq!(self_resurrect_vitals(1000, 100, ENERGY, 20, 0), (200, 100));
    }
}
