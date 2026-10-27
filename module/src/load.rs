use spacetimedb::{reducer, table, ReducerContext, Table};

pub const SHARD_LOAD_RING: usize = 20;
pub const REGION_LOAD_RING: usize = 20;

const MAX_SANE_OCCUPANCY_PCT: f32 = 1000.0;

#[table(accessor = game_shard_load)]
pub struct ShardLoad {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub shard: String,
    pub sampled_at_micros: i64,
    pub writer_occupancy_pct: f32,
    pub sessions: u32,
    #[default(0u64)]
    pub gateway_key: u64,
}

#[table(accessor = game_shard_load_total)]
pub struct ShardLoadTotal {
    #[primary_key]
    pub shard: String,
    pub sessions: u32,
    pub updated_at_micros: i64,
}

#[table(accessor = game_region_load)]
pub struct RegionLoad {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub map_id: u32,
    pub region_id: u32,
    pub sampled_at_micros: i64,
    pub players: u32,
}

pub(crate) fn ring_evict(existing_ids_ascending: &[u64], ring_size: usize) -> &[u64] {
    let keep = ring_size.saturating_sub(1);
    let over = existing_ids_ascending.len().saturating_sub(keep);
    &existing_ids_ascending[..over]
}

pub(crate) fn realm_wide_sessions(samples: impl Iterator<Item = (u64, u64, u32)>) -> u32 {
    let mut latest_per_gateway: std::collections::HashMap<u64, (u64, u32)> = Default::default();
    for (gateway_key, id, sessions) in samples {
        latest_per_gateway
            .entry(gateway_key)
            .and_modify(|(best_id, best_sessions)| {
                if id > *best_id {
                    *best_id = id;
                    *best_sessions = sessions;
                }
            })
            .or_insert((id, sessions));
    }
    latest_per_gateway
        .values()
        .map(|(_, sessions)| *sessions)
        .sum()
}

pub(crate) fn validate_occupancy_pct(pct: f32) -> Result<(), String> {
    if !pct.is_finite() || !(0.0..=MAX_SANE_OCCUPANCY_PCT).contains(&pct) {
        return Err(format!(
            "writer_occupancy_pct must be finite and within [0, {MAX_SANE_OCCUPANCY_PCT}], got {pct}"
        ));
    }
    Ok(())
}

#[reducer]
pub fn record_shard_load(
    ctx: &ReducerContext,
    shard: String,
    writer_occupancy_pct: f32,
    sessions: u32,
    gateway_key: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let shard = shard.trim().to_string();
    if shard.is_empty() {
        return Err("shard must not be empty".to_string());
    }
    validate_occupancy_pct(writer_occupancy_pct)?;
    let table = ctx.db.game_shard_load();
    // Keep one ring per Gateway process so another Gateway's samples cannot evict its history.
    let mut existing: Vec<u64> = table
        .iter()
        .filter(|r| r.shard == shard && r.gateway_key == gateway_key)
        .map(|r| r.id)
        .collect();
    existing.sort_unstable();
    for id in ring_evict(&existing, SHARD_LOAD_RING) {
        table.id().delete(id);
    }
    table.insert(ShardLoad {
        id: 0,
        shard: shard.clone(),
        sampled_at_micros: ctx.timestamp.to_micros_since_unix_epoch(),
        writer_occupancy_pct,
        sessions,
        gateway_key,
    });
    let total_sessions = realm_wide_sessions(
        table
            .iter()
            .filter(|r| r.shard == shard)
            .map(|r| (r.gateway_key, r.id, r.sessions)),
    );
    let totals = ctx.db.game_shard_load_total();
    let total_row = ShardLoadTotal {
        shard: shard.clone(),
        sessions: total_sessions,
        updated_at_micros: ctx.timestamp.to_micros_since_unix_epoch(),
    };
    if totals.shard().find(&shard).is_some() {
        totals.shard().update(total_row);
    } else {
        totals.insert(total_row);
    }
    Ok(())
}

#[reducer]
pub fn record_region_load(
    ctx: &ReducerContext,
    map_id: u32,
    region_id: u32,
    players: u32,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let table = ctx.db.game_region_load();
    let mut existing: Vec<u64> = table
        .iter()
        .filter(|r| r.map_id == map_id && r.region_id == region_id)
        .map(|r| r.id)
        .collect();
    existing.sort_unstable();
    for id in ring_evict(&existing, REGION_LOAD_RING) {
        table.id().delete(id);
    }
    table.insert(RegionLoad {
        id: 0,
        map_id,
        region_id,
        sampled_at_micros: ctx.timestamp.to_micros_since_unix_epoch(),
        players,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_evict_keeps_exactly_enough_room_for_the_row_about_to_be_inserted() {
        assert_eq!(ring_evict(&[], 20), &[] as &[u64]);
        assert_eq!(
            ring_evict(&[1, 2], 3),
            &[] as &[u64],
            "under cap: nothing evicted yet"
        );
        assert_eq!(
            ring_evict(&[1, 2, 3], 3),
            &[1],
            "at cap: the oldest makes room for the row about to land"
        );
        assert_eq!(
            ring_evict(&[1, 2, 3, 4], 3),
            &[1, 2],
            "self-heals a ring already over cap (e.g. after ring_size shrank)"
        );
        assert_eq!(
            ring_evict(&[1, 2, 3], 1),
            &[1, 2, 3],
            "a ring of 1 evicts every existing row — only the incoming one survives"
        );
    }

    #[test]
    fn realm_wide_sessions_sums_only_the_latest_row_per_gateway() {
        assert_eq!(realm_wide_sessions(std::iter::empty()), 0);
        assert_eq!(
            realm_wide_sessions(vec![(1, 10, 5), (1, 11, 7), (1, 12, 9)].into_iter()),
            9,
            "a single gateway's older samples must not be double-counted"
        );
        assert_eq!(
            realm_wide_sessions(vec![(1, 10, 5), (2, 11, 7)].into_iter()),
            12,
            "distinct gateways' latest samples sum to the realm-wide total"
        );
        assert_eq!(
            realm_wide_sessions(vec![(1, 12, 9), (1, 10, 5), (2, 11, 7)].into_iter()),
            16,
            "highest id wins regardless of iteration order"
        );
    }

    #[test]
    fn validate_occupancy_pct_rejects_non_finite_negative_and_absurd_values() {
        assert!(validate_occupancy_pct(0.0).is_ok());
        assert!(validate_occupancy_pct(57.3).is_ok());
        assert!(
            validate_occupancy_pct(MAX_SANE_OCCUPANCY_PCT).is_ok(),
            "the boundary is accepted"
        );
        for (pct, shown) in [
            (-0.1, "-0.1"),
            (f32::NAN, "NaN"),
            (f32::INFINITY, "inf"),
            (1000.1, "1000.1"),
        ] {
            assert_eq!(
                validate_occupancy_pct(pct),
                Err(format!(
                    "writer_occupancy_pct must be finite and within [0, 1000], got {shown}"
                ))
            );
        }
    }
}
