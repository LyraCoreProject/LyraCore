use spacetimedb::ReducerContext;

use super::{refuse, LootRefusal, LootSource};
use crate::actor::ActionRefusal;
use crate::game_corpse_loot;
use crate::game_item_template;

pub(crate) fn take_item(
    ctx: &ReducerContext,
    looter_guid: u64,
    source_guid: u64,
    loot_slot: u8,
) -> Result<(), LootRefusal> {
    let (looter, source) = super::reach(ctx, looter_guid, source_guid)?;
    let source_is_corpse = matches!(source, LootSource::Corpse(_));
    let loot = ctx.db.game_corpse_loot();
    let row = loot
        .by_corpse()
        .filter(&source_guid)
        .find(|l| l.slot == loot_slot)
        .ok_or_else(|| refuse(LootRefusal::NothingToLoot, "no loot in that slot"))?;
    if !row.takeable_by(ctx, looter_guid) {
        let detail = if row.reserved_for != 0 && row.reserved_for != looter_guid {
            "this item is reserved for another player"
        } else if row.quest_only {
            "you do not need this quest item"
        } else if row.master_only {
            "this item requires the master looter to distribute it"
        } else {
            "this item belongs to another looter right now"
        };
        return Err(refuse(LootRefusal::ItemUnavailable, detail));
    }
    let tmpl = ctx
        .db
        .game_item_template()
        .entry()
        .find(row.item_entry)
        .ok_or_else(|| {
            refuse(
                LootRefusal::NothingToLoot,
                &format!("no template for item entry {}", row.item_entry),
            )
        })?;
    let count = row.count.max(1);
    // Grant before deleting the loot row; failed storage leaves it available for retry.
    let mut plan = crate::items::ItemStoragePlan::read(ctx, looter_guid, looter.owner_identity);
    plan.grant(ctx, &tmpl, count, false, Some(row.random_property_id))
        .and_then(|()| plan.commit(ctx))
        .map_err(|cause| refuse(LootRefusal::InventoryFull, &cause.to_string()))?;
    loot.id().delete(row.id);
    if row.quest_only && row.reserved_for == 0 {
        let clone = if source_is_corpse {
            super::clone_quest_loot_for_eligible
        } else {
            super::clone_quest_loot_for_group
        };
        clone(
            ctx,
            looter_guid,
            source_guid,
            row.item_entry,
            count,
            row.random_property_id,
        );
    }
    if source_is_corpse {
        super::refresh_lootable(ctx, source_guid);
    }
    crate::hooks::fire_on_loot(
        ctx,
        &crate::hooks::LootPayload {
            looter_guid,
            corpse_guid: source_guid,
            item_entry: row.item_entry,
            count,
        },
    );
    Ok(())
}

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn request_take_loot(
    ctx: &ReducerContext,
    looter_guid: u64,
    source_guid: u64,
    loot_slot: u8,
) -> Result<(), ActionRefusal> {
    Ok(take_item(ctx, looter_guid, source_guid, loot_slot)?)
}

pub(crate) fn apply_take_loot(
    ctx: &ReducerContext,
    looter_guid: u64,
    source_guid: u64,
    loot_slot: u8,
) -> Result<(), String> {
    Ok(take_item(ctx, looter_guid, source_guid, loot_slot)?)
}
