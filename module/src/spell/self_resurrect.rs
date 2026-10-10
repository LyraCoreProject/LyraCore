//! Self-resurrection: the Self-Resurrection Option a dead Character holds, and its use.
//!
//! The option is chosen once, at death, from the Character's `A_SELF_RESURRECT` auras (a Soulstone).
//! Those auras do not survive death, so the option row is the only record that remains. Using the
//! option reads the named spell's `E_SELF_RESURRECT` effect and revives the Character in place, with
//! no Resurrection Sickness.

use lyracore_shared::constants::{player_flags, unit_vis_flags};
use lyracore_shared::packing::power_type;
use spacetimedb::{ReducerContext, Table};

use super::{
    auras_on, game_resurrect_request, game_self_resurrect_option, game_spell_effect,
    SelfResurrectOption, A_SELF_RESURRECT, E_SELF_RESURRECT,
};
use crate::{game_corpse, game_world_entity};

/// Choose `character_guid`'s Self-Resurrection Option from its auras. Call it at death, before the
/// death aura shed removes the Soulstone aura. The highest `A_SELF_RESURRECT` spell wins. Without one,
/// any row left from an earlier death is deleted, so a stale option never outlives its source.
pub(crate) fn record_self_resurrect_option(ctx: &ReducerContext, character_guid: u64) {
    let chosen = auras_on(ctx, character_guid)
        .filter(|a| a.eff_kind == A_SELF_RESURRECT && a.eff_p0 > 0)
        .map(|a| a.eff_p0 as u32)
        .max();
    let options = ctx.db.game_self_resurrect_option();
    options.character_guid().delete(character_guid);
    if let Some(spell_id) = chosen {
        options.insert(SelfResurrectOption {
            character_guid,
            spell_id,
        });
    }
}

/// Use `character_guid`'s Self-Resurrection Option: revive in place at the option spell's vitals,
/// clear the dead and ghost state, remove the corpse, and spend the option. One transaction. A
/// Refusal (not in the world, not dead, no option, or an option spell without an
/// `E_SELF_RESURRECT` effect) changes nothing.
pub(crate) fn do_self_resurrect(ctx: &ReducerContext, character_guid: u64) -> Result<(), String> {
    let entities = ctx.db.game_world_entity();
    let mut character = entities
        .guid()
        .find(character_guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !character.dead {
        return Err("caller is not dead".to_string());
    }
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
    character.dead = false;
    character.player_flags &= !player_flags::GHOST;
    character.unit_bytes_1 &= !unit_vis_flags::GHOST;
    entities.guid().update(character);
    ctx.db
        .game_corpse()
        .guid()
        .delete(crate::corpse::corpse_guid_for(character_guid));
    clear_resurrect_request_and_option(ctx, character_guid);
    Ok(())
}

/// The health and power a self-resurrection restores, per the `E_SELF_RESURRECT` rule: a negative
/// `base_points` is a flat health amount with `flat_mana` mana, otherwise a percent of max health and
/// max mana. Health lands in `1..=max_health`. A mana user gets the computed mana capped at its max,
/// an energy user a full bar, and every other power type 0.
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

/// Delete `character_guid`'s pending resurrect request from another Character. Release Spirit uses
/// this alone: a ghost keeps its Self-Resurrection Option.
pub(crate) fn clear_resurrect_request(ctx: &ReducerContext, character_guid: u64) {
    ctx.db
        .game_resurrect_request()
        .target_guid()
        .delete(character_guid);
}

/// Delete both ways back `character_guid` may hold: the resurrect request and the
/// Self-Resurrection Option. Every resurrection and leaving the world use this.
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

    /// Minor Soulstone (3026): raw base -401, so 400 health and its 700 mana, capped at max mana.
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

    /// Reincarnation (21169): raw base 19, so 20 percent of max health and max mana.
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
