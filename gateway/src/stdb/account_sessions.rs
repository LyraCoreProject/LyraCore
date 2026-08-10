//! Per-account session-registry primitives, pure code-motion split out of `connection.rs`:
//! `SessionEpochs` (entity-ownership arbitration between two sockets on one account)
//! and `AccountSessions` (the live-socket refcount behind releasing a cached `PlayerConn`).
//! Neither depends on `spacetimedb_sdk` — this is pure sync-primitive bookkeeping,
//! not connection wiring.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Per-account "current in-world session" tracking. The world gateway opens one TCP session per
/// socket, but the cached `PlayerConn` shares ONE node identity across reconnects — so when a stale
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

/// Per-account live-SOCKET refcount — the arbitration behind releasing a cached `PlayerConn`.
///
/// **Why this is not `SessionEpochs`.** The epoch above arbitrates who owns the *entity*: exactly
/// one IN-WORLD session per account is current, and a superseded one must not delete the live
/// player's row. That is a strictly narrower question than the one a connection release has to
/// answer, because the cached `PlayerConn` is shared by every socket on the account — including a
/// socket sitting at CHARACTER SELECT, which holds no epoch at all. Releasing on
/// `release_session(..) == true` alone would therefore be wrong in a real (if narrow) case: socket A
/// is in-world, socket B has authenticated and is at character select, A drops. A owns the epoch, so
/// the epoch gate says "release" — but B is live and its next `player_conn` checkout would silently
/// mint a NEW identity that `establish_session` has not bound, failing a char create/delete issued
/// before B's `player_login` re-binds it.
///
/// So the release predicate is the strictly stronger one: **the last world socket for this account
/// has gone away**. When the count reaches zero nobody can be holding the connection, in-world or
/// not, and the epoch question does not need to be asked at all.
///
/// Not derivable from the `players` cache (`session_count`): that map is keyed by account and says
/// nothing about how many sockets are using an entry.
///
/// # The logon tier and the handover grace
///
/// A LOGON socket is a live user of the same cached connection: `bound_identity` is what OPENS it,
/// so a logon that authenticates and then walks away — an abandoned login, a client that reaches
/// the realm list and quits — leaks exactly the fd + pump thread the original per-account
/// connection leak was about, on a path the world-tier release never reached. So logon sockets
/// attach here too.
///
/// Their detach is [`AccountSessions::detach_deferred`] rather than [`AccountSessions::detach`],
/// because the account's NEXT socket is normally the world session, which reuses the connection the
/// logon just opened and whose identity `establish_session` has already bound into `game_session`
/// and onto `game_character.owner_identity`. Releasing on the logon socket's close would not merely
/// cost a rebuild on every login (undoing the connection reuse the per-account reclaim relies
/// on): the rebuilt connection mints a DIFFERENT
/// identity than the one that was bound moments earlier, so the world phase's
/// `account_by_identity` lookups fail until the next `establish_session`.
///
/// And the two closes are not ordered. Whether the 1.12 client drops its logon socket before or
/// after the world handshake is a CLIENT behaviour we neither control nor want to depend on, so
/// the grace makes both orders safe: a deferred detach parks the account instead of releasing it,
/// any `attach` inside the window cancels the park (that IS the handover), and only
/// [`AccountSessions::reap_idle`] — driven by
/// [`Coordinator::spawn_account_session_reaper`] — releases what nobody claimed.
///
/// The world tier keeps its IMMEDIATE detach: by the time a world socket ends, the handover
/// question is settled, and delaying that release would slow the fd/thread reclaim the ramp
/// measured.
#[derive(Default)]
pub(crate) struct AccountSessions {
    live: Mutex<HashMap<u64, AccountEntry>>,
}

/// One account's row in [`AccountSessions`].
#[derive(Default)]
struct AccountEntry {
    /// Live sockets on this account — world sessions and logon handshakes alike.
    live: u32,
    /// Set when the account's last socket detached DEFERRED (a logon close, see
    /// [`AccountSessions::detach_deferred`]): the connection is kept, unreferenced, until either an
    /// `attach` claims it or the grace elapses. Always `None` while `live > 0`.
    idle_since: Option<Instant>,
}

impl AccountSessions {
    /// The live-socket-refcount map, recovering a poisoned lock (matches `connection.rs`'s
    /// `.lock().unwrap()` discipline, e.g. `coord()`/`call_pipe()`) — a stale read beats poisoning
    /// every future attach/detach for every account on the shard.
    fn live(&self) -> std::sync::MutexGuard<'_, HashMap<u64, AccountEntry>> {
        self.live.lock().unwrap_or_else(|p| {
            log::error!(
                "account-session lock poisoned (a prior panic in a critical section) — recovering"
            );
            p.into_inner()
        })
    }

    /// A socket for `account_id` (world session or logon handshake) may now use the account's
    /// cached connection. Cancels any pending idle release — this is the logon→world handover.
    pub(crate) fn attach(&self, account_id: u64) {
        let mut live = self.live();
        let e = live.entry(account_id).or_default();
        e.live += 1;
        e.idle_since = None;
    }

    /// A WORLD socket for `account_id` has torn down. Returns true iff it was the LAST one — i.e.
    /// the account has no live socket anywhere in the gateway and its cached connection is
    /// unreachable, so releasing it cannot cut anybody's link.
    pub(crate) fn detach(&self, account_id: u64) -> bool {
        let mut live = self.live();
        let Some(e) = live.get_mut(&account_id) else {
            // Defensive: a detach with no matching attach must never claim the account is idle.
            log::warn!("account-session: detach for account {account_id} with no live session recorded");
            return false;
        };
        if e.live == 0 {
            // Parked by a deferred detach and now detached again: the count is already zero, so
            // this cannot be the last socket going away. Bailing rather than falling through to
            // `-= 1`, because an unmatched detach must never underflow into "2^32 live sockets" —
            // that would pin the connection for the process lifetime, i.e. the very leak.
            log::warn!("account-session: detach for idle account {account_id} with no live session recorded");
            return false;
        }
        e.live -= 1;
        if e.live == 0 {
            live.remove(&account_id);
            return true;
        }
        false
    }

    /// A LOGON socket for `account_id` has closed. Never releases: if this was the account's last
    /// socket the entry is PARKED at `now`, so a world session arriving inside the grace reuses the
    /// connection the logon opened (see the type's doc), and [`AccountSessions::reap_idle`]
    /// releases it only if none does.
    pub(crate) fn detach_deferred(&self, account_id: u64, now: Instant) {
        let mut live = self.live();
        let Some(e) = live.get_mut(&account_id) else {
            log::warn!("account-session: logon detach for account {account_id} with no live session recorded");
            return;
        };
        if e.live == 0 {
            log::warn!("account-session: logon detach for idle account {account_id} — already parked");
            return;
        }
        e.live -= 1;
        if e.live == 0 {
            e.idle_since = Some(now);
        }
    }

    /// Accounts parked by [`AccountSessions::detach_deferred`] at least `grace` ago and still
    /// unclaimed: their entries are dropped and their ids returned for the caller to release.
    ///
    /// `idle_since` is the WHOLE predicate, and deliberately so. It is `Some` only while the
    /// account has no socket at all — `detach_deferred` sets it exactly when the count reaches
    /// zero, and [`AccountSessions::attach`] clears it, which is what makes a world session's
    /// arrival cancel (not merely postpone) the release. Re-testing `live == 0` here would look
    /// safer while making both halves redundant, so neither could be broken on its own and no test
    /// could tell: the `debug_assert` below asserts the invariant instead of duplicating it.
    pub(crate) fn reap_idle(&self, now: Instant, grace: Duration) -> Vec<u64> {
        let mut live = self.live();
        let due: Vec<u64> = live
            .iter()
            .filter(|(_, e)| {
                let parked = e.idle_since.is_some_and(|t| now.duration_since(t) >= grace);
                debug_assert!(
                    !parked || e.live == 0,
                    "an account with live sockets is parked for release — some path set \
                     `idle_since` without the count reaching zero, or `attach` stopped clearing it"
                );
                parked
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &due {
            live.remove(id);
        }
        due
    }

    /// Live sockets for `account_id` (tests + diagnostics).
    #[cfg(test)]
    pub(crate) fn live_count(&self, account_id: u64) -> u32 {
        self.live().get(&account_id).map(|e| e.live).unwrap_or(0)
    }

    /// Is `account_id` parked awaiting the handover grace? (tests)
    #[cfg(test)]
    pub(crate) fn is_parked(&self, account_id: u64) -> bool {
        self.live()
            .get(&account_id)
            .is_some_and(|e| e.idle_since.is_some())
    }
}

#[cfg(test)]
mod account_session_tests {
    use super::AccountSessions;
    use std::time::{Duration, Instant};

    #[test]
    fn only_the_last_socket_releases_the_connection() {
        let s = AccountSessions::default();
        let acct = 42;
        s.attach(acct); // socket A
        s.attach(acct); // socket B re-logs on the same account while A is still up
        assert!(
            !s.detach(acct),
            "A's teardown must NOT release the connection B is still using"
        );
        assert_eq!(s.live_count(acct), 1);
        assert!(s.detach(acct), "B's teardown is the last one — release");
        assert_eq!(s.live_count(acct), 0);
    }

    #[test]
    fn a_single_socket_releases_on_its_own_teardown() {
        let s = AccountSessions::default();
        s.attach(7);
        assert!(s.detach(7));
    }

    #[test]
    fn accounts_are_independent() {
        let s = AccountSessions::default();
        s.attach(1);
        s.attach(2);
        assert!(s.detach(1));
        assert!(s.detach(2));
    }

    #[test]
    fn an_unmatched_detach_never_claims_the_account_is_idle() {
        // A detach with no attach (a teardown path that skipped the attach) must be inert rather
        // than release a connection some other socket could be holding.
        let s = AccountSessions::default();
        assert!(!s.detach(99));
    }

    // -------------------------------------------------------------------------------------
    // The logon tier: `detach_deferred` + `reap_idle` — the four danger cases.
    // -------------------------------------------------------------------------------------

    const GRACE: Duration = Duration::from_secs(120);

    /// The leak this issue is about: a logon that authenticates (opening the account's cached
    /// connection) and never proceeds to a world session. Nothing else will ever detach it, so the
    /// reaper is the ONLY thing that can reclaim its fd + pump thread.
    #[test]
    fn a_logon_that_never_enters_the_world_is_reclaimed_after_the_grace() {
        let s = AccountSessions::default();
        let t0 = Instant::now();
        s.attach(5); // logon socket, at `bound_identity`
        s.detach_deferred(5, t0); // client quit at the realm list
        assert!(s.is_parked(5), "the connection is parked, not yet released");
        assert!(
            s.reap_idle(t0 + GRACE - Duration::from_secs(1), GRACE)
                .is_empty(),
            "the grace has not elapsed — nothing may be released yet"
        );
        assert_eq!(s.reap_idle(t0 + GRACE, GRACE), vec![5]);
        assert!(
            s.reap_idle(t0 + GRACE * 2, GRACE).is_empty(),
            "a reaped account is gone from the table — no double release"
        );
    }

    /// DANGER CASE 1 (the handover). The client closes its logon socket the instant it opens the
    /// world one, and the two closes are not ordered. If the logon close released, the world
    /// session would rebuild a connection whose identity `establish_session` has NOT bound.
    #[test]
    fn a_world_session_inside_the_grace_takes_over_the_connection() {
        let s = AccountSessions::default();
        let t0 = Instant::now();
        s.attach(5); // logon socket
        s.detach_deferred(5, t0); // logon socket closes FIRST (client picked a realm)
        s.attach(5); // the world session's handshake, milliseconds later
        assert!(!s.is_parked(5), "the handover cancels the pending release");
        assert!(
            s.reap_idle(t0 + GRACE * 10, GRACE).is_empty(),
            "the world session is LIVE on this connection — reaping it would cut the player's link"
        );
        // …and when the world session does end, the release is immediate, exactly as the
        // world-tier release already does.
        assert!(s.detach(5));
    }

    /// The other order of the same handover: the world session attaches while the logon socket is
    /// still open. The logon close then finds a live socket and parks nothing at all.
    #[test]
    fn a_logon_close_while_the_world_session_is_live_parks_nothing() {
        let s = AccountSessions::default();
        let t0 = Instant::now();
        s.attach(5); // logon socket
        s.attach(5); // world session opens before the client drops the logon socket
        s.detach_deferred(5, t0); // logon socket closes
        assert_eq!(s.live_count(5), 1);
        assert!(!s.is_parked(5));
        assert!(s.reap_idle(t0 + GRACE * 10, GRACE).is_empty());
    }

    /// DANGER CASE 2. A world session is ALREADY live when a fresh logon arrives for the same
    /// account (a second client, or a re-auth while the first is playing). The logon's close must
    /// not park — let alone release — the connection the seated player is using.
    #[test]
    fn a_fresh_logon_never_disturbs_a_live_world_session() {
        let s = AccountSessions::default();
        let t0 = Instant::now();
        s.attach(7); // world session, in-world
        s.attach(7); // a fresh logon for the same account
        s.detach_deferred(7, t0); // the logon socket goes away
        assert_eq!(s.live_count(7), 1);
        assert!(
            s.reap_idle(t0 + GRACE * 10, GRACE).is_empty(),
            "the seated player's connection must never be reaped"
        );
        assert!(s.detach(7), "the world session's own teardown still releases");
    }

    /// DANGER CASE 3. Two concurrent logons on one account: the first close must not release the
    /// connection the second is using, and the pair must release exactly once.
    #[test]
    fn two_concurrent_logons_release_once_and_only_at_the_last() {
        let s = AccountSessions::default();
        let t0 = Instant::now();
        s.attach(9);
        s.attach(9);
        s.detach_deferred(9, t0);
        assert!(!s.is_parked(9), "one logon socket is still using it");
        assert!(s.reap_idle(t0 + GRACE * 10, GRACE).is_empty());
        s.detach_deferred(9, t0 + Duration::from_secs(1));
        // Parked at the SECOND close, so the grace runs from there, not from the first.
        assert!(s
            .reap_idle(t0 + Duration::from_secs(1) + GRACE - Duration::from_millis(1), GRACE)
            .is_empty());
        assert_eq!(s.reap_idle(t0 + Duration::from_secs(1) + GRACE, GRACE), vec![9]);
    }

    /// A parked account that is detached again (a teardown path that ran twice) must not underflow
    /// the count — `0u32 - 1` is 4 billion live sockets, i.e. a connection pinned for the process
    /// lifetime, which is the exact leak this whole issue is about.
    #[test]
    fn a_detach_on_a_parked_account_neither_underflows_nor_releases() {
        let s = AccountSessions::default();
        let t0 = Instant::now();
        s.attach(3);
        s.detach_deferred(3, t0);
        assert!(!s.detach(3), "already parked — not a last-socket teardown");
        s.detach_deferred(3, t0);
        assert_eq!(s.live_count(3), 0);
        assert_eq!(
            s.reap_idle(t0 + GRACE, GRACE),
            vec![3],
            "still exactly one release"
        );
    }

    /// A deferred detach with no attach at all is inert (mirrors the `detach` guard above).
    #[test]
    fn an_unmatched_deferred_detach_parks_nothing() {
        let s = AccountSessions::default();
        let t0 = Instant::now();
        s.detach_deferred(99, t0);
        assert!(s.reap_idle(t0 + GRACE, GRACE).is_empty());
    }

    #[test]
    fn parked_accounts_are_independent() {
        let s = AccountSessions::default();
        let t0 = Instant::now();
        s.attach(1);
        s.attach(2);
        s.detach_deferred(1, t0);
        s.detach_deferred(2, t0 + GRACE);
        let mut due = s.reap_idle(t0 + GRACE, GRACE);
        due.sort_unstable();
        assert_eq!(due, vec![1], "only account 1's grace has elapsed");
        assert_eq!(s.reap_idle(t0 + GRACE * 2, GRACE), vec![2]);
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
