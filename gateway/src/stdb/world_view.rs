//! Shared viewer routing, AOI indexes and Relay queue lifetime.
//!
//! Protocol Families declare their table callbacks beside their codecs. Callbacks update indexes
//! and enqueue owned rows; the World Session writer builds packets. Arming registers callbacks
//! before reconciling the cache on the same Shard pump.

#[cfg(test)]
use crate::codec::combat::subscriptions::{
    aura_audience, cast_audience, combat_audience, impact_audience, melee_audience,
};
#[cfg(test)]
use crate::codec::item::subscriptions::ItemSlotRead;
#[cfg(test)]
use crate::codec::social::subscriptions::{chat_audience, emote_audience};

#[cfg(test)]
use crate::codec::addon::subscriptions::addon_message_appeared;
#[cfg(test)]
use crate::codec::breath::subscriptions::breath_relay_appeared;
#[cfg(test)]
use crate::codec::channel::subscriptions::channel_notice_appeared;
#[cfg(test)]
use crate::codec::combat::subscriptions::cast_event_appeared;
#[cfg(test)]
use crate::codec::combat::subscriptions::combat_event_appeared;
#[cfg(test)]
use crate::codec::combat::subscriptions::impact_appeared;
#[cfg(test)]
use crate::codec::combat::subscriptions::melee_disengaged;
#[cfg(test)]
use crate::codec::combat::subscriptions::melee_engaged;
#[cfg(test)]
use crate::codec::corpse::subscriptions::resurrect_offered;
#[cfg(test)]
use crate::codec::corpse::subscriptions::self_res_option_changed;
#[cfg(test)]
use crate::codec::duel::subscriptions::duel_winner_audience;
use crate::codec::entity::subscriptions::entity_key;
#[cfg(test)]
use crate::codec::entity::subscriptions::entity_vanished;
use crate::codec::entity::subscriptions::offer_create_job;
#[cfg(test)]
use crate::codec::entity::subscriptions::reputation_appeared;
use crate::codec::gameobject::subscriptions::gameobject_appeared;
#[cfg(test)]
use crate::codec::gameobject::subscriptions::gameobject_vanished;
#[cfg(test)]
use crate::codec::guild::subscriptions::charter_petition_opened;
#[cfg(test)]
use crate::codec::guild::subscriptions::guild_event_appeared;
#[cfg(test)]
use crate::codec::guild::subscriptions::guild_membership_changed;
#[cfg(test)]
use crate::codec::guild::subscriptions::petition_event_appeared;
#[cfg(test)]
use crate::codec::item::subscriptions::item_owner_job;
#[cfg(test)]
use crate::codec::item::subscriptions::item_visible_changed;
#[cfg(test)]
use crate::codec::item::subscriptions::trade_event_appeared;
#[cfg(test)]
use crate::codec::leveling::subscriptions::exploration_outbound_for_word;
#[cfg(test)]
use crate::codec::leveling::subscriptions::levelup_appeared;
#[cfg(test)]
use crate::codec::leveling::subscriptions::rest_state_appeared;
#[cfg(test)]
use crate::codec::leveling::subscriptions::xp_appeared;
#[cfg(test)]
use crate::codec::mail::subscriptions::mail_arrived;
#[cfg(test)]
use crate::codec::movement::subscriptions::creature_leg;
#[cfg(test)]
use crate::codec::movement::subscriptions::motion;
#[cfg(test)]
use crate::codec::movement::subscriptions::teleport_appeared;
#[cfg(test)]
use crate::codec::social::subscriptions::character_online_changed;
#[cfg(test)]
use crate::codec::social::subscriptions::claim_closed;
#[cfg(test)]
use crate::codec::social::subscriptions::contact_changed;
#[cfg(test)]
use crate::codec::social::subscriptions::friend_notice_candidates;
#[cfg(test)]
use crate::codec::social::subscriptions::instance_removal_ended;
#[cfg(test)]
use crate::codec::social::subscriptions::instance_removal_started;
#[cfg(test)]
use crate::codec::social::subscriptions::instance_removal_time_left_ms;
#[cfg(test)]
use crate::codec::social::subscriptions::realm_chat_appeared;
use crate::codec::social::subscriptions::replace_contact_sets;
#[cfg(test)]
use crate::codec::social::subscriptions::resident_countdown_ms;
use crate::codec::social::subscriptions::resident_instance_removal_outbound;
#[cfg(test)]
use crate::codec::social::subscriptions::system_message_appeared;
#[cfg(test)]
use crate::codec::taxi::subscriptions::taxi_spline;
#[cfg(test)]
use crate::codec::trainer::subscriptions::skill_changed;
#[cfg(test)]
use crate::codec::weather::subscriptions::weather_changed;
#[cfg(test)]
use crate::codec::weather::subscriptions::zone_crossed;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
#[cfg(test)]
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;

use anyhow::{anyhow, Result};
use lyracore_shared::spatial::{BOX_HALF_SPAN, GRID_CELL_SIZE};
use spacetimedb_sdk::Table;

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
    /// Cleared when this World Session leaves. Queued Relays must not reach its replacement.
    pub(crate) active: std::sync::atomic::AtomicBool,
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
    /// WorldEntity GUIDs already shown to this viewer.
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
    /// What the Member Stats Relay last sent about each group mate. A new viewer starts empty, so
    /// world entry and a group join both get every field on the next tick.
    pub(crate) member_stats: crate::world::MemberStatsRecord,
    /// The Characters on this viewer's own ignore list. Seeded at world entry from its contact
    /// rows on the Home Shard, kept current by that Shard's contact Relay, and rebuilt when the
    /// Shard reconciles after a resubscribe. The Realm Chat Relay reads it for ignorable lines only.
    pub(crate) ignored: Mutex<HashSet<u64>>,
    /// The Characters on this viewer's own friend list. Seeded and kept current exactly like
    /// `ignored`, from the same contact rows and callbacks. The Account Claim Relay reads it to
    /// decide who hears a login/logout notice; `CMSG_FRIEND_LIST` reads it for the friend guids.
    pub(crate) friends: Mutex<HashSet<u64>>,
    /// This viewer's own team (`lyracore_shared::faction::TEAM_ALLIANCE`/`TEAM_HORDE`), read once
    /// at world entry from its race. A friend online/offline notice goes only to a same-team
    /// viewer, the faction separation the friends pane has in vanilla.
    pub(crate) team: u32,
    /// The group events already relayed to this viewer, so one transaction's rows go out once
    /// and in order.
    pub(crate) group_events: GroupEventCursor,
}

impl Viewer {
    /// Move this viewer's weather routing into `zone_id`, answering whether that was a real
    /// crossing — the ONE place [`Viewer::zone_id`] is written after construction.
    ///
    /// The unresolved zone (`0`) is not a crossing and never overwrites a known zone: it means the
    /// Module's terrain could not answer, not that the player left every zone, and treating it as a
    /// destination would send fine weather over a real storm.
    pub(crate) fn enter_zone(&self, zone_id: u32) -> bool {
        zone_id != 0 && self.zone_id.swap(zone_id, Ordering::Relaxed) != zone_id
    }

    /// Whether this viewer ignores `guid`.
    pub(crate) fn ignores(&self, guid: u64) -> bool {
        self.ignored
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&guid)
    }

    pub(crate) fn set_ignored(&self, guid: u64, ignored: bool) {
        let mut set = self.ignored.lock().unwrap_or_else(|p| p.into_inner());
        if ignored {
            set.insert(guid);
        } else {
            set.remove(&guid);
        }
    }

    /// Whether this viewer lists `guid` as a friend.
    pub(crate) fn is_friend(&self, guid: u64) -> bool {
        self.friends
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&guid)
    }

    pub(crate) fn set_friend(&self, guid: u64, present: bool) {
        let mut set = self.friends.lock().unwrap_or_else(|p| p.into_inner());
        if present {
            set.insert(guid);
        } else {
            set.remove(&guid);
        }
    }

    /// A snapshot of this viewer's ignore guids, for `CMSG_FRIEND_LIST`'s `SMSG_IGNORE_LIST`.
    pub(crate) fn ignored_guids(&self) -> Vec<u64> {
        self.ignored
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .copied()
            .collect()
    }

    /// A snapshot of this viewer's friend guids, for `CMSG_FRIEND_LIST`'s `SMSG_FRIEND_LIST`.
    pub(crate) fn friend_guids(&self) -> Vec<u64> {
        self.friends
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .copied()
            .collect()
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

    pub(crate) fn admit_once(&mut self, area_bit: i32) -> bool {
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
            previous.viewer.active.store(false, Ordering::Release);
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

    pub(crate) fn remove(&mut self, session: SessionId) {
        let Some(registered) = self.by_session.remove(&session) else {
            return;
        };
        registered.viewer.active.store(false, Ordering::Release);
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

/// Auras indexed by shard and target. Transfer copies on different shards remain independent.
#[derive(Default)]
pub(crate) struct AuraIndex {
    by_target: Mutex<HashMap<(ShardId, u64), Vec<Aura>>>,
}

impl AuraIndex {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(ShardId, u64), Vec<Aura>>> {
        self.by_target.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn upsert(&self, shard: ShardId, row: &Aura) {
        let mut by_target = self.lock();
        let rows = by_target.entry((shard, row.target_guid)).or_default();
        match rows.iter_mut().find(|a| a.id == row.id) {
            Some(slot) => *slot = row.clone(),
            None => rows.push(row.clone()),
        }
    }

    pub(crate) fn remove(&self, shard: ShardId, row: &Aura) {
        let mut by_target = self.lock();
        let key = (shard, row.target_guid);
        if let Some(rows) = by_target.get_mut(&key) {
            rows.retain(|a| a.id != row.id);
            if rows.is_empty() {
                by_target.remove(&key);
            }
        }
    }

    /// Replace the Shard snapshot and return previous auras grouped by every old or current target.
    /// A newly present target has an empty previous set. Each target needs one snapshot Relay.
    pub(crate) fn replace_shard(
        &self,
        shard: ShardId,
        rows: impl IntoIterator<Item = Aura>,
    ) -> HashMap<u64, Vec<Aura>> {
        let mut by_target = self.lock();
        let targets: Vec<_> = by_target
            .keys()
            .filter(|(owner, _)| *owner == shard)
            .copied()
            .collect();
        let mut old: HashMap<_, _> = targets
            .into_iter()
            .map(|target| (target.1, by_target.remove(&target).unwrap_or_default()))
            .collect();
        for row in rows {
            old.entry(row.target_guid).or_default();
            by_target
                .entry((shard, row.target_guid))
                .or_default()
                .push(row);
        }
        old
    }

    pub(crate) fn on_target(&self, shard: ShardId, target_guid: u64) -> Vec<Aura> {
        self.lock()
            .get(&(shard, target_guid))
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn stealth_count(&self, shard: ShardId, target_guid: u64) -> usize {
        self.lock()
            .get(&(shard, target_guid))
            .map(|rows| {
                rows.iter()
                    .filter(|a| a.eff_kind == crate::codec::combat::subscriptions::A_STEALTH)
                    .count()
            })
            .unwrap_or(0)
    }

    pub(crate) fn is_stealthed(&self, shard: ShardId, target_guid: u64) -> bool {
        self.stealth_count(shard, target_guid) > 0
    }

    pub(crate) fn stats(&self) -> (usize, usize) {
        let by_target = self.lock();
        (by_target.values().map(Vec::len).sum(), by_target.len())
    }
}

/// The gateway-wide shared view: one layered spatial index, the aura index, the live viewer
/// registry, and the shard table the index's [`ShardId`]s point into.
pub(crate) struct WorldView {
    /// One authoritative viewer anchor with independent world-entity and game-object layers.
    pub(crate) spatial: WorldIndex,
    pub(crate) auras: AuraIndex,
    viewers: RwLock<ViewerRegistry>,
    /// [`ShardId`] → that shard's coordinator handle. Written once at startup, read on every
    /// dispatch; the order fixes the ids, so it must never be reordered after arming.
    shards: RwLock<Vec<Coordinator>>,
    next_session: AtomicU64,
    /// Characters whose client was last sent a running Instance Removal countdown. It outlives the
    /// viewer: a world-port drops the source viewer before the source Shard deletes the row, so
    /// the next world entry must hide the countdown itself.
    countdowns_shown: Mutex<HashSet<u64>>,
    /// Characters this Gateway has itself watched the `game_character.online` false→true edge
    /// for (`character_online_changed`), not yet paired with a later Account Claim close. The
    /// friend-notice dedup state that lets a claim close (`account_claim_changed`) answer
    /// FRIEND_OFFLINE only for a Character THIS Gateway confirmed came online — never for a claim
    /// that opened and closed without `player_login` ever running, which happens whenever
    /// `enter_world` refuses after `claim_session` already succeeded (`CMSG_PLAYER_LOGIN`'s
    /// handler runs them in that order). A Gateway that starts, or reconnects a Shard, after a
    /// Character already logged in has no mark for it and answers nothing for that Character's
    /// NEXT close — a known gap, not a correctness hazard: the friends pane itself still reads
    /// live Realm Presence, so it is never wrong, only occasionally a push notification behind.
    online_characters: Mutex<HashSet<u64>>,
}

impl WorldView {
    pub(crate) fn new(cell_scoped: bool) -> Self {
        Self {
            spatial: WorldIndex::new(cell_scoped),
            auras: AuraIndex::default(),
            viewers: RwLock::new(ViewerRegistry::default()),
            shards: RwLock::new(Vec::new()),
            next_session: AtomicU64::new(1),
            countdowns_shown: Mutex::new(HashSet::new()),
            online_characters: Mutex::new(HashSet::new()),
        }
    }

    /// Record whether `character_guid`'s client now shows an Instance Removal countdown.
    pub(crate) fn note_countdown(&self, character_guid: u64, shown: bool) {
        let mut countdowns = self.countdowns_shown.lock().unwrap();
        if shown {
            countdowns.insert(character_guid);
        } else {
            countdowns.remove(&character_guid);
        }
    }

    pub(crate) fn countdown_shown(&self, character_guid: u64) -> bool {
        self.countdowns_shown
            .lock()
            .unwrap()
            .contains(&character_guid)
    }

    /// Record that `guid` reached the `game_character.online` transition — the one signal a
    /// later Account Claim close checks before it answers FRIEND_OFFLINE.
    pub(crate) fn mark_character_online(&self, guid: u64) {
        self.online_characters
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(guid);
    }

    /// Consume `guid`'s online mark, if this Gateway ever recorded one. `true` only for a
    /// Character this Gateway itself watched come online.
    pub(crate) fn take_character_online(&self, guid: u64) -> bool {
        self.online_characters
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&guid)
    }

    pub(crate) fn viewer(&self, session: SessionId) -> Option<Arc<Viewer>> {
        self.viewers
            .read()
            .unwrap()
            .by_session
            .get(&session)
            .map(|registered| registered.viewer.clone())
    }

    pub(crate) fn session_of_owner(&self, self_guid: u64) -> Option<SessionId> {
        self.viewers
            .read()
            .unwrap()
            .session_of_owner
            .get(&self_guid)
            .copied()
    }

    /// The owner's viewer regardless of shard — for a caller that only knows "my own session",
    /// never a peer's (a shard-scoped row Relay resolves through
    /// [`Self::viewer_of_owner_on_shard`] instead, so a transfer's source shard cannot address the
    /// viewer after it re-registers on the destination shard). `CMSG_FRIEND_LIST`'s own contact
    /// read is the production caller: it only ever asks for the CONNECTED session's own guid.
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

    pub(crate) fn viewer_of_identity_on_shard(
        &self,
        shard: ShardId,
        identity: BoundIdentity,
    ) -> Option<Arc<Viewer>> {
        let registry = self.viewers.read().unwrap();
        let session = registry.session_of_identity.get(&identity.0)?;
        let registered = registry.by_session.get(session)?;
        (registered.shard == shard).then(|| registered.viewer.clone())
    }

    #[cfg(test)]
    pub(crate) fn viewer_of_identity(&self, identity: BoundIdentity) -> Option<Arc<Viewer>> {
        let registry = self.viewers.read().unwrap();
        let session = registry.session_of_identity.get(&identity.0)?;
        registry
            .by_session
            .get(session)
            .map(|registered| registered.viewer.clone())
    }

    pub(crate) fn viewers_on_shard(&self, shard: ShardId) -> Vec<Arc<Viewer>> {
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

    /// Every registered viewer on every shard, for a Relay that visits each World Session.
    pub(crate) fn all_viewers(&self) -> Vec<Arc<Viewer>> {
        let registry = self.viewers.read().unwrap();
        registry
            .by_session
            .values()
            .map(|registered| registered.viewer.clone())
            .collect()
    }

    pub(crate) fn shard(&self, id: ShardId) -> Option<Coordinator> {
        self.shards.read().unwrap().get(id).cloned()
    }

    /// The Shard `session`'s viewer is registered on.
    pub(crate) fn shard_of_viewer(&self, session: SessionId) -> Option<ShardId> {
        let registry = self.viewers.read().unwrap();
        registry
            .by_session
            .get(&session)
            .map(|registered| registered.shard)
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
        registry.remove(session);
        self.spatial.remove_viewer(session);
    }

    /// Geometric recipients plus the row owner's session. The owner lookup lives in the viewer
    /// registry because it is session identity, not spatial state.
    pub(crate) fn world_entity_recipients(
        &self,
        shard: ShardId,
        guid: u64,
        key: CellKey,
    ) -> Vec<SessionId> {
        let mut recipients = self.spatial.viewers_of(EntityLayer::WorldEntity, key);
        if let Some(owner) = self.session_of_owner(guid) {
            if !recipients.contains(&owner) {
                recipients.push(owner);
            }
        }
        let registry = self.viewers.read().unwrap();
        recipients.retain(|session| {
            registry
                .by_session
                .get(session)
                .is_some_and(|v| v.shard == shard)
        });
        recipients
    }

    /// The viewers a cell-anchored event reaches: every session anchored within `half_span` cells
    /// of `key`, plus the sessions of the characters the row names. The owner leg keeps a
    /// self-addressed packet (the attacker's ATTACKSTOP, the caster's cast result, a player's own
    /// aura timer) reaching its owner while the owner's anchor and the row's cell disagree by a
    /// heartbeat. When the source Shard has no anchor, only its owners receive the event.
    /// Every recipient must still belong to that Shard after a transfer.
    ///
    /// The per-viewer gate the job runs is the final filter, unchanged; this only stops the pump
    /// enqueueing a job for every viewer on the shard so it can reject the row later.
    pub(crate) fn cell_audience(
        &self,
        shard: ShardId,
        key: Option<CellKey>,
        half_span: i32,
        owners: &[u64],
    ) -> Vec<Arc<Viewer>> {
        let mut sessions = key
            .map(|key| self.spatial.viewers_within(key, half_span))
            .unwrap_or_default();
        let registry = self.viewers.read().unwrap();
        for owner in owners {
            if let Some(session) = registry.session_of_owner.get(owner) {
                if !sessions.contains(session) {
                    sessions.push(*session);
                }
            }
        }
        sessions
            .into_iter()
            .filter_map(|session| registry.by_session.get(&session))
            .filter(|registered| registered.shard == shard)
            .map(|registered| registered.viewer.clone())
            .collect()
    }

    // ===========================================================================================
    //  Arming — one registration per shard, re-run by the coordinator watchdog after a reconnect.
    // ===========================================================================================

    /// Give this view its shard table. Called once, from `Coordinator::connect`, before arming.
    pub(crate) fn set_shards(&self, shards: Vec<Coordinator>) {
        *self.shards.write().unwrap() = shards;
    }
}

/// Register Relays and reconcile the snapshot between messages on this Shard's pump.
/// Connection replacement joins the old pump before this connection becomes readable.
pub(crate) fn arm_shard(view: Arc<WorldView>, coord: Coordinator, shard: ShardId) -> Result<()> {
    let owner = coord.clone();
    coord.0.on_pump(move |conn| {
        crate::stdb::relays::register_shard(view.clone(), owner, shard, &conn.db);
        seed_shard_from_cache(&view, shard, &conn.db);
    })
}

/// Register the cross-shard private relays on the REALM-CORE connection: group
/// events, Realm Chat Lines (whispers included), Channel Notices and Mail Arrivals written
/// realm-side for recipients whose home shard is elsewhere. Same recipient-keyed dispatchers as
/// `arm_shard`'s private tier, armed ONCE per realm-core connection instead of once per session;
/// the last per-session registrations are gone.
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
    crate::stdb::relays::register_realm(view, realm.clone(), coord, &guard.conn.db);
}

/// Reads one Character's `(guild_id, rank_id)` from the Realm-core cache. A closure so relay
/// tests can supply membership without a connection.
pub(crate) type GuildMembershipRead = Arc<dyn Fn(u64) -> Option<(u32, u32)> + Send + Sync>;

/// Reads the Guild data one settings-changing Guild Event needs from the Realm-core cache:
/// `TABARD_CHANGED` needs the query response, `ROSTER_TO_ACTOR` needs the roster, `ROSTER_REFRESH`
/// needs both. `guild_event_appeared` calls this once per event and shares the result with every
/// recipient's job. A closure so relay tests can supply a snapshot without a live connection.
pub(crate) type GuildRosterSnapshotRead =
    Arc<dyn Fn(u32, u8) -> crate::world::GuildEventSnapshot + Send + Sync>;

/// Reads the open Petition of one Guild Charter from the Realm-core cache. A closure so relay
/// tests can supply a Petition without a connection.
pub(crate) type PetitionRead = Arc<dyn Fn(u64) -> Option<crate::codec::PetitionView> + Send + Sync>;

#[cfg(test)]
fn is_initial_apply(event: &spacetimedb_sdk::Event<Reducer>) -> bool {
    matches!(event, spacetimedb_sdk::Event::SubscribeApplied)
}

// ===============================================================================================
//  Entity dispatch
// ===============================================================================================

// ===============================================================================================
//  Gameobject dispatch
// ===============================================================================================

// ===============================================================================================
//  Motion + creature legs — no index of their own: the row carries its own cell.
// ===============================================================================================

// ===============================================================================================
//  Non-spatial dispatch (shared-call path)
// ===============================================================================================

/// Every viewer a shard-broadcast family may reach: the sessions whose characters `shard`'s
/// database owns — the same audience the per-player subscriptions produced (each player connection
/// was to its own shard's database, and the base queries were unscoped `SELECT *`s). Filtered by
/// direct shard membership; family-specific gates (instance, range) run per viewer at the call site.
pub(crate) fn viewers_on_shard(view: &WorldView, shard: ShardId) -> Vec<Arc<Viewer>> {
    view.viewers_on_shard(shard)
}

// ===============================================================================================
//  Cell-anchored event dispatch. The Module stamps the actor's cell on the event row (or the
//  actor's own row is in the index), so the audience is the actor's box rather than the shard;
//  each job's per-viewer gate stays the final filter.
// ===============================================================================================

/// Cells a yard range can reach from an anchor cell: the range in cells, rounded up, plus one
/// because the listener's anchor is its client position while the gate reads its last durable
/// row, and the two can sit one cell apart.
pub(crate) fn cells_within(range_yd: f32) -> i32 {
    (range_yd / GRID_CELL_SIZE).ceil() as i32 + 1
}

/// The database a `game_group_event` row came from. Each database numbers its rows itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum GroupEventSource {
    Shard(ShardId),
    RealmCore,
}

/// The newest `game_group_event` id relayed to one viewer from each source database.
#[derive(Default)]
pub(crate) struct GroupEventCursor(Mutex<HashMap<GroupEventSource, u64>>);

impl GroupEventCursor {
    /// The rows to relay for the insert callback of `row`, oldest first.
    ///
    /// The SDK fires the inserts of one transaction in hash order, and the client must hear them
    /// in insert order: a filled Meeting Stone party's `COMPLETE` comes before `SETQUEUE(0, NONE)`.
    /// So the first callback of a transaction takes every row of that transaction for this
    /// recipient from `cached`, which reads the table that already holds the whole transaction, and
    /// the later callbacks find theirs taken without calling it. Rows of one transaction share
    /// `created_at`.
    pub(crate) fn take(
        &self,
        source: GroupEventSource,
        row: &GroupEvent,
        cached: impl FnOnce() -> Vec<GroupEvent>,
    ) -> Vec<GroupEvent> {
        let mut cursors = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let cursor = cursors.entry(source).or_default();
        if row.id <= *cursor {
            return Vec::new();
        }
        let mut rows: Vec<GroupEvent> = cached()
            .into_iter()
            .filter(|cached| {
                cached.recipient_guid == row.recipient_guid
                    && cached.created_at == row.created_at
                    && cached.id > *cursor
            })
            .collect();
        if !rows.iter().any(|cached| cached.id == row.id) {
            rows.push(row.clone());
        }
        rows.sort_unstable_by_key(|cached| cached.id);
        *cursor = rows.last().map_or(row.id, |last| last.id);
        rows
    }
}

/// The `game_group_event` table as one source database last cached it, grouped by recipient guid.
/// Every recipient of a transaction takes its rows from the same read.
#[derive(Default)]
pub(crate) struct GroupEventSnapshot(HashMap<u64, Vec<GroupEvent>>);

impl GroupEventSnapshot {
    /// [`GroupEventCursor::take`] over the recipient's rows of the snapshot.
    ///
    /// `read_table` runs only when the recipient's group lacks `row`, which is the first callback
    /// of each transaction. The cache already holds the whole transaction by then, so the later
    /// callbacks reuse the read. A row the table lacks reads it again.
    pub(crate) fn take(
        &mut self,
        cursor: &GroupEventCursor,
        source: GroupEventSource,
        row: &GroupEvent,
        read_table: impl FnOnce() -> Vec<GroupEvent>,
    ) -> Vec<GroupEvent> {
        cursor.take(source, row, || {
            let held = self
                .0
                .get(&row.recipient_guid)
                .is_some_and(|rows| rows.iter().any(|held| held.id == row.id));
            if !held {
                self.0.clear();
                for cached in read_table() {
                    self.0
                        .entry(cached.recipient_guid)
                        .or_default()
                        .push(cached);
                }
            }
            self.0.get(&row.recipient_guid).cloned().unwrap_or_default()
        })
    }
}

/// Every World Session on this Gateway whose Character is a member of `guild_id`. The one
/// audience read for broadcast Guild Events.
pub(crate) fn online_audience(
    view: &WorldView,
    membership: &GuildMembershipRead,
    guild_id: u32,
) -> Vec<Arc<Viewer>> {
    view.all_viewers()
        .into_iter()
        .filter(|viewer| membership(viewer.self_guid).is_some_and(|(id, _)| id == guild_id))
        .collect()
}

// The relay-order tests below scan this file's own source, so the production items that follow
// them must stay where they are.
#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod cell_audience_tests;

#[cfg(test)]
mod relay_bench;

#[cfg(test)]
mod item_durable_tests;

#[cfg(all(test, unix))]
mod benilla_movement_tests {
    #[test]
    fn benilla_clients_observe_each_others_movement_within_aoi() {
        crate::world::tests::benilla_tests::two_clients_observe_movement(|view, row| {
            super::motion(view, 0, row, &Default::default());
        });
    }
}

#[allow(clippy::items_after_test_module)]
#[cfg(test)]
mod family_audience_tests {
    use super::{
        addon_message_appeared, charter_petition_opened, duel_winner_audience,
        exploration_outbound_for_word, guild_event_appeared, guild_membership_changed,
        instance_removal_ended, instance_removal_started, instance_removal_time_left_ms,
        is_initial_apply, item_owner_job, item_visible_changed, levelup_appeared, mail_arrived,
        petition_event_appeared, reputation_appeared, resident_countdown_ms,
        self_res_option_changed, sweep_into_view, system_message_appeared, teleport_appeared,
        weather_changed, xp_appeared, zone_crossed, BoundIdentity, ExplorationReplay,
        GuildMembershipRead, GuildRosterSnapshotRead, MotionPending, OwnerGuid, PetitionRead,
        Viewer, WorldView,
    };
    #[cfg(test)]
    use crate::codec::quest::subscriptions::quest_update_packets;
    use crate::codec::social::subscriptions::private_recipient_audience;
    use crate::stdb::aoi::ViewerGates;
    use crate::stdb::bindings::{
        AddonMessage, CharacterExplored, CharacterQuest, GuildEvent, GuildPetition,
        InstanceRemoval, ItemInstance, LevelupEvent, MailArrival, PlayerReputation,
        SystemMessageEvent, TeleportEvent, XpEvent, ZoneWeather,
    };
    use crate::stdb::world_index::{CellKey, EntityLayer};
    use crate::world::GuildEventSnapshot;
    use crate::world::{Outbound, SessionTx};
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};
    use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
    use wow_world_messages::vanilla::{WeatherChangeType, WeatherType};

    fn identity(byte: u8) -> spacetimedb_sdk::Identity {
        spacetimedb_sdk::Identity::from_byte_array([byte; 32])
    }

    pub(crate) fn viewer(session: u64, self_guid: u64) -> Arc<Viewer> {
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
            active: std::sync::atomic::AtomicBool::new(true),
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
            member_stats: Default::default(),
            ignored: Mutex::default(),
            friends: Mutex::default(),
            team: lyracore_shared::faction::TEAM_ALLIANCE,
            group_events: Default::default(),
        })
    }

    fn queued_job(rx: &Receiver<Outbound>) -> Vec<Outbound> {
        match rx.try_recv().expect("one addressed writer job") {
            Outbound::Job(job) => job(),
            _ => panic!("shared owner dispatch must enqueue packet work as a writer job"),
        }
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

    /// The private tier's whole privacy guarantee (group/resurrect): the addressee and
    /// NOBODY else. The zero cases are the leak vectors — an unset recipient must never match a
    /// real viewer and a real recipient must never match a half-initialized viewer.
    #[test]
    fn private_rows_reach_their_addressee_and_nobody_else() {
        assert!(private_recipient_audience(42, 42));
        assert!(
            !private_recipient_audience(42, 43),
            "a third party must never receive a private row"
        );
        assert!(
            !private_recipient_audience(0, 42),
            "recipient 0 (unaddressed) matches no viewer"
        );
        assert!(
            !private_recipient_audience(42, 0),
            "an uninitialized viewer guid matches no row"
        );
        assert!(
            !private_recipient_audience(0, 0),
            "the all-zero case is a leak, not a match — both sides unset must still deny"
        );
    }

    #[test]
    fn a_system_message_reaches_only_its_recipient_and_missing_recipients_are_dropped() {
        let view = WorldView::new(true);
        let (recipient_tx, recipient_rx) = SessionTx::with_depth(0);
        let recipient = viewer_with_tx(1, 9001, identity(1), recipient_tx);
        let (bystander_tx, bystander_rx) = SessionTx::with_depth(0);
        let bystander = viewer_with_tx(2, 9002, identity(2), bystander_tx);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(recipient.clone(), anchor, 0);
        view.add_viewer_on_shard(bystander, anchor, 0);

        let row = SystemMessageEvent {
            id: 1,
            recipient_identity: recipient.bound_identity,
            recipient_guid: recipient.self_guid,
            message: "Package loaded".to_string(),
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        };
        system_message_appeared(&view, &row);

        let outbound = queued_job(&recipient_rx);
        let [Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(actual))] = outbound.as_slice()
        else {
            panic!("the recipient must receive one System Message packet");
        };
        assert_eq!(
            **actual,
            crate::codec::build_gm_system_message(row.message.clone())
        );
        assert!(
            bystander_rx.try_recv().is_err(),
            "a second Character must receive no queued work"
        );

        let missing = SystemMessageEvent {
            recipient_guid: 9999,
            ..row
        };
        system_message_appeared(&view, &missing);
        assert!(recipient_rx.try_recv().is_err());
        assert!(bystander_rx.try_recv().is_err());
    }

    fn raw_packets(outbound: Vec<Outbound>) -> Vec<(u16, Vec<u8>)> {
        outbound
            .into_iter()
            .map(|out| match out {
                Outbound::Raw { opcode, body } => (opcode, body),
                _ => panic!("guild relays write raw packets"),
            })
            .collect()
    }

    fn membership(rows: &[(u64, u32, u32)]) -> GuildMembershipRead {
        let rows = rows.to_vec();
        Arc::new(move |guid| {
            rows.iter()
                .find(|(member, _, _)| *member == guid)
                .map(|(_, guild_id, rank_id)| (*guild_id, *rank_id))
        })
    }

    /// The broadcast (`0..=13`) and addressed-membership (`0x40..`) kinds these audience tests use
    /// never read a Guild snapshot, so a closure that panics if called proves it.
    fn unreachable_snapshot_read() -> GuildRosterSnapshotRead {
        Arc::new(|_guild_id, _kind| panic!("this Guild Event kind must not read a snapshot"))
    }

    /// A snapshot read that always answers `snapshot` and counts its own calls, so a test can
    /// assert a settings-change event reads the Realm-core cache once and shares the result with
    /// every recipient.
    fn counting_snapshot_read(
        snapshot: GuildEventSnapshot,
    ) -> (GuildRosterSnapshotRead, Arc<AtomicU32>) {
        let reads = Arc::new(AtomicU32::new(0));
        let recorded = reads.clone();
        let read: GuildRosterSnapshotRead = Arc::new(move |_guild_id, _kind| {
            recorded.fetch_add(1, Ordering::Relaxed);
            snapshot.clone()
        });
        (read, reads)
    }

    /// SMSG_GUILD_ROSTER for one member whose officer note is `officer_note`, so two calls with
    /// different notes give two packets a test can tell apart by content.
    fn roster_variant(officer_note: &str) -> ServerOpcodeMessage {
        let guild = crate::codec::GuildView {
            guild_id: 7,
            name: "Knights".into(),
            ..Default::default()
        };
        let line = crate::codec::GuildRosterLine {
            guid: 9001,
            name: "Alice".into(),
            officer_note: officer_note.into(),
            ..Default::default()
        };
        ServerOpcodeMessage::SMSG_GUILD_ROSTER(Box::new(crate::codec::build_guild_roster(
            &guild,
            [line],
        )))
    }

    #[test]
    fn a_membership_change_reaches_the_member_and_viewers_that_created_it() {
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (member_tx, member_rx) = SessionTx::with_depth(0);
        let member = viewer_with_tx(1, 9001, identity(1), member_tx);
        let (observer_tx, observer_rx) = SessionTx::with_depth(0);
        let observer = viewer_with_tx(2, 9002, identity(2), observer_tx);
        observer.created.lock().unwrap().insert(9001);
        let (stranger_tx, stranger_rx) = SessionTx::with_depth(0);
        let stranger = viewer_with_tx(3, 9003, identity(3), stranger_tx);
        for viewer in [&member, &observer, &stranger] {
            view.add_viewer_on_shard(viewer.clone(), anchor, 0);
        }
        view.spatial
            .upsert_entity(EntityLayer::WorldEntity, 9001, anchor, 0, 0);

        guild_membership_changed(&view, &membership(&[(9001, 7, 0)]), 9001);
        let founded = crate::codec::build_guild_values(9001, 7, 0);
        assert_eq!(raw_packets(queued_job(&member_rx)), vec![founded.clone()]);
        assert_eq!(raw_packets(queued_job(&observer_rx)), vec![founded]);
        assert!(queued_job(&stranger_rx).is_empty());

        guild_membership_changed(&view, &membership(&[]), 9001);
        assert_eq!(
            raw_packets(queued_job(&member_rx)),
            vec![crate::codec::build_guild_values(9001, 0, 0)]
        );
    }

    #[test]
    fn a_removal_reaches_the_member_before_its_entity_is_indexed() {
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (member_tx, member_rx) = SessionTx::with_depth(0);
        let (other_tx, other_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(viewer_with_tx(1, 9001, identity(1), member_tx), anchor, 0);
        view.add_viewer_on_shard(viewer_with_tx(2, 9002, identity(2), other_tx), anchor, 0);

        guild_membership_changed(&view, &membership(&[]), 9001);
        assert_eq!(
            raw_packets(queued_job(&member_rx)),
            vec![crate::codec::build_guild_values(9001, 0, 0)]
        );
        assert!(other_rx.try_recv().is_err());
    }

    fn guild_event(recipient_guid: u64, kind: u8, subject_guid: u64) -> GuildEvent {
        GuildEvent {
            id: 1,
            guild_id: 7,
            recipient_guid,
            kind,
            subject_guid,
            other_guid: 0,
            strings: vec!["Alice".to_string()],
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        }
    }

    #[test]
    fn a_broadcast_guild_event_reaches_online_members_and_nobody_else() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let mut receivers = Vec::new();
        for (session, guid) in [(1, 9001), (2, 9002), (3, 9003), (4, 9004)] {
            let (tx, rx) = SessionTx::with_depth(0);
            // Members stand on different Shards: the audience is the Guild, not the Shard.
            view.add_viewer_on_shard(
                viewer_with_tx(session, guid, identity(session as u8), tx),
                anchor,
                session as usize,
            );
            receivers.push(rx);
        }
        let [alice, bob, stranger, other_guild] = <[_; 4]>::try_from(receivers).unwrap();
        let members = membership(&[(9001, 7, 0), (9002, 7, 4), (9004, 8, 0)]);
        let snapshot_read = unreachable_snapshot_read();

        guild_event_appeared(
            &view,
            &members,
            &snapshot_read,
            &guild_event(0, event_kind::SIGNED_OFF, 9001),
        );
        let signed_off = crate::codec::build_guild_event_raw(
            event_kind::SIGNED_OFF,
            &["Alice".to_string()],
            9001,
        );
        assert_eq!(raw_packets(queued_job(&alice)), vec![signed_off.clone()]);
        assert_eq!(raw_packets(queued_job(&bob)), vec![signed_off]);
        assert!(stranger.try_recv().is_err());
        assert!(other_guild.try_recv().is_err());

        guild_event_appeared(
            &view,
            &members,
            &snapshot_read,
            &guild_event(0, event_kind::SIGNED_ON, 9001),
        );
        assert!(
            alice.try_recv().is_err(),
            "a member does not hear its own sign-on"
        );
        assert_eq!(raw_packets(queued_job(&bob)).len(), 1);
    }

    #[test]
    fn an_addressed_guild_event_reaches_only_its_recipient() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (recipient_tx, recipient_rx) = SessionTx::with_depth(0);
        let (member_tx, member_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(
            viewer_with_tx(1, 9003, identity(1), recipient_tx),
            anchor,
            0,
        );
        view.add_viewer_on_shard(viewer_with_tx(2, 9001, identity(2), member_tx), anchor, 0);

        // DISBANDED (0x08) is addressed per former member, so it exercises the generic addressed
        // path through the raw SMSG_GUILD_EVENT builder.
        guild_event_appeared(
            &view,
            &membership(&[(9001, 7, 0)]),
            &unreachable_snapshot_read(),
            &guild_event(9003, event_kind::DISBANDED, 9001),
        );
        assert_eq!(raw_packets(queued_job(&recipient_rx)).len(), 1);
        assert!(member_rx.try_recv().is_err());
    }

    #[test]
    fn tabard_changed_reaches_every_online_member_with_the_new_emblem() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (leader_tx, leader_rx) = SessionTx::with_depth(0);
        let (member_tx, member_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(viewer_with_tx(1, 9001, identity(1), leader_tx), anchor, 0);
        view.add_viewer_on_shard(viewer_with_tx(2, 9002, identity(2), member_tx), anchor, 1);
        let emblem = crate::codec::GuildView {
            guild_id: 7,
            name: "Tabard Guild".into(),
            emblem_style: 11,
            emblem_color: 12,
            border_style: 3,
            border_color: 14,
            background_color: 15,
            ..Default::default()
        };
        let query = ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(Box::new(
            crate::codec::build_guild_query_response(&emblem),
        ));
        let snapshot = GuildEventSnapshot {
            query: Some(query.clone()),
            ..GuildEventSnapshot::default()
        };
        let (snapshot_read, reads) = counting_snapshot_read(snapshot);
        let tabard_changed = GuildEvent {
            strings: Vec::new(),
            ..guild_event(0, event_kind::TABARD_CHANGED, 0)
        };

        guild_event_appeared(
            &view,
            &membership(&[(9001, 7, 0), (9002, 7, 4)]),
            &snapshot_read,
            &tabard_changed,
        );

        for rx in [&leader_rx, &member_rx] {
            let [Outbound::Raw { opcode, body }, Outbound::One(response)] =
                <[Outbound; 2]>::try_from(queued_job(rx)).ok().unwrap()
            else {
                panic!("TABARD_CHANGED renders the event, then the query response");
            };
            assert_eq!((opcode, body), (0x0092, vec![9, 0]));
            assert_eq!(response, query);
        }
        assert_eq!(
            reads.load(Ordering::Relaxed),
            1,
            "one snapshot read serves every recipient"
        );
    }

    /// `0x50` (`event_kind::ROSTER_TO_ACTOR`) is addressed, so it takes the same recipient-only
    /// path SIGNED_ON's twin above already proves. This test pins that the actor's job reads the
    /// shared snapshot and picks the roster its own Guild Rank sees.
    #[test]
    fn roster_to_actor_reaches_only_the_actor_with_the_roster_its_rank_sees() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (actor_tx, actor_rx) = SessionTx::with_depth(0);
        let (other_tx, other_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(viewer_with_tx(1, 9001, identity(1), actor_tx), anchor, 0);
        view.add_viewer_on_shard(viewer_with_tx(2, 9002, identity(2), other_tx), anchor, 0);
        let shown = roster_variant("secret");
        let snapshot = GuildEventSnapshot {
            roster_hidden: Some(roster_variant("")),
            roster_shown: Some(shown.clone()),
            rank_rights: vec![(0, 0xF_F1FF)],
            ..GuildEventSnapshot::default()
        };
        let (snapshot_read, reads) = counting_snapshot_read(snapshot);

        guild_event_appeared(
            &view,
            &membership(&[(9001, 7, 0), (9002, 7, 0)]),
            &snapshot_read,
            &guild_event(9001, event_kind::ROSTER_TO_ACTOR, 0),
        );

        let [Outbound::One(actual)] = <[Outbound; 1]>::try_from(queued_job(&actor_rx))
            .ok()
            .unwrap()
        else {
            panic!("expected one roster packet");
        };
        assert_eq!(actual, shown, "rank 0 holds VIEWOFFNOTE in this snapshot");
        assert!(other_rx.try_recv().is_err());
        assert_eq!(reads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn roster_to_actor_with_no_membership_row_sends_nothing() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (actor_tx, actor_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(viewer_with_tx(1, 9001, identity(1), actor_tx), anchor, 0);

        guild_event_appeared(
            &view,
            &membership(&[]),
            &unreachable_snapshot_read(),
            &guild_event(9001, event_kind::ROSTER_TO_ACTOR, 0),
        );

        assert!(queued_job(&actor_rx).is_empty());
    }

    /// The same guard also has to check which Guild the membership row names: `map_or(0, ...)`
    /// ignored the returned guild_id, so a member who moved to another Guild between this job's
    /// enqueue and its run would still get a roster, rendered under its new Guild's own Rank
    /// Rights.
    #[test]
    fn roster_to_actor_with_membership_in_another_guild_sends_nothing() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (actor_tx, actor_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(viewer_with_tx(1, 9001, identity(1), actor_tx), anchor, 0);

        guild_event_appeared(
            &view,
            &membership(&[(9001, 8, 0)]),
            &unreachable_snapshot_read(),
            &guild_event(9001, event_kind::ROSTER_TO_ACTOR, 0),
        );

        assert!(queued_job(&actor_rx).is_empty());
    }

    /// `0x80` (`event_kind::ROSTER_REFRESH`) is a broadcast kind, so it takes the same audience
    /// path the SIGNED_OFF broadcast test above already proves. This test pins that every online
    /// member's own job reads one shared snapshot and picks the roster its own rank sees, so a
    /// large Guild costs one Realm-core read for the whole broadcast, not one per member.
    #[test]
    fn roster_refresh_gives_each_online_member_the_roster_its_own_rank_sees() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let mut receivers = Vec::new();
        for (session, guid) in [(1, 9001), (2, 9002), (3, 9003)] {
            let (tx, rx) = SessionTx::with_depth(0);
            view.add_viewer_on_shard(
                viewer_with_tx(session, guid, identity(session as u8), tx),
                anchor,
                0,
            );
            receivers.push(rx);
        }
        let [leader, member, stranger] = <[_; 3]>::try_from(receivers).unwrap();
        let hidden = roster_variant("");
        let shown = roster_variant("secret");
        let query = ServerOpcodeMessage::SMSG_GUILD_QUERY_RESPONSE(Box::new(
            crate::codec::build_guild_query_response(&crate::codec::GuildView {
                guild_id: 7,
                ..Default::default()
            }),
        ));
        let snapshot = GuildEventSnapshot {
            query: Some(query),
            roster_hidden: Some(hidden.clone()),
            roster_shown: Some(shown.clone()),
            rank_rights: vec![(0, 0xF_F1FF), (4, 0x43)],
        };
        let (snapshot_read, reads) = counting_snapshot_read(snapshot);

        guild_event_appeared(
            &view,
            &membership(&[(9001, 7, 0), (9002, 7, 4)]),
            &snapshot_read,
            &guild_event(0, event_kind::ROSTER_REFRESH, 0),
        );

        let leader_out = queued_job(&leader);
        let member_out = queued_job(&member);
        assert!(stranger.try_recv().is_err());
        let [Outbound::One(_), Outbound::One(leader_roster)] =
            <[Outbound; 2]>::try_from(leader_out).ok().unwrap()
        else {
            panic!("expected a query response then a roster");
        };
        assert_eq!(leader_roster, shown, "rank 0 holds VIEWOFFNOTE");
        let [Outbound::One(_), Outbound::One(member_roster)] =
            <[Outbound; 2]>::try_from(member_out).ok().unwrap()
        else {
            panic!("expected a query response then a roster");
        };
        assert_eq!(member_roster, hidden, "rank 4 cannot view officer notes");
        assert_eq!(
            reads.load(Ordering::Relaxed),
            1,
            "one snapshot read serves every recipient"
        );
    }

    #[test]
    fn an_invite_event_reaches_its_target_as_smsg_guild_invite() {
        use lyracore_shared::guild::event_kind;
        use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (target_tx, target_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(viewer_with_tx(1, 9003, identity(1), target_tx), anchor, 0);

        let mut row = guild_event(9003, event_kind::INVITE, 0);
        row.strings = vec!["Alice".to_string(), "Tracer Guild".to_string()];
        guild_event_appeared(&view, &membership(&[]), &unreachable_snapshot_read(), &row);

        let outbound = queued_job(&target_rx);
        let [Outbound::One(ServerOpcodeMessage::SMSG_GUILD_INVITE(invite))] = outbound.as_slice()
        else {
            panic!(
                "expected exactly one SMSG_GUILD_INVITE, got {} packets",
                outbound.len()
            );
        };
        assert_eq!(invite.player_name, "Alice");
        assert_eq!(invite.guild_name, "Tracer Guild");
    }

    #[test]
    fn a_decline_event_reaches_the_inviter_as_raw_smsg_guild_decline() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (inviter_tx, inviter_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(viewer_with_tx(1, 9003, identity(1), inviter_tx), anchor, 0);

        let row = guild_event(9003, event_kind::DECLINE, 0);
        guild_event_appeared(&view, &membership(&[]), &unreachable_snapshot_read(), &row);

        assert_eq!(
            raw_packets(queued_job(&inviter_rx)),
            vec![(0x0086, b"Alice\0".to_vec())]
        );
    }

    const CHARTER: u64 = 0x4000_0000_0000_0101;

    fn night_watch() -> crate::codec::PetitionView {
        crate::codec::PetitionView {
            petition_id: 42,
            charter_item_guid: CHARTER,
            owner_guid: 9001,
            name: "Night Watch".into(),
            signers: vec![9002],
        }
    }

    /// A Petition read that answers `petition` for the Charter and counts its calls.
    fn counting_petition_read(
        petition: Option<crate::codec::PetitionView>,
    ) -> (PetitionRead, Arc<AtomicU32>) {
        let reads = Arc::new(AtomicU32::new(0));
        let recorded = reads.clone();
        let read: PetitionRead = Arc::new(move |charter_item_guid| {
            recorded.fetch_add(1, Ordering::Relaxed);
            petition
                .clone()
                .filter(|petition| petition.charter_item_guid == charter_item_guid)
        });
        (read, reads)
    }

    fn unreachable_petition_read() -> PetitionRead {
        Arc::new(|_charter_item_guid| panic!("this petition kind must not read a Petition"))
    }

    fn petition_event(recipient_guid: u64, kind: u8, subject_guid: u64) -> GuildEvent {
        GuildEvent {
            guild_id: 0,
            other_guid: CHARTER,
            strings: Vec::new(),
            ..guild_event(recipient_guid, kind, subject_guid)
        }
    }

    /// Registers the recipient 9003 and a bystander 9004 on one shard.
    fn recipient_and_bystander(view: &WorldView) -> (Receiver<Outbound>, Receiver<Outbound>) {
        let anchor = CellKey::at(0, 0, 0, 0);
        let (recipient_tx, recipient_rx) = SessionTx::with_depth(0);
        let (bystander_tx, bystander_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(
            viewer_with_tx(1, 9003, identity(1), recipient_tx),
            anchor,
            0,
        );
        view.add_viewer_on_shard(
            viewer_with_tx(2, 9004, identity(2), bystander_tx),
            anchor,
            0,
        );
        (recipient_rx, bystander_rx)
    }

    #[test]
    fn an_offer_shows_its_target_the_signature_window_read_once() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let (target_rx, bystander_rx) = recipient_and_bystander(&view);
        let (read, reads) = counting_petition_read(Some(night_watch()));

        petition_event_appeared(
            &view,
            &read,
            &petition_event(9003, event_kind::PETITION_OFFERED, 9001),
        );

        let outbound = queued_job(&target_rx);
        let [Outbound::One(ServerOpcodeMessage::SMSG_PETITION_SHOW_SIGNATURES(window))] =
            outbound.as_slice()
        else {
            panic!("expected one SMSG_PETITION_SHOW_SIGNATURES");
        };
        assert_eq!(
            **window,
            crate::codec::build_petition_show_signatures(&night_watch())
        );
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        assert!(bystander_rx.try_recv().is_err());
    }

    #[test]
    fn signature_events_render_sign_results_for_the_charter_and_the_signer() {
        use lyracore_shared::guild::event_kind;
        use wow_world_messages::vanilla::PetitionResult;
        let view = WorldView::new(true);
        let (owner_rx, _bystander_rx) = recipient_and_bystander(&view);
        for (kind, result) in [
            (event_kind::PETITION_SIGNED, PetitionResult::Ok),
            (
                event_kind::PETITION_ALREADY_SIGNED,
                PetitionResult::AlreadySigned,
            ),
        ] {
            petition_event_appeared(
                &view,
                &unreachable_petition_read(),
                &petition_event(9003, kind, 9002),
            );
            let outbound = queued_job(&owner_rx);
            let [Outbound::One(ServerOpcodeMessage::SMSG_PETITION_SIGN_RESULTS(results))] =
                outbound.as_slice()
            else {
                panic!("expected one SMSG_PETITION_SIGN_RESULTS");
            };
            assert_eq!(
                (
                    results.petition.guid(),
                    results.owner.guid(),
                    results.result
                ),
                (CHARTER, 9002, result)
            );
        }
    }

    #[test]
    fn a_decline_event_names_the_decliner_to_the_owner() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let (owner_rx, _bystander_rx) = recipient_and_bystander(&view);
        petition_event_appeared(
            &view,
            &unreachable_petition_read(),
            &petition_event(9003, event_kind::PETITION_DECLINED, 9002),
        );
        let outbound = queued_job(&owner_rx);
        let [Outbound::One(ServerOpcodeMessage::MSG_PETITION_DECLINE(declined))] =
            outbound.as_slice()
        else {
            panic!("expected one MSG_PETITION_DECLINE");
        };
        assert_eq!(declined.petition.guid(), 9002);
    }

    #[test]
    fn a_founder_event_names_the_new_guild() {
        use lyracore_shared::guild::event_kind;
        use wow_world_messages::vanilla::{GuildCommand, GuildCommandResult};
        let view = WorldView::new(true);
        let (signer_rx, _bystander_rx) = recipient_and_bystander(&view);
        let row = GuildEvent {
            guild_id: 12,
            strings: vec!["Night Watch".into()],
            ..petition_event(9003, event_kind::FOUNDER, 0)
        };
        petition_event_appeared(&view, &unreachable_petition_read(), &row);
        let outbound = queued_job(&signer_rx);
        let [Outbound::One(ServerOpcodeMessage::SMSG_GUILD_COMMAND_RESULT(result))] =
            outbound.as_slice()
        else {
            panic!("expected one SMSG_GUILD_COMMAND_RESULT");
        };
        assert_eq!(
            (result.command, result.string.as_str(), result.result),
            (
                GuildCommand::Founder,
                "Night Watch",
                GuildCommandResult::PlayerNoMoreInGuild
            )
        );
    }

    #[test]
    fn a_lost_signature_sends_the_owner_a_fresh_petition_query() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let (owner_rx, _bystander_rx) = recipient_and_bystander(&view);
        let (read, _) = counting_petition_read(Some(night_watch()));
        petition_event_appeared(
            &view,
            &read,
            &petition_event(9003, event_kind::PETITION_CHANGED, 9002),
        );
        let outbound = queued_job(&owner_rx);
        let [Outbound::One(ServerOpcodeMessage::SMSG_PETITION_QUERY_RESPONSE(query))] =
            outbound.as_slice()
        else {
            panic!("expected one SMSG_PETITION_QUERY_RESPONSE");
        };
        assert_eq!(
            (
                query.petition_id,
                query.charter_owner.guid(),
                query.guild_name.as_str()
            ),
            (42, 9001, "Night Watch")
        );

        let (gone, _) = counting_petition_read(None);
        petition_event_appeared(
            &view,
            &gone,
            &petition_event(9003, event_kind::PETITION_CHANGED, 9002),
        );
        assert!(
            queued_job(&owner_rx).is_empty(),
            "a Petition gone by the time the job runs renders nothing"
        );
    }

    #[test]
    fn a_new_petition_shows_its_id_on_the_owners_charter() {
        let view = WorldView::new(true);
        let (owner_rx, bystander_rx) = recipient_and_bystander(&view);
        charter_petition_opened(
            &view,
            &GuildPetition {
                petition_id: 42,
                charter_item_guid: CHARTER,
                owner_guid: 9003,
                owner_name: "Owner".into(),
                team: 469,
                name: "Night Watch".into(),
                created_micros: 0,
            },
        );
        let packets = raw_packets(queued_job(&owner_rx));
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].0, 0x00A9);
        let updates = lyracore_shared::values_mask::parse_values_updates(&packets[0].1);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].guid, CHARTER);
        assert_eq!(updates[0].fields, vec![(22, 42)]);
        assert!(bystander_rx.try_recv().is_err());
    }

    #[test]
    fn a_disband_reaches_each_former_member_exactly_once() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let mut receivers = Vec::new();
        for (session, guid) in [(1, 9001), (2, 9002)] {
            let (tx, rx) = SessionTx::with_depth(0);
            view.add_viewer_on_shard(
                viewer_with_tx(session, guid, identity(session as u8), tx),
                anchor,
                0,
            );
            receivers.push(rx);
        }
        let [alice_rx, bob_rx] = <[_; 2]>::try_from(receivers).unwrap();
        // A disbanded Guild's members are already gone by the time the relay job runs, so
        // `membership` answers `None` for both. DISBANDED is addressed, not read from current
        // membership.
        let no_members = membership(&[]);

        guild_event_appeared(
            &view,
            &no_members,
            &unreachable_snapshot_read(),
            &guild_event(9001, event_kind::DISBANDED, 0),
        );
        guild_event_appeared(
            &view,
            &no_members,
            &unreachable_snapshot_read(),
            &guild_event(9002, event_kind::DISBANDED, 0),
        );

        assert_eq!(queued_job(&alice_rx).len(), 1);
        assert_eq!(queued_job(&bob_rx).len(), 1);
    }

    #[test]
    fn a_removed_event_does_not_reach_the_removed_member() {
        use lyracore_shared::guild::event_kind;
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (remaining_tx, remaining_rx) = SessionTx::with_depth(0);
        let (removed_tx, removed_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(
            viewer_with_tx(1, 9001, identity(1), remaining_tx),
            anchor,
            0,
        );
        // The removed Character's viewer is still on this Gateway (mid-logout, or simply still
        // connected); its membership is already gone, so `online_audience` no longer includes it.
        view.add_viewer_on_shard(viewer_with_tx(2, 9002, identity(2), removed_tx), anchor, 0);

        // Only 9001 remains a member of guild 7; 9002 was just removed.
        let members = membership(&[(9001, 7, 0)]);
        guild_event_appeared(
            &view,
            &members,
            &unreachable_snapshot_read(),
            &guild_event(0, event_kind::REMOVED, 0),
        );

        assert_eq!(queued_job(&remaining_rx).len(), 1);
        assert!(removed_rx.try_recv().is_err());
    }

    /// The owner's `PLAYER_SELF_RES_SPELL` from one queued VALUES packet.
    fn self_res_spell_of(outbound: Vec<Outbound>) -> Option<i32> {
        use wow_world_messages::vanilla::{Object, UpdateMask};
        let [Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(packet))] = outbound.as_slice()
        else {
            panic!("the owner must receive one SMSG_UPDATE_OBJECT");
        };
        match packet.objects.as_slice() {
            [Object::Values {
                mask1: UpdateMask::Player(player),
                ..
            }] => player.player_self_res_spell(),
            other => panic!("expected one Player VALUES block, got {other:?}"),
        }
    }

    #[test]
    fn a_self_resurrect_option_reaches_only_its_character_on_this_shard() {
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (owner_tx, owner_rx) = SessionTx::with_depth(0);
        let (bystander_tx, bystander_rx) = SessionTx::with_depth(0);
        let (remote_tx, remote_rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(viewer_with_tx(1, 9001, identity(1), owner_tx), anchor, 0);
        view.add_viewer_on_shard(
            viewer_with_tx(2, 9002, identity(2), bystander_tx),
            anchor,
            0,
        );
        view.add_viewer_on_shard(viewer_with_tx(3, 9003, identity(3), remote_tx), anchor, 1);

        self_res_option_changed(&view, 0, 9001, 3026);
        assert_eq!(self_res_spell_of(queued_job(&owner_rx)), Some(3026));
        assert!(owner_rx.try_recv().is_err(), "one change queues one job");
        assert!(
            bystander_rx.try_recv().is_err(),
            "a nearby Character never sees another Character's option"
        );

        self_res_option_changed(&view, 0, 9003, 3026);
        assert!(
            remote_rx.try_recv().is_err(),
            "a Character on another Shard is not this Shard's to address"
        );

        self_res_option_changed(&view, 0, 9001, 0);
        assert_eq!(self_res_spell_of(queued_job(&owner_rx)), Some(0));
    }

    #[test]
    fn a_mail_arrival_reaches_its_recipient_on_any_shard_and_an_unknown_guid_gets_nothing() {
        let view = WorldView::new(true);
        let (recipient_tx, recipient_rx) = SessionTx::with_depth(0);
        let recipient = viewer_with_tx(1, 9001, identity(1), recipient_tx);
        let (bystander_tx, bystander_rx) = SessionTx::with_depth(0);
        let bystander = viewer_with_tx(2, 9002, identity(2), bystander_tx);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(recipient.clone(), anchor, 1);
        view.add_viewer_on_shard(bystander, anchor, 0);

        let row = MailArrival {
            id: 1,
            recipient_guid: recipient.self_guid,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        };
        mail_arrived(&view, &row);

        let outbound = queued_job(&recipient_rx);
        let [Outbound::One(ServerOpcodeMessage::SMSG_RECEIVED_MAIL(packet))] = outbound.as_slice()
        else {
            panic!("the recipient must receive one SMSG_RECEIVED_MAIL");
        };
        assert_eq!(packet.unknown1, 0);
        assert!(
            recipient_rx.try_recv().is_err(),
            "one arrival queues one job"
        );
        assert!(
            bystander_rx.try_recv().is_err(),
            "a second Character must receive no queued work"
        );

        mail_arrived(
            &view,
            &MailArrival {
                recipient_guid: 9999,
                ..row
            },
        );
        assert!(recipient_rx.try_recv().is_err());
        assert!(bystander_rx.try_recv().is_err());
    }

    #[test]
    fn duel_winner_aoi_fan_excludes_the_two_private_recipients() {
        assert!(!duel_winner_audience(10, 10, 20));
        assert!(!duel_winner_audience(20, 10, 20));
        assert!(duel_winner_audience(30, 10, 20));
        assert!(!duel_winner_audience(0, 10, 20));
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
            .upsert_entity(EntityLayer::WorldEntity, 77, entering, 0, 0);
        view.spatial
            .upsert_entity(EntityLayer::GameObject, 77, entering, 1, 0);
        let delta = view
            .spatial
            .move_viewer_delta(viewer.session, CellKey::at(0, 0, 1, 0))
            .expect("a cell crossing recenters");
        assert_eq!(delta.world_entities.entered, vec![(77, 0)]);
        assert_eq!(delta.game_objects.entered, vec![(77, 1)]);

        // Removing one table's row cannot erase the equal-valued GUID in the other table.
        assert_eq!(
            view.spatial.remove_entity(EntityLayer::WorldEntity, 77, 0),
            Some((entering, 0))
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
            active: std::sync::atomic::AtomicBool::new(true),
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
            member_stats: Default::default(),
            ignored: Mutex::default(),
            friends: Mutex::default(),
            team: lyracore_shared::faction::TEAM_ALLIANCE,
            group_events: Default::default(),
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
        assert!(view
            .viewer_of_owner(OwnerGuid(replacement.self_guid))
            .is_none());
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

        assert!(
            other_rx.try_recv().is_err(),
            "an unrelated viewer receives nothing"
        );
        let out = queued_job(&owner_rx);
        assert_eq!(out.len(), 1, "one teleport row creates one writer result");
        match &out[0] {
            Outbound::Batch(messages) => {
                assert!(matches!(
                    messages.as_slice(),
                    [
                        ServerOpcodeMessage::SMSG_TRANSFER_PENDING(_),
                        ServerOpcodeMessage::SMSG_NEW_WORLD(_)
                    ]
                ));
            }
            _ => panic!("cross-map teleport must remain one ordered writer batch"),
        }
        assert!(
            owner_rx.try_recv().is_err(),
            "the owner receives exactly one job"
        );
    }

    fn at_micros(micros: i64) -> spacetimedb_sdk::ScheduleAt {
        spacetimedb_sdk::ScheduleAt::Time(spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(
            micros,
        ))
    }

    /// The one `SMSG_RAID_GROUP_ONLY` timer a writer job produced.
    fn raid_group_only_timer(out: &[Outbound]) -> u32 {
        match out {
            [Outbound::One(ServerOpcodeMessage::SMSG_RAID_GROUP_ONLY(packet))] => {
                assert_eq!(
                    packet.error,
                    wow_world_messages::vanilla::RaidGroupError::Required
                );
                packet.homebind_timer
            }
            _ => panic!("expected one SMSG_RAID_GROUP_ONLY"),
        }
    }

    #[test]
    fn an_instance_removal_shows_and_hides_the_countdown_for_its_owner_only() {
        let view = Arc::new(WorldView::new(true));
        let (owner_tx, owner_rx) = SessionTx::with_depth(0);
        let (other_tx, other_rx) = SessionTx::with_depth(0);
        let owner = viewer_with_tx(1, 9001, identity(1), owner_tx);
        let other = viewer_with_tx(2, 9002, identity(2), other_tx);
        let anchor = CellKey::at(36, 7, 0, 0);
        view.add_viewer_on_shard(owner.clone(), anchor, 1);
        view.add_viewer_on_shard(other, anchor, 1);
        // Due in 2100: far enough ahead that the time left is always the full countdown.
        let row = InstanceRemoval {
            scheduled_id: 3,
            scheduled_at: at_micros(4_102_444_800_000_000),
            character_guid: owner.self_guid,
            instance_id: 7,
            group_id: 42,
        };

        instance_removal_started(&view, 0, &row);
        assert!(owner_rx.try_recv().is_err(), "another Shard's row");
        instance_removal_started(&view, 1, &row);
        assert_eq!(raid_group_only_timer(&queued_job(&owner_rx)), 60_000);
        assert!(view.countdown_shown(owner.self_guid));
        instance_removal_ended(&view, 1, &row);
        assert_eq!(raid_group_only_timer(&queued_job(&owner_rx)), 0);
        assert!(!view.countdown_shown(owner.self_guid));
        assert!(owner_rx.try_recv().is_err(), "one job per row change");
        assert!(
            other_rx.try_recv().is_err(),
            "an unrelated viewer receives nothing"
        );
    }

    /// A world-port drops the source viewer before the source Shard deletes the row, so the delete
    /// reaches nobody. The countdown the client still shows is hidden at the next world entry.
    #[test]
    fn a_world_entry_hides_a_countdown_whose_row_is_gone() {
        let view = Arc::new(WorldView::new(true));
        let (tx, rx) = SessionTx::with_depth(0);
        let source = viewer_with_tx(1, 9001, identity(1), tx);
        view.add_viewer_on_shard(source.clone(), CellKey::at(36, 7, 0, 0), 1);
        let row = InstanceRemoval {
            scheduled_id: 3,
            scheduled_at: at_micros(4_102_444_800_000_000),
            character_guid: source.self_guid,
            instance_id: 7,
            group_id: 42,
        };
        instance_removal_started(&view, 1, &row);
        queued_job(&rx);
        view.remove_viewer(source.session);
        instance_removal_ended(&view, 1, &row);

        let shown = view.countdown_shown(9001);
        assert_eq!(resident_countdown_ms(None, shown, 0), Some(0));
        assert_eq!(resident_countdown_ms(None, false, 0), None);
        assert_eq!(
            resident_countdown_ms(
                Some(&at_micros(1_900_000_000_000_000)),
                false,
                1_899_999_955_000_000
            ),
            Some(45_000)
        );
    }

    /// The world-entry sweep and a replayed insert both show the time left, not a fresh 60 s.
    #[test]
    fn the_countdown_shows_the_time_left_rounded_up_and_nothing_once_due() {
        let due = 1_900_000_000_000_000;
        assert_eq!(
            instance_removal_time_left_ms(&at_micros(due), 1_899_999_955_000_000),
            Some(45_000)
        );
        assert_eq!(
            instance_removal_time_left_ms(&at_micros(due), 1_899_999_999_999_999),
            Some(1)
        );
        assert_eq!(
            instance_removal_time_left_ms(&at_micros(due), 1_899_999_900_000_000),
            Some(60_000),
            "a Gateway clock behind the Module's shows no more than the full countdown"
        );
        assert_eq!(
            instance_removal_time_left_ms(&at_micros(due), 1_900_000_000_000_000),
            None
        );
        assert_eq!(
            instance_removal_time_left_ms(&at_micros(due), 1_900_000_000_000_005),
            None
        );
        let interval = spacetimedb_sdk::ScheduleAt::Interval(
            spacetimedb_sdk::TimeDuration::from_micros(1_000_000),
        );
        assert_eq!(
            instance_removal_time_left_ms(&interval, 1_899_999_955_000_000),
            None
        );
    }

    fn worn_item(slot: u8) -> ItemInstance {
        ItemInstance {
            guid: 700,
            entry: 1337,
            owner_identity: identity(1),
            owner_guid: 9001,
            slot,
            stack_count: 1,
            durability: 20,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            enchant_id: 7745,
            soulbound: false,
            random_property_id: 117,
            item_text_id: 0,
        }
    }

    fn visible_items(out: Vec<Outbound>) -> Vec<(u8, u32, u32, u32)> {
        use wow_world_messages::vanilla::{Object, ServerMessage, UpdateMask, VisibleItemIndex};
        let mut items = Vec::new();
        for message in out {
            let Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update)) = message else {
                panic!("peers receive only visible item VALUES");
            };
            let [Object::Values {
                guid1,
                mask1: UpdateMask::Player(mask),
            }] = update.objects.as_slice()
            else {
                panic!("peers receive no private item objects");
            };
            assert_eq!(guid1.guid(), 9001);
            for slot in 0..=18 {
                if let Some(item) =
                    mask.player_visible_item(VisibleItemIndex::try_from(slot).unwrap())
                {
                    assert_eq!(item.enchants[1], 0);
                    items.push((slot, item.item, item.random_property_id, item.enchants[0]));
                }
            }
            let mut bytes = Vec::new();
            update.write_unencrypted_server(&mut bytes).unwrap();
            let decoded = lyracore_shared::values_mask::parse_values_updates(&bytes[4..]);
            assert_eq!(decoded.len(), 1);
            // Build 5875 exposes nineteen 12-word visible items starting at field 258.
            assert!(
                decoded[0]
                    .fields
                    .iter()
                    .all(|&(field, _)| (258..486).contains(&field)),
                "no TYPE, inventory pointers, item fields or Character sheet fields"
            );
        }
        items
    }

    #[test]
    fn visible_gear_changes_reach_peers_with_current_slot_contents() {
        let view = WorldView::new(true);
        let (tx, rx) = SessionTx::with_depth(0);
        let peer = viewer_with_tx(2, 9002, identity(2), tx);
        peer.created.lock().unwrap().insert(9001);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(peer, anchor, 0);
        view.spatial
            .upsert_entity(EntityLayer::WorldEntity, 9001, anchor, 0, 0);
        let committed = Arc::new(Mutex::new(Vec::<ItemInstance>::new()));
        let read: super::ItemSlotRead = {
            let committed = committed.clone();
            Arc::new(move |owner, slot| {
                committed
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|item| item.owner_guid == owner && item.slot == slot)
                    .map(crate::stdb::reads::view_of_item_row)
            })
        };
        for slot in [0, 8, 18] {
            let item = worn_item(slot);
            *committed.lock().unwrap() = vec![item.clone()];
            item_visible_changed(&view, 0, None, Some(&item), read.clone());
            assert_eq!(visible_items(queued_job(&rx)), [(slot, 1337, 117, 823)]);
        }
        let item = worn_item(8);

        for changed in [
            ItemInstance {
                enchant_id: 7748,
                ..item.clone()
            },
            ItemInstance {
                enchant_id: 0,
                ..item.clone()
            },
            ItemInstance {
                entry: 42,
                ..item.clone()
            },
            ItemInstance {
                random_property_id: 118,
                ..item.clone()
            },
        ] {
            *committed.lock().unwrap() = vec![changed.clone()];
            item_visible_changed(&view, 0, Some(&item), Some(&changed), read.clone());
            let enchant = match changed.enchant_id {
                7748 => 724,
                0 => 0,
                _ => 823,
            };
            assert_eq!(
                visible_items(queued_job(&rx)),
                [(8, changed.entry, changed.random_property_id, enchant)]
            );
        }

        let bagged = ItemInstance {
            slot: 23,
            ..item.clone()
        };
        *committed.lock().unwrap() = vec![bagged.clone()];
        item_visible_changed(&view, 0, Some(&item), Some(&bagged), read.clone());
        assert_eq!(visible_items(queued_job(&rx)), [(8, 0, 0, 0)]);
        *committed.lock().unwrap() = vec![item.clone()];
        item_visible_changed(&view, 0, Some(&bagged), Some(&item), read.clone());
        assert_eq!(visible_items(queued_job(&rx)), [(8, 1337, 117, 823)]);

        committed.lock().unwrap().clear();
        item_visible_changed(&view, 0, Some(&item), None, read);
        assert_eq!(visible_items(queued_job(&rx)), [(8, 0, 0, 0)]);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn visible_gear_swap_and_delete_read_replacements_when_the_writer_runs() {
        let view = WorldView::new(true);
        let (tx, rx) = SessionTx::with_depth(0);
        let peer = viewer_with_tx(2, 9002, identity(2), tx);
        peer.created.lock().unwrap().insert(9001);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.add_viewer_on_shard(peer, anchor, 0);
        view.spatial
            .upsert_entity(EntityLayer::WorldEntity, 9001, anchor, 0, 0);
        let old = worn_item(8);
        let moved = ItemInstance {
            slot: 23,
            ..old.clone()
        };
        let committed = Arc::new(Mutex::new(vec![old.clone()]));
        let read: super::ItemSlotRead = {
            let committed = committed.clone();
            Arc::new(move |owner, slot| {
                committed
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|item| item.owner_guid == owner && item.slot == slot)
                    .map(crate::stdb::reads::view_of_item_row)
            })
        };
        item_visible_changed(&view, 0, Some(&old), Some(&moved), read.clone());
        item_visible_changed(&view, 0, Some(&old), None, read);
        *committed.lock().unwrap() = vec![ItemInstance {
            guid: 701,
            entry: 42,
            enchant_id: 7748,
            ..old
        }];
        for _ in 0..2 {
            assert_eq!(visible_items(queued_job(&rx)), [(8, 42, 117, 724)]);
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn visible_gear_is_scoped_to_peers_that_still_see_the_character() {
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        view.spatial
            .upsert_entity(EntityLayer::WorldEntity, 9001, anchor, 0, 0);
        let item = worn_item(8);
        let read: super::ItemSlotRead =
            Arc::new(|_, _| Some(crate::stdb::reads::view_of_item_row(&worn_item(8))));
        let mut queues = Vec::new();
        for (session, guid, cell, shard) in [
            (1, 9001, anchor, 0),
            (2, 9002, anchor, 0),
            (3, 9003, CellKey::at(0, 0, 10, 0), 0),
            (4, 9004, CellKey::at(0, 1, 0, 0), 0),
            (5, 9005, CellKey::at(1, 0, 0, 0), 0),
            (6, 9006, anchor, 1),
        ] {
            let (tx, rx) = SessionTx::with_depth(0);
            let viewer = viewer_with_tx(session, guid, identity(session as u8), tx);
            viewer.created.lock().unwrap().insert(9001);
            view.add_viewer_on_shard(viewer.clone(), cell, shard);
            queues.push((viewer, rx));
        }
        let (peer, rx) = &queues[1];
        item_visible_changed(&view, 0, None, Some(&item), read.clone());
        peer.created.lock().unwrap().remove(&9001);
        assert!(queued_job(rx).is_empty(), "hidden before the writer runs");
        item_visible_changed(&view, 0, None, Some(&item), read.clone());
        peer.created.lock().unwrap().insert(9001);
        assert_eq!(
            visible_items(queued_job(rx)),
            [(8, 1337, 117, 823)],
            "CREATE ran before the gear job"
        );
        item_visible_changed(&view, 0, None, Some(&item), read.clone());
        view.remove_viewer(peer.session);
        assert!(queued_job(rx).is_empty(), "departed World Session");
        for (_, rx) in &queues {
            assert!(
                rx.try_recv().is_err(),
                "owner, distant, other map/instance/Shard receive no peer job"
            );
        }
        view.spatial
            .upsert_entity(EntityLayer::WorldEntity, 9001, anchor, 1, 0);
        item_visible_changed(&view, 0, None, Some(&item), read);
        for (_, rx) in &queues {
            assert!(
                rx.try_recv().is_err(),
                "source Shard cannot relay after Transfer"
            );
        }
    }

    #[test]
    fn visible_gear_ignores_bagged_items_and_changes_to_private_fields() {
        let view = WorldView::new(true);
        let anchor = CellKey::at(0, 0, 0, 0);
        let (tx, rx) = SessionTx::with_depth(0);
        view.add_viewer_on_shard(viewer_with_tx(2, 9002, identity(2), tx), anchor, 0);
        view.spatial
            .upsert_entity(EntityLayer::WorldEntity, 9001, anchor, 0, 0);
        let read: super::ItemSlotRead = Arc::new(|_, _| panic!("no gear read needed"));
        for slot in [19, 23, 120] {
            let bagged = worn_item(slot);
            item_visible_changed(&view, 0, None, Some(&bagged), read.clone());
            item_visible_changed(&view, 0, Some(&bagged), None, read.clone());
            item_visible_changed(
                &view,
                0,
                Some(&bagged),
                Some(&ItemInstance {
                    enchant_id: 0,
                    ..bagged.clone()
                }),
                read.clone(),
            );
        }
        let old = worn_item(8);
        let new = ItemInstance {
            durability: 0,
            stack_count: 3,
            soulbound: true,
            ..old.clone()
        };
        item_visible_changed(&view, 0, Some(&old), Some(&new), read);
        assert!(rx.try_recv().is_err());
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
            vec![Outbound::Raw {
                opcode: 1,
                body: vec![1],
            }]
        });
        assert!(matches!(
            queued_job(&source_rx).as_slice(),
            [Outbound::Raw { .. }]
        ));
        assert!(
            other_rx.try_recv().is_err(),
            "an unrelated item owner receives nothing"
        );

        view.remove_viewer(source.session);
        item_owner_job(&view, 0, source.self_guid, |_| {
            vec![Outbound::Raw {
                opcode: 2,
                body: vec![2],
            }]
        });
        assert!(
            source_rx.try_recv().is_err(),
            "a removed source viewer receives no cascade delete"
        );

        view.add_viewer_on_shard(destination, anchor, 1);
        item_owner_job(&view, 0, source.self_guid, |_| {
            vec![Outbound::Raw {
                opcode: 2,
                body: vec![2],
            }]
        });
        assert!(
            destination_rx.try_recv().is_err(),
            "a source-shard delta applied after destination registration must be dropped"
        );
        item_owner_job(&view, 1, source.self_guid, |_| {
            vec![Outbound::Raw {
                opcode: 3,
                body: vec![3],
            }]
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

        for shard in [1, 0] {
            xp_appeared(
                &view,
                shard,
                &XpEvent {
                    id: 1,
                    recipient_identity: owner.bound_identity,
                    killed_guid: 77,
                    total_exp: 123,
                    created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
                    is_kill: true,
                    rested_bonus: 0,
                },
            );
            levelup_appeared(
                &view,
                shard,
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
                shard,
                &AddonMessage {
                    id: 3,
                    recipient_identity: owner.bound_identity,
                    cmd: "progress".to_string(),
                    payload: "ready".to_string(),
                    created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
                },
            );

            if shard == 1 {
                assert!(
                    owner_rx.try_recv().is_err(),
                    "source Shard events cannot reach a destination owner"
                );
                assert!(other_rx.try_recv().is_err());
                continue;
            }
            assert!(
                other_rx.try_recv().is_err(),
                "an unrelated identity receives nothing"
            );
            assert!(matches!(
                queued_job(&owner_rx).as_slice(),
                [Outbound::Raw { opcode: 0x01d0, .. }]
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
            assert!(
                owner_rx.try_recv().is_err(),
                "each identity row queues exactly once"
            );
        }
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
            [Outbound::Raw {
                opcode: fog_opcode,
                body: fog_body,
            }, Outbound::Raw {
                opcode: toast_opcode,
                body: toast_body,
            }] => {
                assert_eq!((*fog_opcode, fog_body), (new_fog_opcode, &new_fog_body));
                assert_eq!(
                    (*toast_opcode, toast_body),
                    (discovery_opcode, &discovery_body)
                );
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

        assert!(
            other_rx.try_recv().is_err(),
            "an unrelated owner receives nothing"
        );
        for standing in [3175, 3200] {
            let (opcode, body) = crate::codec::build_set_faction_standing_raw(19, standing)
                .expect("a valid reputation index has a packet");
            match queued_job(&owner_rx).as_slice() {
                [Outbound::Raw {
                    opcode: actual_opcode,
                    body: actual_body,
                }] => {
                    assert_eq!((*actual_opcode, actual_body), (opcode, &body));
                }
                _ => panic!("the reputation row must use its reputation_index, not faction_id"),
            }
        }
        assert!(
            owner_rx.try_recv().is_err(),
            "each reputation row queues exactly one job"
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
            view.world_entity_recipients(0, viewer.self_guid, far),
            vec![viewer.session]
        );
        assert_eq!(
            view.world_entity_recipients(0, viewer.self_guid, anchor),
            vec![viewer.session]
        );
        view.remove_viewer(viewer.session);
        assert!(view
            .world_entity_recipients(0, viewer.self_guid, far)
            .is_empty());
    }

    #[test]
    fn world_entry_sweep_defers_resident_reads_to_one_writer_job() {
        let view = Arc::new(WorldView::new(true));
        let (tx, rx) = SessionTx::with_depth(0);
        let viewer = viewer_with_tx(1, 9001, identity(1), tx);
        view.add_viewer_on_shard(viewer.clone(), CellKey::at(0, 0, 0, 0), 0);
        let shards = view.shards.write().unwrap();
        let (done, completed) = std::sync::mpsc::channel();
        let reader = {
            let view = view.clone();
            std::thread::spawn(move || {
                sweep_into_view(&view, &viewer);
                done.send(()).unwrap();
            })
        };

        let returned = completed.recv_timeout(std::time::Duration::from_secs(5));
        drop(shards);
        reader.join().unwrap();
        assert!(
            returned.is_ok(),
            "the reader must not wait for the resident Shard table"
        );
        assert!(queued_job(&rx).is_empty(), "there are no resident rows");
        assert!(rx.try_recv().is_err(), "one sweep must be one writer job");
    }
}

#[cfg(test)]
mod realm_chat_relay_tests {
    use super::{
        channel_notice_appeared, contact_changed, realm_chat_appeared, ExplorationReplay,
        MotionPending, Viewer, WorldView,
    };
    use crate::stdb::aoi::ViewerGates;
    use crate::stdb::bindings::{ChatChannelNoticeEvent, ContactEntry, RealmChatEvent};
    use crate::stdb::world_index::CellKey;
    use crate::world::{Outbound, SessionTx};
    use std::collections::{HashMap, HashSet};
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    const SPEAKER: u64 = 10;

    /// A registered viewer for `self_guid` on `shard`, with the receiving end of its writer queue.
    fn listener(
        view: &WorldView,
        shard: usize,
        self_guid: u64,
    ) -> (Arc<Viewer>, Receiver<Outbound>) {
        let (tx, rx) = SessionTx::with_depth(0);
        let viewer = Arc::new(Viewer {
            active: std::sync::atomic::AtomicBool::new(true),
            session: view.next_session_id(),
            self_guid,
            bound_identity: spacetimedb_sdk::Identity::from_byte_array([self_guid as u8; 32]),
            map_id: 0,
            instance_id: 0,
            zone_id: 0.into(),
            tx,
            created: Arc::new(Mutex::new(HashSet::new())),
            gates: Arc::new(ViewerGates::default()),
            skill_slots: Arc::new(Mutex::new((HashMap::new(), 0))),
            explored: Mutex::new(ExplorationReplay::default()),
            motion_pending: Arc::new(MotionPending::default()),
            member_stats: Default::default(),
            ignored: Mutex::default(),
            friends: Mutex::default(),
            team: lyracore_shared::faction::TEAM_ALLIANCE,
            group_events: Default::default(),
        });
        view.add_viewer_on_shard(viewer.clone(), CellKey::at(0, 0, 0, 0), shard);
        (viewer, rx)
    }

    fn party_line(recipients: &[u64], ignorable: bool) -> RealmChatEvent {
        RealmChatEvent {
            id: 1,
            kind: 1,
            speaker_guid: SPEAKER,
            language: 7,
            chat_tag: 0,
            channel_name: String::new(),
            message: "form up".to_string(),
            recipients: recipients.to_vec(),
            ignorable,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        }
    }

    fn ignore_row(owner_guid: u64, target_guid: u64, is_ignore: bool) -> ContactEntry {
        ContactEntry {
            id: 1,
            owner_guid,
            owner_identity: spacetimedb_sdk::Identity::ZERO,
            target_guid,
            is_ignore,
        }
    }

    /// Every queued packet for one viewer, jobs run in order.
    /// The body of every packet queued for one viewer, jobs run in order. Each packet must be one
    /// framed `SMSG_MESSAGECHAT`: size u16 big-endian, then opcode 0x0096 little-endian.
    fn received(rx: &Receiver<Outbound>) -> Vec<Vec<u8>> {
        let mut bodies = Vec::new();
        while let Ok(outbound) = rx.try_recv() {
            let Outbound::Job(job) = outbound else {
                panic!("the Relay must enqueue packet work as a writer job");
            };
            for packet in job() {
                let Outbound::One(message) = packet else {
                    panic!("a Realm Chat Line is one packet");
                };
                let mut frame = Vec::new();
                message.write_unencrypted_server(&mut frame).unwrap();
                let body = frame.split_off(4);
                assert_eq!(frame[..2], ((body.len() + 2) as u16).to_be_bytes());
                assert_eq!(frame[2..], [0x96, 0x00], "SMSG_MESSAGECHAT");
                bodies.push(body);
            }
        }
        bodies
    }

    /// A PARTY line in Common from `SPEAKER` saying "form up", written out from cm:Chat.cpp:3637-3641
    /// rather than built with the codec the Relay calls: type, language, the sender guid twice, the
    /// sized message with its terminator, then the chat tag.
    fn expected_line() -> Vec<u8> {
        let mut body = vec![0x01]; // CHAT_MSG_PARTY
        body.extend(7u32.to_le_bytes()); // LANG_COMMON
        body.extend(SPEAKER.to_le_bytes());
        body.extend(SPEAKER.to_le_bytes());
        body.extend(8u32.to_le_bytes()); // "form up" and its terminator
        body.extend(b"form up\0");
        body.push(0); // CHAT_TAG_NONE
        body
    }

    #[test]
    fn a_line_reaches_each_recipient_once_on_any_shard_and_nobody_else() {
        let view = WorldView::new(true);
        let (_, speaker_rx) = listener(&view, 0, SPEAKER);
        let (_, member_rx) = listener(&view, 1, 20);
        let (_, bystander_rx) = listener(&view, 0, 30);

        realm_chat_appeared(&view, &party_line(&[SPEAKER, 20, 99], false));

        assert_eq!(received(&speaker_rx), [expected_line()]);
        assert_eq!(received(&member_rx), [expected_line()]);
        assert!(
            received(&bystander_rx).is_empty(),
            "a Character the Module did not name hears nothing"
        );
    }

    #[test]
    fn an_ignorable_line_skips_only_the_listener_who_ignores_the_speaker() {
        let view = WorldView::new(true);
        let (_, ignorer_rx) = listener(&view, 0, 20);
        let (_, other_rx) = listener(&view, 1, 30);
        contact_changed(&view, 0, &ignore_row(20, SPEAKER, true), true);

        realm_chat_appeared(&view, &party_line(&[20, 30], true));
        assert!(received(&ignorer_rx).is_empty());
        assert_eq!(received(&other_rx), [expected_line()]);

        realm_chat_appeared(&view, &party_line(&[20, 30], false));
        assert_eq!(
            received(&ignorer_rx),
            [expected_line()],
            "a line the Module marked not ignorable reaches an ignorer too"
        );
        assert_eq!(received(&other_rx), [expected_line()]);
    }

    /// Every raw packet queued for one viewer, jobs run in order, as `(opcode, body)`.
    fn received_raw(rx: &Receiver<Outbound>) -> Vec<(u16, Vec<u8>)> {
        let mut packets = Vec::new();
        while let Ok(outbound) = rx.try_recv() {
            let Outbound::Job(job) = outbound else {
                panic!("the Relay must enqueue packet work as a writer job");
            };
            for packet in job() {
                let Outbound::Raw { opcode, body } = packet else {
                    panic!("a Channel Notice is one raw packet");
                };
                packets.push((opcode, body));
            }
        }
        packets
    }

    /// JOINED naming `SPEAKER` in "Tx": notice 0x00, the name, then the guid (cm:Channel.cpp:760-764).
    #[test]
    fn a_channel_notice_reaches_each_recipient_on_any_shard_and_nobody_else() {
        let view = WorldView::new(true);
        let (_, first_rx) = listener(&view, 0, 20);
        let (_, second_rx) = listener(&view, 1, 30);
        let (_, bystander_rx) = listener(&view, 0, 40);
        contact_changed(&view, 0, &ignore_row(20, SPEAKER, true), true);

        channel_notice_appeared(
            &view,
            &ChatChannelNoticeEvent {
                id: 1,
                notice: 0x00,
                channel_name: "Tx".to_string(),
                subject_guid: SPEAKER,
                actor_guid: 0,
                old_flags: 0,
                new_flags: 0,
                channel_flags: 0,
                text: String::new(),
                recipients: vec![20, 30, 99],
                created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            },
        );

        let mut joined = vec![0x00, b'T', b'x', 0];
        joined.extend(SPEAKER.to_le_bytes());
        assert_eq!(
            received_raw(&first_rx),
            [(0x0099, joined.clone())],
            "notices skip the ignore list"
        );
        assert_eq!(received_raw(&second_rx), [(0x0099, joined)]);
        assert!(received_raw(&bystander_rx).is_empty());
    }

    /// A resubscribe replays inserts but not the deletes it missed. Reconciling the Shard must
    /// rebuild each of its viewers' ignore sets from the rows that are there now.
    #[test]
    fn reconciling_a_shard_rebuilds_its_viewers_ignore_sets_from_the_contact_rows() {
        let view = Arc::new(WorldView::new(true));
        let (stale, _stale_rx) = listener(&view, 0, 20);
        let (kept, _kept_rx) = listener(&view, 0, 30);
        let (elsewhere, _elsewhere_rx) = listener(&view, 1, 40);
        for (viewer, shard) in [(&stale, 0), (&kept, 0), (&elsewhere, 1)] {
            contact_changed(
                &view,
                shard,
                &ignore_row(viewer.self_guid, SPEAKER, true),
                true,
            );
        }

        super::reconcile_shard(
            &view,
            0,
            vec![],
            vec![],
            vec![],
            vec![
                ignore_row(30, SPEAKER, true),
                ignore_row(30, 77, true),
                ignore_row(20, 77, false),
            ],
        );

        assert!(
            !stale.ignores(SPEAKER),
            "an ignore deleted while disconnected must not survive the resubscribe"
        );
        assert!(kept.ignores(SPEAKER) && kept.ignores(77));
        assert!(!stale.ignores(77), "a friend row is not an ignore");
        assert!(
            elsewhere.ignores(SPEAKER),
            "another Shard's viewers keep the set their own Shard maintains"
        );
    }

    #[test]
    fn the_ignore_set_follows_its_owners_ignore_rows_on_the_owners_shard() {
        let view = WorldView::new(true);
        let (viewer, _rx) = listener(&view, 0, 20);

        contact_changed(&view, 0, &ignore_row(20, SPEAKER, false), true);
        assert!(!viewer.ignores(SPEAKER), "a friend row is not an ignore");

        contact_changed(&view, 1, &ignore_row(20, SPEAKER, true), true);
        assert!(
            !viewer.ignores(SPEAKER),
            "another Shard's copy of the owner's rows does not address this viewer"
        );

        contact_changed(&view, 0, &ignore_row(20, SPEAKER, true), true);
        assert!(viewer.ignores(SPEAKER));

        contact_changed(&view, 0, &ignore_row(20, SPEAKER, true), false);
        assert!(!viewer.ignores(SPEAKER));
    }

    /// The friend set follows the same rule as the ignore set, off the SAME contact rows and
    /// callbacks, filtered by `is_ignore` the other way.
    #[test]
    fn the_friend_set_follows_its_owners_friend_rows_on_the_owners_shard() {
        let view = WorldView::new(true);
        let (viewer, _rx) = listener(&view, 0, 20);

        contact_changed(&view, 0, &ignore_row(20, SPEAKER, true), true);
        assert!(!viewer.is_friend(SPEAKER), "an ignore row is not a friend");

        contact_changed(&view, 1, &ignore_row(20, SPEAKER, false), true);
        assert!(
            !viewer.is_friend(SPEAKER),
            "another Shard's copy of the owner's rows does not address this viewer"
        );

        contact_changed(&view, 0, &ignore_row(20, SPEAKER, false), true);
        assert!(viewer.is_friend(SPEAKER));

        contact_changed(&view, 0, &ignore_row(20, SPEAKER, false), false);
        assert!(!viewer.is_friend(SPEAKER));
    }

    /// A resubscribe rebuilds the friend set the same way it rebuilds the ignore set.
    #[test]
    fn reconciling_a_shard_rebuilds_its_viewers_friend_sets_from_the_contact_rows() {
        let view = Arc::new(WorldView::new(true));
        let (kept, _kept_rx) = listener(&view, 0, 30);
        contact_changed(&view, 0, &ignore_row(30, SPEAKER, false), true);

        super::reconcile_shard(
            &view,
            0,
            vec![],
            vec![],
            vec![],
            vec![ignore_row(30, SPEAKER, false), ignore_row(30, 77, true)],
        );

        assert!(kept.is_friend(SPEAKER));
        assert!(!kept.is_friend(77), "an ignore row is not a friend");
    }
}

#[cfg(test)]
mod account_claim_relay_tests {
    use super::{
        character_online_changed, claim_closed, friend_notice_candidates, AccountClaim, Character,
        ExplorationReplay, MotionPending, Viewer, WorldView,
    };
    use crate::stdb::aoi::ViewerGates;
    use crate::stdb::world_index::CellKey;
    use crate::world::{Outbound, SessionTx};
    use std::collections::{HashMap, HashSet};
    use std::sync::mpsc::Receiver;
    use std::sync::{Arc, Mutex};

    const ALLIANCE: u32 = lyracore_shared::faction::TEAM_ALLIANCE;
    const HORDE: u32 = lyracore_shared::faction::TEAM_HORDE;
    const FRIEND: u64 = 10;

    /// A registered viewer for `self_guid` on `shard`, already listing [`FRIEND`].
    fn listener(
        view: &WorldView,
        shard: usize,
        self_guid: u64,
        team: u32,
    ) -> (Arc<Viewer>, Receiver<Outbound>) {
        let (tx, rx) = SessionTx::with_depth(0);
        let viewer = Arc::new(Viewer {
            active: std::sync::atomic::AtomicBool::new(true),
            session: view.next_session_id(),
            self_guid,
            bound_identity: spacetimedb_sdk::Identity::from_byte_array([self_guid as u8; 32]),
            map_id: 0,
            instance_id: 0,
            zone_id: 0.into(),
            tx,
            created: Arc::new(Mutex::new(HashSet::new())),
            gates: Arc::new(ViewerGates::default()),
            skill_slots: Arc::new(Mutex::new((HashMap::new(), 0))),
            explored: Mutex::new(ExplorationReplay::default()),
            motion_pending: Arc::new(MotionPending::default()),
            member_stats: Default::default(),
            ignored: Mutex::default(),
            friends: Mutex::new(HashSet::from([FRIEND])),
            team,
            group_events: Default::default(),
        });
        view.add_viewer_on_shard(viewer.clone(), CellKey::at(0, 0, 0, 0), shard);
        (viewer, rx)
    }

    fn claim(generation: u64, character_guid: u64, closed: bool) -> AccountClaim {
        AccountClaim {
            account_id: 1,
            generation,
            request_nonce: 1,
            character_guid,
            expires_micros: 1_000_000,
            closed,
        }
    }

    /// A `game_character` row with only the columns this relay reads set to something other than
    /// zero, the rest at whatever a fresh row would have. `session_start_micros` is `player_login`'s
    /// own stamp — 0 outside a session, a real timestamp inside one.
    fn character_row(
        guid: u64,
        race: u8,
        class: u8,
        level: u8,
        zone_id: u32,
        online: bool,
        session_start_micros: u64,
    ) -> Character {
        Character {
            guid,
            account_id: 0,
            owner_identity: spacetimedb_sdk::Identity::ZERO,
            name: String::new(),
            race,
            class,
            gender: 0,
            skin: 0,
            face: 0,
            hair_style: 0,
            hair_color: 0,
            facial_hair: 0,
            level,
            xp: 0,
            next_level_xp: 0,
            map_id: 0,
            zone_id,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            orientation: 0.0,
            first_login: false,
            online,
            money: 0,
            rested_xp: 0,
            last_logout_micros: 0,
            home_map: 0,
            home_zone: 0,
            home_x: 0.0,
            home_y: 0.0,
            home_z: 0.0,
            played_total_secs: 0,
            session_start_micros,
            health: 0,
            power: 0,
            respec_count: 0,
            death_expire_micros: 0,
            pending_instance_id: 0,
            gm_level: 0,
            pending_ghost: false,
            resting: false,
            rested_since_micros: 0,
            pending_godmode: false,
            pending_run_speed_mult_bp: 0,
            bank_bag_slots: 0,
            watched_faction_index: -1,
        }
    }

    /// One case per bullet in `claim_closed`'s doc comment, rows written by hand.
    #[test]
    fn claim_closed_covers_every_bullet() {
        // Inserted open (the account's first-ever claim): not a close.
        assert_eq!(claim_closed(None, &claim(1, FRIEND, false)), None);

        // A renewal — only `expires_micros` moves: not a close.
        let old = claim(1, FRIEND, false);
        let mut renewed = old.clone();
        renewed.expires_micros += 1;
        assert_eq!(claim_closed(Some(&old), &renewed), None);

        // Closed (logout): the Character stops being live.
        let old = claim(1, FRIEND, false);
        let closed = claim(1, FRIEND, true);
        assert_eq!(claim_closed(Some(&old), &closed), Some(FRIEND));

        // Reopened after being closed (a fresh login, a new generation): not a close.
        let old = claim(1, FRIEND, true);
        let reopened = claim(2, FRIEND, false);
        assert_eq!(claim_closed(Some(&old), &reopened), None);

        // Replaced while live, by a DIFFERENT Character (a relogin as someone else on the same
        // Account without an intervening close): the OLD Character stops being live.
        let old = claim(1, FRIEND, false);
        let replaced = claim(2, 99, false);
        assert_eq!(claim_closed(Some(&old), &replaced), Some(FRIEND));

        // Reaped while already closed (the claim reaper closes an already-closed row, or closes it
        // twice across a race): not a (new) close.
        let old = claim(1, FRIEND, true);
        let still_closed = claim(1, FRIEND, true);
        assert_eq!(claim_closed(Some(&old), &still_closed), None);
    }

    /// `friend_notice_candidates` collects every friend-listing viewer on any Shard, and never the
    /// subject's own viewer, whatever it lists.
    #[test]
    fn friend_notice_candidates_lists_every_friend_listing_viewer_but_never_the_subject() {
        let view = WorldView::new(true);
        let (friend, _friend_rx) = listener(&view, 0, 20, ALLIANCE);
        let (elsewhere, _elsewhere_rx) = listener(&view, 1, 21, ALLIANCE);
        let (other_team, _other_team_rx) = listener(&view, 0, 22, HORDE);
        let (non_friend, _non_friend_rx) = listener(&view, 0, 23, ALLIANCE);
        non_friend.friends.lock().unwrap().clear();
        let (subject, _subject_rx) = listener(&view, 0, FRIEND, ALLIANCE);
        subject.friends.lock().unwrap().insert(FRIEND); // hypothetically lists itself

        let mut candidates: Vec<u64> = friend_notice_candidates(&view, FRIEND)
            .iter()
            .map(|viewer| viewer.self_guid)
            .collect();
        candidates.sort_unstable();

        assert_eq!(
            candidates,
            [friend.self_guid, elsewhere.self_guid, other_team.self_guid],
            "every friend-listing viewer on any Shard is a candidate, team included (each caller \
             checks it its own way), but never a non-friend and never the subject's own viewer"
        );
    }

    #[test]
    fn character_online_changed_reaches_a_same_team_friend_with_the_just_committed_row() {
        let view = WorldView::new(true);
        let (_friend, friend_rx) = listener(&view, 0, 20, ALLIANCE);
        let (_other_team, other_team_rx) = listener(&view, 0, 21, HORDE);
        let (non_friend, non_friend_rx) = listener(&view, 0, 22, ALLIANCE);
        non_friend.friends.lock().unwrap().clear();

        // Last logged out at level 21 in zone 12; this login already leveled up and landed the
        // Character in zone 33 — the row's OWN fields, not a stale earlier read.
        let old = character_row(FRIEND, 1, 4, 21, 12, false, 0);
        let new = character_row(FRIEND, 1, 4, 22, 33, true, 500_000);

        character_online_changed(&view, &old, &new);

        match friend_rx
            .try_recv()
            .expect("a same-team friend must hear it")
        {
            Outbound::Job(job) => match job().as_slice() {
                [Outbound::Raw { opcode, body }] => {
                    assert_eq!(*opcode, crate::codec::social::SMSG_FRIEND_STATUS_OPCODE);
                    assert_eq!(body[0], 0x02, "ONLINE");
                    assert_eq!(body[1..9], FRIEND.to_le_bytes());
                    assert_eq!(
                        body[9], 1,
                        "status ONLINE — Away Status always clears at login"
                    );
                    assert_eq!(body[10..14], 33u32.to_le_bytes(), "the POST-login zone");
                    assert_eq!(body[14..18], 22u32.to_le_bytes(), "the POST-login level");
                    assert_eq!(body[18..22], 4u32.to_le_bytes(), "class");
                }
                _ => panic!("expected one Raw packet"),
            },
            _ => panic!("expected a job"),
        }
        assert!(
            other_team_rx.try_recv().is_err(),
            "a viewer on the other team hears nothing"
        );
        assert!(
            non_friend_rx.try_recv().is_err(),
            "a same-team viewer who does not list the guid as a friend hears nothing"
        );
        assert!(
            view.take_character_online(FRIEND),
            "the login must be marked so the later claim close can find it"
        );
    }

    /// An UPDATE that is not a login — an already-online Character's level-up, a logout, or a
    /// same-database transfer's freeze/rebuild with no real login in between — fires nothing and
    /// marks nothing.
    #[test]
    fn character_online_changed_ignores_every_update_that_is_not_a_login() {
        let view = WorldView::new(true);
        let (_friend, friend_rx) = listener(&view, 0, 20, ALLIANCE);

        let old = character_row(FRIEND, 1, 4, 21, 33, true, 500_000);
        let new = character_row(FRIEND, 1, 4, 22, 33, true, 500_000); // a level-up, same session
        character_online_changed(&view, &old, &new);
        assert!(friend_rx.try_recv().is_err());
        assert!(!view.take_character_online(FRIEND));

        let old = character_row(FRIEND, 1, 4, 22, 33, true, 500_000);
        let new = character_row(FRIEND, 1, 4, 22, 33, false, 0); // a real logout
        character_online_changed(&view, &old, &new);
        assert!(friend_rx.try_recv().is_err());

        // A same-database transfer's freeze zeroes session_start_micros without touching `online`
        // (`freeze_live_entity` calls `persist_entity` with `set_offline: false`) — no login
        // happened, so this must not fire either, even though `online` reads true on both sides.
        let old = character_row(FRIEND, 1, 4, 22, 33, true, 500_000);
        let new = character_row(FRIEND, 1, 4, 22, 33, true, 0); // transfer freeze, still online
        character_online_changed(&view, &old, &new);
        assert!(friend_rx.try_recv().is_err());
        assert!(!view.take_character_online(FRIEND));
    }

    #[test]
    fn character_online_changed_fires_for_a_real_login_even_when_online_was_stuck_true() {
        let view = WorldView::new(true);
        let (_friend, friend_rx) = listener(&view, 0, 20, ALLIANCE);

        let old = character_row(FRIEND, 1, 4, 20, 12, true, 0);
        let new = character_row(FRIEND, 1, 4, 20, 12, true, 700_000);
        character_online_changed(&view, &old, &new);

        assert!(
            friend_rx.try_recv().is_ok(),
            "the real login must be heard even though `online` never flipped"
        );
        assert!(
            view.take_character_online(FRIEND),
            "the login must be marked so the later claim close can find it"
        );
    }

    /// **The other half of the fix:** the online mark is one-shot and starts unset, so a claim
    /// that closes for a Character this Gateway never watched come online — a login that fails
    /// after `claim_session` succeeds, before `enter_world`/`player_login` ever runs — has nothing
    /// to consume. No Fake reaches a Coordinator-backed `account_claim_changed` call to prove the
    /// end-to-end "no notice at all" path for a failed login directly.
    #[test]
    fn take_character_online_answers_false_for_a_login_it_never_observed() {
        let view = WorldView::new(true);
        assert!(
            !view.take_character_online(FRIEND),
            "nothing marked this guid online, so there is nothing to consume"
        );
    }
}

#[cfg(test)]
mod group_event_order_tests {
    use super::{GroupEvent, GroupEventCursor, GroupEventSnapshot, GroupEventSource};

    const RECIPIENT: u64 = 7;

    fn event(id: u64, recipient_guid: u64, created_micros: i64) -> GroupEvent {
        GroupEvent {
            id,
            recipient_identity: spacetimedb_sdk::Identity::ZERO,
            kind: 0,
            other_guid: 0,
            other_name: String::new(),
            created_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(created_micros),
            payload: String::new(),
            recipient_guid,
        }
    }

    fn ids(rows: &[GroupEvent]) -> Vec<u64> {
        rows.iter().map(|row| row.id).collect()
    }

    /// One transaction wrote rows 10 to 12 for the recipient and row 13 for someone else. Row 5 is
    /// an older transaction's. The SDK fires the inserts in hash order; the recipient still gets
    /// 10, 11, 12 once, in that order.
    #[test]
    fn the_first_callback_of_a_transaction_relays_its_rows_in_insert_order() {
        let cached = [
            event(5, RECIPIENT, 100),
            event(12, RECIPIENT, 200),
            event(10, RECIPIENT, 200),
            event(13, 8, 200),
            event(11, RECIPIENT, 200),
        ];
        let cursor = GroupEventCursor::default();
        let source = GroupEventSource::RealmCore;

        let first = cursor.take(source, &cached[1], || cached.to_vec());
        let later: Vec<GroupEvent> = [&cached[2], &cached[4]]
            .into_iter()
            .flat_map(|row| {
                cursor.take(source, row, || -> Vec<GroupEvent> {
                    panic!("a row already taken must not read the table again")
                })
            })
            .collect();

        assert_eq!(ids(&first), [10, 11, 12]);
        assert!(later.is_empty());
    }

    /// The next transaction starts after the cursor, even with the same timestamp, and another
    /// database's ids are counted apart.
    #[test]
    fn each_source_database_keeps_its_own_cursor() {
        let cursor = GroupEventCursor::default();
        let realm = [event(10, RECIPIENT, 200), event(11, RECIPIENT, 200)];
        cursor.take(GroupEventSource::RealmCore, &realm[0], || realm.to_vec());

        let next = [
            event(10, RECIPIENT, 200),
            event(11, RECIPIENT, 200),
            event(14, RECIPIENT, 200),
        ];
        let shard = [event(3, RECIPIENT, 200)];

        assert_eq!(
            ids(&cursor.take(GroupEventSource::RealmCore, &next[2], || next.to_vec())),
            [14]
        );
        assert_eq!(
            ids(&cursor.take(GroupEventSource::Shard(0), &shard[0], || shard.to_vec())),
            [3]
        );
    }

    /// A Raid roster list writes two rows for each of 40 members in one transaction. The SDK
    /// fires an insert callback per row; the table is read once, and every member relays both of
    /// its rows once, oldest first.
    #[test]
    fn one_transaction_reads_the_table_once_for_every_recipient() {
        let members = 40_u64;
        let transaction: Vec<GroupEvent> = (0..members)
            .flat_map(|member| {
                let guid = 100 + member;
                [
                    event(1000 + member * 2, guid, 200),
                    event(1001 + member * 2, guid, 200),
                ]
            })
            .collect();
        let mut table = vec![event(5, 100, 100)];
        table.extend(transaction.iter().cloned());

        let mut snapshot = GroupEventSnapshot::default();
        let cursors: Vec<GroupEventCursor> =
            (0..members).map(|_| GroupEventCursor::default()).collect();
        let reads = std::cell::Cell::new(0);
        let mut relayed: Vec<Vec<Vec<u64>>> = vec![Vec::new(); members as usize];
        for row in transaction.iter().rev() {
            let member = (row.recipient_guid - 100) as usize;
            let rows = snapshot.take(&cursors[member], GroupEventSource::RealmCore, row, || {
                reads.set(reads.get() + 1);
                table.clone()
            });
            relayed[member].push(ids(&rows));
        }

        assert_eq!(reads.get(), 1);
        for (member, calls) in relayed.iter().enumerate() {
            let first = 1000 + member as u64 * 2;
            assert_eq!(calls, &[vec![first, first + 1], vec![]]);
        }
    }

    /// A row the snapshot lacks starts the next transaction, even with the same timestamp.
    #[test]
    fn the_next_transaction_reads_the_table_again() {
        let table = [
            event(10, RECIPIENT, 200),
            event(11, RECIPIENT, 200),
            event(12, RECIPIENT, 200),
        ];
        let mut snapshot = GroupEventSnapshot::default();
        let cursor = GroupEventCursor::default();
        let reads = std::cell::Cell::new(0);
        let mut take = |row: &GroupEvent, visible: usize| {
            ids(
                &snapshot.take(&cursor, GroupEventSource::RealmCore, row, || {
                    reads.set(reads.get() + 1);
                    table[..visible].to_vec()
                }),
            )
        };

        assert_eq!(take(&table[0], 2), [10, 11]);
        assert!(take(&table[1], 2).is_empty());
        assert_eq!(take(&table[2], 3), [12]);
        assert_eq!(reads.get(), 2);
    }
}

// ===============================================================================================
//  The pump→writer hand-off
// ===============================================================================================

/// Queue work for one World Session. The socket writer checks its lifetime before running it.
/// Jobs already running finish before the destination entry batch on the same FIFO writer;
/// jobs selected by an old callback but enqueued after that batch are discarded here.
pub(crate) fn enqueue(
    viewer: Arc<Viewer>,
    job: impl FnOnce(Arc<Viewer>) -> Vec<Outbound> + Send + 'static,
) {
    let tx = viewer.tx.clone();
    let _ = tx.send(Outbound::Job(Box::new(move || {
        if !viewer.active.load(Ordering::Acquire) {
            return Vec::new();
        }
        job(viewer)
    })));
}

// ===============================================================================================
//  Viewer recentre — the session-thread half
// ===============================================================================================

/// Move a session's anchor and relay the visibility delta the move implies: a CREATE for everything
/// that entered the box, a DESTROY for everything that left it.
pub(crate) fn recenter(view: &Arc<WorldView>, viewer: &Arc<Viewer>, map_id: u32, x: f32, y: f32) {
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
            enqueue(viewer.clone(), move |viewer| {
                crate::codec::entity::subscriptions::relay_peer_destroy(&viewer, guid, owner_guid)
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
            let (view, viewer, row) = (view.clone(), viewer.clone(), Arc::new(row));
            enqueue(viewer.clone(), move |viewer| {
                crate::codec::entity::subscriptions::offer_peer_create_for(
                    &coord, &view, shard, &viewer, &row,
                )
            });
        }
    }
    {
        for (guid, _) in delta.game_objects.left {
            let viewer = viewer.clone();
            enqueue(viewer.clone(), move |viewer| {
                crate::codec::gameobject::subscriptions::relay_gameobject_destroy(&viewer, guid)
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
            enqueue(viewer.clone(), move |viewer| {
                crate::codec::gameobject::subscriptions::relay_gameobject_create(
                    &coord, &viewer, &row,
                )
            });
        }
    }
}

/// Offer resident rows in one writer job. Reading the AOI and caches inside the job keeps a
/// queued DESTROY from being followed by a CREATE built from an earlier resident snapshot.
/// The writer sends every CREATE and its follow-up VALUES before running another Relay.
pub(crate) fn sweep_into_view(view: &Arc<WorldView>, viewer: &Arc<Viewer>) {
    let view = view.clone();
    enqueue(viewer.clone(), move |viewer| {
        let mut out = Vec::new();
        if let Some(shard) = view
            .spatial
            .shard_of(EntityLayer::WorldEntity, viewer.self_guid)
        {
            if let Some(coord) = view.shard(shard) {
                out.extend(crate::codec::combat::subscriptions::aura_snapshot_outbound(
                    &coord,
                    &view,
                    shard,
                    &viewer,
                    viewer.self_guid,
                    &[],
                ));
            }
        }
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
            out.extend(crate::codec::entity::subscriptions::offer_peer_create_for(
                &coord, &view, shard, &viewer, &row,
            ));
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
            out.extend(
                crate::codec::gameobject::subscriptions::relay_gameobject_create(
                    &coord, &viewer, &row,
                ),
            );
        }

        // The login sequence queued self CREATE before registration, so the resident flight
        // follows it on this writer too.
        for coord in view.shards.read().unwrap().iter() {
            out.extend(
                crate::codec::taxi::subscriptions::resident_taxi_spline_outbound(
                    coord,
                    &viewer,
                    viewer.self_guid,
                ),
            );
        }
        // Only the Home Shard can hold this Character's countdown. Another Shard's row would be a
        // Transfer source's delete still on its way.
        if let Some(coord) = view
            .shard_of_viewer(viewer.session)
            .and_then(|shard| view.shard(shard))
        {
            out.extend(resident_instance_removal_outbound(
                &view,
                &coord,
                viewer.self_guid,
            ));
        }
        out
    });
}

/// Replace this Shard's rows between SDK messages. Its old pump has stopped, and other
/// Shards retain their current rows.
fn seed_shard_from_cache(view: &Arc<WorldView>, shard: ShardId, db: &RemoteTables) {
    reconcile_shard(
        view,
        shard,
        db.game_world_entity().iter().collect(),
        db.game_gameobject().iter().collect(),
        db.game_aura().iter().collect(),
        db.game_character_contact().iter().collect(),
    );
}

fn reconcile_shard(
    view: &Arc<WorldView>,
    shard: ShardId,
    entities: Vec<WorldEntity>,
    objects: Vec<GameObject>,
    auras: Vec<Aura>,
    contacts: Vec<ContactEntry>,
) {
    replace_contact_sets(view, shard, &contacts);
    let aura_targets = view.auras.replace_shard(shard, auras);
    let removed_entities = view.spatial.replace_shard(
        EntityLayer::WorldEntity,
        shard,
        entities
            .iter()
            .map(|row| (row.guid, entity_key(row), row.owner_guid)),
    );
    let removed_objects = view.spatial.replace_shard(
        EntityLayer::GameObject,
        shard,
        objects.iter().map(|row| {
            (
                row.guid,
                CellKey::at(row.map_id, row.instance_id, row.grid_x, row.grid_y),
                0,
            )
        }),
    );
    for (layer, removed) in [
        (EntityLayer::WorldEntity, removed_entities),
        (EntityLayer::GameObject, removed_objects),
    ] {
        for (guid, key, owner_guid) in removed {
            for viewer in view.cell_audience(shard, Some(key), BOX_HALF_SPAN, &[]) {
                if view.spatial.can_see(layer, viewer.session, guid) {
                    continue;
                }
                enqueue(viewer.clone(), move |viewer| match layer {
                    EntityLayer::WorldEntity => {
                        crate::codec::entity::subscriptions::relay_peer_destroy(
                            &viewer, guid, owner_guid,
                        )
                    }
                    EntityLayer::GameObject => {
                        crate::codec::gameobject::subscriptions::relay_gameobject_destroy(
                            &viewer, guid,
                        )
                    }
                });
            }
        }
    }
    if let Some(coord) = view.shard(shard) {
        for (target_guid, previous) in aura_targets {
            let previous = Arc::new(previous);
            let key =
                view.spatial
                    .entity_cell_on_shard(EntityLayer::WorldEntity, target_guid, shard);
            for viewer in view.cell_audience(shard, key, BOX_HALF_SPAN, &[target_guid]) {
                let (coord, view, previous) = (coord.clone(), view.clone(), previous.clone());
                enqueue(viewer.clone(), move |viewer| {
                    crate::codec::combat::subscriptions::aura_snapshot_outbound(
                        &coord,
                        &view,
                        shard,
                        &viewer,
                        target_guid,
                        &previous,
                    )
                });
            }
        }
    }
    // Existing WorldEntity rows retain their CREATE dedup state.
    for row in entities {
        if view.spatial.shard_of(EntityLayer::WorldEntity, row.guid) != Some(shard) {
            continue;
        }
        let row = Arc::new(row);
        for session in view.world_entity_recipients(shard, row.guid, entity_key(&row)) {
            if view
                .viewer(session)
                .is_some_and(|v| v.created.lock().unwrap().contains(&row.guid))
            {
                continue;
            }
            offer_create_job(view, shard, session, &row);
        }
    }
    // Refresh resident GameObjects too: state and rotation may have changed while disconnected.
    // Their CREATE packets do not use the WorldEntity dedup set.
    for row in objects {
        if view.spatial.shard_of(EntityLayer::GameObject, row.guid) == Some(shard) {
            gameobject_appeared(view, shard, &row);
        }
    }
    let (aura_rows, aura_units) = view.auras.stats();
    log::info!(
        "shared AOI shard {shard} reconciled; {aura_rows} auras on {aura_units} shard-local units"
    );
}
