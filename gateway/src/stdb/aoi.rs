//! What is left of the Area-of-Interest tier after the shared-connection model landed: the
//! per-viewer state that outlived the per-player subscription, and the frozen SQL oracle its
//! replacement is tested against.
//!
//! # What used to be here
//!
//! Before the shared-connection model, this file owned a per-player, grid-SCOPED subscription
//! over four tables that recentered (unsubscribe + resubscribe) as the player crossed cells. All
//! of it existed to push AOI filtering into the database.
//!
//! It measured badly. At 600 players it produced ~600 distinct subscription query strings that
//! could neither be shared nor pruned, and every committed transaction woke ~600 SDK pumps —
//! load average 11–14 on 8 cores at 34% CPU with 7 runnable threads, i.e. a wakeup storm rather
//! than work. The filtering now happens in-process, indexed by cell, in
//! [`super::world_index`] + [`super::world_view`], driven by ONE global subscription per shard on
//! the coordinator connection. Every shard's stream feeds one index.
//!
//! `GridBox` itself stayed where it was (`lyracore_shared::spatial`) — it is pure, unit-tested, and
//! still defines the interest square both tiers agree on.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Count every AOI recenter (a viewer crossing a cell) across every session, cheaply — one
/// `fetch_add` on the movement hot path, read back by the periodic log line in `world/mod.rs`. A
/// COUNT, not a rate; the log line takes the delta.
///
/// Under the shared-connection model a recenter is a 10-cell set diff in memory instead of a
/// subscription round trip, so the number is no longer a proxy for database load — it is still
/// the right proxy for how much
/// CREATE/DESTROY churn the movement path is generating.
pub(crate) static AOI_RECENTERS: AtomicU64 = AtomicU64::new(0);

/// Viewer-owned state that a relay running on a SHARED connection cannot look up.
///
/// The ghost flag was already a mirror before the shared-connection model (a relay could not read
/// the viewer's own row, which lives only on its home shard). It stayed a mirror afterwards for
/// the same reason with a different cause: a coordinator callback holds the cache of the shard that owns the ROW, which for
/// a cross-shard peer is not the shard that owns the VIEWER. A plain atomic, written on paths that
/// already run per session — never a cross-shard read per row.
#[derive(Default)]
pub(crate) struct ViewerGates {
    /// The viewer's own GHOST player-flag, for the spirit-healer visibility gate. Seeded at world
    /// entry from the character's own row and edge-detected thereafter by the self-row update relay.
    ///
    /// The seeding is not optional: before the shared-connection model the gate re-read the live
    /// row on every offer, so a player who logged in ALREADY dead saw the spirit healer without any transition ever firing.
    pub(crate) is_ghost: AtomicBool,
}

impl ViewerGates {
    pub(crate) fn is_ghost(&self) -> bool {
        self.is_ghost.load(Ordering::Relaxed)
    }
}

/// The FROZEN subscription-SQL oracle, from before the shared-connection model.
///
/// These builders are byte-for-byte the ones that generated the per-player AOI box subscription this
/// file used to send. They are compiled only under `cfg(test)` and nothing in the gateway calls them
/// any more — they exist so
/// `world_index::tests::the_index_visible_set_equals_the_box_subscription_sql_for_every_viewer` can
/// keep asserting that the in-process cell index selects EXACTLY the rows the database used to
/// select, forever, and not merely at the moment of the cutover.
///
/// **Do not "fix" these to match the index.** Their whole value is being the independent side of
/// that comparison; a change here that makes a failing differential test pass has deleted the test.
#[cfg(test)]
mod frozen_sql_oracle {
    use lyracore_shared::spatial::GridBox;

    /// The four tables a player's AOI box used to scope.
    pub(crate) const BOX_SCOPED_TABLES: [&str; 4] = [
        "game_world_entity",
        "game_gameobject",
        "game_entity_motion",
        "game_creature_spline",
    ];

    fn table_range_query(
        table: &str,
        map_id: u32,
        instance_id: u64,
        gx_min: i32,
        gx_max: i32,
        gy_min: i32,
        gy_max: i32,
    ) -> String {
        format!(
            "SELECT * FROM {table} WHERE map_id = {map_id} AND instance_id = {instance_id} AND grid_x >= {gx_min} AND grid_x <= {gx_max} AND grid_y >= {gy_min} AND grid_y <= {gy_max}"
        )
    }

    /// ONE point-probe query per (table, cell) — the shape the missing-AOI-index fix landed. Kept alongside the range
    /// form because a future change has to satisfy BOTH or it has narrowed visibility on one of
    /// them.
    fn cell_queries(
        map_id: u32,
        instance_id: u64,
        cells: impl Iterator<Item = i64>,
    ) -> Vec<String> {
        let cells: Vec<i64> = cells.collect();
        let mut out = Vec::with_capacity(BOX_SCOPED_TABLES.len() * cells.len());
        for table in BOX_SCOPED_TABLES {
            for cell in &cells {
                out.push(format!(
                    "SELECT * FROM {table} WHERE map_id = {map_id} AND instance_id = {instance_id} AND cell = {cell}"
                ));
            }
        }
        out
    }

    /// THE box's query set — every table the player's 5×5 box scoped, in one place. `per_cell` picks
    /// the query SHAPE: one indexed equality probe per cell, or the single range box.
    pub(crate) fn box_queries(instance_id: u64, box_: &GridBox, per_cell: bool) -> Vec<String> {
        if per_cell {
            return cell_queries(box_.map_id, instance_id, box_.cell_ids());
        }
        let (gx_min, gx_max, gy_min, gy_max) = box_.bounds();
        BOX_SCOPED_TABLES
            .into_iter()
            .map(|table| {
                table_range_query(
                    table,
                    box_.map_id,
                    instance_id,
                    gx_min,
                    gx_max,
                    gy_min,
                    gy_max,
                )
            })
            .collect()
    }
}

#[cfg(test)]
pub(crate) use frozen_sql_oracle::{box_queries, BOX_SCOPED_TABLES};

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::spatial::GridBox;

    /// The oracle must still describe the box the shared crate defines: anchor ± span on both axes,
    /// partition-qualified. If the spatial constants ever move, this fails HERE — the one place that
    /// would otherwise let the frozen oracle and the live index drift apart for a reason that is
    /// nobody's bug.
    #[test]
    fn the_frozen_oracle_still_describes_the_shared_crates_box() {
        let b = GridBox {
            map_id: 0,
            gx: 5,
            gy: 7,
        };
        let qs = box_queries(7, &b, false);
        assert_eq!(qs.len(), BOX_SCOPED_TABLES.len());
        for table in BOX_SCOPED_TABLES {
            assert!(qs.iter().any(|q| q.contains(table)), "{qs:#?}");
        }
        let entity = qs
            .iter()
            .find(|q| q.contains("game_world_entity"))
            .expect("game_world_entity");
        assert!(entity.contains("map_id = 0"));
        assert!(entity.contains("instance_id = 7"));
        assert!(entity.contains("grid_x >= 3") && entity.contains("grid_x <= 7"));
        assert!(entity.contains("grid_y >= 5") && entity.contains("grid_y <= 9"));
    }

    /// The per-cell shape probes exactly the box's own 25 cells per table — no more (a peer spawned
    /// outside the box) and no fewer (a blind spot the player walks into).
    #[test]
    fn the_frozen_oracle_probes_exactly_the_boxs_own_cells() {
        let b = GridBox {
            map_id: 0,
            gx: 5,
            gy: 7,
        };
        let qs = box_queries(0, &b, true);
        let want: Vec<i64> = b.cell_ids().collect();
        assert_eq!(want.len(), 25);
        assert_eq!(qs.len(), BOX_SCOPED_TABLES.len() * want.len());
        for table in BOX_SCOPED_TABLES {
            let got: std::collections::HashSet<String> = qs
                .iter()
                .filter(|q| q.contains(table))
                .map(|q| q[q.rfind("cell = ").expect("a cell term") + 7..].to_string())
                .collect();
            let expect: std::collections::HashSet<String> =
                want.iter().map(|c| c.to_string()).collect();
            assert_eq!(got, expect, "{table} probes the wrong cell set");
        }
    }

    #[test]
    fn viewer_gates_round_trip_the_ghost_flag_exactly() {
        let g = ViewerGates::default();
        assert!(!g.is_ghost());
        g.is_ghost.store(true, Ordering::Relaxed);
        assert!(g.is_ghost());
    }
}
