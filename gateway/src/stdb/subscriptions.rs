//! Session relay lifetime: the `PlayerSubscriptions` viewer guard and packet builders used by
//! `world_view`'s shared per-shard dispatch.
//!
//! **The shared-connection model took the four box-scoped tables out of here entirely.** Peer
//! visibility, peer movement, creature legs and gameobjects are no longer per-player registrations:
//! they ride ONE global subscription per shard on the coordinator connection, and `stdb::world_view`
//! routes each row to the sessions an in-process cell index says can see it. What is left in this
//! file for those tables is the RELAY BODIES — [`offer_peer_create_for`], [`relay_entity_update`],
//! [`relay_peer_destroy`], [`relay_gameobject_create`], [`motion_outbound`],
//! [`creature_leg_outbound`] — each of which
//! decides and encodes for exactly ONE viewer and is run on that session's own writer thread, never
//! on the shared pump.
//!
//! Owner-addressed relays use the same shape: a shard callback selects one viewer, then its writer
//! queue runs the cache reads, state updates, and packet construction.

use crate::codec::{self, CreateKind};
use crate::world::{Outbound, SessionTx};
use anyhow::Result;
use spacetimedb_sdk::Table;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::{Vector3d, WeatherChangeType};

use super::aoi::ViewerGates;
use super::world_index::{CellKey, EntityLayer};
use super::world_view::{self, Viewer, WorldView};
use super::bindings::*;
use super::connection::Coordinator;
use super::views::{corpse_view, entity_view, go_view, hunter_pet_protocol_view};

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
            view.remove_viewer(viewer.session);
        }
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

/// Bag-content slot layout — mirrors the identically-named constants in `module/src/items/ops.rs`.
/// Flat slot space: equipped bags occupy slots 19..=22 (`BAG_SLOT_START..=22`), their CONTENTS
/// live in the flat range `BAG_CONTENT_OFFSET..BAG_CONTENT_END` (120..191 inclusive). Slot math:
/// `bag_idx = (slot - 120) / 18`, `slot_in_bag = (slot - 120) % 18`, `bag_equip = 19 + bag_idx`.
const BAG_CONTENT_OFFSET: u8 = 120;
const MAX_BAG_SIZE: u8 = 18;
const BAG_SLOT_START: u8 = 19;

/// Decompose a bag-content slot (`120..=191`) into `(bag_equip_slot, slot_in_bag)`. Returns `None`
/// for slots outside the bag-content range. `bag_equip_slot` is the slot where the bag itself is
/// equipped (19..=22); `slot_in_bag` is 0-indexed within that bag (0..MAX_BAG_SIZE).
fn bag_content_parts(slot: u8) -> Option<(u8, u8)> {
    if slot < BAG_CONTENT_OFFSET {
        return None;
    }
    let rel = slot - BAG_CONTENT_OFFSET;
    Some((BAG_SLOT_START + rel / MAX_BAG_SIZE, rel % MAX_BAG_SIZE))
}

/// Build the wire relay for one `game_teleport_event` row. Pure: takes the
/// already-derived `still_here` boolean (whether the live entity survived the transaction — the
/// module/gateway-mirrored same-map/cross-map signal, see the `on_teleport` callback's doc comment)
/// instead of a live `ReducerContext`/connection, so it's testable without a DB (module crate
/// convention: extract pure functions and test those).
///
/// `still_here` → same-map: `MSG_MOVE_TELEPORT_ACK` (byte-identical to the previous behavior — the ONLY
/// caller-visible difference from before is that this is now reached via a branch rather than
/// unconditionally). NOT `still_here` → cross-map: `SMSG_TRANSFER_PENDING`+`SMSG_NEW_WORLD` as one batch
/// (`codec::build_cross_map_teleport`). `Err` only for a `map_id` the client's `Map` enum doesn't know
/// (propagated so the caller can log it — never silently drops the relay).
fn build_teleport_relay(
    still_here: bool,
    mover_guid: u64,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
    o: f32,
) -> Result<Outbound> {
    if still_here {
        let m = codec::build_teleport_ack(mover_guid, 0, x, y, z, o);
        Ok(Outbound::One(ServerOpcodeMessage::MSG_MOVE_TELEPORT_ACK(
            Box::new(m),
        )))
    } else {
        let [pending, new_world] = codec::build_cross_map_teleport(map_id, x, y, z, o)?;
        Ok(Outbound::Batch(vec![pending, new_world]))
    }
}

/// Build the complete teleport result on the addressed session's writer queue.
pub(crate) fn teleport_event_outbound(row: &TeleportEvent) -> Vec<Outbound> {
    match build_teleport_relay(
        !row.cross_map,
        row.mover_guid,
        row.map_id,
        row.x,
        row.y,
        row.z,
        row.orientation,
    ) {
        Ok(out) => vec![out],
        Err(error) => {
            log::warn!("teleport relay: {error} (guid {})", row.mover_guid);
            Vec::new()
        }
    }
}

/// Build one addon-language whisper using the addressed viewer as its sender.
pub(crate) fn addon_message_outbound(self_guid: u64, row: &AddonMessage) -> Vec<Outbound> {
    let text = codec::addon::build_bridge_envelope(&row.cmd, &row.payload);
    let (opcode, body) = codec::addon::build_addon_smsg_raw(self_guid, &text);
    vec![Outbound::Raw { opcode, body }]
}

/// Build the owner's Hunter pet descriptor VALUES for one owner-addressed row.
pub(crate) fn hunter_pet_outbound(row: &HunterPetProtocol) -> Vec<Outbound> {
    vec![Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(codec::build_hunter_pet_values(&hunter_pet_protocol_view(
            row.clone(),
        ))),
    ))]
}

/// Build the XP-gain result for one identity-addressed row.
pub(crate) fn xp_event_outbound(row: &XpEvent) -> Vec<Outbound> {
    let message = codec::build_log_xpgain(row.killed_guid, row.total_exp, row.is_kill);
    vec![Outbound::One(ServerOpcodeMessage::SMSG_LOG_XPGAIN(
        Box::new(message),
    ))]
}

/// Build the level-up result for one identity-addressed row.
pub(crate) fn levelup_event_outbound(row: &LevelupEvent) -> Vec<Outbound> {
    let message = codec::build_levelup_info(
        row.new_level,
        row.health_gained,
        row.mana_gained,
        row.strength_gained,
        row.agility_gained,
        row.stamina_gained,
        row.intellect_gained,
        row.spirit_gained,
    );
    vec![Outbound::One(ServerOpcodeMessage::SMSG_LEVELUP_INFO(
        Box::new(message),
    ))]
}

/// Stealth presence marker (taxonomy `A_STEALTH`): a unit carrying ≥1 such aura is stealthed and must
/// be hidden from OTHER players' clients. Mirrors `module::spell::taxonomy::A_STEALTH = 0xA8`.
pub(crate) const A_STEALTH: u8 = 0xA8;

/// The visibility action a single A_STEALTH aura insert/delete implies, decided from the POST-change
/// count of A_STEALTH auras on the stealther's guid. A Stealth cast applies TWO A_STEALTH auras (and a
/// break removes both); since the SDK applies the whole row-delta before firing callbacks, both inserts
/// fire seeing count 2 and both deletes seeing count 0 — so we HIDE on ANY insert while stealthed
/// (count >= 1) and REVEAL when the count reaches 0. Pure + unit-tested; the exactly-once guarantee
/// against an already-hidden/visible peer is the per-viewer `created` set at the call site, not a boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StealthAction {
    Hide,
    Reveal,
    None,
}

/// Decide the visibility action for an A_STEALTH aura change. `is_insert` distinguishes the insert
/// (gain) path from the delete (loss) path; `post_change_count` is the number of A_STEALTH auras on the
/// stealther AFTER the change. A Stealth cast applies TWO A_STEALTH auras, and the SDK applies the whole
/// row-delta to the cache BEFORE firing per-row callbacks, so BOTH inserts fire with the count already at
/// 2 (and both deletes with it at 0). So HIDE on ANY insert that leaves the unit stealthed (count >= 1)
/// and REVEAL on the loss that brings the count to 0; the exactly-once guarantee is the per-viewer
/// `created` set at the call site (a duplicate HIDE/REVEAL is a no-op there), NOT a count boundary.
pub(crate) fn stealth_action(is_insert: bool, post_change_count: usize) -> StealthAction {
    match (is_insert, post_change_count) {
        (true, n) if n >= 1 => StealthAction::Hide,
        (false, 0) => StealthAction::Reveal,
        _ => StealthAction::None,
    }
}

/// Build the peer CREATE_OBJECT2 `SMSG_UPDATE_OBJECT` for a `game_world_entity` row, reading the peer's
/// equipped gear RLS-bypassed via the coordinator (slots 0..=18, model-visible) for a PLAYER. Shared by
/// the AOI-entry on_insert relay and the stealth REVEAL path so a re-shown stealther renders identically
/// (gear and all). Returns `None` only on an encode error (logged by the caller).
fn build_peer_create(
    coord: &Coordinator,
    viewer_guid: u64,
    row: &WorldEntity,
) -> Option<ServerOpcodeMessage> {
    let inv: Vec<(u8, u64, u32)> =
        if row.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0 {
            coord
                .player_items(row.guid)
                .unwrap_or_default()
                .into_iter()
                .filter(|i| i.slot <= 18)
                .map(|i| (i.slot, i.guid, i.entry))
                .collect()
        } else {
            Vec::new()
        };
    let mut view = entity_view(row.clone(), 0);
    view.dynamic_flags = projected_dynamic_flags(&coord.0.coord().conn.db, viewer_guid, row);
    // Peers pass no skill rows: the SkillInfo block is a self-descriptor (the client renders
    // only its OWN skill pane); a peer CREATE ignores it.
    match codec::build_create_object(&view, CreateKind::Peer, &inv, &[]) {
        Ok(m) => Some(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m))),
        Err(e) => {
            log::warn!("peer create encode failed for guid {}: {e}", row.guid);
            None
        }
    }
}

struct LootTagProjectionRows<'a> {
    tap: Option<&'a CreatureQuestTap>,
    tag_group: Option<&'a CreatureLootTagGroup>,
    tag_members: &'a [CreatureQuestTapMember],
    current_members: &'a [GroupMember],
    corpse_eligible: &'a [CorpseLootEligible],
}

/// Render the stored entity flags for one viewer without changing the durable row.
fn viewer_relative_dynamic_flags(
    viewer_guid: u64,
    entity: &WorldEntity,
    rows: LootTagProjectionRows<'_>,
) -> u32 {
    use lyracore_shared::constants::unit_dynamic_flags::{LOOTABLE, TAPPED_BY_PLAYER};

    if entity.dead {
        return if rows
            .corpse_eligible
            .iter()
            .any(|row| row.corpse_guid == entity.guid && row.eligible_guid == viewer_guid)
        {
            entity.dynamic_flags
        } else {
            entity.dynamic_flags & !LOOTABLE
        };
    }

    let Some(tap) = rows.tap.filter(|tap| tap.creature_guid == entity.guid) else {
        return entity.dynamic_flags;
    };
    let entitled = match rows.tag_group.filter(|group| group.creature_guid == entity.guid) {
        Some(group) => {
            rows.tag_members.iter().any(|member| {
                member.creature_guid == entity.guid && member.character_guid == viewer_guid
            }) && rows.current_members.iter().any(|member| {
                member.character_guid == viewer_guid && member.group_id == group.group_id
            })
        }
        None => viewer_guid == tap.character_guid,
    };
    if entitled {
        entity.dynamic_flags | TAPPED_BY_PLAYER
    } else {
        entity.dynamic_flags & !TAPPED_BY_PLAYER
    }
}

fn projected_dynamic_flags(
    db: &RemoteTables,
    viewer_guid: u64,
    entity: &WorldEntity,
) -> u32 {
    let tap = db.game_creature_quest_tap().creature_guid().find(&entity.guid);
    let tag_group = db
        .game_creature_loot_tag_group()
        .creature_guid()
        .find(&entity.guid);
    let tag_members: Vec<_> = db
        .game_creature_quest_tap_member()
        .iter()
        .filter(|member| member.creature_guid == entity.guid)
        .collect();
    let current_members: Vec<_> = db
        .game_group_member()
        .iter()
        .filter(|member| member.character_guid == viewer_guid)
        .collect();
    let corpse_eligible: Vec<_> = db
        .game_corpse_loot_eligible()
        .iter()
        .filter(|eligible| eligible.corpse_guid == entity.guid)
        .collect();
    viewer_relative_dynamic_flags(
        viewer_guid,
        entity,
        LootTagProjectionRows {
            tap: tap.as_ref(),
            tag_group: tag_group.as_ref(),
            tag_members: &tag_members,
            current_members: &current_members,
            corpse_eligible: &corpse_eligible,
        },
    )
}

/// May an instance-tagged CORPSE/GAMEOBJECT row be CREATE-relayed to this
/// viewer? Instance equality against a KNOWN viewer instance — `None` (viewer entity not resident,
/// see `viewer_instance`) relays nothing: suppress-then-sweep is strictly safer than the old
/// default-to-0 (which leaked instance-0 rows to a viewer whose real instance wasn't known yet).
/// Kept map-agnostic deliberately: instance-0 rows keep today's relay behavior byte-identical for
/// every resident open-world viewer, while a per-instance copy (or an in-instance corpse) reaches
/// ONLY viewers inside that instance. DELETE relays stay ungated (the `on_melee_delete` precedent —
/// SMSG_DESTROY_OBJECT for a never-created guid is a client no-op, and gating deletes risks
/// leaking a stale object on an instance transition). Pure — unit-tested below.
pub(crate) fn instance_relay_gate(row_instance_id: u64, viewer_instance_id: Option<u64>) -> bool {
    viewer_instance_id == Some(row_instance_id)
}

/// One corpse CREATE relay — the shared body of the `game_corpse` on_insert callback AND the
/// post-AOI resident sweep: with `LYRACORE_AOI=1` the base sub's corpse
/// callbacks fire before the viewer's entity is resident, so the gate suppresses them (see
/// `viewer_instance`); once the AOI entity subscription has applied, the sweep re-offers every
/// resident corpse row through the same gate. Sends the CREATE plus, for the viewer's OWN corpse,
/// the reclaim-delay packet.
/// Diagnosis counters for a failure where the peer-movement relay died permanently on a session —
/// the motion subscription stopped delivering while the module kept accepting movement. Deliberately
/// atomics read ONCE at logout rather than a log per call: a
/// `log::info!` on the relay path made the intermittent failure vanish across six consecutive runs,
/// so any instrument that touches the hot path perturbs the very race being measured.
///
/// `MOTION_CALLS` counts entries to [`relay_entity_motion`] (before any guard), `MOTION_SENT` counts
/// packets actually queued. Comparing them against the client's observed peer-moves localises a
/// failing run to one of three hops: subscription→callback (calls stay 0), guard rejection
/// (calls >> sent), or gateway→socket (sent >> observed).
pub(crate) static MOTION_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static MOTION_SENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The THIRD hop the two counters above could not see: queued → handed to the
/// writer. Both relays used to push with `let _ = tx.send(..)`, so a send to a session whose writer
/// half was already gone got discarded with no trace — a delivery path degrading silently, the exact
/// failure *shape* the stuck-relay incident and other orphaned writers had. Incremented by BOTH
/// [`relay_entity_motion`] and [`relay_creature_leg`]: they feed the same per-session writer channel,
/// so one counter answers "is the writer still taking peer-motion packets at all".
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

fn relay_corpse_create(tx: &SessionTx, self_guid: u64, row: &Corpse) {
    for o in corpse_create_outbound(self_guid, row) {
        let _ = tx.send(o);
    }
}

/// The corpse CREATE body as outbound packets — shared by the per-player relay, the login
/// resident sweep, and the shared-dispatch twin. The owner's reclaim-delay
/// packet rides inside, keyed on the VIEWER's guid.
pub(crate) fn corpse_create_outbound(self_guid: u64, row: &Corpse) -> Vec<Outbound> {
    let m = codec::build_corpse_create_object(&corpse_view(row.clone()));
    let mut out = vec![Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(m),
    ))];
    if row.owner_guid == self_guid {
        out.push(Outbound::One(
            ServerOpcodeMessage::SMSG_CORPSE_RECLAIM_DELAY(
                wow_world_messages::vanilla::SMSG_CORPSE_RECLAIM_DELAY {
                    delay: Duration::from_micros(row.reclaim_delay_micros as u64),
                },
            ),
        ));
    }
    out
}

/// The shared-dispatch twin of the per-player `on_corpse_update` relay (corpses): the body→bones
/// re-CREATE. The instance gate already ran in `world_view::corpse_changed`.
pub(crate) fn relay_corpse_update(row: &Corpse) -> Vec<Outbound> {
    let m = codec::build_corpse_create_object(&corpse_view(row.clone()));
    vec![Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(m),
    ))]
}

/// The pre-dedup visibility gate `offer_peer_create` runs before it ever touches the `created` set —
/// self-skip, instance isolation, spirit-healer ghost-gating, and the
/// currently-stealthed check. Pure over ALREADY-READ state (the caller does the DB lookups and passes
/// their results in) so it is unit-testable without a live SDK connection — the three scenarios
/// this predicate must get right (re-entry re-creates, login sees pre-existing peers, cross-instance
/// stays excluded) all reduce to "does this predicate return true for this row" plus the dedup-set
/// mechanics exercised separately below. Order/semantics moved verbatim out of the old inline body:
/// - Self is never offered (the dedup set is pre-seeded with `self_guid` instead).
/// - A cross-instance row is excluded BEFORE the dedup insert, so it is never marked `created` even
///   transiently (the same trap the encode-failure rollback below guards against).
/// - A spirit-healer creature is visible ONLY to a GHOST viewer (the released-spirit corpse-run state).
/// - A CURRENTLY-stealthed peer (≥1 A_STEALTH aura) is excluded; the later REVEAL re-offers it.
fn peer_create_gate(
    row_guid: u64,
    self_guid: u64,
    row_instance_id: u64,
    viewer_instance_id: u64,
    row_npc_flags: u32,
    viewer_is_ghost: bool,
    row_is_stealthed: bool,
) -> bool {
    const SPIRITHEALER_NPC_FLAG: u32 = lyracore_shared::constants::npc_flags::SPIRITHEALER; // 0x20
    if row_guid == self_guid {
        return false;
    }
    if row_instance_id != viewer_instance_id {
        return false;
    }
    if row_npc_flags & SPIRITHEALER_NPC_FLAG != 0 && !viewer_is_ghost {
        return false;
    }
    if row_is_stealthed {
        return false;
    }
    true
}

/// `game_chat_event.chat_type` discriminant for YELL (mirrors `chat::CHAT_YELL` on the module side —
/// duplicated here rather than depended on, same as the module's own doc comment on that constant
/// expects of its callers). Module-level (not local to `on_chat`) so the range constants live
/// beside the discriminant they gate.
const CHAT_YELL: u8 = 1;
/// SAY proximity range (vanilla ~25yd). See [`chat_in_range`].
const SAY_RANGE_YD: f32 = 25.0;
/// YELL proximity range (vanilla ~300yd). Same rationale as `SAY_RANGE_YD`.
const YELL_RANGE_YD: f32 = 300.0;
#[cfg(test)]
const SAY_RANGE_SQ: f32 = SAY_RANGE_YD * SAY_RANGE_YD; // 625.0 yd²
#[cfg(test)]
const YELL_RANGE_SQ: f32 = YELL_RANGE_YD * YELL_RANGE_YD; // 90_000.0 yd²

/// How far a `game_chat_event` line of this kind carries. Every kind on that table is
/// range-bound: SAY and text emotes at SAY range, YELL at its own; party, guild and whispers
/// ride other tables.
pub(crate) fn chat_range_yd(chat_type: u8) -> f32 {
    if chat_type == CHAT_YELL {
        YELL_RANGE_YD
    } else {
        SAY_RANGE_YD
    }
}

// ==================================================================================================
//  The SHARED-dispatch relay bodies.
//
//  Each of these is one audience-of-one relay: `stdb::world_view` decides WHICH sessions a row
//  concerns (via the cell index) and enqueues one of these per session as an `Outbound::Job`, so
//  the gating + encoding below runs on that session's own writer thread rather than on the shard's
//  single coordinator pump. They are the same bodies the per-player callbacks ran; what changed is
//  where the viewer-owned state comes from — a `Viewer` handle instead of a closure capture — and
//  that the two facts an away connection could not read (the viewer's instance and ghost flag) are
//  now supplied rather than looked up, which is what let the away tier be deleted.
// ==================================================================================================

/// The full gated "show this row to the viewer" path — [`peer_create_gate`], then the per-viewer
/// `created` dedup, then the CREATE send. Shared by the insert dispatch, the update re-entry branch,
/// the recenter's entered-set, and the world-entry sweep.
///
/// `db` is the cache of the shard that HOLDS the row (its aura rows and pet-spell rows live there);
/// `coord` is that shard's handle, used to read the peer's equipped gear RLS-bypassed. The viewer's
/// own instance and ghost state come off [`Viewer`] — never a lookup in `db`, because a row on
/// another shard's cache has no copy of the viewer's row to look them up in (the reason cross-seam
/// visibility ever needed a second per-player connection at all).
///
/// On a `build_peer_create` encode failure the guid is ROLLED BACK out of `created`: leaving it in
/// permanently suppresses the peer (a second latent bug in the same re-entry path, below).
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class the \
              shared-dispatch path's differential test exists to prevent"]
pub(crate) fn offer_peer_create_for(
    coord: &Coordinator,
    view: &WorldView,
    viewer: &Viewer,
    row: &WorldEntity,
) -> Vec<Outbound> {
    const SPIRITHEALER_NPC_FLAG: u32 = lyracore_shared::constants::npc_flags::SPIRITHEALER; // 0x20
    if row.guid == viewer.self_guid || row.instance_id != viewer.instance_id {
        return Vec::new();
    }
    let viewer_is_ghost = viewer.gates.is_ghost();
    // A spirit healer viewed by a living player is refused regardless of stealth.
    if row.npc_flags & SPIRITHEALER_NPC_FLAG != 0 && !viewer_is_ghost {
        return Vec::new();
    }
    let row_is_stealthed = view.auras.is_stealthed(row.guid);
    let guard = coord.0.coord();
    let db = &guard.conn.db;
    if !peer_create_gate(
        row.guid,
        viewer.self_guid,
        row.instance_id,
        viewer.instance_id,
        row.npc_flags,
        viewer_is_ghost,
        row_is_stealthed,
    ) {
        return Vec::new();
    }
    if !viewer.created.lock().unwrap().insert(row.guid) {
        return Vec::new();
    }
    let Some(m) = build_peer_create(coord, viewer.self_guid, row) else {
        viewer.created.lock().unwrap().remove(&row.guid);
        return Vec::new();
    };
    let mut out = vec![Outbound::One(m)];
    if let Some(equipment) = db.game_encounter_equip().creature_guid().find(&row.guid) {
        append_encounter_equip_after_create(&mut out, viewer, &equipment);
    }
    // Pet action bar: this viewer's OWN summoned pet just appeared — bind it client-side
    // (UNIT_FIELD_SUMMON + the pet action bar).
    if row.owner_guid == viewer.self_guid {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(codec::build_owner_summon_values(viewer.self_guid, row.guid)),
        )));
        if let Some(pet) = db
            .game_hunter_pet_protocol()
            .iter()
            .find(|pet| pet.live_pet_guid == row.guid && pet.owner_guid == viewer.self_guid)
        {
            out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                Box::new(codec::build_hunter_pet_values(&hunter_pet_protocol_view(
                    pet,
                ))),
            )));
        }
        let spells: Vec<u32> = db
            .game_creature_cast()
            .iter()
            .filter(|c| c.creature_entry == row.entry)
            .map(|c| c.spell_id)
            .collect();
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_PET_SPELLS(
            Box::new(codec::build_pet_spells(row.guid, &spells)),
        )));
    }
    if let Some(spline) = db
        .game_taxi_passenger_spline()
        .character_guid()
        .find(&row.guid)
    {
        // CREATE is already first in `out`. A resident flight must follow it in the same writer
        // work item so AOI entry can never observe MONSTER_MOVE for an unknown passenger.
        append_resident_taxi_after_create(
            &mut out,
            &viewer.created,
            viewer.self_guid,
            &spline,
        );
    }
    out
}

/// Relay one durable creature virtual-item projection to a viewer that already holds the creature.
/// A delete clears all three display slots. The raw builder is the crash-safe sparse VALUES path.
pub(crate) fn encounter_equip_outbound(
    viewer: &Viewer,
    row: &EncounterEquip,
    cleared: bool,
) -> Vec<Outbound> {
    if !viewer.created.lock().unwrap().contains(&row.creature_guid) {
        return Vec::new();
    }
    let (main_hand, off_hand, ranged) = if cleared {
        (0, 0, 0)
    } else {
        (row.main_hand, row.off_hand, row.ranged)
    };
    let (opcode, body) =
        codec::build_virtual_item_values(row.creature_guid, main_hand, off_hand, ranged);
    vec![Outbound::Raw { opcode, body }]
}

fn append_encounter_equip_after_create(
    out: &mut Vec<Outbound>,
    viewer: &Viewer,
    row: &EncounterEquip,
) {
    out.extend(encounter_equip_outbound(viewer, row, false));
}

/// One entity row changed, for one viewer: re-entry-as-UPDATE first (a peer that
/// left the box and returns arrives as an update of a still-cached row, and without this branch it
/// is permanently invisible until relog), otherwise the pure field diff, plus the viewer's own
/// spirit-healer ghost transition.
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class the \
              shared-dispatch path's differential test exists to prevent"]
pub(crate) fn relay_entity_update(
    coord: &Coordinator,
    view: &WorldView,
    viewer: &Viewer,
    old: &WorldEntity,
    new: &WorldEntity,
) -> Vec<Outbound> {
    const SPIRITHEALER_NPC_FLAG: u32 = lyracore_shared::constants::npc_flags::SPIRITHEALER;
    const GHOST_PLAYER_FLAG: u32 = lyracore_shared::constants::player_flags::GHOST;
    if new.guid != viewer.self_guid {
        let shown = viewer.created.lock().unwrap().contains(&new.guid);
        if is_update_reentry(new.guid, viewer.self_guid, shown) {
            return offer_peer_create_for(coord, view, viewer, new);
        }
    }
    let dynamic_flags = {
        let guard = coord.0.coord();
        projected_dynamic_flags(&guard.conn.db, viewer.self_guid, new)
    };
    let mut out: Vec<Outbound> = entity_update_to_outbound_with_dynamic_flags(old, new, dynamic_flags)
        .into_iter()
        .map(Outbound::One)
        .collect();
    if new.guid == viewer.self_guid {
        if let Some(is_ghost) =
            ghost_transition(old.player_flags, new.player_flags, GHOST_PLAYER_FLAG)
        {
            viewer
                .gates
                .is_ghost
                .store(is_ghost, std::sync::atomic::Ordering::Relaxed);
            // Every spirit healer currently IN VIEW (the index answers that; before the
            // shared-connection model this scanned the AOI-scoped connection cache, which was the
            // same set). Idempotent through
            // `created`, exactly like the stealth Reveal/Hide arms.
            let guard = coord.0.coord();
            let healers: Vec<WorldEntity> = guard
                .conn
                .db
                .game_world_entity()
                .iter()
                .filter(|e| {
                    e.npc_flags & SPIRITHEALER_NPC_FLAG != 0 && e.instance_id == new.instance_id
                })
                .collect();
            drop(guard);
            for h in healers {
                if is_ghost {
                    out.extend(offer_peer_create_for(coord, view, viewer, &h));
                } else if viewer.created.lock().unwrap().remove(&h.guid) {
                    out.push(Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                        codec::build_destroy_object(h.guid),
                    )));
                }
            }
        }
    }
    out
}

/// Re-send the projected flags for visible live Loot Tags after the shard membership mirror
/// changes. Membership does not rewrite a creature's `WorldEntity`, so its normal VALUES relay has
/// nothing to carry.
pub(crate) fn loot_tag_flags_after_membership_change(
    coord: &Coordinator,
    viewer: &Viewer,
) -> Vec<Outbound> {
    use lyracore_shared::constants::unit_dynamic_flags::TAPPED;

    let shown = viewer.created.lock().unwrap().clone();
    let guard = coord.0.coord();
    shown
        .iter()
        .filter_map(|guid| guard.conn.db.game_world_entity().guid().find(guid))
        .filter(|entity| !entity.dead && entity.dynamic_flags & TAPPED != 0)
        .map(|entity| {
            Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                codec::build_dynamic_flags_values(
                    entity.guid,
                    projected_dynamic_flags(&guard.conn.db, viewer.self_guid, &entity),
                ),
            )))
        })
        .collect()
}

/// A peer left this viewer's view (its row was deleted, or it walked out of the box). DESTROY once,
/// and only if the viewer had actually been shown it.
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class the \
              shared-dispatch path's differential test exists to prevent"]
pub(crate) fn relay_peer_destroy(viewer: &Viewer, guid: u64, owner_guid: u64) -> Vec<Outbound> {
    if guid == viewer.self_guid {
        return Vec::new();
    }
    if !viewer.created.lock().unwrap().remove(&guid) {
        return Vec::new();
    }
    let mut out = vec![Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
        codec::build_destroy_object(guid),
    ))];
    // Pet action bar: this viewer's own pet despawned — clear UNIT_FIELD_SUMMON and send the empty
    // SMSG_PET_SPELLS (the vanilla "remove pet bar" form).
    if owner_guid == viewer.self_guid {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(codec::build_owner_summon_values(viewer.self_guid, 0)),
        )));
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_PET_SPELLS(
            Box::new(codec::build_pet_spells_clear()),
        )));
    }
    out
}

/// A gameobject entering (or changing state within) this viewer's view → CREATE_OBJECT, joined to
/// its static template on the holding shard's coordinator cache.
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class the \
              shared-dispatch path's differential test exists to prevent"]
pub(crate) fn relay_gameobject_create(
    coord: &Coordinator,
    viewer: &Viewer,
    row: &GameObject,
) -> Vec<Outbound> {
    if !instance_relay_gate(row.instance_id, Some(viewer.instance_id)) {
        return Vec::new();
    }
    let guard = coord.0.coord();
    let Some(tmpl) = guard
        .conn
        .db
        .game_gameobject_template()
        .entry()
        .find(&row.template_entry)
    else {
        return Vec::new();
    };
    let view = go_view(row.clone(), &tmpl);
    // #515: the CREATE_OBJECT's typed descriptor builder can only carry rot0 of the 4-float spawn
    // quaternion (gtker's slot-0-only wall), so the other 3 slots — and the yaw-derive fallback for
    // an all-zero stored quaternion — ride a second raw VALUES frame right behind CREATE. The client
    // has already materialized the object by the time this second frame arrives (same ordering the
    // aura/quest-log raw-VALUES relays rely on elsewhere in this file).
    let (rot_opcode, rot_body) = codec::build_gameobject_rotation_values(&view);
    vec![
        Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_gameobject_create_object(&view),
        ))),
        Outbound::Raw {
            opcode: rot_opcode,
            body: rot_body,
        },
    ]
}

/// A gameobject leaving view. Ungated on purpose (the `on_melee_delete` precedent): DESTROY for a
/// guid the client never created is a client no-op, and gating it risks a stale prop.
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class the \
              shared-dispatch path's differential test exists to prevent"]
pub(crate) fn relay_gameobject_destroy(_viewer: &Viewer, guid: u64) -> Vec<Outbound> {
    vec![Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
        codec::build_destroy_object(guid),
    ))]
}

/// One mover's motion row → `MSG_MOVE_*` bytes for one session. The `created` guard is the same one
/// the relay before the shared-connection model had: no MSG_MOVE for a guid this client never got a
/// CREATE for.
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class the \
              shared-dispatch path's differential test exists to prevent"]
/// The shared-dispatch twin of the per-player `on_roll` relay (`/roll`) in
/// `subscribe_player_events`. Same packet, byte for byte; the audience decision (shard identity)
/// already happened in `world_view::roll_appeared`, and rolls are public, so there is no
/// per-viewer state to consult here.
pub(crate) fn relay_roll(row: &RollEvent) -> Vec<Outbound> {
    let m = codec::build_random_roll(row.roller_guid, row.min_roll, row.max_roll, row.result);
    vec![Outbound::One(ServerOpcodeMessage::MSG_RANDOM_ROLL(
        Box::new(m),
    ))]
}

/// Encode one zone's sky. The audience decision (the viewer's stored zone) already happened in
/// `world_view::weather_changed`, and the changed row carried the whole packet, so nothing is read
/// here.
pub(crate) fn weather_outbound(
    view: codec::ZoneWeatherView,
    change: WeatherChangeType,
) -> Vec<Outbound> {
    vec![Outbound::One(ServerOpcodeMessage::SMSG_WEATHER(Box::new(
        codec::build_weather(view, change),
    )))]
}

/// Read `zone_id`'s current sky off this shard's cache and encode it — the zone-entry leg, run on
/// the arriving viewer's own writer thread. Goes through the same `WeatherStore` seam world entry
/// uses, so a zone with no row means fine weather here too.
pub(crate) fn zone_weather_outbound<St: crate::world::WeatherStore + ?Sized>(
    store: &St,
    zone_id: u32,
    change: WeatherChangeType,
) -> Vec<Outbound> {
    vec![Outbound::One(crate::world::zone_weather_message(
        store, zone_id, change,
    ))]
}

/// The shared-dispatch twin of the per-player
/// `on_rest_insert` relay (rest state). Same raw PLAYER_BYTES_2 VALUES packet; the audience (owner
/// only) was already resolved by the owner-session lookup in `world_view::rest_state_appeared`.
pub(crate) fn relay_rest_state(self_guid: u64, player_bytes_2: u32) -> Vec<Outbound> {
    let (opcode, body) = codec::build_rest_state_values(self_guid, player_bytes_2);
    vec![Outbound::Raw { opcode, body }]
}

/// Shared-dispatch recipient leg for a breath relay row. The owner-session lookup in
/// `world_view::breath_relay_appeared` has already established the self-only audience.
pub(crate) fn relay_breath_event(
    character_guid: u64,
    kind: u8,
    time_remaining_ms: u32,
    duration_ms: u32,
    damage: u32,
) -> Vec<Outbound> {
    let msg = match kind {
        0 => codec::build_breath_timer_start(time_remaining_ms, duration_ms),
        1 => codec::build_breath_timer_stop(),
        2 => codec::build_drowning_damage_log(character_guid, damage),
        unknown => {
            log::warn!("breath relay: dropping unknown kind {unknown} for {character_guid}");
            return Vec::new();
        }
    };
    vec![Outbound::One(msg)]
}

/// Swing log: the ONE body both legs run — the per-player `on_combat`
/// callback sends what this returns; the shared dispatch enqueues it per viewer. Gated on the
/// viewer's `created` set (no point animating an invisible attacker's swing — the victim's health
/// still moves via the entity VALUES relay if the victim is in scope).
///
/// `tx` is used ONLY for the delayed ranged-impact damage log (auto-shot: the number arrives WITH the
/// arrow, via a thread per landed shot — a shared timer wheel if archer armies happen); every
/// immediate packet is RETURNED so the shared path writes it at the job's queue position.
pub(crate) fn combat_event_outbound(
    tx: &SessionTx,
    created: &Mutex<HashSet<u64>>,
    row: &CombatEvent,
) -> Vec<Outbound> {
    if !created.lock().unwrap().contains(&row.attacker_guid) {
        return Vec::new();
    }
    let mut out = Vec::new();
    // Auto Shot vanilla shot shape: a RANGED shot is a SPELL on the wire — SMSG_SPELL_GO (a HIT
    // carries the target in `hits`; a MISS in `misses` — the client renders the white "Miss"
    // from that list) followed by SMSG_SPELLNONMELEEDAMAGELOG for a landed hit ("Your Auto
    // Shot hits X for N"). NEVER SMSG_ATTACKERSTATEUPDATE — that is the MELEE swing packet,
    // and sending it per shot animated a melee swing over the ranged pose (the "idle between
    // shots" bug). Melee (ranged_spell_id 0) keeps the ATTACKERSTATEUPDATE path unchanged.
    if row.ranged_spell_id != 0 {
        // Auto Shot stamps an ammo display id → the AMMO flag fires the arrow graphic (24 =
        // INVTYPE_AMMO). Wand Shoot (ammo_display_id 0) → no flag → the bolt. Deliberate
        // simplification: inv-type 24 is hardcoded.
        let ammo = if row.ammo_display_id != 0 {
            Some((row.ammo_display_id, 24))
        } else {
            None
        };
        let miss = row.hit_info == 2; // module HIT_MISS
        let go = codec::build_spell_go_outcome(
            row.attacker_guid,
            row.ranged_spell_id,
            row.target_guid,
            ammo,
            miss,
        );
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(Box::new(
            go,
        ))));
        if !miss && row.damage > 0 {
            let log = codec::build_spell_non_melee_damage_log(
                row.target_guid,
                row.attacker_guid,
                row.ranged_spell_id,
                row.damage,
                0,                 // physical
                row.hit_info == 1, // module HIT_CRIT
                0,
                0,
            );
            let msg = Outbound::One(ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(
                log,
            )));
            // Auto Shot: the shot's damage lands at fire + travel (module ranged_impact applies
            // the health there) — hold the LOG to the same moment so the number arrives
            // WITH the arrow, not at the muzzle.
            if row.impact_delay_ms > 0 {
                let tx_late = tx.clone();
                let delay = std::time::Duration::from_millis(row.impact_delay_ms as u64);
                std::thread::spawn(move || {
                    std::thread::sleep(delay);
                    let _ = tx_late.send(msg);
                });
            } else {
                out.push(msg);
            }
        }
    } else if !row.spell_swing {
        // A fired on-next-swing spell (Heroic Strike/Cleave) REPLACES the white hit — the
        // whole swing rides the spell's cast-event row (GO + yellow named damage log), so this
        // event sends NO white ATTACKERSTATEUPDATE. killing_blow/ATTACKSTOP below still applies.
        let m = codec::build_attacker_state_update(
            row.attacker_guid,
            row.target_guid,
            row.damage,
            row.hit_info,
            row.blocked_amount,
            0,
        );
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(
            Box::new(m),
        )));
    }
    // C2: on a MELEE killing blow, tell the attacker to leave combat stance. The target itself
    // vanishes via the game_world_entity on_delete → SMSG_DESTROY_OBJECT relay. A RANGED kill
    // sends no ATTACKSTOP (the client was never in melee-attack state; vanilla sends none) —
    // its stop signal is the SMSG_CANCEL_AUTO_REPEAT from the engagement-row delete relay
    // (kill_creature's disengage frees the attacker's row).
    if row.killing_blow && row.ranged_spell_id == 0 {
        let stop = codec::build_attack_stop(row.attacker_guid, row.target_guid);
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(
            Box::new(stop),
        )));
    }
    out
}

/// Combat-stance engage leg: SMSG_ATTACKSTART for a melee engagement —
/// the one body both legs run. Ranged rows are NOT melee combat (Auto Shot: ATTACKSTART would animate a
/// melee swing between shots); the `created` gate skips an out-of-scope attacker like the swing
/// relay.
pub(crate) fn melee_engage_outbound(
    created: &Mutex<HashSet<u64>>,
    row: &MeleeAttack,
) -> Vec<Outbound> {
    if row.ranged_spell_id != 0 {
        return Vec::new();
    }
    if !created.lock().unwrap().contains(&row.attacker_guid) {
        return Vec::new();
    }
    let m = codec::build_attack_start(row.attacker_guid, row.target_guid);
    vec![Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(
        Box::new(m),
    ))]
}

/// Combat-stance disengage leg. A RANGED row's delete is the ONE
/// server-initiated auto-repeat teardown choke point: the OWNING player (and only them) gets the
/// 0-byte SMSG_CANCEL_AUTO_REPEAT so its toggle drops in lockstep (Auto Shot); a melee row's delete is
/// SMSG_ATTACKSTOP for everyone (a non-kill disengage leaves stance too).
pub(crate) fn melee_disengage_outbound(self_guid: u64, row: &MeleeAttack) -> Vec<Outbound> {
    if row.ranged_spell_id != 0 {
        if row.attacker_guid == self_guid {
            return vec![Outbound::One(ServerOpcodeMessage::SMSG_CANCEL_AUTO_REPEAT)];
        }
        return Vec::new();
    }
    let m = codec::build_attack_stop(row.attacker_guid, row.target_guid);
    vec![Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(
        Box::new(m),
    ))]
}

/// Spell cast visuals: the ONE body both legs run — the full cast-lock
/// contract (START/GO sequencing, interrupt teardown, pushback, proc log, damage/heal logs,
/// cooldown), pure over the row + the viewer's own guid. Every caster-private branch keys on
/// `self_guid` exactly as the per-player closure did.
pub(crate) fn cast_event_outbound(self_guid: u64, row: &SpellCastEvent) -> Vec<Outbound> {
    let mut out = Vec::new();
    // INTERRUPT signal (cast-interrupt-on-damage): the victim's mid-cast timed spell was cancelled.
    // Relay SMSG_SPELL_FAILURE{spell, Interrupted} to the caster so the client tears down its cast
    // bar (no SMSG_SPELL_GO follows — the cast never resolved). This row carries ONLY caster/spell,
    // so it must be handled before the START/GO/COOLDOWN sequence below (it has cast_time_ms 0).
    if row.is_interrupted {
        // SELF-ONLY for a PLAYER caster: game_spell_cast_event is a global public subscription,
        // so this closure fires for EVERY player. SMSG_SPELL_FAILURE is a caster-private cast-bar
        // teardown (unlike the START/GO broadcast visuals), so relay it ONLY to the caster — else
        // bystanders get stray "Interrupted" feedback and a bystander mid-casting the same spell
        // risks a spurious teardown.
        // A creature or pet caster (0xF130/0xF140 high-guid) has no self — broadcast so every observer's
        // mob cast bar tears down on a Kick/Counterspell (the packet carries the mob's guid, so
        // a bystander's own bar is untouched; mirrors the START broadcast that drew the bar).
        let is_creature = matches!(row.caster_guid >> 48, 0xF130 | 0xF140);
        if is_creature || row.caster_guid == self_guid {
            let m = codec::build_spell_failure(row.caster_guid, row.spell_id);
            out.push(Outbound::One(
                ServerOpcodeMessage::SMSG_SPELL_FAILURE(Box::new(m)),
            ));
        }
        // A row that names a concrete reason (an on-next-swing strike that could not pay its cost at
        // the swing) follows the teardown with the failed cast result, in vanilla's order
        // (SendInterrupted then SendCastResult). The client needs BOTH: the teardown closes the cast
        // bar, the cast result releases the ability it holds as its current melee spell and prints
        // the red "Not enough rage" line. Caster-private, like the teardown above — the raw encoder,
        // because the typed SMSG_CAST_RESULT inverts the success/failure semantics.
        if row.caster_guid == self_guid {
            if let Some(reason) = codec::cast_failure_reason_for_code(row.failure_reason) {
                out.push(Outbound::Raw {
                    opcode: 0x0130,
                    body: codec::build_cast_result_failed(row.spell_id, reason),
                });
            }
        }
        return out;
    }
    // PUSHBACK signal: a direct hit slid the caster's in-progress timed cast's
    // fire time. Broadcast (NOT self-only) — SMSG_SPELL_DELAYED is a caster-visible cast-bar
    // shift, like SMSG_SPELL_START/GO below, so anyone watching the caster's cast bar sees it
    // slide (unlike SMSG_SPELL_FAILURE above, which is a private cast-bar-teardown message). This
    // row carries ONLY caster/spell/delay_ms, so it must be handled before the START/GO/COOLDOWN
    // sequence below (it has cast_time_ms 0 and is_completion false, so it does NOT take either
    // of those branches, but returning explicitly keeps this a single-purpose row like the
    // is_interrupted branch above it).
    if row.delay_ms > 0 {
        let m = codec::build_spell_delayed(row.caster_guid, row.delay_ms);
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_DELAYED(
            Box::new(m),
        )));
        return out;
    }
    // PROC-LOG row: a swing-proc damage line (Seal of Righteousness holy riding a landed
    // melee swing). ONLY the named yellow combat-log/floating number — never START/GO/cooldown
    // (nothing casts; the seal aura is already up). Broadcast like the damage log below.
    if row.is_proc_log {
        if row.damage > 0 {
            let log = codec::build_spell_non_melee_damage_log(
                row.target_guid,
                row.caster_guid,
                row.spell_id,
                row.damage,
                row.school,
                row.is_crit,
                row.resisted,
                row.absorbed,
            );
            out.push(Outbound::One(
                ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)),
            ));
        }
        return out;
    }
    if row.cast_time_ms > 0 {
        // Cast-START (a timed spell): SMSG_SPELL_START with the cast-bar duration so observers
        // see the bar FILL. The GO/COOLDOWN follow on the cast-GO COMPLETION event.
        let start = codec::build_spell_start(
            row.caster_guid,
            row.spell_id,
            row.cast_time_ms,
            0,
            None,
        );
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
            Box::new(start),
        )));
        return out;
    }
    // Cast-GO (cast_time_ms == 0). Mangos-faithful sequence:
    //   - GENUINE INSTANT (is_completion=false): START(0)+GO — SendSpellStart fires for EVERY
    //     non-triggered cast (timer 0 for an instant) to register the pending cast, then cast() →
    //     SendSpellGo finalizes it. START flags = 0x02, GO flags = 0x0100 (set in the builders).
    //   - TIMED COMPLETION (is_completion=true): GO ALONE — the begin already sent START(cast_time);
    //     a 2nd START(0) reset the bar to zero-length ("stuck on full"). The SMSG_SPELL_GO is the
    //     client's cast finalizer (it matches by caster guid + spell id to release the pending cast).
    // The CASTER's OWN instant cast (cast_time 0 → this GO branch, !is_completion) already got
    // START+GO SYNCHRONOUSLY from the CMSG_CAST_SPELL handler (so they precede the aura effects the
    // SDK's alphabetical callback order would otherwise send first). Skip the duplicate to the caster
    // — but still relay it to OBSERVERS (caster != self) and for TIMED COMPLETIONS (is_completion,
    // which the gateway did NOT sync-send). NOTE: a player's TRIGGERED instant cast (channel-tick
    // missile / on-hit trigger) also matches !is_completion && caster==self and is suppressed here —
    // it did not get a synchronous send, so it loses its caster-side visual until the client_initiated
    // flag lands (tracked follow-up). Acceptable for the slice; observers still see it.
    // Suppress ONLY what the CMSG handler actually sent synchronously — the row says so
    // (client_initiated rides from the cast_spell reducer path alone). The old shape
    // (!is_completion && self) also swallowed the caster's TRIGGERED instants — channel-tick
    // missiles (Arcane Missiles/Drain Life), on-hit trigger_spell procs — which never had a
    // synchronous send, so their caster saw no visual while observers did.
    let caster_synced_instant = row.client_initiated && row.caster_guid == self_guid;
    if !caster_synced_instant {
        if !row.is_completion {
            let start =
                codec::build_spell_start(row.caster_guid, row.spell_id, 0, 0, None);
            out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
                Box::new(start),
            )));
        }
        // CAST_RESULT(OK) is caster-only: clears the pending spell state so the subsequent
        // GO can release m_currentSpells. Only for timed completions — the instant-cast
        // caster got it synchronously from the CMSG_CAST_SPELL handler.
        if row.is_completion && row.caster_guid == self_guid {
            out.push(Outbound::Raw {
                opcode: 0x0130,
                body: codec::build_cast_result_ok(row.spell_id),
            });
        }
        let mut go =
            codec::build_spell_go(row.caster_guid, row.spell_id, row.target_guid, None);
        // A 0-damage on-next-swing FIRE that rode a missed/dodged/parried swing reports
        // the outcome in the GO's miss list — the client prints the yellow "Your Heroic
        // Strike missed/was dodged/was parried" line (the white MISS was suppressed via
        // spell_swing). swing_hit_info uses the CombatEvent codes (2 miss, 3 dodge, 4 parry);
        // every non-swing row carries 0, keeping this branch dead for ordinary casts.
        if row.is_completion && row.damage == 0 {
            use wow_world_messages::vanilla::{SpellMiss, SpellMissInfo};
            let miss_info = match row.swing_hit_info {
                2 => Some(SpellMissInfo::Miss),
                3 => Some(SpellMissInfo::Dodge),
                4 => Some(SpellMissInfo::Parry),
                _ => None,
            };
            if let Some(mi) = miss_info {
                go.hits.clear();
                go.misses = vec![SpellMiss {
                    target: wow_world_messages::Guid::new(row.target_guid),
                    miss_info: mi,
                }];
            }
        }
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(
            Box::new(go),
        )));
    }
    // FIX 2: the floating spell damage number — relay SMSG_SPELLNONMELEEDAMAGELOG when this cast
    // dealt damage (the module summed it onto the row + stored the school INDEX). 0 damage (heals /
    // buffs / a begin-START) → no log. Sent after the GO so the number floats over the hit. The
    // crit flag + resisted/absorbed breakdown ride from the row → CriticalHit hit_info + the
    // (N resisted)/(N absorbed) suffixes. NOTE: a FULLY-absorbed hit has damage==0 (no health
    // write) but absorbed>0; this slice keeps the `damage > 0` gate, so a 0-damage full-absorb is
    // intentionally NOT logged (vanilla's absorb-only "Absorb" text is a separate log variant —
    // out of scope). Widen the gate to `|| row.absorbed > 0 || row.resisted > 0` to add it later.
    if row.damage > 0 {
        let log = codec::build_spell_non_melee_damage_log(
            row.target_guid,
            row.caster_guid,
            row.spell_id,
            row.damage,
            row.school,
            row.is_crit,
            row.resisted,
            row.absorbed,
        );
        out.push(Outbound::One(
            ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)),
        ));
    }
    // The green floating heal number + combat-log line — SMSG_SPELLHEALLOG whenever
    // this cast restored health (module sums EFFECTIVE heal onto the row; overheal-only
    // casts carry 0 and stay silent, matching the damage gate's shape).
    if row.healed > 0 {
        use wow_world_messages::vanilla::SMSG_SPELLHEALLOG;
        use wow_world_messages::Guid;
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELLHEALLOG(
            Box::new(SMSG_SPELLHEALLOG {
                victim: Guid::new(row.target_guid),
                caster: Guid::new(row.caster_guid),
                id: row.spell_id,
                damage: row.healed,
                critical: row.is_crit,
            }),
        )));
    }
    // SMSG_SPELL_COOLDOWN — ONLY for a spell that actually HAS a cooldown (Mortal Strike, Judgement),
    // with the REAL value. mangos does NOT send a cooldown packet per cast; we used to send one
    // (cooldown=0) after EVERY cast, which STUCK the client's action button ("yellow casting outline" +
    // "Another action is in progress" — could only cast each spell once). The SMSG_SPELL_GO above is
    // what releases the client's pending-cast state (as in mangos); a 0-cooldown cast sends nothing.
    if row.cooldown_ms > 0 {
        let cd =
            codec::build_spell_cooldown(row.caster_guid, row.spell_id, row.cooldown_ms);
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_COOLDOWN(
            Box::new(cd),
        )));
    }
    out
}

/// The full aura-array VALUES block for `target_guid`. `auras` is that unit's current aura set
/// ([`crate::stdb::world_view::AuraIndex::on_target`]); the filter only guards a caller that
/// hands over a wider set.
pub(crate) fn aura_sync(
    auras: impl Iterator<Item = crate::stdb::bindings::Aura>,
    target_guid: u64,
) -> Outbound {
    let slots: Vec<codec::update_mask::AuraSlot> = auras
        .filter(|a| a.target_guid == target_guid)
        .map(|a| codec::update_mask::AuraSlot {
            slot: a.slot,
            spell_id: a.spell_id,
            flags: a.flags,
            level: a.level,
        })
        .collect();
    let mask = codec::update_mask::full_aura_mask(&slots);
    let (opcode, body) = codec::build_values_update_raw(target_guid, &mask);
    Outbound::Raw { opcode, body }
}
// Live ARMOR on the character sheet (the operator's Demon Skin bug): a player's own
// `A_MOD_RESISTANCE(armor)` aura applying or expiring must push `UNIT_FIELD_RESISTANCES[0]` — the
// module keeps `e.armor` at BASE (combat folds the effective value on demand), so the sheet never
// moves without this relay. Recompute the EFFECTIVE armor from this connection's CURRENT cache
// (base + armor auras + gear — `effective_armor` reads the post-change set, so apply AND expire
// both land the right value, exactly like `run_speed_packet`). Returns `None` unless `changed` is a
// self armor aura, so ordinary buffs/debuffs don't spam the opcode. Self-scoped: the sheet shows
// only your own armor, so no peer relay is needed.
pub(crate) fn armor_packet(coord: &Coordinator, changed: &Aura, self_guid: u64) -> Option<Outbound> {
    const A_MOD_RESISTANCE: u8 = 0xA1; // taxonomy A_MOD_RESISTANCE
    const RESIST_ARMOR_MASK: u32 = 0x01; // taxonomy RESIST_ARMOR bit (eff_p0 is a school MASK)
    if changed.target_guid != self_guid
        || changed.eff_kind != A_MOD_RESISTANCE
        || (changed.eff_p0 as u32 & RESIST_ARMOR_MASK) == 0
    {
        return None;
    }
    // The entity row (the BASE armor term) lives only on the coordinator now — the
    // per-player connection no longer subscribes `game_world_entity` at all. The
    // coordinator's cache also carries the auras, the item instances and the item
    // templates, so this fold is complete there in a way it never was on the player
    // connection (which lost `game_item_template` to the connection reclaim).
    let guard = coord.0.coord();
    let db = &guard.conn.db;
    let eff = super::armor::effective_armor(db, self_guid);
    // Carry the positive AURA portion alongside the total so the paperdoll renders the
    // green "(+N)" (Devotion Aura showed as plain white armor). Raw path — the positive
    // field has no gtker setter. Login self-corrects through this same relay (the SDK
    // replays aura rows after subscription-apply).
    let pos = super::armor::aura_armor_positive(db, self_guid);
    Some(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(codec::build_armor_values(self_guid, eff, pos)),
    )))
}
// Live PAPERDOLL STR/AGI/STA/INT/SPI/AP/damage-range on an aura apply/expire (#517, the Battle Shout
// bug): `module::spell::recompute_sheet` is the SOURCE OF TRUTH and already re-derived + wrote
// `game_world_entity.sheet_*` in the SAME reducer transaction that inserted/deleted `changed` — this
// is a plain re-push of that row, never a second aura fold (the trap the ORIGINAL gateway-only
// `sheet_stats` hit: it never read `game_aura` at all). Gated on the aura KIND matching what
// `recompute_sheet` actually reacts to (`aura_moves_sheet`'s mirror), so an unrelated buff/debuff
// (a DoT, a slow) doesn't spam the opcode — purely a re-push filter, not aura interpretation. Self-
// scoped: the sheet shows only your own numbers, so no peer relay is needed.
pub(crate) fn sheet_packet(coord: &Coordinator, changed: &Aura, self_guid: u64) -> Option<Outbound> {
    const A_MOD_STAT: u8 = 0xA0; // taxonomy A_MOD_STAT
    const A_MOD_COMBAT: u8 = 0xA3; // taxonomy A_MOD_COMBAT
    const COMBAT_ATTACK_POWER: i32 = 0; // taxonomy COMBAT_ATTACK_POWER
    let moves_sheet = changed.eff_kind == A_MOD_STAT
        || (changed.eff_kind == A_MOD_COMBAT && changed.eff_p0 == COMBAT_ATTACK_POWER);
    if changed.target_guid != self_guid || !moves_sheet {
        return None;
    }
    let guard = coord.0.coord();
    let db = &guard.conn.db;
    let st = super::armor::sheet_stats(db, self_guid);
    drop(guard);
    st.map(|st| {
        Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
            codec::build_sheet_stats_values(self_guid, &st),
        )))
    })
}
// Stealth peer-visibility: when a NON-self peer's A_STEALTH presence crosses the 0↔1 boundary,
// HIDE it from this viewer (SMSG_DESTROY_OBJECT + evict from `created`) on the gain, REVEAL it
// (re-CREATE + re-insert into `created`) on the loss. The dispatch runs this once per viewer
// whose box covers the stealther, so each in-scope client hides/reveals on its own `tx`. Self is
// excluded (`changed.target_guid != self_guid`) so a stealther never hides from itself.
// Idempotency is the `created` set: HIDE only
// fires (and DESTROYs) if the guid was created; REVEAL only fires (and CREATEs) if it wasn't —
// re-hiding a hidden peer or re-revealing a visible one is a no-op. The stealther's entity row is
// read from the firing connection's cache (`ctx.db`); a guid the viewer can't see in scope has no
// row → REVEAL self-skips. `coord` reads the peer's gear RLS-bypassed on reveal (same as insert).
// One argument per piece of per-viewer state the decision needs; they are not grouped
// because the caller has them as separate captures.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stealth_visibility(
    post_change_stealth_count: usize,
    view: &WorldView,
    session: u64,
    coord: &Coordinator,
    created: &Arc<Mutex<HashSet<u64>>>,
    changed: &Aura,
    self_guid: u64,
    is_insert: bool,
) -> Vec<Outbound> {
    if changed.eff_kind != A_STEALTH || changed.target_guid == self_guid {
        return Vec::new();
    }
    match stealth_action(is_insert, post_change_stealth_count) {
        StealthAction::Hide => {
            // Evict so a later REVEAL (and ordinary AOI re-entry) re-CREATEs; only DESTROY if the
            // viewer actually had it (idempotent — a never-created peer needs no DESTROY).
            if created.lock().unwrap().remove(&changed.target_guid) {
                vec![Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                    codec::build_destroy_object(changed.target_guid),
                ))]
            } else {
                Vec::new()
            }
        }
        StealthAction::Reveal => {
            // Re-CREATE only if not currently shown (idempotent) AND the peer is in this viewer's
            // scope (its row is in the cache). Insert into `created` BEFORE building so the dedup
            // is consistent; if the row is gone (out of scope), undo the insert and emit nothing.
            if !created.lock().unwrap().insert(changed.target_guid) {
                return Vec::new();
            }
            // "Is the peer in this viewer's scope" used to be "is its row in this
            // connection's cache", which the per-player box subscription made equivalent.
            // The shared connection's cache holds the whole world, so the question has to be
            // put to the cell index instead — otherwise a stealther unstealthing on the far
            // side of the zone would CREATE for everyone.
            if !view
                .spatial
                .can_see(EntityLayer::WorldEntity, session, changed.target_guid)
            {
                created.lock().unwrap().remove(&changed.target_guid);
                return Vec::new();
            }
            match coord
                .0
                .coord()
                .conn
                .db
                .game_world_entity()
                .guid()
                .find(&changed.target_guid)
            {
                Some(row) => match build_peer_create(coord, self_guid, &row) {
                    Some(m) => vec![Outbound::One(m)],
                    None => {
                        // Encode failure: roll the dedup entry back like `offer_peer_create`
                        // does, else this guid is permanently suppressed (marked created with
                        // no CREATE ever sent) — the same latent re-entry bug fixed on the
                        // insert/update path.
                        created.lock().unwrap().remove(&changed.target_guid);
                        Vec::new()
                    }
                },
                None => {
                    created.lock().unwrap().remove(&changed.target_guid);
                    Vec::new()
                }
            }
        }
        StealthAction::None => Vec::new(),
    }
}

/// Aura insert leg: array sync, self-only duration/run-speed/armor/sheet packets, and the
/// stealth HIDE transition. The target's current aura set comes from the gateway's aura index,
/// post-change like the cache it mirrors; `stealth_count` was taken on the pump.
#[allow(clippy::too_many_arguments)]
pub(crate) fn aura_insert_outbound(
    coord: &Coordinator,
    view: &WorldView,
    session: u64,
    created: &Arc<Mutex<HashSet<u64>>>,
    self_guid: u64,
    row: &Aura,
    stealth_count: usize,
) -> Vec<Outbound> {
    let mut out = Vec::new();
    // A stealth-hidden peer (not in `created`) must get NO per-peer relay: a partial VALUES on
    // a DESTROYed object is a client crash/desync vector.
    let visible = row.target_guid == self_guid || created.lock().unwrap().contains(&row.target_guid);
    let current = visible.then(|| view.auras.on_target(row.target_guid));
    if let Some(current) = &current {
        out.push(aura_sync(current.iter().cloned(), row.target_guid));
    }
    if let Some(o) = aura_duration_packet(row, self_guid) {
        out.push(o);
    }
    if let Some(current) = current {
        if let Some(o) = run_speed_packet(current.into_iter(), row, self_guid) {
            out.push(o);
        }
        if let Some(o) = armor_packet(coord, row, self_guid) {
            out.push(o);
        }
        if let Some(o) = sheet_packet(coord, row, self_guid) {
            out.push(o);
        }
    }
    out.extend(stealth_visibility(
        stealth_count,
        view,
        session,
        coord,
        created,
        row,
        self_guid,
        true,
    ));
    out
}

/// Aura update leg (no stealth transition on an update).
pub(crate) fn aura_update_outbound(
    coord: &Coordinator,
    view: &WorldView,
    created: &Arc<Mutex<HashSet<u64>>>,
    self_guid: u64,
    row: &Aura,
    expires_changed: bool,
) -> Vec<Outbound> {
    let mut out = Vec::new();
    let visible = row.target_guid == self_guid || created.lock().unwrap().contains(&row.target_guid);
    let current = visible.then(|| view.auras.on_target(row.target_guid));
    if let Some(current) = &current {
        out.push(aura_sync(current.iter().cloned(), row.target_guid));
    }
    // Re-send the timer only when the duration window actually changed (a refresh).
    if expires_changed {
        if let Some(o) = aura_duration_packet(row, self_guid) {
            out.push(o);
        }
    }
    if let Some(current) = current {
        if let Some(o) = run_speed_packet(current.into_iter(), row, self_guid) {
            out.push(o);
        }
        if let Some(o) = armor_packet(coord, row, self_guid) {
            out.push(o);
        }
        if let Some(o) = sheet_packet(coord, row, self_guid) {
            out.push(o);
        }
    }
    out
}

/// Aura delete leg (the REVEAL half of stealth). The aura index is post-delete, so the
/// sync/speed folds read the remaining set.
#[allow(clippy::too_many_arguments)]
pub(crate) fn aura_delete_outbound(
    coord: &Coordinator,
    view: &WorldView,
    session: u64,
    created: &Arc<Mutex<HashSet<u64>>>,
    self_guid: u64,
    row: &Aura,
    stealth_count: usize,
) -> Vec<Outbound> {
    let mut out = Vec::new();
    let visible = row.target_guid == self_guid || created.lock().unwrap().contains(&row.target_guid);
    if visible {
        let current = view.auras.on_target(row.target_guid);
        out.push(aura_sync(current.iter().cloned(), row.target_guid));
        if let Some(o) = run_speed_packet(current.into_iter(), row, self_guid) {
            out.push(o);
        }
        if let Some(o) = armor_packet(coord, row, self_guid) {
            out.push(o);
        }
        if let Some(o) = sheet_packet(coord, row, self_guid) {
            out.push(o);
        }
    }
    out.extend(stealth_visibility(
        stealth_count,
        view,
        session,
        coord,
        created,
        row,
        self_guid,
        false,
    ));
    out
}

/// Trade-status relay (#120): `game_trade_event.kind` → the `SMSG_TRADE_STATUS` variant, to the
/// row's recipient and nobody else (audience resolved by the caller, the `whisper_event_outbound`
/// shape). The kind byte is `lyracore_shared::trade::event_kind` — NOT the vanilla discriminant;
/// this match IS the wire mapping. An unknown kind (a newer module mid-rollout) drops with a warn
/// rather than desyncing the window.
pub(crate) fn trade_event_outbound(row: &TradeEvent) -> Vec<Outbound> {
    use lyracore_shared::trade::event_kind as kind;
    use wow_world_messages::vanilla::SMSG_TRADE_STATUS;
    // The OFFER_* kinds carry a whole-side snapshot and decode to the fixed-444-byte extended
    // status instead of a plain status (#121); `self_player` is the kind, not an inference.
    if row.kind == kind::OFFER_SELF || row.kind == kind::OFFER_PARTNER {
        // WIRE POLARITY (mangoszero `SendUpdateTrade`): the byte is `1 means traders data,
        // 0 means own` — so the field is SET when the packet describes the PARTNER's side,
        // despite the binding's `self_player` name. Live-client verification is #124's pass.
        return match trade_offer_extended(row.kind == kind::OFFER_PARTNER, &row.payload) {
            Some(msg) => vec![Outbound::One(
                ServerOpcodeMessage::SMSG_TRADE_STATUS_EXTENDED(Box::new(msg)),
            )],
            None => {
                log::warn!(
                    "trade relay: unparseable offer payload {:?} (event {})",
                    row.payload,
                    row.id
                );
                Vec::new()
            }
        };
    }
    let status = match row.kind {
        kind::BEGIN_TRADE => Some(SMSG_TRADE_STATUS::BeginTrade {
            unknown1: wow_world_messages::Guid::new(row.other_guid),
        }),
        kind::OPEN_WINDOW => Some(SMSG_TRADE_STATUS::OpenWindow),
        kind::TRADE_CANCELED => Some(SMSG_TRADE_STATUS::TradeCanceled),
        kind::BUSY => Some(SMSG_TRADE_STATUS::Busy),
        kind::NO_TARGET => Some(SMSG_TRADE_STATUS::NoTarget),
        kind::TARGET_TO_FAR => Some(SMSG_TRADE_STATUS::TargetToFar),
        kind::WRONG_FACTION => Some(SMSG_TRADE_STATUS::WrongFaction),
        kind::YOU_DEAD => Some(SMSG_TRADE_STATUS::YouDead),
        kind::TARGET_DEAD => Some(SMSG_TRADE_STATUS::TargetDead),
        kind::IGNORE_YOU => Some(SMSG_TRADE_STATUS::IgnoreYou),
        kind::TRADE_ACCEPT => Some(SMSG_TRADE_STATUS::TradeAccept),
        kind::BACK_TO_TRADE => Some(SMSG_TRADE_STATUS::BackToTrade),
        kind::TRADE_COMPLETE => Some(SMSG_TRADE_STATUS::TradeComplete),
        // Commit refused on bag space (#122): the window closes with the inventory error;
        // `target_error` says WHOSE bags — false = yours, true = the partner's.
        kind::INV_FULL_SELF => Some(SMSG_TRADE_STATUS::CloseWindow {
            inventory_result: wow_world_messages::vanilla::InventoryResult::InventoryFull,
            item_limit_category_id: 0,
            target_error: false,
        }),
        kind::INV_FULL_PARTNER => Some(SMSG_TRADE_STATUS::CloseWindow {
            inventory_result: wow_world_messages::vanilla::InventoryResult::InventoryFull,
            item_limit_category_id: 0,
            target_error: true,
        }),
        kind::GOLD_FAIL_SELF => Some(SMSG_TRADE_STATUS::CloseWindow {
            inventory_result: wow_world_messages::vanilla::InventoryResult::NotEnoughMoney,
            item_limit_category_id: 0,
            target_error: false,
        }),
        kind::GOLD_FAIL_PARTNER => Some(SMSG_TRADE_STATUS::CloseWindow {
            inventory_result: wow_world_messages::vanilla::InventoryResult::NotEnoughMoney,
            item_limit_category_id: 0,
            target_error: true,
        }),
        other => {
            log::warn!("trade relay: unknown kind {other} (event {})", row.id);
            None
        }
    };
    match status {
        Some(s) => vec![Outbound::One(ServerOpcodeMessage::SMSG_TRADE_STATUS(
            Box::new(s),
        ))],
        None => Vec::new(),
    }
}

/// Project one Duel lifecycle edge for one recipient. The module inserts one row per participant,
/// so this function deliberately builds one copy of each typed protocol packet per row.
pub(crate) fn duel_event_outbound(
    row: &DuelEvent,
    template: Option<&codec::GameObjectTemplateView>,
) -> Vec<Outbound> {
    use lyracore_shared::duel::{completion_kind, event_kind};
    use wow_world_messages::vanilla::{
        SMSG_DUEL_COMPLETE, SMSG_DUEL_COUNTDOWN, SMSG_DUEL_REQUESTED,
    };

    let raw_values = |guid, arbiter, team| {
        let (opcode, body) = codec::build_duel_player_values(guid, arbiter, team);
        Outbound::Raw { opcode, body }
    };

    match row.kind {
        event_kind::REQUESTED => {
            let Some(template) = template else {
                log::warn!(
                    "duel relay: missing duel-flag template {} (event {})",
                    row.flag_entry,
                    row.id
                );
                return Vec::new();
            };
            let flag = codec::GameObjectView {
                guid: row.flag_guid,
                template_entry: row.flag_entry,
                x: row.flag_x,
                y: row.flag_y,
                z: row.flag_z,
                orientation: row.flag_orientation,
                state: 1,
                type_id: template.type_id,
                display_id: template.display_id,
                rotation_0: 0.0,
                rotation_1: 0.0,
                rotation_2: 0.0,
                rotation_3: 0.0,
                size: 1.0,
            };
            let (rotation_opcode, rotation_body) = codec::build_gameobject_rotation_values(&flag);
            vec![
                Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                    codec::build_gameobject_create_object(&flag),
                ))),
                Outbound::Raw {
                    opcode: rotation_opcode,
                    body: rotation_body,
                },
                raw_values(row.initiator_guid, Some(row.flag_guid), None),
                raw_values(row.challenged_guid, Some(row.flag_guid), None),
                Outbound::One(ServerOpcodeMessage::SMSG_DUEL_REQUESTED(Box::new(
                    SMSG_DUEL_REQUESTED {
                        // The protocol calls this `initiator`; vanilla uses it as the arbiter GO.
                        initiator: wow_world_messages::Guid::new(row.flag_guid),
                        target: wow_world_messages::Guid::new(row.initiator_guid),
                    },
                ))),
            ]
        }
        event_kind::COUNTDOWN => vec![Outbound::One(ServerOpcodeMessage::SMSG_DUEL_COUNTDOWN(
            SMSG_DUEL_COUNTDOWN {
                // The library labels this Duration as seconds, but writes `as_secs()` directly.
                // Vanilla's field is milliseconds and CMaNGOS writes 3000, so 3000 seconds here
                // is the typed API spelling that produces the correct u32 wire value.
                time: Duration::from_secs(3_000),
            },
        ))],
        event_kind::ACTIVE => vec![
            raw_values(row.initiator_guid, None, Some(1)),
            raw_values(row.challenged_guid, None, Some(2)),
        ],
        event_kind::OUT_OF_BOUNDS => vec![Outbound::One(ServerOpcodeMessage::SMSG_DUEL_OUTOFBOUNDS)],
        event_kind::IN_BOUNDS => vec![Outbound::One(ServerOpcodeMessage::SMSG_DUEL_INBOUNDS)],
        event_kind::COMPLETE => {
            let mut outbound = vec![Outbound::One(ServerOpcodeMessage::SMSG_DUEL_COMPLETE(
                SMSG_DUEL_COMPLETE {
                    ended_without_interruption: row.completion_kind
                        != completion_kind::INTERRUPTED,
                },
            ))];
            outbound.extend(duel_winner_outbound(row));
            outbound.extend([
                raw_values(row.initiator_guid, Some(0), Some(0)),
                raw_values(row.challenged_guid, Some(0), Some(0)),
                Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                    codec::build_destroy_object(row.flag_guid),
                )),
            ]);
            outbound
        }
        other => {
            log::warn!("duel relay: unknown kind {other} (event {})", row.id);
            Vec::new()
        }
    }
}

/// The one winner announcement for a completed Duel. Participant relays include it after
/// `SMSG_DUEL_COMPLETE`; the shared view also fans it to nearby non-participants.
pub(crate) fn duel_winner_outbound(row: &DuelEvent) -> Vec<Outbound> {
    use lyracore_shared::duel::completion_kind;
    use wow_world_base::shared::duel_winner_reason_vanilla_tbc_wrath::DuelWinnerReason;
    use wow_world_messages::vanilla::SMSG_DUEL_WINNER;

    if row.winner_guid == 0
        || row.loser_guid == 0
        || row.winner_name.is_empty()
        || row.loser_name.is_empty()
    {
        return Vec::new();
    }
    let reason = match row.completion_kind {
        completion_kind::WON => DuelWinnerReason::Won,
        completion_kind::FLED => DuelWinnerReason::Fled,
        _ => return Vec::new(),
    };
    // `opponent_name` is the first CString on the wire and carries the WINNER: mangos sends
    // this packet from the loser, announcing its opponent as victor.
    vec![Outbound::One(ServerOpcodeMessage::SMSG_DUEL_WINNER(Box::new(
        SMSG_DUEL_WINNER {
            reason,
            opponent_name: row.winner_name.clone(),
            initiator_name: row.loser_name.clone(),
        },
    )))]
}

/// Decode an `OFFER_*` payload into the fixed-444-byte `SMSG_TRADE_STATUS_EXTENDED` (#121):
/// counts are 7/7 (the cmangos constant), unused slots stay zeroed (`TradeSlot::default`), and
/// every filled slot carries the module-resolved stack/durability/enchant fields. Fails closed
/// with the payload decoder. `describes_partner` sets the wire's misnamed `self_player` byte —
/// mangoszero: "1 means traders data, 0 means own".
fn trade_offer_extended(
    describes_partner: bool,
    payload: &str,
) -> Option<wow_world_messages::vanilla::SMSG_TRADE_STATUS_EXTENDED> {
    use wow_world_messages::vanilla::{TradeSlot, SMSG_TRADE_STATUS_EXTENDED};
    let (gold, views) = lyracore_shared::trade::decode_offer(payload)?;
    let mut trade_slots = [TradeSlot::default(); 7];
    for v in views {
        let slot = trade_slots.get_mut(v.trade_slot as usize)?;
        *slot = TradeSlot {
            trade_slot_number: v.trade_slot,
            item: v.entry,
            display_id: v.display_id,
            stack_count: v.stack_count,
            enchantment: v.enchantment,
            max_durability: v.max_durability,
            durability: v.durability,
            ..TradeSlot::default()
        };
    }
    Some(SMSG_TRADE_STATUS_EXTENDED {
        // `1 means traders data` — see the call-site polarity note.
        self_player: describes_partner,
        trade_slot_count1: 7,
        trade_slot_count2: 7,
        money_in_trade: wow_world_messages::vanilla::Gold::new(gold),
        spell_on_lowest_slot: 0,
        trade_slots,
    })
}

/// A roster written on realm-core carries no names: the directory database holds no character
/// rows. Fill every blank from the world shards' caches before the frame reaches a client, which
/// otherwise renders the member nameless — and a headless client waiting for a name never sees it.
fn name_the_roster(
    coord: &Coordinator,
    members: Vec<(u64, String, bool)>,
) -> Vec<(u64, String, bool)> {
    let shards = coord.all_shards();
    fill_member_names(members, |guid| {
        std::iter::once(coord)
            .chain(shards.iter())
            .find_map(|c| c.character_by_guid(guid).ok().flatten())
            .map(|c| c.name)
    })
}

/// The pure half of [`name_the_roster`]: only a blank name is looked up, and a lookup that fails
/// leaves it blank rather than dropping the member.
pub(crate) fn fill_member_names(
    members: Vec<(u64, String, bool)>,
    lookup: impl Fn(u64) -> Option<String>,
) -> Vec<(u64, String, bool)> {
    members
        .into_iter()
        .map(|(guid, name, online)| {
            let name = if name.is_empty() {
                lookup(guid).unwrap_or_default()
            } else {
                name
            };
            (guid, name, online)
        })
        .collect()
}

/// Group / loot-roll / quest-share: the ONE kind-decode body both legs
/// run. PRIVATE data — the audience (the row's recipient, and nobody else) is resolved by the
/// caller (RLS on the per-player leg; the owner-session lookup + `private_recipient_audience` on
/// the shared leg). `coord` is the privileged handle the QUEST_SHARE detail JOIN needs.
pub(crate) fn group_event_outbound(
    coord: &Coordinator,
    self_guid: u64,
    row: &GroupEvent,
) -> Vec<Outbound> {
    use lyracore_shared::group::event_kind as group_kind;
    use lyracore_shared::loot_roll::event_kind as roll_kind;
    use lyracore_shared::quest::share_event_kind as quest_share_kind;
    let msg = match row.kind {
        group_kind::INVITE => Some(ServerOpcodeMessage::SMSG_GROUP_INVITE(Box::new(
            codec::build_group_invite(row.other_name.clone()),
        ))),
        group_kind::LIST => match lyracore_shared::group::decode_roster(&row.payload) {
            Some((leader, loot_method, loot_threshold, master_looter_guid, members)) => {
                let members = name_the_roster(coord, members);
                Some(ServerOpcodeMessage::SMSG_GROUP_LIST(Box::new(codec::build_group_list(
                    self_guid, leader, loot_method, loot_threshold, master_looter_guid, &members,
                ))))
            }
            None => {
                log::warn!(
                    "group LIST relay: unparseable roster payload {:?} (event {})",
                    row.payload, row.id
                );
                None
            }
        },
        group_kind::DECLINE => Some(ServerOpcodeMessage::SMSG_GROUP_DECLINE(Box::new(
            codec::build_group_decline(row.other_name.clone()),
        ))),
        group_kind::DESTROYED => Some(ServerOpcodeMessage::SMSG_GROUP_DESTROYED),
        // A party (`/p`) chat line, one row per recipient (every OTHER member
        // + an echo to the sender — both pushed by `module/src/chat.rs::party_chat`).
        // `row.other_guid` is the SPEAKER (resolved/pushed by `group::push_event`, same
        // convention the roll kinds below use); `row.payload` is the raw message text
        // (`encode_party_chat` is a pass-through — nothing else to decode).
        group_kind::PARTY_CHAT => match lyracore_shared::group::decode_party_chat(&row.payload) {
            Some(message) => Some(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(
                codec::build_party_chat(row.other_guid, message),
            ))),
            None => {
                log::warn!("party PARTY_CHAT relay: unparseable payload {:?} (event {})", row.payload, row.id);
                None
            }
        },
        roll_kind::ROLL_START => match lyracore_shared::loot_roll::decode_start(&row.payload) {
            Some((corpse_guid, slot, item_entry, countdown_ms)) => {
                Some(ServerOpcodeMessage::SMSG_LOOT_START_ROLL(Box::new(codec::build_loot_start_roll(
                    corpse_guid, slot, item_entry, countdown_ms,
                ))))
            }
            None => {
                log::warn!("loot ROLL_START relay: unparseable payload {:?} (event {})", row.payload, row.id);
                None
            }
        },
        roll_kind::ROLL_VOTE => match lyracore_shared::loot_roll::decode_vote(&row.payload) {
            Some((corpse_guid, slot, item_entry, roll_number, vote, auto_pass)) => {
                Some(ServerOpcodeMessage::SMSG_LOOT_ROLL(Box::new(codec::build_loot_roll(
                    corpse_guid, slot, row.other_guid, item_entry, roll_number, vote, auto_pass,
                ))))
            }
            None => {
                log::warn!("loot ROLL_VOTE relay: unparseable payload {:?} (event {})", row.payload, row.id);
                None
            }
        },
        roll_kind::ROLL_WON => match lyracore_shared::loot_roll::decode_won(&row.payload) {
            Some((corpse_guid, slot, item_entry, winning_roll, winning_vote)) => {
                Some(ServerOpcodeMessage::SMSG_LOOT_ROLL_WON(Box::new(codec::build_loot_roll_won(
                    corpse_guid, slot, item_entry, row.other_guid, winning_roll, winning_vote,
                ))))
            }
            None => {
                log::warn!("loot ROLL_WON relay: unparseable payload {:?} (event {})", row.payload, row.id);
                None
            }
        },
        roll_kind::MASTER_LIST => match lyracore_shared::loot_roll::decode_master_list(&row.payload) {
            Some((_corpse_guid, eligible)) => Some(ServerOpcodeMessage::SMSG_LOOT_MASTER_LIST(Box::new(
                codec::build_loot_master_list(&eligible),
            ))),
            None => {
                log::warn!("loot MASTER_LIST relay: unparseable payload {:?} (event {})", row.payload, row.id);
                None
            }
        },
        // A grouped money-loot split's per-recipient share → the SAME
        // `SMSG_LOOT_MONEY_NOTIFY` the (now-removed) unconditional gateway send used to build,
        // just per-recipient and carrying the SHARE instead of the total (`amount` here IS the
        // wire field, matching `codec::build_loot_money_notify`'s single `amount: u32`).
        roll_kind::MONEY_SHARE => match lyracore_shared::loot_roll::decode_money_share(&row.payload) {
            Some(share) => Some(ServerOpcodeMessage::SMSG_LOOT_MONEY_NOTIFY(codec::build_loot_money_notify(share))),
            None => {
                log::warn!("loot MONEY_SHARE relay: unparseable payload {:?} (event {})", row.payload, row.id);
                None
            }
        },
        // Quest sharing: an eligible party member receives the shared quest —
        // `row.other_guid` is the SHARER (resolved/pushed by `group::push_event`), `row.payload`
        // is the quest entry. Opens the DETAILS screen with the SHARER as "giver" (the
        // recipient's own `CMSG_QUESTGIVER_ACCEPT_QUEST` then re-validates fresh via the
        // module's `GiverKind::Party` — this relay never authorizes anything by itself).
        quest_share_kind::QUEST_SHARE => match row.payload.parse::<u32>() {
            Ok(quest_id) => match coord.quest_detail(quest_id) {
                Ok(Some(detail)) => {
                    let (opcode, body) = codec::build_quest_details_raw(row.other_guid, &detail);
                    return vec![Outbound::Raw { opcode, body }];
                }
                Ok(None) => {
                    log::warn!("quest QUEST_SHARE relay: quest {quest_id} not loaded (event {})", row.id);
                    None
                }
                Err(e) => {
                    log::warn!("quest QUEST_SHARE relay: quest_detail lookup failed (event {}): {e}", row.id);
                    None
                }
            },
            Err(_) => {
                log::warn!("quest QUEST_SHARE relay: unparseable payload {:?} (event {})", row.payload, row.id);
                None
            }
        },
        // Quest sharing: the SENDER's per-member feedback line, one row per member,
        // always (whether or not the share actually landed) — `row.other_guid` is that member,
        // `row.payload` is the `share_result` wire byte (mirrors gtker's `QuestPartyMessage` 1:1).
        quest_share_kind::QUEST_PUSH_RESULT => match row.payload.parse::<u8>() {
            Ok(code) => Some(ServerOpcodeMessage::MSG_QUEST_PUSH_RESULT(Box::new(
                codec::build_quest_push_result(row.other_guid, code),
            ))),
            Err(_) => {
                log::warn!("quest QUEST_PUSH_RESULT relay: unparseable payload {:?} (event {})", row.payload, row.id);
                None
            }
        },
        other => {
            log::warn!("group event relay: unknown kind {other} (id {})", row.id);
            None
        }
    };
    match msg {
        Some(m) => vec![Outbound::One(m)],
        None => Vec::new(),
    }
}

/// The shared-dispatch "who may see this row" predicate for the PRIVATE recipient-addressed families
/// (whisper, group/loot-roll/quest-share, resurrect prompt): the row's addressee and nobody else. On
/// the shared feed this — together with the owner-session lookup that enforces it structurally — is
/// the entire privacy guarantee RLS used to provide.
pub(crate) fn private_recipient_audience(row_recipient_guid: u64, viewer_guid: u64) -> bool {
    // 0 is "unaddressed"/"uninitialized", never a real character — an equality alone would let an
    // unaddressed row match a half-initialized viewer (0 == 0), so zero denies on either side.
    row_recipient_guid != 0 && row_recipient_guid == viewer_guid
}

/// Whisper: the packet body both legs run. Audience resolved by the
/// caller — RLS per-player, the recipient owner-session lookup on the shared leg.
pub(crate) fn whisper_event_outbound(row: &WhisperEvent) -> Vec<Outbound> {
    let m = codec::build_whisper(row.other_guid, row.is_inform, row.message.clone());
    vec![Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
        Box::new(m),
    ))]
}

/// Build a Package System Message after the caller validates the recipient.
pub(crate) fn system_message_event_outbound(row: &SystemMessageEvent) -> Vec<Outbound> {
    let message = codec::build_gm_system_message(row.message.clone());
    vec![Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
        Box::new(message),
    ))]
}

/// Resurrect prompt: the packet body both legs run. Audience: the
/// offer's target, resolved by the caller.
pub(crate) fn resurrect_request_outbound(row: &ResurrectRequest) -> Vec<Outbound> {
    let m = codec::build_resurrect_request(row.caster_guid, row.caster_name.clone());
    vec![Outbound::One(ServerOpcodeMessage::SMSG_RESURRECT_REQUEST(
        Box::new(m),
    ))]
}

/// Projectile impact: the floating damage number for a projectile that
/// finished its travel — never a START/GO. Pure over the row; broadcast.
pub(crate) fn impact_event_outbound(row: &SpellImpactEvent) -> Vec<Outbound> {
    if row.damage == 0 {
        return Vec::new(); // a fully-absorbed impact (or a vanished target) logs nothing
    }
    let log = codec::build_spell_non_melee_damage_log(
        row.target_guid,
        row.caster_guid,
        row.spell_id,
        row.damage,
        row.school,
        row.is_crit,
        row.resisted,
        row.absorbed,
    );
    vec![Outbound::One(ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)))]
}

/// Nearby chat: the one body both legs run. A player speaker always hears
/// their own line; everyone else is `chat_in_range`-gated (say/text emote ~25yd, yell ~300yd, map +
/// instance fenced), with both endpoints read from the COORDINATOR's global cache — the AOI-scoped
/// per-player cache could not see a 100–300yd YELL speaker. Missing endpoint → drop (safer than
/// flooding).
pub(crate) fn chat_event_outbound(
    coord: &Coordinator,
    self_guid: u64,
    row: &ChatEvent,
) -> Vec<Outbound> {
    if row.sender_guid != self_guid {
        let range_yd = chat_range_yd(row.chat_type);
        let range_sq = range_yd * range_yd;
        let guard = coord.0.coord();
        let speaker = match guard
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&row.sender_guid)
        {
            Some(e) => e,
            None => return Vec::new(),
        };
        let listener = match guard.conn.db.game_world_entity().guid().find(&self_guid) {
            Some(e) => e,
            None => return Vec::new(),
        };
        if !chat_in_range(
            speaker.map_id,
            speaker.instance_id,
            speaker.x,
            speaker.y,
            listener.map_id,
            listener.instance_id,
            listener.x,
            listener.y,
            range_sq,
        ) {
            return Vec::new();
        }
    }
    let sender_name = if matches!(row.sender_guid >> 48, 0xF130 | 0xF140) {
        let guard = coord.0.coord();
        let Some(speaker) = guard
            .conn
            .db
            .game_world_entity()
            .guid()
            .find(&row.sender_guid)
        else {
            return Vec::new();
        };
        let Some(template) = guard
            .conn
            .db
            .game_creature_template()
            .entry()
            .find(&speaker.entry)
        else {
            return Vec::new();
        };
        Some(template.name)
    } else {
        None
    };
    vec![chat_event_message(row, sender_name)]
}

fn chat_event_message(row: &ChatEvent, sender_name: Option<String>) -> Outbound {
    let message = codec::build_chat_message_to(
        row.sender_guid,
        sender_name,
        row.target_guid,
        row.chat_type,
        row.language,
        row.message.clone(),
    );
    Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(Box::new(message)))
}

/// Chat channels: membership IS the audience (no proximity — General
/// spans the zone), checked against the coordinator's `game_channel_member` cache per viewer. The
/// sender hears their echo through the same path (they're a member too).
pub(crate) fn channel_event_outbound(
    coord: &Coordinator,
    self_guid: u64,
    row: &ChannelEvent,
) -> Vec<Outbound> {
    let member = {
        // Edition-2021 MutexGuard temporary-scope trap (danger-zones): single statement, bound.
        let guard = coord.0.coord();
        let is_member = guard
            .conn
            .db
            .game_channel_member()
            .iter()
            .any(|m| m.character_guid == self_guid && m.channel == row.channel);
        is_member
    };
    if !member {
        return Vec::new();
    }
    let m = codec::build_channel_message(
        row.sender_guid,
        row.channel_display.clone(),
        row.message.clone(),
    );
    vec![Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
        Box::new(m),
    ))]
}

/// Emotes: SMSG_TEXT_EMOTE + SMSG_EMOTE, broadcast (no range gate today
/// — preserved as-is for A/B equality). The target name resolves through the coordinator cache
/// (player, else creature template); unknown ids degrade gracefully so the rest still relays.
pub(crate) fn emote_event_outbound(coord: &Coordinator, row: &EmoteEvent) -> Vec<Outbound> {
    // target_guid == 0 → untargeted emote (the client sends 0 when nothing is selected).
    let target_name = if row.target_guid != 0 {
        let guard = coord.0.coord();
        let db = &guard.conn.db;
        db.game_character()
            .guid()
            .find(&row.target_guid)
            .map(|c| c.name)
            .or_else(|| {
                db.game_world_entity()
                    .guid()
                    .find(&row.target_guid)
                    .and_then(|e| {
                        db.game_creature_template()
                            .entry()
                            .find(&e.entry)
                            .map(|c| c.name)
                    })
            })
    } else {
        None
    };
    let mut out = Vec::new();
    if let Some(m) = codec::build_text_emote(
        row.sender_guid,
        row.text_emote,
        row.emote_anim,
        target_name,
    ) {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_TEXT_EMOTE(
            Box::new(m),
        )));
    }
    if let Some(a) = codec::build_emote_anim(row.sender_guid, row.emote_anim) {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_EMOTE(Box::new(a))));
    }
    out
}

/// Dynamic objects: the shared-dispatch twin of the per-player
/// `on_dynobj_insert` relay. The instance gate already ran in `world_view::dynobj_appeared`.
pub(crate) fn relay_dynobj_create(row: &DynamicObject) -> Vec<Outbound> {
    let m = codec::build_dynamicobject_create_object(
        row.guid,
        row.caster_guid,
        row.spell_id,
        row.x,
        row.y,
        row.z,
        row.radius_yd,
    );
    vec![Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(m),
    ))]
}

/// A bare SMSG_DESTROY_OBJECT — the delete leg of the dynobj/corpse families, shared because the
/// packet is nothing but the guid.
pub(crate) fn relay_destroy_object(guid: u64) -> Vec<Outbound> {
    vec![Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
        codec::build_destroy_object(guid),
    ))]
}

/// Skill pane: the shared-dispatch twin of the per-player skill relays
/// (insert and update ran the same body there too). Audience (owner only) resolved by the
/// owner-session lookup in `world_view::skill_changed`; the slot allocation writes the viewer's
/// OWN `skill_slots` — the same map the per-player leg captures, seeded from the login layout.
pub(crate) fn relay_skill(
    viewer: &Viewer,
    skill_line: u32,
    current: u16,
    max_rank: u16,
) -> Vec<Outbound> {
    let mut guard = viewer.skill_slots.lock().unwrap();
    let (map, next_free) = &mut *guard;
    let slot = *map.entry(skill_line).or_insert_with(|| {
        let s = *next_free;
        *next_free += 1;
        s
    });
    if let Ok(sk) = wow_world_messages::vanilla::Skill::try_from(skill_line) {
        if let Some(m) = codec::build_skill_values(viewer.self_guid, slot, sk, current, max_rank) {
            return vec![Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                Box::new(m),
            ))];
        }
    }
    Vec::new()
}

pub(crate) fn motion_outbound(
    created: &Mutex<HashSet<u64>>,
    self_guid: u64,
    row: &EntityMotion,
) -> Vec<Outbound> {
    MOTION_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if row.guid == self_guid || !created.lock().unwrap().contains(&row.guid) {
        return Vec::new();
    }
    match codec::build_movement_relay_raw(row.opcode as u32, row.guid, &row.movement_info) {
        Some((opcode, body)) => {
            MOTION_SENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            vec![Outbound::Raw { opcode, body }]
        }
        None if lyracore_shared::opcodes::movement::is_slice_move(row.opcode as u32) => {
            log::warn!(
                "motion relay: malformed movement_info ({} bytes) for mover {}",
                row.movement_info.len(),
                row.guid
            );
            Vec::new()
        }
        None => Vec::new(),
    }
}

/// One creature's spline row → `SMSG_MONSTER_MOVE` for one session.
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class the \
              shared-dispatch path's differential test exists to prevent"]
pub(crate) fn creature_leg_outbound(
    created: &Mutex<HashSet<u64>>,
    row: &CreatureSpline,
) -> Vec<Outbound> {
    if !created.lock().unwrap().contains(&row.guid) {
        return Vec::new();
    }
    let start = Vector3d {
        x: row.sx,
        y: row.sy,
        z: row.sz,
    };
    // #518: a facing-only leg (the mover doesn't move, `emit_facing_spline`'s carrier) gets the
    // `FacingAngle` variant — the only 1.12 `SMSG_MONSTER_MOVE` shape that turns a STATIONARY
    // creature without a spline the client would otherwise have nothing to interpolate from.
    if row.facing {
        return vec![Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(Box::new(
            codec::build_monster_move_facing(row.guid, start, row.facing_angle, row.spline_id),
        )))];
    }
    let dest = Vector3d {
        x: row.dx,
        y: row.dy,
        z: row.dz,
    };
    vec![Outbound::One(ServerOpcodeMessage::SMSG_MONSTER_MOVE(
        Box::new(codec::build_monster_move(
            row.guid,
            start,
            dest,
            row.dur_ms,
            row.spline_id,
            row.run,
        )),
    ))]
}

/// One module-owned passenger route to self and observers. Ordinary player motion deliberately
/// excludes self; taxi presentation must include it because the client is not driving the mover.
pub(crate) fn taxi_spline_outbound(
    created: &Mutex<HashSet<u64>>,
    self_guid: u64,
    row: &TaxiPassengerSpline,
) -> Vec<Outbound> {
    if row.character_guid != self_guid && !created.lock().unwrap().contains(&row.character_guid) {
        return Vec::new();
    }
    let points: Vec<Vector3d> = row
        .points
        .chunks_exact(3)
        .map(|point| Vector3d {
            x: point[0],
            y: point[1],
            z: point[2],
        })
        .collect();
    if points.is_empty() || points.len() * 3 != row.points.len() {
        log::warn!(
            "taxi spline: malformed route ({} coordinates) for passenger {}",
            row.points.len(),
            row.character_guid
        );
        return Vec::new();
    }
    let start = Vector3d {
        x: row.start_x,
        y: row.start_y,
        z: row.start_z,
    };
    codec::build_taxi_move_raw(
        row.character_guid,
        start,
        points,
        row.duration_ms,
        row.spline_id,
    )
    .map_or_else(Vec::new, |(opcode, body)| vec![Outbound::Raw { opcode, body }])
}

fn append_resident_taxi_after_create(
    created_outbound: &mut Vec<Outbound>,
    created: &Mutex<HashSet<u64>>,
    self_guid: u64,
    row: &TaxiPassengerSpline,
) {
    created_outbound.extend(taxi_spline_outbound(created, self_guid, row));
}

/// Replay a resident passenger spline after the owner's self CREATE/login batch. The same helper
/// is used by reconnect recovery, keeping the CREATE-before-MONSTER_MOVE ordering explicit.
pub(crate) fn resident_taxi_spline_outbound(
    coord: &Coordinator,
    viewer: &Viewer,
    character_guid: u64,
) -> Vec<Outbound> {
    let guard = coord.0.coord();
    let Some(row) = guard
        .conn
        .db
        .game_taxi_passenger_spline()
        .character_guid()
        .find(&character_guid)
    else {
        return Vec::new();
    };
    taxi_spline_outbound(&viewer.created, viewer.self_guid, &row)
}

/// The `on_update` dispatch decision: does this update need to be treated as a
/// re-entry (offer a fresh CREATE via `offer_peer_create`) instead of a field diff? True exactly when
/// the guid isn't self AND isn't already in the viewer's `created` set — i.e. either this is the first
/// time this viewer has seen the row, or it left scope (evicted by `on_delete`) and is now returning as
/// an UPDATE of the SDK's still-cached row instead of a fresh `on_insert`. Pure over the dedup-set
/// membership so the branch is unit-testable without a live SDK connection.
fn is_update_reentry(guid: u64, self_guid: u64, currently_shown: bool) -> bool {
    guid != self_guid && !currently_shown
}

/// The SAY/YELL proximity gate: does a speaker at `(speaker_map, speaker_instance,
/// speaker_x, speaker_y)` reach a listener at `(listener_map, listener_instance, listener_x,
/// listener_y)` within `range_sq` (already squared — pass `SAY_RANGE_SQ`/`YELL_RANGE_SQ`)? Different
/// maps or different instances never hear each other (also guards the distance arithmetic against
/// comparing positions on unrelated maps). Pure — no I/O — so it is the ONE tested source of truth
/// for `on_chat`'s range gate.
#[allow(clippy::too_many_arguments)]
pub(crate) fn chat_in_range(
    speaker_map: u32,
    speaker_instance: u64,
    speaker_x: f32,
    speaker_y: f32,
    listener_map: u32,
    listener_instance: u64,
    listener_x: f32,
    listener_y: f32,
    range_sq: f32,
) -> bool {
    if speaker_map != listener_map || speaker_instance != listener_instance {
        return false;
    }
    let dx = speaker_x - listener_x;
    let dy = speaker_y - listener_y;
    dx * dx + dy * dy <= range_sq
}

/// Edge-detect for the spirit-healer reveal: did the GHOST player-flag (mask) cross 0↔1 between `old`
/// and `new`? Returns `Some(true)` on an alive→ghost crossing (REVEAL the in-scope spirit healers),
/// `Some(false)` on ghost→alive (DESTROY them), `None` when the bit didn't change (the common heartbeat /
/// any other field update — no reveal work). Pure so the on_update closure stays a thin relay and this
/// stays unit-testable (the closure itself needs a live EventContext to enumerate healers).
pub(crate) fn ghost_transition(old_flags: u32, new_flags: u32, ghost_mask: u32) -> Option<bool> {
    let was = old_flags & ghost_mask != 0;
    let is = new_flags & ghost_mask != 0;
    if was == is {
        None
    } else {
        Some(is)
    }
}

/// Diff an observed `game_world_entity` row (`old` → `new`) into the partial-VALUES `SMSG_UPDATE_OBJECT`
/// packets that replicate the change to this player's client. Pure (no I/O) so it is unit-testable — the
/// `on_update` relay callback just sends whatever this returns, in order. The level-up branch is
/// MUTUALLY EXCLUSIVE with the individual field diffs: a ding pushes ONE combined VALUES block (level +
/// vitals + xp together) instead of the separate health/xp packets, so the panel + bars flip in lockstep.
/// Player-only fields (xp / coinage / ghost / power) are gated on the PLAYER bit; health and dynamic-flags
/// apply to any unit (creatures' health bars + loot sparkle). Order is preserved from the original relay.
#[cfg(test)]
pub(crate) fn entity_update_to_outbound(
    old: &WorldEntity,
    new: &WorldEntity,
) -> Vec<ServerOpcodeMessage> {
    entity_update_to_outbound_with_dynamic_flags(old, new, new.dynamic_flags)
}

fn entity_update_to_outbound_with_dynamic_flags(
    old: &WorldEntity,
    new: &WorldEntity,
    dynamic_flags: u32,
) -> Vec<ServerOpcodeMessage> {
    let mut out = Vec::new();
    let is_player = new.type_mask & lyracore_shared::constants::type_mask::PLAYER_BIT != 0;
    if is_player && old.level != new.level {
        // Level-up ding: level + maxhealth + health + xp in ONE VALUES block (the panel + health/XP bars
        // flip together). SMSG_LEVELUP_INFO (relayed separately) is the cosmetic popup only.
        let m = codec::build_levelup_values(
            new.guid,
            new.level,
            new.health,
            new.max_health,
            new.xp,
            new.next_level_xp,
            (new.unit_bytes_0 >> 24) as u8,
            new.max_power,
        );
        out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
    } else {
        if old.health != new.health {
            let m = codec::build_health_values(new.guid, new.health);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Dynamic-flags VALUES relay: the killing blow sets UNIT_DYNFLAG_LOOTABLE (0x1) on a corpse that
        // rolled money (slice 3); the loot reducer clears it. NOTE: UNIT_DYNFLAG_DEAD (0x20) is NEVER set
        // (it is feign-death in vanilla — see combat + lyracore-shared constants).
        if old.dynamic_flags != new.dynamic_flags {
            let m = codec::build_dynamic_flags_values(new.guid, dynamic_flags);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Target relay (UNIT_FIELD_TARGET): observers see who this unit selected — the target ring /
        // target-of-target / assist. Any unit (a creature's aggro target relays too), not player-gated.
        if old.target_guid != new.target_guid {
            let m = codec::build_target_values(new.guid, new.target_guid);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Max-vitals relay: a non-level-up max_health/max_power change (+Stamina/+Intellect gear or a
        // Fortitude/Mark aura) moves the bar DENOMINATOR live for observers. Any unit.
        if old.max_health != new.max_health || old.max_power != new.max_power {
            let power_b = (new.unit_bytes_0 >> 24) as u8;
            let m =
                codec::build_max_vitals_values(new.guid, new.max_health, power_b, new.max_power);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Taxi presentation changes mount display + the TAXI_FLIGHT unit flag in one mask (activation
        // and landing set both atomically), so the client never sees an intermediate
        // mounted-but-controllable or unmounted-but-in-flight state — detected by the flight bit itself
        // toggling, not just "unit_flags changed", so an unrelated flag change alongside a land mount
        // doesn't get mistaken for a taxi transition. A LAND mount/dismount changes `mount_display_id`
        // ALONE (the flight bit is untouched) and relays through the standalone single-field builder
        // instead, never alongside the coupled one for the same change.
        let taxi_flight_toggled = (old.unit_flags ^ new.unit_flags)
            & lyracore_shared::constants::unit_flags::TAXI_FLIGHT
            != 0;
        if old.mount_display_id != new.mount_display_id && taxi_flight_toggled {
            let m = codec::build_taxi_presentation_values(
                new.guid,
                new.mount_display_id,
                new.unit_flags,
            );
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        } else {
            if old.mount_display_id != new.mount_display_id {
                let m = codec::build_mount_display_values(new.guid, new.mount_display_id);
                out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
            }
            if old.unit_flags != new.unit_flags {
                let m = codec::build_unit_flags_values(new.guid, new.unit_flags);
                out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
            }
        }
        // Sheath relay (#101): UNIT_FIELD_BYTES_2 byte 0 flipping as a unit draws or stows its weapon.
        // Any unit, not player-gated — a creature drawing on engage is the same wire field. Without
        // this arm the server knows the state and no observer ever hears about it.
        if old.unit_bytes_2 != new.unit_bytes_2 {
            let m = codec::build_sheath_values(new.guid, new.unit_bytes_2);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Live XP-bar update (slice 1): players only (creatures never change xp; player-only fields).
        let xp_changed = old.xp != new.xp || old.next_level_xp != new.next_level_xp;
        if xp_changed && is_player {
            let m = codec::build_player_xp_values(new.guid, new.xp, new.next_level_xp);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Live purse update (slice 3): PLAYER_FIELD_COINAGE on money change (looting). Players only.
        if old.money != new.money && is_player {
            let m = codec::build_coinage_values(new.guid, new.money);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Live bank bag slot count: PLAYER_BYTES_2 byte 2 on a purchase, so the bank window grows a
        // slot without a relog. Scoped to byte 2 only — byte 3 (rest state) already relays through
        // its own dedicated `game_rest_state_event` path and must not double-fire here.
        const BANK_SLOT_BYTE: u32 = 0x00FF_0000;
        if is_player
            && (old.player_bytes_2 & BANK_SLOT_BYTE) != (new.player_bytes_2 & BANK_SLOT_BYTE)
        {
            let m = codec::build_bank_bag_slots_values(new.guid, new.player_bytes_2);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Ghost transition (slice 5): PLAYER_FLAGS_GHOST + the UNIT_FIELD_BYTES_1 vis bit on Release Spirit
        // (set) and reclaim (cleared) — observers see the player turn translucent/solid. Players only.
        if is_player
            && (old.player_flags != new.player_flags || old.unit_bytes_1 != new.unit_bytes_1)
        {
            let m = codec::build_ghost_values(new.guid, new.player_flags, new.unit_bytes_1);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // Live power update: rage/energy/mana changing in combat. The power-type byte of unit_bytes_0 picks
        // the right UNIT_FIELD_POWER_N. Players only — else the rage bar stays at 0 and rage spells grey out.
        if is_player && old.power != new.power {
            let power_b = (new.unit_bytes_0 >> 24) as u8;
            let m = codec::build_power_values(new.guid, power_b, new.power);
            out.push(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)));
        }
        // GM playtest `.speed`: a `run_speed_mult_bp` change relays
        // SMSG_FORCE_RUN_SPEED_CHANGE — player movement is client-authoritative, so the server-side
        // field alone never speeds the client up. Mirrors the existing `A_MOD_SPEED(MOVE)` aura relay
        // (`run_speed_packet`, same `BASE_RUN_SPEED` constant) but keyed off THIS field, any unit (not
        // player-gated — a GM could in principle god/speed-tag a creature down the line, and the relay
        // is inert for anyone not watching this guid).
        if old.run_speed_mult_bp != new.run_speed_mult_bp {
            const BASE_RUN_SPEED: f32 = 7.0; // vanilla base run yd/s — same constant as `run_speed_packet`
            let speed = BASE_RUN_SPEED * (new.run_speed_mult_bp as f32 / 10_000.0);
            out.push(ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(Box::new(
                codec::build_force_run_speed(new.guid, speed),
            )));
        }
    }
    out
}

/// SMSG_UPDATE_AURA_DURATION (buff timer): the 1.12 UpdateMask aura array carries NO duration,
/// so without this the client shows "0 seconds"/flashing icon. Only for THIS player's own auras
/// (the slot indexes the player's aura array) with a finite window; the duration is the full
/// applied span (expires - applied), which is what the client shows on apply/refresh. Pure →
/// unit-tested (hoisted verbatim out of `subscribe_player_events`, no logic change).
fn aura_duration_packet(row: &Aura, self_guid: u64) -> Option<Outbound> {
    if row.target_guid != self_guid {
        return None;
    }
    // i64::MAX is the infinite-duration sentinel; no timer packet for toggle auras.
    if row.expires_at.to_micros_since_unix_epoch() == i64::MAX {
        return None;
    }
    let ms = (row.expires_at.to_micros_since_unix_epoch()
        - row.applied_at.to_micros_since_unix_epoch())
        / 1000;
    if ms <= 0 {
        return None;
    }
    Some(Outbound::One(
        ServerOpcodeMessage::SMSG_UPDATE_AURA_DURATION(codec::build_aura_duration(
            row.slot, ms as u32,
        )),
    ))
}

/// Sprint / move-speed: a player's `A_MOD_SPEED(MOVE)` aura changing must push
/// SMSG_FORCE_RUN_SPEED_CHANGE — player movement is client-authoritative, so the server-side aura
/// alone never speeds the client up. Recompute the SUMMED move-speed % from the player's CURRENT
/// auras (post-change — on_delete fires after the row is gone — so apply AND expire both land the
/// right speed). Returns None unless `changed` is a move-speed aura on THIS player, so ordinary
/// buffs/debuffs don't spam the opcode. Pure → unit-tested (hoisted verbatim out of
/// `subscribe_player_events`, no logic change).
fn run_speed_packet(
    auras: impl Iterator<Item = crate::stdb::bindings::Aura>,
    changed: &Aura,
    self_guid: u64,
) -> Option<Outbound> {
    const A_MOD_SPEED: u8 = 0xA4; // taxonomy A_MOD_SPEED
    const SPEED_MOVE: i32 = 0; // taxonomy SPEED_MOVE (eff_p0)
    const BASE_RUN_SPEED: f32 = 7.0; // vanilla base run yd/s
    if changed.target_guid != self_guid
        || changed.eff_kind != A_MOD_SPEED
        || changed.eff_p0 != SPEED_MOVE
    {
        return None;
    }
    let pct: i32 = auras
        .filter(|a| {
            a.target_guid == self_guid && a.eff_kind == A_MOD_SPEED && a.eff_p0 == SPEED_MOVE
        })
        .map(|a| a.amount)
        .sum();
    let speed = BASE_RUN_SPEED * (1.0 + pct as f32 / 100.0);
    Some(Outbound::One(
        ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(Box::new(codec::build_force_run_speed(
            self_guid, speed,
        ))),
    ))
}

/// Build the full quest-log update from the coordinator cache. The empty mask deliberately clears
/// every quest-log field when the final quest leaves the log.
pub(crate) fn quest_log_outbound(
    db: &crate::stdb::bindings::RemoteTables,
    self_guid: u64,
) -> Option<Outbound> {
    if !crate::config::quest_log_fields_enabled() {
        return None;
    }
    let quests: Vec<_> = db.game_character_quest().iter().collect();
    let slots = super::reads::build_quest_log_slots(db, &quests, self_guid);
    let mask = codec::update_mask::full_quest_log_mask(&slots);
    let (opcode, body) = codec::build_values_update_raw(self_guid, &mask);
    Some(Outbound::Raw { opcode, body })
}

/// Build all packets for one quest update, in the order the client expects.
pub(crate) fn quest_update_outbound(
    db: &crate::stdb::bindings::RemoteTables,
    self_guid: u64,
    old: &CharacterQuest,
    row: &CharacterQuest,
) -> Vec<Outbound> {
    let objectives: Vec<_> = db
        .game_quest_objective()
        .iter()
        .filter(|objective| objective.quest_entry == row.quest_entry)
        .map(|objective| {
            (
                objective.kind,
                objective.obj_index,
                objective.target_entry,
                objective.required_count,
            )
        })
        .collect();
    quest_update_packets(old, row, &objectives, quest_log_outbound(db, self_guid))
}

/// Build the feedback that precedes a full quest-log sync for one update.
pub(crate) fn quest_update_feedback(
    old: &CharacterQuest,
    row: &CharacterQuest,
    objectives: &[(u8, u8, u32, u32)],
) -> Vec<Outbound> {
    let mut outbound = codec::kill_progress_add_kills(
        row.quest_entry,
        &old.counts,
        &row.counts,
        objectives,
    )
    .into_iter()
    .map(|packet| Outbound::One(ServerOpcodeMessage::SMSG_QUESTUPDATE_ADD_KILL(Box::new(packet))))
    .collect::<Vec<_>>();
    if !old.failed && row.failed {
        outbound.push(Outbound::One(
            ServerOpcodeMessage::SMSG_QUESTUPDATE_FAILEDTIMER(
                codec::build_questupdate_failedtimer(row.quest_entry),
            ),
        ));
    }
    outbound
}

/// Append a post-update full-log sync after kill and timer feedback.
pub(crate) fn quest_update_packets(
    old: &CharacterQuest,
    row: &CharacterQuest,
    objectives: &[(u8, u8, u32, u32)],
    quest_log: Option<Outbound>,
) -> Vec<Outbound> {
    let mut outbound = quest_update_feedback(old, row, objectives);
    if let Some(packet) = quest_log {
        outbound.push(packet);
    }
    outbound
}

fn item_gain_feedback(
    db: &RemoteTables,
    self_guid: u64,
    slot: u8,
    entry: u32,
    gained: u32,
    stack_add: bool,
) -> Vec<Outbound> {
    if gained == 0 {
        return Vec::new();
    }
    let mut out = vec![Outbound::One(ServerOpcodeMessage::SMSG_ITEM_PUSH_RESULT(
        Box::new(codec::build_item_push_result(
            self_guid,
            255,
            slot as u32,
            entry,
            gained,
            stack_add,
        )),
    ))];
    let wanted = db.game_character_quest().iter().any(|q| {
        q.character_guid == self_guid
            && !q.rewarded
            && !q.failed
            && db
                .game_quest_objective()
                .iter()
                .any(|o| o.quest_entry == q.quest_entry && o.kind == 1 && o.target_entry == entry)
    });
    if wanted {
        use wow_world_messages::vanilla::SMSG_QUESTUPDATE_ADD_ITEM;
        out.push(Outbound::One(
            ServerOpcodeMessage::SMSG_QUESTUPDATE_ADD_ITEM(SMSG_QUESTUPDATE_ADD_ITEM {
                required_item_id: entry,
                items_required: gained,
            }),
        ));
    }
    out
}

/// Build the complete live-inventory insert result for one owner.
pub(crate) fn item_instance_insert_outbound(
    db: &RemoteTables,
    self_guid: u64,
    row: &ItemInstance,
) -> Vec<Outbound> {
    let mut out = Vec::new();
    let (max_durability, container_slots) = db
        .game_item_template()
        .entry()
        .find(&row.entry)
        .map(|t| (t.max_durability, t.container_slots))
        .unwrap_or((row.durability, 0));
    let view = codec::ItemInstanceView {
        guid: row.guid,
        entry: row.entry,
        owner_guid: row.owner_guid,
        slot: row.slot,
        stack_count: row.stack_count,
        durability: row.durability,
        max_durability,
        container_slots,
    };
    out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(codec::build_item_create_object(&view)),
    )));
    if let Some(values) = codec::build_inv_slot_values(self_guid, row.slot, row.guid) {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(values),
        )));
    }
    if let Some(values) = codec::build_visible_item_values(self_guid, row.slot, row.entry) {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(values),
        )));
    }
    if let Some((bag_slot, slot_in_bag)) = bag_content_parts(row.slot) {
        if let Some(bag) = db
            .game_item_instance()
            .iter()
            .find(|item| item.owner_guid == self_guid && item.slot == bag_slot)
        {
            let (opcode, body) =
                codec::build_container_slot_values(bag.guid, slot_in_bag, row.guid);
            out.push(Outbound::Raw { opcode, body });
        }
    }
    // The item object and the descriptor that makes it reachable must precede gain feedback. The
    // shared owner-view enqueues this complete vector as one writer job, so another producer cannot
    // splice quest completion between CREATE and its inventory/container pointer.
    out.extend(item_gain_feedback(
        db,
        self_guid,
        row.slot,
        row.entry,
        row.stack_count,
        false,
    ));
    if row.slot <= 18 {
        append_item_armor_and_sheet(db, self_guid, &mut out);
    }
    out
}

/// Build the complete live-inventory delete result for one owner.
pub(crate) fn item_instance_delete_outbound(
    db: &RemoteTables,
    self_guid: u64,
    row: &ItemInstance,
) -> Vec<Outbound> {
    let mut out = Vec::new();
    if let Some(values) = codec::build_inv_slot_values(self_guid, row.slot, 0) {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(values),
        )));
    }
    if let Some(values) = codec::build_visible_item_values(self_guid, row.slot, 0) {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(values),
        )));
    }
    if let Some((bag_slot, slot_in_bag)) = bag_content_parts(row.slot) {
        if let Some(bag) = db
            .game_item_instance()
            .iter()
            .find(|item| item.owner_guid == self_guid && item.slot == bag_slot)
        {
            let (opcode, body) = codec::build_container_slot_values(bag.guid, slot_in_bag, 0);
            out.push(Outbound::Raw { opcode, body });
        }
    }
    out.push(Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
        codec::build_destroy_object(row.guid),
    )));
    if row.slot <= 18 {
        append_item_armor_and_sheet(db, self_guid, &mut out);
    }
    out
}

/// Build the complete live-inventory update result for one owner.
pub(crate) fn item_instance_update_outbound(
    db: &RemoteTables,
    self_guid: u64,
    old: &ItemInstance,
    row: &ItemInstance,
) -> Vec<Outbound> {
    let old_slot_is_empty = old.slot != row.slot
        && !db
            .game_item_instance()
            .iter()
            .any(|item| item.owner_guid == self_guid && item.slot == old.slot);
    let mut out = if row.slot == old.slot && row.stack_count > old.stack_count {
        item_gain_feedback(
            db,
            self_guid,
            row.slot,
            row.entry,
            row.stack_count - old.stack_count,
            true,
        )
    } else {
        Vec::new()
    };
    if old.slot != row.slot {
        append_final_item_slots(self_guid, old.slot, row.slot, &mut out, |slot| {
            db.game_item_instance()
                .iter()
                .find(|item| item.owner_guid == self_guid && item.slot == slot)
                .map(|item| (item.guid, item.entry))
        });
    }
    if old.stack_count != row.stack_count || old.durability != row.durability {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(codec::build_item_values(
                row.guid,
                row.stack_count,
                row.durability,
            )),
        )));
    }
    let moved_into_equipment = old.slot != row.slot && row.slot <= 18;
    let emptied_equipment = old.slot != row.slot && old.slot <= 18 && old_slot_is_empty;
    if moved_into_equipment || emptied_equipment
        || row.slot <= 18 && (old.durability == 0) != (row.durability == 0)
    {
        append_item_armor_and_sheet(db, self_guid, &mut out);
    }
    out
}

fn append_final_item_slots(
    self_guid: u64,
    old_slot: u8,
    new_slot: u8,
    out: &mut Vec<Outbound>,
    item_in_slot: impl Fn(u8) -> Option<(u64, u32)>,
) {
    let old_item = item_in_slot(old_slot);
    let new_item = item_in_slot(new_slot).unwrap_or_default();
    let slots = old_item
        .is_none()
        .then_some((old_slot, (0, 0)))
        .into_iter()
        .chain([(new_slot, new_item)]);
    for (slot, (guid, _)) in slots.clone() {
        if let Some(values) = codec::build_inv_slot_values(self_guid, slot, guid) {
            out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                Box::new(values),
            )));
        }
    }
    for (slot, (_, entry)) in slots.clone() {
        if let Some(values) = codec::build_visible_item_values(self_guid, slot, entry) {
            out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                Box::new(values),
            )));
        }
    }
    for (slot, (guid, _)) in slots {
        if let Some((bag_slot, slot_in_bag)) = bag_content_parts(slot) {
            if let Some((bag_guid, _)) = item_in_slot(bag_slot) {
                let (opcode, body) =
                    codec::build_container_slot_values(bag_guid, slot_in_bag, guid);
                out.push(Outbound::Raw { opcode, body });
            }
        }
    }
}

fn append_item_armor_and_sheet(db: &RemoteTables, self_guid: u64, out: &mut Vec<Outbound>) {
    let armor = super::armor::effective_armor(db, self_guid);
    out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(codec::build_resistance_values(self_guid, armor)),
    )));
    if let Some(stats) = super::armor::sheet_stats(db, self_guid) {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(codec::build_sheet_stats_values(self_guid, &stats)),
        )));
    }
}
impl Coordinator {
    /// Prepare and register one live viewer. Row callbacks are already armed once per shard in
    /// `world_view::arm_shard`; this method registers no callback of its own.
    pub fn subscribe_player_events(
        &self,
        account_id: u64,
        self_guid: u64,
        arrival: &codec::EntityView,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions> {
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
        let viewer = Arc::new(Viewer {
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
            let eff = super::armor::effective_armor(db, self_guid);
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
                        if let Err(e) = crate::world::party::run_bot_invite_intent(
                            &store,
                            intent_id,
                            op,
                            inviter_guid,
                            target_guid,
                        ) {
                            log::debug!(
                                "playerbots: group intent {intent_id} op {op} ({inviter_guid} -> \
                                 {target_guid}) did not execute: {e:#}"
                            );
                        }
                    });
                if let Err(error) = spawned {
                    log::error!(
                        "playerbots: could not start consumer for group intent {intent_id} op {op} \
                         ({inviter_guid} -> {target_guid}): {error}"
                    );
                }
            });
    }

    /// Wire up the session-less Shard crossing relay, the transfer twin of
    /// [`spawn_bot_invite_relay`]: one registration per connected WORLD SHARD, called ONCE at
    /// gateway startup (`main.rs`), with its own reconnect hook so a module republish re-binds the
    /// callback to the fresh `LiveConn`.
    ///
    /// A bot's party walks through a portal and the module writes a `game_bot_transfer_intent` row.
    /// There is no session to notice it — that is the whole reason the row exists — so this
    /// connection is the only one that ever could.
    pub fn spawn_bot_transfer_relay(&self) {
        for shard in self.all_shards() {
            shard.arm_bot_transfer_relay();
            let hook_shard = shard.clone();
            shard
                .0
                .on_reconnect
                .lock()
                .unwrap()
                .push(std::sync::Arc::new(move || {
                    hook_shard.arm_bot_transfer_relay();
                }));
        }
    }

    /// Wire up the character-gone relay: one registration per connected WORLD SHARD, armed once at
    /// gateway startup (`main.rs`), re-armed on reconnect like the two intent relays above.
    ///
    /// realm-core owns party membership and holds no character rows, so a Character a Shard deletes
    /// — a despawned playerbot, a deleted character — stays a member there until something asks.
    /// This watches every Shard's `game_character` deletes and, when the Character exists on no
    /// Shard at all, makes it leave its realm-core party. A Transfer deletes its source copy only
    /// after the destination copy exists, so a moved Character is found and left alone.
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
                }));
        }
    }

    fn arm_character_gone_relay(&self) {
        use crate::world::WorldStore as _;
        let store = self.clone();
        self.0
            .coord()
            .conn
            .db
            .game_character()
            .on_delete(move |_ctx, row| {
                let Some(realm) = store.realm_store() else {
                    return; // one database: the Shard's own sweep already removed the member
                };
                if crate::world::party::character_anywhere(&store, row.guid)
                    .ok()
                    .flatten()
                    .is_some()
                {
                    return;
                }
                match realm.realm_group_op(
                    lyracore_shared::group::realm_op::LEAVE,
                    row.guid,
                    0,
                    0,
                    0,
                ) {
                    Ok(()) => log::info!(
                        "party: character {} exists on no Shard and left its realm-core party",
                        row.guid
                    ),
                    Err(e) => log::debug!(
                        "party: character {} exists on no Shard; realm-core had no party to leave \
                         ({e:#})",
                        row.guid
                    ),
                }
            });
    }

    /// One shard's half of [`spawn_bot_transfer_relay`], for the same reason
    /// [`arm_bot_invite_relay`](Self::arm_bot_invite_relay) is split out: the initial call and the
    /// watchdog's post-reconnect re-arm must run the IDENTICAL registration, and this re-reads
    /// `self.0.coord()` fresh so the re-arm binds to the NEW connection rather than the dead one.
    ///
    /// A refused intent is a `warn!`, not the invite relay's `debug!`: a serendipity invite that
    /// does not happen is a party that did not form, but a crossing that does not happen is a bot
    /// left behind on the wrong Shard while its party fights without it.
    fn arm_bot_transfer_relay(&self) {
        let store = self.clone();
        self.0
            .coord()
            .conn
            .db
            .game_bot_transfer_intent()
            .on_insert(move |_ctx, row| {
                if let Err(e) = crate::world::transfer::run_bot_transfer(
                    &store,
                    row.bot_guid,
                    row.destination_map,
                    row.destination_instance,
                    &row.reason,
                ) {
                    log::warn!(
                        "playerbots: transfer intent {} (character {} -> map {} instance {}) did \
                         not execute: {e:#}",
                        row.id,
                        row.bot_guid,
                        row.destination_map,
                        row.destination_instance
                    );
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realm-core roster arrives nameless; only the blanks are filled, and a name the cache
    /// cannot answer stays blank instead of removing the member from the frame.
    #[test]
    fn a_nameless_roster_member_is_named_from_the_cache_and_a_named_one_is_kept() {
        let members = vec![
            (7, String::new(), true),
            (8, "Ginger".to_string(), true),
            (9, String::new(), false),
        ];
        let named = fill_member_names(members, |guid| match guid {
            7 => Some("Tankbot1".to_string()),
            8 => Some("Wrong".to_string()),
            _ => None,
        });
        assert_eq!(
            named,
            vec![
                (7, "Tankbot1".to_string(), true),
                (8, "Ginger".to_string(), true),
                (9, String::new(), false),
            ]
        );
    }

    fn viewer_with_created(creature_guid: u64) -> Viewer {
        let (tx, _rx) = SessionTx::with_depth(0);
        Viewer {
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
            let (expected_opcode, expected_body) = codec::build_virtual_item_values(
                creature_guid,
                expected.0,
                expected.1,
                expected.2,
            );
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
        for chat_type in [0, CHAT_YELL, codec::social::CHAT_TEXT_EMOTE] {
            let out = chat_event_message(
                &ChatEvent {
                    chat_type,
                    ..base.clone()
                },
                Some("Defias Thug".into()),
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
        assert_eq!(requests[0].initiator, wow_world_messages::Guid::new(row.flag_guid));
        assert_eq!(requests[0].target, wow_world_messages::Guid::new(row.initiator_guid));
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

    /// The trade-status wire mapping (#120): every `lyracore_shared::trade::event_kind` the module
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
                Outbound::One(ServerOpcodeMessage::SMSG_TRADE_STATUS(s)) => (**s).clone(),
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
        assert_eq!(status_of(kind::TRADE_CANCELED), SMSG_TRADE_STATUS::TradeCanceled);
        assert_eq!(status_of(kind::BUSY), SMSG_TRADE_STATUS::Busy);
        assert_eq!(status_of(kind::NO_TARGET), SMSG_TRADE_STATUS::NoTarget);
        assert_eq!(status_of(kind::TARGET_TO_FAR), SMSG_TRADE_STATUS::TargetToFar);
        assert_eq!(status_of(kind::WRONG_FACTION), SMSG_TRADE_STATUS::WrongFaction);
        assert_eq!(status_of(kind::YOU_DEAD), SMSG_TRADE_STATUS::YouDead);
        assert_eq!(status_of(kind::TARGET_DEAD), SMSG_TRADE_STATUS::TargetDead);
        assert_eq!(status_of(kind::IGNORE_YOU), SMSG_TRADE_STATUS::IgnoreYou);
        assert_eq!(status_of(kind::TRADE_ACCEPT), SMSG_TRADE_STATUS::TradeAccept);
        assert_eq!(status_of(kind::BACK_TO_TRADE), SMSG_TRADE_STATUS::BackToTrade);
        assert_eq!(status_of(kind::TRADE_COMPLETE), SMSG_TRADE_STATUS::TradeComplete);
        // The bag-space refusal closes the window naming WHOSE bags overflowed.
        let full_self = status_of(kind::INV_FULL_SELF);
        let full_partner = status_of(kind::INV_FULL_PARTNER);
        match (&full_self, &full_partner) {
            (
                SMSG_TRADE_STATUS::CloseWindow { target_error: false, inventory_result: a, .. },
                SMSG_TRADE_STATUS::CloseWindow { target_error: true, inventory_result: b, .. },
            ) => {
                assert_eq!(*a, wow_world_messages::vanilla::InventoryResult::InventoryFull);
                assert_eq!(*b, wow_world_messages::vanilla::InventoryResult::InventoryFull);
            }
            other => panic!("expected CloseWindow pair with flipped target_error, got {other:?}"),
        }
        // The purse-failure pair mirrors it with NotEnoughMoney.
        match (&status_of(kind::GOLD_FAIL_SELF), &status_of(kind::GOLD_FAIL_PARTNER)) {
            (
                SMSG_TRADE_STATUS::CloseWindow { target_error: false, inventory_result: a, .. },
                SMSG_TRADE_STATUS::CloseWindow { target_error: true, inventory_result: b, .. },
            ) => {
                assert_eq!(*a, wow_world_messages::vanilla::InventoryResult::NotEnoughMoney);
                assert_eq!(*b, wow_world_messages::vanilla::InventoryResult::NotEnoughMoney);
            }
            other => panic!("expected NotEnoughMoney CloseWindow pair, got {other:?}"),
        }

        assert!(
            trade_event_outbound(&event(200)).is_empty(),
            "an unknown kind must drop, not guess a status"
        );
    }

    /// The OFFER_* kinds decode to the fixed-444-byte extended status (#121): the polarity byte
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
                Some(Outbound::One(ServerOpcodeMessage::SMSG_TRADE_STATUS_EXTENDED(m))) => {
                    (**m).clone()
                }
                _ => panic!("kind {k}: expected one SMSG_TRADE_STATUS_EXTENDED"),
            }
        };

        // Wire polarity (mangoszero): the `self_player` BYTE is "1 means traders data, 0 means
        // own", so your OWN echo carries 0 and the partner's side carries 1.
        let own = extended(kind::OFFER_SELF);
        assert!(!own.self_player, "OFFER_SELF describes your own window: wire byte 0");
        let theirs = extended(kind::OFFER_PARTNER);
        assert!(theirs.self_player, "OFFER_PARTNER describes the partner's side: wire byte 1");

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

    const HEROIC_STRIKE_CASTER: u64 = 7;
    /// The module's `CAST_FAIL_NO_POWER`.
    const NO_POWER_CODE: u8 = 1;

    /// An interruption signal row for `spell_id` cast by `HEROIC_STRIKE_CASTER`, carrying
    /// `failure_reason`. Every other field stays at the module baseline's zero, which is what such a
    /// row really looks like on the wire.
    fn interrupted_cast(spell_id: u32, failure_reason: u8) -> SpellCastEvent {
        SpellCastEvent {
            id: 1,
            caster_guid: HEROIC_STRIKE_CASTER,
            spell_id,
            created_at: spacetimedb_sdk::Timestamp::UNIX_EPOCH,
            target_guid: 0,
            cast_time_ms: 0,
            is_completion: false,
            damage: 0,
            school: 0,
            is_crit: false,
            resisted: 0,
            absorbed: 0,
            is_interrupted: true,
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
            failure_reason,
        }
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

    /// A live-entity row with no pending change; tests clone it and mutate one field at a time.
    /// `type_mask` carries the PLAYER bit by default (most relayed fields are player-only).
    fn player_entity() -> WorldEntity {
        WorldEntity {
            guid: 1,
            owner_identity: spacetimedb_sdk::Identity::from_byte_array([0u8; 32]),
            account_id: 1,
            map_id: 0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            orientation: 0.0,
            grid_x: 0,
            grid_y: 0,
            cell: lyracore_shared::spatial::grid_cell_id(0, 0),
            last_move_ms: 0,
            type_mask: lyracore_shared::constants::type_mask::PLAYER_BIT,
            entry: 0,
            scale_x: 1.0,
            health: 100,
            max_health: 100,
            power: 100,
            max_power: 100,
            level: 1,
            faction_template: 0,
            unit_bytes_0: 0,
            display_id: 0,
            native_display_id: 0,
            unit_flags: 0,
            base_attack_time_ms: 2000,
            dynamic_flags: 0,
            dead: false,
            player_bytes: 0,
            player_bytes_2: 0,
            player_bytes_3: 0,
            player_flags: 0,
            xp: 0,
            next_level_xp: 100,
            target_guid: 0,
            money: 0,
            unit_bytes_1: 0,
            unit_bytes_2: 0,
            strength: 1,
            agility: 1,
            stamina: 1,
            intellect: 1,
            spirit: 1,
            npc_flags: 0,
            armor: 0,
            leg_ends_ms: 0,
            wp_target: 0,
            movement_flags: 0,
            combat_until_ms: 0,
            pickpocketed: false,
            next_swing_spell: 0,
            overpower_until_ms: 0,
            revenge_until_ms: 0,
            stance: 0,
            owner_guid: 0,
            skinned: false,
            mana_regen_paused_until_ms: 0,
            death_expire_micros: 0,
            instance_id: 0,
            run_speed_mult_bp: 10_000,
            godmode: false,
            resting: false,
            sheet_str_bonus: 0,
            sheet_agi_bonus: 0,
            sheet_sta_bonus: 0,
            sheet_int_bonus: 0,
            sheet_spi_bonus: 0,
            sheet_ap_base: 0,
            sheet_ap_mods: 0,
            sheet_dmg_min: 0,
            sheet_dmg_max: 0,
            sheet_crit_bp: 0,
            bank_bag_slots: 0,
            mount_display_id: 0,
            zone_id: 0,
            sheet_ranged_ap: 0,
            sheet_ranged_dmg_min: 0,
            sheet_ranged_dmg_max: 0,
        }
    }

    fn creature_entity() -> WorldEntity {
        let mut entity = player_entity();
        entity.guid = 99;
        entity.type_mask = lyracore_shared::constants::type_mask::CREATURE;
        entity.dynamic_flags = lyracore_shared::constants::unit_dynamic_flags::TAPPED;
        entity
    }

    fn tap(character_guid: u64) -> CreatureQuestTap {
        CreatureQuestTap {
            creature_guid: 99,
            character_guid,
        }
    }

    fn tag_member(character_guid: u64) -> CreatureQuestTapMember {
        CreatureQuestTapMember {
            id: character_guid,
            creature_guid: 99,
            character_guid,
        }
    }

    fn current_member(character_guid: u64, group_id: u64) -> GroupMember {
        GroupMember {
            id: character_guid,
            group_id,
            character_guid,
            owner_identity: spacetimedb_sdk::Identity::from_byte_array([0; 32]),
        }
    }

    fn project_flags(
        viewer_guid: u64,
        entity: &WorldEntity,
        tap: Option<&CreatureQuestTap>,
        tag_group: Option<&CreatureLootTagGroup>,
        tag_members: &[CreatureQuestTapMember],
        current_members: &[GroupMember],
        corpse_eligible: &[CorpseLootEligible],
    ) -> u32 {
        viewer_relative_dynamic_flags(
            viewer_guid,
            entity,
            LootTagProjectionRows {
                tap,
                tag_group,
                tag_members,
                current_members,
                corpse_eligible,
            },
        )
    }

    #[test]
    fn solo_tagger_sees_tapped_by_player() {
        let entity = creature_entity();
        let tap = tap(7);
        assert_eq!(
            project_flags(7, &entity, Some(&tap), None, &[], &[], &[]),
            entity.dynamic_flags | lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER
        );
    }

    #[test]
    fn snapshot_group_member_sees_tapped_by_player() {
        let entity = creature_entity();
        let tap = tap(7);
        let group = CreatureLootTagGroup {
            creature_guid: entity.guid,
            group_id: 42,
        };
        let members = [tag_member(7), tag_member(8)];
        let current = [current_member(8, 42)];
        assert_eq!(
            project_flags(8, &entity, Some(&tap), Some(&group), &members, &current, &[]),
            entity.dynamic_flags | lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER
        );
    }

    #[test]
    fn stranger_sees_only_stored_tapped_flag() {
        let entity = creature_entity();
        let tap = tap(7);
        assert_eq!(
            project_flags(8, &entity, Some(&tap), None, &[], &[], &[]),
            entity.dynamic_flags
        );
    }

    #[test]
    fn later_group_joiner_sees_only_stored_tapped_flag() {
        let entity = creature_entity();
        let tap = tap(7);
        let group = CreatureLootTagGroup {
            creature_guid: entity.guid,
            group_id: 42,
        };
        let members = [tag_member(7)];
        let current = [current_member(8, 42)];
        assert_eq!(
            project_flags(8, &entity, Some(&tap), Some(&group), &members, &current, &[]),
            entity.dynamic_flags
        );
    }

    #[test]
    fn group_leaver_loses_tapped_by_player() {
        let entity = creature_entity();
        let tap = tap(7);
        let group = CreatureLootTagGroup {
            creature_guid: entity.guid,
            group_id: 42,
        };
        let members = [tag_member(7), tag_member(8)];
        let current = [current_member(8, 99)];
        assert_eq!(
            project_flags(8, &entity, Some(&tap), Some(&group), &members, &current, &[]),
            entity.dynamic_flags
        );
    }

    #[test]
    fn eligible_corpse_retains_lootable() {
        let mut entity = creature_entity();
        entity.dead = true;
        entity.dynamic_flags = lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
        let eligible = [CorpseLootEligible {
            id: 1,
            corpse_guid: entity.guid,
            eligible_guid: 7,
        }];
        assert_eq!(
            project_flags(7, &entity, None, None, &[], &[], &eligible),
            entity.dynamic_flags
        );
    }

    #[test]
    fn foreign_viewer_does_not_see_corpse_lootable() {
        let mut entity = creature_entity();
        entity.dead = true;
        entity.dynamic_flags = lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
        let eligible = [CorpseLootEligible {
            id: 1,
            corpse_guid: entity.guid,
            eligible_guid: 7,
        }];
        assert_eq!(project_flags(8, &entity, None, None, &[], &[], &eligible), 0);
    }

    #[test]
    fn empty_corpse_eligibility_does_not_show_lootable() {
        let mut entity = creature_entity();
        entity.dead = true;
        entity.dynamic_flags = lyracore_shared::constants::unit_dynamic_flags::LOOTABLE;
        assert_eq!(project_flags(7, &entity, None, None, &[], &[], &[]), 0);
    }

    #[test]
    fn untagged_entity_flags_are_unchanged() {
        let mut entity = creature_entity();
        entity.dynamic_flags = 0x42;
        assert_eq!(project_flags(7, &entity, None, None, &[], &[], &[]), 0x42);
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

    /// #101: a sheath change must relay, or drawing a weapon is invisible to everyone but the player
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
        let projected = new.dynamic_flags
            | lyracore_shared::constants::unit_dynamic_flags::TAPPED_BY_PLAYER;
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
        // The taxi-landing twin of the activation test above: display AND the flight bit clear
        // TOGETHER. Must relay through the SAME coupled builder, never the standalone land-mount one
        // alongside it (the double-fire this ticket's diff separation must avoid).
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

    /// A minimal `Aura` row for the packet-builder tests: `target_guid`/`eff_kind`/`eff_p0`/`amount`
    /// are the fields those builders branch on; `applied_at`/`expires_at` (raw micros) drive the
    /// duration math. Everything else is a fixed, inert value.
    fn aura(
        target_guid: u64,
        eff_kind: u8,
        eff_p0: i32,
        amount: i32,
        applied_at_micros: i64,
        expires_at_micros: i64,
    ) -> Aura {
        Aura {
            id: 1,
            target_guid,
            caster_guid: target_guid,
            spell_id: 6673,
            slot: 3,
            level: 1,
            flags: 0,
            applied_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(applied_at_micros),
            expires_at: spacetimedb_sdk::Timestamp::from_micros_since_unix_epoch(expires_at_micros),
            effect_id: 1,
            eff_kind,
            amount,
            eff_p0,
            eff_p0_kind: 0,
            eff_p1: 0,
            period_ms: 0,
            amount_remaining: 0,
            stacks: 1,
            next_tick_micros: 0,
            channel_target: 0,
            enters_combat: false,
            proc_flags: 0,
            proc_chance: 0,
            proc_ppm: 0.0,
            proc_ex: 0,
            proc_school_mask: 0,
            proc_family_name: 0,
            proc_family_flags: 0,
            proc_charges: 0,
            proc_icd_ms: 0,
            proc_ready_micros: 0,
        }
    }

    #[test]
    fn aura_duration_packet_pins_the_remaining_window_for_self_only() {
        let self_guid = 1u64;
        // A 5-second remaining window on the player's OWN aura, in slot 3.
        let mine = aura(self_guid, 0xA1, 1, 10, 0, 5_000_000);
        let out = aura_duration_packet(&mine, self_guid);
        let m = match out {
            Some(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_AURA_DURATION(m))) => m,
            _ => panic!("expected Some(Outbound::One(SMSG_UPDATE_AURA_DURATION))"),
        };
        assert_eq!(m, codec::build_aura_duration(3, 5000));
        // A peer's aura (not self) never gets a duration packet — the slot only indexes the
        // viewer's OWN aura array.
        assert!(aura_duration_packet(&mine, self_guid + 1).is_none());
        // The infinite-duration sentinel (a toggle aura) never gets a timer packet.
        let toggle = aura(self_guid, 0xA1, 1, 10, 0, i64::MAX);
        assert!(aura_duration_packet(&toggle, self_guid).is_none());
        // A non-positive remaining window (expires_at <= applied_at) emits nothing.
        let expired = aura(self_guid, 0xA1, 1, 10, 5_000_000, 5_000_000);
        assert!(aura_duration_packet(&expired, self_guid).is_none());
    }

    #[test]
    fn run_speed_packet_sums_move_speed_auras_on_self_only() {
        let self_guid = 1u64;
        // Two move-speed auras on self (e.g. Sprint 25% + a totem 5%) sum into ONE force-speed packet.
        let a1 = aura(self_guid, 0xA4, 0, 25, 0, i64::MAX);
        let a2 = aura(self_guid, 0xA4, 0, 5, 0, i64::MAX);
        let current = vec![a1.clone(), a2.clone()];
        let out = run_speed_packet(current.clone().into_iter(), &a1, self_guid);
        let m = match out {
            Some(Outbound::One(ServerOpcodeMessage::SMSG_FORCE_RUN_SPEED_CHANGE(m))) => m,
            _ => panic!("expected Some(Outbound::One(SMSG_FORCE_RUN_SPEED_CHANGE))"),
        };
        let expected_speed = 7.0f32 * (1.0 + 30_f32 / 100.0); // BASE_RUN_SPEED * (1 + summed pct)
        assert_eq!(*m, codec::build_force_run_speed(self_guid, expected_speed));
        // A non-move-speed change on self spams nothing (ordinary buffs/debuffs don't trigger this).
        let unrelated = aura(self_guid, 0xA1, 1, 10, 0, i64::MAX);
        assert!(run_speed_packet(current.clone().into_iter(), &unrelated, self_guid).is_none());
        // A move-speed change on a DIFFERENT guid (a peer) never triggers self's packet.
        let peer_changed = aura(self_guid + 1, 0xA4, 0, 25, 0, i64::MAX);
        assert!(run_speed_packet(current.into_iter(), &peer_changed, self_guid).is_none());
    }

    #[test]
    fn bag_content_parts_boundaries() {
        assert_eq!(bag_content_parts(119), None); // just below the bag-content range
        assert_eq!(bag_content_parts(120), Some((19, 0))); // first bag, first slot
        assert_eq!(bag_content_parts(137), Some((19, 17))); // first bag, last slot (0..MAX_BAG_SIZE)
        assert_eq!(bag_content_parts(138), Some((20, 0))); // second bag, first slot
        assert_eq!(bag_content_parts(191), Some((22, 17))); // fourth (last) bag, last slot
                                                            // The function has no upper-bound gate on the nominal 4-bag range (that gate lives at the
                                                            // caller, which only ever looks up a bag that actually exists) — pin its actual unclamped
                                                            // behavior past 191 rather than asserting an invariant it doesn't enforce.
        assert_eq!(bag_content_parts(192), Some((23, 0)));
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
                .map(|item| (item.1, item.2))
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
            (slot == 0).then_some((202, 1337))
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
            (slot == 24).then_some((202, 1337))
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
        // ...but visible to a ghost.
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

    /// The non-test half of this file as the source scans below may read it: `//` comments STRIPPED
    /// and every whitespace run collapsed to one space.
    ///
    /// Both steps are mutation findings, not tidiness. Counting matches in the raw text made every
    /// scan below satisfiable by a COMMENT quoting the pattern while the real call was deleted —
    /// live-mutated green, with both gates permanently shut (no discovery toast, no inn-crossing
    /// relay, ever). Collapsing whitespace is the other half: it stops a rustfmt line-split from
    /// failing a scan that nothing behavioural broke, and it lets a scan pin a whole STATEMENT
    /// rather than a bare identifier — which is what makes "call it and throw the packet away"
    /// visible.
    fn scanned_source() -> String {
        let src = include_str!("subscriptions.rs");
        let body = src
            .split("mod tests {")
            .next()
            .expect("the non-test half of this file");
        let decommented: String = body
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        decommented.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The span of ONE top-level `fn NAME(` in a DIFFERENT file of this module tree — the
    /// shared-connection model moved the AOI wiring out of this file, and a tripwire that can only
    /// see this file would have gone
    /// quiet rather than failing. Brace-matched, so it works on `impl`-nested layouts too. Returns
    /// the RAW (not comment-stripped) text — callers that need the mutation-hardened decommenting
    /// run it through [`decommented`] themselves, same two-step `scanned_source` already uses.
    fn top_level_fn_body_of(file: &str, name: &str) -> &'static str {
        let src: &'static str = match file {
            "world_view.rs" => include_str!("world_view.rs"),
            "connection.rs" => include_str!("connection.rs"),
            other => panic!("no scanner registered for `{other}`"),
        };
        let start = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("`fn {name}(` not found in {file}"));
        let open = src[start..].find('{').expect("a body") + start;
        let mut depth = 0usize;
        for (i, c) in src[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &src[open..open + i + 1];
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces scanning `{name}` in {file}");
    }

    /// Comment-stripped, whitespace-collapsed form of an arbitrary source span — the same two-step
    /// mutation-hardening [`scanned_source`] applies to the whole file (strip a `//` line comment
    /// FIRST, so a scan can't be satisfied by a comment quoting the pattern; collapse whitespace
    /// SECOND, so a rustfmt line-split can't fail a scan that pins a whole statement).
    fn decommented(text: &str) -> String {
        let stripped: String = text
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        stripped.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The call-site tripwire for the shared-view `Viewer`'s construction.
    ///
    /// The session's `Viewer` must hold the same dedup/gate state prepared during world entry.
    /// Hand it a fresh instance and the exactly-once CREATE or spirit-healer gate silently splits.
    #[test]
    fn the_viewer_is_registered_with_the_shared_dedup_sets_and_gates() {
        let code = scanned_source();
        for field in [
            "bound_identity: self_identity,",
            "created: created.clone(),",
            "gates: viewer_gates.clone(),",
            "skill_slots: skill_slots.clone(),",
        ] {
            assert!(
                code.contains(field),
                "the shared-view `Viewer` is no longer constructed with `{field}` — world entry and \
                 shared writer jobs now use different relay state"
            );
        }
        assert!(
            code.contains("view.add_viewer( self, viewer.clone(), CellKey::of_position(login_map, login_instance, login_x, login_y), )"),
            "the session is no longer registered with the shared AOI view at its LOGIN cell — \
             without the registration the client sees an empty world; with the wrong anchor it sees \
             somebody else's neighbourhood"
        );
    }


    /// Tripwire for the realm-core PRIVATE tier (#22 → #483): the cross-shard whisper/group
    /// twins are armed ONCE per realm-core connection (`arm_realm_private`), gated on realm-core
    /// being a DISTINCT database. Without the gate a single-database gateway registers a SECOND
    /// callback on tables `arm_shard` already watches and every private packet is delivered
    /// twice; without the arming a cross-shard whisper is written on realm-core and delivered to
    /// nobody. No fake in this tree can reach either (a callback on another database's live
    /// coordinator connection), so the wiring is pinned in source, comment-stripped.
    #[test]
    fn the_realm_private_tier_is_armed_once_gated_on_a_distinct_database() {
        let body = decommented(top_level_fn_body_of("connection.rs", "arm_shared_world_view"));
        assert!(
            body.contains("if let Ok(realm) = self.realm_core() {"),
            "arming no longer resolves the realm-core handle — pointed anywhere else it hears \
             nothing at all"
        );
        assert!(
            body.contains("if !shards.iter().any(|s| s.shard_name() == realm.shard_name()) {"),
            "the realm-core private tier lost its DISTINCT-DATABASE guard. On a single-database \
             gateway (or with `LYRACORE_REALM_CORE` naming a world shard) `arm_shard` already \
             watches these tables, so arming realm-core too delivers every invite dialog and \
             whisper line twice."
        );
        assert!(
            body.contains("super::world_view::arm_realm_private(view.clone(), realm.clone(), world.clone());"),
            "the realm-core private tier is never armed — a cross-shard whisper is written on \
             realm-core and delivered to nobody"
        );
        assert_eq!(
            body.matches("arm_realm_private(").count(),
            2,
            "the realm-core private tier must be armed exactly twice in source: once at connect \
             and once inside the reconnect re-arm hook — a module republish closes the websocket, \
             and without the hook the relay dies silently at the first republish"
        );
    }

    /// The other half: `arm_realm_private` must register BOTH tables through the same
    /// recipient-keyed dispatchers as `arm_shard`'s private tier. The recipient filter is
    /// structural there (`session_of_owner(row.recipient_guid)` + `private_recipient_audience`)
    /// — these reads ride the OWNER TOKEN, which bypasses RLS, so a dispatcher that fanned to
    /// viewers instead would hand every player's private whispers to every session.
    #[test]
    fn the_realm_private_relays_ride_the_recipient_keyed_dispatchers() {
        let body = decommented(top_level_fn_body_of("world_view.rs", "arm_realm_private"));
        assert!(
            body.contains("wire_insert_live(db.game_whisper_event(), \"realm.game_whisper_event.insert\", &view, |v, row| { whisper_appeared(v, row) });"),
            "arm_realm_private no longer relays realm-core whispers through `whisper_appeared`"
        );
        assert!(
            body.contains("wire_insert_live(db.game_group_event(), \"realm.game_group_event.insert\", &view, move |v, row| { group_event_appeared(v, &coord, row) });"),
            "arm_realm_private no longer relays realm-core group events through \
             `group_event_appeared` (which also carries the QUEST_SHARE detail JOIN through a \
             WORLD handle — realm-core's cache has no quest catalogue)"
        );
        // The dispatchers themselves stay recipient-keyed: the whisper body must resolve the
        // recipient's session FIRST and re-assert the audience predicate.
        let whisper = decommented(top_level_fn_body_of("world_view.rs", "whisper_appeared"));
        assert!(
            whisper.contains("session_of_owner(row.recipient_guid)")
                && whisper.contains("private_recipient_audience(row.recipient_guid, viewer.self_guid)"),
            "whisper_appeared is no longer recipient-keyed — on an owner-token read that is a \
             privacy leak: every session would receive every player's private whispers"
        );
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

    fn relay_creature_leg(tx: &SessionTx, created: &Arc<Mutex<HashSet<u64>>>, row: &CreatureSpline) {
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

    /// **Fan-out collapse.** Both relays push with a discarding send; a session whose writer half is
    /// already gone used to swallow the packet with no trace. Both discard sites are driven here
    /// (a dropped receiver is exactly the "writer gone" case) and the counter must move for each.
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
            },
        );
        assert_eq!(
            MOTION_DROPPED.load(Relaxed),
            before + 2,
            "a creature leg discarded by a dead writer channel must increment MOTION_DROPPED too"
        );
    }

    /// #518: a `facing`-flagged `game_creature_spline` row relays as `SMSG_MONSTER_MOVE`'s
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

        // A non-facing leg (the pre-518 baseline) keeps the Normal variant unchanged.
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
        assert!(line.contains("calls=5000"), "cumulative calls missing: {line}");
        assert!(line.contains("sent=4500"), "cumulative sent missing: {line}");
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
        assert!(line.contains("submitted=400"), "submitted count missing: {line}");
        assert!(!line.contains("completed="), "retired completed count remains: {line}");
        assert!(!line.contains("outstanding="), "retired outstanding count remains: {line}");

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

    /// The relay's own WIRING, which no unit test can reach: `relay_entity_motion` only ever runs
    /// because both halves of the `game_entity_motion` subscription call it (insert = a mover's first
    /// heartbeat in this box, update = every one after). Deleting either closure leaves every test
    /// above green and freezes peers — the stuck-relay shape, and the "grep every subscriber when a
    /// relay changes shape" hazard the peer-motion relay work opens with. A scan (not a mock) for the
    /// reason the other tripwires in this file are: the callbacks are registered on a live
    /// `DbConnection`.
    /// Both halves of the motion relay, re-pinned for the shared-connection model: `game_entity_motion` is
    /// UPSERTED per mover per heartbeat, so `on_insert` is a mover's first heartbeat in the world and
    /// `on_update` is essentially all peer movement. Dropping either registration is silent — peers
    /// freeze after their first step, or never move at all. Same for the creature-leg twin.
    #[test]
    fn both_halves_of_the_motion_and_spline_relays_are_registered_286() {
        let arm = decommented(top_level_fn_body_of("world_view.rs", "arm_shard"));
        // #490 factored every registration through `wire_insert`/`wire_update` (world_view.rs),
        // so the literal `.on_insert(`/`.on_update(` chain off the table handle is gone from
        // `arm_shard`'s own body — what's left to scan for is the (helper, table, label) triple
        // that proves the callback is still wired, not skipped.
        for (helper, table, label, what) in [
            ("wire_insert", "game_entity_motion", "game_entity_motion.insert", "a mover's FIRST heartbeat"),
            ("wire_update", "game_entity_motion", "game_entity_motion.update", "every heartbeat after the first — i.e. all peer movement"),
            ("wire_insert", "game_creature_spline", "game_creature_spline.insert", "a creature's first leg"),
            ("wire_update", "game_creature_spline", "game_creature_spline.update", "every leg after the first"),
        ] {
            assert!(
                arm.contains(&format!("{helper}(db.{table}()")) && arm.contains(&format!("\"{label}\"")),
                "the {table} {label} half is no longer registered on the shared coordinator \
                 dispatch — {what} is silently dropped for every session"
            );
        }
        // And the dispatch routes them through the cell index rather than broadcasting.
        let m = decommented(top_level_fn_body_of("world_view.rs", "motion"));
        assert!(
            m.contains("view.spatial.viewers_of(EntityLayer::WorldEntity, key)"),
            "peer motion no longer asks the cell index who can see the mover — either every session \
             gets every mover (the fan-out this issue removed) or none do"
        );
    }


    /// And the instrument's wiring: the counters are only ever read by the 10-second task in
    /// `world::run`. Without this scan, reverting that task to the old hand-formatted line — no
    /// delivery ratio, no dropped count — leaves the whole suite green, which is precisely how the
    /// 63 % under-delivery went unlogged in the first place.
    #[test]
    fn the_10s_task_prints_the_delivery_instrument_287() {
        let body = crate::test_scan::code_of(
            include_str!("../world/mod.rs"),
            "pub async fn run(",
        );
        assert!(
            body.contains("crate::stdb::subscriptions::MOTION_DROPPED.load(Relaxed)")
                && body.contains("crate::stdb::subscriptions::motionstat_line("),
            "the MOTIONSTAT task no longer reads MOTION_DROPPED and formats through \
             `motionstat_line` — the delivery ratio and dropped count are gone from the log, and a \
             degrading relay is once again silent. Body was:\n{body}"
        );
    }

    /// And the fan-out signal's WIRING (task B1). `fanout_health_step` is pure and fully tested above,
    /// which is exactly the trap playbook §8 names: the decision can be perfect while the only thing
    /// that CALLS it — the 10-second task in `world::run`, unreachable from a unit test — quietly
    /// disappears, leaving the suite green and the under-delivery silent again. A scan, because the
    /// task body is a `tokio::spawn` inside `run`; asserted on both the step call and the `log::warn!`
    /// that is the entire deliverable (computing the verdict and not printing it would be the same
    /// "reported the call, not the effect" defect class).
    #[test]
    fn the_10s_task_warns_on_a_fanout_collapse_b1() {
        let body = crate::test_scan::code_of(
            include_str!("../world/mod.rs"),
            "pub async fn run(",
        );
        assert!(
            body.contains("crate::stdb::subscriptions::fanout_health_step(fan, fanout, submitted_delta)"),
            "the 10s task no longer runs the fan-out collapse check — peer movement can degrade by \
             40% and the log will say nothing but a MOTIONSTAT line nobody can calibrate. Body \
             was:\n{body}"
        );
        assert!(
            body.contains("if let Some(low_windows) = fanout_warn {") && body.contains("log::warn!"),
            "the fan-out verdict is computed but no longer WARNED — the whole point is that it says \
             something out loud. Body was:\n{body}"
        );
    }

}
