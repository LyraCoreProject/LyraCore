//! `SessionEpochs` — entity-ownership arbitration between two sockets on one account, pure
//! code-motion split out of `connection.rs`. Depends on no `spacetimedb_sdk`: pure sync-primitive
//! bookkeeping, not connection wiring. (`AccountSessions`, the live-socket refcount behind
//! releasing per-player connections, died with them — #483.)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Per-account "current in-world session" tracking. The world gateway opens one TCP session per
/// socket, but the account is ONE actor across reconnects — so when a stale
/// socket finally tears down and asks the module to delete the player's entity, the module can't
/// tell it apart from a newer session that re-logged in on the same account, and would delete the
/// LIVE player. Each `player_login` claims a fresh epoch (becoming the current owner of the entity);
/// teardown deletes the entity only if its epoch is still current. The gateway must arbitrate this
/// because the distinction (which socket) only exists here, not in the module.
#[derive(Default)]
pub(crate) struct SessionEpochs {
    next: AtomicU64,
    current: Mutex<HashMap<u64, u64>>,
}

impl SessionEpochs {
    /// The current-epoch map, recovering a poisoned lock (matches `connection.rs`'s
    /// `.lock().unwrap()` discipline, e.g. `coord()`/`call_pipe()`) — a stale read beats poisoning
    /// every future claim/release for every account on the shard.
    fn current(&self) -> std::sync::MutexGuard<'_, HashMap<u64, u64>> {
        self.current.lock().unwrap_or_else(|p| {
            log::error!(
                "session-epoch lock poisoned (a prior panic in a critical section) — recovering"
            );
            p.into_inner()
        })
    }

    /// Claim a fresh epoch for `account_id` and make it current.
    pub(crate) fn claim(&self, account_id: u64) -> u64 {
        let epoch = self.next.fetch_add(1, Ordering::Relaxed);
        self.current().insert(account_id, epoch);
        epoch
    }

    /// Release `epoch`; returns true iff it was still the current epoch (caller owns the entity, so
    /// it's safe to delete it on logout), false if a newer login superseded it (do NOT delete).
    pub(crate) fn release(&self, account_id: u64, epoch: u64) -> bool {
        let mut current = self.current();
        if current.get(&account_id) == Some(&epoch) {
            current.remove(&account_id);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod session_epoch_tests {
    use super::SessionEpochs;

    #[test]
    fn newer_login_supersedes_a_stale_session() {
        let s = SessionEpochs::default();
        let acct = 42;
        let a = s.claim(acct); // socket A enters the world
        let b = s.claim(acct); // socket B re-logs on the same account, superseding A
        assert_ne!(a, b);
        // A's late teardown must NOT delete the entity — B owns it now (the bug this fixes).
        assert!(
            !s.release(acct, a),
            "a superseded epoch must not own the entity"
        );
        // B's teardown does own it and deletes once; a double release is a no-op.
        assert!(s.release(acct, b), "the current epoch owns the entity");
        assert!(
            !s.release(acct, b),
            "releasing an already-released epoch is a no-op"
        );
    }

    #[test]
    fn distinct_accounts_are_independent() {
        let s = SessionEpochs::default();
        let e1 = s.claim(1);
        let e2 = s.claim(2);
        assert!(s.release(1, e1));
        assert!(s.release(2, e2));
    }
}
