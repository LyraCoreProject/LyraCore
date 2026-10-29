use lyracore_shared::region::RegionMap;
use spacetimedb::{reducer, table, ReducerContext, Table};

pub(crate) fn region_key(map_id: u32, region_id: u32) -> u64 {
    ((map_id as u64) << 32) | region_id as u64
}

#[table(accessor = game_map_region)]
pub struct MapRegion {
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub region_id: u32,
    pub gx_min: i32,
    pub gx_max: i32,
    pub gy_min: i32,
    pub gy_max: i32,
}

#[table(accessor = game_region_assignment)]
pub struct RegionAssignment {
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub region_id: u32,
    pub shard: String,
    pub epoch: u64,
    pub updated_micros: i64,
}

#[reducer]
pub fn import_map_regions(ctx: &ReducerContext, packed: String) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let (map, rejected) = RegionMap::parse(&packed);
    if !rejected.is_empty() {
        return Err(format!(
            "seam menu rejected {} row(s): {}",
            rejected.len(),
            rejected.join("; ")
        ));
    }
    let regions = ctx.db.game_map_region();
    let keys: Vec<u64> = regions.iter().map(|r| r.key).collect();
    for k in keys {
        regions.key().delete(k);
    }
    for r in map.regions() {
        regions.insert(MapRegion {
            key: region_key(r.map_id, r.region_id),
            map_id: r.map_id,
            region_id: r.region_id,
            gx_min: r.gx_min,
            gx_max: r.gx_max,
            gy_min: r.gy_min,
            gy_max: r.gy_max,
        });
    }
    Ok(())
}

pub(crate) fn epoch_accepted(current: Option<u64>, incoming: u64) -> bool {
    current.is_none_or(|c| incoming > c)
}

pub(crate) fn assignment_write(
    current: Option<u64>,
    incoming: u64,
    shard: &str,
) -> Result<String, u64> {
    if !epoch_accepted(current, incoming) {
        return Err(current.unwrap_or(0));
    }
    Ok(shard.trim().to_string())
}

#[reducer]
pub fn set_region_assignment(
    ctx: &ReducerContext,
    map_id: u32,
    region_id: u32,
    shard: String,
    epoch: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if region_id == lyracore_shared::region::DEFAULT_REGION {
        return Err(format!(
            "region {} is \"the rest of the map\" and is never assignable — it always routes \
             through the (map, instance) shard map",
            lyracore_shared::region::DEFAULT_REGION
        ));
    }
    let table = ctx.db.game_region_assignment();
    let key = region_key(map_id, region_id);
    let current = table.key().find(key);
    let shard =
        assignment_write(current.as_ref().map(|e| e.epoch), epoch, &shard).map_err(|at| {
            format!(
            "stale epoch {epoch} for map {map_id} region {region_id}: the current assignment is \
             already at epoch {at}"
        )
        })?;
    let row = RegionAssignment {
        key,
        map_id,
        region_id,
        shard,
        epoch,
        updated_micros: crate::time::now(ctx).to_micros_since_unix_epoch(),
    };
    if current.is_some() {
        table.key().update(row);
    } else {
        table.insert(row);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_strictly_higher_epoch_may_replace_an_assignment() {
        assert!(
            epoch_accepted(None, 0),
            "the first assignment accepts any epoch"
        );
        assert!(epoch_accepted(None, 7));
        assert!(epoch_accepted(Some(1), 2));
        assert!(!epoch_accepted(Some(2), 1), "a stale retry must be refused");
        assert!(
            !epoch_accepted(Some(2), 2),
            "an equal epoch is a race, not a flip"
        );
        assert!(epoch_accepted(Some(u64::MAX - 1), u64::MAX));
    }

    #[test]
    fn an_unassign_is_a_tombstone_so_a_stale_retry_cannot_resurrect_the_old_assignment() {
        assert_eq!(
            assignment_write(None, 2, "pool-b"),
            Ok("pool-b".to_string())
        );
        assert_eq!(
            assignment_write(Some(2), 3, "  "),
            Ok(String::new()),
            "un-assign keeps the row"
        );
        assert_eq!(
            assignment_write(Some(3), 2, "pool-b"),
            Err(3),
            "the stale retry is refused"
        );
        assert_eq!(
            assignment_write(Some(3), 4, " pool-c "),
            Ok("pool-c".to_string())
        );
    }

    #[test]
    fn region_keys_are_unique_per_map_and_region() {
        let keys = [
            region_key(0, 1),
            region_key(0, 2),
            region_key(1, 1),
            region_key(1, 2),
            region_key(u32::MAX, u32::MAX),
        ];
        let unique: std::collections::HashSet<u64> = keys.iter().copied().collect();
        assert_eq!(
            unique.len(),
            keys.len(),
            "packed region keys must not collide"
        );
        assert_eq!(region_key(0, 1), 1);
        assert_eq!(region_key(1, 0), 1u64 << 32);
    }
}
