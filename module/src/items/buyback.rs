use spacetimedb::{table, ReducerContext, Table, Timestamp};

use super::{game_character_buyback, ItemInstance, ItemTemplate};

pub(crate) const BUYBACK_START: u8 = 69;
pub(crate) const BUYBACK_END: u8 = 81;

#[table(accessor = game_buyback_item, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct BuybackItem {
    #[primary_key]
    pub item_guid: u64,
    pub character_guid: u64,
    pub slot: u8,
    pub price: u32,
    pub timestamp: u32,
    pub item: ItemInstance,
    pub temporary_enchantment: Option<super::temporary_enchantment::PausedEnchantment>,
}

#[table(accessor = game_buyback_state)]
pub struct BuybackState {
    #[primary_key]
    pub character_guid: u64,
    pub login_at: Timestamp,
    pub current_slot: u8,
}

pub(crate) fn begin_session(ctx: &ReducerContext, character_guid: u64) {
    clear(ctx, character_guid);
    ctx.db.game_buyback_state().insert(BuybackState {
        character_guid,
        login_at: crate::time::now(ctx),
        current_slot: BUYBACK_START,
    });
}

pub(crate) fn entries(ctx: &ReducerContext, character_guid: u64) -> Vec<BuybackItem> {
    let mut rows: Vec<_> = ctx
        .db
        .game_buyback_item()
        .by_character()
        .filter(character_guid)
        .collect();
    rows.sort_by_key(|row| row.slot);
    rows
}

pub(crate) fn clear(ctx: &ReducerContext, character_guid: u64) {
    for row in entries(ctx, character_guid) {
        ctx.db.game_buyback_item().item_guid().delete(row.item_guid);
        super::forget_temporary_enchantment(ctx, row.item_guid);
        crate::mail_text::drop_letter_text(ctx, row.item.item_text_id);
        if !crate::transfer::is_in_transit(ctx, character_guid) {
            crate::native::buyback_removed(ctx, &row);
            crate::native::buyback_destroyed(ctx, &row);
        }
    }
    ctx.db
        .game_buyback_state()
        .character_guid()
        .delete(character_guid);
    for row in ctx
        .db
        .game_character_buyback()
        .by_player_guid()
        .filter(character_guid)
        .collect::<Vec<_>>()
    {
        ctx.db.game_character_buyback().id().delete(row.id);
    }
}

pub(crate) fn discard_legacy(ctx: &ReducerContext) {
    for guid in crate::native::world_session_characters(ctx) {
        if ctx
            .db
            .game_buyback_state()
            .character_guid()
            .find(guid)
            .is_none()
        {
            crate::native::end_world_session(ctx, guid);
        }
    }
    let rows = ctx.db.game_character_buyback();
    for row in rows.iter().collect::<Vec<_>>() {
        rows.id().delete(row.id);
    }
}

fn next_slot(current: u8, rows: &[BuybackItem]) -> u8 {
    let occupied = |slot| rows.iter().any(|row| row.slot == slot);
    if !occupied(current) {
        return current;
    }
    (BUYBACK_START + 1..BUYBACK_END)
        .find(|slot| !occupied(*slot))
        .unwrap_or_else(|| {
            (BUYBACK_START..BUYBACK_END)
                .min_by_key(|slot| {
                    rows.iter()
                        .find(|row| row.slot == *slot)
                        .map_or(0, |row| row.timestamp)
                })
                .expect("buyback has twelve slots")
        })
}

pub(super) fn sell(
    ctx: &ReducerContext,
    original: ItemInstance,
    template: &ItemTemplate,
    count: u32,
    price: u32,
) -> Result<(), String> {
    let character_guid = original.owner_guid;
    let mut item = original.clone();
    let partial = count < original.stack_count;
    if partial {
        item.guid = super::next_item_guid(ctx)?;
        item.stack_count = count;
        item.durability = template.max_durability;
        item.enchant_id = 0;
        item.item_text_id = 0;
        item.created_at = crate::time::now(ctx);
    }
    item.slot = u8::MAX;
    let mut state = ctx
        .db
        .game_buyback_state()
        .character_guid()
        .find(character_guid)
        .unwrap_or(BuybackState {
            character_guid,
            login_at: crate::time::now(ctx),
            current_slot: BUYBACK_START,
        });
    let old = entries(ctx, character_guid);
    let current_occupied = old.iter().any(|row| row.slot == state.current_slot);
    let slot = next_slot(state.current_slot, &old);
    let evicted = old.into_iter().find(|row| row.slot == slot);
    if let Some(row) = &evicted {
        ctx.db.game_buyback_item().item_guid().delete(row.item_guid);
        super::forget_temporary_enchantment(ctx, row.item_guid);
        crate::mail_text::drop_letter_text(ctx, row.item.item_text_id);
    }
    if current_occupied {
        state.current_slot = slot;
    }
    if state.current_slot < BUYBACK_END - 1 {
        state.current_slot += 1;
    }
    let elapsed =
        crate::time::now_secs(ctx) as i64 - state.login_at.to_micros_since_unix_epoch() / 1_000_000;
    let row = ctx.db.game_buyback_item().insert(BuybackItem {
        item_guid: item.guid,
        character_guid,
        slot,
        price,
        timestamp: (elapsed as u32).wrapping_add(30 * 3600),
        item,
        temporary_enchantment: if partial {
            None
        } else {
            super::temporary_enchantment::pause(ctx, original.guid)
        },
    });
    if ctx
        .db
        .game_buyback_state()
        .character_guid()
        .find(character_guid)
        .is_some()
    {
        ctx.db.game_buyback_state().character_guid().update(state);
    } else {
        ctx.db.game_buyback_state().insert(state);
    }
    let remaining = if partial {
        let mut remaining = original.clone();
        remaining.stack_count -= count;
        Some(remaining)
    } else {
        None
    };
    original.sell(ctx, remaining, &row, evicted.as_ref());
    Ok(())
}

pub(super) fn take(ctx: &ReducerContext, row: &BuybackItem) {
    ctx.db.game_buyback_item().item_guid().delete(row.item_guid);
    if let Some(mut state) = ctx
        .db
        .game_buyback_state()
        .character_guid()
        .find(row.character_guid)
    {
        if entries(ctx, row.character_guid)
            .iter()
            .any(|item| item.slot == state.current_slot)
        {
            state.current_slot = row.slot;
            ctx.db.game_buyback_state().character_guid().update(state);
        }
    }
    crate::native::buyback_removed(ctx, row);
}

crate::character_owned!(delete, fn sweep_delete_game_buyback_item(ctx, character_guid) {
    clear(ctx, character_guid);
});
crate::character_owned!(delete, fn sweep_delete_game_buyback_state(ctx, character_guid) {
    ctx.db.game_buyback_state().character_guid().delete(character_guid);
});
crate::character_owned!(transfer, fn sweep_transfer_game_buyback_item(ctx, character_guid, io) {
    table = game_buyback_item,
    by = by_character,
    keep_key,
});
crate::character_owned!(transfer, fn sweep_transfer_game_buyback_state(ctx, character_guid, io) {
    table = game_buyback_state,
    primary_key = character_guid,
});
crate::character_owned!(restamp, fn sweep_restamp_game_buyback_item(ctx, character_guid, identity) {
    for mut row in entries(ctx, character_guid) {
        if row.item.owner_identity != identity {
            row.item.owner_identity = identity;
            ctx.db.game_buyback_item().item_guid().update(row);
        }
    }
});
