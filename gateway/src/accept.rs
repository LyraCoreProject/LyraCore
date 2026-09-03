//! Accept-loop resource policy. This is the one place that decides whether an `accept(2)` failure
//! ends a listener or costs one connection, and it provides the shared non-waiting capacity that
//! keeps accepted sockets out of Tokio's unbounded blocking-task queue. Root-caused on the
//! mass-session login storm that killed the gateway outright (see below).
//!
//! # The bug this exists to prevent
//!
//! `main` joins the two listener tasks with `tokio::try_join!`, and both accept loops used to write
//! `listener.accept().await?`. tokio retries exactly one errno — `WouldBlock`
//! (`tokio/src/net/tcp/listener.rs`: every other `Err(e)` is `return Poll::Ready(Err(e))`) — so any
//! other errno travelled out of the accept loop, into `try_join!`, out of `main`, and ended the
//! process. On 2026-08-07 that is exactly what happened, and the gateway's last line was:
//!
//! ```text
//! Error: Too many open files (os error 24)
//! ```
//!
//! One `EMFILE` — a *per-call* condition that says nothing about the listener — took down every
//! session on the realm. `ECONNABORTED` (a peer resetting while queued in the backlog, which is what
//! a synthetic load harness killing its clients produces en masse) would have done the same.
//!
//! # The policy, and why it is shaped as a fatal ALLOWLIST
//!
//! [`classify_accept_error`] names a small, explicit set of errnos as fatal and treats everything
//! else as transient. The list is deliberately in that direction rather than the other:
//!
//! - The fatal set is exactly the errnos that mean **the listening socket itself is unusable**, so
//!   retrying could only spin forever. Each is also a condition **no remote peer can induce** — they
//!   are local programming/lifetime faults, not traffic.
//! - Everything else — including errnos we have never seen — is about *one connection* or a
//!   momentary shortage, and the honest response is to log it loudly and take the next connection.
//!   An unknown errno ending the realm is the failure mode we are fixing; an unknown errno costing
//!   one connection is not a failure mode at all.
//!
//! This is not "swallow anything". Every transient error is logged at WARN by the caller with its
//! errno text, and [`AcceptBackoff`] bounds the cost of a *permanent* condition we misjudged as
//! transient: the loop degrades to one attempt (and one log line) per second instead of spinning a
//! core at full tilt. A gateway that is loudly degraded beats a gateway that is gone.
//!
//! # Why exactly these four are fatal
//!
//! | errno | what it means for `accept` | why retrying is pointless |
//! |---|---|---|
//! | `EBADF` | the listener fd is not an open descriptor | it will never become one; every retry returns `EBADF` |
//! | `ENOTSOCK` | the fd is open but is not a socket | same fd, same answer, forever |
//! | `EINVAL` | the socket is not listening (or `addrlen` is bogus) | our listener came from `TcpListener::bind`, so this can only mean the fd was replaced underneath us |
//! | `EFAULT` | the address argument is not writable | a bug in tokio or in us; it will not fix itself between iterations |
//!
//! `EOPNOTSUPP` is the interesting **exclusion**. `accept(2)` lists it twice with opposite meanings: as
//! "the socket is not of type `SOCK_STREAM`" (permanent) *and* in the Linux-specific set of
//! already-pending network errors — `ENETDOWN`, `EPROTO`, `ENOPROTOOPT`, `EHOSTDOWN`, `ENONET`,
//! `EHOSTUNREACH`, `EOPNOTSUPP`, `ENETUNREACH` — which the man page says "should be treated like
//! `EAGAIN` by retrying". The permanent reading is impossible for us by construction: this listener
//! is a `TcpListener::bind`, hence `SOCK_STREAM`, hence the pending-network reading is the only one
//! that can apply. So it retries. `EPERM` likewise: on Linux `accept` reports it when a firewall
//! rule forbids *that* connection — a per-connection verdict, not a listener verdict.
//!
//! An `io::Error` carrying **no** raw errno (a synthetic error, never observed from `accept`) is
//! treated as transient for the same reason unknown errnos are: we cannot show it is permanent, and
//! the backoff caps what being wrong costs.

use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Non-waiting capacity shared by both listeners before they submit blocking session tasks.
///
/// A permit stays with the task for its whole lifetime. This keeps submitted plus running session
/// tasks at or below the blocking-pool ceiling instead of moving excess accepted sockets into
/// Tokio's unbounded blocking-task queue.
#[derive(Debug, Clone)]
pub(crate) struct BlockingTaskCapacity {
    permits: Arc<Semaphore>,
}

impl BlockingTaskCapacity {
    pub(crate) fn new(limit: usize) -> Self {
        assert!(limit > 0, "blocking-task capacity must be nonzero");
        Self {
            permits: Arc::new(Semaphore::new(limit)),
        }
    }

    /// Take a task seat immediately. `None` refuses this connection; it never waits in a queue.
    pub(crate) fn try_admit(&self) -> Option<BlockingTaskPermit> {
        self.permits
            .clone()
            .try_acquire_owned()
            .ok()
            .map(|permit| BlockingTaskPermit { _permit: permit })
    }

    #[cfg(test)]
    pub(crate) fn available(&self) -> usize {
        self.permits.available_permits()
    }
}

/// One submitted or running blocking session task. Dropping it returns the task seat, including
/// during unwinding.
pub(crate) struct BlockingTaskPermit {
    _permit: OwnedSemaphorePermit,
}

/// What the accept loop should do about an `accept(2)` (or per-socket setup) failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptOutcome {
    /// Log it and take the next connection. Costs one connection, never the realm.
    Retry,
    /// The listener itself is unusable — end the task. Retrying would spin forever.
    Fatal,
}

/// The errnos that mean the *listening socket* is broken rather than one connection.
///
/// Kept as data so the test below can assert the whole set at once, and so adding to it is a
/// visible, reviewable act. See the module docs for why each one is here — and, just as
/// importantly, why `EMFILE`/`ENFILE`/`ECONNABORTED`/`EOPNOTSUPP` are **not**.
const FATAL_ACCEPT_ERRNOS: &[i32] = &[libc::EBADF, libc::ENOTSOCK, libc::EINVAL, libc::EFAULT];

/// Decide whether an accept-path error ends the listener task or one connection.
///
/// Fatal is a short explicit allowlist ([`FATAL_ACCEPT_ERRNOS`]); everything else — known transient
/// errnos, errnos we have never seen, and errors with no raw errno at all — is [`AcceptOutcome::Retry`].
pub fn classify_accept_error(e: &io::Error) -> AcceptOutcome {
    match e.raw_os_error() {
        Some(errno) if FATAL_ACCEPT_ERRNOS.contains(&errno) => AcceptOutcome::Fatal,
        _ => AcceptOutcome::Retry,
    }
}

/// First failure retries immediately: a lone `ECONNABORTED` is the common case and should not cost
/// the next player any latency.
const BACKOFF_BASE_MS: u64 = 10;
/// Ceiling on the sleep. Under sustained `EMFILE` this is what the loop settles at — one attempt and
/// one log line per second, rather than a spun core and a flooded log.
const BACKOFF_CAP_MS: u64 = 1_000;

/// How long to wait after the `consecutive`-th back-to-back transient accept failure.
///
/// `1` is zero (retry at once), then 10ms doubling to a 1s cap. Under a saturated fd table
/// `accept` returns `EMFILE` *immediately*, so without this the loop is a busy-spin that burns a
/// core and writes a log line per iteration — which makes a recoverable shortage look like a hang.
pub fn backoff_delay(consecutive: u32) -> Duration {
    if consecutive <= 1 {
        return Duration::ZERO;
    }
    // `checked_shl` is NOT the tool here and the test below caught it being used: it validates only
    // the shift AMOUNT, so `10u64.checked_shl(63)` is a cheerful `Some(0)` — every set bit shifted
    // off the top — and `0.min(cap)` restores the exact busy-spin this function exists to prevent.
    // Clamp the shift, then saturate the multiply, so overshoot lands on the cap instead of zero.
    let steps = (consecutive - 2).min(u64::BITS - 1);
    let ms = BACKOFF_BASE_MS
        .saturating_mul(1u64 << steps)
        .min(BACKOFF_CAP_MS);
    Duration::from_millis(ms)
}

/// Consecutive-transient-failure counter for one accept loop. Reset by every accepted connection,
/// so a healthy gateway that sees one bad connection an hour never sleeps at all.
#[derive(Debug, Default, Clone, Copy)]
pub struct AcceptBackoff {
    consecutive: u32,
}

impl AcceptBackoff {
    pub const fn new() -> Self {
        Self { consecutive: 0 }
    }

    /// A connection came through — the shortage, whatever it was, is over.
    pub fn record_success(&mut self) {
        self.consecutive = 0;
    }

    /// Record one transient failure; returns how long to sleep before the next `accept`.
    pub fn record_failure(&mut self) -> Duration {
        self.consecutive = self.consecutive.saturating_add(1);
        backoff_delay(self.consecutive)
    }

    /// How many failures in a row we are into — for the log line, so an operator can tell one bad
    /// connection from a gateway that has been out of file descriptors for the last minute.
    pub fn consecutive(&self) -> u32 {
        self.consecutive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(errno: i32) -> io::Error {
        io::Error::from_raw_os_error(errno)
    }

    /// The whole point of this change: the errno that actually killed the realm on 2026-08-07 — and
    /// its siblings among the transient errnos this module retries — must cost one connection, not
    /// the process.
    #[test]
    fn the_errnos_that_killed_the_realm_are_transient() {
        for (errno, name) in [
            (
                libc::EMFILE,
                "EMFILE — this process is out of fds (the 08-07 death)",
            ),
            (libc::ENFILE, "ENFILE — the system is out of fds"),
            (
                libc::ECONNABORTED,
                "ECONNABORTED — peer reset while queued in the backlog",
            ),
            (libc::EINTR, "EINTR — a signal interrupted the call"),
            (libc::ENOBUFS, "ENOBUFS — kernel buffer pressure"),
            (libc::ENOMEM, "ENOMEM — kernel memory pressure"),
        ] {
            assert_eq!(
                classify_accept_error(&err(errno)),
                AcceptOutcome::Retry,
                "{name} must not end the listener"
            );
        }
    }

    /// The Linux "already-pending network error" family, which `accept(2)` says to retry like
    /// `EAGAIN`. `EOPNOTSUPP` is in here on purpose — see the module docs for why its other,
    /// permanent meaning cannot apply to a `TcpListener::bind`.
    #[test]
    fn pending_network_errors_and_per_connection_verdicts_are_transient() {
        for errno in [
            libc::ENETDOWN,
            libc::EPROTO,
            libc::ENOPROTOOPT,
            libc::EHOSTDOWN,
            libc::EHOSTUNREACH,
            libc::EOPNOTSUPP,
            libc::ENETUNREACH,
            libc::ECONNRESET,
            libc::EPERM,
            libc::EAGAIN,
        ] {
            assert_eq!(
                classify_accept_error(&err(errno)),
                AcceptOutcome::Retry,
                "errno {errno} should retry"
            );
        }
    }

    /// The fatal set, pinned in full. If this test has to change, someone is changing the
    /// availability policy of the whole realm and should have to say so in a diff.
    #[test]
    fn only_a_broken_listener_is_fatal() {
        for errno in [libc::EBADF, libc::ENOTSOCK, libc::EINVAL, libc::EFAULT] {
            assert_eq!(
                classify_accept_error(&err(errno)),
                AcceptOutcome::Fatal,
                "errno {errno} means the listener itself is unusable"
            );
        }
        assert_eq!(
            FATAL_ACCEPT_ERRNOS.len(),
            4,
            "the fatal set is these four and no others"
        );
    }

    /// An errno nobody has enumerated must not end the realm. This is the direction the allowlist
    /// is shaped for, so pin it against a value that is not in any list above.
    #[test]
    fn an_unknown_errno_retries_rather_than_ending_the_realm() {
        assert_eq!(classify_accept_error(&err(4095)), AcceptOutcome::Retry);
    }

    /// `accept` has never handed us an errno-less error, but if it did, "we cannot prove it is
    /// permanent" resolves to retry — the backoff bounds the cost of being wrong.
    #[test]
    fn an_error_with_no_raw_errno_retries() {
        let synthetic = io::Error::other("no errno here");
        assert_eq!(synthetic.raw_os_error(), None);
        assert_eq!(classify_accept_error(&synthetic), AcceptOutcome::Retry);
    }

    #[test]
    fn the_first_failure_retries_immediately_then_backs_off() {
        assert_eq!(backoff_delay(0), Duration::ZERO);
        assert_eq!(backoff_delay(1), Duration::ZERO);
        assert_eq!(backoff_delay(2), Duration::from_millis(10));
        assert_eq!(backoff_delay(3), Duration::from_millis(20));
        assert_eq!(backoff_delay(4), Duration::from_millis(40));
        assert_eq!(backoff_delay(8), Duration::from_millis(640));
    }

    /// A sustained shortage settles at one attempt per second and stays there — including for
    /// shift counts past `u64`'s width, which is where a naive `<<` would panic or wrap to zero and
    /// silently restore the busy-spin.
    #[test]
    fn the_backoff_saturates_at_one_second_and_never_wraps() {
        assert_eq!(backoff_delay(9), Duration::from_millis(1_000));
        for n in [10u32, 64, 65, 1_000, u32::MAX] {
            assert_eq!(
                backoff_delay(n),
                Duration::from_millis(BACKOFF_CAP_MS),
                "consecutive={n} must stay at the cap"
            );
        }
    }

    #[test]
    fn an_accepted_connection_clears_the_backoff() {
        let mut b = AcceptBackoff::new();
        assert_eq!(b.record_failure(), Duration::ZERO);
        assert_eq!(b.record_failure(), Duration::from_millis(10));
        assert_eq!(b.record_failure(), Duration::from_millis(20));
        assert_eq!(b.consecutive(), 3);
        b.record_success();
        assert_eq!(b.consecutive(), 0);
        // ...and the next lone failure is free again, so one bad connection an hour costs nothing.
        assert_eq!(b.record_failure(), Duration::ZERO);
    }

    /// The counter runs for the life of the process; it must not wrap back into "first failure,
    /// retry immediately" after 4 billion consecutive errors.
    #[test]
    fn the_counter_saturates_rather_than_wrapping() {
        let mut b = AcceptBackoff {
            consecutive: u32::MAX,
        };
        assert_eq!(b.record_failure(), Duration::from_millis(BACKOFF_CAP_MS));
        assert_eq!(b.consecutive(), u32::MAX);
    }

    #[test]
    fn excess_connections_are_refused_without_waiting_for_a_permit() {
        let capacity = BlockingTaskCapacity::new(2);
        let clone = capacity.clone();
        let _first = capacity.try_admit().expect("the first task has a seat");
        let _second = clone.try_admit().expect("the second task has a seat");

        assert!(
            capacity.try_admit().is_none(),
            "an excess connection must not enter a wait queue"
        );
        assert_eq!(capacity.available(), 0);

        drop(_first);
        let _replacement = capacity
            .try_admit()
            .expect("a task that exits before submission returns its seat");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn blocking_task_capacity_returns_after_errors_and_panics() {
        let capacity = BlockingTaskCapacity::new(1);

        let permit = capacity.try_admit().expect("the failing task has a seat");
        let failed = tokio::task::spawn_blocking(move || {
            let _task_permit = permit;
            Err::<(), ()>(())
        })
        .await
        .expect("the failing task returns normally");
        assert!(failed.is_err());
        assert_eq!(capacity.available(), 1, "an error must return its seat");

        let permit = capacity.try_admit().expect("the panicking task has a seat");
        let panicked = tokio::task::spawn_blocking(move || {
            let _task_permit = permit;
            panic!("test panic");
        })
        .await;
        assert!(panicked.unwrap_err().is_panic());
        assert_eq!(capacity.available(), 1, "an unwind must return its seat");
    }

    #[test]
    fn both_listeners_admit_before_they_spawn_a_blocking_task() {
        for (listener, source, signature) in [
            (
                "logon",
                include_str!("logon/mod.rs"),
                "pub async fn run(cfg: GatewayConfig, coordinator: Coordinator) -> Result<()> {",
            ),
            (
                "world",
                include_str!("world/mod.rs"),
                "pub async fn run(cfg: GatewayConfig, coordinator: Coordinator) -> Result<()> {",
            ),
        ] {
            let body = crate::test_scan::code_of(source, signature);
            let admit = body
                .find("cfg.blocking_task_capacity.try_admit()")
                .unwrap_or_else(|| panic!("{listener} does not check blocking-task capacity"));
            let spawn = body
                .find("tokio::task::spawn_blocking")
                .unwrap_or_else(|| panic!("{listener} does not spawn its blocking task"));
            assert!(
                admit < spawn,
                "{listener} queues the task before it checks capacity"
            );
            assert!(
                body[spawn..].contains("let _task_permit = task_permit;"),
                "{listener} does not hold its permit for the blocking task's whole lifetime"
            );
        }
    }

    #[test]
    fn blocking_task_capacity_uses_the_runtime_pool_ceiling() {
        let main_body =
            crate::test_scan::code_of(include_str!("main.rs"), "fn main() -> Result<()> {");
        assert!(
            main_body.contains("let max_blocking_threads = config::max_blocking_threads();")
                && main_body.contains(".max_blocking_threads(max_blocking_threads)"),
            "the runtime blocking pool no longer uses the configured ceiling"
        );

        let config_body =
            crate::test_scan::code_of(include_str!("config.rs"), "pub fn from_env() -> Self {");
        assert!(
            config_body.contains(
                "blocking_task_capacity: BlockingTaskCapacity::new(max_blocking_threads())"
            ),
            "the listener capacity no longer uses the runtime blocking-pool setting"
        );
    }
}
