//! Read docs/danger-zones.md#vendor-stock-migration before changing restock.

use spacetimedb::{reducer, ReducerContext, Table};

use super::tables::{
    game_vendor_stock, game_vendor_stock_runtime, VendorStock, VendorStockRuntime,
};
use super::{ItemTemplate, NpcVendor};

#[reducer]
pub fn native_vendor_stock_runtime(ctx: &ReducerContext, runtime_id: u128) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let runtime = ctx.db.game_vendor_stock_runtime();
    if runtime
        .id()
        .find(0)
        .is_some_and(|row| row.runtime_id == runtime_id)
    {
        return Ok(());
    }
    let stock = ctx.db.game_vendor_stock();
    for row in stock.iter().collect::<Vec<_>>() {
        stock.key().delete(row.key);
    }
    runtime.id().delete(0);
    runtime.insert(VendorStockRuntime { id: 0, runtime_id });
    Ok(())
}

fn seconds(ctx: &ReducerContext) -> i64 {
    crate::time::now(ctx).to_micros_since_unix_epoch() / 1_000_000
}

fn spawn_guid(ctx: &ReducerContext, vendor: u64) -> u64 {
    use crate::game_creature_spawn;
    ctx.db
        .game_creature_spawn()
        .guid()
        .find(vendor)
        .filter(|spawn| spawn.instance_id == 0 && spawn.source_guid != 0)
        .map_or(vendor, |spawn| spawn.source_guid)
}

fn key(vendor: u64, item: u32) -> u128 {
    (u128::from(vendor) << 32) | u128::from(item)
}

pub(crate) fn available(
    ctx: &ReducerContext,
    vendor: u64,
    offer: &NpcVendor,
    item: &ItemTemplate,
) -> u32 {
    if offer.max_count == 0 {
        return u32::MAX;
    }
    if offer.restock_seconds == 0 {
        return offer.max_count;
    }
    let vendor = spawn_guid(ctx, vendor);
    let stock = ctx.db.game_vendor_stock();
    let Some(mut row) = stock.key().find(key(vendor, item.entry)) else {
        return offer.max_count;
    };
    let now = seconds(ctx);
    let elapsed = now.saturating_sub(row.last_increment_seconds).max(0) as u64;
    let increments = elapsed / u64::from(row.restock_seconds.max(1));
    if increments != 0 {
        let count = u64::from(row.count)
            .saturating_add(increments.saturating_mul(u64::from(item.buy_count.max(1))));
        if count >= u64::from(offer.max_count) {
            stock.key().delete(row.key);
            return offer.max_count;
        }
        row.count = count as u32;
        row.last_increment_seconds = now;
        let count = row.count;
        stock.key().update(row);
        return count;
    }
    row.count
}

fn restock_delay(seconds: u32, flags: u32, random_percent: u32, population: usize) -> u32 {
    let mut delay = seconds;
    if flags & 1 != 0 {
        delay = (delay as f32 * (random_percent as f32 / 100.0)) as u32;
    }
    if flags & 2 != 0 && population > 2500 {
        delay = (delay as f32 * (2500.0 / population as f32)) as u32;
    }
    delay.max(1)
}

pub(crate) fn purchase(ctx: &ReducerContext, vendor: u64, offer: &NpcVendor, remaining_count: u32) {
    if offer.max_count == 0 || offer.restock_seconds == 0 {
        return;
    }
    let vendor = spawn_guid(ctx, vendor);
    let population = if offer.stock_flags & 2 != 0 {
        crate::native::vendor_session_population(ctx)
    } else {
        0
    };
    let random_percent = if offer.stock_flags & 1 != 0 {
        80 + ctx.random::<u32>() % 41
    } else {
        100
    };
    let row = VendorStock {
        key: key(vendor, offer.item_entry),
        vendor_guid: vendor,
        count: remaining_count,
        last_increment_seconds: seconds(ctx),
        restock_seconds: restock_delay(
            offer.restock_seconds,
            offer.stock_flags,
            random_percent,
            population,
        ),
    };
    let stock = ctx.db.game_vendor_stock();
    stock.key().delete(row.key);
    stock.insert(row);
}

pub(crate) fn forget(ctx: &ReducerContext, vendor: u64) {
    let vendor = spawn_guid(ctx, vendor);
    let stock = ctx.db.game_vendor_stock();
    for row in stock.by_vendor().filter(vendor).collect::<Vec<_>>() {
        stock.key().delete(row.key);
    }
}

#[cfg(test)]
mod tests {
    use super::restock_delay;

    #[test]
    fn stock_flags_choose_an_integer_delay_at_each_purchase() {
        assert_eq!(restock_delay(101, 0, 80, 5000), 101);
        assert_eq!(restock_delay(101, 1, 80, 5000), 80);
        assert_eq!(restock_delay(101, 1, 120, 5000), 121);
        assert_eq!(restock_delay(101, 2, 80, 2500), 101);
        assert_eq!(restock_delay(101, 2, 80, 5000), 50);
        assert_eq!(restock_delay(101, 3, 120, 5000), 60);
        assert_eq!(restock_delay(1, 1, 80, 0), 1);
        assert_eq!(restock_delay(1, 2, 100, 5000), 1);
    }
}
