//! The pre-auth read deadline both listeners put on an accepted socket.
//!
//! Each connection pins one blocking-pool thread, so a peer that connects and sends nothing would
//! hold a thread until the pool (512 by default) has no seat left for a real login. The listener
//! sets the deadline on the accepted socket; the session clears it once the peer has proven itself,
//! because both long-lived read loops treat a timed-out read as a session-ending error.

use std::time::Duration;

/// Idle time a logon connection gets between handshake packets until its proof succeeds.
pub const LOGON_AUTH_READ_DEADLINE: Duration = Duration::from_secs(10);
/// Idle time a world connection gets to send `CMSG_AUTH_SESSION`.
pub const WORLD_AUTH_READ_DEADLINE: Duration = Duration::from_secs(15);

/// A socket whose blocking reads can be bounded, so a session can end its own pre-auth deadline.
pub trait ReadDeadline {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()>;
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
