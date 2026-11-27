use lyracore_shared::item_property as stat;
use lyracore_shared::spell::taxonomy::{
    A_MOD_RESISTANCE, RESISTANCE_BASE_PCT, RESISTANCE_BASE_VALUE, RESISTANCE_TOTAL_VALUE,
    RESIST_ARMOR,
};
use spacetimedb::ReducerContext;

use super::{aura_amounts, AuraAmount};
use crate::WorldEntity;

const SCHOOL_BITS: [u8; 7] = [1, 2, 4, 8, 16, 32, 64];
const STAT_KINDS: [u8; 7] = [
    stat::ARMOR,
    stat::HOLY_RESISTANCE,
    stat::FIRE_RESISTANCE,
    stat::NATURE_RESISTANCE,
    stat::FROST_RESISTANCE,
    stat::SHADOW_RESISTANCE,
    stat::ARCANE_RESISTANCE,
];

#[derive(Clone, Copy, Debug)]
pub(crate) struct ResistanceItem {
    pub(crate) slot: u8,
    pub(crate) max_durability: u32,
    pub(crate) durability: u32,
    pub(crate) template: [i32; 7],
    pub(crate) enchant_id: u32,
    pub(crate) property_enchants: [u32; 3],
}

/// vmangos Unit::GetTotalResistanceValue: ((create + base) * base_pct + total) * total_pct, in f32.
#[derive(Clone, Copy, Debug)]
struct ResistanceTerms {
    base: i32,
    base_pct: f32,
    total: i32,
}

impl ResistanceTerms {
    fn resolve(self, create: u32, dynamic: u32) -> i32 {
        let base = create as f32 + self.base as f32;
        let total = i64::from(self.total) + i64::from(dynamic);
        (base * self.base_pct + total as f32) as i32
    }
}

fn base_pct_factor(percent: i32) -> f32 {
    // vmangos ApplyPercentModFloatVar keeps a -100 percent modifier from zeroing the unit modifier.
    let percent = if percent == -100 {
        -99.99
    } else {
        percent as f32
    };
    (100.0 + percent) / 100.0
}

fn resistance_terms(
    school_mask: u8,
    auras: impl IntoIterator<Item = AuraAmount>,
    items: impl IntoIterator<Item = ResistanceItem>,
    mut enchant_stat: impl FnMut(u32, u8) -> i32,
) -> ResistanceTerms {
    let mut terms = ResistanceTerms {
        base: 0,
        base_pct: 1.0,
        total: 0,
    };
    for aura in auras {
        if aura.kind != A_MOD_RESISTANCE || aura.parameter as u32 & u32::from(school_mask) == 0 {
            continue;
        }
        match aura.p1 {
            RESISTANCE_BASE_VALUE => terms.base = terms.base.saturating_add(aura.total()),
            RESISTANCE_BASE_PCT => terms.base_pct *= base_pct_factor(aura.total()),
            _ => terms.total = terms.total.saturating_add(aura.total()),
        }
    }
    let Some(index) = school_index(school_mask) else {
        return terms;
    };
    let kind = STAT_KINDS[index];
    let granting: Vec<ResistanceItem> = items
        .into_iter()
        .filter(|item| {
            lyracore_shared::item::grants_equipped_stats(
                item.slot,
                item.max_durability,
                item.durability,
            )
        })
        .collect();
    let template = granting.iter().map(|item| item.template[index]);
    terms.base = template.fold(terms.base, i32::saturating_add);
    for item in &granting {
        let enchant = if item.enchant_id == 0 {
            0
        } else {
            enchant_stat(item.enchant_id, kind)
        };
        let property = stat::property_stat(item.property_enchants, |id| {
            if id == 0 {
                0
            } else {
                enchant_stat(id, kind)
            }
        });
        terms.total = terms
            .total
            .saturating_add(stat::item_stat(0, enchant, property));
    }
    terms
}

fn unit_terms(ctx: &ReducerContext, unit_guid: u64, school_mask: u8) -> ResistanceTerms {
    resistance_terms(
        school_mask,
        aura_amounts(ctx, unit_guid),
        crate::items::resistance_items(ctx, unit_guid),
        |id, kind| crate::items::enchant_stat(ctx, id, kind),
    )
}

/// The signed Aura and gear resistance of a school, before any floor at zero.
pub(crate) fn resistance_bonus(ctx: &ReducerContext, unit_guid: u64, school_mask: u8) -> i32 {
    unit_terms(ctx, unit_guid, school_mask).resolve(0, 0)
}

pub(crate) fn effective_armor(ctx: &ReducerContext, unit: &WorldEntity) -> u32 {
    effective_resistance(ctx, unit, RESIST_ARMOR)
}

/// The UNIT_FIELD_RESISTANCES value: `unit.armor` is the agility armor of a Character, which the
/// base percent does not multiply, and the template armor of a creature, which it does.
pub(crate) fn effective_resistance(
    ctx: &ReducerContext,
    unit: &WorldEntity,
    school_mask: u8,
) -> u32 {
    let terms = unit_terms(ctx, unit.guid, school_mask);
    let value = match (school_mask == RESIST_ARMOR, unit.is_player()) {
        (false, _) => terms.resolve(0, 0),
        (true, true) => terms.resolve(0, unit.armor),
        (true, false) => terms.resolve(unit.armor, 0),
    };
    value.max(0) as u32
}

/// The client's positive armor buff field: flat Auras only, as vmangos Aura::HandleAuraModResistance.
pub(crate) fn positive_armor(auras: impl IntoIterator<Item = AuraAmount>) -> u32 {
    auras
        .into_iter()
        .filter(|aura| {
            aura.kind == A_MOD_RESISTANCE
                && aura.p1 == RESISTANCE_TOTAL_VALUE
                && aura.parameter as u32 & u32::from(SCHOOL_BITS[0]) != 0
        })
        .map(AuraAmount::total)
        .filter(|&amount| amount > 0)
        .fold(0u32, |sum, amount| sum.saturating_add(amount as u32))
}

fn school_index(school_mask: u8) -> Option<usize> {
    let index = school_mask.trailing_zeros() as usize;
    (index < SCHOOL_BITS.len()).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chest() -> ResistanceItem {
        ResistanceItem {
            slot: 4,
            max_durability: 100,
            durability: 100,
            template: [120, 1, 2, 3, 4, 5, 6],
            enchant_id: 1,
            property_enchants: [2, 3, 0],
        }
    }

    fn aura(parameter: i32, amount: i32, stacks: u8, p1: i32) -> AuraAmount {
        AuraAmount {
            kind: A_MOD_RESISTANCE,
            parameter,
            amount,
            stacks,
            p1,
        }
    }

    fn enchants(id: u32, kind: u8) -> i32 {
        match (id, kind) {
            (1, stat::ARMOR) => 11,
            (2, stat::ARMOR) => 7,
            (3, stat::ARMOR) => -2,
            (2, stat::FIRE_RESISTANCE) => 9,
            _ => 0,
        }
    }

    fn resistance(
        school_mask: u8,
        auras: impl IntoIterator<Item = AuraAmount>,
        items: impl IntoIterator<Item = ResistanceItem>,
    ) -> ResistanceTerms {
        resistance_terms(school_mask, auras, items, enchants)
    }

    #[test]
    fn resistance_combines_stack_masks_equipment_and_property_effects() {
        let auras = [
            aura(5, 20, 0, RESISTANCE_TOTAL_VALUE),
            aura(1, -10, 3, RESISTANCE_TOTAL_VALUE),
            AuraAmount {
                kind: 0,
                ..aura(1, 500, 1, RESISTANCE_TOTAL_VALUE)
            },
        ];
        let items = [
            chest(),
            ResistanceItem {
                slot: 23,
                ..chest()
            },
            ResistanceItem {
                durability: 0,
                ..chest()
            },
            ResistanceItem {
                slot: 18,
                max_durability: 0,
                durability: 0,
                enchant_id: 0,
                property_enchants: [0; 3],
                template: [5; 7],
            },
        ];
        assert_eq!(resistance(1, auras, items).resolve(0, 40), 171);
        assert_eq!(resistance(4, auras, items).resolve(0, 0), 36);
        assert_eq!(resistance(5, auras, items).resolve(0, 0), 131);
        assert_eq!(positive_armor(auras), 20);
    }

    #[test]
    fn a_base_percent_multiplies_only_the_template_and_base_value_armor() {
        let auras = [
            aura(1, 180, 1, RESISTANCE_BASE_PCT),
            aura(1, 10, 1, RESISTANCE_BASE_VALUE),
            aura(1, 30, 1, RESISTANCE_TOTAL_VALUE),
        ];
        let item = ResistanceItem {
            template: [100, 0, 0, 0, 0, 0, 0],
            ..chest()
        };
        let terms = resistance(1, auras, [item]);
        assert_eq!(terms.resolve(0, 40), 308 + 30 + 11 + 7 - 2 + 40);
        assert_eq!(terms.resolve(40, 0), 420 + 30 + 11 + 7 - 2);
        assert_eq!(positive_armor(auras), 30);
    }

    #[test]
    fn base_percents_compound_and_a_negative_hundred_percent_leaves_the_total_value() {
        let item = ResistanceItem {
            enchant_id: 0,
            property_enchants: [0; 3],
            ..chest()
        };
        let compounded = [
            aura(1, 100, 1, RESISTANCE_BASE_PCT),
            aura(1, 100, 1, RESISTANCE_BASE_PCT),
        ];
        assert_eq!(resistance(1, compounded, [item]).resolve(0, 0), 480);
        let weakened = [
            aura(1, -100, 1, RESISTANCE_BASE_PCT),
            aura(1, 30, 1, RESISTANCE_TOTAL_VALUE),
        ];
        assert_eq!(resistance(1, weakened, [item]).resolve(0, 5), 35);
        let other_school = [aura(4, 180, 1, RESISTANCE_BASE_PCT)];
        assert_eq!(resistance(1, other_school, [item]).resolve(0, 0), 120);
    }

    #[test]
    fn a_base_percent_multiplies_a_school_resistance_from_items() {
        let auras = [aura(4, 100, 1, RESISTANCE_BASE_PCT)];
        assert_eq!(resistance(4, auras, [chest()]).resolve(0, 0), 4 + 9);
    }

    #[test]
    fn resistance_saturates_and_ignores_masks_without_a_school() {
        let item = ResistanceItem {
            template: [i32::MAX; 7],
            ..chest()
        };
        let auras = [aura(1, 10, 1, RESISTANCE_TOTAL_VALUE)];
        assert_eq!(resistance(1, auras, [item]).resolve(0, 40), i32::MAX);
        assert_eq!(resistance(0, auras, [item]).resolve(0, 0), 0);
        assert_eq!(resistance(128, auras, [item]).resolve(0, 0), 0);
    }
}
