use spacetimedb::ReducerContext;

use super::{game_item_loot, game_loot_money, LootRow};
use crate::{game_corpse_loot, game_item_instance, game_item_template, game_lock};
use crate::{ItemInstance, WorldEntity};

const ITEM_FLAG_LOOTABLE: u32 = 0x4;
const INVTYPE_NON_EQUIP: u8 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OpenItemRefusal {
    ItemMissing,
    LooterDead,
    ItemLocked,
    NotLootable,
}

impl OpenItemRefusal {
    pub(crate) fn as_tag(self) -> &'static str {
        match self {
            Self::ItemMissing => "loot:item_missing",
            Self::LooterDead => "loot:looter_dead",
            Self::ItemLocked => "loot:item_locked",
            Self::NotLootable => "loot:item_not_lootable",
        }
    }
}

/// vmangos `HandleOpenItemOpcode` and the item branch of `Player::SendLoot`. The first open rolls
/// the item's loot and money, which stay on the item until it is emptied.
pub(crate) fn open_item(
    ctx: &ReducerContext,
    looter: &WorldEntity,
    item: Option<ItemInstance>,
) -> Result<(), OpenItemRefusal> {
    let item = item.ok_or(OpenItemRefusal::ItemMissing)?;
    let template = ctx
        .db
        .game_item_template()
        .entry()
        .find(item.entry)
        .ok_or(OpenItemRefusal::ItemMissing)?;
    if looter.dead {
        return Err(OpenItemRefusal::LooterDead);
    }
    if needs_lockpicking(ctx, template.lock_id) {
        return Err(OpenItemRefusal::ItemLocked);
    }
    // vmangos keeps ITEM_FLAG_LOOTABLE only on items that cannot be equipped.
    if template.item_flags & ITEM_FLAG_LOOTABLE == 0 || template.inventory_type != INVTYPE_NON_EQUIP
    {
        return Err(OpenItemRefusal::NotLootable);
    }
    if has_generated_loot(ctx, item.guid) {
        return Ok(());
    }
    let rows = ctx
        .db
        .game_item_loot()
        .by_lootable()
        .filter(&item.entry)
        .map(LootRow::from);
    let winners = super::roll_loot_rows(ctx, item.guid, rows);
    super::insert_corpse_rows(ctx, item.guid, winners);
    super::stock_purse(
        ctx,
        item.guid,
        item.owner_guid,
        (template.money_min, template.money_max),
    );
    Ok(())
}

/// vmangos `Item::HasGeneratedLoot`: the item was opened and still holds its rolled loot. Such an
/// item cannot be mailed, traded, auctioned, split or merged into.
pub(crate) fn has_generated_loot(ctx: &ReducerContext, item_guid: u64) -> bool {
    ctx.db
        .game_loot_money()
        .source_guid()
        .find(item_guid)
        .is_some()
}

// vmangos refuses a lock whose first two openers need a skill; no item here is ever unlocked.
fn needs_lockpicking(ctx: &ReducerContext, lock_id: u32) -> bool {
    if lock_id == 0 {
        return false;
    }
    let mut openers = ctx.db.game_lock().by_lock().filter(&lock_id).peekable();
    openers.peek().is_none() || openers.any(|row| row.index < 2 && row.required_skill != 0)
}

/// vmangos `DoLootRelease` destroys an opened item once nothing is left for its owner.
pub(crate) fn release_item(ctx: &ReducerContext, looter_guid: u64, item_guid: u64) {
    let held = ctx
        .db
        .game_item_instance()
        .guid()
        .find(item_guid)
        .is_some_and(|item| item.owner_guid == looter_guid);
    let emptied_purse = ctx
        .db
        .game_loot_money()
        .source_guid()
        .find(item_guid)
        .is_some_and(|purse| purse.money == 0);
    if !held || !emptied_purse {
        return;
    }
    if ctx
        .db
        .game_corpse_loot()
        .by_corpse()
        .filter(&item_guid)
        .any(|row| row.takeable_by(ctx, looter_guid))
    {
        return;
    }
    ItemInstance::delete(ctx, item_guid);
}

pub(crate) fn forget_item_loot(ctx: &ReducerContext, item_guid: u64) {
    ctx.db.game_loot_money().source_guid().delete(item_guid);
    let loot = ctx.db.game_corpse_loot();
    let rows: Vec<u64> = loot
        .by_corpse()
        .filter(&item_guid)
        .map(|row| row.id)
        .collect();
    for id in rows {
        loot.id().delete(id);
    }
}
