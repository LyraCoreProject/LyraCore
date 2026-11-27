use spacetimedb::{table, ReducerContext, Table};

use crate::{game_item_instance, game_item_template, ItemTemplate};

/// The item spell charges one item has spent; no row means it holds its template charges.
#[table(accessor = game_item_spell_charges, index(accessor = by_owner, btree(columns = [owner_guid])))]
pub struct ItemSpellCharges {
    #[primary_key]
    pub item_guid: u64,
    pub owner_guid: u64,
    pub spent: u32,
}

crate::outside_table_catalog!(game_item_spell_charges);

crate::character_owned!(delete, fn sweep_delete_game_item_spell_charges(ctx, character_guid) {
    let items: Vec<u64> = ctx
        .db
        .game_item_spell_charges()
        .by_owner()
        .filter(&character_guid)
        .map(|row| row.item_guid)
        .collect();
    for item_guid in items {
        forget_item(ctx, item_guid);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_item_spell_charges(ctx, character_guid, io) {
    table = game_item_spell_charges,
    by = by_owner,
    keep_key,
});

/// What one cast does to the item that started it, as vmangos `Spell::TakeCastItem` decides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CastItemUse {
    Keep,
    SpendCharge,
    UseUp,
}

fn template_charges(template: &ItemTemplate) -> [i32; 5] {
    [
        template.spellcharges_1,
        template.spellcharges_2,
        template.spellcharges_3,
        template.spellcharges_4,
        template.spellcharges_5,
    ]
}

fn limited_charges(template: &ItemTemplate) -> impl Iterator<Item = i32> {
    super::equip::spells(template)
        .into_iter()
        .zip(template_charges(template))
        .filter(|((spell, _), charges)| *spell != 0 && *charges != 0)
        .map(|(_, charges)| charges)
}

// vmangos stores spent charges only on an item that cannot stack.
fn keeps_spent_charges(template: &ItemTemplate) -> bool {
    template.max_stack < 2
}

fn remaining(charges: i32, spent: u32) -> u32 {
    charges.unsigned_abs().saturating_sub(spent)
}

/// The charges per spell slot that ITEM_FIELD_SPELL_CHARGES shows: what a limited spell has left,
/// signed as the template's. An item that stacks keeps its template charges.
pub(crate) fn displayed_charges(template: &ItemTemplate, spent: u32) -> [i32; 5] {
    let mut shown = template_charges(template);
    if keeps_spent_charges(template) {
        for ((spell, _), charges) in super::equip::spells(template).into_iter().zip(&mut shown) {
            if spell != 0 && *charges != 0 {
                *charges = charges.signum() * remaining(*charges, spent) as i32;
            }
        }
    }
    shown
}

/// vmangos `Spell::CheckItems`: false when a spell slot with charges has none left.
pub(crate) fn charges_remain(template: &ItemTemplate, spent: u32) -> bool {
    limited_charges(template).all(|charges| remaining(charges, spent) > 0)
}

/// vmangos `Spell::TakeCastItem`: only an expendable spell's last charge uses the item up.
pub(crate) fn cast_item_use(template: &ItemTemplate, spent: u32) -> CastItemUse {
    let mut expendable = false;
    let mut last_charge = false;
    let mut limited = false;
    for charges in limited_charges(template) {
        limited = true;
        expendable |= charges < 0;
        last_charge = remaining(charges, spent) <= 1;
    }
    if expendable && last_charge {
        CastItemUse::UseUp
    } else if limited && keeps_spent_charges(template) {
        CastItemUse::SpendCharge
    } else {
        CastItemUse::Keep
    }
}

pub(crate) fn spent_charges(ctx: &ReducerContext, item_guid: u64) -> u32 {
    ctx.db
        .game_item_spell_charges()
        .item_guid()
        .find(item_guid)
        .map_or(0, |row| row.spent)
}

/// The template and spent charges of the item `item_guid`; `None` when it or its template is gone.
fn charge_state(ctx: &ReducerContext, item_guid: u64) -> Option<(ItemTemplate, u32)> {
    let item = ctx.db.game_item_instance().guid().find(item_guid)?;
    let template = ctx.db.game_item_template().entry().find(item.entry)?;
    Some((template, spent_charges(ctx, item_guid)))
}

/// What a cast would do now to the item `item_guid`; a missing item is kept.
pub(crate) fn cast_item_use_of(ctx: &ReducerContext, item_guid: u64) -> CastItemUse {
    charge_state(ctx, item_guid).map_or(CastItemUse::Keep, |(template, spent)| {
        cast_item_use(&template, spent)
    })
}

/// Whether `item_guid` has a charge left for each limited spell; a missing item passes.
pub(crate) fn item_charges_remain(ctx: &ReducerContext, item_guid: u64) -> bool {
    charge_state(ctx, item_guid).is_none_or(|(template, spent)| charges_remain(&template, spent))
}

/// Whether the item `item_guid` has spent a spell charge, which mail and auctions cannot carry.
pub(crate) fn has_spent_charges(ctx: &ReducerContext, item_guid: u64) -> bool {
    ctx.db
        .game_item_spell_charges()
        .item_guid()
        .find(item_guid)
        .is_some()
}

/// Applies a cast's [`CastItemUse`] to an item that may be gone by now.
pub(crate) fn take_cast_item(ctx: &ReducerContext, item_guid: u64, item_use: CastItemUse) {
    match item_use {
        CastItemUse::Keep => {}
        CastItemUse::SpendCharge => {
            if let Some(item) = ctx.db.game_item_instance().guid().find(item_guid) {
                let spent = spent_charges(ctx, item_guid) + 1;
                record_spent(ctx, item_guid, item.owner_guid, spent);
            }
        }
        CastItemUse::UseUp => super::use_up_one(ctx, item_guid),
    }
}

/// Records the charges an item has spent; zero removes its row.
pub(crate) fn record_spent(ctx: &ReducerContext, item_guid: u64, owner_guid: u64, spent: u32) {
    let rows = ctx.db.game_item_spell_charges();
    let row = ItemSpellCharges {
        item_guid,
        owner_guid,
        spent,
    };
    if spent == 0 {
        rows.item_guid().delete(item_guid);
    } else if rows.item_guid().find(item_guid).is_some() {
        rows.item_guid().update(row);
    } else {
        rows.insert(row);
    }
    if let Some(item) = ctx.db.game_item_instance().guid().find(item_guid) {
        crate::native::inventory::spell_charges_changed(ctx, &item);
    }
}

pub(crate) fn forget_item(ctx: &ReducerContext, item_guid: u64) {
    ctx.db
        .game_item_spell_charges()
        .item_guid()
        .delete(item_guid);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(max_stack: u32, slots: [(u32, i32); 2]) -> ItemTemplate {
        ItemTemplate {
            max_stack,
            spellid_1: slots[0].0,
            spellcharges_1: slots[0].1,
            spellid_2: slots[1].0,
            spellcharges_2: slots[1].1,
            ..ItemTemplate::blank(1, "Fixture")
        }
    }

    #[test]
    fn an_item_spell_without_charges_keeps_its_item() {
        let trinket = template(1, [(100, 0), (0, 0)]);
        assert_eq!(cast_item_use(&trinket, 0), CastItemUse::Keep);
        assert!(charges_remain(&trinket, 7));
    }

    #[test]
    fn a_potion_stack_uses_up_one_item_per_cast() {
        let potion = template(5, [(100, -1), (0, 0)]);
        assert_eq!(cast_item_use(&potion, 0), CastItemUse::UseUp);
        assert!(charges_remain(&potion, 0));
    }

    #[test]
    fn an_expendable_item_with_three_charges_is_used_up_by_its_third_cast() {
        let wand = template(1, [(100, -3), (0, 0)]);
        assert_eq!(cast_item_use(&wand, 0), CastItemUse::SpendCharge);
        assert_eq!(cast_item_use(&wand, 1), CastItemUse::SpendCharge);
        assert_eq!(cast_item_use(&wand, 2), CastItemUse::UseUp);
    }

    #[test]
    fn positive_charges_count_down_and_the_item_stays_without_charges() {
        let rod = template(1, [(100, 2), (0, 0)]);
        assert_eq!(cast_item_use(&rod, 0), CastItemUse::SpendCharge);
        assert_eq!(cast_item_use(&rod, 1), CastItemUse::SpendCharge);
        assert!(charges_remain(&rod, 1));
        assert!(!charges_remain(&rod, 2));
    }

    #[test]
    fn a_stack_with_positive_charges_spends_nothing() {
        let stack = template(20, [(100, 3), (0, 0)]);
        assert_eq!(cast_item_use(&stack, 0), CastItemUse::Keep);
    }

    #[test]
    fn the_last_limited_spell_slot_decides_whether_the_charges_ran_out() {
        let item = template(1, [(100, -1), (200, 3)]);
        assert_eq!(cast_item_use(&item, 0), CastItemUse::SpendCharge);
        assert!(!charges_remain(&item, 1));
    }

    #[test]
    fn the_client_sees_the_charges_a_limited_spell_has_left() {
        let wand = template(1, [(100, -3), (200, 4)]);
        assert_eq!(displayed_charges(&wand, 0), [-3, 4, 0, 0, 0]);
        assert_eq!(displayed_charges(&wand, 2), [-1, 2, 0, 0, 0]);
        assert_eq!(displayed_charges(&wand, 9), [0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_stack_shows_its_template_charges_whatever_was_spent() {
        let potion = template(5, [(100, -1), (0, 0)]);
        assert_eq!(displayed_charges(&potion, 3), [-1, 0, 0, 0, 0]);
    }

    #[test]
    fn charges_on_an_empty_spell_slot_are_ignored() {
        let item = template(1, [(0, -1), (0, 0)]);
        assert_eq!(cast_item_use(&item, 0), CastItemUse::Keep);
    }
}
