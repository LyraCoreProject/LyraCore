//! Per-shard writer occupancy and per-region population, queryable as plain SQL.
//!
//! The GATEWAY is the one component that can see the whole realm — every shard's `/v1/metrics`
//! endpoint, every session, every live position — so it is the one that SAMPLES (a periodic
//! background task, `gateway/src/load_sample.rs`). This module only RECORDS what it is told, the
//! same split `region.rs` uses for the assignment table: an operator-gated write, authoritative on
//! realm-core, inert everywhere else (nothing calls these reducers on a world shard's own
//! connection in production, but nothing would break if something did — the module never reads a
//! shard id, so it has no idea which database it is running on).
//!
//! Two tables, RING-BUFFERED rather than TTL-reaped — each keeps the last [`SHARD_LOAD_RING`] /
//! [`REGION_LOAD_RING`] samples per key, oldest evicted on insert. The reason this is
//! spelled out: a `game_*` table nothing ever reaps grows forever, and a scheduled reaper is not the
//! only way to avoid that — bounding the table AT WRITE TIME needs no schedule, no GC pass, and no
//! extra reducer. At the gateway's default 30s sample cadence, 20 samples is ~10 minutes of
//! history: enough to eyeball a sustained trend against a momentary spike without a single `ORDER
//! BY` (`spacetime sql` has none — `docs/danger-zones.md` §2).
//!
//! See `docs/region-sharding.md` for the two `spacetime sql` queries an operator runs against this
//! data, and `docs/danger-zones.md` §1.2 for why these are hand-authored gateway bindings rather
//! than a `spacetime generate` regen.

use spacetimedb::{reducer, table, ReducerContext, Table};

/// Keep at most this many samples per shard (~10 minutes at the gateway's default 30s cadence).
pub const SHARD_LOAD_RING: usize = 20;
/// Same, per `(map_id, region_id)`.
pub const REGION_LOAD_RING: usize = 20;

/// Occupancy readings above this are almost certainly a unit error (a fraction instead of a
/// percentage, or raw seconds instead of a percentage) rather than a real writer — refuse rather
/// than record garbage an operator would otherwise have to notice by eye.
const MAX_SANE_OCCUPANCY_PCT: f32 = 1000.0;

/// One periodic sample of a world shard's writer: `spacetime_txn_cpu_time_sec_sum` delta ÷
/// wall-clock delta (the same computation the internal capacity-benchmark tool reads from the
/// node's `/v1/metrics`, sampled continuously here instead of only during a deliberate ramp) plus
/// the session count the gateway already tracks per shard.
///
/// PRIVATE (no `public`): ops telemetry, not game state — same rationale as
/// `game_region_assignment`. The gateway's coordinator connection (owner token, RLS bypass) reads
/// it exactly the way it reads that table, and an operator reads it with `spacetime sql`.
#[table(accessor = game_shard_load)]
pub struct ShardLoad {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub shard: String,
    pub sampled_at_micros: i64,
    pub writer_occupancy_pct: f32,
    pub sessions: u32,
    /// Which GATEWAY PROCESS wrote this sample. With N gateway processes sampling the
    /// same shard, `sessions` is only ever that ONE process's own player-connection cache size
    /// (`Coordinator::session_count`) — never the shard's realm-wide total. Keying the ring by
    /// `(shard, gateway_key)` instead of `shard` alone means every gateway's samples survive
    /// side-by-side rather than the last writer clobbering the rest, so an operator (or a future
    /// realm-list balancer) can sum the latest row per `gateway_key` to get the real total.
    ///
    /// A deterministic (FNV-1a) hash of the gateway's own `LYRACORE_GATEWAY_ID` (config-supplied,
    /// default hostname:port — `load_sample::gateway_key`), not the raw string: END-appending a
    /// `String` column here cannot be defaulted (`#[default(...)]` cannot default a `String` —
    /// `docs/danger-zones.md` §1.2), so the identity crosses the wire as the number instead. `0` is
    /// what every pre-migration row reads as — an "unknown gateway" bucket, not a collision with a
    /// real one. A hash rather than a registry-assigned id trades a (negligible, at realm-gateway
    /// counts) collision risk for needing no extra table or find-or-insert reducer; it is an
    /// internal telemetry key, not game state.
    #[default(0u64)]
    pub gateway_key: u64,
}

/// The realm-wide total for one shard, MATERIALIZED at write time (the actual
/// Done-when: `game_shard_load` alone is per-`(shard, gateway_key)` rows, and `spacetime sql` has
/// no `GROUP BY`/`SUM` (`docs/danger-zones.md` §2) to fold those into a total on read — so
/// [`record_shard_load`] recomputes and upserts this row on every sample instead, and an operator
/// queries `game_shard_load_total` directly for the number the issue asks for.
///
/// PRIVATE: same rationale as [`ShardLoad`].
#[table(accessor = game_shard_load_total)]
pub struct ShardLoadTotal {
    #[primary_key]
    pub shard: String,
    /// Sum, across every `gateway_key` that has ever sampled this shard, of that gateway's
    /// LATEST `sessions` reading — see [`realm_wide_sessions`].
    pub sessions: u32,
    pub updated_at_micros: i64,
}

/// One periodic sample of a region's open-world player population, bucketed by the gateway from
/// live session positions against the region definitions (`game_map_region` — see `crate::region`
/// for the hierarchy). Approximate by construction: a player mid-move between two samples is
/// counted wherever they were AT the sample instant, and only assignable regions are ever recorded
/// — the catch-all "rest of the map" bucket (`lyracore_shared::region::DEFAULT_REGION`) answers nothing
/// an operator can act on, so the gateway never sends it here.
///
/// PRIVATE: same rationale as [`ShardLoad`].
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

/// Which existing ids (already ASCENDING = oldest first) must be evicted before inserting one more
/// row, so the ring never holds more than `ring_size` rows for one key — including self-healing a
/// ring that is already over cap (e.g. after `ring_size` shrinks in a future edit). Pure so it is
/// testable without a `ReducerContext` (this crate's convention: never mock one, extract a pure
/// function instead).
pub(crate) fn ring_evict(existing_ids_ascending: &[u64], ring_size: usize) -> &[u64] {
    let keep = ring_size.saturating_sub(1); // room for the row about to be inserted
    let over = existing_ids_ascending.len().saturating_sub(keep);
    &existing_ids_ascending[..over]
}

/// Fold one shard's `(gateway_key, id, sessions)` samples into the realm-wide total: for each
/// `gateway_key`, only its HIGHEST `id` (auto-inc, so highest = most recent) contributes — a
/// gateway that has sampled 20 times must count once, not 20 — then those latest-per-gateway
/// values are summed. Pure so [`record_shard_load`]'s upsert and the test below share one rule,
/// same convention as [`ring_evict`].
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

/// The occupancy sanity gate, decided purely so both reducer and test share the exact rule. See
/// [`MAX_SANE_OCCUPANCY_PCT`]'s doc for why the upper bound exists.
pub(crate) fn validate_occupancy_pct(pct: f32) -> Result<(), String> {
    // The `is_finite` arm runs FIRST and is what rejects NaN; `contains` alone would too (it is
    // false for NaN, so the negation is true), but spelling it out keeps the error message honest.
    if !pct.is_finite() || !(0.0..=MAX_SANE_OCCUPANCY_PCT).contains(&pct) {
        return Err(format!(
            "writer_occupancy_pct must be finite and within [0, {MAX_SANE_OCCUPANCY_PCT}], got {pct}"
        ));
    }
    Ok(())
}

/// Record one shard's writer-occupancy + session sample. Operator-gated like
/// [`crate::region::set_region_assignment`] — in production this is called by the gateway's own
/// periodic sampling task over its privileged coordinator connection
/// (that connection's identity IS the claimed operator, the same one
/// that fires every other privileged reducer), not by a player.
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
    // Ring-scoped per (shard, gateway_key), NOT per shard alone — with N gateways
    // sampling the same shard, keying eviction (and therefore the ring) by shard alone let each
    // gateway's insert evict every OTHER gateway's rows, so only the last writer's share ever
    // survived. Scoping the ring per gateway too means every gateway keeps its own ~10 minutes of
    // history, and summing the latest row per gateway_key for a shard gives the realm-wide total.
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
    // Recompute and upsert the realm-wide total (the actual Done-when) rather than
    // leaving that fold as a manual eyeball step — see `ShardLoadTotal`'s doc.
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

/// Record one region's open-world player-count sample. Same gating and ring-buffer
/// discipline as [`record_shard_load`]; see that reducer's doc.
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
        // One gateway, three samples over time (rising id): only the newest counts.
        assert_eq!(
            realm_wide_sessions(vec![(1, 10, 5), (1, 11, 7), (1, 12, 9)].into_iter()),
            9,
            "a single gateway's older samples must not be double-counted"
        );
        // Two gateways sampling the same shard: their latest rows sum to the realm-wide total.
        assert_eq!(
            realm_wide_sessions(vec![(1, 10, 5), (2, 11, 7)].into_iter()),
            12,
            "distinct gateways' latest samples sum to the realm-wide total (issue #308)"
        );
        // Ids need not arrive in order.
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
        assert!(validate_occupancy_pct(-0.1).is_err());
        assert!(validate_occupancy_pct(f32::NAN).is_err());
        assert!(validate_occupancy_pct(f32::INFINITY).is_err());
        assert!(validate_occupancy_pct(MAX_SANE_OCCUPANCY_PCT + 0.1).is_err());
    }

    /// Call-site tripwire, same style as `region.rs`'s `the_reducer_routes_every_write_through_the_epoch_rule`:
    /// the pure rules above are worthless if the reducer stops asking them, and neither reducer can
    /// run without a live node — so a `.contains()` scan on the SOURCE is what a green suite has.
    #[test]
    fn both_reducers_route_every_write_through_the_operator_gate_and_the_ring() {
        let src = include_str!("load.rs");
        // The file is laid out record_shard_load, then record_region_load, then the test module —
        // so splitting on those three anchors, in order, isolates each reducer's own body.
        let (_, after_shard_fn) = src
            .split_once("pub fn record_shard_load")
            .expect("still named record_shard_load");
        let (shard_body, after_region_fn) = after_shard_fn
            .split_once("pub fn record_region_load")
            .expect("still named record_region_load");
        let (region_body, _) = after_region_fn
            .split_once("#[cfg(test)]")
            .expect("the test module still follows");

        let strip = |s: &str| -> String {
            s.lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let shard_code = strip(shard_body);
        let region_code = strip(region_body);

        assert!(
            shard_code.contains("require_operator(ctx)?"),
            "record_shard_load must stay operator-gated"
        );
        assert!(
            shard_code.contains("validate_occupancy_pct("),
            "record_shard_load must validate occupancy"
        );
        assert!(
            shard_code.contains("ring_evict("),
            "record_shard_load must ring-evict before inserting"
        );
        assert!(
            shard_code.contains("r.gateway_key == gateway_key"),
            "record_shard_load must scope the ring by gateway_key too (issue #308) — filtering by \
             shard alone lets one gateway's insert evict every OTHER gateway's rows"
        );
        assert!(
            shard_code.contains("realm_wide_sessions("),
            "record_shard_load must fold every gateway's latest sample into the materialized \
             realm-wide total (issue #308's actual Done-when) — keying the ring by gateway_key \
             alone leaves summation a manual step"
        );

        assert!(
            region_code.contains("require_operator(ctx)?"),
            "record_region_load must stay operator-gated"
        );
        assert!(
            region_code.contains("ring_evict("),
            "record_region_load must ring-evict before inserting"
        );
    }
}
