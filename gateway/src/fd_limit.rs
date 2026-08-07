//! Raise this process's `RLIMIT_NOFILE` soft limit to its hard limit at startup (#451, from the
//! #447 root cause).
//!
//! # Why a server does this to itself
//!
//! A process may raise its own soft limit up to the hard limit with no privileges at all
//! (`setrlimit(2)`: "an unprivileged process may set only its soft limit... to a value in the range
//! from 0 up to the hard limit"). The soft limit is a *courtesy default* meant to catch runaway
//! programs, not a security boundary — which is why servers routinely lift it and why leaving it
//! alone is the unusual choice, not the safe one.
//!
//! The measured consequence of not doing it (2026-08-07, #447): a default Docker container has
//! `RLIMIT_NOFILE` soft **1024** against a hard limit of **524288**, and the gateway died at ~200
//! sessions with `Error: Too many open files (os error 24)` — a 512× headroom sitting unused. A live
//! session costs 3–5 descriptors: the client socket, the `try_clone` dup the writer thread owns, and
//! one SpacetimeDB websocket per shard the player's view touches (a dispersed realm opens more, per
//! #451's thread-per-player measurement). So the stock 1024 buys roughly 200 players on a five-shard
//! realm, and the operator gets no warning that this is the number.
//!
//! # Failure is never fatal
//!
//! Both syscalls are best-effort. If either fails we log and continue: a gateway running with a low
//! limit still serves the players it can fit, whereas one that refuses to start serves nobody. That
//! is also why this runs *before* the tokio runtime is built — the limit should be in place before
//! anything opens a descriptor, and it needs no runtime to do it.
//!
//! This is a mitigation for the fd *ceiling*, not for a leak. The per-account `PlayerConn` leak
//! (#449, `release_player_conn` is dead code) is a separate defect: raising the limit buys headroom,
//! it does not stop the accumulation. #451's third change — the accept loop surviving `EMFILE` — is
//! what keeps the realm alive when the headroom does run out.

use std::io;

/// What [`plan_raise`] decided to do about the limits it read. Split out from the syscalls so the
/// decision — including the "don't bother" and "never lower" cases — is a pure function with tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaisePlan {
    /// Soft is already at (or, pathologically, above) hard. Nothing to raise, no syscall to make.
    AlreadyAtHard { limit: u64 },
    /// Raise the soft limit from `from` to `to`.
    Raise { from: u64, to: u64 },
}

/// Decide what to do given the current soft/hard `RLIMIT_NOFILE` pair.
///
/// The only rule with teeth: **never lower**. `setrlimit` would happily accept a soft limit below
/// the current one and it is irreversible for the hard limit, so a container that has already been
/// launched with `--ulimit nofile=65536:65536` must come out of here untouched rather than
/// "normalised". A soft limit somehow *above* hard (which the kernel does not produce, but which we
/// should not react to by shrinking it) takes the same branch.
pub fn plan_raise(soft: u64, hard: u64) -> RaisePlan {
    if soft >= hard {
        RaisePlan::AlreadyAtHard { limit: soft }
    } else {
        RaisePlan::Raise {
            from: soft,
            to: hard,
        }
    }
}

/// Render a limit for a log line, spelling `RLIM_INFINITY` as `unlimited` rather than as
/// `18446744073709551615` — an operator reading the startup banner should not have to recognise
/// `u64::MAX` on sight.
pub fn describe_limit(v: u64) -> String {
    if v == unlimited() {
        "unlimited".to_string()
    } else {
        v.to_string()
    }
}

/// `RLIM_INFINITY` as a `u64`.
///
/// This module speaks `u64` throughout and relies on `libc::rlim_t` **being** `u64` — true on Linux
/// and macOS, the two platforms this workspace builds on. No `as` cast anywhere here, deliberately:
/// a platform with a narrower `rlim_t` should fail to compile loudly rather than have a cast
/// quietly truncate a limit. The `const` below is that check, evaluated at compile time.
const fn unlimited() -> u64 {
    libc::RLIM_INFINITY
}

/// Raise `RLIMIT_NOFILE` soft to hard, logging the before/after. Never fails: every error path logs
/// and returns.
///
/// Returns the soft limit in force when this returns, purely so tests and callers can assert on it;
/// the log line is the real product.
pub fn raise_nofile_soft_to_hard() -> Option<u64> {
    let (soft, hard) = match read_nofile() {
        Ok(pair) => pair,
        Err(e) => {
            log::warn!(
                "fd limit: could not read RLIMIT_NOFILE ({e}) — continuing with whatever limit was \
                 inherited. If sessions die with \"Too many open files\", raise it outside the \
                 process (`ulimit -n`, or docker `--ulimit nofile=65536`)."
            );
            return None;
        }
    };

    match plan_raise(soft, hard) {
        RaisePlan::AlreadyAtHard { limit } => {
            log::info!(
                "fd limit: RLIMIT_NOFILE soft is already at the hard limit ({}) — nothing to raise",
                describe_limit(limit)
            );
            Some(limit)
        }
        RaisePlan::Raise { from, to } => match set_nofile_soft(to, hard) {
            Ok(()) => {
                log::info!(
                    "fd limit: RLIMIT_NOFILE soft raised {} -> {} (hard {})",
                    describe_limit(from),
                    describe_limit(to),
                    describe_limit(hard)
                );
                Some(to)
            }
            Err(e) => {
                log::warn!(
                    "fd limit: could not raise RLIMIT_NOFILE soft {} -> {} ({e}) — continuing at \
                     {}. A live session costs 3-5 descriptors, so expect roughly {} concurrent \
                     players before accept starts failing with EMFILE.",
                    describe_limit(from),
                    describe_limit(to),
                    describe_limit(from),
                    from / 5
                );
                Some(from)
            }
        },
    }
}

fn read_nofile() -> io::Result<(u64, u64)> {
    let mut lim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `getrlimit` only writes through the pointer, and `lim` is a live, correctly typed,
    // fully initialised `rlimit` we own for the duration of the call.
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((lim.rlim_cur, lim.rlim_max))
}

fn set_nofile_soft(soft: u64, hard: u64) -> io::Result<()> {
    let lim = libc::rlimit {
        rlim_cur: soft,
        rlim_max: hard,
    };
    // SAFETY: `setrlimit` only reads through the pointer, and `lim` is a live, correctly typed,
    // fully initialised `rlimit` we own for the duration of the call.
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lim) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The container shape #447 died in: soft 1024, hard 524288 — 512x of headroom that nothing was
    /// claiming.
    #[test]
    fn the_default_docker_shape_gets_raised() {
        assert_eq!(
            plan_raise(1024, 524_288),
            RaisePlan::Raise {
                from: 1024,
                to: 524_288
            }
        );
    }

    /// An operator who already raised it outside the process must not be second-guessed, and — the
    /// property that actually matters — must never be silently LOWERED.
    #[test]
    fn an_already_raised_limit_is_left_alone() {
        assert_eq!(
            plan_raise(65_536, 65_536),
            RaisePlan::AlreadyAtHard { limit: 65_536 }
        );
        // Pathological (the kernel does not produce it), but shrinking would be the wrong answer.
        assert_eq!(
            plan_raise(65_536, 1024),
            RaisePlan::AlreadyAtHard { limit: 65_536 }
        );
    }

    /// The maintainer-workstation shape from #447's table: nothing to do, and no syscall made.
    #[test]
    fn an_unlimited_hard_limit_is_still_a_raise_when_soft_is_lower() {
        let inf = unlimited();
        assert_eq!(
            plan_raise(1024, inf),
            RaisePlan::Raise {
                from: 1024,
                to: inf
            }
        );
        assert_eq!(
            plan_raise(inf, inf),
            RaisePlan::AlreadyAtHard { limit: inf }
        );
    }

    #[test]
    fn unlimited_reads_as_a_word_not_as_u64_max() {
        assert_eq!(describe_limit(unlimited()), "unlimited");
        assert_eq!(describe_limit(1024), "1024");
        assert_eq!(describe_limit(524_288), "524288");
    }

    /// End to end against the real kernel — the whole claim of the module, checked for real rather
    /// than asserted about a plan. It mutates only this test process's own rlimit, and only upward.
    ///
    /// Deliberately tolerant of `setrlimit` being denied outright (a seccomp-filtered sandbox), so
    /// this reports the module's own bug and not the environment's policy: what is pinned is
    /// **"never lower, never move the hard limit"**, plus "reached the hard limit" whenever the
    /// syscall was actually allowed to run.
    #[test]
    fn raising_for_real_never_lowers_and_reaches_the_hard_limit_when_permitted() {
        let (soft_before, hard) = read_nofile().expect("getrlimit on our own process");
        let reported = raise_nofile_soft_to_hard().expect("getrlimit must not fail on ourselves");

        let (soft_after, hard_after) = read_nofile().expect("getrlimit on our own process");
        assert_eq!(
            soft_after, reported,
            "the reported limit is the one in force"
        );
        assert!(
            soft_after >= soft_before,
            "the soft limit must never be lowered ({soft_before} -> {soft_after})"
        );
        assert_eq!(hard_after, hard, "the hard limit must not move");
        if soft_after != hard {
            // A silent no-op is observationally identical to a refused syscall, so prove which one
            // happened instead of tolerating both: if the raise works when made directly, then
            // `raise_nofile_soft_to_hard` skipping it is a bug in this module.
            let e = set_nofile_soft(hard, hard).expect_err(
                "setrlimit succeeded when called directly but raise_nofile_soft_to_hard left the \
                 soft limit below hard — that is a bug here, not a sandbox policy",
            );
            eprintln!("setrlimit is denied in this environment ({e}); tolerated, see the doc");
        }
    }
}
