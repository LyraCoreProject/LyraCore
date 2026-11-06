use lyracore_shared::{aura::stacked_amount, packing::power_type::MANA, spell::taxonomy::*};
use spacetimedb::{table, ReducerContext, Table};

use crate::{game_spell, game_world_entity, WorldEntity};

#[table(accessor = game_health_regen_carry)]
pub struct HealthRegenCarry {
    #[primary_key]
    pub character_guid: u64,
    pub fraction: f32,
}

crate::character_owned!(delete, fn sweep_delete_game_health_regen_carry(ctx, character_guid) {
    ctx.db.game_health_regen_carry().character_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_health_regen_carry());

pub(crate) fn clear_character_regeneration(ctx: &ReducerContext, guid: u64) {
    ctx.db
        .game_health_regen_carry()
        .character_guid()
        .delete(guid);
}

#[derive(Default)]
struct Bonuses {
    mana_per_five: f32,
    health_per_five: f32,
    food_per_tick: f32,
    spirit_mana_rate: f32,
    spirit_health_rate: f32,
    combat_health_pct: f32,
    casting_mana_pct: f32,
    polymorphed: bool,
}

impl Bonuses {
    fn read(ctx: &ReducerContext, e: &WorldEntity) -> Self {
        let mut bonuses = Self {
            spirit_mana_rate: 1.0,
            spirit_health_rate: 1.0,
            mana_per_five: crate::items::equipped_stat_bonus(
                ctx,
                e.guid,
                crate::items::EquipStat::ManaPerFive,
            ) as f32,
            health_per_five: crate::items::equipped_stat_bonus(
                ctx,
                e.guid,
                crate::items::EquipStat::HealthPerFive,
            ) as f32,
            ..Self::default()
        };
        let now = crate::time::now(ctx);
        for aura in crate::spell::auras_on(ctx, e.guid)
            .filter(|aura| !crate::spell::is_due_for_expiry(aura.eff_kind, aura.expires_at, now))
        {
            let amount = stacked_amount(aura.amount, aura.stacks) as f32;
            match aura.eff_kind {
                A_MOD_POWER_REGEN if aura.eff_p0 == i32::from(MANA) => {
                    bonuses.mana_per_five += amount
                }
                A_MOD_HEALTH_REGEN => bonuses.health_per_five += amount,
                A_MOD_REGEN => {
                    bonuses.food_per_tick +=
                        amount * 2000.0 / if aura.eff_p1 > 0 { aura.eff_p1 } else { 5000 } as f32
                }
                A_MOD_HEALTH_REGEN_PCT => bonuses.spirit_health_rate *= (100.0 + amount) / 100.0,
                A_MOD_POWER_REGEN_PCT if aura.eff_p0 == i32::from(MANA) => {
                    bonuses.spirit_mana_rate *= (100.0 + amount) / 100.0
                }
                A_COMBAT_HEALTH_REGEN_PCT => bonuses.combat_health_pct += amount,
                A_MANA_REGEN_INTERRUPT => bonuses.casting_mana_pct += amount,
                A_CONTROL if aura.eff_p0 == M_POLY => {
                    bonuses.polymorphed |= ctx
                        .db
                        .game_spell()
                        .spell_id()
                        .find(aura.spell_id)
                        .is_some_and(|spell| spell.mechanic == 17);
                }
                _ => {}
            }
        }
        bonuses.casting_mana_pct = bonuses.casting_mana_pct.min(100.0);
        bonuses
    }

    fn mana_per_tick(&self, class: u8, spirit: u32, casting: bool) -> u32 {
        let spirit = mana_per_second(class, spirit) * self.spirit_mana_rate;
        let rate = self.mana_per_five / 5.0
            + spirit
                * if casting {
                    self.casting_mana_pct / 100.0
                } else {
                    1.0
                };
        (rate * 2.0).max(0.0) as u32
    }
}

// The caller credits whole two-second ticks; mana discards fractions and health carries them.
pub(super) fn regenerate(ctx: &ReducerContext, ticks: u64) {
    let now_ms = crate::time::now(ctx).to_micros_since_unix_epoch().max(0) as u64 / 1000;
    let recovering: Vec<_> = ctx
        .db
        .game_world_entity()
        .by_regeneration_pending()
        .filter(true)
        .filter(|e| e.is_player())
        .collect();
    for mut e in recovering {
        let bonuses = Bonuses::read(ctx, &e);
        let in_combat = e.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0
            || super::is_engaged(ctx, e.guid);
        let class = (e.unit_bytes_0 >> 8) as u8;
        let before = (e.health, e.power);
        if e.power_type() == MANA && e.power < e.max_power {
            let per_tick =
                bonuses.mana_per_tick(class, e.spirit, now_ms < e.mana_regen_paused_until_ms);
            e.power = u64::from(e.power)
                .saturating_add(ticks.saturating_mul(u64::from(per_tick)))
                .min(u64::from(e.max_power)) as u32;
        }
        if e.health < e.max_health
            && (!in_combat
                || bonuses.combat_health_pct != 0.0
                || bonuses.health_per_five != 0.0
                || bonuses.polymorphed)
        {
            let carry = ctx.db.game_health_regen_carry();
            let previous = carry.character_guid().find(e.guid);
            let mut fraction = previous.as_ref().map_or(0.0, |row| row.fraction);
            let gain = health_per_tick(&e, class, in_combat, &bonuses);
            let ticks = if gain == 0.0 { ticks.min(1) } else { ticks };
            for _ in 0..ticks {
                let amount = gain + fraction;
                fraction = amount - amount as i32 as f32;
                e.health = e
                    .health
                    .saturating_add((amount as i32).max(0) as u32)
                    .min(e.max_health);
                if e.health == e.max_health {
                    break;
                }
            }
            let row = HealthRegenCarry {
                character_guid: e.guid,
                fraction,
            };
            if previous.is_some() {
                carry.character_guid().update(row);
            } else {
                carry.insert(row);
            }
        }
        if before != (e.health, e.power) {
            e.update(ctx);
        }
    }
}

fn mana_per_second(class: u8, spirit: u32) -> f32 {
    let spirit = spirit as f32;
    match class {
        2 | 3 | 9 | 11 => (spirit / 5.0 + 15.0) / 2.0,
        5 | 8 => (spirit / 4.0 + 12.5) / 2.0,
        7 => (spirit / 5.0 + 17.0) / 2.0,
        _ => 0.0,
    }
}

fn health_per_tick(e: &WorldEntity, class: u8, in_combat: bool, bonuses: &Bonuses) -> f32 {
    let mut gain = if bonuses.polymorphed {
        e.max_health as f32 / 10.0
    } else {
        let base = health_per_spirit(class, e.spirit);
        let mut gain = base
            * if in_combat {
                bonuses.combat_health_pct / 100.0
            } else {
                bonuses.spirit_health_rate
            };
        if e.unit_bytes_1 & 0xff != 0 {
            gain *= 1.5;
        }
        if !in_combat {
            gain += bonuses.food_per_tick;
        }
        gain
    };
    gain += 2.0 * (bonuses.health_per_five / 5.0);
    gain
}

fn health_per_spirit(class: u8, spirit: u32) -> f32 {
    let spirit = f64::from(spirit);
    (match class {
        1 => spirit * 1.26 - 22.6,
        2 => spirit * 0.25,
        3 => spirit * 0.43 - 5.5,
        4 => spirit * 0.84 - 13.0,
        5 => spirit * 0.15 + 1.4,
        7 => spirit * 0.28 - 3.6,
        8 | 11 => spirit * 0.11 + 1.0,
        9 => spirit * 0.12 + 1.5,
        _ => 0.0,
    })
    .max(0.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_mana_formulas_are_class_specific_and_ignore_level() {
        for (class, expected) in [
            (2, 17.5),
            (3, 17.5),
            (5, 18.75),
            (7, 18.5),
            (8, 18.75),
            (9, 17.5),
            (11, 17.5),
        ] {
            assert_eq!(mana_per_second(class, 100), expected);
        }
    }

    #[test]
    fn mana_regeneration_truncates_each_tick_after_adding_mp5_and_spirit() {
        let mut bonuses = Bonuses {
            mana_per_five: 12.0,
            spirit_mana_rate: 1.0,
            ..Bonuses::default()
        };
        assert_eq!(bonuses.mana_per_tick(8, 100, true), 4);
        assert_eq!(bonuses.mana_per_tick(8, 100, false), 42);
        bonuses.casting_mana_pct = 30.0;
        assert_eq!(bonuses.mana_per_tick(8, 100, true), 16);
        bonuses.spirit_mana_rate = 5.0;
        assert_eq!(bonuses.mana_per_tick(8, 100, false), 192);
        assert_eq!(bonuses.mana_per_tick(8, 100, true), 61);
    }

    #[test]
    fn health_spirit_uses_the_source_class_formula_with_a_zero_floor() {
        for (class, expected) in [
            (1, 103.4),
            (2, 25.0),
            (3, 37.5),
            (4, 71.0),
            (5, 16.4),
            (7, 24.4),
            (8, 12.0),
            (9, 13.5),
            (11, 12.0),
        ] {
            assert_eq!(health_per_spirit(class, 100), expected);
        }
        assert_eq!(health_per_spirit(4, 0), 0.0);
    }
}
