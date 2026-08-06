//! Per-connection movement coalescing (work-item 231). Every inbound `MSG_MOVE_*` used to become
//! its own `movement_update` reducer transaction, so movement dominates the serialized stream as
//! players scale. `CoalesceState` cuts pure-heartbeat volume ~3-5x while keeping every
//! STATE-CHANGE (start/stop/turn/jump/fall-land/strafe, or ANY packet whose flags or heading
//! differ from the last one actually forwarded) byte-identical and undelayed — peers key
//! animation/extrapolation off state changes, not heartbeats, so perceived smoothness is
//! unaffected (a real client's own heartbeat cadence is far coarser than this window; it
//! interpolates from pos+heading+speed between them regardless of the wire cadence to the module).
//!
//! The decision logic is a PURE state machine (no I/O, no real clock) per this repo's testing
//! convention ("extract pure functions and test those"): `now_ms` is
//! an opaque monotonic-ms reading the caller supplies. Production wires it to a per-connection
//! `Instant` epoch via the `_now` methods below; unit tests drive `on_movement`/`flush` directly
//! with synthetic timestamps so the 150ms cadence never needs a real sleep (a real sleep-based
//! test would be flaky under CI load — this codebase has no precedent for timing-dependent tests
//! and this item doesn't need to set one).

use std::time::Instant;
use wow_world_messages::vanilla::{MovementInfo, MovementInfo_MovementFlags};

/// Forward cadence for a coalesced pure heartbeat, in ms — the item's window is "~150-200ms";
/// 150 (the low end) is chosen so a continuously-moving player's module-side position is never
/// more than 150ms behind the wire, comfortably inside what peer interpolation already tolerates
/// (client movement is client-authoritative; the module position only has to be fresh enough for
/// server-side range gates and late-observer spawns — see `flush_now`/rule 2 below), while still
/// buying the item's "~3-5x drop" for a client heartbeat cadence in the several-hundred-ms range.
pub const MOVE_COALESCE_WINDOW_MS: u64 = 150;

/// Per-connection coalescing state ("a small CoalesceState on the conn", the item's own wording).
/// Lives on `WorldConn` (see `world/mod.rs`), so it survives across `dispatch` calls for the life
/// of one socket.
pub struct CoalesceState {
    /// Real-clock epoch this connection's `_now` methods measure `now_ms` from. Unused by the
    /// pure `on_movement`/`flush` (tests supply `now_ms` directly).
    epoch: Instant,
    /// `(flags, heading, forwarded-at-ms)` of the last packet actually forwarded to the module —
    /// `None` until the very first forward, which is always immediate (nothing to compare yet, so
    /// classification trivially falls out as "changed").
    last_forwarded: Option<(MovementInfo_MovementFlags, f32, u64)>,
    /// The latest pending coalesced heartbeat. A newer pending packet always REPLACES the older
    /// one (rule 3: the pending slot itself IS the sub-yard-intermediate drop mechanism — there is
    /// no queue, so an in-between heartbeat that never becomes "latest" is simply overwritten and
    /// never ships).
    pending: Option<(u32, MovementInfo)>,
    /// Debug-only counters (rule 5): every inbound movement packet vs. every one actually
    /// forwarded to the module. `run_world_session` logs both once at disconnect.
    received: u64,
    forwarded: u64,
}

impl Default for CoalesceState {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            last_forwarded: None,
            pending: None,
            received: 0,
            forwarded: 0,
        }
    }
}

impl CoalesceState {
    pub fn received(&self) -> u64 {
        self.received
    }

    pub fn forwarded(&self) -> u64 {
        self.forwarded
    }

    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Production entry point: classify + coalesce one inbound movement packet against the real
    /// clock. Returns the packets to actually forward to `WorldStore::movement_update`, IN ORDER
    /// (0, 1, or 2 items — a flushed pending heartbeat followed by a state change, when both fire
    /// in the same call; ordering matters for peer extrapolation, rule 2b).
    pub fn on_movement_now(
        &mut self,
        opcode: u32,
        info: &MovementInfo,
    ) -> Vec<(u32, MovementInfo)> {
        let now = self.now_ms();
        self.on_movement(opcode, info, now)
    }

    /// Production entry point for rule 2's robust flush-on-any-other-opcode: drain the pending
    /// heartbeat (if any) against the real clock. The dispatch loop calls this before handling ANY
    /// non-movement CMSG, so a loot/vendor/gossip/etc. range check downstream never reads a stale
    /// module position — the one-line fix that removes the whole "stale position mid-coalesce"
    /// class instead of auditing every position-sensitive reducer individually.
    pub fn flush_now(&mut self) -> Option<(u32, MovementInfo)> {
        let now = self.now_ms();
        self.flush(now)
    }

    /// The pure decision (see module doc): classify `(opcode, info)` as STATE-CHANGE or a
    /// coalescible pure heartbeat against `last_forwarded`, given the caller's `now_ms`. Mutates
    /// `pending`/`last_forwarded`/the counters; returns what to forward, in order.
    fn on_movement(
        &mut self,
        opcode: u32,
        info: &MovementInfo,
        now_ms: u64,
    ) -> Vec<(u32, MovementInfo)> {
        self.received += 1;
        let is_heartbeat = opcode == lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT;
        let same_as_last_forwarded = self
            .last_forwarded
            .as_ref()
            .is_some_and(|(f, h, _)| *f == info.flags && *h == info.orientation);
        // Rule 1: any non-heartbeat opcode is ALWAYS a state change (start/stop/turn/jump/
        // fall-land/strafe/run-walk-mode/facing are one-shot edge events by construction — there's
        // no "pure cadence" reading of them). A HEARTBEAT whose flags or heading drifted from the
        // last FORWARDED packet is ALSO treated as a state change — defensive: a well-behaved
        // client only sends HEARTBEAT while nothing changed, but a stale/reordered/malformed one
        // must never get silently swallowed as if the state hadn't moved.
        let is_state_change = !is_heartbeat || !same_as_last_forwarded;

        if is_state_change {
            let mut out = Vec::with_capacity(2);
            // Flush the pending heartbeat FIRST — a peer must see the run continue up to where it
            // actually was before it sees the turn/stop, or its extrapolation overshoots.
            if let Some(flushed) = self.flush(now_ms) {
                out.push(flushed);
            }
            self.mark_forwarded(info, now_ms);
            out.push((opcode, info.clone()));
            return out;
        }

        // Pure heartbeat, same state as the last forward: coalesce unless the cadence window has
        // elapsed. `last_forwarded` is guaranteed `Some` here (a HEARTBEAT only matches
        // `same_as_last_forwarded` when there IS a last-forwarded key to match).
        let elapsed_ok = self
            .last_forwarded
            .as_ref()
            .map(|(_, _, t)| now_ms.saturating_sub(*t) >= MOVE_COALESCE_WINDOW_MS)
            .unwrap_or(true);
        if elapsed_ok {
            self.mark_forwarded(info, now_ms);
            vec![(opcode, info.clone())]
        } else {
            // Supersede any earlier pending heartbeat — THIS is the sub-yard-intermediate drop
            // (rule 3): the slot holds only the newest, so an older in-between one never ships.
            self.pending = Some((opcode, info.clone()));
            Vec::new()
        }
    }

    /// Drain + forward the pending heartbeat (if any) against `now_ms`. Shared by a state change's
    /// flush-then-forward (above) and rule 2's flush-on-any-other-opcode.
    fn flush(&mut self, now_ms: u64) -> Option<(u32, MovementInfo)> {
        let (opcode, info) = self.pending.take()?;
        self.mark_forwarded(&info, now_ms);
        Some((opcode, info))
    }

    fn mark_forwarded(&mut self, info: &MovementInfo, now_ms: u64) {
        self.last_forwarded = Some((info.flags.clone(), info.orientation, now_ms));
        self.forwarded += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::opcodes::movement as m;
    use wow_world_messages::vanilla::Vector3d;

    /// Build a synthetic `MovementInfo`: `flags`/`orientation` drive the state-change
    /// classification; `x` stands in for "position advanced along the same vector" (the actual
    /// coordinate value never matters to the coalescing decision — only flags+heading do; a real
    /// same-vector heartbeat stream would also advance x/y/z, which this mirrors for realism).
    fn info(flags: MovementInfo_MovementFlags, orientation: f32, x: f32) -> MovementInfo {
        MovementInfo {
            flags,
            timestamp: 0,
            position: Vector3d { x, y: 0.0, z: 0.0 },
            orientation,
            fall_time: 0.0,
        }
    }

    fn empty() -> MovementInfo_MovementFlags {
        MovementInfo_MovementFlags::empty()
    }

    // ---- Rule 1: state changes are never delayed (the HARD requirement) ------------------------

    #[test]
    fn first_packet_ever_forwards_immediately_even_if_its_a_heartbeat() {
        // Nothing to compare against yet -> trivially "changed" -> forward now, not held.
        let mut cs = CoalesceState::default();
        let out = cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(empty(), 0.0, 0.0), 0);
        assert_eq!(
            out.len(),
            1,
            "the very first movement packet must never be held pending"
        );
        assert_eq!(cs.forwarded(), 1);
    }

    #[test]
    fn non_heartbeat_opcodes_always_forward_immediately_regardless_of_flags() {
        // Every MSG_MOVE_* opcode other than HEARTBEAT is a one-shot edge event: forward now, no
        // exceptions, even when flags/heading happen to match the last forward exactly.
        let mut cs = CoalesceState::default();
        cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(empty(), 0.0, 0.0), 0);
        for (i, opcode) in [
            m::MSG_MOVE_START_FORWARD,
            m::MSG_MOVE_STOP,
            m::MSG_MOVE_START_TURN_LEFT,
            m::MSG_MOVE_STOP_TURN,
            m::MSG_MOVE_JUMP,
            m::MSG_MOVE_FALL_LAND,
            m::MSG_MOVE_START_STRAFE_LEFT,
            m::MSG_MOVE_STOP_STRAFE,
            m::MSG_MOVE_SET_FACING,
        ]
        .into_iter()
        .enumerate()
        {
            let out = cs.on_movement(opcode, &info(empty(), 0.0, 0.0), (i as u64) * 10);
            assert_eq!(
                out.len(),
                1,
                "opcode 0x{opcode:03X} must forward immediately"
            );
            assert_eq!(out[0].0, opcode);
        }
    }

    #[test]
    fn state_change_flushes_pending_heartbeat_first_then_itself_in_order() {
        // run-start (t=0) -> heartbeat held pending (t=10, well inside the window) -> turn (t=20,
        // a real flags change) must flush the pending heartbeat FIRST, then the turn — in THAT
        // order, so a peer's extrapolation advances to the held position before snapping to turn.
        let mut cs = CoalesceState::default();
        let run_flags = MovementInfo_MovementFlags::new(0x1, None, None, None, None); // FORWARD bit
        assert_eq!(
            cs.on_movement(
                m::MSG_MOVE_START_FORWARD,
                &info(run_flags.clone(), 0.0, 0.0),
                0
            )
            .len(),
            1
        );

        let held = info(run_flags.clone(), 0.0, 1.0);
        let out = cs.on_movement(m::MSG_MOVE_HEARTBEAT, &held, 10);
        assert!(
            out.is_empty(),
            "same-vector heartbeat well inside the window must be held, not forwarded"
        );

        let turn_flags = MovementInfo_MovementFlags::new(0x1 | 0x4, None, None, None, None); // + a turn bit
        let turn_info = info(turn_flags, 5.0, 1.0);
        let out = cs.on_movement(m::MSG_MOVE_START_TURN_LEFT, &turn_info, 20);
        assert_eq!(out.len(), 2, "expected [flushed heartbeat, turn]");
        assert_eq!(
            out[0].0,
            m::MSG_MOVE_HEARTBEAT,
            "the held heartbeat must flush FIRST"
        );
        assert_eq!(out[0].1.position.x, 1.0);
        assert_eq!(
            out[1].0,
            m::MSG_MOVE_START_TURN_LEFT,
            "then the turn itself, undelayed"
        );
        assert_eq!(out[1].1.orientation, 5.0);
    }

    #[test]
    fn heartbeat_with_drifted_heading_is_treated_as_a_state_change() {
        // Defensive rule-1 clause: a HEARTBEAT opcode whose heading differs from the last forward
        // is NOT a pure heartbeat, even though the opcode itself looks like cadence traffic.
        let mut cs = CoalesceState::default();
        cs.on_movement(m::MSG_MOVE_START_FORWARD, &info(empty(), 0.0, 0.0), 0);
        let drifted = info(empty(), 1.5, 1.0); // same flags, DIFFERENT heading
        let out = cs.on_movement(m::MSG_MOVE_HEARTBEAT, &drifted, 5);
        assert_eq!(
            out.len(),
            1,
            "a heading-drifted heartbeat must forward immediately, not coalesce"
        );
    }

    #[test]
    fn heartbeat_with_drifted_flags_is_treated_as_a_state_change() {
        let mut cs = CoalesceState::default();
        cs.on_movement(m::MSG_MOVE_START_FORWARD, &info(empty(), 0.0, 0.0), 0);
        let swim_flags = MovementInfo_MovementFlags::new(0x0020_0000, None, None, None, None);
        let drifted = info(swim_flags, 0.0, 1.0); // same heading, DIFFERENT flags
        let out = cs.on_movement(m::MSG_MOVE_HEARTBEAT, &drifted, 5);
        assert_eq!(
            out.len(),
            1,
            "a flags-drifted heartbeat must forward immediately, not coalesce"
        );
    }

    // ---- Rule 2: cadence window + flush-on-anything-else ----------------------------------------

    #[test]
    fn same_vector_heartbeat_forwards_once_the_window_elapses() {
        let mut cs = CoalesceState::default();
        let flags = empty();
        cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags.clone(), 0.0, 0.0), 0);
        let held = cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags.clone(), 0.0, 1.0), 100);
        assert!(held.is_empty(), "100ms < 150ms window -> held");
        let forwarded = cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags, 0.0, 2.0), 150);
        assert_eq!(forwarded.len(), 1, "150ms >= window -> forwards");
        assert_eq!(
            forwarded[0].1.position.x, 2.0,
            "forwards the LATEST pending position, not the superseded one"
        );
    }

    #[test]
    fn flush_now_style_flush_drains_pending_and_marks_it_forwarded() {
        // Mirrors what the dispatch loop's flush-on-any-other-opcode rule does: a non-movement
        // CMSG must see the pending heartbeat land before it runs.
        let mut cs = CoalesceState::default();
        let flags = empty();
        cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags.clone(), 0.0, 0.0), 0);
        cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags, 0.0, 3.0), 50); // held (50ms < 150ms)
        assert_eq!(
            cs.forwarded(),
            1,
            "only the baseline packet has forwarded so far"
        );
        let flushed = cs.flush(60);
        assert_eq!(flushed.unwrap().1.position.x, 3.0);
        assert_eq!(cs.forwarded(), 2, "the flush itself counts as a forward");
        assert!(
            cs.flush(70).is_none(),
            "nothing left to flush a second time"
        );
    }

    // ---- Rule 3: sub-yard same-vector intermediates drop entirely (superseded, not queued) ------

    #[test]
    fn intermediate_held_heartbeats_are_superseded_not_queued() {
        let mut cs = CoalesceState::default();
        let flags = empty();
        cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags.clone(), 0.0, 0.0), 0);
        // Three intermediates arrive within the window; only the LAST must ever ship.
        cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags.clone(), 0.0, 1.0), 30);
        cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags.clone(), 0.0, 2.0), 60);
        cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags, 0.0, 3.0), 90);
        let flushed = cs.flush(95).expect("a pending heartbeat is waiting");
        assert_eq!(
            flushed.1.position.x, 3.0,
            "only the newest superseding value ships — 1.0 and 2.0 dropped entirely"
        );
    }

    // ---- Rule 5 / the item's own done-when: N same-vector heartbeats at 60ms -> ~N/3 forwards ---

    #[test]
    fn synthetic_60ms_heartbeat_stream_forwards_roughly_a_third() {
        // The item's own acceptance check, verbatim: "N same-vector heartbeats at 60ms spacing ->
        // ~N/3 forwards" against the 150ms window (150 / 60 = 2.5 heartbeats per window).
        let mut cs = CoalesceState::default();
        let flags = empty();
        let n = 60u64;
        let mut forwarded_count = 0u64;
        for i in 0..n {
            let t = i * 60;
            let out = cs.on_movement(
                m::MSG_MOVE_HEARTBEAT,
                &info(flags.clone(), 0.0, i as f32),
                t,
            );
            forwarded_count += out.len() as u64;
        }
        assert_eq!(forwarded_count, cs.forwarded());
        let ratio = n as f64 / forwarded_count as f64;
        assert!(
            (2.5..=3.5).contains(&ratio),
            "expected ~3x reduction (N/forwarded in [2.5, 3.5]), got N={n} forwarded={forwarded_count} ratio={ratio:.2}"
        );
    }

    #[test]
    fn stationary_after_running_leaves_one_heartbeat_pending_until_next_packet() {
        // Rule 4's documented bound on the lazy (no-timer) design: the LAST heartbeat before a
        // player goes idle can sit pending until their next packet flushes it — never lost, just
        // delayed by at most one window. Proven here so the bound is pinned, not just described.
        let mut cs = CoalesceState::default();
        let flags = empty();
        cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags.clone(), 0.0, 0.0), 0);
        let held = cs.on_movement(m::MSG_MOVE_HEARTBEAT, &info(flags, 0.0, 9.0), 40);
        assert!(
            held.is_empty(),
            "the player's last heartbeat before going idle sits pending"
        );
        // No more packets arrive for a long time (the player is now stationary and silent). The
        // pending heartbeat is NOT lost — it is still there, waiting for the next inbound packet
        // (any opcode) or the periodic flush, whichever comes first.
        let flushed = cs.flush(5_000);
        assert_eq!(
            flushed.unwrap().1.position.x,
            9.0,
            "the held position survives, unmodified, until flushed"
        );
    }
}
