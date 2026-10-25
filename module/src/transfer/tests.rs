use super::*;
use crate::Aura;
use spacetimedb::Timestamp;

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
fn manifest_exclude_holds_only_local_runtime_state() {
    assert_eq!(
        MANIFEST_EXCLUDE,
        [
            "game_transfer_out",
            "game_aoi_grant",
            "game_aoi_viewer",
            "game_native_player",
            "game_native_member_stats",
            "game_character_relocation",
            "game_transport_passenger",
        ],
        "Only local relocation, Transfer machinery and connection state may be excluded."
    );
}

#[test]
fn generated_table_names_look_like_real_accessors() {
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

#[test]
fn aura_absolute_expiry_survives_the_export_import_round_trip_with_the_same_remaining_time() {
    let applied_at = Timestamp::from_micros_since_unix_epoch(1_000_000_000);
    let expires_at = Timestamp::from_micros_since_unix_epoch(1_000_000_000 + 10 * 60 * 1_000_000);
    let aura = Aura {
        id: 77,
        target_guid: 42,
        caster_guid: 42,
        spell_id: 19750,
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

    assert_eq!(
        decoded[0].expires_at, expires_at,
        "expires_at was altered by the round trip"
    );
    assert_eq!(
        decoded[0].applied_at, applied_at,
        "applied_at was altered by the round trip"
    );

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Begin,
    Import,
    Finish,
    ReapFresh,
    ReapStale,
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

#[test]
fn exhaustive_interleavings_never_dupe_and_never_lose_the_character() {
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

#[test]
fn crash_point_a_during_begin_leaves_the_character_resident() {
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
    let aborted = m;
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
    assert_eq!(plan_import(false, false), ImportPlan::NoEscrow);
}

#[test]
fn begin_replay_is_a_no_op_in_both_escrowed_and_imported_phases() {
    let escrowed = step(Model::initial(), Step::Begin);
    assert_eq!(plan_begin(Some(GUID), GUID, true, true), BeginPlan::Replay);
    assert_eq!(step(escrowed, Step::Begin), escrowed);
    let imported = step(escrowed, Step::Import);
    assert_eq!(step(imported, Step::Begin), imported);
    assert_eq!(plan_begin(None, GUID, false, false), BeginPlan::NoSource);
}

#[test]
fn a_transfer_id_reused_for_a_different_character_is_refused_not_replayed() {
    assert_eq!(
        plan_begin(Some(GUID), GUID + 1, true, false),
        BeginPlan::IdCollision
    );
    assert_eq!(
        plan_begin(Some(GUID), GUID + 1, true, true),
        BeginPlan::IdCollision
    );
    assert_eq!(plan_begin(Some(GUID), GUID, true, true), BeginPlan::Replay);
}

#[test]
fn a_character_already_escrowed_under_another_id_cannot_be_escrowed_twice() {
    assert_eq!(
        plan_begin(None, GUID, true, true),
        BeginPlan::AlreadyInTransit
    );
    assert_eq!(plan_begin(None, GUID, true, false), BeginPlan::Escrow);
}

#[test]
fn reaper_never_guesses_when_the_destination_cannot_be_consulted() {
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
    assert_eq!(
        recovery(true, Some(false), TRANSFER_STALE_MICROS),
        Recovery::Rollback
    );
}

#[test]
fn the_recovery_verdict_is_enumerated_over_its_whole_input_space() {
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

#[test]
fn the_pure_planners_are_enumerated_over_their_whole_input_space() {
    assert_eq!(phase(false, false), Phase::Resident);
    assert_eq!(phase(false, true), Phase::Resident);
    assert_eq!(phase(true, false), Phase::Escrowed);
    assert_eq!(phase(true, true), Phase::Imported);

    assert_eq!(plan_import(false, false), ImportPlan::NoEscrow);
    assert_eq!(plan_import(true, false), ImportPlan::Apply);
    assert_eq!(plan_import(false, true), ImportPlan::Replay);
    assert_eq!(plan_import(true, true), ImportPlan::Replay);

    assert_eq!(plan_finish(false, false), FinishPlan::AlreadyDone);
    assert_eq!(plan_finish(false, true), FinishPlan::AlreadyDone);
    assert_eq!(
        plan_finish(true, false),
        FinishPlan::NotImported,
        "finishing an un-imported escrow destroys the ONLY durable copy — this refusal is the \
             delete-last invariant itself"
    );
    assert_eq!(plan_finish(true, true), FinishPlan::Complete);

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
        m = Model {
            src_durable: true,
            dst_durable: false,
            out_row: false,
            in_row: false,
        };
    }
}

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
    assert_eq!(
        ExportBlob {
            money: 1_000,
            ..out.clone()
        },
        blob
    );

    let rich = spacetimedb::sats::bsatn::to_vec(&ExportBlob {
        money: u32::MAX - 1,
        ..blob
    })
    .expect("serializes");
    let folded = fold_money_delta(&rich, 99).expect("folds");
    let out: ExportBlob = spacetimedb::sats::bsatn::from_slice(&folded).expect("round-trips");
    assert_eq!(out.money, u32::MAX);

    assert!(fold_money_delta(b"not a blob", 1).is_err());
}

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

    let mut outcome = Ok(());
    let none: Vec<ManifestEntry> = decode_rows(&[], &mut outcome);
    assert!(none.is_empty() && outcome.is_ok());

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
    fn live_src(&self) -> bool {
        self.src_durable && login_allowed(self.out_row, self.in_row_src)
    }
    fn live_dst(&self) -> bool {
        self.dst_durable && login_allowed(false, self.in_row_dst)
    }
    fn settled(&self) -> bool {
        !self.out_row && !self.in_row_src && !self.in_row_dst
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum XStep {
    Begin,
    Import,
    Confirm,
    Finish,
    Release,
    ReapSrcStale,
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
        XStep::ReapDst => {
            assert_eq!(
                recovery(false, Some(m.in_row_dst), TRANSFER_STALE_MICROS),
                Recovery::Hold
            );
            m
        }
    }
}

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
    let escrowed = xstep(Xdb::initial(), XStep::Begin);
    let imported = xstep(escrowed, XStep::Import);
    for m in [escrowed, imported] {
        assert_eq!(
            xstep(m, XStep::ReapSrcStale),
            m,
            "an unattested cross-database escrow must be HELD, not rolled back: {m:?}"
        );
    }
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

const OLD_BLOBS_MAY_LACK: [&str; 7] = [
    "game_auction_bid_hold",
    "game_auction_hold",
    "game_character_away",
    "game_fatigue_state",
    "game_group_target_icon",
    "game_guild_fee_hold",
    "game_instance_removal",
];

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
    assert_eq!(oldest.len() + OLD_BLOBS_MAY_LACK.len(), manifest().len());
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
        ("game_fatigue_state", count),
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
    assert_eq!(applied.get(), 8);
}

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
        assert_eq!(manifest().len(), 45);
        assert_eq!(manifest_without(&OLD_BLOBS_MAY_LACK).len(), 38);
    }
}
