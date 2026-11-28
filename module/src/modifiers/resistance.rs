use lyracore_shared::item_property as stat;
use lyracore_shared::spell::taxonomy::{
    A_MOD_RESISTANCE, RESISTANCE_BASE_PCT, RESISTANCE_BASE_VALUE, RESISTANCE_EXCLUSIVE,
    RESISTANCE_INTELLECT_PCT, RESISTANCE_TOTAL_PCT, RESISTANCE_TOTAL_VALUE, RESIST_ARMOR, STAT_INT,
};
use spacetimedb::ReducerContext;

use super::{aura_amounts, AuraAmount};
use crate::{Aura, WorldEntity};

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

/// vmangos Unit::GetTotalResistanceValue: (((create + base) * base_pct) + total) * total_pct, in f32.
/// An exclusive Aura adds only the strongest bonus and the strongest malus to the total, and a
/// Character's armor adds `intellect_pct` percent of its Intellect to the total, as
/// Player::UpdateArmor does.
#[derive(Clone, Copy, Debug)]
struct ResistanceTerms {
    base: i32,
    base_pct: f32,
    total: i32,
    total_pct: f32,
    exclusive_bonus: i32,
    exclusive_malus: i32,
    intellect_pct: i32,
    intellect: u32,
}

/// What `game_world_entity.armor` adds to armor: a Character stores its agility armor, a flat
/// term the percents of the base value never multiply, and a creature its template armor, which
/// is part of the base value. Magic schools take nothing from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntityArmor {
    AgilityArmor(u32),
    TemplateArmor(u32),
    None,
}

impl EntityArmor {
    fn of(unit: &WorldEntity, school_mask: u8) -> Self {
        match (school_mask == RESIST_ARMOR, unit.is_player()) {
            (false, _) => Self::None,
            (true, true) => Self::AgilityArmor(unit.armor),
            (true, false) => Self::TemplateArmor(unit.armor),
        }
    }
}

impl ResistanceTerms {
    fn with_auras(mut self, school_mask: u8, auras: impl IntoIterator<Item = AuraAmount>) -> Self {
        for aura in auras {
            if aura.kind != A_MOD_RESISTANCE || aura.parameter as u32 & u32::from(school_mask) == 0
            {
                continue;
            }
            match aura.p1 {
                RESISTANCE_BASE_VALUE => self.base = self.base.saturating_add(aura.total()),
                RESISTANCE_BASE_PCT => self.base_pct *= percent_factor(aura.total()),
                RESISTANCE_TOTAL_PCT => self.total_pct *= percent_factor(aura.total()),
                RESISTANCE_EXCLUSIVE => {
                    self.exclusive_bonus = self.exclusive_bonus.max(aura.total());
                    self.exclusive_malus = self.exclusive_malus.min(aura.total());
                }
                RESISTANCE_INTELLECT_PCT => {
                    self.intellect_pct = self.intellect_pct.saturating_add(aura.total());
                }
                _ => self.total = self.total.saturating_add(aura.total()),
            }
        }
        self
    }

    fn resolve(self, entity_armor: EntityArmor) -> i32 {
        let (create, agility) = match entity_armor {
            EntityArmor::TemplateArmor(armor) => (armor, 0),
            EntityArmor::AgilityArmor(armor) => (0, armor),
            EntityArmor::None => (0, 0),
        };
        let base = create as f32 + self.base as f32;
        let total = i64::from(self.total)
            + i64::from(agility)
            + i64::from(self.exclusive_bonus)
            + i64::from(self.exclusive_malus);
        let intellect_armor = self.intellect as f32 * (self.intellect_pct as f32 * 0.01);
        ((base * self.base_pct + total as f32 + intellect_armor) * self.total_pct) as i32
    }
}

fn percent_factor(percent: i32) -> f32 {
    // vmangos ApplyPercentModFloatVar keeps a -100 percent modifier from zeroing the unit modifier.
    let percent = if percent == -100 {
        -99.99
    } else {
        percent as f32
    };
    (100.0 + percent) / 100.0
}

fn gear_terms(
    school_mask: u8,
    items: impl IntoIterator<Item = ResistanceItem>,
    mut enchant_stat: impl FnMut(u32, u8) -> i32,
) -> ResistanceTerms {
    let mut terms = ResistanceTerms {
        base: 0,
        base_pct: 1.0,
        total: 0,
        total_pct: 1.0,
        exclusive_bonus: 0,
        exclusive_malus: 0,
        intellect_pct: 0,
        intellect: 0,
    };
    let Some(index) = school_index(school_mask) else {
        return terms;
    };
    let kind = STAT_KINDS[index];
    for item in items.into_iter().filter(|item| {
        lyracore_shared::item::grants_equipped_stats(
            item.slot,
            item.max_durability,
            item.durability,
        )
    }) {
        terms.base = terms.base.saturating_add(item.template[index]);
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

fn unit_gear_terms(ctx: &ReducerContext, unit_guid: u64, school_mask: u8) -> ResistanceTerms {
    gear_terms(
        school_mask,
        crate::items::resistance_items(ctx, unit_guid),
        |id, kind| crate::items::enchant_stat(ctx, id, kind),
    )
}

/// The signed Aura and gear resistance of a school, before any floor at zero.
pub(crate) fn resistance_bonus(ctx: &ReducerContext, unit_guid: u64, school_mask: u8) -> i32 {
    unit_gear_terms(ctx, unit_guid, school_mask)
        .with_auras(school_mask, aura_amounts(ctx, unit_guid))
        .resolve(EntityArmor::None)
}

/// Whether a change to `changed` moves the armor a Character shows: an armor Aura, or an
/// Intellect Aura while an Intellect-armor Aura is held.
pub(crate) fn moves_armor(changed: &Aura, held: &[Aura]) -> bool {
    (changed.eff_kind == A_MOD_RESISTANCE && changed.eff_p0 & 0x7f != 0)
        || (super::stats::moves_intellect(changed.eff_kind, changed.eff_p0)
            && held.iter().any(|aura| {
                aura.eff_kind == A_MOD_RESISTANCE && aura.eff_p1 == RESISTANCE_INTELLECT_PCT
            }))
}

pub(crate) fn effective_armor(ctx: &ReducerContext, unit: &WorldEntity) -> u32 {
    effective_resistance(ctx, unit, RESIST_ARMOR)
}

/// The UNIT_FIELD_RESISTANCES value of a school, floored at zero.
pub(crate) fn effective_resistance(
    ctx: &ReducerContext,
    unit: &WorldEntity,
    school_mask: u8,
) -> u32 {
    let mut terms = unit_gear_terms(ctx, unit.guid, school_mask)
        .with_auras(school_mask, aura_amounts(ctx, unit.guid));
    if terms.intellect_pct != 0 && school_mask == RESIST_ARMOR && unit.is_player() {
        terms.intellect = super::effective_stat(ctx, unit, STAT_INT);
    }
    terms.resolve(EntityArmor::of(unit, school_mask)).max(0) as u32
}

/// The client's positive armor buff field: what each flat or total percent Aura with a positive
/// amount adds to armor, as vmangos `ApplyResistanceBuffModsMod` records it.
pub(crate) fn positive_armor(
    ctx: &ReducerContext,
    unit: &WorldEntity,
    auras: &[AuraAmount],
) -> u32 {
    let gear = unit_gear_terms(ctx, unit.guid, RESIST_ARMOR);
    positive_buff(auras, |held| {
        gear.with_auras(RESIST_ARMOR, held.iter().copied())
            .resolve(EntityArmor::of(unit, RESIST_ARMOR))
    })
}

fn positive_buff(auras: &[AuraAmount], armor_with: impl Fn(&[AuraAmount]) -> i32) -> u32 {
    let with_all = armor_with(auras);
    let mut sum = 0u32;
    for (index, aura) in auras.iter().enumerate() {
        let buffs_armor = aura.kind == A_MOD_RESISTANCE
            && matches!(aura.p1, RESISTANCE_TOTAL_VALUE | RESISTANCE_TOTAL_PCT)
            && aura.parameter as u32 & u32::from(RESIST_ARMOR) != 0
            && aura.total() > 0;
        if buffs_armor {
            let others: Vec<AuraAmount> = auras
                .iter()
                .enumerate()
                .filter_map(|(other, held)| (other != index).then_some(*held))
                .collect();
            sum = sum.saturating_add((with_all - armor_with(&others)).max(0) as u32);
        }
    }
    sum
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

    fn positive(auras: &[AuraAmount]) -> u32 {
        let gear = gear_terms(1, [chest()], enchants);
        positive_buff(auras, |held| {
            gear.with_auras(1, held.iter().copied())
                .resolve(EntityArmor::AgilityArmor(40))
        })
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
        gear_terms(school_mask, items, enchants).with_auras(school_mask, auras)
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
        assert_eq!(
            resistance(1, auras, items).resolve(EntityArmor::AgilityArmor(40)),
            171
        );
        assert_eq!(resistance(4, auras, items).resolve(EntityArmor::None), 36);
        assert_eq!(resistance(5, auras, items).resolve(EntityArmor::None), 131);
        assert_eq!(positive(&auras), 20);
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
        assert_eq!(
            terms.resolve(EntityArmor::AgilityArmor(40)),
            308 + 30 + 11 + 7 - 2 + 40
        );
        assert_eq!(
            terms.resolve(EntityArmor::TemplateArmor(40)),
            420 + 30 + 11 + 7 - 2
        );
        assert_eq!(positive(&auras), 30);
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
        assert_eq!(
            resistance(1, compounded, [item]).resolve(EntityArmor::None),
            480
        );
        let weakened = [
            aura(1, -100, 1, RESISTANCE_BASE_PCT),
            aura(1, 30, 1, RESISTANCE_TOTAL_VALUE),
        ];
        assert_eq!(
            resistance(1, weakened, [item]).resolve(EntityArmor::AgilityArmor(5)),
            35
        );
        let other_school = [aura(4, 180, 1, RESISTANCE_BASE_PCT)];
        assert_eq!(
            resistance(1, other_school, [item]).resolve(EntityArmor::None),
            120
        );
    }

    #[test]
    fn a_base_percent_multiplies_a_school_resistance_from_items() {
        let auras = [aura(4, 100, 1, RESISTANCE_BASE_PCT)];
        assert_eq!(
            resistance(4, auras, [chest()]).resolve(EntityArmor::None),
            4 + 9
        );
    }

    #[test]
    fn resistance_saturates_and_ignores_masks_without_a_school() {
        let item = ResistanceItem {
            template: [i32::MAX; 7],
            ..chest()
        };
        let auras = [aura(1, 10, 1, RESISTANCE_TOTAL_VALUE)];
        assert_eq!(
            resistance(1, auras, [item]).resolve(EntityArmor::AgilityArmor(40)),
            i32::MAX
        );
        assert_eq!(resistance(0, auras, [item]).resolve(EntityArmor::None), 0);
        assert_eq!(resistance(128, auras, [item]).resolve(EntityArmor::None), 0);
    }

    #[test]
    fn a_total_percent_multiplies_the_whole_value_including_flat_terms() {
        let item = ResistanceItem {
            enchant_id: 0,
            property_enchants: [0; 3],
            ..chest()
        };
        let inspiration = [
            aura(1, 25, 1, RESISTANCE_TOTAL_PCT),
            aura(1, 100, 1, RESISTANCE_BASE_PCT),
            aura(1, 30, 1, RESISTANCE_TOTAL_VALUE),
        ];
        let terms = resistance(1, inspiration, [item]);
        assert_eq!(terms.resolve(EntityArmor::AgilityArmor(10)), 350);
        let recklessness = [aura(1, -35, 1, RESISTANCE_TOTAL_PCT)];
        assert_eq!(
            resistance(1, recklessness, [item]).resolve(EntityArmor::AgilityArmor(40)),
            104
        );
        let sickness = [aura(126, -75, 1, RESISTANCE_TOTAL_PCT)];
        assert_eq!(
            resistance(4, sickness, [chest()]).resolve(EntityArmor::None),
            2
        );
        let erased = [aura(1, -100, 1, RESISTANCE_TOTAL_PCT)];
        assert_eq!(resistance(1, erased, [item]).resolve(EntityArmor::None), 0);
    }

    #[test]
    fn exclusive_auras_count_only_the_strongest_bonus_and_malus_of_each_school() {
        let fire_resistance = [
            aura(4, 30, 1, RESISTANCE_EXCLUSIVE),
            aura(4, 50, 1, RESISTANCE_EXCLUSIVE),
            aura(4, -10, 1, RESISTANCE_EXCLUSIVE),
            aura(4, -20, 1, RESISTANCE_EXCLUSIVE),
            aura(4, 10, 1, RESISTANCE_TOTAL_VALUE),
            aura(8, 70, 1, RESISTANCE_EXCLUSIVE),
        ];
        let fire = resistance(4, fire_resistance, [chest()]);
        assert_eq!(fire.resolve(EntityArmor::None), 2 + 9 + 50 - 20 + 10);
        let nature = resistance(8, fire_resistance, [chest()]);
        assert_eq!(nature.resolve(EntityArmor::None), 3 + 70);
        let all_schools = [aura(126, 25, 1, RESISTANCE_EXCLUSIVE)];
        assert_eq!(
            resistance(4, all_schools, [chest()]).resolve(EntityArmor::None),
            2 + 9 + 25
        );
        assert_eq!(positive(&fire_resistance), 0);
    }

    #[test]
    fn an_exclusive_aura_is_a_total_value_that_a_total_percent_multiplies() {
        let auras = [
            aura(4, 40, 1, RESISTANCE_EXCLUSIVE),
            aura(4, 100, 1, RESISTANCE_BASE_PCT),
            aura(4, 50, 1, RESISTANCE_TOTAL_PCT),
        ];
        assert_eq!(
            resistance(4, auras, [chest()]).resolve(EntityArmor::None),
            79
        );
    }

    #[test]
    fn intellect_armor_is_a_flat_term_of_a_character_that_no_base_percent_multiplies() {
        let arcane_resilience = [aura(1, 50, 1, RESISTANCE_INTELLECT_PCT)];
        let mut terms = resistance(1, arcane_resilience, [chest()]);
        assert_eq!(terms.resolve(EntityArmor::AgilityArmor(40)), 120 + 16 + 40);
        terms.intellect = 90;
        assert_eq!(
            terms.resolve(EntityArmor::AgilityArmor(40)),
            120 + 16 + 40 + 45
        );
        let doubled = [
            aura(1, 50, 1, RESISTANCE_INTELLECT_PCT),
            aura(1, 100, 1, RESISTANCE_BASE_PCT),
            aura(1, 10, 1, RESISTANCE_TOTAL_PCT),
        ];
        let mut terms = resistance(1, doubled, [chest()]);
        terms.intellect = 90;
        assert_eq!(terms.resolve(EntityArmor::AgilityArmor(40)), 375);
        assert_eq!(positive(&arcane_resilience), 0);
    }

    #[test]
    fn the_positive_buff_field_counts_flat_and_total_percent_auras_only() {
        let held = [
            aura(1, 20, 1, RESISTANCE_TOTAL_VALUE),
            aura(1, 25, 1, RESISTANCE_TOTAL_PCT),
            aura(1, 180, 1, RESISTANCE_BASE_PCT),
            aura(1, 10, 1, RESISTANCE_BASE_VALUE),
            aura(1, -35, 1, RESISTANCE_TOTAL_PCT),
            aura(4, 50, 1, RESISTANCE_TOTAL_VALUE),
        ];
        let armor_with = |held: &[AuraAmount]| {
            gear_terms(1, [chest()], enchants)
                .with_auras(1, held.iter().copied())
                .resolve(EntityArmor::AgilityArmor(40))
        };
        let flat_only = [held[0]];
        assert_eq!(positive_buff(&flat_only, armor_with), 20);
        let inspired = [held[1]];
        assert_eq!(positive_buff(&inspired, armor_with), 44);
        assert_eq!(positive_buff(&held[2..], armor_with), 0);
    }
}
