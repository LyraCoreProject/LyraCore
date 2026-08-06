//! Regions: the cell→region→shard hierarchy as DATA (issue #23, spec #12).
//!
//! Two tables, one job each, and **no game logic in this file reads either of them**:
//!
//! - [`MapRegion`] — the baked region definitions (`region_of(map_id, cell)`), content data loaded
//!   by [`import_map_regions`] from the seam-menu format in `docs/region-sharding.md`. Imported
//!   alongside the world ETL that bakes `grid_x`/`grid_y`, because it is the same kind of thing: a
//!   pre-computed spatial fact, not something gameplay derives at runtime.
//! - [`RegionAssignment`] — `{ map_id, region_id, shard, epoch }`, the epoch-versioned ops table
//!   that says which DATABASE owns a region. It is authoritative **on realm-core** (the same wasm
//!   published under another database name — see `realm_core.rs` for why that is the whole
//!   mechanism), and the gateway subscribes to it.
//!
//! # Why the module never reads a shard id
//!
//! `shard` is a database name: a GATEWAY routing fact. The moment a reducer branches on it, the
//! module stops being relocatable and the world stops being shardable — so `tripwires.rs`'s
//! `partition_discipline_tripwire` fails the build if the accessor or the `.shard` field is touched
//! anywhere but this file. The reducer below writes it; nothing reads it.
//!
//! # What this ticket deliberately does NOT do
//!
//! Nothing here migrates anything. With every region of a map assigned to one shard — or with no
//! assignment rows at all, the default — routing is a strict no-op and the seams are dormant.
//! View-merge, warm handoff, relayed intents and live migration are later tickets, gated on the
//! #18 benchmark showing a tuned writer actually saturating.

use lyracore_shared::region::RegionMap;
use spacetimedb::{reducer, table, ReducerContext, Table};

/// `(map_id, region_id)` as one u64 primary key. Same trick as `terrain::cell_key` — SpacetimeDB
/// takes a single `#[primary_key]` column, and a packed key is cheaper than a scan-and-match.
pub fn region_key(map_id: u32, region_id: u32) -> u64 {
    ((map_id as u64) << 32) | region_id as u64
}

/// One region definition: `region_id` on `map_id` covers the inclusive cell rectangle
/// `[gx_min..=gx_max] × [gy_min..=gy_max]`. Content data — replaced wholesale by
/// [`import_map_regions`], never written at runtime. [static]
///
/// PRIVATE (no `public`): the seam menu is operations data. It is harmless in itself, but a client
/// has no use for it and the gateway's coordinator connection (owner token, RLS bypass) reads it
/// exactly the way it reads `game_character_shard`.
#[table(accessor = game_map_region)]
pub struct MapRegion {
    /// [`region_key`]`(map_id, region_id)`.
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub region_id: u32,
    pub gx_min: i32,
    pub gx_max: i32,
    pub gy_min: i32,
    pub gy_max: i32,
}

/// `region_assignment { map_id, region_id, shard, epoch }` — which database owns a region, and at
/// which epoch. Authoritative on realm-core; inert everywhere else (nothing writes it there).
///
/// **Epoch semantics.** `epoch` is a monotonic version stamp per `(map_id, region_id)`: a flip must
/// carry a STRICTLY higher epoch than the row it replaces, so a retried or reordered operator call
/// can never resurrect a superseded assignment. A flip re-routes **new entrants only** — the gateway
/// resolves a region→shard exactly once per world entry — and this ticket moves no resident.
///
/// PRIVATE: this table names the realm's databases. Never hand a client the topology.
#[table(accessor = game_region_assignment)]
pub struct RegionAssignment {
    /// [`region_key`]`(map_id, region_id)`.
    #[primary_key]
    pub key: u64,
    pub map_id: u32,
    pub region_id: u32,
    /// The DATABASE NAME that owns this region — gateway routing data. No module game logic reads
    /// this column (`partition_discipline_tripwire` enforces it).
    pub shard: String,
    /// Monotonic per `(map_id, region_id)`; see the type docs.
    pub epoch: u64,
    /// When the flip landed — diagnostics only.
    pub updated_micros: i64,
}

/// Replace the realm's whole seam menu with `packed` (the `docs/region-sharding.md` format).
/// Operator-only, like every other importer.
///
/// Rejects the WHOLE payload if any line is malformed or breaks the geometry rules (overlap, the
/// ~10×10 cell floor, a reserved region id) rather than importing a partial menu: this is content
/// data with an operator watching, so failing loudly beats a silently half-drawn map. A dropped row
/// would leave its cells on region 0, which routes through the plain shard map — safe, but not what
/// the operator asked for.
// Deliberate simplification: replace-everything, no append variant and no per-map scoping. A
// realm's entire seam menu is a few dozen lines of text (~3-6 useful regions per zone, per the
// spec's floor analysis), so it fits in one `spacetime call` argument with room to spare. Ceiling:
// importing one zone's menu means passing the whole realm's file. Upgrade path: the `_append`
// twin `terrain.rs` already demonstrates, if a menu ever outgrows one call.
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

/// The epoch rule, as a pure function so it can be tested without a node: an assignment may only be
/// replaced by a STRICTLY higher epoch. The first assignment for a region accepts any epoch (there
/// is nothing to supersede). Equal is rejected too — two operators picking the same number is a
/// race, and the second one should hear about it rather than silently win or silently lose.
pub(crate) fn epoch_accepted(current: Option<u64>, incoming: u64) -> bool {
    current.is_none_or(|c| incoming > c)
}

/// What [`set_region_assignment`] must write, decided PURELY (the reducer only executes it) so the
/// epoch rule *and* the shape of an un-assign are testable without a node.
///
/// `Ok(shard)` — store this shard name at the incoming epoch. **`Ok("")` is a TOMBSTONE**: an
/// un-assigned region, which the gateway routes exactly like a missing row. `Err(current)` — refuse,
/// the stored row is already at that epoch or newer.
///
/// An un-assign is a tombstone rather than a delete, and that is the whole point: deleting the row
/// would drop the epoch high-water mark with it, so the very next stale retry
/// (`set_region_assignment 0 2 pool-b 1` after an un-assign at epoch 2) would find `current = None`,
/// be accepted, and RESURRECT the assignment the un-assign just superseded. A tombstone keeps the
/// mark, and the gateway already treats an empty shard as "no region opinion", so routing is
/// unchanged either way.
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

/// Flip a region's shard assignment (operator-only). Called against **realm-core**.
///
/// - `epoch` must be strictly greater than the current row's, so a stale retry cannot undo a newer
///   flip. Equal or lower is an error, not a silent no-op — an operator who sees "stale epoch"
///   knows someone else moved first.
/// - An EMPTY `shard` un-assigns the region: the row is kept as a TOMBSTONE at the new epoch and the
///   region falls back to the ordinary `(map_id, instance_id)` shard map, i.e. to today's behavior.
///   That is how a demo flip is reverted, and it costs an epoch bump so the revert itself is
///   ordered — and, because the row survives, so is everything after it ([`assignment_write`]).
/// - The region's GEOMETRY is deliberately not checked: definitions are imported on the world
///   shards by the ETL, and realm-core does not run it. An assignment naming a region no gateway
///   can resolve routes nobody, which is the same no-op as having no row at all.
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
        updated_micros: ctx.timestamp.to_micros_since_unix_epoch(),
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
        // Epoch semantics, module half: a retried or reordered operator call can never resurrect a
        // superseded assignment, and two operators picking the same number is an error rather than
        // a coin flip. The gateway half (highest epoch wins among the rows it can see) is pinned by
        // `the_highest_epoch_wins_...` in `gateway/src/config.rs`.
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
        // The adversarial case the delete-on-unassign form got WRONG:
        //   1. flip region 2 to pool-b at epoch 2, 2. flip back (empty shard) at epoch 3,
        //   3. a retried/reordered `… pool-b 2` arrives.
        // With the row deleted at step 2, step 3 sees `current = None`, is ACCEPTED, and the region
        // is silently back on pool-b at a superseded epoch. With the tombstone, it is refused.
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
        // And the tombstone is not a wall: the operator's next real flip still lands.
        assert_eq!(
            assignment_write(Some(3), 4, " pool-c "),
            Ok("pool-c".to_string())
        );
    }

    #[test]
    fn the_reducer_routes_every_write_through_the_epoch_rule() {
        // The call-site tripwire (the pattern `transfer.rs` uses for its fences): the pure rules
        // above are worthless if the reducer stops asking them, and the reducer itself cannot run
        // without a node — so deleting `assignment_write` from `set_region_assignment`, or writing
        // the row without it, turns THIS test red instead of leaving the whole suite green.
        let src = include_str!("region.rs");
        let body = src
            .split_once("pub fn set_region_assignment")
            .expect("the reducer is still named set_region_assignment")
            .1;
        let body = body.split_once("#[cfg(test)]").map_or(body, |(b, _)| b);
        let code: String = body
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            code.contains("assignment_write("),
            "set_region_assignment must decide its write with `assignment_write` — the epoch rule \
             and the tombstone form are only enforced if the reducer actually calls it"
        );
        assert!(
            !code.contains("delete(&key)"),
            "an un-assign must leave a TOMBSTONE, not delete the row: deleting it drops the epoch \
             high-water mark and a stale retry can then resurrect the superseded assignment"
        );
    }

    #[test]
    fn region_keys_are_unique_per_map_and_region() {
        // The packed key is the primary key: a collision would let one region silently overwrite
        // another's definition or assignment. `region_id` occupies the low half, `map_id` the high.
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
