//! Accept-loop resource policy. See `docs/accept-policy.md` for the contract and rationale.

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
/// `docs/accept-policy.md` explains the allowlist and the transient resource errors.
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

    /// Resource exhaustion and interrupted connections leave the listener available.
    #[test]
    fn the_errnos_that_killed_the_realm_are_transient() {
        for (errno, name) in [
            (libc::EMFILE, "EMFILE: this process is out of fds"),
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
}
