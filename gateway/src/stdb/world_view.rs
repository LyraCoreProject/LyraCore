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
//! After: spatial, broadcast, private, and owner-addressed tables ride the coordinator's existing
//! global subscription (one per shard). The callbacks below are registered ONCE per shard.
//! [`WorldIndex`] answers spatial audience questions; typed owner and identity indexes select one
//! viewer for addressed rows. Every shard's stream feeds ONE view spanning the whole realm.
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
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{anyhow, Result};
use spacetimedb_sdk::{Table, TableWithPrimaryKey};
use wow_world_messages::vanilla::WeatherChangeType;

use super::aoi::{ViewerGates, AOI_RECENTERS};
use super::bindings::*;
use super::connection::Coordinator;
use super::world_index::{CellKey, EntityLayer, SessionId, ShardId, WorldIndex};
use crate::world::{Outbound, SessionTx};

/// Everything the dispatch needs to know about one live world session. Registered at world entry
/// (`subscribe_player_events`) and dropped by `PlayerSubscriptions`'s guard.
///
/// Every field here is session relay state used by the shared callbacks' writer jobs.
pub(crate) struct Viewer {
    pub(crate) session: SessionId,
    pub(crate) self_guid: u64,
    /// The account identity stamped on identity-addressed event rows.
    pub(crate) bound_identity: spacetimedb_sdk::Identity,
    /// The map this session is on. Constant for the session's lifetime — every path that changes
    /// a character's map (cross-map teleport, portal entry) tears the subscriptions
    /// down and calls `subscribe_player_events` again, which builds a fresh viewer.
    pub(crate) map_id: u32,
    /// The partition this session is playing in — supplied by the caller (login row / transfer
    /// plan), never re-read from a cache. The lesson: the authoritative value is known at
    /// construction, and a second possibly-stale copy can only disagree with it.
    pub(crate) instance_id: u64,
    /// The zone this session is standing in — the weather relay's whole audience predicate.
    ///
    /// Unlike `map_id` this changes inside one session: walking, a same-map teleport and a taxi
    /// landing all cross zone boundaries without tearing the viewer down, so it is interior-mutable
    /// and written from the self entity's own `zone_id` deltas. `0` is the unresolved zone and
    /// matches no weather row. Seeded at `add_viewer` time with the zone world entry already sent
    /// weather for, so the first stored value is never a phantom crossing.
    pub(crate) zone_id: AtomicU32,
    pub(crate) tx: SessionTx,
    /// The per-viewer "already shown" dedup set — the exactly-once guarantee behind every CREATE.
    pub(crate) created: Arc<Mutex<HashSet<u64>>>,
    pub(crate) gates: Arc<ViewerGates>,
    /// PLAYER_SKILL_INFO slot map `(skill_line → slot, next_free)` — seeded at login from the same
    /// deterministic layout the CREATE used.
    pub(crate) skill_slots: Arc<Mutex<(HashMap<u32, u8>, u8)>>,
    /// Areas known before this viewer entered the world. Reconnect replays use the same set, so
    /// restoring fog never repeats the discovery feedback.
    pub(crate) explored: Mutex<ExplorationReplay>,
    /// This viewer's motion-coalescing buffer. Shared-dispatch-only state (no per-player
    /// twin), so it is constructed inline and pinned by no tripwire.
    pub(crate) motion_pending: Arc<MotionPending>,
}

impl Viewer {
    /// Move this viewer's weather routing into `zone_id`, answering whether that was a real
    /// crossing — the ONE place [`Viewer::zone_id`] is written after construction.
    ///
    /// The unresolved zone (`0`) is not a crossing and never overwrites a known zone: it means the
    /// Module's terrain could not answer, not that the player left every zone, and treating it as a
    /// destination would send fine weather over a real storm.
    fn enter_zone(&self, zone_id: u32) -> bool {
        zone_id != 0 && self.zone_id.swap(zone_id, Ordering::Relaxed) != zone_id
    }
}

/// Per-viewer exploration replay state. `area_bit` is stable across a transfer's re-minted row
/// ids, so it identifies discoveries rather than the table's surrogate key.
#[derive(Default)]
pub(crate) struct ExplorationReplay {
    announced: HashSet<i32>,
}

impl ExplorationReplay {
    pub(crate) fn seed(&mut self, area_bits: impl IntoIterator<Item = i32>) {
        self.announced.extend(area_bits);
    }

    fn admit_once(&mut self, area_bit: i32) -> bool {
        self.announced.insert(area_bit)
    }
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

struct RegisteredViewer {
    viewer: Arc<Viewer>,
    shard: ShardId,
}

/// A viewer address carried by owner-GUID-scoped rows.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct OwnerGuid(pub(crate) u64);

/// A viewer address carried by identity-scoped rows.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct BoundIdentity(pub(crate) spacetimedb_sdk::Identity);

#[derive(Default)]
struct ViewerRegistry {
    by_session: HashMap<SessionId, RegisteredViewer>,
    session_of_owner: HashMap<u64, SessionId>,
    session_of_identity: HashMap<spacetimedb_sdk::Identity, SessionId>,
    sessions_by_shard: HashMap<ShardId, HashSet<SessionId>>,
}

impl ViewerRegistry {
    fn register(&mut self, viewer: Arc<Viewer>, shard: ShardId) {
        let session = viewer.session;
        if let Some(previous) = self.by_session.remove(&session) {
            self.remove_indexes(session, &previous);
        }
        self.session_of_owner.insert(viewer.self_guid, session);
        self.session_of_identity
            .insert(viewer.bound_identity, session);
        self.sessions_by_shard
            .entry(shard)
            .or_default()
            .insert(session);
        self.by_session
            .insert(session, RegisteredViewer { viewer, shard });
    }

    fn remove(&mut self, session: SessionId) {
        let Some(registered) = self.by_session.remove(&session) else {
            return;
        };
        self.remove_indexes(session, &registered);
    }

    fn remove_indexes(&mut self, session: SessionId, registered: &RegisteredViewer) {
        let owner = registered.viewer.self_guid;
        if self.session_of_owner.get(&owner) == Some(&session) {
            self.session_of_owner.remove(&owner);
        }
        let identity = registered.viewer.bound_identity;
        if self.session_of_identity.get(&identity) == Some(&session) {
            self.session_of_identity.remove(&identity);
        }
        if let Some(sessions) = self.sessions_by_shard.get_mut(&registered.shard) {
            sessions.remove(&session);
            if sessions.is_empty() {
                self.sessions_by_shard.remove(&registered.shard);
            }
        }
    }
}

/// The gateway-wide shared view: one layered spatial index, the live viewer registry, and the
/// shard table the index's [`ShardId`]s point into.
pub(crate) struct WorldView {
    /// One authoritative viewer anchor with independent world-entity and game-object layers.
    pub(crate) spatial: WorldIndex,
    viewers: RwLock<ViewerRegistry>,
    /// [`ShardId`] → that shard's coordinator handle. Written once at startup, read on every
    /// dispatch; the order fixes the ids, so it must never be reordered after arming.
    shards: RwLock<Vec<Coordinator>>,
    next_session: AtomicU64,
}

impl WorldView {
    pub(crate) fn new(cell_scoped: bool) -> Self {
        Self {
            spatial: WorldIndex::new(cell_scoped),
            viewers: RwLock::new(ViewerRegistry::default()),
            shards: RwLock::new(Vec::new()),
            next_session: AtomicU64::new(1),
        }
    }

    fn viewer(&self, session: SessionId) -> Option<Arc<Viewer>> {
        self.viewers
            .read()
            .unwrap()
            .by_session
            .get(&session)
            .map(|registered| registered.viewer.clone())
    }

    fn session_of_owner(&self, self_guid: u64) -> Option<SessionId> {
        self.viewers
            .read()
            .unwrap()
            .session_of_owner
            .get(&self_guid)
            .copied()
    }

    /// Test probe: the owner's viewer regardless of shard. Production owner relays resolve
    /// through [`Self::viewer_of_owner_on_shard`].
    #[cfg(test)]
    pub(crate) fn viewer_of_owner(&self, owner: OwnerGuid) -> Option<Arc<Viewer>> {
        let registry = self.viewers.read().unwrap();
        let session = registry.session_of_owner.get(&owner.0)?;
        registry
            .by_session
            .get(session)
            .map(|registered| registered.viewer.clone())
    }

    /// The owner's viewer, only when it is registered on `shard`. Shard-row owner relays
    /// (teleport/quest/exploration/reputation/item) resolve through this so a transfer's source
    /// shard cannot address the viewer after it re-registered on the destination shard.
    pub(crate) fn viewer_of_owner_on_shard(
        &self,
        shard: ShardId,
        owner: OwnerGuid,
    ) -> Option<Arc<Viewer>> {
        let registry = self.viewers.read().unwrap();
        let session = registry.session_of_owner.get(&owner.0)?;
        let registered = registry.by_session.get(session)?;
        (registered.shard == shard).then(|| registered.viewer.clone())
    }

    pub(crate) fn viewer_of_identity(&self, identity: BoundIdentity) -> Option<Arc<Viewer>> {
        let registry = self.viewers.read().unwrap();
        let session = registry.session_of_identity.get(&identity.0)?;
        registry
            .by_session
            .get(session)
            .map(|registered| registered.viewer.clone())
    }

    fn viewers_on_shard(&self, shard: ShardId) -> Vec<Arc<Viewer>> {
        let registry = self.viewers.read().unwrap();
        registry
            .sessions_by_shard
            .get(&shard)
            .into_iter()
            .flatten()
            .filter_map(|session| {
                registry
                    .by_session
                    .get(session)
                    .map(|registered| registered.viewer.clone())
            })
            .collect()
    }

    fn shard(&self, id: ShardId) -> Option<Coordinator> {
        self.shards.read().unwrap().get(id).cloned()
    }

    /// Mint a session id. Monotonic and never reused, so a relogin can never inherit a previous
    /// session's interest set.
    pub(crate) fn next_session_id(&self) -> SessionId {
        self.next_session.fetch_add(1, Ordering::Relaxed)
    }

    /// Register a live session on the shard owned by `coord`. The coordinator must be one of the
    /// stable handles installed by [`Self::set_shards`].
    pub(crate) fn add_viewer(
        &self,
        coord: &Coordinator,
        viewer: Arc<Viewer>,
        anchor: CellKey,
    ) -> Result<()> {
        let shards = self.shards.read().unwrap();
        let shard = shards
            .iter()
            .position(|registered| registered.shard_name() == coord.shard_name())
            .ok_or_else(|| {
                anyhow!(
                    "cannot register viewer {}: coordinator shard `{}` is absent from the world-view shard table ({:?})",
                    viewer.session,
                    coord.shard_name(),
                    shards.iter().map(|known| known.shard_name()).collect::<Vec<_>>()
                )
            })?;
        drop(shards);
        self.add_viewer_on_shard(viewer, anchor, shard);
        Ok(())
    }

    pub(crate) fn add_viewer_on_shard(&self, viewer: Arc<Viewer>, anchor: CellKey, shard: ShardId) {
        let mut registry = self.viewers.write().unwrap();
        self.spatial.add_viewer(viewer.session, anchor);
        registry.register(viewer, shard);
    }

    pub(crate) fn remove_viewer(&self, session: SessionId) {
        let mut registry = self.viewers.write().unwrap();
        self.spatial.remove_viewer(session);
        registry.remove(session);
    }

    /// Geometric recipients plus the row owner's session. The owner lookup lives in the viewer
    /// registry because it is session identity, not spatial state.
    fn world_entity_recipients(&self, guid: u64, key: CellKey) -> Vec<SessionId> {
        let mut recipients = self.spatial.viewers_of(EntityLayer::WorldEntity, key);
        if let Some(owner) = self.session_of_owner(guid) {
            if !recipients.contains(&owner) {
                recipients.push(owner);
            }
        }
        recipients
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

    // ---- owner-scoped events ---------------------------------------------------------------
    // These callbacks address one viewer directly and survive coordinator reconnects because
    // `arm_shard` is re-run on the replacement connection. Every table here holds SHARD rows,
    // so each owner lookup is scoped to this shard: a late delta from a transfer's source shard
    // (its cascade deletes may still sit in the pump) must never route to the viewer already
    // registered on the destination shard. Event-flavored legs (teleport, faction announce,
    // item toast) additionally drop the rows a fresh connection's initial apply replays —
    // see `wire_insert_live`.
    wire_insert_live(
        db.game_teleport_event(),
        "game_teleport_event.insert",
        &view,
        move |v, row| teleport_appeared(v, shard, row),
    );
    wire_insert_live(db.game_addon_message(), "game_addon_message.insert", &view, |v, row| {
        addon_message_appeared(v, row)
    });
    wire_insert_live(db.game_xp_event(), "game_xp_event.insert", &view, |v, row| {
        xp_appeared(v, row)
    });
    wire_insert_live(db.game_levelup_event(), "game_levelup_event.insert", &view, |v, row| {
        levelup_appeared(v, row)
    });
    let quest_insert_coord = coord.clone();
    wire_insert(
        db.game_character_quest(),
        "game_character_quest.insert",
        &view,
        move |v, row| quest_inserted(v, &quest_insert_coord, shard, row),
    );
    let quest_update_coord = coord.clone();
    wire_update(
        db.game_character_quest(),
        "game_character_quest.update",
        &view,
        move |v, old, row| quest_updated(v, &quest_update_coord, shard, old, row),
    );
    let quest_delete_coord = coord.clone();
    wire_delete(
        db.game_character_quest(),
        "game_character_quest.delete",
        &view,
        move |v, row| quest_deleted(v, &quest_delete_coord, shard, row),
    );
    {
        let coord = coord.clone();
        wire_insert(
            db.game_character_explored(),
            "game_character_explored.insert",
            &view,
            move |v, row| exploration_appeared(v, &coord, shard, row),
        );
    }
    wire_insert_live(
        db.game_player_reputation(),
        "game_player_reputation.insert",
        &view,
        move |v, row| reputation_appeared(v, shard, row),
    );
    wire_update(
        db.game_player_reputation(),
        "game_player_reputation.update",
        &view,
        move |v, _old, row| reputation_appeared(v, shard, row),
    );
    wire_insert(
        db.game_hunter_pet_protocol(),
        "game_hunter_pet_protocol.insert",
        &view,
        move |v, row| hunter_pet_appeared(v, shard, row),
    );
    wire_update(
        db.game_hunter_pet_protocol(),
        "game_hunter_pet_protocol.update",
        &view,
        move |v, old, row| {
            if old != row {
                hunter_pet_appeared(v, shard, row)
            }
        },
    );
    {
        let coord = coord.clone();
        wire_insert_live(
            db.game_item_instance(),
            "game_item_instance.insert",
            &view,
            move |v, row| item_inserted(v, &coord, shard, row),
        );
    }
    {
        let coord = coord.clone();
        wire_update(
            db.game_item_instance(),
            "game_item_instance.update",
            &view,
            move |v, old, row| item_updated(v, &coord, shard, old, row),
        );
    }
    {
        let coord = coord.clone();
        wire_delete(
            db.game_item_instance(),
            "game_item_instance.delete",
            &view,
            move |v, row| item_deleted(v, &coord, shard, row),
        );
    }

    // ---- game_world_entity -----------------------------------------------------------------
    wire_insert(db.game_world_entity(), "game_world_entity.insert", &view, move |v, row| {
        entity_appeared(v, shard, row)
    });
    {
        // The update leg carries the live zone as well as the position: the Module re-stamps
        // `zone_id` on the same grid-crossing that moves the row, so a viewer's own row is the
        // gateway's only zone signal (`CMSG_ZONEUPDATE` is deliberately unhandled).
        let coord = coord.clone();
        wire_update(db.game_world_entity(), "game_world_entity.update", &view, move |v, old, new| {
            entity_changed(v, shard, old, new);
            zone_crossed(v, &coord, shard, new.guid, old.zone_id, new.zone_id);
        });
    }
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

    // ---- game_taxi_passenger_spline (server-owned player routes) ---------------------------
    wire_insert(
        db.game_taxi_passenger_spline(),
        "game_taxi_passenger_spline.insert",
        &view,
        |v, row| taxi_spline(v, row),
    );
    wire_update(
        db.game_taxi_passenger_spline(),
        "game_taxi_passenger_spline.update",
        &view,
        |v, _old, row| taxi_spline(v, row),
    );

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

    // ---- game_breath_relay_event ------------------------------------------------------------
    // Breath start/stop edges + server-resolved drowning combat log. Self-only relay.
    wire_insert(db.game_breath_relay_event(), "game_breath_relay_event.insert", &view, |v, row| {
        breath_relay_appeared(v, row)
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

    // ---- recipient-keyed private event relays -----------------------------------------------
    // The PRIVATE tier: every row is addressed to exactly one recipient, and on this feed the
    // gateway owns the guarantee RLS used to give. Delivery is recipient-keyed (the owner-session
    // lookup) + the explicit `private_recipient_audience` predicate — never a viewer fan.
    // (The realm-core whisper/group twins for CROSS-shard delivery ride the same dispatchers,
    // armed once per realm-core connection by `arm_realm_private` below.)
    wire_insert_live(db.game_resurrect_request(), "game_resurrect_request.insert", &view, |v, row| {
        resurrect_offered(v, row)
    });
    wire_insert_live(db.game_whisper_event(), "game_whisper_event.insert", &view, |v, row| {
        whisper_appeared(v, row)
    });
    {
        let coord = coord.clone();
        wire_insert_live(db.game_group_event(), "game_group_event.insert", &view, move |v, row| {
            group_event_appeared(v, &coord, row)
        });
    }
    wire_insert_live(db.game_trade_event(), "game_trade_event.insert", &view, |v, row| {
        trade_event_appeared(v, row)
    });
    {
        let coord = coord.clone();
        wire_insert_live(db.game_duel_event(), "game_duel_event.insert", &view, move |v, row| {
            duel_event_appeared(v, &coord, row)
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

    // ---- game_zone_weather -----------------------------------------------------------------------
    // The live sky. ONE callback per shard, routed by the viewer's stored zone — there are no
    // per-character weather subscriptions. Insert and update run the same body: the first row a
    // zone ever gets is as much a visible change as every later one.
    wire_insert(db.game_zone_weather(), "game_zone_weather.insert", &view, move |v, row| {
        weather_changed(v, shard, row)
    });
    wire_update(db.game_zone_weather(), "game_zone_weather.update", &view, move |v, _old, row| {
        weather_changed(v, shard, row)
    });

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
    wire_insert_live(db.game_whisper_event(), "realm.game_whisper_event.insert", &view, |v, row| {
        whisper_appeared(v, row)
    });
    wire_insert_live(db.game_group_event(), "realm.game_group_event.insert", &view, move |v, row| {
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

/// True for a row callback fired by a subscription's initial apply rather than a live delta. A
/// coordinator (re)connect replays every resident row as an insert flavored `SubscribeApplied`,
/// racing the watchdog's re-arm — so an armed callback can see the whole table again.
fn is_initial_apply(event: &spacetimedb_sdk::Event<Reducer>) -> bool {
    matches!(event, spacetimedb_sdk::Event::SubscribeApplied)
}

/// Register an INSERT callback for an event-flavored family (toast/announce/teleport): rows
/// replayed by the initial apply are dropped, only live deltas run `f`. State-sync families keep
/// [`wire_insert`] — their replays re-send idempotent state.
fn wire_insert_live<T>(
    table: T,
    label: &'static str,
    view: &Arc<WorldView>,
    f: impl Fn(&Arc<WorldView>, &T::Row) + Send + 'static,
) where
    T: Table<EventContext = EventContext>,
{
    let view = view.clone();
    table.on_insert(move |ctx, row| {
        if is_initial_apply(&ctx.event) {
            return;
        }
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
    view.spatial
        .upsert_entity(EntityLayer::WorldEntity, row.guid, key, shard);
    let row = Arc::new(row.clone());
    for session in view.world_entity_recipients(row.guid, key) {
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
    let previous = view
        .spatial
        .upsert_entity(EntityLayer::WorldEntity, new.guid, key, shard);
    let new_recipients = view.world_entity_recipients(new.guid, key);
    if let Some(old_key) = previous {
        if old_key != key {
            let still: HashSet<SessionId> = new_recipients.iter().copied().collect();
            for session in view.spatial.viewers_of(EntityLayer::WorldEntity, old_key) {
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
        .spatial
        .remove_entity(EntityLayer::WorldEntity, row.guid)
        .unwrap_or_else(|| entity_key(row));
    for session in view.world_entity_recipients(row.guid, key) {
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
    let previous = view
        .spatial
        .upsert_entity(EntityLayer::GameObject, row.guid, key, shard);
    let recipients = view.spatial.viewers_of(EntityLayer::GameObject, key);
    if let Some(old_key) = previous {
        if old_key != key {
            let still: HashSet<SessionId> = recipients.iter().copied().collect();
            for session in view.spatial.viewers_of(EntityLayer::GameObject, old_key) {
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
        .spatial
        .remove_entity(EntityLayer::GameObject, row.guid)
        .unwrap_or_else(|| CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y));
    for session in view.spatial.viewers_of(EntityLayer::GameObject, key) {
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
    // The MOVER is deliberately not a recipient (`world_entity_recipients`' self leg is skipped
    // here): a
    // player must never receive their own movement echoed back, or they fight the server for
    // authority over their position. `relay_entity_motion` re-checks it anyway.
    for session in view.spatial.viewers_of(EntityLayer::WorldEntity, key) {
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
    for session in view.spatial.viewers_of(EntityLayer::WorldEntity, key) {
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
/// direct shard membership; family-specific gates (instance, range) run per viewer at the call site.
fn viewers_on_shard(view: &WorldView, shard: ShardId) -> Vec<Arc<Viewer>> {
    view.viewers_on_shard(shard)
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

/// A passenger spline is always sent to its owner. Nearby sessions receive it only after the
/// outbound visibility check confirms that the passenger already exists in their client world.
fn taxi_spline(view: &WorldView, row: &TaxiPassengerSpline) {
    let key = CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y);
    let mut sessions: std::collections::HashSet<_> =
        view.spatial.viewers_of(EntityLayer::WorldEntity, key).into_iter().collect();
    if let Some(owner) = view.session_of_owner(row.character_guid) {
        sessions.insert(owner);
    }
    let row = row.clone();
    for session in sessions {
        let Some(viewer) = view.viewer(session) else {
            continue;
        };
        let row = row.clone();
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::taxi_spline_outbound(
                &viewer.created,
                viewer.self_guid,
                &row,
            )
        });
    }
}

/// A teleport row names one character owner on this shard. Packet construction stays on that
/// viewer's writer.
fn teleport_appeared(view: &WorldView, shard: ShardId, row: &TeleportEvent) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(row.mover_guid)) else {
        return;
    };
    let row = row.clone();
    let tx = viewer.tx.clone();
    enqueue(&tx, move || {
        super::subscriptions::teleport_event_outbound(&row)
    });
}

/// Addon messages are identity-addressed, with the addressed viewer as the wire sender.
fn addon_message_appeared(view: &WorldView, row: &AddonMessage) {
    let Some(viewer) = view.viewer_of_identity(BoundIdentity(row.recipient_identity)) else {
        return;
    };
    let row = row.clone();
    let tx = viewer.tx.clone();
    enqueue(&tx, move || {
        super::subscriptions::addon_message_outbound(viewer.self_guid, &row)
    });
}

/// XP rows are delivered to the one viewer bound to their recipient identity.
fn xp_appeared(view: &WorldView, row: &XpEvent) {
    let Some(viewer) = view.viewer_of_identity(BoundIdentity(row.recipient_identity)) else {
        return;
    };
    let row = row.clone();
    let tx = viewer.tx.clone();
    enqueue(&tx, move || super::subscriptions::xp_event_outbound(&row));
}

/// Level-up rows are delivered to the one viewer bound to their recipient identity.
fn levelup_appeared(view: &WorldView, row: &LevelupEvent) {
    let Some(viewer) = view.viewer_of_identity(BoundIdentity(row.recipient_identity)) else {
        return;
    };
    let row = row.clone();
    let tx = viewer.tx.clone();
    enqueue(&tx, move || {
        super::subscriptions::levelup_event_outbound(&row)
    });
}

/// A quest-log insert names one owner. The full log is read after the delta on that owner's writer.
fn quest_inserted(view: &WorldView, coord: &Coordinator, shard: ShardId, row: &CharacterQuest) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(row.character_guid)) else {
        return;
    };
    let (coord, self_guid, tx) = (coord.clone(), viewer.self_guid, viewer.tx.clone());
    enqueue(&tx, move || {
        let guard = coord.0.coord();
        super::subscriptions::quest_log_outbound(&guard.conn.db, self_guid)
            .into_iter()
            .collect()
    });
}

/// A quest update keeps kill feedback, timer failure, then the full quest-log sync in one writer job.
fn quest_updated(
    view: &WorldView,
    coord: &Coordinator,
    shard: ShardId,
    old: &CharacterQuest,
    row: &CharacterQuest,
) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(row.character_guid)) else {
        return;
    };
    let (coord, self_guid, old, row, tx) = (
        coord.clone(),
        viewer.self_guid,
        old.clone(),
        row.clone(),
        viewer.tx.clone(),
    );
    enqueue(&tx, move || {
        let guard = coord.0.coord();
        super::subscriptions::quest_update_outbound(&guard.conn.db, self_guid, &old, &row)
    });
}

/// A quest deletion re-sends the full log, including the all-zero final-log clear.
fn quest_deleted(view: &WorldView, coord: &Coordinator, shard: ShardId, row: &CharacterQuest) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(row.character_guid)) else {
        return;
    };
    let (coord, self_guid, tx) = (coord.clone(), viewer.self_guid, viewer.tx.clone());
    enqueue(&tx, move || {
        let guard = coord.0.coord();
        super::subscriptions::quest_log_outbound(&guard.conn.db, self_guid)
            .into_iter()
            .collect()
    });
}

/// An explored-area row restores the complete fog word first, then reports a new discovery.
/// Cache reads and replay-state mutation stay on the addressed session's writer queue.
fn exploration_appeared(
    view: &WorldView,
    coord: &Coordinator,
    shard: ShardId,
    row: &CharacterExplored,
) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(row.character_guid)) else {
        return;
    };
    let (row, coord) = (row.clone(), coord.clone());
    let tx = viewer.tx.clone();
    enqueue(&tx, move || exploration_outbound(&coord, &viewer, &row));
}

fn exploration_outbound(
    coord: &Coordinator,
    viewer: &Viewer,
    row: &CharacterExplored,
) -> Vec<Outbound> {
    if row.area_bit < 0 {
        return Vec::new();
    }
    let word_idx = (row.area_bit / 32) as u16;
    let lo = word_idx as i32 * 32;
    let word = {
        let guard = coord.0.coord();
        guard
            .conn
            .db
            .game_character_explored()
            .iter()
            .filter(|candidate| {
                candidate.character_guid == viewer.self_guid
                    && candidate.area_bit >= lo
                    && candidate.area_bit < lo + 32
            })
            .fold(0u32, |word, candidate| {
                word | (1u32 << (candidate.area_bit - lo))
            })
    };
    exploration_outbound_for_word(viewer, row, word_idx, word)
}

fn exploration_outbound_for_word(
    viewer: &Viewer,
    row: &CharacterExplored,
    word_idx: u16,
    word: u32,
) -> Vec<Outbound> {
    let (opcode, body) =
        crate::codec::build_explored_zones_values(viewer.self_guid, word_idx, word);
    let mut outbound = vec![Outbound::Raw { opcode, body }];
    if viewer.explored.lock().unwrap().admit_once(row.area_bit) {
        let (opcode, body) =
            crate::codec::build_exploration_experience_raw(row.area_id, row.experience);
        outbound.push(Outbound::Raw { opcode, body });
    }
    outbound
}

/// A standing row is delivered only to its owner. The wire packet uses the Faction.dbc
/// ReputationListID, never the game's faction id.
fn reputation_appeared(view: &WorldView, shard: ShardId, row: &PlayerReputation) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(row.character_guid)) else {
        return;
    };
    let row = row.clone();
    let tx = viewer.tx.clone();
    enqueue(&tx, move || reputation_outbound(&row));
}

/// Hunter pet descriptors are idempotent VALUES, so replay re-sends are state-sync, not toasts.
fn hunter_pet_appeared(view: &WorldView, shard: ShardId, row: &HunterPetProtocol) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(row.owner_guid)) else {
        return;
    };
    let row = row.clone();
    let tx = viewer.tx.clone();
    enqueue(&tx, move || super::subscriptions::hunter_pet_outbound(&row));
}

fn reputation_outbound(row: &PlayerReputation) -> Vec<Outbound> {
    if row.reputation_index < 0 {
        return Vec::new();
    }
    crate::codec::build_set_faction_standing_raw(row.reputation_index as u32, row.standing)
        .map(|(opcode, body)| vec![Outbound::Raw { opcode, body }])
        .unwrap_or_default()
}

/// Select one item owner registered on the firing shard; cache reads and packet work run on its
/// writer queue.
fn item_owner_job(
    view: &WorldView,
    shard: ShardId,
    owner_guid: u64,
    job: impl FnOnce(Arc<Viewer>) -> Vec<Outbound> + Send + 'static,
) {
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(owner_guid)) else {
        return;
    };
    let tx = viewer.tx.clone();
    enqueue(&tx, move || job(viewer));
}

fn item_inserted(view: &WorldView, coord: &Coordinator, shard: ShardId, row: &ItemInstance) {
    let (coord, row) = (coord.clone(), row.clone());
    item_owner_job(view, shard, row.owner_guid, move |viewer| {
        let guard = coord.0.coord();
        super::subscriptions::item_instance_insert_outbound(&guard.conn.db, viewer.self_guid, &row)
    });
}

fn item_updated(
    view: &WorldView,
    coord: &Coordinator,
    shard: ShardId,
    old: &ItemInstance,
    row: &ItemInstance,
) {
    let (coord, old, row) = (coord.clone(), old.clone(), row.clone());
    item_owner_job(view, shard, row.owner_guid, move |viewer| {
        let guard = coord.0.coord();
        super::subscriptions::item_instance_update_outbound(
            &guard.conn.db,
            viewer.self_guid,
            &old,
            &row,
        )
    });
}

fn item_deleted(view: &WorldView, coord: &Coordinator, shard: ShardId, row: &ItemInstance) {
    let (coord, row) = (coord.clone(), row.clone());
    item_owner_job(view, shard, row.owner_guid, move |viewer| {
        let guard = coord.0.coord();
        super::subscriptions::item_instance_delete_outbound(&guard.conn.db, viewer.self_guid, &row)
    });
}

/// A rest-state flip landed on a shard's coordinator feed. Self-only family: the row names its
/// owner and the owner is the ONLY lawful recipient, so the audience predicate IS the
/// owner-session lookup — there is no candidate set to filter,
/// and a row whose owner has no live session here is dropped.
///
/// This family needs no replay state: nothing re-subscribes per session on this path, so rows
/// arriving after arming are live flips. A coordinator watchdog re-arm can replay rows still
/// inside their GC TTL, which idempotently re-sends the current rest byte.
fn rest_state_appeared(view: &WorldView, row: &RestStateEvent) {
    let Some(session) = view.session_of_owner(row.character_guid) else {
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

/// Breath relay events are delivered only to their owner.
fn breath_relay_appeared(view: &WorldView, row: &BreathRelayEvent) {
    let Some(session) = view.session_of_owner(row.character_guid) else {
        return;
    };
    let Some(viewer) = view.viewer(session) else {
        return;
    };
    let (guid, kind, remaining, duration, damage) = (
        row.character_guid,
        row.kind,
        row.time_remaining_ms,
        row.duration_ms,
        row.damage,
    );
    enqueue(&viewer.tx, move || {
        super::subscriptions::relay_breath_event(guid, kind, remaining, duration, damage)
    });
}

/// A skill row changed (line learned / skill-up). Self-only family, same owner-session-lookup
/// audience as [`rest_state_appeared`] — the slot allocation runs in the job on the owner's own
/// writer thread, against the viewer's `skill_slots` (the same map the per-player leg uses).
fn skill_changed(view: &WorldView, row: &PlayerSkill) {
    let Some(session) = view.session_of_owner(row.character_guid) else {
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
    let Some(session) = view.session_of_owner(row.target_guid) else {
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
    let Some(session) = view.session_of_owner(row.recipient_guid) else {
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

/// A trade status landed → the `SMSG_TRADE_STATUS` packet to the row's RECIPIENT and nobody
/// else (same shape as [`whisper_appeared`]) (#120).
fn trade_event_appeared(view: &WorldView, row: &TradeEvent) {
    let Some(session) = view.session_of_owner(row.recipient_guid) else {
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
        super::subscriptions::trade_event_outbound(&row)
    });
}

/// A Duel lifecycle edge landed for one participant. Private packets go only to that participant;
/// the initiator-addressed completion row also drives the one nearby winner announcement.
fn duel_event_appeared(view: &WorldView, coord: &Coordinator, row: &DuelEvent) {
    if let Some(viewer) = view
        .session_of_owner(row.recipient_guid)
        .and_then(|session| view.viewer(session))
        .filter(|viewer| {
            super::subscriptions::private_recipient_audience(row.recipient_guid, viewer.self_guid)
        })
    {
        let row = row.clone();
        let coord = coord.clone();
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            let template = (row.kind == lyracore_shared::duel::event_kind::REQUESTED)
                .then(|| coord.gameobject_template(row.flag_entry).ok().flatten())
                .flatten();
            super::subscriptions::duel_event_outbound(&row, template.as_ref())
        });
    }

    // The module writes two participant rows. Use the initiator-addressed copy as the one AOI fan
    // trigger, and exclude both private recipients so each client receives one winner packet.
    if row.kind != lyracore_shared::duel::event_kind::COMPLETE
        || row.recipient_guid != row.initiator_guid
        || row.winner_guid == 0
    {
        return;
    }
    let key = CellKey::of_position(row.map_id, row.instance_id, row.flag_x, row.flag_y);
    let row = Arc::new(row.clone());
    for session in view.spatial.viewers_of(EntityLayer::WorldEntity, key) {
        let Some(viewer) = view.viewer(session) else {
            continue;
        };
        if !duel_winner_audience(viewer.self_guid, row.initiator_guid, row.challenged_guid) {
            continue;
        }
        let row = row.clone();
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::duel_winner_outbound(&row)
        });
    }
}

/// A group/loot-roll/quest-share event landed → the kind-decoded packet to the row's RECIPIENT
/// and nobody else (same shape as [`whisper_appeared`]; `coord` feeds the QUEST_SHARE detail
/// JOIN inside the job).
fn group_event_appeared(view: &WorldView, coord: &Coordinator, row: &GroupEvent) {
    let Some(session) = view.session_of_owner(row.recipient_guid) else {
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

/// A zone's sky changed → `SMSG_WEATHER` to every viewer standing in that zone on this shard, and
/// to nobody else. The zone comparison IS the audience: weather is per zone, so an Elwynn row can
/// never reach a Westfall viewer however close the two stand.
///
/// Smooth, because this is a scheduled drift the client should fade in. The synchronizing sends
/// (world entry, zone entry) are Instant and come from [`zone_crossed`] and the login path.
///
/// The row carries the whole packet, so no cache read happens in the job — which also means a
/// later row cannot overtake an earlier one's encoding and show a client the wrong sky.
fn weather_changed(view: &WorldView, shard: ShardId, row: &ZoneWeather) {
    let zone_id = row.zone_id;
    let view_of_row = crate::codec::ZoneWeatherView {
        weather_type: row.weather_type,
        intensity: row.intensity,
    };
    for viewer in viewers_on_shard(view, shard) {
        if viewer.zone_id.load(Ordering::Relaxed) != zone_id {
            continue;
        }
        let tx = viewer.tx.clone();
        enqueue(&tx, move || {
            super::subscriptions::weather_outbound(view_of_row, WeatherChangeType::Smooth)
        });
    }
}

/// The `game_zone_weather` relay leg, reachable from the world-session tests: the encrypted-wire
/// test drives the SAME body [`arm_shard`] wires to the table callback, so a live weather row and a
/// test row take one path. Test-only because a shard's own callback is the sole production caller.
#[cfg(test)]
pub(crate) fn relay_zone_weather(view: &WorldView, shard: ShardId, row: &ZoneWeather) {
    weather_changed(view, shard, row);
}

/// The self entity's own row says it is standing in a different zone than the viewer's routing
/// state remembers → move the routing state and send the destination zone's current weather ONCE,
/// Instant, so the stale source-zone sky is replaced immediately (story 7).
///
/// This covers walking, a same-map teleport and a taxi landing — every path that changes the zone
/// without rebuilding the session. A cross-map path tears the viewer down and re-runs world entry,
/// which sends its own weather.
///
/// `store` is this shard's coordinator, taken through the [`crate::world::WeatherStore`] seam
/// because the destination's sky is READ, on the arriving viewer's own writer thread, rather than
/// carried by the row that triggered this.
///
/// The row-delta comparison comes FIRST because this sits on the pump, under the highest-rate
/// table in the module: every creature heartbeat and every player step that stays inside its zone
/// leaves here without touching the viewer registry. Creatures never carry a zone at all, so their
/// updates are all `0 -> 0`.
fn zone_crossed<St>(
    view: &WorldView,
    store: &St,
    shard: ShardId,
    guid: u64,
    old_zone: u32,
    zone_id: u32,
) where
    St: crate::world::WeatherStore + Clone + 'static,
{
    if zone_id == old_zone {
        return;
    }
    let Some(viewer) = view.viewer_of_owner_on_shard(shard, OwnerGuid(guid)) else {
        return;
    };
    if !viewer.enter_zone(zone_id) {
        return;
    }
    let (store, tx) = (store.clone(), viewer.tx.clone());
    enqueue(&tx, move || {
        super::subscriptions::zone_weather_outbound(&store, zone_id, WeatherChangeType::Instant)
    });
}

/// The AOI bystander leg of the winner announcement: both participants get their own copy, so the
/// fan-out must skip them (and the unregistered zero guid).
fn duel_winner_audience(viewer_guid: u64, initiator_guid: u64, challenged_guid: u64) -> bool {
    viewer_guid != 0 && viewer_guid != initiator_guid && viewer_guid != challenged_guid
}

#[cfg(test)]
mod family_audience_tests {
    use super::{
        addon_message_appeared, duel_winner_audience, exploration_outbound_for_word,
        is_initial_apply, item_owner_job, levelup_appeared, reputation_appeared, teleport_appeared,
        weather_changed, xp_appeared, zone_crossed, BoundIdentity, ExplorationReplay,
        MotionPending, OwnerGuid, Viewer, WorldView,
    };
    use crate::stdb::aoi::ViewerGates;
    use crate::stdb::bindings::{
        AddonMessage, CharacterExplored, CharacterQuest, LevelupEvent, PlayerReputation,
        TeleportEvent, XpEvent, ZoneWeather,
    };
    use crate::stdb::subscriptions::{private_recipient_audience, quest_update_packets};
    use crate::stdb::world_index::{CellKey, EntityLayer};
    use crate::world::{Outbound, SessionTx};
    use std::collections::{HashMap, HashSet};
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::{WeatherChangeType, WeatherType};

    fn identity(byte: u8) -> spacetimedb_sdk::Identity {
        spacetimedb_sdk::Identity::from_byte_array([byte; 32])
    }

    fn viewer(session: u64, self_guid: u64) -> Arc<Viewer> {
        let (tx, _rx) = SessionTx::with_depth(0);
        viewer_with_tx(session, self_guid, identity(session as u8), tx)
    }

    fn viewer_with_tx(
        session: u64,
        self_guid: u64,
        bound_identity: spacetimedb_sdk::Identity,
        tx: SessionTx,
    ) -> Arc<Viewer> {
        viewer_in_zone(session, self_guid, bound_identity, 0, tx)
    }

    /// A viewer already standing in `zone_id` — the weather relay's audience state.
    fn viewer_in_zone(
        session: u64,
        self_guid: u64,
        bound_identity: spacetimedb_sdk::Identity,
        zone_id: u32,
        tx: SessionTx,
    ) -> Arc<Viewer> {
        Arc::new(Viewer {
            session,
            self_guid,
            bound_identity,
            map_id: 0,
            instance_id: 0,
            zone_id: zone_id.into(),
            tx,
            created: Arc::new(Mutex::new(HashSet::new())),
            gates: Arc::new(ViewerGates::default()),
            skill_slots: Arc::new(Mutex::new((HashMap::new(), 0))),
            explored: Mutex::new(ExplorationReplay::default()),
            motion_pending: Arc::new(MotionPending::default()),
        })
    }

    fn queued_job(rx: &Receiver<Outbound>) -> Vec<Outbound> {
        match rx.try_recv().expect("one addressed writer job") {
            Outbound::Job(job) => job(),
            _ => panic!("shared owner dispatch must enqueue packet work as a writer job"),
        }
    }

    /// Collapse all whitespace so structural source assertions survive rustfmt.
    fn no_ws(source: &str) -> String {
        source.split_whitespace().collect()
    }

    fn explored(area_bit: i32, area_id: u32, experience: u32) -> CharacterExplored {
        CharacterExplored {
            id: 1,
            character_guid: 9001,
            area_bit,
            area_id,
            experience,
        }
    }

    fn reputation(character_guid: u64, reputation_index: i32, standing: i32) -> PlayerReputation {
        PlayerReputation {
            id: 1,
            character_guid,
            owner_identity: identity(1),
            faction_id: 72,
            standing,
            reputation_index,
            at_war: false,
        }
    }

    fn teleport(mover_guid: u64) -> TeleportEvent {
        TeleportEvent {
            id: 1,
            recipient_identity: identity(1),
            mover_guid,
            map_id: 36,
            x: 10.0,
            y: 20.0,
            z: 30.0,
            orientation: 1.5,
            created_micros: 0,
            cross_map: true,
        }
    }

    const ELWYNN: u32 = 12;
    const WESTFALL: u32 = 40;

    fn zone_weather(zone_id: u32, weather_type: u8, intensity: f32) -> ZoneWeather {
        ZoneWeather {
            zone_id,
            weather_type,
            intensity,
            changed_at_micros: 0,
        }
    }

    /// A `WeatherStore` holding one zone's sky. Stands in for a shard's `game_zone_weather` cache,
    /// which a unit test has no node to populate.
    #[derive(Clone)]
    struct CannedWeather(Option<(u32, u8, f32)>);

    impl crate::world::WeatherStore for CannedWeather {
        fn zone_weather(
            &self,
            zone_id: u32,
        ) -> anyhow::Result<Option<crate::codec::ZoneWeatherView>> {
            Ok(self.0.and_then(|(zone, weather_type, intensity)| {
                (zone == zone_id).then_some(crate::codec::ZoneWeatherView {
                    weather_type,
                    intensity,
                })
            }))
        }
    }

    fn only_weather(out: Vec<Outbound>) -> wow_world_messages::vanilla::SMSG_WEATHER {
        match out.as_slice() {
            [Outbound::One(ServerOpcodeMessage::SMSG_WEATHER(m))] => **m,
            other => panic!(
                "expected exactly one SMSG_WEATHER, got {} messages",
                other.len()
            ),
        }
    }

    /// Stories 5 and 6, the Elwynn/Westfall pair: a zone's weather reaches every viewer standing in
    /// THAT zone and no viewer anywhere else, however near they are — weather is per zone, and the
    /// spatial index has no say in it.
    #[test]
    fn a_weather_row_reaches_its_own_zone_and_no_other() {
        let view = WorldView::new(true);
        let (elwynn_tx, elwynn_rx) = SessionTx::with_depth(0);
        let (also_elwynn_tx, also_elwynn_rx) = SessionTx::with_depth(0);
        let (westfall_tx, westfall_rx) = SessionTx::with_depth(0);
        let (zoneless_tx, zoneless_rx) = SessionTx::with_depth(0);
        let anchor = CellKey::at(0, 0, 0, 0);
        for (session, guid, zone, tx) in [
            (1, 9001, ELWYNN, elwynn_tx),
            (2, 9002, ELWYNN, also_elwynn_tx),
            (3, 9003, WESTFALL, westfall_tx),
            (4, 9004, 0, zoneless_tx),
        ] {
            let v = viewer_in_zone(session, guid, identity(session as u8), zone, tx);
            view.add_viewer_on_shard(v, anchor, 0);
        }

        weather_changed(&view, 0, &zone_weather(ELWYNN, 1, 0.5));

        for rx in [&elwynn_rx, &also_elwynn_rx] {
            let m = only_weather(queued_job(rx));
            assert_eq!(m.weather_type, WeatherType::Rain);
            assert_eq!(m.grade, 0.5);
            assert_eq!(m.sound_id, 8534);
            assert_eq!(
                m.change,
                WeatherChangeType::Smooth,
                "a scheduled change fades in rather than snapping"
            );
        }
        assert!(
            westfall_rx.try_recv().is_err(),
            "Elwynn's weather must not reach a viewer standing in Westfall"
        );
        assert!(
            zoneless_rx.try_recv().is_err(),
            "the unresolved zone matches no weather row"
        );
    }

    /// Story 7: crossing into a new zone replaces the stale source-zone sky ONCE, instantly.
    /// Staying put is silence — the slow scheduler must not turn every grid crossing into a packet.
    #[test]
    fn crossing_into_a_zone_sends_its_weather_once() {
        let view = WorldView::new(true);
        let (walker_tx, walker_rx) = SessionTx::with_depth(0);
        let (resident_tx, resident_rx) = SessionTx::with_depth(0);
        let anchor = CellKey::at(0, 0, 0, 0);
        let walker = viewer_in_zone(1, 9001, identity(1), ELWYNN, walker_tx);
        let resident = viewer_in_zone(2, 9002, identity(2), WESTFALL, resident_tx);
        view.add_viewer_on_shard(walker.clone(), anchor, 0);
        view.add_viewer_on_shard(resident.clone(), anchor, 0);
        let store = CannedWeather(Some((WESTFALL, 3, 0.9)));

        // Elwynn → Westfall.
        zone_crossed(&view, &store, 0, walker.self_guid, ELWYNN, WESTFALL);
        let m = only_weather(queued_job(&walker_rx));
        assert_eq!(m.weather_type, WeatherType::Storm);
        assert_eq!(m.grade, 0.9);
        assert_eq!(m.sound_id, 8558);
        assert_eq!(m.change, WeatherChangeType::Instant);

        // Still walking around Westfall; a row the Module could not resolve a zone for; and a row
        // whose own delta claims a crossing the viewer has already been told about (a re-created
        // entity row starts from the unresolved zone).
        zone_crossed(&view, &store, 0, walker.self_guid, WESTFALL, WESTFALL);
        zone_crossed(&view, &store, 0, walker.self_guid, WESTFALL, 0);
        zone_crossed(&view, &store, 0, walker.self_guid, 0, WESTFALL);
        assert!(
            walker_rx.try_recv().is_err(),
            "staying inside a zone sends nothing, the unresolved zone is not a crossing, and a \
             zone the viewer is already in is not re-sent"
        );
        assert_eq!(
            walker.zone_id.load(std::sync::atomic::Ordering::Relaxed),
            WESTFALL,
            "an unresolved zone must not erase the zone the viewer is really in"
        );
        assert!(
            resident_rx.try_recv().is_err(),
            "another player's crossing is not this viewer's business"
        );

        // …and the arriving viewer now routes with the destination zone.
        weather_changed(&view, 0, &zone_weather(WESTFALL, 2, 0.3));
        assert_eq!(
            only_weather(queued_job(&walker_rx)).weather_type,
            WeatherType::Snow
        );
    }

    /// Story 10: a destination with no weather row is fine weather, not a missing packet — the
    /// client would otherwise keep rendering the sky of the zone it left.
    #[test]
    fn crossing_into_a_zone_with_no_weather_row_sends_fine_weather() {
        let view = WorldView::new(true);
        let (tx, rx) = SessionTx::with_depth(0);
        let walker = viewer_in_zone(1, 9001, identity(1), ELWYNN, tx);
        view.add_viewer_on_shard(walker.clone(), CellKey::at(0, 0, 0, 0), 0);

        zone_crossed(
            &view,
            &CannedWeather(None),
            0,
            walker.self_guid,
            ELWYNN,
            WESTFALL,
        );

        let m = only_weather(queued_job(&rx));
        assert_eq!(m.weather_type, WeatherType::Fine);
        assert_eq!(m.grade, 0.0);
        assert_eq!(m.sound_id, 0);
        assert_eq!(m.change, WeatherChangeType::Instant);
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

    #[test]
    fn duel_winner_aoi_fan_excludes_the_two_private_recipients() {
        assert!(!duel_winner_audience(10, 10, 20));
        assert!(!duel_winner_audience(20, 10, 20));
        assert!(duel_winner_audience(30, 10, 20));
        assert!(!duel_winner_audience(0, 10, 20));

        let source = include_str!("world_view.rs");
        let body = crate::test_scan::code_of(source, "fn duel_event_appeared(");
        assert!(body.contains("CellKey::of_position"));
        assert!(body.contains("view.spatial.viewers_of(EntityLayer::WorldEntity, key)"));
        assert!(body.contains("duel_winner_outbound"));
    }

    #[test]
    fn breath_relay_owner_lookup_selects_only_its_own_session() {
        let view = WorldView::new(true);
        // No owner session on this gateway: the relay must be dropped.
        assert_eq!(view.session_of_owner(41), None);
        // The registry tests pin the registration mechanics; the callback source scan below
        // pins that breath events actually take this exact lookup rather than an AOI broadcast.
        let source = include_str!("world_view.rs");
        let start = source.find("fn breath_relay_appeared").unwrap();
        let body = &source[start..];
        assert!(body.contains("view.session_of_owner(row.character_guid)"));
        assert!(!body[..body.find("fn skill_changed").unwrap()]
            .contains("viewers_on_shard"), "breath rows must never broadcast to shard viewers");
    }

    #[test]
    fn one_recenter_reports_both_layers_and_keeps_equal_guids_independent() {
        let view = WorldView::new(true);
        let viewer = viewer(1, 9001);
        view.add_viewer_on_shard(viewer.clone(), CellKey::at(0, 0, 0, 0), 0);

        // Moving the anchor one cell right adds x=3 to the 5x5 neighbourhood. The same GUID in
        // both row tables must appear once in each independent layer delta.
        let entering = CellKey::at(0, 0, 3, 0);
        view.spatial
            .upsert_entity(EntityLayer::WorldEntity, 77, entering, 0);
        view.spatial
            .upsert_entity(EntityLayer::GameObject, 77, entering, 1);
        let delta = view
            .spatial
            .move_viewer_delta(viewer.session, CellKey::at(0, 0, 1, 0))
            .expect("a cell crossing recenters");
        assert_eq!(delta.world_entities.entered, vec![(77, 0)]);
        assert_eq!(delta.game_objects.entered, vec![(77, 1)]);

        // Removing one table's row cannot erase the equal-valued GUID in the other table.
        assert_eq!(
            view.spatial.remove_entity(EntityLayer::WorldEntity, 77),
            Some(entering)
        );
        assert!(view
            .spatial
            .visible_entities(EntityLayer::WorldEntity, viewer.session)
            .is_empty());
        assert_eq!(
            view.spatial
                .visible_entities(EntityLayer::GameObject, viewer.session),
            vec![(77, 1)]
        );
    }

    #[test]
    fn removing_an_old_session_preserves_the_new_relogin_owner_mapping() {
        let view = WorldView::new(true);
        let old = viewer(1, 9001);
        let (new_tx, _new_rx) = SessionTx::with_depth(0);
        let new = viewer_with_tx(2, 9001, old.bound_identity, new_tx);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(old.clone(), anchor, 0);
        view.add_viewer_on_shard(new.clone(), anchor, 0);

        view.remove_viewer(old.session);
        assert_eq!(view.session_of_owner(9001), Some(new.session));
        assert!(view.viewer(new.session).is_some());
        view.remove_viewer(old.session); // idempotent
        assert_eq!(view.session_of_owner(9001), Some(new.session));
        assert_eq!(
            view.viewer_of_identity(BoundIdentity(new.bound_identity))
                .map(|viewer| viewer.session),
            Some(new.session)
        );
    }

    #[test]
    fn quest_owner_routing_follows_a_transfer_to_its_destination_viewer() {
        let view = WorldView::new(true);
        let source = viewer(1, 9001);
        let destination = viewer(2, 9001);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(source.clone(), anchor, 0);
        view.remove_viewer(source.session);
        view.add_viewer_on_shard(destination.clone(), anchor, 1);

        assert_eq!(
            view.viewer_of_owner(OwnerGuid(9001))
                .map(|viewer| viewer.session),
            Some(destination.session),
            "a later quest row must route only to the destination viewer"
        );
        assert!(
            view.viewer_of_owner_on_shard(0, OwnerGuid(9001)).is_none(),
            "a source-shard cascade delta applied after the transfer must find no viewer"
        );
        assert_eq!(
            view.viewer_of_owner_on_shard(1, OwnerGuid(9001))
                .map(|viewer| viewer.session),
            Some(destination.session)
        );
    }

    #[test]
    fn quest_update_orders_kills_then_timer_then_full_log() {
        let old = CharacterQuest {
            id: 1,
            character_guid: 9001,
            owner_identity: identity(1),
            quest_entry: 77,
            counts: vec![1],
            rewarded: false,
            deadline_micros: 0,
            failed: false,
        };
        let updated = CharacterQuest {
            counts: vec![2],
            failed: true,
            ..old.clone()
        };
        let outbound = quest_update_packets(
            &old,
            &updated,
            &[(0, 0, 123, 4)],
            Some(Outbound::Raw {
                opcode: 1,
                body: vec![0],
            }),
        );

        assert!(matches!(
            outbound.as_slice(),
            [
                Outbound::One(ServerOpcodeMessage::SMSG_QUESTUPDATE_ADD_KILL(_)),
                Outbound::One(ServerOpcodeMessage::SMSG_QUESTUPDATE_FAILEDTIMER(_)),
                Outbound::Raw { .. },
            ]
        ));
    }

    #[test]
    fn replacing_a_session_cleans_every_old_registry_index() {
        let view = WorldView::new(true);
        let old = viewer(7, 9001);
        let replacement = Arc::new(Viewer {
            session: old.session,
            self_guid: 9002,
            bound_identity: identity(99),
            map_id: 1,
            instance_id: 2,
            zone_id: 0.into(),
            tx: old.tx.clone(),
            created: old.created.clone(),
            gates: old.gates.clone(),
            skill_slots: old.skill_slots.clone(),
            explored: Mutex::new(ExplorationReplay::default()),
            motion_pending: old.motion_pending.clone(),
        });
        view.add_viewer_on_shard(old.clone(), CellKey::at(0, 0, 0, 0), 3);
        view.add_viewer_on_shard(replacement.clone(), CellKey::at(1, 2, 0, 0), 4);

        assert!(view.viewer_of_owner(OwnerGuid(old.self_guid)).is_none());
        assert!(view
            .viewer_of_identity(BoundIdentity(old.bound_identity))
            .is_none());
        assert!(view.viewers_on_shard(3).is_empty());
        assert_eq!(
            view.viewer_of_owner(OwnerGuid(replacement.self_guid))
                .map(|viewer| viewer.session),
            Some(replacement.session)
        );
        assert_eq!(
            view.viewer_of_identity(BoundIdentity(replacement.bound_identity))
                .map(|viewer| viewer.session),
            Some(replacement.session)
        );
        assert_eq!(view.viewers_on_shard(4).len(), 1);

        view.remove_viewer(replacement.session);
        assert!(view.viewer_of_owner(OwnerGuid(replacement.self_guid)).is_none());
        assert!(view
            .viewer_of_identity(BoundIdentity(replacement.bound_identity))
            .is_none());
        assert!(view.viewers_on_shard(4).is_empty());
    }

    #[test]
    fn teleport_routes_one_writer_job_to_its_owner_and_keeps_cross_map_order() {
        let view = WorldView::new(true);
        let (owner_tx, owner_rx) = SessionTx::with_depth(0);
        let (other_tx, other_rx) = SessionTx::with_depth(0);
        let owner = viewer_with_tx(1, 9001, identity(1), owner_tx);
        let other = viewer_with_tx(2, 9002, identity(2), other_tx);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(owner.clone(), anchor, 0);
        view.add_viewer_on_shard(other, anchor, 0);

        teleport_appeared(&view, 0, &teleport(owner.self_guid));

        assert!(other_rx.try_recv().is_err(), "an unrelated viewer receives nothing");
        let out = queued_job(&owner_rx);
        assert_eq!(out.len(), 1, "one teleport row creates one writer result");
        match &out[0] {
            Outbound::Batch(messages) => {
                assert!(matches!(
                    messages.as_slice(),
                    [ServerOpcodeMessage::SMSG_TRANSFER_PENDING(_), ServerOpcodeMessage::SMSG_NEW_WORLD(_)]
                ));
            }
            _ => panic!("cross-map teleport must remain one ordered writer batch"),
        }
        assert!(owner_rx.try_recv().is_err(), "the owner receives exactly one job");
    }

    #[test]
    fn item_rows_route_only_to_the_owner_and_follow_transfer_replacement() {
        let view = WorldView::new(true);
        let (source_tx, source_rx) = SessionTx::with_depth(0);
        let (other_tx, other_rx) = SessionTx::with_depth(0);
        let (destination_tx, destination_rx) = SessionTx::with_depth(0);
        let source = viewer_with_tx(1, 9001, identity(1), source_tx);
        let other = viewer_with_tx(2, 9002, identity(2), other_tx);
        let destination = viewer_with_tx(3, 9001, identity(1), destination_tx);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(source.clone(), anchor, 0);
        view.add_viewer_on_shard(other, anchor, 0);

        item_owner_job(&view, 0, source.self_guid, |_| {
            vec![Outbound::Raw { opcode: 1, body: vec![1] }]
        });
        assert!(matches!(queued_job(&source_rx).as_slice(), [Outbound::Raw { .. }]));
        assert!(other_rx.try_recv().is_err(), "an unrelated item owner receives nothing");

        view.remove_viewer(source.session);
        item_owner_job(&view, 0, source.self_guid, |_| {
            vec![Outbound::Raw { opcode: 2, body: vec![2] }]
        });
        assert!(source_rx.try_recv().is_err(), "a removed source viewer receives no cascade delete");

        view.add_viewer_on_shard(destination, anchor, 1);
        item_owner_job(&view, 0, source.self_guid, |_| {
            vec![Outbound::Raw { opcode: 2, body: vec![2] }]
        });
        assert!(
            destination_rx.try_recv().is_err(),
            "a source-shard delta applied after destination registration must be dropped"
        );
        item_owner_job(&view, 1, source.self_guid, |_| {
            vec![Outbound::Raw { opcode: 3, body: vec![3] }]
        });
        assert!(matches!(
            queued_job(&destination_rx).as_slice(),
            [Outbound::Raw { opcode: 3, .. }]
        ));
    }

    #[test]
    fn identity_events_route_once_in_fifo_order_and_addon_uses_the_viewer_guid() {
        let view = WorldView::new(true);
        let (owner_tx, owner_rx) = SessionTx::with_depth(0);
        let (other_tx, other_rx) = SessionTx::with_depth(0);
        let owner = viewer_with_tx(1, 0x1122_3344_5566_7788, identity(7), owner_tx);
        let other = viewer_with_tx(2, 9002, identity(8), other_tx);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(owner.clone(), anchor, 0);
        view.add_viewer_on_shard(other, anchor, 0);

        xp_appeared(
            &view,
            &XpEvent {
                id: 1,
                recipient_identity: owner.bound_identity,
                killed_guid: 77,
                total_exp: 123,
                created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
                is_kill: true,
            },
        );
        levelup_appeared(
            &view,
            &LevelupEvent {
                id: 2,
                recipient_identity: owner.bound_identity,
                new_level: 10,
                health_gained: 20,
                created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
                mana_gained: 15,
                strength_gained: 1,
                agility_gained: 2,
                stamina_gained: 3,
                intellect_gained: 4,
                spirit_gained: 5,
            },
        );
        addon_message_appeared(
            &view,
            &AddonMessage {
                id: 3,
                recipient_identity: owner.bound_identity,
                cmd: "progress".to_string(),
                payload: "ready".to_string(),
                created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            },
        );

        assert!(other_rx.try_recv().is_err(), "an unrelated identity receives nothing");
        assert!(matches!(
            queued_job(&owner_rx).as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_LOG_XPGAIN(_))]
        ));
        assert!(matches!(
            queued_job(&owner_rx).as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_LEVELUP_INFO(_))]
        ));
        match queued_job(&owner_rx).as_slice() {
            [Outbound::Raw { body, .. }] => assert_eq!(
                &body[5..13],
                &owner.self_guid.to_le_bytes(),
                "the addon sender is the addressed viewer"
            ),
            _ => panic!("addon event must produce one raw SMSG_MESSAGECHAT"),
        }
        assert!(owner_rx.try_recv().is_err(), "each identity row queues exactly once");
    }

    #[test]
    fn exploration_replays_fog_before_discovery_and_reconnects_stay_quiet() {
        let viewer = viewer(1, 9001);
        viewer.explored.lock().unwrap().seed([40]);

        let resident = explored(40, 87, 120);
        let resident_out = exploration_outbound_for_word(&viewer, &resident, 1, 1 << 8);
        let (fog_opcode, fog_body) = crate::codec::build_explored_zones_values(9001, 1, 1 << 8);
        match resident_out.as_slice() {
            [Outbound::Raw { opcode, body }] => {
                assert_eq!((*opcode, body), (fog_opcode, &fog_body));
            }
            _ => panic!("a seeded area restores fog without discovery feedback"),
        }

        let fresh = explored(41, 88, 121);
        let fresh_out = exploration_outbound_for_word(&viewer, &fresh, 1, (1 << 8) | (1 << 9));
        let (new_fog_opcode, new_fog_body) =
            crate::codec::build_explored_zones_values(9001, 1, (1 << 8) | (1 << 9));
        let (discovery_opcode, discovery_body) =
            crate::codec::build_exploration_experience_raw(88, 121);
        match fresh_out.as_slice() {
            [
                Outbound::Raw { opcode: fog_opcode, body: fog_body },
                Outbound::Raw { opcode: toast_opcode, body: toast_body },
            ] => {
                assert_eq!((*fog_opcode, fog_body), (new_fog_opcode, &new_fog_body));
                assert_eq!((*toast_opcode, toast_body), (discovery_opcode, &discovery_body));
            }
            _ => panic!("fog VALUES must precede discovery feedback for a new area"),
        }
    }

    #[test]
    fn reputation_insert_and_update_route_only_to_the_owner_by_reputation_index() {
        let view = WorldView::new(true);
        let (owner_tx, owner_rx) = SessionTx::with_depth(0);
        let (other_tx, other_rx) = SessionTx::with_depth(0);
        let owner = viewer_with_tx(1, 9001, identity(1), owner_tx);
        let other = viewer_with_tx(2, 9002, identity(2), other_tx);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(owner, anchor, 0);
        view.add_viewer_on_shard(other, anchor, 0);

        reputation_appeared(&view, 0, &reputation(9001, 19, 3175));
        reputation_appeared(&view, 0, &reputation(9001, 19, 3200));

        assert!(other_rx.try_recv().is_err(), "an unrelated owner receives nothing");
        for standing in [3175, 3200] {
            let (opcode, body) = crate::codec::build_set_faction_standing_raw(19, standing)
                .expect("a valid reputation index has a packet");
            match queued_job(&owner_rx).as_slice() {
                [Outbound::Raw { opcode: actual_opcode, body: actual_body }] => {
                    assert_eq!((*actual_opcode, actual_body), (opcode, &body));
                }
                _ => panic!("the reputation row must use its reputation_index, not faction_id"),
            }
        }
        assert!(owner_rx.try_recv().is_err(), "each reputation row queues exactly one job");
    }

    #[test]
    fn migrated_owner_callbacks_are_armed_once_and_not_registered_per_session() {
        let source = include_str!("world_view.rs");
        let start = source.find("pub(crate) fn arm_shard").unwrap();
        let end = source[start..]
            .find("pub(crate) fn arm_realm_private")
            .map(|offset| start + offset)
            .unwrap();
        let arm = &source[start..end];
        for table in [
            "game_teleport_event",
            "game_addon_message",
            "game_xp_event",
            "game_levelup_event",
            "game_character_explored",
            "game_player_reputation",
            "game_item_instance",
        ] {
            assert_eq!(
                arm.matches(&format!("db.{table}()")).count(),
                match table {
                    "game_player_reputation" => 2,
                    "game_item_instance" => 3,
                    _ => 1,
                },
                "{table} must have the expected shared callbacks per arm_shard call"
            );
        }
        let arm_flat = no_ws(arm);
        for (helper, label) in [
            ("wire_insert", "game_character_quest.insert"),
            ("wire_update", "game_character_quest.update"),
            ("wire_delete", "game_character_quest.delete"),
        ] {
            assert_eq!(
                arm_flat
                    .matches(&format!("{helper}(db.game_character_quest()"))
                    .count(),
                1,
                "game_character_quest must have one {helper} callback per arm_shard call"
            );
            assert!(arm.contains(&format!("\"{label}\"")));
        }
        assert_eq!(arm.matches("\"game_item_instance.update\"").count(), 1);
        assert_eq!(arm.matches("\"game_item_instance.delete\"").count(), 1);

        let subscriptions = include_str!("subscriptions.rs");
        let subscribe = subscriptions
            .split("pub fn subscribe_player_events(")
            .nth(1)
            .and_then(|body| body.split("pub fn spawn_bot_invite_relay").next())
            .expect("subscribe_player_events body");
        for callback in [".on_insert(", ".on_update(", ".on_delete("] {
            assert!(
                !subscribe.contains(callback),
                "subscribe_player_events must only register a viewer, not row callback {callback}"
            );
        }
        let guard = subscriptions
            .split("pub struct PlayerSubscriptions")
            .nth(1)
            .and_then(|body| body.split("impl PlayerSubscriptions").next())
            .expect("PlayerSubscriptions fields");
        assert!(
            !guard.contains("teardown") && !guard.contains("FnOnce"),
            "PlayerSubscriptions must own viewer lifetime only"
        );

        let exploration = source
            .split("fn exploration_appeared")
            .nth(1)
            .and_then(|body| body.split("fn exploration_outbound").next())
            .map(no_ws)
            .expect("the exploration owner relay");
        assert!(
            exploration.contains("viewer_of_owner_on_shard(shard,OwnerGuid(row.character_guid))")
                && exploration.contains("enqueue(&tx,move||exploration_outbound"),
            "exploration must select its owner then defer all work to that viewer's writer"
        );
        assert!(
            subscriptions.find("explored.seed(").unwrap()
                < subscriptions.find("view.add_viewer(").unwrap(),
            "resident exploration state must be seeded before viewer registration"
        );

        assert_eq!(
            include_str!("connection.rs")
                .matches("super::world_view::arm_shard(")
                .count(),
            2,
            "arm_shard must run once at connect and once from the reconnect hook"
        );

        for handler in ["quest_inserted", "quest_updated", "quest_deleted"] {
            let start = source.find(&format!("fn {handler}")).unwrap();
            let body = no_ws(&source[start..]);
            assert!(
                body.contains("view.viewer_of_owner_on_shard(shard,OwnerGuid(row.character_guid))")
                    && body.contains("enqueue(&tx,move||")
                    && body.contains("coord.0.coord()"),
                "{handler} must select one shard-scoped owner and defer cache reads and packet construction to its writer job"
            );
        }
        let update = no_ws(&source[source.find("fn quest_updated").unwrap()..]);
        assert!(
            update.contains("quest_update_outbound(&guard.conn.db,self_guid,&old,&row)"),
            "quest update must preserve its feedback and full-log ordering in one writer job"
        );
    }

    /// The reconnect contract, behaviorally: a re-arm builds a fresh callback generation over the
    /// same shared view (each generation is a `view.clone()` capture, exactly what `wire_insert`
    /// takes), and the viewer registered before the reconnect still receives owner relays through
    /// the new generation — while the fresh connection's initial-apply replays stay suppressed.
    #[test]
    fn a_rearmed_owner_relay_still_reaches_the_logged_in_viewer() {
        let view = std::sync::Arc::new(WorldView::new(true));
        let (owner_tx, owner_rx) = SessionTx::with_depth(0);
        let owner = viewer_with_tx(1, 9001, identity(1), owner_tx);
        view.add_viewer_on_shard(owner.clone(), CellKey::at(0, 0, 0, 0), 0);

        let arm = |view: &std::sync::Arc<WorldView>| {
            let view = view.clone();
            move |row: &TeleportEvent| teleport_appeared(&view, 0, row)
        };
        let first_generation = arm(&view);
        first_generation(&teleport(owner.self_guid));
        assert_eq!(queued_job(&owner_rx).len(), 1);

        // The watchdog's re-arm: a new generation on the replacement connection; the old one
        // dies with its `LiveConn`. The viewer never re-registers.
        let rearmed = arm(&view);
        drop(first_generation);
        rearmed(&teleport(owner.self_guid));
        assert_eq!(
            queued_job(&owner_rx).len(),
            1,
            "a re-armed owner relay must still address the logged-in viewer"
        );
        assert!(
            owner_rx.try_recv().is_err(),
            "each live row queues exactly once"
        );

        // The replay flavor the re-armed generation races: initial-apply rows are history, not
        // live deltas, and the event-family registrations drop them before dispatch.
        assert!(is_initial_apply(&spacetimedb_sdk::Event::SubscribeApplied));
        assert!(!is_initial_apply(&spacetimedb_sdk::Event::Transaction));
    }

    #[test]
    fn shard_membership_is_direct_and_does_not_wait_for_the_self_entity_row() {
        let view = WorldView::new(true);
        let first = viewer(1, 9001);
        let second = viewer(2, 9002);
        view.add_viewer_on_shard(first.clone(), CellKey::at(0, 0, 0, 0), 0);
        view.add_viewer_on_shard(second.clone(), CellKey::at(1, 0, 0, 0), 1);

        assert_eq!(view.spatial.shard_of(EntityLayer::WorldEntity, 9001), None);
        assert_eq!(
            view.viewers_on_shard(0)
                .into_iter()
                .map(|viewer| viewer.session)
                .collect::<HashSet<_>>(),
            HashSet::from([first.session])
        );
        assert_eq!(
            view.viewers_on_shard(1)
                .into_iter()
                .map(|viewer| viewer.session)
                .collect::<HashSet<_>>(),
            HashSet::from([second.session])
        );
    }

    #[test]
    fn a_single_shard_selects_every_registered_viewer() {
        let view = WorldView::new(true);
        let first = viewer(1, 9001);
        let second = viewer(2, 9002);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(first.clone(), anchor, 0);
        view.add_viewer_on_shard(second.clone(), anchor, 0);

        assert_eq!(
            view.viewers_on_shard(0)
                .into_iter()
                .map(|viewer| viewer.session)
                .collect::<HashSet<_>>(),
            HashSet::from([first.session, second.session])
        );
    }

    #[test]
    fn removal_immediately_cleans_shard_owner_and_spatial_membership() {
        let view = WorldView::new(true);
        let viewer = viewer(7, 4242);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(viewer.clone(), anchor, 3);

        view.remove_viewer(viewer.session);

        assert!(view.viewers_on_shard(3).is_empty());
        assert_eq!(view.session_of_owner(viewer.self_guid), None);
        assert_eq!(view.spatial.viewer_cell(viewer.session), None);
        view.remove_viewer(viewer.session);
    }

    #[test]
    fn a_player_always_receives_their_own_row_before_the_anchor_catches_up() {
        let view = WorldView::new(true);
        let viewer = viewer(7, 4242);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(viewer.clone(), anchor, 0);
        let far = CellKey::at(0, 0, 99, 99);

        assert!(view
            .spatial
            .viewers_of(EntityLayer::WorldEntity, far)
            .is_empty());
        assert_eq!(
            view.world_entity_recipients(viewer.self_guid, far),
            vec![viewer.session]
        );
        assert_eq!(
            view.world_entity_recipients(viewer.self_guid, anchor),
            vec![viewer.session]
        );
        view.remove_viewer(viewer.session);
        assert!(view
            .world_entity_recipients(viewer.self_guid, far)
            .is_empty());
    }

    #[test]
    fn recenter_and_sweep_enqueue_world_entities_before_game_objects() {
        let source = include_str!("world_view.rs");
        let recenter_start = source.rfind("pub(crate) fn recenter(view:").unwrap();
        let sweep_start = source.rfind("pub(crate) fn sweep_into_view(view:").unwrap();
        let recenter = &source[recenter_start..sweep_start];
        assert!(
            recenter.find("delta.world_entities").unwrap()
                < recenter.find("delta.game_objects").unwrap(),
            "a recenter must enqueue world-entity visibility work before game-object work"
        );

        let seed_start = source.rfind("pub(crate) fn seed_from_caches(view:").unwrap();
        let sweep = &source[sweep_start..seed_start];
        assert!(
            sweep.find("EntityLayer::WorldEntity").unwrap()
                < sweep.find("EntityLayer::GameObject").unwrap(),
            "the initial sweep must enqueue world-entity visibility work before game-object work"
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
    let Some(delta) = view.spatial.move_viewer_delta(viewer.session, key) else {
        return; // no crossing: the by-far-most-common per-heartbeat call, and it allocates nothing
    };
    AOI_RECENTERS.fetch_add(1, Ordering::Relaxed);

    // Everything below is enqueued as a JOB rather than sent directly, for ORDERING: the shard
    // pumps are concurrently enqueueing CREATE/VALUES jobs for the same session, and putting both
    // producers on the one queue is what makes "the DESTROY for a peer I walked away from lands
    // after the CREATE that showed it to me" true rather than a race between two threads.
    {
        for (guid, shard) in delta.world_entities.left {
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
        for (guid, shard) in delta.world_entities.entered {
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
    {
        for (guid, _) in delta.game_objects.left {
            let viewer = viewer.clone();
            enqueue(&viewer.tx.clone(), move || {
                super::subscriptions::relay_gameobject_destroy(&viewer, guid)
            });
        }
        for (guid, shard) in delta.game_objects.entered {
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
    for (guid, shard) in view
        .spatial
        .visible_entities(EntityLayer::WorldEntity, viewer.session)
    {
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
    for (guid, shard) in view
        .spatial
        .visible_entities(EntityLayer::GameObject, viewer.session)
    {
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

    // The login sequence queued the owner's self CREATE before registering this viewer. Chain the
    // resident flight now so reconnect observes CREATE before MONSTER_MOVE on the same writer.
    for coord in view.shards.read().unwrap().iter() {
        for outbound in super::subscriptions::resident_taxi_spline_outbound(
            coord,
            viewer,
            viewer.self_guid,
        ) {
            let _ = viewer.tx.send(outbound);
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
            view.spatial
                .upsert_entity(EntityLayer::WorldEntity, row.guid, entity_key(&row), id);
        }
        for row in guard.conn.db.game_gameobject().iter() {
            view.spatial.upsert_entity(
                EntityLayer::GameObject,
                row.guid,
                CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y),
                id,
            );
        }
    }
    let (entities, _, cells) = view.spatial.stats(EntityLayer::WorldEntity);
    let (gos, _, go_cells) = view.spatial.stats(EntityLayer::GameObject);
    log::info!(
        "shared AOI index seeded from {} shard cache(s): {entities} entities in {cells} cells, \
         {gos} gameobjects in {go_cells} cells",
        view.shard_count()
    );
}
