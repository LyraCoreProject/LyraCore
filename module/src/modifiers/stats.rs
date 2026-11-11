use lyracore_shared::spell::taxonomy::{
    A_DISARM, A_MOD_COMBAT, A_MOD_STAT, A_MOD_STAT_PCT, COMBAT_ATTACK_POWER, COMBAT_CRIT,
    COMBAT_RANGED_ATTACK_POWER, STAT_AGI, STAT_ALL, STAT_INT, STAT_SPI, STAT_STA, STAT_STR,
};
use spacetimedb::ReducerContext;

use super::{aura_amounts, AuraAmount};
use crate::items::EquipStat;
use crate::WorldEntity;

// The inputs `recompute_vitals` and `recompute_sheet` read; `moves_*` must match them.
pub(crate) const VITALS_STATS: [u8; 3] = [STAT_STA, STAT_INT, STAT_SPI];
pub(crate) const SHEET_STATS: [u8; 5] = [STAT_STR, STAT_AGI, STAT_STA, STAT_INT, STAT_SPI];
const SHEET_COMBAT_FIELDS: [u8; 3] = [COMBAT_ATTACK_POWER, COMBAT_RANGED_ATTACK_POWER, COMBAT_CRIT];

/// Base value plus flat aura and gear bonuses, scaled by the stat's percent auras.
pub(crate) fn effective_stat(ctx: &ReducerContext, unit: &WorldEntity, stat: u8) -> u32 {
    stat_value(
        base_stat(ctx, unit, stat),
        crate::items::equipped_stat_bonus(ctx, unit.guid, gear_stat(stat)),
        aura_amounts(ctx, unit.guid),
        stat,
    )
}

// A Character's stored spirit is effective, so its base comes from the level table.
fn base_stat(ctx: &ReducerContext, unit: &WorldEntity, stat: u8) -> u32 {
    match stat {
        STAT_STR => unit.strength,
        STAT_AGI => unit.agility,
        STAT_STA => unit.stamina,
        STAT_INT => unit.intellect,
        _ if unit.is_player() => {
            let (race, class, _, _) = lyracore_shared::packing::unpack4(unit.unit_bytes_0);
            crate::stats::base_attributes_for(ctx, race, class, unit.level).4
        }
        _ => unit.spirit,
    }
}

fn gear_stat(stat: u8) -> EquipStat {
    match stat {
        STAT_STR => EquipStat::Strength,
        STAT_AGI => EquipStat::Agility,
        STAT_STA => EquipStat::Stamina,
        STAT_INT => EquipStat::Intellect,
        _ => EquipStat::Spirit,
    }
}

fn names_stat(parameter: i32, stat: u8) -> bool {
    parameter == i32::from(stat) || parameter == i32::from(STAT_ALL)
}

fn moves_stat(kind: u8, parameter: i32, stat: u8) -> bool {
    matches!(kind, A_MOD_STAT | A_MOD_STAT_PCT) && names_stat(parameter, stat)
}

// vmangos `Unit::GetTotalStatValue`: percent stat auras scale the base, gear and flat aura sum.
fn stat_value(base: u32, gear: i32, auras: impl IntoIterator<Item = AuraAmount>, stat: u8) -> u32 {
    let (mut flat, mut pct) = (i128::from(gear), 0i128);
    for aura in auras {
        if !names_stat(aura.parameter, stat) {
            continue;
        }
        match aura.kind {
            A_MOD_STAT => flat += i128::from(aura.total()),
            A_MOD_STAT_PCT => pct += i128::from(aura.total()),
            _ => {}
        }
    }
    ((i128::from(base) + flat) * (100 + pct) / 100).clamp(0, i128::from(u32::MAX)) as u32
}

pub(crate) fn moves_vitals(kind: u8, parameter: i32) -> bool {
    VITALS_STATS
        .into_iter()
        .any(|stat| moves_stat(kind, parameter, stat))
}

pub(crate) fn moves_sheet(kind: u8, parameter: i32) -> bool {
    SHEET_STATS
        .into_iter()
        .any(|stat| moves_stat(kind, parameter, stat))
        || (kind == A_MOD_COMBAT
            && SHEET_COMBAT_FIELDS
                .into_iter()
                .any(|field| parameter == i32::from(field)))
        || kind == A_DISARM
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aura(kind: u8, stat: u8, amount: i32) -> AuraAmount {
        AuraAmount {
            kind,
            parameter: i32::from(stat),
            amount,
            stacks: 1,
        }
    }

    #[test]
    fn flat_stat_sums_base_aura_and_gear_and_clamps_at_zero() {
        assert_eq!(stat_value(23, 0, [], STAT_STR), 23);
        assert_eq!(
            stat_value(23, 7, [aura(A_MOD_STAT, STAT_STR, 5)], STAT_STR),
            35
        );
        assert_eq!(
            stat_value(10, 3, [aura(A_MOD_STAT, STAT_STR, -25)], STAT_STR),
            0
        );
        assert_eq!(
            stat_value(10, 0, [aura(A_MOD_STAT, STAT_AGI, 9)], STAT_STR),
            10
        );
    }

    #[test]
    fn blessing_of_kings_raises_every_stat_by_ten_percent() {
        let kings = [aura(A_MOD_STAT_PCT, STAT_ALL, 10)];
        for stat in SHEET_STATS {
            assert_eq!(stat_value(100, 0, kings, stat), 110);
        }
    }

    #[test]
    fn spirit_and_stat_percent_auras_move_the_stored_vitals() {
        assert!(moves_vitals(A_MOD_STAT, i32::from(STAT_SPI)));
        assert!(moves_vitals(A_MOD_STAT_PCT, i32::from(STAT_ALL)));
        assert!(moves_sheet(A_MOD_STAT_PCT, i32::from(STAT_STR)));
    }
}
