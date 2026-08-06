//! #72 seam-crossing DETECTION ("Phase C warm handoff", `docs/region-sharding.md`). This module
//! answers exactly one question: has a moving player's cell just resolved, on the SECOND
//! CONSECUTIVE reading, to a database other than the one they are pinned to? If so it hands back a
//! [`SeamCrossing`] for the caller to act on. Detection and action are split on purpose — this
//! module stays a pure function of cell history and moves nobody itself; the caller
//! (`handle_seam_crossing` in `world/mod.rs`) owns the combat/cooldown guards and actually drives
//! the handoff via the escrowed transfer. That handoff is shipped and default on
//! (`LYRACORE_WARM_HANDOFF`, see `docs/region-sharding.md`) — this module is the piece it stands
//! on, not a slice still waiting to be consumed.
//!
//! The hysteresis rule is the design spec's own wording: "trigger when the player penetrates ≥1
//! full cell past the seam". A player standing on the line, or strafing across it, must not thrash
//! a handoff back and forth — crossing back has to cost more than crossing did — so [`SeamTracker`]
//! confirms only on the SECOND foreign cell in a row, never the first, and never twice for the same
//! dwell (one crossing per foreign leg, however many more foreign cells follow it).

use lyracore_shared::spatial::grid_cell;

/// A confirmed hand-off candidate. `target_db` is the database the SECOND consecutive foreign cell
/// resolved to; `(gx, gy)` is that cell. Slice 2's payload — slice 1 only logs it.
#[derive(Debug, Clone, PartialEq)]
pub struct SeamCrossing {
    pub target_db: String,
    pub gx: i32,
    pub gy: i32,
}

/// The streak. `Idle` and `Awaiting` are pre-confirmation; `Confirmed` suppresses a re-confirm for
/// every subsequent cell that resolves to the SAME db, so one continuous foreign dwell reports
/// exactly one crossing, however many cells it spans.
#[derive(Debug, Default)]
enum SeamState {
    #[default]
    Idle,
    Awaiting {
        db: String,
    },
    Confirmed {
        db: String,
    },
}

/// Per-session hysteresis state, owned by [`super::InWorld`] (reset every world entry — a fresh
/// `enter_world` means a fresh map, exactly like the AOI tracker's own box anchor).
#[derive(Debug, Default)]
pub struct SeamTracker {
    /// The last cell this tracker was asked about. `None` only before the first movement packet of
    /// the session — which therefore always counts as "changed", the same convention
    /// `GridBox::needs_recenter` uses for a fresh anchor.
    last_cell: Option<(i32, i32)>,
    state: SeamState,
}

impl SeamTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// #72 slice 2: un-confirm a crossing that was detected but could NOT be acted on (combat in
    /// progress, or the per-session cooldown hasn't elapsed) — `Confirmed{db}` back to
    /// `Awaiting{db}`. Without this, a skipped handoff is stuck: `Confirmed` suppresses every
    /// further crossing into the SAME db for as long as the dwell continues (that's the whole
    /// point of the hysteresis rule), so a player who entered combat exactly on the seam would
    /// never get another chance to hand off while they stayed in the foreign region. Rearming
    /// means the very NEXT foreign cell re-confirms (one cell, not two — `Awaiting` is already
    /// primed) rather than requiring the player to leave the region and re-enter.
    ///
    /// A no-op from `Idle` or `Awaiting` — neither has a confirmation to undo.
    pub fn rearm(&mut self) {
        if let SeamState::Confirmed { db } = &self.state {
            self.state = SeamState::Awaiting { db: db.clone() };
        }
    }

    /// Drive the tracker from a just-arrived movement packet's `(x, y)`. `resolve` answers "which
    /// database does the region containing this cell belong to, relative to the session's home
    /// shard" — `Some(db)` for a FOREIGN db, `None` for home or "no opinion" (see the call site in
    /// `world::mod` for how a raw `resolve_region_shard` answer gets folded into that). Called AT
    /// MOST ONCE, and only when the cell actually changed: the caller passes it as a closure
    /// specifically so the (DB-touching) resolution itself is skipped on a same-cell heartbeat, not
    /// just its result discarded — the zero-cost-when-nothing-changed property the cadence rule
    /// (danger-zones-adjacent: "not every heartbeat") exists to protect.
    pub fn check(
        &mut self,
        x: f32,
        y: f32,
        resolve: impl FnOnce(i32, i32) -> Option<String>,
    ) -> Option<SeamCrossing> {
        let cell = grid_cell(x, y);
        if self.last_cell == Some(cell) {
            return None;
        }
        self.last_cell = Some(cell);
        let foreign = resolve(cell.0, cell.1);
        self.observe(cell.0, cell.1, foreign.as_deref())
    }

    /// The pure hysteresis step, independent of cell-change gating — split out so it is unit-
    /// testable without a position or a resolver. `foreign` is the resolution the caller has
    /// already reduced to "a different db than home, or nothing to say".
    fn observe(&mut self, gx: i32, gy: i32, foreign: Option<&str>) -> Option<SeamCrossing> {
        let prev = std::mem::take(&mut self.state);
        match foreign {
            None => {
                self.state = SeamState::Idle;
                None
            }
            Some(db) => match prev {
                SeamState::Awaiting { db: p } if p == db => {
                    self.state = SeamState::Confirmed { db: db.to_string() };
                    Some(SeamCrossing {
                        target_db: db.to_string(),
                        gx,
                        gy,
                    })
                }
                SeamState::Confirmed { db: p } if p == db => {
                    self.state = SeamState::Confirmed { db: db.to_string() };
                    None
                }
                // Idle, or a DIFFERENT foreign db than the one being awaited/confirmed: restart the
                // streak against this db rather than confirming against the old one — two candidate
                // shards straddling the same seam must not cross-pollinate their hysteresis.
                _ => {
                    self.state = SeamState::Awaiting { db: db.to_string() };
                    None
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// First foreign cell never confirms — a straddler jittering across the line once must not
    /// hand off. This is the whole point of the hysteresis rule.
    #[test]
    fn one_foreign_cell_does_not_confirm() {
        let mut t = SeamTracker::new();
        assert_eq!(t.observe(1, 1, Some("world-2")), None);
    }

    /// The SECOND consecutive cell resolving to the SAME foreign db confirms exactly once —
    /// "penetrate ≥1 full cell past the seam" — and does not re-confirm while the dwell continues.
    #[test]
    fn second_consecutive_same_foreign_cell_confirms_once_and_not_again() {
        let mut t = SeamTracker::new();
        assert_eq!(t.observe(1, 1, Some("world-2")), None);
        assert_eq!(
            t.observe(1, 2, Some("world-2")),
            Some(SeamCrossing {
                target_db: "world-2".into(),
                gx: 1,
                gy: 2
            })
        );
        // Still dwelling in the foreign region: no re-confirm, however many more foreign cells.
        assert_eq!(t.observe(1, 3, Some("world-2")), None);
        assert_eq!(t.observe(1, 4, Some("world-2")), None);
    }

    /// foreign → home → foreign resets the streak: the second leg needs its OWN second cell, it
    /// does not get to reuse the first leg's.
    #[test]
    fn a_home_cell_between_two_foreign_legs_resets_the_streak() {
        let mut t = SeamTracker::new();
        assert_eq!(t.observe(1, 1, Some("world-2")), None); // 1st foreign cell
        assert_eq!(t.observe(1, 2, None), None); // back home — resets
        assert_eq!(t.observe(1, 3, Some("world-2")), None); // 1st of a NEW leg — no confirm yet
        assert_eq!(
            t.observe(1, 4, Some("world-2")),
            Some(SeamCrossing {
                target_db: "world-2".into(),
                gx: 1,
                gy: 4
            })
        ); // 2nd of the new leg — confirms
    }

    /// `None` resets even a CONFIRMED streak — a resident who returns home and crosses again must
    /// re-earn the confirmation, not free-ride on the earlier one.
    #[test]
    fn none_resets_a_confirmed_streak_too() {
        let mut t = SeamTracker::new();
        t.observe(1, 1, Some("world-2"));
        assert!(t.observe(1, 2, Some("world-2")).is_some()); // confirmed
        assert_eq!(t.observe(1, 3, None), None); // home
        assert_eq!(t.observe(1, 4, Some("world-2")), None); // 1st of a new leg — no confirm
    }

    /// A DIFFERENT foreign db restarts the streak rather than confirming against the old one.
    #[test]
    fn a_different_foreign_db_restarts_the_streak_rather_than_confirming() {
        let mut t = SeamTracker::new();
        assert_eq!(t.observe(1, 1, Some("world-2")), None);
        assert_eq!(t.observe(1, 2, Some("world-3")), None); // different db — restarts, no confirm
        assert_eq!(
            t.observe(1, 3, Some("world-3")),
            Some(SeamCrossing {
                target_db: "world-3".into(),
                gx: 1,
                gy: 3
            })
        );
    }

    /// `check` gates `resolve` on an ACTUAL cell change (the cadence rule: "only when the cell
    /// changed, NOT every heartbeat") and never calls it twice for the same cell.
    #[test]
    fn check_only_calls_resolve_on_a_cell_change() {
        let mut t = SeamTracker::new();
        let calls = std::cell::Cell::new(0u32);
        let resolve = |_gx, _gy| {
            calls.set(calls.get() + 1);
            None
        };
        t.check(-8949.95, -132.493, resolve);
        assert_eq!(
            calls.get(),
            1,
            "the first packet of a session always counts as a change"
        );
        t.check(-8945.0, -130.0, resolve); // a few yards — same 50yd cell
        assert_eq!(
            calls.get(),
            1,
            "a same-cell heartbeat must not touch resolve"
        );
        t.check(-8949.95, -132.493 - 600.0, resolve); // well over a cell away
        assert_eq!(calls.get(), 2);
    }

    /// #72 slice 2: `rearm` un-confirms a `Confirmed` streak so the very NEXT foreign cell
    /// re-confirms — one cell, not two (the whole point of skip-then-rearm: a combat skip must
    /// not force the player all the way back out of the region before trying again).
    #[test]
    fn rearm_lets_the_very_next_foreign_cell_reconfirm() {
        let mut t = SeamTracker::new();
        assert!(t.observe(1, 1, Some("world-2")).is_none());
        assert!(t.observe(1, 2, Some("world-2")).is_some()); // confirmed
        t.rearm();
        // One more foreign cell (not two) re-confirms — `rearm` primed `Awaiting`, it didn't
        // reset all the way to `Idle`.
        assert_eq!(
            t.observe(1, 3, Some("world-2")),
            Some(SeamCrossing {
                target_db: "world-2".into(),
                gx: 1,
                gy: 3
            })
        );
    }

    /// `rearm` is a no-op outside `Confirmed` — nothing to undo from `Idle` or `Awaiting`.
    #[test]
    fn rearm_is_a_no_op_outside_confirmed() {
        let mut t = SeamTracker::new();
        t.rearm(); // Idle: no-op
        assert_eq!(t.observe(1, 1, Some("world-2")), None);
        t.rearm(); // Awaiting: no-op — still needs its second cell, not a third
        assert_eq!(
            t.observe(1, 2, Some("world-2")),
            Some(SeamCrossing {
                target_db: "world-2".into(),
                gx: 1,
                gy: 2
            })
        );
    }
}
