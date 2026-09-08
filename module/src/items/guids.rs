//! Item identities issued from the Shard's durable GUID Range.

use spacetimedb::ReducerContext;

use super::tables::game_item_instance;

const ITEM_PREFIX: u64 = lyracore_shared::constants::starter_item::HIGHGUID_ITEM << 48;
const FORMAT_BIT: u64 = 1 << 47;
const MAX_LOW_ID: u64 = FORMAT_BIT - 1;

/// Reserve the complete batch before changing inventory. The shared high-water mark never
/// rewinds, so crafting and Trading cannot reuse identities deleted in the same transaction.
pub(crate) fn allocate_item_guids(ctx: &ReducerContext, count: usize) -> Result<Vec<u64>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let ids = crate::auth::reserve_guids(ctx, count as u64, MAX_LOW_ID)?;
    let guids: Vec<u64> = ids.map(|id| ITEM_PREFIX | FORMAT_BIT | id).collect();
    if let Some(guid) = guids
        .iter()
        .find(|guid| ctx.db.game_item_instance().guid().find(**guid).is_some())
    {
        return Err(format!("ITEM_GUID_CONFLICT: item {guid}"));
    }
    Ok(guids)
}

pub(crate) fn next_item_guid(ctx: &ReducerContext) -> Result<u64, String> {
    Ok(allocate_item_guids(ctx, 1)?[0])
}
