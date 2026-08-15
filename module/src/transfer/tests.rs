//! The crash matrix, the pure-planner enumerations, and the source-scan tripwires that pin the
//! fences a pure model cannot see.
//!
//! Split out of `transfer.rs` by #380; `harness.rs` holds the half that EXECUTES the protocol
//! against two in-memory databases.

use super::*;
use crate::Aura;
use spacetimedb::Timestamp;

// -------------------------------------------------------------------------------------
// Manifest / blob
// -------------------------------------------------------------------------------------

#[test]
fn manifest_is_the_generated_enumeration_minus_the_machinery() {
    let m = manifest();
    assert!(
        !m.is_empty(),
        "the character-owned enumeration is empty — build.rs codegen regressed"
    );
    for t in crate::CHARACTER_OWNED_TABLES {
        let present = m.iter().any(|e| e.table == *t);
        if MANIFEST_EXCLUDE.contains(t) {
            assert!(
                !present,
                "{t} is transfer machinery and must not be in its own export blob"
            );
        } else {
            assert!(
                present,
                "character-owned table {t} is missing from the transfer manifest — the manifest \
                     must derive from CHARACTER_OWNED_TABLES, never from a hand-kept parallel list"
            );
        }
    }
    assert_eq!(
        m.len(),
        crate::CHARACTER_OWNED_TABLES.len() - MANIFEST_EXCLUDE.len()
    );
}

/// [`MANIFEST_EXCLUDE`] is the ONLY input to three separate subtractions — the manifest, the
/// export loop, and (issue #42) the arriving payload's required set — and nothing pinned its
/// CONTENTS. Verified by mutation: adding a real character-owned table to it left all 501
/// module tests green while that table silently vanished from the manifest, was never exported,
/// and was no longer required of an arriving payload. That is the #42 defect reintroduced one
/// name at a time, and it is data loss (a transfer would drop the rows) rather than a missing
/// popup. The test above cannot see it: both sides of its length equation move together.
///
/// Adding a name here is a DECISION. Make it in this assertion, with the reason.
#[test]
fn manifest_exclude_holds_only_transfer_machinery() {
    assert_eq!(
        MANIFEST_EXCLUDE,
        // `game_transfer_out` is the escrow row itself: it must not ride inside its own export
        // blob. No table a CHARACTER owns belongs on this list.
        ["game_transfer_out"],
        "MANIFEST_EXCLUDE changed. Every name on it is dropped from the transfer manifest, from \
             every export blob, AND from the set an arriving payload must cover (#42) — so a \
             character-owned table added here loses its rows at every shard crossing, silently and \
             with no other test failing. Only transfer MACHINERY may be listed."
    );
}

#[test]
fn generated_table_names_look_like_real_accessors() {
    // build.rs derives these by stripping `sweep_delete_` off the marker fn names; a rename that
    // broke the convention would put a bogus table in every export blob.
    for t in crate::CHARACTER_OWNED_TABLES {
        assert!(
            t.starts_with("game_") || t.starts_with("pkg_"),
            "{t} does not look like a table accessor — check the `sweep_delete_<accessor>` \
                 naming of the character_owned! delete markers"
        );
    }
}

#[test]
fn hot_marks_name_only_real_manifest_tables() {
    let m = manifest();
    for h in HOT_TABLES {
        assert!(
            m.iter().any(|e| e.table == *h && e.hot),
            "HOT_TABLES names {h}, which is not in the transfer manifest — a table rename left \
                 the hot/cold marks stale"
        );
    }
    assert!(
        m.iter().any(|e| !e.hot),
        "every table marked hot — the cold tier is meant to exist"
    );
}

// -------------------------------------------------------------------------------------
// Issue #72 hot-state audit: auras ON the character (buffs/debuffs/Stealth) must ride the
// blob like every other manifest table. Before this, `game_aura` had NO `character_owned!`
// marker at all (its columns name `target_guid`/`caster_guid`, neither of which the tripwire
// in tripwires.rs recognizes), so a warm handoff silently dropped every buff, DoT, HoT and the
// Rogue's Stealth presence on the source database — the destination simply never got them.
// -------------------------------------------------------------------------------------

/// Mutation-check per the playbook (§8): reverting `sweep_delete_game_aura` /
/// `sweep_transfer_game_aura` back out (or repointing the transfer arm at `not_transported`
/// without adding `game_aura` to `NOT_TRANSPORTED`) makes THIS assertion fail — `game_aura`
/// would no longer be in the generated manifest, or would no longer be marked hot. The two
/// generic ratchets above (`every_manifest_table_can_cross_a_database_boundary`,
/// `the_not_transported_allowlist_matches_the_arms_that_decline`) only prove a table THAT IS
/// in the manifest is shaped correctly — neither one required `game_aura` to be there at all,
/// which is exactly how the drop went unnoticed. This is the test that would have caught it.
#[test]
fn aura_rows_are_a_manifest_table_marked_hot() {
    let m = manifest();
    assert!(
            m.iter().any(|e| e.table == "game_aura" && e.hot),
            "game_aura is missing from the transfer manifest (or not marked hot) — a warm handoff \
             would carry gear/spells/quests/reputation/cooldowns but silently drop every buff, DoT, \
             HoT and Stealth presence on the source database. manifest was: {m:?}"
        );
}

/// The exact question issue #72 asked: does a 10-minute buff resume with the right REMAINING
/// time, or does the export/import round trip re-base it? `applied_at`/`expires_at` are
/// `Timestamp` — an ABSOLUTE point in wall-clock time, never a duration — so the answer should
/// be "unchanged bit-for-bit", and remaining-duration-against-a-fixed-`now` should therefore be
/// identical before and after. `encode_rows`/`decode_rows` are the exact codec
/// `sweep_transfer_game_aura`'s `move_rows` call drives (pure, so testable with no
/// `ReducerContext`); a bug that re-derived `expires_at` from "duration remaining" at export
/// time (the relative-time trap the issue explicitly called out) would still pass a bsatn
/// round-trip, but would desync from `now` between the two sides — which is what the second
/// assertion below actually checks, not just that the bytes decode.
#[test]
fn aura_absolute_expiry_survives_the_export_import_round_trip_with_the_same_remaining_time() {
    let applied_at = Timestamp::from_micros_since_unix_epoch(1_000_000_000);
    // A 10-minute buff.
    let expires_at = Timestamp::from_micros_since_unix_epoch(1_000_000_000 + 10 * 60 * 1_000_000);
    let aura = Aura {
        id: 77,
        target_guid: 42,
        caster_guid: 42,
        spell_id: 19750, // Flash of Light-shaped fixture id; value is inert here
        slot: 0,
        level: 60,
        flags: 0,
        applied_at,
        expires_at,
        effect_id: 1,
        eff_kind: 0,
        amount: 100,
        eff_p0: 0,
        eff_p0_kind: 0,
        eff_p1: 0,
        period_ms: 0,
        amount_remaining: 0,
        stacks: 1,
        next_tick_micros: 0,
        channel_target: 0,
        enters_combat: false,
    };

    let bytes = encode_rows(vec![aura]);
    assert!(
        !bytes.is_empty(),
        "encode_rows produced an empty buffer for a non-empty Vec<Aura>"
    );
    let mut outcome = Ok(());
    let decoded = decode_rows::<Aura>(&bytes, &mut outcome);
    assert!(
        outcome.is_ok(),
        "decode_rows reported a failure: {outcome:?}"
    );
    assert_eq!(decoded.len(), 1);

    // Bit-for-bit: the codec must not touch the timestamp at all.
    assert_eq!(
        decoded[0].expires_at, expires_at,
        "expires_at was altered by the round trip"
    );
    assert_eq!(
        decoded[0].applied_at, applied_at,
        "applied_at was altered by the round trip"
    );

    // The actual player-visible property: remaining time against a fixed `now` 4 minutes in —
    // i.e. 6 minutes still owed — must be identical whether read before or after the hop.
    let now = Timestamp::from_micros_since_unix_epoch(1_000_000_000 + 4 * 60 * 1_000_000);
    let remaining_before =
        expires_at.to_micros_since_unix_epoch() - now.to_micros_since_unix_epoch();
    let remaining_after =
        decoded[0].expires_at.to_micros_since_unix_epoch() - now.to_micros_since_unix_epoch();
    assert_eq!(
        remaining_before,
        6 * 60 * 1_000_000,
        "fixture arithmetic sanity check"
    );
    assert_eq!(
            remaining_after, remaining_before,
            "a 10-minute buff with 6 minutes left before the hop must still have exactly 6 minutes \
             left after it — the destination reads the SAME absolute deadline against its own clock"
        );
}

/// The transfer arm must RE-MINT `id` rather than keep it — `Aura.id` is `#[auto_inc]`, a
/// surrogate key local to ONE database, so carrying the source's id verbatim either collides
/// with an unrelated row the destination already minted under that same id, or (same-value
/// coincidence aside) simply means nothing there.
///
/// Since #380 that is one word in the arm's declaration (`remint = id` vs `keep_key`) rather
/// than a hand-written `row.id = 0` the macro cannot see — so this scan asserts the DECLARED
/// choice. `keep_key` is right for exactly one table in the tree (`game_item_instance`, whose
/// guid is derived from the owner and is the id the CLIENT knows the item by); everything else
/// re-mints, and picking the wrong one here is a live PK collision at the destination.
#[test]
fn aura_transfer_arm_remints_the_auto_inc_id_before_insert() {
    let body = shape_of(
        include_str!("../spell/tables.rs"),
        "fn sweep_transfer_game_aura(",
    );
    assert!(
        body.contains("remint = id"),
        "sweep_transfer_game_aura no longer re-mints `id` for the arriving row — an imported \
             aura would either collide with an existing auto_inc id on the destination or silently \
             masquerade as one that already means something there. Declaration was:\n{body}"
    );
}

#[test]
fn taxi_discovery_transfer_arm_remints_the_auto_inc_id_before_insert() {
    let body = shape_of(
        include_str!("../taxi.rs"),
        "fn sweep_transfer_game_character_taxi_node(",
    );
    assert!(
        body.contains("remint = id"),
        "taxi discovery surrogate ids are local to one database and must be re-minted on arrival. \
         Declaration was:\n{body}"
    );
}

#[test]
fn export_blob_round_trips_through_bsatn() {
    let blob = ExportBlob {
        transfer_id: 7,
        character_guid: 42,
        money: 9999,
        manifest: manifest(),
        dest_map_id: 36,
        dest_instance_id: 7,
        dest_x: 10.0,
        dest_y: -20.0,
        dest_z: 30.0,
        dest_o: 1.25,
        character_row: vec![1, 2, 3, 4],
        payload: vec![TableRows {
            table: "game_player_spell".to_string(),
            rows: vec![9, 8, 7],
        }],
    };
    let bytes = spacetimedb::sats::bsatn::to_vec(&blob).expect("blob serializes");
    let back: ExportBlob = spacetimedb::sats::bsatn::from_slice(&bytes).expect("blob deserializes");
    assert_eq!(blob, back);
    assert_eq!(
        back.payload[0].rows,
        vec![9, 8, 7],
        "the ROWS survive the round trip, not just the manifest"
    );
}

// -------------------------------------------------------------------------------------
// The model: a two-sided world driven by the SAME pure fns the reducers execute
// -------------------------------------------------------------------------------------

/// The observable world, modelled with the SOURCE and DESTINATION durable copies as separate
/// facts (the cross-database truth). The same-database reducers implement this as one
/// re-partitioned row, which is a strictly safer refinement — one row can never be lost while a
/// copy exists, nor duplicated.
/// The one character the single-transfer model moves. Named because `plan_begin` now compares
/// the escrowed guid against the caller's (see the id-collision tests below).
const GUID: u64 = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
struct Model {
    src_durable: bool,
    dst_durable: bool,
    out_row: bool,
    in_row: bool,
}

impl Model {
    fn initial() -> Self {
        Model {
            src_durable: true,
            dst_durable: false,
            out_row: false,
            in_row: false,
        }
    }
    /// A live, actable copy exists here iff a durable copy exists AND the in-transit fence is
    /// down — exactly what `login_allowed` gates and what deleting the `game_world_entity` row
    /// in `begin_transfer` enforces for every targeting/aggro/threat/AOI gate in the module.
    fn live_src(&self) -> bool {
        self.src_durable && login_allowed(self.out_row, self.in_row)
    }
    fn live_dst(&self) -> bool {
        self.dst_durable && login_allowed(self.out_row, self.in_row)
    }
    fn settled(&self) -> bool {
        !self.out_row && !self.in_row
    }
}

/// Every step is ONE transaction: it either applies wholly or not at all. A CRASH DURING a step
/// is therefore the same observable as never having run it — which is why enumerating all step
/// sequences (including every truncation) is exactly the crash matrix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Begin,
    Import,
    Finish,
    /// The reaper firing before the escrow is stale (must be inert).
    ReapFresh,
    /// The reaper firing on a stale escrow.
    ReapStale,
    /// The reaper firing on a stale escrow while the destination cannot be consulted
    /// (cross-database partition — must never guess).
    ReapUnreachable,
}

const ALL_STEPS: &[Step] = &[
    Step::Begin,
    Step::Import,
    Step::Finish,
    Step::ReapFresh,
    Step::ReapStale,
    Step::ReapUnreachable,
];

fn complete(m: Model) -> Model {
    // Delete-last: the source copy goes, then both escrow rows. The destination copy — already
    // durable, which `plan_finish`/`recovery` guarantee before we get here — is released.
    Model {
        src_durable: false,
        dst_durable: m.dst_durable,
        out_row: false,
        in_row: false,
    }
}

fn step(m: Model, s: Step) -> Model {
    match s {
        Step::Begin => match plan_begin(
            // One character, one transfer id: the id is "in use" exactly while a row names it.
            if m.out_row || m.in_row {
                Some(GUID)
            } else {
                None
            },
            GUID,
            m.src_durable,
            !login_allowed(m.out_row, m.in_row),
        ) {
            BeginPlan::Escrow => Model { out_row: true, ..m },
            BeginPlan::Replay
            | BeginPlan::NoSource
            | BeginPlan::IdCollision
            | BeginPlan::AlreadyInTransit => m,
        },
        Step::Import => match plan_import(m.out_row, m.in_row) {
            ImportPlan::Apply => Model {
                in_row: true,
                dst_durable: true,
                ..m
            },
            ImportPlan::Replay | ImportPlan::NoEscrow => m,
        },
        Step::Finish => match plan_finish(m.out_row, m.in_row) {
            FinishPlan::Complete => complete(m),
            FinishPlan::AlreadyDone | FinishPlan::NotImported => m,
        },
        Step::ReapFresh => reap(m, Some(m.in_row), 0),
        Step::ReapStale => reap(m, Some(m.in_row), TRANSFER_STALE_MICROS),
        Step::ReapUnreachable => reap(m, None, TRANSFER_STALE_MICROS),
    }
}

fn reap(m: Model, dest_imported: Option<bool>, age: i64) -> Model {
    match recovery(m.out_row, dest_imported, age) {
        Recovery::Hold => m,
        Recovery::Rollback => Model {
            out_row: false,
            ..m
        },
        Recovery::RollForward => complete(m),
    }
}

/// The two invariants this whole ticket exists for, plus the two that keep them true.
// Each assertion is spelled `!(<the bad state>)` so the predicate reads as the negation of the
// failure its message names. De Morgan'd forms (`!a || b`) are equivalent but no longer line up
// with the messages, which is the only way these invariants stay checkable by eye.
#[allow(clippy::nonminimal_bool)]
fn check(m: Model, trace: &[Step]) {
    assert!(
        m.src_durable || m.dst_durable,
        "ZERO DURABLE COPIES after {trace:?} — the character was lost (state {m:?})"
    );
    assert!(
        !(m.live_src() && m.live_dst()),
        "DUAL LIVENESS after {trace:?} — the character is actable on both sides (state {m:?})"
    );
    assert!(
        !(m.in_row && !m.out_row),
        "orphan in-row after {trace:?}: the destination escrow outlived the source's claim, so \
             a later rollback could not see that the import happened (state {m:?})"
    );
    assert!(
        !(m.dst_durable && m.src_durable && m.settled()),
        "TWO SETTLED COPIES after {trace:?} — the escrow cleared with both sides durable, which \
             is a dupe the moment either logs in (state {m:?})"
    );
}

/// Drive the reaper to a fixpoint (what an abandoned transfer eventually gets).
fn reap_to_fixpoint(mut m: Model) -> Model {
    for _ in 0..8 {
        let next = step(m, Step::ReapStale);
        if next == m {
            return m;
        }
        m = next;
    }
    panic!("reaper did not reach a fixpoint from {m:?}");
}

// -------------------------------------------------------------------------------------
// Property: every interleaving, every crash point
// -------------------------------------------------------------------------------------

#[test]
fn exhaustive_interleavings_never_dupe_and_never_lose_the_character() {
    // Every sequence of up to DEPTH steps over the full step alphabet. Because a step is one
    // transaction, TRUNCATING a sequence models a crash at that step boundary, and every
    // truncation of every enumerated sequence is itself enumerated — so this is the crash matrix
    // and the interleaving matrix at once.
    const DEPTH: usize = 6;
    let mut trace = Vec::with_capacity(DEPTH);
    let mut states = std::collections::HashSet::new();
    fn walk(
        m: Model,
        depth: usize,
        trace: &mut Vec<Step>,
        states: &mut std::collections::HashSet<Model>,
    ) {
        check(m, trace);
        states.insert(m);
        if depth == 0 {
            return;
        }
        for &s in ALL_STEPS {
            trace.push(s);
            walk(step(m, s), depth - 1, trace, states);
            trace.pop();
        }
    }
    walk(Model::initial(), DEPTH, &mut trace, &mut states);
    // Sanity that the walk actually explored the protocol rather than sitting still.
    assert!(
        states.len() >= 4,
        "the walk only reached {} distinct states — the model is not moving",
        states.len()
    );
}

#[test]
fn every_reachable_state_settles_whole_on_exactly_one_side() {
    const DEPTH: usize = 6;
    fn walk(m: Model, depth: usize, trace: &mut Vec<Step>) {
        let settled = reap_to_fixpoint(m);
        assert!(
            settled.settled(),
            "reaper left escrow rows behind from {trace:?}: {settled:?}"
        );
        assert_ne!(
                settled.src_durable, settled.dst_durable,
                "after reaping to a fixpoint from {trace:?} the character is on {} sides, not one: {settled:?}",
                if settled.src_durable { "two" } else { "zero" }
            );
        assert!(
            settled.live_src() ^ settled.live_dst(),
            "settled state from {trace:?} has no single live copy: {settled:?}"
        );
        if depth == 0 {
            return;
        }
        for &s in ALL_STEPS {
            trace.push(s);
            walk(step(m, s), depth - 1, trace);
            trace.pop();
        }
    }
    walk(Model::initial(), DEPTH, &mut Vec::new());
}

// -------------------------------------------------------------------------------------
// The named crash points (the same facts the property test covers, spelled out as evidence)
// -------------------------------------------------------------------------------------

#[test]
fn crash_point_a_during_begin_leaves_the_character_resident() {
    // An aborted transaction commits nothing: identical to never having called begin.
    let m = Model::initial();
    assert!(m.live_src() && !m.live_dst() && m.settled());
    assert_eq!(
        reap_to_fixpoint(m),
        m,
        "nothing escrowed — the reaper must be inert"
    );
}

#[test]
fn crash_point_b_after_begin_rolls_back() {
    let m = step(Model::initial(), Step::Begin);
    assert_eq!(phase(m.out_row, m.in_row), Phase::Escrowed);
    assert!(
        !m.live_src() && !m.live_dst(),
        "frozen: no live copy anywhere while escrowed"
    );
    assert!(
        m.src_durable,
        "the source copy is still durable — nothing has been deleted yet"
    );
    // Fresh reaper: inert. Stale reaper: rollback.
    assert_eq!(step(m, Step::ReapFresh), m);
    let recovered = reap_to_fixpoint(m);
    assert!(
        recovered.live_src() && !recovered.dst_durable,
        "{recovered:?}"
    );
}

#[test]
fn crash_point_c_during_import_is_indistinguishable_from_crash_point_b() {
    let m = step(Model::initial(), Step::Begin);
    let aborted = m; // the import transaction rolled back
    assert_eq!(aborted, step(m, Step::ReapFresh));
    assert!(reap_to_fixpoint(aborted).live_src());
}

#[test]
fn crash_point_d_after_import_rolls_forward_never_back() {
    let m = step(step(Model::initial(), Step::Begin), Step::Import);
    assert_eq!(phase(m.out_row, m.in_row), Phase::Imported);
    assert!(
        m.src_durable && m.dst_durable,
        "two durable copies, zero live — the safe overlap"
    );
    assert!(!m.live_src() && !m.live_dst());
    assert_eq!(
        recovery(m.out_row, Some(m.in_row), TRANSFER_STALE_MICROS),
        Recovery::RollForward,
        "past the point of no return the reaper must NEVER roll back"
    );
    let recovered = reap_to_fixpoint(m);
    assert!(
        recovered.live_dst() && !recovered.src_durable,
        "{recovered:?}"
    );
}

#[test]
fn crash_point_e_during_finish_is_retryable() {
    let imported = step(step(Model::initial(), Step::Begin), Step::Import);
    // Aborted finish == no state change; a retry completes.
    let retried = step(imported, Step::Finish);
    assert!(retried.live_dst() && !retried.src_durable && retried.settled());
    // ...and so does the reaper if the driver never retries.
    assert_eq!(reap_to_fixpoint(imported), retried);
}

#[test]
fn crash_point_f_after_finish_is_terminal_and_replay_safe() {
    let done = step(
        step(step(Model::initial(), Step::Begin), Step::Import),
        Step::Finish,
    );
    assert!(done.live_dst() && done.settled());
    for &s in ALL_STEPS {
        assert_eq!(
            step(done, s),
            done,
            "{s:?} after a completed transfer must be a no-op"
        );
    }
}

#[test]
fn finish_before_import_is_refused_so_no_state_ever_has_zero_durable_copies() {
    let escrowed = step(Model::initial(), Step::Begin);
    assert_eq!(
        plan_finish(escrowed.out_row, escrowed.in_row),
        FinishPlan::NotImported
    );
    assert_eq!(
        step(escrowed, Step::Finish),
        escrowed,
        "finish must not touch the source copy"
    );
}

#[test]
fn import_replay_is_a_no_op() {
    let imported = step(step(Model::initial(), Step::Begin), Step::Import);
    assert_eq!(
        plan_import(imported.out_row, imported.in_row),
        ImportPlan::Replay
    );
    assert_eq!(step(imported, Step::Import), imported);
    // And an import with no escrow (e.g. replayed after finish) is refused outright.
    assert_eq!(plan_import(false, false), ImportPlan::NoEscrow);
}

#[test]
fn begin_replay_is_a_no_op_in_both_escrowed_and_imported_phases() {
    let escrowed = step(Model::initial(), Step::Begin);
    assert_eq!(plan_begin(Some(GUID), GUID, true, true), BeginPlan::Replay);
    assert_eq!(step(escrowed, Step::Begin), escrowed);
    let imported = step(escrowed, Step::Import);
    assert_eq!(step(imported, Step::Begin), imported);
    // A begin against a character with no durable source copy is an error, not an escrow.
    assert_eq!(plan_begin(None, GUID, false, false), BeginPlan::NoSource);
}

#[test]
fn a_transfer_id_reused_for_a_different_character_is_refused_not_replayed() {
    // `transfer_id` is CALLER-chosen, so a driver can reuse one that is still escrowed for
    // someone else. Answering `Replay` (= `Ok(())`) there tells that driver its character is
    // escrowed when it is not; the driver then drives import/finish on the id and moves the
    // OTHER character to ITS destination, reporting success for a transfer that never happened.
    assert_eq!(
        plan_begin(Some(GUID), GUID + 1, true, false),
        BeginPlan::IdCollision
    );
    // ...in the imported phase too — the in-row is consulted as the fallback claim.
    assert_eq!(
        plan_begin(Some(GUID), GUID + 1, true, true),
        BeginPlan::IdCollision
    );
    // The matching guid is still an honest replay.
    assert_eq!(plan_begin(Some(GUID), GUID, true, true), BeginPlan::Replay);
}

#[test]
fn a_character_already_escrowed_under_another_id_cannot_be_escrowed_twice() {
    // Two escrows on one character = two destinations each holding a claim on it. Cross-database
    // both would import and the character is DUPLICATED — and no per-transfer-id check can see
    // it, because the second id's ledger rows are empty. Only the by-character lookup can.
    assert_eq!(
        plan_begin(None, GUID, true, true),
        BeginPlan::AlreadyInTransit
    );
    // A fresh id for a character that is NOT in transit is the normal escrow.
    assert_eq!(plan_begin(None, GUID, true, false), BeginPlan::Escrow);
}

#[test]
fn reaper_never_guesses_when_the_destination_cannot_be_consulted() {
    // The cross-database partition case: rolling back against a successful import DUPES,
    // rolling forward against a failed one DELETES. Holding is the only safe answer.
    assert_eq!(
        recovery(true, None, TRANSFER_STALE_MICROS * 1000),
        Recovery::Hold
    );
    let escrowed = step(Model::initial(), Step::Begin);
    assert_eq!(step(escrowed, Step::ReapUnreachable), escrowed);
    let imported = step(escrowed, Step::Import);
    assert_eq!(step(imported, Step::ReapUnreachable), imported);
}

#[test]
fn reaper_is_inert_before_the_stale_window_and_with_no_escrow() {
    assert_eq!(
        recovery(true, Some(false), TRANSFER_STALE_MICROS - 1),
        Recovery::Hold
    );
    assert_eq!(
        recovery(true, Some(true), TRANSFER_STALE_MICROS - 1),
        Recovery::Hold
    );
    assert_eq!(
        recovery(false, Some(false), TRANSFER_STALE_MICROS * 10),
        Recovery::Hold
    );
    // Exactly at the window it acts (inclusive boundary, the instance-reaper precedent).
    assert_eq!(
        recovery(true, Some(false), TRANSFER_STALE_MICROS),
        Recovery::Rollback
    );
}

/// #223 — the ENTIRE recovery decision, enumerated rather than sampled.
///
/// The tests above sample `recovery` at the points that mattered when each was written. This
/// walks every combination of its three inputs — `has_out` × `dest_imported` (all three states,
/// `None` included) × the age boundary and both sides of it — and states the expected verdict
/// for each in one table.
///
/// Worth having as well as the samples because the failure it catches is a MISSING arm, not a
/// wrong one: an arm that stops being reached (a reordered `if`, a `>=` become `>`, a `None`
/// folded into `Some(false)`) leaves every sampled point still passing while some other point
/// silently changes verdict. Each of those verdicts is either a duplicated character or a
/// deleted one — this is the function whose doc calls it "the single most load-bearing" in the
/// file, and the reason `dest_imported` is an `Option` at all.
#[test]
fn the_recovery_verdict_is_enumerated_over_its_whole_input_space() {
    // Ages spanning the staleness boundary, including the exact edge (inclusive) and the
    // degenerate values a clock skew or an unset timestamp can produce.
    let fresh = [i64::MIN, -1, 0, 1, TRANSFER_STALE_MICROS - 1];
    let stale = [
        TRANSFER_STALE_MICROS,
        TRANSFER_STALE_MICROS + 1,
        TRANSFER_STALE_MICROS * 1000,
        i64::MAX,
    ];

    for age in fresh.iter().chain(stale.iter()).copied() {
        for dest in [None, Some(false), Some(true)] {
            assert_eq!(
                recovery(false, dest, age),
                Recovery::Hold,
                "no out-row means nothing is escrowed here, so there is nothing to recover — \
                     age {age}, destination {dest:?}"
            );
        }
    }

    for age in fresh {
        for dest in [None, Some(false), Some(true)] {
            assert_eq!(
                recovery(true, dest, age),
                Recovery::Hold,
                "an escrow younger than the {TRANSFER_STALE_MICROS}µs window belongs to a \
                     driver that may still be working; reaping it races the live transfer — age \
                     {age}, destination {dest:?}"
            );
        }
    }

    for age in stale {
        assert_eq!(
            recovery(true, None, age),
            Recovery::Hold,
            "an unconsultable destination must HOLD FOREVER, at any age ({age}). Guessing \
                 rollback against a successful import duplicates the character; guessing forward \
                 against a failed one destroys it. A frozen character is recoverable — neither of \
                 those is."
        );
        assert_eq!(
            recovery(true, Some(false), age),
            Recovery::Rollback,
            "the destination provably has no copy, so the only durable copy is the source's — \
                 unfreeze it (age {age})"
        );
        assert_eq!(
            recovery(true, Some(true), age),
            Recovery::RollForward,
            "the destination copy is DURABLE, so the transfer may only ever complete — rolling \
                 back here would leave two copies (age {age})"
        );
    }
}

/// The remaining pure planners, likewise enumerated: their inputs are one or two booleans plus a
/// guid comparison, so the whole space is small enough to state outright.
///
/// `login_allowed` is the in-transit fence — the predicate that stops a character being
/// materialised on a shard while a copy of it is mid-flight elsewhere. Every non-Resident phase
/// must refuse, INCLUDING the `(false, true)` shape `phase` documents as unreachable: an
/// arrival in-row that outlived its out-row still means another database is holding a claim, so
/// the fence must not relax just because the state "cannot happen".
#[test]
fn the_pure_planners_are_enumerated_over_their_whole_input_space() {
    // phase
    assert_eq!(phase(false, false), Phase::Resident);
    assert_eq!(phase(false, true), Phase::Resident);
    assert_eq!(phase(true, false), Phase::Escrowed);
    assert_eq!(phase(true, true), Phase::Imported);

    // plan_import
    assert_eq!(plan_import(false, false), ImportPlan::NoEscrow);
    assert_eq!(plan_import(true, false), ImportPlan::Apply);
    assert_eq!(plan_import(false, true), ImportPlan::Replay);
    assert_eq!(plan_import(true, true), ImportPlan::Replay);

    // plan_finish
    assert_eq!(plan_finish(false, false), FinishPlan::AlreadyDone);
    assert_eq!(plan_finish(false, true), FinishPlan::AlreadyDone);
    assert_eq!(
        plan_finish(true, false),
        FinishPlan::NotImported,
        "finishing an un-imported escrow destroys the ONLY durable copy — this refusal is the \
             delete-last invariant itself"
    );
    assert_eq!(plan_finish(true, true), FinishPlan::Complete);

    // login_allowed — only a fully resident character may be materialised.
    assert!(login_allowed(false, false));
    assert!(
        !login_allowed(true, false),
        "an escrowed character is frozen on this shard; logging it in un-freezes a copy \
             another database is about to import"
    );
    assert!(
        !login_allowed(false, true),
        "an unreleased ARRIVAL row still means a transfer is in flight — the fence must not \
             relax on the shape `phase` treats as unreachable"
    );
    assert!(!login_allowed(true, true));

    // escrowed_guid — out-row first, in-row as the fallback that un-strands #36's blocker 2.
    assert_eq!(escrowed_guid(None, None), None);
    assert_eq!(escrowed_guid(Some(7), None), Some(7));
    assert_eq!(
            escrowed_guid(None, Some(9)),
            Some(9),
            "a database holding ONLY an unreleased arrival in-row must still read as escrowed for \
             that character. Reading it as an unused id is the permanent login wedge the #36 review \
             found (blocker 2): `is_in_transit` sees the in-row and refuses the hop out, and nothing \
             ever reaps it, because `reap_transfers` iterates game_transfer_out only — so the \
             character is refused a transfer off the shard it is stuck on, permanently, with no \
             operator recourse"
        );
    assert_eq!(
        escrowed_guid(Some(7), Some(9)),
        Some(7),
        "the source out-row wins: it names the copy that is actually frozen here"
    );

    // plan_begin — the id-reuse and double-escrow refusals, over every input shape.
    for source_durable in [false, true] {
        for in_transit in [false, true] {
            assert_eq!(
                plan_begin(Some(7), 7, source_durable, in_transit),
                BeginPlan::Replay,
                "the same id for the same character is a retry, whatever else is true"
            );
            assert_eq!(
                plan_begin(Some(7), 9, source_durable, in_transit),
                BeginPlan::IdCollision,
                "an id reused for a DIFFERENT character must never answer Replay — the driver \
                     would then drive the remaining steps and move the other character while \
                     reporting success for one that never moved"
            );
        }
    }
    assert_eq!(
        plan_begin(None, 7, false, false),
        BeginPlan::NoSource,
        "there is nothing here to freeze"
    );
    assert_eq!(
        plan_begin(None, 7, false, true),
        BeginPlan::NoSource,
        "no durable source is checked BEFORE in-transit: with no copy here, in-transit is not \
             the actionable complaint"
    );
    assert_eq!(
        plan_begin(None, 7, true, true),
        BeginPlan::AlreadyInTransit,
        "a character escrowed twice under two ids has two destinations each holding a claim; \
             cross-database, both import and the character is DUPLICATED"
    );
    assert_eq!(plan_begin(None, 7, true, false), BeginPlan::Escrow);
}

#[test]
fn a_character_round_trips_between_two_partitions() {
    // The tracer: A -> B, then B -> A. Each leg is the full three-step protocol.
    let mut m = Model::initial();
    for leg in 0..2 {
        m = step(m, Step::Begin);
        assert!(
            !m.live_src() && !m.live_dst(),
            "leg {leg}: frozen mid-flight"
        );
        m = step(m, Step::Import);
        m = step(m, Step::Finish);
        assert!(m.settled(), "leg {leg}: escrow cleared");
        assert_ne!(
            m.src_durable, m.dst_durable,
            "leg {leg}: exactly one durable copy"
        );
        // Re-home for the return leg: the destination becomes the next leg's source.
        m = Model {
            src_durable: true,
            dst_durable: false,
            out_row: false,
            in_row: false,
        };
    }
}

// -------------------------------------------------------------------------------------
// ENFORCEMENT tripwire: the fence is three lines of code in three files, and the pure model
// cannot see any of them
// -------------------------------------------------------------------------------------

/// Everything above tests the DECISION surface. AC#4 ("no reducer can act on an in-transit
/// character") is not a decision, it is three call sites — and deleting any of them left the
/// whole 428-test suite green, because a pure model has no reducers in it. So source-scan them,
/// the `character_owned_tripwire` pattern in `tripwires.rs`: a tripwire is the only thing that can
/// catch a chokepoint that stopped being called.
///
/// Isolate the enclosing fn body before matching, so the assertion is "THIS fn calls the gate",
/// not "the file mentions it somewhere in a doc comment".
///
/// `body_of`/`code_of` (comment-stripped) are the shared scan primitives in
/// [`crate::test_scan`] (issue #64 — this used to be six near-identical copies across this file
/// and five others, and they had already drifted: the first cut of
/// `every_refuse_verdict_call_site_still_routes_through_the_by_guid_chokepoint` matched
/// comments that EXPLAIN each fence rather than the fence itself, and a trailing (as opposed to
/// whole-line) comment defeated the weak four-file version entirely).
use crate::test_scan::{body_of, code_of, shape_of};

/// Issue #64: this used to be the ONLY guard on `entity_by_owner`'s in-transit fence, and it is
/// a source scan — it can prove the right identifiers appear, but not that they mean the right
/// thing. The SENSE is now pinned directly (no `ReducerContext` needed) by
/// `helpers::tests::gate_in_transit_refuses_an_in_transit_candidate_and_returns_a_normal_one`;
/// swap `gate_in_transit`'s two branches and that test fails by name, something this scan could
/// never do. This test still earns its keep: it is what catches the fence being deleted, or its
/// call to the gate being routed around, outright.
///
/// #380 collapsed all three chokepoints onto `helpers::gate_by_guid`, so the exact-shape STRING
/// PIN that used to sit next to this test (a whitespace-collapsed verbatim copy of
/// `entity_by_owner`'s body, which every legitimate edit had to update twice) is gone: there is
/// one two-line expression left where a `!` could hide, and it is inside `.cargo/mutants.toml`'s
/// cargo-mutants scope — where `replace gate_by_guid -> None` is CAUGHT.
#[test]
fn the_actor_chokepoint_still_calls_the_in_transit_gate() {
    let body = code_of(include_str!("../helpers.rs"), "pub fn entity_by_owner(");
    assert!(
        body.contains("gate_by_guid("),
        "helpers::entity_by_owner no longer routes its result through gate_by_guid. That call \
             is the ONE gate covering all 60+ player-fired reducers (issue #16, AC#4); without it \
             every one of them can act on a character that is mid-transfer. Body was:\n{body}"
    );
}

/// ...and the gate they all share still consults the ledger and still routes the answer through
/// the pure decision. Splitting these two apart — computing `in_transit` and never passing it
/// on, or passing a constant — is the one edit the three call-site scans above cannot see, now
/// that they only prove `gate_by_guid` is CALLED.
#[test]
fn the_shared_gate_consults_the_ledger_and_defers_to_the_pure_decision() {
    let body = code_of(include_str!("../helpers.rs"), "fn gate_by_guid<T>(");
    assert!(
        body.contains("crate::transfer::is_in_transit(ctx, guid_of(row))"),
        "helpers::gate_by_guid no longer asks the transfer ledger about the candidate's guid — \
             every in-transit fence in the tree is then open, and each of the three chokepoints \
             still LOOKS fenced. Body was:\n{body}"
    );
    assert!(
            body.contains("gate_in_transit(candidate, in_transit)"),
            "helpers::gate_by_guid no longer hands its answer to `gate_in_transit`, the pure \
             decision that `gate_in_transit_refuses_an_in_transit_candidate_and_returns_a_normal_one` \
             pins. Body was:\n{body}"
        );
}

#[test]
fn player_login_still_refuses_an_in_transit_character() {
    // #468 stage 4d: the fence lives in the shared login core — both the sender reducer and
    // gw_player_login delegate there, so pinning the core covers both entries.
    let body = body_of(include_str!("../world.rs"), "pub(crate) fn apply_player_login(");
    assert!(
        body.contains("is_in_transit"),
        "world::player_login no longer fences in-transit characters. Login is the one path that \
             can re-materialise a live entity on a shard the character has LEFT — that is the \
             dual-liveness dupe the escrow exists to prevent. Body was:\n{body}"
    );
}

#[test]
fn import_character_still_refuses_when_no_destination_copy_materialises() {
    // The model says `ImportPlan::Apply` ⇒ the destination copy is durable. The reducer must
    // MAKE that true: if it files the in-row while the apply silently did nothing, the in-row
    // licenses `finish_transfer` (and the reaper's roll-forward) to clear the escrow, settling
    // the transfer with ZERO durable copies. Model-vs-reality is exactly the gap a pure test
    // cannot see.
    let body = body_of(include_str!("mod.rs"), "pub fn import_character(");
    assert!(
        body.contains("no durable row at the destination"),
        "import_character no longer refuses when the character row is absent — it can now file \
             an in-row for a destination copy that does not exist. Body was:\n{body}"
    );
}

// -------------------------------------------------------------------------------------
// ENFORCEMENT tripwires for the BY-GUID chokepoint (issue #30), one NAMED test per fenced
// path — the #26 review's lesson: deleting a fence must turn a test red, and a pure model
// sees no reducers, so each of these is a source scan of the call site's own body.
// -------------------------------------------------------------------------------------

/// The gate itself. Same shared decision as the actor chokepoint — all three route through
/// `helpers::gate_by_guid` and thence `gate_in_transit`, so
/// `gate_in_transit_refuses_an_in_transit_candidate_and_returns_a_normal_one` pins the SENSE of
/// all three at once (issue #64), and
/// `the_shared_gate_consults_the_ledger_and_defers_to_the_pure_decision` pins the one wrapper.
/// This scan is what catches the fence being deleted, or routed around, outright.
#[test]
fn the_by_guid_chokepoint_still_calls_the_in_transit_gate() {
    for signature in ["pub fn character_by_guid(", "pub fn character_by_name("] {
        let body = code_of(include_str!("../helpers.rs"), signature);
        assert!(
            body.contains("gate_by_guid("),
            "helpers::{signature} no longer routes its result through gate_by_guid. That call \
                 is the gate for every reducer that reaches a character by guid or by name \
                 (issue #30); without it the REFUSE verdicts in this module's table are all open. \
                 Body was:\n{body}"
        );
    }
}

/// Every REFUSE-verdict call site, one assertion each: `(file, fn signature, what breaks)`.
/// Deleting the gate call from any one of them fails THIS test by name.
///
/// A sign inversion (issue #64) does NOT need re-checking at each of these ~12 sites: none of
/// them touches `is_in_transit` — they call `character_by_guid`/`character_by_name`, and the
/// SENSE of those two now lives in exactly one place (`helpers::gate_in_transit`), pinned once
/// by `the_by_guid_chokepoint_is_exactly_the_pinned_shape`. A sign flip anywhere in the fence
/// itself fails that ONE test regardless of which of these call sites reads it; this test only
/// needs to prove each site still calls the (now sense-safe) wrapper at all.
#[test]
fn every_refuse_verdict_call_site_still_routes_through_the_by_guid_chokepoint() {
    // #386 split `debug.rs` into the `debug/` directory; every `"debug.rs"` site below now
    // scans this concatenation-of-the-whole-directory blob instead of one `include_str!`.
    let debug_src = crate::test_scan::debug_dir_src();
    let sites: &[(&str, &str, &str, &str)] = &[
        (
            "auth.rs",
            include_str!("../auth.rs"),
            "pub fn delete_character(",
            "delete_character destroys a durable copy another shard holds a claim on — \
                 cross-database that is the annihilation case (the destination's arrival copy \
                 survives and its reaper never settles)",
        ),
        (
            "chat.rs",
            include_str!("../chat.rs"),
            // #479 moved the body into the actor-explicit core; the fence travelled with it.
            "pub(crate) fn apply_send_whisper(",
            "apply_send_whisper reaches an in-transit character by NAME because begin_transfer \
                 persists with `set_offline: false`",
        ),
        (
            "gm.rs",
            include_str!("../gm.rs"),
            "pub fn set_gm_level(",
            "set_gm_level writes gm_level onto the source copy by NAME",
        ),
        (
            "debug/mod.rs",
            debug_src.as_str(),
            "pub fn debug_spawn_player_entity(",
            "debug_spawn_player_entity is player_login's RE-MATERIALISATION path wearing a \
                 harness hat — unfenced it is a second route to the dual-liveness dupe",
        ),
        (
            "debug/mod.rs",
            debug_src.as_str(),
            "pub fn debug_set_money(",
            "debug_set_money writes Character.money on the source copy",
        ),
        (
            "debug/mod.rs",
            debug_src.as_str(),
            "pub fn debug_expire_quest(",
            "debug_expire_quest writes game_character_quest, a MANIFEST table",
        ),
        (
            "debug/mod.rs",
            debug_src.as_str(),
            "pub fn debug_grant_reputation(",
            "debug_grant_reputation writes game_player_reputation, a MANIFEST table",
        ),
        (
            "debug/instance.rs",
            debug_src.as_str(),
            "pub fn debug_grant_default_actions(",
            "debug_grant_default_actions writes game_player_action, a HOT manifest table",
        ),
        (
            "skill.rs",
            include_str!("../skill.rs"),
            "pub fn debug_reseed_skills(",
            "debug_reseed_skills writes game_player_skill, a HOT manifest table",
        ),
        // --- Added by the #30 review's independent call-site audit. ---
        (
            "world.rs",
            include_str!("../world.rs"),
            "pub fn debug_delete_character(",
            "debug_delete_character is auth::delete_character's gate-free harness twin, and \
                 STRICTLY worse: cascade_delete_character runs the character_owned! sweep, which \
                 includes sweep_delete_game_transfer_out, so an unfenced call destroys the \
                 character AND both escrow rows in one transaction — cross-database the \
                 destination's arrival copy is left with no source out-row and `recovery` answers \
                 Hold forever",
        ),
        (
            "world.rs",
            include_str!("../world.rs"),
            "pub(crate) fn recall_to_home(",
            "recall_to_home is the ONE teleport_player caller that needs no live entity (it \
                 reads the home coords off the durable row), so by-guid — via \
                 debug_use_hearthstone — it is the only route by which teleport_player's \
                 unconditional durable-row write lands on an escrowed character, moving FIVE \
                 ExportBlob fields plus the pending_instance_id that in_transit_instances reads",
        ),
        (
            "debug/mod.rs",
            debug_src.as_str(),
            "pub fn debug_set_level(",
            "debug_set_level drives stats::set_character_level, which writes Character.level \
                 (an ExportBlob field) and Character.xp on the DURABLE row and needs no live \
                 entity to do it",
        ),
    ];
    for (file, src, signature, why) in sites {
        let body = code_of(src, signature);
        // `require_character` (issue #371) is `character_by_guid(..).ok_or_else(..)` folded
        // into one helper — the fence is the SAME `character_by_guid` call, just no longer
        // spelled out at the caller, so it counts as routing through the chokepoint too.
        assert!(
            body.contains("character_by_guid")
                || body.contains("character_by_name")
                || body.contains("require_character"),
            "{file}'s `{signature}` no longer routes through helpers::character_by_guid / \
             character_by_name / require_character — the REFUSE fence is gone (issue #30). \
             {why}. Body was:\n{body}"
        );
    }
}

/// The BACKGROUND writers, found by the #30 review's independent audit. Both are `game_tick_pass!`
/// bodies, so the argument that fences every other caller in their files — "the guid came from a
/// live entity, and `begin_transfer` deleted it" — does not reach them: neither reads
/// `game_world_entity` at all. They carry their own `is_in_transit` gate instead of routing
/// through `character_by_guid`, because both already hold the row (or its owned row) from a scan.
///
/// Both refusals are DEFERRALS in substance, which is why REFUSE is honest here rather than
/// value-dropping: the rest pass leaves `rested_since_micros` running so the next pass banks the
/// whole span, and the quest pass leaves `deadline_micros` set so the next pass fails the quest.
#[test]
fn the_background_tick_passes_skip_in_transit_characters() {
    let sites: &[(&str, &str, &str, &str)] = &[
            (
                "rest.rs",
                include_str!("../rest.rs"),
                "fn rested_accrue_pass(",
                "the 30s rested-accrual pass filters the DURABLE row's `resting` flag and never \
                 looks at a live entity. `begin_transfer` persists with `set_offline: false`, and \
                 `persist_entity`'s `set_offline` branch is the one that calls \
                 `materialize_on_logout` to stop the rest clock — so a character escrowed in an inn \
                 keeps `resting == true` with a running clock and this pass rewrites `rested_xp` on \
                 the frozen row every 30s, for as long as the escrow is held",
            ),
            (
                "quest.rs",
                include_str!("../quest.rs"),
                "fn quest_timer_pass(",
                "the 0.5s timed-quest expiry pass writes `game_character_quest`, a MANIFEST table — \
                 the same table and the same reasoning that fenced its harness twin \
                 `debug::debug_expire_quest`, which this pass reached past",
            ),
        ];
    for (file, src, signature, why) in sites {
        let body = code_of(src, signature);
        assert!(
            body.contains("is_in_transit"),
            "{file}'s `{signature}` no longer skips in-transit characters (issue #30). {why}. \
                 Body was:\n{body}"
        );
    }
}

/// The instance reaper's REFUSE verdict has a different shape (it refuses to read an instance
/// as EMPTY, rather than refusing to find a character), so it gets its own tripwire.
#[test]
fn the_instance_reaper_still_holds_instances_claimed_by_an_in_transit_character() {
    let body = code_of(include_str!("../instance.rs"), "fn occupied_instances(");
    assert!(
        body.contains("in_transit_instances"),
        "instance::occupied_instances no longer counts instances claimed by an in-transit \
             character. Occupancy is read from LIVE entities and begin_transfer deletes the live \
             entity, so without this the instance a transfer is flying into or out of reads as \
             empty, gets torn down, and takes its game_instance_binding manifest rows with it \
             (issue #30). Body was:\n{body}"
    );
}

/// The DEFER verdict — the shape test for the whole taxonomy. A REFUSAL here would silently
/// drop a third party's copper, so the fence must be a fold into the blob, not a rejection.
#[test]
fn credit_purse_defers_the_copper_into_the_blob_instead_of_dropping_it() {
    let body = code_of(include_str!("../loot/mod.rs"), "fn credit_purse(");
    assert!(
        body.contains("defer_money_delta"),
        "loot::credit_purse no longer defers a post-begin coin share into the escrowed export \
             blob (issue #30). This is the ONE audited by-guid path a refusal gets wrong: the \
             recipient is a party member collecting their share of someone else's kill, so \
             dropping the write shorts a third party who is not transferring and cannot know why. \
             Body was:\n{body}"
    );
    // ...and the fold must run BEFORE the durable-row write it accompanies, so an early return
    // added above it can never skip the deferral while still paying the row.
    let fold_at = body.find("defer_money_delta").expect("asserted above");
    let write_at = body.find("chars.guid().update(c)").expect(
        "credit_purse no longer writes the durable character row — re-derive this ordering",
    );
    assert!(
        fold_at < write_at,
        "the blob fold must precede the durable-row write. Body:\n{body}"
    );
}

/// The pure half of the DEFER verdict, driven directly: value in, value out, nothing dropped.
#[test]
fn folding_a_money_delta_adds_it_to_the_escrowed_blob() {
    let blob = ExportBlob {
        transfer_id: 7,
        character_guid: 42,
        money: 1_000,
        manifest: vec![ManifestEntry {
            table: "game_player_spell".to_string(),
            hot: true,
        }],
        dest_map_id: 36,
        dest_instance_id: 1,
        dest_x: 0.0,
        dest_y: 0.0,
        dest_z: 0.0,
        dest_o: 0.0,
        character_row: vec![42],
        payload: Vec::new(),
    };
    let bytes = spacetimedb::sats::bsatn::to_vec(&blob).expect("serializes");
    let folded = fold_money_delta(&bytes, 250).expect("folds");
    let out: ExportBlob = spacetimedb::sats::bsatn::from_slice(&folded).expect("round-trips");
    assert_eq!(
        out.money, 1_250,
        "the deferred copper must land in the blob"
    );
    // Everything else is untouched — the fold is a delta, not a rewrite.
    assert_eq!(
        ExportBlob {
            money: 1_000,
            ..out.clone()
        },
        blob
    );

    // Saturating, like every other purse write in the module: an overflowing delta clamps
    // rather than wrapping a character's fortune back to nothing.
    let rich = spacetimedb::sats::bsatn::to_vec(&ExportBlob {
        money: u32::MAX - 1,
        ..blob
    })
    .expect("serializes");
    let folded = fold_money_delta(&rich, 99).expect("folds");
    let out: ExportBlob = spacetimedb::sats::bsatn::from_slice(&folded).expect("round-trips");
    assert_eq!(out.money, u32::MAX);

    // A corrupt blob is an Err, never a silent zero — the escrow is left byte-identical so the
    // single loud failure stays at `import_character`.
    assert!(fold_money_delta(b"not a blob", 1).is_err());
}

/// The REGENERATE verdict, pinned by its REASON rather than by a fence (there is none to
/// delete). `Character.owner_identity` is per-CONNECTION derived state: if the blob ever grows
/// a field for it, it ships a source-gateway identity that is meaningless at the destination
/// and overwritten on arrival — a field always wrong on arrival is worse than no field.
#[test]
fn owner_identity_is_regenerated_at_the_destination_never_carried() {
    let blob = code_of(include_str!("transport.rs"), "pub struct ExportBlob");
    assert!(
        !blob.contains("owner_identity"),
        "ExportBlob grew an owner_identity field. That value is rebound from the LIVE \
             connection by `establish_session` at every logon and restamped onto the owned rows by \
             `player_login`, so a carried copy arrives stale and is immediately overwritten — the \
             REGENERATE verdict in this file's table (issue #30). Blob was:\n{blob}"
    );
    // #468 stage 4d: restamp lives in the shared login core (owner = ctx.sender() on the sender
    // path, the account's bound identity on the gateway path — same regenerate semantics).
    let login = code_of(include_str!("../world.rs"), "pub(crate) fn apply_player_login(");
    assert!(
        login.contains("restamp_owned_data"),
        "world::player_login no longer restamps the owned rows from ctx.sender(), which is the \
             mechanism the REGENERATE verdict for `establish_session` depends on: without it, a \
             character arriving at a destination shard keeps a stale owner binding and its rows go \
             RLS-invisible to the player. Body was:\n{login}"
    );
}

#[test]
fn the_in_transit_predicate_is_the_login_fence_itself() {
    // `is_in_transit` must stay defined AS `!login_allowed(..)`: the exhaustive walk's
    // dual-liveness assertion is an assertion about `login_allowed`, and it only transfers to
    // the real gates because they share this one predicate.
    let body = body_of(include_str!("mod.rs"), "pub(crate) fn is_in_transit(");
    assert!(
        body.contains("!login_allowed(has_out, has_in)"),
        "is_in_transit stopped being `!login_allowed(..)`, so the crash matrix's dual-liveness \
             proof no longer covers the real chokepoints. Body was:\n{body}"
    );
}

// -------------------------------------------------------------------------------------
// CROSS-DATABASE (issue #19): the transport ratchet, and the six-step crash matrix
// -------------------------------------------------------------------------------------

/// THE RATCHET. A character-owned table with no `character_owned!(transfer, ..)` arm does not
/// cross a database boundary — and unlike a missing delete sweep (which leaks rows, loudly,
/// forever) that failure is INVISIBLE: the character simply arrives without that table's data,
/// and the source copy it came from has already been cascade-deleted. There is no second chance
/// and no error anywhere. So: every manifest table must have an arm, and a NEW character-owned
/// table fails this test by name in the same edit that adds it.
///
/// "Not transported" is a legal answer — via the `character_owned!(not_transported, ..)` marker
/// kind, written AT the table (see `rest.rs` / `group.rs`'s invite row) — because a decision
/// recorded at the table is a different thing from an omission nobody noticed.
///
/// Reads `crate::CHARACTER_OWNED_TRANSFER_NAMES`, the plain-string half of the generated
/// registry (#380). Deliberately not `crate::CHARACTER_OWNED_TRANSFERS` itself: referencing that
/// array materializes every registered fn's POINTER, which drags the SpacetimeDB host imports
/// (`datastore_insert_bsatn`, `row_iter_bsatn_advance`, …) into this native test binary, which
/// cannot link them. Same reasoning — and the same discovery-by-linker-error — as
/// `tripwires::build_scan_strip_tripwire::commented_out_markers_do_not_register`. Before #380 this test string-parsed the
/// generated Rust source to get the same list; build.rs emits it directly now.
#[test]
fn every_manifest_table_can_cross_a_database_boundary() {
    let transports: Vec<String> = crate::CHARACTER_OWNED_TRANSFER_NAMES
        .iter()
        .map(|t| (*t).to_string())
        .collect();
    assert!(
        !transports.is_empty(),
        "the generated transport registry is EMPTY — the build.rs marker scan found no \
             `character_owned!(transfer, ..)` arms at all, so nothing would cross a database \
             boundary and this ratchet would pass vacuously"
    );
    let mut missing = Vec::new();
    for entry in manifest() {
        if !transports.contains(&entry.table) {
            missing.push(entry.table);
        }
    }
    assert!(
            missing.is_empty(),
            "character-owned table(s) with NO cross-database transport arm: {missing:?}\n\
             Their rows would be silently dropped the first time a character moves between two \
             SpacetimeDB databases (issue #19) — the source copy is cascade-deleted by \
             finish_transfer, so the data is gone with no error anywhere. Add \
             `crate::character_owned!(transfer, fn sweep_transfer_<accessor>(ctx, guid, io) {{ .. }})` \
             next to the table (see any of the existing arms), or `transfer::not_transported(io)` if \
             the rows genuinely must not cross — but write the decision AT the table."
        );
    // ...and no arm may name a table that is not in the manifest (a rename that left the arm's
    // `sweep_transfer_<accessor>` name stale would ship rows under a table nobody imports).
    let tables: Vec<String> = manifest().into_iter().map(|e| e.table).collect();
    for t in &transports {
        assert!(
            tables.contains(t) || MANIFEST_EXCLUDE.contains(&t.as_str()),
            "transport arm names {t}, which is not a manifest table — check the \
                 `sweep_transfer_<table_accessor>` naming"
        );
    }
}

/// THE RATCHET'S SECOND HALF, and the one that actually stops silent data loss.
///
/// `every_manifest_table_can_cross_a_database_boundary` only proves an arm EXISTS. It cannot
/// tell a real transport arm from one that declines to carry anything — so the single edit it is
/// supposed to prevent used to walk straight past it. Verified by mutation: repointing
/// `sweep_transfer_game_item_instance` at `not_transported` left all 468 module tests green
/// while deleting every character's entire inventory and equipped gear on every shard hop.
///
/// **The mechanism changed in #380.** "Transports" vs "declines" is no longer a property of an
/// arm's BODY that a scanner has to read back out of the source — a transport arm has no body
/// any more. It is the `character_owned!` marker KIND (`transfer` vs `not_transported`), which
/// build.rs already parses, so the mechanical half of the decision arrives here as a generated
/// list. The reasoned half is [`NOT_TRANSPORTED`], where each entry is written out with its
/// justification. This test is the equality between them, and it fails in BOTH directions: a
/// table that stops transporting without being justified, and an allowlist entry that quietly
/// started transporting again (or that names a table which no longer exists).
///
/// What this replaced was a 100-line brace-depth parser plus a documented adversarial arms race
/// — `contains("move_rows")`, then "exactly once", then "at the top of the arm", then "and its
/// export closure filters by the guid" — each round added after the previous one was defeated by
/// a dead branch. None of those mutations can be written any more: the macro emits the
/// `move_rows` call, the guid filter and the destination insert itself, so an arm that skips any
/// of them does not parse.
#[test]
fn the_not_transported_allowlist_matches_the_arms_that_decline() {
    let mut declared: Vec<&str> = NOT_TRANSPORTED.to_vec();
    declared.sort_unstable();
    assert_eq!(
            declared,
            crate::CHARACTER_OWNED_NOT_TRANSPORTED,
            "`transfer::NOT_TRANSPORTED` and the tables whose arms are written with the \
             `character_owned!(not_transported, ..)` marker kind disagree.\n\
             \n\
             A table on the GENERATED side but not on NOT_TRANSPORTED: its rows are silently \
             DROPPED on every database hop — no error, and `finish_transfer` cascade-deletes the \
             source copy they came from moments later. If that really is the decision, add the \
             table to NOT_TRANSPORTED *with its reason*; otherwise write its arm as a `transfer` \
             arm.\n\
             A table on NOT_TRANSPORTED but not on the generated side: the written decision is \
             stale — either the arm transports again, or a rename left the allowlist naming a table \
             that no longer exists, which silently licenses the next table to take that name."
        );
    assert!(
        !crate::CHARACTER_OWNED_NOT_TRANSPORTED.is_empty(),
        "the generated decline list is EMPTY — build.rs's `not_transported` marker scan found \
             nothing, and an empty list makes the equality above pass vacuously the moment \
             NOT_TRANSPORTED is emptied too"
    );
}

/// The transport arms are the only thing that moves rows — but `move_rows` is what makes them
/// move anything, and NOTHING in this crate executes it (every module test is pure or a source
/// scan; the arms need a `ReducerContext`). Verified by mutation: making the `Import` arm an
/// unconditional early return — so that NO manifest table's rows ever arrive, on any transfer —
/// left all 468 module tests green. Until there is a harness that can run a reducer natively,
/// the two halves are pinned here.
/// REAL behavioural coverage of the row codec — the half of the transport that actually carries
/// player data, and which had none: every module test is pure or a source scan, so nothing in
/// this crate ever executed `move_rows`. Mutation-testing confirmed the gap (making the import
/// arm inert, so that NO table's rows ever arrive on any transfer, left all 468 tests green).
/// [`encode_rows`]/[`decode_rows`] exist as `ReducerContext`-free halves precisely so this test
/// can drive them.
#[test]
fn the_row_codec_round_trips_and_refuses_garbage() {
    let rows = vec![
        ManifestEntry {
            table: "game_item_instance".into(),
            hot: true,
        },
        ManifestEntry {
            table: "game_character_quest".into(),
            hot: false,
        },
    ];
    let bytes = encode_rows(rows.clone());
    assert!(
        !bytes.is_empty(),
        "a non-empty table must serialize to a non-empty payload"
    );

    let mut outcome = Ok(());
    let back: Vec<ManifestEntry> = decode_rows(&bytes, &mut outcome);
    assert!(outcome.is_ok());
    assert_eq!(back, rows, "every arriving row must come back, in order");

    // An empty payload is "this table had no rows", not an error.
    let mut outcome = Ok(());
    let none: Vec<ManifestEntry> = decode_rows(&[], &mut outcome);
    assert!(none.is_empty() && outcome.is_ok());

    // Garbage is refused LOUDLY and yields nothing — a half-applied table is the one outcome
    // worse than none, because the in-row filed afterwards licenses deleting the source copy.
    let mut outcome = Ok(());
    let broken: Vec<ManifestEntry> = decode_rows(&[0xff, 0xff, 0xff, 0xff], &mut outcome);
    assert!(
        broken.is_empty(),
        "a payload that does not decode must apply NOTHING"
    );
    assert!(
        outcome.is_err(),
        "a payload that does not decode must record the failure"
    );

    // ...and the FIRST failure is the one kept (import_rows walks every table with one outcome).
    let first = outcome.clone();
    let _: Vec<ManifestEntry> = decode_rows(&[0x01], &mut outcome);
    assert_eq!(
        outcome, first,
        "a later failure must not overwrite the first one"
    );
}

#[test]
fn move_rows_delegates_to_the_codec_in_both_directions() {
    let body = code_of(
        include_str!("transport.rs"),
        "pub(crate) fn move_rows<C, R>(",
    );
    assert!(
        body.contains("encode_rows(export())"),
        "move_rows' Export arm no longer serializes the exported rows — every table would ship \
             an empty payload and the character arrives naked. Body was:\n{body}"
    );
    assert!(
        body.contains("decode_rows::<R>(bytes, outcome)") && body.contains("insert(ctx, r)"),
        "move_rows' Import arm no longer decodes and INSERTS the arriving rows. That one arm is \
             the whole of cross-database row delivery: with it inert every character arrives with \
             no gear, spells, skills, talents, reputations or quest log, and finish_transfer \
             destroys the source copy they came from a moment later. Body was:\n{body}"
    );
    // Neither arm may short-circuit: an early return anywhere in this shim drops a whole
    // table's rows silently, and the codec's own empty-payload case is handled in `decode_rows`.
    assert!(
        !body.contains("return"),
        "move_rows grew an early return. It is a two-line shim over the codec precisely so it \
             cannot acquire a path that skips a table. Body was:\n{body}"
    );
    // Export must serialize the rows the CHARACTER owns — a mover handed the wrong guid ships
    // an empty payload for every table, with no error anywhere.
    let exp = code_of(
        include_str!("transport.rs"),
        "pub(crate) fn export_rows_via<C>(",
    );
    assert!(
        exp.contains("mover(ctx, character_guid,"),
        "export_rows no longer passes the transferring character's guid to each mover — every \
             table would export the rows of some other guid (or none). Body was:\n{exp}"
    );
}

/// The cross-database world: FOUR ledger facts instead of two, because neither database can
/// read the other's. `in_row_src` is the ATTESTATION (`confirm_import`); `in_row_dst` is the
/// arrival copy's own fence, cleared by `release_transfer`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
struct Xdb {
    src_durable: bool,
    dst_durable: bool,
    out_row: bool,
    in_row_src: bool,
    in_row_dst: bool,
}

impl Xdb {
    fn initial() -> Self {
        Xdb {
            src_durable: true,
            dst_durable: false,
            out_row: false,
            in_row_src: false,
            in_row_dst: false,
        }
    }
    /// The SAME predicate the real gates use, applied to each database's own ledger rows.
    fn live_src(&self) -> bool {
        self.src_durable && login_allowed(self.out_row, self.in_row_src)
    }
    fn live_dst(&self) -> bool {
        // The destination holds no out-row (the escrow's source claim is on the other database),
        // so its fence is the in-row alone — which `login_allowed` already covers.
        self.dst_durable && login_allowed(false, self.in_row_dst)
    }
    fn settled(&self) -> bool {
        !self.out_row && !self.in_row_src && !self.in_row_dst
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum XStep {
    Begin,
    /// `import_character_blob` at the destination. Only reachable when the source escrow
    /// exists, because the blob the gateway carries is what `begin_transfer` produced.
    Import,
    /// `confirm_import` on the source. GATED on the destination copy being durable — that gate
    /// is the gateway's obligation (`run_transfer` calls it only after the import committed),
    /// not something the source database can check.
    Confirm,
    /// `finish_transfer` on the source.
    Finish,
    /// `release_transfer` at the destination.
    Release,
    /// The SOURCE reaper on a stale escrow.
    ReapSrcStale,
    /// The DESTINATION reaper on its stale arrival row — must be inert (no out-row there).
    ReapDst,
}

const ALL_XSTEPS: &[XStep] = &[
    XStep::Begin,
    XStep::Import,
    XStep::Confirm,
    XStep::Finish,
    XStep::Release,
    XStep::ReapSrcStale,
    XStep::ReapDst,
];

fn xstep(m: Xdb, s: XStep) -> Xdb {
    match s {
        XStep::Begin => match plan_begin(
            if m.out_row { Some(GUID) } else { None },
            GUID,
            m.src_durable,
            !login_allowed(m.out_row, m.in_row_src),
        ) {
            BeginPlan::Escrow => Xdb { out_row: true, ..m },
            _ => m,
        },
        // No blob without an escrow; replay on the in-row PK.
        XStep::Import => {
            if !m.out_row || m.in_row_dst {
                m
            } else {
                Xdb {
                    dst_durable: true,
                    in_row_dst: true,
                    ..m
                }
            }
        }
        // `confirm_import` refuses with no out-row; the gateway only calls it post-import.
        XStep::Confirm => {
            if m.out_row && m.in_row_dst {
                Xdb {
                    in_row_src: true,
                    ..m
                }
            } else {
                m
            }
        }
        XStep::Finish => match plan_finish(m.out_row, m.in_row_src) {
            // Delete-last: the source copy is cascade-deleted, then the escrow rows.
            FinishPlan::Complete => Xdb {
                src_durable: false,
                out_row: false,
                in_row_src: false,
                ..m
            },
            FinishPlan::AlreadyDone | FinishPlan::NotImported => m,
        },
        XStep::Release => Xdb {
            in_row_dst: false,
            ..m
        },
        XStep::ReapSrcStale => {
            // The cross-database reading of the local in-row: absent means UNATTESTED, not
            // "not imported".
            let imported = if m.in_row_src { Some(true) } else { None };
            match recovery(m.out_row, imported, TRANSFER_STALE_MICROS) {
                Recovery::Hold => m,
                Recovery::Rollback => Xdb {
                    out_row: false,
                    ..m
                },
                Recovery::RollForward => Xdb {
                    src_durable: false,
                    out_row: false,
                    in_row_src: false,
                    ..m
                },
            }
        }
        // The destination has no out-row, so `recovery` answers Hold: an arrival copy is never
        // reaped by the database it arrived on.
        XStep::ReapDst => {
            assert_eq!(
                recovery(false, Some(m.in_row_dst), TRANSFER_STALE_MICROS),
                Recovery::Hold
            );
            m
        }
    }
}

// Same `!(<the bad state>)` convention as `check` above, for the same readability reason.
#[allow(clippy::nonminimal_bool)]
fn xcheck(m: Xdb, trace: &[XStep]) {
    assert!(
        m.src_durable || m.dst_durable,
        "ZERO DURABLE COPIES after {trace:?} — the character was lost (state {m:?})"
    );
    assert!(
        !(m.live_src() && m.live_dst()),
        "DUAL LIVENESS after {trace:?} — the character is actable on BOTH databases, which is \
             the cross-database dupe this whole protocol exists to prevent (state {m:?})"
    );
    assert!(
        !(m.dst_durable && m.src_durable && m.settled()),
        "TWO SETTLED COPIES after {trace:?} — the escrow cleared with a durable character on \
             both databases; whichever one the player logs into next, the other is a ghost dupe \
             (state {m:?})"
    );
    assert!(
        !(m.in_row_src && !m.out_row),
        "orphan attestation after {trace:?}: the source's in-row outlived its own out-row, so \
             the escrow is no longer readable as in-flight (state {m:?})"
    );
}

#[test]
fn the_cross_database_sequence_never_dupes_and_never_loses_the_character() {
    // Same reasoning as the same-database walk: one step = one transaction, so TRUNCATING a
    // sequence models a crash at that boundary, and every truncation is itself enumerated.
    // Depth 7 covers the full happy path (begin→import→confirm→finish→release) plus two
    // arbitrary extra steps at any position.
    const DEPTH: usize = 7;
    let mut seen = std::collections::HashSet::new();
    fn walk(
        m: Xdb,
        depth: usize,
        trace: &mut Vec<XStep>,
        seen: &mut std::collections::HashSet<Xdb>,
    ) {
        xcheck(m, trace);
        seen.insert(m);
        if depth == 0 {
            return;
        }
        for &s in ALL_XSTEPS {
            trace.push(s);
            walk(xstep(m, s), depth - 1, trace, seen);
            trace.pop();
        }
    }
    walk(Xdb::initial(), DEPTH, &mut Vec::new(), &mut seen);
    assert!(
        seen.len() >= 6,
        "the walk only reached {} states — the model is not moving",
        seen.len()
    );
    // The happy path really does end with the character live on the destination and NOTHING
    // durable on the source (i.e. the walk above is not vacuously safe by never moving).
    let mut m = Xdb::initial();
    for s in [
        XStep::Begin,
        XStep::Import,
        XStep::Confirm,
        XStep::Finish,
        XStep::Release,
    ] {
        m = xstep(m, s);
    }
    assert!(m.settled() && m.live_dst() && !m.src_durable, "{m:?}");
}

#[test]
fn a_cross_database_escrow_is_never_rolled_back_before_it_is_attested() {
    // THE cross-database safety rule. Rolling back would unfreeze the source copy — and the
    // destination copy may already be durable and live, because the source cannot see it. The
    // reaper must HOLD (recoverable) rather than guess (a duplicated character).
    let escrowed = xstep(Xdb::initial(), XStep::Begin);
    let imported = xstep(escrowed, XStep::Import);
    for m in [escrowed, imported] {
        assert_eq!(
            xstep(m, XStep::ReapSrcStale),
            m,
            "an unattested cross-database escrow must be HELD, not rolled back: {m:?}"
        );
    }
    // Once attested, roll-forward IS correct — a driver that died between confirm and finish
    // is completed by the reaper rather than leaving the player frozen forever.
    let attested = xstep(imported, XStep::Confirm);
    let reaped = xstep(attested, XStep::ReapSrcStale);
    assert!(!reaped.src_durable && !reaped.out_row, "{reaped:?}");
    assert!(
        reaped.dst_durable,
        "roll-forward must not touch the destination copy"
    );
}

#[test]
fn finish_before_the_attestation_cannot_destroy_the_source_copy() {
    // `finish_transfer` is the only step that destroys the source copy, and cross-database the
    // in-row it demands is the gateway's attestation that the destination copy is durable. An
    // out-of-order finish therefore refuses — the same `plan_finish` guard as same-database,
    // which is the point of routing the cross-database flow back through it.
    let escrowed = xstep(Xdb::initial(), XStep::Begin);
    let imported = xstep(escrowed, XStep::Import);
    assert_eq!(
        plan_finish(escrowed.out_row, escrowed.in_row_src),
        FinishPlan::NotImported
    );
    assert_eq!(xstep(escrowed, XStep::Finish), escrowed);
    assert_eq!(
        xstep(imported, XStep::Finish),
        imported,
        "not attested yet — still refused"
    );
}

#[test]
fn the_arrival_copy_stays_fenced_until_the_source_copy_is_gone() {
    // Release is LAST for a reason: while the source copy still exists, a live destination copy
    // plus a source that could be unfrozen is the dupe. Walk the happy path and assert the
    // destination is not live until finish has run.
    let mut m = Xdb::initial();
    for s in [XStep::Begin, XStep::Import, XStep::Confirm] {
        m = xstep(m, s);
        assert!(
            !m.live_dst(),
            "the arrival copy must stay fenced through {s:?}: {m:?}"
        );
        assert!(
            !m.live_src(),
            "the source copy is frozen from begin onwards: {m:?}"
        );
    }
    let finished = xstep(m, XStep::Finish);
    assert!(
        !finished.src_durable,
        "the source copy is gone before the release"
    );
    let released = xstep(finished, XStep::Release);
    assert!(released.live_dst() && released.settled(), "{released:?}");
}

// --- ENFORCEMENT tripwires for the cross-database machinery (a pure model sees no reducers) ---

// `finish_destroys_the_source_copy_for_a_cross_database_transfer` and its ordering assertions
// used to live here as a source scan of `fn do_finish(`. `do_finish` is now a two-line adapter
// over `apply_finish` (issue #34's seam), and every property that scan asserted — the cascade,
// its `cross_database` gate, detach-before-cascade, cascade-before-record_shard — is executed
// for real against `harness::FakeDb`. See `mod harness`'s `finish_*` tests.

#[test]
fn import_character_blob_still_proves_the_destination_copy_is_durable() {
    // The reducer is now a two-line adapter over `apply_import_blob` (issue #37's seam), so the
    // guards below live in that body — but the ADAPTER still has to call it, and still has to
    // gate on the operator. Both, in order, or the whole harness is testing dead code.
    let shim = code_of(include_str!("mod.rs"), "pub fn import_character_blob(");
    let gate_at = shim.find("require_operator(ctx)?").expect(
        "import_character_blob dropped its operator gate — materialising a character from a \
             client-supplied blob is the whole of shard-hop forgery",
    );
    let call_at = shim.find("apply_import_blob(").unwrap_or_else(|| {
        panic!(
            "import_character_blob no longer calls `apply_import_blob` — every guard the \
                 harness executes is then unreachable from the reducer the gateway actually calls. \
                 Body was:\n{shim}"
        )
    });
    assert!(
        gate_at < call_at,
        "the operator gate must precede the import. Body:\n{shim}"
    );

    let body = code_of(
        include_str!("mod.rs"),
        "pub(crate) fn apply_import_blob<S: ImportSink>(",
    );
    assert!(
        body.contains("no durable row at the destination"),
        "import_character_blob no longer re-reads `game_character` to prove the arrival copy \
             materialised before filing the in-row. That in-row is what licenses `confirm_import` \
             and then `finish_transfer` to CASCADE-DELETE the source copy, so filing it against an \
             apply that silently did nothing settles the transfer with zero durable copies. Body \
             was:\n{body}"
    );
    assert!(
        body.contains("already has a LIVE entity on this shard"),
        "import_character_blob no longer refuses a character that is already LIVE here — \
             importing on top of a resident character is the dual-liveness dupe with extra steps \
             (and cross-database the source cannot see it). Body was:\n{body}"
    );
    // The MANIFEST-DRIFT guard — #16's contract, which the blob path inherited and which no
    // test pinned (verified by mutation: deleting it left all 468 module tests green). Without
    // it a shard on a different build accepts a payload for a table set it does not have, and
    // silently drops whatever it cannot place.
    assert!(
        body.contains("check_manifest(transfer_id, &decoded.manifest)?"),
        "import_character_blob no longer compares the arriving manifest against THIS build's. \
             A shard running a different character-owned table set would accept the import and \
             silently drop the tables it does not know — with the source copy deleted moments \
             later. Body was:\n{body}"
    );
    // The payload must be applied FALLIBLY: `import_rows` refuses an unknown table or an
    // undecodable one, and swallowing that error commits a PARTIAL character and then files the
    // in-row that licenses destroying the whole source copy.
    assert!(
        body.contains("sink.import_rows(guid, &decoded.payload)?"),
        "import_character_blob no longer propagates `import_rows`' error. A partial import is \
             the one outcome worse than none: the in-row filed immediately below it licenses \
             finish_transfer to cascade-delete the source copy the missing rows came from. Body \
             was:\n{body}"
    );
    // #30's `defer_money_delta` residual: the blob's money is the escrowed value PLUS every
    // delta folded in after the freeze, and this assignment is the only thing that replays them
    // at the destination. Dropping it left all 468 module tests green.
    assert!(
        body.contains("c.money = decoded.money"),
        "import_character_blob no longer applies `blob.money`, so a `credit_purse` folded into \
             the escrow by `defer_money_delta` after the freeze (issue #30's DEFER verdict) dies \
             with the source copy. Body was:\n{body}"
    );
}

/// THE SEAM'S OWN BLIND SPOT — `CtxShard` and the reducer shims, pinned by EXACT SHAPE.
///
/// `harness` executes every `apply_*` body, the codec and both registry loops. What it can
/// never execute is the thin layer that binds them to a real `ReducerContext`: [`CtxShard`]'s
/// one-line methods and the `#[reducer]` shims over them. The harness substitutes `FakeDb` for
/// every one of them, so any edit there is invisible to it — and each is a single line whose
/// damage is total. Verified by mutation, all green before this test existed:
///
/// | edit | live consequence |
/// |---|---|
/// | `CtxShard::import_rows` → `Ok(())` | **no manifest table's rows ever arrive** |
/// | `CtxShard::export_rows` → `vec![]` | every table exports an EMPTY payload |
/// | `CtxShard::freeze_live_entity` → no-op | the character stays LIVE on the shard it left |
/// | `CtxShard::arm_reaper` → no-op | recovery is never scheduled; an abandoned transfer is a permanently frozen player |
/// | `CtxShard::escrows` → `vec![]` | the reaper walks nothing, same outcome |
/// | `CtxShard::record_shard` → no-op | the source keeps no forwarding receipt |
/// | `CtxShard::ensure_shadow_account` → no-op | the arriving player cannot log in at all |
/// | `CtxShard::has_live_entity` → `false` | the dual-liveness dupe guard is dead |
/// | `CtxShard::bump_guid_high_water` → no-op | #59: this database can re-issue an imported character's guid |
/// | `CtxShard::own_guid_range` → `None` | #237's gate always reads "foreign"; a local arrival stops ratcheting |
/// | a `dest_y`/`dest_z` transposition in `begin_transfer`'s shim | the character arrives at the wrong place, and NOTHING else can see it |
/// | `return Ok(())` above any shim's `apply_*` call | the reducer the gateway calls does nothing while every test passes |
///
/// **Why this pin survived #380 when the others did not.** The plan was to replace every
/// exact-shape pin with the cargo-mutants gate (`.cargo/mutants.toml`), and for the pins over
/// `begin_transfer`'s and `reap_transfers`' 120-line BODIES that is exactly what happened —
/// those bodies are `apply_begin`/`apply_reap` now and the harness runs them, so the stringified
/// twin that had to be edited alongside every legitimate change is gone. But cargo-mutants
/// cannot rescue THIS layer, and that is a measurement, not a guess: the first full run reported
/// 54 MISSED mutants across `CtxShard` and 6 more across the shims, because a mutation tool can
/// only ask whether a test FAILS, and no headless test in this crate can execute a
/// `ReducerContext` at all. So the instrument here is still equality — not `contains`, because a
/// `contains` scan is defeated by leaving its own text in a dead branch.
///
/// The cost is real and is the right one: this layer is *supposed* to be twenty-odd
/// pass-throughs, it should almost never change, and if it stops being that then the harness
/// below it stops meaning what it claims. Re-bless a deliberate change here with the same care.
#[test]
fn the_production_adapter_is_the_pass_through_the_harness_assumes() {
    let src = include_str!("mod.rs");
    for (signature, want) in [
            (
                "impl ShardLedger for CtxShard<'_> {",
                "{ fn out_row(&self, transfer_id: u64) -> Option<TransferOut> { self.ctx .db \
                 .game_transfer_out() .transfer_id() .find(transfer_id) } fn in_row(&self, transfer_id: u64) \
                 -> Option<TransferIn> { self.ctx .db .game_transfer_in() .transfer_id() .find(transfer_id) } \
                 fn file_out_row(&mut self, row: TransferOut) { self.ctx.db.game_transfer_out().insert(row); \
                 } fn file_in_row(&mut self, row: TransferIn) { self.ctx.db.game_transfer_in().insert(row); } \
                 fn delete_out_row(&mut self, transfer_id: u64) { self.ctx .db .game_transfer_out() \
                 .transfer_id() .delete(transfer_id); } fn delete_in_row(&mut self, transfer_id: u64) { \
                 self.ctx .db .game_transfer_in() .transfer_id() .delete(transfer_id); } fn \
                 has_character(&self, guid: u64) -> bool { \
                 self.ctx.db.game_character().guid().find(guid).is_some() } fn now_micros(&self) -> i64 { \
                 self.ctx.timestamp.to_micros_since_unix_epoch() } }",
            ),
            (
                "impl BeginSink for CtxShard<'_> {",
                "{ fn has_active_taxi_flight(&self, guid: u64) -> bool { \
                 crate::taxi::is_in_flight(self.ctx, guid) } fn \
                 is_in_transit(&self, guid: u64) -> bool { is_in_transit(self.ctx, guid) } fn \
                 freeze_live_entity(&mut self, guid: u64) { let entities = self.ctx.db.game_world_entity(); \
                 if let Some(e) = entities.guid().find(guid) { crate::world::persist_entity(self.ctx, &e, \
                 false); entities.guid().delete(guid); } } fn export_rows(&self, guid: u64) -> Vec<TableRows> \
                 { export_rows(self.ctx, guid) } fn with_character<T>( &self, guid: u64, f: impl \
                 FnOnce(&crate::character::Character) -> T, ) -> Option<T> { self.ctx .db .game_character() \
                 .guid() .find(guid) .map(|c| f(&c)) } fn arm_reaper(&mut self) { let sched = \
                 self.ctx.db.game_transfer_reaper_schedule(); if sched.iter().next().is_none() { \
                 sched.insert(TransferReaperSchedule { scheduled_id: 0, scheduled_at: \
                 ScheduleAt::Interval(TimeDuration::from_micros( TRANSFER_REAP_INTERVAL_MICROS as i64, )), \
                 }); } } }",
            ),
            (
                "impl ImportSink for CtxShard<'_> {",
                "{ fn has_live_entity(&self, guid: u64) -> bool { \
                 self.ctx.db.game_world_entity().guid().find(guid).is_some() } fn \
                 cascade_delete_character(&mut self, guid: u64) { \
                 crate::world::cascade_delete_character(self.ctx, guid); } fn insert_character(&mut self, c: \
                 crate::character::Character) { self.ctx.db.game_character().insert(c); } fn import_rows(&mut \
                 self, guid: u64, payload: &[TableRows]) -> Result<(), String> { import_rows(self.ctx, guid, \
                 payload) } fn ensure_shadow_account(&mut self, account_id: u64) { \
                 crate::auth::ensure_shadow_account(self.ctx, account_id); } fn bump_guid_high_water(&mut \
                 self, guid: u64) { crate::auth::bump_guid_high_water(self.ctx, guid); } fn \
                 own_guid_range(&self) -> Option<(u64, u64)> { self.ctx .db .game_guid_range() .id() .find(0) \
                 .map(|r| (r.base, r.size)) } }",
            ),
            (
                "impl FinishSink for CtxShard<'_> {",
                "{ fn detach_for_transfer(&mut self, guid: u64) { crate::group::detach_for_transfer(self.ctx, \
                 guid); } fn cascade_delete_character(&mut self, guid: u64) { \
                 crate::world::cascade_delete_character(self.ctx, guid); } fn record_shard(&mut self, guid: \
                 u64, map_id: u32, instance_id: u64) { crate::realm_core::record_shard(self.ctx, guid, \
                 map_id, instance_id); } }",
            ),
            (
                "impl ReapSink for CtxShard<'_> {",
                "{ fn escrows(&self) -> Vec<(u64, u64, i64, bool)> { self.ctx .db .game_transfer_out() \
                 .iter() .map(|o| { ( o.transfer_id, o.character_guid, o.created_micros, o.cross_database, ) \
                 }) .collect() } }",
            ),
            (
                // Taxi refusal precedes trade teardown; after that, trade teardown still precedes
                // the escrow write that flips the in-transit fence.
                "pub fn begin_transfer(",
                "{ require_operator(ctx)?; if crate::taxi::is_in_flight(ctx, character_guid) { return \
                 Err(\"PLAYER_IN_TAXI_FLIGHT\".to_string()); } \
                 crate::trade::cancel_trade_for(ctx, character_guid); \
                 apply_begin( &mut CtxShard { ctx }, transfer_id, character_guid, \
                 Destination { map_id: dest_map_id, instance_id: dest_instance_id, x: dest_x, y: dest_y, z: \
                 dest_z, o: dest_o, }, cross_database, ) }",
            ),
            (
                "pub fn import_character_blob(",
                "{ require_operator(ctx)?; apply_import_blob(&mut CtxShard { ctx }, transfer_id, blob) }",
            ),
            (
                "pub fn confirm_import(",
                "{ require_operator(ctx)?; apply_confirm(&mut CtxShard { ctx }, transfer_id) }",
            ),
            (
                "pub fn release_transfer(",
                "{ require_operator(ctx)?; apply_release(&mut CtxShard { ctx }, transfer_id) }",
            ),
            (
                "pub fn finish_transfer(",
                "{ require_operator(ctx)?; apply_finish_step(&mut CtxShard { ctx }, transfer_id) }",
            ),
            (
                "pub fn reap_transfers(",
                "{ if ctx.sender() != ctx.database_identity() { return; } apply_reap(&mut CtxShard { ctx }); \
                 }",
            ),
        ] {
            let want = want.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(
                shape_of(src, signature),
                want,
                "`{signature}` is no longer the exact pass-through `mod harness` assumes it is. The \
                 harness runs the SHARED body underneath this layer with a `FakeDb` substituted for \
                 every line here, and cargo-mutants cannot reach it either (no headless test can \
                 execute a ReducerContext) — so nothing but this assertion covers the edit. A \
                 no-op'd method or a short-circuited shim leaves the whole suite green while the \
                 transport silently loses rows, or does nothing at all. If the change is \
                 deliberate, update the expected shape here with the same care."
            );
        }
}

#[test]
fn the_transport_arms_are_the_only_thing_that_moves_rows() {
    // `export_rows` must derive from the GENERATED registry, never a hand-kept list — the same
    // property `manifest()` has, for the same reason: a parallel list drifts silently and the
    // drift is invisible until a character arrives missing a table.
    let body = code_of(include_str!("transport.rs"), "pub(crate) fn export_rows(");
    assert!(
        body.contains("CHARACTER_OWNED_TRANSFERS"),
        "export_rows no longer iterates the generated transport registry. Body was:\n{body}"
    );
    // And an arriving table this build has no arm for must ABORT, never be skipped: a skipped
    // table is a table whose rows are dropped, and the source copy is about to be deleted.
    let body = code_of(
        include_str!("transport.rs"),
        "pub(crate) fn import_rows_via<C>(",
    );
    assert!(
        body.contains("refusing a partial import"),
        "import_rows no longer refuses an unknown table — it now silently drops its rows, and \
             `finish_transfer` will destroy the source copy they came from. Body was:\n{body}"
    );
    // Issue #42 AC 2 — the refusal must be LOUD, naming the tables in the module log and not
    // only in the returned error. Nothing in this crate can capture SpacetimeDB's `log::error!`,
    // so this is a source scan: the behavioural half (refuse, name them, file no in-row) is
    // covered for real by `a_payload_missing_a_manifest_table_is_refused_and_files_no_in_row`,
    // and gutting the log line alone was a live mutation survivor of that test.
    //
    // COUNTING the occurrences is not enough — that was itself a mutation survivor: strip the
    // names out of the format arguments and park the second occurrence in a dead
    // `let _dead = missing.join(", ");`, and the count is still two while the operator gets a
    // refusal with no diagnosis. So pin the join as the log macro's own ARGUMENT.
    let norm = body.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        norm.contains("missing.len(), missing.join(\", \") );"),
        "the missing-table refusal no longer LOGS the table names as `log::error!` arguments \
             (they must reach both the module log and the returned error) — an operator watching a \
             shard reject an import would see a refusal with no diagnosis. Body was:\n{body}"
    );
    assert!(
        body.contains("log::error!") && body.matches("missing.join(\", \")").count() == 2,
        "the missing-table refusal no longer names the tables in BOTH the module log and the \
             returned error. Body was:\n{body}"
    );
}

// -------------------------------------------------------------------------------------
//  SCAN-PINNED, and why (issue #37 AC 5)
//
//  Both guards below are a single call inside a reducer body, and their effect is only
//  observable in real table state — `teardown_instance_inner`'s row deletion and
//  `ensure_group`'s insert both need a `ReducerContext`, which no unit test can construct.
//  Extracting a sink trait for each (the seam `apply_import_blob` uses) would be more
//  scaffolding than the one line it protects, so these two stay TEXT-pinned. That is a
//  deliberate, written decision, not an oversight — `mod harness`'s header lists
//  them as the crate's remaining scan-only transfer guards.
// -------------------------------------------------------------------------------------

/// The cross-database eviction must KEEP the instance lease. `delete_row: true` destroys the
/// `game_instance` row and its `game_instance_binding` rows — the bindings that just travelled
/// in the arriving character's blob — so the next party member's `resolve_or_create_instance`
/// mints a SECOND instance and the party is split across two copies of the dungeon.
#[test]
fn the_cross_database_eviction_keeps_the_instance_lease() {
    let body = code_of(
        include_str!("../instance.rs"),
        "pub fn evict_instance_population(",
    );
    // ONE teardown call, so the `false` cannot be a decoy sitting next to a live `true` — the
    // dead-branch trick that defeats every `contains` scan.
    assert_eq!(
        body.matches("teardown_instance_inner(").count(),
        1,
        "evict_instance_population tears the instance down more than once. It has exactly one \
             teardown, and it keeps the lease. Body was:\n{body}"
    );
    assert!(
        body.contains("teardown_instance_inner(ctx, instance_id, false)"),
        "evict_instance_population no longer keeps the instance LEASE (issue #19 AC#2). \
             Evicting with `delete_row: true` destroys the `game_instance` row and its bindings, \
             so the next member to hop resolves to a BRAND-NEW instance and the party is split \
             across two copies of the run. Body was:\n{body}"
    );
}

/// Issue #22 (group slice) DELETED the #19 interim group mirror — membership is realm-core's,
/// and the blob carrying a `begin_transfer` snapshot of it would race the authority (which is
/// what made a party SPLIT across the boundary unable to see itself in the first place).
///
/// This is the inverse of the tripwire it replaces. Re-adding a transport arm for
/// `game_group_member` would put a second writer on membership: the blob's snapshot would land
/// at the destination AFTER the gateway pushed realm-core's roster there, so the stale copy
/// would win — silently, and only for characters that crossed a boundary.
#[test]
fn party_membership_does_not_ride_the_export_blob() {
    assert!(
        crate::CHARACTER_OWNED_TRANSFER_NAMES.contains(&"game_group_member"),
        "game_group_member lost its transport arm entirely (declining is an arm too) — the \
             ratchet can no longer say anything about it"
    );
    assert!(
            crate::CHARACTER_OWNED_NOT_TRANSPORTED.contains(&"game_group_member"),
            "`game_group_member` transports again. Party membership is authoritative on realm-core \
             (#22): the gateway re-pushes the roster onto the destination at world entry, and a \
             blob snapshot taken back at `begin_transfer` would overwrite it with the membership the \
             character had when it stepped into the portal."
        );
    assert!(
        NOT_TRANSPORTED.contains(&"game_group_member"),
        "the decision must also be written on the allowlist, with its reason"
    );
}
