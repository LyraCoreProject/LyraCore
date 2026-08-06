//! FIFO admission gate for world-session establishment (#180): a login storm arrives faster than
//! the writer can absorb enter-world/subscription-setup cost — measured 2026-07-30 at 500 sessions,
//! 40 ms stagger: `subscribe` alone was 38% of the writer, 148 reducer calls timed out, and 233 of
//! 500 players were silently dropped (`create_character`/`establish_session`/`release_transfer`/
//! `set_target` all among the timeouts). Rather than let every connection race straight into
//! `CMSG_PLAYER_LOGIN`, cap how many world sessions the gateway holds open at once; anyone over the
//! cap queues, holding their socket and rendering the client's NATIVE "Position in queue: N" screen
//! (`SMSG_AUTH_RESPONSE`'s `AuthWaitQueue { queue_position }` variant — the same message vmangos/
//! cmangos use) until a seat opens up. Nobody is dropped; the storm degrades into a wait instead.
//!
//! **Gate point**: `world_handshake` in `world/mod.rs`, right after the SRP client proof validates
//! (so the header cipher already exists — every packet from here on, including the queued resends,
//! must be encrypted) but BEFORE `AUTH_OK`. That is before the client can even reach
//! `CMSG_CHAR_ENUM`, let alone `CMSG_PLAYER_LOGIN`'s `subscribe_player_events` — the actual
//! expensive part #180 measured. NOT the logon tier (SRP + realm list on 3724): that authenticates
//! the account, not world capacity, and stays ungated.
//!
//! **Config** (both env vars, both default `0` = unlimited — an unconfigured gateway, including
//! every single-player dev realm, never queues anyone):
//! - `LYRACORE_MAX_SESSIONS` — the seat cap. A seat is held from admission (`AUTH_OK`) until the socket
//!   closes, covering character-select browsing as well as in-world play: the cheapest place to
//!   stop a storm is before any of it starts, not just before the expensive part.
//! - `LYRACORE_ADMIT_CONCURRENCY` — an ADDITIONAL rate limit on how many queued sessions may be admitted
//!   per tick (1s), independent of how many seats happen to be free at once. Without it, a mass
//!   simultaneous disconnect (a raid wipe, a restart draining a full queue) frees N seats in the
//!   same instant and admits the entire backlog at once — reproducing the exact storm this exists
//!   to prevent, just one hop later. With it, admissions trickle even when capacity allows more.
//!
//! Deliberate simplification: no priority tiers, no persistence. A disconnected queued socket just
//! leaves the line ([`LoginQueue::cancel`]); a reconnect gets a fresh ticket at the back.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default rate-limiter tick for `LYRACORE_ADMIT_CONCURRENCY` (production). Tests inject a much shorter
/// tick via [`LoginQueue::with_tick`] so the rate limiter can be exercised without real sleeps.
const ADMIT_TICK: Duration = Duration::from_secs(1);

/// A queued connection's place in line. Opaque — meaningful only as an argument back into the same
/// [`LoginQueue`] that issued it.
pub type Ticket = u64;

/// The result of [`LoginQueue::request`].
#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    /// A seat was free (or the queue is unlimited): proceed immediately, no wait.
    Admitted,
    /// No seat was free: `Ticket` is this caller's place in the FIFO line. Poll
    /// [`LoginQueue::position`] until it returns `None`.
    Queued(Ticket),
}

struct Waiter {
    ticket: Ticket,
    queued_at: Instant,
}

struct State {
    /// Sessions currently holding a seat.
    active: usize,
    /// FIFO line, front = next to admit.
    waiting: VecDeque<Waiter>,
    next_ticket: Ticket,
    /// Rate-limiter bookkeeping for `admit_concurrency > 0` — see `drain_locked`.
    tick_started: Instant,
    admitted_this_tick: usize,
}

/// FIFO admission gate for world-session establishment. Cheap to construct and cheap to consult
/// when unconfigured: [`LoginQueue::request`] returns `Admitted` before ever touching the mutex
/// when `max_sessions == 0`, so an unconfigured gateway pays nothing for this existing.
pub struct LoginQueue {
    max_sessions: usize,
    admit_concurrency: usize,
    tick: Duration,
    state: Mutex<State>,
    /// Cumulative admissions since construction (immediate + drained-from-queue), for the
    /// QUEUESTAT admitted/s rate — read-and-diff by the periodic reporting task, same convention as
    /// `MOTION_CALLS` et al. in `stdb/subscriptions.rs`. An instance field, not a global static: a
    /// bare `static` would leak counts across unrelated `LoginQueue`s (every test that builds its
    /// own queue would pollute every other test's rate).
    admitted_total: AtomicU64,
}

impl LoginQueue {
    /// `max_sessions == 0` disables the gate entirely (today's behavior, byte for byte — see the
    /// module docs). `admit_concurrency == 0` disables the extra per-tick rate limit: seats fill as
    /// fast as they free, subject only to `max_sessions`. Uses the production 1s tick; see
    /// [`Self::with_tick`] for tests.
    pub fn new(max_sessions: usize, admit_concurrency: usize) -> Self {
        Self::with_tick(max_sessions, admit_concurrency, ADMIT_TICK)
    }

    /// Like [`Self::new`] with an injectable tick length, so a test exercising the rate limiter
    /// doesn't have to sleep a full second per admission.
    pub fn with_tick(max_sessions: usize, admit_concurrency: usize, tick: Duration) -> Self {
        Self {
            max_sessions,
            admit_concurrency,
            tick,
            state: Mutex::new(State {
                active: 0,
                waiting: VecDeque::new(),
                next_ticket: 0,
                tick_started: Instant::now(),
                admitted_this_tick: 0,
            }),
            admitted_total: AtomicU64::new(0),
        }
    }

    /// The gate every unconfigured gateway gets: `LYRACORE_MAX_SESSIONS` unset ⇒ this, and every
    /// existing test/call site that doesn't care about #180 passes this to stay byte-identical to
    /// pre-#180 behavior.
    ///
    /// Test-only: a production gateway builds its queue with [`LoginQueue::from_env`], which already
    /// yields an unlimited gate when `LYRACORE_MAX_SESSIONS` is unset.
    #[cfg(test)]
    pub fn unlimited() -> Self {
        Self::new(0, 0)
    }

    /// Read `LYRACORE_MAX_SESSIONS` / `LYRACORE_ADMIT_CONCURRENCY` (`config::max_sessions`/`admit_concurrency`,
    /// both default `0` = unlimited).
    pub fn from_env() -> Self {
        Self::new(
            crate::config::max_sessions(),
            crate::config::admit_concurrency(),
        )
    }

    /// Ask for a seat. Immediate [`Admission::Admitted`] when the queue is unlimited, or when the
    /// line is empty and a seat happens to be free; otherwise joins the FIFO tail — always, even if
    /// capacity is technically free, so a new arrival can never jump an existing line (fairness is
    /// unconditional; only [`Self::depart`]/[`Self::position`] ever drain it).
    pub fn request(&self) -> Admission {
        if self.max_sessions == 0 {
            return Admission::Admitted;
        }
        let mut s = self.state.lock().unwrap();
        if s.waiting.is_empty() && s.active < self.max_sessions {
            s.active += 1;
            self.admitted_total.fetch_add(1, Ordering::Relaxed);
            return Admission::Admitted;
        }
        let ticket = s.next_ticket;
        s.next_ticket += 1;
        s.waiting.push_back(Waiter {
            ticket,
            queued_at: Instant::now(),
        });
        Admission::Queued(ticket)
    }

    /// 1-based position of `ticket` in line, or `None` once it has been admitted (the caller should
    /// stop polling and proceed). Opportunistically drains the front of the line first, so an
    /// admission becomes visible to whichever queued connection next calls this — not only to
    /// whoever happens to call [`Self::depart`].
    pub fn position(&self, ticket: Ticket) -> Option<usize> {
        let mut s = self.state.lock().unwrap();
        self.drain_locked(&mut s);
        s.waiting
            .iter()
            .position(|w| w.ticket == ticket)
            .map(|i| i + 1)
    }

    /// Leave the line without ever taking a seat — the disconnect-while-queued case. A no-op if
    /// `ticket` was already admitted (it is simply not found in `waiting` anymore).
    pub fn cancel(&self, ticket: Ticket) {
        let mut s = self.state.lock().unwrap();
        s.waiting.retain(|w| w.ticket != ticket);
    }

    /// Release a held seat — a world session ended (socket closed), however it ended. A no-op for
    /// the unlimited queue. Immediately tries to drain the line into the freed capacity, subject to
    /// the rate limit, so admission is responsive rather than waiting for some queued connection's
    /// own next poll.
    pub fn depart(&self) {
        if self.max_sessions == 0 {
            return;
        }
        let mut s = self.state.lock().unwrap();
        s.active = s.active.saturating_sub(1);
        self.drain_locked(&mut s);
    }

    /// Force a drain pass with no state change of its own — a backstop so periodic reporting (or
    /// anything else with a spare moment) can keep the line moving even if every queued
    /// connection's own poll cadence is slow.
    pub fn tick(&self) {
        let mut s = self.state.lock().unwrap();
        self.drain_locked(&mut s);
    }

    /// Move FIFO-front tickets from `waiting` into `active`. Always bounded by free capacity.
    /// ADDITIONALLY bounded by `admit_concurrency` per `tick` when that throttle is configured: a
    /// fresh tick resets the per-tick allowance, so demand beyond it waits for the next tick rather
    /// than draining in one burst just because many seats are free right now.
    fn drain_locked(&self, s: &mut State) {
        if self.admit_concurrency > 0 {
            let now = Instant::now();
            if now.duration_since(s.tick_started) >= self.tick {
                s.tick_started = now;
                s.admitted_this_tick = 0;
            }
        }
        loop {
            if s.active >= self.max_sessions {
                break;
            }
            if self.admit_concurrency > 0 && s.admitted_this_tick >= self.admit_concurrency {
                break;
            }
            let Some(_front) = s.waiting.pop_front() else {
                break;
            };
            s.active += 1;
            s.admitted_this_tick += 1;
            self.admitted_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Current queue depth (sessions NOT yet admitted), for QUEUESTAT.
    pub fn depth(&self) -> usize {
        self.state.lock().unwrap().waiting.len()
    }

    /// Sessions currently holding a seat.
    pub fn active(&self) -> usize {
        self.state.lock().unwrap().active
    }

    /// How long the longest-waiting queued connection has been in line, for QUEUESTAT. `None` when
    /// the queue is empty.
    pub fn oldest_wait(&self) -> Option<Duration> {
        let s = self.state.lock().unwrap();
        s.waiting.front().map(|w| w.queued_at.elapsed())
    }

    /// Cumulative admissions since construction, for the QUEUESTAT admitted/s rate (the caller
    /// reads this every N seconds and diffs against its own last reading).
    pub fn admitted_total(&self) -> u64 {
        self.admitted_total.load(Ordering::Relaxed)
    }

    /// The configured seat cap (`0` = unlimited) — for the startup log line and QUEUESTAT.
    pub fn max_sessions(&self) -> usize {
        self.max_sessions
    }

    /// The configured per-tick admission rate limit (`0` = unlimited) — for the startup log line.
    pub fn admit_concurrency(&self) -> usize {
        self.admit_concurrency
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===================================================================================
    //  #223 — the seat CAP itself: the limit that keeps a login storm from taking the realm
    //  down, and the release paths that keep seats from leaking.
    // ===================================================================================

    /// The property the whole cap exists for: however many connections arrive AT ONCE, no more than
    /// `max_sessions` of them ever hold a seat. `request()` decides that under the mutex, so this
    /// drives 64 threads at a cap of 4 and asserts the ledger reconciles exactly.
    ///
    /// Deterministic under any thread interleaving: it asserts only invariants
    /// (`admitted <= cap`, `active == admitted`, `depth == arrivals - admitted`), never a
    /// particular ordering or timing. The counts are read after every thread has joined, so there
    /// is no in-flight state to race with.
    #[test]
    fn a_simultaneous_login_storm_never_seats_more_than_the_cap() {
        const CAP: usize = 4;
        const ARRIVALS: usize = 64;
        let q = std::sync::Arc::new(LoginQueue::new(CAP, 0));

        let admitted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handles: Vec<_> = (0..ARRIVALS)
            .map(|_| {
                let q = q.clone();
                let admitted = admitted.clone();
                std::thread::spawn(move || {
                    if q.request() == Admission::Admitted {
                        admitted.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().expect("no arrival thread may panic");
        }

        let admitted = admitted.load(Ordering::Relaxed);
        assert_eq!(
            admitted, CAP,
            "with {ARRIVALS} simultaneous arrivals and {CAP} seats, exactly {CAP} must be admitted \
             — fewer means seats were stranded, more means the cap was breached under contention"
        );
        assert_eq!(
            q.active(),
            CAP,
            "the ledger must agree with what was handed out"
        );
        assert_eq!(
            q.depth(),
            ARRIVALS - CAP,
            "every arrival that was not seated must be IN LINE, not silently dropped"
        );
        assert_eq!(
            q.admitted_total(),
            CAP as u64,
            "the QUEUESTAT counter must count each admission exactly once"
        );
    }

    /// Session cleanup on the release side. `run_world_session_with_queue` calls `depart()` in its
    /// teardown for every session that was admitted — including the ones that ended by error or by
    /// a severed socket. If a seat could leak, a realm at its cap would refuse logins forever after
    /// enough crashed clients, with no way back short of a restart.
    ///
    /// A full cycle is driven twice over, so the queue is proven to return to EXACTLY its initial
    /// state rather than merely to a working one.
    #[test]
    fn every_seat_returns_to_the_pool_when_its_session_ends() {
        let q = LoginQueue::new(3, 0);
        for round in 0..2 {
            for _ in 0..3 {
                assert_eq!(q.request(), Admission::Admitted, "round {round}");
            }
            assert_eq!(q.active(), 3);
            // The 4th arrival is the one the cap is for; it gives up rather than waiting.
            let Admission::Queued(t) = q.request() else {
                panic!("round {round}: the 4th arrival must queue behind the 3 seats");
            };
            q.cancel(t);

            for _ in 0..3 {
                q.depart();
            }
            assert_eq!(
                q.active(),
                0,
                "round {round}: every seat must come back — a leaked seat permanently shrinks the \
                 realm's capacity and only a restart recovers it"
            );
            assert_eq!(q.depth(), 0, "round {round}: nobody may be left in line");
        }
    }

    /// A seat must never be released twice. `depart()` is called from session teardown, and #180's
    /// own comment notes the care taken that it runs exactly once per admitted session — but
    /// `active` is a `usize`, so a double release would underflow to `usize::MAX` and the cap would
    /// be permanently, silently unenforceable. The `saturating_sub` is what prevents that; this
    /// pins it.
    #[test]
    fn releasing_a_seat_more_often_than_it_was_taken_never_underflows_the_cap() {
        let q = LoginQueue::new(2, 0);
        assert_eq!(q.request(), Admission::Admitted);
        q.depart();
        q.depart(); // the spurious one
        q.depart();
        assert_eq!(
            q.active(),
            0,
            "an extra release must clamp at zero, not wrap to usize::MAX — a wrapped counter makes \
             `active < max_sessions` false forever and every subsequent login queues behind nothing"
        );
        // And the cap still works afterwards.
        assert_eq!(q.request(), Admission::Admitted);
        assert_eq!(q.request(), Admission::Admitted);
        assert!(matches!(q.request(), Admission::Queued(_)));
    }

    /// `depart()` on the UNLIMITED queue short-circuits before touching the mutex, so the counters
    /// stay at zero. Worth pinning because it is the configuration nearly every deployment runs,
    /// and because it means the unlimited path costs no lock at all on the disconnect hot path.
    #[test]
    fn the_unlimited_queue_takes_no_lock_and_keeps_no_ledger_on_departure() {
        let q = LoginQueue::unlimited();
        for _ in 0..100 {
            assert_eq!(q.request(), Admission::Admitted);
            q.depart();
        }
        assert_eq!(q.active(), 0);
        assert_eq!(q.depth(), 0);
        assert_eq!(
            q.admitted_total(),
            0,
            "the unlimited queue is a pure pass-through — it never even counts"
        );
    }

    /// `tick()` is the backstop drain: nothing about it changes state on its own, it just gives the
    /// line a chance to move when no queued connection happens to be polling and no session happens
    /// to be departing. It is called from the periodic QUEUESTAT reporting, and until now nothing
    /// exercised it at all.
    ///
    /// The scenario is the one it exists for: a seat freed while the only waiter is between polls.
    #[test]
    fn tick_drains_the_line_into_free_capacity_without_any_other_event() {
        let q = LoginQueue::new(1, 0);
        assert_eq!(q.request(), Admission::Admitted);
        let Admission::Queued(t) = q.request() else {
            panic!("the second arrival must queue behind the one seat");
        };
        assert_eq!(q.position(t), Some(1));

        // Free the seat WITHOUT letting `depart`'s own drain run, by reaching past it: this is the
        // state a gateway is in when a session ends and the waiter is asleep between polls.
        q.state.lock().unwrap().active = 0;
        assert_eq!(
            q.depth(),
            1,
            "precondition: the waiter is still in line and nothing has drained it"
        );

        q.tick();
        assert_eq!(
            q.depth(),
            0,
            "tick() must move the waiter into the free seat — without it, a line can sit still \
             while capacity is available"
        );
        assert_eq!(q.active(), 1);
        assert_eq!(
            q.position(t),
            None,
            "an admitted ticket must stop reporting a position, which is how the waiter learns to \
             proceed"
        );
    }

    /// `tick()` on an empty queue, and on an unlimited one, must be a harmless no-op — it runs on a
    /// timer, so it runs far more often than there is anything to do.
    #[test]
    fn tick_is_inert_when_there_is_nothing_to_drain() {
        let q = LoginQueue::new(2, 0);
        q.tick();
        assert_eq!((q.active(), q.depth(), q.admitted_total()), (0, 0, 0));

        let u = LoginQueue::unlimited();
        u.tick();
        assert_eq!((u.active(), u.depth(), u.admitted_total()), (0, 0, 0));
    }

    /// The configured limits have to be readable back — the startup log line and QUEUESTAT both
    /// report them, and an operator who cannot see the cap the gateway actually applied cannot tell
    /// a misread env var from a working one. (`LYRACORE_MAX_SESSIONS` is exactly the kind of
    /// silently-ignored setting the shard-map vars are also prone to.)
    #[test]
    fn the_configured_limits_are_reported_back_verbatim() {
        let q = LoginQueue::new(250, 8);
        assert_eq!(q.max_sessions(), 250);
        assert_eq!(q.admit_concurrency(), 8);

        let u = LoginQueue::unlimited();
        assert_eq!(u.max_sessions(), 0, "0 is the unlimited sentinel");
        assert_eq!(u.admit_concurrency(), 0);
    }

    /// Tickets must never be reused. `cancel` removes a waiter but `next_ticket` only ever
    /// increases, so a reconnecting client's stale ticket can never match a live one — which would
    /// otherwise let it read (or cancel) a stranger's place in line.
    #[test]
    fn a_cancelled_tickets_number_is_never_handed_out_again() {
        let q = LoginQueue::new(1, 0);
        assert_eq!(q.request(), Admission::Admitted);

        let mut seen = Vec::new();
        for _ in 0..5 {
            let Admission::Queued(t) = q.request() else {
                panic!("every arrival past the one seat must queue");
            };
            assert!(
                !seen.contains(&t),
                "ticket {t} was issued twice — a stale ticket would then address a live waiter"
            );
            seen.push(t);
            q.cancel(t); // leave immediately, freeing the number for reuse if it were reusable
        }
        assert_eq!(
            seen,
            vec![0, 1, 2, 3, 4],
            "tickets must be strictly increasing, never recycled from cancellations"
        );
        assert_eq!(q.depth(), 0, "every one of them left the line");
    }

    /// `oldest_wait` feeds the QUEUESTAT "longest wait" figure. `None` on an empty line is what
    /// tells the reporter there is nothing to report; a zero would read as "someone is waiting, for
    /// no time at all".
    #[test]
    fn oldest_wait_is_none_when_nobody_is_in_line() {
        let q = LoginQueue::new(1, 0);
        assert_eq!(q.oldest_wait(), None, "an empty queue has no oldest waiter");
        assert_eq!(q.request(), Admission::Admitted);
        assert_eq!(
            q.oldest_wait(),
            None,
            "an ADMITTED session is not waiting — only queued arrivals count"
        );
        let Admission::Queued(t) = q.request() else {
            panic!("expected a queued arrival");
        };
        assert!(q.oldest_wait().is_some(), "a queued arrival is a waiter");
        q.cancel(t);
        assert_eq!(
            q.oldest_wait(),
            None,
            "and it stops being one the moment it leaves"
        );
    }

    #[test]
    fn unlimited_never_queues_regardless_of_volume() {
        let q = LoginQueue::unlimited();
        for _ in 0..1000 {
            assert_eq!(q.request(), Admission::Admitted);
        }
        // No bookkeeping at all — active/depth stay at their initial values because `request`
        // short-circuits before touching the mutex.
        assert_eq!(q.depth(), 0);
        assert_eq!(q.active(), 0);
    }

    #[test]
    fn cap_zero_is_the_unlimited_shape_too() {
        // `LoginQueue::new(0, N)` for any N must behave exactly like `unlimited()` — the cap is
        // what disables the gate, not the concurrency knob.
        let q = LoginQueue::new(0, 5);
        assert_eq!(q.request(), Admission::Admitted);
        assert_eq!(q.depth(), 0);
    }

    #[test]
    fn fifo_order_and_positions() {
        let q = LoginQueue::new(1, 0);
        // Take the only seat directly (simulates another already-admitted session).
        assert_eq!(q.request(), Admission::Admitted);

        let Admission::Queued(a) = q.request() else {
            panic!("expected A to queue")
        };
        let Admission::Queued(b) = q.request() else {
            panic!("expected B to queue")
        };
        let Admission::Queued(c) = q.request() else {
            panic!("expected C to queue")
        };

        assert_eq!(q.position(a), Some(1));
        assert_eq!(q.position(b), Some(2));
        assert_eq!(q.position(c), Some(3));
        assert_eq!(q.depth(), 3);
    }

    #[test]
    fn position_updates_on_admission_in_fifo_order() {
        let q = LoginQueue::new(1, 0);
        assert_eq!(q.request(), Admission::Admitted); // the only seat

        let Admission::Queued(a) = q.request() else {
            panic!()
        };
        let Admission::Queued(b) = q.request() else {
            panic!()
        };
        let Admission::Queued(c) = q.request() else {
            panic!()
        };
        assert_eq!(
            (q.position(a), q.position(b), q.position(c)),
            (Some(1), Some(2), Some(3))
        );

        // Free the seat: A (the front) must be admitted, not B or C.
        q.depart();
        assert_eq!(
            q.position(a),
            None,
            "A should be admitted — it was at the front"
        );
        assert_eq!(q.position(b), Some(1), "B moves up to the front");
        assert_eq!(q.position(c), Some(2));

        q.depart();
        assert_eq!(q.position(b), None);
        assert_eq!(q.position(c), Some(1));

        q.depart();
        assert_eq!(q.position(c), None);
        assert_eq!(q.depth(), 0);
    }

    #[test]
    fn disconnect_while_queued_just_leaves_the_line() {
        let q = LoginQueue::new(1, 0);
        assert_eq!(q.request(), Admission::Admitted);

        let Admission::Queued(a) = q.request() else {
            panic!()
        };
        let Admission::Queued(b) = q.request() else {
            panic!()
        };
        assert_eq!(q.depth(), 2);

        // A disconnects while waiting: it leaves the line and consumes no seat.
        q.cancel(a);
        assert_eq!(q.depth(), 1);
        assert_eq!(
            q.position(a),
            None,
            "cancelled tickets read the same as admitted — gone either way"
        );
        assert_eq!(
            q.position(b),
            Some(1),
            "B moves up even though nobody was admitted"
        );
        assert_eq!(
            q.active(),
            1,
            "cancelling a WAITING ticket must never free or grant a seat"
        );

        // Freeing the real seat now admits B, not a phantom slot for the cancelled A.
        q.depart();
        assert_eq!(q.position(b), None);
        assert_eq!(q.active(), 1);
        assert_eq!(q.depth(), 0);
    }

    #[test]
    fn cancelling_an_already_admitted_ticket_is_a_harmless_noop() {
        let q = LoginQueue::new(0, 0);
        // Unlimited: `request()` never mints a real ticket, but a caller that (incorrectly) still
        // calls `cancel` on some arbitrary id must not panic or corrupt state.
        q.cancel(999);
        assert_eq!(q.depth(), 0);
    }

    /// Fill every seat via the empty-line fast path in `request()` itself (not the drain limiter):
    /// each of the first `cap` calls sees an empty line and free capacity, so all `cap` of them are
    /// `Admitted` outright. Returns once `active() == cap`.
    fn fill_to_capacity(q: &LoginQueue, cap: usize) {
        for _ in 0..cap {
            assert_eq!(q.request(), Admission::Admitted);
        }
        assert_eq!(q.active(), cap);
    }

    #[test]
    fn admit_concurrency_throttles_the_drain_rate_even_with_free_capacity() {
        // Cap 10, full house, then queue three behind it. A long tick so the test can assert the
        // throttle actually bound something before the next tick would roll over.
        let q = LoginQueue::with_tick(10, 1, Duration::from_secs(3600));
        fill_to_capacity(&q, 10);
        let Admission::Queued(a) = q.request() else {
            panic!("expected A to queue: the house is full")
        };
        let Admission::Queued(b) = q.request() else {
            panic!()
        };
        let Admission::Queued(c) = q.request() else {
            panic!()
        };
        assert_eq!(q.depth(), 3);

        // Simulate 5 near-simultaneous departures (a raid wipe) — plenty of freed capacity (5 of
        // 10 seats), but the per-tick allowance is 1: only the FIRST of these five `depart()` calls
        // may actually admit anyone; the rest find `admitted_this_tick` already spent.
        for _ in 0..5 {
            q.depart();
        }
        assert_eq!(
            q.position(a),
            None,
            "A admitted — the one this tick's allowance covers"
        );
        assert_eq!(
            q.position(b),
            Some(1),
            "B and C stay queued: this tick's allowance is spent"
        );
        assert_eq!(q.position(c), Some(2));
        assert_eq!(
            q.active(),
            6,
            "5 seats freed, only 1 refilled: 10 - 5 + 1 = 6"
        );

        // Polling again within the SAME tick must not admit more.
        assert_eq!(q.position(b), Some(1));
    }

    #[test]
    fn admit_concurrency_releases_more_on_the_next_tick() {
        let tick = Duration::from_millis(20);
        let q = LoginQueue::with_tick(10, 1, tick);
        fill_to_capacity(&q, 10);
        let Admission::Queued(a) = q.request() else {
            panic!()
        };
        let Admission::Queued(b) = q.request() else {
            panic!()
        };
        q.depart();
        q.depart();
        assert_eq!(q.position(a), None, "A admitted this tick");
        assert_eq!(
            q.position(b),
            Some(1),
            "B waits for the next tick even though a 2nd seat freed"
        );

        std::thread::sleep(tick * 3);
        assert_eq!(q.position(b), None, "the next tick's allowance admits B");
    }

    #[test]
    fn oldest_wait_reports_the_front_of_the_line() {
        let q = LoginQueue::new(1, 0);
        assert!(q.oldest_wait().is_none(), "nobody waiting yet");
        assert_eq!(q.request(), Admission::Admitted);
        assert!(
            q.oldest_wait().is_none(),
            "the one session is active, not waiting"
        );

        let Admission::Queued(_a) = q.request() else {
            panic!()
        };
        std::thread::sleep(Duration::from_millis(5));
        let waited = q.oldest_wait().expect("A is waiting");
        assert!(waited >= Duration::from_millis(5));
    }

    #[test]
    fn admitted_total_counts_both_immediate_and_drained_admissions() {
        let q = LoginQueue::new(1, 0);
        assert_eq!(q.admitted_total(), 0);
        assert_eq!(q.request(), Admission::Admitted); // immediate: +1
        assert_eq!(q.admitted_total(), 1);

        let Admission::Queued(a) = q.request() else {
            panic!()
        };
        assert_eq!(q.admitted_total(), 1, "queuing does not itself admit");
        q.depart();
        let _ = a;
        assert_eq!(
            q.admitted_total(),
            2,
            "the drain on depart counts as an admission"
        );
    }
}
