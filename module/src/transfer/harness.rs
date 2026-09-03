//! The place where the transfer protocol is **executed** rather than scanned.
//!
//! # Why this did not exist
//!
//! `ReducerContext` cannot be constructed in a unit test, so every other test in `module/` is
//! either a pure model or a source scan. That left `export_rows` / `import_rows` / `move_rows` /
//! `import_character_blob` pinned by their TEXT: the review ran 21 mutations against that
//! surface and 17 left the suite green or hung it — including repointing
//! `sweep_transfer_game_item_instance` at `not_transported` (which deletes every character's
//! inventory and gear on every hop) with **468 passed, 0 failed**.
//!
//! # The seam
//!
//! Two ordinary Rust generalisations, no framework:
//!
//! 1. [`move_rows`] / [`export_rows_via`] / [`import_rows_via`] are generic over the CONTEXT type
//!    and take the transport registry as a PARAMETER. Production binds `C = ReducerContext` and
//!    `crate::CHARACTER_OWNED_TRANSFERS` (`export_rows`, `import_rows`); the harness binds
//!    `C = FakeDb` and [`ARMS`]. The loop bodies, the `MANIFEST_EXCLUDE` filter, the codec and the
//!    unknown-table refusal are the SAME code either way.
//! 2. **Every step of the protocol** is written against a sink trait over [`ShardLedger`] —
//!    `apply_begin`, `apply_import_blob`, `apply_confirm`, `apply_finish_step`/`apply_finish`,
//!    `apply_release`, `apply_reap`. `CtxShard` is the one production adapter; [`FakeDb`] is the
//!    test one. Every guard each reducer has is executed here against real bsatn blobs built by the
//!    real [`build_export_blob`] — including the whole six-step cross-database sequence, driven
//!    across TWO `FakeDb`s in
//!    `the_six_step_sequence_moves_a_populated_character_between_two_databases`.
//!
//! # The ceiling — what this harness still CANNOT run, and why
//!
//! The ~16 per-table `character_owned!(transfer, ..)` arms themselves. Their expansions are
//! `ctx.db.game_item_instance().by_owner_guid().filter(..)` — real table accessors that only exist
//! on a real `ReducerContext`, and making them generic would mean rewriting every table accessor in
//! the module behind a trait, which is not a testing change but a rewrite of the module. So:
//!
//! * **What is executed here**: the transport plumbing every arm flows through (the codec, the
//!   guid it is handed, the registry lookup, the export/import loops, `not_transported`), plus every
//!   `apply_*` step body.
//! * **What a source scan still covers, and why**:
//!   - each REAL table's arm EXISTS (`every_manifest_table_can_cross_a_database_boundary`) — read
//!     off build.rs's generated `CHARACTER_OWNED_TRANSFER_NAMES`, not off source text;
//!   - each arm transports rather than declining
//!     (`the_not_transported_allowlist_matches_the_arms_that_decline`) — likewise generated.
//!     moved this from a 100-line brace-depth parser to the `character_owned!` marker KIND, so
//!     "does this arm actually move rows" is a parse-time property now, not a scan's guess;
//!   - the cross-database eviction keeps the instance LEASE
//!     (`the_cross_database_eviction_keeps_the_instance_lease`) — one call inside a reducer whose
//!     only observable effect is real table state, and extracting a sink for it would be more
//!     scaffolding than the line it protects. A deliberate, written decision.
//! * **The seam's own blind spot** — `CtxShard`, the thin production layer this harness substitutes
//!   `FakeDb` for. Nothing here runs any of its methods, and each is a single line whose damage is
//!   total: no-op'ing `CtxShard::import_rows` means **no manifest table's rows ever arrive**, and an
//!   early `return Ok(())` in a reducer shim means the reducer the gateway calls does nothing at all
//!   while every test below still passes. It stays pinned by EXACT-SHAPE equality
//!   (`tests::the_production_adapter_is_the_pass_through_the_harness_assumes`), and the
//!   cargo-mutants run is what proved that pin still has to exist: a mutation tool can only ask
//!   whether a test FAILS, and 54 mutants across `CtxShard` were MISSED because no headless test can
//!   execute a `ReducerContext` at all. What DID retire is the pins over `begin_transfer`'s and
//!   `reap_transfers`' 120-line bodies — those bodies are `apply_begin`/`apply_reap` now, and this
//!   file runs them.
//! * **What is still not covered anywhere headless**: SpacetimeDB's transaction rollback. A real
//!   `Err` from `import_character_blob` unwinds every write it made; [`FakeDb`] keeps them. Every
//!   refusal test below therefore asserts on the **in-row** — the row whose absence is what
//!   actually stops `finish_transfer` from destroying the source copy — and never on "nothing was
//!   written".
//!
//! Deliberate simplification: three fake tables, not twenty. The transport is table-agnostic by
//! construction (one opaque `TableRows` per arm), so a fourth fake table would exercise no new line
//! of production code. The three are chosen to cover the three SHAPES an arm can have: transports,
//! transports with a different row type, and declines.

use super::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

// --- Fake manifest tables. Row types only need what `move_rows` requires. ---

/// Stands for `game_item_instance` & friends: the character's stuff.
#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
pub struct GearRow {
    pub owner: u64,
    pub slot: u32,
    pub item: u32,
}

/// A SECOND row type, so a payload that swapped two tables' bytes fails to decode rather than
/// silently applying the wrong table's rows.
#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
pub struct QuestRow {
    pub owner: u64,
    pub quest: u32,
    pub note: String,
}

/// Stands for `game_rest_state_event` & friends — an allowlisted `not_transported` table.
#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
pub struct RelayRow {
    pub owner: u64,
    pub blip: u32,
}

/// One "database". `RefCell`, not `Mutex`, on purpose: a re-entrant access PANICS (a named
/// test failure) instead of deadlocking. A hang is not a pass — see the same fix applied to the
/// gateway's `FakeShardDb`.
#[derive(Default)]
pub struct FakeDb {
    chars: RefCell<HashMap<u64, crate::character::Character>>,
    gear: RefCell<Vec<GearRow>>,
    quests: RefCell<Vec<QuestRow>>,
    relay: RefCell<Vec<RelayRow>>,
    /// `game_world_entity` — presence means the character is LIVE here.
    live: RefCell<HashSet<u64>>,
    // Test double: the map mirrors `game_transfer_in`'s columns, so the tuple is the row shape.
    #[allow(clippy::type_complexity)]
    /// `game_transfer_in`: transfer_id -> (character_guid, blob, created_micros).
    in_rows: RefCell<HashMap<u64, (u64, Vec<u8>, i64)>>,
    /// `game_transfer_out`: transfer_id -> the escrow's routing fields.
    out_rows: RefCell<HashMap<u64, XOut>>,
    /// `game_group_member`: character_guid -> group_id.
    group_members: RefCell<HashMap<u64, u64>>,
    /// Parties torn down by `remove_member`'s disband — what the character-owned DELETE sweep
    /// does to a party when one of its members is deleted, and precisely what
    /// `detach_for_transfer` exists to run AHEAD of (AC#4).
    disbanded: RefCell<HashSet<u64>>,
    /// `game_character_shard`: the forwarding receipt. CHARACTER-OWNED, so the cascade sweeps
    /// it — which is why `apply_finish` must record it AFTER the cascade, never before.
    shard_index: RefCell<HashMap<u64, (u32, u64)>>,
    accounts: RefCell<HashSet<u64>>,
    now: Cell<i64>,
    /// Makes `insert_character` silently do nothing — the ONE thing a real destination can do
    /// that this fake otherwise cannot: accept the call and materialise no row. That is the
    /// state `apply_import_blob`'s post-import PROOF exists to catch, and without a way to
    /// reach it the proof was pinned only by its own text (a mutation survivor).
    swallow_inserts: Cell<bool>,
    /// `game_guid_allocator.high_water`: what `bump_guid_high_water` has ratcheted
    /// it to. Starts at 0 (unseeded), same as a fresh database.
    guid_high_water: Cell<u64>,
    /// `game_guid_range`: the `(base, size)` THIS fake database mints from,
    /// if any. `None` (the default) models a database that never installed one — same as a
    /// fresh real database before `install_guid_range` runs.
    guid_range: Cell<Option<(u64, u64)>>,
    /// `game_transfer_reaper_schedule`: has `begin_transfer` armed the reaper on this database?
    /// A bool rather than a row, because the only thing the protocol asserts about it is that
    /// the FIRST escrow arms it — a live database that never gets one has a frozen player
    /// nobody ever recovers.
    reaper_armed: Cell<bool>,
    /// How many times `freeze_live_entity` ran. The live-entity delete is the fence covering
    /// every targeting/aggro/threat/AOI gate on a real shard, and `live` alone cannot tell
    /// "there was nothing to freeze" from "the freeze was skipped".
    froze: Cell<u32>,
    active_taxi: RefCell<HashSet<u64>>,
}

/// The escrow out-row's columns, kept as their own struct so the fake never needs `TransferOut`
/// itself to be `Clone`. It mirrors every column now: `apply_begin` writes one and
/// `apply_confirm` copies the escrowed BLOB onto the attestation, so a fake that dropped the
/// blob would have made the six-step sequence untestable end to end.
#[derive(Clone, Default)]
pub struct XOut {
    pub character_guid: u64,
    pub dest_map_id: u32,
    pub dest_instance_id: u64,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_z: f32,
    pub dest_o: f32,
    pub blob: Vec<u8>,
    pub created_micros: i64,
    pub cross_database: bool,
}

// --- The transport arms, built on the REAL `move_rows`. ---

fn arm_gear(db: &FakeDb, guid: u64, io: &mut RowIo<'_>) {
    move_rows(
        db,
        io,
        || {
            db.gear
                .borrow()
                .iter()
                .filter(|r| r.owner == guid)
                .cloned()
                .collect::<Vec<_>>()
        },
        |db, r| db.gear.borrow_mut().push(r),
    );
}

fn arm_quests(db: &FakeDb, guid: u64, io: &mut RowIo<'_>) {
    move_rows(
        db,
        io,
        || {
            db.quests
                .borrow()
                .iter()
                .filter(|r| r.owner == guid)
                .cloned()
                .collect::<Vec<_>>()
        },
        |db, r| db.quests.borrow_mut().push(r),
    );
}

/// The allowlisted decline, running the REAL [`not_transported`].
fn arm_relay(db: &FakeDb, guid: u64, io: &mut RowIo<'_>) {
    let _ = (db, guid);
    not_transported(io);
}

/// Transfer MACHINERY, present in the registry (it earns a delete sweep) and filtered out of
/// the export by `MANIFEST_EXCLUDE` — the same name and the same reason as production.
fn arm_machinery(db: &FakeDb, guid: u64, io: &mut RowIo<'_>) {
    move_rows(
        db,
        io,
        || {
            db.gear
                .borrow()
                .iter()
                .filter(|r| r.owner == guid)
                .cloned()
                .collect::<Vec<_>>()
        },
        |db, r| db.gear.borrow_mut().push(r),
    );
}

/// The harness's transport registry — the stand-in for `crate::CHARACTER_OWNED_TRANSFERS`.
pub const ARMS: &[TransportArm<'static, FakeDb>] = &[
    ("harness_gear", arm_gear),
    ("harness_quest", arm_quests),
    ("harness_relay", arm_relay),
    ("game_transfer_out", arm_machinery),
];

/// The tables the harness expects to see in a payload — [`ARMS`] minus `MANIFEST_EXCLUDE`.
const TRANSPORTED: &[&str] = &["harness_gear", "harness_quest", "harness_relay"];

const NOW: i64 = 1_700_000_000_000_000;

impl FakeDb {
    fn new() -> Self {
        let db = Self::default();
        db.now.set(NOW);
        db
    }

    /// A source database holding `guid` plus a NEIGHBOUR character whose rows must never
    /// travel — the guid filter is the only thing keeping them apart, and a mover handed the
    /// wrong guid (or `0`) is a real mutation that used to be invisible.
    fn populated(guid: u64) -> Self {
        let db = Self::new();
        db.chars
            .borrow_mut()
            .insert(guid, fixture_character(guid, "Ponytail"));
        db.gear.borrow_mut().extend([
            GearRow {
                owner: guid,
                slot: 0,
                item: 2361,
            },
            GearRow {
                owner: guid,
                slot: 4,
                item: 6098,
            },
            GearRow {
                owner: NEIGHBOUR,
                slot: 0,
                item: 25,
            },
        ]);
        db.quests.borrow_mut().extend([
            QuestRow {
                owner: guid,
                quest: 62,
                note: "kobold vermin".into(),
            },
            QuestRow {
                owner: NEIGHBOUR,
                quest: 3,
                note: "not yours".into(),
            },
        ]);
        db.relay.borrow_mut().push(RelayRow {
            owner: guid,
            blip: 9,
        });
        db
    }

    fn gear_of(&self, guid: u64) -> Vec<GearRow> {
        self.gear
            .borrow()
            .iter()
            .filter(|r| r.owner == guid)
            .cloned()
            .collect()
    }
    fn quests_of(&self, guid: u64) -> Vec<QuestRow> {
        self.quests
            .borrow()
            .iter()
            .filter(|r| r.owner == guid)
            .cloned()
            .collect()
    }
    fn has_in_row(&self, transfer_id: u64) -> bool {
        self.in_rows.borrow().contains_key(&transfer_id)
    }
    /// Read one field off the arrived character row without needing `Character: Clone`.
    fn with_char<T>(&self, guid: u64, f: impl FnOnce(&crate::character::Character) -> T) -> T {
        let chars = self.chars.borrow();
        let c = chars.get(&guid).expect("no character row");
        f(c)
    }

    /// `world::cascade_delete_character` — the character-owned DELETE sweep, one place so both
    /// sinks run the same teardown. It sweeps every table the character owns, and that
    /// deliberately includes the two whose sweeps make `apply_finish`'s ORDER load-bearing:
    /// `game_group_member` (whose real sweep is `remove_member`, i.e. DISBAND) and
    /// `game_character_shard` (the forwarding receipt).
    fn cascade(&self, guid: u64) {
        self.chars.borrow_mut().remove(&guid);
        self.gear.borrow_mut().retain(|r| r.owner != guid);
        self.quests.borrow_mut().retain(|r| r.owner != guid);
        self.relay.borrow_mut().retain(|r| r.owner != guid);
        self.shard_index.borrow_mut().remove(&guid);
        if let Some(group_id) = self.group_members.borrow_mut().remove(&guid) {
            // `sweep_delete_game_group_member` -> `remove_member`: the party the deleted
            // character belonged to is torn down around it.
            self.disbanded.borrow_mut().insert(group_id);
        }
    }

    fn escrow(&self, transfer_id: u64, row: XOut) {
        self.out_rows.borrow_mut().insert(transfer_id, row);
    }
    fn receipt(&self, guid: u64) -> Option<(u32, u64)> {
        self.shard_index.borrow().get(&guid).copied()
    }
    fn settled(&self, transfer_id: u64) -> bool {
        !self.in_rows.borrow().contains_key(&transfer_id)
            && !self.out_rows.borrow().contains_key(&transfer_id)
    }
    fn party_of(&self, guid: u64) -> Option<u64> {
        self.group_members.borrow().get(&guid).copied()
    }
    fn is_disbanded(&self, group_id: u64) -> bool {
        self.disbanded.borrow().contains(&group_id)
    }
}

/// The escrow ledger, shared by every step. `TransferOut` is not `Clone`, so the fake keeps the
/// columns and rebuilds a row on demand — which also means `out_row` returns something the
/// production code can consume verbatim (blob included, so `apply_confirm` really does copy the
/// escrowed bytes onto the attestation).
impl ShardLedger for FakeDb {
    fn out_row(&self, transfer_id: u64) -> Option<TransferOut> {
        self.out_rows
            .borrow()
            .get(&transfer_id)
            .map(|o| TransferOut {
                transfer_id,
                character_guid: o.character_guid,
                dest_map_id: o.dest_map_id,
                dest_instance_id: o.dest_instance_id,
                dest_x: o.dest_x,
                dest_y: o.dest_y,
                dest_z: o.dest_z,
                dest_o: o.dest_o,
                blob: o.blob.clone(),
                created_micros: o.created_micros,
                cross_database: o.cross_database,
            })
    }
    fn in_row(&self, transfer_id: u64) -> Option<TransferIn> {
        self.in_rows
            .borrow()
            .get(&transfer_id)
            .map(|(guid, blob, created)| TransferIn {
                transfer_id,
                character_guid: *guid,
                blob: blob.clone(),
                created_micros: *created,
            })
    }
    fn file_out_row(&mut self, row: TransferOut) {
        self.out_rows.borrow_mut().insert(
            row.transfer_id,
            XOut {
                character_guid: row.character_guid,
                dest_map_id: row.dest_map_id,
                dest_instance_id: row.dest_instance_id,
                dest_x: row.dest_x,
                dest_y: row.dest_y,
                dest_z: row.dest_z,
                dest_o: row.dest_o,
                blob: row.blob,
                created_micros: row.created_micros,
                cross_database: row.cross_database,
            },
        );
    }
    fn file_in_row(&mut self, row: TransferIn) {
        self.in_rows.borrow_mut().insert(
            row.transfer_id,
            (row.character_guid, row.blob, row.created_micros),
        );
    }
    fn delete_out_row(&mut self, transfer_id: u64) {
        self.out_rows.borrow_mut().remove(&transfer_id);
    }
    fn delete_in_row(&mut self, transfer_id: u64) {
        self.in_rows.borrow_mut().remove(&transfer_id);
    }
    fn has_character(&self, guid: u64) -> bool {
        self.chars.borrow().contains_key(&guid)
    }
    fn now_micros(&self) -> i64 {
        self.now.get()
    }
}

/// The seam: `apply_begin` runs here for real — the freeze, the export, the blob build and
/// the escrow row, in order.
impl BeginSink for FakeDb {
    fn has_active_taxi_flight(&self, guid: u64) -> bool {
        self.active_taxi.borrow().contains(&guid)
    }
    fn is_in_transit(&self, guid: u64) -> bool {
        // The SAME predicate the production sink reaches through `transfer::is_in_transit`:
        // either escrow row naming this character fences it.
        let has_out = self
            .out_rows
            .borrow()
            .values()
            .any(|o| o.character_guid == guid);
        let has_in = self.in_rows.borrow().values().any(|(g, _, _)| *g == guid);
        !login_allowed(has_out, has_in)
    }
    fn freeze_live_entity(&mut self, guid: u64) {
        // The fake has no separate live-entity state to persist BACK into the character row
        // (`FakeDb`'s character row is the only copy), so the observable half is the delete —
        // which is the half that matters: it is what makes the character invisible to every
        // targeting/aggro/threat/AOI gate on a real shard.
        self.live.borrow_mut().remove(&guid);
        self.froze.set(self.froze.get() + 1);
    }
    fn export_rows(&self, guid: u64) -> Vec<TableRows> {
        export_rows_via(self, guid, ARMS)
    }
    fn with_character<T>(
        &self,
        guid: u64,
        f: impl FnOnce(&crate::character::Character) -> T,
    ) -> Option<T> {
        self.chars.borrow().get(&guid).map(f)
    }
    fn arm_reaper(&mut self) {
        self.reaper_armed.set(true);
    }
}

#[test]
fn active_taxi_refuses_transfer_before_any_source_mutation() {
    let mut src = beginning(GUID);
    src.active_taxi.borrow_mut().insert(GUID);
    let before_live = src.live.borrow().clone();
    let before_chars = src.chars.borrow().keys().copied().collect::<HashSet<_>>();

    let error = apply_begin(&mut src, XFER, GUID, DEST, true).expect_err("taxi owns movement");

    assert_eq!(error, "PLAYER_IN_TAXI_FLIGHT");
    assert_eq!(*src.live.borrow(), before_live);
    assert_eq!(
        src.chars.borrow().keys().copied().collect::<HashSet<_>>(),
        before_chars
    );
    assert_eq!(src.froze.get(), 0);
    assert!(src.out_rows.borrow().is_empty());
    assert!(!src.reaper_armed.get());
}

/// The seam: `apply_finish` runs here for real, ordering included.
impl FinishSink for FakeDb {
    fn detach_for_transfer(&mut self, guid: u64) {
        // The RAW removal: membership goes, the party does not.
        self.group_members.borrow_mut().remove(&guid);
    }
    fn cascade_delete_character(&mut self, guid: u64) {
        self.cascade(guid);
    }
    fn record_shard(&mut self, guid: u64, map_id: u32, instance_id: u64) {
        self.shard_index
            .borrow_mut()
            .insert(guid, (map_id, instance_id));
    }
}

impl ReapSink for FakeDb {
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)> {
        let mut rows: Vec<(u64, u64, i64, bool)> = self
            .out_rows
            .borrow()
            .iter()
            .map(|(id, o)| (*id, o.character_guid, o.created_micros, o.cross_database))
            .collect();
        // A HashMap has no order and the real table's iteration order is not one the protocol
        // may depend on — sorting keeps a multi-escrow test's failure message readable without
        // implying an ordering guarantee.
        rows.sort_unstable();
        rows
    }
}

impl ImportSink for FakeDb {
    fn has_live_entity(&self, guid: u64) -> bool {
        self.live.borrow().contains(&guid)
    }
    fn cascade_delete_character(&mut self, guid: u64) {
        self.cascade(guid);
    }
    fn insert_character(&mut self, c: crate::character::Character) {
        if self.swallow_inserts.get() {
            return; // the destination accepted the call and materialised nothing
        }
        assert!(
            self.chars.borrow().get(&c.guid).is_none(),
            "insert on an occupied primary key — SpacetimeDB would PANIC the transaction here"
        );
        self.chars.borrow_mut().insert(c.guid, c);
    }
    fn import_rows(&mut self, guid: u64, payload: &[TableRows]) -> Result<(), String> {
        // The REAL loop, against the harness registry.
        import_rows_via(&*self, guid, payload, ARMS)
    }
    fn ensure_shadow_account(&mut self, account_id: u64) {
        self.accounts.borrow_mut().insert(account_id);
    }
    fn bump_guid_high_water(&mut self, guid: u64) {
        // Same never-lowers rule as the real `auth::bump_guid_high_water` — exercised directly
        // by `guid_high_water_never_moves_backwards` below.
        if guid > self.guid_high_water.get() {
            self.guid_high_water.set(guid);
        }
    }
    fn own_guid_range(&self) -> Option<(u64, u64)> {
        self.guid_range.get()
    }
}

const GUID: u64 = 4242;
const NEIGHBOUR: u64 = 4343;
const XFER: u64 = GUID; // the transfer id IS the character guid
                        // `o` is an arbitrary FACING (radians), not an approximation of π: the fixture only has to be a
                        // legal orientation that survives the escrow round-trip byte-for-byte.
#[allow(clippy::approx_constant)]
const DEST: Destination = Destination {
    map_id: 36,
    instance_id: 7,
    x: -16.0,
    y: 62.0,
    z: 12.5,
    o: 3.14,
};
/// The range a `dst` fixture installs in tests that need one — small and round
/// so a "foreign" guid reads as obviously outside it at a glance, deliberately not shaped
/// like a real shard's billion-wide slot so a failing assertion's numbers read as test
/// fixture, never as a live shard's actual range.
const LOCAL_RANGE: (u64, u64) = (0, 1_000_000);
/// A guid OUTSIDE `LOCAL_RANGE` — stands for a `lyracore-world-1`-born character (a
/// different slot entirely) crossing into a database that owns `LOCAL_RANGE`. This is the
/// live trigger: any Kalimdor-born character crossing into `lyracore`.
const FOREIGN_GUID: u64 = 5_000_000_000;

/// A fully populated character — every field set to something DISTINGUISHABLE, so a field that
/// fails to travel shows up as a value mismatch rather than as two zeroes comparing equal.
fn fixture_character(guid: u64, name: &str) -> crate::character::Character {
    crate::character::Character {
        guid,
        account_id: 77,
        owner_identity: spacetimedb::Identity::ZERO,
        name: name.to_string(),
        race: 1,
        class: 4,
        gender: 1,
        skin: 2,
        face: 3,
        hair_style: 4,
        hair_color: 5,
        facial_hair: 6,
        level: 23,
        xp: 12_345,
        next_level_xp: 40_000,
        map_id: 0,
        zone_id: 12,
        x: -9450.0,
        y: 40.0,
        z: 56.0,
        orientation: 0.5,
        first_login: false,
        online: true,
        money: 987_654,
        rested_xp: 4_200,
        last_logout_micros: 111,
        home_map: 0,
        home_zone: 12,
        home_x: -9464.0,
        home_y: 32.0,
        home_z: 56.0,
        played_total_secs: 98_765,
        session_start_micros: 222,
        health: 640,
        power: 310,
        respec_count: 2,
        death_expire_micros: 333,
        pending_instance_id: 0,
        gm_level: 1,
        pending_ghost: false,
        resting: true,
        rested_since_micros: 444,
        // 289: a GM mid-`.god`/`.speed` hopping shards — DISTINGUISHABLE from the column
        // defaults (false / 10000), so a carry field that fails to travel shows up as a value
        // mismatch rather than two defaults comparing equal.
        pending_godmode: true,
        pending_run_speed_mult_bp: 30_000,
        // Distinguishable from the column default so a slot count that fails to travel shows up.
        bank_bag_slots: 3,
    }
}

/// The EXPORT half, end to end: the real registry loop + the real blob builder, exactly as
/// `begin_transfer` composes them.
fn export(db: &FakeDb, guid: u64, transfer_id: u64, dest: Destination) -> ExportBlob {
    let payload = export_rows_via(db, guid, ARMS);
    db.with_char(guid, |c| {
        build_export_blob(transfer_id, c, dest, payload.clone())
    })
    .expect("the fixture character serializes")
}

fn wire(blob: &ExportBlob) -> Vec<u8> {
    spacetimedb::sats::bsatn::to_vec(blob).expect("blob serializes")
}

fn rows_named<'a>(blob: &'a ExportBlob, table: &str) -> &'a TableRows {
    blob.payload
        .iter()
        .find(|t| t.table == table)
        .unwrap_or_else(|| panic!("payload has no entry for {table}"))
}

// =======================================================================================
//  AC 1 — a populated character crosses whole, with the right VALUES
// =======================================================================================

#[test]
fn a_populated_character_crosses_a_database_with_every_row_and_value() {
    let src = FakeDb::populated(GUID);
    let blob = export(&src, GUID, XFER, DEST);

    // The payload is one entry per registry table MINUS the machinery.
    let names: Vec<&str> = blob.payload.iter().map(|t| t.table.as_str()).collect();
    assert_eq!(
        names, TRANSPORTED,
        "the export payload must be exactly the transported registry, in registry order — \
             `game_transfer_out` is MACHINERY and exporting the escrow inside its own blob is \
             nonsense (MANIFEST_EXCLUDE)"
    );
    assert!(
        !rows_named(&blob, "harness_gear").rows.is_empty(),
        "a table with rows must ship a NON-EMPTY payload — an empty one is indistinguishable \
             from 'this character owned nothing', and the source copy is deleted moments later"
    );
    assert!(
        rows_named(&blob, "harness_relay").rows.is_empty(),
        "a `not_transported` arm must export nothing"
    );

    let mut dst = FakeDb::new();
    apply_import_blob(&mut dst, XFER, wire(&blob)).expect("the import commits");

    // --- the character row, field by field ---
    dst.with_char(GUID, |c| {
        let want = fixture_character(GUID, "Ponytail");
        assert_eq!(c.name, want.name, "the NAME did not travel");
        assert_eq!(c.level, want.level, "the LEVEL did not travel");
        assert_eq!(c.xp, want.xp, "XP did not travel");
        assert_eq!(c.next_level_xp, want.next_level_xp);
        assert_eq!(c.account_id, want.account_id);
        assert_eq!(c.race, want.race);
        assert_eq!(c.class, want.class);
        assert_eq!(c.health, want.health, "HEALTH did not travel");
        assert_eq!(c.power, want.power, "POWER did not travel");
        assert_eq!(
            c.money, want.money,
            "MONEY did not travel (#30's DEFER residual)"
        );
        assert_eq!(c.rested_xp, want.rested_xp, "rested XP did not travel");
        assert_eq!(c.resting, want.resting);
        assert_eq!(c.rested_since_micros, want.rested_since_micros);
        assert_eq!(c.played_total_secs, want.played_total_secs);
        assert_eq!(c.gm_level, want.gm_level, "the GM level did not travel");
        // 289: `.tele valley` (map 0 -> map 1) IS this path — a cross-DATABASE hop. The GM
        // playtest carry columns must arrive with the character, or the destination's
        // `build_player_entity` rebuilds a mortal, 1x GM (the reported bug: killed repeatedly by
        // Durotar wildlife with nothing printed).
        assert_eq!(
            c.pending_godmode, want.pending_godmode,
            "GODMODE did not travel — the arriving GM rebuilds mortal (work-item 289)"
        );
        assert_eq!(
            c.pending_run_speed_mult_bp, want.pending_run_speed_mult_bp,
            "the `.speed` multiplier did not travel (work-item 289)"
        );
        assert_eq!(c.respec_count, want.respec_count);
        assert_eq!(
            c.bank_bag_slots, want.bank_bag_slots,
            "the purchased bank bag slots did not travel"
        );
        assert_eq!(c.home_map, want.home_map);
        assert_eq!(c.home_x, want.home_x);
        assert_eq!(c.zone_id, want.zone_id);
        // ...and the DESTINATION overwrites exactly the six positional fields, no more.
        assert_eq!(
            c.map_id, DEST.map_id,
            "the arrival must be on the destination MAP"
        );
        assert_eq!(c.pending_instance_id, DEST.instance_id);
        assert_eq!(
            (c.x, c.y, c.z, c.orientation),
            (DEST.x, DEST.y, DEST.z, DEST.o)
        );
    });

    // --- the owned rows, value by value ---
    assert_eq!(
        dst.gear_of(GUID),
        vec![
            GearRow {
                owner: GUID,
                slot: 0,
                item: 2361
            },
            GearRow {
                owner: GUID,
                slot: 4,
                item: 6098
            },
        ],
        "the character's GEAR must arrive with its values, in order — this is the assertion \
             that repointing an arm at `not_transported` (468 passed, 0 failed) had to defeat"
    );
    assert_eq!(
        dst.quests_of(GUID),
        vec![QuestRow {
            owner: GUID,
            quest: 62,
            note: "kobold vermin".into()
        }],
        "a SECOND table's rows must arrive too — one working arm is not a working transport"
    );
    assert!(
        dst.relay.borrow().is_empty(),
        "an allowlisted `not_transported` table must arrive EMPTY, not be re-materialised"
    );

    // --- the neighbour never travelled ---
    assert!(
        dst.gear_of(NEIGHBOUR).is_empty() && dst.quests_of(NEIGHBOUR).is_empty(),
        "another character's rows rode along — the export mover was handed the wrong guid, \
             which on a real shard hands a stranger's inventory to the arriving player"
    );

    // --- the ledger + the shadow account ---
    assert!(
        dst.has_in_row(XFER),
        "the in-row must be filed once the copy is durable"
    );
    assert_eq!(
        dst.in_row(XFER).unwrap().character_guid,
        GUID,
        "the in-row must name the character it licensed the deletion of"
    );
    assert_eq!(dst.in_row(XFER).unwrap().created_micros, NOW);
    assert!(
        dst.accounts.borrow().contains(&77),
        "no shadow `game_account` row — `gw::gw_player_login` resolves the account by id, \
             so the arriving player cannot log in at all"
    );
}

// =======================================================================================
//  AC 2 — export -> import -> export is a fixed point
// =======================================================================================

#[test]
fn export_import_export_produces_an_identical_blob() {
    // The hop RELOCATES the character, so blob 1 and blob 2 differ in exactly the position
    // fields by design. Hopping to where the character already IS makes the transport a fixed
    // point, which is the honest way to state "nothing else changed" as an equality.
    let src = FakeDb::populated(GUID);
    let here = src.with_char(GUID, |c| Destination {
        map_id: c.map_id,
        instance_id: c.pending_instance_id,
        x: c.x,
        y: c.y,
        z: c.z,
        o: c.orientation,
    });

    let first = export(&src, GUID, XFER, here);
    let mut dst = FakeDb::new();
    apply_import_blob(&mut dst, XFER, wire(&first)).expect("hop 1 commits");
    let second = export(&dst, GUID, XFER, here);
    assert_eq!(
        first, second,
        "export -> import -> export lost or changed something"
    );
    assert_eq!(
        wire(&first),
        wire(&second),
        "the blobs compare equal but do not serialize equal — a f32 NaN or a field the \
             PartialEq skips"
    );

    // And once more, so the property is 'stable', not merely 'survives one hop'.
    let mut third_db = FakeDb::new();
    apply_import_blob(&mut third_db, XFER, wire(&second)).expect("hop 2 commits");
    assert_eq!(export(&third_db, GUID, XFER, here), first);

    // A hop to a DIFFERENT destination differs in the six positional fields and NOTHING else.
    // Those six live on the CHARACTER ROW now (deleted the blob's duplicate copies of
    // them), so the assertion reads them off the arrived row rather than off the blob header.
    let moved = export(&dst, GUID, XFER, DEST);
    let mut far = FakeDb::new();
    apply_import_blob(&mut far, XFER, wire(&moved)).expect("hop 3 commits");
    let after = export(&far, GUID, XFER, DEST);
    assert_eq!(
        after.payload, first.payload,
        "the ROWS must survive a relocating hop unchanged"
    );
    assert_eq!(after.money, first.money);
    far.with_char(GUID, |c| {
        let want = fixture_character(GUID, "Ponytail");
        assert_eq!(
            c.health, want.health,
            "HEALTH must survive a relocating hop"
        );
        assert_eq!(
            c.level, want.level,
            "the LEVEL must survive a relocating hop"
        );
        assert_eq!(
            (c.map_id, c.x, c.y, c.z, c.orientation),
            (DEST.map_id, DEST.x, DEST.y, DEST.z, DEST.o),
            "the relocation must land on the destination"
        );
    });
}

// =======================================================================================
//  Importing a character ratchets the destination's guid allocator
// =======================================================================================

/// AC#3: an imported character bumps the destination's guid high-water mark, so a
/// `create_character` on THIS database afterward can never hand out the same guid. This is
/// the LOCAL-range half of the fix — `GUID` sits inside `dst`'s own `LOCAL_RANGE`,
/// so the new gate must not regress this property: importing a character that belongs HERE
/// still ratchets exactly as before.
#[test]
fn importing_a_character_bumps_the_destinations_guid_high_water() {
    let src = FakeDb::populated(GUID);
    let blob = export(&src, GUID, XFER, DEST);

    let mut dst = FakeDb::new();
    dst.guid_range.set(Some(LOCAL_RANGE)); // GUID (4242) is inside [0, 1_000_000)
    assert_eq!(
        dst.guid_high_water.get(),
        0,
        "a fresh database starts unseeded"
    );
    apply_import_blob(&mut dst, XFER, wire(&blob)).expect("the import commits");
    assert_eq!(
        dst.guid_high_water.get(),
        GUID,
        "a LOCAL-range arrival must still ratchet the destination's mark"
    );
}

/// The mark never moves backwards — importing a LOWER guid after a higher one is already
/// tracked must not pull the mark down (it would re-open the higher guid to reuse). `dst`
/// installs `LOCAL_RANGE` so `GUID`'s bump attempt actually reaches `bump_guid_high_water`
/// (and therefore its never-lowers rule) rather than being skipped by the gate for an
/// unrelated reason — the two properties are independent and this test must keep exercising
/// the never-lowers one specifically.
#[test]
fn a_lower_imported_guid_does_not_pull_the_high_water_mark_down() {
    let mut dst = FakeDb::new();
    dst.guid_range.set(Some(LOCAL_RANGE));
    dst.bump_guid_high_water(9000);
    let src = FakeDb::populated(GUID); // GUID (4242) < 9000, still inside LOCAL_RANGE
    let blob = export(&src, GUID, XFER, DEST);
    apply_import_blob(&mut dst, XFER, wire(&blob)).expect("the import commits");
    assert_eq!(
        dst.guid_high_water.get(),
        9000,
        "a lower arrival must not lower the mark"
    );
}

/// **The regression fixed here.** An arrival whose guid belongs to ANOTHER
/// shard's range must leave THIS database's allocator untouched — ranges are disjoint by
/// construction, so `FOREIGN_GUID` can never collide
/// with anything `dst` mints, and ratcheting past it anyway is exactly what pushed
/// `lyracore`'s real high-water mark to its own range end with zero local characters
/// above it — every subsequent local `create_character` then failed
/// `GUID_RANGE_EXHAUSTED`. The import itself must still SUCCEED (a foreign arrival is not an
/// error — only the allocator bump is skipped).
#[test]
fn importing_a_foreign_range_guid_leaves_the_destinations_allocator_untouched() {
    let src = FakeDb::populated(FOREIGN_GUID);
    let blob = export(&src, FOREIGN_GUID, FOREIGN_GUID, DEST); // transfer_id IS the guid

    let mut dst = FakeDb::new();
    dst.guid_range.set(Some(LOCAL_RANGE)); // FOREIGN_GUID is nowhere near [0, 1_000_000)
    apply_import_blob(&mut dst, FOREIGN_GUID, wire(&blob))
        .expect("a foreign-range arrival still materialises here");
    assert_eq!(
        dst.guid_high_water.get(),
        0,
        "a foreign-range arrival must leave this database's own allocator untouched"
    );
    assert!(
        dst.has_character(FOREIGN_GUID),
        "the character itself must still land"
    );
}

// =======================================================================================
//  The guards — each one EXECUTED, and each one a mutation that used to survive
// =======================================================================================

#[test]
fn an_import_that_cannot_place_a_table_files_no_in_row() {
    // The mutation: `import_rows(..)?` -> `let _ = import_rows(..)`. A partial character is
    // committed and the in-row filed anyway, which licenses cascade-deleting the source copy
    // the missing rows came from.
    let src = FakeDb::populated(GUID);
    let mut blob = export(&src, GUID, XFER, DEST);
    blob.payload.push(TableRows {
        table: "harness_from_the_future".into(),
        rows: vec![1, 2, 3],
    });

    let mut dst = FakeDb::new();
    let err = apply_import_blob(&mut dst, XFER, wire(&blob))
        .expect_err("an unplaceable table must ABORT the import");
    assert!(err.contains("refusing a partial import"), "{err}");
    assert!(
        !dst.has_in_row(XFER),
        "an in-row was filed for an import that could not place a table — that row is what \
             licenses finish_transfer to destroy the source copy"
    );
}

/// The inverse of the test above and the one the drift contract forgot: a table the
/// registry expects that the PAYLOAD does not carry. This used to import with a clean `Ok(())`,
/// file the in-row, and license `finish_transfer` to destroy the complete source copy of a
/// character that had arrived without that table.
#[test]
fn a_payload_missing_a_manifest_table_is_refused_and_files_no_in_row() {
    // EVERY transported table, one at a time — including the `not_transported` one, whose entry
    // is empty but still required: the blob protocol emits it, so its absence means the payload
    // was not built by this protocol and nothing about it can be trusted.
    for absent in TRANSPORTED {
        let src = FakeDb::populated(GUID);
        let mut blob = export(&src, GUID, XFER, DEST);
        blob.payload.retain(|t| t.table != *absent);
        assert_eq!(
            blob.payload.len(),
            TRANSPORTED.len() - 1,
            "the fixture removed one table"
        );

        let mut dst = FakeDb::new();
        let err = apply_import_blob(&mut dst, XFER, wire(&blob))
            .expect_err("a payload missing a manifest table must ABORT the import");
        assert!(
            err.contains(absent) && err.contains("refusing a partial import"),
            "the refusal must NAME the missing table: {err}"
        );
        assert!(
            !dst.has_in_row(XFER),
            "an in-row was filed for a payload missing {absent} — that row is what licenses \
                 finish_transfer to destroy the COMPLETE source copy, so the character would be \
                 left with only the partial one (issue #42)"
        );
    }

    // And several missing at once: every name in the refusal, so an operator reading the log
    // learns the whole gap rather than the first table alphabetically.
    let src = FakeDb::populated(GUID);
    let mut blob = export(&src, GUID, XFER, DEST);
    blob.payload.clear();
    let mut dst = FakeDb::new();
    let err = apply_import_blob(&mut dst, XFER, wire(&blob))
        .expect_err("an EMPTY payload is the extreme case of the same defect");
    for table in TRANSPORTED {
        assert!(err.contains(table), "the refusal must name {table}: {err}");
    }
    assert!(!dst.has_in_row(XFER));
}

#[test]
fn an_undecodable_table_payload_aborts_the_import() {
    let src = FakeDb::populated(GUID);
    let mut blob = export(&src, GUID, XFER, DEST);
    blob.payload
        .iter_mut()
        .find(|t| t.table == "harness_gear")
        .expect("gear entry")
        .rows = vec![0xff; 8];

    let mut dst = FakeDb::new();
    let err = apply_import_blob(&mut dst, XFER, wire(&blob))
        .expect_err("garbage rows must abort the import");
    assert!(
        err.contains("harness_gear") && err.contains("deserialize"),
        "{err}"
    );
    assert!(
        !dst.has_in_row(XFER),
        "no in-row may be filed for an aborted import"
    );
}

#[test]
fn a_manifest_from_another_build_is_refused() {
    // the stated contract. Deleting this guard left 468 tests green.
    let src = FakeDb::populated(GUID);
    let mut blob = export(&src, GUID, XFER, DEST);
    blob.manifest.push(ManifestEntry {
        table: "game_from_the_future".into(),
        hot: false,
    });

    let mut dst = FakeDb::new();
    let err = apply_import_blob(&mut dst, XFER, wire(&blob))
        .expect_err("a drifted manifest must be refused");
    assert!(err.contains("manifest mismatch"), "{err}");
    assert!(!dst.has_in_row(XFER));
    assert!(
        !dst.has_character(GUID),
        "nothing may be materialised from a drifted manifest"
    );
}

#[test]
fn a_blob_filed_under_a_different_transfer_id_is_refused() {
    let src = FakeDb::populated(GUID);
    let blob = export(&src, GUID, XFER, DEST);
    let mut dst = FakeDb::new();
    let err = apply_import_blob(&mut dst, XFER + 1, wire(&blob))
        .expect_err("the blob names another transfer");
    assert!(err.contains("blob names transfer"), "{err}");
    assert!(!dst.has_in_row(XFER + 1));
}

#[test]
fn a_character_row_that_disagrees_with_the_blob_is_refused() {
    let src = FakeDb::populated(GUID);
    let mut blob = export(&src, GUID, XFER, DEST);
    // The serialized row says GUID; the blob's header claims someone else. Importing would
    // materialise a row under a guid nobody escrowed.
    blob.character_guid = NEIGHBOUR;
    let mut dst = FakeDb::new();
    let err = apply_import_blob(&mut dst, XFER, wire(&blob))
        .expect_err("the arriving row must match the blob's guid");
    assert!(err.contains("arriving character row is guid"), "{err}");
    assert!(!dst.has_character(NEIGHBOUR) && !dst.has_in_row(XFER));
}

#[test]
fn transfer_id_zero_is_reserved() {
    let src = FakeDb::populated(GUID);
    let mut blob = export(&src, GUID, 0, DEST);
    blob.transfer_id = 0;
    let mut dst = FakeDb::new();
    assert!(apply_import_blob(&mut dst, 0, wire(&blob)).is_err());
    assert!(!dst.has_character(GUID));
}

#[test]
fn a_replayed_import_is_a_no_op_and_never_duplicates_a_row() {
    let src = FakeDb::populated(GUID);
    let blob = export(&src, GUID, XFER, DEST);
    let mut dst = FakeDb::new();
    apply_import_blob(&mut dst, XFER, wire(&blob)).expect("first import");
    let before = (dst.gear_of(GUID), dst.quests_of(GUID));

    apply_import_blob(&mut dst, XFER, wire(&blob)).expect("a replay must answer Ok");
    assert_eq!(
        (dst.gear_of(GUID), dst.quests_of(GUID)),
        before,
        "a replayed import duplicated the character's rows — the driver that crashed without \
             learning whether its call landed simply calls again, so this is the COMMON path"
    );
}

#[test]
fn a_reused_transfer_id_is_refused_at_the_destination() {
    // The destination twin of `BeginPlan::IdCollision`: answering Ok here would let the driver
    // go on to finish (i.e. DELETE) a source copy whose destination copy does not exist.
    let src = FakeDb::populated(GUID);
    let mut dst = FakeDb::new();
    apply_import_blob(&mut dst, XFER, wire(&export(&src, GUID, XFER, DEST))).expect("import");

    let other = FakeDb::populated(NEIGHBOUR);
    let stranger = export(&other, NEIGHBOUR, XFER, DEST);
    let err = apply_import_blob(&mut dst, XFER, wire(&stranger))
        .expect_err("the id is already imported for a DIFFERENT character");
    assert!(err.contains("refusing to reuse"), "{err}");
    assert!(!dst.has_character(NEIGHBOUR));
}

#[test]
fn importing_on_top_of_a_live_character_is_refused() {
    let src = FakeDb::populated(GUID);
    let blob = export(&src, GUID, XFER, DEST);
    let mut dst = FakeDb::new();
    dst.live.borrow_mut().insert(GUID);
    let err = apply_import_blob(&mut dst, XFER, wire(&blob))
        .expect_err("the character is already LIVE here — the dual-liveness dupe");
    assert!(err.contains("already has a LIVE entity"), "{err}");
    assert!(!dst.has_in_row(XFER));
}

#[test]
fn a_stale_copy_from_an_earlier_hop_is_wiped_before_the_arrival_lands() {
    // The destination still holds a half-finished hop in the OTHER direction. Its owned rows
    // must be cascaded away, or the arriving loadout lands on top of an older one.
    let src = FakeDb::populated(GUID);
    let blob = export(&src, GUID, XFER, DEST);

    let mut dst = FakeDb::new();
    dst.chars
        .borrow_mut()
        .insert(GUID, fixture_character(GUID, "Ponytail"));
    dst.gear.borrow_mut().push(GearRow {
        owner: GUID,
        slot: 9,
        item: 1,
    }); // the stale loadout
    apply_import_blob(&mut dst, XFER, wire(&blob)).expect("the import commits over the stale copy");

    assert_eq!(
        dst.gear_of(GUID),
        vec![
            GearRow {
                owner: GUID,
                slot: 0,
                item: 2361
            },
            GearRow {
                owner: GUID,
                slot: 4,
                item: 6098
            },
        ],
        "the stale loadout survived alongside the arriving one — two loadouts on one character"
    );
    assert!(dst.has_in_row(XFER));
}

/// The wipe above only fired when `has_character(guid)` was true, so an ORPHANED
/// owned row (this guid's OWN item/quest/etc. left behind with no accompanying `game_character`
/// row and no `game_transfer_in` witness — the residual state a table-level witness cannot see,
/// since it is a fact about a DIFFERENT table) survived untouched and a fresh import landed
/// its rows on top. Item rows are guid-namespaced to the OWNER (`item_guid_for`), so this can
/// only ever collide with the SAME character's own leftover, never a stranger's — real
/// SpacetimeDB would PANIC on the duplicate primary key exactly as `import_character_blob`'s
/// crash trace showed on `game_item_instance`. The fake never panics on a duplicate push, so
/// this is pinned on the resulting ROW COUNT instead: a second copy is exactly as wrong as a
/// panic, just silent here.
///
/// Mutation target: re-add `if sink.has_character(guid) { .. }` around the cascade call in
/// `apply_import_blob` and this goes red (the orphaned gear row survives alongside the fresh
/// pair below).
#[test]
fn orphaned_owned_rows_with_no_character_row_are_wiped_before_a_fresh_import_lands() {
    let src = FakeDb::populated(GUID);
    let blob = export(&src, GUID, XFER, DEST);

    let mut dst = FakeDb::new();
    // No `chars` entry and no `in_rows` entry for XFER — the exact combination a transfer-id
    // witness cannot rule out, since it only ever answers for ITS OWN table.
    dst.gear.borrow_mut().push(GearRow {
        owner: GUID,
        slot: 9,
        item: 1,
    }); // orphaned leftover
    apply_import_blob(&mut dst, XFER, wire(&blob))
        .expect("importing over an orphaned owned row must not panic or refuse");

    assert_eq!(
        dst.gear_of(GUID),
        vec![
            GearRow {
                owner: GUID,
                slot: 0,
                item: 2361
            },
            GearRow {
                owner: GUID,
                slot: 4,
                item: 6098
            },
        ],
        "the orphaned leftover survived alongside the arriving loadout — a real destination \
             would have PANICKED inserting a duplicate game_item_instance primary key here"
    );
}

#[test]
fn money_folded_into_the_escrow_after_the_freeze_arrives_with_the_character() {
    // the DEFER verdict, end to end: `credit_purse` on an in-transit character folds copper
    // into the escrowed blob, and `apply_import_blob`'s `c.money = decoded.money` is the ONLY
    // thing that replays it at the destination. Deleting that line left 468 tests green.
    let src = FakeDb::populated(GUID);
    let blob = export(&src, GUID, XFER, DEST);
    let base = blob.money;
    let folded = fold_money_delta(&wire(&blob), 1_500).expect("the fold rewrites the blob");

    let mut dst = FakeDb::new();
    apply_import_blob(&mut dst, XFER, folded).expect("import");
    dst.with_char(GUID, |c| {
        assert_eq!(
            c.money,
            base + 1_500,
            "the deferred copper died with the source copy — a party member's loot share, \
                 silently dropped"
        );
    });
}

#[test]
fn a_character_with_nothing_still_crosses() {
    // The empty-payload case is NOT an error: `decode_rows` reads an empty payload as "this
    // table had no rows", and a fresh level-1 character legitimately owns almost nothing.
    let src = FakeDb::new();
    src.chars
        .borrow_mut()
        .insert(GUID, fixture_character(GUID, "Ponytail"));
    let blob = export(&src, GUID, XFER, DEST);
    assert!(decoded::<GearRow>(&blob.payload, "harness_gear").is_empty());
    assert!(decoded::<QuestRow>(&blob.payload, "harness_quest").is_empty());

    let mut dst = FakeDb::new();
    apply_import_blob(&mut dst, XFER, wire(&blob)).expect("an empty character still crosses");
    assert!(dst.has_character(GUID) && dst.has_in_row(XFER));
    assert!(dst.gear_of(GUID).is_empty());
}

/// The post-import durability PROOF, EXECUTED. `Apply` ⇒ the destination copy is durable is a
/// claim the model makes; this is the only place it is checked against a destination that
/// accepted the insert and materialised nothing. Deleting the proof left 468 tests green,
/// and it stayed scan-only through the first cut of this harness — the scan is defeated by
/// leaving its own message in a dead branch, which is how it was found.
#[test]
fn an_arrival_that_never_materialised_files_no_in_row() {
    let src = FakeDb::populated(GUID);
    let blob = export(&src, GUID, XFER, DEST);

    let mut dst = FakeDb::new();
    dst.swallow_inserts.set(true);
    let err = apply_import_blob(&mut dst, XFER, wire(&blob))
        .expect_err("an import that materialised no character row must REFUSE");
    assert!(err.contains("no durable row at the destination"), "{err}");
    assert!(
        !dst.has_in_row(XFER),
        "an in-row was filed against an import that materialised nothing. That row is what \
             licenses `confirm_import` and then `finish_transfer` to cascade-delete the SOURCE \
             copy, so filing it here settles the transfer with ZERO durable copies — the one \
             unrecoverable outcome in the whole protocol."
    );
}

#[test]
fn a_corrupt_blob_is_refused_rather_than_half_applied() {
    let mut dst = FakeDb::new();
    let err = apply_import_blob(&mut dst, XFER, vec![0xff; 16]).expect_err("garbage");
    assert!(err.contains("corrupt export blob"), "{err}");
    assert!(!dst.has_in_row(XFER) && dst.chars.borrow().is_empty());
}

/// The export loop's OWN guards, driven directly: the guid it hands each mover, and the
/// machinery filter. (`export_rows` passing guid `0` to every mover was a survivor.)
#[test]
fn the_export_loop_hands_each_mover_the_transferring_guid() {
    let src = FakeDb::populated(GUID);
    let mine = export_rows_via(&src, GUID, ARMS);
    let theirs = export_rows_via(&src, NEIGHBOUR, ARMS);
    let nobody = export_rows_via(&src, 0, ARMS);

    assert_ne!(
        rows_named2(&mine, "harness_gear"),
        rows_named2(&theirs, "harness_gear"),
        "two different characters exported the same gear — the mover is not being handed the \
             guid it was asked for"
    );
    assert!(
        decoded::<GearRow>(&nobody, "harness_gear").is_empty(),
        "guid 0 owns nothing, so it must export nothing — `export_rows` handing every mover a \
             hardcoded 0 was a #36 mutation that left the whole suite green"
    );
    assert!(
        !mine.iter().any(|t| t.table == "game_transfer_out"),
        "MANIFEST_EXCLUDE stopped filtering the escrow out of its own export blob"
    );
}

fn rows_named2<'a>(payload: &'a [TableRows], table: &str) -> &'a [u8] {
    &payload
        .iter()
        .find(|t| t.table == table)
        .expect("entry")
        .rows
}

/// Decode a payload entry back into rows. NOTE: bsatn of an EMPTY `Vec<R>` is a 4-byte length
/// prefix, not zero bytes — "no rows" is only literally empty for a `not_transported` arm — so
/// "did this table ship anything?" has to be asked of the DECODED rows.
fn decoded<R>(payload: &[TableRows], table: &str) -> Vec<R>
where
    R: spacetimedb::SpacetimeType + for<'de> spacetimedb::sats::Deserialize<'de>,
{
    let mut outcome = Ok(());
    let rows = decode_rows::<R>(rows_named2(payload, table), &mut outcome);
    outcome.expect("the harness payload decodes");
    rows
}

/// The import loop applies rows to the guid it was ASKED for, not the one the rows claim —
/// there is no re-owning step in the transport, so a mis-addressed import is visible.
#[test]
fn the_import_loop_refuses_a_table_this_build_has_no_arm_for() {
    let db = FakeDb::new();
    let err = import_rows_via(
        &db,
        GUID,
        &[TableRows {
            table: "game_not_here".into(),
            rows: vec![],
        }],
        ARMS,
    )
    .expect_err("an unknown table must abort");
    assert!(err.contains("refusing a partial import"), "{err}");
}

// -----------------------------------------------------------------------------------------
//  `apply_finish` — the delete-last body, executed.
//
//  These four replace the `fn do_finish(` source scan that used to stand in for them. Each
//  states the live consequence of the single line it pins; deleting that line turns exactly
//  this test red.
// -----------------------------------------------------------------------------------------

const XDEST: (u32, u64) = (36, 7);

/// A source database holding `guid` mid-transfer: its rows, its escrow out-row, and a
/// forwarding receipt still naming where it USED to be.
fn finishing(guid: u64, cross_database: bool) -> FakeDb {
    let db = FakeDb::populated(guid);
    db.escrow(
        guid,
        XOut {
            character_guid: guid,
            dest_map_id: XDEST.0,
            dest_instance_id: XDEST.1,
            cross_database,
            ..XOut::default()
        },
    );
    db.shard_index.borrow_mut().insert(guid, (0, 0));
    db
}

#[test]
fn finish_destroys_the_source_copy_for_a_cross_database_transfer() {
    let mut db = finishing(GUID, true);
    apply_finish(&mut db, XFER);
    assert!(
        !db.has_character(GUID),
        "the source copy must be destroyed (delete-last)"
    );
    assert!(
        db.gear_of(GUID).is_empty() && db.quests_of(GUID).is_empty(),
        "the cascade must take the character's OWNED rows with it — a half-swept source leaves \
             orphan gear and quest rows under a guid that will be reissued"
    );
    assert!(
        !db.gear_of(NEIGHBOUR).is_empty(),
        "the cascade swept a character it was not given — the guid filter is gone"
    );
    assert!(
        db.settled(XFER),
        "both escrow rows must be gone: the out-row is the source's CLAIM, and leaving it \
             behind freezes the character out of every future transfer AND of its own login"
    );
}

#[test]
fn a_same_database_finish_never_cascades_the_shared_character_row() {
    let mut db = finishing(GUID, false);
    apply_finish(&mut db, XFER);
    assert!(
        db.has_character(GUID),
        "same-database the two partitions SHARE `game_character`, so an unconditional cascade \
             destroys the DESTINATION copy too — the escrow's `cross_database` gate is what stops \
             `finish_transfer` from deleting the character it just moved"
    );
    assert!(
        db.settled(XFER),
        "the escrow still clears on a same-database finish"
    );
}

/// AC#4: a shard hop is not a departure.
#[test]
fn finish_detaches_the_party_before_the_cascade_so_the_party_survives_the_hop() {
    let mut db = finishing(GUID, true);
    db.group_members.borrow_mut().insert(GUID, 5);
    db.group_members.borrow_mut().insert(NEIGHBOUR, 5);

    apply_finish(&mut db, XFER);

    assert!(
        !db.is_disbanded(5),
        "the party was DISBANDED by the hop. `apply_finish` must call `detach_for_transfer` \
             BEFORE the cascade: the cascade's `game_group_member` sweep is `remove_member`, which \
             fires DESTROYED at everyone left behind and tears down a two-person party the instant \
             its first member steps through the portal — so the second member's export blob would \
             carry no membership at all"
    );
    assert_eq!(
        db.party_of(NEIGHBOUR),
        Some(5),
        "the members left behind must keep the SAME group_id, or the party cannot re-form at \
             the destination"
    );
    assert_eq!(
        db.party_of(GUID),
        None,
        "the hopping member's own membership does travel"
    );
}

/// The merge order, as a behaviour rather than a comment.
#[test]
fn the_forwarding_receipt_survives_the_cascade_that_would_sweep_it() {
    let mut db = finishing(GUID, true);
    apply_finish(&mut db, XFER);
    assert_eq!(
        db.receipt(GUID),
        Some(XDEST),
        "the source shard kept no forwarding receipt naming the DESTINATION. Two mutations \
             land here and both are silent live: dropping `record_shard` (the escrow settles and \
             the directory never learns where), and writing it BEFORE the cascade (which sweeps \
             `game_character_shard` — it is character-owned — and wipes the receipt that had just \
             been written). A gateway whose realm-core is unconfigured has nothing but this row to \
             find a character that moved off the shard it is asking."
    );
}

/// `not_transported` is not a synonym for "arm missing": it must actively CLEAR the export
/// buffer, or a re-used buffer would ship the previous table's bytes under this table's name.
#[test]
fn not_transported_exports_nothing_and_absorbs_anything() {
    let mut buf = vec![1, 2, 3];
    not_transported(&mut RowIo::Export(&mut buf));
    assert!(
        buf.is_empty(),
        "a declining arm must leave an EMPTY payload"
    );

    let mut outcome = Ok(());
    not_transported(&mut RowIo::Import(&[0xff, 0xff], &mut outcome));
    assert!(
        outcome.is_ok(),
        "a declining arm must not fail an import it ignores"
    );
}

// -----------------------------------------------------------------------------------------
//  `apply_begin` — step 1, EXECUTED.
//
//  Everything below used to be three `.contains()` scans of `begin_transfer`'s 120-line body
//  ("it deletes the live entity row", "it calls export_rows", "it calls build_export_blob").
//  A scan cannot tell whether the escrow it wrote is one the destination can actually read.
// -----------------------------------------------------------------------------------------

/// A source database with the character LIVE on it — the normal shape of a hop: the player is
/// standing in the world when the portal fires.
fn beginning(guid: u64) -> FakeDb {
    let db = FakeDb::populated(guid);
    db.live.borrow_mut().insert(guid);
    db
}

#[test]
fn begin_freezes_the_live_entity_and_escrows_a_blob_the_destination_can_read() {
    let mut src = beginning(GUID);
    apply_begin(&mut src, XFER, GUID, DEST, true).expect("the escrow commits");

    assert_eq!(
        src.froze.get(),
        1,
        "begin did not freeze the live entity. That single delete of `game_world_entity` is \
             what makes the character invisible to the ~50 map_id/instance_id target gates, the \
             aggro candidate scan, the threat lists and the AOI relay — none of which consult the \
             ledger. Without it the in-transit fence holds only for the ACTOR side."
    );
    assert!(
        !src.has_live_entity(GUID),
        "the live entity survived the freeze — the character is still actable on the shard it \
             is leaving"
    );
    assert!(
        src.has_character(GUID),
        "begin destroyed the source's DURABLE copy. Nothing may do that but finish_transfer, \
             and only after the destination copy is committed (delete-last)"
    );
    assert!(
        src.reaper_armed.get(),
        "the first escrow did not arm the reaper. `seed::init` only runs on a FRESH database, \
             so this lazy arming is the only thing that ever schedules recovery on a live one — \
             without it an abandoned transfer is a frozen player nobody comes back for"
    );

    let out = ShardLedger::out_row(&src, XFER).expect("the escrow row is filed");
    assert_eq!(out.character_guid, GUID);
    assert_eq!(
        (out.dest_map_id, out.dest_instance_id),
        (DEST.map_id, DEST.instance_id)
    );
    assert!(out.cross_database, "the cross_database flag must travel");

    // The blob is the whole point: it has to be one the DESTINATION accepts and materialise the
    // character with its rows. Anything less — an empty payload, a guid-0 export, a blob built
    // by hand instead of by `build_export_blob` — passes a scan and loses a character.
    let mut dst = FakeDb::new();
    apply_import_blob(&mut dst, XFER, out.blob).expect("the escrowed blob imports");
    assert!(dst.has_character(GUID));
    assert_eq!(
        dst.gear_of(GUID),
        src.gear_of(GUID),
        "the escrowed blob did not carry the character's rows"
    );
}

#[test]
fn a_replayed_begin_is_a_no_op_and_never_re_escrows() {
    let mut src = beginning(GUID);
    apply_begin(&mut src, XFER, GUID, DEST, true).expect("first begin");
    let first = ShardLedger::out_row(&src, XFER).expect("escrowed").blob;

    apply_begin(&mut src, XFER, GUID, DEST, true).expect("a replay must answer Ok");
    assert_eq!(
        ShardLedger::out_row(&src, XFER)
            .expect("still escrowed")
            .blob,
        first,
        "a replayed begin rebuilt the blob. The driver that crashed without learning whether \
             its call landed simply calls again, so this is the COMMON path — and re-exporting \
             would snapshot a character that has been frozen since the first call"
    );
    assert_eq!(
        src.froze.get(),
        1,
        "a replayed begin re-ran the freeze — harmless here, but it means the replay took the \
             Escrow arm rather than the Replay one"
    );
}

#[test]
fn begin_refuses_a_transfer_id_already_escrowed_for_a_different_character() {
    let mut src = beginning(GUID);
    src.chars
        .borrow_mut()
        .insert(NEIGHBOUR, fixture_character(NEIGHBOUR, "Stranger"));
    apply_begin(&mut src, XFER, GUID, DEST, true).expect("first begin");

    let err = apply_begin(&mut src, XFER, NEIGHBOUR, DEST, true)
        .expect_err("the id is already escrowed for someone else");
    assert!(err.contains("refusing to reuse the"), "{err}");
    assert_eq!(
        ShardLedger::out_row(&src, XFER)
            .expect("still escrowed")
            .character_guid,
        GUID,
        "the second call overwrote the first character's escrow — the driver would then drive \
             import/finish on the id and move the OTHER character, reporting success for one that \
             never moved"
    );
}

#[test]
fn begin_refuses_a_character_that_is_already_in_transit_under_another_id() {
    let mut src = beginning(GUID);
    apply_begin(&mut src, XFER, GUID, DEST, true).expect("first begin");
    let err = apply_begin(&mut src, XFER + 1, GUID, DEST, true)
        .expect_err("the character already holds an escrow");
    assert!(err.contains("already in transit"), "{err}");
    assert!(
        ShardLedger::out_row(&src, XFER + 1).is_none(),
        "a character escrowed twice has two destinations each holding a claim on it; \
             cross-database BOTH import and the character is DUPLICATED"
    );
}

#[test]
fn begin_refuses_a_character_with_no_durable_row_and_a_reserved_id() {
    let mut src = FakeDb::new();
    let err = apply_begin(&mut src, XFER, GUID, DEST, true).expect_err("nothing to freeze");
    assert!(err.contains("no such character"), "{err}");
    assert!(ShardLedger::out_row(&src, XFER).is_none());

    let mut src = beginning(GUID);
    let err = apply_begin(&mut src, 0, GUID, DEST, true).expect_err("id 0 is the sentinel");
    assert!(err.contains("reserved"), "{err}");
    assert_eq!(
        src.froze.get(),
        0,
        "a refused begin froze the character anyway — it is now invisible to every target gate \
             with no escrow row to recover it"
    );
}

// -----------------------------------------------------------------------------------------
//  `apply_confirm` / `apply_release` — the two cross-database-only steps, EXECUTED.
// -----------------------------------------------------------------------------------------

#[test]
fn confirm_files_the_attestation_from_the_escrowed_blob() {
    let mut src = beginning(GUID);
    apply_begin(&mut src, XFER, GUID, DEST, true).expect("begin");
    apply_confirm(&mut src, XFER).expect("the attestation is filed");

    let inb = ShardLedger::in_row(&src, XFER).expect("the in-row is the attestation");
    assert_eq!(inb.character_guid, GUID);
    assert_eq!(
        inb.blob,
        ShardLedger::out_row(&src, XFER).expect("escrowed").blob,
        "the attestation must carry the escrowed blob verbatim — it is the destination's own \
             replay copy if an interrupted apply has to be re-driven"
    );
    apply_confirm(&mut src, XFER).expect("a replayed confirm must answer Ok");
}

#[test]
fn confirm_refuses_a_same_database_escrow_and_an_absent_one() {
    let mut src = beginning(GUID);
    apply_begin(&mut src, XFER, GUID, DEST, false).expect("a SAME-database begin");
    let err = apply_confirm(&mut src, XFER).expect_err("same-database attestations are forged");
    assert!(err.contains("refusing to forge an attestation"), "{err}");
    assert!(
        ShardLedger::in_row(&src, XFER).is_none(),
        "same-database, `import_character` files the in-row itself as a fact it can READ. A \
             forged one licenses finish_transfer with no import at all"
    );

    let mut empty = FakeDb::new();
    let err = apply_confirm(&mut empty, XFER).expect_err("there is no escrow here to attest for");
    assert!(err.contains("nothing escrowed here"), "{err}");
    assert!(ShardLedger::in_row(&empty, XFER).is_none());
}

#[test]
fn release_drops_the_arrival_fence_but_refuses_on_the_source() {
    let src = FakeDb::populated(GUID);
    let blob = export(&src, GUID, XFER, DEST);
    let mut dst = FakeDb::new();
    apply_import_blob(&mut dst, XFER, wire(&blob)).expect("import");
    assert!(
        BeginSink::is_in_transit(&dst, GUID),
        "the arrival copy is fenced by its own in-row until release"
    );

    apply_release(&mut dst, XFER).expect("release");
    assert!(
        !BeginSink::is_in_transit(&dst, GUID),
        "release did not drop the arrival fence — the character is durable at the destination \
             and cannot log in anywhere"
    );
    apply_release(&mut dst, XFER).expect("a replayed release must answer Ok");

    // ...and on a database that holds the SOURCE claim, release is the wrong call.
    let mut source = beginning(NEIGHBOUR);
    apply_begin(&mut source, NEIGHBOUR, NEIGHBOUR, DEST, true).expect("begin");
    apply_confirm(&mut source, NEIGHBOUR).expect("confirm");
    let err = apply_release(&mut source, NEIGHBOUR)
        .expect_err("this database holds the out-row — finish_transfer is the correct call");
    assert!(err.contains("call finish_transfer"), "{err}");
    assert!(
        ShardLedger::out_row(&source, NEIGHBOUR).is_some(),
        "dropping the in-row alone would strand the out-row and unfreeze nothing"
    );
}

// -----------------------------------------------------------------------------------------
//  The WHOLE six-step cross-database sequence, across two FakeDbs.
//
//  Every step above is proven in isolation; this is the one that proves they COMPOSE — that
//  the blob step 1 wrote is the blob step 2 reads, that step 2b's attestation is what step 3
//  demands, and that the character ends up live on exactly one side with nothing stranded on
//  the other.
// -----------------------------------------------------------------------------------------

#[test]
fn the_six_step_sequence_moves_a_populated_character_between_two_databases() {
    let mut src = beginning(GUID);
    let mut dst = FakeDb::new();
    let before = (src.gear_of(GUID), src.quests_of(GUID));

    // 1. SOURCE: freeze + serialize + delete the live entity.
    apply_begin(&mut src, XFER, GUID, DEST, true).expect("step 1: begin");
    assert!(
        !BeginSink::is_in_transit(&dst, GUID),
        "the destination has heard nothing yet"
    );

    // 2. DESTINATION: materialise from the blob the gateway carried.
    let blob = ShardLedger::out_row(&src, XFER).expect("escrowed").blob;
    apply_import_blob(&mut dst, XFER, blob).expect("step 2: import");
    assert!(
        src.has_character(GUID) && dst.has_character(GUID),
        "two durable copies, zero live — the safe overlap"
    );
    assert!(
        BeginSink::is_in_transit(&src, GUID) && BeginSink::is_in_transit(&dst, GUID),
        "both copies stay fenced for the whole of the overlap"
    );

    // 2b. SOURCE: the gateway attests that the arrival copy is durable.
    apply_confirm(&mut src, XFER).expect("step 2b: confirm");

    // 3. SOURCE: delete-last.
    apply_finish_step(&mut src, XFER).expect("step 3: finish");
    assert!(
        !src.has_character(GUID),
        "the source copy must be gone once the destination copy is attested"
    );
    assert!(
        src.gear_of(GUID).is_empty() && src.quests_of(GUID).is_empty(),
        "the cascade must take the source's OWNED rows with it"
    );
    assert!(src.settled(XFER), "the source escrow is cleared");
    assert_eq!(
        src.receipt(GUID),
        Some((DEST.map_id, DEST.instance_id)),
        "the source shard must keep a forwarding receipt naming the destination"
    );
    assert!(
        !dst.has_live_entity(GUID) && BeginSink::is_in_transit(&dst, GUID),
        "the arrival copy stays fenced until the source copy is provably gone"
    );

    // 4. DESTINATION: drop the arrival fence. The character is live, on one side only.
    apply_release(&mut dst, XFER).expect("step 4: release");
    assert!(dst.settled(XFER) && src.settled(XFER), "nothing stranded");
    assert!(
        !BeginSink::is_in_transit(&dst, GUID),
        "the character is live at the destination"
    );
    assert_eq!(
        (dst.gear_of(GUID), dst.quests_of(GUID)),
        before,
        "the character arrived with the gear and quest rows it left with"
    );
    dst.with_char(GUID, |c| {
        let want = fixture_character(GUID, "Ponytail");
        assert_eq!(c.name, want.name);
        assert_eq!(c.level, want.level);
        assert_eq!(c.money, want.money);
        assert_eq!(
            (c.map_id, c.pending_instance_id),
            (DEST.map_id, DEST.instance_id),
            "the arrival must be at the destination it was escrowed for"
        );
    });
    assert!(
        src.gear_of(NEIGHBOUR).is_empty() || !src.has_character(NEIGHBOUR),
        "a bystander's rows must not have been swept by the hop"
    );
}

// -----------------------------------------------------------------------------------------
//  `apply_reap` — the recovery net, EXECUTED.
//
//  The `cross_database && !has_in` rule below used to be pinned by a `.contains()` scan of
//  `reap_transfers`' own text, which cannot tell the rule apart from its inverse.
// -----------------------------------------------------------------------------------------

/// Push the fake's clock past the staleness window, so the next `apply_reap` acts.
fn go_stale(db: &FakeDb) {
    db.now.set(NOW + TRANSFER_STALE_MICROS);
}

#[test]
fn the_reaper_is_inert_before_the_stale_window() {
    let mut src = beginning(GUID);
    apply_begin(&mut src, XFER, GUID, DEST, false).expect("begin");
    src.now.set(NOW + TRANSFER_STALE_MICROS - 1);
    apply_reap(&mut src);
    assert!(
        ShardLedger::out_row(&src, XFER).is_some(),
        "the reaper raced a driver that may still be working — it is the net for a CRASHED \
             driver, never a race against a slow one"
    );
}

#[test]
fn the_reaper_rolls_back_a_same_database_escrow_that_provably_never_imported() {
    let mut src = beginning(GUID);
    apply_begin(&mut src, XFER, GUID, DEST, false).expect("begin");
    go_stale(&src);
    apply_reap(&mut src);
    assert!(
        src.settled(XFER),
        "the escrow must be dropped — the destination provably has no copy"
    );
    assert!(
        src.has_character(GUID) && !BeginSink::is_in_transit(&src, GUID),
        "the only durable copy is the source's, so the reaper must UNFREEZE it, not delete it"
    );
}

/// **THE cross-database safety rule**, and the one mutation the same-database crash matrix
/// cannot see. An absent in-row here does not mean "not imported" — it means "not yet
/// ATTESTED", because the arrival copy is on another database. Reading it as `Some(false)`
/// rolls the escrow BACK while the destination copy may already be live: a DUPLICATED
/// character, which is unrecoverable. A frozen one is not.
#[test]
fn the_reaper_holds_an_unattested_cross_database_escrow_forever() {
    let mut src = beginning(GUID);
    apply_begin(&mut src, XFER, GUID, DEST, true).expect("begin");
    // Meanwhile the destination imported perfectly — the source simply cannot see it.
    let mut dst = FakeDb::new();
    let blob = ShardLedger::out_row(&src, XFER).expect("escrowed").blob;
    apply_import_blob(&mut dst, XFER, blob).expect("the import committed at the destination");

    for multiplier in [1, 1_000] {
        src.now
            .set(NOW + TRANSFER_STALE_MICROS.saturating_mul(multiplier));
        apply_reap(&mut src);
        assert!(
            ShardLedger::out_row(&src, XFER).is_some(),
            "the reaper rolled back an UNATTESTED cross-database escrow at {multiplier}x the \
                 stale window. The destination copy is already durable and would go live, while \
                 this rollback unfreezes the source copy — the character is now on two databases, \
                 and nothing can tell which one is real."
        );
        assert!(src.has_character(GUID), "and the source copy is untouched");
    }
}

#[test]
fn the_reaper_rolls_forward_an_attested_escrow_the_driver_abandoned() {
    let mut src = beginning(GUID);
    apply_begin(&mut src, XFER, GUID, DEST, true).expect("begin");
    apply_confirm(&mut src, XFER).expect("the destination copy is attested durable");
    // ...and the driver dies here, between confirm and finish.
    go_stale(&src);
    apply_reap(&mut src);

    assert!(
        src.settled(XFER),
        "an attested escrow must be COMPLETED, not left frozen — past the point of no return \
             the transfer may only ever roll forward"
    );
    assert!(
        !src.has_character(GUID),
        "roll-forward is delete-last: the source copy goes, exactly as finish_transfer would \
             have done it"
    );
    assert_eq!(
        src.receipt(GUID),
        Some((DEST.map_id, DEST.instance_id)),
        "the roll-forward runs the SAME body as finish, forwarding receipt included"
    );
}

#[test]
fn finish_before_the_attestation_refuses_and_leaves_the_source_copy_alone() {
    let mut src = beginning(GUID);
    apply_begin(&mut src, XFER, GUID, DEST, true).expect("begin");
    let err = apply_finish_step(&mut src, XFER).expect_err("not imported — refuse");
    assert!(err.contains("not imported"), "{err}");
    assert!(
        src.has_character(GUID) && ShardLedger::out_row(&src, XFER).is_some(),
        "finish is the ONLY step that destroys the source copy, and doing it here would leave \
             ZERO durable copies"
    );

    // ...and on a database with no escrow at all it is a logged NO-OP, not an error: the
    // gateway fans a finish out across shards and only one of them holds the claim.
    let mut elsewhere = FakeDb::new();
    apply_finish_step(&mut elsewhere, XFER).expect("no escrow here — nothing to finish");
}
