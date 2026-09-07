//! Random Property selection and the shared enchantment contribution to equipped stats.

use super::{game_item_template, EquipStat, ItemTemplate};
use lyracore_shared::item_property as kind;
use spacetimedb::{table, ReducerContext, Table};

#[table(accessor = game_item_random_property, public)]
pub struct ItemRandomProperty {
    #[primary_key]
    pub property_id: u32,
    pub enchant_id_1: u32,
    pub enchant_id_2: u32,
    pub enchant_id_3: u32,
    pub suffix: String,
}

#[table(accessor = game_item_enchantment, public, index(accessor = by_enchant, btree(columns = [enchant_id])))]
pub struct ItemEnchantment {
    #[primary_key]
    pub id: u64,
    pub enchant_id: u32,
    pub effect_index: u8,
    pub kind: u8,
    pub amount: i32,
    pub spell_id: u32,
    pub school_mask: u32,
}

#[table(accessor = game_item_property_weight, public, index(accessor = by_pool, btree(columns = [pool_id])))]
pub struct ItemPropertyWeight {
    #[primary_key]
    pub id: u64,
    pub pool_id: u32,
    pub property_id: u32,
    pub weight: u32,
}

/// Stable property-ID order, normalized to the total integer weight. Band endpoints round up.
/// Input is in 0..10000. Empty pools, zero IDs, duplicate IDs and zero weights are invalid.
fn property_pick(mut members: Vec<(u32, u32)>, roll: u32) -> Option<u32> {
    if roll >= 10_000 || members.is_empty() {
        return None;
    }
    members.sort_unstable_by_key(|member| member.0);
    if members.iter().any(|&(id, weight)| id == 0 || weight == 0)
        || members.windows(2).any(|pair| pair[0].0 == pair[1].0)
    {
        return None;
    }
    let total: u128 = members.iter().map(|member| u128::from(member.1)).sum();
    let point = u128::from(roll) * total;
    let mut cumulative = 0u128;
    for (id, weight) in members {
        cumulative += u128::from(weight);
        if point < cumulative * 10_000 {
            return Some(id);
        }
    }
    None
}

pub(crate) fn select_property(ctx: &ReducerContext, tmpl: &ItemTemplate) -> Result<u32, String> {
    if tmpl.random_property == 0 {
        return Ok(0);
    }
    let members: Vec<_> = ctx
        .db
        .game_item_property_weight()
        .by_pool()
        .filter(tmpl.random_property)
        .map(|row| (row.property_id, row.weight))
        .collect();
    if members.iter().any(|(id, _)| {
        ctx.db
            .game_item_random_property()
            .property_id()
            .find(id)
            .is_none()
    }) {
        return Err(format!(
            "property pool {} has a missing Random Property",
            tmpl.random_property
        ));
    }
    property_pick(members, ctx.random::<u32>() % 10_000).ok_or_else(|| {
        format!(
            "invalid property pool {} for item {}",
            tmpl.random_property, tmpl.entry
        )
    })
}

pub(crate) fn select_loot_property(ctx: &ReducerContext, entry: u32) -> Result<u32, String> {
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(entry)
        .ok_or_else(|| format!("no template for loot item {entry}"))?;
    select_property(ctx, &tmpl)
}

pub(crate) fn enchant_stat(ctx: &ReducerContext, enchant_id: u32, stat_kind: u8) -> i32 {
    if enchant_id == 0 {
        return 0;
    }
    ctx.db
        .game_item_enchantment()
        .by_enchant()
        .filter(enchant_id)
        .filter(|row| row.kind != kind::UNKNOWN && row.kind == stat_kind)
        .fold(0i32, |sum, row| sum.saturating_add(row.amount))
}

pub(crate) fn property_stat(ctx: &ReducerContext, property_id: u32, stat_kind: u8) -> i32 {
    if property_id == 0 {
        return 0;
    }
    let Some(property) = ctx
        .db
        .game_item_random_property()
        .property_id()
        .find(property_id)
    else {
        return 0;
    };
    [
        property.enchant_id_1,
        property.enchant_id_2,
        property.enchant_id_3,
    ]
    .into_iter()
    .map(|id| enchant_stat(ctx, id, stat_kind))
    .fold(0i32, i32::saturating_add)
}

pub(crate) fn is_known_enchant(ctx: &ReducerContext, enchant_id: u32) -> bool {
    enchant_id != 0
        && ctx
            .db
            .game_item_enchantment()
            .by_enchant()
            .filter(enchant_id)
            .any(|row| row.kind != kind::UNKNOWN)
}

impl EquipStat {
    pub(crate) fn kind(self) -> u8 {
        match self {
            Self::Strength => kind::STRENGTH,
            Self::Agility => kind::AGILITY,
            Self::Stamina => kind::STAMINA,
            Self::Intellect => kind::INTELLECT,
            Self::Spirit => kind::SPIRIT,
            Self::Crit => kind::CRIT,
            Self::Hit => kind::HIT,
            Self::Armor => kind::ARMOR,
        }
    }
}

pub(crate) fn seed_compatibility_enchantments(ctx: &ReducerContext) {
    for (enchant_id, kind, amount) in kind::COMPATIBILITY_ENCHANTMENTS {
        let id = u64::from(enchant_id) << 8;
        if ctx.db.game_item_enchantment().id().find(id).is_none() {
            ctx.db.game_item_enchantment().insert(ItemEnchantment {
                id,
                enchant_id,
                effect_index: 0,
                kind,
                amount,
                spell_id: 0,
                school_mask: 0,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::property_pick;

    #[test]
    fn every_input_follows_proportional_bands_in_property_order() {
        for roll in 0..10_000 {
            assert_eq!(
                property_pick(vec![(30, 30), (10, 10), (20, 20)], roll),
                Some(if roll < 1667 {
                    10
                } else if roll < 5000 {
                    20
                } else {
                    30
                })
            );
        }
    }

    #[test]
    fn underweight_and_overweight_pools_cover_every_input() {
        for roll in 0..10_000 {
            assert_eq!(
                property_pick(vec![(10, 4976), (20, 4976)], roll),
                Some(if roll < 5000 { 10 } else { 20 })
            );
            assert_eq!(
                property_pick(vec![(10, 4367), (20, 8734)], roll),
                Some(if roll < 3334 { 10 } else { 20 })
            );
        }
    }

    #[test]
    fn invalid_pool_or_input_has_no_selection() {
        for members in [vec![], vec![(0, 1)], vec![(1, 0)], vec![(1, 1), (1, 2)]] {
            assert_eq!(property_pick(members, 0), None);
        }
        assert_eq!(property_pick(vec![(1, 1)], 10_000), None);
        assert_eq!(
            property_pick(vec![(1, u32::MAX), (2, u32::MAX)], 9999),
            Some(2)
        );
    }
}
