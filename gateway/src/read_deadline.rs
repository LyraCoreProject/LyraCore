//! The total pre-auth I/O budget shared by both listeners.
//!
//! Each connection pins one blocking-pool thread, so a peer that connects and sends nothing would
//! hold a thread until the pool (512 by default) has no seat left for a real login. The listener
//! starts one budget when it accepts the socket. Each pre-auth read or write receives only the time
//! that remains. An async watchdog shuts down the socket at the same deadline even if its blocking
//! task has not started. The session clears both socket timeouts and cancels the watchdog once the
//! peer has proven itself.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Total time a logon connection gets to complete authentication after accept.
pub const LOGON_AUTH_DEADLINE: Duration = Duration::from_secs(10);
/// Total time a world connection gets to complete client proof after accept.
pub const WORLD_AUTH_DEADLINE: Duration = Duration::from_secs(15);

/// A socket whose blocking reads and writes can share one pre-auth deadline.
pub trait IoDeadline {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()>;
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

/// One total budget shared by every read and write before authentication succeeds.
pub(crate) struct PreAuthDeadline<C = SystemClock> {
    expires_at: Instant,
    clock: C,
    watchdog: Option<PreAuthWatchdog>,
}

impl PreAuthDeadline<SystemClock> {
    pub(crate) fn after(budget: Duration) -> Self {
        let now = Instant::now();
        Self {
            expires_at: now + budget,
            clock: SystemClock,
            watchdog: None,
        }
    }

    /// Shut the socket down at this deadline even if its blocking task has not started yet.
    pub(crate) fn arm(&mut self, stream: &std::net::TcpStream) -> std::io::Result<()> {
        let stream = stream.try_clone()?;
        let expires_at = tokio::time::Instant::from_std(self.expires_at);
        let (cancel, canceled) = oneshot::channel();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep_until(expires_at) => {
                    let _ = stream.shutdown(Shutdown::Both);
                }
                _ = canceled => {}
            }
        });
        self.watchdog = Some(PreAuthWatchdog {
            cancel: Some(cancel),
        });
        Ok(())
    }
}

impl<C: DeadlineClock> PreAuthDeadline<C> {
    #[cfg(test)]
    pub(crate) fn with_clock(expires_at: Instant, clock: C) -> Self {
        Self {
            expires_at,
            clock,
            watchdog: None,
        }
    }

    pub(crate) fn io<'a, S: Read + Write + IoDeadline>(
        &'a self,
        stream: &'a mut S,
    ) -> PreAuthIo<'a, S, C> {
        PreAuthIo {
            stream,
            deadline: self,
        }
    }

    /// End the pre-auth policy without leaving either socket timeout on the long-lived session.
    pub(crate) fn finish<S: IoDeadline>(&mut self, stream: &S) -> std::io::Result<()> {
        let read = stream.set_read_timeout(None);
        let write = stream.set_write_timeout(None);
        self.watchdog.take();
        read.and(write)
    }

    fn remaining(&self) -> std::io::Result<Duration> {
        self.expires_at
            .checked_duration_since(self.clock.now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(deadline_elapsed)
    }
}

pub(crate) struct PreAuthIo<'a, S, C> {
    stream: &'a mut S,
    deadline: &'a PreAuthDeadline<C>,
}

impl<S: Read + Write + IoDeadline, C: DeadlineClock> Read for PreAuthIo<'_, S, C> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.stream
            .set_read_timeout(Some(self.deadline.remaining()?))?;
        let result = self.stream.read(buf);
        self.deadline.check_after(result)
    }
}

impl<S: Read + Write + IoDeadline, C: DeadlineClock> Write for PreAuthIo<'_, S, C> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.stream
            .set_write_timeout(Some(self.deadline.remaining()?))?;
        let result = self.stream.write(buf);
        self.deadline.check_after(result)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream
            .set_write_timeout(Some(self.deadline.remaining()?))?;
        let result = self.stream.flush();
        self.deadline.check_after(result)
    }
}

impl<C: DeadlineClock> PreAuthDeadline<C> {
    fn check_after<T>(&self, result: std::io::Result<T>) -> std::io::Result<T> {
        let value = result?;
        if self.clock.now() >= self.expires_at {
            return Err(deadline_elapsed());
        }
        Ok(value)
    }
}

struct PreAuthWatchdog {
    cancel: Option<oneshot::Sender<()>>,
}

impl Drop for PreAuthWatchdog {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }
}

fn deadline_elapsed() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "absolute pre-auth I/O deadline elapsed",
    )
}

impl IoDeadline for std::net::TcpStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        std::net::TcpStream::set_read_timeout(self, timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        std::net::TcpStream::set_write_timeout(self, timeout)
    }
}

#[cfg(all(test, unix))]
impl IoDeadline for std::os::unix::net::UnixStream {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        std::os::unix::net::UnixStream::set_read_timeout(self, timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        std::os::unix::net::UnixStream::set_write_timeout(self, timeout)
    }
}

/// Linux reports `WouldBlock` when a socket read or write timeout expires; other platforms may
/// report `TimedOut`.
pub fn is_io_deadline(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::rc::Rc;
    use std::time::Instant;

    fn tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

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

    impl Write for DripReader {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Ok(0)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl IoDeadline for DripReader {
        fn set_read_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }

        fn set_write_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct DripWriter {
        clock: ManualClock,
        per_byte: Duration,
        written: Vec<u8>,
    }

    impl Read for DripWriter {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }

    impl Write for DripWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let Some(&byte) = buf.first() else {
                return Ok(0);
            };
            self.clock.now.set(self.clock.now.get() + self.per_byte);
            self.written.push(byte);
            Ok(1)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl IoDeadline for DripWriter {
        fn set_read_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }

        fn set_write_timeout(&self, _timeout: Option<Duration>) -> std::io::Result<()> {
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
            .io(&mut source)
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

        deadline.io(&mut source).read_exact(&mut byte).unwrap();
        let error = deadline
            .io(&mut source)
            .read_exact(&mut byte)
            .expect_err("a later pre-auth message must not receive a fresh deadline");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn slow_writes_cannot_extend_the_absolute_deadline() {
        let start = Instant::now();
        let clock = ManualClock {
            now: Rc::new(Cell::new(start)),
        };
        let deadline =
            PreAuthDeadline::with_clock(start + Duration::from_millis(10), clock.clone());
        let mut sink = DripWriter {
            clock,
            per_byte: Duration::from_millis(4),
            written: Vec::new(),
        };

        let error = deadline
            .io(&mut sink)
            .write_all(&[1, 2, 3])
            .expect_err("three slow bytes must exceed one ten-millisecond deadline");

        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert_eq!(sink.written, [1, 2, 3]);
    }

    #[tokio::test]
    async fn finishing_auth_cancels_the_watchdog_and_clears_both_socket_timeouts() {
        let (mut client, mut server) = tcp_pair();
        server
            .set_read_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        server
            .set_write_timeout(Some(Duration::from_millis(10)))
            .unwrap();
        let mut deadline = PreAuthDeadline::after(Duration::from_millis(40));
        deadline.arm(&server).unwrap();

        deadline.finish(&server).unwrap();
        assert_eq!(server.read_timeout().unwrap(), None);
        assert_eq!(server.write_timeout().unwrap(), None);
        tokio::time::sleep(Duration::from_millis(80)).await;

        client.write_all(&[7]).unwrap();
        let mut byte = [0];
        server.read_exact(&mut byte).unwrap();
        assert_eq!(
            byte,
            [7],
            "the canceled watchdog must leave the socket open"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watchdog_closes_the_socket_without_handler_io() {
        let (mut client, server) = tcp_pair();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut deadline = PreAuthDeadline::after(Duration::from_millis(40));
        deadline.arm(&server).unwrap();

        let read = tokio::task::spawn_blocking(move || {
            let mut byte = [0];
            client.read(&mut byte)
        });
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), read)
                .await
                .expect("the watchdog must close the peer promptly")
                .expect("the client read task must not panic")
                .unwrap(),
            0
        );

        drop(deadline);
        drop(server);
    }

    #[test]
    fn watchdog_expires_a_socket_while_the_blocking_pool_is_saturated() {
        use crate::accept::BlockingTaskCapacity;

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (blocker_started, wait_for_blocker) = std::sync::mpsc::channel();
        let (release_blocker, blocker_release) = std::sync::mpsc::channel();
        runtime.spawn_blocking(move || {
            blocker_started.send(()).unwrap();
            blocker_release.recv().unwrap();
        });
        wait_for_blocker
            .recv_timeout(Duration::from_secs(1))
            .expect("the only blocking thread must be occupied");

        let (mut client, mut server) = tcp_pair();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let capacity = BlockingTaskCapacity::new(1);
        let queued_task = {
            let _runtime_guard = runtime.enter();
            let mut deadline = PreAuthDeadline::after(Duration::from_millis(75));
            deadline.arm(&server).unwrap();
            let permit = capacity.try_admit().unwrap();
            tokio::task::spawn_blocking(move || {
                let _task_permit = permit;
                let mut byte = [0];
                deadline.io(&mut server).read(&mut byte)
            })
        };

        let mut byte = [0];
        assert_eq!(
            client.read(&mut byte).unwrap(),
            0,
            "the watchdog must close a socket whose handler cannot start"
        );
        assert!(
            !queued_task.is_finished(),
            "the session task must still be behind the saturated pool"
        );
        assert!(
            capacity.try_admit().is_none(),
            "the one queued task still owns the only permit"
        );

        release_blocker.send(()).unwrap();
        let error = runtime
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(1), queued_task)
                    .await
                    .expect("the bounded queued task must start once the pool frees")
                    .expect("the expired task must return")
            })
            .expect_err("the expired deadline must refuse its first read");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(1), async {
                while capacity.available() == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the expired task must return its permit");
        });
    }
}
