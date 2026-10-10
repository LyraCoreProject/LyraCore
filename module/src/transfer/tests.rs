//! The crash matrix, the pure-planner enumerations, and the transport ratchets.
//!
//! Split out of `transfer.rs`; `harness.rs` holds the half that EXECUTES the protocol
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

#[test]
fn manifest_exclude_holds_only_transfer_machinery() {
    assert_eq!(
        MANIFEST_EXCLUDE,
        // `game_transfer_out` is the escrow row itself: it must not ride inside its own export
        // blob. No table a CHARACTER owns belongs on this list.
        ["game_transfer_out"],
        "MANIFEST_EXCLUDE changed. Every name on it is dropped from the transfer manifest, from \
             every export blob, AND from the set an arriving payload must cover — so a \
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
fn command_issuer_import_accepts_only_its_arriving_character() {
    let owned = crate::bridge::PartyCommandIssuer {
        character_guid: GUID,
        last_sequence: 7,
    };
    let foreign = crate::bridge::PartyCommandIssuer {
        character_guid: GUID + 1,
        last_sequence: 8,
    };

    assert_eq!(admit_command_issuer_import(GUID, &[], false), Ok(()));
    assert_eq!(admit_command_issuer_import(GUID, &[owned], false), Ok(()));
    assert_eq!(
        admit_command_issuer_import(GUID, &[foreign], false),
        Err(CommandIssuerImportRefusal::OwnerMismatch {
            expected: GUID,
            actual: GUID + 1,
        })
    );
}

#[test]
fn command_issuer_import_rejects_extra_rows_and_destination_conflicts() {
    let rows = [
        crate::bridge::PartyCommandIssuer {
            character_guid: GUID,
            last_sequence: 7,
        },
        crate::bridge::PartyCommandIssuer {
            character_guid: GUID,
            last_sequence: 8,
        },
    ];

    assert_eq!(
        admit_command_issuer_import(GUID, &rows, false),
        Err(CommandIssuerImportRefusal::TooManyRows { count: 2 })
    );
    assert_eq!(
        admit_command_issuer_import(GUID, &rows[..1], true),
        Err(CommandIssuerImportRefusal::DestinationConflict {
            character_guid: GUID,
        })
    );
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
// Hot-state audit: auras ON the character (buffs/debuffs/Stealth) must ride the
// blob like every other manifest table. Before this, `game_aura` had NO `character_owned!`
// marker at all (its columns name `target_guid`/`caster_guid`, neither of which the tripwire
// in tripwires.rs recognizes), so a warm handoff silently dropped every buff, DoT, HoT and the
// Rogue's Stealth presence on the source database — the destination simply never got them.
// -------------------------------------------------------------------------------------

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

/// The exact question asked: does a 10-minute buff resume with the right REMAINING
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
        proc_flags: 0,
        proc_chance: 0,
        proc_ppm: 0.0,
        proc_ex: 0,
        proc_school_mask: 0,
        proc_family_name: 0,
        proc_family_flags: 0,
        proc_charges: 0,
        proc_icd_ms: 0,
        proc_ready_micros: 0,
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

/// The ENTIRE recovery decision, enumerated rather than sampled.
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

    // escrowed_guid — out-row first, in-row as the fallback that un-strands blocker 2.
    assert_eq!(escrowed_guid(None, None), None);
    assert_eq!(escrowed_guid(Some(7), None), Some(7));
    assert_eq!(
        escrowed_guid(None, Some(9)),
        Some(9),
        "an unreleased arrival must resolve the escrowed Character guid"
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

// -------------------------------------------------------------------------------------
// CROSS-DATABASE: the transport ratchet, and the six-step crash matrix
// -------------------------------------------------------------------------------------

/// THE RATCHET. A character-owned table with no `character_owned!(transfer,..)` arm does not
/// cross a database boundary — and unlike a missing delete sweep (which leaks rows, loudly,
/// forever) that failure is INVISIBLE: the character simply arrives without that table's data,
/// and the source copy it came from has already been cascade-deleted. There is no second chance
/// and no error anywhere. So: every manifest table must have an arm, and a NEW character-owned
/// table fails this test by name in the same edit that adds it.
///
/// "Not transported" is a legal answer, via the `character_owned!(not_transported,..)` marker
/// kind, written AT the table (see `rest.rs` / `group.rs`'s invite row) — because a decision
/// recorded at the table is a different thing from an omission nobody noticed.
///
/// Reads `crate::CHARACTER_OWNED_TRANSFER_NAMES`, the plain-string half of the generated
/// registry. Deliberately not `crate::CHARACTER_OWNED_TRANSFERS` itself: referencing that
/// array materializes every registered fn's POINTER, which drags the SpacetimeDB host imports
/// (`datastore_insert_bsatn`, `row_iter_bsatn_advance`, …) into this native test binary, which
/// cannot link them. Same reasoning — and the same discovery-by-linker-error — as
/// `tripwires::build_scan_strip_tripwire::commented_out_markers_do_not_register`. Before this test string-parsed the
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
             SpacetimeDB databases — the source copy is cascade-deleted by \
             finish_transfer, so the data is gone with no error anywhere. Add \
             `crate::character_owned!(transfer, fn sweep_transfer_<accessor>(ctx, guid, io) {{ .. }})` \
             next to the table (see any of the existing arms), or `transfer::not_transported(io)` if \
             the rows genuinely must not cross — but write the decision AT the table."
        );
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

/// One table, one transport arm — the invariant that keeps
/// [`the_not_transported_allowlist_matches_the_arms_that_decline`] honest now that it only covers
/// core tables. A second arm for a table that already has one exports its rows twice, and a second
/// arm that DECLINES is how a drop-in Package would stop a core table from crossing without any
/// core file changing. build.rs refuses to emit such a registry at all; this is the outcome of that
/// refusal, asserted where a reader of the allowlist will find it.
#[test]
fn every_transported_table_has_exactly_one_arm() {
    let mut seen: Vec<&str> = Vec::new();
    let mut doubled: Vec<&str> = Vec::new();
    for table in crate::CHARACTER_OWNED_TRANSFER_NAMES {
        if seen.contains(table) {
            doubled.push(table);
        }
        seen.push(table);
    }
    assert!(
        doubled.is_empty(),
        "table(s) with two transport arms: {doubled:?} — one arm carries the rows and the other \
         decides again, and which of the two wins is the order build.rs happened to scan the files \
         in"
    );
}

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

    let first = outcome.clone();
    let _: Vec<ManifestEntry> = decode_rows(&[0x01], &mut outcome);
    assert_eq!(
        outcome, first,
        "a later failure must not overwrite the first one"
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

/// (group slice) DELETED the interim group mirror — membership is realm-core's,
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
            "`game_group_member` transports again. Party membership is authoritative on Realm-core; \
             the Gateway re-pushes the roster onto the destination at world entry, and a \
             blob snapshot taken back at `begin_transfer` would overwrite it with the membership the \
             character had when it stepped into the portal."
        );
    assert!(
        NOT_TRANSPORTED.contains(&"game_group_member"),
        "the decision must also be written on the allowlist, with its reason"
    );
}

/// An Instance Removal belongs to the instance it counts down for. The source cascade deletes the
/// row when the Character leaves, and the blob carries nothing, so a Character that walks out of
/// the Instance Pool through the portal is never sent home later.
#[test]
fn the_instance_removal_dies_with_the_source_copy_and_is_not_carried() {
    assert!(
        crate::CHARACTER_OWNED_TABLES.contains(&"game_instance_removal"),
        "game_instance_removal has no delete sweep, so the source cascade leaves the countdown \
         running after the Character left the instance"
    );
    assert!(
        crate::CHARACTER_OWNED_NOT_TRANSPORTED.contains(&"game_instance_removal"),
        "game_instance_removal transports: the destination would count down for an instance the \
         Character is no longer in"
    );
    assert!(NOT_TRANSPORTED.contains(&"game_instance_removal"));
}

/// The tables an older blob may lack, written out: every manifest table added since Core commit
/// `decd7821`.
const OLD_BLOBS_MAY_LACK: [&str; 6] = [
    "game_auction_bid_hold",
    "game_auction_hold",
    "game_character_away",
    "game_group_target_icon",
    "game_guild_fee_hold",
    "game_instance_removal",
];

/// This build's manifest without `lacking`, as a build without those tables exported it.
fn manifest_without(lacking: &[&str]) -> Vec<ManifestEntry> {
    manifest()
        .into_iter()
        .filter(|entry| !lacking.contains(&entry.table.as_str()))
        .collect()
}

fn empty(table: &str) -> TableRows {
    TableRows {
        table: table.to_owned(),
        rows: Vec::new(),
    }
}

/// A blob from an older build lacks some of the listed tables. Each one alone, and all of them
/// together, must import, so a Transfer in flight across the publish finishes.
#[test]
fn a_blob_lacking_any_listed_table_imports_with_it_empty() {
    let carried = TableRows {
        table: "game_item_instance".to_owned(),
        rows: vec![1, 2, 3],
    };
    for table in OLD_BLOBS_MAY_LACK {
        let arriving = manifest_without(&[table]);
        assert_eq!(arriving.len() + 1, manifest().len(), "{table}");
        assert_eq!(check_manifest(7, &arriving), Ok(()), "{table}");
        assert_eq!(
            payload_for_this_build(&arriving, std::slice::from_ref(&carried)),
            vec![carried.clone(), empty(table)],
            "{table} arrives with no rows"
        );
    }
    let oldest = manifest_without(&OLD_BLOBS_MAY_LACK);
    assert_eq!(oldest.len() + 6, manifest().len());
    assert_eq!(check_manifest(7, &oldest), Ok(()));
    let mut filled = vec![carried.clone()];
    filled.extend(OLD_BLOBS_MAY_LACK.map(empty));
    assert_eq!(
        payload_for_this_build(&oldest, std::slice::from_ref(&carried)),
        filled
    );
    assert_eq!(
        payload_for_this_build(&manifest(), std::slice::from_ref(&carried)),
        vec![carried],
        "a current blob is imported as it came, so the coverage check still sees any gap"
    );
}

/// Only the listed tables may be missing. Any other gap, an extra table, or a changed hot mark is
/// still drift.
#[test]
fn any_other_manifest_difference_is_still_drift() {
    let mismatch = |arriving: &[ManifestEntry]| {
        check_manifest(7, arriving)
            .unwrap_err()
            .contains("manifest mismatch")
    };
    assert_eq!(check_manifest(7, &manifest()), Ok(()));
    assert!(mismatch(&manifest_without(&["game_item_instance"])));
    assert!(mismatch(&manifest_without(&[
        "game_auction_hold",
        "game_item_instance"
    ])));
    let mut extra = manifest();
    extra.push(ManifestEntry {
        table: "game_unknown_table".to_owned(),
        hot: false,
    });
    assert!(mismatch(&extra));
    let mut rehot = manifest_without(&["game_auction_hold"]);
    let entry = rehot
        .iter_mut()
        .find(|entry| entry.table == "game_character_away")
        .expect("the Auto-Reply table is in the manifest");
    entry.hot = !entry.hot;
    assert!(mismatch(&rehot));
}

/// The filled payload passes the coverage check that refuses a partial import.
#[test]
fn a_filled_old_payload_covers_every_transport_arm() {
    fn count(applied: &std::cell::Cell<usize>, _: u64, _: &mut RowIo<'_>) {
        applied.set(applied.get() + 1);
    }
    let applied = std::cell::Cell::new(0);
    let arms: &[TransportArm<'_, std::cell::Cell<usize>>] = &[
        ("game_item_instance", count),
        ("game_auction_bid_hold", count),
        ("game_auction_hold", count),
        ("game_character_away", count),
        ("game_group_target_icon", count),
        ("game_guild_fee_hold", count),
        ("game_instance_removal", count),
    ];
    let carried = [TableRows {
        table: "game_item_instance".to_owned(),
        rows: Vec::new(),
    }];
    assert!(import_rows_via(&applied, 73, &carried, arms).is_err());
    assert_eq!(applied.get(), 0);
    import_rows_via(
        &applied,
        73,
        &payload_for_this_build(&manifest_without(&OLD_BLOBS_MAY_LACK), &carried),
        arms,
    )
    .expect("the filled payload imports");
    assert_eq!(applied.get(), 7);
}

/// Pins the list and the Core manifest by count. A Core table added to the manifest fails here:
/// add it to `TRANSFER_TABLES_OLD_BLOBS_MAY_LACK` too, then change these numbers. A linked Package
/// adds its own tables, so the counts hold only for a build without one.
#[test]
fn the_tables_old_blobs_may_lack_are_pinned() {
    assert_eq!(TRANSFER_TABLES_OLD_BLOBS_MAY_LACK, OLD_BLOBS_MAY_LACK);
    for table in OLD_BLOBS_MAY_LACK {
        assert!(
            manifest().iter().any(|entry| entry.table == table),
            "{table} is not in this build's manifest"
        );
    }
    #[cfg(not(has_packages))]
    {
        assert_eq!(manifest().len(), 44);
        assert_eq!(manifest_without(&OLD_BLOBS_MAY_LACK).len(), 38);
    }
}
