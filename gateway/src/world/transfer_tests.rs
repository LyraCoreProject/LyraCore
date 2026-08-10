//! Cross-database transfer — Phase A of the elastic-sharding spec.
//!
//! A child module of `world::tests` for the same reason as its siblings — it reaches
//! `InMemoryStore` and the fixtures below without widening anything. Unlike the other extracted
//! sections, the fixture TYPES this section drives (`FakeChar`, `FakeEscrow`, `FakeShardDb`,
//! `fake_blob`/`parse_blob`, and the generic `lk` lock helper) stay defined in `tests.rs` itself:
//! tests.rs's own `InMemoryStore` (the `xdb`/`xstep` glue its `Store` impl uses) and two
//! world-port-abort regression tests earlier in that file construct `FakeShardDb`/`FakeChar`
//! directly, so those definitions are a shared fixture, not section-local — see the comment above
//! them in `tests.rs`. `sharded_stores`/`drive_routed_session`/`ShardCallLog` are `shard_routing_tests`'s,
//! reused here the same way `loot_tests` reuses `party_tests`'s fixtures.

use super::shard_routing_tests::{drive_routed_session, sharded_stores, ShardCallLog};
use super::*;

// ===========================================================================================
//  Cross-database transfer — Phase A of the elastic-sharding spec.
//
//  `FakeShardDb` is a faithful re-implementation of the MODULE's escrow guards
//  (`module/src/transfer/mod.rs`'s `plan_begin`/`plan_import`/`plan_finish` + `release_transfer`'s
//  source check), so these tests exercise the one thing the module cannot check for itself: the
//  ORDER the gateway drives two databases in, because each database can only see its own ledger.
//  Two `FakeShardDb`s stand for two SpacetimeDB databases — the same shape `sharded_stores`
//  uses for routing.
//
//  Deliberately NOT a permissive mock: a fake that recorded calls and returned Ok would let every
//  ordering mutation pass, which is the exact coverage gap the transfer-primitive and
//  in-transit-gate reviews kept finding.
// ===========================================================================================

/// Run a test body under a wall-clock deadline, so a hang is a FAILURE rather than a CI job that
/// sits at "still running" until someone kills it. Used on the cross-database driver tests — the
/// ones that walk two databases through a multi-step protocol and are therefore the only place in
/// this suite where a wedge could be a loop rather than a lock.
fn no_hang<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let h = std::thread::spawn(f);
    // Poll `is_finished` rather than shipping the result through a channel, so a body that PANICS
    // still propagates its own panic message (via `resume_unwind`) instead of being reported as a
    // hang. The hang is the only thing this wrapper is allowed to rename.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while !h.is_finished() {
        assert!(
            std::time::Instant::now() < deadline,
            "test body did not finish within {secs}s — treating the hang as a FAILURE. \
             A `cargo test` with no per-test timeout reports a wedged test as 'still running', \
             which reads as neither a pass nor a fail; it must read as a fail."
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    match h.join() {
        Ok(v) => v,
        Err(e) => std::panic::resume_unwind(e),
    }
}

/// A store handle over a `FakeShardDb`, with an optional injected failure at one named step — how
/// "the gateway was killed here" is simulated (that step's transaction never commits).
fn xstore(
    shard: &str,
    db: std::sync::Arc<FakeShardDb>,
    calls: ShardCallLog,
    kill_at: Option<&str>,
) -> std::sync::Arc<InMemoryStore> {
    std::sync::Arc::new(InMemoryStore {
        shard: shard.into(),
        calls,
        xdb: Some(db),
        kill_at: kill_at.map(|s| s.to_string()),
        ..Default::default()
    })
}

const XGUID: u64 = 1;

/// A fresh two-database topology: the character is resident on `world`, and its durable row already
/// names the instance destination — which is what `teleport_player` writes before it despawns the
/// entity for a cross-map hop, i.e. the state the WORLDPORT_ACK handler finds.
#[allow(clippy::type_complexity)]
fn xdb_pair(
    kill_at: Option<&str>,
) -> (
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<InMemoryStore>,
    std::sync::Arc<FakeShardDb>,
    std::sync::Arc<FakeShardDb>,
    ShardCallLog,
) {
    let calls: ShardCallLog = Default::default();
    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    let dst_db = FakeShardDb::empty();
    let src = xstore("world", src_db.clone(), calls.clone(), kill_at);
    let dst = xstore("instances", dst_db.clone(), calls.clone(), kill_at);
    (src, dst, src_db, dst_db, calls)
}

/// A deadlock found in review, turned into a named failure.
///
/// `FakeShardDb::import_character_blob` used to hold the `in_rows` guard across `db.live()`, which
/// locks `in_rows` again — only the `&&` short-circuit in `has()` kept the happy path alive. When a
/// driver mutation reached that line the gateway suite HUNG instead of turning a test red. Every
/// lock now goes through `lk` (`try_lock`), so the same re-entrancy is an instant, named panic.
///
/// This test asserts the property directly: hold a guard, take the same mutex again, and the
/// process must come back with a failure rather than never coming back at all.
#[test]
fn a_re_entrant_lock_on_the_fake_shard_db_fails_instead_of_hanging() {
    let db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    let _held = lk(&db.in_rows);
    // `live()` reads `in_rows`. With `lock()` this call never returns.
    let hit = no_hang(5, {
        let db = db.clone();
        move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| db.live(XGUID)))
                .err()
                .map(|e| {
                    e.downcast_ref::<String>().cloned().unwrap_or_else(|| {
                        e.downcast_ref::<&str>()
                            .map(|s| s.to_string())
                            .unwrap_or_default()
                    })
                })
        }
    });
    let msg = hit.expect(
        "a re-entrant lock on FakeShardDb did not fail — it either succeeded (the fake is no \
         longer mutex-guarded) or it would have hung, and a hang is not a pass",
    );
    assert!(
        msg.contains("re-entrant lock on FakeShardDb"),
        "unexpected panic: {msg}"
    );
}

#[test]
fn a_character_moves_whole_between_two_databases_with_its_rows() {
    // Wall-clock net: a wedged driver must FAIL, not hang the suite.
    no_hang(30, || {
        let (src, dst, src_db, dst_db, calls) = xdb_pair(None);
        super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID)
            .expect("transfer completes");

        assert!(
            !src_db.has(XGUID),
            "the source copy must be destroyed (delete-last)"
        );
        assert!(
            dst_db.live(XGUID),
            "the character must be LIVE at the destination"
        );
        assert_eq!(
            dst_db.get(XGUID).unwrap(),
            FakeChar { map_id: 36, instance_id: 7, payload: "gear+spells".into() },
            "the character-owned ROWS must arrive, not just its identity — a manifest-only blob lands \
             a naked character with no gear, spells or quest log"
        );
        assert!(
            src_db.settled() && dst_db.settled(),
            "no escrow row may outlive a completed transfer"
        );
        assert!(
            lk(&dst_db.instances).contains(&7),
            "the instance must be mirrored onto the destination shard"
        );
        assert_eq!(
            *lk(&src_db.evicted),
            vec![7],
            "the source shard must stop ticking the instance once the run has moved"
        );
        let log = calls.lock().unwrap().clone();
        assert_eq!(
            log,
            vec![
                // The speculative fence-clear on the SOURCE, before anything else: the transfer id is
                // the character guid, so an arrival in-row left here by an earlier hop would make
                // `begin_transfer` replay into a no-op (see
                // `a_second_transfer_of_the_same_character_is_never_swallowed_as_a_replay`). It costs
                // one no-op reducer call on a fresh transfer and is the same cheap release the
                // already-home path makes.
                ("world".to_string(), "release_transfer".to_string()),
                ("world".to_string(), "begin_transfer".to_string()),
                ("instances".to_string(), "ensure_instance".to_string()),
                ("instances".to_string(), "import_character_blob".to_string()),
                ("world".to_string(), "confirm_import".to_string()),
                ("world".to_string(), "finish_transfer".to_string()),
                // realm-core learns where the character settled HERE — after the escrow's own
                // transaction committed, before the arrival copy goes live.
                ("world".to_string(), "publish_shard_index".to_string()),
                ("instances".to_string(), "release_transfer".to_string()),
                ("world".to_string(), "evict_instance_population".to_string()),
            ],
            "the step ORDER is the safety property neither database can check for itself"
        );
    });
}

/// The realm-core character→shard index is written BY THE TRANSFER, not left for
/// a future login's probe to discover.
///
/// Before this, `set_character_shard` had exactly one caller in the whole gateway — the login
/// self-heal — so a completed cross-database transfer updated the SOURCE database's copy of the
/// index (transactionally, inside `finish_transfer`) and nothing else. The copy `home_shard`
/// actually reads is realm-core's, and it learned about the move at the character's next login, by
/// probing every shard. The requirement that realm-core's index be correct without relying on that
/// probe was unmet, and looked correct only because the probe masked it.
#[test]
fn a_completed_transfer_publishes_the_destination_to_the_realm_core_index() {
    no_hang(30, || {
        let (src, dst, _src_db, _dst_db, _calls) = xdb_pair(None);
        super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID)
            .expect("transfer completes");
        assert_eq!(
            *src.realm_index.lock().unwrap(),
            vec![(XGUID, 36, 7)],
            "the drive settled the character on map 36 / instance 7 and told realm-core nothing. \
             Without this write the index is only ever corrected by the login self-heal, so every \
             login pays a full shard probe to rediscover a fact the transfer already knew — and both \
             the transfer flow and the region-routing seam route on an index that is never true. \
             The published location must be the ESCROW's destination, which is what \
             `finish_transfer` just settled."
        );
    });
}

/// Step 5b publishes the ESCROW OUT-ROW's destination, never the caller's `plan`.
///
/// This is the clause the whole "a replication, not a stale-index generator" argument rests on —
/// the index can only ever name a destination `finish_transfer` actually settled, because it is read
/// from the same row `do_finish` recorded its own receipt from. Every other clause was executed;
/// this one was not, and substituting `plan.dest_*` for `escrow.dest_*` survived the whole suite
/// (found by adversarial review). The two agree on today's call paths, which is exactly why nothing
/// noticed — and `run_transfer` re-reads the escrow precisely because they are not guaranteed to.
#[test]
fn a_resumed_transfer_publishes_the_escrow_destination_not_the_callers_plan() {
    no_hang(30, || {
        let (src, dst, _src_db, _dst_db, _calls) = xdb_pair(None);
        // Open the escrow against the destination the durable row names (map 36 / instance 7).
        let opened = src
            .character_destination(XGUID)
            .expect("the durable row names a destination");
        src.begin_transfer(&opened).expect("the escrow opens");
        // Now drive with a plan naming somewhere ELSE. `begin_transfer` answers `Replay` — the row
        // on disk is the authority and the plan is ignored — so the transfer settles at 36/7.
        let stale = super::transfer::TransferPlan {
            dest_map_id: 0,
            dest_instance_id: 0,
            ..opened
        };
        super::transfer::run_transfer_injected(src.as_ref(), dst.as_ref(), &stale, None)
            .expect("the drive completes against the escrow on disk");
        assert_eq!(
            *src.realm_index.lock().unwrap(),
            vec![(XGUID, 36, 7)],
            "the index was published from the DRIVER'S PLAN instead of the escrow out-row. The plan \
             is whatever the caller happened to hand in; the escrow is what `finish_transfer` just \
             settled and what `do_finish` wrote the source's own receipt from. Publishing the plan \
             makes step 5b able to name a destination the transfer did not go to — the exact \
             stale-index generator this write exists to rule out — and it does so silently, \
             because on the ordinary call paths the two happen to agree."
        );
    });
}

/// The write is a REQUIRED step of the drive, not a best-effort side call: an unreachable
/// realm-core fails the transfer rather than silently leaving the directory wrong.
#[test]
fn a_transfer_whose_index_publish_fails_does_not_report_success() {
    no_hang(30, || {
        let calls: ShardCallLog = Default::default();
        let src_db = FakeShardDb::with_character(
            XGUID,
            FakeChar {
                map_id: 36,
                instance_id: 7,
                payload: "gear+spells".into(),
            },
        );
        let dst_db = FakeShardDb::empty();
        let src = std::sync::Arc::new(InMemoryStore {
            shard: "world".into(),
            calls: calls.clone(),
            xdb: Some(src_db.clone()),
            publish_error: Some("realm-core database lyracore-realm is not connected".into()),
            ..Default::default()
        });
        let dst = xstore("instances", dst_db.clone(), calls.clone(), None);
        let err = super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID)
            .expect_err("a failed index publish must fail the drive");
        assert!(
            err.to_string().contains("lyracore-realm"),
            "the failure must name realm-core, not be swallowed: a publish that shrugged off an \
             unreachable index would be exactly the best-effort, independently-committing write \
             this fix exists to remove. Got: {err:#}"
        );

        // …and the failure is RECOVERABLE, which is what makes propagating it safe: the character
        // is already whole at the destination, only fenced, so a fresh driver with a working
        // realm-core finishes the job. Nothing is lost and nothing is duplicated.
        assert!(!src_db.has(XGUID) && dst_db.has(XGUID) && !dst_db.live(XGUID));
        let src2 = xstore("world", src_db.clone(), calls.clone(), None);
        let dst2 = xstore("instances", dst_db.clone(), calls, None);
        super::transfer::settle_transfer(dst2.as_ref(), dst2.as_ref(), XGUID)
            .expect("a fresh driver recovers the fenced arrival copy");
        assert!(dst_db.live(XGUID) && dst_db.settled() && src_db.settled());
        drop(src2);
    });
}

#[test]
fn a_gateway_kill_at_every_transfer_step_recovers_to_exactly_one_whole_copy() {
    // Wall-clock net: a wedged driver must FAIL, not hang the suite.
    no_hang(30, || {
        // Headless half: kill the driver at every step boundary, then let a fresh STATELESS
        // driver re-run — the character ends whole on exactly one shard, every time.
        //
        // Driven off `ABORT_STEPS` itself rather than a literal copy of it. The index-publish fix
        // added step 5b (`publish_shard_index`) to `ABORT_STEPS` and to the drive, but the literal
        // list here was not updated — so the one boundary that fix introduced was the one boundary
        // this matrix did not kill at, while that fix's own PR reported the matrix as covering it.
        // A hand-copied list of the thing under test can only ever drift in the direction that
        // loses coverage.
        for kill_at in super::transfer::ABORT_STEPS {
            let (src, dst, src_db, dst_db, _) = xdb_pair(Some(kill_at));
            let first = super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID);
            if kill_at == "evict_instance_population" {
                // The eviction is deliberately best-effort: the character is already whole by then, so
                // failing the player's login over a performance wart would be strictly worse.
                first.expect("an eviction failure must not fail the transfer");
            } else {
                assert!(
                    first.is_err(),
                    "the injected kill at {kill_at} must abort the drive"
                );
            }

            // INVARIANT AT THE CRASH POINT.
            assert!(
                src_db.has(XGUID) || dst_db.has(XGUID),
                "ZERO durable copies after a kill at {kill_at} — the character was lost"
            );
            assert!(
                !(src_db.live(XGUID) && dst_db.live(XGUID)),
                "the character is LIVE on both databases after a kill at {kill_at} — a dupe"
            );

            // A brand-new driver with NO memory of the interrupted attempt: it re-derives the plan from
            // durable state alone (the escrow row, or the character row's own destination), which is
            // the whole of gateway-restart recovery.
            let calls: ShardCallLog = Default::default();
            let src2 = xstore("world", src_db.clone(), calls.clone(), None);
            let dst2 = xstore("instances", dst_db.clone(), calls.clone(), None);
            let holder: &dyn WorldStore = if src_db.has(XGUID) {
                src2.as_ref()
            } else {
                dst2.as_ref()
            };
            super::transfer::settle_transfer(holder, dst2.as_ref(), XGUID)
                .unwrap_or_else(|e| panic!("recovery after a kill at {kill_at} failed: {e:#}"));

            assert!(
                !src_db.has(XGUID),
                "after recovering from a kill at {kill_at} the source copy must be gone"
            );
            assert!(
                dst_db.live(XGUID),
                "after recovering from a kill at {kill_at} the character must be live at the destination"
            );
            assert_eq!(
                dst_db.get(XGUID).unwrap().payload,
                "gear+spells",
                "recovery from a kill at {kill_at} must not lose the character-owned rows"
            );
            assert!(
                src_db.settled() && dst_db.settled(),
                "recovery from a kill at {kill_at} left an escrow row behind"
            );
        }
    });
}

/// `LYRACORE_TRANSFER_ABORT_AFTER=<step>` must let the named step COMMIT and then kill the driver
/// before the next one — that is the only way the live crash-recovery matrix can aim at a specific
/// crash boundary in a drive that completes in ~17ms. In a `cargo test` build the injected death is
/// a panic rather than `process::abort()` (see `transfer::die_by_injection`), so it is observable here.
#[test]
fn an_injected_abort_stops_the_driver_after_the_named_step_and_before_the_next() {
    for (i, step) in super::transfer::ABORT_STEPS.iter().enumerate() {
        let (src, dst, src_db, dst_db, calls) = xdb_pair(None);
        let plan = src
            .character_destination(XGUID)
            .expect("the durable row names the destination");
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            super::transfer::run_transfer_injected(src.as_ref(), dst.as_ref(), &plan, Some(step))
        }));
        assert!(
            outcome.is_err(),
            "LYRACORE_TRANSFER_ABORT_AFTER={step} did not kill the driver — it returned {:?}. A step that \
             merely returns (Ok or Err) is a CLEAN exit and reproduces nothing about a kill -9.",
            outcome.as_ref().map(|r| r.is_ok())
        );

        // The step named must have RUN (its transaction committed), and nothing after it may have.
        let log = calls.lock().unwrap().clone();
        let names: Vec<&str> = log.iter().map(|(_, n)| n.as_str()).collect();
        assert_eq!(
            names.last().copied(),
            Some(*step),
            "LYRACORE_TRANSFER_ABORT_AFTER={step} left the shard-call log ending at {:?} — the abort must \
             land AFTER {step} commits, not before it and not after a later step",
            names.last()
        );
        assert_eq!(
            names.len(),
            i + 1,
            "LYRACORE_TRANSFER_ABORT_AFTER={step} drove {} shard calls ({names:?}) — expected exactly the \
             {} steps up to and including {step}",
            names.len(),
            i + 1
        );

        // And the same invariant the live matrix asserts against the two real databases.
        assert!(
            src_db.has(XGUID) || dst_db.has(XGUID),
            "ZERO durable copies after an injected abort at {step} — the character was lost"
        );
        assert!(
            !(src_db.live(XGUID) && dst_db.live(XGUID)),
            "the character is LIVE on both databases after an injected abort at {step} — a dupe"
        );
    }
}

/// The unconfigured default must be indistinguishable from having no injection point at all: same
/// shard calls, same order, same result. (This repo has shipped three "unconfigured is
/// byte-identical" violations already; this is the guard against a fourth.)
#[test]
fn an_unset_transfer_abort_injection_changes_nothing() {
    assert_eq!(
        std::env::var("LYRACORE_TRANSFER_ABORT_AFTER").ok(),
        None,
        "LYRACORE_TRANSFER_ABORT_AFTER is set in this test process — the fault injector is opt-in and no \
         normal run (or test run) may have it in the environment"
    );

    let (src, dst, src_db, dst_db, calls) = xdb_pair(None);
    let plan = src.character_destination(XGUID).unwrap();
    super::transfer::run_transfer_injected(src.as_ref(), dst.as_ref(), &plan, None)
        .expect("an unconfigured drive must complete exactly as before");

    let injected: Vec<String> = calls
        .lock()
        .unwrap()
        .iter()
        .map(|(_, n)| n.clone())
        .collect();
    assert_eq!(
        injected,
        super::transfer::ABORT_STEPS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "an unconfigured drive must run every step, in order, and nothing else"
    );
    assert!(
        !src_db.has(XGUID) && dst_db.live(XGUID),
        "and land the character whole at the destination"
    );
}

/// Source-scan tripwire for the one line no in-process test can reach: `run_transfer`'s ENV WIRING.
///
/// Both tests above drive `run_transfer_injected` directly — deliberately, so a parallel test
/// runner never has process-global env mutated underneath it — which leaves the wrapper that
/// actually arms the injector in production completely unexercised. Found by mutation during this
/// PR's review: replacing the call's last argument with a literal `None` (the injector still
/// present, still compiled, permanently DISARMED) left all 370 gateway tests GREEN, while
/// `LYRACORE_TRANSFER_ABORT_AFTER` did nothing and every step of the live crash-recovery matrix
/// would time out waiting for a death that can no longer happen.
///
/// The unmatched-step warning is pinned here for the same reason: it is the only thing standing
/// between a typo'd step name and a crash matrix that reports PASS for a crash that never fired,
/// and no in-process test asserts a log line.
#[test]
fn run_transfer_still_arms_the_injector_from_the_environment() {
    let src = include_str!("transfer.rs");
    let at = src
        .find("pub fn run_transfer(")
        .expect("`run_transfer` moved");
    let end = src[at..].find("\n}\n").expect("`run_transfer` body");
    let body = &src[at..at + end];
    assert!(
        body.contains("std::env::var(\"LYRACORE_TRANSFER_ABORT_AFTER\")"),
        "`run_transfer` no longer reads LYRACORE_TRANSFER_ABORT_AFTER — the injector is dead in the \
         PRODUCTION build (the tests call `run_transfer_injected` directly and stay green). Body \
         was:\n{body}"
    );
    assert!(
        body.contains("run_transfer_injected(src, dst, plan, abort_after.as_deref())"),
        "`run_transfer` reads the env but no longer THREADS it into `run_transfer_injected` — the \
         read is decorative and every crash point is permanently disarmed. Body was:\n{body}"
    );
    assert!(
        body.contains("ABORT_STEPS.contains(&step)"),
        "`run_transfer` no longer validates the step name against `ABORT_STEPS` — a typo'd \
         LYRACORE_TRANSFER_ABORT_AFTER would then abort NOTHING, silently, and the crash matrix would \
         report a PASS for a crash that never happened. Body was:\n{body}"
    );
}

#[test]
fn the_driver_never_attests_an_import_that_did_not_commit() {
    // `confirm_import` files the in-row that licenses `finish_transfer` to CASCADE-DELETE the
    // source copy. Attesting before the destination copy is durable is the one unrecoverable
    // ordering bug in the protocol — and it is the GATEWAY's to prevent, because the source
    // database cannot see the destination.
    let (src, dst, src_db, dst_db, calls) = xdb_pair(Some("import_character_blob"));
    let err = super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID)
        .expect_err("a failed import must abort the drive");
    assert!(
        format!("{err:#}").contains("import_character_blob"),
        "{err:#}"
    );

    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter()
            .any(|(_, c)| c == "confirm_import" || c == "finish_transfer"),
        "nothing may attest or finish after a failed import: {log:?}"
    );
    assert!(
        src_db.has(XGUID),
        "the source copy must survive a failed import"
    );
    assert!(!dst_db.has(XGUID), "no destination copy materialised");
}

#[test]
fn a_transfer_is_never_finished_before_the_destination_copy_is_durable() {
    // The module's own guard, driven through the gateway: `finish_transfer` refuses while the
    // in-row is absent, so even a driver that skipped `confirm_import` cannot destroy the source.
    let (_, _, src_db, dst_db, _) = xdb_pair(None);
    let calls: ShardCallLog = Default::default();
    let src = xstore("world", src_db.clone(), calls.clone(), None);
    let _dst = xstore("instances", dst_db, calls, None);
    let plan = src
        .character_destination(XGUID)
        .expect("the durable row names the destination");
    src.begin_transfer(&plan).expect("escrow opens");

    let err = src
        .finish_transfer(plan.transfer_id)
        .expect_err("finish must refuse");
    assert!(format!("{err:#}").contains("not imported"), "{err:#}");
    assert!(src_db.has(XGUID), "the source copy must still be there");
}

#[test]
fn the_arrival_copy_is_fenced_until_the_source_copy_is_destroyed() {
    // Delete-last, observed from the outside: at every prefix of the drive there is at most ONE
    // live copy, and the destination only goes live after the source copy is gone.
    let (_, _, src_db, dst_db, _) = xdb_pair(None);
    let calls: ShardCallLog = Default::default();
    let src = xstore("world", src_db.clone(), calls.clone(), None);
    let dst = xstore("instances", dst_db.clone(), calls, None);
    let plan = src.character_destination(XGUID).unwrap();

    src.begin_transfer(&plan).unwrap();
    assert!(
        !src_db.live(XGUID) && !dst_db.has(XGUID),
        "frozen on the source, nothing arrived yet"
    );
    let escrow = src.escrowed_transfer(XGUID).unwrap();
    dst.import_character_blob(escrow.transfer_id, &escrow.blob)
        .unwrap();
    assert!(
        dst_db.has(XGUID) && !dst_db.live(XGUID),
        "the arrival copy is durable but FENCED while the source copy still exists"
    );
    assert!(
        src_db.has(XGUID) && !src_db.live(XGUID),
        "and the source copy is durable but frozen"
    );
    src.confirm_import(escrow.transfer_id).unwrap();
    src.finish_transfer(escrow.transfer_id).unwrap();
    assert!(
        !src_db.has(XGUID),
        "the source copy is destroyed BEFORE the release"
    );
    assert!(!dst_db.live(XGUID), "still fenced until the release");
    dst.release_transfer(escrow.transfer_id).unwrap();
    assert!(dst_db.live(XGUID));
}

/// The regression this test exists for: the SECOND party member walking
/// into a dungeon whose instance the first member already opened.
///
/// Live, this was the case that broke — the first player transferred perfectly, repeatedly, and the
/// party member behind her hung on the loading screen forever with `run_transfer` never entered.
/// The driver half of that is here: two characters whose durable rows name the SAME instance both
/// have to land on the instances shard, in that one instance, with the destination mirroring it
/// once and spawning its population once. A second `ensure_instance` that re-created the dungeon
/// would be a party playing in two copies of Deadmines.
#[test]
fn a_second_party_member_transfers_into_the_instance_the_first_one_opened() {
    no_hang(30, || {
        const LEADER: u64 = XGUID;
        const MEMBER: u64 = XGUID + 1;
        let calls: ShardCallLog = Default::default();
        let src_db = FakeShardDb::with_character(
            LEADER,
            FakeChar {
                map_id: 36,
                instance_id: 7,
                payload: "leader-gear".into(),
            },
        );
        // The second member resolved to the SAME instance id at the portal — that is what the
        // module's party-first resolution (and the `game_instance_binding` each member carries in
        // their blob) is for. Both rows sit on the world shard; both are owed a transfer.
        lk(&src_db.characters).insert(
            MEMBER,
            FakeChar {
                map_id: 36,
                instance_id: 7,
                payload: "member-gear".into(),
            },
        );
        let dst_db = FakeShardDb::empty();
        let src = xstore("world", src_db.clone(), calls.clone(), None);
        let dst = xstore("instances", dst_db.clone(), calls.clone(), None);

        super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), LEADER)
            .expect("the first member transfers");
        super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), MEMBER)
            .expect("the SECOND member must transfer too — this is the entry that hung live");

        for (guid, payload) in [(LEADER, "leader-gear"), (MEMBER, "member-gear")] {
            assert!(
                dst_db.live(guid),
                "guid {guid} must be live on the instances shard"
            );
            assert!(!src_db.has(guid), "guid {guid}'s source copy must be gone");
            let landed = dst_db.get(guid).unwrap();
            assert_eq!(
                landed.instance_id, 7,
                "guid {guid} landed in a DIFFERENT instance — the party is split"
            );
            assert_eq!(
                landed.payload, payload,
                "guid {guid} arrived without its rows"
            );
        }
        assert_eq!(
            *lk(&dst_db.instances),
            std::collections::HashSet::from([7]),
            "exactly one instance may exist on the destination — a second is a second dungeon"
        );
        assert_eq!(
            *lk(&dst_db.populated),
            vec![7],
            "the destination must SPAWN the instance once; the second member joins the live one"
        );
        assert!(
            src_db.settled() && dst_db.settled(),
            "no escrow may outlive either transfer"
        );
        let log = calls.lock().unwrap().clone();
        assert_eq!(
            log.iter()
                .filter(|(s, c)| s == "world" && c == "begin_transfer")
                .count(),
            2,
            "BOTH members must really be escrowed off the world shard — the live failure was the \
             second one's transfer never running at all: {log:?}"
        );
    });
}

#[test]
fn the_instance_is_mirrored_before_the_character_arrives_in_it() {
    // Ordering, not just presence: `player_login`'s stranding guard DIVERTS a character whose
    // `pending_instance_id` names an instance that does not exist on this shard — so an import
    // that landed before the mirror would put the player outside the dungeon they walked into.
    let (src, dst, _, _, calls) = xdb_pair(None);
    super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID).unwrap();
    let log = calls.lock().unwrap().clone();
    let mirror = log
        .iter()
        .position(|(_, c)| c == "ensure_instance")
        .expect("mirrored");
    let import = log
        .iter()
        .position(|(_, c)| c == "import_character_blob")
        .expect("imported");
    assert!(
        mirror < import,
        "the instance must exist before the character lands in it: {log:?}"
    );
}

#[test]
fn an_open_world_destination_mirrors_and_evicts_nothing() {
    // Zoning OUT: instance 0 is the open world, which is not an instance and must never be
    // "mirrored" (the module refuses id 0) or evicted (that would tear down the open world).
    let calls: ShardCallLog = Default::default();
    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 0,
            instance_id: 0,
            payload: "gear+spells".into(),
        },
    );
    let dst_db = FakeShardDb::empty();
    let src = xstore("instances", src_db, calls.clone(), None);
    let dst = xstore("world", dst_db.clone(), calls.clone(), None);
    super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID).unwrap();

    assert!(dst_db.live(XGUID), "the character must come back out whole");
    let log = calls.lock().unwrap().clone();
    assert!(
        !log.iter()
            .any(|(_, c)| c == "ensure_instance" || c == "evict_instance_population"),
        "instance 0 is the open world — it is neither mirrored nor evicted: {log:?}"
    );
}

#[test]
fn a_character_already_on_its_home_shard_is_not_transferred_but_is_unfenced() {
    // The steady state (every login that does not cross a boundary), PLUS the one crash window
    // that leaves an arrival fence behind with no escrow anywhere to re-drive from: killed between
    // `finish_transfer` and `release_transfer`. Without the speculative release the character would
    // be fenced out of its own login forever.
    let calls: ShardCallLog = Default::default();
    let db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    lk(&db.in_rows).insert(XGUID, XGUID); // the orphaned arrival fence
    let home = xstore("instances", db.clone(), calls.clone(), None);
    assert!(!db.live(XGUID), "precondition: the character is fenced");

    super::transfer::settle_transfer(home.as_ref(), home.as_ref(), XGUID).unwrap();
    assert!(db.live(XGUID), "the stranded arrival fence must be cleared");
    let log = calls.lock().unwrap().clone();
    assert_eq!(
        log.iter().filter(|(_, c)| c == "begin_transfer").count(),
        0,
        "a character already on its home shard must never be re-escrowed: {log:?}"
    );
}

#[test]
fn a_second_transfer_of_the_same_character_is_never_swallowed_as_a_replay() {
    // THE REPEAT-TRANSFER CASE (found by adversarial review). The transfer id IS the character
    // guid, so every hop a character ever makes reuses ONE id — and `plan_begin` reads "an out-row
    // OR an in-row filed under this id names this character" as `BeginPlan::Replay`, i.e. `Ok(())`.
    //
    // Reachable state: the character hopped world -> instances and the driver died between
    // `finish_transfer` and `release_transfer`, so the instances shard holds the character AND an
    // unreleased arrival in-row under id == guid. Now it has to hop OUT again (its location is
    // owned by another shard: a shard-map edit, or a diverted
    // instance re-entry). Without the fence being cleared first, `begin_transfer` on the instances
    // shard replays into `Ok(())` while escrowing NOTHING — and the character is stuck on a shard
    // it can never leave, failing its own login on every attempt, with no operator recourse.
    let calls: ShardCallLog = Default::default();
    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 0,
            instance_id: 0,
            payload: "gear+spells".into(),
        },
    );
    lk(&src_db.in_rows).insert(XGUID, XGUID); // the previous hop's unreleased fence
    let dst_db = FakeShardDb::empty();
    let src = xstore("instances", src_db.clone(), calls.clone(), None);
    let dst = xstore("world", dst_db.clone(), calls.clone(), None);

    super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID)
        .expect("a repeat transfer of the same character must actually run");

    assert!(
        !src_db.has(XGUID),
        "the source copy must be destroyed — not silently left behind"
    );
    assert!(
        dst_db.live(XGUID),
        "the character must arrive LIVE on the far side of hop two"
    );
    assert_eq!(
        dst_db.get(XGUID).unwrap().payload,
        "gear+spells",
        "hop two must carry the rows exactly as hop one did"
    );
    assert!(
        src_db.settled() && dst_db.settled(),
        "no escrow row may outlive the second transfer"
    );
    let log = calls.lock().unwrap().clone();
    assert_eq!(
        log.iter()
            .filter(|(s, c)| s == "instances" && c == "begin_transfer")
            .count(),
        1,
        "begin_transfer must have really escrowed, not replayed into a no-op: {log:?}"
    );
}

#[test]
fn a_failed_transfer_fails_the_login_instead_of_entering_the_world_anyway() {
    // A half-moved character must never be let into the world on whichever shard happened to
    // answer. Both outcomes are recoverable (the escrow holds and the next login re-drives it), but
    // only refusing is honest — and entering anyway is how a character ends up live on the shard
    // that is about to have its copy destroyed.
    let (store, _) = sharded_stores();
    let failing = std::sync::Arc::new(InMemoryStore {
        shard: "world".into(),
        username: "TESTER".into(),
        session: Some(WorldSession {
            account_id: 7,
            session_key: K,
        }),
        characters: store.characters.clone(),
        login_entity: Some(warrior_entity()),
        settle_error: Some("instances shard unreachable".into()),
        ..Default::default()
    });
    let (mut client, server_end) = UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || run_world_session(server_end, failing.as_ref()));
    let (mut c_enc, mut c_dec) = client_handshake(&mut client, "TESTER", K);
    CMSG_PLAYER_LOGIN { guid: Guid::new(1) }
        .write_encrypted_client(&mut client, &mut c_enc)
        .unwrap();
    // No login sequence arrives; the session ends with the transfer's error.
    let _ = ServerOpcodeMessage::read_encrypted(&mut client, &mut c_dec);
    drop(client);
    let outcome = server.join().unwrap();
    let err = outcome.expect_err("a failed transfer must fail the session, not enter the world");
    assert!(
        format!("{err:#}").contains("instances shard unreachable"),
        "{err:#}"
    );
}

#[test]
fn entering_the_world_binds_this_accounts_identity_on_the_shard_it_landed_on() {
    // A character that arrived via `import_character_blob` has only a SHADOW account row on the
    // destination, with no identity bound — and `world::player_login` resolves its caller through
    // `account_by_identity`. Without this bind the arriving player cannot log in at all, on a
    // database the logon tier never touched.
    let (store, calls) = sharded_stores();
    let home = store
        .home
        .clone()
        .expect("the fixture routes to a home shard");
    let _ = drive_routed_session(store, calls.clone());
    assert_eq!(
        *home.bound_sessions.lock().unwrap(),
        vec![7],
        "the home shard must have this account's identity bound before player_login runs"
    );
    let log = calls.lock().unwrap().clone();
    let bind = log
        .iter()
        .position(|(s, c)| s == "instances" && c == "bind_shard_session");
    let login = log
        .iter()
        .position(|(s, c)| s == "instances" && c == "player_login");
    assert!(
        bind < login && bind.is_some(),
        "the identity must be bound BEFORE player_login, not after: {log:?}"
    );
}

/// ENFORCEMENT tripwire, the module's `body_of` pattern: the production routing read lives on
/// `Coordinator` and needs a live SDK cache, so no mock can drive it — and a mutation of it
/// survived the first cut of this file's mutation pass. Source-scan it instead.
#[test]
fn the_routing_read_uses_the_pending_instance_id_not_a_hardcoded_zero() {
    let src = include_str!("../stdb/reads/account.rs");
    let start = src
        .find("pub fn character_location(")
        .expect("`character_location` moved — re-derive this tripwire");
    let body = &src[start..start + src[start..].find("\n    }").expect("fn has a body")];
    let code: String = body
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.contains("c.pending_instance_id"),
        "character_location no longer reads `pending_instance_id` for a character with no live \
         entity. That column is where `teleport_player` parks the DESTINATION instance for a \
         cross-map hop, so it is the whole routing key for instance entry: reading 0 there \
         routes a player walking into Deadmines by MAP alone, which is correct only until a shard \
         map names a bucket (`389:0=pool-a`, see `config::ShardMap`). Body was:\n{code}"
    );
}

/// Sibling tripwire (same reason — a live SDK cache no mock reaches): a character parked inside a
/// dungeon lives on the INSTANCE shard, and Phase A has no realm-core index to ask.
#[test]
fn the_character_select_list_still_unions_across_every_shard() {
    let ws = include_str!("../stdb/world_store.rs");
    let at = ws
        .find("fn characters(&self, account_id: u64)")
        .expect("`characters` moved");
    assert!(
        ws[at..at + 900].contains("self.all_shards()"),
        "the character-select list no longer unions across shards — asking only the realm \
         database makes a character that logged out inside an instance vanish from character \
         select entirely, because its durable row is on the instance shard."
    );
}

// The escrow-priority tripwire that used to live here (`locate_character_still_prefers_the_shard_
// holding_the_escrow`, a source scan of `Coordinator::locate_character`) was retired once
// `settle_home_shard`'s holder lookup became `realm_core::locate_home_shard`, generic over the
// `RealmDb` seam, and the escrow-priority property is pinned BEHAVIOURALLY there instead —
// `locate_home_shard_still_prefers_the_shard_holding_the_escrow_in_the_fallback_scan` in
// `realm_core.rs`, which runs the real fallback-scan code against `fake::Handle` rather than
// matching its source text.

/// Sibling tripwire: what `Coordinator::instance_shard_for` actually FORWARDS.
///
/// `ShardMap::instance_owner` is pinned by its own unit tests and the call site in
/// `settle_home_shard` is pinned by `routing_call_site_tests`, but the three-line adapter between
/// them is reachable from neither — it needs a live `ShardSet`. Verified by mutation: each of the
/// three substitutions below left all 391 gateway tests green while deleting or inverting the
/// stickiness rule outright.
#[test]
fn instance_shard_for_still_forwards_the_holder_the_instance_and_the_connected_set() {
    let conn = include_str!("../stdb/connection.rs");
    let at = conn
        .find("pub(crate) fn instance_shard_for(")
        .expect("`instance_shard_for` moved");
    let body: String = conn[at..at + 500]
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    // (a) the HOLDER, not `self`. `self` is the session's handle — on a login that is the default
    //     shard, which is never a member of a dungeon map's pool, so stickiness could never fire.
    // (b) the real `instance_id`. A literal `0` makes `instance_owner`'s open-world guard reject
    //     every call, and the map decides again — i.e. live runs fork on a pool resize.
    // (c) the CONNECTED predicate. `|_| true` would return a holder the gateway never reached,
    //     which `shard_handle` then cannot resolve, pinning the session to whatever asked.
    assert!(
        body.contains("instance_owner(map_id, instance_id, holder,"),
        "instance_shard_for no longer forwards (map_id, instance_id, holder) verbatim to \
         `ShardMap::instance_owner`. The holder is the ONLY durable evidence of which pool \
         member a live dungeon run is on; substituting `self.shard_name()` or a literal instance \
         id silently restores the routing that predates the instance-shard pool and forks every \
         live run when the operator adds a second instances database. Body was:\n{body}"
    );
    assert!(
        body.contains("self.1.conns.contains_key(d)"),
        "instance_shard_for's `connected` predicate no longer reads the live connection set — a \
         stickiness answer naming a database the gateway never reached cannot be routed to, and \
         must degrade to the shard map like every other resolver in `config.rs`. Body was:\n{body}"
    );
}

#[test]
fn a_resumed_transfer_reuses_the_escrowed_destination_not_the_character_row() {
    // Resume authority: once an escrow exists, ITS destination is the one the destination shard may
    // already hold an imported copy for. Re-deriving from the (frozen) character row instead would
    // drive the second half of the transfer at a different place than the first half imported into.
    let calls: ShardCallLog = Default::default();
    let src_db = FakeShardDb::with_character(
        XGUID,
        FakeChar {
            map_id: 36,
            instance_id: 7,
            payload: "gear+spells".into(),
        },
    );
    lk(&src_db.out_rows).insert(
        XGUID,
        FakeEscrow {
            transfer_id: XGUID,
            character_guid: XGUID,
            dest_map_id: 36,
            dest_instance_id: 42, // ← the escrow's destination, deliberately NOT the row's
            blob: fake_blob(XGUID, 36, 42, "gear+spells"),
        },
    );
    let dst_db = FakeShardDb::empty();
    let src = xstore("world", src_db.clone(), calls.clone(), None);
    let dst = xstore("instances", dst_db.clone(), calls.clone(), None);
    super::transfer::settle_transfer(src.as_ref(), dst.as_ref(), XGUID).unwrap();
    assert_eq!(
        dst_db.get(XGUID).unwrap().instance_id,
        42,
        "the resumed transfer must land where the ESCROW says, not where the character row says"
    );
    assert!(
        lk(&dst_db.instances).contains(&42),
        "and it must mirror the escrow's instance"
    );
}
