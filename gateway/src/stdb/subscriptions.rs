//! World Session registration, queue metrics and durable background work.

use crate::codec;
#[cfg(test)]
use crate::codec::auction::subscriptions::auction_notice_outbound;
#[cfg(test)]
use crate::codec::combat::subscriptions::cast_event_outbound;
#[cfg(test)]
use crate::codec::combat::subscriptions::combat_event_outbound;
#[cfg(test)]
use crate::codec::combat::subscriptions::stealth_action;
#[cfg(test)]
use crate::codec::combat::subscriptions::StealthAction;
#[cfg(test)]
use crate::codec::combat::subscriptions::A_STEALTH;
use crate::codec::corpse::subscriptions::relay_corpse_create;
#[cfg(test)]
use crate::codec::duel::subscriptions::duel_event_outbound;
#[cfg(test)]
use crate::codec::entity::subscriptions::append_encounter_equip_after_create;
#[cfg(test)]
use crate::codec::entity::subscriptions::encounter_equip_outbound;
#[cfg(test)]
use crate::codec::entity::subscriptions::entity_update_to_outbound;
#[cfg(test)]
use crate::codec::entity::subscriptions::entity_update_to_outbound_with_dynamic_flags;
#[cfg(test)]
use crate::codec::entity::subscriptions::ghost_transition;
use crate::codec::entity::subscriptions::instance_relay_gate;
#[cfg(test)]
use crate::codec::entity::subscriptions::is_update_reentry;
#[cfg(test)]
use crate::codec::entity::subscriptions::peer_create_gate;
#[cfg(test)]
use crate::codec::entity::subscriptions::peer_create_outbound;
#[cfg(test)]
use crate::codec::entity::subscriptions::peer_entity_view;
#[cfg(test)]
use crate::codec::item::subscriptions::append_final_item_slots;
#[cfg(test)]
use crate::codec::item::subscriptions::enchant_change_outbound;
#[cfg(test)]
use crate::codec::item::subscriptions::trade_event_outbound;
#[cfg(test)]
use crate::codec::movement::subscriptions::append_resident_creature_after_create;
#[cfg(test)]
use crate::codec::movement::subscriptions::build_teleport_relay;
#[cfg(test)]
use crate::codec::movement::subscriptions::creature_leg_outbound;
#[cfg(test)]
use crate::codec::movement::subscriptions::motion_outbound;
#[cfg(test)]
use crate::codec::social::subscriptions::chat_event_message;
#[cfg(test)]
use crate::codec::social::subscriptions::chat_in_range;
#[cfg(test)]
use crate::codec::social::subscriptions::nearby_chat_tag;
#[cfg(test)]
use crate::codec::social::subscriptions::system_message_event_outbound;
#[cfg(test)]
use crate::codec::social::subscriptions::SAY_RANGE_SQ;
#[cfg(test)]
use crate::codec::social::subscriptions::YELL_RANGE_SQ;
#[cfg(test)]
use crate::codec::taxi::subscriptions::append_resident_taxi_after_create;
#[cfg(test)]
use crate::codec::taxi::subscriptions::taxi_spline_outbound;
#[cfg(test)]
use crate::codec::CreateKind;
use crate::world::{Outbound, SessionTx};
use anyhow::{anyhow, Result};
use spacetimedb_sdk::Table;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
#[cfg(test)]
use wow_world_messages::vanilla::Vector3d;

use super::aoi::ViewerGates;
use super::bindings::*;
use super::connection::Coordinator;
#[cfg(test)]
use super::reads::view_of_item_row;
#[cfg(test)]
use super::views::entity_view;
use super::world_index::CellKey;
use super::world_view::{self, Viewer, WorldView};

#[cfg(test)]
pub(crate) static DURABLE_TOPOLOGY_ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard for one world session's shared-view registration, held by the world connection.
/// Dropping the guard makes the session unreachable from every shard-level dispatcher.
pub struct PlayerSubscriptions {
    /// This session's registration in the gateway-wide shared view. `None` only for the
    /// in-memory test store. Dropping the guard unregisters it, which is what stops the shared
    /// coordinator dispatch from ever enqueueing for a dead session.
    viewer: Option<Arc<Viewer>>,
    view: Option<Arc<WorldView>>,
}

impl PlayerSubscriptions {
    /// A no-op guard (used by the in-memory test store; drops harmlessly).
    #[cfg(test)]
    pub fn empty() -> Self {
        Self {
            viewer: None,
            view: None,
        }
    }

    /// Build a real shared-view registration for world-session ordering tests.
    #[cfg(test)]
    pub(crate) fn registered_for_test(
        view: Arc<WorldView>,
        self_guid: u64,
        arrival: &codec::EntityView,
        tx: SessionTx,
    ) -> Self {
        let (instance_id, map_id, x, y) =
            (arrival.instance_id, arrival.map_id, arrival.x, arrival.y);
        let mut identity = [0; 32];
        identity[..8].copy_from_slice(&self_guid.to_le_bytes());
        let viewer = Arc::new(Viewer {
            active: std::sync::atomic::AtomicBool::new(true),
            session: view.next_session_id(),
            self_guid,
            bound_identity: spacetimedb_sdk::Identity::from_byte_array(identity),
            instance_id,
            map_id,
            zone_id: arrival.zone_id.into(),
            tx,
            created: Arc::new(Mutex::new(HashSet::from([self_guid]))),
            gates: Arc::new(ViewerGates::default()),
            skill_slots: Arc::new(Mutex::new((std::collections::HashMap::new(), 0))),
            explored: Mutex::new(world_view::ExplorationReplay::default()),
            motion_pending: Arc::new(world_view::MotionPending::default()),
            member_stats: Default::default(),
            ignored: Mutex::default(),
            friends: Mutex::default(),
            team: lyracore_shared::faction::TEAM_ALLIANCE,
            group_events: Default::default(),
        });
        view.add_viewer_on_shard(
            viewer.clone(),
            CellKey::of_position(map_id, instance_id, x, y),
            0,
        );
        Self {
            viewer: Some(viewer),
            view: Some(view),
        }
    }

    /// Remove this session from owner, identity, shard, and spatial routing immediately.
    ///
    /// World-port uses this before driving a cross-shard transfer because the drive can delete
    /// owner rows on the source shard. `Drop` calls the same method, so failure and re-entry paths
    /// remain idempotent.
    pub(crate) fn unregister_viewer(&mut self) {
        if let (Some(view), Some(viewer)) = (self.view.take(), self.viewer.take()) {
            viewer
                .active
                .store(false, std::sync::atomic::Ordering::Release);
            view.remove_viewer(viewer.session);
        }
    }

    /// What this session's client holds of its group mates' Member Stats. `None` for the in-memory
    /// test store.
    pub(crate) fn member_stats_record(&self) -> Option<&crate::world::MemberStatsRecord> {
        self.viewer.as_ref().map(|viewer| &viewer.member_stats)
    }

    /// Drive the shared AOI index from the player's movement: on a cell crossing, move the
    /// viewer's anchor and relay the CREATE/DESTROY delta the move implies.
    ///
    /// Runs on the session's own thread and does no I/O: the delta is a 10-cell set diff in memory
    /// and the entering rows are read out of coordinator caches that are already resident. Before
    /// Before the shared-connection model, this was an unsubscribe + resubscribe round trip per
    /// cell crossing, per player.
    pub fn aoi_update(&mut self, x: f32, y: f32) {
        let (Some(viewer), Some(view)) = (self.viewer.as_ref(), self.view.as_ref()) else {
            return;
        };
        world_view::recenter(view, viewer, viewer.map_id, x, y);
    }
}

impl Drop for PlayerSubscriptions {
    fn drop(&mut self) {
        self.unregister_viewer();
    }
}

/// Diagnosis counters for a failure where the peer-movement relay died permanently on a session —
/// the motion subscription stopped delivering while the module kept accepting movement. Deliberately
/// atomics read ONCE at logout rather than a log per call: a
/// `log::info!` on the relay path made the intermittent failure vanish across six consecutive runs,
/// so any instrument that touches the hot path perturbs the very race being measured.
///
/// `MOTION_CALLS` counts per-viewer [`motion_outbound`] work before its audience Gate.
/// `MOTION_SENT` counts packets built, and `MOTION_DROPPED` counts discarded writer work.
/// These are fan-out counters. Incoming subscription rows are counted separately per Shard.
pub(crate) static MOTION_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static MOTION_SENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) static MOTION_DROPPED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

// One argument per counter in the diagnostic line this formats.
#[allow(clippy::too_many_arguments)]
/// The `MOTIONSTAT` 10-second line — pure, so what the operator reads is pinned by a
/// test instead of by eyeballing a `log::info!`. Deltas are the CHANGE over the last window, which is
/// what makes a degradation visible while the cumulative totals keep climbing.
///
/// `delivery` is `sent/calls` over the window: the relay's own guard-rejection rate. `fanout` is
/// `calls per submitted movement` — the observers-per-mover the subscription engine actually
/// delivered to. That second number is the one the 371-client run needed and did not have: at 371
/// co-located clients each submitted movement SHOULD wake ~370 relay callbacks, and the under-
/// delivery showed up as that ratio collapsing, not as a guard rejecting anything.
pub(crate) fn motionstat_line(
    calls: u64,
    sent: u64,
    dropped: u64,
    d_calls: u64,
    d_sent: u64,
    d_dropped: u64,
    d_submitted: u64,
    submitted: u64,
) -> String {
    let delivery = if d_calls > 0 {
        format!("{:.1}%", 100.0 * d_sent as f64 / d_calls as f64)
    } else {
        "--".to_string()
    };
    let fanout = match fanout_ratio(d_calls, d_submitted) {
        Some(f) => format!("{f:.1}"),
        None => "--".to_string(),
    };
    format!(
        "MOTIONSTAT calls={calls} sent={sent} dropped={dropped} \
         (+{d_calls} +{d_sent} +{d_dropped} in 10s) | delivery={delivery} fanout={fanout}/move \
         | move submitted={submitted}"
    )
}

/// Observers per submitted movement over one window — `None` for a window with no movement at all
/// (an idle server must not read as a fan-out of zero). ONE definition, shared by the `MOTIONSTAT`
/// line and the collapse check below, so the number the operator reads and the number the warning
/// fires on can never disagree.
pub(crate) fn fanout_ratio(d_calls: u64, d_submitted: u64) -> Option<f64> {
    (d_submitted > 0).then(|| d_calls as f64 / d_submitted as f64)
}

/// How far below its own trailing baseline the fan-out must fall to count as collapsed. 0.6 = "at
/// least 40 % of the observers a mover was reaching ten seconds ago are no longer being reached".
/// Well outside the wobble of a live crowd (players logging in and out, AOI boxes recentering, a
/// window that lands mid-teleport), so it takes an actual step change to reach.
pub(crate) const FANOUT_COLLAPSE_RATIO: f64 = 0.6;

/// Consecutive collapsed windows before the warning fires — 3 × 10 s, so ~30 s of sustained
/// under-delivery. THE HYSTERESIS IS A DELIBERATE DIVERGENCE FROM THE STUCK-RELAY SIGNAL (the
/// `MOTION_CALLS`/`MOTION_SENT` diagnosis above), which has none
/// and re-fires every window: that signal keys on "movement submitted but the relay callback fired almost
/// never", which is a binary, unambiguous, does-not-self-heal condition worth shouting about
/// instantly. Fan-out is a RATIO of two noisy counters over a 10-second window, and it moves for
/// legitimate reasons every time the crowd changes shape. A signal like that fires spuriously if it
/// is instant, and a warning that has cried wolf once is a warning the operator greps away.
pub(crate) const FANOUT_COLLAPSE_WINDOWS: u32 = 3;

/// After this many consecutive collapsed windows the current (low) fan-out is accepted as the new
/// baseline, which silences the warning until the NEXT collapse. Deliberate: fan-out also drops
/// permanently for the entirely legitimate reason that the crowd thinned, and an alarm that never
/// stops is an alarm nobody reads. So the operator gets windows 3..6 of the collapse — ~30 s of loud
/// warnings — and then the gateway adapts.
pub(crate) const FANOUT_REBASELINE_WINDOWS: u32 = 6;

/// Weight of the newest healthy window in the trailing baseline. An EWMA rather than a ring of
/// recent windows because it needs no allocation and no window bookkeeping, and because the
/// smoothing constant is the only tunable: 0.3 over 10-second windows ≈ a 30-second memory, fast
/// enough to follow a crowd filling up a zone and slow enough that one quiet window does not become
/// the reference point. Note the baseline is fed ONLY by non-collapsed windows (see
/// [`fanout_health_step`]) — an EWMA that absorbed the collapse would decay under the current
/// fan-out within two windows and the run would never reach [`FANOUT_COLLAPSE_WINDOWS`].
pub(crate) const FANOUT_BASELINE_ALPHA: f64 = 0.3;

/// Trailing fan-out baseline + the length of the current run of collapsed windows. Lives in the
/// 10-second `MOTIONSTAT` task; nothing on the relay hot path touches it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct FanoutHealth {
    /// `None` until the first informative window establishes it.
    pub(crate) baseline: Option<f64>,
    pub(crate) low_windows: u32,
}

/// One 10-second window of the fan-out health check — pure, so the decision is unit-tested directly
/// instead of being eyeballed in a log. Returns the next state, and
/// `Some(consecutive_collapsed_windows)` when this window should warn (`None` = stay quiet).
///
/// **What it catches.** Peer movement being delivered to fewer observers than it was a moment ago,
/// while players are still moving — the 371-client shape, where nothing errored, nothing logged, and
/// peers simply updated less often than they moved. `delivery` (= sent/calls) read ~100 % throughout
/// that run, because the shortfall was `calls` never reaching the rate it should have; `fanout` is
/// the field that showed it, and this turns that field from "a number an operator has to know the
/// expected value of" into a warning.
///
/// **What it does NOT catch — read this before trusting it.**
/// * **A gateway that has been degraded since startup.** The baseline is the gateway's OWN recent
///   behaviour, so if fan-out was already wrong when the first window was measured, that wrong value
///   IS the baseline and nothing ever trips. This check reports a CHANGE, never an absolute truth.
///   Nothing in the gateway currently knows how many observers a mover *should* have reached —
///   the AOI box is per-session, invisible to this task, and `GridBox` has no
///   overlap test, so a true expectation needs a global registry of per-session boxes plus box
///   intersection. That is the honest version of this warning and it is not what this is.
/// * **A slow slide.** A degradation that takes minutes to develop keeps each window within
///   [`FANOUT_COLLAPSE_RATIO`] of the last, and the baseline follows it down. Only a step change of
///   ≥40 % inside ~30 s trips.
/// * **A collapse that starts during a quiet spell.** Windows below
///   [`crate::world::MOVE_ACTIVITY_FLOOR`] submitted movements are treated as uninformative: they do
///   not warn, and they break a run in progress.
/// * **Under-delivery to SOME sessions.** Both counters are process-global, so one wedged session
///   among 300 healthy ones is invisible here.
///
/// A TOTAL loss of the relay trips this as well as the stuck-relay signal (fan-out goes to ~0, which is
/// certainly below any baseline) — two lines about one failure, 30 s apart, which is the right way
/// round: the stuck-relay signal is instant and keeps repeating, this one adds "and it stayed that way".
pub(crate) fn fanout_health_step(
    state: FanoutHealth,
    fanout: Option<f64>,
    submitted_delta: u64,
) -> (FanoutHealth, Option<u32>) {
    // Uninformative window: too little movement to compare, or none at all. Keep the baseline (a
    // quiet window is not evidence about fan-out) but break any run in progress — the same activity
    // floor the stuck-relay signal uses, for the same reason: a handful of movements with little relay
    // traffic is one lonely player, not a broken relay.
    let Some(f) = fanout.filter(|_| submitted_delta > crate::world::MOVE_ACTIVITY_FLOOR) else {
        return (
            FanoutHealth {
                low_windows: 0,
                ..state
            },
            None,
        );
    };
    let Some(baseline) = state.baseline else {
        // First informative window: nothing to compare against yet.
        return (
            FanoutHealth {
                baseline: Some(f),
                low_windows: 0,
            },
            None,
        );
    };
    if f >= baseline * FANOUT_COLLAPSE_RATIO {
        // Healthy window — fold it into the trailing baseline and forget any partial run.
        let next = baseline + FANOUT_BASELINE_ALPHA * (f - baseline);
        return (
            FanoutHealth {
                baseline: Some(next),
                low_windows: 0,
            },
            None,
        );
    }
    let low_windows = state.low_windows + 1;
    let warn = (low_windows >= FANOUT_COLLAPSE_WINDOWS).then_some(low_windows);
    if low_windows >= FANOUT_REBASELINE_WINDOWS {
        // Warned for ~30 s and it is still low: accept this as the new normal (see
        // FANOUT_REBASELINE_WINDOWS) — this window still warns, the next one will not.
        return (
            FanoutHealth {
                baseline: Some(f),
                low_windows: 0,
            },
            warn,
        );
    }
    (
        FanoutHealth {
            baseline: Some(baseline),
            low_windows,
        },
        warn,
    )
}

/// **Should this peer-motion packet be dropped instead of queued?** The one decision behind the
/// per-session egress bound (task B2) — pure and unit-tested, called with `tx.depth()`.
///
/// The channel behind [`crate::world::SessionTx`] is unbounded, so a session whose socket has
/// stopped taking bytes grows its backlog for as long as the stall lasts: a memory path under crowd
/// load, and the reason `MOTION_DROPPED` could not mean backpressure before this. Bounding the
/// CHANNEL is not available to us — ~110 of the send sites run inside SpacetimeDB SDK row callbacks
/// on the connection's pump thread, and a blocking send from there on a full channel would deadlock
/// that connection (see `SessionTx`'s own comment). So the queue stays unbounded and the only
/// traffic that can plausibly outrun a socket sheds itself.
///
/// **Why only peer motion and creature legs, out of ~180 send sites.** They are the N²-shaped
/// traffic: one packet per OBSERVER per mover per heartbeat, ~740/s per session at 371 co-located
/// clients, versus a handful per player action for everything else. And they are the only sends that
/// are SELF-HEALING when lost — a movement heartbeat is superseded by the next one 500 ms later, so
/// a shed packet costs a moment of coarser interpolation on a peer that is already being rendered
/// from stale data (the queue is half a second deep). A quest-log page, an item CREATE, a
/// `SMSG_DESTROY_OBJECT` is not superseded by anything: dropping one leaves the client permanently
/// wrong. Those keep queueing exactly as before.
///
/// Shedding is counted in [`MOTION_DROPPED`], so it is visible on the existing `MOTIONSTAT` line as
/// both a rising `dropped` and a falling `delivery` — a shed that fires is not silent.
pub(crate) fn shed_motion_at_depth(depth: usize) -> bool {
    depth > crate::world::EGRESS_SHED_DEPTH
}

impl Coordinator {
    pub fn spawn_party_command_relay(&self) {
        for shard in self.all_shards() {
            let polling_shard = shard.clone();
            if let Err(error) = std::thread::Builder::new()
                .name("party-command-dispatch".into())
                .spawn(move || loop {
                    polling_shard.dispatch_party_command_intents();
                    std::thread::sleep(Duration::from_millis(100));
                })
            {
                log::error!("could not start party command dispatcher: {error}");
            }
        }
    }

    fn dispatch_party_command_intents(&self) {
        const DISPATCH_LANE_LIMIT: usize = lyracore_shared::group::COMMAND_DISPATCH_LANES as usize;
        let pending = {
            let live = self.0.coord();
            let lanes: Vec<_> = live
                .conn
                .db
                .game_party_command_dispatch_lane()
                .iter()
                .take(DISPATCH_LANE_LIMIT + 1)
                .map(|lane| lane.head_intent_id)
                .collect();
            if lanes.len() > DISPATCH_LANE_LIMIT {
                if !self
                    .0
                    .party_command_lane_overflow
                    .swap(true, Ordering::AcqRel)
                {
                    log::error!(
                        "party command dispatcher found more than {DISPATCH_LANE_LIMIT} lanes; \
                         command dispatch on this Shard is stopped until the Module and Gateway \
                         lane counts agree"
                    );
                }
                return;
            }
            if self
                .0
                .party_command_lane_overflow
                .swap(false, Ordering::AcqRel)
            {
                log::info!("party command dispatcher lane count recovered on this Shard");
            }
            lanes
                .into_iter()
                .filter_map(|head_intent_id| {
                    live.conn
                        .db
                        .game_party_command_intent()
                        .id()
                        .find(&head_intent_id)
                        .map(|row| party_command_intent(&row))
                })
                .collect::<Vec<_>>()
        };
        for intent in pending {
            attempt_party_command(self, intent);
        }
    }

    /// Prepare and register one live viewer. Row callbacks are already armed once per shard in
    /// `world_view::arm_shard`; this method registers no callback of its own.
    pub fn subscribe_player_events(
        &self,
        account_id: u64,
        self_guid: u64,
        arrival: &codec::EntityView,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions> {
        self.watch_session(tx.clone());
        let (login_instance, login_map, login_zone, login_x, login_y) = (
            arrival.instance_id,
            arrival.map_id,
            arrival.zone_id,
            arrival.x,
            arrival.y,
        );
        // Ghost-only visibility constants (vanilla 1.12): a spirit-healer creature carries
        // UNIT_NPC_FLAGS SPIRITHEALER and is visible ONLY to a viewer in the GHOST player-flag state.
        // BUG FIX: was 0x4000 — but 0x4000 is REPAIR in cmangos 1.12 numbering (SPIRITHEALER is 0x20).
        // The wrong constant (a) failed to ghost-gate the real Spirit Healer (npc_flags 0x21) and
        // (b) wrongly ghost-gated the 39 armorers/quartermasters that carry REPAIR (0x4000), hiding
        // them from living players.
        let created = Arc::new(Mutex::new(HashSet::from([self_guid])));
        // The view-merge fast-follow gap: the one piece of viewer-owned state the away leg's
        // peer-visibility gate needs but cannot read off an away connection. Written by the ghost-flag
        // relay (this viewer's OWN row), read by every per-viewer relay body in this file — a
        // shared coordinator callback holds the cache of the shard that owns the ROW, which for a
        // cross-shard peer is not the shard that owns the VIEWER. See `ViewerGates`'s doc comment.
        let viewer_gates = Arc::new(ViewerGates::default());
        // This session's handle on the gateway-wide shared view. Minted HERE, at the top,
        // because relays registered further down capture it — the actual REGISTRATION (which makes
        // the shared dispatch start enqueueing for this session) happens near the end, once
        // everything that could still fail with `?` has succeeded.
        let view = self.world_view();
        let session = view.next_session_id();

        // ======================================================================================
        //  SKILL-UP — game_player_skill (insert/update)
        //  Live PLAYER_SKILL_INFO[slot] VALUES so a newly-learned or trained skill line updates the
        //  open pane without a relog.
        // ======================================================================================
        // LIVE skill pane relay: a game_player_skill INSERT (new line learned) or UPDATE
        // (skill-up/train) pushes the PLAYER_SKILL_INFO[slot] partial so the open pane moves and
        // the client prints its own "Your skill in X has increased to N." line — no relog. The
        // slot map is seeded from the SAME deterministic layout the login CREATE used
        // (codec::skill_slot_layout); a line learned mid-session appends at the next free slot,
        // exactly as the next login's CREATE will lay it out.
        let skill_slots: std::sync::Arc<
            std::sync::Mutex<(std::collections::HashMap<u32, u8>, u8)>,
        > = {
            let class_b = {
                let guard = self.0.coord();
                guard
                    .conn
                    .db
                    .game_world_entity()
                    .guid()
                    .find(&self_guid)
                    .map(|e| ((e.unit_bytes_0 >> 8) & 0xFF) as u8)
                    .unwrap_or(0)
            };
            let learned = self.player_skills(self_guid).unwrap_or_default();
            let layout = codec::skill_slot_layout(class_b, &learned);
            let map: std::collections::HashMap<u32, u8> = layout
                .iter()
                .enumerate()
                .map(|(i, &(sk, _, _))| (sk.as_int() as u32, i as u8))
                .collect();
            let next_free = layout.len() as u8;
            std::sync::Arc::new(std::sync::Mutex::new((map, next_free)))
        };

        let mut explored = world_view::ExplorationReplay::default();

        // The bound identity WITHOUT touching a per-player connection — synthetic under the
        // flag (what establish_session bound and the module stamps on event rows), the cached
        // connection's real identity otherwise. Identical values to the old `player.identity` read.
        // Stored with the viewer for identity-addressed shared dispatch.
        let self_identity =
            spacetimedb_sdk::Identity::from_byte_array(self.bound_identity(account_id)?);

        {
            // Seed the viewer's replay state before registration. A coordinator reconnect replays
            // every resident row: exploration is state-sync (the fog word must re-send, the
            // discovery feedback must not repeat), so it carries this per-viewer seed, while the
            // event families (teleport, faction announce, item toast) drop replayed rows outright
            // in `world_view`'s live-insert registrations.
            let guard = self.0.coord();
            explored.seed(
                guard
                    .conn
                    .db
                    .game_character_explored()
                    .iter()
                    .filter(|r| r.character_guid == self_guid)
                    .map(|r| r.area_bit),
            );
        }

        // Register with the gateway-wide shared view, then sweep everything already inside
        // the box into this client's world.
        //
        // The sweep is not belt-and-braces here, it is the ONLY thing that populates a fresh
        // client: there is no subscription apply to fire per-row `on_insert` any more — the rows
        // were resident in the coordinator caches long before this session existed. It is
        // idempotent through `created` regardless.
        // Seed the ghost mirror from the character's OWN row before anything can read it. Before
        // the shared-connection model, the spirit-healer gate re-read the live row on every offer,
        // so a player who logged in
        // ALREADY DEAD saw the healer without any ghost transition ever firing; the mirror has to
        // start at the truth or that login regresses.
        {
            const GHOST_PLAYER_FLAG: u32 = lyracore_shared::constants::player_flags::GHOST;
            let is_ghost = self
                .0
                .coord()
                .conn
                .db
                .game_world_entity()
                .guid()
                .find(&self_guid)
                .map(|e| e.player_flags & GHOST_PLAYER_FLAG != 0)
                .unwrap_or(false);
            viewer_gates
                .is_ghost
                .store(is_ghost, std::sync::atomic::Ordering::Relaxed);
        }
        // The Realm Chat Relay's per-listener filter, and the Account Claim Relay's per-listener
        // audience: this Character's own ignore and friend guids, from its contact rows on this
        // Home Shard. The contact Relay keeps both current from here on. Read from the Shard's
        // owner-keyed `ContactIndex` rather than `contact_lists`, which reads the registered Viewer
        // this call is about to seed.
        let (ignored, friends): (HashSet<u64>, HashSet<u64>) = {
            let guard = self.0.coord();
            let contacts = guard.contacts.read().unwrap_or_else(|p| p.into_inner());
            (
                contacts.ignored_by(self_guid).into_iter().collect(),
                contacts.friends_of(self_guid).into_iter().collect(),
            )
        };
        let team = lyracore_shared::faction::team_for_race((arrival.unit_bytes_0 & 0xFF) as u8);
        let viewer = Arc::new(Viewer {
            active: std::sync::atomic::AtomicBool::new(true),
            session,
            self_guid,
            bound_identity: self_identity,
            instance_id: login_instance,
            map_id: login_map,
            zone_id: login_zone.into(),
            tx: tx.clone(),
            created: created.clone(),
            gates: viewer_gates.clone(),
            skill_slots: skill_slots.clone(),
            explored: Mutex::new(explored),
            motion_pending: Arc::new(world_view::MotionPending::default()),
            member_stats: Default::default(),
            ignored: Mutex::new(ignored),
            friends: Mutex::new(friends),
            team,
            group_events: Default::default(),
        });
        view.add_viewer(
            self,
            viewer.clone(),
            CellKey::of_position(login_map, login_instance, login_x, login_y),
        )?;
        world_view::sweep_into_view(&view, &viewer);

        // Corpse resident sweep: corpse rows ride the base subscription
        // and their callbacks may have fired before this point. Re-offer every resident one through
        // the same instance gate — idempotent for the client (a repeat CREATE for the same corpse
        // guid re-renders it), and the gate is now the session's authoritative `login_instance`
        // rather than a cache lookup that could answer `None`.
        //
        // Read from the COORDINATOR cache: same global rows in both flag states,
        // while the per-player cache stops carrying the corpse query under the flag.
        let resident_corpses: Vec<Corpse> = {
            let guard = self.0.coord();
            guard.conn.db.game_corpse().iter().collect()
        };
        for row in &resident_corpses {
            if instance_relay_gate(row.instance_id, Some(login_instance)) {
                relay_corpse_create(&tx, self_guid, row);
            }
        }
        // Armor/sheet resident sweep (login-low-armor live find, 2026-07-11): the base sub's ITEM
        // and aura replay callbacks fire before the self entity is readable, so `effective_armor`'s
        // base term read 0 and their pushes OVERWROTE the CREATE's correct gear-folded armor. Push
        // the authoritative values once, from the COORDINATOR's cache — which holds the entity, the
        // auras, the item instances AND the item templates, i.e. every term of the fold (the
        // per-player cache lost `game_item_template` to the connection reclaim and has never held
        // the entity since the shared-connection model).
        {
            let guard = self.0.coord();
            let db = &guard.conn.db;
            let eff = super::armor::effective_resistances(db, self_guid);
            let pos = super::armor::aura_armor_positive(db, self_guid);
            let sheet = super::armor::sheet_stats(db, self_guid);
            drop(guard);
            let _ = tx.send(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                Box::new(codec::build_armor_values(self_guid, eff, pos)),
            )));
            if let Some(st) = sheet {
                let _ = tx.send(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                    Box::new(codec::build_sheet_stats_values(self_guid, &st)),
                )));
            }
        }
        // Login fog restore: with no per-player subscription there
        // is no initial-sync replay to re-send the fog words, so sweep them explicitly from the
        // coordinator cache — one idempotent full-word PLAYER_EXPLORED_ZONES VALUES per explored
        // 32-bit bucket of THIS character.
        {
            let guard = self.0.coord();
            let mut words: std::collections::HashMap<u16, u32> = std::collections::HashMap::new();
            for r in guard.conn.db.game_character_explored().iter() {
                if r.character_guid != self_guid || r.area_bit < 0 {
                    continue;
                }
                *words.entry((r.area_bit / 32) as u16).or_default() |= 1u32 << (r.area_bit % 32);
            }
            drop(guard);
            for (word_idx, word) in words {
                let (opcode, body) = codec::build_explored_zones_values(self_guid, word_idx, word);
                let _ = tx.send(Outbound::Raw { opcode, body });
            }
        }

        Ok(PlayerSubscriptions {
            viewer: Some(viewer),
            view: Some(view),
        })
    }
}

fn party_command_intent(row: &PartyCommandIntent) -> crate::world::party::PartyCommandIntent {
    crate::world::party::PartyCommandIntent {
        id: row.id,
        source_identity: row.source_identity,
        issuer_guid: row.issuer_guid,
        issuer_sequence: row.issuer_sequence,
        kind: row.command.kind,
        bot_guid: row.command.bot_guid,
        authority_member_guid: row.command.authority_member_guid,
        exact_target_guid: row.command.exact_target_guid,
        expires_micros: row.expires_micros,
    }
}

fn attempt_party_command(store: &Coordinator, intent: crate::world::party::PartyCommandIntent) {
    let claim_token = next_party_command_claim_token();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as i64;
    if now >= intent.expires_micros {
        if let Err(error) =
            crate::world::party::finish_expired_party_command_intent(store, &intent, claim_token)
        {
            log::debug!("party command intent {} expiry retry: {error}", intent.id);
            let _ = store.defer_party_command_intent(intent.id, claim_token);
        }
        return;
    }
    match crate::world::party::run_party_command_intent(store, &intent, claim_token) {
        Ok(crate::world::party::CompanionCommandOutcome::WaitingForCapacity) => {
            if let Err(error) = store.defer_party_command_intent(intent.id, claim_token) {
                log::debug!("party command intent {} defer: {error}", intent.id);
            }
        }
        Ok(_) => {}
        Err(error) => {
            log::debug!("party command intent {} retry: {error}", intent.id);
            let _ = store.defer_party_command_intent(intent.id, claim_token);
        }
    }
}

fn next_party_command_claim_token() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    (time ^ NEXT.fetch_add(1, Ordering::Relaxed)).max(1)
}

impl Coordinator {
    /// Wire up the bot-initiated (serendipity) invite relay: one registration per
    /// connected WORLD SHARD — `all_shards()`, not a per-player relay — because there is no player
    /// session to hang this off. A bot's goal tick decides "invite this fellow quester" with no
    /// client behind it, so nothing else in the gateway would ever notice the row.
    ///
    /// Called ONCE, at gateway startup (`main.rs`). Also installs its own reconnect hook, parallel
    /// to `world_view::arm_shard`, so a module republish binds the callback to the fresh `LiveConn`.
    pub fn spawn_bot_invite_relay(&self) {
        for shard in self.all_shards() {
            shard.arm_bot_invite_relay();
            let hook_shard = shard.clone();
            shard
                .0
                .on_reconnect
                .lock()
                .unwrap()
                .push(std::sync::Arc::new(move || {
                    hook_shard.arm_bot_invite_relay();
                }));
        }
    }

    /// One shard's half of [`spawn_bot_invite_relay`] — pulled out so both the initial call and the
    /// watchdog's post-reconnect re-arm run the IDENTICAL registration, rather than two copies that
    /// could drift. Re-reads `self.0.coord()` fresh each call, which is the whole point: called again
    /// after a reconnect, it registers on the NEW `LiveConn`, not the dead one the old callback was
    /// bound to.
    fn arm_bot_invite_relay(&self) {
        let store = self.clone();
        self.0
            .coord()
            .conn
            .db
            .game_bot_invite_intent()
            .on_insert(move |_ctx, row| {
                let store = store.clone();
                let (intent_id, op, inviter_guid, target_guid) =
                    (row.id, row.op, row.inviter_guid, row.target_guid);
                // The claim targets this same Shard. Run it off the SDK callback thread so a
                // one-pipe Gateway can receive the reducer completion instead of blocking its own
                // connection pump.
                let spawned = std::thread::Builder::new()
                    .name("bot-invite-intent".into())
                    .spawn(move || {
                        match crate::world::party::run_bot_invite_intent(
                            &store,
                            intent_id,
                            op,
                            inviter_guid,
                            target_guid,
                        ) {
                            Ok(crate::world::party::PartyOutcome::Ran) => {}
                            outcome => log::debug!(
                                "group intent {intent_id} op {op} ({inviter_guid} -> \
                                 {target_guid}) did not execute: {outcome:?}"
                            ),
                        }
                    });
                if let Err(error) = spawned {
                    log::error!(
                        "could not start consumer for group intent {intent_id} op {op} \
                         ({inviter_guid} -> {target_guid}): {error}"
                    );
                }
            });
    }

    /// Start the Roster Revision Relay: Realm-core's `game_group_roster_revision` callbacks mark a
    /// party dirty, and one worker pushes it to every stale World Shard mirror. Called once at
    /// startup. Arms nothing unless Realm-core is a distinct database. A Realm-core reconnect
    /// re-arms the callbacks on the fresh connection; changes missed while it was down are the
    /// reconciliation pass's, which that reconnect also starts.
    pub fn spawn_roster_revision_relay(&self) {
        let realm = match self.realm_core() {
            Ok(realm) => realm,
            Err(error) => {
                log::error!(
                    "party: the Roster Revision Relay is not running; Realm-core is unavailable at \
                     startup ({error:#}). World Shard mirrors follow only party ops and the \
                     reconnect pass until the Gateway restarts"
                );
                return;
            }
        };
        if self
            .all_shards()
            .iter()
            .any(|shard| shard.shard_name() == realm.shard_name())
        {
            return;
        }
        let relay = match crate::world::party_mirror::RosterRevisionRelay::spawn(self.clone()) {
            Ok(relay) => relay,
            Err(error) => {
                log::error!("party: could not start the Roster Revision Relay: {error}");
                return;
            }
        };
        realm.arm_roster_revision_relay(&relay);
        let hook_realm = realm.clone();
        realm
            .0
            .on_reconnect
            .lock()
            .unwrap()
            .push(std::sync::Arc::new(move || {
                hook_realm.arm_roster_revision_relay(&relay);
            }));
    }

    /// Realm-core's half of [`Self::spawn_roster_revision_relay`], on the current connection. The
    /// callbacks only mark the party dirty; the worker makes every reducer call.
    fn arm_roster_revision_relay(
        &self,
        relay: &std::sync::Arc<crate::world::party_mirror::RosterRevisionRelay>,
    ) {
        use spacetimedb_sdk::TableWithPrimaryKey;
        let live = self.0.coord();
        let inserted = relay.clone();
        live.conn
            .db
            .game_group_roster_revision()
            .on_insert(move |_ctx, row| inserted.mark_dirty(row.group_id));
        let updated = relay.clone();
        live.conn
            .db
            .game_group_roster_revision()
            .on_update(move |_ctx, _old, row| updated.mark_dirty(row.group_id));
    }

    /// Start one bounded Transfer Intent dispatcher per configured World Shard. Each pass reads the
    /// current connection, so a reconnect needs no second thread or connection-scoped callback.
    pub fn spawn_bot_transfer_relay(&self) {
        for shard in self.all_shards() {
            if let Err(error) = std::thread::Builder::new()
                .name("bot-transfer-dispatch".into())
                .spawn(move || {
                    let mut after_id = 0;
                    loop {
                        after_id = shard.dispatch_bot_transfer_intents(after_id);
                        std::thread::sleep(Duration::from_millis(100));
                    }
                })
            {
                log::error!("could not start bot Transfer dispatcher: {error}");
            }
        }
    }

    fn dispatch_bot_transfer_intents(&self, after_id: u64) -> u64 {
        use lyracore_shared::transfer::BOT_TRANSFER_PENDING_LIMIT;
        const BOT_TRANSFER_WORK_LIMIT: usize = 4;
        let mut pending = {
            let live = self.0.coord();
            live.conn
                .db
                .game_bot_transfer_intent()
                .iter()
                .take(BOT_TRANSFER_PENDING_LIMIT + 1)
                .collect::<Vec<_>>()
        };
        if pending.len() > BOT_TRANSFER_PENDING_LIMIT {
            if !self
                .0
                .bot_transfer_pending_overflow
                .swap(true, Ordering::AcqRel)
            {
                log::error!(
                    "bot Transfer dispatcher found more than {BOT_TRANSFER_PENDING_LIMIT} pending \
                     rows; this populated state predates the bounded writer Gate"
                );
            }
            return after_id;
        }
        if self
            .0
            .bot_transfer_pending_overflow
            .swap(false, Ordering::AcqRel)
        {
            log::info!("bot Transfer dispatcher pending count recovered on this Shard");
        }
        pending.sort_by_key(|intent| intent.id);
        rotate_transfer_work(&mut pending, after_id, |intent| intent.id);
        let mut next_after = after_id;
        for row in pending.into_iter().take(BOT_TRANSFER_WORK_LIMIT) {
            next_after = row.id;
            let intent = crate::world::transfer::BotTransferIntent {
                id: row.id,
                bot_guid: row.bot_guid,
                destination_map: row.destination_map,
                destination_instance: row.destination_instance,
                reason: row.reason,
                created_micros: row.created_at.to_micros_since_unix_epoch(),
                controller_generation: row.controller_generation,
                arrival_ready: row.arrival_ready,
                source_module_identity: row.source_module_identity,
                source_map: row.source_map,
                source_instance: row.source_instance,
                source_locator_revision: row.source_locator_revision,
            };
            attempt_bot_transfer(self, &intent);
        }
        next_after
    }

    /// Reconcile party and guild membership when a World Shard deletes a Character.
    ///
    /// Fresh Character subscriptions on every Shard distinguish deletion from a Transfer whose
    /// destination update has not reached this Gateway yet.
    pub fn spawn_character_gone_relay(&self) {
        for shard in self.all_shards() {
            shard.arm_character_gone_relay();
            let hook_shard = shard.clone();
            shard
                .0
                .on_reconnect
                .lock()
                .unwrap()
                .push(std::sync::Arc::new(move || {
                    hook_shard.arm_character_gone_relay();
                    hook_shard.request_deleted_character_reconciliation();
                }));
        }
        if self.is_sharded() {
            if let Ok(realm) = self.realm_core() {
                let reconciliation_store = self.clone();
                realm
                    .0
                    .on_reconnect
                    .lock()
                    .unwrap()
                    .push(std::sync::Arc::new(move || {
                        reconciliation_store.request_deleted_character_reconciliation();
                    }));
            }
        }
        self.request_deleted_character_reconciliation();
    }

    /// A full pass: every party member and every Character guild state names. Startup and a
    /// reconnect ask for it, since a delete may have gone unseen.
    fn request_deleted_character_reconciliation(&self) {
        self.1.guild_cleanup.lock().unwrap().sweep = true;
        self.start_deleted_character_reconciliation();
    }

    /// A `game_character` delete: the party pass, and guild cleanup for `character_guid` only.
    fn request_deleted_character_cleanup(&self, character_guid: u64) {
        self.1
            .guild_cleanup
            .lock()
            .unwrap()
            .deleted
            .insert(character_guid);
        self.start_deleted_character_reconciliation();
    }

    fn start_deleted_character_reconciliation(&self) {
        self.1
            .deleted_character_reconciliation_requested
            .store(true, Ordering::Release);
        if self
            .1
            .deleted_character_reconciliation_running
            .swap(true, Ordering::AcqRel)
        {
            return;
        }
        let store = self.clone();
        let spawned = std::thread::Builder::new()
            .name("reconcile-deleted-characters".into())
            .spawn(move || {
                let mut retry_delay = Duration::from_millis(100);
                loop {
                    store
                        .1
                        .deleted_character_reconciliation_requested
                        .store(false, Ordering::Release);
                    // Both run on every pass: a party failure must not hold up guild cleanup.
                    let guild_work = std::mem::take(&mut *store.1.guild_cleanup.lock().unwrap());
                    let parties = crate::world::party::reconcile_deleted_character_parties(&store);
                    let guilds =
                        crate::world::reconcile_deleted_guild_characters(&store, &guild_work);
                    if guilds.is_err() {
                        store.1.guild_cleanup.lock().unwrap().merge(guild_work);
                    }
                    if let Err(error) = parties.and(guilds) {
                        log::warn!(
                            "deleted Character reconciliation deferred ({error:#}); retrying in \
                             {retry_delay:?}"
                        );
                        store
                            .1
                            .deleted_character_reconciliation_requested
                            .store(true, Ordering::Release);
                        std::thread::sleep(retry_delay);
                        retry_delay = (retry_delay * 2).min(Duration::from_secs(5));
                        continue;
                    } else {
                        retry_delay = Duration::from_millis(100);
                    }

                    if store
                        .1
                        .deleted_character_reconciliation_requested
                        .load(Ordering::Acquire)
                    {
                        continue;
                    }
                    store
                        .1
                        .deleted_character_reconciliation_running
                        .store(false, Ordering::Release);
                    if !store
                        .1
                        .deleted_character_reconciliation_requested
                        .load(Ordering::Acquire)
                        || store
                            .1
                            .deleted_character_reconciliation_running
                            .swap(true, Ordering::AcqRel)
                    {
                        break;
                    }
                }
            });
        if let Err(error) = spawned {
            self.1
                .deleted_character_reconciliation_running
                .store(false, Ordering::Release);
            log::error!("party: could not start deleted Character reconciliation: {error}");
        }
    }

    fn arm_character_gone_relay(&self) {
        let live = self.0.coord();
        let inserted_revision = live.character_revision();
        live.conn.db.game_character().on_insert(move |_ctx, _row| {
            inserted_revision.fetch_add(1, Ordering::Release);
        });

        let deleted_revision = live.character_revision();
        let store = self.clone();
        live.conn.db.game_character().on_delete(move |_ctx, row| {
            deleted_revision.fetch_add(1, Ordering::Release);
            store.request_deleted_character_cleanup(row.guid);
        });
    }
}

fn rotate_transfer_work<T>(rows: &mut [T], after_id: u64, id: impl Fn(&T) -> u64) {
    let split = rows.partition_point(|row| id(row) <= after_id);
    rows.rotate_left(split);
}

fn attempt_bot_transfer(store: &Coordinator, intent: &crate::world::transfer::BotTransferIntent) {
    let claim_token = match next_bot_transfer_claim_token() {
        Ok(token) => token,
        Err(error) => {
            log::warn!(
                "bot Transfer Intent {} could not mint a claim: {error:#}",
                intent.id
            );
            return;
        }
    };
    if let Err(error) = store.claim_bot_transfer_intent(
        intent.id,
        intent.bot_guid,
        intent.controller_generation,
        claim_token,
    ) {
        log::debug!("bot Transfer Intent {} claim: {error}", intent.id);
        return;
    }
    if intent.source_module_identity == spacetimedb_sdk::Identity::ZERO {
        log::debug!(
            "bot Transfer Intent {} normalized its source Module identity; a later bounded pass \
             will claim the populated row",
            intent.id
        );
        return;
    }
    if let Err(error) = crate::world::transfer::run_bot_transfer_intent(store, intent, claim_token)
    {
        log::warn!(
            "bot Transfer Intent {} (Character {} -> map {} instance {}) did not settle: \
             {error:#}",
            intent.id,
            intent.bot_guid,
            intent.destination_map,
            intent.destination_instance
        );
        return;
    }
    if let Err(error) = store.complete_bot_transfer_intent(
        intent.id,
        intent.bot_guid,
        intent.controller_generation,
        claim_token,
    ) {
        log::warn!(
            "bot Transfer Intent {} settled but its exact completion did not commit: {error:#}",
            intent.id
        );
    }
}

fn next_bot_transfer_claim_token() -> Result<u64> {
    loop {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes)
            .map_err(|error| anyhow!("OS randomness unavailable: {error}"))?;
        let token = u64::from_le_bytes(bytes);
        if token != 0 {
            return Ok(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::combat::subscriptions::tests::aura;
    use crate::codec::entity::subscriptions::tests::{creature_entity, player_entity};

    #[test]
    fn transfer_work_continues_after_the_prior_pass_then_wraps() {
        let mut ids = [1, 2, 3, 4, 5, 6];
        rotate_transfer_work(&mut ids, 4, |id| *id);
        assert_eq!(ids, [5, 6, 1, 2, 3, 4]);

        let mut wrapped = [1, 2, 3, 4, 5, 6];
        rotate_transfer_work(&mut wrapped, 6, |id| *id);
        assert_eq!(wrapped, [1, 2, 3, 4, 5, 6]);
    }

    fn swing_event() -> CombatEvent {
        CombatEvent {
            id: 1,
            attacker_guid: 7,
            target_guid: 11,
            damage: 23,
            hit_info: 0,
            killing_blow: false,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            blocked_amount: 0,
            ranged_spell_id: 0,
            ammo_display_id: 0,
            spell_swing: false,
            impact_delay_ms: 0,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
        }
    }

    #[test]
    fn ranged_launch_relays_one_spell_go_for_a_hit_or_miss() {
        let created = Mutex::new(HashSet::from([7]));
        for (hit_info, hits, misses) in [(0, 1, 0), (2, 0, 1)] {
            let row = CombatEvent {
                damage: 0,
                hit_info,
                ranged_spell_id: 75,
                ammo_display_id: 5996,
                impact_delay_ms: 500,
                ..swing_event()
            };
            let out = combat_event_outbound(&created, &row);
            let [Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(go))] = out.as_slice() else {
                panic!("a ranged launch must emit exactly one SPELL_GO");
            };
            assert_eq!((go.caster.guid(), go.spell), (7, 75));
            assert_eq!((go.hits.len(), go.misses.len()), (hits, misses));
            assert!(combat_event_outbound(&Mutex::new(HashSet::new()), &row).is_empty());
        }
    }

    #[test]
    fn ranged_impact_relays_only_its_authoritative_damage() {
        let row = SpellCastEvent {
            id: 1,
            caster_guid: 7,
            spell_id: 75,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            target_guid: 11,
            cast_time_ms: 0,
            is_completion: false,
            damage: 23,
            school: 0,
            is_crit: false,
            resisted: 0,
            absorbed: 0,
            is_interrupted: false,
            cooldown_ms: 0,
            delay_ms: 0,
            healed: 0,
            is_proc_log: true,
            swing_hit_info: 0,
            client_initiated: false,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            failure_reason: 0,
            kind: 5,
        };
        let out = cast_event_outbound(7, &row);
        let [Outbound::One(ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(log))] = out.as_slice()
        else {
            panic!("an impact must emit exactly one damage log");
        };
        assert_eq!(
            (
                log.attacker.guid(),
                log.target.guid(),
                log.spell,
                log.damage
            ),
            (7, 11, 75, 23)
        );
    }

    #[test]
    fn melee_swing_relays_damage_then_stops_on_a_killing_blow() {
        let created = Mutex::new(HashSet::from([7]));
        let mut row = swing_event();
        let out = combat_event_outbound(&created, &row);
        assert!(matches!(
            out.as_slice(),
            [Outbound::One(
                ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(_)
            )]
        ));

        row.killing_blow = true;
        let out = combat_event_outbound(&created, &row);
        assert!(matches!(
            out.as_slice(),
            [
                Outbound::One(ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(_)),
                Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(_))
            ]
        ));
    }

    fn viewer_with_created(creature_guid: u64) -> Viewer {
        let (tx, _rx) = SessionTx::with_depth(0);
        Viewer {
            active: std::sync::atomic::AtomicBool::new(true),
            session: 1,
            self_guid: 7,
            bound_identity: spacetimedb_sdk::Identity::from_byte_array([0; 32]),
            map_id: 0,
            instance_id: 0,
            zone_id: 0.into(),
            tx,
            created: Arc::new(Mutex::new(HashSet::from([creature_guid]))),
            gates: Arc::new(ViewerGates::default()),
            skill_slots: Arc::new(Mutex::new((std::collections::HashMap::new(), 0))),
            explored: Mutex::new(world_view::ExplorationReplay::default()),
            motion_pending: Arc::new(world_view::MotionPending::default()),
            member_stats: Default::default(),
            ignored: Mutex::default(),
            friends: Mutex::default(),
            team: lyracore_shared::faction::TEAM_ALLIANCE,
            group_events: Default::default(),
        }
    }

    #[test]
    fn encounter_equipment_reaches_live_and_late_viewers_in_wire_order() {
        let creature_guid = 0xF130_0000_0000_002A;
        let viewer = viewer_with_created(creature_guid);
        let equipment = EncounterEquip {
            creature_guid,
            instance_id: 0,
            main_hand: 2_196,
            off_hand: 2_716,
            ranged: 0,
        };

        for (row, cleared, expected) in [
            (equipment.clone(), false, (2_196, 2_716, 0)),
            (
                EncounterEquip {
                    main_hand: 2_716,
                    off_hand: 0,
                    ranged: 2_196,
                    ..equipment.clone()
                },
                false,
                (2_716, 0, 2_196),
            ),
            (equipment.clone(), true, (0, 0, 0)),
        ] {
            let out = encounter_equip_outbound(&viewer, &row, cleared);
            let (expected_opcode, expected_body) =
                codec::build_virtual_item_values(creature_guid, expected.0, expected.1, expected.2);
            assert!(matches!(
                out.as_slice(),
                [Outbound::Raw { opcode, body }]
                    if *opcode == expected_opcode && *body == expected_body
            ));
        }

        let creature = codec::EntityView {
            guid: creature_guid,
            type_mask: lyracore_shared::constants::type_mask::CREATURE,
            entry: 42,
            scale_x: 1.0,
            health: 1,
            max_health: 1,
            level: 1,
            run_speed_mult_bp: 10_000,
            unit_bytes_0: 0x0101,
            display_id: 1,
            native_display_id: 1,
            ..Default::default()
        };
        let create = codec::build_create_object(&creature, CreateKind::Peer, &[], &[]).unwrap();
        let mut late = vec![Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(create),
        ))];
        append_encounter_equip_after_create(&mut late, &viewer, &equipment);
        assert!(matches!(
            late.as_slice(),
            [
                Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(_)),
                Outbound::Raw { .. }
            ]
        ));
    }

    #[test]
    fn chat_event_target_reaches_every_addressable_creature_packet() {
        let base = ChatEvent {
            id: 1,
            sender_guid: 0xF130_0000_0000_002A,
            chat_type: 0,
            language: 0,
            message: "You there!".into(),
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            target_guid: 77,
        };
        for chat_type in [
            lyracore_shared::chat::broadcast_chat::SAY,
            lyracore_shared::chat::broadcast_chat::YELL,
            lyracore_shared::chat::broadcast_chat::CREATURE_TEXT_EMOTE,
        ] {
            let out = chat_event_message(
                &ChatEvent {
                    chat_type,
                    ..base.clone()
                },
                Some("Defias Thug".into()),
                0,
            );
            let Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(message)) = out else {
                panic!("expected creature chat packet");
            };
            match &message.chat_type {
                wow_world_messages::vanilla::SMSG_MESSAGECHAT_ChatType::MonsterSay {
                    target,
                    ..
                }
                | wow_world_messages::vanilla::SMSG_MESSAGECHAT_ChatType::MonsterYell {
                    target,
                    ..
                } => assert_eq!(target.guid(), 77),
                // `MonsterEmote` has no addressee field on the wire; its single guid names the
                // speaker, so the addressed target is legitimately absent here.
                wow_world_messages::vanilla::SMSG_MESSAGECHAT_ChatType::MonsterEmote {
                    monster,
                    ..
                } => assert_eq!(monster.guid(), 0xF130_0000_0000_002A),
                other => panic!("expected addressable creature chat, got {other:?}"),
            }
        }
    }

    #[test]
    fn system_message_outbound_reuses_the_existing_system_chat_builder() {
        let row = SystemMessageEvent {
            id: 1,
            recipient_identity: spacetimedb_sdk::Identity::from_byte_array([7; 32]),
            recipient_guid: 9001,
            message: "Package loaded".to_string(),
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        };
        let outbound = system_message_event_outbound(&row);
        let [Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(actual))] = outbound.as_slice()
        else {
            panic!("one System Message packet expected");
        };

        assert_eq!(
            **actual,
            codec::build_gm_system_message(row.message.clone())
        );
    }

    fn duel_event(kind: u8, completion_kind: u8) -> DuelEvent {
        DuelEvent {
            id: 1,
            recipient_identity: spacetimedb_sdk::Identity::from_byte_array([0u8; 32]),
            recipient_guid: 10,
            kind,
            completion_kind,
            duel_id: 3,
            flag_guid: 0xF110_2000_0000_0003,
            flag_entry: 21680,
            initiator_guid: 10,
            challenged_guid: 20,
            winner_guid: 0,
            loser_guid: 0,
            map_id: 1,
            instance_id: 7,
            flag_x: 3.0,
            flag_y: 4.0,
            flag_z: 5.0,
            flag_orientation: 1.25,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            winner_name: String::new(),
            loser_name: String::new(),
        }
    }

    #[test]
    fn duel_request_projects_one_typed_request_with_flag_arbiter_and_challenger() {
        let template = codec::GameObjectTemplateView {
            type_id: 16,
            display_id: 787,
            name: "Duel Arbiter".into(),
            data0: 0,
            data1: 0,
            data2: 0,
        };
        let row = duel_event(0, 0);
        let out = duel_event_outbound(&row, Some(&template));

        assert_eq!(out.len(), 5);
        let requests: Vec<_> = out
            .iter()
            .filter_map(|message| match message {
                Outbound::One(ServerOpcodeMessage::SMSG_DUEL_REQUESTED(request)) => Some(request),
                _ => None,
            })
            .collect();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].initiator,
            wow_world_messages::Guid::new(row.flag_guid)
        );
        assert_eq!(
            requests[0].target,
            wow_world_messages::Guid::new(row.initiator_guid)
        );
    }

    #[test]
    fn duel_countdown_active_and_interruption_project_the_expected_edges() {
        let countdown = duel_event_outbound(&duel_event(1, 0), None);
        assert!(matches!(
            countdown.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_DUEL_COUNTDOWN(message))]
                if message.time.as_secs() == 3_000
        ));

        let active = duel_event_outbound(&duel_event(2, 0), None);
        assert_eq!(active.len(), 2);
        assert!(active.iter().all(|out| matches!(out, Outbound::Raw { .. })));

        let interrupted = duel_event_outbound(&duel_event(3, 0), None);
        assert_eq!(interrupted.len(), 4);
        assert!(matches!(
            &interrupted[0],
            Outbound::One(ServerOpcodeMessage::SMSG_DUEL_COMPLETE(message))
                if !message.ended_without_interruption
        ));
        assert!(matches!(
            interrupted.last(),
            Some(Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(_)))
        ));
    }

    #[test]
    fn duel_winner_projects_names_and_the_terminal_reason() {
        let mut won = duel_event(3, 2);
        won.winner_guid = 10;
        won.loser_guid = 20;
        won.winner_name = "Winner".into();
        won.loser_name = "Loser".into();
        let out = duel_event_outbound(&won, None);
        assert!(matches!(
                    out.as_slice(),
                    [
                        Outbound::One(ServerOpcodeMessage::SMSG_DUEL_COMPLETE(_)),
                        Outbound::One(ServerOpcodeMessage::SMSG_DUEL_WINNER(message)),
        ..
                        // `opponent_name` is written first on the wire and must carry the winner.
                    ] if message.reason == wow_world_base::shared::duel_winner_reason_vanilla_tbc_wrath::DuelWinnerReason::Won
                        && message.opponent_name == "Winner"
                        && message.initiator_name == "Loser"
                ));

        let mut fled_event = duel_event(3, 3);
        fled_event.winner_guid = 10;
        fled_event.loser_guid = 20;
        fled_event.winner_name = "Winner".into();
        fled_event.loser_name = "Loser".into();
        let fled = duel_event_outbound(&fled_event, None);
        assert!(matches!(
            &fled[1],
            Outbound::One(ServerOpcodeMessage::SMSG_DUEL_WINNER(message))
                if message.reason == wow_world_base::shared::duel_winner_reason_vanilla_tbc_wrath::DuelWinnerReason::Fled
        ));
    }

    #[test]
    fn duel_boundary_edges_project_only_the_recipient_transition() {
        let out_of_bounds = duel_event_outbound(&duel_event(4, 0), None);
        assert!(matches!(
            out_of_bounds.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_DUEL_OUTOFBOUNDS)]
        ));

        let in_bounds = duel_event_outbound(&duel_event(5, 0), None);
        assert!(matches!(
            in_bounds.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_DUEL_INBOUNDS)]
        ));
    }

    /// The trade-status wire mapping: every `lyracore_shared::trade::event_kind` the module
    /// emits decodes to its `SMSG_TRADE_STATUS` variant — `BeginTrade` carrying the counterparty
    /// guid the client needs to open the window — and an unknown kind (newer module mid-rollout)
    /// drops rather than desyncing the trade window.
    #[test]
    fn trade_event_kinds_decode_to_their_trade_status_variants() {
        use lyracore_shared::trade::event_kind as kind;
        use wow_world_messages::vanilla::SMSG_TRADE_STATUS;

        let event = |k: u8| TradeEvent {
            id: 1,
            recipient_identity: spacetimedb_sdk::Identity::from_byte_array([0u8; 32]),
            kind: k,
            other_guid: 77,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            recipient_guid: 1,
            payload: String::new(),
        };
        let status_of = |k: u8| -> SMSG_TRADE_STATUS {
            let out = trade_event_outbound(&event(k));
            assert_eq!(out.len(), 1, "kind {k} must decode to exactly one packet");
            match &out[0] {
                Outbound::One(ServerOpcodeMessage::SMSG_TRADE_STATUS(s)) => **s,
                Outbound::One(other) => panic!("kind {k}: expected SMSG_TRADE_STATUS, got {other}"),
                _ => panic!("kind {k}: expected a single SMSG_TRADE_STATUS packet"),
            }
        };

        assert_eq!(
            status_of(kind::BEGIN_TRADE),
            SMSG_TRADE_STATUS::BeginTrade {
                unknown1: wow_world_messages::Guid::new(77)
            },
            "BeginTrade must carry the initiator guid off the row"
        );
        assert_eq!(status_of(kind::OPEN_WINDOW), SMSG_TRADE_STATUS::OpenWindow);
        assert_eq!(
            status_of(kind::TRADE_CANCELED),
            SMSG_TRADE_STATUS::TradeCanceled
        );
        assert_eq!(status_of(kind::BUSY), SMSG_TRADE_STATUS::Busy);
        assert_eq!(status_of(kind::NO_TARGET), SMSG_TRADE_STATUS::NoTarget);
        assert_eq!(
            status_of(kind::TARGET_TO_FAR),
            SMSG_TRADE_STATUS::TargetToFar
        );
        assert_eq!(
            status_of(kind::WRONG_FACTION),
            SMSG_TRADE_STATUS::WrongFaction
        );
        assert_eq!(status_of(kind::YOU_DEAD), SMSG_TRADE_STATUS::YouDead);
        assert_eq!(status_of(kind::TARGET_DEAD), SMSG_TRADE_STATUS::TargetDead);
        assert_eq!(status_of(kind::IGNORE_YOU), SMSG_TRADE_STATUS::IgnoreYou);
        assert_eq!(
            status_of(kind::TRADE_ACCEPT),
            SMSG_TRADE_STATUS::TradeAccept
        );
        assert_eq!(
            status_of(kind::BACK_TO_TRADE),
            SMSG_TRADE_STATUS::BackToTrade
        );
        assert_eq!(
            status_of(kind::TRADE_COMPLETE),
            SMSG_TRADE_STATUS::TradeComplete
        );
        // The bag-space refusal closes the window naming WHOSE bags overflowed.
        let full_self = status_of(kind::INV_FULL_SELF);
        let full_partner = status_of(kind::INV_FULL_PARTNER);
        match (&full_self, &full_partner) {
            (
                SMSG_TRADE_STATUS::CloseWindow {
                    target_error: false,
                    inventory_result: a,
                    ..
                },
                SMSG_TRADE_STATUS::CloseWindow {
                    target_error: true,
                    inventory_result: b,
                    ..
                },
            ) => {
                assert_eq!(
                    *a,
                    wow_world_messages::vanilla::InventoryResult::InventoryFull
                );
                assert_eq!(
                    *b,
                    wow_world_messages::vanilla::InventoryResult::InventoryFull
                );
            }
            other => panic!("expected CloseWindow pair with flipped target_error, got {other:?}"),
        }
        // The purse-failure pair mirrors it with NotEnoughMoney.
        match (
            &status_of(kind::GOLD_FAIL_SELF),
            &status_of(kind::GOLD_FAIL_PARTNER),
        ) {
            (
                SMSG_TRADE_STATUS::CloseWindow {
                    target_error: false,
                    inventory_result: a,
                    ..
                },
                SMSG_TRADE_STATUS::CloseWindow {
                    target_error: true,
                    inventory_result: b,
                    ..
                },
            ) => {
                assert_eq!(
                    *a,
                    wow_world_messages::vanilla::InventoryResult::NotEnoughMoney
                );
                assert_eq!(
                    *b,
                    wow_world_messages::vanilla::InventoryResult::NotEnoughMoney
                );
            }
            other => panic!("expected NotEnoughMoney CloseWindow pair, got {other:?}"),
        }

        assert!(
            trade_event_outbound(&event(200)).is_empty(),
            "an unknown kind must drop, not guess a status"
        );
    }

    /// Auction Notice kind decoding (`cm:AuctionHouseHandler.cpp`/`AuctionHouseMgr.cpp`): Outbid
    /// and Won go out on `SMSG_AUCTION_BIDDER_NOTIFICATION`, Sold, Expired and New bid go out on
    /// `SMSG_AUCTION_OWNER_NOTIFICATION`, Removed goes out on `SMSG_AUCTION_REMOVED_NOTIFICATION`,
    /// and an unrecognized kind or an unimported house both drop the packet. The Auction Mail the
    /// same transaction wrote still reaches the inbox.
    #[test]
    fn auction_notice_kinds_decode_to_their_own_packet_and_an_unknown_one_drops() {
        use lyracore_shared::auction::auction_notice::{
            EXPIRED, NEW_BID, OUTBID, REMOVED, SOLD, WON,
        };
        use wow_world_messages::vanilla::AuctionHouse;

        let notice = |kind: u8, house: u32| AuctionNotice {
            id: 1,
            recipient_guid: 7,
            kind,
            house,
            auction_id: 41,
            item_entry: 25,
            random_property_id: 117,
            bid: 500,
            out_bid: 25,
            bidder_guid: 9,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
        };

        for kind in [OUTBID, WON] {
            let out = auction_notice_outbound(&notice(kind, 1));
            assert_eq!(
                out.len(),
                1,
                "kind {kind} must decode to exactly one packet"
            );
            match &out[0] {
                Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_BIDDER_NOTIFICATION(packet)) => {
                    assert_eq!(packet.auction_house, AuctionHouse::Stormwind);
                    assert_eq!(packet.bidder.guid(), 9);
                }
                Outbound::One(other) => {
                    panic!("kind {kind}: expected SMSG_AUCTION_BIDDER_NOTIFICATION, got {other}")
                }
                _ => {
                    panic!("kind {kind}: expected a single SMSG_AUCTION_BIDDER_NOTIFICATION packet")
                }
            }
        }

        for kind in [SOLD, EXPIRED, NEW_BID] {
            let out = auction_notice_outbound(&notice(kind, 1));
            assert_eq!(
                out.len(),
                1,
                "kind {kind} must decode to exactly one packet"
            );
            match &out[0] {
                Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_OWNER_NOTIFICATION(packet)) => {
                    assert_eq!(packet.auction_id, 41);
                }
                Outbound::One(other) => {
                    panic!("kind {kind}: expected SMSG_AUCTION_OWNER_NOTIFICATION, got {other}")
                }
                _ => {
                    panic!("kind {kind}: expected a single SMSG_AUCTION_OWNER_NOTIFICATION packet")
                }
            }
        }

        let out = auction_notice_outbound(&notice(REMOVED, 1));
        assert!(
            matches!(
                out.as_slice(),
                [Outbound::One(ServerOpcodeMessage::SMSG_AUCTION_REMOVED_NOTIFICATION(packet))]
                    if packet.item == 41
                        && packet.item_template == 25
                        && packet.random_property_id == 117
            ),
            "Removed goes out on SMSG_AUCTION_REMOVED_NOTIFICATION"
        );

        assert!(
            auction_notice_outbound(&notice(200, 1)).is_empty(),
            "an unknown kind must drop, not guess a packet"
        );
        assert!(
            auction_notice_outbound(&notice(OUTBID, 99)).is_empty(),
            "a house outside 1-7 must drop rather than send an unresolved house"
        );
    }

    /// The OFFER_* kinds decode to the fixed-444-byte extended status: the polarity byte
    /// comes from the KIND (never inferred), the window-visible item fields survive the payload
    /// round-trip into the right wire slots, unused slots stay zeroed, and a malformed payload
    /// drops the packet entirely.
    #[test]
    fn offer_events_decode_to_the_extended_status_with_the_kind_deciding_self_player() {
        use lyracore_shared::trade::{encode_offer, event_kind as kind, OfferSlot};
        use wow_world_messages::vanilla::SMSG_TRADE_STATUS_EXTENDED;

        let payload = encode_offer(
            1_2345,
            &[OfferSlot {
                trade_slot: 6,
                entry: 6948,
                display_id: 6418,
                stack_count: 5,
                enchantment: 2564,
                durability: 34,
                max_durability: 40,
                random_property_id: 0,
            }],
        );
        let event = |k: u8, p: &str| TradeEvent {
            id: 1,
            recipient_identity: spacetimedb_sdk::Identity::from_byte_array([0u8; 32]),
            kind: k,
            other_guid: 77,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            recipient_guid: 1,
            payload: p.to_string(),
        };
        let extended = |k: u8| -> SMSG_TRADE_STATUS_EXTENDED {
            let out = trade_event_outbound(&event(k, &payload));
            match out.first() {
                Some(Outbound::One(ServerOpcodeMessage::SMSG_TRADE_STATUS_EXTENDED(m))) => **m,
                _ => panic!("kind {k}: expected one SMSG_TRADE_STATUS_EXTENDED"),
            }
        };

        // Wire polarity (mangoszero): the `self_player` BYTE is "1 means traders data, 0 means
        // own", so your OWN echo carries 0 and the partner's side carries 1.
        let own = extended(kind::OFFER_SELF);
        assert!(
            !own.self_player,
            "OFFER_SELF describes your own window: wire byte 0"
        );
        let theirs = extended(kind::OFFER_PARTNER);
        assert!(
            theirs.self_player,
            "OFFER_PARTNER describes the partner's side: wire byte 1"
        );

        assert_eq!(own.money_in_trade.as_int(), 1_2345);
        assert_eq!((own.trade_slot_count1, own.trade_slot_count2), (7, 7));
        let s = &own.trade_slots[6];
        assert_eq!(
            (s.trade_slot_number, s.item, s.display_id, s.stack_count),
            (6, 6948, 6418, 5)
        );
        assert_eq!(
            (s.enchantment, s.durability, s.max_durability),
            (2564, 34, 40)
        );
        assert_eq!(
            own.trade_slots[0],
            Default::default(),
            "untouched slots stay zeroed"
        );

        assert!(
            trade_event_outbound(&event(kind::OFFER_SELF, "not|a,valid,payload")).is_empty(),
            "a malformed offer payload must drop the packet"
        );
    }

    #[test]
    fn a_compatibility_enchant_in_an_offer_goes_out_as_its_client_enchantment() {
        use lyracore_shared::trade::{encode_offer, event_kind as kind, OfferSlot};
        let slot = |trade_slot, enchantment| OfferSlot {
            trade_slot,
            entry: 6948,
            display_id: 6418,
            stack_count: 1,
            enchantment,
            durability: 40,
            max_durability: 40,
            random_property_id: 0,
        };
        let payload = encode_offer(0, &[slot(0, 7745), slot(1, 7748)]);
        let event = TradeEvent {
            id: 1,
            recipient_identity: spacetimedb_sdk::Identity::from_byte_array([0u8; 32]),
            kind: kind::OFFER_SELF,
            other_guid: 77,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            recipient_guid: 1,
            payload,
        };
        let out = trade_event_outbound(&event);
        let Some(Outbound::One(ServerOpcodeMessage::SMSG_TRADE_STATUS_EXTENDED(extended))) =
            out.first()
        else {
            panic!("expected one SMSG_TRADE_STATUS_EXTENDED");
        };
        assert_eq!(extended.trade_slots[0].enchantment, 823);
        assert_eq!(extended.trade_slots[1].enchantment, 724);
    }

    const HEROIC_STRIKE_CASTER: u64 = 7;
    /// The module's `CAST_FAIL_NO_POWER`.
    const NO_POWER_CODE: u8 = 1;

    fn spell_cast_event(kind: u8) -> SpellCastEvent {
        SpellCastEvent {
            id: 1,
            caster_guid: HEROIC_STRIKE_CASTER,
            spell_id: 133,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            target_guid: 11,
            cast_time_ms: 0,
            is_completion: false,
            damage: 0,
            school: 0,
            is_crit: false,
            resisted: 0,
            absorbed: 0,
            is_interrupted: false,
            cooldown_ms: 0,
            delay_ms: 0,
            healed: 0,
            is_proc_log: false,
            swing_hit_info: 0,
            client_initiated: false,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            failure_reason: 0,
            kind,
        }
    }

    /// An interruption signal row for `spell_id` cast by `HEROIC_STRIKE_CASTER`, carrying
    /// `failure_reason`. Every other field stays at the module baseline's zero, which is what such a
    /// row really looks like on the wire.
    fn interrupted_cast(spell_id: u32, failure_reason: u8) -> SpellCastEvent {
        SpellCastEvent {
            spell_id,
            target_guid: 0,
            is_interrupted: true,
            failure_reason,
            ..spell_cast_event(3)
        }
    }

    #[test]
    fn explicit_spell_cast_event_kinds_override_legacy_discriminators() {
        let contradictory = |kind| SpellCastEvent {
            cast_time_ms: 1_500,
            is_completion: true,
            damage: 23,
            is_interrupted: true,
            delay_ms: 500,
            is_proc_log: true,
            kind,
            ..spell_cast_event(kind)
        };

        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER, &contradictory(1)).as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(_))]
        ));
        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER + 1, &contradictory(2)).as_slice(),
            [
                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(_)),
                Outbound::One(ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(_))
            ]
        ));

        let interrupt = SpellCastEvent {
            kind: 3,
            ..spell_cast_event(3)
        };
        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER, &interrupt).as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_SPELL_FAILURE(_))]
        ));

        let pushback = SpellCastEvent {
            delay_ms: 500,
            kind: 4,
            ..spell_cast_event(4)
        };
        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER, &pushback).as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_SPELL_DELAYED(_))]
        ));

        let proc_log = SpellCastEvent {
            damage: 23,
            kind: 5,
            ..spell_cast_event(5)
        };
        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER, &proc_log).as_slice(),
            [Outbound::One(
                ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(_)
            )]
        ));
    }

    #[test]
    fn legacy_spell_cast_event_rows_keep_the_field_based_decoder() {
        let start = SpellCastEvent {
            cast_time_ms: 1_500,
            ..spell_cast_event(0)
        };
        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER, &start).as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(_))]
        ));

        let go = spell_cast_event(0);
        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER + 1, &go).as_slice(),
            [
                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(_)),
                Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(_))
            ]
        ));

        let interrupt = SpellCastEvent {
            is_interrupted: true,
            ..spell_cast_event(0)
        };
        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER, &interrupt).as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_SPELL_FAILURE(_))]
        ));

        let pushback = SpellCastEvent {
            delay_ms: 500,
            ..spell_cast_event(0)
        };
        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER, &pushback).as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_SPELL_DELAYED(_))]
        ));

        let proc_log = SpellCastEvent {
            damage: 23,
            is_proc_log: true,
            ..spell_cast_event(0)
        };
        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER, &proc_log).as_slice(),
            [Outbound::One(
                ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(_)
            )]
        ));

        let old_precedence = SpellCastEvent {
            cast_time_ms: 1_500,
            is_interrupted: true,
            delay_ms: 500,
            is_proc_log: true,
            ..spell_cast_event(0)
        };
        assert!(matches!(
            cast_event_outbound(HEROIC_STRIKE_CASTER, &old_precedence).as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_SPELL_FAILURE(_))]
        ));
    }

    #[test]
    fn unknown_spell_cast_event_kind_emits_no_packet() {
        let row = SpellCastEvent {
            is_interrupted: true,
            delay_ms: 500,
            is_proc_log: true,
            kind: u8::MAX,
            ..spell_cast_event(u8::MAX)
        };
        assert!(cast_event_outbound(HEROIC_STRIKE_CASTER, &row).is_empty());
    }

    /// A queued strike that could not pay its cost at the swing: the caster gets the cast-bar
    /// teardown and then the failed cast result naming the queued spell with the vanilla Not Enough
    /// Power reason (0x4D). No SMSG_SPELL_GO — the strike never fired.
    #[test]
    fn a_deferred_power_failure_relays_the_teardown_then_the_failed_cast_result() {
        let out = cast_event_outbound(HEROIC_STRIKE_CASTER, &interrupted_cast(78, NO_POWER_CODE));

        assert_eq!(out.len(), 2, "expected exactly the two vanilla messages");
        match &out[0] {
            Outbound::One(ServerOpcodeMessage::SMSG_SPELL_FAILURE(m)) => {
                assert_eq!(m.guid.guid(), HEROIC_STRIKE_CASTER);
                assert_eq!(m.spell, 78);
            }
            _ => panic!("expected the interruption teardown first"),
        }
        match &out[1] {
            Outbound::Raw { opcode, body } => {
                assert_eq!(*opcode, 0x0130);
                // spell(u32 LE) + CAST_FAILED(0x02) + SPELL_FAILED_NO_POWER(0x4D).
                assert_eq!(body, &vec![78, 0, 0, 0, 0x02, 0x4D]);
            }
            _ => panic!("expected the raw failed cast result second"),
        }
    }

    /// The failed cast result is caster-private: a bystander watching the same row gets nothing.
    #[test]
    fn a_bystander_sees_neither_half_of_a_deferred_power_failure() {
        let out = cast_event_outbound(
            HEROIC_STRIKE_CASTER + 1,
            &interrupted_cast(78, NO_POWER_CODE),
        );
        assert!(out.is_empty(), "both messages are caster-private");
    }

    /// An interruption with no concrete reason (a damaged mid-cast timed spell) still sends the
    /// teardown alone — the cast result belongs to a failure the server can name.
    #[test]
    fn an_ordinary_interruption_still_sends_the_teardown_alone() {
        let out = cast_event_outbound(HEROIC_STRIKE_CASTER, &interrupted_cast(133, 0));
        assert_eq!(out.len(), 1, "expected the teardown alone");
        assert!(matches!(
            &out[0],
            Outbound::One(ServerOpcodeMessage::SMSG_SPELL_FAILURE(_))
        ));
    }

    fn created_player(out: &[Outbound]) -> wow_world_messages::vanilla::UpdatePlayer {
        use wow_world_messages::vanilla::{Object, UpdateMask};
        let Some(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(create))) = out.first()
        else {
            panic!("a peer CREATE comes first");
        };
        let [Object::CreateObject2 {
            mask2: UpdateMask::Player(player),
            ..
        }] = create.objects.as_slice()
        else {
            panic!("the CREATE must carry a player mask");
        };
        player.clone()
    }

    /// A Human Warrior: the CREATE encoder refuses race 0.
    fn human_warrior() -> WorldEntity {
        WorldEntity {
            unit_bytes_0: 1 | (1 << 8) | (1 << 24),
            ..player_entity()
        }
    }

    #[test]
    fn a_peer_player_create_carries_its_guild_projection() {
        let row = human_warrior();
        let view = peer_entity_view(&row, 0, |guid| {
            assert_eq!(guid, row.guid);
            (7, 3)
        });
        let player = created_player(&peer_create_outbound(&view, &[], &[], None).unwrap());
        assert_eq!(player.player_guildid(), Some(7));
        assert_eq!(player.player_guildrank(), Some(3));
    }

    #[test]
    fn a_peer_outside_any_guild_is_created_with_an_explicit_zero() {
        let row = human_warrior();
        let player = created_player(
            &peer_create_outbound(&peer_entity_view(&row, 0, |_| (0, 0)), &[], &[], None).unwrap(),
        );
        assert_eq!(player.player_guildid(), Some(0));
        assert_eq!(player.player_guildrank(), Some(0));
    }

    #[test]
    fn a_creature_create_reads_no_guild() {
        let view = peer_entity_view(&creature_entity(), 0, |_| panic!("a creature has no Guild"));
        assert_eq!((view.guild_id, view.guild_rank), (0, 0));
    }

    #[test]
    fn no_change_emits_nothing() {
        let e = player_entity();
        assert!(entity_update_to_outbound(&e, &e).is_empty());
    }

    #[test]
    fn single_health_change_emits_one() {
        let old = player_entity();
        let mut new = old.clone();
        new.health = 50;
        let out = entity_update_to_outbound(&old, &new);
        // Decode + pin: guid and the exact health value ride the health-only VALUES packet.
        assert_eq!(
            out,
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_health_values(new.guid, 50)
            ))]
        );
    }

    /// a sheath change must relay, or drawing a weapon is invisible to everyone but the player
    /// who did it. The state lives in BYTE 0 of `unit_bytes_2` — pin the decoded packet, and pin that
    /// an unchanged row stays silent (the client re-sends its current state on every weapon swap, and
    /// a relay per no-op is a broadcast amplifier on a busy cell).
    #[test]
    fn sheath_change_relays_and_a_no_op_stays_silent() {
        let old = player_entity();
        let mut new = old.clone();
        new.unit_bytes_2 = 1; // SHEATH_STATE_MELEE — weapons drawn
        assert_eq!(
            entity_update_to_outbound(&old, &new),
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_sheath_values(new.guid, 1)
            ))]
        );
        assert!(
            entity_update_to_outbound(&old, &old.clone()).is_empty(),
            "an unchanged row must not relay a sheath packet"
        );
    }

    #[test]
    fn independent_field_changes_each_emit() {
        // health (any unit) + dynamic_flags (any unit) + power (player) = three separate packets,
        // in the relay's fixed emission order — decoded and pinned by guid + value, not just count.
        let old = player_entity();
        let mut new = old.clone();
        new.health = 50;
        new.dynamic_flags = 1;
        new.power = 30;
        let power_b = (new.unit_bytes_0 >> 24) as u8;
        let out = entity_update_to_outbound(&old, &new);
        assert_eq!(
            out,
            vec![
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_health_values(
                    new.guid, 50
                ))),
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                    codec::build_dynamic_flags_values(new.guid, 1)
                )),
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_power_values(
                    new.guid, power_b, 30
                ))),
            ]
        );
    }

    #[test]
    fn dynamic_flag_values_uses_the_viewer_projection() {
        let mut old = creature_entity();
        old.dynamic_flags = 0;
        let mut new = old.clone();
        new.dynamic_flags = lyracore_shared::constants::unit_dynamic_flags::TAPPED;
        let projected =
            new.dynamic_flags | lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER;
        assert_eq!(
            entity_update_to_outbound_with_dynamic_flags(&old, &new, projected),
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_dynamic_flags_values(new.guid, projected)
            ))]
        );
    }

    #[test]
    fn levelup_is_exclusive_with_field_diffs() {
        // A ding collapses level + vitals + xp into ONE combined VALUES block, NOT separate health/xp
        // packets — even though health and xp also changed this tick.
        let old = player_entity();
        let mut new = old.clone();
        new.level = 2;
        new.health = 120;
        new.max_health = 120;
        new.xp = 10;
        assert_eq!(entity_update_to_outbound(&old, &new).len(), 1);
    }

    /// cm:Player.cpp:16571-16599: a player's say, yell and `/e` carry `GetChatTag()`, AFK 1 and
    /// DND 2. A creature's line carries none, whatever its flags word holds.
    #[test]
    fn a_nearby_line_carries_the_player_speakers_away_tag() {
        let mut speaker = player_entity();
        assert_eq!(nearby_chat_tag(Some(&speaker)), 0);
        speaker.player_flags = 0x02;
        assert_eq!(nearby_chat_tag(Some(&speaker)), 1);
        speaker.player_flags = 0x04 | 0x10;
        assert_eq!(nearby_chat_tag(Some(&speaker)), 2);
        speaker.type_mask = 0;
        assert_eq!(nearby_chat_tag(Some(&speaker)), 0, "a creature");
        assert_eq!(nearby_chat_tag(None), 0, "a speaker already gone");
    }

    /// `/afk` sets PLAYER_FLAGS_AFK (0x02, descriptor index 190). Self and every observer run this
    /// same diff, so each client holding the object gets one VALUES update carrying the new word.
    #[test]
    fn an_afk_toggle_relays_player_flags_to_every_viewer() {
        let old = player_entity();
        for (flags, label) in [(0x02, "AFK"), (0x04, "DND"), (0x00, "cleared")] {
            let mut new = old.clone();
            new.player_flags = flags;
            let before = if flags == 0 {
                let mut away = old.clone();
                away.player_flags = 0x02;
                away
            } else {
                old.clone()
            };
            let out = entity_update_to_outbound(&before, &new);
            let [ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update)] = out.as_slice() else {
                panic!("{label}: expected one VALUES update, got {out:?}");
            };
            let mut wire = Vec::new();
            wow_world_messages::vanilla::ServerMessage::write_unencrypted_server(
                &**update, &mut wire,
            )
            .unwrap();
            let values = lyracore_shared::values_mask::parse_values_updates(&wire[4..]);
            assert_eq!(values.len(), 1, "{label}");
            assert_eq!(values[0].guid, 1, "{label}");
            assert!(
                values[0].fields.contains(&(190, flags)),
                "{label}: PLAYER_FLAGS missing from {:?}",
                values[0].fields
            );
            assert!(
                !values[0].fields.iter().any(|&(index, _)| index == 2),
                "{label}: OBJECT_FIELD_TYPE leaked into a partial update"
            );
        }
    }

    #[test]
    fn player_only_fields_are_gated_for_creatures() {
        // A creature (no PLAYER bit) relays health + dynamic_flags but NOT xp/coinage/ghost/power —
        // decoded and pinned as exactly those two packets, not just a count of two.
        let mut old = player_entity();
        old.type_mask = 0;
        let mut new = old.clone();
        new.health = 50; // emits (any unit)
        new.dynamic_flags = 1; // emits (any unit)
        new.xp = 10; // gated out (player-only)
        new.money = 10; // gated out
        new.power = 30; // gated out
        new.player_flags = 0x10; // gated out
        let out = entity_update_to_outbound(&old, &new);
        assert_eq!(
            out,
            vec![
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_health_values(
                    new.guid, 50
                ))),
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                    codec::build_dynamic_flags_values(new.guid, 1)
                )),
            ]
        );
    }

    #[test]
    fn creature_levelup_does_not_take_the_exclusive_branch() {
        // The level-up branch is player-only; a creature whose level changed still diffs health
        // normally. A bare length check can't tell the two branches apart here — the player-only
        // ding ALSO emits exactly one packet — so decode the one packet and pin it as the plain
        // Unit-mask health update (build_health_values), not the Player-mask combined ding
        // (build_levelup_values) a regressed player-gate would otherwise produce with the same count.
        let mut old = player_entity();
        old.type_mask = 0;
        let mut new = old.clone();
        new.level = 5;
        new.health = 50;
        let out = entity_update_to_outbound(&old, &new);
        assert_eq!(
            out,
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_health_values(new.guid, new.health)
            ))]
        );
    }

    #[test]
    fn target_change_emits_one() {
        // UNIT_FIELD_TARGET relay — any unit, one packet, guid + target value pinned.
        let old = player_entity();
        let mut new = old.clone();
        new.target_guid = 0xDEAD_BEEF;
        let out = entity_update_to_outbound(&old, &new);
        assert_eq!(
            out,
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_target_values(new.guid, 0xDEAD_BEEF)
            ))]
        );
    }

    #[test]
    fn bank_bag_slot_purchase_relays_and_the_rest_byte_survives() {
        // A bought bank bag slot lands in byte 2 of PLAYER_BYTES_2; the relay must carry the FULL
        // field (a partial VALUES overwrites the whole u32) so byte 0 (facial hair) and byte 3 (rest
        // state — HAZARD: a zero byte 3 crashes the 5875 client's XP bar) ride along unchanged.
        let mut old = player_entity();
        old.player_bytes_2 = lyracore_shared::packing::player_bytes_2_with_rest(7, 0, false); // facial hair 7, 0 slots, NORMAL
        let mut new = old.clone();
        new.player_bytes_2 = lyracore_shared::packing::with_bank_bag_slots(old.player_bytes_2, 1);
        let out = entity_update_to_outbound(&old, &new);
        assert_eq!(
            out,
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_bank_bag_slots_values(new.guid, new.player_bytes_2)
            ))]
        );
        let (facial_hair, _, bank_slots, rest) =
            lyracore_shared::packing::unpack4(new.player_bytes_2);
        assert_eq!(
            facial_hair, 7,
            "facial hair (byte 0) must survive the purchase"
        );
        assert_eq!(bank_slots, 1);
        assert_eq!(
            rest,
            lyracore_shared::packing::REST_STATE_NORMAL,
            "rest state (byte 3) must stay a valid non-zero value"
        );
    }

    #[test]
    fn a_rest_state_flip_alone_does_not_relay_the_bank_slot_packet() {
        // Byte 3 changing (rest flip) must NOT trip the byte-2-scoped bank slot relay — that byte
        // already relays through its own dedicated `game_rest_state_event` path.
        let old = player_entity();
        let mut new = old.clone();
        new.player_bytes_2 = lyracore_shared::packing::player_bytes_2_with_rest(0, 0, true); // RESTED
        assert!(entity_update_to_outbound(&old, &new).is_empty());
    }

    #[test]
    fn max_vitals_change_emits_one() {
        // A non-level-up max_health change (gear/aura) — any unit, one packet, guid + max_health/
        // power values pinned (max_power carries through unchanged from `player_entity()`).
        let old = player_entity();
        let mut new = old.clone();
        new.max_health = 150;
        let power_b = (new.unit_bytes_0 >> 24) as u8;
        let out = entity_update_to_outbound(&old, &new);
        assert_eq!(
            out,
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_max_vitals_values(new.guid, 150, power_b, new.max_power)
            ))]
        );
    }

    #[test]
    fn ghost_transition_edge_detects_the_spirit_healer_reveal() {
        const GHOST: u32 = lyracore_shared::constants::player_flags::GHOST; // 0x10
                                                                            // alive→ghost (Release Spirit): REVEAL the in-scope spirit healers (Some(true)).
        assert_eq!(ghost_transition(0, GHOST, GHOST), Some(true));
        // ghost→alive (reclaim corpse / spirit-healer res): DESTROY them (Some(false)).
        assert_eq!(ghost_transition(GHOST, 0, GHOST), Some(false));
        // No GHOST-bit change → no reveal work, even when OTHER player-flags toggle this update
        // (the common heartbeat / unrelated flag write must NOT re-fire the reveal).
        assert_eq!(ghost_transition(0, 0, GHOST), None);
        assert_eq!(ghost_transition(GHOST, GHOST, GHOST), None);
        assert_eq!(ghost_transition(0x4 | GHOST, 0x8 | GHOST, GHOST), None); // ghost stays set, others flip
        assert_eq!(ghost_transition(0x4, 0x8, GHOST), None); // ghost stays clear, others flip
    }

    // ---- the SAY/YELL range math -----------------------------------------------------------------

    #[test]
    fn chat_in_range_gates_by_distance() {
        // `chat_in_range` only ever sees plain map/x/y — distance in WORLD SPACE.
        let range_sq = 25.0f32 * 25.0; // SAY_RANGE_SQ, spelled out so the test pins the shape not the const
                                       // 10 yards apart -> in range.
        assert!(chat_in_range(
            0, 0, 100.0, 100.0, 0, 0, 105.0, 108.0, range_sq
        ));
        // Exactly on the boundary (25yd) -> in range (inclusive, `<=`).
        assert!(chat_in_range(
            0, 0, 100.0, 100.0, 0, 0, 125.0, 100.0, range_sq
        ));
        // Just past the boundary -> out of range: this is the assertion the wire script's "beyond
        // /say range but within view -> NO chat delivery" case pins end-to-end.
        assert!(!chat_in_range(
            0, 0, 100.0, 100.0, 0, 0, 125.01, 100.0, range_sq
        ));
    }

    #[test]
    fn chat_in_range_never_crosses_a_map_or_instance_boundary_regardless_of_distance() {
        // Same coordinates, different map -> never in range (also guards the arithmetic: comparing
        // x/y across two unrelated maps would otherwise produce a meaningless "distance").
        assert!(!chat_in_range(
            0, 0, 100.0, 100.0, 1, 0, 100.0, 100.0, 90_000.0
        ));
        // Same map, different instance (e.g. an open-world speaker vs. a dungeon-instance listener
        // whose rows happen to share a database) -> never in range either (mirrors `peer_create_gate`'s
        // instance isolation).
        assert!(!chat_in_range(
            0, 0, 100.0, 100.0, 0, 1, 100.0, 100.0, 90_000.0
        ));
    }

    #[test]
    fn chat_in_range_yell_reaches_further_than_say_at_the_same_distance() {
        // A speaker 200yd away: outside SAY range, inside YELL range — yell's wider radius must
        // not be silently clipped to say range.
        assert!(!chat_in_range(
            0,
            0,
            0.0,
            0.0,
            0,
            0,
            200.0,
            0.0,
            SAY_RANGE_SQ
        ));
        assert!(chat_in_range(
            0,
            0,
            0.0,
            0.0,
            0,
            0,
            200.0,
            0.0,
            YELL_RANGE_SQ
        ));
    }

    /// The text-emote range shares SAY's 25yd radius (`ListenRange.TextEmote` = `ListenRange.Say`
    /// = 25 in cm:mangosd.conf.dist.in:1092): the same boundary math, over the shared constant.
    #[test]
    fn chat_in_range_gates_a_text_emote_at_the_same_25yd_boundary_as_say() {
        let range_sq = lyracore_shared::chat::broadcast_chat::TEXT_EMOTE_RANGE_YD
            * lyracore_shared::chat::broadcast_chat::TEXT_EMOTE_RANGE_YD;
        assert_eq!(range_sq, SAY_RANGE_SQ);
        assert!(chat_in_range(0, 0, 0.0, 0.0, 0, 0, 25.0, 0.0, range_sq));
        assert!(!chat_in_range(0, 0, 0.0, 0.0, 0, 0, 25.01, 0.0, range_sq));
    }

    #[test]
    fn in_combat_flag_change_emits_one() {
        // UNIT_FLAG_IN_COMBAT toggling on unit_flags — any unit, one VALUES packet, guid + the
        // exact post-toggle flags word pinned.
        let old = player_entity();
        let mut new = old.clone();
        new.unit_flags |= lyracore_shared::constants::unit_flags::IN_COMBAT;
        let out = entity_update_to_outbound(&old, &new);
        assert_eq!(
            out,
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_unit_flags_values(new.guid, new.unit_flags)
            ))]
        );
    }

    #[test]
    fn taxi_mount_and_flight_flag_change_emit_one_atomic_presentation_values() {
        let old = player_entity();
        let mut new = old.clone();
        new.mount_display_id = 1147;
        new.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT;
        assert_eq!(
            entity_update_to_outbound(&old, &new),
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_taxi_presentation_values(
                    new.guid,
                    new.mount_display_id,
                    new.unit_flags,
                )
            ))]
        );
    }

    #[test]
    fn taxi_landing_emits_one_atomic_presentation_update_not_a_land_mount_duplicate() {
        let mut old = player_entity();
        old.mount_display_id = 1147;
        old.unit_flags |= lyracore_shared::constants::unit_flags::TAXI_FLIGHT;
        let mut new = old.clone();
        new.mount_display_id = 0;
        new.unit_flags &= !lyracore_shared::constants::unit_flags::TAXI_FLIGHT;
        assert_eq!(
            entity_update_to_outbound(&old, &new),
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_taxi_presentation_values(
                    new.guid,
                    new.mount_display_id,
                    new.unit_flags,
                )
            ))]
        );
    }

    #[test]
    fn land_mount_display_change_emits_the_standalone_builder_not_taxi_presentation() {
        // A land mount changes ONLY `mount_display_id` — TAXI_FLIGHT is untouched — so it must relay
        // through the decoupled single-field builder, not the taxi-coupled one.
        let old = player_entity();
        let mut new = old.clone();
        new.mount_display_id = 1147;
        assert_eq!(
            entity_update_to_outbound(&old, &new),
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_mount_display_values(new.guid, new.mount_display_id)
            ))]
        );
    }

    #[test]
    fn land_dismount_display_change_emits_the_standalone_builder_with_zero() {
        let mut old = player_entity();
        old.mount_display_id = 1147;
        let mut new = old.clone();
        new.mount_display_id = 0; // dismount — TAXI_FLIGHT was never set for a land mount
        assert_eq!(
            entity_update_to_outbound(&old, &new),
            vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_mount_display_values(new.guid, 0)
            ))]
        );
    }

    #[test]
    fn a_land_mount_display_change_alongside_an_unrelated_flag_change_emits_both_packets() {
        // The flag change is real but NOT the taxi flight bit — both relay, separately, neither
        // coupled through the taxi builder.
        let old = player_entity();
        let mut new = old.clone();
        new.mount_display_id = 1147;
        new.unit_flags |= lyracore_shared::constants::unit_flags::IN_COMBAT;
        assert_eq!(
            entity_update_to_outbound(&old, &new),
            vec![
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                    codec::build_mount_display_values(new.guid, new.mount_display_id)
                )),
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_unit_flags_values(
                    new.guid,
                    new.unit_flags
                ))),
            ]
        );
    }

    #[test]
    fn run_speed_mult_bp_change_emits_force_run_speed_change_with_the_derived_speed() {
        // GM playtest `.speed`: a run_speed_mult_bp change relays
        // SMSG_FORCE_RUN_SPEED_CHANGE with speed = BASE_RUN_SPEED (7.0) * bp/10000 — pin the derived
        // value end-to-end (this is the "codec pin for the speed message" this feature calls for).
        let old = player_entity();
        let mut new = old.clone();
        new.run_speed_mult_bp = 30_000; // .speed 3 -> 3x
        let out = entity_update_to_outbound(&old, &new);
        assert_eq!(
            out,
            vec![ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(Box::new(
                codec::build_force_run_speed(new.guid, 21.0)
            ))]
        );
    }

    #[test]
    fn appearance_change_relays_the_wisp_and_the_restored_native_display() {
        let alive = player_entity();
        let mut wisp = alive.clone();
        wisp.display_id = 1825;
        for (old, new) in [(&alive, &wisp), (&wisp, &alive)] {
            assert_eq!(
                entity_update_to_outbound(old, new),
                vec![ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                    codec::build_display_values(new.guid, new.display_id),
                ))]
            );
            assert_eq!(new.native_display_id, old.native_display_id);
        }
    }

    #[test]
    fn run_speed_mult_bp_unchanged_emits_nothing_extra() {
        let old = player_entity();
        let mut new = old.clone();
        new.health = 50; // an unrelated change must not spuriously trigger the speed relay
        let out = entity_update_to_outbound(&old, &new);
        assert!(!out
            .iter()
            .any(|m| matches!(m, ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(_))));
    }

    #[test]
    fn stealth_both_inserts_hide_both_deletes_reveal_created_dedupes() {
        // A Stealth cast applies TWO A_STEALTH auras and the SDK fires both inserts AFTER applying the
        // delta, so both see post-count 2 -> both HIDE (the per-viewer `created` set makes the 2nd a
        // no-op at the call site). A break removes both -> two deletes seeing post-count 0 -> both REVEAL
        // (created-deduped to one). The decision is purely "stealthed -> Hide, not stealthed -> Reveal".
        assert_eq!(stealth_action(true, 1), StealthAction::Hide);
        assert_eq!(stealth_action(true, 2), StealthAction::Hide);
        assert_eq!(stealth_action(true, 3), StealthAction::Hide);
        assert_eq!(stealth_action(false, 1), StealthAction::None); // still stealthed after one of two deletes
        assert_eq!(stealth_action(false, 0), StealthAction::Reveal);
        assert_eq!(stealth_action(false, 2), StealthAction::None); // over-count delete does not reveal
                                                                   // An insert can never leave count 0 (it adds one) -> defensively a no-op.
        assert_eq!(stealth_action(true, 0), StealthAction::None);
    }

    #[test]
    fn creature_relays_target_and_max_vitals() {
        // The target + max-vitals arms are NOT player-gated: a creature relays both, decoded and
        // pinned in emission order (target before max-vitals) with their exact values.
        let mut old = player_entity();
        old.type_mask = 0;
        let mut new = old.clone();
        new.target_guid = 7;
        new.max_power = 50;
        let power_b = (new.unit_bytes_0 >> 24) as u8;
        let out = entity_update_to_outbound(&old, &new);
        assert_eq!(
            out,
            vec![
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_target_values(
                    new.guid, 7
                ))),
                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_max_vitals_values(
                    new.guid,
                    new.max_health,
                    power_b,
                    50
                ))),
            ]
        );
    }

    #[test]
    fn peer_reveal_restores_remaining_auras_after_create() {
        use codec::update_mask::idx;
        use wow_world_messages::vanilla::{Object, UpdateMask, VisibleItemIndex};

        for mut entity in [player_entity(), creature_entity()] {
            entity.unit_bytes_0 = 0x0000_0101;
            let mut remaining = aura(entity.guid, 0xA1, 1, 10, 0, i64::MAX);
            remaining.spell_id = 1459;
            remaining.level = 40;
            remaining.flags = 9;
            let index = world_view::AuraIndex::default();
            index.upsert(0, &remaining);
            let mut other_shard = remaining.clone();
            other_shard.spell_id = 9999;
            index.upsert(1, &other_shard);

            let mut stealth = aura(entity.guid, A_STEALTH, 0, 0, 0, i64::MAX);
            stealth.id = 2;
            stealth.slot = 0;
            let mut second_stealth = stealth.clone();
            second_stealth.id = 3;
            second_stealth.slot = 1;
            index.upsert(0, &stealth);
            index.upsert(0, &second_stealth);
            index.remove(0, &stealth);
            assert_eq!(index.stealth_count(0, entity.guid), 1);
            index.remove(0, &second_stealth);
            assert_eq!(index.stealth_count(0, entity.guid), 0);

            let view = entity_view(entity, 0);
            let out = peer_create_outbound(
                &view,
                &[codec::ItemInstanceView {
                    guid: 700,
                    entry: 1337,
                    enchantment: 823,
                    ..Default::default()
                }],
                &index.on_target(0, view.guid),
                None,
            )
            .unwrap();
            let [Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(create)), Outbound::Raw { opcode, body }] =
                out.as_slice()
            else {
                panic!("a peer needs CREATE followed by its remaining aura array");
            };
            let [Object::CreateObject2 { guid3, mask2, .. }] = create.objects.as_slice() else {
                panic!("the first packet must create the peer");
            };
            assert_eq!(guid3.guid(), view.guid);
            if let UpdateMask::Player(player) = mask2 {
                let visible = player
                    .player_visible_item(VisibleItemIndex::Index0)
                    .unwrap();
                assert_eq!((visible.item, visible.enchants), (1337, [823, 0]));
            }
            assert_eq!(*opcode, 0x00A9);
            let decoded = lyracore_shared::values_mask::parse_values_updates(body);
            assert_eq!(decoded.len(), 1);
            assert_eq!(decoded[0].guid, view.guid);
            let fields: std::collections::HashMap<_, _> =
                decoded[0].fields.iter().copied().collect();
            for slot in 0..idx::AURA_SLOTS {
                assert_eq!(
                    fields[&(idx::UNIT_AURA + slot)],
                    if slot == 3 { 1459 } else { 0 }
                );
            }
            assert_eq!(fields[&idx::UNIT_AURAFLAGS], 0x9000);
            assert_eq!(fields[&idx::UNIT_AURALEVELS], 0x2800_0000);
            assert!(!fields.contains_key(&idx::OBJECT_TYPE));
        }
    }

    #[test]
    fn peer_create_without_auras_needs_no_followup() {
        let mut entity = player_entity();
        entity.unit_bytes_0 = 0x0000_0101;
        let out = peer_create_outbound(&entity_view(entity, 0), &[], &[], None).unwrap();
        assert!(matches!(
            out.as_slice(),
            [Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(_))]
        ));
    }

    #[test]
    fn peer_create_restores_the_wisp_appearance_and_movement_speeds() {
        use lyracore_shared::spell::taxonomy::{A_MOD_SPEED, SPEED_MOVE, SPEED_SWIM};
        use wow_world_messages::vanilla::{Object, UpdateMask};
        let mut entity = player_entity();
        entity.display_id = 1825;
        entity.unit_bytes_0 = 0x0000_0104;
        entity.run_speed_mult_bp = 10_000;
        let view = entity_view(entity, 0);
        let auras = [
            aura(
                view.guid,
                A_MOD_SPEED,
                i32::from(SPEED_MOVE),
                25,
                0,
                i64::MAX,
            ),
            aura(
                view.guid,
                A_MOD_SPEED,
                i32::from(SPEED_MOVE),
                50,
                0,
                i64::MAX,
            ),
            aura(
                view.guid,
                A_MOD_SPEED,
                i32::from(SPEED_SWIM),
                25,
                0,
                i64::MAX,
            ),
            aura(
                view.guid,
                A_MOD_SPEED,
                i32::from(SPEED_SWIM),
                50,
                0,
                i64::MAX,
            ),
        ];
        let out = peer_create_outbound(&view, &[], &auras, None).unwrap();
        let Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(create)) = &out[0] else {
            panic!("peer CREATE");
        };
        let Object::CreateObject2 {
            movement2,
            mask2: UpdateMask::Player(player),
            ..
        } = &create.objects[0]
        else {
            panic!("Character CREATE");
        };
        assert_eq!(player.unit_displayid(), Some(1825));
        let wow_world_messages::vanilla::MovementBlock_UpdateFlag_Living::Living {
            running_speed,
            swimming_speed,
            ..
        } = movement2.update_flag.get_living().unwrap()
        else {
            panic!("living movement");
        };
        assert_eq!(*running_speed, 10.5);
        assert!(
            (*swimming_speed - lyracore_shared::constants::speeds::SWIM * 1.5).abs() < 0.000001
        );
    }

    #[test]
    fn peer_create_uses_the_remaining_ghost_speed_and_restores_baseline_after_removal() {
        use lyracore_shared::spell::taxonomy::{A_MOD_SPEED, SPEED_MOVE, SPEED_SWIM};
        use wow_world_messages::vanilla::{MovementBlock_UpdateFlag_Living, Object};
        let mut entity = player_entity();
        entity.unit_bytes_0 = 0x0000_0101;
        entity.run_speed_mult_bp = 10_000;
        let view = entity_view(entity, 0);
        let ghost = vec![
            aura(
                view.guid,
                A_MOD_SPEED,
                i32::from(SPEED_MOVE),
                25,
                0,
                i64::MAX,
            ),
            aura(
                view.guid,
                A_MOD_SPEED,
                i32::from(SPEED_SWIM),
                25,
                0,
                i64::MAX,
            ),
        ];
        for (auras, expected_run, expected_swim) in
            [(ghost, 8.75, 5.902_777_7), (vec![], 7.0, 4.722_222)]
        {
            let out = peer_create_outbound(&view, &[], &auras, None).unwrap();
            let Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(create)) = &out[0] else {
                panic!("peer CREATE");
            };
            let Object::CreateObject2 { movement2, .. } = &create.objects[0] else {
                panic!("Character CREATE");
            };
            let MovementBlock_UpdateFlag_Living::Living {
                running_speed,
                swimming_speed,
                ..
            } = movement2.update_flag.get_living().unwrap()
            else {
                panic!("living movement");
            };
            assert_eq!(*running_speed, expected_run);
            assert!((*swimming_speed - expected_swim).abs() < 0.000001);
        }
    }

    #[test]
    fn peer_create_preserves_gm_run_speed_without_movement_auras() {
        use wow_world_messages::vanilla::{MovementBlock_UpdateFlag_Living, Object};
        let mut entity = player_entity();
        entity.unit_bytes_0 = 0x0000_0101;
        entity.run_speed_mult_bp = 30_000;
        let view = entity_view(entity, 0);
        let out = peer_create_outbound(&view, &[], &[], None).unwrap();
        let Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(create)) = &out[0] else {
            panic!("peer CREATE");
        };
        let Object::CreateObject2 { movement2, .. } = &create.objects[0] else {
            panic!("Character CREATE");
        };
        let MovementBlock_UpdateFlag_Living::Living { running_speed, .. } =
            movement2.update_flag.get_living().unwrap()
        else {
            panic!("living movement");
        };
        assert_eq!(*running_speed, 21.0);
    }

    #[test]
    fn peer_create_applies_always_and_not_stack_run_bonuses() {
        use lyracore_shared::spell::taxonomy::{
            A_MOD_SPEED, SPEED_MOVE_ALWAYS, SPEED_MOVE_NOT_STACK,
        };
        use wow_world_messages::vanilla::{MovementBlock_UpdateFlag_Living, Object};
        let mut entity = player_entity();
        entity.unit_bytes_0 = 0x0000_0101;
        let view = entity_view(entity, 0);
        for speed_kind in [SPEED_MOVE_ALWAYS, SPEED_MOVE_NOT_STACK] {
            let auras = [aura(
                view.guid,
                A_MOD_SPEED,
                i32::from(speed_kind),
                50,
                0,
                i64::MAX,
            )];
            let out = peer_create_outbound(&view, &[], &auras, None).unwrap();
            let Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(create)) = &out[0] else {
                panic!("peer CREATE");
            };
            let Object::CreateObject2 { movement2, .. } = &create.objects[0] else {
                panic!("Character CREATE");
            };
            let MovementBlock_UpdateFlag_Living::Living { running_speed, .. } =
                movement2.update_flag.get_living().unwrap()
            else {
                panic!("living movement");
            };
            assert_eq!(*running_speed, 10.5);
        }
    }

    #[test]
    fn bag_content_parts_boundaries() {
        assert_eq!(codec::bag_content_parts(119), None); // just below the bag-content range
        assert_eq!(codec::bag_content_parts(120), Some((19, 0))); // first bag, first slot
        assert_eq!(codec::bag_content_parts(137), Some((19, 17))); // first bag, last slot (0..MAX_BAG_SIZE)
        assert_eq!(codec::bag_content_parts(138), Some((20, 0))); // second bag, first slot
        assert_eq!(codec::bag_content_parts(191), Some((22, 17))); // fourth (last) bag, last slot
        assert_eq!(codec::bag_content_parts(192), None);
    }

    fn item_view(guid: u64, entry: u32) -> codec::ItemInstanceView {
        codec::ItemInstanceView {
            guid,
            entry,
            ..Default::default()
        }
    }

    fn item_row(slot: u8, enchant_id: u32) -> ItemInstance {
        ItemInstance {
            guid: 0x4000_0000_0000_0007,
            entry: 1337,
            owner_identity: spacetimedb_sdk::Identity::from_byte_array([0u8; 32]),
            owner_guid: 7,
            slot,
            stack_count: 1,
            durability: 20,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            enchant_id,
            soulbound: false,
            random_property_id: 0,
            item_text_id: 0,
        }
    }

    /// The enchantment word of the item update and the first enchant word of the visible item in an
    /// in-place enchant outbound, with the raw field indexes of every VALUES block.
    fn enchant_words(out: &[Outbound]) -> (Option<u32>, Option<u32>, Vec<Vec<u16>>) {
        use wow_world_messages::vanilla::{Object, ServerMessage, UpdateMask, VisibleItemIndex};
        let (mut item_word, mut visible_word) = (None, None);
        let mut field_indexes = Vec::new();
        for message in out {
            let Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update)) = message else {
                continue;
            };
            let mut bytes = Vec::new();
            update.write_unencrypted_server(&mut bytes).unwrap();
            for values in lyracore_shared::values_mask::parse_values_updates(&bytes[4..]) {
                field_indexes.push(values.fields.iter().map(|&(idx, _)| idx).collect());
            }
            match &update.objects[0] {
                Object::Values {
                    mask1: UpdateMask::Item(item),
                    ..
                } => item_word = item.item_enchantment().map(|word| word as u32),
                Object::Values {
                    mask1: UpdateMask::Player(player),
                    ..
                } => {
                    visible_word = player
                        .player_visible_item(VisibleItemIndex::Index8)
                        .map(|visible| visible.enchants[0])
                }
                _ => {}
            }
        }
        (item_word, visible_word, field_indexes)
    }

    #[test]
    fn enchanting_a_worn_item_in_place_updates_its_enchant_word_and_visible_item() {
        for (stored, client) in [(7745, 823), (7748, 724), (2564, 2564)] {
            let out = enchant_change_outbound(7, &item_row(8, 0), &item_row(8, stored));
            let (item_word, visible_word, fields) = enchant_words(&out);
            assert_eq!(item_word, Some(client), "stored {stored}");
            assert_eq!(visible_word, Some(client), "stored {stored}");
            assert!(
                fields.iter().all(|idx| !idx.contains(&2)),
                "OBJECT_FIELD_TYPE must stay off the partial update"
            );
        }
    }

    #[test]
    fn enchanting_a_bagged_item_in_place_updates_only_its_enchant_word() {
        let out = enchant_change_outbound(7, &item_row(23, 0), &item_row(23, 7745));
        let (item_word, visible_word, _) = enchant_words(&out);
        assert_eq!(item_word, Some(823));
        assert_eq!(visible_word, None);
    }

    #[test]
    fn a_change_that_leaves_the_enchant_alone_sends_no_enchant_word() {
        let mut worn = item_row(8, 7745);
        worn.durability = 3;
        assert!(enchant_change_outbound(7, &item_row(8, 7745), &worn).is_empty());
    }

    #[test]
    fn an_enchant_change_on_a_guild_charter_leaves_its_petition_id_alone() {
        let charter = |enchant_id| ItemInstance {
            entry: lyracore_shared::guild::GUILD_CHARTER_ENTRY,
            ..item_row(23, enchant_id)
        };
        assert!(enchant_change_outbound(7, &charter(0), &charter(7745)).is_empty());
    }

    #[test]
    fn removing_a_permanent_enchant_in_place_clears_the_word() {
        let out = enchant_change_outbound(7, &item_row(8, 7745), &item_row(8, 0));
        let (item_word, visible_word, _) = enchant_words(&out);
        assert_eq!((item_word, visible_word), (Some(0), Some(0)));
    }

    #[test]
    fn a_slot_change_with_an_enchant_change_leaves_the_visible_item_to_the_slot_projection() {
        let out = enchant_change_outbound(7, &item_row(23, 0), &item_row(8, 7745));
        let (item_word, visible_word, _) = enchant_words(&out);
        assert_eq!((item_word, visible_word), (Some(823), None));
    }

    #[test]
    fn a_worn_item_moved_into_a_slot_shows_its_client_enchantment_on_the_visible_item() {
        use wow_world_messages::vanilla::{Object, UpdateMask, VisibleItemIndex};
        let committed = item_row(8, 7748);
        let mut out = Vec::new();
        append_final_item_slots(7, 23, 8, &mut out, |slot| {
            (slot == 8).then(|| view_of_item_row(&committed))
        });
        let mut words = Vec::new();
        for message in &out {
            let Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update)) = message else {
                continue;
            };
            if let Object::Values {
                mask1: UpdateMask::Player(player),
                ..
            } = &update.objects[0]
            {
                words.extend(
                    player
                        .player_visible_item(VisibleItemIndex::Index8)
                        .map(|visible| visible.enchants),
                );
            }
        }
        assert_eq!(words, [[724, 0]]);
    }

    #[test]
    fn occupied_equipment_swap_projects_committed_slot_occupants() {
        use wow_world_messages::vanilla::{Guid, ItemSlot, Object, UpdateMask, VisibleItemIndex};

        const CHARACTER: u64 = 7;
        const EQUIPMENT_SLOT: u8 = 0;
        const BACKPACK_SLOT: u8 = 23;
        const DISPLACED_GUID: u64 = 101;
        const INCOMING_GUID: u64 = 202;
        const INCOMING_ENTRY: u32 = 1337;

        let committed = [
            (EQUIPMENT_SLOT, INCOMING_GUID, INCOMING_ENTRY),
            (BACKPACK_SLOT, DISPLACED_GUID, 42),
        ];
        let item_in_slot = |slot| {
            committed
                .iter()
                .find(|item| item.0 == slot)
                .map(|item| codec::ItemInstanceView {
                    guid: item.1,
                    entry: item.2,
                    ..Default::default()
                })
        };
        let mut outbound = Vec::new();

        append_final_item_slots(
            CHARACTER,
            EQUIPMENT_SLOT,
            BACKPACK_SLOT,
            &mut outbound,
            item_in_slot,
        );
        append_final_item_slots(
            CHARACTER,
            BACKPACK_SLOT,
            EQUIPMENT_SLOT,
            &mut outbound,
            item_in_slot,
        );

        let mut equipment = Vec::new();
        let mut backpack = Vec::new();
        let mut visible_head = Vec::new();
        for message in &outbound {
            let Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update)) = message else {
                continue;
            };
            let Object::Values {
                mask1: UpdateMask::Player(player),
                ..
            } = &update.objects[0]
            else {
                continue;
            };
            equipment.extend(player.player_field_inv(ItemSlot::Head));
            backpack.extend(player.player_field_inv(ItemSlot::Inventory0));
            visible_head.extend(
                player
                    .player_visible_item(VisibleItemIndex::Index0)
                    .map(|item| item.item),
            );
        }

        assert_eq!(equipment, [Guid::new(INCOMING_GUID)]);
        assert_eq!(backpack, [Guid::new(DISPLACED_GUID)]);
        assert_eq!(visible_head, [INCOMING_ENTRY]);
        assert!(!equipment.contains(&Guid::new(0)));
        assert!(!backpack.contains(&Guid::new(0)));
    }

    #[test]
    fn empty_equipment_destination_clears_backpack_and_sets_equipment() {
        use wow_world_messages::vanilla::{Guid, ItemSlot, Object, UpdateMask};

        let mut outbound = Vec::new();
        append_final_item_slots(7, 23, 0, &mut outbound, |slot| {
            (slot == 0).then(|| item_view(202, 1337))
        });

        let mut equipment = Vec::new();
        let mut backpack = Vec::new();
        for message in &outbound {
            let Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update)) = message else {
                continue;
            };
            let Object::Values {
                mask1: UpdateMask::Player(player),
                ..
            } = &update.objects[0]
            else {
                continue;
            };
            equipment.extend(player.player_field_inv(ItemSlot::Head));
            backpack.extend(player.player_field_inv(ItemSlot::Inventory0));
        }

        assert_eq!(equipment, [Guid::new(202)]);
        assert_eq!(backpack, [Guid::new(0)]);
    }

    #[test]
    fn ordinary_backpack_move_clears_source_and_sets_destination() {
        use wow_world_messages::vanilla::{Guid, ItemSlot, Object, UpdateMask};

        let mut outbound = Vec::new();
        append_final_item_slots(7, 23, 24, &mut outbound, |slot| {
            (slot == 24).then(|| item_view(202, 1337))
        });

        let mut source = Vec::new();
        let mut destination = Vec::new();
        for message in &outbound {
            let Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(update)) = message else {
                continue;
            };
            let Object::Values {
                mask1: UpdateMask::Player(player),
                ..
            } = &update.objects[0]
            else {
                continue;
            };
            source.extend(player.player_field_inv(ItemSlot::Inventory0));
            destination.extend(player.player_field_inv(ItemSlot::Inventory1));
        }

        assert_eq!(source, [Guid::new(0)]);
        assert_eq!(destination, [Guid::new(202)]);
    }

    #[test]
    fn stealth_dedup_composes_with_a_real_hashset_exactly_once() {
        // `stealth_visibility` pairs `stealth_action`'s Hide/Reveal decision with `created`'s
        // remove()/insert() return value (only act if the set's membership actually changed) — this
        // composes that SAME discipline over a real `std::collections::HashSet<u64>` (not a mock) to
        // pin the "exactly once" guarantee a Stealth cast's two A_STEALTH inserts/deletes rely on.
        let target = 99u64;
        let mut created: HashSet<u64> = HashSet::from([target]); // starts visible
        let mut destroys = 0;
        // A Stealth cast applies TWO A_STEALTH auras; the SDK fires both inserts AFTER applying the
        // whole delta, so both see post-count 2 -> both decide Hide.
        for count in [2, 2] {
            if stealth_action(true, count) == StealthAction::Hide && created.remove(&target) {
                destroys += 1;
            }
        }
        assert_eq!(destroys, 1, "two HIDE decisions must destroy exactly once");
        assert!(!created.contains(&target));
        // A break removes both auras; both deletes see post-count 0 -> both decide Reveal.
        let mut creates = 0;
        for count in [0, 0] {
            if stealth_action(false, count) == StealthAction::Reveal && created.insert(target) {
                creates += 1;
            }
        }
        assert_eq!(
            creates, 1,
            "two REVEAL decisions must (re)create exactly once"
        );
        assert!(created.contains(&target));
    }

    // ---- Cross-map teleport relay --------------------------------------------

    #[test]
    fn teleport_relay_same_map_is_byte_identical_to_the_pre_224_ack() {
        let out = build_teleport_relay(true, 7, 0, 1.0, 2.0, 3.0, 0.5).unwrap();
        match out {
            Outbound::One(ServerOpcodeMessage::MSG_MOVE_TELEPORT_ACK(m)) => {
                assert_eq!(m.guid.guid(), 7);
                assert_eq!(
                    m.info.position,
                    wow_world_messages::vanilla::Vector3d {
                        x: 1.0,
                        y: 2.0,
                        z: 3.0
                    }
                );
                assert_eq!(m.info.orientation, 0.5);
            }
            _ => panic!("expected a same-map relay to be a single MSG_MOVE_TELEPORT_ACK"),
        }
    }

    #[test]
    fn teleport_relay_cross_map_sends_transfer_pending_then_new_world() {
        let out = build_teleport_relay(false, 7, 36, 10.0, 20.0, 30.0, 1.5).unwrap();
        match out {
            Outbound::Batch(msgs) => {
                assert_eq!(msgs.len(), 2, "TRANSFER_PENDING + NEW_WORLD, nothing else");
                match &msgs[0] {
                    ServerOpcodeMessage::SMSG_TRANSFER_PENDING(p) => {
                        assert_eq!(p.map.as_int(), 36);
                        assert!(p.has_transport.is_none());
                    }
                    _ => panic!("expected SMSG_TRANSFER_PENDING first"),
                }
                match &msgs[1] {
                    ServerOpcodeMessage::SMSG_NEW_WORLD(w) => {
                        assert_eq!(w.map.as_int(), 36);
                        assert_eq!(
                            w.position,
                            wow_world_messages::vanilla::Vector3d {
                                x: 10.0,
                                y: 20.0,
                                z: 30.0
                            }
                        );
                        assert_eq!(w.orientation, 1.5);
                    }
                    _ => panic!("expected SMSG_NEW_WORLD second"),
                }
            }
            _ => {
                panic!("expected a cross-map relay to be a Batch of [TRANSFER_PENDING, NEW_WORLD]")
            }
        }
    }

    #[test]
    fn teleport_relay_rejects_a_map_id_the_client_enum_does_not_know() {
        // `Map` is a closed vanilla enum; an id past its known range must surface as an Err (never
        // silently drop the whole relay or panic).
        assert!(build_teleport_relay(false, 7, u32::MAX, 0.0, 0.0, 0.0, 0.0).is_err());
    }

    // ---- AOI re-entry, initial-apply, instance isolation --------------
    //
    // `offer_peer_create`/`is_update_reentry` themselves need a live SDK connection (`RemoteTables`
    // is only constructible off a real subscription) to exercise end-to-end, so — per this repo's
    // established convention (`ghost_transition`, `stealth_action`, `entity_update_to_outbound`) —
    // the DECISION logic they're built from is extracted pure (`peer_create_gate`,
    // `is_update_reentry`) and tested directly here; both are the ACTUAL functions
    // `offer_peer_create`/the `on_update` callback call, not a parallel re-implementation.

    #[test]
    fn peer_create_gate_excludes_self_regardless_of_other_fields() {
        // Self is never offered a CREATE of its own row (the dedup set is pre-seeded with
        // self_guid instead) — true even if every other gate would otherwise pass.
        assert!(!peer_create_gate(1, 1, 0, 0, 0, true, false));
    }

    #[test]
    fn peer_create_gate_excludes_a_different_instance_work_item_190() {
        // Same instance (both open-world 0, or both some future dungeon instance N) -> visible.
        assert!(peer_create_gate(2, 1, 0, 0, 0, false, false));
        assert!(peer_create_gate(2, 1, 7, 7, 0, false, false));
        // Different instance (e.g. a dungeon party's row vs. an open-world viewer, or two
        // different parties' instances of the same dungeon) -> excluded. This is the gate the
        // The login sweep and the re-entry path BOTH sit behind — a cross-instance row must
        // never be marked `created`, even transiently, by either path.
        assert!(!peer_create_gate(2, 1, 7, 0, 0, false, false));
        assert!(!peer_create_gate(2, 1, 3, 7, 0, false, false));
    }

    #[test]
    fn peer_create_gate_spirithealer_and_stealth_gates_unaffected_by_the_refactor() {
        const SPIRITHEALER: u32 = lyracore_shared::constants::npc_flags::SPIRITHEALER;
        // A spirit healer is invisible to a living viewer...
        assert!(!peer_create_gate(2, 1, 0, 0, SPIRITHEALER, false, false));
        assert!(peer_create_gate(2, 1, 0, 0, SPIRITHEALER, true, false));
        // A non-spirit-healer entity is unaffected by the viewer's ghost state.
        assert!(peer_create_gate(2, 1, 0, 0, 0, false, false));
        // A currently-stealthed peer is excluded regardless of everything else.
        assert!(!peer_create_gate(2, 1, 0, 0, 0, false, true));
    }

    #[test]
    fn reentry_after_leaving_aoi_recreates_exactly_once_work_item_144() {
        // The exact re-entry scenario: observer sees peer (CREATE) -> peer leaves the AOI box
        // (on_delete evicts it from `created`, sends DESTROY) -> peer returns, delivered as an
        // UPDATE of the SDK's still-cached row (not a fresh on_insert) -> the observer must
        // re-CREATE it, not silently drop it forever.
        let self_guid = 1u64;
        let peer = 42u64;
        let mut created: HashSet<u64> = HashSet::from([self_guid, peer]); // currently visible

        // Leave: on_delete's `if del_created.lock().unwrap().remove(&row.guid)` — must actually
        // evict, or the later re-entry check below sees a false "already shown".
        assert!(
            created.remove(&peer),
            "on_delete must evict the leaving peer from `created`"
        );
        assert!(!created.contains(&peer));

        // Re-entry arrives as an on_update, not on_insert (the SDK's refcounted cache still holds
        // the row). The callback's actual dispatch predicate must say "treat as re-entry".
        assert!(is_update_reentry(peer, self_guid, created.contains(&peer)));

        // `offer_peer_create`'s gate passes (same instance, not stealthed, not a spirit healer) ->
        // the dedup insert succeeds -> exactly one CREATE goes out.
        assert!(peer_create_gate(peer, self_guid, 0, 0, 0, false, false));
        assert!(
            created.insert(peer),
            "the re-entry offer must (re)insert into `created`"
        );

        // A THIRD sighting of the same still-visible peer (an ordinary field update, e.g. a
        // heartbeat) must NOT be treated as another re-entry — it's already shown, so it diffs
        // normally instead of re-CREATEing (which would desync the client).
        assert!(!is_update_reentry(peer, self_guid, created.contains(&peer)));

        // Self never triggers the re-entry branch, even if (hypothetically) it were ever evicted.
        assert!(!is_update_reentry(self_guid, self_guid, false));
    }

    #[test]
    fn login_resident_sweep_creates_every_pre_existing_peer_exactly_once_work_item_145() {
        // The login-sweep scenario: a fresh login's AOI box already contains peers/creatures (rows
        // resident in the connection's cache from the just-applied subscription) whose
        // `on_insert` may or may not have fired. The sweep in `subscribe_player_events` offers
        // every resident row through this SAME gate + dedup — model it directly.
        let self_guid = 1u64;
        let residents = [2u64, 3, 4]; // pre-existing peers already in the login AOI box
        let mut created: HashSet<u64> = HashSet::from([self_guid]);

        let mut creates_sent = 0;
        for guid in residents {
            if peer_create_gate(guid, self_guid, 0, 0, 0, false, false) && created.insert(guid) {
                creates_sent += 1;
            }
        }
        assert_eq!(
            creates_sent, 3,
            "every pre-existing resident must get a CREATE at login, not just self (the login-sweep bug: \
             only self + gameobjects appeared)"
        );
        assert!(residents.iter().all(|g| created.contains(g)));

        // Idempotency (the sweep's own correctness claim): if the SDK ALSO fired an apply-time
        // on_insert for one resident before the sweep runs, the sweep must not double-create it.
        creates_sent = 0;
        for guid in residents {
            if peer_create_gate(guid, self_guid, 0, 0, 0, false, false) && created.insert(guid) {
                creates_sent += 1;
            }
        }
        assert_eq!(
            creates_sent, 0,
            "a second sweep over already-created residents must be a no-op"
        );
    }

    #[test]
    fn login_resident_sweep_excludes_cross_instance_residents_work_item_190() {
        // A row resident in the connection's cache from a DIFFERENT instance than the viewer
        // (always a no-op today since every entity is instance 0, but the
        // gate must already hold the line) must never be swept into `created`.
        let self_guid = 1u64;
        let viewer_instance_id = 0u64;
        let same_instance_peer = 2u64;
        let other_instance_peer = 3u64;
        let mut created: HashSet<u64> = HashSet::from([self_guid]);

        for (guid, row_instance) in [(same_instance_peer, 0u64), (other_instance_peer, 7u64)] {
            if peer_create_gate(
                guid,
                self_guid,
                row_instance,
                viewer_instance_id,
                0,
                false,
                false,
            ) {
                created.insert(guid);
            }
        }
        assert!(created.contains(&same_instance_peer));
        assert!(
            !created.contains(&other_instance_peer),
            "a cross-instance resident must stay excluded"
        );
    }

    #[test]
    fn corpse_and_gameobject_creates_relay_only_within_the_viewers_instance_work_item_190_slice_2()
    {
        // Open-world rows to an open-world viewer: byte-identical to the previous behavior (always relayed).
        assert!(instance_relay_gate(0, Some(0)));
        // A per-instance GO copy / in-instance corpse reaches ONLY viewers inside that instance…
        assert!(instance_relay_gate(7, Some(7)));
        assert!(
            !instance_relay_gate(7, Some(0)),
            "an open-world viewer must not see instance 7's rows"
        );
        assert!(
            !instance_relay_gate(7, Some(9)),
            "another party's instance must not see them either"
        );
        // …and an instanced viewer no longer receives the static instance-0 originals (they are the
        // COPY SOURCES on the dungeon map — the copies are what render inside the run).
        assert!(!instance_relay_gate(0, Some(7)));
        // Unknown viewer instance (entity not resident — the AOI-on login window) relays NOTHING,
        // not even instance 0: suppress-then-sweep, never guess.
        assert!(!instance_relay_gate(0, None));
        assert!(!instance_relay_gate(7, None));
    }

    // ── Peer-motion relay: the CALL SITE, not just the codec helper ──────────
    //
    // §8 of the agent playbook: the dominant failure here is a test that pins an extracted helper
    // while the wiring that calls it stays free to vanish. `codec::tests` proves the raw builder
    // emits the same bytes the typed path did; these tests drive `relay_entity_motion` /
    // `relay_creature_leg` themselves — a real `SessionTx`, a real `created` set, a real row —
    // so deleting the send, sending the wrong variant, passing the wrong guid or opcode, or dropping
    // a guard all show up here.

    /// `MOTION_CALLS`/`SENT`/`DROPPED` are process-global atomics and the test binary runs tests in
    /// PARALLEL, so any test that moves them or measures a delta around them must hold this lock.
    /// Without it, the delta a counter test measures silently includes whatever a concurrent relay
    /// test did (which is exactly how the shed test below first turned the motion-counters test red). Poison is
    /// ignored deliberately: a panicking test has already failed and must not cascade into every
    /// other one.
    static MOTION_COUNTERS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Test shims for the two relays the shared-connection model split in half.
    ///
    /// Before the shared-connection model, each of these pushed straight onto the session channel
    /// from the per-player SDK
    /// callback. Now the SHARED dispatch (`stdb::world_view`) makes the shed decision on the pump
    /// and enqueues a job, and the job calls the `*_outbound` half on the session's writer thread.
    /// These shims are that pair, back to back, so the wire-pinning and counter tests below keep
    /// asserting on the queue exactly as they did — and keep failing if either half regresses.
    fn relay_entity_motion(
        tx: &SessionTx,
        created: &Arc<Mutex<HashSet<u64>>>,
        self_guid: u64,
        row: &EntityMotion,
    ) {
        if shed_motion_at_depth(tx.depth()) {
            MOTION_DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        for o in motion_outbound(created, self_guid, row) {
            if tx.send(o).is_err() {
                MOTION_DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    fn relay_creature_leg(
        tx: &SessionTx,
        created: &Arc<Mutex<HashSet<u64>>>,
        row: &CreatureSpline,
    ) {
        if shed_motion_at_depth(tx.depth()) {
            MOTION_DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return;
        }
        for o in creature_leg_outbound(created, row) {
            if tx.send(o).is_err() {
                MOTION_DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    fn motion_counter_lock() -> std::sync::MutexGuard<'static, ()> {
        MOTION_COUNTERS.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One motion row for `guid`, carrying the wire-form `MovementInfo` the module stores.
    fn motion_row(guid: u64, opcode: u16, movement_info: Vec<u8>) -> EntityMotion {
        EntityMotion {
            guid,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            cell: lyracore_shared::spatial::grid_cell_id(0, 0),
            opcode,
            movement_info,
            seq: 0,
        }
    }

    fn taxi_row(character_guid: u64) -> TaxiPassengerSpline {
        TaxiPassengerSpline {
            character_guid,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            cell: lyracore_shared::spatial::grid_cell_id(0, 0),
            start_x: 1.0,
            start_y: 2.0,
            start_z: 3.0,
            points: vec![4.0, 5.0, 6.0, 7.0, 8.0, 9.0],
            duration_ms: 1_000,
            spline_id: 10,
        }
    }

    fn creature_spline(guid: u64, start_micros: u64, dur_ms: u32) -> CreatureSpline {
        CreatureSpline {
            guid,
            start_micros,
            dur_ms,
            sx: 10.0,
            sy: 20.0,
            sz: 30.0,
            dx: 30.0,
            dy: 40.0,
            dz: 50.0,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            spline_id: 10,
            run: true,
            cell: lyracore_shared::spatial::grid_cell_id(0, 0),
            facing: false,
            facing_angle: 0.0,
            path: None,
        }
    }

    fn creature_create_outbound() -> Vec<Outbound> {
        let mut creature = creature_entity();
        creature.unit_bytes_0 = 0x0000_0101;
        peer_create_outbound(&entity_view(creature, 0), &[], &[], None).unwrap()
    }

    #[test]
    fn taxi_spline_reaches_self_and_only_created_observers() {
        let row = taxi_row(42);
        let created = Mutex::new(HashSet::new());
        assert_eq!(taxi_spline_outbound(&created, 42, &row).len(), 1);
        assert!(taxi_spline_outbound(&created, 7, &row).is_empty());

        created.lock().unwrap().insert(42);
        assert_eq!(taxi_spline_outbound(&created, 7, &row).len(), 1);
    }

    #[test]
    fn malformed_taxi_spline_is_not_written_to_a_client() {
        let mut row = taxi_row(42);
        row.points.pop();
        assert!(taxi_spline_outbound(&Mutex::new(HashSet::new()), 42, &row).is_empty());
    }

    #[test]
    fn resident_taxi_spline_is_chained_after_passenger_create() {
        let row = taxi_row(42);
        let created = Mutex::new(HashSet::from([42]));
        let mut outbound = vec![Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
            codec::build_destroy_object(99),
        ))];

        append_resident_taxi_after_create(&mut outbound, &created, 7, &row);

        assert!(matches!(outbound.first(), Some(Outbound::One(_))));
        assert!(matches!(outbound.get(1), Some(Outbound::Raw { opcode, .. }) if *opcode == 0x00DD));
    }

    #[test]
    fn an_empty_retained_path_stops_the_client_instead_of_sending_invalid_geometry() {
        let created = Mutex::new(HashSet::from([99]));
        let mut row = creature_spline(99, 1_000_000, 1_000);
        row.path = Some(crate::stdb::bindings::CreaturePath {
            navigation: crate::stdb::bindings::NavigationInputs {
                imported_revision: None,
                navigation_enabled: false,
                collision_enabled: false,
                coverage_enabled: false,
                static_generation: None,
                coverage_generation: None,
            },
            points: Vec::new(),
        });
        let outbound = creature_leg_outbound(&created, &row);
        let [Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(stop))] = outbound.as_slice()
        else {
            panic!("malformed paths need a stop");
        };
        assert_eq!(stop.duration, 0);
        assert_eq!(stop.splines[0], stop.spline_point);
    }

    #[test]
    fn resident_waypoint_path_resumes_after_the_turn() {
        let guid = 99;
        let created = Mutex::new(HashSet::from([guid]));
        let mut row = creature_spline(guid, 1_000_000, 7_000);
        (row.sx, row.sy, row.sz) = (0.0, 0.0, 0.0);
        (row.dx, row.dy, row.dz) = (3.0, 4.0, 0.0);
        row.path = Some(crate::stdb::bindings::CreaturePath {
            navigation: crate::stdb::bindings::NavigationInputs {
                imported_revision: None,
                navigation_enabled: false,
                collision_enabled: false,
                coverage_enabled: false,
                static_generation: None,
                coverage_generation: None,
            },
            points: vec![
                crate::stdb::bindings::CreaturePathPoint {
                    x: 3.0,
                    y: 0.0,
                    z: 0.0,
                },
                crate::stdb::bindings::CreaturePathPoint {
                    x: 3.0,
                    y: 4.0,
                    z: 0.0,
                },
            ],
        });
        let mut outbound = Vec::new();
        append_resident_creature_after_create(&mut outbound, &created, 0, Some(&row), 4_500_000);
        let [Outbound::Raw { opcode, body }] = outbound.as_slice() else {
            panic!("expected a retained path");
        };
        assert_eq!(*opcode, 0xdd);
        assert_eq!(&body[..2], &[1, 99]);
        let f32_at = |offset| f32::from_le_bytes(body[offset..offset + 4].try_into().unwrap());
        let u32_at = |offset| u32::from_le_bytes(body[offset..offset + 4].try_into().unwrap());
        assert_eq!((f32_at(2), f32_at(6), f32_at(10)), (3.0, 0.5, 0.0));
        assert_eq!(u32_at(23), 3_500);
        assert_eq!(u32_at(27), 1);
        assert_eq!((f32_at(31), f32_at(35), f32_at(39)), (3.0, 4.0, 0.0));
        assert_eq!(body.len(), 43);
    }

    #[test]
    fn resident_creature_replay_follows_create_from_the_current_point() {
        let guid = 99;
        let created = Mutex::new(HashSet::from([guid]));
        let row = creature_spline(guid, 1_000_000, 1_000);
        let mut outbound = creature_create_outbound();

        append_resident_creature_after_create(&mut outbound, &created, 0, Some(&row), 1_250_500);

        let [Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(create)), Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(movement))] =
            outbound.as_slice()
        else {
            panic!("a resident creature needs CREATE followed by MONSTER_MOVE");
        };
        let [wow_world_messages::vanilla::Object::CreateObject2 { guid3, .. }] =
            create.objects.as_slice()
        else {
            panic!("the first packet must create the resident creature");
        };
        assert_eq!(guid3.guid(), guid);
        assert_eq!(movement.guid.guid(), guid);
        assert!((movement.spline_point.x - 15.01).abs() < 0.0001);
        assert!((movement.spline_point.y - 25.01).abs() < 0.0001);
        assert!((movement.spline_point.z - 35.01).abs() < 0.0001);
        assert_eq!(
            movement.splines,
            vec![Vector3d {
                x: 30.0,
                y: 40.0,
                z: 50.0,
            }]
        );
        assert_eq!(
            movement.duration, 750,
            "sub-millisecond remainder rounds up so replay cannot move faster than the durable leg"
        );
    }

    #[test]
    fn resident_creature_replay_skips_absent_expired_and_ineligible_legs() {
        let guid = 99;
        let row = creature_spline(guid, 1_000_000, 1_000);
        let mut outbound = Vec::new();

        append_resident_creature_after_create(
            &mut outbound,
            &Mutex::new(HashSet::from([guid])),
            0,
            None,
            1_250_000,
        );
        append_resident_creature_after_create(
            &mut outbound,
            &Mutex::new(HashSet::new()),
            0,
            Some(&row),
            1_250_000,
        );
        append_resident_creature_after_create(
            &mut outbound,
            &Mutex::new(HashSet::from([guid])),
            1,
            Some(&row),
            1_250_000,
        );
        append_resident_creature_after_create(
            &mut outbound,
            &Mutex::new(HashSet::from([guid])),
            0,
            Some(&row),
            2_000_000,
        );

        assert!(outbound.is_empty());
    }

    #[test]
    fn resident_facing_replay_follows_create_even_after_its_timestamp() {
        let guid = 99;
        let created = Mutex::new(HashSet::from([guid]));
        let mut row = creature_spline(guid, 1_000_000, 0);
        row.facing = true;
        row.facing_angle = 2.1;
        let mut outbound = creature_create_outbound();

        append_resident_creature_after_create(&mut outbound, &created, 0, Some(&row), 9_000_000);

        let [Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(_)), Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(movement))] =
            outbound.as_slice()
        else {
            panic!("a resident creature needs CREATE followed by its facing state");
        };
        assert_eq!(
            movement.move_type,
            wow_world_messages::vanilla::SMSG_MONSTER_MOVE_MonsterMoveType::FacingAngle {
                angle: 2.1,
            }
        );
    }

    /// A running-forward movement block plus the bytes the module would have stored for it.
    fn moving_forward() -> (wow_world_messages::vanilla::MovementInfo, Vec<u8>) {
        let info = wow_world_messages::vanilla::MovementInfo {
            flags: wow_world_messages::vanilla::MovementInfo_MovementFlags::new_forward(),
            timestamp: 0x0BAD_F00D,
            position: Vector3d {
                x: -8949.95,
                y: -132.493,
                z: 83.5312,
            },
            orientation: 2.5,
            fall_time: 0.0,
        };
        let bytes = codec::movement_info_to_bytes(&info).unwrap();
        (info, bytes)
    }

    /// What the observer's writer actually receives for one peer heartbeat: an `Outbound::Raw` whose
    /// opcode and body equal what the TYPED path would have serialized. The typed
    /// message is built here independently of the raw builder, so this fails if the relay passes the
    /// wrong guid, the wrong opcode, the row's bytes in the wrong place — or stops sending at all.
    #[test]
    fn the_motion_relay_queues_the_bytes_the_typed_path_would_have_sent() {
        let _counters = motion_counter_lock();
        let (info, bytes) = moving_forward();
        let (tx, rx) = crate::world::SessionTx::with_depth(0);
        let created = Arc::new(Mutex::new(HashSet::from([42u64])));
        let opcode = lyracore_shared::opcodes::movement::MSG_MOVE_START_FORWARD;

        relay_entity_motion(&tx, &created, 1, &motion_row(42, opcode as u16, bytes));

        let mut framed = Vec::new();
        codec::build_movement_relay(opcode, 42, info)
            .unwrap()
            .write_unencrypted_server(&mut framed)
            .unwrap();
        match rx
            .try_recv()
            .expect("the observer's writer must have been handed a packet")
        {
            Outbound::Raw { opcode, body } => {
                assert_eq!(
                    opcode,
                    u16::from_le_bytes([framed[2], framed[3]]),
                    "relayed opcode"
                );
                assert_eq!(
                    body,
                    &framed[4..],
                    "relayed body must match the typed serialization"
                );
            }
            Outbound::One(m) => panic!("peer motion must ride Outbound::Raw, got a typed {m}"),
            Outbound::Batch(v) => panic!(
                "peer motion must ride Outbound::Raw, got a {}-batch",
                v.len()
            ),
            Outbound::Job(_) => panic!("peer motion must be encoded by the time it is queued"),
        }
        assert!(rx.try_recv().is_err(), "exactly one packet per motion row");
    }

    /// The two guards that were free before the raw path and must stay: a player never receives
    /// their OWN movement echoed back (they would fight the server for authority over their
    /// position), and no `MSG_MOVE_*` goes out for a guid this client never got a CREATE for
    /// (a movement packet for an unknown entity desyncs the client).
    #[test]
    fn the_motion_relay_skips_self_and_uncreated_movers() {
        let _counters = motion_counter_lock();
        let (_, bytes) = moving_forward();
        let opcode = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16;
        let (tx, rx) = crate::world::SessionTx::with_depth(0);
        let created = Arc::new(Mutex::new(HashSet::from([42u64, 7u64])));

        relay_entity_motion(&tx, &created, 7, &motion_row(7, opcode, bytes.clone()));
        assert!(
            rx.try_recv().is_err(),
            "a player's own movement must never be echoed back"
        );

        relay_entity_motion(&tx, &created, 7, &motion_row(999, opcode, bytes));
        assert!(
            rx.try_recv().is_err(),
            "no MSG_MOVE for a guid this client never got a CREATE for"
        );
    }

    /// A malformed row must produce no packet at all — the raw path memcpys the block, so a short
    /// one would be framed with a `size` the client trusts and would desync its header stream.
    #[test]
    fn the_motion_relay_drops_a_truncated_movement_block() {
        let _counters = motion_counter_lock();
        let (_, bytes) = moving_forward();
        let (tx, rx) = crate::world::SessionTx::with_depth(0);
        let created = Arc::new(Mutex::new(HashSet::from([42u64])));
        let opcode = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16;

        relay_entity_motion(
            &tx,
            &created,
            1,
            &motion_row(42, opcode, bytes[..20].to_vec()),
        );
        assert!(
            rx.try_recv().is_err(),
            "a truncated movement block must not be framed"
        );
    }

    ///
    /// Deltas rather than absolutes because the counter is a process-global atomic; this is the only
    /// test that increments it, so the deltas are exact.
    #[test]
    fn a_discarded_relay_send_increments_the_dropped_counter_287() {
        let _counters = motion_counter_lock();
        use std::sync::atomic::Ordering::Relaxed;
        let (_, bytes) = moving_forward();
        let created = Arc::new(Mutex::new(HashSet::from([42u64])));
        let opcode = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16;
        let before = MOTION_DROPPED.load(Relaxed);

        // A live receiver: delivered, so nothing is counted as dropped.
        let (tx, rx) = crate::world::SessionTx::with_depth(0);
        relay_entity_motion(&tx, &created, 1, &motion_row(42, opcode, bytes.clone()));
        assert!(rx.try_recv().is_ok());
        assert_eq!(
            MOTION_DROPPED.load(Relaxed),
            before,
            "a delivered packet is not a drop"
        );

        // The writer half is gone (the session ended) — the send is discarded and MUST be counted.
        let (dead_tx, dead_rx) = crate::world::SessionTx::with_depth(0);
        drop(dead_rx);
        relay_entity_motion(&dead_tx, &created, 1, &motion_row(42, opcode, bytes));
        assert_eq!(
            MOTION_DROPPED.load(Relaxed),
            before + 1,
            "a peer-motion packet discarded by a dead writer channel must increment MOTION_DROPPED"
        );

        // The sibling relay discards the same way (creature legs share the writer channel).
        relay_creature_leg(
            &dead_tx,
            &created,
            &CreatureSpline {
                guid: 42,
                start_micros: 0,
                dur_ms: 500,
                sx: 1.0,
                sy: 2.0,
                sz: 3.0,
                dx: 4.0,
                dy: 5.0,
                dz: 6.0,
                map_id: 0,
                instance_id: 0,
                grid_x: 0,
                grid_y: 0,
                spline_id: 1,
                run: true,
                cell: lyracore_shared::spatial::grid_cell_id(0, 0),
                facing: false,
                facing_angle: 0.0,
                path: None,
            },
        );
        assert_eq!(
            MOTION_DROPPED.load(Relaxed),
            before + 2,
            "a creature leg discarded by a dead writer channel must increment MOTION_DROPPED too"
        );
    }

    /// a `facing`-flagged `game_creature_spline` row relays as `SMSG_MONSTER_MOVE`'s
    /// `FacingAngle` variant, not the `Normal` one every other leg uses — the whole point being that
    /// a stationary creature otherwise has no wire-level way to tell the client its heading changed.
    #[test]
    fn a_facing_leg_relays_as_the_facing_angle_variant() {
        let created = Arc::new(Mutex::new(HashSet::from([42u64])));
        let row = CreatureSpline {
            guid: 42,
            start_micros: 0,
            dur_ms: 0,
            sx: 10.0,
            sy: 20.0,
            sz: 30.0,
            dx: 10.0,
            dy: 20.0,
            dz: 30.0,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            spline_id: 7,
            run: false,
            cell: lyracore_shared::spatial::grid_cell_id(0, 0),
            facing: true,
            facing_angle: 2.1,
            path: None,
        };
        let out = creature_leg_outbound(&created, &row);
        assert_eq!(out.len(), 1);
        let Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(msg)) = &out[0] else {
            panic!("expected SMSG_MONSTER_MOVE");
        };
        assert_eq!(
            msg.move_type,
            wow_world_messages::vanilla::SMSG_MONSTER_MOVE_MonsterMoveType::FacingAngle {
                angle: 2.1
            },
            "facing leg must use the FacingAngle variant, not Normal"
        );
        assert_eq!(msg.duration, 0);

        // A non-facing leg keeps the Normal variant unchanged.
        let mut normal_row = row.clone();
        normal_row.facing = false;
        normal_row.dur_ms = 500;
        let out = creature_leg_outbound(&created, &normal_row);
        let Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(msg)) = &out[0] else {
            panic!("expected SMSG_MONSTER_MOVE");
        };
        assert_eq!(
            msg.move_type,
            wow_world_messages::vanilla::SMSG_MONSTER_MOVE_MonsterMoveType::Normal
        );
    }

    /// The operator-facing half of the delivery instrument: the 10-second line must carry the
    /// delivery ratio, the
    /// per-movement fan-out, and the dropped count — the three numbers the 371-client run had to
    /// reconstruct by hand afterwards. Pinned as text because that text IS the instrument.
    #[test]
    fn the_motionstat_line_reports_delivery_dropped_and_fanout_287() {
        // 1000 callbacks, 900 packets queued, 7 discarded, from 10 submitted movements.
        let line = motionstat_line(5000, 4500, 21, 1000, 900, 7, 10, 400);
        assert!(
            line.contains("calls=5000"),
            "cumulative calls missing: {line}"
        );
        assert!(
            line.contains("sent=4500"),
            "cumulative sent missing: {line}"
        );
        assert!(
            line.contains("dropped=21"),
            "cumulative dropped count missing: {line}"
        );
        assert!(
            line.contains("+7 in 10s"),
            "the window's dropped delta missing: {line}"
        );
        assert!(
            line.contains("delivery=90.0%"),
            "delivery ratio missing/wrong: {line}"
        );
        assert!(
            line.contains("fanout=100.0/move"),
            "per-movement fan-out missing/wrong: {line}"
        );
        assert!(
            line.contains("submitted=400"),
            "submitted count missing: {line}"
        );
        assert!(
            !line.contains("completed="),
            "retired completed count remains: {line}"
        );
        assert!(
            !line.contains("outstanding="),
            "retired outstanding count remains: {line}"
        );

        // An idle window must not divide by zero or print a fake 0 %.
        let idle = motionstat_line(5000, 4500, 21, 0, 0, 0, 0, 400);
        assert!(
            idle.contains("delivery=--") && idle.contains("fanout=--/move"),
            "{idle}"
        );
    }

    // ── Fan-out collapse signal ─────────────────────────────────────────────────────────────────

    /// The shared ratio: observers per submitted movement, and `None` — never a fake 0.0 — for a
    /// window with no movement in it. The `MOTIONSTAT` line and the warning both read this, which is
    /// the point: a number the operator sees and a number the gateway warns on cannot disagree.
    #[test]
    fn fanout_ratio_is_observers_per_move_and_none_when_idle() {
        assert_eq!(fanout_ratio(3710, 10), Some(371.0));
        assert_eq!(
            fanout_ratio(0, 10),
            Some(0.0),
            "delivering to nobody is a real 0, not idle"
        );
        assert_eq!(
            fanout_ratio(500, 0),
            None,
            "no movement submitted = nothing to divide by"
        );
    }

    /// A quiet window is not evidence. Below `MOVE_ACTIVITY_FLOOR` submitted movements the check must
    /// stay silent AND keep the baseline it learned while the server was busy — one lone player
    /// wandering with nobody nearby must not become the reference point for a full zone.
    #[test]
    fn a_quiet_window_neither_warns_nor_rebaselines_b1() {
        let busy = FanoutHealth {
            baseline: Some(300.0),
            low_windows: 2,
        };
        let (next, warn) = fanout_health_step(busy, Some(1.0), crate::world::MOVE_ACTIVITY_FLOOR);
        assert_eq!(warn, None, "a below-floor window must never warn");
        assert_eq!(
            next.baseline,
            Some(300.0),
            "the busy-server baseline must survive a quiet window"
        );
        assert_eq!(
            next.low_windows, 0,
            "and it breaks any collapse run in progress"
        );

        // No movement at all: same treatment, no divide-by-zero fake.
        let (idle, warn) = fanout_health_step(busy, None, 0);
        assert_eq!((idle.baseline, warn), (Some(300.0), None));
    }

    /// The first informative window can only ESTABLISH the baseline — there is nothing to compare it
    /// against. This is also the honest limit of the whole signal: a gateway that is already
    /// under-delivering when it starts up bakes that into the baseline and never trips.
    #[test]
    fn the_first_window_establishes_the_baseline_and_cannot_warn_b1() {
        let (next, warn) = fanout_health_step(FanoutHealth::default(), Some(40.0), 1_000);
        assert_eq!(
            (next.baseline, next.low_windows, warn),
            (Some(40.0), 0, None)
        );
    }

    /// **The hysteresis, which is the deliberate divergence from the stuck-relay signal.** A 40 %+ fan-out
    /// collapse must persist for `FANOUT_COLLAPSE_WINDOWS` consecutive windows before it says
    /// anything: the ratio wobbles, and a warning that cries wolf gets grepped away. Windows 1 and 2
    /// are silent; window 3 warns and reports the run length.
    #[test]
    fn a_fanout_collapse_warns_only_after_three_consecutive_windows_b1() {
        let mut st = FanoutHealth {
            baseline: Some(300.0),
            low_windows: 0,
        };
        for window in 1..=2 {
            let (next, warn) = fanout_health_step(st, Some(150.0), 1_000);
            assert_eq!(
                warn, None,
                "window {window} of a collapse must stay quiet (hysteresis)"
            );
            assert_eq!(
                next.low_windows, window,
                "the run must be counted while it is quiet"
            );
            st = next;
        }
        let (next, warn) = fanout_health_step(st, Some(150.0), 1_000);
        assert_eq!(
            warn,
            Some(3),
            "the third consecutive collapsed window must warn"
        );
        assert_eq!(
            next.baseline,
            Some(300.0),
            "a collapsed window must NOT be folded into the baseline — a baseline that absorbed the \
             collapse would decay under the current fan-out and the run could never reach 3"
        );
    }

    /// Half a collapse is not a collapse: one bad window followed by a healthy one resets the run, so
    /// an alternating wobble never fires however long it goes on.
    #[test]
    fn a_healthy_window_resets_the_collapse_run_b1() {
        let mut st = FanoutHealth {
            baseline: Some(300.0),
            low_windows: 0,
        };
        for _ in 0..10 {
            let (low, warn) = fanout_health_step(st, Some(100.0), 1_000);
            assert_eq!(warn, None, "a single collapsed window must never warn");
            let (healthy, warn) = fanout_health_step(low, Some(300.0), 1_000);
            assert_eq!(warn, None);
            assert_eq!(
                healthy.low_windows, 0,
                "a healthy window must clear the run"
            );
            st = healthy;
        }
    }

    /// Just above the ratio is NOT a collapse (the boundary), and a healthy window moves the trailing
    /// baseline by exactly one EWMA step — the mechanism that lets the signal follow a zone filling up
    /// instead of firing on it.
    #[test]
    fn the_collapse_boundary_and_the_ewma_step_b1() {
        let st = FanoutHealth {
            baseline: Some(100.0),
            low_windows: 2,
        };
        // Exactly at the ratio: healthy, so the run resets rather than firing on window 3.
        let (next, warn) = fanout_health_step(st, Some(100.0 * FANOUT_COLLAPSE_RATIO), 1_000);
        assert_eq!(
            warn, None,
            "fan-out exactly at the ratio is not yet a collapse"
        );
        assert_eq!(next.low_windows, 0);
        // baseline 100 + 0.3 * (60 - 100) = 88
        assert!(
            (next.baseline.unwrap() - 88.0).abs() < 1e-9,
            "one EWMA step at alpha {FANOUT_BASELINE_ALPHA}, got {:?}",
            next.baseline
        );
        // A hair below it, on the third window, does fire.
        let (_, warn) = fanout_health_step(st, Some(100.0 * FANOUT_COLLAPSE_RATIO - 0.001), 1_000);
        assert_eq!(
            warn,
            Some(3),
            "a hair below the ratio on window 3 must fire"
        );
    }

    /// After `FANOUT_REBASELINE_WINDOWS` the low level is accepted as the new normal, so the warning
    /// stops. Deliberate and documented: fan-out also drops permanently because the crowd thinned,
    /// and an alarm that never stops is one nobody reads. It fires for windows 3..6 — ~30s of loud
    /// warnings — and then goes quiet with the new baseline in place.
    #[test]
    fn a_sustained_collapse_rebaselines_and_stops_warning_b1() {
        let mut st = FanoutHealth {
            baseline: Some(300.0),
            low_windows: 0,
        };
        let mut fired = Vec::new();
        for _ in 1..=8 {
            let (next, warn) = fanout_health_step(st, Some(100.0), 1_000);
            if let Some(n) = warn {
                fired.push(n);
            }
            st = next;
        }
        assert_eq!(
            fired,
            vec![3, 4, 5, 6],
            "the warning must fire on windows 3..6 of a sustained collapse and then adapt"
        );
        assert_eq!(
            st.baseline,
            Some(100.0),
            "the low level becomes the new baseline"
        );
    }

    // ── Egress shedding (task B2) ──────────────────────────────────────────────────────────────────

    /// The shed decision and its boundary. A session AT the threshold still gets its packet — only a
    /// queue that has grown PAST it sheds — so the constant reads as "this many queued packets are
    /// acceptable", which is how the comment on it is written.
    #[test]
    fn the_egress_shed_boundary_is_strictly_above_the_threshold_b2() {
        assert!(!shed_motion_at_depth(0), "an empty queue must never shed");
        assert!(
            !shed_motion_at_depth(crate::world::EGRESS_SHED_DEPTH),
            "exactly at the threshold is still allowed through"
        );
        assert!(
            shed_motion_at_depth(crate::world::EGRESS_SHED_DEPTH + 1),
            "one past the threshold sheds"
        );
    }

    /// The shed at its CALL SITE (playbook §8: pinning the pure decision while the wiring stays free
    /// to vanish is this repo's most repeated test defect). A real `SessionTx` whose queue is one item
    /// past the threshold must produce NO packet from either high-volume relay, and each skip must be
    /// counted in `MOTION_DROPPED` — so a shed shows up on `MOTIONSTAT` as falling `delivery` instead
    /// of being silent.
    #[test]
    fn a_backed_up_session_sheds_peer_motion_and_creature_legs_b2() {
        let _counters = motion_counter_lock();
        use std::sync::atomic::Ordering::Relaxed;
        let (_, bytes) = moving_forward();
        let created = Arc::new(Mutex::new(HashSet::from([42u64])));
        let opcode = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT as u16;
        let leg = CreatureSpline {
            guid: 42,
            start_micros: 0,
            dur_ms: 500,
            sx: 1.0,
            sy: 2.0,
            sz: 3.0,
            dx: 4.0,
            dy: 5.0,
            dz: 6.0,
            map_id: 0,
            instance_id: 0,
            grid_x: 0,
            grid_y: 0,
            spline_id: 1,
            run: true,
            cell: lyracore_shared::spatial::grid_cell_id(0, 0),
            facing: false,
            facing_angle: 0.0,
            path: None,
        };

        // A session AT the threshold still receives its packet (this half is what stops the test from
        // passing for the trivial reason that the relays send nothing at all). One `SessionTx` each,
        // because a delivered packet raises the depth by one — the second send on a shared sender
        // would already be over the line, which is the mechanism working, not a failure.
        let (tx, rx) = crate::world::SessionTx::with_depth(crate::world::EGRESS_SHED_DEPTH);
        relay_entity_motion(&tx, &created, 1, &motion_row(42, opcode, bytes.clone()));
        assert!(
            rx.try_recv().is_ok(),
            "at-threshold peer motion must still flow"
        );
        let (tx, rx) = crate::world::SessionTx::with_depth(crate::world::EGRESS_SHED_DEPTH);
        relay_creature_leg(&tx, &created, &leg);
        assert!(
            rx.try_recv().is_ok(),
            "at-threshold creature legs must still flow"
        );

        // One item past it: each relay skips, and each counts its skip. Driven SEPARATELY so a
        // failure names which of the two call sites lost its shed.
        let (sent_before, dropped_before) =
            (MOTION_SENT.load(Relaxed), MOTION_DROPPED.load(Relaxed));
        let (tx, rx) = crate::world::SessionTx::with_depth(crate::world::EGRESS_SHED_DEPTH + 1);
        relay_entity_motion(&tx, &created, 1, &motion_row(42, opcode, bytes));
        assert!(
            rx.try_recv().is_err(),
            "PEER MOTION was queued onto a session more than EGRESS_SHED_DEPTH packets behind"
        );
        assert_eq!(
            MOTION_DROPPED.load(Relaxed) - dropped_before,
            1,
            "the shed peer-motion packet must be counted in MOTION_DROPPED"
        );
        let (tx, rx) = crate::world::SessionTx::with_depth(crate::world::EGRESS_SHED_DEPTH + 1);
        relay_creature_leg(&tx, &created, &leg);
        assert!(
            rx.try_recv().is_err(),
            "a CREATURE LEG was queued onto a session more than EGRESS_SHED_DEPTH packets behind"
        );
        assert_eq!(
            MOTION_DROPPED.load(Relaxed) - dropped_before,
            2,
            "the shed creature leg must be counted in MOTION_DROPPED too"
        );
        assert_eq!(
            MOTION_SENT.load(Relaxed),
            sent_before,
            "a shed packet is not a sent one — otherwise `delivery` would still read 100%"
        );
    }

    /// The depth counter's own arithmetic: every enqueue counts, and a send that FAILS (the writer
    /// half is gone) must roll its increment back. Without the rollback a session that ends while
    /// relays are still firing would leave a permanently high depth — on a live gateway the
    /// `SessionTx` clones outlive the writer briefly on every logout.
    #[test]
    fn the_egress_depth_counts_queued_items_and_rolls_back_a_failed_send_b2() {
        let (tx, rx) = crate::world::SessionTx::with_depth(0);
        assert_eq!(tx.depth(), 0);
        for expected in 1..=3 {
            tx.send(Outbound::Raw {
                opcode: 1,
                body: vec![],
            })
            .unwrap();
            assert_eq!(tx.depth(), expected, "each queued item must count");
        }
        drop(rx);
        assert!(tx
            .send(Outbound::Raw {
                opcode: 1,
                body: vec![]
            })
            .is_err());
        assert_eq!(
            tx.depth(),
            3,
            "a send that queued nothing must not leave the depth raised"
        );
    }
}

#[cfg(test)]
#[path = "subscriptions_character_gone_durable_tests.rs"]
mod character_gone_durable_tests;

#[cfg(test)]
#[path = "subscriptions_roster_relay_durable_tests.rs"]
mod roster_relay_durable_tests;

#[cfg(test)]
#[path = "subscriptions_meeting_stone_durable_tests.rs"]
mod meeting_stone_durable_tests;

#[cfg(test)]
#[path = "subscriptions_party_command_durable_tests.rs"]
mod party_command_durable_tests;
