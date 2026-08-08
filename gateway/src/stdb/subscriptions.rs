//! Per-player view subscriptions: the `PlayerSubscriptions` RAII guard (callback teardown +
//! unsubscribe on drop) and `Coordinator::subscribe_player_events` — the 51-callback relay that
//! turns row deltas on the per-account connection into outbound SMSG. Moved verbatim from
//! `mod.rs`; that function's own doc comment (just above it) is the structure map, the teardown-
//! contract pointer, and the planned carve (#353).
//!
//! **#468 took the four box-scoped tables out of here entirely.** Peer visibility, peer movement,
//! creature legs and gameobjects are no longer per-player registrations: they ride ONE global
//! subscription per shard on the coordinator connection, and `stdb::world_view` routes each row to
//! the sessions an in-process cell index says can see it. What is left in this file for those
//! tables is the RELAY BODIES — [`offer_peer_create_for`], [`relay_entity_update`],
//! [`relay_peer_destroy`], [`relay_gameobject_create`], [`motion_outbound`],
//! [`creature_leg_outbound`] — each of which
//! decides and encodes for exactly ONE viewer and is run on that session's own writer thread, never
//! on the shared pump.
//!
//! That carve is also the shape #353 asks for on the rest: pure-ish top-level items with their own
//! tests, called from a thin registration. [`entity_update_to_outbound`] was the first of them.

use crate::codec::{self, CreateKind};
use crate::world::{Outbound, SessionTx};
use anyhow::{anyhow, Result};
use spacetimedb_sdk::{DbContext, SubscriptionHandle as _, Table, TableWithPrimaryKey};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use wow_world_messages::vanilla::opcodes::ServerOpcodeMessage;
use wow_world_messages::vanilla::Vector3d;

use super::aoi::ViewerGates;
use super::world_index::CellKey;
use super::world_view::{self, Viewer, WorldView};
use super::bindings::*;
use super::connection::{Coordinator, PlayerConn};
use super::views::{corpse_view, entity_view, go_view};

/// RAII guard for one world session's per-player view subscriptions + delta callbacks, held by the
/// world connection. Because the `PlayerConn` is cached and reused across logins, the callbacks
/// MUST be removed (and the subscription dropped) when the session ends, or a relogin would
/// double-register them and peers would double-spawn. Dropping the guard does exactly that.
pub struct PlayerSubscriptions {
    conn: Option<Arc<PlayerConn>>,
    sub: Option<SubscriptionHandle>,
    // A homogeneous teardown list is the point (see the doc above): one `push` per relay instead of a hand-mirrored field.
    #[allow(clippy::type_complexity)]
    /// One teardown closure per registered relay callback, each removing its callback from the
    /// cached `PlayerConn`; drained in `Drop`. Homogeneous so adding a relay is a single `push` at
    /// registration instead of a new field hand-mirrored across the struct, `empty()`, and `Drop`.
    teardowns: Vec<Box<dyn FnOnce(&PlayerConn) + Send>>,
    /// This session's registration in the gateway-wide shared view (#468). `None` only for the
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
            conn: None,
            sub: None,
            teardowns: Vec::new(),
            viewer: None,
            view: None,
        }
    }

    /// Drive the shared AOI index from the player's movement: on a cell crossing, move the
    /// viewer's anchor and relay the CREATE/DESTROY delta the move implies.
    ///
    /// Runs on the session's own thread and does no I/O: the delta is a 10-cell set diff in memory
    /// and the entering rows are read out of coordinator caches that are already resident. Before
    /// #468 this was an unsubscribe + resubscribe round trip per cell crossing, per player.
    pub fn aoi_update(&mut self, x: f32, y: f32) {
        let (Some(viewer), Some(view)) = (self.viewer.as_ref(), self.view.as_ref()) else {
            return;
        };
        world_view::recenter(view, viewer, viewer.map_id, x, y);
    }
}

impl Drop for PlayerSubscriptions {
    fn drop(&mut self) {
        // Remove every relay callback (else a relogin on the cached PlayerConn double-registers them
        // and peers double-spawn), then unsubscribe.
        if let Some(conn) = self.conn.take() {
            for teardown in self.teardowns.drain(..) {
                teardown(&conn);
            }
        }
        if let Some(h) = self.sub.take() {
            let _ = h.unsubscribe();
        }
        // #468: leave the shared view LAST — while the registration is live the coordinator
        // dispatch may still enqueue for this session, which is harmless (the writer is draining or
        // gone) but pointless.
        if let (Some(view), Some(viewer)) = (self.view.take(), self.viewer.take()) {
            view.remove_viewer(viewer.session, viewer.self_guid);
        }
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

/// Build the wire relay for one `game_teleport_event` row (work-item 224). Pure: takes the
/// already-derived `still_here` boolean (whether the live entity survived the transaction — the
/// module/gateway-mirrored same-map/cross-map signal, see the `on_teleport` callback's doc comment)
/// instead of a live `ReducerContext`/connection, so it's testable without a DB (module crate
/// convention: extract pure functions and test those).
///
/// `still_here` → same-map: `MSG_MOVE_TELEPORT_ACK` (byte-identical to the pre-224 behavior — the ONLY
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
fn build_peer_create(coord: &Coordinator, row: &WorldEntity) -> Option<ServerOpcodeMessage> {
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
    // Peers pass no skill rows: the SkillInfo block is a self-descriptor (the client renders
    // only its OWN skill pane); a peer CREATE ignores it.
    match codec::build_create_object(&entity_view(row.clone(), 0), CreateKind::Peer, &inv, &[]) {
        Ok(m) => Some(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m))),
        Err(e) => {
            log::warn!("peer create encode failed for guid {}: {e}", row.guid);
            None
        }
    }
}

/// Work-item 190 slice 2: may an instance-tagged CORPSE/GAMEOBJECT row be CREATE-relayed to this
/// viewer? Instance equality against a KNOWN viewer instance — `None` (viewer entity not resident,
/// see `viewer_instance`) relays nothing: suppress-then-sweep is strictly safer than the old
/// default-to-0 (which leaked instance-0 rows to a viewer whose real instance wasn't known yet).
/// Kept map-agnostic deliberately: instance-0 rows keep today's relay behavior byte-identical for
/// every resident open-world viewer, while a per-instance copy (or an in-instance corpse) reaches
/// ONLY viewers inside that instance. DELETE relays stay ungated (the `on_melee_delete` precedent —
/// SMSG_DESTROY_OBJECT for a never-created guid is a client no-op, and gating deletes risks
/// leaking a stale object on an instance transition). Pure — unit-tested below.
fn instance_relay_gate(row_instance_id: u64, viewer_instance_id: Option<u64>) -> bool {
    viewer_instance_id == Some(row_instance_id)
}

/// One corpse CREATE relay — the shared body of the `game_corpse` on_insert callback AND the
/// post-AOI resident sweep (190 slice 2 review MEDIUM): with `LYRACORE_AOI=1` the base sub's corpse
/// callbacks fire before the viewer's entity is resident, so the gate suppresses them (see
/// `viewer_instance`); once the AOI entity subscription has applied, the sweep re-offers every
/// resident corpse row through the same gate. Sends the CREATE plus, for the viewer's OWN corpse,
/// the reclaim-delay packet (work-item 201).
/// #109 diagnosis counters. Deliberately atomics read ONCE at logout rather than a log per call: a
/// `log::info!` on the relay path made the intermittent failure vanish across six consecutive runs,
/// so any instrument that touches the hot path perturbs the very race being measured.
///
/// `MOTION_CALLS` counts entries to [`relay_entity_motion`] (before any guard), `MOTION_SENT` counts
/// packets actually queued. Comparing them against the client's observed peer-moves localises a
/// failing run to one of three hops: subscription→callback (calls stay 0), guard rejection
/// (calls >> sent), or gateway→socket (sent >> observed).
pub(crate) static MOTION_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static MOTION_SENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The THIRD hop the two counters above could not see (work-item 287): queued → handed to the
/// writer. Both relays used to push with `let _ = tx.send(..)`, so a send to a session whose writer
/// half was already gone got discarded with no trace — a delivery path degrading silently, the exact
/// failure *shape* #109 and perf 2.3's orphaned writers had. Incremented by BOTH
/// [`relay_entity_motion`] and [`relay_creature_leg`]: they feed the same per-session writer channel,
/// so one counter answers "is the writer still taking peer-motion packets at all".
pub(crate) static MOTION_DROPPED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

// One argument per counter in the diagnostic line this formats.
#[allow(clippy::too_many_arguments)]
/// The `MOTIONSTAT` 10-second line (work-item 287) — pure, so what the operator reads is pinned by a
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
    completed: u64,
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
         | move submitted={submitted} completed={completed} outstanding={}",
        submitted.saturating_sub(completed)
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
/// under-delivery. THE HYSTERESIS IS A DELIBERATE DIVERGENCE FROM THE #109 SIGNAL, which has none
/// and re-fires every window: #109 keys on "movement submitted but the relay callback fired almost
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
/// A TOTAL loss of the relay trips this as well as the #109 signal (fan-out goes to ~0, which is
/// certainly below any baseline) — two lines about one failure, 30 s apart, which is the right way
/// round: #109's is instant and keeps repeating, this one adds "and it stayed that way".
pub(crate) fn fanout_health_step(
    state: FanoutHealth,
    fanout: Option<f64>,
    submitted_delta: u64,
) -> (FanoutHealth, Option<u32>) {
    // Uninformative window: too little movement to compare, or none at all. Keep the baseline (a
    // quiet window is not evidence about fan-out) but break any run in progress — the same activity
    // floor the #109 signal uses, for the same reason: a handful of movements with little relay
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
    let m = codec::build_corpse_create_object(&corpse_view(row.clone()));
    let _ = tx.send(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(m),
    )));
    if row.owner_guid == self_guid {
        let _ = tx.send(Outbound::One(
            ServerOpcodeMessage::SMSG_CORPSE_RECLAIM_DELAY(
                wow_world_messages::vanilla::SMSG_CORPSE_RECLAIM_DELAY {
                    delay: Duration::from_micros(row.reclaim_delay_micros as u64),
                },
            ),
        ));
    }
}

/// The pre-dedup visibility gate `offer_peer_create` runs before it ever touches the `created` set —
/// self-skip, instance isolation (work-item 190 slice 1), spirit-healer ghost-gating, and the
/// currently-stealthed check. Pure over ALREADY-READ state (the caller does the DB lookups and passes
/// their results in) so it is unit-testable without a live SDK connection — the three scenarios
/// work-items 144/145/190 care about (re-entry re-creates, login sees pre-existing peers, cross-instance
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
/// SAY proximity range, squared (vanilla ~25yd). See [`chat_in_range`].
const SAY_RANGE_SQ: f32 = 25.0 * 25.0; // 625.0 yd²
/// YELL proximity range, squared (vanilla ~300yd). Same rationale as `SAY_RANGE_SQ`.
const YELL_RANGE_SQ: f32 = 300.0 * 300.0; // 90_000.0 yd²

// ==================================================================================================
//  #468 stage 1 — the SHARED-dispatch relay bodies.
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
/// another shard's cache has no copy of the viewer's row to look them up in (the reason #73 needed a
/// second per-player connection at all).
///
/// On a `build_peer_create` encode failure the guid is ROLLED BACK out of `created`: leaving it in
/// permanently suppresses the peer (work-item 144's second latent bug).
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class #468's \
              differential test exists to prevent"]
pub(crate) fn offer_peer_create_for(
    coord: &Coordinator,
    viewer: &Viewer,
    row: &WorldEntity,
) -> Vec<Outbound> {
    const SPIRITHEALER_NPC_FLAG: u32 = lyracore_shared::constants::npc_flags::SPIRITHEALER; // 0x20
    if row.guid == viewer.self_guid || row.instance_id != viewer.instance_id {
        return Vec::new();
    }
    let viewer_is_ghost = viewer.gates.is_ghost();
    // Short-circuit BEFORE the aura scan: a spirit healer viewed by a living player is refused
    // regardless of stealth, and the pre-#468 code returned here without ever touching game_aura.
    if row.npc_flags & SPIRITHEALER_NPC_FLAG != 0 && !viewer_is_ghost {
        return Vec::new();
    }
    let guard = coord.0.coord();
    let db = &guard.conn.db;
    let row_is_stealthed = db
        .game_aura()
        .iter()
        .any(|a| a.target_guid == row.guid && a.eff_kind == A_STEALTH);
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
    let Some(m) = build_peer_create(coord, row) else {
        viewer.created.lock().unwrap().remove(&row.guid);
        return Vec::new();
    };
    let mut out = vec![Outbound::One(m)];
    // 023: this viewer's OWN summoned pet just appeared — bind it client-side (UNIT_FIELD_SUMMON +
    // the pet action bar).
    if row.owner_guid == viewer.self_guid {
        out.push(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
            Box::new(codec::build_owner_summon_values(viewer.self_guid, row.guid)),
        )));
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
    out
}

/// One entity row changed, for one viewer: re-entry-as-UPDATE first (work-item 144 — a peer that
/// left the box and returns arrives as an update of a still-cached row, and without this branch it
/// is permanently invisible until relog), otherwise the pure field diff, plus the viewer's own
/// spirit-healer ghost transition.
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class #468's \
              differential test exists to prevent"]
pub(crate) fn relay_entity_update(
    coord: &Coordinator,
    viewer: &Viewer,
    old: &WorldEntity,
    new: &WorldEntity,
) -> Vec<Outbound> {
    const SPIRITHEALER_NPC_FLAG: u32 = lyracore_shared::constants::npc_flags::SPIRITHEALER;
    const GHOST_PLAYER_FLAG: u32 = lyracore_shared::constants::player_flags::GHOST;
    if new.guid != viewer.self_guid {
        let shown = viewer.created.lock().unwrap().contains(&new.guid);
        if is_update_reentry(new.guid, viewer.self_guid, shown) {
            return offer_peer_create_for(coord, viewer, new);
        }
    }
    let mut out: Vec<Outbound> = entity_update_to_outbound(old, new)
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
            // Every spirit healer currently IN VIEW (the index answers that; pre-#468 this scanned
            // the AOI-scoped connection cache, which was the same set). Idempotent through
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
                    out.extend(offer_peer_create_for(coord, viewer, &h));
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

/// A peer left this viewer's view (its row was deleted, or it walked out of the box). DESTROY once,
/// and only if the viewer had actually been shown it.
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class #468's \
              differential test exists to prevent"]
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
    // 023: this viewer's own pet despawned — clear UNIT_FIELD_SUMMON and send the empty
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
              dropping the result is a silently invisible peer, which is the exact class #468's \
              differential test exists to prevent"]
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
    vec![Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
        Box::new(codec::build_gameobject_create_object(&go_view(
            row.clone(),
            &tmpl,
        ))),
    ))]
}

/// A gameobject leaving view. Ungated on purpose (the `on_melee_delete` precedent): DESTROY for a
/// guid the client never created is a client no-op, and gating it risks a stale prop.
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class #468's \
              differential test exists to prevent"]
pub(crate) fn relay_gameobject_destroy(_viewer: &Viewer, guid: u64) -> Vec<Outbound> {
    vec![Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
        codec::build_destroy_object(guid),
    ))]
}

/// One mover's motion row → `MSG_MOVE_*` bytes for one session. The `created` guard is the same one
/// the pre-#468 relay had: no MSG_MOVE for a guid this client never got a CREATE for.
#[must_use = "these relay bodies RETURN the packets to send — the caller enqueues them; \
              dropping the result is a silently invisible peer, which is the exact class #468's \
              differential test exists to prevent"]
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
              dropping the result is a silently invisible peer, which is the exact class #468's \
              differential test exists to prevent"]
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

/// The `on_update` dispatch decision (work-item 144): does this update need to be treated as a
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
pub(crate) fn entity_update_to_outbound(
    old: &WorldEntity,
    new: &WorldEntity,
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
            let m = codec::build_dynamic_flags_values(new.guid, new.dynamic_flags);
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
        // In-combat relay (UNIT_FIELD_FLAGS): the UNIT_FLAG_IN_COMBAT bit toggling as a unit enters/leaves
        // combat → observers see the combat indicator (incl. a pure caster). Any unit, not player-gated.
        if old.unit_flags != new.unit_flags {
            let m = codec::build_unit_flags_values(new.guid, new.unit_flags);
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
        // GM playtest `.speed` (work-item 223): a `run_speed_mult_bp` change relays
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

/// Login-replay suppression for the per-player relays that see their table's HISTORY replayed as
/// `on_insert` when the base subscription's initial update is applied (exploration's "Discovered"
/// toast, the rest-state PLAYER_BYTES_2 flip). Both must relay only what happens LIVE, after the
/// player is in the world.
///
/// **Why a flag and not just a frozen id set** (issue #41). The previous shape froze the row ids
/// present at login and skipped those — two ways to misfire, and the live run hit both:
///
///  1. **Re-minted ids.** `sweep_transfer_game_character_explored` sets `r.id = 0` so the
///     destination's `auto_inc` mints fresh ids, and every imported row read as a new discovery —
///     the reported symptom (one "Discovered: <area> — N experience gained" per explored area, per
///     shard crossing, quoting XP that was never granted). Hence [`Self::admit_once`] keys on
///     `area_bit`, which is what drives the fog word and is stable across a re-mint.
///  2. **The freeze RACES the replay.** The set was frozen on the calling thread after the
///     subscription ack, but the SDK applies the update to the client cache, fires `on_applied`
///     (which is what releases that thread) and THEN invokes the row callbacks — on its own
///     `run_threaded` pump. So the replay callbacks can run before the set is frozen, and keying
///     that set differently would not change it. The gate is therefore CLOSED until the freeze
///     completes: nothing the initial apply replays can toast, whatever order the two threads run
///     in, because a genuinely live discovery cannot happen before the player is in the world.
///
/// The cache is written before either callback runs, so a row whose replay lost the race is still
/// captured by the seed passed to [`Self::open`].
#[derive(Default)]
pub(crate) struct ReplayGate {
    /// False until the initial-apply snapshot has been taken. Everything before that is history.
    open: bool,
    /// Exploration only: `area_bit`s already announced (or present at login). A STABLE key —
    /// unlike the `auto_inc` row id, it survives the re-mint a cross-database transfer performs.
    announced: HashSet<i32>,
}

impl ReplayGate {
    /// Take the initial-apply snapshot and open the gate. `already` is the set of `area_bit`s the
    /// client cache holds for this character at that moment (empty for the keyless relays).
    fn open(&mut self, already: impl IntoIterator<Item = i32>) {
        self.announced.extend(already);
        self.open = true;
    }

    /// One arriving row on a keyless relay: relay it only once the gate is open.
    fn admit(&mut self) -> bool {
        self.open
    }

    /// One arriving exploration row: announce it only once the gate is open AND this `area_bit`
    /// has not been announced before (a re-minted duplicate of an area the character already
    /// explored is not a discovery).
    fn admit_once(&mut self, area_bit: i32) -> bool {
        self.open && self.announced.insert(area_bit)
    }
}

/// The "Discovered: &lt;area&gt;" toast for one arriving `game_character_explored` row, or `None`
/// when the row is a login/import REPLAY rather than a live discovery. Pure (bar the gate it
/// mutates) → unit-tested; the fog VALUES deliberately does NOT go through here, since it must fire
/// for every row so the map restores. See [`ReplayGate`] for what the gate is protecting against.
fn discovery_packet(gate: &Mutex<ReplayGate>, row: &CharacterExplored) -> Option<Outbound> {
    if !gate.lock().unwrap().admit_once(row.area_bit) {
        return None;
    }
    let (opcode, body) = codec::build_exploration_experience_raw(row.area_id, row.experience);
    Some(Outbound::Raw { opcode, body })
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

impl Coordinator {
    // The relay registration hands each callback its own `Coordinator` clone; the arity is the number of relays, not an accidental type.
    #[allow(clippy::type_complexity)]
    /// Subscribe the per-account connection to its per-player views and turn row deltas into
    /// outbound SMSG pushed onto `tx` (Phase 6/7). 51 `on_insert`/`on_update`/`on_delete`
    /// registrations across the table groups indexed below, most on `player.conn` (this
    /// session's own per-player connection) and a minority pinned to `self.0.coord()` (the
    /// COORDINATOR connection, immune to this connection's AOI-resubscription churn — the 279
    /// delivery law, used for anything a dropped callback would make worse than a late packet:
    /// XP, level-up, quest log, item instances, teleport, the addon bridge, reputation, and the
    /// realm-core group/whisper/explored-fog twins). Each group carries a `// ====` banner at
    /// its registration site, top to bottom in this same order:
    ///
    ///   - `game_world_entity` — peer CREATE/DESTROY/VALUES + spirit-healer ghost-reveal
    ///   - `game_entity_motion` — peer movement
    ///   - `game_creature_spline` — creature legs
    ///   - `game_combat_event` — melee/ranged swing log
    ///   - `game_melee_attack` — combat stance (ATTACKSTART/STOP)
    ///   - `game_player_skill` — live skill pane
    ///   - `game_character_explored` — exploration fog (per-player leg, then its coordinator twin)
    ///   - `game_rest_state_event` — rest state (zzz / blue XP bar)
    ///   - `game_xp_event` / `game_levelup_event` — XP + level-up (coordinator)
    ///   - `game_dynamic_object` — ground-area spell visuals
    ///   - `game_corpse` — player corpses
    ///   - `game_resurrect_request` — resurrect prompt
    ///   - `game_gameobject` — gameobjects, incl. door/chest state toggles
    ///   - `game_spell_cast_event` — spell cast visuals
    ///   - `game_spell_impact_event` — deferred projectile impact damage
    ///   - `game_chat_event` / `game_channel_event` / `game_emote_event` — chat, channels, emotes
    ///   - `game_whisper_event` / `game_group_event` — whisper + party (each per-player leg, then
    ///     its realm-core twin); `game_group_event` also carries loot-roll (187) and quest-share
    ///     (194), reused by reserved kind range rather than a table of their own
    ///   - `game_roll_event` — `/roll` broadcast
    ///   - `game_aura` — buff/debuff array sync, self armor, stealth peer-visibility
    ///   - `game_character_quest` — quest log (coordinator)
    ///   - `game_item_instance` — bag sync + gain feedback + armor/durability (coordinator)
    ///   - `game_teleport_event` — teleport ack / cross-map transfer (coordinator)
    ///   - `game_addon_message` — addon bridge (coordinator)
    ///   - `game_player_reputation` — reputation standing (coordinator)
    ///
    /// `game_movement_event` and `game_creature_move_event`, the two tables the doc comment here
    /// used to name, are GONE (perf catalog 2.1) — replaced by the `game_entity_motion` /
    /// `game_creature_spline` box subscriptions above; nothing writes either any more (#350).
    ///
    /// TEARDOWN CONTRACT: every registration above needs a matching `remove_on_*` in the
    /// `teardowns` vec near the end of this function — up to ~2,800 lines away, the distance
    /// issue #353 flagged. That vec's own comment repeats the rule ("Adding a relay = register it
    /// + push its teardown here") and `every_registered_player_callback_has_a_teardown_issue_89`
    /// (this file's test module) checks it by scanning this file's own source text: a missed
    /// teardown relays a logged-out player's state — private whispers, in the failure mode that
    /// vec's comment spells out — into a dead channel for the gateway's whole lifetime.
    ///
    /// PLANNED CARVE (#353, not done here — this function is still one ~3,000-line body; this
    /// doc comment plus the inline `// ====` banners are the alpha-scope deliverable, a map for
    /// the reader, not the refactor): extract one `fn register_<table>_relays(...) ->
    /// Vec<Box<dyn FnOnce(&PlayerConn) + Send>>` per group above, each returning its OWN
    /// teardowns so registration and teardown are co-located instead of mirrored ~2,800 lines
    /// apart — this file's own `offer_peer_create_for` family (#468) proves the pattern. Mechanical,
    /// no behavior change; land it one table-group
    /// at a time so each PR stays reviewable, and re-run
    /// `every_registered_player_callback_has_a_teardown_issue_89` plus the `scanned_source`-based
    /// pins after every step — they scan this file's own text, so a carve that moves code
    /// without updating a pinned literal string fails loudly instead of silently.
    ///
    /// The returned guard removes the callbacks + unsubscribes on drop (the connection is reused).
    pub fn subscribe_player_events(
        &self,
        account_id: u64,
        self_guid: u64,
        login_instance: u64,
        login_map: u32,
        login_x: f32,
        login_y: f32,
        tx: SessionTx,
    ) -> Result<PlayerSubscriptions> {
        // Ghost-only visibility constants (vanilla 1.12): a spirit-healer creature carries
        // UNIT_NPC_FLAGS SPIRITHEALER and is visible ONLY to a viewer in the GHOST player-flag state.
        // BUG FIX: was 0x4000 — but 0x4000 is REPAIR in cmangos 1.12 numbering (SPIRITHEALER is 0x20).
        // The wrong constant (a) failed to ghost-gate the real Spirit Healer (npc_flags 0x21) and
        // (b) wrongly ghost-gated the 39 armorers/quartermasters that carry REPAIR (0x4000), hiding
        // them from living players.
        let player = self.player_conn(account_id)?;
        let created = Arc::new(Mutex::new(HashSet::from([self_guid])));
        // #73 view-merge / #207 fast-follow 2: the one piece of viewer-owned state the away leg's
        // peer-visibility gate needs but cannot read off an away connection. Written by the ghost-flag
        // relay (this viewer's OWN row), read by every per-viewer relay body in this file — a
        // shared coordinator callback holds the cache of the shard that owns the ROW, which for a
        // cross-shard peer is not the shard that owns the VIEWER. See `ViewerGates`'s doc comment.
        let viewer_gates = Arc::new(ViewerGates::default());
        // #468: this session's handle on the gateway-wide shared view. Minted HERE, at the top,
        // because relays registered further down capture it — the actual REGISTRATION (which makes
        // the shared dispatch start enqueueing for this session) happens near the end, once
        // everything that could still fail with `?` has succeeded.
        let view = self.world_view();
        let session = view.next_session_id();
        let stealth_session = session;

        // ======================================================================================
        //  MELEE/RANGED SWING LOG — game_combat_event (insert)
        //  SMSG_ATTACKERSTATEUPDATE for a melee swing, or SMSG_SPELL_GO +
        //  SMSG_SPELLNONMELEEDAMAGELOG for a ranged Auto Shot/Wand shot; SMSG_ATTACKSTOP on a melee
        //  killing blow.
        // ======================================================================================
        // Melee swing log (broadcast; no RLS) → SMSG_ATTACKERSTATEUPDATE (swing animation + damage
        // text). The victim's health bar is moved separately by the on_update VALUES relay above.
        let cb_tx = tx.clone();
        let cb_created = created.clone();
        let on_combat = player
            .conn
            .db
            .game_combat_event()
            .on_insert(move |_ctx, row| {
                // With AOI on, skip a swing whose ATTACKER is out of the player's scope (not created) — no
                // point animating an invisible attacker's swing (the victim's health still moves via the
                // entity on_update VALUES relay if the victim is in scope). Off → `created` holds all → no-op.
                if !cb_created.lock().unwrap().contains(&row.attacker_guid) {
                    return;
                }
                // #10/097 vanilla shot shape: a RANGED shot is a SPELL on the wire — SMSG_SPELL_GO (a HIT
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
                    let _ = cb_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(
                        Box::new(go),
                    )));
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
                        let msg = Outbound::One(ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(
                            Box::new(log),
                        ));
                        // (097) The shot's damage lands at fire + travel (module ranged_impact applies
                        // the health there) — hold the LOG to the same moment so the number arrives
                        // WITH the arrow, not at the muzzle. Deliberate simplification: a thread per
                        // landed shot (~1 per 2.3s per shooter); move to a shared timer wheel if
                        // archer armies happen.
                        if row.impact_delay_ms > 0 {
                            let tx_late = cb_tx.clone();
                            let delay =
                                std::time::Duration::from_millis(row.impact_delay_ms as u64);
                            std::thread::spawn(move || {
                                std::thread::sleep(delay);
                                let _ = tx_late.send(msg);
                            });
                        } else {
                            let _ = cb_tx.send(msg);
                        }
                    }
                } else if !row.spell_swing {
                    // 114: a fired on-next-swing spell (Heroic Strike/Cleave) REPLACES the white hit — the
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
                    let _ = cb_tx.send(Outbound::One(
                        ServerOpcodeMessage::SMSG_ATTACKERSTATEUPDATE(Box::new(m)),
                    ));
                }
                // C2: on a MELEE killing blow, tell the attacker to leave combat stance. The target itself
                // vanishes via the game_world_entity on_delete → SMSG_DESTROY_OBJECT relay. A RANGED kill
                // sends no ATTACKSTOP (the client was never in melee-attack state; vanilla sends none) —
                // its stop signal is the SMSG_CANCEL_AUTO_REPEAT from the engagement-row delete relay
                // below (kill_creature's disengage frees the attacker's row).
                if row.killing_blow && row.ranged_spell_id == 0 {
                    let stop = codec::build_attack_stop(row.attacker_guid, row.target_guid);
                    let _ = cb_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(
                        Box::new(stop),
                    )));
                }
            });

        // ======================================================================================
        //  COMBAT STANCE — game_melee_attack (insert/delete)
        //  SMSG_ATTACKSTART on engagement; SMSG_ATTACKSTOP (or SMSG_CANCEL_AUTO_REPEAT for a ranged
        //  auto-repeat) on disengage.
        // ======================================================================================
        // Combat STANCE relay (broadcast; no RLS): a melee/ranged engagement ENGAGE (game_melee_attack
        // insert) → SMSG_ATTACKSTART so observers see the attacker draw weapon + hold the combat stance
        // between swings; the row's removal (disengage: kill / flee / evade / logout) → SMSG_ATTACKSTOP
        // (so a non-kill disengage leaves stance too — not just the killing blow above). Covers
        // AUTO-ATTACK combat; a pure-caster engagement has no melee row, so UNIT_FLAG_IN_COMBAT for that
        // case is a follow-up. AOI-guard the start like the swing relay (skip an out-of-scope attacker).
        let atk_tx = tx.clone();
        let atk_created = created.clone();
        let on_melee_insert = player
            .conn
            .db
            .game_melee_attack()
            .on_insert(move |_ctx, row| {
                // A RANGED auto-repeat (Auto Shot / wand Shoot, ranged_spell_id != 0) is NOT melee combat:
                // SMSG_ATTACKSTART would put the client in MELEE stance and animate a melee swing between
                // shots (user: "we swap to melee in between ranged"). The shot is shown by SPELL_START/GO
                // relay above — skip the stance packet. Melee rows (incl. the creature's retaliation) relay
                // ATTACKSTART as before. [097]
                if row.ranged_spell_id != 0 {
                    return;
                }
                if !atk_created.lock().unwrap().contains(&row.attacker_guid) {
                    return;
                }
                let m = codec::build_attack_start(row.attacker_guid, row.target_guid);
                let _ = atk_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTART(
                    Box::new(m),
                )));
            });
        let atk_stop_tx = tx.clone();
        let on_melee_delete = player
            .conn
            .db
            .game_melee_attack()
            .on_delete(move |_ctx, row| {
                // A RANGED row's delete is the ONE server-initiated auto-repeat teardown choke point
                // (vanilla likewise announces the cancel from the server side): whatever killed the
                // loop — the target died, leash evade, out of ammo, a per-shot hard fail (range/LoS/
                // facing/too-close), the client's own cancel — the OWNING player gets the 0-byte
                // SMSG_CANCEL_AUTO_REPEAT so its toggle drops in lockstep. Without it the client stayed
                // toggled ON over a dead loop and the next press cancelled instead of casting (097).
                // No ATTACKSTOP for ranged (we never sent its ATTACKSTART). Other viewers: no packet.
                if row.ranged_spell_id != 0 {
                    if row.attacker_guid == self_guid {
                        let _ = atk_stop_tx
                            .send(Outbound::One(ServerOpcodeMessage::SMSG_CANCEL_AUTO_REPEAT));
                    }
                    return;
                }
                let m = codec::build_attack_stop(row.attacker_guid, row.target_guid);
                let _ = atk_stop_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_ATTACKSTOP(
                    Box::new(m),
                )));
            });

        // ======================================================================================
        //  SKILL-UP — game_player_skill (insert/update)
        //  Live PLAYER_SKILL_INFO[slot] VALUES so a newly-learned or trained skill line updates the
        //  open pane without a relog.
        // ======================================================================================
        // LIVE skill pane relay (234): a game_player_skill INSERT (new line learned) or UPDATE
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
        let skill_tx = tx.clone();
        let skill_slots_ins = skill_slots.clone();
        let on_skill_insert = player
            .conn
            .db
            .game_player_skill()
            .on_insert(move |_ctx, row| {
                if row.character_guid != self_guid {
                    return; // RLS already scopes to self; belt+braces
                }
                let mut guard = skill_slots_ins.lock().unwrap();
                let (map, next_free) = &mut *guard;
                let slot = *map.entry(row.skill_line).or_insert_with(|| {
                    let s = *next_free;
                    *next_free += 1;
                    s
                });
                if let Ok(sk) = wow_world_messages::vanilla::Skill::try_from(row.skill_line) {
                    if let Some(m) =
                        codec::build_skill_values(self_guid, slot, sk, row.current, row.max_rank)
                    {
                        let _ = skill_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)),
                        ));
                    }
                }
            });
        let skill_upd_tx = tx.clone();
        let skill_slots_upd = skill_slots.clone();
        let on_skill_update =
            player
                .conn
                .db
                .game_player_skill()
                .on_update(move |_ctx, _old, row| {
                    if row.character_guid != self_guid {
                        return;
                    }
                    let mut guard = skill_slots_upd.lock().unwrap();
                    let (map, next_free) = &mut *guard;
                    let slot = *map.entry(row.skill_line).or_insert_with(|| {
                        let s = *next_free;
                        *next_free += 1;
                        s
                    });
                    if let Ok(sk) = wow_world_messages::vanilla::Skill::try_from(row.skill_line) {
                        if let Some(m) = codec::build_skill_values(
                            self_guid,
                            slot,
                            sk,
                            row.current,
                            row.max_rank,
                        ) {
                            let _ = skill_upd_tx.send(Outbound::One(
                                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(m)),
                            ));
                        }
                    }
                });

        // ======================================================================================
        //  EXPLORATION FOG, per-player leg — game_character_explored (insert)
        //  PLAYER_EXPLORED_ZONES fog-word VALUES on every row, including the login initial-sync
        //  replay, plus the one-shot "Discovered: <area>" popup on a genuinely fresh bit. See the
        //  coordinator twin below for the delivery-law guarantee.
        // ======================================================================================
        // Explored areas (200) → the PLAYER_EXPLORED_ZONES map-fog word. Fires on a live discovery AND
        // on the login initial-sync of the char's stored areas — idempotent VALUES, so login re-sends
        // every word and restores the fog (exactly like the skill-VALUES relay above). A partial VALUES
        // overwrites the whole word, so recompute the FULL word (OR of every explored bit of THIS char
        // in the same 32-bucket) rather than the single new bit, or co-word areas get clobbered.
        // Exploration (200): the login/import replay must not toast a "Discovered" popup (the fog
        // VALUES still fires for all). See `ReplayGate` — it is CLOSED until the snapshot below is
        // taken, and keys on `area_bit` so a transfer's re-minted row ids cannot read as discoveries
        // (issue #41).
        let explored_replay: Arc<Mutex<ReplayGate>> = Arc::new(Mutex::new(ReplayGate::default()));
        let explored_tx = tx.clone();
        let explored_gate = explored_replay.clone();
        let on_explored_insert =
            player
                .conn
                .db
                .game_character_explored()
                .on_insert(move |ctx, row| {
                    if row.character_guid != self_guid || row.area_bit < 0 {
                        return; // no RLS on this table → filter to self; skip a defensive negative bit
                    }
                    // Fog VALUES — for EVERY row (incl. the login initial-sync), so the map fog restores.
                    let word_idx = (row.area_bit / 32) as u16;
                    let lo = word_idx as i32 * 32;
                    let mut word: u32 = 0;
                    for r in ctx.db.game_character_explored().iter() {
                        if r.character_guid == self_guid && r.area_bit >= lo && r.area_bit < lo + 32
                        {
                            word |= 1u32 << (r.area_bit - lo);
                        }
                    }
                    let (opcode, body) =
                        codec::build_explored_zones_values(self_guid, word_idx, word);
                    let _ = explored_tx.send(Outbound::Raw { opcode, body });
                    // "Discovered: <area>" text popup — a FRESH discovery only. Skips the login initial-sync
                    // replay AND the rows a cross-database transfer re-inserts (else every already-explored
                    // area toasts a Discovered line, quoting XP nobody granted — issue #41).
                    if let Some(out) = discovery_packet(&explored_gate, row) {
                        let _ = explored_tx.send(out);
                    }
                });

        // ======================================================================================
        //  REST STATE — game_rest_state_event (insert)
        //  PLAYER_BYTES_2 (zzz icon + blue XP bar) on a live inn crossing; the login rest state
        //  itself rides the CREATE byte, not this relay.
        // ======================================================================================
        // Rest state (196): a LIVE inn crossing relays PLAYER_BYTES_2 (zzz icon + blue XP bar). The login
        // state rides the CREATE byte (spawn bakes it from Character.resting), so the historical events
        // present at login are skipped — the same `ReplayGate` as the Discovered popup above.
        //
        // #41 AUDIT — the re-minted-id assumption: this relay is CLEAR of it. `game_rest_state_event`
        // is on `transfer::NOT_TRANSPORTED` (a one-shot relay row with a GC TTL; the durable rest
        // state travels on the character row), so no transfer ever re-inserts one here and no id is
        // ever re-minted. It DID share the other half of #41 — the frozen id set racing the replay
        // callbacks, which could relay a HISTORICAL event's `player_bytes_2` over the correct login
        // byte — so it moves onto the same gate, which needs no key at all: nothing before the gate
        // opens is a live inn crossing.
        let rest_replay: Arc<Mutex<ReplayGate>> = Arc::new(Mutex::new(ReplayGate::default()));
        let rest_tx = tx.clone();
        let rest_gate = rest_replay.clone();
        let on_rest_insert = player
            .conn
            .db
            .game_rest_state_event()
            .on_insert(move |_ctx, row| {
                if row.character_guid != self_guid || !rest_gate.lock().unwrap().admit() {
                    return; // filter to self; skip the login initial-sync replay
                }
                let (opcode, body) = codec::build_rest_state_values(self_guid, row.player_bytes_2);
                let _ = rest_tx.send(Outbound::Raw { opcode, body });
            });

        // ======================================================================================
        //  XP + LEVEL-UP, coordinator-registered — game_xp_event / game_levelup_event (insert)
        //  SMSG_LOG_XPGAIN / SMSG_LEVELUP_INFO. Moved off the per-player connection by 279 (the 277
        //  loss class): both ride kill transactions concurrent with AOI churn on that connection.
        // ======================================================================================
        // XP gain (slice 1) → SMSG_LOG_XPGAIN; level-up → SMSG_LEVELUP_INFO. Both event tables are
        // COORDINATOR-registered since 279 (the 277 loss class): xp/levelup events ride KILL
        // transactions, which can be large and concurrent with movement (AOI churn on the
        // per-player conn) — a lost SMSG_LEVELUP_INFO was observed as the levelup_info suite
        // flake. The coordinator bypasses the recipient RLS, so the closure now self-filters
        // on recipient_identity (the session's bound player identity).
        let self_identity = player.identity;
        let xp_tx = tx.clone();
        let on_xp = self
            .0
            .coord()
            .conn
            .db
            .game_xp_event()
            .on_insert(move |_ctx, row| {
                if row.recipient_identity != self_identity {
                    return;
                }
                // #72: belt-and-suspenders — nothing about a warm handoff should produce a kill/XP event
                // mid-swap, but this closure is self-keyed like every other relay this flag covers.
                let m = codec::build_log_xpgain(row.killed_guid, row.total_exp, row.is_kill);
                let _ = xp_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_LOG_XPGAIN(
                    Box::new(m),
                )));
            });
        let lvl_tx = tx.clone();
        let on_levelup = self
            .0
            .coord()
            .conn
            .db
            .game_levelup_event()
            .on_insert(move |_ctx, row| {
                if row.recipient_identity != self_identity {
                    return;
                }
                let m = codec::build_levelup_info(
                    row.new_level,
                    row.health_gained,
                    row.mana_gained,
                    row.strength_gained,
                    row.agility_gained,
                    row.stamina_gained,
                    row.intellect_gained,
                    row.spirit_gained,
                );
                let _ = lvl_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_LEVELUP_INFO(
                    Box::new(m),
                )));
            });

        // ======================================================================================
        //  EXPLORATION FOG, coordinator twin — game_character_explored (insert)
        //  The 279 delivery-law guarantee for the per-player leg above: re-sends the same
        //  idempotent fog VALUES from a connection that never churns, so a fresh-login discovery
        //  can't be dropped by AOI resubscription.
        // ======================================================================================
        // Fog-word GUARANTEE for live discoveries (279 law, live find 2026-07-19): the fresh-login
        // first-movement discovery is exactly the AOI-churn window where the per-player callback
        // drops — a new character discovered Northshire server-side and the map never cleared. The
        // coordinator re-sends the same idempotent full-word VALUES for every fresh insert (double
        // fog with the per-player relay is harmless); the "Discovered" popup deliberately stays on
        // the per-player path only, so it never toasts twice. Login restore is untouched (the
        // per-player initial-sync below owns it — the coordinator's long-lived sub sees no insert
        // at a player's login).
        let explored_coord_tx = tx.clone();
        let on_explored_coord =
            self.0
                .coord()
                .conn
                .db
                .game_character_explored()
                .on_insert(move |ctx, row| {
                    if row.character_guid != self_guid || row.area_bit < 0 {
                        return;
                    }
                    let word_idx = (row.area_bit / 32) as u16;
                    let lo = word_idx as i32 * 32;
                    let mut word: u32 = 0;
                    for r in ctx.db.game_character_explored().iter() {
                        if r.character_guid == self_guid && r.area_bit >= lo && r.area_bit < lo + 32
                        {
                            word |= 1u32 << (r.area_bit - lo);
                        }
                    }
                    let (opcode, body) =
                        codec::build_explored_zones_values(self_guid, word_idx, word);
                    let _ = explored_coord_tx.send(Outbound::Raw { opcode, body });
                });

        // ======================================================================================
        //  DYNAMIC OBJECTS — game_dynamic_object (insert/delete)
        //  Ground-area spell visuals (Consecration's swirl etc.) → CREATE on insert, DESTROY on the
        //  area's reap.
        // ======================================================================================
        // Ground-area DYNAMICOBJECTs (118, Consecration's swirl; broadcast, no RLS) → CREATE on
        // insert, DESTROY on the area's reap. The 5875 client renders the ground effect from
        // DYNAMICOBJECT_SPELLID's SpellVisual — the cast packets alone draw nothing (live find).
        // Instance-gated like corpses; the 0xF100… guid space never collides. Short-lived (≤ the
        // area's duration), so no login/AOI resident sweep — a mid-area login misses the visual.
        let dynobj_ins_tx = tx.clone();
        let on_dynobj_insert = player
            .conn
            .db
            .game_dynamic_object()
            .on_insert(move |_ctx, row| {
                if !instance_relay_gate(row.instance_id, Some(login_instance)) {
                    return;
                }
                let m = codec::build_dynamicobject_create_object(
                    row.guid,
                    row.caster_guid,
                    row.spell_id,
                    row.x,
                    row.y,
                    row.z,
                    row.radius_yd,
                );
                let _ = dynobj_ins_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                    Box::new(m),
                )));
            });
        let dynobj_del_tx = tx.clone();
        let on_dynobj_delete = player
            .conn
            .db
            .game_dynamic_object()
            .on_delete(move |_ctx, row| {
                let _ = dynobj_del_tx.send(Outbound::One(
                    ServerOpcodeMessage::SMSG_DESTROY_OBJECT(codec::build_destroy_object(row.guid)),
                ));
            });

        // ======================================================================================
        //  CORPSES — game_corpse (insert/delete/update)
        //  Player corpse CREATE_OBJECT on death, DESTROY on reclaim/decay, re-CREATE on the
        //  body→bones state change.
        // ======================================================================================
        // Player corpses (slice 5; broadcast, no RLS) → CORPSE CREATE_OBJECT on insert (a body left at
        // a death location), SMSG_DESTROY_OBJECT on delete (reclaim/decay). The corpse guid (0xF101…)
        // never collides with a player/creature guid, so no self-skip/dedup is needed.
        let corpse_ins_tx = tx.clone();
        let on_corpse_insert = player.conn.db.game_corpse().on_insert(move |_ctx, row| {
            // 190 slice 2: corpse rows are instance-tagged — CREATE only for same-instance viewers
            // (instance-0 corpses relay exactly as before; a Deadmines corpse stays inside its run).
            if !instance_relay_gate(row.instance_id, Some(login_instance)) {
                return;
            }
            // Shared body with the post-AOI resident sweep — see `relay_corpse_create` (the
            // owner's reclaim-delay packet, work-item 201, rides inside it).
            relay_corpse_create(&corpse_ins_tx, self_guid, row);
        });
        let corpse_del_tx = tx.clone();
        let on_corpse_delete = player.conn.db.game_corpse().on_delete(move |_ctx, row| {
            let _ = corpse_del_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                codec::build_destroy_object(row.guid),
            )));
        });
        // Corpse state changes (work-item 201: body → bones decay, `gc.rs`'s reaper) → re-emit the
        // CREATE_OBJECT so a viewer's client re-renders it with the current bones flag. Mirrors the
        // insert relay above (same builder); UNVERIFIED-until-observed whether the 5875 client actually
        // re-renders a CORPSE object on a repeat CREATE for the same guid rather than no-op'ing it.
        let corpse_upd_tx = tx.clone();
        let on_corpse_update = player
            .conn
            .db
            .game_corpse()
            .on_update(move |_ctx, _old, row| {
                // 190 slice 2: same instance gate as the insert relay (the body→bones re-emit must not
                // leak a cross-instance corpse either).
                if !instance_relay_gate(row.instance_id, Some(login_instance)) {
                    return;
                }
                let m = codec::build_corpse_create_object(&corpse_view(row.clone()));
                let _ = corpse_upd_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                    Box::new(m),
                )));
            });

        // ======================================================================================
        //  RESURRECT PROMPT — game_resurrect_request (insert)
        //  SMSG_RESURRECT_REQUEST for an offer addressed to this player; no delete-side relay (the
        //  client's own accept/decline resolves it).
        // ======================================================================================
        // Resurrection accept-prompt (#014; RLS-scoped — this subscription only sees the offer addressed
        // to THIS player, same shape as `game_whisper_event`) → SMSG_RESURRECT_REQUEST. The row is deleted
        // by `resurrect_response` (accept or decline) or replaced by a fresh offer; no delete-side relay is
        // needed (the 5875 client's resurrect prompt has no server-driven dismiss opcode — it times out or
        // is dismissed by the player's own accept/decline, which this same CMSG round-trip already resolves).
        let rez_tx = tx.clone();
        let on_resurrect_insert =
            player
                .conn
                .db
                .game_resurrect_request()
                .on_insert(move |_ctx, row| {
                    let m =
                        codec::build_resurrect_request(row.caster_guid, row.caster_name.clone());
                    let _ = rez_tx.send(Outbound::One(
                        ServerOpcodeMessage::SMSG_RESURRECT_REQUEST(Box::new(m)),
                    ));
                });

        // ======================================================================================
        //  SPELL CAST VISUALS — game_spell_cast_event (insert)
        //  SMSG_SPELL_START/GO/interrupt/cooldown sequence for a cast; the deferred impact damage
        //  number is a SEPARATE table, below.
        // ======================================================================================
        // Aura+spell tracer (broadcast, no RLS — buffs are visible unit state). A cast emits the
        // visual (SMSG_SPELL_GO) on every cast; an aura insert/refresh writes the slot-0 buff icon via
        // a partial UNIT_FIELD_AURA VALUES update, and its delete (expiry) clears the slot (zeros).
        let cast_tx = tx.clone();
        let on_cast = player
            .conn
            .db
            .game_spell_cast_event()
            .on_insert(move |_ctx, row| {
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
                    // 171: a CREATURE caster (0xF130 high-guid) has no self — broadcast so every observer's
                    // mob cast bar tears down on a Kick/Counterspell (the packet carries the mob's guid, so
                    // a bystander's own bar is untouched; mirrors the START broadcast that drew the bar).
                    let is_creature = row.caster_guid >> 48 == 0xF130;
                    if is_creature || row.caster_guid == self_guid {
                        let m = codec::build_spell_failure(row.caster_guid, row.spell_id);
                        let _ = cast_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_SPELL_FAILURE(Box::new(m)),
                        ));
                    }
                    return;
                }
                // PUSHBACK signal (work-item 039): a direct hit slid the caster's in-progress timed cast's
                // fire time. Broadcast (NOT self-only) — SMSG_SPELL_DELAYED is a caster-visible cast-bar
                // shift, like SMSG_SPELL_START/GO below, so anyone watching the caster's cast bar sees it
                // slide (unlike SMSG_SPELL_FAILURE above, which is a private cast-bar-teardown message). This
                // row carries ONLY caster/spell/delay_ms, so it must be handled before the START/GO/COOLDOWN
                // sequence below (it has cast_time_ms 0 and is_completion false, so it does NOT take either
                // of those branches, but returning explicitly keeps this a single-purpose row like the
                // is_interrupted branch above it).
                if row.delay_ms > 0 {
                    let m = codec::build_spell_delayed(row.caster_guid, row.delay_ms);
                    let _ = cast_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_DELAYED(
                        Box::new(m),
                    )));
                    return;
                }
                // PROC-LOG row (114): a swing-proc damage line (Seal of Righteousness holy riding a landed
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
                        let _ = cast_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)),
                        ));
                    }
                    return;
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
                    let _ = cast_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
                        Box::new(start),
                    )));
                    return;
                }
                // [083] Cast-GO (cast_time_ms == 0). Mangos-faithful sequence:
                //   - GENUINE INSTANT (is_completion=false): START(0)+GO — SendSpellStart fires for EVERY
                //     non-triggered cast (timer 0 for an instant) to register the pending cast, then cast() →
                //     SendSpellGo finalizes it. START flags = 0x02, GO flags = 0x0100 (set in the builders).
                //   - TIMED COMPLETION (is_completion=true): GO ALONE — the begin already sent START(cast_time);
                //     a 2nd START(0) reset the bar to zero-length ("stuck on full"). The SMSG_SPELL_GO is the
                //     client's cast finalizer (it matches by caster guid + spell id to release the pending cast).
                // [083] The CASTER's OWN instant cast (cast_time 0 → this GO branch, !is_completion) already got
                // START+GO SYNCHRONOUSLY from the CMSG_CAST_SPELL handler (so they precede the aura effects the
                // SDK's alphabetical callback order would otherwise send first). Skip the duplicate to the caster
                // — but still relay it to OBSERVERS (caster != self) and for TIMED COMPLETIONS (is_completion,
                // which the gateway did NOT sync-send). NOTE: a player's TRIGGERED instant cast (channel-tick
                // missile / on-hit trigger) also matches !is_completion && caster==self and is suppressed here —
                // it did not get a synchronous send, so it loses its caster-side visual until the client_initiated
                // flag lands (tracked follow-up). Acceptable for the slice; observers still see it.
                // 088: suppress ONLY what the CMSG handler actually sent synchronously — the row says so
                // (client_initiated rides from the cast_spell reducer path alone). The old shape
                // (!is_completion && self) also swallowed the caster's TRIGGERED instants — channel-tick
                // missiles (Arcane Missiles/Drain Life), on-hit trigger_spell procs — which never had a
                // synchronous send, so their caster saw no visual while observers did.
                let caster_synced_instant = row.client_initiated && row.caster_guid == self_guid;
                if !caster_synced_instant {
                    if !row.is_completion {
                        let start =
                            codec::build_spell_start(row.caster_guid, row.spell_id, 0, 0, None);
                        let _ = cast_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_START(
                            Box::new(start),
                        )));
                    }
                    // CAST_RESULT(OK) is caster-only: clears the pending spell state so the subsequent
                    // GO can release m_currentSpells. Only for timed completions — the instant-cast
                    // caster got it synchronously from the CMSG_CAST_SPELL handler. [083]
                    if row.is_completion && row.caster_guid == self_guid {
                        let _ = cast_tx.send(Outbound::Raw {
                            opcode: 0x0130,
                            body: codec::build_cast_result_ok(row.spell_id),
                        });
                    }
                    let mut go =
                        codec::build_spell_go(row.caster_guid, row.spell_id, row.target_guid, None);
                    // 114: a 0-damage on-next-swing FIRE that rode a missed/dodged/parried swing reports
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
                    let _ = cast_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_GO(
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
                    let _ = cast_tx.send(Outbound::One(
                        ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)),
                    ));
                }
                // 251: the green floating heal number + combat-log line — SMSG_SPELLHEALLOG whenever
                // this cast restored health (module sums EFFECTIVE heal onto the row; overheal-only
                // casts carry 0 and stay silent, matching the damage gate's shape).
                if row.healed > 0 {
                    use wow_world_messages::vanilla::SMSG_SPELLHEALLOG;
                    use wow_world_messages::Guid;
                    let _ = cast_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELLHEALLOG(
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
                    let _ = cast_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_SPELL_COOLDOWN(
                        Box::new(cd),
                    )));
                }
            });
        // ======================================================================================
        //  SPELL IMPACT DAMAGE — game_spell_impact_event (insert)
        //  The floating damage number for a projectile that has finished its travel time (#084);
        //  never re-sends START/GO.
        // ======================================================================================
        // Deferred PROJECTILE-IMPACT damage log (#084 — Shadow Bolt lands on impact, not at cast
        // resolution). A SEPARATE table + listener from `on_cast` above (never touches it): the module's
        // `fire_spell_impact` (module/src/spell/scheduler.rs) inserts a `game_spell_impact_event` row when
        // a projectile's missile travel time elapses, carrying ONLY the already-resolved damage figures.
        // `SMSG_SPELL_GO` (the trajectory) already fired synchronously at cast time via `on_cast` /
        // the CMSG_CAST_SPELL handler (world/mod.rs) — this relay sends NOTHING but the floating damage
        // number, so it can never replay a duplicate START/GO for a cast that already visually resolved.
        let impact_tx = tx.clone();
        let on_impact = player
            .conn
            .db
            .game_spell_impact_event()
            .on_insert(move |_ctx, row| {
                if row.damage == 0 {
                    return; // a fully-absorbed impact (or a target that vanished before landing) logs nothing
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
                let _ = impact_tx.send(Outbound::One(
                    ServerOpcodeMessage::SMSG_SPELLNONMELEEDAMAGELOG(Box::new(log)),
                ));
            });
        // ======================================================================================
        //  CHAT — game_chat_event (insert)
        //  SMSG_MESSAGECHAT for a range-gated Say/Yell.
        // ======================================================================================
        // Say/yell broadcast (social tier; public, no RLS) → SMSG_MESSAGECHAT. Range-gated:
        // SAY ~25 yd, YELL ~300 yd (vanilla proximity values). The speaker always hears their own
        // line (self_guid == sender_guid short-circuits the distance check, matching vanilla).
        // Both endpoints are looked up from game_world_entity; if either is missing (edge-case
        // during login / logout) the message is dropped — safer than flooding all clients.
        let chat_tx = tx.clone();
        let chat_coord = self.clone(); // global db handle — bypasses AOI-scoped ctx.db for YELL range lookups
        let on_chat = player
            .conn
            .db
            .game_chat_event()
            .on_insert(move |_ctx, row| {
                // Speaker always hears their own message (vanilla behaviour).
                if row.sender_guid != self_guid {
                    let range_sq = if row.chat_type == CHAT_YELL {
                        YELL_RANGE_SQ
                    } else {
                        SAY_RANGE_SQ // SAY and any future proximity type default to SAY range
                    };
                    // Look up positions through the coordinator's global db (not ctx.db, which is
                    // AOI-scoped when LYRACORE_AOI is enabled). Under AOI the per-player
                    // game_world_entity subscription only covers a ~250yd span; a YELL speaker at
                    // 100–300yd would return None from ctx.db → message silently dropped. The
                    // coordinator's conn has SELECT * and always finds both endpoints.
                    let coord = chat_coord.0.coord();
                    let speaker = match coord
                        .conn
                        .db
                        .game_world_entity()
                        .guid()
                        .find(&row.sender_guid)
                    {
                        Some(e) => e,
                        None => return,
                    };
                    let listener = match coord.conn.db.game_world_entity().guid().find(&self_guid) {
                        Some(e) => e,
                        None => return,
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
                        return;
                    }
                }
                let m = codec::build_chat_message(
                    row.sender_guid,
                    row.chat_type,
                    row.language,
                    row.message.clone(),
                );
                let _ = chat_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
                    Box::new(m),
                )));
            });
        // ======================================================================================
        //  CHAT CHANNELS — game_channel_event (insert)
        //  SMSG_MESSAGECHAT for a line on a channel this connection is a member of
        //  (game_channel_member is subscribed but has no callback of its own).
        // ======================================================================================
        // Chat channels (065): a channel line reaches every MEMBER, anywhere (no proximity —
        // General spans the zone; membership IS the audience). Each connection filters on its OWN
        // membership row (game_channel_member is in this player's subscription); the sender hears
        // their echo through the same path (they're a member too — vanilla echoes channel lines).
        let channel_tx = tx.clone();
        let on_channel = player
            .conn
            .db
            .game_channel_event()
            .on_insert(move |ctx, row| {
                let member = ctx
                    .db
                    .game_channel_member()
                    .iter()
                    .any(|m| m.character_guid == self_guid && m.channel == row.channel);
                if !member {
                    return;
                }
                let m = codec::build_channel_message(
                    row.sender_guid,
                    row.channel_display.clone(),
                    row.message.clone(),
                );
                let _ = channel_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
                    Box::new(m),
                )));
            });
        // ======================================================================================
        //  EMOTES — game_emote_event (insert)
        //  SMSG_TEXT_EMOTE + SMSG_EMOTE for a text/targeted emote.
        // ======================================================================================
        // Emote broadcast (social tier; public, no RLS) → SMSG_TEXT_EMOTE (the "X dances." line) + the
        // SMSG_EMOTE animation. Both degrade gracefully: an unknown text-emote / animation id is simply
        // skipped so the rest still relays.
        let emote_tx = tx.clone();
        let emote_coord = self.clone(); // global db handle — resolves the target guid's name across owners
        let on_emote = player
            .conn
            .db
            .game_emote_event()
            .on_insert(move |_ctx, row| {
                // target_guid == 0 → untargeted emote (the client sends 0 when nothing is selected).
                // Target may be a player (game_character) or an NPC/creature (game_world_entity ->
                // game_creature_template), same join as reads.rs's synthesized_objectives.
                let target_name = if row.target_guid != 0 {
                    let db = &emote_coord.0.coord().conn.db;
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
                if let Some(m) = codec::build_text_emote(
                    row.sender_guid,
                    row.text_emote,
                    row.emote_anim,
                    target_name,
                ) {
                    let _ = emote_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_TEXT_EMOTE(
                        Box::new(m),
                    )));
                }
                if let Some(a) = codec::build_emote_anim(row.sender_guid, row.emote_anim) {
                    let _ =
                        emote_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_EMOTE(Box::new(a))));
                }
            });
        // ======================================================================================
        //  WHISPER, per-player leg — game_whisper_event (insert)
        //  SMSG_MESSAGECHAT (Whisper/WhisperInform) for a row RLS-addressed to this player. See the
        //  realm-core twin below for the cross-database case.
        // ======================================================================================
        // Whisper (social tier; RLS-scoped — this subscription only sees rows addressed to this
        // player) → SMSG_MESSAGECHAT (Whisper "X whispers:" or WhisperInform "To X:").
        let whisper_tx = tx.clone();
        let on_whisper = player
            .conn
            .db
            .game_whisper_event()
            .on_insert(move |_ctx, row| {
                let m = codec::build_whisper(row.other_guid, row.is_inform, row.message.clone());
                let _ = whisper_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
                    Box::new(m),
                )));
            });

        // ======================================================================================
        //  GROUP / LOOT ROLL / QUEST SHARE, per-player leg — game_group_event (insert)
        //  Party invite/roster/decline/destroy packets. This SAME table is reused, by reserved kind
        //  range, for loot/master-loot rolls (work-item 187) and quest share/push-result (work-item
        //  194). See the realm-core twin below.
        // ======================================================================================
        // Group events (066) → the party packets. INVITE/DECLINE carry the pre-resolved name;
        // LIST carries the full roster IN THE EVENT ROW'S PAYLOAD — the module serializes it in
        // the same transaction as the membership change (payload-carry; decoded by
        // lyracore_shared::group::decode_roster), so the roster is exactly what that change produced;
        // DESTROYED clears the party UI. Kinds and grammar are the lyracore-shared group contract
        // (work-item 163) — the module writes the same constants. Unknown kinds are dropped loudly.
        use lyracore_shared::group::event_kind as group_kind;
        // Work-item 187: the roll/master-loot relay REUSES this SAME per-recipient event table
        // instead of a new gateway-subscribed table (see `lyracore_shared::loot_roll`'s module doc for
        // the full rationale) — its kinds live in the reserved `4..=7` range, decoded below
        // alongside the group-membership kinds.
        use lyracore_shared::loot_roll::event_kind as roll_kind;
        // Work-item 194 (sharing): QUEST_SHARE/QUEST_PUSH_RESULT reuse this SAME per-recipient table —
        // kinds 10/11, the next free slots after PARTY_CHAT (9). QUEST_SHARE needs a full quest-detail
        // JOIN (quest_template/text/objectives/rewards) that ISN'T in this player's own per-connection
        // subscription set (NOT ONE of the four is, since 292 dropped game_quest_objective from that
        // list — see the base_queries DO-NOT-RE-ADD note below), so it
        // goes through a CLONED privileged coordinator handle (the SAME `quest_detail` inherent method
        // `world/mod.rs`'s CMSG_QUEST_QUERY handler uses), not `ctx.db`.
        use lyracore_shared::quest::share_event_kind as quest_share_kind;
        let quest_share_coord = self.clone();
        let group_tx = tx.clone();
        let on_group_event = player.conn.db.game_group_event().on_insert(move |_ctx, row| {
            let msg = match row.kind {
                group_kind::INVITE => Some(ServerOpcodeMessage::SMSG_GROUP_INVITE(Box::new(
                    codec::build_group_invite(row.other_name.clone()),
                ))),
                group_kind::LIST => match lyracore_shared::group::decode_roster(&row.payload) {
                    Some((leader, loot_method, loot_threshold, master_looter_guid, members)) => {
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
                // Work-item 199: a party (`/p`) chat line, one row per recipient (every OTHER member
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
                // Work-item 221: a grouped money-loot split's per-recipient share → the SAME
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
                // Work-item 194 (sharing): an eligible party member receives the shared quest —
                // `row.other_guid` is the SHARER (resolved/pushed by `group::push_event`), `row.payload`
                // is the quest entry. Opens the DETAILS screen with the SHARER as "giver" (the
                // recipient's own `CMSG_QUESTGIVER_ACCEPT_QUEST` then re-validates fresh via the
                // module's `GiverKind::Party` — this relay never authorizes anything by itself).
                quest_share_kind::QUEST_SHARE => match row.payload.parse::<u32>() {
                    Ok(quest_id) => match quest_share_coord.quest_detail(quest_id) {
                        Ok(Some(detail)) => Some(ServerOpcodeMessage::SMSG_QUESTGIVER_QUEST_DETAILS(Box::new(
                            codec::build_quest_details(row.other_guid, &detail),
                        ))),
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
                // Work-item 194 (sharing): the SENDER's per-member feedback line, one row per member,
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
            if let Some(m) = msg {
                let _ = group_tx.send(Outbound::One(m));
            }
        });
        // ======================================================================================
        //  GROUP, realm-core twin — game_group_event (insert, on the realm-core coordinator
        //  connection)
        //  #22: party events for a session whose shard differs from realm-core, where membership
        //  actually changes; the owner token bypasses RLS and the closure self-filters on
        //  recipient_guid.
        // ======================================================================================
        // REALM-CORE group relay (#22, group slice). The party packets a MULTI-DATABASE gateway
        // must deliver come from the directory database, not from this player's shard: realm-core is
        // where membership changes, so it is where the events are written.
        //
        // It rides the realm-core COORDINATOR connection rather than a per-player one, for the
        // reason the 277/279 delivery law already establishes for teleport/XP/levelup: the player
        // connection is on the WRONG DATABASE entirely here (its identity is minted per database, so
        // there is no realm-core connection this player could authenticate as, and no RLS predicate
        // that could name them). The owner token bypasses RLS, so every player's rows arrive and each
        // session self-filters on `recipient_guid` — the guid being the one realm-wide name a
        // character has.
        //
        // Only the four MEMBERSHIP kinds are handled here. Loot rolls, money shares, `/p` chat and
        // quest sharing are produced by SHARD code against the shard's mirror and keep riding the
        // per-player relay above, untouched by this slice.
        //
        // TWO conditions, and the second is not an optimization. Unsharded → `None`, nothing is
        // registered, and the relay above is the only one, exactly as before #22. And realm-core
        // must be a DIFFERENT DATABASE than the one this session is on: with `LYRACORE_SHARD_MAP` set but
        // `LYRACORE_REALM_CORE` unset, `realm_core()` answers the default database — so for a session that
        // is ALREADY on it, the party events would arrive twice (once here through the owner token,
        // once through the per-player RLS relay above) and every invite dialog and roster refresh
        // would be sent to the client in duplicate. A session on any OTHER shard still needs this
        // relay in that configuration, which is why the test is "different database", not
        // "realm-core configured".
        let realm_group_relay = self
            .is_sharded()
            .then(|| self.realm_core().ok())
            .flatten()
            .filter(|rc| Coordinator::shard_name(rc) != Coordinator::shard_name(self));
        let on_realm_group = realm_group_relay.as_ref().map(|realm| {
            let realm_reader = realm.clone();
            // The NAME/ONLINE resolver: this handle, whose `character_by_guid`/`entity_in_world`
            // union every connected shard (realm-core holds no character rows, so a party frame
            // cannot be rendered from it alone).
            let names = self.clone();
            let realm_tx = tx.clone();
            realm.0.coord().conn.db.game_group_event().on_insert(move |_ctx, row| {
                if row.recipient_guid != self_guid {
                    return; // another player's event — the owner token sees them all
                }
                let msg = match row.kind {
                    group_kind::INVITE | group_kind::DECLINE => {
                        let name = names
                            .character_by_guid(row.other_guid)
                            .ok()
                            .flatten()
                            .map(|c| c.name)
                            .unwrap_or_default();
                        Some(if row.kind == group_kind::INVITE {
                            ServerOpcodeMessage::SMSG_GROUP_INVITE(Box::new(codec::build_group_invite(name)))
                        } else {
                            ServerOpcodeMessage::SMSG_GROUP_DECLINE(Box::new(codec::build_group_decline(name)))
                        })
                    }
                    // The roster is READ rather than payload-carried. The module's payload-carry
                    // exists so a relay never races a cross-connection read — and here it would not:
                    // the membership rows and this event row arrive on the SAME realm-core
                    // subscription, in the same transaction, so the cache already holds the change
                    // that produced the event. Reading it also means names and online flags come
                    // from the shards that actually have them, which the payload could not carry.
                    group_kind::LIST => match realm_reader.group_roster(self_guid) {
                        Some(roster) => {
                            Some(crate::world::party::render_list(&names, self_guid, &roster))
                        }
                        None => {
                            log::warn!(
                                "realm group relay: LIST for guid {self_guid} but realm-core has no \
                                 roster for them (event {})",
                                row.id
                            );
                            None
                        }
                    },
                    group_kind::DESTROYED => Some(ServerOpcodeMessage::SMSG_GROUP_DESTROYED),
                    // Issue #50: a vote landing or a roll resolving is written HERE, on realm-core,
                    // once the roll has been promoted — voting is routed to realm-core exclusively in
                    // a sharded deployment (`world::loot::run_vote`), so these two kinds are no longer
                    // shard-produced the way the comment below still correctly describes the rest.
                    // Same decode the per-player relay above uses; `ROLL_START` stays shard-produced
                    // (it fires locally, before promotion, from the same transaction that stages the
                    // roll — see `module/src/loot.rs`'s module doc), so it is deliberately NOT here.
                    roll_kind::ROLL_VOTE => match lyracore_shared::loot_roll::decode_vote(&row.payload) {
                        Some((corpse_guid, slot, item_entry, roll_number, vote, auto_pass)) => {
                            Some(ServerOpcodeMessage::SMSG_LOOT_ROLL(Box::new(codec::build_loot_roll(
                                corpse_guid, slot, row.other_guid, item_entry, roll_number, vote, auto_pass,
                            ))))
                        }
                        None => {
                            log::warn!(
                                "realm loot ROLL_VOTE relay: unparseable payload {:?} (event {})",
                                row.payload, row.id
                            );
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
                            log::warn!(
                                "realm loot ROLL_WON relay: unparseable payload {:?} (event {})",
                                row.payload, row.id
                            );
                            None
                        }
                    },
                    // Every other kind is shard-produced (ROLL_START, master-loot, party chat, quest
                    // sharing) and is relayed by the per-player callback above. Realm-core never
                    // writes them.
                    _ => None,
                };
                if let Some(m) = msg {
                    let _ = realm_tx.send(Outbound::One(m));
                }
            })
        });
        // ======================================================================================
        //  WHISPER, realm-core twin — game_whisper_event (insert, on the realm-core coordinator
        //  connection)
        //  #22: a whisper between two players on different databases is written on realm-core, the
        //  only place both can be named; same self-filter as the group twin above.
        // ======================================================================================
        // REALM-CORE whisper relay (#22, whisper slice). Same connection, same gate, same self-filter
        // as the group relay above — and it has to be this connection: a whisper between two players
        // on DIFFERENT databases is written on realm-core (the only place both can be named), so no
        // per-player subscription on either shard will ever see the row. The owner token bypasses RLS,
        // every player's rows arrive here, and `recipient_guid` is what makes this session's whispers
        // this session's; `recipient_identity` is ZERO on realm-core and addresses nobody.
        //
        // The reused `realm_group_relay` handle carries the DIFFERENT-DATABASE guard with it: with
        // `LYRACORE_SHARD_MAP` set and `LYRACORE_REALM_CORE` unset, `realm_core()` answers the DEFAULT database,
        // and a session already on it would get every whisper line twice (once here through the owner
        // token, once through the per-player RLS relay above) — one "To Bob:" echo rendered twice in
        // the chat frame per whisper sent.
        //
        // No name resolution: `SMSG_MESSAGECHAT` carries the OTHER party's guid and the client
        // resolves the name itself, over `CMSG_NAME_QUERY` — which the whisper slice made realm-wide
        // for exactly this reason (`world::party::character_anywhere`).
        let on_realm_whisper = realm_group_relay.as_ref().map(|realm| {
            let realm_tx = tx.clone();
            realm
                .0
                .coord()
                .conn
                .db
                .game_whisper_event()
                .on_insert(move |_ctx, row| {
                    if row.recipient_guid != self_guid {
                        return;
                    }
                    let m =
                        codec::build_whisper(row.other_guid, row.is_inform, row.message.clone());
                    let _ = realm_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT(
                        Box::new(m),
                    )));
                })
        });

        // ======================================================================================
        //  ROLL — game_roll_event (insert)
        //  MSG_RANDOM_ROLL broadcast to every connection, including the roller (matches vanilla).
        // ======================================================================================
        // /roll broadcast (social tier; public, no RLS) → MSG_RANDOM_ROLL_Server. Every connection
        // fans each roll row, so the roller sees their own result too (as vanilla does). The result is
        // server-computed server-side in the module's send_roll reducer.
        let roll_tx = tx.clone();
        let on_roll = player
            .conn
            .db
            .game_roll_event()
            .on_insert(move |_ctx, row| {
                let m = codec::build_random_roll(
                    row.roller_guid,
                    row.min_roll,
                    row.max_roll,
                    row.result,
                );
                let _ = roll_tx.send(Outbound::One(ServerOpcodeMessage::MSG_RANDOM_ROLL(
                    Box::new(m),
                )));
            });
        // ======================================================================================
        //  AURA — game_aura (insert/update/delete)
        //  Full-array UNIT_FIELD_AURA re-sync on any change; the self-only armor-sheet relay; and
        //  stealth peer-visibility (HIDE/REVEAL) for a non-self A_STEALTH crossing.
        // ======================================================================================
        // Aura array sync (multi-slot, via the raw-send path). On ANY aura change we re-send the
        // target's FULL current aura array: the packed flag/level words are shared across slots, so
        // the whole set must be composed together (codec::update_mask::full_aura_mask), and each aura
        // is written at its authoritative module-assigned `row.slot`. Routed through Outbound::Raw
        // because gtker's typed builder exposes only slot 0. on_delete fires AFTER the SDK cache drops
        // the row, so iterating the cache yields the post-delete set → the removed slot self-clears.
        fn aura_sync(
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
        fn armor_packet(coord: &Coordinator, changed: &Aura, self_guid: u64) -> Option<Outbound> {
            const A_MOD_RESISTANCE: u8 = 0xA1; // taxonomy A_MOD_RESISTANCE
            const RESIST_ARMOR_MASK: u32 = 0x01; // taxonomy RESIST_ARMOR bit (eff_p0 is a school MASK)
            if changed.target_guid != self_guid
                || changed.eff_kind != A_MOD_RESISTANCE
                || (changed.eff_p0 as u32 & RESIST_ARMOR_MASK) == 0
            {
                return None;
            }
            // #468: the entity row (the BASE armor term) lives only on the coordinator now — the
            // per-player connection no longer subscribes `game_world_entity` at all. The
            // coordinator's cache also carries the auras, the item instances and the item
            // templates, so this fold is complete there in a way it never was on the player
            // connection (which lost `game_item_template` to #292).
            let guard = coord.0.coord();
            let db = &guard.conn.db;
            let eff = super::armor::effective_armor(db, self_guid);
            // 082: carry the positive AURA portion alongside the total so the paperdoll renders the
            // green "(+N)" (Devotion Aura showed as plain white armor). Raw path — the positive
            // field has no gtker setter. Login self-corrects through this same relay (the SDK
            // replays aura rows after subscription-apply).
            let pos = super::armor::aura_armor_positive(db, self_guid);
            Some(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                Box::new(codec::build_armor_values(self_guid, eff, pos)),
            )))
        }
        // Stealth peer-visibility: when a NON-self peer's A_STEALTH presence crosses the 0↔1 boundary,
        // HIDE it from this viewer (SMSG_DESTROY_OBJECT + evict from `created`) on the gain, REVEAL it
        // (re-CREATE + re-insert into `created`) on the loss. The recipient set is implicit — every
        // viewer's own connection drains the broadcast `game_aura` table and runs THIS closure, so each
        // in-scope client hides/reveals on its own `tx`. Self is excluded (`changed.target_guid !=
        // self_guid`) so a stealther never hides from itself. Idempotency is the `created` set: HIDE only
        // fires (and DESTROYs) if the guid was created; REVEAL only fires (and CREATEs) if it wasn't —
        // re-hiding a hidden peer or re-revealing a visible one is a no-op. The stealther's entity row is
        // read from the firing connection's cache (`ctx.db`); a guid the viewer can't see in scope has no
        // row → REVEAL self-skips. `coord` reads the peer's gear RLS-bypassed on reveal (same as insert).
        // One argument per piece of per-viewer state the decision needs; they are not grouped
        // because the caller has them as separate captures.
        #[allow(clippy::too_many_arguments)]
        fn stealth_visibility(
            ctx: &EventContext,
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
            let count = ctx
                .db
                .game_aura()
                .iter()
                .filter(|a| a.target_guid == changed.target_guid && a.eff_kind == A_STEALTH)
                .count();
            match stealth_action(is_insert, count) {
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
                    // #468: "is the peer in this viewer's scope" used to be "is its row in this
                    // connection's cache", which the per-player box subscription made equivalent.
                    // The shared connection's cache holds the whole world, so the question has to be
                    // put to the cell index instead — otherwise a stealther unstealthing on the far
                    // side of the zone would CREATE for everyone.
                    if !view.entities.can_see(session, changed.target_guid) {
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
                        Some(row) => match build_peer_create(coord, &row) {
                            Some(m) => vec![Outbound::One(m)],
                            None => {
                                // Encode failure: roll the dedup entry back like `offer_peer_create`
                                // does, else this guid is permanently suppressed (marked created with
                                // no CREATE ever sent) — the same latent bug 144 fixed on the
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
        let stealth_ins_coord = self.clone();
        let stealth_ins_view = view.clone();
        let armor_coord_ins = self.clone();
        let stealth_ins_created = created.clone();
        let aura_ins_tx = tx.clone();
        let on_aura_insert = player.conn.db.game_aura().on_insert(move |ctx, row| {
            // A peer hidden by stealth is evicted from `created`; it must receive NO further per-peer
            // relays for that guid — a partial-VALUES aura_sync (or run-speed) on a DESTROYed object is a
            // client crash/desync vector. Checked BEFORE stealth_visibility's HIDE below. Self always visible.
            let visible = row.target_guid == self_guid
                || stealth_ins_created
                    .lock()
                    .unwrap()
                    .contains(&row.target_guid);
            if visible {
                let _ = aura_ins_tx.send(aura_sync(ctx.db.game_aura().iter(), row.target_guid));
            }
            if let Some(o) = aura_duration_packet(row, self_guid) {
                let _ = aura_ins_tx.send(o);
            }
            if visible {
                if let Some(o) = run_speed_packet(ctx.db.game_aura().iter(), row, self_guid) {
                    let _ = aura_ins_tx.send(o);
                }
                // Live armor (Demon Skin): a self armor aura insert re-pushes UNIT_FIELD_RESISTANCES[0].
                if let Some(o) = armor_packet(&armor_coord_ins, row, self_guid) {
                    let _ = aura_ins_tx.send(o);
                }
            }
            for o in stealth_visibility(
                ctx,
                &stealth_ins_view,
                stealth_session,
                &stealth_ins_coord,
                &stealth_ins_created,
                row,
                self_guid,
                true,
            ) {
                let _ = aura_ins_tx.send(o);
            }
        });
        let aura_upd_tx = tx.clone();
        let stealth_upd_created = created.clone();
        let armor_coord_upd = self.clone();
        let on_aura_update = player.conn.db.game_aura().on_update(move |ctx, old, row| {
            // A stealth-hidden peer (not in `created`) gets no VALUES/speed relay (see on_aura_insert).
            let visible = row.target_guid == self_guid
                || stealth_upd_created
                    .lock()
                    .unwrap()
                    .contains(&row.target_guid);
            if visible {
                let _ = aura_upd_tx.send(aura_sync(ctx.db.game_aura().iter(), row.target_guid));
            }
            // Re-send the timer only when the duration window actually changed (a refresh) — not on
            // unrelated flag/level updates, which would jitter the client's countdown back to full.
            if old.expires_at != row.expires_at {
                if let Some(o) = aura_duration_packet(row, self_guid) {
                    let _ = aura_upd_tx.send(o);
                }
            }
            if visible {
                if let Some(o) = run_speed_packet(ctx.db.game_aura().iter(), row, self_guid) {
                    let _ = aura_upd_tx.send(o);
                }
                // Live armor: a self armor aura whose fields changed (e.g. stacks) re-pushes the sheet.
                if let Some(o) = armor_packet(&armor_coord_upd, row, self_guid) {
                    let _ = aura_upd_tx.send(o);
                }
            }
        });
        let stealth_del_coord = self.clone();
        let stealth_del_view = view.clone();
        let armor_coord_del = self.clone();
        let stealth_del_created = created.clone();
        let aura_del_tx = tx.clone();
        let on_aura_delete = player.conn.db.game_aura().on_delete(move |ctx, row| {
            // Checked BEFORE stealth_visibility's REVEAL below: a still-hidden peer (the first of two
            // A_STEALTH deletes) gets no relay; once REVEAL re-adds it to `created`, the next delete sends.
            let visible = row.target_guid == self_guid
                || stealth_del_created
                    .lock()
                    .unwrap()
                    .contains(&row.target_guid);
            if visible {
                let _ = aura_del_tx.send(aura_sync(ctx.db.game_aura().iter(), row.target_guid));
                if let Some(o) = run_speed_packet(ctx.db.game_aura().iter(), row, self_guid) {
                    let _ = aura_del_tx.send(o);
                }
                // Live armor (Demon Skin expiry): on_delete fires AFTER the row is gone, so the fold reads
                // the remaining auras → the sheet drops back to base + gear.
                if let Some(o) = armor_packet(&armor_coord_del, row, self_guid) {
                    let _ = aura_del_tx.send(o);
                }
            }
            for o in stealth_visibility(
                ctx,
                &stealth_del_view,
                stealth_session,
                &stealth_del_coord,
                &stealth_del_created,
                row,
                self_guid,
                false,
            ) {
                let _ = aura_del_tx.send(o);
            }
        });

        // ======================================================================================
        //  QUEST LOG, coordinator-registered — game_character_quest (insert/update/delete)
        //  Full PLAYER_QUEST_LOG re-sync (raw-send) on any change to this player's log; moved off
        //  the per-player connection by 279 for the same reason as XP/level-up above.
        // ======================================================================================
        // Quest-log array sync (Phase 2, raw-send path). On ANY change to this player's quest log
        // (accept / kill-progress / turn-in), re-send the FULL PLAYER_QUEST_LOG block (full sync so a
        // turned-in quest's slot clears). Reads ctx.db (the connection that fired) so the change is
        // already visible; shares build_quest_log_slots with the login read so slots line up. GATED
        // behind LYRACORE_QUEST_LOG (default ON) — returns None when set to 0 (escape hatch; the quest-log
        // descriptor layout is verified against the live 5875 client).
        fn quest_log_sync(
            db: &crate::stdb::bindings::RemoteTables,
            self_guid: u64,
        ) -> Option<Outbound> {
            if !crate::config::quest_log_fields_enabled() {
                return None;
            }
            // build_quest_log_slots is now INVENTORY-AWARE (collect-quest completion), so it reads `db`
            // (game_quest_objective + game_item_instance). Both callers pass the COORDINATOR's cache
            // (`quest_log_sync` is only ever invoked from coordinator-registered callbacks, 279), which
            // holds every player's rows and is the only place game_quest_objective lives since 292.
            let quests: Vec<_> = db.game_character_quest().iter().collect();
            let slots = super::reads::build_quest_log_slots(db, &quests, self_guid);
            // Send the FULL quest-log sync on EVERY change, including the empty case. Turning in the LAST
            // quest flips its `rewarded` flag (an UPDATE, kept row) → build_quest_log_slots (filters
            // !rewarded) returns EMPTY → full_quest_log_mask(&[]) zeroes all 60 slot fields, which is HOW the
            // client removes the quest (slot 0's quest_id 783 → 0). This MUST be sent or the turned-in quest
            // lingers in the log and the questgiver's overhead `?` stays stale until the next quest-log change
            // (e.g. accepting the follow-up) forces a relay. An earlier empty-slots guard skipped this on a
            // MISDIAGNOSIS — the real McBride crash was SMSG_SET_FACTION_STANDING sending faction_id instead of
            // the rep-index (fixed in dee80cb). The all-zero mask is structurally identical to the working
            // "1 quest left" case (19 slots already zeroed); a zeroed slot 0 is the same clear, not a crash.
            let mask = codec::update_mask::full_quest_log_mask(&slots);
            let (opcode, body) = codec::build_values_update_raw(self_guid, &mask);
            Some(Outbound::Raw { opcode, body })
        }
        let quest_ins_tx = tx.clone();
        // COORDINATOR-registered since 279 (the 277 loss class): quest-log rows ride kill and
        // turn-in transactions — large, movement-concurrent, and a lost update leaves the quest
        // log desynced until relog. The coordinator sees EVERY player's rows (owner RLS bypass),
        // so each closure now guards on character_guid; quest_log_sync reads the coordinator
        // cache, which is a superset, and already filters by self_guid.
        let on_quest_insert =
            self.0
                .coord()
                .conn
                .db
                .game_character_quest()
                .on_insert(move |ctx, row| {
                    if row.character_guid != self_guid {
                        return;
                    }
                    if let Some(o) = quest_log_sync(&ctx.db, self_guid) {
                        let _ = quest_ins_tx.send(o);
                    }
                });
        let quest_upd_tx = tx.clone();
        let on_quest_update =
            self.0
                .coord()
                .conn
                .db
                .game_character_quest()
                .on_update(move |ctx, old, row| {
                    if row.character_guid != self_guid {
                        return;
                    }
                    // Kill-progress feedback (#3): emit SMSG_QUESTUPDATE_ADD_KILL ("Creature slain: n/N") for each
                    // KILL objective whose count INCREASED this update. Diff logic is the unit-tested pure
                    // `codec::kill_progress_add_kills`; here we just gather this quest's objectives and relay.
                    let objs: Vec<(u8, u8, u32, u32)> = ctx
                        .db
                        .game_quest_objective()
                        .iter()
                        .filter(|o| o.quest_entry == row.quest_entry)
                        .map(|o| (o.kind, o.obj_index, o.target_entry, o.required_count))
                        .collect();
                    for m in codec::kill_progress_add_kills(
                        row.quest_entry,
                        &old.counts,
                        &row.counts,
                        &objs,
                    ) {
                        let _ = quest_upd_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_QUESTUPDATE_ADD_KILL(Box::new(m)),
                        ));
                    }
                    // Timed-quest expiry (work-item 194): the tick flips `failed` false->true — relay
                    // SMSG_QUESTUPDATE_FAILEDTIMER so the client's quest-log entry shows FAILED. Diffed (not a
                    // bare `if row.failed`) so this fires exactly once, on the transition, never on every
                    // subsequent update to an already-failed row.
                    if !old.failed && row.failed {
                        let _ = quest_upd_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_QUESTUPDATE_FAILEDTIMER(
                                codec::build_questupdate_failedtimer(row.quest_entry),
                            ),
                        ));
                    }
                    if let Some(o) = quest_log_sync(&ctx.db, self_guid) {
                        let _ = quest_upd_tx.send(o);
                    }
                });
        let quest_del_tx = tx.clone();
        let on_quest_delete =
            self.0
                .coord()
                .conn
                .db
                .game_character_quest()
                .on_delete(move |ctx, row| {
                    if row.character_guid != self_guid {
                        return;
                    }
                    // #72 (the second live defect, alongside items): finish_transfer's cascade deletes every
                    // game_character_quest row for this character on the SOURCE database — without this guard,
                    // quest_log_sync would read the now-empty set and send an all-zero PLAYER_QUEST_LOG VALUES,
                    // visually wiping the quest log on the client for the brief window before the OLD subs are
                    // torn down, even though the destination's import holds the byte-identical rows.
                    if let Some(o) = quest_log_sync(&ctx.db, self_guid) {
                        let _ = quest_del_tx.send(o);
                    }
                });

        // ======================================================================================
        //  ITEM INSTANCE, coordinator-registered — game_item_instance (insert/update/delete)
        //  Bag CREATE/DESTROY, the PLAYER_FIELD_INV_SLOT pointer, SMSG_ITEM_PUSH_RESULT gain-
        //  feedback + quest item-objective toast, and the armor/sheet-stats relay on an equip-slot
        //  or durability-broken crossing.
        // ======================================================================================
        // Inventory live-sync: a new item (vendor buy / loot take / quest reward) appears in the bag,
        // a removed item (sell / consume) vanishes — WITHOUT a relog. Scoped to THIS player's own items
        // (owner_guid). Each needs BOTH the item object (CREATE/DESTROY) AND the player's
        // PLAYER_FIELD_INV_SLOT pointer (set/clear) so the client places/removes it in the bag cell.
        // (The coinage half of a buy/sell already rides the game_world_entity on_update relay.)
        // Item-GAIN feedback gate (185/#15): the base sub's initial apply fires on_item_insert for
        // every owned item at LOGIN — and those callbacks arrive AFTER on_applied acks (wire-observed:
        // an on_applied-flipped boolean still let the whole bag toast at login), so the gate is the
        // AOI dedup pattern instead: snapshot the item guids resident at apply, suppress exactly those.
        let initial_item_guids: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<u64>>> =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        // Same replay gate for REPUTATION rows (live find 2026-07-11): login standings travel
        // silently in SMSG_INITIALIZE_FACTIONS; the apply-replayed rows must not re-announce
        // ("you are now Friendly with Stormwind" at every login). Keyed by faction_id (rows are
        // already self-filtered).
        let initial_rep_factions: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<u32>>> =
            std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        // The client recomputes watched-quest ITEM objectives (tracker + "x/y" floaty) when told an
        // item ARRIVED. COLLECT_ITEM progress is live-inventory server-side (no counter row change),
        // so this is the only signal path. Shared by insert (new stack) + update (stack grew).
        let item_gain_feedback = {
            let tx = tx.clone();
            let initial_item_guids = initial_item_guids.clone();
            move |db: &crate::stdb::bindings::RemoteTables,
                  item_guid: u64,
                  slot: u8,
                  entry: u32,
                  gained: u32,
                  stack_add: bool| {
                // item_guid 0 = an UPDATE event (stack growth — never an apply-time replay);
                // otherwise suppress the login-apply replay of already-owned rows.
                if gained == 0
                    || (item_guid != 0 && initial_item_guids.lock().unwrap().contains(&item_guid))
                {
                    return;
                }
                let m = codec::build_item_push_result(
                    self_guid,
                    255,
                    slot as u32,
                    entry,
                    gained,
                    stack_add,
                );
                let _ = tx.send(Outbound::One(ServerOpcodeMessage::SMSG_ITEM_PUSH_RESULT(
                    Box::new(m),
                )));
                // Quest item-objective toast: entry matches an incomplete COLLECT_ITEM objective of
                // an active (un-rewarded, un-failed) quest → SMSG_QUESTUPDATE_ADD_ITEM(entry, gained).
                let wanted = db.game_character_quest().iter().any(|q| {
                    q.character_guid == self_guid
                        && !q.rewarded
                        && !q.failed
                        && db.game_quest_objective().iter().any(|o| {
                            o.quest_entry == q.quest_entry && o.kind == 1 && o.target_entry == entry
                        })
                });
                if wanted {
                    use wow_world_messages::vanilla::SMSG_QUESTUPDATE_ADD_ITEM;
                    let _ = tx.send(Outbound::One(
                        ServerOpcodeMessage::SMSG_QUESTUPDATE_ADD_ITEM(SMSG_QUESTUPDATE_ADD_ITEM {
                            required_item_id: entry,
                            items_required: gained,
                        }),
                    ));
                }
            }
        };
        let item_gain_ins = item_gain_feedback.clone();
        let item_gain_upd = item_gain_feedback;
        let item_ins_tx = tx.clone();
        let on_item_insert =
            self.0
                .coord()
                .conn
                .db
                .game_item_instance()
                .on_insert(move |ctx, row| {
                    if row.owner_guid != self_guid {
                        return;
                    }
                    item_gain_ins(
                        &ctx.db,
                        row.guid,
                        row.slot,
                        row.entry,
                        row.stack_count,
                        false,
                    );
                    // Look up the template for max_durability + container_slots (game_item_template is
                    // subscribed on the COORDINATOR, which is the cache `ctx.db` is here — this callback is
                    // coordinator-registered, and 292 removed the catalogue from the per-player set).
                    // Fall back to safe defaults if missing.
                    let (max_durability, container_slots) = ctx
                        .db
                        .game_item_template()
                        .entry()
                        .find(&row.entry)
                        .map(|t| (t.max_durability, t.container_slots))
                        .unwrap_or((row.durability, 0)); // max = current durability as fallback
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
                    // Item object FIRST, then the slot pointer (the pointer must reference an existing object).
                    let _ =
                        item_ins_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_UPDATE_OBJECT(
                            Box::new(codec::build_item_create_object(&view)),
                        )));
                    if let Some(o) = codec::build_inv_slot_values(self_guid, row.slot, row.guid) {
                        let _ = item_ins_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(o)),
                        ));
                    }
                    // 087: an item landing directly in an EQUIPMENT slot renders on the model/paperdoll.
                    if let Some(o) =
                        codec::build_visible_item_values(self_guid, row.slot, row.entry)
                    {
                        let _ = item_ins_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(o)),
                        ));
                    }
                    // For bag-content slots (120..=191), `build_inv_slot_values` returns None (no
                    // PLAYER_FIELD_INV_SLOT pointer for bag contents). The slot pointer instead lives on
                    // the container object's own CONTAINER_FIELD_SLOT_N descriptor — send a raw VALUES
                    // update on the bag's guid so the client populates the bag window.
                    if let Some((bag_equip_slot, slot_in_bag)) = bag_content_parts(row.slot) {
                        if let Some(bag) = ctx
                            .db
                            .game_item_instance()
                            .iter()
                            .find(|i| i.owner_guid == self_guid && i.slot == bag_equip_slot)
                        {
                            let (opcode, body) =
                                codec::build_container_slot_values(bag.guid, slot_in_bag, row.guid);
                            let _ = item_ins_tx.send(Outbound::Raw { opcode, body });
                        }
                    }
                    // Live armor: an item entering an EQUIPMENT slot (0..=18) changes worn armor → re-push the
                    // sheet (also corrects gear armor on the initial-state inserts at login). A bag insert (slot
                    // > 18) is skipped — it can't change worn armor.
                    if row.slot <= 18 {
                        let eff = super::armor::effective_armor(&ctx.db, self_guid);
                        let _ = item_ins_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                                codec::build_resistance_values(self_guid, eff),
                            )),
                        ));
                        // 053: gear moved -> re-push the paperdoll stats/AP/damage-range alongside armor
                        // (same trigger set; the login initial-state item replay corrects the CREATE's
                        // base-only values the same way it does armor).
                        if let Some(st) = super::armor::sheet_stats(&ctx.db, self_guid) {
                            let _ = item_ins_tx.send(Outbound::One(
                                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                                    codec::build_sheet_stats_values(self_guid, &st),
                                )),
                            ));
                        }
                    }
                });
        let item_del_tx = tx.clone();
        let on_item_delete =
            self.0
                .coord()
                .conn
                .db
                .game_item_instance()
                .on_delete(move |ctx, row| {
                    if row.owner_guid != self_guid {
                        return;
                    }
                    // #72 (the diagnosed live defect): finish_transfer's cascade deletes every
                    // game_item_instance row this character owns on the SOURCE database while these OLD subs
                    // are still registered — without this guard every item relayed SMSG_DESTROY_OBJECT +
                    // cleared its inventory/visible-item slots on the client, even though the destination's
                    // import holds the byte-identical rows and no equipment was actually lost.
                    // Clear the slot pointer FIRST (so the client never points at a doomed guid), then destroy.
                    if let Some(o) = codec::build_inv_slot_values(self_guid, row.slot, 0) {
                        let _ = item_del_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(o)),
                        ));
                    }
                    // 087: an item destroyed out of an EQUIPMENT slot un-renders from the model.
                    if let Some(o) = codec::build_visible_item_values(self_guid, row.slot, 0) {
                        let _ = item_del_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(o)),
                        ));
                    }
                    // For bag-content slots, clear the container's CONTAINER_FIELD_SLOT_N before destroying
                    // the item object — same clear-first discipline as the player INV_SLOT pointer above.
                    // on_delete fires after the row leaves the cache; the bag item itself is a different
                    // row and is still present, so the iter() lookup succeeds.
                    if let Some((bag_equip_slot, slot_in_bag)) = bag_content_parts(row.slot) {
                        if let Some(bag) = ctx
                            .db
                            .game_item_instance()
                            .iter()
                            .find(|i| i.owner_guid == self_guid && i.slot == bag_equip_slot)
                        {
                            let (opcode, body) =
                                codec::build_container_slot_values(bag.guid, slot_in_bag, 0);
                            let _ = item_del_tx.send(Outbound::Raw { opcode, body });
                        }
                    }
                    let _ =
                        item_del_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_DESTROY_OBJECT(
                            codec::build_destroy_object(row.guid),
                        )));
                    // Live armor: deleting a WORN item (an equipment slot) drops its armor → re-push the sheet.
                    // on_delete fires after the row leaves the cache, so the fold reads the remaining gear.
                    if row.slot <= 18 {
                        let eff = super::armor::effective_armor(&ctx.db, self_guid);
                        let _ = item_del_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                                codec::build_resistance_values(self_guid, eff),
                            )),
                        ));
                        // 053: gear moved -> re-push the paperdoll stats/AP/damage-range alongside armor
                        // (same trigger set; the login initial-state item replay corrects the CREATE's
                        // base-only values the same way it does armor).
                        if let Some(st) = super::armor::sheet_stats(&ctx.db, self_guid) {
                            let _ = item_del_tx.send(Outbound::One(
                                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                                    codec::build_sheet_stats_values(self_guid, &st),
                                )),
                            ));
                        }
                    }
                });
        // Item CHANGES (move/equip = slot change; merge/split/consume = stack change; wear/repair =
        // durability change). On a slot move, re-point the player's bag-slot pointers (clear old, set
        // new); on a stack/durability change, push the item object's own fields. Without this the client
        // shows stale slot/count/durability until relog.
        let item_upd_tx = tx.clone();
        let on_item_update =
            self.0
                .coord()
                .conn
                .db
                .game_item_instance()
                .on_update(move |ctx, old, row| {
                    if row.owner_guid != self_guid {
                        return;
                    }
                    if row.slot == old.slot && row.stack_count > old.stack_count {
                        item_gain_upd(
                            &ctx.db,
                            0,
                            row.slot,
                            row.entry,
                            row.stack_count - old.stack_count,
                            true,
                        );
                    }
                    if old.slot != row.slot {
                        if let Some(o) = codec::build_inv_slot_values(self_guid, old.slot, 0) {
                            let _ = item_upd_tx.send(Outbound::One(
                                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(o)),
                            ));
                        }
                        if let Some(o) = codec::build_inv_slot_values(self_guid, row.slot, row.guid)
                        {
                            let _ = item_upd_tx.send(Outbound::One(
                                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(o)),
                            ));
                        }
                        // 087: equip/unequip moves render/un-render the gear on the model + paperdoll —
                        // the login create sets PLAYER_VISIBLE_ITEM but nothing relayed it mid-session,
                        // so equipped gear was invisible until relog. Clear the old equipment slot's
                        // entry, set the new one (each is a no-op None for non-equipment slots).
                        if let Some(o) = codec::build_visible_item_values(self_guid, old.slot, 0) {
                            let _ = item_upd_tx.send(Outbound::One(
                                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(o)),
                            ));
                        }
                        if let Some(o) =
                            codec::build_visible_item_values(self_guid, row.slot, row.entry)
                        {
                            let _ = item_upd_tx.send(Outbound::One(
                                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(o)),
                            ));
                        }
                        // For bag-content slot moves: clear the container's old CONTAINER_FIELD_SLOT_N and
                        // set the new one. build_inv_slot_values returns None for slots >= 120 (correct —
                        // the player INV_SLOT pointer covers only equip/backpack); we use the raw path here
                        // targeting the BAG object's guid instead.
                        if let Some((old_bag_slot, old_slot_in_bag)) = bag_content_parts(old.slot) {
                            if let Some(bag) = ctx
                                .db
                                .game_item_instance()
                                .iter()
                                .find(|i| i.owner_guid == self_guid && i.slot == old_bag_slot)
                            {
                                let (opcode, body) = codec::build_container_slot_values(
                                    bag.guid,
                                    old_slot_in_bag,
                                    0,
                                );
                                let _ = item_upd_tx.send(Outbound::Raw { opcode, body });
                            }
                        }
                        if let Some((new_bag_slot, new_slot_in_bag)) = bag_content_parts(row.slot) {
                            if let Some(bag) = ctx
                                .db
                                .game_item_instance()
                                .iter()
                                .find(|i| i.owner_guid == self_guid && i.slot == new_bag_slot)
                            {
                                let (opcode, body) = codec::build_container_slot_values(
                                    bag.guid,
                                    new_slot_in_bag,
                                    row.guid,
                                );
                                let _ = item_upd_tx.send(Outbound::Raw { opcode, body });
                            }
                        }
                    }
                    if old.stack_count != row.stack_count || old.durability != row.durability {
                        let _ = item_upd_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                                codec::build_item_values(row.guid, row.stack_count, row.durability),
                            )),
                        ));
                    }
                    // Live armor: a slot change that touches the EQUIPMENT region (equip/unequip crosses 0..=18)
                    // OR a durability change that crosses the BROKEN threshold on a worn item (a broken item grants
                    // no armor) changes worn armor → re-push UNIT_FIELD_RESISTANCES[0]. The SDK applies the row to
                    // the cache before this fires, so the fold reads the post-change set.
                    let slot_crossed_equip =
                        old.slot != row.slot && (old.slot <= 18 || row.slot <= 18);
                    let broken_crossed =
                        row.slot <= 18 && (old.durability == 0) != (row.durability == 0);
                    if slot_crossed_equip || broken_crossed {
                        let eff = super::armor::effective_armor(&ctx.db, self_guid);
                        let _ = item_upd_tx.send(Outbound::One(
                            ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                                codec::build_resistance_values(self_guid, eff),
                            )),
                        ));
                        // 053: gear moved -> re-push the paperdoll stats/AP/damage-range alongside armor
                        // (same trigger set; the login initial-state item replay corrects the CREATE's
                        // base-only values the same way it does armor).
                        if let Some(st) = super::armor::sheet_stats(&ctx.db, self_guid) {
                            let _ = item_upd_tx.send(Outbound::One(
                                ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(
                                    codec::build_sheet_stats_values(self_guid, &st),
                                )),
                            ));
                        }
                    }
                });

        // ======================================================================================
        //  TELEPORT, coordinator-registered — game_teleport_event (insert)
        //  MSG_MOVE_TELEPORT_ACK (same-map) or SMSG_TRANSFER_PENDING+SMSG_NEW_WORLD (cross-map); on
        //  the coordinator since 277 because the per-player connection's AOI resubscription churn
        //  can swallow the event mid-flight.
        // ======================================================================================
        // Teleport relay (#11, work-item 224): a pending teleport for THIS player → MSG_MOVE_TELEPORT_ACK
        // (same-map) or SMSG_TRANSFER_PENDING+SMSG_NEW_WORLD (cross-map). Registered on the
        // COORDINATOR connection since 277, NOT `player.conn`: the per-player conn's AOI grid
        // subscriptions churn mid-flight (aoi.rs recenter, subscribe-new/unsubscribe-old), and a
        // concurrent transaction's deltas folded into an in-flight apply could swallow the event —
        // observed 100% on an instance-CREATING portal entry (~200-row transaction): the pair was
        // never sent and the despawned player limbo'd. The coordinator's subscription set is stable
        // (never per-query-swapped) and its owner token bypasses the recipient RLS; this closure
        // self-filters by mover_guid exactly as before, so routing is unchanged. Known edge: a
        // coordinator watchdog swap drops this callback until the session relogs — that blip
        // already disrupts every session, accepted. The branch is `still_here`: `teleport_player`
        // updates the entity IN PLACE same-map and DESPAWNS it cross-map; `row.cross_map` is the
        // authoritative module-side mirror of that decision. Cross-map: TRANSFER_PENDING and
        // NEW_WORLD go out together immediately (TRANSFER_PENDING first — the wire order the
        // 1.12.1 client requires); the client only replies
        // `MSG_MOVE_WORLDPORT_ACK` once its OWN load finishes, handled in `world/mod.rs`'s `enter_world`.
        let tele_tx = tx.clone();
        let tele_coord = self.clone(); // teardown handle — the callback lives on the coordinator db
        let on_teleport =
            self.0
                .coord()
                .conn
                .db
                .game_teleport_event()
                .on_insert(move |_ctx, row| {
                    if row.mover_guid != self_guid {
                        return;
                    }
                    // AUTHORITATIVE same-map/cross-map signal from the module (`cross_map`), NOT live-entity
                    // presence: with AOI on, a far same-map teleport moves the self entity out of this viewer's
                    // grid-scoped subscription, so `find(self_guid)` reads absent post-txn and the old proxy
                    // wrongly chose the cross-map (NEW_WORLD/loading-screen) path — hanging the client on a
                    // same-map `.tele` across zones. `still_here == !cross_map`.
                    let still_here = !row.cross_map;
                    match build_teleport_relay(
                        still_here,
                        row.mover_guid,
                        row.map_id,
                        row.x,
                        row.y,
                        row.z,
                        row.orientation,
                    ) {
                        Ok(out) => {
                            let _ = tele_tx.send(out);
                        }
                        Err(e) => log::warn!("teleport relay: {e} (guid {})", row.mover_guid),
                    }
                });

        // ======================================================================================
        //  ADDON MESSAGE, coordinator-registered — game_addon_message (insert)
        //  A queued server→client addon message relayed as an addon-language whisper (work-item
        //  184).
        // ======================================================================================
        // Addon-bridge relay (184): a queued server→client addon message becomes an
        // addon-language whisper (raw-built — gtker's Language enum has no LANG_ADDON) the
        // client surfaces to addons as CHAT_MSG_ADDON. COORDINATOR-registered (the 279 law:
        // this is the custom-UI state stream — a dropped frame is a stuck progress bar), so the
        // closure self-filters on the session's bound identity.
        let addon_tx = tx.clone();
        let addon_identity = player.identity;
        let on_addon = self
            .0
            .coord()
            .conn
            .db
            .game_addon_message()
            .on_insert(move |_ctx, row| {
                if row.recipient_identity != addon_identity {
                    return;
                }
                let text = codec::addon::build_bridge_envelope(&row.cmd, &row.payload);
                let (opcode, body) = codec::addon::build_addon_smsg_raw(self_guid, &text);
                let _ = addon_tx.send(Outbound::Raw { opcode, body });
            });

        // ======================================================================================
        //  REPUTATION, coordinator-registered — game_player_reputation (insert/update)
        //  SMSG_SET_FACTION_STANDING on a standing change for this player (#13); the same
        //  coordinator/replay-gate shape as XP and quest-log above.
        // ======================================================================================
        // Reputation relay (#13): a stored standing for THIS player changed (quest turn-in rep gain) →
        // SMSG_SET_FACTION_STANDING so the client's reputation bar moves WITHOUT a relog. RLS scopes the
        // table to this player; the character_guid guard is belt-and-suspenders. build_set_faction_standing
        // returns None for a faction id the client enum doesn't know (nothing to show) → skip silently.
        let rep_ins_tx = tx.clone();
        let rep_gate = initial_rep_factions.clone();
        let on_rep_insert =
            self.0
                .coord()
                .conn
                .db
                .game_player_reputation()
                .on_insert(move |_ctx, row| {
                    if row.character_guid != self_guid {
                        return;
                    }
                    // Login-apply replay (185 pattern): this standing already reached the client via
                    // INITIALIZE_FACTIONS — a re-relay makes the client toast it as a fresh gain.
                    //
                    // #41 AUDIT — deliberately left keyed as it is, on BOTH counts. (a) `faction_id` is the
                    // game's own faction id, not a surrogate PK, so a transfer's re-mint cannot change it —
                    // the exploration defect has no analogue here. (b) The freeze/replay race cannot reach
                    // it either: this callback rides the COORDINATOR connection, whose subscription is
                    // long-lived and was applied long before this player logged in, so a player's login
                    // replays nothing here and the rows an import writes land while this callback is not
                    // yet registered. It fires only on a real post-registration insert.
                    if rep_gate.lock().unwrap().contains(&row.faction_id) {
                        return;
                    }
                    // Send the Faction.dbc ReputationListID (the small 0..63 index the client addresses), NOT
                    // faction_id — sending the id crashed the client. A `< 0` row has no rep bar → skip (stale
                    // pre-migration -1 rows self-heal: the next grant re-stamps the real index).
                    if row.reputation_index >= 0 {
                        if let Some((opcode, body)) = codec::build_set_faction_standing_raw(
                            row.reputation_index as u32,
                            row.standing,
                        ) {
                            let _ = rep_ins_tx.send(Outbound::Raw { opcode, body });
                        }
                    }
                });
        let rep_upd_tx = tx.clone();
        let on_rep_update =
            self.0
                .coord()
                .conn
                .db
                .game_player_reputation()
                .on_update(move |_ctx, _old, row| {
                    if row.character_guid != self_guid {
                        return;
                    }
                    if row.reputation_index >= 0 {
                        if let Some((opcode, body)) = codec::build_set_faction_standing_raw(
                            row.reputation_index as u32,
                            row.standing,
                        ) {
                            let _ = rep_upd_tx.send(Outbound::Raw { opcode, body });
                        }
                    }
                });

        // Explored areas (200), SCOPED TO THIS CHARACTER — 292 second pass. `game_character_explored`
        // is `public` with NO RLS filter (`module/src/exploration.rs`: "the gateway filters its
        // `on_insert` by `character_guid == self`; a per-recipient RLS is a scaling follow-up"), so an
        // unfiltered `SELECT *` made every session cache all 2,045 rows — every OTHER character's map
        // fog — to use only its own handful. Scoping the QUERY (the AOI-tracker mechanism, `aoi.rs`)
        // gets the same row reduction as an RLS filter would, entirely inside the gateway: no schema
        // migration, no binding lockstep, and the COORDINATOR's global subscription
        // (`connection.rs:239`, the 279 fog-word guarantee leg) is untouched by construction rather
        // than by relying on the owner token's RLS bypass.
        //
        // Why not the RLS filter: the table has no `owner_identity` column, so every deployed
        // `#[client_visibility_filter]` in the module (16 of them, all `<identity col> = :sender`) is
        // inapplicable — it would need either an end-appended identity column plus a restamp sweep and
        // a publish/restart lockstep, or a JOIN-shaped filter used nowhere in this tree. And NOTHING
        // offline validates RLS SQL: a filter naming a column that does not exist still prints
        // PREFLIGHT OK (measured), while a filter the node rejects at subscribe time fails
        // `subscribe()` below and breaks every login. Not a trade worth making for ~2k rows.
        //
        // `character_guid` is indexed (`by_character`), and the `character_guid == self_guid` guards in
        // the `on_insert` relays above become redundant — deliberately KEPT: they are what makes the
        // coordinator leg (same table, global sub, no RLS) correct, and re-widening this query must not
        // be able to leak another character's fog through the per-player leg.
        let explored_query =
            format!("SELECT * FROM game_character_explored WHERE character_guid = {self_guid}");

        // The per-player subscription set. `game_world_entity` is the ONLY spatial one: when AOI is on it
        // rides a SEPARATE grid-scoped subscription (the tracker, created below) and is OMITTED here; when
        // off it's a global SELECT * in this base set (the proven path). Everything else is unchanged.
        let base_queries: Vec<&str> = vec![
            // game_movement_event is gone (perf catalog 2.1) — peer movement rides the AOI-scoped
            // game_entity_motion box subscription instead. Nothing writes the table any more.
            // game_creature_move_event is gone — creature legs ride the AOI-scoped
            // game_creature_spline box subscription. Nothing writes the table any more.
            "SELECT * FROM game_combat_event",
            "SELECT * FROM game_dynamic_object",
            // Engagement rows (broadcast) → SMSG_ATTACKSTART/STOP combat-stance relay.
            "SELECT * FROM game_melee_attack",
            "SELECT * FROM game_xp_event",
            "SELECT * FROM game_levelup_event",
            // Explored areas (200) → the on_insert above sets the PLAYER_EXPLORED_ZONES fog word.
            // Own rows ONLY — see `explored_query` above; never re-widen this to a bare
            // `SELECT * FROM game_character_explored`.
            &explored_query,
            // Rest-state flips (196) → the on_insert below relays PLAYER_BYTES_2 (zzz + blue bar).
            "SELECT * FROM game_rest_state_event",
            "SELECT * FROM game_corpse",
            // Gameobject TEMPLATES (static, small — the insert/update relays join per-conn).
            // The GO INSTANCE table is grid-scoped through the AOI tracker when AOI is on
            // (246 — the global sub shipped ~4.6k zone GOs to every client: the login burst
            // AND the felt frame hitch); AOI-off keeps the proven global query below.
            // ⚠ DO NOT re-add the select-star subscription for `game_gameobject_template` (292).
            // 1,411 rows of static display data that the COORDINATOR already subscribes once for
            // the whole gateway; all three consumers (the GO create/update relays and the
            // resident sweep) read it from there now. Templates are display fields, never a
            // visibility gate, so no instance/stealth scoping rides on this.
            //
            // No measurable RSS saving was observed (see the note at `go_ins_coord`) — this is
            // duplicate-cache hygiene, not the fix for the per-connection memory problem.
            "SELECT * FROM game_aura",
            "SELECT * FROM game_player_skill",
            // Pet bar spells (023): tiny static table joined when the viewer's own pet CREATE
            // relays (the Imp's Firebolt in SMSG_PET_SPELLS slots 3–6).
            "SELECT * FROM game_creature_cast",
            // Resurrection accept-prompt (#014): the player's own pending offer (RLS-scoped).
            "SELECT * FROM game_resurrect_request",
            // Own items (RLS-scoped) so vendor buy/sell + loot reflect in the bag without a relog.
            "SELECT * FROM game_item_instance",
            // ⚠ DO NOT re-add `SELECT * FROM game_item_template` here (work-item 292). It was in
            // this list until 2026-07-30 with the note "it's the same small table the coordinator
            // already holds". The coordinator half is true — it is one of the 61 coordinator
            // queries — but the table is 17,720 rows x 32 columns, and this list runs on EVERY
            // player connection, so each session materialised its own copy of the whole item
            // catalogue. MEASURED, 20 connections, same fixture: 283MB -> 1236MB with it
            // (~48MB/connection), 283MB -> 503MB without (~11MB/connection). At 500 players that
            // is ~24GB versus ~5.5GB, on a 31GB box — see 292 and #285.
            //
            // Nothing on the player connection read it: every consumer — reads.rs (char-select
            // gear), the durability/container-slot lookup below, and armor::effective_armor /
            // sheet_stats via the item-instance callback — runs on the COORDINATOR's db, which
            // subscribes the catalogue once for the whole gateway.
            "SELECT * FROM game_spell_cast_event",
            // Deferred projectile-impact damage log (#084) — a SEPARATE table from game_spell_cast_event
            // (never touches the on_cast cast-visual relay above).
            "SELECT * FROM game_spell_impact_event",
            "SELECT * FROM game_chat_event",
            "SELECT * FROM game_channel_event",
            "SELECT * FROM game_channel_member",
            "SELECT * FROM game_emote_event",
            "SELECT * FROM game_roll_event",
            "SELECT * FROM game_whisper_event",
            // Group notifications (066): the player's own invite/roster/decline/destroy events
            // (RLS-scoped, the whisper pattern).
            "SELECT * FROM game_group_event",
            // Quest-log relay (Phase 2): the player's own quest rows (RLS-scoped). The public
            // objective rows the `complete` state is computed from are read on the COORDINATOR.
            "SELECT * FROM game_character_quest",
            // ⚠ DO NOT re-add `SELECT * FROM game_quest_objective` here (work-item 292, second
            // pass). 396 rows x 6 narrow columns — a SMALL win (tens of KB per session, not the
            // MBs #124's item catalogue was worth), taken because it is provably dead weight on
            // this connection, not because it moves RSS much.
            // Every reader resolves to the coordinator's cache, which subscribes this table for
            // the whole gateway at `connection.rs:290`:
            //   - `reads.rs::quest_objectives_complete` / `viewer_needs_quest_item` /
            //     `quest_detail_view` / `synthesized_objectives` / `build_quest_log_slots` — all
            //     take `&RemoteTables` and every caller passes `self.0.coord()`'s db
            //     (`corpse_loot`, `quest_detail`, `player_quest_log`).
            //   - `quest_log_sync` + the kill-progress diff in THIS file read `ctx.db` inside
            //     callbacks registered on `self.0.coord().conn.db.game_character_quest()`, and
            //     `item_gain_feedback`'s COLLECT_ITEM toast likewise runs from the coordinator's
            //     `game_item_instance` callbacks (both moved there by 279).
            // Nothing registers a row callback on `player.conn.db.game_quest_objective()`, so
            // dropping the query cannot silence a relay — unlike `game_aura`,
            // `game_player_skill` and `game_gameobject_template`, which MUST stay (their
            // callbacks/`entry().find()` reads live on the player connection).
            // Teleport relay (#11): the player's own pending teleports (RLS-scoped) → MSG_MOVE_TELEPORT_ACK.
            "SELECT * FROM game_teleport_event",
            // Reputation relay (#13): the player's own standings (RLS-scoped) → SMSG_SET_FACTION_STANDING.
            "SELECT * FROM game_player_reputation",
        ];
        // #468: the four box-scoped tables (`game_world_entity`, `game_gameobject`,
        // `game_entity_motion`, `game_creature_spline`) are deliberately ABSENT from this list in
        // BOTH modes. They ride ONE global subscription per shard on the coordinator connection and
        // are routed to sessions by the in-process cell index (`stdb::world_view`). Re-adding any of
        // them here would put ~600 copies of the largest table in the gateway back on the heap and
        // restore the per-connection wakeup this issue removed.
        //
        // Apply the base subscription and block.
        let (atx, arx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
        let atx_err = atx.clone();
        let sub = player
            .conn
            .subscription_builder()
            .on_applied(move |_ctx| {
                let _ = atx.send(Ok(()));
            })
            .on_error(move |_ctx, err| {
                let _ = atx_err.send(Err(format!("{err}")));
            })
            .subscribe(base_queries);
        match arx.recv_timeout(Duration::from_secs(15)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(anyhow!("player subscription error: {e}")),
            Err(_) => return Err(anyhow!("player subscriptions not applied within 15s")),
        }

        // Freeze the login-apply item snapshot (the cache is fully applied once the ack lands
        // above; the row callbacks that replay these guids arrive after and get suppressed).
        {
            let mut set = initial_item_guids.lock().unwrap();
            set.extend(player.conn.db.game_item_instance().iter().map(|i| i.guid));
        }
        {
            let mut set = initial_rep_factions.lock().unwrap();
            set.extend(
                player
                    .conn
                    .db
                    .game_player_reputation()
                    .iter()
                    .filter(|r| r.character_guid == self_guid)
                    .map(|r| r.faction_id),
            );
        }
        {
            // Exploration (200/#41): OPEN the discovery gate, seeded with the area_bits the character
            // has already explored (whether they arrived by login or by a cross-database import — the
            // cache is written before the replay callbacks run, so both are in here). Their on_insert
            // replay therefore skips the "Discovered" popup while the fog VALUES still fires for all.
            // Only a genuinely new area_bit, discovered after this point, toasts.
            explored_replay.lock().unwrap().open(
                player
                    .conn
                    .db
                    .game_character_explored()
                    .iter()
                    .filter(|r| r.character_guid == self_guid)
                    .map(|r| r.area_bit),
            );
        }
        {
            // Rest state (196): open the rest gate — the historical events replayed above are history
            // (the login rest byte rides the CREATE). Only post-login inn crossings relay PLAYER_BYTES_2.
            rest_replay.lock().unwrap().open([]);
        }

        // #468: register with the gateway-wide shared view, then sweep everything already inside
        // the box into this client's world.
        //
        // The sweep is not belt-and-braces here, it is the ONLY thing that populates a fresh
        // client: there is no subscription apply to fire per-row `on_insert` any more — the rows
        // were resident in the coordinator caches long before this session existed. It is
        // idempotent through `created` regardless.
        // Seed the ghost mirror from the character's OWN row before anything can read it. Pre-#468
        // the spirit-healer gate re-read the live row on every offer, so a player who logged in
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
            viewer_gates.is_ghost.store(is_ghost, std::sync::atomic::Ordering::Relaxed);
        }
        let viewer = Arc::new(Viewer {
            session,
            self_guid,
            instance_id: login_instance,
            map_id: login_map,
            tx: tx.clone(),
            created: created.clone(),
            gates: viewer_gates.clone(),
        });
        view.add_viewer(
            viewer.clone(),
            CellKey::of_position(login_map, login_instance, login_x, login_y),
        );
        world_view::sweep_into_view(&view, &viewer);

        // Corpse resident sweep (190 slice 2 review MEDIUM): corpse rows ride the base subscription
        // and their callbacks may have fired before this point. Re-offer every resident one through
        // the same instance gate — idempotent for the client (a repeat CREATE for the same corpse
        // guid re-renders it), and the gate is now the session's authoritative `login_instance`
        // rather than a cache lookup that could answer `None`.
        let resident_corpses: Vec<Corpse> = player.conn.db.game_corpse().iter().collect();
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
        // per-player cache lost `game_item_template` to #292 and has never held the entity since
        // #468).
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

        // One teardown per callback registered above. Adding a relay = register it + push its
        // teardown here; the struct/empty()/Drop stay untouched.
        // 279: these teardown handles all target the COORDINATOR db (their callbacks were
        // moved off the churning per-player conn — see each registration's comment).
        let (td_xp, td_lvl, td_ii, td_id, td_iu, td_qi, td_qu, td_qd, td_ri, td_ru, td_am, td_ex) = (
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
        );
        let teardowns: Vec<Box<dyn FnOnce(&PlayerConn) + Send>> = vec![
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_combat_event().remove_on_insert(on_combat);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_melee_attack()
                    .remove_on_insert(on_melee_insert);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_melee_attack()
                    .remove_on_delete(on_melee_delete);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_xp.0.coord();
                l.conn.db.game_xp_event().remove_on_insert(on_xp);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_lvl.0.coord();
                l.conn.db.game_levelup_event().remove_on_insert(on_levelup);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_player_skill()
                    .remove_on_insert(on_skill_insert);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_player_skill()
                    .remove_on_update(on_skill_update);
            }),
            // Issue #89: this one was missing. Every world entry registered ANOTHER copy of
            // on_explored_insert on the same table without ever removing the last one, so a single
            // live discovery fired the "Discovered: <area>" toast once per accumulated registration
            // (the coordinator-side sibling below, on_explored_coord, was already torn down — this
            // was the one exception in the whole file).
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_character_explored()
                    .remove_on_insert(on_explored_insert);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_dynamic_object()
                    .remove_on_insert(on_dynobj_insert);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_dynamic_object()
                    .remove_on_delete(on_dynobj_delete);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_corpse().remove_on_insert(on_corpse_insert);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_corpse().remove_on_delete(on_corpse_delete);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_corpse().remove_on_update(on_corpse_update);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_resurrect_request()
                    .remove_on_insert(on_resurrect_insert);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_spell_cast_event().remove_on_insert(on_cast);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_spell_impact_event()
                    .remove_on_insert(on_impact);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_chat_event().remove_on_insert(on_chat);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_channel_event().remove_on_insert(on_channel);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_emote_event().remove_on_insert(on_emote);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_roll_event().remove_on_insert(on_roll);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_whisper_event().remove_on_insert(on_whisper);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_group_event()
                    .remove_on_insert(on_group_event);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_aura().remove_on_insert(on_aura_insert);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_aura().remove_on_update(on_aura_update);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn.db.game_aura().remove_on_delete(on_aura_delete);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_ii.0.coord();
                l.conn
                    .db
                    .game_item_instance()
                    .remove_on_insert(on_item_insert);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_id.0.coord();
                l.conn
                    .db
                    .game_item_instance()
                    .remove_on_delete(on_item_delete);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_iu.0.coord();
                l.conn
                    .db
                    .game_item_instance()
                    .remove_on_update(on_item_update);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_qi.0.coord();
                l.conn
                    .db
                    .game_character_quest()
                    .remove_on_insert(on_quest_insert);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_qu.0.coord();
                l.conn
                    .db
                    .game_character_quest()
                    .remove_on_update(on_quest_update);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_qd.0.coord();
                l.conn
                    .db
                    .game_character_quest()
                    .remove_on_delete(on_quest_delete);
            }),
            // 277: the teleport callback lives on the COORDINATOR db (see its registration) —
            // remove it there; the &PlayerConn arg is unused for this one.
            Box::new(move |_c: &PlayerConn| {
                tele_coord
                    .0
                    .coord()
                    .conn
                    .db
                    .game_teleport_event()
                    .remove_on_insert(on_teleport);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_ri.0.coord();
                l.conn
                    .db
                    .game_player_reputation()
                    .remove_on_insert(on_rep_insert);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_ru.0.coord();
                l.conn
                    .db
                    .game_player_reputation()
                    .remove_on_update(on_rep_update);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_am.0.coord();
                l.conn.db.game_addon_message().remove_on_insert(on_addon);
            }),
            Box::new(move |_c: &PlayerConn| {
                let l = td_ex.0.coord();
                l.conn
                    .db
                    .game_character_explored()
                    .remove_on_insert(on_explored_coord);
            }),
            Box::new(move |c: &PlayerConn| {
                c.conn
                    .db
                    .game_rest_state_event()
                    .remove_on_insert(on_rest_insert);
            }),
        ];
        // #22: the realm-core group callback lives on ANOTHER DATABASE's coordinator connection, so
        // it is torn down against that handle (the `&PlayerConn` arg is unused, exactly as for the
        // teleport/XP coordinator relays above). Registered only on a multi-database gateway, so on
        // a single-database one this pushes nothing and the teardown list is unchanged.
        //
        // Leaking this one would be worse than leaking a shard callback: the realm-core connection
        // outlives every session, so a stale closure would keep relaying a logged-out player's party
        // packets into a dead channel for the gateway's whole lifetime, once per relogin.
        let mut teardowns = teardowns;
        if let (Some(realm), Some(handle)) = (realm_group_relay.clone(), on_realm_group) {
            teardowns.push(Box::new(move |_c: &PlayerConn| {
                realm
                    .0
                    .coord()
                    .conn
                    .db
                    .game_group_event()
                    .remove_on_insert(handle);
            }));
        }
        // #22 (whisper slice): the same teardown, for the callback registered one table over. Leaking
        // it is the worse half of the pair — a stale closure on a connection that outlives every
        // session would keep pushing a logged-out player's PRIVATE whisper lines into a dead channel,
        // once more per relogin, for the gateway's whole lifetime.
        if let (Some(realm), Some(handle)) = (realm_group_relay, on_realm_whisper) {
            teardowns.push(Box::new(move |_c: &PlayerConn| {
                realm
                    .0
                    .coord()
                    .conn
                    .db
                    .game_whisper_event()
                    .remove_on_insert(handle);
            }));
        }

        Ok(PlayerSubscriptions {
            conn: Some(player),
            sub: Some(sub),
            teardowns,
            viewer: Some(viewer),
            view: Some(view),
        })
    }
}

impl Coordinator {
    /// Wire up the bot-initiated (serendipity) invite relay (issue #54): one registration per
    /// connected WORLD SHARD — `all_shards()`, not a per-player relay — because there is no player
    /// session to hang this off. A bot's goal tick decides "invite this fellow quester" with no
    /// client behind it, so nothing else in the gateway would ever notice the row.
    ///
    /// Called ONCE, at gateway startup (`main.rs`). Also installs the RE-ARM hook
    /// (`CoordinatorInner::on_reconnect`, `connection.rs`) that keeps it alive across a coordinator
    /// reconnect: every OTHER coordinator-level relay in this file re-registers itself implicitly,
    /// because it is armed from inside a per-player LOGIN, and a login happens again after any
    /// reconnect. This one has no login to hang a re-arm off — it runs once at startup — so without
    /// the hook a reconnect (which the watchdog's own doc comment calls out as covering "a
    /// SpacetimeDB migration", i.e. a routine module republish, not a rare event on this project)
    /// would leave it registered on a dead, disconnected `LiveConn` forever.
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
                if let Err(e) =
                    crate::world::party::run_bot_invite(&store, row.inviter_guid, row.target_guid)
                {
                    log::debug!(
                        "playerbots: serendipity intent {} ({} -> {}) did not execute: {e:#}",
                        row.id,
                        row.inviter_guid,
                        row.target_guid
                    );
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            cell: lyracore_shared::spatial::grid_cell_id(0, 0),
        }
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
        // instance isolation, work-item 190).
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
    fn run_speed_mult_bp_change_emits_force_run_speed_change_with_the_derived_speed() {
        // GM playtest `.speed` (work-item 223): a run_speed_mult_bp change relays
        // SMSG_FORCE_RUN_SPEED_CHANGE with speed = BASE_RUN_SPEED (7.0) * bp/10000 — pin the derived
        // value end-to-end (this is the "codec pin for the speed message" the work item calls for).
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

    // ---- Work-item 224: cross-map teleport relay --------------------------------------------

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

    // ---- Work-items 144/145/190: AOI re-entry, initial-apply, instance isolation --------------
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
        // 145 login sweep and the 144 re-entry path BOTH sit behind — a cross-instance row must
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
        // The exact scenario 144 reports: observer sees peer (CREATE) -> peer leaves the AOI box
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
        // The 145 scenario: a fresh login's AOI box already contains peers/creatures (rows
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
            "every pre-existing resident must get a CREATE at login, not just self (145's bug: \
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
        // (work-item 190 slice 1: always a no-op today since every entity is instance 0, but the
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
        // Open-world rows to an open-world viewer: byte-identical to pre-190 behavior (always relayed).
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
        // not even instance 0: suppress-then-sweep, never guess (190 slice 2 review MEDIUM).
        assert!(!instance_relay_gate(0, None));
        assert!(!instance_relay_gate(7, None));
    }

    // -------------------------------------------------------------------------------------------
    //  Issue #41 — the shard transfer must not re-announce every explored area
    // -------------------------------------------------------------------------------------------

    /// One `game_character_explored` row. `id` is the `auto_inc` surrogate the destination RE-MINTS
    /// on import — the field the old gate keyed on, and the whole of the defect.
    fn explored(id: u64, area_bit: i32, area_id: u32, experience: u32) -> CharacterExplored {
        CharacterExplored {
            id,
            character_guid: 7,
            area_bit,
            area_id,
            experience,
        }
    }

    /// The reported live symptom, end to end through the REAL relay decision: a character arrives on
    /// another database, its explored rows are re-inserted with FRESH ids, and not one of them may
    /// toast "Discovered: <area> — N experience gained".
    #[test]
    fn an_imported_explored_row_with_a_re_minted_id_emits_no_discovery_packet_issue_41() {
        let login = [explored(1, 40, 87, 120), explored(2, 87, 12, 60)];
        let gate = Mutex::new(ReplayGate::default());

        // Login / initial apply: the replay callbacks and the snapshot race (see `ReplayGate`), so
        // assert BOTH orders. Replay-first — the order that produced the live symptom:
        for row in &login {
            assert!(
                discovery_packet(&gate, row).is_none(),
                "the initial-apply replay must never toast, gate closed"
            );
        }
        gate.lock().unwrap().open(login.iter().map(|r| r.area_bit));

        // Now the transfer: the SAME areas arrive as brand-new rows with re-minted ids (and, since
        // `experience` stores the XP granted at the ORIGINAL discovery, a convincingly wrong number).
        for row in [explored(9001, 40, 87, 120), explored(9002, 87, 12, 60)] {
            assert!(
                discovery_packet(&gate, &row).is_none(),
                "a re-minted row for an already-explored area_bit is not a discovery (issue #41)"
            );
        }

        // Snapshot-first (the other side of the race) must reach the same verdict.
        let gate = Mutex::new(ReplayGate::default());
        gate.lock().unwrap().open(login.iter().map(|r| r.area_bit));
        for row in [explored(9001, 40, 87, 120), explored(9002, 87, 12, 60)] {
            assert!(discovery_packet(&gate, &row).is_none());
        }
    }

    /// …while a GENUINELY new area still toasts, exactly once, carrying its own area/XP.
    #[test]
    fn a_fresh_discovery_still_emits_its_packet_once_issue_41() {
        let gate = Mutex::new(ReplayGate::default());
        gate.lock().unwrap().open([40, 87]);

        let fresh = explored(9003, 108, 9, 155);
        let Some(Outbound::Raw { opcode, body }) = discovery_packet(&gate, &fresh) else {
            panic!("a never-before-explored area_bit must toast a discovery packet");
        };
        let (want_op, want_body) = codec::build_exploration_experience_raw(9, 155);
        assert_eq!((opcode, body), (want_op, want_body));

        // Idempotent: the same area arriving again (a later transfer, a duplicated relay) is silent.
        assert!(
            discovery_packet(&gate, &explored(9004, 108, 9, 155)).is_none(),
            "an area announced once must never announce again"
        );
    }

    /// The keyless half of the gate (rest state, 196): nothing relays until the snapshot is taken.
    #[test]
    fn the_replay_gate_admits_nothing_before_it_is_opened_issue_41() {
        let mut gate = ReplayGate::default();
        assert!(
            !gate.admit(),
            "a historical rest event replayed at login must not flip PLAYER_BYTES_2"
        );
        gate.open([]);
        assert!(gate.admit(), "a live inn crossing after login must relay");
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

    /// 292: the per-player subscription list must NEVER carry a static CATALOGUE table. This list
    /// runs once per session, so anything in it is materialised into every player's client cache;
    /// `game_item_template` alone (17,720 rows x 32 columns) measured ~37MB per connection, taking
    /// 20 connections from +220MB to +953MB. The coordinator subscribes these once for the whole
    /// gateway and every reader already uses that handle.
    ///
    /// Scanned on the comment-stripped source, so the DO-NOT-RE-ADD note at the call site cannot
    /// satisfy this test — the mutation it catches is someone pasting the query back in.
    #[test]
    fn the_per_player_subscription_carries_no_static_catalogue_table() {
        let code = scanned_source();
        for table in [
            "game_item_template",
            "game_gameobject_template",
            "game_spell",
            "game_creature_template",
            "game_quest_template",
            // 292 second pass. Not tens of MB (396 rows) — kept in this list because it is
            // provably dead weight on the player connection: no callback is registered on
            // `player.conn.db.game_quest_objective()` and every reader takes the coordinator's
            // cache. The mutation this catches is a future quest feature "needing objectives on
            // the player connection" and pasting the query back instead of reaching for
            // `self.0.coord()` the way `quest_log_sync` and `build_quest_log_slots` already do.
            "game_quest_objective",
        ] {
            assert!(
                !code.contains(&format!("\"SELECT * FROM {table}\"")),
                "the per-player subscription list has re-acquired the static world table \
                 `{table}`. That is a whole extra copy of it in EVERY session's client cache for \
                 data the coordinator already holds once — up to tens of MB each for the big \
                 catalogues (292, the likely mechanism behind #285's out-of-memory death at ~500 \
                 sessions), and dead weight even for the small ones. If a player-connection \
                 callback genuinely needs these rows, take `self.0.coord()`'s cache instead."
            );
        }
    }

    /// 292 second pass: the per-player `game_character_explored` subscription must stay SCOPED to the
    /// viewer's own character. The table is `public` with no RLS filter, so a bare `SELECT *` caches
    /// every character's map fog (2,045 rows at Elwynn scale) into every session to serve the viewer's
    /// own handful — and it grows with the character roster, not with the world.
    ///
    /// Both halves are needed, and both are real mutations. Only asserting the WHERE clause exists lets
    /// someone re-add the unfiltered query alongside it (a union subscription = the whole table back,
    /// silently); only asserting the unfiltered query is absent passes if the whole line is deleted,
    /// which would leave the fog relay with no rows at all — no login fog restore, ever.
    ///
    /// Scanned comment-stripped, so the DO-NOT-WIDEN notes at the call site cannot satisfy it.
    #[test]
    fn the_explored_subscription_is_scoped_to_the_viewers_own_character() {
        let code = scanned_source();
        assert!(
            code.contains(
                "let explored_query = format!(\"SELECT * FROM game_character_explored WHERE \
                 character_guid = {self_guid}\");"
            ),
            "the per-player game_character_explored subscription is no longer scoped to \
             `character_guid = {{self_guid}}` (292). Unscoped, every session caches every \
             character's explored rows — the whole fog table per connection — to read only its own."
        );
        assert!(
            !code.contains("\"SELECT * FROM game_character_explored\""),
            "the UNFILTERED `SELECT * FROM game_character_explored` is back in a gateway \
             subscription list in this file. The per-player leg must stay `character_guid`-scoped \
             (292); the global one belongs to the COORDINATOR only (connection.rs, the 279 \
             fog-word guarantee)."
        );
    }

    /// The span of ONE top-level `fn NAME(` in a DIFFERENT file of this module tree — #468 moved
    /// the AOI wiring out of this file, and a tripwire that can only see this file would have gone
    /// quiet rather than failing. Brace-matched, so it works on `impl`-nested layouts too. Returns
    /// the RAW (not comment-stripped) text — callers that need the mutation-hardened decommenting
    /// run it through [`decommented`] themselves, same two-step `scanned_source` already uses.
    fn top_level_fn_body_of(file: &str, name: &str) -> &'static str {
        let src: &'static str = match file {
            "world_view.rs" => include_str!("world_view.rs"),
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
    /// The session's `Viewer` must be built with the SAME dedup/gate state the per-player relays in
    /// this file capture — one `created` set, one `ViewerGates`. Hand either of them a fresh
    /// instance instead and the guarantee they exist for (exactly-once CREATE delivery, the
    /// spirit-healer ghost gate) is void, with nothing else failing.
    #[test]
    fn the_viewer_is_registered_with_the_shared_dedup_sets_and_gates() {
        let code = scanned_source();
        for field in [
            "created: created.clone(),",
            "gates: viewer_gates.clone(),",
        ] {
            assert!(
                code.contains(field),
                "the shared-view `Viewer` is no longer constructed with `{field}` — it now holds a \
                 private copy of state the per-player relays in this file still write to, so the two \
                 halves of every dedup/gate silently disagree"
            );
        }
        assert!(
            code.contains("view.add_viewer( viewer.clone(), CellKey::of_position(login_map, login_instance, login_x, login_y), );"),
            "the session is no longer registered with the shared AOI view at its LOGIN cell — \
             without the registration the client sees an empty world; with the wrong anchor it sees \
             somebody else's neighbourhood"
        );
    }


    /// Tripwire for the #22 (group slice) REALM-CORE relay — the only delivery path a multi-database
    /// gateway has for party packets, and one no fake in this tree can reach (it registers a callback
    /// on another database's live coordinator connection).
    ///
    /// Every clause here is a mutation that deletes the feature while leaving all 405 gateway tests
    /// green: without `is_sharded()` a single-database gateway registers a SECOND callback on the
    /// same table and every group packet is delivered twice; without `realm_core()` it listens to
    /// the wrong database and hears nothing; without the `recipient_guid` self-filter the
    /// owner-token connection relays EVERY player's party packets to EVERY session (the RLS the
    /// per-player relay leans on does not exist here); and without the teardown the callback outlives
    /// the session on a connection that never closes.
    #[test]
    fn the_realm_core_group_relay_is_gated_filtered_and_torn_down() {
        let code = scanned_source();
        assert!(
            code.contains("let realm_group_relay = self .is_sharded() .then(|| self.realm_core().ok()) .flatten()"),
            "the realm-core group relay is no longer gated on `is_sharded()` + `realm_core()`. \
             Unsharded, it registers a SECOND callback on a table the per-player relay already \
             watches and every group packet is delivered twice; pointed at any other database, it \
             hears nothing at all."
        );
        assert!(
            code.contains(".filter(|rc| Coordinator::shard_name(rc) != Coordinator::shard_name(self));"),
            "the realm-core group relay lost its DIFFERENT-DATABASE guard. With `LYRACORE_SHARD_MAP` set \
             and `LYRACORE_REALM_CORE` unset, `realm_core()` answers the DEFAULT database — so a session \
             already on it would receive every invite dialog and roster refresh twice: once here \
             through the owner token, once through the per-player RLS relay above."
        );
        // Pinned as the FIRST statement of the callback, not merely as a substring somewhere in the
        // file. Verified by mutation (review of PR #49): wrapping this exact line in `if false { … }`
        // left all 410 tests green while the owner-token relay delivered every player's invite
        // dialogs and party frames to every session — the same dead-binding/decoy trick that has
        // defeated five of this batch's source scans. Anchoring to the closure's opening brace is
        // what makes a wrapper, a `let`, or a preceding statement visible.
        assert!(
            code.contains(
                ".game_group_event().on_insert(move |_ctx, row| { if row.recipient_guid != self_guid { return; }"
            ),
            "the realm-core group relay lost its per-session `recipient_guid` filter, or the filter \
             is no longer the FIRST thing the callback does. It reads through the OWNER TOKEN, which \
             bypasses RLS, so without this every session receives every player's invites and party \
             frames — a privacy leak and a scrambled party UI."
        );
        assert!(
            code.contains(
                "realm .0 .coord() .conn .db .game_group_event() .remove_on_insert(handle);"
            ),
            "the realm-core group callback is no longer torn down. It lives on a connection that \
             outlives every session, so each relogin would leave another closure relaying a \
             logged-out player's party packets into a dead channel, forever."
        );
    }

    /// Tripwire for the #22 (WHISPER slice) realm-core relay — the ONLY delivery path a
    /// multi-database gateway has for a whisper between two players on different databases, and one
    /// no fake in this tree can reach (it registers a callback on another database's live coordinator
    /// connection).
    ///
    /// Each clause is a mutation that removes the feature with every other gateway test green:
    /// delete the registration and a cross-shard whisper is written on realm-core and delivered to
    /// nobody; drop the `recipient_guid` self-filter and the owner-token connection hands EVERY
    /// player's private whispers to EVERY session (there is no RLS on this read — the whisper rows on
    /// realm-core are addressed to `Identity::ZERO`, which is the point); drop the teardown and a
    /// logged-out player's whisper lines keep being pushed into a dead channel, once more per relogin,
    /// on a connection that never closes.
    ///
    /// The gate itself is not re-scanned here: this relay deliberately reuses `realm_group_relay`, so
    /// the `is_sharded()` + `realm_core()` + different-database clauses above cover both. What IS
    /// pinned is that it reuses it — a copy that re-derived the handle could lose the guard.
    #[test]
    fn the_realm_core_whisper_relay_is_gated_filtered_and_torn_down() {
        let code = scanned_source();
        // Anchored to the callback's opening brace, not to a bare identifier: the group relay's own
        // filter scan was defeated (PR #49 review) by wrapping the scanned line in `if false { … }`,
        // and before that by a comment quoting it. Pinning the registration THROUGH the first
        // statement makes a wrapper, a `let`, or a preceding statement visible.
        assert!(
            code.contains(
                "let on_realm_whisper = realm_group_relay.as_ref().map(|realm| { let realm_tx = tx.clone(); \
                 realm .0 .coord() .conn .db .game_whisper_event() .on_insert(move |_ctx, row| { \
                 if row.recipient_guid != self_guid { return; }"
            ),
            "the realm-core whisper relay is gone, no longer rides the GATED `realm_group_relay` \
             handle (which carries the `is_sharded()` + different-database guard — without it a \
             session on the default database receives every whisper twice), or no longer self-filters \
             on `recipient_guid` as the FIRST thing its callback does. Unfiltered, this relay reads \
             through the OWNER TOKEN and hands every player's private whispers to every session."
        );
        // Pinned THROUGH the callback's closing `}) });`, so the two statements above are the WHOLE
        // body. A `contains` proves presence, never exclusivity: appending a second
        // `build_whisper(..)`/`send(..)` pair (a shadowed `let m`, which compiles and warns about
        // nothing) left every gateway test green while relaying each private line TWICE, once with
        // `is_inform` flipped — the seventh way a scan in this batch was defeated, and the mirror of
        // the `if false { … }` one. Closing the brace closes that door.
        assert!(
            code.contains(
                "let m = codec::build_whisper(row.other_guid, row.is_inform, row.message.clone()); \
                 let _ = realm_tx.send(Outbound::One(ServerOpcodeMessage::SMSG_MESSAGECHAT( \
                 Box::new(m), ))); \
                 }) });"
            ),
            "the realm-core whisper relay no longer BUILDS and SENDS the chat packet — or no longer \
             does exactly that and nothing else. A callback that decodes the row and drops it is the \
             same defect as no callback at all (this file's `discovery_packet` tripwire's own mutation \
             class); a callback that sends a SECOND, altered packet delivers every private whisper \
             line twice."
        );
        assert!(
            code.contains(
                "realm .0 .coord() .conn .db .game_whisper_event() .remove_on_insert(handle);"
            ),
            "the realm-core whisper callback is no longer torn down. It lives on a connection that \
             outlives every session, so each relogin leaves another closure pushing a logged-out \
             player's PRIVATE whisper lines into a dead channel, forever."
        );
    }

    /// Tripwire: the exploration relay must route its popup through [`discovery_packet`], which is
    /// the only thing the tests above can see, AND must actually SEND what it returns. Deleting the
    /// call and inlining the codec would restore the defect with every test above still green, so
    /// the ONE remaining call site of `build_exploration_experience_raw` in this file has to be the
    /// gated one — and the gated one has to reach the wire (mutation: `let _ = discovery_packet(..)`
    /// kept the identifier and killed the feature, green).
    #[test]
    fn the_discovery_popup_has_exactly_one_gated_call_site_issue_41() {
        let body = scanned_source();
        assert_eq!(
            body.matches("build_exploration_experience_raw").count(),
            1,
            "the \"Discovered\" popup must be built in `discovery_packet` and nowhere else — an \
             ungated second call site is the #41 defect wearing a different hat"
        );
        assert_eq!(
            body.matches("discovery_packet(&explored_gate, row)").count(),
            1,
            "the game_character_explored on_insert relay must ask `discovery_packet` whether to toast"
        );
        assert!(
            body.contains(
                "if let Some(out) = discovery_packet(&explored_gate, row) { let _ = explored_tx.send(out); }"
            ),
            "`discovery_packet`'s answer must be SENT — asking the gate and dropping the packet \
             leaves every test in this module green while no player ever sees a \"Discovered\" line \
             again (the inverse of #41, and the mutation that survived the identifier-only scan)"
        );
    }

    /// Tripwire for the OTHER half of the wiring, and the one mutation the behavioural tests above
    /// cannot see: the gates have to be OPENED after the subscription ack, seeded from the client
    /// cache's `area_bit`s. Deleting either `open` call leaves every test green while silently
    /// killing the feature it protects (no discovery ever toasts again; no inn crossing ever
    /// relays) — the inverse defect of #41, and just as invisible.
    ///
    /// AFTER is the whole point, and counting occurrences never checked it: hoisting both `open`
    /// calls to just BEFORE `.subscribe()` keeps all three counts at one and reinstates #41 in full
    /// — the gate is open, the seed is read from an empty cache, and the initial apply toasts every
    /// explored area again. So this asserts source ORDER against the ack wait, not just presence.
    ///
    /// Note: still a source scan — the weakest form of tripwire. It is what is available:
    /// opening the gate happens inside `subscribe_player_events`, which needs a live SpacetimeDB
    /// connection, so no test in this crate can execute it. It reads [`scanned_source`], so a
    /// comment quoting the call no longer satisfies it. The behavioural half (`ReplayGate` +
    /// `discovery_packet`) is covered for real above.
    #[test]
    fn both_replay_gates_are_opened_after_the_subscription_ack_issue_41() {
        let body = scanned_source();
        assert_eq!(
            body.matches("explored_replay.lock().unwrap().open(")
                .count(),
            1,
            "the discovery gate must be opened once, after the subscription ack"
        );
        assert_eq!(
            body.matches(".map(|r| r.area_bit),").count(),
            1,
            "the discovery gate must be seeded with area_bits — the key that survives the \
             destination's auto_inc re-mint (issue #41); seeding it with row ids is the defect"
        );
        assert_eq!(
            body.matches("rest_replay.lock().unwrap().open(").count(),
            1,
            "the rest-state gate must be opened once, after the subscription ack"
        );
        // ORDER. The ack wait is the line that both `open` calls must follow.
        let ack = body
            .find("arx.recv_timeout(")
            .expect("the base subscription's ack wait — the thing the gates are opened after");
        for (needle, gate) in [
            ("explored_replay.lock().unwrap().open(", "discovery"),
            ("rest_replay.lock().unwrap().open(", "rest-state"),
        ] {
            assert!(
                body.find(needle).expect("asserted present above") > ack,
                "the {gate} gate is opened BEFORE the subscription ack. Then the seed is read from \
                 a cache the initial apply has not been written into yet, the gate is already open \
                 when the replay callbacks fire, and every already-explored area toasts again — \
                 issue #41 exactly, with every count in this test still equal to one"
            );
        }
    }

    // -------------------------------------------------------------------------------------------
    //  Issue #89 — every player-connection callback registered here must be torn down
    // -------------------------------------------------------------------------------------------
    //
    // The actual #89 defect: `on_explored_insert` (game_character_explored, the toast relay) was
    // registered on EVERY world entry and never removed from the teardown list — its coordinator-side
    // sibling `on_explored_coord` had one, this one did not. Each accumulated registration is another
    // live callback on the same table, so one discovery fired the "Discovered: <area>" toast once per
    // registration; the XP relay (`on_xp`) IS torn down, so it fired exactly once — precisely the
    // asymmetry the operator reported ("two popups, XP once").

    /// Every `let <ident> = <expr>.on_insert(`/`.on_update(`/`.on_delete(...);` REGISTRATION in `text`
    /// (the statement bounded by the first `;` after `<ident> =` — true for every DIRECT registration
    /// in this file, which all call the event method immediately after the `=`) that has no matching
    /// `remove_on_insert(<ident>)` / `remove_on_update(<ident>)` / `remove_on_delete(<ident>)` call
    /// anywhere else in `text`. Returns each unpaired registration as `"<ident> (<method>)"`.
    ///
    /// Does NOT see the two `.map(|realm| { ... })`-wrapped registrations (`on_realm_group`,
    /// `on_realm_whisper`) — their teardown sits behind an `Option`, not a plain `let`, so they don't
    /// fit this statement shape. Both are already covered by their own dedicated tripwires:
    /// `the_realm_core_group_relay_is_gated_filtered_and_torn_down` and
    /// `the_realm_core_whisper_relay_is_gated_filtered_and_torn_down`, above.
    fn unpaired_registrations(text: &str) -> Vec<String> {
        let mut missing = Vec::new();
        let mut search_from = 0usize;
        while let Some(rel) = text[search_from..].find("let ") {
            let ident_start = search_from + rel + 4;
            let ident_end = text[ident_start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map(|o| ident_start + o)
                .unwrap_or(text.len());
            let ident = &text[ident_start..ident_end];
            search_from = ident_end;
            if ident.is_empty() {
                continue; // `let (a, b, ...) = ...` / `let mut ...` — not a single bound name
            }
            let after = text[ident_end..].trim_start();
            if !after.starts_with('=') || after.starts_with("==") {
                continue; // e.g. `let x: SomeType = ...` — the `=` isn't immediately after the ident
            }
            let stmt_end = text[ident_end..]
                .find(';')
                .map(|o| ident_end + o)
                .unwrap_or(text.len());
            let stmt = &text[ident_end..stmt_end];
            let Some(method) = ["on_insert", "on_update", "on_delete"]
                .into_iter()
                .find(|m| stmt.contains(&format!(".{m}(")))
            else {
                continue; // not an event-callback registration
            };
            let teardown = format!("remove_{method}({ident})");
            if !text.contains(&teardown) {
                missing.push(format!("{ident} ({method})"));
            }
        }
        missing
    }

    /// Proves [`unpaired_registrations`] actually catches a missing teardown rather than being
    /// vacuously green (a scanner with an inverted or short-circuited check would pass the real audit
    /// below for the wrong reason). A synthetic snippet with one paired and one unpaired registration.
    #[test]
    fn the_teardown_scanner_flags_a_synthetic_missing_teardown() {
        let synthetic = "let on_paired = tbl.on_insert(move |_ctx, row| { touch(row); }); \
                          let on_leaked = tbl2.on_insert(move |_ctx, row| { touch(row); }); \
                          fn teardown() { tbl.remove_on_insert(on_paired); }";
        assert_eq!(
            unpaired_registrations(synthetic),
            vec!["on_leaked (on_insert)".to_string()],
            "the scanner must flag exactly the registration with no matching remove_on_insert call, \
             and not the paired one next to it"
        );
    }

    // ── Peer-motion relay: the CALL SITE, not just the codec helper (work-items 286 + 287) ──────────
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
    /// test did (which is exactly how the shed test below first turned the 287 test red). Poison is
    /// ignored deliberately: a panicking test has already failed and must not cascade into every
    /// other one.
    static MOTION_COUNTERS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Test shims for the two relays #468 split in half.
    ///
    /// Before #468 each of these pushed straight onto the session channel from the per-player SDK
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
    /// opcode and body equal what the TYPED path would have serialized (work-item 286). The typed
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

    /// **Work-item 287.** Both relays push with a discarding send; a session whose writer half is
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
            },
        );
        assert_eq!(
            MOTION_DROPPED.load(Relaxed),
            before + 2,
            "a creature leg discarded by a dead writer channel must increment MOTION_DROPPED too"
        );
    }

    /// The operator-facing half of 287: the 10-second line must carry the delivery ratio, the
    /// per-movement fan-out, and the dropped count — the three numbers the 371-client run had to
    /// reconstruct by hand afterwards. Pinned as text because that text IS the instrument.
    #[test]
    fn the_motionstat_line_reports_delivery_dropped_and_fanout_287() {
        // 1000 callbacks, 900 packets queued, 7 discarded, from 10 submitted movements.
        let line = motionstat_line(5000, 4500, 21, 1000, 900, 7, 10, 400, 380);
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
            line.contains("outstanding=20"),
            "the pre-existing move counters must survive: {line}"
        );

        // An idle window must not divide by zero or print a fake 0 %.
        let idle = motionstat_line(5000, 4500, 21, 0, 0, 0, 0, 400, 400);
        assert!(
            idle.contains("delivery=--") && idle.contains("fanout=--/move"),
            "{idle}"
        );
    }

    // ── Fan-out collapse signal (task B1, finishing 287) ────────────────────────────────────────────

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

    /// **The hysteresis, which is the deliberate divergence from the #109 signal.** A 40 %+ fan-out
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
    /// above green and freezes peers — the #109 shape, and the "grep every subscriber when a relay
    /// changes shape" hazard work-item 286 opens with. A scan (not a mock) for the reason the other
    /// tripwires in this file are: the callbacks are registered on a live `DbConnection`.
    /// Both halves of the motion relay (work-item 286, re-pinned for #468): `game_entity_motion` is
    /// UPSERTED per mover per heartbeat, so `on_insert` is a mover's first heartbeat in the world and
    /// `on_update` is essentially all peer movement. Dropping either registration is silent — peers
    /// freeze after their first step, or never move at all. Same for the creature-leg twin.
    #[test]
    fn both_halves_of_the_motion_and_spline_relays_are_registered_286() {
        let arm = decommented(top_level_fn_body_of("world_view.rs", "arm_shard"));
        for (table, half, what) in [
            ("game_entity_motion", "on_insert", "a mover's FIRST heartbeat"),
            ("game_entity_motion", "on_update", "every heartbeat after the first — i.e. all peer movement"),
            ("game_creature_spline", "on_insert", "a creature's first leg"),
            ("game_creature_spline", "on_update", "every leg after the first"),
        ] {
            assert!(
                arm.contains(&format!("{table}().{half}(")),
                "the {table} {half} half is no longer registered on the shared coordinator \
                 dispatch — {what} is silently dropped for every session"
            );
        }
        // And the dispatch routes them through the cell index rather than broadcasting.
        let m = decommented(top_level_fn_body_of("world_view.rs", "motion"));
        assert!(
            m.contains("view.entities.viewers_of(key)"),
            "peer motion no longer asks the cell index who can see the mover — either every session \
             gets every mover (the fan-out this issue removed) or none do"
        );
    }


    /// And the instrument's wiring (287): the counters are only ever read by the 10-second task in
    /// `world::run`. Without this scan, reverting that task to the old hand-formatted line — no
    /// delivery ratio, no dropped count — leaves the whole suite green, which is precisely how the
    /// 63 % under-delivery went unlogged in the first place.
    #[test]
    fn the_10s_task_prints_the_delivery_instrument_287() {
        let body = crate::test_scan::code_of(
            include_str!("../world/mod.rs"),
            "pub async fn run(cfg: GatewayConfig, coordinator: Coordinator) -> Result<()> {",
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
    /// that is the entire deliverable (computing the verdict and not printing it would be #99's
    /// "reported the call, not the effect" defect).
    #[test]
    fn the_10s_task_warns_on_a_fanout_collapse_b1() {
        let body = crate::test_scan::code_of(
            include_str!("../world/mod.rs"),
            "pub async fn run(cfg: GatewayConfig, coordinator: Coordinator) -> Result<()> {",
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

    /// The actual issue #89 audit: no player-connection callback registered in this file may be left
    /// without a teardown. Mutation (verified live, reverted with `Edit`): deleting the
    /// `on_explored_insert` teardown line this test exists to protect turns this test red with the
    /// exact identifier named in the failure message.
    #[test]
    fn every_registered_player_callback_has_a_teardown_issue_89() {
        let missing = unpaired_registrations(&scanned_source());
        assert!(
            missing.is_empty(),
            "the following callbacks are registered with no matching `remove_on_*` teardown — each \
             leaks another live subscription per world entry (or per session), and a single live \
             event then fires its relay once per accumulated registration — issue #89's exact defect, \
             reproduced for a different table: {missing:?}"
        );
    }
}
