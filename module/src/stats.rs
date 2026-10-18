use lyracore_shared::packing::power_type;
use spacetimedb::{table, ReducerContext};

use crate::{game_character, game_world_entity, WorldEntity};

#[table(accessor = game_class_level_stats, public)]
pub struct ClassLevelStats {
    #[primary_key]
    pub class_level: u32,
    pub class: u8,
    pub level: u32,
    pub base_health: u32,
    pub base_mana: u32,
}

#[table(accessor = game_level_stats, public)]
pub struct LevelStats {
    #[primary_key]
    pub race_class_level: u32,
    pub race: u8,
    pub class: u8,
    pub level: u32,
    pub strength: u32,
    pub agility: u32,
    pub stamina: u32,
    pub intellect: u32,
    pub spirit: u32,
}

pub(crate) fn class_level_key(class: u8, level: u32) -> u32 {
    ((class as u32) << 8) | (level & 0xFF)
}
pub(crate) fn race_class_level_key(race: u8, class: u8, level: u32) -> u32 {
    ((race as u32) << 16) | ((class as u32) << 8) | (level & 0xFF)
}

pub(crate) fn hp_from_stamina(stamina: u32) -> u32 {
    if stamina <= 20 {
        stamina
    } else {
        20 + (stamina - 20) * 10
    }
}

pub(crate) fn mana_from_intellect(intellect: u32) -> u32 {
    if intellect <= 20 {
        intellect
    } else {
        20 + (intellect - 20) * 15
    }
}

pub(crate) fn max_health_for(ctx: &ReducerContext, race: u8, class: u8, level: u32) -> u32 {
    let base = ctx
        .db
        .game_class_level_stats()
        .class_level()
        .find(class_level_key(class, level));
    let attrs = ctx
        .db
        .game_level_stats()
        .race_class_level()
        .find(race_class_level_key(race, class, level));
    match (base, attrs) {
        (Some(b), Some(a)) => b.base_health + hp_from_stamina(a.stamina),
        _ => crate::xp::max_health_for_level(level),
    }
}

pub(crate) fn max_power_for(ctx: &ReducerContext, race: u8, class: u8, level: u32) -> u32 {
    match power_type::for_class(class) {
        power_type::MANA => {
            let base = ctx
                .db
                .game_class_level_stats()
                .class_level()
                .find(class_level_key(class, level));
            let attrs = ctx
                .db
                .game_level_stats()
                .race_class_level()
                .find(race_class_level_key(race, class, level));
            match (base, attrs) {
                (Some(b), Some(a)) => b.base_mana + mana_from_intellect(a.intellect),
                _ => 0,
            }
        }
        power_type::ENERGY => 100,
        _ => 1000,
    }
}

pub(crate) fn starting_power(pt: u8, max_power: u32) -> u32 {
    if pt == power_type::MANA || pt == power_type::ENERGY {
        max_power
    } else {
        0
    }
}

pub(crate) fn base_attributes_for(
    ctx: &ReducerContext,
    race: u8,
    class: u8,
    level: u32,
) -> (u32, u32, u32, u32, u32) {
    match ctx
        .db
        .game_level_stats()
        .race_class_level()
        .find(race_class_level_key(race, class, level))
    {
        Some(a) => (a.strength, a.agility, a.stamina, a.intellect, a.spirit),
        None => (0, 0, 0, 0, 0),
    }
}

pub(crate) struct LevelStatsDelta {
    pub old_max_health: u32,
    pub old_max_power: u32,
    pub old_strength: u32,
    pub old_agility: u32,
    pub old_stamina: u32,
    pub old_intellect: u32,
    pub old_spirit: u32,
}

pub(crate) fn apply_level_stats(
    ctx: &ReducerContext,
    e: &mut WorldEntity,
    race: u8,
    class: u8,
    level: u32,
) -> LevelStatsDelta {
    let old = LevelStatsDelta {
        old_max_health: e.max_health,
        old_max_power: e.max_power,
        old_strength: e.strength,
        old_agility: e.agility,
        old_stamina: e.stamina,
        old_intellect: e.intellect,
        old_spirit: e.spirit,
    };
    let (strength, agility, stamina, intellect, spirit) =
        base_attributes_for(ctx, race, class, level);
    e.strength = strength;
    e.agility = agility;
    e.stamina = stamina;
    e.intellect = intellect;
    e.spirit = spirit;
    e.armor = agility * 2;
    e.max_health = max_health_for(ctx, race, class, level);
    e.max_power = match crate::spell::form::power_kind(class, e.stance) {
        power_type::RAGE => 1000,
        power_type::ENERGY => 100,
        _ => max_power_for(ctx, race, class, level),
    };
    old
}

pub(crate) fn set_character_level(
    ctx: &ReducerContext,
    character_guid: u64,
    level: u32,
) -> Result<(), String> {
    let char_row = ctx.db.game_character().guid().find(character_guid);
    let (race, class) = char_row
        .as_ref()
        .map(|c| (c.race, c.class))
        .unwrap_or((1, 1));

    let entities = ctx.db.game_world_entity();
    let live = entities.guid().find(character_guid);
    if live.is_none() && char_row.is_none() {
        return Err(format!(
            "no live entity or character row for guid {character_guid}"
        ));
    }
    if let Some(mut e) = live {
        e.level = level;
        e.xp = 0;
        e.next_level_xp = crate::xp::xp_to_next_level(level);
        apply_level_stats(ctx, &mut e, race, class, level);
        e.health = e.max_health;
        e.power = if e.power_type() == power_type::RAGE {
            e.power.min(e.max_power)
        } else {
            e.max_power
        };
        e.update(ctx);
        crate::spell::recompute_vitals(ctx, character_guid);
        crate::spell::recompute_sheet(ctx, character_guid);
    }

    let chars = ctx.db.game_character();
    if let Some(mut c) = chars.guid().find(character_guid) {
        c.level = level as u8;
        c.xp = 0;
        c.next_level_xp = crate::xp::xp_to_next_level(level);
        chars.guid().update(c);
    }
    spacetimedb::log::info!("set_character_level: guid {character_guid} -> level {level}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamina_curve_matches_vanilla_breakpoints() {
        assert_eq!(hp_from_stamina(0), 0);
        assert_eq!(hp_from_stamina(20), 20);
        assert_eq!(hp_from_stamina(21), 30);
        assert_eq!(20 + hp_from_stamina(22), 60);
    }

    #[test]
    fn intellect_curve_matches_vanilla_breakpoints() {
        assert_eq!(mana_from_intellect(20), 20);
        assert_eq!(mana_from_intellect(21), 35);
    }

    #[test]
    fn starting_power_full_for_mana_and_energy_empty_for_rage() {
        assert_eq!(starting_power(power_type::MANA, 1500), 1500);
        assert_eq!(starting_power(power_type::ENERGY, 100), 100);
        assert_eq!(starting_power(power_type::RAGE, 1000), 0);
        assert_eq!(starting_power(power_type::MANA, 0), 0);
    }

    #[test]
    fn keys_pack_class_race_level_uniquely() {
        assert_eq!(class_level_key(1, 1), (1 << 8) | 1);
        assert_eq!(class_level_key(9, 60), (9 << 8) | 60);
        assert_ne!(class_level_key(1, 60), class_level_key(60, 1));
        assert_eq!(race_class_level_key(1, 1, 1), (1 << 16) | (1 << 8) | 1);
        assert_ne!(race_class_level_key(1, 2, 3), race_class_level_key(3, 2, 1));
    }
}
