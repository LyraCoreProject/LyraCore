//! The total pre-auth read budget shared by both listeners.
//!
//! Each connection pins one blocking-pool thread, so a peer that connects and sends nothing would
//! hold a thread until the pool (512 by default) has no seat left for a real login. The listener
//! starts one budget when it accepts the socket. Each pre-auth read receives only the time that
//! remains, and the session clears the socket timeout once the peer has proven itself because both
//! long-lived read loops treat a timed-out read as a session-ending error.

use std::io::Read;
use std::time::{Duration, Instant};

/// Total time a logon connection gets to complete authentication after accept.
pub const LOGON_AUTH_READ_DEADLINE: Duration = Duration::from_secs(10);
/// Total time a world connection gets to send `CMSG_AUTH_SESSION` after accept.
pub const WORLD_AUTH_READ_DEADLINE: Duration = Duration::from_secs(15);

/// A socket whose blocking reads can be bounded, so a session can end its own pre-auth deadline.
pub trait ReadDeadline {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()>;
}

pub(crate) trait DeadlineClock {
    fn now(&self) -> Instant;
}

#[derive(Clone, Copy)]
pub(crate) struct SystemClock;

impl DeadlineClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// One total budget shared by every read before authentication succeeds.
pub(crate) struct PreAuthDeadline<C = SystemClock> {
    expires_at: Instant,
    clock: C,
}

impl PreAuthDeadline<SystemClock> {
    pub(crate) fn after(budget: Duration) -> Self {
        let now = Instant::now();
        Self {
            expires_at: now + budget,
            clock: SystemClock,
        }
    }
}

impl<C: DeadlineClock> PreAuthDeadline<C> {
    #[cfg(test)]
    pub(crate) fn with_clock(expires_at: Instant, clock: C) -> Self {
        Self { expires_at, clock }
    }

    pub(crate) fn reader<'a, S: Read + ReadDeadline>(
        &'a self,
        stream: &'a mut S,
    ) -> PreAuthReader<'a, S, C> {
        PreAuthReader {
            stream,
            deadline: self,
        }
    }

    fn remaining(&self) -> std::io::Result<Duration> {
        self.expires_at
            .checked_duration_since(self.clock.now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(deadline_elapsed)
    }
}

pub(crate) struct PreAuthReader<'a, S, C> {
    stream: &'a mut S,
    deadline: &'a PreAuthDeadline<C>,
}

impl<S: Read + ReadDeadline, C: DeadlineClock> Read for PreAuthReader<'_, S, C> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.stream
            .set_read_timeout(Some(self.deadline.remaining()?))?;
        let read = self.stream.read(buf)?;
        if self.deadline.clock.now() >= self.deadline.expires_at {
            return Err(deadline_elapsed());
        }
        Ok(read)
    }
}

fn deadline_elapsed() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "absolute pre-auth read deadline elapsed",
    )
}

impl ReadDeadline for std::net::TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        std::net::TcpStream::set_read_timeout(self, timeout)
    }
}

#[cfg(all(test, unix))]
impl ReadDeadline for std::os::unix::net::UnixStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, timeout)
    }
}

/// On Linux a read that hits `SO_RCVTIMEO` reports `WouldBlock`; other platforms say `TimedOut`.
pub fn is_read_deadline(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::Read;
    use std::rc::Rc;
    use std::time::Instant;

    #[derive(Clone)]
    struct ManualClock {
        now: Rc<Cell<Instant>>,
    }

    impl DeadlineClock for ManualClock {
        fn now(&self) -> Instant {
            self.now.get()
        }
    }

    struct DripReader {
        bytes: std::vec::IntoIter<u8>,
        clock: ManualClock,
        per_byte: Duration,
    }

    impl Read for DripReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let Some(byte) = self.bytes.next() else {
                return Ok(0);
            };
            self.clock.now.set(self.clock.now.get() + self.per_byte);
            buf[0] = byte;
            Ok(1)
        }
    }

    impl ReadDeadline for DripReader {
        fn set_read_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn slow_progress_cannot_extend_the_absolute_deadline() {
        let start = Instant::now();
        let clock = ManualClock {
            now: Rc::new(Cell::new(start)),
        };
        let deadline =
            PreAuthDeadline::with_clock(start + Duration::from_millis(10), clock.clone());
        let mut source = DripReader {
            bytes: vec![1, 2, 3].into_iter(),
            clock,
            per_byte: Duration::from_millis(4),
        };
        let mut bytes = [0; 3];

        let error = deadline
            .reader(&mut source)
            .read_exact(&mut bytes)
            .expect_err("three slow bytes must exceed one ten-millisecond deadline");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn a_second_read_keeps_the_first_reads_deadline() {
        let start = Instant::now();
        let clock = ManualClock {
            now: Rc::new(Cell::new(start)),
        };
        let deadline =
            PreAuthDeadline::with_clock(start + Duration::from_millis(10), clock.clone());
        let mut source = DripReader {
            bytes: vec![1, 2].into_iter(),
            clock,
            per_byte: Duration::from_millis(6),
        };
        let mut byte = [0];

        deadline.reader(&mut source).read_exact(&mut byte).unwrap();
        let error = deadline
            .reader(&mut source)
            .read_exact(&mut byte)
            .expect_err("a later pre-auth message must not receive a fresh deadline");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }
}
