//! The SHARED area-of-interest dispatch (#468 stage 1) — one set of row callbacks per shard's
//! coordinator connection, routed to sessions through [`WorldIndex`].
//!
//! # What replaced what
//!
//! Before: each player opened their own SpacetimeDB connection, subscribed a 5×5 grid box over
//! `game_world_entity` / `game_gameobject` / `game_entity_motion` / `game_creature_spline`,
//! re-subscribed on every cell crossing, and registered its own row callbacks on that connection.
//! ~600 unshareable query strings, ~600 SDK pumps woken per committed transaction.
//!
//! After: those four tables ride the coordinator's existing global subscription (one per shard),
//! the callbacks below are registered ONCE per shard, and [`WorldIndex`] answers "which sessions can
//! see this row". Every shard's stream feeds ONE index keyed by `(map_id, instance_id, cell)` —
//! guids are globally unique across databases (#103/#108), so one index spans the whole realm.
//!
//! # The two rules this file exists to keep
//!
//! **1. The pump does bookkeeping and ENQUEUE only.** The SDK forbids parallel
//! `advance_one_message` callers, so a shard's coordinator pump is a single thread that every
//! session now depends on. It must never encode a packet or take a session's lock for long: it
//! updates the index, asks it for recipients, and pushes a [`crate::world::Outbound::Job`] — a
//! closure that does the gating and encoding LATER, on that session's own writer thread. If relay
//! work ran here, one slow session would stall the whole realm instead of only itself. The row is
//! wrapped in an `Arc` once per delta so the fan-out is a refcount bump per recipient, not a clone.
//!
//! **2. A panicking callback must not take the realm down.** On a per-player connection a panicking
//! row callback killed one session. On a shared connection it would kill every session on that
//! shard, so every callback body here runs inside `catch_unwind`.
//!
//! # What did NOT move
//!
//! Every per-session visibility gate is unchanged and still evaluated per session, just later (in
//! the job) instead of on the pump: the `created` dedup set, the instance gate, the spirit-healer
//! ghost gate, and the stealth gate. The security posture is unchanged in kind —
//! these four tables were already read through an RLS-bypassing owner connection on the
//! coordinator; what is new is that the gateway, not the subscription engine, decides who sees
//! them, which is exactly the filtering responsibility #468 names.

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use spacetimedb_sdk::{Table, TableWithPrimaryKey};

use super::aoi::{ViewerGates, AOI_RECENTERS};
use super::bindings::*;
use super::connection::Coordinator;
use super::world_index::{CellKey, SessionId, ShardId, WorldIndex};
use crate::world::{Outbound, SessionTx};

/// Everything the dispatch needs to know about one live world session. Registered at world entry
/// (`subscribe_player_events`) and dropped by `PlayerSubscriptions`'s guard.
///
/// Every field here is state the per-player relay closures used to CAPTURE; making it one shared
/// struct is what lets a single registration serve every session.
pub(crate) struct Viewer {
    pub(crate) session: SessionId,
    pub(crate) self_guid: u64,
    /// The map this session is on. Constant for the session's lifetime — every path that changes
    /// a character's map (cross-map teleport, portal entry) tears the subscriptions
    /// down and calls `subscribe_player_events` again, which builds a fresh viewer.
    pub(crate) map_id: u32,
    /// The partition this session is playing in — supplied by the caller (login row / transfer
    /// plan), never re-read from a cache. #456's lesson: the authoritative value is known at
    /// construction, and a second possibly-stale copy can only disagree with it.
    pub(crate) instance_id: u64,
    pub(crate) tx: SessionTx,
    /// The per-viewer "already shown" dedup set — the exactly-once guarantee behind every CREATE.
    pub(crate) created: Arc<Mutex<HashSet<u64>>>,
    pub(crate) gates: Arc<ViewerGates>,
}

/// The gateway-wide shared view: the two cell indexes, the live viewer registry, and the shard
/// table the indexes' [`ShardId`]s point into.
pub(crate) struct WorldView {
    /// `game_world_entity` — players, creatures, pets.
    pub(crate) entities: WorldIndex,
    /// `game_gameobject` — kept in its OWN index rather than sharing the entity one. Nothing today
    /// says a GO guid can never equal an entity guid, and a shared index would make that assumption
    /// load-bearing for visibility.
    pub(crate) gameobjects: WorldIndex,
    viewers: RwLock<HashMap<SessionId, Arc<Viewer>>>,
    /// [`ShardId`] → that shard's coordinator handle. Written once at startup, read on every
    /// dispatch; the order fixes the ids, so it must never be reordered after arming.
    shards: RwLock<Vec<Coordinator>>,
    next_session: AtomicU64,
}

impl WorldView {
    pub(crate) fn new(cell_scoped: bool) -> Self {
        Self {
            entities: WorldIndex::new(cell_scoped),
            gameobjects: WorldIndex::new(cell_scoped),
            viewers: RwLock::new(HashMap::new()),
            shards: RwLock::new(Vec::new()),
            next_session: AtomicU64::new(1),
        }
    }

    fn viewer(&self, session: SessionId) -> Option<Arc<Viewer>> {
        self.viewers.read().unwrap().get(&session).cloned()
    }

    fn shard(&self, id: ShardId) -> Option<Coordinator> {
        self.shards.read().unwrap().get(id).cloned()
    }

    /// Mint a session id. Monotonic and never reused, so a relogin can never inherit a previous
    /// session's interest set.
    pub(crate) fn next_session_id(&self) -> SessionId {
        self.next_session.fetch_add(1, Ordering::Relaxed)
    }

    /// Register a live session. The caller supplies the anchor (its login position) so the first
    /// sweep sees the right box.
    pub(crate) fn add_viewer(&self, viewer: Arc<Viewer>, anchor: CellKey) {
        self.entities
            .add_viewer(viewer.session, viewer.self_guid, anchor);
        self.gameobjects
            .add_viewer(viewer.session, viewer.self_guid, anchor);
        self.viewers
            .write()
            .unwrap()
            .insert(viewer.session, viewer);
    }

    pub(crate) fn remove_viewer(&self, session: SessionId, self_guid: u64) {
        self.entities.remove_viewer(session, self_guid);
        self.gameobjects.remove_viewer(session, self_guid);
        self.viewers.write().unwrap().remove(&session);
    }

    // ===========================================================================================
    //  Arming — one registration per shard, re-run by the coordinator watchdog after a reconnect.
    // ===========================================================================================

    /// Give this view its shard table. Called once, from `Coordinator::connect`, before arming.
    pub(crate) fn set_shards(&self, shards: Vec<Coordinator>) {
        *self.shards.write().unwrap() = shards;
    }

    /// The number of shards registered — used by the ops log line and by the arming loop.
    pub(crate) fn shard_count(&self) -> usize {
        self.shards.read().unwrap().len()
    }
}

/// Register the shared AOI relays on ONE shard's coordinator connection.
///
/// Re-reads `coord.0.coord()` on every call, which is what makes it correct as the watchdog's
/// post-reconnect re-arm hook too: called again, it registers on the FRESH `LiveConn`. The old
/// connection's callbacks die with it, so nothing needs removing.
pub(crate) fn arm_shard(view: Arc<WorldView>, coord: Coordinator, shard: ShardId) {
    let guard = coord.0.coord();
    let db = &guard.conn.db;

    // ---- game_world_entity -----------------------------------------------------------------
    {
        let view = view.clone();
        db.game_world_entity().on_insert(move |_ctx, row| {
            guarded("game_world_entity.insert", || {
                entity_appeared(&view, shard, row)
            });
        });
    }
    {
        let view = view.clone();
        db.game_world_entity().on_update(move |_ctx, old, new| {
            guarded("game_world_entity.update", || {
                entity_changed(&view, shard, old, new)
            });
        });
    }
    {
        let view = view.clone();
        db.game_world_entity().on_delete(move |_ctx, row| {
            guarded("game_world_entity.delete", || entity_vanished(&view, row));
        });
    }

    // ---- game_gameobject -------------------------------------------------------------------
    {
        let view = view.clone();
        db.game_gameobject().on_insert(move |_ctx, row| {
            guarded("game_gameobject.insert", || {
                gameobject_appeared(&view, shard, row)
            });
        });
    }
    {
        let view = view.clone();
        db.game_gameobject().on_update(move |_ctx, _old, row| {
            guarded("game_gameobject.update", || {
                gameobject_appeared(&view, shard, row)
            });
        });
    }
    {
        let view = view.clone();
        db.game_gameobject().on_delete(move |_ctx, row| {
            guarded("game_gameobject.delete", || gameobject_vanished(&view, row));
        });
    }

    // ---- game_entity_motion (peer movement) ------------------------------------------------
    {
        let view = view.clone();
        db.game_entity_motion().on_insert(move |_ctx, row| {
            guarded("game_entity_motion.insert", || motion(&view, row));
        });
    }
    {
        let view = view.clone();
        db.game_entity_motion().on_update(move |_ctx, _old, row| {
            guarded("game_entity_motion.update", || motion(&view, row));
        });
    }

    // ---- game_creature_spline (creature legs) ----------------------------------------------
    {
        let view = view.clone();
        db.game_creature_spline().on_insert(move |_ctx, row| {
            guarded("game_creature_spline.insert", || creature_leg(&view, row));
        });
    }
    {
        let view = view.clone();
        db.game_creature_spline().on_update(move |_ctx, _old, row| {
            guarded("game_creature_spline.update", || creature_leg(&view, row));
        });
    }

    drop(guard);
}

/// Run one shared-connection callback body with a panic firewall. On a per-player connection a
/// panicking row callback cost one session; here it would cost the shard, so the panic is logged
/// and swallowed. Deliberately not a re-panic and not a `Result`: there is no caller to handle it —
/// the SDK pump is the caller.
fn guarded(what: &str, body: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(body)).is_err() {
        log::error!(
            "shared AOI dispatch: a `{what}` callback PANICKED. It was contained (this shard's \
             pump is still running and every other session is unaffected), but the delta it was \
             handling was dropped — a peer may be missing or stale for one or more sessions until \
             its next update."
        );
    }
}

// ===============================================================================================
//  Entity dispatch
// ===============================================================================================

/// A row's cell, taken from the grid columns the WRITER stamped — not recomputed from `x`/`y`.
///
/// The subscription predicate this replaces filtered on `grid_x`/`grid_y`, so reading the same
/// columns is what makes the index and the deleted SQL agree by construction rather than by both
/// happening to call `grid_cell` the same way.
fn entity_key(row: &WorldEntity) -> CellKey {
    CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y)
}

/// A row entered the world (or this shard). Index it, then offer a CREATE to everyone who can see
/// its cell.
fn entity_appeared(view: &WorldView, shard: ShardId, row: &WorldEntity) {
    let key = entity_key(row);
    view.entities.upsert_entity(row.guid, key, shard);
    let row = Arc::new(row.clone());
    for session in view.entities.recipients_of(row.guid, key) {
        offer_create_job(view, shard, session, &row);
    }
}

/// A row changed. Three audiences, decided from the cell delta:
///
/// * sessions that can see the NEW cell get the same "create if unseen, else diff" treatment the
///   per-player `on_update` gave them (work-item 144's re-entry-as-update branch, unchanged);
/// * sessions that could see the OLD cell but not the new one get a DESTROY — the packet the SDK
///   used to produce as an `on_delete` when a row left a box;
/// * nobody else is touched.
fn entity_changed(view: &WorldView, shard: ShardId, old: &WorldEntity, new: &WorldEntity) {
    let key = entity_key(new);
    let previous = view.entities.upsert_entity(new.guid, key, shard);
    let new_recipients = view.entities.recipients_of(new.guid, key);
    if let Some(old_key) = previous {
        if old_key != key {
            let still: HashSet<SessionId> = new_recipients.iter().copied().collect();
            for session in view.entities.viewers_of(old_key) {
                if !still.contains(&session) {
                    destroy_job(view, session, new.guid, new.owner_guid);
                }
            }
        }
    }
    let old = Arc::new(old.clone());
    let new = Arc::new(new.clone());
    for session in new_recipients {
        update_job(view, shard, session, &old, &new);
    }
}

/// The row is gone from the database. Everyone who could see it gets a DESTROY.
fn entity_vanished(view: &WorldView, row: &WorldEntity) {
    // Take the cell the index remembers rather than recomputing from the row: a delete's payload is
    // the LAST known row, and if a move and a delete land in the same transaction the index is the
    // one that knows where the viewers were told it was.
    let key = view
        .entities
        .remove_entity(row.guid)
        .unwrap_or_else(|| entity_key(row));
    for session in view.entities.recipients_of(row.guid, key) {
        destroy_job(view, session, row.guid, row.owner_guid);
    }
}

fn offer_create_job(view: &WorldView, shard: ShardId, session: SessionId, row: &Arc<WorldEntity>) {
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    let Some(coord) = view.shard(shard) else {
        return;
    };
    let row = row.clone();
    let tx = viewer.tx.clone();
    enqueue(&tx, move || {
        super::subscriptions::offer_peer_create_for(&coord, &viewer, &row)
    });
}

fn update_job(
    view: &WorldView,
    shard: ShardId,
    session: SessionId,
    old: &Arc<WorldEntity>,
    new: &Arc<WorldEntity>,
) {
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    let Some(coord) = view.shard(shard) else {
        return;
    };
    let (old, new) = (old.clone(), new.clone());
    let tx = viewer.tx.clone();
    enqueue(&tx, move || {
        super::subscriptions::relay_entity_update(&coord, &viewer, &old, &new)
    });
}

fn destroy_job(view: &WorldView, session: SessionId, guid: u64, owner_guid: u64) {
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    let tx = viewer.tx.clone();
    enqueue(&tx, move || {
        super::subscriptions::relay_peer_destroy(&viewer, guid, owner_guid)
    });
}

// ===============================================================================================
//  Gameobject dispatch
// ===============================================================================================

fn gameobject_appeared(view: &WorldView, shard: ShardId, row: &GameObject) {
    let key = CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y);
    let previous = view.gameobjects.upsert_entity(row.guid, key, shard);
    let recipients = view.gameobjects.viewers_of(key);
    if let Some(old_key) = previous {
        if old_key != key {
            let still: HashSet<SessionId> = recipients.iter().copied().collect();
            for session in view.gameobjects.viewers_of(old_key) {
                if !still.contains(&session) {
                    if let Some(viewer) = view.viewer(session) {
                        let tx = viewer.tx.clone();
                        let guid = row.guid;
                        enqueue(&tx, move || {
                            super::subscriptions::relay_peer_destroy(&viewer, guid, 0)
                        });
                    }
                }
            }
        }
    }
    let Some(coord) = view.shard(shard) else {
        return;
    };
    let row = Arc::new(row.clone());
    for session in recipients {
        let Some(viewer) = view.viewer(session) else {
            continue;
        };
        let (coord, row) = (coord.clone(), row.clone());
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::relay_gameobject_create(&coord, &viewer, &row)
        });
    }
}

fn gameobject_vanished(view: &WorldView, row: &GameObject) {
    let key = view
        .gameobjects
        .remove_entity(row.guid)
        .unwrap_or_else(|| CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y));
    for session in view.gameobjects.viewers_of(key) {
        let Some(viewer) = view.viewer(session) else {
            continue;
        };
        let tx = viewer.tx.clone();
        let guid = row.guid;
        // DESTROY stays ungated (the `on_melee_delete` precedent): SMSG_DESTROY_OBJECT for a guid
        // the client never created is a client no-op, and gating it risks a stale object.
        enqueue(&tx, move || {
            super::subscriptions::relay_gameobject_destroy(&viewer, guid)
        });
    }
}

// ===============================================================================================
//  Motion + creature legs — no index of their own: the row carries its own cell.
// ===============================================================================================

fn motion(view: &WorldView, row: &EntityMotion) {
    let key = CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y);
    let row = Arc::new(row.clone());
    // The MOVER is deliberately not a recipient (`recipients_of`'s self leg is skipped here): a
    // player must never receive their own movement echoed back, or they fight the server for
    // authority over their position. `relay_entity_motion` re-checks it anyway.
    for session in view.entities.viewers_of(key) {
        let Some(viewer) = view.viewer(session) else {
            continue;
        };
        if viewer.self_guid == row.guid {
            continue;
        }
        let (tx, row) = (viewer.tx.clone(), row.clone());
        // The egress shed decision is made HERE, on the pump, deliberately: it is one atomic load,
        // and making it before the enqueue is what keeps a backed-up session from growing its queue
        // with work nobody will read (task B2's contract, unchanged).
        if super::subscriptions::shed_motion_at_depth(tx.depth()) {
            super::subscriptions::MOTION_DROPPED.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let created = viewer.created.clone();
        let self_guid = viewer.self_guid;
        enqueue(&tx, move || {
            super::subscriptions::motion_outbound(&created, self_guid, &row)
        });
    }
}

fn creature_leg(view: &WorldView, row: &CreatureSpline) {
    let key = CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y);
    let row = Arc::new(row.clone());
    for session in view.entities.viewers_of(key) {
        let Some(viewer) = view.viewer(session) else {
            continue;
        };
        let (tx, row) = (viewer.tx.clone(), row.clone());
        if super::subscriptions::shed_motion_at_depth(tx.depth()) {
            super::subscriptions::MOTION_DROPPED.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let created = viewer.created.clone();
        enqueue(&tx, move || {
            super::subscriptions::creature_leg_outbound(&created, &row)
        });
    }
}

// ===============================================================================================
//  The pump→writer hand-off
// ===============================================================================================

/// Push one unit of deferred relay work onto a session's writer queue.
///
/// This is the ONLY thing the shared pump does per recipient — a boxed closure and a channel send.
/// Everything the closure does (gates, cache reads, encoding) happens on that session's own writer
/// thread, so a slow or panicking relay costs exactly one session, which is the property the
/// per-player connections used to give us for free.
fn enqueue(tx: &SessionTx, job: impl FnOnce() -> Vec<Outbound> + Send + 'static) {
    let _ = tx.send(Outbound::Job(Box::new(job)));
}

// ===============================================================================================
//  Viewer recentre — the session-thread half
// ===============================================================================================

/// Move a session's anchor and relay the visibility delta the move implies: a CREATE for everything
/// that entered the box, a DESTROY for everything that left it.
///
/// The subscription engine used to produce these as `on_insert`/`on_delete` when a box was
/// re-subscribed. Runs on the session's OWN movement thread — never on a pump — and does no I/O:
/// the entering rows are read out of the shard coordinator caches already in memory.
pub(crate) fn recenter(view: &WorldView, viewer: &Arc<Viewer>, map_id: u32, x: f32, y: f32) {
    let key = CellKey::of_position(map_id, viewer.instance_id, x, y);
    let entity_delta = view.entities.move_viewer_delta(viewer.session, key);
    let go_delta = view.gameobjects.move_viewer_delta(viewer.session, key);
    if entity_delta.is_none() && go_delta.is_none() {
        return; // no crossing: the by-far-most-common per-heartbeat call, and it allocates nothing
    }
    AOI_RECENTERS.fetch_add(1, Ordering::Relaxed);

    // Everything below is enqueued as a JOB rather than sent directly, for ORDERING: the shard
    // pumps are concurrently enqueueing CREATE/VALUES jobs for the same session, and putting both
    // producers on the one queue is what makes "the DESTROY for a peer I walked away from lands
    // after the CREATE that showed it to me" true rather than a race between two threads.
    if let Some(delta) = entity_delta {
        for (guid, shard) in delta.left {
            let owner_guid = view
                .shard(shard)
                .and_then(|c| {
                    let g = c.0.coord();
                    let row = g.conn.db.game_world_entity().guid().find(&guid);
                    row.map(|r| r.owner_guid)
                })
                .unwrap_or(0);
            let viewer = viewer.clone();
            enqueue(&viewer.tx.clone(), move || {
                super::subscriptions::relay_peer_destroy(&viewer, guid, owner_guid)
            });
        }
        for (guid, shard) in delta.entered {
            let Some(coord) = view.shard(shard) else {
                continue;
            };
            let Some(row) = coord
                .0
                .coord()
                .conn
                .db
                .game_world_entity()
                .guid()
                .find(&guid)
            else {
                continue;
            };
            let (viewer, row) = (viewer.clone(), Arc::new(row));
            enqueue(&viewer.tx.clone(), move || {
                super::subscriptions::offer_peer_create_for(&coord, &viewer, &row)
            });
        }
    }
    if let Some(delta) = go_delta {
        for (guid, _) in delta.left {
            let viewer = viewer.clone();
            enqueue(&viewer.tx.clone(), move || {
                super::subscriptions::relay_gameobject_destroy(&viewer, guid)
            });
        }
        for (guid, shard) in delta.entered {
            let Some(coord) = view.shard(shard) else {
                continue;
            };
            let Some(row) = coord.0.coord().conn.db.game_gameobject().guid().find(&guid) else {
                continue;
            };
            let (viewer, row) = (viewer.clone(), Arc::new(row));
            enqueue(&viewer.tx.clone(), move || {
                super::subscriptions::relay_gameobject_create(&coord, &viewer, &row)
            });
        }
    }
}

/// The world-entry sweep: offer every row already inside the fresh viewer's box.
///
/// A subscription's first apply never reliably fired per-row `on_insert` (work-item 145), which is
/// why the per-player path had a sweep too; here there is no apply at all — the rows were already
/// resident in the coordinator caches long before this session existed — so the sweep is not a
/// belt-and-braces measure, it is the ONLY thing that populates a fresh client's world.
pub(crate) fn sweep_into_view(view: &WorldView, viewer: &Arc<Viewer>) {
    for (guid, shard) in view.entities.visible_entities(viewer.session) {
        let Some(coord) = view.shard(shard) else {
            continue;
        };
        let Some(row) = coord
            .0
            .coord()
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&guid)
        else {
            continue;
        };
        for o in super::subscriptions::offer_peer_create_for(&coord, viewer, &row) {
            let _ = viewer.tx.send(o);
        }
    }
    for (guid, shard) in view.gameobjects.visible_entities(viewer.session) {
        let Some(coord) = view.shard(shard) else {
            continue;
        };
        let Some(row) = coord.0.coord().conn.db.game_gameobject().guid().find(&guid) else {
            continue;
        };
        for o in super::subscriptions::relay_gameobject_create(&coord, viewer, &row) {
            let _ = viewer.tx.send(o);
        }
    }
}

/// Seed the index from the coordinator caches at startup — the rows that were already resident when
/// the gateway connected never fire `on_insert`, so without this the world is invisible until every
/// entity happens to move.
pub(crate) fn seed_from_caches(view: &WorldView) {
    let shards = view.shards.read().unwrap().clone();
    for (id, coord) in shards.iter().enumerate() {
        let guard = coord.0.coord();
        for row in guard.conn.db.game_world_entity().iter() {
            view.entities.upsert_entity(row.guid, entity_key(&row), id);
        }
        for row in guard.conn.db.game_gameobject().iter() {
            view.gameobjects.upsert_entity(
                row.guid,
                CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y),
                id,
            );
        }
    }
    let (entities, _, cells) = view.entities.stats();
    let (gos, _, go_cells) = view.gameobjects.stats();
    log::info!(
        "shared AOI index seeded from {} shard cache(s): {entities} entities in {cells} cells, \
         {gos} gameobjects in {go_cells} cells",
        view.shard_count()
    );
}

