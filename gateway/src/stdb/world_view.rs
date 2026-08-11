//! The SHARED area-of-interest dispatch — one set of row callbacks per shard's
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
//! guids are globally unique across databases, so one index spans the whole realm.
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
//! them, which is exactly the filtering responsibility the shared-connection model shifted here.

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
    /// plan), never re-read from a cache. The lesson: the authoritative value is known at
    /// construction, and a second possibly-stale copy can only disagree with it.
    pub(crate) instance_id: u64,
    pub(crate) tx: SessionTx,
    /// The per-viewer "already shown" dedup set — the exactly-once guarantee behind every CREATE.
    pub(crate) created: Arc<Mutex<HashSet<u64>>>,
    pub(crate) gates: Arc<ViewerGates>,
    /// PLAYER_SKILL_INFO slot map `(skill_line → slot, next_free)` — seeded at login from the same
    /// deterministic layout the CREATE used. Shared with the per-player skill
    /// relays exactly like `created`/`gates`: one instance, whichever leg the flag picks.
    pub(crate) skill_slots: Arc<Mutex<(HashMap<u32, u8>, u8)>>,
    /// This viewer's motion-coalescing buffer. Shared-dispatch-only state (no per-player
    /// twin), so it is constructed inline and pinned by no tripwire.
    pub(crate) motion_pending: Arc<MotionPending>,
}

/// Dense-crowd coalescing: the pump UPSERTS the latest motion row per mover here and queues
/// at most ONE flush job per viewer; a superseded heartbeat dies in the map instead of living as a
/// queued job. A viewer needs only the LATEST position of each mover per flush — with N movers in
/// a cell the per-viewer queue cost is now O(distinct movers), not O(heartbeats), which is the
/// difference between a packed cell starving logins and not (the measured dense-500 numbers).
#[derive(Default)]
pub(crate) struct MotionPending {
    pub(crate) entity: Mutex<HashMap<u64, Arc<EntityMotion>>>,
    pub(crate) creature: Mutex<HashMap<u64, Arc<CreatureSpline>>>,
    /// True while a flush job sits in the viewer's queue. Cleared by the flush BEFORE it drains,
    /// so an upsert racing the drain re-queues a (possibly empty, harmless) follow-up flush
    /// rather than being stranded.
    pub(crate) flush_queued: std::sync::atomic::AtomicBool,
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
    wire_insert(db.game_world_entity(), "game_world_entity.insert", &view, move |v, row| {
        entity_appeared(v, shard, row)
    });
    wire_update(db.game_world_entity(), "game_world_entity.update", &view, move |v, old, new| {
        entity_changed(v, shard, old, new)
    });
    wire_delete(db.game_world_entity(), "game_world_entity.delete", &view, |v, row| {
        entity_vanished(v, row)
    });

    // ---- game_gameobject -------------------------------------------------------------------
    wire_insert(db.game_gameobject(), "game_gameobject.insert", &view, move |v, row| {
        gameobject_appeared(v, shard, row)
    });
    wire_update(db.game_gameobject(), "game_gameobject.update", &view, move |v, _old, row| {
        gameobject_appeared(v, shard, row)
    });
    wire_delete(db.game_gameobject(), "game_gameobject.delete", &view, |v, row| {
        gameobject_vanished(v, row)
    });

    // ---- game_entity_motion (peer movement) ------------------------------------------------
    wire_insert(db.game_entity_motion(), "game_entity_motion.insert", &view, |v, row| {
        motion(v, row)
    });
    wire_update(db.game_entity_motion(), "game_entity_motion.update", &view, |v, _old, row| {
        motion(v, row)
    });

    // ---- game_creature_spline (creature legs) ----------------------------------------------
    wire_insert(db.game_creature_spline(), "game_creature_spline.insert", &view, |v, row| {
        creature_leg(v, row)
    });
    wire_update(db.game_creature_spline(), "game_creature_spline.update", &view, |v, _old, row| {
        creature_leg(v, row)
    });

    // ---- game_roll_event -------------------------------------------------------------------
    // /roll broadcast.
    wire_insert(db.game_roll_event(), "game_roll_event.insert", &view, move |v, row| {
        roll_appeared(v, shard, row)
    });

    // ---- game_rest_state_event --------------------------------------------------------------
    // Rest-state flips (zzz + blue XP bar). Self-only relay.
    wire_insert(db.game_rest_state_event(), "game_rest_state_event.insert", &view, |v, row| {
        rest_state_appeared(v, row)
    });

    // ---- game_dynamic_object -----------------------------------------------------------------
    // Ground-area spell visuals. Shard broadcast, instance-gated per viewer on insert; the
    // delete leg is ungated, matching the per-player relay byte for byte.
    wire_insert(db.game_dynamic_object(), "game_dynamic_object.insert", &view, move |v, row| {
        dynobj_appeared(v, shard, row)
    });
    wire_delete(db.game_dynamic_object(), "game_dynamic_object.delete", &view, move |v, row| {
        dynobj_vanished(v, shard, row)
    });

    // ---- game_corpse --------------------------------------------------------------------------
    // Player corpses. Shard broadcast, instance-gated per viewer on insert/update (the
    // body→bones re-CREATE); the delete leg is ungated, matching the per-player relay.
    wire_insert(db.game_corpse(), "game_corpse.insert", &view, move |v, row| {
        corpse_appeared(v, shard, row)
    });
    wire_update(db.game_corpse(), "game_corpse.update", &view, move |v, _old, row| {
        corpse_changed(v, shard, row)
    });
    wire_delete(db.game_corpse(), "game_corpse.delete", &view, move |v, row| {
        corpse_vanished(v, shard, row)
    });

    // ---- game_combat_event --------------------------------------------------------------------
    // Melee/ranged swing log. Shard broadcast; the created-set visibility gate runs inside the
    // job (`combat_event_outbound`), per viewer.
    wire_insert(db.game_combat_event(), "game_combat_event.insert", &view, move |v, row| {
        combat_event_appeared(v, shard, row)
    });

    // ---- game_melee_attack --------------------------------------------------------------------
    // Combat stance (ATTACKSTART/STOP + the owner-only ranged CANCEL_AUTO_REPEAT).
    wire_insert(db.game_melee_attack(), "game_melee_attack.insert", &view, move |v, row| {
        melee_engaged(v, shard, row)
    });
    wire_delete(db.game_melee_attack(), "game_melee_attack.delete", &view, move |v, row| {
        melee_disengaged(v, shard, row)
    });

    // ---- game_resurrect_request / game_whisper_event / game_group_event -----------------------
    // The PRIVATE tier: every row is addressed to exactly one recipient, and on this feed the
    // gateway owns the guarantee RLS used to give. Delivery is recipient-keyed (the owner-session
    // lookup) + the explicit `private_recipient_audience` predicate — never a viewer fan.
    // (The realm-core whisper/group twins for CROSS-shard delivery ride the same dispatchers,
    // armed once per realm-core connection by `arm_realm_private` below.)
    wire_insert(db.game_resurrect_request(), "game_resurrect_request.insert", &view, |v, row| {
        resurrect_offered(v, row)
    });
    wire_insert(db.game_whisper_event(), "game_whisper_event.insert", &view, |v, row| {
        whisper_appeared(v, row)
    });
    {
        let coord = coord.clone();
        wire_insert(db.game_group_event(), "game_group_event.insert", &view, move |v, row| {
            group_event_appeared(v, &coord, row)
        });
    }

    // ---- game_aura ------------------------------------------------------------------------------
    // The aura composite: array sync (visibility-gated), self-only duration/run-speed/armor,
    // and the stealth HIDE/REVEAL transitions. The A_STEALTH count is computed HERE on the pump,
    // where this connection's cache is exactly post-change; everything else runs in the job.
    {
        let view = view.clone();
        let coord = coord.clone();
        db.game_aura().on_insert(move |ctx, row| {
            let n = ctx
                .db
                .game_aura()
                .iter()
                .filter(|a| {
                    a.target_guid == row.target_guid
                        && a.eff_kind == super::subscriptions::A_STEALTH
                })
                .count();
            guarded("game_aura.insert", || aura_applied(&view, &coord, shard, row, n));
        });
    }
    {
        let view = view.clone();
        let coord = coord.clone();
        db.game_aura().on_update(move |_ctx, old, row| {
            let expires_changed = old.expires_at != row.expires_at;
            guarded("game_aura.update", || {
                aura_updated(&view, &coord, shard, row, expires_changed)
            });
        });
    }
    {
        let view = view.clone();
        let coord = coord.clone();
        db.game_aura().on_delete(move |ctx, row| {
            let n = ctx
                .db
                .game_aura()
                .iter()
                .filter(|a| {
                    a.target_guid == row.target_guid
                        && a.eff_kind == super::subscriptions::A_STEALTH
                })
                .count();
            guarded("game_aura.delete", || aura_removed(&view, &coord, shard, row, n));
        });
    }

    // ---- game_spell_cast_event / game_spell_impact_event --------------------------------------
    // Cast visuals (the full cast-lock contract, per viewer) + deferred projectile impact logs.
    wire_insert(db.game_spell_cast_event(), "game_spell_cast_event.insert", &view, move |v, row| {
        cast_event_appeared(v, shard, row)
    });
    wire_insert(db.game_spell_impact_event(), "game_spell_impact_event.insert", &view, move |v, row| {
        impact_appeared(v, shard, row)
    });

    // ---- game_emote_event / game_chat_event / game_channel_event ------------------------------
    // The social tier. Emotes broadcast; say/yell range-gate per viewer; channel lines
    // membership-gate per viewer — each inside the job, via the shared bodies.
    {
        let coord = coord.clone();
        wire_insert(db.game_emote_event(), "game_emote_event.insert", &view, move |v, row| {
            emote_appeared(v, &coord, shard, row)
        });
    }
    {
        let coord = coord.clone();
        wire_insert(db.game_chat_event(), "game_chat_event.insert", &view, move |v, row| {
            chat_appeared(v, &coord, shard, row)
        });
    }
    {
        let coord = coord.clone();
        wire_insert(db.game_channel_event(), "game_channel_event.insert", &view, move |v, row| {
            channel_appeared(v, &coord, shard, row)
        });
    }

    // ---- game_player_skill ----------------------------------------------------------------------
    // Live skill pane. Self-only relay; insert (line learned) and update (skill-up) run the same
    // body against the viewer's own slot map.
    wire_insert(db.game_player_skill(), "game_player_skill.insert", &view, |v, row| {
        skill_changed(v, row)
    });
    wire_update(db.game_player_skill(), "game_player_skill.update", &view, |v, _old, row| {
        skill_changed(v, row)
    });

    drop(guard);
}

/// Register the cross-shard PRIVATE-tier twins (#22 → #483) on the REALM-CORE connection: whisper
/// and group events written realm-side for a recipient whose home shard is elsewhere. Same
/// recipient-keyed dispatchers as `arm_shard`'s private tier, armed ONCE per realm-core
/// connection instead of once per session — the last per-session registrations are gone (#483).
///
/// Only called when realm-core is a DISTINCT database (`Coordinator::connect` gates it): on a
/// single-database gateway the world shard's own `arm_shard` registration already watches these
/// tables, and a second registration would deliver every private packet twice.
///
/// `coord` is a WORLD handle (the default shard) — `group_event_appeared` needs one for the
/// QUEST_SHARE detail JOIN; realm-core's own cache holds no quest catalogue.
///
/// Re-reads `realm.0.coord()` on every call, so it doubles as the watchdog's post-reconnect
/// re-arm hook, exactly like `arm_shard`.
pub(crate) fn arm_realm_private(view: Arc<WorldView>, realm: Coordinator, coord: Coordinator) {
    let guard = realm.0.coord();
    let db = &guard.conn.db;
    wire_insert(db.game_whisper_event(), "realm.game_whisper_event.insert", &view, |v, row| {
        whisper_appeared(v, row)
    });
    wire_insert(db.game_group_event(), "realm.game_group_event.insert", &view, move |v, row| {
        group_event_appeared(v, &coord, row)
    });
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

// -----------------------------------------------------------------------------------------------
//  Registration helpers — the clone-per-callback + `guarded(label, ...)` shape that every
//  `arm_shard` registration repeated verbatim, factored to one line per callsite. `T::Row` and
//  `EventContext` are pinned to the module's generated types, so a mismatched table/body pairing
//  is a compile error, not a mislabeled log line.
// -----------------------------------------------------------------------------------------------

/// Register an INSERT callback: `wire_insert(db.game_x(), "game_x.insert", &view, |v, row| body(v, row));`
fn wire_insert<T>(
    table: T,
    label: &'static str,
    view: &Arc<WorldView>,
    f: impl Fn(&Arc<WorldView>, &T::Row) + Send + 'static,
) where
    T: Table<EventContext = EventContext>,
{
    let view = view.clone();
    table.on_insert(move |_ctx, row| {
        guarded(label, || f(&view, row));
    });
}

/// Register an UPDATE callback (see [`wire_insert`]); `f` gets `(view, old, new)`.
fn wire_update<T>(
    table: T,
    label: &'static str,
    view: &Arc<WorldView>,
    f: impl Fn(&Arc<WorldView>, &T::Row, &T::Row) + Send + 'static,
) where
    T: TableWithPrimaryKey<EventContext = EventContext>,
{
    let view = view.clone();
    table.on_update(move |_ctx, old, new| {
        guarded(label, || f(&view, old, new));
    });
}

/// Register a DELETE callback (see [`wire_insert`]).
fn wire_delete<T>(
    table: T,
    label: &'static str,
    view: &Arc<WorldView>,
    f: impl Fn(&Arc<WorldView>, &T::Row) + Send + 'static,
) where
    T: Table<EventContext = EventContext>,
{
    let view = view.clone();
    table.on_delete(move |_ctx, row| {
        guarded(label, || f(&view, row));
    });
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
///   per-player `on_update` gave them (the re-entry-as-update branch, unchanged);
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
        // SUPERSEDE, don't append — the viewer needs only this mover's latest position, so
        // the upsert replaces any not-yet-flushed heartbeat from the same mover and the queue
        // carries at most one flush job regardless of crowd size.
        viewer
            .motion_pending
            .entity
            .lock()
            .unwrap()
            .insert(row.guid, row.clone());
        maybe_queue_motion_flush(view, &viewer);
    }
}

fn creature_leg(view: &WorldView, row: &CreatureSpline) {
    let key = CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y);
    let row = Arc::new(row.clone());
    for session in view.entities.viewers_of(key) {
        let Some(viewer) = view.viewer(session) else {
            continue;
        };
        // Same supersession as player motion — latest leg per creature wins.
        viewer
            .motion_pending
            .creature
            .lock()
            .unwrap()
            .insert(row.guid, row.clone());
        maybe_queue_motion_flush(view, &viewer);
    }
}

/// Queue this viewer's ONE motion-flush job, unless one is already queued. The egress shed
/// decision keeps its pump-side, one-atomic-load shape (task B2's contract): a shed drops the
/// FLUSH — the pending map keeps superseding meanwhile, the flag resets so a later heartbeat
/// retries, and nothing is stranded.
fn maybe_queue_motion_flush(_view: &WorldView, viewer: &Arc<Viewer>) {
    use std::sync::atomic::Ordering as AtOrd;
    if viewer.motion_pending.flush_queued.swap(true, AtOrd::AcqRel) {
        return; // a queued flush will carry the upsert that just landed
    }
    let tx = viewer.tx.clone();
    if super::subscriptions::shed_motion_at_depth(tx.depth()) {
        super::subscriptions::MOTION_DROPPED.fetch_add(1, Ordering::Relaxed);
        viewer.motion_pending.flush_queued.store(false, AtOrd::Release);
        return;
    }
    let viewer = viewer.clone();
    enqueue(&tx, move || {
        // Clear BEFORE draining: an upsert racing the drain re-queues a follow-up flush (possibly
        // empty — harmless) instead of being stranded behind a still-set flag.
        viewer
            .motion_pending
            .flush_queued
            .store(false, AtOrd::Release);
        let moves: Vec<Arc<EntityMotion>> = {
            let mut m = viewer.motion_pending.entity.lock().unwrap();
            m.drain().map(|(_, r)| r).collect()
        };
        let legs: Vec<Arc<CreatureSpline>> = {
            let mut m = viewer.motion_pending.creature.lock().unwrap();
            m.drain().map(|(_, r)| r).collect()
        };
        let mut out = Vec::new();
        for row in moves {
            out.extend(super::subscriptions::motion_outbound(
                &viewer.created,
                viewer.self_guid,
                &row,
            ));
        }
        for row in legs {
            out.extend(super::subscriptions::creature_leg_outbound(
                &viewer.created,
                &row,
            ));
        }
        out
    });
}

// ===============================================================================================
//  Non-spatial dispatch (shared-call path)
// ===============================================================================================

/// Every viewer a shard-broadcast family may reach: the sessions whose characters `shard`'s
/// database owns — the same audience the per-player subscriptions produced (each player connection
/// was to its own shard's database, and the base queries were unscoped `SELECT *`s). Filtered by
/// [`shard_audience`]; family-specific gates (instance, range) run per viewer at the call site.
fn viewers_on_shard(view: &WorldView, shard: ShardId) -> Vec<Arc<Viewer>> {
    view.viewers
        .read()
        .unwrap()
        .values()
        .filter(|v| shard_audience(view.entities.shard_of(v.self_guid), shard))
        .cloned()
        .collect()
}

/// A /roll landed on `shard`'s coordinator feed. Rolls are public (vanilla broadcasts every /roll,
/// roller included), so the shard audience is the whole predicate. The packet build runs in the
/// job, on each session's own writer thread, per this file's rule 1.
fn roll_appeared(view: &WorldView, shard: ShardId, row: &RollEvent) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let row = row.clone();
        let tx = viewer.tx.clone();
        enqueue(&tx, move || super::subscriptions::relay_roll(&row));
    }
}

/// A ground-area dynamic object appeared (Consecration's swirl etc.) → CREATE for every
/// same-instance viewer on the row's shard — the same `instance_relay_gate` the per-player leg
/// runs. Short-lived, so no login/AOI resident sweep (a mid-area login misses the visual),
/// exactly as before.
fn dynobj_appeared(view: &WorldView, shard: ShardId, row: &DynamicObject) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        if !super::subscriptions::instance_relay_gate(row.instance_id, Some(viewer.instance_id)) {
            continue;
        }
        let row = row.clone();
        let tx = viewer.tx.clone();
        enqueue(&tx, move || super::subscriptions::relay_dynobj_create(&row));
    }
}

/// A dynamic object's area ended → DESTROY to every viewer on the shard. Deliberately NOT
/// instance-gated: the per-player delete leg never was (a DESTROY for a guid the client doesn't
/// know is a no-op), and the A/B contract is byte-for-byte equality.
fn dynobj_vanished(view: &WorldView, shard: ShardId, row: &DynamicObject) {
    let guid = row.guid;
    for viewer in viewers_on_shard(view, shard) {
        let tx = viewer.tx.clone();
        enqueue(&tx, move || super::subscriptions::relay_destroy_object(guid));
    }
}

/// A rest-state flip landed on a shard's coordinator feed. Self-only family: the row names its
/// owner and the owner is the ONLY lawful recipient, so the audience predicate IS the
/// owner-session lookup (`WorldIndex::session_of_owner`) — there is no candidate set to filter,
/// and a row whose owner has no live session here is dropped.
///
/// The per-player leg's `ReplayGate` (login initial-sync suppression) has no shared twin by
/// construction: nothing re-subscribes per session on this path, so rows arriving after arming are
/// live flips. (A coordinator watchdog re-arm can replay rows still inside their GC TTL — an
/// idempotent re-send of the current rest byte, the same class every coordinator-ridden 279 relay
/// already accepts.)
fn rest_state_appeared(view: &WorldView, row: &RestStateEvent) {
    let Some(session) = view.entities.session_of_owner(row.character_guid) else {
        return;
    };
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    let (self_guid, bytes) = (row.character_guid, row.player_bytes_2);
    enqueue(&viewer.tx, move || {
        super::subscriptions::relay_rest_state(self_guid, bytes)
    });
}

/// A skill row changed (line learned / skill-up). Self-only family, same owner-session-lookup
/// audience as [`rest_state_appeared`] — the slot allocation runs in the job on the owner's own
/// writer thread, against the viewer's `skill_slots` (the same map the per-player leg uses).
fn skill_changed(view: &WorldView, row: &PlayerSkill) {
    let Some(session) = view.entities.session_of_owner(row.character_guid) else {
        return;
    };
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    let (line, cur, max) = (row.skill_line, row.current, row.max_rank);
    let tx = viewer.tx.clone();
    enqueue(&tx, move || {
        super::subscriptions::relay_skill(&viewer, line, cur, max)
    });
}

/// A player corpse appeared → CREATE for every same-instance viewer on the row's shard, with the
/// owner's reclaim-delay packet riding inside (`corpse_create_outbound`, the same body the
/// per-player relay and the login resident sweep use).
fn corpse_appeared(view: &WorldView, shard: ShardId, row: &Corpse) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        if !super::subscriptions::instance_relay_gate(row.instance_id, Some(viewer.instance_id)) {
            continue;
        }
        let row = row.clone();
        let self_guid = viewer.self_guid;
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::corpse_create_outbound(self_guid, &row)
        });
    }
}

/// A corpse's state changed (body → bones decay) → re-emit the CREATE so the client re-renders it,
/// same instance gate as the insert leg (the re-emit must not leak across instances either).
fn corpse_changed(view: &WorldView, shard: ShardId, row: &Corpse) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        if !super::subscriptions::instance_relay_gate(row.instance_id, Some(viewer.instance_id)) {
            continue;
        }
        let row = row.clone();
        let tx = viewer.tx.clone();
        enqueue(&tx, move || super::subscriptions::relay_corpse_update(&row));
    }
}

/// A corpse was reclaimed/decayed → DESTROY to every viewer on the shard. Ungated like the
/// per-player delete leg (a DESTROY for an unknown guid is a client no-op).
fn corpse_vanished(view: &WorldView, shard: ShardId, row: &Corpse) {
    let guid = row.guid;
    for viewer in viewers_on_shard(view, shard) {
        let tx = viewer.tx.clone();
        enqueue(&tx, move || super::subscriptions::relay_destroy_object(guid));
    }
}

/// A swing landed → the full 097 packet fork (`combat_event_outbound`: ranged GO + delayed damage
/// log / melee ATTACKERSTATEUPDATE / killing-blow ATTACKSTOP) per viewer, gated on that viewer's
/// `created` set inside the job.
fn combat_event_appeared(view: &WorldView, shard: ShardId, row: &CombatEvent) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let row = row.clone();
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::combat_event_outbound(&viewer.tx, &viewer.created, &row)
        });
    }
}

/// A melee/ranged engagement began → SMSG_ATTACKSTART per viewer (`melee_engage_outbound` runs
/// the ranged skip + created gate in the job).
fn melee_engaged(view: &WorldView, shard: ShardId, row: &MeleeAttack) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let row = row.clone();
        let created = viewer.created.clone();
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::melee_engage_outbound(&created, &row)
        });
    }
}

/// An engagement row was removed → ATTACKSTOP / the owner-only CANCEL_AUTO_REPEAT
/// (`melee_disengage_outbound`, keyed on each viewer's own guid).
fn melee_disengaged(view: &WorldView, shard: ShardId, row: &MeleeAttack) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let row = row.clone();
        let self_guid = viewer.self_guid;
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::melee_disengage_outbound(self_guid, &row)
        });
    }
}

/// A resurrect offer landed → SMSG_RESURRECT_REQUEST to the offer's TARGET and nobody else.
fn resurrect_offered(view: &WorldView, row: &ResurrectRequest) {
    let Some(session) = view.entities.session_of_owner(row.target_guid) else {
        return;
    };
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    if !super::subscriptions::private_recipient_audience(row.target_guid, viewer.self_guid) {
        return;
    }
    let row = row.clone();
    let tx = viewer.tx.clone();
    enqueue(&tx, move || {
        super::subscriptions::resurrect_request_outbound(&row)
    });
}

/// A whisper landed → SMSG_MESSAGECHAT to the row's RECIPIENT and nobody else. This is the RLS
/// relocation the 4c plan names: the lookup is by the recipient's guid, so a non-recipient
/// session is structurally unreachable, and the explicit predicate re-asserts it.
fn whisper_appeared(view: &WorldView, row: &WhisperEvent) {
    let Some(session) = view.entities.session_of_owner(row.recipient_guid) else {
        return;
    };
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    if !super::subscriptions::private_recipient_audience(row.recipient_guid, viewer.self_guid) {
        return;
    }
    let row = row.clone();
    let tx = viewer.tx.clone();
    enqueue(&tx, move || {
        super::subscriptions::whisper_event_outbound(&row)
    });
}

/// A group/loot-roll/quest-share event landed → the kind-decoded packet to the row's RECIPIENT
/// and nobody else (same shape as [`whisper_appeared`]; `coord` feeds the QUEST_SHARE detail
/// JOIN inside the job).
fn group_event_appeared(view: &WorldView, coord: &Coordinator, row: &GroupEvent) {
    let Some(session) = view.entities.session_of_owner(row.recipient_guid) else {
        return;
    };
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    if !super::subscriptions::private_recipient_audience(row.recipient_guid, viewer.self_guid) {
        return;
    }
    let (row, coord) = (row.clone(), coord.clone());
    let self_guid = viewer.self_guid;
    let tx = viewer.tx.clone();
    enqueue(&tx, move || {
        super::subscriptions::group_event_outbound(&coord, self_guid, &row)
    });
}

/// An aura appeared → per viewer: array sync + self-only packets + the stealth HIDE transition
/// (`aura_insert_outbound`, all gates inside the job against that viewer's own state).
fn aura_applied(view: &Arc<WorldView>, coord: &Coordinator, shard: ShardId, row: &Aura, stealth_count: usize) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let (row, coord, view) = (row.clone(), coord.clone(), view.clone());
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::aura_insert_outbound(
                &coord,
                &view,
                viewer.session,
                &viewer.created,
                viewer.self_guid,
                &row,
                stealth_count,
            )
        });
    }
}

/// An aura changed → per viewer: array sync + refresh-only duration + self-only packets.
fn aura_updated(view: &Arc<WorldView>, coord: &Coordinator, shard: ShardId, row: &Aura, expires_changed: bool) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let (row, coord) = (row.clone(), coord.clone());
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::aura_update_outbound(
                &coord,
                &viewer.created,
                viewer.self_guid,
                &row,
                expires_changed,
            )
        });
    }
}

/// An aura expired/was removed → per viewer: array sync + self-only packets + the stealth REVEAL
/// transition.
fn aura_removed(view: &Arc<WorldView>, coord: &Coordinator, shard: ShardId, row: &Aura, stealth_count: usize) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let (row, coord, view) = (row.clone(), coord.clone(), view.clone());
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::aura_delete_outbound(
                &coord,
                &view,
                viewer.session,
                &viewer.created,
                viewer.self_guid,
                &row,
                stealth_count,
            )
        });
    }
}

/// A cast event landed → the full cast-lock sequence per viewer (`cast_event_outbound`: the
/// caster-private branches key on each viewer's own guid inside the job).
fn cast_event_appeared(view: &WorldView, shard: ShardId, row: &SpellCastEvent) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let row = row.clone();
        let self_guid = viewer.self_guid;
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::cast_event_outbound(self_guid, &row)
        });
    }
}

/// A projectile impact landed → the floating damage number per viewer (broadcast, like the
/// per-player leg).
fn impact_appeared(view: &WorldView, shard: ShardId, row: &SpellImpactEvent) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let row = row.clone();
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::impact_event_outbound(&row)
        });
    }
}

/// An emote landed → SMSG_TEXT_EMOTE + SMSG_EMOTE per viewer on the shard (broadcast, no range
/// gate — parity with the per-player leg). Name resolve runs in the job against `coord`'s cache.
fn emote_appeared(view: &WorldView, coord: &Coordinator, shard: ShardId, row: &EmoteEvent) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let (row, coord) = (row.clone(), coord.clone());
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::emote_event_outbound(&coord, &row)
        });
    }
}

/// A say/yell line landed → SMSG_MESSAGECHAT per viewer, `chat_in_range`-gated inside the job
/// (`chat_event_outbound`: speaker echo, SAY/YELL radii, map+instance fence, coordinator-cache
/// endpoint lookups).
fn chat_appeared(view: &WorldView, coord: &Coordinator, shard: ShardId, row: &ChatEvent) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let (row, coord) = (row.clone(), coord.clone());
        let self_guid = viewer.self_guid;
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::chat_event_outbound(&coord, self_guid, &row)
        });
    }
}

/// A channel line landed → SMSG_MESSAGECHAT per MEMBER (membership checked in the job against the
/// coordinator's `game_channel_member` cache — membership IS the audience, no proximity).
fn channel_appeared(view: &WorldView, coord: &Coordinator, shard: ShardId, row: &ChannelEvent) {
    let row = Arc::new(row.clone());
    for viewer in viewers_on_shard(view, shard) {
        let (row, coord) = (row.clone(), coord.clone());
        let self_guid = viewer.self_guid;
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::channel_event_outbound(&coord, self_guid, &row)
        });
    }
}

/// The shared shard-identity predicate every non-spatial broadcast family starts from (via
/// [`viewers_on_shard`]): a viewer is in a row's audience iff the shard that owns their character
/// is the shard the row landed on. `None` (the viewer's own entity not indexed yet — the login
/// race window) denies, matching the per-player path where a not-yet-applied subscription
/// delivers nothing.
fn shard_audience(viewer_shard: Option<ShardId>, row_shard: ShardId) -> bool {
    viewer_shard == Some(row_shard)
}

#[cfg(test)]
mod family_audience_tests {
    use super::shard_audience;
    use crate::stdb::subscriptions::private_recipient_audience;

    #[test]
    fn shard_broadcasts_reach_only_the_rows_shard_and_deny_unindexed_viewers() {
        assert!(shard_audience(Some(0), 0));
        assert!(!shard_audience(Some(1), 0), "another shard's viewer must not hear the broadcast");
        assert!(!shard_audience(None, 0), "an unindexed viewer (login race) is denied, not defaulted in");
    }

    /// The private tier's whole privacy guarantee (whisper/group/resurrect): the addressee and
    /// NOBODY else. The zero cases are the leak vectors — an unset recipient must never match a
    /// real viewer and a real recipient must never match a half-initialized viewer.
    #[test]
    fn private_rows_reach_their_addressee_and_nobody_else() {
        assert!(private_recipient_audience(42, 42));
        assert!(!private_recipient_audience(42, 43), "a third party must never receive a private row");
        assert!(!private_recipient_audience(0, 42), "recipient 0 (unaddressed) matches no viewer");
        assert!(!private_recipient_audience(42, 0), "an uninitialized viewer guid matches no row");
        assert!(
            !private_recipient_audience(0, 0),
            "the all-zero case is a leak, not a match — both sides unset must still deny"
        );
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
/// A subscription's first apply never reliably fired per-row `on_insert`, which is
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

