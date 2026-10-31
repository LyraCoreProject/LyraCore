use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp};

use super::{game_item_instance, ItemInstance};

// vmangos spell_enchant_charges: the charges a temporary enchantment spell puts on its item.
#[table(accessor = game_spell_enchant_charges)]
pub struct SpellEnchantCharges {
    #[primary_key]
    pub spell_id: u32,
    pub charges: u32,
}

// The schedule fires at `expires_at` and removes the enchantment.
#[table(accessor = game_temporary_enchantment, scheduled(expire_temporary_enchantment))]
pub struct TemporaryEnchantment {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    #[unique]
    pub item_guid: u64,
    pub enchant_id: u32,
    pub expires_at: Timestamp,
    // Zero means no charge limit.
    pub charges: u32,
}

pub(crate) fn temporary_enchantment(
    ctx: &ReducerContext,
    item_guid: u64,
) -> Option<TemporaryEnchantment> {
    ctx.db
        .game_temporary_enchantment()
        .item_guid()
        .find(item_guid)
}

pub(crate) fn temporary_enchant_id(ctx: &ReducerContext, item_guid: u64) -> u32 {
    temporary_enchantment(ctx, item_guid).map_or(0, |row| row.enchant_id)
}

// Follows vmangos Spell::EffectEnchantItemTmp: the new enchantment replaces the old one.
pub(crate) fn enchant_item_temporarily(
    ctx: &ReducerContext,
    item: &ItemInstance,
    spell_id: u32,
    enchant_id: u32,
    duration_ms: u32,
) {
    let charges = ctx
        .db
        .game_spell_enchant_charges()
        .spell_id()
        .find(spell_id)
        .map_or(0, |row| row.charges);
    let expires_at = crate::time::now(ctx)
        .checked_add(TimeDuration::from_micros(i64::from(duration_ms) * 1000))
        .unwrap_or(crate::time::now(ctx));
    let enchantments = ctx.db.game_temporary_enchantment();
    enchantments.item_guid().delete(item.guid);
    enchantments.insert(TemporaryEnchantment {
        scheduled_id: 0,
        scheduled_at: crate::time::schedule_time(ctx, expires_at),
        item_guid: item.guid,
        enchant_id,
        expires_at,
        charges,
    });
    changed(ctx, item);
}

// vmangos Player::CastItemCombatSpell spends a charge per proc and clears the slot on the last.
pub(crate) fn spend_charge(ctx: &ReducerContext, item_guid: u64) {
    let Some(mut row) = temporary_enchantment(ctx, item_guid) else {
        return;
    };
    match row.charges {
        0 => return,
        1 => {
            ctx.db
                .game_temporary_enchantment()
                .item_guid()
                .delete(item_guid);
        }
        _ => {
            row.charges -= 1;
            ctx.db
                .game_temporary_enchantment()
                .scheduled_id()
                .update(row);
        }
    }
    if let Some(item) = ctx.db.game_item_instance().guid().find(item_guid) {
        changed(ctx, &item);
    }
}

pub(crate) fn forget_item(ctx: &ReducerContext, item_guid: u64) {
    ctx.db
        .game_temporary_enchantment()
        .item_guid()
        .delete(item_guid);
}

#[reducer]
pub(crate) fn expire_temporary_enchantment(ctx: &ReducerContext, row: TemporaryEnchantment) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    ctx.db
        .game_temporary_enchantment()
        .scheduled_id()
        .delete(row.scheduled_id);
    if let Some(item) = ctx.db.game_item_instance().guid().find(row.item_guid) {
        changed(ctx, &item);
    }
}

fn changed(ctx: &ReducerContext, item: &ItemInstance) {
    crate::native::temporary_enchantment_changed(ctx, item);
    if item.slot <= super::equip_slot::END {
        crate::spell::recompute_vitals(ctx, item.owner_guid);
        crate::spell::recompute_sheet(ctx, item.owner_guid);
    }
}
