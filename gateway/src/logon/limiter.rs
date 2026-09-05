//! Logon Limiter: the caps on SRP6 proof attempts, per connection and per peer address.
//!
//! Every challenge and every proof costs the gateway a modular exponentiation, and a wrong
//! password was free to retry without bound. The limiter is in-memory and per gateway process;
//! nothing here is durable, so a restart forgives every address.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Logon attempts one connection may start. The classic client's own retry ceiling.
pub const ATTEMPTS_PER_CONNECTION: u32 = 3;
/// Failed logons one address may make per window; the rest of the window is refused.
pub const FAILURES_PER_WINDOW: u32 = 10;
pub const FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// Open logon connections one address may hold at once.
pub const CONNECTIONS_PER_ADDRESS: usize = 8;
/// Pause before answering a failed proof, so a guess costs wall-clock time as well as a modexp.
pub const PROOF_FAILURE_DELAY: Duration = Duration::from_millis(200);

const PRUNE_INTERVAL: Duration = Duration::from_secs(1);

/// Why the limiter closed or refused a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogonRefusal {
    /// The address already holds `CONNECTIONS_PER_ADDRESS` open logon connections.
    TooManyConnections,
    /// The address failed `FAILURES_PER_WINDOW` logons inside the current window.
    TooManyFailures,
    /// This connection has used its `ATTEMPTS_PER_CONNECTION`.
    AttemptsExhausted,
}

impl fmt::Display for LogonRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyConnections => write!(
                f,
                "address holds {CONNECTIONS_PER_ADDRESS} open logon connections"
            ),
            Self::TooManyFailures => write!(
                f,
                "address failed {FAILURES_PER_WINDOW} logons within {}s",
                FAILURE_WINDOW.as_secs()
            ),
            Self::AttemptsExhausted => {
                write!(
                    f,
                    "connection used its {ATTEMPTS_PER_CONNECTION} logon attempts"
                )
            }
        }
    }
}

impl std::error::Error for LogonRefusal {}

#[derive(Default)]
struct AddressRecord {
    open_connections: usize,
    /// Start of the failure window; `None` until the first failure.
    window_started: Option<Instant>,
    failures: u32,
}

impl AddressRecord {
    /// Drop a window that has run out, so the next failure opens a fresh one.
    fn roll_window(&mut self, now: Instant) {
        if self
            .window_started
            .is_some_and(|started| now.duration_since(started) >= FAILURE_WINDOW)
        {
            self.window_started = None;
            self.failures = 0;
        }
    }

    fn refuses(&mut self, now: Instant) -> bool {
        self.roll_window(now);
        self.failures >= FAILURES_PER_WINDOW
    }

    fn record_failure(&mut self, now: Instant) {
        self.roll_window(now);
        self.window_started.get_or_insert(now);
        self.failures = self.failures.saturating_add(1);
    }

    /// Nothing left to remember: no open connection and no live window.
    fn is_idle(&mut self, now: Instant) -> bool {
        self.roll_window(now);
        self.open_connections == 0 && self.window_started.is_none()
    }
}

#[derive(Default)]
struct TrackedAddresses {
    records: HashMap<IpAddr, AddressRecord>,
    last_pruned: Option<Instant>,
}

/// Shared across every logon connection of one listener.
#[derive(Default)]
pub struct LogonLimiter {
    addresses: Mutex<TrackedAddresses>,
}

impl LogonLimiter {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Admit one connection from `ip`, or refuse it before it costs a thread. Check this address
    /// every time; prune idle addresses on the first admission at least one second after the last
    /// scan. Admissions between scans only look up this address under the shared lock.
    pub fn admit(
        self: &Arc<Self>,
        ip: IpAddr,
        now: Instant,
    ) -> Result<LogonConnection, LogonRefusal> {
        let mut addresses = self.lock();
        if addresses
            .last_pruned
            .is_none_or(|last| now.duration_since(last) >= PRUNE_INTERVAL)
        {
            addresses.records.retain(|_, record| !record.is_idle(now));
            addresses.last_pruned = Some(now);
        }
        let record = addresses.records.entry(ip).or_default();
        if record.open_connections >= CONNECTIONS_PER_ADDRESS {
            return Err(LogonRefusal::TooManyConnections);
        }
        if record.refuses(now) {
            return Err(LogonRefusal::TooManyFailures);
        }
        record.open_connections += 1;
        Ok(LogonConnection {
            limiter: Arc::clone(self),
            ip,
            attempts: 0,
        })
    }

    fn refuses(&self, ip: IpAddr, now: Instant) -> bool {
        self.lock()
            .records
            .get_mut(&ip)
            .is_some_and(|record| record.refuses(now))
    }

    fn record_failure(&self, ip: IpAddr, now: Instant) {
        self.lock()
            .records
            .entry(ip)
            .or_default()
            .record_failure(now);
    }

    fn release(&self, ip: IpAddr) {
        if let Some(record) = self.lock().records.get_mut(&ip) {
            record.open_connections = record.open_connections.saturating_sub(1);
        }
    }

    /// A poisoned lock only means a connection task panicked mid-update; the counts are still usable.
    fn lock(&self) -> std::sync::MutexGuard<'_, TrackedAddresses> {
        self.addresses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    pub fn tracked_addresses(&self) -> usize {
        self.lock().records.len()
    }
}

/// One admitted connection's share of the limits. Dropping it frees its address slot.
pub struct LogonConnection {
    limiter: Arc<LogonLimiter>,
    ip: IpAddr,
    attempts: u32,
}

impl LogonConnection {
    /// Called on every challenge, before any SRP math. Refuses once this connection or its
    /// address is out of attempts.
    pub fn start_attempt(&mut self, now: Instant) -> Result<(), LogonRefusal> {
        if self.attempts >= ATTEMPTS_PER_CONNECTION {
            return Err(LogonRefusal::AttemptsExhausted);
        }
        if self.limiter.refuses(self.ip, now) {
            return Err(LogonRefusal::TooManyFailures);
        }
        self.attempts += 1;
        Ok(())
    }

    /// Called after a refused challenge or a failed proof. Counts it against the address, and
    /// returns the refusal that closes this connection once its attempts are spent.
    pub fn record_failure(&mut self, now: Instant) -> Result<(), LogonRefusal> {
        self.limiter.record_failure(self.ip, now);
        if self.attempts >= ATTEMPTS_PER_CONNECTION {
            return Err(LogonRefusal::AttemptsExhausted);
        }
        Ok(())
    }
}

impl Drop for LogonConnection {
    fn drop(&mut self) {
        self.limiter.release(self.ip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
    }

    #[test]
    fn a_connection_gets_three_attempts_then_closes_on_its_third_failure() {
        let limiter = LogonLimiter::new();
        let now = Instant::now();
        let mut connection = limiter.admit(ip(1), now).unwrap();
        for _ in 0..ATTEMPTS_PER_CONNECTION - 1 {
            connection.start_attempt(now).unwrap();
            connection.record_failure(now).unwrap();
        }
        connection.start_attempt(now).unwrap();
        assert_eq!(
            connection.record_failure(now),
            Err(LogonRefusal::AttemptsExhausted)
        );
        assert_eq!(
            connection.start_attempt(now),
            Err(LogonRefusal::AttemptsExhausted)
        );
    }

    #[test]
    fn an_address_is_refused_after_ten_failures_until_the_window_ends() {
        let limiter = LogonLimiter::new();
        let start = Instant::now();
        for i in 0..FAILURES_PER_WINDOW {
            let mut connection = limiter.admit(ip(1), start).unwrap();
            connection.start_attempt(start).unwrap();
            // Every attempt on a fresh connection is that connection's first, so none closes.
            connection
                .record_failure(start)
                .unwrap_or_else(|_| panic!("failure {i} closed"));
        }
        assert_eq!(
            limiter.admit(ip(1), start).err(),
            Some(LogonRefusal::TooManyFailures),
            "the eleventh attempt is refused at accept"
        );
        assert!(
            limiter.admit(ip(2), start).is_ok(),
            "the window is per address"
        );

        let later = start + FAILURE_WINDOW - Duration::from_millis(1);
        assert_eq!(
            limiter.admit(ip(1), later).err(),
            Some(LogonRefusal::TooManyFailures)
        );
        let window_over = start + FAILURE_WINDOW;
        assert!(limiter.admit(ip(1), window_over).is_ok());
    }

    #[test]
    fn a_window_that_fills_mid_connection_refuses_the_next_challenge() {
        let limiter = LogonLimiter::new();
        let now = Instant::now();
        let mut long_lived = limiter.admit(ip(1), now).unwrap();
        for _ in 0..FAILURES_PER_WINDOW {
            let mut other = limiter.admit(ip(1), now).unwrap();
            other.start_attempt(now).unwrap();
            other.record_failure(now).unwrap();
        }
        assert_eq!(
            long_lived.start_attempt(now),
            Err(LogonRefusal::TooManyFailures)
        );
    }

    #[test]
    fn an_address_holds_at_most_eight_open_connections() {
        let limiter = LogonLimiter::new();
        let now = Instant::now();
        let held: Vec<_> = (0..CONNECTIONS_PER_ADDRESS)
            .map(|_| limiter.admit(ip(1), now).unwrap())
            .collect();
        assert_eq!(
            limiter.admit(ip(1), now).err(),
            Some(LogonRefusal::TooManyConnections)
        );
        assert!(limiter.admit(ip(2), now).is_ok());
        drop(held);
        assert!(
            limiter.admit(ip(1), now).is_ok(),
            "closing a connection frees its slot"
        );
    }

    #[test]
    fn idle_addresses_are_pruned_at_the_interval_and_not_on_each_admission() {
        let limiter = LogonLimiter::new();
        let start = Instant::now();
        drop(limiter.admit(ip(1), start).unwrap());
        drop(
            limiter
                .admit(ip(2), start + Duration::from_millis(999))
                .unwrap(),
        );
        assert_eq!(limiter.tracked_addresses(), 2, "no second scan yet");

        let _open = limiter
            .admit(ip(3), start + Duration::from_secs(1))
            .unwrap();
        assert_eq!(
            limiter.tracked_addresses(),
            1,
            "both idle records are removed"
        );
    }

    #[test]
    fn address_windows_expire_even_between_cleanup_scans() {
        let limiter = LogonLimiter::new();
        let start = Instant::now();
        let mut held = limiter.admit(ip(1), start).unwrap();
        for address in [ip(1), ip(2)] {
            for _ in 0..FAILURES_PER_WINDOW {
                let mut connection = limiter.admit(address, start).unwrap();
                connection.start_attempt(start).unwrap();
                connection.record_failure(start).unwrap();
            }
        }
        let before_expiry = start + FAILURE_WINDOW - Duration::from_millis(1);
        drop(limiter.admit(ip(3), before_expiry).unwrap());
        assert_eq!(
            limiter.admit(ip(2), before_expiry).err(),
            Some(LogonRefusal::TooManyFailures)
        );
        assert_eq!(
            held.start_attempt(before_expiry),
            Err(LogonRefusal::TooManyFailures)
        );

        let expired = start + FAILURE_WINDOW;
        assert!(limiter.admit(ip(2), expired).is_ok());
        assert_eq!(held.start_attempt(expired), Ok(()));
        assert_eq!(limiter.tracked_addresses(), 3, "no global cleanup was due");
    }

    #[test]
    fn expired_failure_windows_are_pruned_on_the_next_cleanup() {
        let limiter = LogonLimiter::new();
        let start = Instant::now();
        for last in 1..=50 {
            let mut connection = limiter.admit(ip(last), start).unwrap();
            connection.start_attempt(start).unwrap();
            connection.record_failure(start).unwrap();
        }
        assert_eq!(limiter.tracked_addresses(), 50, "live windows are kept");

        let window_over = start + FAILURE_WINDOW;
        let _open = limiter.admit(ip(200), window_over).unwrap();
        assert_eq!(
            limiter.tracked_addresses(),
            1,
            "only the address with an open connection remains"
        );
    }
}
