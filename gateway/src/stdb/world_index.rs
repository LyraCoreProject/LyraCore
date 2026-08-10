//! The in-process AOI index — the gateway-side replacement for ~600 per-player,
//! box-scoped SpacetimeDB subscriptions.
//!
//! # Why this exists
//!
//! Before the shared-connection model, every session opened its own SpacetimeDB connection and
//! subscribed a 5×5 grid box over four tables (`game_world_entity`, `game_gameobject`, `game_entity_motion`,
//! `game_creature_spline`), re-subscribing on every cell crossing. At 600 players that is ~600
//! distinct query strings that can neither be shared nor pruned, and every committed transaction
//! woke ~600 SDK pumps to evaluate them — measured as load average 11–14 on 8 cores with only 7
//! runnable threads and 34% CPU, i.e. a WAKEUP problem, not a CPU problem (SpacetimeDB's own
//! clockworklabs/SpacetimeDB#2783).
//!
//! Now the four tables ride ONE global subscription per shard, on that shard's coordinator
//! connection, and this structure answers the question the subscription engine used to answer:
//! **which sessions can see this row?**
//!
//! # The shape, and why it is this shape
//!
//! Interest is a SYMMETRIC square: a viewer anchored on cell `A` sees exactly the cells within
//! `±BOX_HALF_SPAN` of `A` on each axis (`lyracore_shared::spatial::GridBox::bounds`). Symmetry is
//! what makes the inverse cheap — "who sees cell `E`" is "every viewer anchored within ±SPAN of
//! `E`", the same 25-cell neighbourhood walked in the other direction. So both directions are a
//! fixed 25 hash lookups plus the size of the answer, INDEPENDENT of how many players are online:
//!
//! | operation | cost |
//! |---|---|
//! | [`WorldIndex::upsert_entity`] (a row moved / appeared) | O(1) |
//! | [`WorldIndex::viewers_of`] (row → sessions) | O(25 + V), V = viewers that can see it |
//! | [`WorldIndex::move_viewer_delta`] (a cell crossing) | O(50 + delta) — 5 cells in, 5 out on an axis crossing |
//! | [`WorldIndex::visible_entities`] (the login sweep) | O(25 + E), E = entities in view |
//!
//! A naive "for each session, is this row in its box" scan would be O(players) PER ROW UPDATE,
//! which at 600 players and ~2 movement heartbeats/s is the same fan-out problem in a different
//! process. That shape is deliberately not available here.
//!
//! # Shards
//!
//! Guids are globally unique across databases, so every shard's coordinator stream
//! feeds ONE index keyed by `(map_id, instance_id, cell)` — one view spans the whole realm. Each
//! entity remembers which shard's cache holds its row ([`ShardId`]) so a sweep can read the row
//! back without searching every shard.
//!
//! # The escape hatch
//!
//! `LYRACORE_AOI=0` (`config::aoi_enabled`) keeps its old meaning — no cell scoping, every viewer
//! sees the whole partition. That path is deliberately O(viewers) / O(entities): it reproduces the
//! cost profile of the global subscription it stands in for, and it is a debugging hatch, not a
//! production mode.

use lyracore_shared::spatial::{grid_cell, grid_cell_id, BOX_HALF_SPAN};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// One world session's identity inside the index. Minted per world session (NOT per account: an
/// account can hold a logon socket and a world socket at once, and only the latter is a viewer),
/// so a relogin can never inherit the previous session's interest set.
pub type SessionId = u64;

/// Which shard's coordinator cache holds an entity's row — an index into the dispatcher's shard
/// table, kept alongside the cell so a sweep can read the row back without probing every shard.
pub type ShardId = usize;

/// A grid cell, partition-qualified. `cell` is the packed `(grid_x, grid_y)` id
/// ([`grid_cell_id`]) — a bijection on the whole `(i32, i32)` domain, so this triple names exactly
/// one cell of exactly one `(map_id, instance_id)` partition and two distinct cells can never
/// collide.
///
/// **`instance_id` is part of the key, not a filter applied afterwards**: a per-instance
/// dungeon copy reuses its source map's coordinates, so a key without the partition would place a
/// raid boss and an open-world critter in the same bucket.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CellKey {
    pub map_id: u32,
    pub instance_id: u64,
    pub cell: i64,
}

impl CellKey {
    /// The key for grid cell `(gx, gy)` of one partition.
    pub fn at(map_id: u32, instance_id: u64, gx: i32, gy: i32) -> Self {
        Self {
            map_id,
            instance_id,
            cell: grid_cell_id(gx, gy),
        }
    }

    /// The key for a world position — the one call every row writer and every viewer recenter uses,
    /// so a cell can never be computed two different ways.
    pub fn of_position(map_id: u32, instance_id: u64, x: f32, y: f32) -> Self {
        let (gx, gy) = grid_cell(x, y);
        Self::at(map_id, instance_id, gx, gy)
    }

    /// This key's partition — the `(map_id, instance_id)` pair the AOI-off path scopes by.
    pub fn partition(&self) -> (u32, u64) {
        (self.map_id, self.instance_id)
    }

    /// The 25 keys of the `(2·BOX_HALF_SPAN+1)`-square neighbourhood centred here, this key
    /// included.
    ///
    /// Used in BOTH directions, which is the whole trick: centred on an ENTITY's cell it enumerates
    /// the anchors of every viewer that can see it; centred on a VIEWER's anchor it enumerates the
    /// cells of every entity they can see. Both are true only because the interest square is
    /// symmetric — `neighbourhood_is_exactly_the_grid_box_bounds` (below) pins it against
    /// `GridBox::bounds`, the predicate the deleted subscription SQL was built from.
    pub fn neighbourhood(&self) -> impl Iterator<Item = CellKey> {
        let (gx, gy) = lyracore_shared::spatial::grid_cell_of_id(self.cell);
        let (map_id, instance_id) = (self.map_id, self.instance_id);
        ((gx - BOX_HALF_SPAN)..=(gx + BOX_HALF_SPAN)).flat_map(move |cx| {
            ((gy - BOX_HALF_SPAN)..=(gy + BOX_HALF_SPAN))
                .map(move |cy| CellKey::at(map_id, instance_id, cx, cy))
        })
    }

    /// (Inspection only — the dispatch never needs it; the differential tests do.)
    #[cfg(test)]
    /// The `GridBox` this key anchors — the bridge back to the shared spatial crate, and what the
    /// differential test drives the (frozen) subscription-SQL oracle from.
    pub fn grid_box(&self) -> lyracore_shared::spatial::GridBox {
        let (gx, gy) = lyracore_shared::spatial::grid_cell_of_id(self.cell);
        lyracore_shared::spatial::GridBox {
            map_id: self.map_id,
            gx,
            gy,
        }
    }
}

/// What a viewer's cell crossing changed: the rows that entered its box and the guids that left it.
/// The gateway turns these into the CREATE / DESTROY packets a re-subscribed box used to generate.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct ViewerDelta {
    pub entered: Vec<(u64, ShardId)>,
    pub left: Vec<(u64, ShardId)>,
}

#[derive(Default)]
struct Inner {
    /// cell → the guids resident in it. The forward direction of the interest question.
    cell_entities: HashMap<CellKey, HashSet<u64>>,
    /// guid → where it is and whose cache holds it. Also the "did it move?" memory that makes
    /// [`WorldIndex::upsert_entity`] O(1) instead of a search.
    entity_cell: HashMap<u64, (CellKey, ShardId)>,
    /// cell → the sessions ANCHORED there (not the sessions that can see it). Small by
    /// construction — a `Vec` beats a `HashSet` at these sizes and keeps iteration cache-friendly.
    cell_viewers: HashMap<CellKey, Vec<SessionId>>,
    /// session → its anchor cell. The recenter's "where was it" half.
    viewer_cell: HashMap<SessionId, CellKey>,
    /// guid → session, for the ONE row that must reach a viewer regardless of geometry: their own.
    /// A player's entity row and their anchor are written by two different paths (the module's
    /// movement write and the gateway's movement handler), so a self-VALUES packet must never be
    /// gated on the two agreeing at that instant.
    self_guids: HashMap<u64, SessionId>,
}

/// The gateway-wide AOI index. One instance per gateway, shared by every shard's coordinator
/// dispatch and by every world session.
///
/// Everything is behind ONE mutex on purpose: every operation is a handful of hash lookups with no
/// I/O and no encoding inside the critical section (see the module doc's complexity table), so the
/// lock is held for nanoseconds and sharding it would buy contention accounting rather than
/// throughput. The rule that keeps it that way: **never call out into relay/encode work while
/// holding this lock** — `viewers_of` and `visible_entities` return owned answers precisely so the
/// caller does its work after the guard drops.
pub struct WorldIndex {
    inner: Mutex<Inner>,
    /// `false` = `LYRACORE_AOI=0`: no cell scoping, whole-partition visibility (see the module doc).
    cell_scoped: bool,
}

impl WorldIndex {
    pub fn new(cell_scoped: bool) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            cell_scoped,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // Matches the file-wide `.lock().unwrap()` discipline in `connection.rs`: a poisoned index
        // must degrade to a possibly-stale read, never take every future relay down with it.
        self.inner.lock().unwrap_or_else(|p| {
            log::error!("world index lock poisoned (a prior panic in a critical section) — recovering");
            p.into_inner()
        })
    }

    // ---------------------------------------------------------------------------------------
    //  Entities
    // ---------------------------------------------------------------------------------------

    /// Record (or move) an entity. Returns the cell it came FROM when this was a move, so a caller
    /// can decide whether a viewer needs a DESTROY for it.
    pub fn upsert_entity(&self, guid: u64, key: CellKey, shard: ShardId) -> Option<CellKey> {
        let mut inner = self.lock();
        let previous = inner.entity_cell.insert(guid, (key, shard)).map(|(k, _)| k);
        match previous {
            Some(old) if old == key => return Some(old),
            Some(old) => {
                if let Some(set) = inner.cell_entities.get_mut(&old) {
                    set.remove(&guid);
                    if set.is_empty() {
                        inner.cell_entities.remove(&old);
                    }
                }
            }
            None => {}
        }
        inner.cell_entities.entry(key).or_default().insert(guid);
        previous
    }

    /// Which shard's coordinator cache holds this entity's row — `None` for a guid the index has
    /// never seen. The 4c non-spatial dispatch uses it as the "same database as the row" half of a
    /// family's audience predicate (a viewer's per-player subscription only ever saw rows from the
    /// shard their own connection was on).
    pub fn shard_of(&self, guid: u64) -> Option<ShardId> {
        self.lock().entity_cell.get(&guid).map(|(_, s)| *s)
    }

    /// The session whose OWN character is `guid` — `None` if that character has no live session on
    /// this gateway. The self-only 4c families (rest state, skill pane, …) resolve their audience
    /// through this: the row names its owner and the owner is the only lawful recipient, so the
    /// lookup IS the visibility predicate — there is no candidate set to filter.
    pub fn session_of_owner(&self, guid: u64) -> Option<SessionId> {
        self.lock().self_guids.get(&guid).copied()
    }

    /// Forget an entity (its row was deleted). Returns the cell it was in, if it was known.
    pub fn remove_entity(&self, guid: u64) -> Option<CellKey> {
        let mut inner = self.lock();
        let (key, _) = inner.entity_cell.remove(&guid)?;
        if let Some(set) = inner.cell_entities.get_mut(&key) {
            set.remove(&guid);
            if set.is_empty() {
                inner.cell_entities.remove(&key);
            }
        }
        Some(key)
    }

    /// (Inspection only — the dispatch never needs it; the differential tests do.)
    #[cfg(test)]
    /// Where the index believes `guid` is. `None` for a guid it has never seen (a row on a table it
    /// does not index, or one deleted already).
    pub fn entity_cell(&self, guid: u64) -> Option<CellKey> {
        self.lock().entity_cell.get(&guid).map(|(k, _)| *k)
    }

    // ---------------------------------------------------------------------------------------
    //  Viewers
    // ---------------------------------------------------------------------------------------

    /// Register a session as a viewer anchored on `key`, owning entity `self_guid`.
    pub fn add_viewer(&self, session: SessionId, self_guid: u64, key: CellKey) {
        let mut inner = self.lock();
        inner.self_guids.insert(self_guid, session);
        Self::place_viewer(&mut inner, session, key);
    }

    /// Unregister a session. Idempotent.
    pub fn remove_viewer(&self, session: SessionId, self_guid: u64) {
        let mut inner = self.lock();
        // Only drop the self-guid mapping if it still points at THIS session: a relogin on the same
        // character registers the new session before the old one's guard drops.
        if inner.self_guids.get(&self_guid) == Some(&session) {
            inner.self_guids.remove(&self_guid);
        }
        if let Some(old) = inner.viewer_cell.remove(&session) {
            Self::unplace_viewer(&mut inner, session, old);
        }
    }

    /// (Inspection only — the dispatch never needs it; the differential tests do.)
    #[cfg(test)]
    /// A viewer's current anchor.
    pub fn viewer_cell(&self, session: SessionId) -> Option<CellKey> {
        self.lock().viewer_cell.get(&session).copied()
    }

    fn place_viewer(inner: &mut Inner, session: SessionId, key: CellKey) {
        if let Some(old) = inner.viewer_cell.insert(session, key) {
            if old == key {
                return;
            }
            Self::unplace_viewer(inner, session, old);
        }
        inner.cell_viewers.entry(key).or_default().push(session);
    }

    fn unplace_viewer(inner: &mut Inner, session: SessionId, key: CellKey) {
        if let Some(v) = inner.cell_viewers.get_mut(&key) {
            v.retain(|s| *s != session);
            if v.is_empty() {
                inner.cell_viewers.remove(&key);
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    //  The two interest queries
    // ---------------------------------------------------------------------------------------

    /// Every session that can see cell `key` — the dispatch's hot path, called once per indexed row
    /// delta. O(25 + V) with cell scoping on; O(total viewers) with `LYRACORE_AOI=0`.
    pub fn viewers_of(&self, key: CellKey) -> Vec<SessionId> {
        let inner = self.lock();
        if !self.cell_scoped {
            let partition = key.partition();
            return inner
                .viewer_cell
                .iter()
                .filter(|(_, k)| k.partition() == partition)
                .map(|(s, _)| *s)
                .collect();
        }
        let mut out = Vec::new();
        for anchor in key.neighbourhood() {
            if let Some(sessions) = inner.cell_viewers.get(&anchor) {
                out.extend_from_slice(sessions);
            }
        }
        out
    }

    /// Every session that must be told about a row for `guid` — [`Self::viewers_of`] plus the guid's
    /// OWN session if it has one.
    ///
    /// The self leg is not an optimisation, it is a correctness gate. A player's entity row is
    /// written by the module (the movement/health write) while their anchor is moved by the
    /// gateway's movement handler; in the window between an entity crossing a cell and its own
    /// session recentring, a purely geometric answer would drop that player's OWN health/level
    /// VALUES packet. Cheap: one extra hash lookup per delta.
    pub fn recipients_of(&self, guid: u64, key: CellKey) -> Vec<SessionId> {
        let mut out = self.viewers_of(key);
        if let Some(owner) = self.lock().self_guids.get(&guid).copied() {
            if !out.contains(&owner) {
                out.push(owner);
            }
        }
        out
    }

    /// Move a viewer's anchor and report the VISIBILITY delta the move implies, or `None` when the
    /// anchor did not change (the overwhelmingly common per-heartbeat call — a plain hash lookup and
    /// a compare, no allocation).
    ///
    /// The subscription engine used to produce this delta itself, as the `on_insert`/`on_delete`
    /// pair a re-subscribed box generated. Computed by diffing the two 25-cell neighbourhoods and
    /// gathering guids only from the cells that actually differ: an axis crossing touches 5 cells in
    /// and 5 out, so the answer costs ~50 hash lookups plus the size of the answer — never a scan of
    /// the world or of the box's contents.
    pub fn move_viewer_delta(&self, session: SessionId, key: CellKey) -> Option<ViewerDelta> {
        let mut inner = self.lock();
        let old = *inner.viewer_cell.get(&session)?;
        if old == key {
            return None;
        }
        Self::place_viewer(&mut inner, session, key);
        if !self.cell_scoped {
            // No cell scoping: the box IS the partition, so a move inside one changes nothing and a
            // move ACROSS one is a map/instance change, which tears the whole session's view down
            // and rebuilds it through `sweep_into_view` rather than through a delta.
            return Some(ViewerDelta::default());
        }
        let old_cells: HashSet<CellKey> = old.neighbourhood().collect();
        let new_cells: HashSet<CellKey> = key.neighbourhood().collect();
        let gather = |cells: &HashSet<CellKey>, other: &HashSet<CellKey>| -> Vec<(u64, ShardId)> {
            let mut out = Vec::new();
            for cell in cells.difference(other) {
                if let Some(guids) = inner.cell_entities.get(cell) {
                    for guid in guids {
                        let shard = inner.entity_cell.get(guid).map(|(_, s)| *s).unwrap_or(0);
                        out.push((*guid, shard));
                    }
                }
            }
            out
        };
        Some(ViewerDelta {
            entered: gather(&new_cells, &old_cells),
            left: gather(&old_cells, &new_cells),
        })
    }

    /// Every entity a viewer can see, with the shard whose cache holds each row — the login /
    /// world-entry sweep, and the inverse the differential test checks. O(25 + E) with cell scoping
    /// on; O(world) with `LYRACORE_AOI=0`.
    pub fn visible_entities(&self, session: SessionId) -> Vec<(u64, ShardId)> {
        let inner = self.lock();
        let Some(anchor) = inner.viewer_cell.get(&session).copied() else {
            return Vec::new();
        };
        if !self.cell_scoped {
            let partition = anchor.partition();
            return inner
                .entity_cell
                .iter()
                .filter(|(_, (k, _))| k.partition() == partition)
                .map(|(guid, (_, shard))| (*guid, *shard))
                .collect();
        }
        let mut out = Vec::new();
        for cell in anchor.neighbourhood() {
            if let Some(guids) = inner.cell_entities.get(&cell) {
                for guid in guids {
                    if let Some((_, shard)) = inner.entity_cell.get(guid) {
                        out.push((*guid, *shard));
                    }
                }
            }
        }
        out
    }

    /// Can `session` currently see `guid`? The point query behind relays that are driven by a
    /// DIFFERENT table (a stealth aura, say) and therefore have no cell of their own to dispatch on.
    /// Before the shared-connection model these asked "is the row in my connection's cache",
    /// which the box subscription made equivalent; on a shared connection the cache holds the whole world, so the question has
    /// to be asked of the index instead.
    pub fn can_see(&self, session: SessionId, guid: u64) -> bool {
        let inner = self.lock();
        let Some(anchor) = inner.viewer_cell.get(&session) else {
            return false;
        };
        let Some((cell, _)) = inner.entity_cell.get(&guid) else {
            return false;
        };
        if anchor.partition() != cell.partition() {
            return false;
        }
        if !self.cell_scoped {
            return true;
        }
        let (ax, ay) = lyracore_shared::spatial::grid_cell_of_id(anchor.cell);
        let (cx, cy) = lyracore_shared::spatial::grid_cell_of_id(cell.cell);
        (ax - cx).abs() <= BOX_HALF_SPAN && (ay - cy).abs() <= BOX_HALF_SPAN
    }

    /// Live sizes, for the periodic ops log line: `(entities, viewers, occupied entity cells)`.
    pub fn stats(&self) -> (usize, usize, usize) {
        let inner = self.lock();
        (
            inner.entity_cell.len(),
            inner.viewer_cell.len(),
            inner.cell_entities.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdb::aoi::box_queries;

    // ===========================================================================================
    //  Fixture + the SQL ORACLE.
    //
    //  The oracle is the SUBSCRIPTION SQL ITSELF — `box_queries` builds the exact WHERE clause the
    //  per-player box subscription used to send, and `sql_selects` below EVALUATES that text.
    //  Deliberately a parser rather than a re-implementation of the predicate: a re-implementation
    //  would drift with the index it is supposed to be independent of, and the class of bug this
    //  whole test exists to catch (a peer that never appears, or one visible through a wall) is
    //  exactly the class where index and predicate disagree by one cell.
    // ===========================================================================================

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct Row {
        guid: u64,
        map_id: u32,
        instance_id: u64,
        gx: i32,
        gy: i32,
    }

    /// Evaluate one generated AOI query against a fixture row. Understands exactly the grammar the
    /// AOI builders emit — `col = n`, `col >= n`, `col <= n`, joined by ` AND ` — and PANICS on
    /// anything else, so a future query shape cannot silently pass by being unparsed.
    fn sql_selects(query: &str, row: &Row) -> bool {
        let (_, where_clause) = query.split_once(" WHERE ").expect("an AOI query has a WHERE");
        where_clause.split(" AND ").all(|term| {
            let mut parts = term.split_whitespace();
            let col = parts.next().expect("a column");
            let op = parts.next().expect("an operator");
            let literal: i64 = parts
                .next()
                .expect("a literal")
                .parse()
                .expect("an integer literal");
            assert!(parts.next().is_none(), "unexpected trailing token in `{term}`");
            let actual: i64 = match col {
                "map_id" => row.map_id as i64,
                "instance_id" => row.instance_id as i64,
                "grid_x" => row.gx as i64,
                "grid_y" => row.gy as i64,
                "cell" => grid_cell_id(row.gx, row.gy),
                other => panic!("the oracle does not know column `{other}` — teach it or fix the query"),
            };
            match op {
                "=" => actual == literal,
                ">=" => actual >= literal,
                "<=" => actual <= literal,
                other => panic!("the oracle does not know operator `{other}`"),
            }
        })
    }

    /// The set the deleted per-player subscription would have delivered to a viewer: the rows the
    /// `game_world_entity` box query selects.
    fn sql_visible(viewer: CellKey, rows: &[Row]) -> HashSet<u64> {
        let queries = box_queries(viewer.instance_id, &viewer.grid_box(), false);
        let entity_query = queries
            .iter()
            .find(|q| q.starts_with("SELECT * FROM game_world_entity "))
            .expect("the box query set must scope game_world_entity");
        rows.iter()
            .filter(|r| sql_selects(entity_query, r))
            .map(|r| r.guid)
            .collect()
    }

    /// The same set, per the index.
    fn index_visible(index: &WorldIndex, session: SessionId) -> HashSet<u64> {
        index
            .visible_entities(session)
            .into_iter()
            .map(|(guid, _)| guid)
            .collect()
    }

    fn build(rows: &[Row]) -> WorldIndex {
        let index = WorldIndex::new(true);
        for r in rows {
            index.upsert_entity(r.guid, CellKey::at(r.map_id, r.instance_id, r.gx, r.gy), 0);
        }
        index
    }

    /// xorshift64* — a deterministic PRNG so a failure is reproducible from the seed alone, with no
    /// new dev-dependency.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn range(&mut self, lo: i32, hi: i32) -> i32 {
            lo + (self.next() % ((hi - lo + 1) as u64)) as i32
        }
    }

    // ===========================================================================================
    //  THE DIFFERENTIAL PROPERTY (the shared-connection model's whole safety argument)
    // ===========================================================================================

    /// For every viewer, over randomised fixtures: **the index's visible set equals the set the box
    /// subscription's SQL predicate selects.** Both directions matter and both are checked by set
    /// equality — a missing guid is a peer that never appears, an extra one is a peer visible
    /// through a wall.
    ///
    /// The fixture deliberately packs viewers and entities into a small cell range (so boxes
    /// overlap heavily, and edge cells are exercised constantly), spreads them over several
    /// `(map_id, instance_id)` partitions (so a partition leak shows up), and includes negative
    /// grid indices (legal: `grid_cell` inverts the axis, so out-of-bounds coordinates produce
    /// them).
    #[test]
    fn the_index_visible_set_equals_the_box_subscription_sql_for_every_viewer() {
        let mut rng = Rng(0x5EED_1234_ABCD_0001);
        for trial in 0..200 {
            let rows: Vec<Row> = (0..60)
                .map(|i| Row {
                    guid: 1000 + i,
                    map_id: rng.range(0, 1) as u32,
                    instance_id: rng.range(0, 2) as u64,
                    gx: rng.range(-4, 4),
                    gy: rng.range(-4, 4),
                })
                .collect();
            let index = build(&rows);
            for v in 0..12u64 {
                let viewer = CellKey::at(
                    rng.range(0, 1) as u32,
                    rng.range(0, 2) as u64,
                    rng.range(-4, 4),
                    rng.range(-4, 4),
                );
                index.add_viewer(v, 9_000_000 + v, viewer);
                assert_eq!(
                    index_visible(&index, v),
                    sql_visible(viewer, &rows),
                    "trial {trial}, viewer {v} anchored at {viewer:?}: the index and the box \
                     subscription SQL disagree about who is visible"
                );
                index.remove_viewer(v, 9_000_000 + v);
            }
        }
    }

    /// The INVERSE direction, over the same oracle: `viewers_of(cell)` must name exactly the
    /// viewers whose box query selects a row in that cell. The dispatch calls this one on every row
    /// delta, so an asymmetry here is a peer that spawns but never moves (or moves but never
    /// spawns) — the two halves would be scoped by different predicates.
    #[test]
    fn viewers_of_a_cell_is_exactly_the_inverse_of_the_box_subscription_sql() {
        let mut rng = Rng(0x00C0_FFEE_0000_0042);
        for _ in 0..200 {
            let viewers: Vec<(SessionId, CellKey)> = (0..16u64)
                .map(|v| {
                    (
                        v,
                        CellKey::at(
                            rng.range(0, 1) as u32,
                            rng.range(0, 1) as u64,
                            rng.range(-3, 3),
                            rng.range(-3, 3),
                        ),
                    )
                })
                .collect();
            let index = WorldIndex::new(true);
            for (v, key) in &viewers {
                index.add_viewer(*v, 9_000_000 + *v, *key);
            }
            for _ in 0..10 {
                let row = Row {
                    guid: 7,
                    map_id: rng.range(0, 1) as u32,
                    instance_id: rng.range(0, 1) as u64,
                    gx: rng.range(-5, 5),
                    gy: rng.range(-5, 5),
                };
                let cell = CellKey::at(row.map_id, row.instance_id, row.gx, row.gy);
                let from_index: HashSet<SessionId> = index.viewers_of(cell).into_iter().collect();
                let from_sql: HashSet<SessionId> = viewers
                    .iter()
                    .filter(|(_, anchor)| !sql_visible(*anchor, &[row]).is_empty())
                    .map(|(v, _)| *v)
                    .collect();
                assert_eq!(
                    from_index, from_sql,
                    "row {row:?} — the dispatch's recipient set disagrees with the SQL that used \
                     to deliver it"
                );
            }
        }
    }

    /// The recenter delta must be exactly the difference between the two SQL-defined visible sets.
    /// This is the half of the change the subscription engine used to do for us: re-subscribing a
    /// box produced the enter/leave events itself, and getting the delta wrong here is invisible in
    /// review and shows up live as a peer that never despawns, or one that never comes back.
    #[test]
    fn a_recenter_delta_is_exactly_the_difference_between_the_two_sql_visible_sets() {
        let mut rng = Rng(0xDEAD_BEEF_0000_0007);
        for _ in 0..300 {
            let rows: Vec<Row> = (0..50)
                .map(|i| Row {
                    guid: 2000 + i,
                    map_id: 0,
                    instance_id: 0,
                    gx: rng.range(-6, 6),
                    gy: rng.range(-6, 6),
                })
                .collect();
            let index = build(&rows);
            let from = CellKey::at(0, 0, rng.range(-4, 4), rng.range(-4, 4));
            let to = CellKey::at(0, 0, rng.range(-4, 4), rng.range(-4, 4));
            index.add_viewer(1, 999, from);
            let before = sql_visible(from, &rows);
            let delta = index.move_viewer_delta(1, to);
            let after = sql_visible(to, &rows);
            match delta {
                None => assert_eq!(from, to, "a delta of None must mean the anchor did not move"),
                Some(d) => {
                    let entered: HashSet<u64> = d.entered.iter().map(|(g, _)| *g).collect();
                    let left: HashSet<u64> = d.left.iter().map(|(g, _)| *g).collect();
                    assert_eq!(
                        entered,
                        after.difference(&before).copied().collect::<HashSet<u64>>(),
                        "entered set wrong moving {from:?} -> {to:?}"
                    );
                    assert_eq!(
                        left,
                        before.difference(&after).copied().collect::<HashSet<u64>>(),
                        "left set wrong moving {from:?} -> {to:?}"
                    );
                    // And the index agrees with the SQL at the destination, so the delta and the
                    // absolute answer can never drift apart.
                    assert_eq!(index_visible(&index, 1), after);
                }
            }
            index.remove_viewer(1, 999);
        }
    }

    /// The property the whole 25-lookup shape rests on: the neighbourhood of a cell is EXACTLY the
    /// cell set `GridBox::bounds` accepts. If the shared crate's span ever changes, this fails here
    /// rather than silently shrinking everyone's view.
    #[test]
    fn neighbourhood_is_exactly_the_grid_box_bounds() {
        let key = CellKey::at(3, 9, 12, -7);
        let (x0, x1, y0, y1) = key.grid_box().bounds();
        let got: HashSet<(i32, i32)> = key
            .neighbourhood()
            .map(|k| lyracore_shared::spatial::grid_cell_of_id(k.cell))
            .collect();
        let want: HashSet<(i32, i32)> = (x0..=x1).flat_map(|x| (y0..=y1).map(move |y| (x, y))).collect();
        assert_eq!(got, want);
        assert_eq!(got.len(), ((2 * BOX_HALF_SPAN + 1) as usize).pow(2));
        // Every neighbour keeps the partition — a leak here is a dungeon copy visible from the
        // open world.
        assert!(key
            .neighbourhood()
            .all(|k| k.map_id == 3 && k.instance_id == 9));
    }

    // ===========================================================================================
    //  Bookkeeping
    // ===========================================================================================

    #[test]
    fn an_entity_move_leaves_no_residue_in_its_old_cell() {
        let index = WorldIndex::new(true);
        let a = CellKey::at(0, 0, 0, 0);
        let b = CellKey::at(0, 0, 40, 40); // far outside a's box
        index.upsert_entity(5, a, 0);
        index.add_viewer(1, 999, a);
        assert_eq!(index_visible(&index, 1), HashSet::from([5]));
        assert_eq!(index.upsert_entity(5, b, 0), Some(a));
        assert!(index_visible(&index, 1).is_empty(), "stale cell membership");
        assert_eq!(index.entity_cell(5), Some(b));
        assert_eq!(index.remove_entity(5), Some(b));
        assert_eq!(index.entity_cell(5), None);
        assert_eq!(index.remove_entity(5), None, "removal is idempotent");
    }

    #[test]
    fn a_viewer_recenter_is_a_cell_swap_and_reports_only_real_crossings() {
        let index = WorldIndex::new(true);
        let a = CellKey::at(0, 0, 0, 0);
        index.add_viewer(1, 999, a);
        assert!(index.move_viewer_delta(1, a).is_none(), "same cell is not a recenter");
        assert!(index.move_viewer_delta(1, CellKey::at(0, 0, 1, 0)).is_some());
        assert_eq!(index.viewer_cell(1), Some(CellKey::at(0, 0, 1, 0)));
        // Only one anchor is ever held: walking away must not leave the session visible from the
        // cell it started in.
        index.upsert_entity(5, CellKey::at(0, 0, 0, 0), 0);
        index.move_viewer_delta(1, CellKey::at(0, 0, 30, 30));
        assert!(index.viewers_of(CellKey::at(0, 0, 0, 0)).is_empty());
        assert!(
            index.move_viewer_delta(2, a).is_none(),
            "an unregistered session never moves"
        );
        index.remove_viewer(1, 999);
        assert_eq!(index.viewer_cell(1), None);
        index.remove_viewer(1, 999); // idempotent
    }

    /// The self leg of [`WorldIndex::recipients_of`]: a player must receive their OWN row's updates
    /// even in the window where their entity has crossed a cell but their session has not
    /// recentred yet — the window a purely geometric answer would drop a self health/level packet
    /// in.
    #[test]
    fn a_player_always_receives_their_own_row_even_before_their_anchor_catches_up() {
        let index = WorldIndex::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        index.add_viewer(7, 4242, anchor);
        let far = CellKey::at(0, 0, 99, 99);
        assert!(index.viewers_of(far).is_empty());
        assert_eq!(index.recipients_of(4242, far), vec![7]);
        // And never twice when geometry already covers it.
        assert_eq!(index.recipients_of(4242, anchor), vec![7]);
        index.remove_viewer(7, 4242);
        assert!(index.recipients_of(4242, far).is_empty());
    }

    /// `LYRACORE_AOI=0` reproduces the pre-AOI global-subscription semantics: whole partition, and
    /// nothing across a partition boundary.
    #[test]
    fn the_aoi_off_hatch_sees_the_whole_partition_and_nothing_outside_it() {
        let index = WorldIndex::new(false);
        index.upsert_entity(1, CellKey::at(0, 0, 0, 0), 0);
        index.upsert_entity(2, CellKey::at(0, 0, 500, 500), 0);
        index.upsert_entity(3, CellKey::at(0, 77, 0, 0), 0); // another instance
        index.upsert_entity(4, CellKey::at(1, 0, 0, 0), 0); // another map
        index.add_viewer(1, 999, CellKey::at(0, 0, 0, 0));
        assert_eq!(index_visible(&index, 1), HashSet::from([1, 2]));
        assert_eq!(index.viewers_of(CellKey::at(0, 0, 900, 900)), vec![1]);
        assert!(index.viewers_of(CellKey::at(0, 77, 0, 0)).is_empty());
    }

    /// `can_see` is a point query and `visible_entities` is the set form of the same question —
    /// they must never disagree, or the stealth reveal would show a peer the box does not contain
    /// (or hide one it does).
    #[test]
    fn can_see_agrees_with_visible_entities_everywhere() {
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        let rows: Vec<Row> = (0..80)
            .map(|i| Row {
                guid: 3000 + i,
                map_id: rng.range(0, 1) as u32,
                instance_id: rng.range(0, 1) as u64,
                gx: rng.range(-6, 6),
                gy: rng.range(-6, 6),
            })
            .collect();
        let index = build(&rows);
        for _ in 0..50 {
            let anchor = CellKey::at(
                rng.range(0, 1) as u32,
                rng.range(0, 1) as u64,
                rng.range(-5, 5),
                rng.range(-5, 5),
            );
            index.add_viewer(1, 999, anchor);
            let visible = index_visible(&index, 1);
            for r in &rows {
                assert_eq!(
                    index.can_see(1, r.guid),
                    visible.contains(&r.guid),
                    "anchor {anchor:?} row {r:?}"
                );
            }
            assert!(!index.can_see(1, 1), "an unknown guid is never visible");
            assert!(!index.can_see(99, rows[0].guid), "an unknown session sees nothing");
            index.remove_viewer(1, 999);
        }
    }

    #[test]
    fn stats_report_what_the_ops_line_prints() {
        let index = WorldIndex::new(true);
        index.upsert_entity(1, CellKey::at(0, 0, 0, 0), 0);
        index.upsert_entity(2, CellKey::at(0, 0, 0, 0), 0);
        index.upsert_entity(3, CellKey::at(0, 0, 1, 0), 0);
        index.add_viewer(1, 999, CellKey::at(0, 0, 0, 0));
        assert_eq!(index.stats(), (3, 1, 2));
    }

    /// `session_of_owner` is a 4c VISIBILITY predicate (the self-only families deliver through it
    /// and nothing else), so its whole contract is pinned: resolves the owner while the session is
    /// live, never anyone else, and stops resolving the moment the viewer is removed.
    #[test]
    fn session_of_owner_resolves_only_the_live_owner_session() {
        let index = WorldIndex::new(true);
        index.add_viewer(7, 999, CellKey::at(0, 0, 0, 0));
        assert_eq!(index.session_of_owner(999), Some(7));
        assert_eq!(index.session_of_owner(998), None, "an unknown guid resolves nobody");
        index.remove_viewer(7, 999);
        assert_eq!(
            index.session_of_owner(999),
            None,
            "a logged-out owner must stop receiving self-only relays immediately"
        );
    }
}
