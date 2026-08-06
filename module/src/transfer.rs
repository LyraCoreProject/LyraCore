//! Escrowed character transfer — the ONE primitive behind every sharding granularity (issue #16,
//! spec issue #12: instance entry, continent travel, seam crossings, bulk region migration).
//!
//! # The protocol
//!
//! ```text
//!            begin_transfer            import_character            finish_transfer
//!  Resident ────────────────► Escrowed ────────────────► Imported ────────────────► Resident(dest)
//!   src live                  src frozen                src frozen                  dst live
//!   1 durable                 1 durable (src)           2 durable (src+dst)         1 durable (dst)
//!   0 escrow rows             out-row                   out-row + in-row            0 escrow rows
//!                                  │                         │
//!                                  │ reap (stale, not        │ reap (stale, imported)
//!                                  │ imported) → ROLLBACK    │ → ROLL FORWARD = finish
//!                                  ▼                         ▼
//!                             Resident(src)             Resident(dest)
//! ```
//!
//! Two rules make every step retryable and every crash recoverable:
//!
//! 1. **Delete-last.** The source's durable copy is destroyed only by `finish_transfer`, and only
//!    after the destination's copy is committed (`plan_finish` REFUSES when the in-row is absent).
//!    So no reachable state has zero durable copies.
//! 2. **Idempotent import keyed by transfer id.** `game_transfer_in.transfer_id` is the primary
//!    key, so a replayed `import_character` is a no-op — a driver that crashed without learning
//!    whether its call landed can simply call again.
//!
//! # The in-transit fence, and EXACTLY how far it reaches
//!
//! While either escrow row exists the character is *in transit* — [`is_in_transit`]. Four chokepoints
//! enforce that, and it is worth being precise about which side each one covers, because the epic
//! (#19, the real cross-database move) is built on top of this claim:
//!
//! 1. `helpers::entity_by_owner` — the ACTOR side. Every player-fired reducer resolves "who is
//!    acting" through it, so an in-transit character can take no action itself.
//! 2. `world::player_login` — the RE-MATERIALISATION side. The one path that could put a live entity
//!    back on a shard the character has left (the dual-liveness dupe).
//! 3. `begin_transfer`'s delete of the live `game_world_entity` row — the TARGET side. The ~50
//!    hand-rolled `map_id`/`instance_id` gates, the aggro candidate scan, the threat lists and the
//!    AOI relay all resolve through that row, so they stop seeing the character by construction.
//! 4. `helpers::character_by_guid` / `character_by_name` — the BY-GUID side (issue #30). Reducers
//!    that reach a character by guid or by name straight into `game_character` (or into a
//!    character-owned table) touch none of the first three; this pair is the gate they route
//!    through, and it reads an in-transit character as ABSENT so each caller's existing
//!    "no such character" arm fires (no new error string, no gateway edit).
//!
//! # The by-guid verdict table (issue #30)
//!
//! Refusal is NOT the right answer everywhere, so the class is settled per path rather than
//! uniformly. Same-database every one of these is harmless (a write lands on the SAME row the
//! destination reads); cross-database each unfenced one is a lost write, because the export blob was
//! serialized at `begin_transfer`.
//!
//! **REFUSE** — route through `character_by_guid` / `character_by_name`; an in-transit character
//! reads as absent:
//!
//! - `auth::delete_character` — the one that DESTROYS a durable copy another shard holds a claim on.
//!   Keeps its distinguished `CHAR_IN_TRANSIT` error (see the guard there).
//! - `chat::send_whisper` — reachable because `begin_transfer` deliberately persists with
//!   `set_offline: false`, so `Character.online` stays true. A visibility leak; refusing lands in
//!   the gateway's existing `SMSG_CHAT_PLAYER_NOT_FOUND` mapping.
//! - `gm::set_gm_level` — an operator write to the source copy.
//! - `debug::debug_spawn_player_entity` — `player_login`'s re-materialisation wearing a harness hat.
//! - the remaining `debug_reducers` writers: `debug_set_money`, `debug_expire_quest`,
//!   `debug_grant_reputation`, `debug_grant_default_actions`, `skill::debug_reseed_skills`.
//! - `instance::reap_instances` — the odd one out in shape, same verdict in substance: it refuses to
//!   read an instance as EMPTY while a transfer into or out of it is in flight
//!   (`in_transit_instances`), so it cannot tear down the character's `game_instance_binding`
//!   manifest rows mid-hop.
//!
//! Five more, found by the review's independent call-site audit. The first three are by-guid writers
//! the "the guid came from a live entity, and `begin_transfer` deleted it" argument does NOT cover;
//! the last two are BACKGROUND tick passes, which that argument cannot reach at all because neither
//! reads `game_world_entity`:
//!
//! - `world::debug_delete_character` — `auth::delete_character`'s gate-free harness twin, and
//!   strictly worse: `cascade_delete_character` runs the `character_owned!` sweep, which includes
//!   `sweep_delete_game_transfer_out`, so an unfenced call destroys the character AND both escrow
//!   rows in one transaction. Cross-database the destination's arrival copy is then left with no
//!   source out-row, `recovery` answers `Hold` forever, and the character is wedged frozen. Keeps a
//!   raw existence probe alongside the fence so a MISSING character stays the harness's no-op.
//! - `world::recall_to_home` — the ONE `teleport_player` caller needing no live entity (it reads the
//!   home coords straight off the durable row), and therefore the only route by which
//!   `teleport_player`'s unconditional durable-row write reaches an escrowed character: it moves
//!   `map_id`/`x`/`y`/`z`/`orientation` — five `ExportBlob` fields — plus the `pending_instance_id`
//!   that `in_transit_instances` reads. Reachable by guid through `debug::debug_use_hearthstone`;
//!   fenced in `recall_to_home` itself so a future by-guid caller inherits the gate.
//! - `debug::debug_set_level` — drives `stats::set_character_level`, which writes `Character.level`
//!   (an `ExportBlob` field) and `Character.xp` on the durable row and needs no live entity to do it.
//!   The `debug_set_money` sibling; fenced at the reducer, not in `stats`, because `gm_command`'s
//!   caller is already entity-resolved and the core also serves guids with no character row.
//! - `rest::rested_accrue_pass` — the 30s live-rested accrual. `begin_transfer` persists with
//!   `set_offline: false`, and it is `persist_entity`'s `set_offline` branch that would have called
//!   `materialize_on_logout` to stop the rest clock — so a character escrowed while standing in an
//!   inn keeps `resting == true` with a running `rested_since_micros` and this pass rewrites
//!   `rested_xp` on the frozen row every 30s, for as long as the escrow is held.
//! - `quest::quest_timer_pass` — the 0.5s timed-quest expiry, which writes `game_character_quest`, a
//!   MANIFEST table. The same table and the same reasoning that fenced its harness twin
//!   `debug::debug_expire_quest`, which this pass reached straight past.
//!
//! Both tick passes carry their own `is_in_transit` gate rather than routing through
//! `character_by_guid` (each already holds the row, or its owned row, from a scan), and in both the
//! refusal is a DEFERRAL in substance — which is why REFUSE is honest here rather than
//! value-dropping. The rest pass leaves `rested_since_micros` running, so the first pass after the
//! transfer settles banks the entire span in one step; the quest pass leaves `deadline_micros` set,
//! so the first pass after it settles fails the quest exactly as it would have.
//!
//! **DEFER into the blob as a post-begin delta** — `loot::credit_purse`. A party member collects
//! their share of someone else's kill, so refusing would silently DROP a third party's copper; that
//! third party is not transferring and could not know why they were shorted.
//! `defer_money_delta` folds it into the escrowed blob so the value travels with the character.
//!
//! **REGENERATE at the destination** — `auth::establish_session`, deliberately UNFENCED. It rewrites
//! `Character.owner_identity`, which is per-CONNECTION derived state, not durable character data:
//! `establish_session` is its only writer and rebinds it from the live connection at every logon,
//! which is also why `player_login` restamps the owned rows from `ctx.sender()`. Cross-database the
//! player arrives through the DESTINATION's gateway, which binds it correctly with no help from the
//! blob. Carrying it would ship a source-gateway identity that is meaningless on arrival and
//! overwritten immediately — a field always wrong on arrival is worse than no field, because the
//! next reader cannot tell it is vestigial. Pinned by
//! `owner_identity_is_regenerated_at_the_destination_never_carried`.
//!
//! **NOT A TRANSFER CONCERN — settled by issue #22 (group slice)** — `group::group_accept` /
//! `group_uninvite` / `group_leave`. These were the taxonomy's last three OPEN entries, deliberately
//! left undecided pending the realm-core boundary. That boundary now exists, and it dissolves the
//! question rather than answering it: party membership is authoritative on REALM-CORE, so a third
//! party's accept/kick/leave for an in-transit character is a write to the DIRECTORY database, which
//! is not the database the escrow froze and not a database this transfer touches at all. There is no
//! source-copy write left to lose, so there is nothing here to fence.
//!
//! What replaced the fence question is a REPLICATION one, and it is answered in the other direction:
//! `game_group_member` moved onto [`NOT_TRANSPORTED`] (it must not ride the blob — the blob's
//! snapshot would race the authority), and the gateway re-pushes realm-core's roster onto the
//! destination at world entry via `group::sync_group_mirror`. A character kicked mid-loading-screen
//! therefore arrives ungrouped, which is what actually happened, instead of arriving with the
//! membership it had when it stepped into the portal.
//!
//! The audited exception list is now EMPTY: every by-guid path in the tree is either fenced, listed
//! above with its verdict, or — for these three — no longer a world shard's state.
//!
//! Enforcement: each fenced call site is pinned by a source-scan tripwire in `mod tests` below
//! (deleting a fence turns a NAMED test red — a pure model has no reducers in it and sees none of
//! them), and `lib.rs`'s `character_fence_tripwire` is the ratchet that stops a NEW unfenced by-guid
//! path from being added.
//!
//! # The CROSS-DATABASE protocol (issue #19)
//!
//! #16 shipped the state machine within ONE database (two `instance_id` partitions, so
//! `import_character` re-partitions a row rather than materialising a second). #19 makes the move
//! real: two SpacetimeDB databases, with the gateway carrying the blob between them. The escrow is
//! not forked — the same ledger, the same `plan_*` verdicts, the same reaper — but two facts the
//! same-database deployment gets for free have to be supplied explicitly:
//!
//! **1. The rows have to actually travel.** `ExportBlob` carries `character_row` (the whole
//! `game_character` row, bsatn) plus a `payload` of one `TableRows` per manifest table, produced and
//! consumed by that table's `character_owned!(transfer, ..)` arm. One marker per table, next to the
//! table, exactly like the delete/restamp arms — and
//! `every_manifest_table_can_cross_a_database_boundary` fails if a manifest table has no arm,
//! because a missing arm is silent data LOSS the first time a character crosses.
//!
//! **2. Neither side can see the other's ledger row.** So the step sequence gains two calls, and
//! `cross_database` on the out-row marks the escrow as one where a local ledger read means
//! something different:
//!
//! ```text
//!   SOURCE db                                  DESTINATION db
//!   begin_transfer  ──── blob ────────────────►
//!                                              import_character_blob   (materialise + in-row)
//!                   ◄─── "it committed" ───────
//!   confirm_import  (files the SOURCE in-row = the attestation)
//!   finish_transfer (cascade-delete the source copy, clear the escrow)
//!                   ──── "source is gone" ────►
//!                                              release_transfer        (drop the in-row → LIVE)
//! ```
//!
//! - `confirm_import` exists because `finish_transfer` refuses without an in-row, and cross-database
//!   the source never gets one from `import_character`. It moves the "is the destination copy
//!   durable?" judgement to the gateway — the one component that can see both databases — and
//!   nothing downstream changes, because the in-row still means exactly what it meant.
//! - `release_transfer` exists because the destination's in-row is on the destination, so
//!   `finish_transfer` (running on the source) cannot clear it. It is called LAST, so the arrival
//!   copy stays fenced until the source copy is provably gone.
//! - `reap_transfers` must NOT read a missing in-row as "not imported" for a cross-database escrow —
//!   it means "not yet attested". It answers `None` (the documented unconsultable case) and HOLDS.
//!   Cross-database escrows therefore never roll back; recovery is forward-only, re-driven by the
//!   gateway at the player's next world entry.
//! - `do_finish` cascade-deletes the source copy for a cross-database escrow (there IS a second copy
//!   now), which is a no-op same-database by construction.
//!
//! What is still parked: the LIVE two-database run. Every acceptance criterion of #19 needs a real
//! 1.12.1 client, two published databases and an operator — see issue #231. What is proven
//! headlessly is the state machine (the crash
//! matrix below, now walked for the six-step cross-database sequence too), the transport ratchet,
//! and the gateway's orchestration/ordering against two in-memory stores.
//!
//! Everything in the "pure core" section below is `ReducerContext`-free on purpose: it is the whole
//! decision surface of the protocol, so the crash matrix in `mod tests` drives the SAME functions
//! the reducers execute. [server]

use spacetimedb::{
    log, reducer, table, ReducerContext, ScheduleAt, SpacetimeType, Table, TimeDuration,
};

use crate::auth::game_guid_range;
use crate::helpers::require_operator;
use crate::{game_character, game_world_entity};

// ===========================================================================================
//  Policy constants
// ===========================================================================================

/// How long an escrow row may sit without progressing before the reaper treats the driver as dead
/// and recovers the transfer. Generous relative to the spec's ~1s warm-handoff budget: the reaper
/// is the net for a CRASHED driver, never a race against a slow one.
pub(crate) const TRANSFER_STALE_MICROS: i64 = 30_000_000; // 30s

/// `reap_transfers` cadence. Seconds-scale: an abandoned transfer is a frozen player staring at a
/// loading screen, so recovery latency is user-visible in a way the 60s instance reaper's is not.
const TRANSFER_REAP_INTERVAL_MICROS: u64 = 5_000_000; // 5s

/// The subset of [`crate::CHARACTER_OWNED_TABLES`] marked **hot**: state the destination needs in
/// the player's first frame (worn gear, castable abilities, trained skills, the action bar). The
/// rest is **cold** — correct to stream in behind the loading screen.
///
/// Deliberate simplification: in v1 the mark is CARRIED but not ACTED ON — one blob ships
/// everything, because same-database transfers have nothing to stream. It becomes load-bearing
/// when the seam-crossing warm handoff has to fit a ~1s budget (spec #12, Phase C): cold tables
/// move after the handshake.
/// Verified against the generated enumeration by `hot_marks_name_only_real_manifest_tables`.
const HOT_TABLES: &[&str] = &[
    "game_item_instance",
    "game_player_action",
    "game_player_skill",
    "game_player_spell",
    "game_character_talent",
    // Issue #72 hot-state audit: a buff/debuff bar (and Stealth, which is presence-only — no timer
    // to stream in "behind" anything) is exactly the first-frame-visible state this mark describes.
    "game_aura",
];

/// Manifest entries that are transfer MACHINERY rather than character data. `game_transfer_out`
/// earns a `character_owned!` delete sweep (a deleted character must not leave escrow rows behind)
/// and therefore lands in the generated enumeration — but exporting the escrow inside its own
/// export blob is nonsense.
const MANIFEST_EXCLUDE: &[&str] = &["game_transfer_out"];

/// The manifest tables whose rows deliberately do NOT cross a database boundary — the ONLY tables
/// whose `character_owned!(transfer, ..)` arm may call [`not_transported`] (issue #19 review).
///
/// The arm-exists ratchet (`every_manifest_table_can_cross_a_database_boundary`) cannot tell a
/// transport arm from a `not_transported` stub, so on its own it is defeated by the one edit it
/// exists to stop: swapping a real table's arm for `not_transported(io)` keeps the ratchet green
/// while every character silently arrives without that table's rows. Verified by mutation — pointing
/// `sweep_transfer_game_item_instance` at `not_transported` left all 468 module tests passing while
/// deleting every player's gear on every hop.
///
/// So the "not transported" decision is written HERE as well as at the table, and
/// `not_transported_is_an_allowlisted_decision_not_a_silent_drop` fails if the two disagree in
/// either direction. Each entry needs its reason, exactly like `EXEMPT_ACCESSORS` in `lib.rs`:
///
/// - `game_rest_state_event` — a one-shot relay row with a GC TTL; the DURABLE rest state
///   (`resting` / `rested_xp` / `rested_since_micros`) lives on the character row and travels in
///   `character_row`.
/// - `game_group_invite` — a 2-minute dialog whose inviter is by definition not transferring.
/// - `game_pet_command` — the live pet's stay/follow/aggressive state; the pet is a
///   `game_world_entity`, which does not cross, so its command row has nothing to attach to.
/// - `game_group_member` — party membership (#22, group slice). Authoritative on REALM-CORE, so the
///   blob must not carry it: a snapshot taken at `begin_transfer` would race the authority, and it
///   is exactly the snapshot #19's interim mirror was (a party SPLIT across the boundary could never
///   see itself). The gateway re-pushes realm-core's roster onto the destination at world entry
///   (`sync_group_mirror`), so membership crosses by replication rather than by carriage.
/// - `game_character_shard` — the realm-core character→shard directory (#20). A routing HINT about
///   where the character is, and the blob exists to change that: the snapshot `begin_transfer` takes
///   still names the SOURCE, so carrying it would hand the destination a forwarding receipt pointing
///   back at the shard the character just left. `do_finish` rewrites the source's own row to name the
///   destination, and the authoritative copy on realm-core is the gateway's write.
// Read only by the ratchet below — it is a written DECISION, kept next to `MANIFEST_EXCLUDE` in
// the file that owns the protocol rather than hidden in `mod tests`, so the next person to reach
// for `not_transported` finds the list they have to justify themselves against.
#[cfg_attr(not(test), allow(dead_code))]
const NOT_TRANSPORTED: &[&str] = &[
    "game_rest_state_event",
    "game_group_invite",
    "game_group_member",
    "game_pet_command",
    "game_character_shard",
];

// ===========================================================================================
//  Ledger tables
// ===========================================================================================

/// SOURCE-side escrow: "this character is frozen here, its state is serialized in `blob`, and it is
/// on its way to `dest_*`". Its existence IS the in-transit flag.
///
/// Note: the spec asks for the in-transit flag as a COLUMN on `game_character`. It lives here
/// instead because `game_character` is gateway-subscribed with a HAND-MAINTAINED binding
/// (`docs/danger-zones.md` §1.2 / §1.6) — a column there needs a lockstep gateway edit, which is
/// issue #19's tree, not this one's. The ledger row is a strictly stronger flag anyway (it carries
/// the destination and the blob), and the `by_character` index makes the check one indexed lookup.
/// Upgrade path: fold into a `Character.in_transit_id` column when the gateway bindings are
/// regenerated for multi-shard routing.
///
/// NOT public / NOT gateway-subscribed (the `game_instance` precedent): no client and no relay ever
/// reads escrow rows, so no binding file exists for them. [server]
#[table(
    accessor = game_transfer_out,
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct TransferOut {
    /// Caller-chosen (the gateway orchestrator's) transfer id — NOT `auto_inc`. It is the
    /// idempotency key shared with the destination, so it must be stable across a retry.
    #[primary_key]
    pub transfer_id: u64,
    pub character_guid: u64,
    pub dest_map_id: u32,
    pub dest_instance_id: u64,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_z: f32,
    pub dest_o: f32,
    /// bsatn of [`ExportBlob`].
    pub blob: Vec<u8>,
    pub created_micros: i64,
    /// **Is the destination a different DATABASE?** (issue #19.) Two things hang off it, and both
    /// are dupe/loss safety, not convenience:
    ///
    /// 1. `do_finish` must `cascade_delete_character` the source copy — same-database there is no
    ///    second copy to delete, cross-database there is and leaving it is a dupe the moment the
    ///    character logs in on either side.
    /// 2. The REAPER must answer `None` ("destination unconsultable") instead of reading the local
    ///    in-row. Cross-database the in-row lives on the OTHER database, so a local read says
    ///    "not imported" for a transfer that imported perfectly — and `recovery` would roll BACK
    ///    past the point of no return, duplicating the character. See [`reap_transfers`].
    ///
    /// END-appended + `#[default(false)]` → additive auto-migration; every pre-#19 escrow row reads
    /// as same-database, which is exactly what it was.
    #[default(false)]
    pub cross_database: bool,
}

/// DESTINATION-side escrow: the committed, durable arrival copy. Written by `import_character`;
/// its presence is what licenses `finish_transfer` to destroy the source copy (delete-last). [server]
#[table(
    accessor = game_transfer_in,
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct TransferIn {
    #[primary_key]
    pub transfer_id: u64,
    pub character_guid: u64,
    /// The blob exactly as escrowed on the source — kept until `finish_transfer` so an interrupted
    /// apply can be replayed from the destination's own storage.
    pub blob: Vec<u8>,
    pub created_micros: i64,
}

/// Drives [`reap_transfers`]. Armed lazily by `begin_transfer` (see there). [server]
#[table(accessor = game_transfer_reaper_schedule, scheduled(reap_transfers))]
pub struct TransferReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

// A deleted character must not leave escrow rows behind (they would freeze a guid that no longer
// exists, and — after guid reuse — a brand-new character). Sweeps BOTH ledger tables; one marker
// per file is what the `lib.rs` tripwire requires. NOTE: this puts `game_transfer_out` into the
// generated `CHARACTER_OWNED_TABLES` enumeration, which `MANIFEST_EXCLUDE` above then filters back
// out of the export blob — the escrow is machinery, not character data.
crate::character_owned!(delete, fn sweep_delete_game_transfer_out(ctx, character_guid) {
    let out = ctx.db.game_transfer_out();
    let ids: Vec<u64> = out.by_character().filter(character_guid).map(|r| r.transfer_id).collect();
    for id in ids {
        out.transfer_id().delete(id);
    }
    let inb = ctx.db.game_transfer_in();
    let ids: Vec<u64> = inb.by_character().filter(character_guid).map(|r| r.transfer_id).collect();
    for id in ids {
        inb.transfer_id().delete(id);
    }
});

// ===========================================================================================
//  Export blob
// ===========================================================================================

/// One manifest row: a character-owned table plus its hot/cold mark.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntry {
    pub table: String,
    pub hot: bool,
}

/// One manifest table's ROWS, serialized (issue #19). `rows` is bsatn of that table's `Vec<Row>`,
/// produced and consumed by the table's own `character_owned!(transfer, ..)` arm — the only code
/// that knows the row type. Everything between the two arms treats it as opaque bytes, which is
/// what lets ONE blob carry every table with zero per-table code in the protocol itself.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct TableRows {
    pub table: String,
    pub rows: Vec<u8>,
}

// ===========================================================================================
//  Cross-database row transport (issue #19)
// ===========================================================================================

/// The direction a `character_owned!(transfer, ..)` arm is running in. ONE body serves both, so a
/// table cannot ship rows it does not know how to receive (the drift that would silently drop a
/// table's data at the destination).
pub enum RowIo<'a> {
    /// Collect this table's rows for the character into `0` (bsatn of `Vec<Row>`).
    Export(&'a mut Vec<u8>),
    /// Apply the rows in `0`; `1` accumulates the first decode failure.
    Import(&'a [u8], &'a mut Result<(), String>),
}

/// The body of every transport arm: `export` yields the character's rows on THIS database,
/// `insert` puts one arriving row back. Pure plumbing — the arm supplies the two table-typed
/// halves and nothing else.
///
/// IMPORT IS DELETE-THEN-INSERT at the call site level: [`import_rows`] does not clear the table,
/// because a destination that already holds rows for this guid is either (a) an import REPLAY,
/// which `import_character_blob` short-circuits before reaching here, or (b) a same-guid character
/// that never left — which cannot happen, since `begin_transfer` refuses to escrow a character
/// twice. Inserting into a table that already has the row would be a PK panic, which is a loud
/// failure and not a silent dupe; see `import_character_blob`'s wipe-first guard.
///
/// `C` is the context type. In production it is ALWAYS `ReducerContext` (inferred at every arm, so
/// no arm changed shape when this parameter appeared) — it is generic solely so the execution
/// harness in `mod transport_harness` can drive this exact body against a fake store. See that
/// module's header for the seam and its ceiling (issue #37).
pub(crate) fn move_rows<C, R>(
    ctx: &C,
    io: &mut RowIo<'_>,
    export: impl FnOnce() -> Vec<R>,
    insert: impl Fn(&C, R),
) where
    R: spacetimedb::SpacetimeType
        + spacetimedb::sats::Serialize
        + for<'de> spacetimedb::sats::Deserialize<'de>,
{
    match io {
        RowIo::Export(out) => **out = encode_rows(export()),
        RowIo::Import(bytes, outcome) => {
            for r in decode_rows::<R>(bytes, outcome) {
                insert(ctx, r);
            }
        }
    }
}

/// The EXPORT codec, split out of [`move_rows`] so it is `ReducerContext`-free and therefore
/// natively testable (`the_row_codec_round_trips_and_refuses_garbage`). Every module test in this
/// crate is pure or a source scan — nothing can run a reducer — so a codec left inside `move_rows`
/// has literally no behavioural coverage, and mutation-testing this file proved exactly that.
///
/// An unserializable row would silently ship an EMPTY table, and the import-side manifest check
/// cannot tell "no rows" from "lost rows" — so the failure is logged loudly and the buffer left
/// empty rather than half-written.
pub(crate) fn encode_rows<R>(rows: Vec<R>) -> Vec<u8>
where
    R: spacetimedb::SpacetimeType + spacetimedb::sats::Serialize,
{
    match spacetimedb::sats::bsatn::to_vec(&rows) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::error!("transfer export: cannot serialize rows: {e}");
            Vec::new()
        }
    }
}

/// The IMPORT codec, the counterpart of [`encode_rows`]. An EMPTY payload means "this table had no
/// rows for this character" and yields nothing; anything that fails to decode records the FIRST
/// failure in `outcome` (which `import_rows` turns into a whole-transaction abort) and yields
/// nothing, so a table is never half-applied.
pub(crate) fn decode_rows<R>(bytes: &[u8], outcome: &mut Result<(), String>) -> Vec<R>
where
    R: spacetimedb::SpacetimeType + for<'de> spacetimedb::sats::Deserialize<'de>,
{
    if bytes.is_empty() {
        return Vec::new(); // the table had no rows for this character
    }
    match spacetimedb::sats::bsatn::from_slice::<Vec<R>>(bytes) {
        Ok(rows) => rows,
        Err(e) => {
            if outcome.is_ok() {
                *outcome = Err(format!("cannot deserialize arriving rows: {e}"));
            }
            Vec::new()
        }
    }
}

/// A transport arm for a table whose rows deliberately do NOT cross: one-shot relay/event rows with
/// a GC TTL, whose durable half lives elsewhere (usually on the character row itself). Exports
/// nothing and ignores anything that arrives.
///
/// This exists so "not transported" is a DECISION written at the table, rather than a missing arm
/// that the ratchet would have to distinguish from an oversight — which it cannot.
pub(crate) fn not_transported(io: &mut RowIo<'_>) {
    match io {
        RowIo::Export(out) => out.clear(),
        RowIo::Import(..) => {}
    }
}

/// One entry of a transport registry: a table name and the `character_owned!(transfer, ..)` arm
/// that moves its rows. `crate::CHARACTER_OWNED_TRANSFERS` is `&[TransportArm<ReducerContext>]`;
/// the harness supplies its own slice over a fake context (issue #37).
pub(crate) type TransportArm<'a, C> = (&'a str, fn(&C, u64, &mut RowIo<'_>));

/// Serialize every manifest table's rows for `character_guid` — the payload half of the export
/// blob. Tables with no transport arm are ABSENT from the result (and caught by
/// `every_manifest_table_can_cross_a_database_boundary`, not silently).
///
/// The registry is a PARAMETER so this loop can be executed by `mod transport_harness` — it is the
/// only seam by which a test in this crate can run the real export body, since the arms themselves
/// need a `ReducerContext` (issue #37).
///
/// **No coverage check here, deliberately** (issue #42 AC 4). The import side needs one because it
/// consumes a payload from ANOTHER database; this loop MANUFACTURES the payload from the same
/// registry it would check against, pushing one entry per non-excluded arm unconditionally, so
/// "the payload covers the registry" is a tautology no mutation of this body can break without
/// also breaking `a_populated_character_crosses_a_database_with_every_row_and_value` (which pins
/// the payload's table list to `TRANSPORTED`, in registry order). The one export failure that IS
/// real — [`encode_rows`] logging and yielding an empty buffer when a row will not serialize — a
/// coverage check cannot see either, because the entry is present and "no rows" is a legal empty.
/// Catching that needs `RowIo::Export` to carry a `Result`, which is a protocol change, not this
/// issue.
pub(crate) fn export_rows_via<C>(
    ctx: &C,
    character_guid: u64,
    arms: &[TransportArm<'_, C>],
) -> Vec<TableRows> {
    let mut out = Vec::new();
    for (table, mover) in arms {
        if MANIFEST_EXCLUDE.contains(table) {
            continue;
        }
        let mut rows = Vec::new();
        mover(ctx, character_guid, &mut RowIo::Export(&mut rows));
        out.push(TableRows {
            table: (*table).to_string(),
            rows,
        });
    }
    out
}

/// The production binding of [`export_rows_via`]: this build's generated registry.
pub(crate) fn export_rows(ctx: &ReducerContext, character_guid: u64) -> Vec<TableRows> {
    export_rows_via(ctx, character_guid, crate::CHARACTER_OWNED_TRANSFERS)
}

/// Apply an arriving payload. Refuses (whole transaction aborts) on a table this build does not
/// know, a table this build DOES know that the payload does not carry, or a payload it cannot
/// decode — a partial import is the one outcome worse than none, since the in-row filed afterwards
/// would license deleting the source copy.
///
/// The coverage half is issue #42: this loop used to iterate the PAYLOAD, so a blob missing one or
/// more manifest tables imported with a clean `Ok(())`, the in-row was filed, and `finish_transfer`
/// then destroyed the complete source copy of a character that had arrived partial. The unknown-table
/// direction (#16's drift contract) was already guarded; the inverse was not, which reads as an
/// oversight rather than a decision. Low reachability while every shard runs the same build, routine
/// at Phase B (#24), where a rolling deploy makes payload and registry disagree by design.
///
/// Note: the required set is every registry table minus [`MANIFEST_EXCLUDE`] — including the
/// `not_transported` ones, which is stricter than "the tables that carry rows" and needs no second
/// list to stay in step. [`export_rows_via`] emits an entry for each of them too (an EMPTY one), so
/// this is exactly the contract a blob built by this protocol already satisfies; a blob that omits
/// the entry entirely was not built by it.
///
/// Registry-parameterized for the same reason [`export_rows_via`] is (issue #37).
pub(crate) fn import_rows_via<C>(
    ctx: &C,
    character_guid: u64,
    payload: &[TableRows],
    arms: &[TransportArm<'_, C>],
) -> Result<(), String> {
    // COVERAGE FIRST — before a single row is applied, so a short payload aborts having written
    // nothing rather than half a character.
    let missing: Vec<&str> = arms
        .iter()
        .map(|(table, _)| *table)
        .filter(|table| !MANIFEST_EXCLUDE.contains(table))
        .filter(|table| !payload.iter().any(|entry| entry.table == *table))
        .collect();
    if !missing.is_empty() {
        // Loud: the names are the whole diagnosis (which build shipped it, and what the character
        // would have lost had this been accepted).
        log::error!(
            "transfer import: arriving payload is MISSING {} manifest table(s) for character \
             {character_guid}: {} — refusing the import so the source copy survives",
            missing.len(),
            missing.join(", ")
        );
        return Err(format!(
            "arriving payload does not carry manifest table(s) {} which this shard expects — \
             refusing a partial import",
            missing.join(", ")
        ));
    }
    for entry in payload {
        let Some((_, mover)) = arms.iter().find(|(t, _)| *t == entry.table) else {
            return Err(format!(
                "arriving payload names table {} which this shard has no transport arm for — \
                 refusing a partial import",
                entry.table
            ));
        };
        let mut outcome = Ok(());
        mover(
            ctx,
            character_guid,
            &mut RowIo::Import(&entry.rows, &mut outcome),
        );
        outcome.map_err(|e| format!("table {}: {e}", entry.table))?;
    }
    Ok(())
}

/// The production binding of [`import_rows_via`]: this build's generated registry.
pub(crate) fn import_rows(
    ctx: &ReducerContext,
    character_guid: u64,
    payload: &[TableRows],
) -> Result<(), String> {
    import_rows_via(
        ctx,
        character_guid,
        payload,
        crate::CHARACTER_OWNED_TRANSFERS,
    )
}

/// What crosses the seam. The HOT character state travels as typed fields (it is the same handful
/// of values `persist_entity` writes back to the character row); the character-owned TABLES travel
/// as the manifest, derived from `CHARACTER_OWNED_TABLES` — the build-time enumeration generated
/// from the `character_owned!` delete markers, so it can never drift from the sweep registry.
///
/// The manifest is the load-bearing SCHEMA half: the destination compares it against its OWN build
/// (`manifest()`) and refuses an import from a shard whose character-owned table set differs. The
/// `payload` alongside it is the DATA half (issue #19) — the actual rows, one entry per manifest
/// table, produced by that table's `character_owned!(transfer, ..)` arm. `character_row` is the
/// `game_character` row itself, serialized whole so a column added to `Character` travels without
/// anyone remembering to add a field here.
#[derive(SpacetimeType, Clone, Debug, PartialEq)]
pub struct ExportBlob {
    pub transfer_id: u64,
    pub character_guid: u64,
    pub name: String,
    pub level: u8,
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub o: f32,
    pub health: u32,
    pub power: u32,
    pub money: u32,
    pub manifest: Vec<ManifestEntry>,
    // --- issue #19: what makes the blob a real cross-DATABASE move rather than a manifest ---
    /// Where the character is going. Carried in the blob (not just the source's out-row) because
    /// cross-database the blob is the ONLY thing that reaches the destination.
    pub dest_map_id: u32,
    pub dest_instance_id: u64,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_z: f32,
    pub dest_o: f32,
    /// bsatn of the whole `Character` row. Opaque here on purpose: `import_character_blob` decodes
    /// it with the DESTINATION's own `Character` type, so a shard on a different build fails loudly
    /// at decode instead of silently dropping the columns it does not know.
    pub character_row: Vec<u8>,
    /// One entry per manifest table, in `CHARACTER_OWNED_TRANSFERS` order.
    pub payload: Vec<TableRows>,
}

/// Where a transfer is going. Six positional `dest_*` arguments in a row is the shape that makes a
/// transposed pair invisible, and `build_export_blob` is now called from two places.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Destination {
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub o: f32,
}

/// Build the export blob for `character`. `ReducerContext`-free, so the harness can produce a REAL
/// blob from a fixture character and feed it to the REAL importer (issue #37) — the export half of
/// the round-trip property. `payload` comes from [`export_rows`], the only part that needs a
/// database.
pub(crate) fn build_export_blob(
    transfer_id: u64,
    character: &crate::character::Character,
    dest: Destination,
    payload: Vec<TableRows>,
) -> Result<ExportBlob, String> {
    let character_row = spacetimedb::sats::bsatn::to_vec(&character)
        .map_err(|e| format!("transfer {transfer_id}: cannot serialize the character row: {e}"))?;
    Ok(ExportBlob {
        transfer_id,
        character_guid: character.guid,
        name: character.name.clone(),
        level: character.level,
        map_id: character.map_id,
        instance_id: character.pending_instance_id,
        x: character.x,
        y: character.y,
        z: character.z,
        o: character.orientation,
        health: character.health,
        power: character.power,
        money: character.money,
        manifest: manifest(),
        dest_map_id: dest.map_id,
        dest_instance_id: dest.instance_id,
        dest_x: dest.x,
        dest_y: dest.y,
        dest_z: dest.z,
        dest_o: dest.o,
        character_row,
        payload,
    })
}

/// THE transfer manifest: every character-owned table this build knows about, hot/cold marked.
/// Pure (reads only the generated const), so the tripwire tests below can assert it natively.
pub(crate) fn manifest() -> Vec<ManifestEntry> {
    crate::CHARACTER_OWNED_TABLES
        .iter()
        .filter(|t| !MANIFEST_EXCLUDE.contains(t))
        .map(|t| ManifestEntry {
            table: (*t).to_string(),
            hot: HOT_TABLES.contains(t),
        })
        .collect()
}

// ===========================================================================================
//  Pure core — the entire decision surface of the protocol
// ===========================================================================================

/// Where a transfer id stands, read off the two ledger rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Phase {
    /// No escrow: the character is simply resident wherever its durable copy is.
    Resident,
    /// Out-row only — frozen on the source, nothing durable at the destination yet.
    Escrowed,
    /// Out-row + in-row — the destination copy is durable; the source copy is still there, frozen.
    Imported,
}

pub(crate) fn phase(has_out: bool, has_in: bool) -> Phase {
    match (has_out, has_in) {
        (false, _) => {
            // (false, true) is unreachable: every writer that clears the out-row clears the in-row
            // in the SAME transaction. Treat it as Resident — the only safe reading, since a lone
            // in-row licenses nothing (no source copy is waiting to be deleted).
            Phase::Resident
        }
        (true, false) => Phase::Escrowed,
        (true, true) => Phase::Imported,
    }
}

/// What `begin_transfer` must do. Pure; the reducer only executes the verdict.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BeginPlan {
    /// This transfer id is already escrowed (or already imported) FOR THIS CHARACTER — a retry, do
    /// nothing.
    Replay,
    /// No durable source copy to freeze.
    NoSource,
    /// This transfer id is escrowed for a DIFFERENT character. Refuse — see [`plan_begin`].
    IdCollision,
    /// This character is already escrowed under a DIFFERENT transfer id. Refuse — see [`plan_begin`].
    AlreadyInTransit,
    /// Freeze + serialize + delete the live entity, in one transaction.
    Escrow,
}

/// `escrowed_guid` is the character named by the ledger rows filed under THIS transfer id (out-row
/// first, in-row as the fallback), or `None` when the id is unused.
///
/// The two refusals matter because `transfer_id` is CALLER-chosen (the gateway orchestrator's), not
/// `auto_inc`, so both collisions are reachable by inputs:
///
/// - **`IdCollision`.** Returning `Replay` on an id reused for a different character would answer
///   `Ok(())` — "your call already landed" — to a driver whose character was never escrowed. That
///   driver then drives `import_character`/`finish_transfer` on the id and moves the OTHER
///   character to its destination, while reporting success for one that never moved.
/// - **`AlreadyInTransit`.** A character escrowed twice under two ids has two destinations each
///   holding a claim on it; cross-database, both would import and the character is DUPLICATED. (The
///   ledger is keyed by transfer id, so no per-id check can see this — it needs the by-character
///   lookup `is_in_transit` does.)
pub(crate) fn plan_begin(
    escrowed_guid: Option<u64>,
    character_guid: u64,
    source_durable: bool,
    character_in_transit: bool,
) -> BeginPlan {
    if let Some(escrowed) = escrowed_guid {
        return if escrowed == character_guid {
            BeginPlan::Replay
        } else {
            BeginPlan::IdCollision
        };
    }
    if !source_durable {
        return BeginPlan::NoSource;
    }
    if character_in_transit {
        return BeginPlan::AlreadyInTransit;
    }
    BeginPlan::Escrow
}

/// Which character the ledger rows filed under one transfer id name: the SOURCE out-row's, with
/// the DESTINATION in-row's as the fallback.
///
/// The fallback is not tidiness. Now that the transfer id IS the character guid (#19), a database
/// holding only an unreleased ARRIVAL in-row — the state left by a driver killed between
/// `finish_transfer` and `release_transfer` — would otherwise read as an UNUSED id. `plan_begin`
/// would then answer `Escrow`... except it never gets there, because [`is_in_transit`] sees the
/// in-row and answers `AlreadyInTransit`, so the character is refused a transfer OUT of the shard
/// it is stuck on, permanently, with no operator recourse (the #36 review's blocker 2). Reading the
/// in-row here is what turns that into the `Replay` the `settle_transfer` fence-clear then repairs.
///
/// Pure, so it is pinned by a real assertion rather than by the reducer's text — the whole line was
/// a #36 mutation survivor (issue #37).
pub(crate) fn escrowed_guid(out_row: Option<u64>, in_row: Option<u64>) -> Option<u64> {
    out_row.or(in_row)
}

/// What `import_character` must do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ImportPlan {
    /// The in-row already exists — the unique transfer-id key makes replay a no-op.
    Replay,
    /// Nothing was escrowed under this id. Refuse: importing without an escrow would materialise a
    /// copy no source is waiting to release (the dupe path).
    NoEscrow,
    /// Commit the arrival copy.
    Apply,
}

pub(crate) fn plan_import(has_out: bool, has_in: bool) -> ImportPlan {
    if has_in {
        return ImportPlan::Replay;
    }
    if !has_out {
        return ImportPlan::NoEscrow;
    }
    ImportPlan::Apply
}

/// What `finish_transfer` must do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FinishPlan {
    /// Both rows are already gone — a retry after a successful finish.
    AlreadyDone,
    /// Escrowed but NOT imported. Refuse: this is the only step that destroys the source copy, and
    /// doing it here would leave zero durable copies.
    NotImported,
    /// Delete-last: destroy the source copy, then both escrow rows.
    Complete,
}

pub(crate) fn plan_finish(has_out: bool, has_in: bool) -> FinishPlan {
    match phase(has_out, has_in) {
        Phase::Resident => FinishPlan::AlreadyDone,
        Phase::Escrowed => FinishPlan::NotImported,
        Phase::Imported => FinishPlan::Complete,
    }
}

/// What the reaper must do about an escrow whose driver never came back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Recovery {
    /// Do nothing — not stale yet, nothing escrowed, or the destination could not be consulted.
    Hold,
    /// The import provably never happened: drop the escrow and unfreeze on the source.
    Rollback,
    /// The destination copy is durable: complete the transfer (delete-last) instead of undoing it.
    RollForward,
}

/// The recovery rule, and the single most load-bearing function in this file.
///
/// `dest_imported` is deliberately an `Option`: **once the destination copy is durable the transfer
/// may only ever roll FORWARD**, so a reaper that cannot establish which side of that line it is on
/// must not guess — guessing rollback against a successful import duplicates the character, and
/// guessing roll-forward against a failed one destroys it. Same-database (this ticket) always
/// answers `Some(..)` by reading the in-row directly.
///
/// Note: cross-database (issue #19) the answer comes from the gateway querying the destination
/// shard, and `None` — destination unreachable — holds the escrow indefinitely. That is the correct
/// failure mode: a frozen character is recoverable, a duplicated or deleted one is not. Escalation
/// (alerting an operator on a long-held escrow) is ops tooling, not this primitive.
pub(crate) fn recovery(has_out: bool, dest_imported: Option<bool>, age_micros: i64) -> Recovery {
    if !has_out {
        return Recovery::Hold;
    }
    if age_micros < TRANSFER_STALE_MICROS {
        return Recovery::Hold;
    }
    match dest_imported {
        Some(true) => Recovery::RollForward,
        Some(false) => Recovery::Rollback,
        None => Recovery::Hold,
    }
}

/// May a login materialise a live entity for this character here? Only when it holds no escrow.
/// This is the in-transit fence expressed as a predicate, so it is testable without a context.
pub(crate) fn login_allowed(has_out: bool, has_in: bool) -> bool {
    !has_out && !has_in
}

// ===========================================================================================
//  Ledger reads
// ===========================================================================================

/// Is `character_guid` mid-transfer? Two indexed lookups. The gate every caller uses — see
/// `helpers::entity_by_owner` (the actor chokepoint, 60+ call sites) and `world::player_login`.
pub(crate) fn is_in_transit(ctx: &ReducerContext, character_guid: u64) -> bool {
    let has_out = ctx
        .db
        .game_transfer_out()
        .by_character()
        .filter(character_guid)
        .next()
        .is_some();
    let has_in = ctx
        .db
        .game_transfer_in()
        .by_character()
        .filter(character_guid)
        .next()
        .is_some();
    // The same predicate the crash matrix drives — in-transit is exactly "login is fenced".
    !login_allowed(has_out, has_in)
}

/// Every instance id an in-transit character has a claim on: the escrow's DESTINATION and the
/// source instance `begin_transfer` parked on the durable row. Consumed by
/// `instance::occupied_instances` (issue #30, REFUSE verdict) so the instance reaper cannot tear
/// down an instance — and with it the character's `game_instance_binding` manifest rows — while a
/// transfer into or out of it is still in flight.
///
/// Deliberate simplification: a full scan of `game_transfer_out`, not an index probe — the escrow
/// table holds one row per IN-FLIGHT transfer (seconds of lifetime, reaped at 30s), so it is empty
/// in the common case and tiny in the worst one, and the reaper it feeds runs once a minute.
/// Upgrade path: none needed until transfer rates approach per-tick.
pub(crate) fn in_transit_instances(ctx: &ReducerContext) -> Vec<u64> {
    let chars = ctx.db.game_character();
    let mut out = Vec::new();
    for row in ctx.db.game_transfer_out().iter() {
        if row.dest_instance_id != 0 {
            out.push(row.dest_instance_id);
        }
        // Deliberately the RAW row read, not `helpers::character_by_guid`: this call site exists
        // BECAUSE the character is in transit, so the fenced reader would return `None` every time.
        if let Some(c) = chars.guid().find(row.character_guid) {
            if c.pending_instance_id != 0 {
                out.push(c.pending_instance_id);
            }
        }
    }
    out
}

/// DEFER verdict (issue #30) — fold a post-`begin_transfer` `money` credit into the escrowed export
/// blob, so value that lands after the character was serialized still travels with it. Returns
/// `true` when the delta was deferred (the caller is talking to an in-transit character), `false`
/// when there is nothing in escrow and the caller should just write the row.
///
/// This is the verdict a refusal would get WRONG: `loot::credit_purse` pays a party member their
/// share of someone else's kill. Refusing would drop a third party's copper on the floor, and that
/// third party is not the one transferring and cannot know why they were shorted.
///
/// Both escrow phases fold. Pre-import (out-row only) the blob is what ships, so the delta arrives
/// with the character. Post-import (out-row + in-row) the destination copy is already durable and
/// nothing reads the blob again — same-database that is harmless because both partitions share the
/// one `game_character` row the caller also writes directly, so no copper is lost either way.
/// Note: the residual post-import window is the ONE place a cross-database delta would still
/// need a destination-side replay hop, and that hop is #19's (it owns the gateway leg that would
/// carry it). Folding unconditionally here means the final blob is already correct when #19 wires
/// `finish_transfer` to ship residual deltas — no call-site edit at that point.
pub(crate) fn defer_money_delta(ctx: &ReducerContext, character_guid: u64, amount: u32) -> bool {
    let outs = ctx.db.game_transfer_out();
    let Some(mut row) = outs.by_character().filter(character_guid).next() else {
        return false;
    };
    match fold_money_delta(&row.blob, amount) {
        Ok(blob) => {
            row.blob = blob;
            let transfer_id = row.transfer_id;
            outs.transfer_id().update(row);
            log::info!(
                "defer_money_delta: {amount} copper folded into transfer {transfer_id}'s blob for \
                 in-transit character {character_guid}"
            );
        }
        // A corrupt blob is already fatal at `import_character`; refusing to write here keeps the
        // escrow byte-identical so that failure stays the single, loud one.
        Err(e) => log::error!(
            "defer_money_delta: character {character_guid} in transit but its blob is unreadable \
             ({e}) — {amount} copper NOT deferred"
        ),
    }
    true
}

/// The pure half of [`defer_money_delta`]: add `amount` copper to a serialized [`ExportBlob`].
/// Saturating, mirroring every other purse write in the module. Pure, so the fold is unit-testable
/// without a `ReducerContext`.
pub(crate) fn fold_money_delta(blob: &[u8], amount: u32) -> Result<Vec<u8>, String> {
    let mut decoded: ExportBlob = spacetimedb::sats::bsatn::from_slice(blob)
        .map_err(|e| format!("corrupt export blob: {e}"))?;
    decoded.money = decoded.money.saturating_add(amount);
    spacetimedb::sats::bsatn::to_vec(&decoded).map_err(|e| format!("cannot reserialize blob: {e}"))
}

fn ledger(ctx: &ReducerContext, transfer_id: u64) -> (Option<TransferOut>, Option<TransferIn>) {
    (
        ctx.db.game_transfer_out().transfer_id().find(transfer_id),
        ctx.db.game_transfer_in().transfer_id().find(transfer_id),
    )
}

// ===========================================================================================
//  Reducers
// ===========================================================================================

/// **Step 1 — freeze + serialize + delete the live entity, in ONE transaction.**
///
/// Operator-gated: this is orchestration machinery the gateway drives, never a client action.
/// Idempotent on `transfer_id` — a driver that crashed without learning whether its call landed
/// simply calls again.
#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn begin_transfer(
    ctx: &ReducerContext,
    transfer_id: u64,
    character_guid: u64,
    dest_map_id: u32,
    dest_instance_id: u64,
    dest_x: f32,
    dest_y: f32,
    dest_z: f32,
    dest_o: f32,
    cross_database: bool,
) -> Result<(), String> {
    require_operator(ctx)?;
    if transfer_id == 0 {
        return Err("transfer_id 0 is reserved (it is the \"no transfer\" sentinel)".to_string());
    }
    let (out, inb) = ledger(ctx, transfer_id);
    let chars = ctx.db.game_character();
    let character = chars.guid().find(character_guid);
    // Whose escrow is filed under this id, if any.
    let escrowed_guid = escrowed_guid(
        out.as_ref().map(|o| o.character_guid),
        inb.as_ref().map(|i| i.character_guid),
    );

    let plan = plan_begin(
        escrowed_guid,
        character_guid,
        character.is_some(),
        is_in_transit(ctx, character_guid),
    );
    match plan {
        BeginPlan::Replay => return Ok(()),
        BeginPlan::NoSource => return Err(format!("no such character: {character_guid}")),
        BeginPlan::IdCollision => {
            return Err(format!(
                "transfer {transfer_id} is already escrowed for character {} — refusing to reuse the \
                 id for character {character_guid}",
                escrowed_guid.expect("IdCollision implies an escrow exists")
            ));
        }
        BeginPlan::AlreadyInTransit => {
            return Err(format!("character {character_guid} is already in transit"));
        }
        BeginPlan::Escrow => {}
    }
    let character = character.expect("BeginPlan::Escrow implies the character row exists");

    // Persist the live entity into the durable character row FIRST: it is the source copy a
    // rollback restores, so anything the entity is holding (position, vitals, coin, XP) must be in
    // it before the entity row goes away. Then delete the entity — that single delete is what makes
    // the character invisible to every targeting, aggro, threat and AOI-relay gate in the module,
    // because those all resolve through `game_world_entity`.
    //
    // `set_offline: false` — the CROSS-MAP TELEPORT precedent (`world::teleport_player`), not the
    // logout one: a shard hop is a loading screen. `true` would force `pending_ghost` false (a free
    // resurrect for a transferring ghost, work-item 226's landmine) and start the rested-XP clock.
    let entities = ctx.db.game_world_entity();
    let live = entities.guid().find(character_guid);
    if let Some(e) = &live {
        crate::world::persist_entity(ctx, e, false);
        entities.guid().delete(character_guid);
    }
    // Re-read: `persist_entity` just rewrote the row we are about to serialize.
    let character = chars.guid().find(character_guid).unwrap_or(character);

    // The ROWS (issue #19). Serialized here, in the same transaction that froze the character, so
    // the payload is a consistent snapshot: nothing can write a manifest table between the freeze
    // and the export (the in-transit fence covers every writer bar the audited exceptions in this
    // file's verdict table).
    let payload = export_rows(ctx, character_guid);
    let dest = Destination {
        map_id: dest_map_id,
        instance_id: dest_instance_id,
        x: dest_x,
        y: dest_y,
        z: dest_z,
        o: dest_o,
    };
    let blob = build_export_blob(transfer_id, &character, dest, payload)?;
    let blob = spacetimedb::sats::bsatn::to_vec(&blob)
        .map_err(|e| format!("transfer {transfer_id}: cannot serialize export blob: {e}"))?;

    ctx.db.game_transfer_out().insert(TransferOut {
        transfer_id,
        character_guid,
        dest_map_id,
        dest_instance_id,
        dest_x,
        dest_y,
        dest_z,
        dest_o,
        blob,
        created_micros: ctx.timestamp.to_micros_since_unix_epoch(),
        cross_database,
    });

    // Deliberate simplification: arm the reaper lazily, here, instead of in `seed.rs`'s `init`.
    // `init` only runs on a FRESH database, so a schedule row added there would never arm the
    // already-published live DB (it would need a `debug_rearm_*` ritual — see
    // docs/danger-zones.md §3). First transfer arms it; idempotent because the table holds at most
    // one row.
    let sched = ctx.db.game_transfer_reaper_schedule();
    if sched.iter().next().is_none() {
        sched.insert(TransferReaperSchedule {
            scheduled_id: 0,
            scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
                TRANSFER_REAP_INTERVAL_MICROS as i64,
            )),
        });
    }

    log::info!(
        "begin_transfer: {transfer_id} escrowed character {character_guid} -> map {dest_map_id} instance {dest_instance_id}"
    );
    Ok(())
}

/// **Step 2 — commit the arrival copy. Idempotent on `transfer_id` (unique PK).**
///
/// Same-database tracer: the escrowed blob is read from the local out-row. Cross-database (issue
/// #19) the gateway ships the blob and this reducer takes it as an argument — the ONLY difference,
/// which is why the state machine is provable now.
#[reducer]
pub fn import_character(ctx: &ReducerContext, transfer_id: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let (out, inb) = ledger(ctx, transfer_id);
    match plan_import(out.is_some(), inb.is_some()) {
        ImportPlan::Replay => return Ok(()),
        ImportPlan::NoEscrow => {
            return Err(format!("transfer {transfer_id}: no escrow to import"));
        }
        ImportPlan::Apply => {}
    }
    let out = out.expect("ImportPlan::Apply implies the out-row exists");

    let blob: ExportBlob = spacetimedb::sats::bsatn::from_slice(&out.blob)
        .map_err(|e| format!("transfer {transfer_id}: corrupt export blob: {e}"))?;
    // Schema-drift guard: the destination compares the arriving manifest against its OWN build. A
    // shard running a different character-owned table set would otherwise silently drop data.
    if blob.manifest != manifest() {
        return Err(format!(
            "transfer {transfer_id}: manifest mismatch — source exported {} character-owned tables, \
             this shard knows {}",
            blob.manifest.len(),
            manifest().len()
        ));
    }

    // Apply the arrival: re-partition the durable character row onto the destination. The character
    // stays frozen (the in-row is now also an in-transit fence) until `finish_transfer` releases it.
    //
    // The missing-row arm is a HARD ERROR, never a silent skip: writing the in-row while no
    // destination copy materialised is the one way to reach zero durable copies. The in-row is what
    // licenses `finish_transfer` (and the reaper's roll-forward) to clear the escrow, so a silent
    // skip would settle the transfer with nothing on either side — exactly the loss this ticket
    // exists to make unreachable. Returning `Err` aborts the whole transaction, so the in-row below
    // is never committed and the escrow stays recoverable (the reaper rolls it BACK).
    //
    // Note: same-database, "materialise the destination copy" IS this re-partition — both
    // partitions share `game_character`, so there is no second row to create and the guid/unique-name
    // constraints could not admit one, and the row is guaranteed present because `begin_transfer`
    // refused without it and `cascade_delete_character` sweeps the escrow rows in the same
    // transaction it deletes the character. Cross-database this is where the blob's rows are inserted
    // into the destination's own `game_character` + character-owned tables — and there the guard
    // stops being a formality, because the destination genuinely has no row until the insert lands.
    let chars = ctx.db.game_character();
    let Some(mut c) = chars.guid().find(out.character_guid) else {
        return Err(format!(
            "transfer {transfer_id}: character {} has no durable row at the destination — refusing \
             to file an in-row that would license deleting the source copy",
            out.character_guid
        ));
    };
    c.map_id = out.dest_map_id;
    c.pending_instance_id = out.dest_instance_id;
    c.x = out.dest_x;
    c.y = out.dest_y;
    c.z = out.dest_z;
    c.orientation = out.dest_o;
    chars.guid().update(c);

    ctx.db.game_transfer_in().insert(TransferIn {
        transfer_id,
        character_guid: out.character_guid,
        blob: out.blob.clone(),
        created_micros: ctx.timestamp.to_micros_since_unix_epoch(),
    });

    log::info!(
        "import_character: {transfer_id} committed character {} at map {} instance {}",
        out.character_guid,
        out.dest_map_id,
        out.dest_instance_id
    );
    Ok(())
}

/// **Step 2, CROSS-DATABASE — commit the arrival copy from a blob the gateway carried here.**
///
/// The same step as [`import_character`], for the deployment where the source's out-row is on
/// ANOTHER database and this one has never heard of the transfer. Two consequences:
///
/// - The escrow evidence is the BLOB, not a local out-row (`plan_import`'s `has_out` can only ever
///   be false here). That is not a weakening: the blob is only produced by `begin_transfer`, and
///   this reducer is operator-gated, so it is exactly as forgeable as calling `begin_transfer`.
/// - "Materialise the destination copy" is a real INSERT — of `game_character` and of every
///   manifest table's rows — instead of the same-database re-partition.
///
/// Idempotent on `transfer_id` (the in-row PK), like its same-database twin: a driver that crashed
/// without learning whether its call landed simply calls again.
#[reducer]
pub fn import_character_blob(
    ctx: &ReducerContext,
    transfer_id: u64,
    blob: Vec<u8>,
) -> Result<(), String> {
    require_operator(ctx)?;
    apply_import_blob(&mut CtxImportSink { ctx }, transfer_id, blob)
}

/// The destination database, reduced to exactly what [`apply_import_blob`] touches.
///
/// This trait IS the seam of issue #37. `ReducerContext` cannot be constructed in a unit test, so
/// before this existed nothing in the crate could execute the import — its guards were pinned by
/// source scans that matched their own text, and 17 of 21 mutations against them left the suite
/// green. `CtxImportSink` is the production adapter (a one-line body per method, all real table
/// access); `transport_harness::FakeDb` is the test one, so the harness runs THIS body rather than
/// a model of it.
///
/// Deliberate simplification: eleven methods, no associated types, no lifetimes on the trait. The
/// one thing it does NOT model is SpacetimeDB's transaction rollback — a fake sink keeps whatever
/// an aborted import wrote. Every harness test therefore asserts on the IN-ROW (which the real rollback would also
/// remove) rather than on the absence of partial writes.
pub(crate) trait ImportSink {
    /// `game_transfer_in` under this id, if any.
    fn in_row(&self, transfer_id: u64) -> Option<TransferIn>;
    /// Is there a LIVE `game_world_entity` for this guid here?
    fn has_live_entity(&self, guid: u64) -> bool;
    /// Is there a durable `game_character` row for this guid here?
    fn has_character(&self, guid: u64) -> bool;
    fn cascade_delete_character(&mut self, guid: u64);
    fn insert_character(&mut self, c: crate::character::Character);
    /// The payload half — [`import_rows`] against this database's transport registry.
    fn import_rows(&mut self, guid: u64, payload: &[TableRows]) -> Result<(), String>;
    fn ensure_shadow_account(&mut self, account_id: u64);
    fn file_in_row(&mut self, row: TransferIn);
    fn now_micros(&self) -> i64;
    /// Ratchet THIS database's guid high-water mark up to at least `guid` (issue #59 AC#3) — so a
    /// locally created character can never later collide with one this database received by import.
    /// Unconditional, same as `world::cascade_delete_character`'s call through this same method —
    /// the GATE on whether an arriving guid is even allowed to reach here lives in
    /// `apply_import_blob` (issue #237), not in this method, precisely so `CtxImportSink` and
    /// `FakeDb` can never diverge on it (see `own_guid_range`'s doc comment).
    fn bump_guid_high_water(&mut self, guid: u64);
    /// THIS database's own `game_guid_range` (`(base, size)`), if one is installed. A FACT query,
    /// not a decision — issue #237's `apply_import_blob` is what turns it into "should this
    /// arrival ratchet the allocator" via `auth::in_guid_range`, so both sinks share the one
    /// decision instead of each re-implementing it. Putting the gate here instead would let a
    /// `FakeDb` that forgot to reimplement it silently validate a shard-poisoning import as green
    /// — exactly the seam #37's doc comment above warns about.
    fn own_guid_range(&self) -> Option<(u64, u64)>;
}

/// The production [`ImportSink`]: the real `ReducerContext`.
struct CtxImportSink<'a> {
    ctx: &'a ReducerContext,
}

impl ImportSink for CtxImportSink<'_> {
    fn in_row(&self, transfer_id: u64) -> Option<TransferIn> {
        self.ctx
            .db
            .game_transfer_in()
            .transfer_id()
            .find(transfer_id)
    }
    fn has_live_entity(&self, guid: u64) -> bool {
        self.ctx.db.game_world_entity().guid().find(guid).is_some()
    }
    fn has_character(&self, guid: u64) -> bool {
        self.ctx.db.game_character().guid().find(guid).is_some()
    }
    fn cascade_delete_character(&mut self, guid: u64) {
        crate::world::cascade_delete_character(self.ctx, guid);
    }
    fn insert_character(&mut self, c: crate::character::Character) {
        self.ctx.db.game_character().insert(c);
    }
    fn import_rows(&mut self, guid: u64, payload: &[TableRows]) -> Result<(), String> {
        import_rows(self.ctx, guid, payload)
    }
    fn ensure_shadow_account(&mut self, account_id: u64) {
        crate::auth::ensure_shadow_account(self.ctx, account_id);
    }
    fn file_in_row(&mut self, row: TransferIn) {
        self.ctx.db.game_transfer_in().insert(row);
    }
    fn now_micros(&self) -> i64 {
        self.ctx.timestamp.to_micros_since_unix_epoch()
    }
    fn bump_guid_high_water(&mut self, guid: u64) {
        crate::auth::bump_guid_high_water(self.ctx, guid);
    }
    fn own_guid_range(&self) -> Option<(u64, u64)> {
        self.ctx
            .db
            .game_guid_range()
            .id()
            .find(0)
            .map(|r| (r.base, r.size))
    }
}

/// The whole of `import_character_blob` bar the operator gate — every guard, in order, over an
/// [`ImportSink`]. Executed for real by `transport_harness` (issue #37).
pub(crate) fn apply_import_blob<S: ImportSink>(
    sink: &mut S,
    transfer_id: u64,
    blob: Vec<u8>,
) -> Result<(), String> {
    if transfer_id == 0 {
        return Err("transfer_id 0 is reserved (it is the \"no transfer\" sentinel)".to_string());
    }
    if let Some(existing) = sink.in_row(transfer_id) {
        // REPLAY. Guarded by guid so a REUSED transfer id cannot be answered `Ok(())` for a
        // character that was never imported — the destination-side twin of `BeginPlan::IdCollision`,
        // and the same failure it prevents: the driver would go on to finish (i.e. DELETE) a source
        // copy whose destination copy does not exist.
        let decoded: ExportBlob = spacetimedb::sats::bsatn::from_slice(&blob)
            .map_err(|e| format!("transfer {transfer_id}: corrupt export blob: {e}"))?;
        if existing.character_guid != decoded.character_guid {
            return Err(format!(
                "transfer {transfer_id} is already imported for character {} — refusing to reuse \
                 the id for character {}",
                existing.character_guid, decoded.character_guid
            ));
        }
        return Ok(());
    }

    let decoded: ExportBlob = spacetimedb::sats::bsatn::from_slice(&blob)
        .map_err(|e| format!("transfer {transfer_id}: corrupt export blob: {e}"))?;
    if decoded.transfer_id != transfer_id {
        return Err(format!(
            "transfer {transfer_id}: blob names transfer {} — refusing to import a blob filed \
             under a different id",
            decoded.transfer_id
        ));
    }
    // Schema-drift guard, identical to `import_character`'s: a shard running a different
    // character-owned table set would otherwise silently drop data.
    if decoded.manifest != manifest() {
        return Err(format!(
            "transfer {transfer_id}: manifest mismatch — source exported {} character-owned tables, \
             this shard knows {}",
            decoded.manifest.len(),
            manifest().len()
        ));
    }
    let guid = decoded.character_guid;

    // A live entity for this guid here means the character is ALREADY resident on this database —
    // importing on top of it would be the dual-liveness dupe wearing a different hat. Refuse.
    if sink.has_live_entity(guid) {
        return Err(format!(
            "transfer {transfer_id}: character {guid} already has a LIVE entity on this shard — \
             refusing to import a second copy"
        ));
    }

    // Materialise `game_character`. Decoded with THIS build's `Character` type, then overwritten
    // with the destination coordinates and the money the blob carries (which is the escrowed value
    // PLUS any `defer_money_delta` folded in after the freeze — issue #30's residual, replayed here
    // because this is the hop that reads the blob at the destination).
    let mut c: crate::character::Character =
        spacetimedb::sats::bsatn::from_slice(&decoded.character_row).map_err(|e| {
            format!("transfer {transfer_id}: cannot decode the arriving character row: {e}")
        })?;
    if c.guid != guid {
        return Err(format!(
            "transfer {transfer_id}: the arriving character row is guid {} but the blob names {guid}",
            c.guid
        ));
    }
    c.map_id = decoded.dest_map_id;
    c.pending_instance_id = decoded.dest_instance_id;
    c.x = decoded.dest_x;
    c.y = decoded.dest_y;
    c.z = decoded.dest_z;
    c.orientation = decoded.dest_o;
    c.money = decoded.money;
    let account_id = c.account_id;
    // A stale copy from an earlier, half-finished hop in the OTHER direction gets its owned rows
    // wiped before the re-insert, so the arriving payload never lands on top of rows that would
    // PK-collide (or worse, survive as a second, older loadout).
    //
    // UNCONDITIONAL (issue #83): this used to run only `if sink.has_character(guid)`, which wipes
    // every owned table when `game_character` itself survived — but says nothing about a table
    // that has this guid's rows WITHOUT a `game_character` row to key off. That combination is
    // exactly the one no transfer-id witness can see (the witness is a fact about
    // `game_transfer_in`, not about `game_item_instance`), and item rows are guid-namespaced to
    // the OWNER (`item_guid_for`), so a collision there can only ever be this SAME character's own
    // leftover. Sweeping every time makes the destination table set an idempotent function of
    // "what belongs to this guid right now" rather than depending on `game_character` also having
    // survived intact — cheap when there is nothing to sweep (every delete-by-key on an absent row
    // is a no-op) and the only thing standing between a genuinely orphaned row and a hard panic on
    // replay. See `orphaned_owned_rows_with_no_character_row_are_wiped_before_a_fresh_import_lands`.
    sink.cascade_delete_character(guid);
    sink.insert_character(c);
    // Issue #59 AC#3: ratchet this database's guid allocator past `guid` NOW, in the same
    // transaction as the materialisation — so a `create_character` racing this import (or run any
    // time after) can never hand the same guid to a brand-new local character.
    //
    // ONLY when `guid` is actually inside THIS database's own range (issue #237). Ranges are
    // disjoint by construction (#108) — a foreign-range arrival (e.g. a world-1 character crossing
    // into core) can never collide with anything core mints, so ratcheting past it protects
    // nothing and instead walks core's own mark toward, and eventually past, its own range end.
    // That is exactly what was hit live: `lyracore`'s high_water sat at its range end with
    // zero local characters above it, and every local `create_character` failed
    // GUID_RANGE_EXHAUSTED. A local-range arrival still ratchets, unchanged from #59 AC#3.
    if crate::auth::in_guid_range(sink.own_guid_range(), guid) {
        sink.bump_guid_high_water(guid);
    }
    sink.import_rows(guid, &decoded.payload)?;

    // The destination has no `game_account` row (accounts are realm-scoped and live on the default
    // database until realm-core, #22). `player_login` resolves the caller through
    // `account_by_identity`, so without one the arriving player cannot log in at all.
    // Deliberate simplification: a SHADOW account — id + a synthetic username, no credentials. The
    // gateway rebinds `identity` through `establish_session` on this shard at every world entry, which is the only
    // field this row exists to hold. Upgrade path: realm-core owns accounts (#12 Phase B) and this
    // whole arm goes away. Never a login credential: SRP runs on the logon tier against the realm
    // database, never here.
    sink.ensure_shadow_account(account_id);

    // The model says `Apply` ⇒ the destination copy is durable. PROVE it before filing the in-row:
    // that row is what licenses `finish_transfer` (and the reaper's roll-forward) to destroy the
    // source copy, so filing it against an apply that did nothing settles the transfer with zero
    // durable copies — no durable row at the destination is the one unrecoverable outcome.
    if !sink.has_character(guid) {
        return Err(format!(
            "transfer {transfer_id}: character {guid} has no durable row at the destination after \
             the import — refusing to file an in-row that would license deleting the source copy"
        ));
    }

    let created_micros = sink.now_micros();
    sink.file_in_row(TransferIn {
        transfer_id,
        character_guid: guid,
        blob,
        created_micros,
    });
    log::info!(
        "import_character_blob: {transfer_id} materialised character {guid} ({} rows across {} \
         tables) at map {} instance {}",
        decoded.payload.iter().map(|t| t.rows.len()).sum::<usize>(),
        decoded.payload.len(),
        decoded.dest_map_id,
        decoded.dest_instance_id
    );
    Ok(())
}

/// **Step 2b, CROSS-DATABASE — the driver attests that the arrival copy is durable.**
///
/// Same-database, `import_character` files the in-row itself: source and destination are the same
/// database, so "the destination copy exists" is a fact the source can READ. Cross-database it is
/// not — the arrival copy is on another node — and `finish_transfer` (the only step that destroys
/// the source copy) refuses without an in-row, exactly as it should.
///
/// So the gateway, which is the one component that can see both databases, writes the in-row HERE
/// after `import_character_blob` has COMMITTED at the destination. Everything downstream —
/// `plan_finish`, `recovery`, the whole #16 crash matrix — then works unchanged, because the in-row
/// still means precisely what it meant: *the destination copy is durable*.
///
/// **The obligation this moves out of the module**: nothing here can check that the import really
/// happened. Calling this before a successful import would license destroying the source copy with
/// no destination copy — the one unrecoverable outcome. That ordering is the GATEWAY's, enforced in
/// `world::transfer::run_transfer` and pinned by
/// `a_confirm_before_a_successful_import_is_never_issued`; the module's guard is only that an
/// unattested cross-database escrow can never be rolled back (see [`reap_transfers`]), so a driver
/// that dies before attesting leaves a recoverable frozen character rather than a lost one.
#[reducer]
pub fn confirm_import(ctx: &ReducerContext, transfer_id: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let (out, inb) = ledger(ctx, transfer_id);
    let Some(out) = out else {
        return Err(format!(
            "transfer {transfer_id}: nothing escrowed here to confirm an import for"
        ));
    };
    if !out.cross_database {
        return Err(format!(
            "transfer {transfer_id}: same-database transfers file their own in-row in \
             import_character — refusing to forge an attestation"
        ));
    }
    if inb.is_some() {
        return Ok(()); // already attested — replay-safe like every other step
    }
    ctx.db.game_transfer_in().insert(TransferIn {
        transfer_id,
        character_guid: out.character_guid,
        blob: out.blob.clone(),
        created_micros: ctx.timestamp.to_micros_since_unix_epoch(),
    });
    log::info!(
        "confirm_import: {transfer_id} — destination copy of character {} attested durable; the \
         source copy may now be released",
        out.character_guid
    );
    Ok(())
}

/// **Step 4, CROSS-DATABASE — release the arrival copy.** Deletes the DESTINATION's in-row, which
/// is the last thing fencing the imported character (`is_in_transit` reads either escrow row).
///
/// Cross-database `finish_transfer` runs on the SOURCE and can only clear the source's own rows;
/// the destination's in-row is on the other database and needs its own call. Order matters and is
/// the whole of delete-last: the gateway calls `finish_transfer` on the source FIRST (destroying
/// the source copy), and only then this — so between them the character is durable on both sides
/// but LIVE on neither, never the reverse.
///
/// Refuses while a local out-row exists: that would mean this database is also a SOURCE for the
/// same id, i.e. the same-database deployment, where `finish_transfer` is the correct call and
/// dropping the in-row alone would strand the out-row and unfreeze nothing.
#[reducer]
pub fn release_transfer(ctx: &ReducerContext, transfer_id: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let (out, inb) = ledger(ctx, transfer_id);
    if out.is_some() {
        return Err(format!(
            "transfer {transfer_id}: this database holds the SOURCE out-row — call finish_transfer, \
             not release_transfer (releasing alone would leave the source's claim behind)"
        ));
    }
    let Some(row) = inb else {
        return Ok(()); // already released — replay-safe, like every other step
    };
    ctx.db.game_transfer_in().transfer_id().delete(transfer_id);
    log::info!(
        "release_transfer: {transfer_id} — character {} is live at the destination",
        row.character_guid
    );
    Ok(())
}

/// **Step 3 — delete-last.** Destroys the source copy and clears both escrow rows, releasing the
/// character at the destination. REFUSES while the in-row is absent: that guard is what makes
/// "zero durable copies" unreachable.
#[reducer]
pub fn finish_transfer(ctx: &ReducerContext, transfer_id: u64) -> Result<(), String> {
    require_operator(ctx)?;
    let (out, inb) = ledger(ctx, transfer_id);
    match plan_finish(out.is_some(), inb.is_some()) {
        // NO-OP, and it must SAY so (#99). The gateway's drive calls `finish_transfer` on a specific
        // database and reads `Ok(())` as "the source released the character", then proceeds to
        // publish_shard_index / release_transfer / evict. Returning success silently here makes a
        // finish driven against the WRONG database indistinguishable from one that did the work —
        // which is exactly the question anyone debugging a stranded copy is asking. Cheap to log:
        // the fan-out makes this a normal, expected occurrence, not an error, so it is `info!`.
        FinishPlan::AlreadyDone => {
            spacetimedb::log::info!(
                "finish_transfer: {transfer_id} — no escrow on this database, nothing to finish \
                 (already settled here, or this call reached the wrong shard)"
            );
            Ok(())
        }
        FinishPlan::NotImported => Err(format!(
            "transfer {transfer_id}: not imported — refusing to release the source copy"
        )),
        FinishPlan::Complete => {
            do_finish(ctx, transfer_id);
            Ok(())
        }
    }
}

/// The SOURCE database, reduced to exactly what [`apply_finish`] touches.
///
/// The same seam as [`ImportSink`], for the same reason (issue #37, extended by #34).
/// `ReducerContext` cannot be constructed in a unit test, so `do_finish`'s ORDER — detach, then
/// cascade, then write the forwarding receipt, then delete the escrow — was pinned only by source
/// scans that matched their own text, and the ordering constraint is not cosmetic:
/// `game_character_shard` is itself character-owned, so a cascade that ran after `record_shard`
/// would wipe the receipt it had just written. `CtxFinishSink` is the production adapter (one line
/// per method, all real table access); `transport_harness::FakeDb` is the test one, so the harness
/// runs THIS body rather than a model of it.
pub(crate) trait FinishSink {
    /// `game_transfer_out` under this id, if any.
    fn out_row(&self, transfer_id: u64) -> Option<TransferOut>;
    /// `group::detach_for_transfer` — raw membership removal: no leader transfer, no disband.
    fn detach_for_transfer(&mut self, guid: u64);
    fn cascade_delete_character(&mut self, guid: u64);
    /// `realm_core::record_shard` — this database's forwarding receipt for the character.
    fn record_shard(&mut self, guid: u64, map_id: u32, instance_id: u64);
    /// Delete BOTH escrow rows under this id. The out-row is the source's claim, deleted last.
    fn clear_escrow(&mut self, transfer_id: u64);
}

/// The production [`FinishSink`]: the real `ReducerContext`.
struct CtxFinishSink<'a> {
    ctx: &'a ReducerContext,
}

impl FinishSink for CtxFinishSink<'_> {
    fn out_row(&self, transfer_id: u64) -> Option<TransferOut> {
        self.ctx
            .db
            .game_transfer_out()
            .transfer_id()
            .find(transfer_id)
    }
    fn detach_for_transfer(&mut self, guid: u64) {
        crate::group::detach_for_transfer(self.ctx, guid);
    }
    fn cascade_delete_character(&mut self, guid: u64) {
        crate::world::cascade_delete_character(self.ctx, guid);
    }
    fn record_shard(&mut self, guid: u64, map_id: u32, instance_id: u64) {
        crate::realm_core::record_shard(self.ctx, guid, map_id, instance_id);
    }
    fn clear_escrow(&mut self, transfer_id: u64) {
        self.ctx
            .db
            .game_transfer_in()
            .transfer_id()
            .delete(transfer_id);
        self.ctx
            .db
            .game_transfer_out()
            .transfer_id()
            .delete(transfer_id);
    }
}

/// The delete-last body, shared by `finish_transfer` and the reaper's roll-forward.
fn do_finish(ctx: &ReducerContext, transfer_id: u64) {
    apply_finish(&mut CtxFinishSink { ctx }, transfer_id)
}

/// The whole of [`do_finish`], written against a [`FinishSink`]. Executed for real — order and all
/// — by `transport_harness` (issue #34).
pub(crate) fn apply_finish<S: FinishSink>(sink: &mut S, transfer_id: u64) {
    let mut cascaded = false;
    if let Some(out) = sink.out_row(transfer_id) {
        // CROSS-DATABASE (issue #19): the source shard's teardown is `cascade_delete_character` —
        // the existing character-owned delete sweep, the exact counterpart of the payload this blob
        // exported. Skipping it leaves a full second copy of the character on the source, which is a
        // dupe the moment anything logs it in. Same-database there is nothing to cascade: the two
        // partitions share `game_character`, so destroying the "source copy" would destroy the
        // destination copy too. What IS deleted last, in both deployments, is the source's escrow
        // row — the source's claim on the character.
        cascaded = out.cross_database;
        if out.cross_database {
            // The group MIRROR (issue #19, AC#4) — done HERE, before the cascade, rather than inside
            // `sweep_delete_game_group_member`, so it depends on no sweep ordering. A shard hop is not
            // a departure: `remove_member`'s leader-transfer/disband would tear the party down on the
            // source the instant the FIRST member hops, and the second member would then arrive with no
            // membership at all — the party would not survive the boundary for the one case AC#4 is
            // about. Detaching raw leaves the `game_group` row (and any members still on this shard)
            // intact, so every member that follows carries the SAME group_id and the destination's
            // import re-forms one party.
            sink.detach_for_transfer(out.character_guid);
            sink.cascade_delete_character(out.character_guid);
        }
        // #20 AC#3: the character→shard index entry is written HERE, inside the same transaction that
        // releases the escrow, from the out-row's own destination fields — so "the escrow settled" and
        // "the directory says where it settled" can never disagree on this database.
        //
        // Cross-database, this is only HALF the index (issue #34). There is no transaction spanning
        // two SpacetimeDB databases, so realm-core's copy — the one `home_shard` actually reads —
        // cannot be written from here at all. What the gateway does instead is REPLICATE this row:
        // `world::transfer::run_transfer` publishes the same `(guid, map, instance)` to realm-core
        // as a required step of the drive, strictly AFTER `finish_transfer` returned Ok, so it can
        // never name a destination for a transfer that did not settle. That is the strongest form
        // available across two databases. It is NOT a guarantee: if the gateway dies (or the publish
        // fails) between `finish_transfer` and step 5b, realm-core's copy keeps naming the old
        // shard, and the recovery path does not re-drive `run_transfer`, so nothing republishes it.
        // The index is therefore still specified as a HINT the gateway CONFIRMS by probing rather
        // than trusts, and the login self-heal is still the terminal fallback — except that the
        // self-heal is currently unreachable in production (the gateway's `settle_home_shard`
        // overrides the `home_shard` that hosts it and locates the character by scanning instead).
        // Nothing in production reads this directory yet either, so the two cancel out today; both
        // have to be fixed together before anything routes on it. See
        // `gateway/src/realm_core.rs::publish_shard_index`.
        //
        // MERGE ORDER (#19 × #20): this runs AFTER the cascade, never before. `game_character_shard`
        // is itself character-owned (`realm_core.rs`'s delete sweep), so a cascade that ran second
        // would wipe the forwarding receipt the source shard is supposed to keep — the receipt that
        // lets a gateway whose realm-core is unconfigured (`realm_core()` == the default shard) still
        // find a character that moved off it.
        sink.record_shard(out.character_guid, out.dest_map_id, out.dest_instance_id);
    }
    sink.clear_escrow(transfer_id);
    // Name what actually happened (#99): "complete" alone read identically whether the source copy
    // was cascade-deleted or the transfer was same-database (where there is no source copy to
    // destroy). "finish ran" and "the source copy is gone" are different facts and were one line.
    log::info!(
        "finish_transfer: {transfer_id} complete — escrow cleared, source copy {}",
        if cascaded {
            "DELETED (cross-database)"
        } else {
            "kept (same-database transfer)"
        }
    );
}

/// The reaper: recovers any transfer whose driver never came back. Rolls FORWARD past the point of
/// no return (the destination copy is durable), rolls BACK before it, and holds otherwise.
#[reducer]
pub fn reap_transfers(ctx: &ReducerContext, _schedule: TransferReaperSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let stale: Vec<(u64, u64, i64, bool)> = ctx
        .db
        .game_transfer_out()
        .iter()
        .map(|o| {
            (
                o.transfer_id,
                o.character_guid,
                now - o.created_micros,
                o.cross_database,
            )
        })
        .collect();
    for (transfer_id, character_guid, age, cross_database) in stale {
        let has_in = ctx
            .db
            .game_transfer_in()
            .transfer_id()
            .find(transfer_id)
            .is_some();
        // CROSS-DATABASE (issue #19): the destination's arrival copy is on ANOTHER database, so an
        // ABSENT in-row here does not mean "not imported" — it means "not yet ATTESTED"
        // ([`confirm_import`]). Reading it as `Some(false)` would roll the escrow BACK past the
        // point of no return, unfreezing the source copy while the destination copy is live: a
        // DUPLICATED character. `None` is the documented "destination unconsultable" answer; it
        // holds the escrow, which is recoverable, and the gateway (the one component that can reach
        // both databases) re-drives it forward at the player's next world entry.
        //
        // A PRESENT in-row is the attestation, and it means exactly what it means same-database —
        // the destination copy is durable — so roll-forward is correct and the reaper can complete
        // a transfer whose driver died between `confirm_import` and `finish_transfer`.
        let imported = if cross_database && !has_in {
            None
        } else {
            Some(has_in)
        };
        match recovery(true, imported, age) {
            Recovery::Hold => {}
            Recovery::Rollback => {
                ctx.db.game_transfer_out().transfer_id().delete(transfer_id);
                log::warn!(
                    "reap_transfers: {transfer_id} abandoned before import — rolled BACK, character \
                     {character_guid} unfrozen at the source"
                );
            }
            Recovery::RollForward => {
                do_finish(ctx, transfer_id);
                log::warn!(
                    "reap_transfers: {transfer_id} abandoned after import — rolled FORWARD, character \
                     {character_guid} released at the destination"
                );
            }
        }
    }
}

// ===========================================================================================
//  Tests — the crash matrix
// ===========================================================================================

#[cfg(test)]
mod tests {
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
    // in lib.rs recognizes), so a warm handoff silently dropped every buff, DoT, HoT and the
    // Rogue's Stealth presence on the source database — the destination simply never got them.
    // -------------------------------------------------------------------------------------

    /// Mutation-check per the playbook (§8): reverting `sweep_delete_game_aura` /
    /// `sweep_transfer_game_aura` back out (or repointing the transfer arm at `not_transported`
    /// without adding `game_aura` to `NOT_TRANSPORTED`) makes THIS assertion fail — `game_aura`
    /// would no longer be in the generated manifest, or would no longer be marked hot. The two
    /// generic ratchets above (`every_manifest_table_can_cross_a_database_boundary`,
    /// `not_transported_is_an_allowlisted_decision_not_a_silent_drop`) only prove a table THAT IS
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
        let expires_at =
            Timestamp::from_micros_since_unix_epoch(1_000_000_000 + 10 * 60 * 1_000_000);
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

    /// The transfer arm's `id` field must be reset before insert — `Aura.id` is `#[auto_inc]`, a
    /// surrogate key local to ONE database, so carrying the source's id verbatim either collides
    /// with an unrelated row the destination already minted under that same id, or (same-value
    /// coincidence aside) simply means nothing there. Source-scan, mirroring
    /// `sweep_transfer_game_character_talent`'s identical reset of its own auto_inc PK — the
    /// production body cannot run in a unit test (no `ReducerContext`), so the shape IS the
    /// assertion, exactly like every other transport-arm test in this file.
    #[test]
    fn aura_transfer_arm_remints_the_auto_inc_id_before_insert() {
        // NOTE: deliberately NOT the full `character_owned!(transfer, fn sweep_transfer_...`
        // marker text — `transport_arm_bodies()` (below) scans every file in the crate for that
        // exact literal substring and does not exclude string literals (only `//` comments), so
        // spelling it out here would make THIS test's own source register as a spurious transport
        // arm. `fn sweep_transfer_game_aura(` alone is unique enough and matches the anchor style
        // every other source-scan test in this file already uses (a bare `fn <name>(`).
        let body = crate::test_scan::code_of(
            include_str!("spell/tables.rs"),
            "fn sweep_transfer_game_aura(",
        );
        assert!(
            body.contains("row.id = 0"),
            "sweep_transfer_game_aura no longer resets `id` before inserting the arriving row — an \
             imported aura would either collide with an existing auto_inc id on the destination or \
             silently masquerade as one that already means something there. Body was:\n{body}"
        );
    }

    #[test]
    fn export_blob_round_trips_through_bsatn() {
        let blob = ExportBlob {
            transfer_id: 7,
            character_guid: 42,
            name: "Tracer".to_string(),
            level: 12,
            map_id: 0,
            instance_id: 3,
            x: 1.5,
            y: -2.5,
            z: 3.25,
            o: 0.75,
            health: 300,
            power: 120,
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
        let back: ExportBlob =
            spacetimedb::sats::bsatn::from_slice(&bytes).expect("blob deserializes");
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
            "a database holding only an unreleased arrival row must NOT read as an unused id, or \
             the character is refused a transfer off the shard it is stuck on, permanently"
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

    #[test]
    fn the_in_transit_fence_blocks_login_in_every_non_resident_phase() {
        assert!(
            login_allowed(false, false),
            "a resident character may log in"
        );
        assert!(!login_allowed(true, false), "escrowed: frozen");
        assert!(
            !login_allowed(true, true),
            "imported but not finished: still frozen"
        );
        assert!(
            !login_allowed(false, true),
            "a lone in-row still fences (defence in depth)"
        );
    }

    // -------------------------------------------------------------------------------------
    // ENFORCEMENT tripwire: the fence is three lines of code in three files, and the pure model
    // cannot see any of them
    // -------------------------------------------------------------------------------------

    /// Everything above tests the DECISION surface. AC#4 ("no reducer can act on an in-transit
    /// character") is not a decision, it is three call sites — and deleting any of them left the
    /// whole 428-test suite green, because a pure model has no reducers in it. So source-scan them,
    /// the `character_owned_tripwire` pattern in `lib.rs`: a tripwire is the only thing that can
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
    /// call to `gate_in_transit` being routed around, outright.
    #[test]
    fn the_actor_chokepoint_still_calls_the_in_transit_gate() {
        let body = code_of(include_str!("helpers.rs"), "pub fn entity_by_owner(");
        assert!(
            body.contains("is_in_transit") && body.contains("gate_in_transit(candidate, in_transit)"),
            "helpers::entity_by_owner no longer routes its result through gate_in_transit. That \
             call is the ONE gate covering all 60+ player-fired reducers (issue #16, AC#4); without \
             it every one of them can act on a character that is mid-transfer. Body was:\n{body}"
        );
    }

    /// The historical mutant this file is named after, generalised: `entity_by_owner` used to spell
    /// the fence `.next().filter(|e| !is_in_transit(..))`, and dropping the leading `!` — same
    /// identifiers, same order, opposite meaning — made it return ONLY in-transit entities with all
    /// 533 module tests green (issue #64). Moving the sense into `gate_in_transit` closes that exact
    /// mutant, but round-1 of THIS fix left a sibling one standing: negating the call site's
    /// `in_transit` boolean itself (`!candidate.as_ref().is_some_and(..)` instead of
    /// `candidate.as_ref().is_some_and(..)`) still satisfies a scan that only looks for `!` glued
    /// directly onto `is_in_transit`, because the `!` lands on `candidate` instead — a live mutation
    /// verified this: `.contains("!is_in_transit")`-style checks stayed green under it.
    ///
    /// A `.contains()` scan cannot rule out every place a `!` could hide in three lines of glue code
    /// it did not write in full. Exact-shape equality can: this body has NO legitimate `!` anywhere
    /// in it, so the whole thing is pinned verbatim rather than probed for one substring.
    #[test]
    fn entity_by_owner_is_exactly_the_pinned_shape() {
        let want =
            "{ let candidate = ctx.db.game_world_entity().by_owner().filter(&owner).next(); \
                     let in_transit = candidate .as_ref() .is_some_and(|e| \
                     crate::transfer::is_in_transit(ctx, e.guid)); \
                     gate_in_transit(candidate, in_transit) }";
        let want = want.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(
            shape_of(include_str!("helpers.rs"), "pub fn entity_by_owner("),
            want,
            "helpers::entity_by_owner no longer matches the pinned shape — in particular, if a `!` \
             appears ANYWHERE in this body the fence's sense may be inverted with nothing else able \
             to see it (issue #64). If the change is deliberate, update the expected shape here with \
             the same care `transfer.rs`'s production-adapter pins get."
        );
    }

    #[test]
    fn player_login_still_refuses_an_in_transit_character() {
        let body = body_of(include_str!("world.rs"), "pub fn player_login(");
        assert!(
            body.contains("is_in_transit"),
            "world::player_login no longer fences in-transit characters. Login is the one path that \
             can re-materialise a live entity on a shard the character has LEFT — that is the \
             dual-liveness dupe the escrow exists to prevent. Body was:\n{body}"
        );
    }

    #[test]
    fn begin_transfer_still_deletes_the_live_entity_row() {
        let body = body_of(include_str!("transfer.rs"), "pub fn begin_transfer(");
        assert!(
            body.contains("entities.guid().delete(character_guid)"),
            "begin_transfer no longer deletes the live `game_world_entity` row. That single delete \
             is what makes the character invisible to the ~50 hand-rolled map_id/instance_id target \
             gates, the aggro candidate scan, the threat lists and the AOI relay — none of which \
             consult the ledger. Without it, AC#4 holds only for the actor side."
        );
    }

    #[test]
    fn import_character_still_refuses_when_no_destination_copy_materialises() {
        // The model says `ImportPlan::Apply` ⇒ the destination copy is durable. The reducer must
        // MAKE that true: if it files the in-row while the apply silently did nothing, the in-row
        // licenses `finish_transfer` (and the reaper's roll-forward) to clear the escrow, settling
        // the transfer with ZERO durable copies. Model-vs-reality is exactly the gap a pure test
        // cannot see.
        let body = body_of(include_str!("transfer.rs"), "pub fn import_character(");
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

    /// The gate itself. Same shared decision as the actor chokepoint — both `character_by_guid`
    /// and `character_by_name` route through `helpers::gate_in_transit`, so
    /// `gate_in_transit_refuses_an_in_transit_candidate_and_returns_a_normal_one` pins the SENSE of
    /// both at once (issue #64). This scan is what catches the fence being deleted, or its call to
    /// `gate_in_transit` routed around, outright.
    #[test]
    fn the_by_guid_chokepoint_still_calls_the_in_transit_gate() {
        for signature in ["pub fn character_by_guid(", "pub fn character_by_name("] {
            let body = code_of(include_str!("helpers.rs"), signature);
            assert!(
                body.contains("is_in_transit")
                    && body.contains("gate_in_transit(candidate, in_transit)"),
                "helpers::{signature} no longer routes its result through gate_in_transit. That \
                 call is the gate for every reducer that reaches a character by guid or by name \
                 (issue #30); without it the REFUSE verdicts in this file's table are all open. \
                 Body was:\n{body}"
            );
        }
    }

    /// The same historical mutant as the actor chokepoint's sibling test (issue #64), and the same
    /// fix for the same reason a `.contains("!is_in_transit")`-style scan cannot rule out: a `!`
    /// landing on `candidate` instead of on `is_in_transit` itself. Each body has NO legitimate `!`
    /// anywhere in it, so it is pinned verbatim.
    #[test]
    fn the_by_guid_chokepoint_is_exactly_the_pinned_shape() {
        let sites: &[(&str, &str)] = &[
            (
                "pub fn character_by_guid(",
                "{ let candidate = ctx.db.game_character().guid().find(guid); \
                 let in_transit = candidate .as_ref() .is_some_and(|c| \
                 crate::transfer::is_in_transit(ctx, c.guid)); \
                 gate_in_transit(candidate, in_transit) }",
            ),
            (
                "pub fn character_by_name(",
                "{ let candidate = ctx .db .game_character() .iter() .find(|c| \
                 c.name.eq_ignore_ascii_case(name)); \
                 let in_transit = candidate .as_ref() .is_some_and(|c| \
                 crate::transfer::is_in_transit(ctx, c.guid)); \
                 gate_in_transit(candidate, in_transit) }",
            ),
        ];
        for (signature, want) in sites {
            let want = want.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(
                shape_of(include_str!("helpers.rs"), signature),
                want,
                "helpers::{signature} no longer matches the pinned shape — in particular, if a `!` \
                 appears ANYWHERE in this body the fence's sense may be inverted with nothing else \
                 able to see it (issue #64/#30). If the change is deliberate, update the expected \
                 shape here with the same care."
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
        let sites: &[(&str, &str, &str, &str)] = &[
            (
                "auth.rs",
                include_str!("auth.rs"),
                "pub fn delete_character(",
                "delete_character destroys a durable copy another shard holds a claim on — \
                 cross-database that is the annihilation case (the destination's arrival copy \
                 survives and its reaper never settles)",
            ),
            (
                "chat.rs",
                include_str!("chat.rs"),
                "pub fn send_whisper(",
                "send_whisper reaches an in-transit character by NAME because begin_transfer \
                 persists with `set_offline: false`",
            ),
            (
                "gm.rs",
                include_str!("gm.rs"),
                "pub fn set_gm_level(",
                "set_gm_level writes gm_level onto the source copy by NAME",
            ),
            (
                "debug.rs",
                include_str!("debug.rs"),
                "pub fn debug_spawn_player_entity(",
                "debug_spawn_player_entity is player_login's RE-MATERIALISATION path wearing a \
                 harness hat — unfenced it is a second route to the dual-liveness dupe",
            ),
            (
                "debug.rs",
                include_str!("debug.rs"),
                "pub fn debug_set_money(",
                "debug_set_money writes Character.money on the source copy",
            ),
            (
                "debug.rs",
                include_str!("debug.rs"),
                "pub fn debug_expire_quest(",
                "debug_expire_quest writes game_character_quest, a MANIFEST table",
            ),
            (
                "debug.rs",
                include_str!("debug.rs"),
                "pub fn debug_grant_reputation(",
                "debug_grant_reputation writes game_player_reputation, a MANIFEST table",
            ),
            (
                "debug.rs",
                include_str!("debug.rs"),
                "pub fn debug_grant_default_actions(",
                "debug_grant_default_actions writes game_player_action, a HOT manifest table",
            ),
            (
                "skill.rs",
                include_str!("skill.rs"),
                "pub fn debug_reseed_skills(",
                "debug_reseed_skills writes game_player_skill, a HOT manifest table",
            ),
            // --- Added by the #30 review's independent call-site audit. ---
            (
                "world.rs",
                include_str!("world.rs"),
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
                include_str!("world.rs"),
                "pub(crate) fn recall_to_home(",
                "recall_to_home is the ONE teleport_player caller that needs no live entity (it \
                 reads the home coords off the durable row), so by-guid — via \
                 debug_use_hearthstone — it is the only route by which teleport_player's \
                 unconditional durable-row write lands on an escrowed character, moving FIVE \
                 ExportBlob fields plus the pending_instance_id that in_transit_instances reads",
            ),
            (
                "debug.rs",
                include_str!("debug.rs"),
                "pub fn debug_set_level(",
                "debug_set_level drives stats::set_character_level, which writes Character.level \
                 (an ExportBlob field) and Character.xp on the DURABLE row and needs no live \
                 entity to do it",
            ),
        ];
        for (file, src, signature, why) in sites {
            let body = code_of(src, signature);
            assert!(
                body.contains("character_by_guid") || body.contains("character_by_name"),
                "{file}'s `{signature}` no longer routes through helpers::character_by_guid / \
                 character_by_name — the REFUSE fence is gone (issue #30). {why}. Body was:\n{body}"
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
                include_str!("rest.rs"),
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
                include_str!("quest.rs"),
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
        let body = code_of(include_str!("instance.rs"), "fn occupied_instances(");
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
        let body = code_of(include_str!("loot.rs"), "fn credit_purse(");
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
            name: "Ginger".to_string(),
            level: 5,
            map_id: 0,
            instance_id: 0,
            x: 1.0,
            y: 2.0,
            z: 3.0,
            o: 0.5,
            health: 100,
            power: 50,
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
        let blob = code_of(include_str!("transfer.rs"), "pub struct ExportBlob");
        assert!(
            !blob.contains("owner_identity"),
            "ExportBlob grew an owner_identity field. That value is rebound from the LIVE \
             connection by `establish_session` at every logon and restamped onto the owned rows by \
             `player_login`, so a carried copy arrives stale and is immediately overwritten — the \
             REGENERATE verdict in this file's table (issue #30). Blob was:\n{blob}"
        );
        let login = code_of(include_str!("world.rs"), "pub fn player_login(");
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
        let body = body_of(include_str!("transfer.rs"), "pub(crate) fn is_in_transit(");
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
    /// "Not transported" is a legal answer — via `transfer::not_transported`, written AT the table
    /// (see `rest.rs` / `group.rs`'s invite row) — because a decision recorded at the table is a
    /// different thing from an omission nobody noticed.
    /// The table names in one generated registry const, read off the GENERATED TEXT.
    ///
    /// Deliberately not `crate::CHARACTER_OWNED_TRANSFERS`: referencing the const array
    /// materializes every registered fn's POINTER, which drags the SpacetimeDB host imports
    /// (`datastore_insert_bsatn`, `row_iter_bsatn_advance`, …) into this native test binary, which
    /// cannot link them. Same reasoning — and the same discovery-by-linker-error — as
    /// `lib.rs::commented_out_markers_do_not_register`.
    fn generated_registry_tables(const_name: &str) -> Vec<String> {
        let src = include_str!(concat!(env!("OUT_DIR"), "/character_sweeps.rs"));
        let at = src
            .find(&format!("pub const {const_name}"))
            .unwrap_or_else(|| panic!("{const_name} is not in the generated registry"));
        let open = src[at..].find("&[").expect("const has an array literal") + at;
        let close = src[open..].find("];").expect("array literal is terminated") + open;
        src[open..close]
            .match_indices('"')
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| src[open + c[0] + 1..open + c[1]].to_string())
            .collect()
    }

    #[test]
    fn every_manifest_table_can_cross_a_database_boundary() {
        let transports = generated_registry_tables("CHARACTER_OWNED_TRANSFERS");
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

    /// Every `character_owned!(transfer, ..)` arm in the tree, as `(table, arm body)`. Read off DISK
    /// (the `character_owned_tripwire` walker, so `packages/*/src` is covered too) rather than
    /// `include_str!`, because the arms live in ~19 different files and a hand-kept list of them
    /// would be the exact parallel-list drift this whole registry exists to abolish.
    fn transport_arm_bodies() -> Vec<(String, String)> {
        const MARKER: &str = "character_owned!(transfer, fn sweep_transfer_";
        let mut out = Vec::new();
        for file in crate::character_owned_tripwire::scanned_files() {
            let content = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
            for (idx, _) in content.match_indices(MARKER) {
                let line_start = content[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
                if content[line_start..idx].trim_start().starts_with("//") {
                    continue; // the doc/prose mentions of the marker, incl. this file's own
                }
                let after = &content[idx + MARKER.len()..];
                let name_end = after
                    .find('(')
                    .unwrap_or_else(|| panic!("malformed transport arm in {file:?}"));
                let table = after[..name_end].to_string();
                // The marker also appears inside assertion/prose STRINGS in this very file, spelled
                // with a `<accessor>` placeholder. A real arm's name is a plain identifier.
                if table.is_empty()
                    || !table
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
                {
                    continue;
                }
                let open = after.find('{').expect("arm has a body");
                let mut depth = 0i32;
                let mut body = String::new();
                for (i, c) in after[open..].char_indices() {
                    match c {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        body = after[open..=open + i].to_string();
                        break;
                    }
                }
                assert!(!body.is_empty(), "unterminated transport arm for {table}");
                let code: String = body
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push((table, code));
            }
        }
        out
    }

    /// THE RATCHET'S SECOND HALF, and the one that actually stops silent data loss.
    ///
    /// `every_manifest_table_can_cross_a_database_boundary` only proves an arm EXISTS. It cannot
    /// tell a real transport arm from a `not_transported(io)` stub — so the single edit it is
    /// supposed to prevent walks straight past it. Verified by mutation: repointing
    /// `sweep_transfer_game_item_instance` at `not_transported` left all 468 module tests green
    /// while deleting every character's entire inventory and equipped gear on every shard hop.
    ///
    /// So: an arm either calls `move_rows` (it transports) or it is named in [`NOT_TRANSPORTED`]
    /// with a written reason, and nothing in between. Both directions fail — a table that stops
    /// transporting, and an allowlist entry that quietly started transporting again (or that names
    /// a table which no longer exists).
    #[test]
    fn not_transported_is_an_allowlisted_decision_not_a_silent_drop() {
        let arms = transport_arm_bodies();
        assert!(
            arms.len() >= manifest().len(),
            "the on-disk scan found only {} transport arms for {} manifest tables — the scanner is \
             broken, and a broken scanner passes vacuously",
            arms.len(),
            manifest().len()
        );
        for (table, body) in &arms {
            let moves = body.contains("move_rows");
            let drops = body.contains("not_transported");
            assert!(
                moves ^ drops,
                "sweep_transfer_{table} neither transports (`transfer::move_rows`) nor declines to \
                 (`transfer::not_transported`) — or does both. Every arm must do exactly one, \
                 because those are the only two answers the protocol has. Body was:\n{body}"
            );
            // ...and it must do that ONE thing UNCONDITIONALLY, at the top of the arm.
            //
            // `contains` alone is defeated by keeping the sanctioned call alive in a dead branch
            // and doing the damage next to it — an adversarial review turned
            //
            //     if false { crate::transfer::move_rows(..) }
            //     match io { RowIo::Export(out) => out.clear(), RowIo::Import(..) => {} }
            //
            // green while every character's inventory and gear was dropped on every hop, which is
            // the exact defect this ratchet was written for. All 20 arms in the tree have one of
            // exactly two shapes, so the shapes themselves are the assertion.
            let shape = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                !shape.contains("RowIo"),
                "sweep_transfer_{table} matches on `RowIo` itself. An arm supplies the two \
                 table-typed halves and NOTHING else — the direction match belongs to \
                 `move_rows`/`not_transported`, which is what makes a table unable to ship rows it \
                 cannot receive. Body was:\n{body}"
            );
            let calls =
                shape.matches("move_rows").count() + shape.matches("not_transported").count();
            assert_eq!(
                calls, 1,
                "sweep_transfer_{table} mentions the transport helpers {calls} times. Exactly one \
                 call, exactly once: a second (or dead) mention is how a repointed arm hides from \
                 this scan. Body was:\n{body}"
            );
            if drops {
                assert_eq!(
                    shape,
                    "{ let _ = (ctx, character_guid); crate::transfer::not_transported(io); }",
                    "sweep_transfer_{table} declines to transport, but not in the one sanctioned \
                     shape. A declining arm has NO logic of its own — anything else is either a \
                     transport in disguise or a silent drop wearing the allowlist's clothes."
                );
            } else {
                assert!(
                    shape.starts_with("{ crate::transfer::move_rows("),
                    "sweep_transfer_{table} does not OPEN with its `move_rows` call. Anything \
                     ahead of it is a branch that can skip the transport, and a skipped table's \
                     rows are gone the moment finish_transfer deletes the source copy. Body \
                     was:\n{body}"
                );
                // The two CLOSURES are the half no test can execute (they need a real
                // `ReducerContext`), so they get the only check text can give: the export must
                // actually select this character's rows, and the import must actually insert one.
                // Emptying them — `|| Vec::new()` and `|_, _| {}` — is `not_transported` wearing
                // the sanctioned shape, and it was green until this assertion existed.
                assert!(
                    shape.contains("filter(&character_guid)"),
                    "sweep_transfer_{table}'s EXPORT closure does not select rows by \
                     `character_guid`. A closure that yields nothing ships an empty payload, which \
                     the destination cannot tell from 'this character owned nothing' — and the \
                     source copy is cascade-deleted moments later. Body was:\n{body}"
                );
                assert!(
                    shape.contains(".insert("),
                    "sweep_transfer_{table}'s IMPORT closure does not insert the arriving row. The \
                     rows cross the wire and are then dropped on the floor at the destination. \
                     Body was:\n{body}"
                );
            }
            if drops {
                assert!(
                    NOT_TRANSPORTED.contains(&table.as_str()),
                    "sweep_transfer_{table} calls `not_transported`, but {table} is NOT on \
                     `transfer::NOT_TRANSPORTED`. A character's {table} rows would be silently \
                     DROPPED on every database hop — no error, and the source copy is \
                     cascade-deleted by finish_transfer immediately afterwards. If that really is \
                     the decision, add {table} to NOT_TRANSPORTED with its reason; otherwise this \
                     arm must call `transfer::move_rows`."
                );
            } else {
                assert!(
                    !NOT_TRANSPORTED.contains(&table.as_str()),
                    "{table} is on `transfer::NOT_TRANSPORTED` but its arm transports rows — the \
                     allowlist and the code disagree, so the written decision is stale."
                );
            }
        }
        let tables: Vec<&str> = arms.iter().map(|(t, _)| t.as_str()).collect();
        for t in NOT_TRANSPORTED {
            assert!(
                tables.contains(t),
                "`transfer::NOT_TRANSPORTED` names {t}, which has no transport arm at all — a \
                 rename left the allowlist stale, and a stale allowlist silently licenses the next \
                 table that takes that name."
            );
        }
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
            include_str!("transfer.rs"),
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
            include_str!("transfer.rs"),
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

    #[test]
    fn the_cross_database_reaper_never_reads_a_missing_in_row_as_not_imported() {
        let body = code_of(include_str!("transfer.rs"), "pub fn reap_transfers(");
        assert!(
            body.contains("cross_database && !has_in") && body.contains("None"),
            "reap_transfers no longer answers `None` for an UNATTESTED cross-database escrow \
             (issue #19). Cross-database the in-row is the gateway's attestation, not a readable \
             fact about the destination: reading its absence as `Some(false)` rolls the escrow BACK \
             while the destination copy may already be live — a DUPLICATED character, and the one \
             mutation the same-database crash matrix cannot see. Body was:\n{body}"
        );
    }

    // `finish_destroys_the_source_copy_for_a_cross_database_transfer` and its ordering assertions
    // used to live here as a source scan of `fn do_finish(`. `do_finish` is now a two-line adapter
    // over `apply_finish` (issue #34's seam), and every property that scan asserted — the cascade,
    // its `cross_database` gate, detach-before-cascade, cascade-before-record_shard — is executed
    // for real against `transport_harness::FakeDb`. See `mod transport_harness`'s `finish_*` tests.

    #[test]
    fn import_character_blob_still_proves_the_destination_copy_is_durable() {
        // The reducer is now a two-line adapter over `apply_import_blob` (issue #37's seam), so the
        // guards below live in that body — but the ADAPTER still has to call it, and still has to
        // gate on the operator. Both, in order, or the whole harness is testing dead code.
        let shim = code_of(include_str!("transfer.rs"), "pub fn import_character_blob(");
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
            include_str!("transfer.rs"),
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
            body.contains("decoded.manifest != manifest()"),
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

    // Note on `shape_of` (deliberately `//`: it is about the helper this file `use`s, not a doc for
    // the test below).
    // [`code_of`] with every whitespace run collapsed, so a body can be compared for EQUALITY
    // rather than for `contains`. That difference is the whole point of the test below: a
    // `contains` assertion is defeated by leaving the string it looks for in a dead branch, and
    // this repo's scans have been defeated that way repeatedly. (Shared as `test_scan::shape_of`,
    // `use`d at the top of this `mod tests`.)

    /// THE SEAM'S OWN BLIND SPOT — the production adapters, pinned by EXACT SHAPE.
    ///
    /// `mod transport_harness` executes `export_rows_via`, `import_rows_via` and
    /// `apply_import_blob` — the SHARED bodies. What it can never execute is the thin production
    /// layer that binds them to a real `ReducerContext`: `export_rows`, `import_rows`, the
    /// `import_character_blob` shim, and `CtxImportSink`'s eleven one-line methods. The harness
    /// substitutes `FakeDb` for every one of them, so any edit there is invisible to it — and each
    /// is a single line whose damage is total. Verified by mutation, all with **498 passed, 0
    /// failed** before this test existed:
    ///
    /// | edit | live consequence |
    /// |---|---|
    /// | `CtxImportSink::import_rows` → `Ok(())` | **no manifest table's rows ever arrive** |
    /// | `export_rows` → `export_rows_via(ctx, 0, ..)` | every table exports an EMPTY payload |
    /// | `export_rows` → `export_rows_via(ctx, guid, &[])` | same, via an empty registry |
    /// | `import_character_blob` → `return Ok(())` above the call | the reducer the gateway calls does NOTHING; the whole harness is testing dead code |
    /// | `ensure_shadow_account` → no-op | the arriving player cannot log in at all |
    /// | `in_row` → `None` | replay detection dead: a re-driven import PK-panics |
    /// | `has_live_entity` → `false` | the dual-liveness dupe guard is dead |
    /// | `bump_guid_high_water` → no-op | issue #59: this database can re-issue an imported character's guid to a brand-new local one |
    /// | `own_guid_range` → always `None` | issue #237's gate always reads "foreign" — a LOCAL-range arrival stops ratcheting, regressing #59 AC#3 |
    ///
    /// So these four bodies are compared for EQUALITY against the shape they are allowed to have.
    /// Any change — including one that keeps the old text alive in a dead branch — fails here and
    /// must be re-blessed deliberately. That is the correct cost: this layer is *supposed* to be
    /// four pass-throughs and an adapter, and if it ever stops being that, the harness below it
    /// stops meaning what it claims.
    #[test]
    fn the_production_adapters_are_the_pass_throughs_the_harness_assumes() {
        let src = include_str!("transfer.rs");
        for (signature, want) in [
            (
                "pub(crate) fn export_rows(",
                "{ export_rows_via(ctx, character_guid, crate::CHARACTER_OWNED_TRANSFERS) }",
            ),
            (
                "pub(crate) fn import_rows(",
                "{ import_rows_via( ctx, character_guid, payload, crate::CHARACTER_OWNED_TRANSFERS, ) }",
            ),
            (
                "pub fn import_character_blob(",
                "{ require_operator(ctx)?; \
                   apply_import_blob(&mut CtxImportSink { ctx }, transfer_id, blob) }",
            ),
            (
                "impl ImportSink for CtxImportSink<'_> {",
                "{ fn in_row(&self, transfer_id: u64) -> Option<TransferIn> { \
                   self.ctx .db .game_transfer_in() .transfer_id() .find(transfer_id) } \
                   fn has_live_entity(&self, guid: u64) -> bool { \
                   self.ctx.db.game_world_entity().guid().find(guid).is_some() } \
                   fn has_character(&self, guid: u64) -> bool { \
                   self.ctx.db.game_character().guid().find(guid).is_some() } \
                   fn cascade_delete_character(&mut self, guid: u64) { \
                   crate::world::cascade_delete_character(self.ctx, guid); } \
                   fn insert_character(&mut self, c: crate::character::Character) { \
                   self.ctx.db.game_character().insert(c); } \
                   fn import_rows(&mut self, guid: u64, payload: &[TableRows]) -> Result<(), String> { \
                   import_rows(self.ctx, guid, payload) } \
                   fn ensure_shadow_account(&mut self, account_id: u64) { \
                   crate::auth::ensure_shadow_account(self.ctx, account_id); } \
                   fn file_in_row(&mut self, row: TransferIn) { \
                   self.ctx.db.game_transfer_in().insert(row); } \
                   fn now_micros(&self) -> i64 { self.ctx.timestamp.to_micros_since_unix_epoch() } \
                   fn bump_guid_high_water(&mut self, guid: u64) { \
                   crate::auth::bump_guid_high_water(self.ctx, guid); } \
                   fn own_guid_range(&self) -> Option<(u64, u64)> { \
                   self.ctx .db .game_guid_range() .id() .find(0) .map(|r| (r.base, r.size)) } }",
            ),
            // Issue #34's seam, pinned the same way and for the same reason: `apply_finish` runs
            // under `FakeDb`, so a no-op'd `record_shard` here (or a `clear_escrow` that forgot the
            // out-row, which strands the character's claim forever) is invisible to the harness.
            (
                "fn do_finish(",
                "{ apply_finish(&mut CtxFinishSink { ctx }, transfer_id) }",
            ),
            (
                "impl FinishSink for CtxFinishSink<'_> {",
                "{ fn out_row(&self, transfer_id: u64) -> Option<TransferOut> { \
                   self.ctx .db .game_transfer_out() .transfer_id() .find(transfer_id) } \
                   fn detach_for_transfer(&mut self, guid: u64) { \
                   crate::group::detach_for_transfer(self.ctx, guid); } \
                   fn cascade_delete_character(&mut self, guid: u64) { \
                   crate::world::cascade_delete_character(self.ctx, guid); } \
                   fn record_shard(&mut self, guid: u64, map_id: u32, instance_id: u64) { \
                   crate::realm_core::record_shard(self.ctx, guid, map_id, instance_id); } \
                   fn clear_escrow(&mut self, transfer_id: u64) { \
                   self.ctx .db .game_transfer_in() .transfer_id() .delete(transfer_id); \
                   self.ctx .db .game_transfer_out() .transfer_id() .delete(transfer_id); } }",
            ),
        ] {
            let want = want.split_whitespace().collect::<Vec<_>>().join(" ");
            assert_eq!(
                shape_of(src, signature),
                want,
                "`{signature}` is no longer the exact pass-through `mod transport_harness` assumes \
                 it is. The harness runs the SHARED body underneath this layer with a `FakeDb` \
                 substituted for every line here, so nothing it asserts covers this edit — a \
                 no-op'd method or a short-circuited shim here leaves all 498 tests green while \
                 the transport silently loses rows, or does nothing at all. If the change is \
                 deliberate, update the expected shape in this test with the same care."
            );
        }
    }

    #[test]
    fn the_transport_arms_are_the_only_thing_that_moves_rows() {
        // `export_rows` must derive from the GENERATED registry, never a hand-kept list — the same
        // property `manifest()` has, for the same reason: a parallel list drifts silently and the
        // drift is invisible until a character arrives missing a table.
        let body = code_of(include_str!("transfer.rs"), "pub(crate) fn export_rows(");
        assert!(
            body.contains("CHARACTER_OWNED_TRANSFERS"),
            "export_rows no longer iterates the generated transport registry. Body was:\n{body}"
        );
        // And an arriving table this build has no arm for must ABORT, never be skipped: a skipped
        // table is a table whose rows are dropped, and the source copy is about to be deleted.
        let body = code_of(
            include_str!("transfer.rs"),
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

    #[test]
    fn begin_transfer_serializes_the_rows_not_just_the_manifest() {
        let body = code_of(include_str!("transfer.rs"), "pub fn begin_transfer(");
        assert!(
            body.contains("export_rows(ctx, character_guid)"),
            "begin_transfer no longer exports the character-owned ROWS into the blob (issue #19) — \
             the blob is back to being a manifest, and a cross-database arrival would be a naked \
             character with no gear, spells, skills or quest log. Body was:\n{body}"
        );
        assert!(
            body.contains("build_export_blob(transfer_id, &character, dest, payload)?"),
            "begin_transfer no longer builds the blob through `build_export_blob` — that is the \
             `ReducerContext`-free half the round-trip harness drives, so a hand-rolled blob here \
             is a blob no test has ever seen. Body was:\n{body}"
        );
        // Assert on the SERIALIZATION CALL, not on the field name: `let character_row = Vec::new();`
        // contains "character_row" and ships an EMPTY row, which the arriving side then fails to
        // decode — a mutation that survived the first cut of this assertion (the same
        // matched-the-name-not-the-behavior trap `code_of`'s doc records).
        let built = code_of(
            include_str!("transfer.rs"),
            "pub(crate) fn build_export_blob(",
        );
        assert!(
            built.contains("bsatn::to_vec(&character)"),
            "build_export_blob no longer serializes the `game_character` row itself — \
             cross-database there is no row at the destination to re-partition, so the blob's \
             `character_row` IS the character. Body was:\n{built}"
        );
    }

    #[test]
    fn confirm_import_refuses_to_attest_a_same_database_transfer() {
        // Same-database, `import_character` files the in-row itself as a FACT it can read. Letting
        // the driver forge one would license `finish_transfer` with no import at all.
        let body = code_of(include_str!("transfer.rs"), "pub fn confirm_import(");
        assert!(
            body.contains("cross_database") && body.contains("refusing to forge an attestation"),
            "confirm_import no longer refuses same-database escrows. Body was:\n{body}"
        );
        assert!(
            body.contains("nothing escrowed here"),
            "confirm_import no longer requires an out-row — it would file an in-row for a transfer \
             that was never begun. Body was:\n{body}"
        );
    }

    #[test]
    fn release_transfer_refuses_when_this_database_is_the_source() {
        let body = code_of(include_str!("transfer.rs"), "pub fn release_transfer(");
        assert!(
            body.contains("out.is_some()") || body.contains("call finish_transfer"),
            "release_transfer no longer refuses when a local out-row exists — on the SOURCE that \
             would drop the attestation and leave the out-row behind, unfreezing nothing and \
             making the escrow unreadable as in-flight. Body was:\n{body}"
        );
    }

    /// A LONE arrival fence still names its character. Deleting the in-row fallback left the whole
    /// suite green (#36 review, blocker 2) and wedges the character out of its own login forever —
    /// see [`escrowed_guid`] for the exact path.
    #[test]
    fn a_lone_arrival_fence_still_names_the_character_it_fences() {
        assert_eq!(escrowed_guid(None, None), None, "an unused id names nobody");
        assert_eq!(
            escrowed_guid(Some(7), None),
            Some(7),
            "the out-row is the source's claim"
        );
        assert_eq!(
            escrowed_guid(None, Some(7)),
            Some(7),
            "a database holding ONLY an unreleased arrival in-row must still read as escrowed for \
             that character. Reading it as an unused id is the permanent login wedge the #36 \
             review found: `is_in_transit` sees the in-row and refuses the hop out, and nothing \
             ever reaps it (`reap_transfers` iterates game_transfer_out only)."
        );
        assert_eq!(
            escrowed_guid(Some(7), Some(9)),
            Some(7),
            "the SOURCE claim wins — it is the side that holds the blob and can be driven from"
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
    //  deliberate, written decision, not an oversight — `mod transport_harness`'s header lists
    //  them as the crate's remaining scan-only transfer guards.
    // -------------------------------------------------------------------------------------

    /// The cross-database eviction must KEEP the instance lease. `delete_row: true` destroys the
    /// `game_instance` row and its `game_instance_binding` rows — the bindings that just travelled
    /// in the arriving character's blob — so the next party member's `resolve_or_create_instance`
    /// mints a SECOND instance and the party is split across two copies of the dungeon.
    #[test]
    fn the_cross_database_eviction_keeps_the_instance_lease() {
        let body = code_of(
            include_str!("instance.rs"),
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
        let arms = transport_arm_bodies();
        let (_, body) = arms
            .iter()
            .find(|(t, _)| t == "game_group_member")
            .expect("game_group_member still has a transport arm (declining is an arm too)");
        assert!(
            body.contains("not_transported"),
            "`game_group_member` transports again. Party membership is authoritative on realm-core \
             (#22): the gateway re-pushes the roster onto the destination at world entry, and a \
             blob snapshot taken back at `begin_transfer` would overwrite it with the membership the \
             character had when it stepped into the portal. Body was:\n{body}"
        );
        assert!(
            NOT_TRANSPORTED.contains(&"game_group_member"),
            "the decision must also be written on the allowlist, with its reason"
        );
    }

    #[test]
    fn phase_reads_the_ledger_exactly() {
        assert_eq!(phase(false, false), Phase::Resident);
        assert_eq!(phase(true, false), Phase::Escrowed);
        assert_eq!(phase(true, true), Phase::Imported);
        assert_eq!(
            phase(false, true),
            Phase::Resident,
            "a lone in-row licenses no deletion"
        );
    }
}

// ===========================================================================================
//  EXECUTION HARNESS (issue #37) — the transport, RUN rather than scanned
// ===========================================================================================

/// The first thing in this crate that actually **executes** the cross-database transport.
///
/// # Why this did not exist
///
/// `ReducerContext` cannot be constructed in a unit test, so every other test in `module/` is
/// either a pure model or a source scan. That left `export_rows` / `import_rows` / `move_rows` /
/// `import_character_blob` pinned by their TEXT: the #36 review ran 21 mutations against that
/// surface and 17 left the suite green or hung it — including repointing
/// `sweep_transfer_game_item_instance` at `not_transported` (which deletes every character's
/// inventory and gear on every hop) with **468 passed, 0 failed**.
///
/// # The seam
///
/// Two ordinary Rust generalisations, no framework:
///
/// 1. [`move_rows`] / [`export_rows_via`] / [`import_rows_via`] are generic over the CONTEXT type
///    and take the transport registry as a PARAMETER. Production binds `C = ReducerContext` and
///    `crate::CHARACTER_OWNED_TRANSFERS` (`export_rows`, `import_rows`); the harness binds
///    `C = FakeDb` and [`ARMS`]. The loop bodies, the `MANIFEST_EXCLUDE` filter, the codec and the
///    unknown-table refusal are the SAME code either way.
/// 2. [`apply_import_blob`] is the whole of `import_character_blob` bar the operator gate, written
///    against the [`ImportSink`] trait. `CtxImportSink` is the production adapter; [`FakeDb`] is
///    the test one. Every guard the reducer has is executed here against real bsatn blobs built by
///    the real [`build_export_blob`].
///
/// # The ceiling — what this harness still CANNOT run, and why
///
/// The 19 per-table `character_owned!(transfer, ..)` arms themselves. Their bodies are
/// `ctx.db.game_item_instance().by_owner().filter(..)` — real table accessors that only exist on a
/// real `ReducerContext`, and making them generic would mean rewriting every table accessor in the
/// module behind a trait, which is not a testing change but a rewrite of the module. So:
///
/// * **What is executed here**: the transport plumbing every arm flows through (the codec, the
///   guid it is handed, the registry lookup, the export/import loops, `not_transported`), plus all
///   of `apply_import_blob`.
/// * **What is still pinned by a source scan, and why** — four guards, each a single call inside a
///   reducer or an arm whose only observable effect is real table state:
///   - each REAL table's arm EXISTS (`every_manifest_table_can_cross_a_database_boundary`);
///   - each arm transports rather than dropping
///     (`not_transported_is_an_allowlisted_decision_not_a_silent_drop`, the #36 fix — this is the
///     one that catches repointing `sweep_transfer_game_item_instance` at `not_transported`);
///   - the cross-database eviction keeps the instance LEASE
///     (`the_cross_database_eviction_keeps_the_instance_lease`);
///   - the group mirror carries the SOURCE `group_id`
///     (`the_group_mirror_preserves_the_source_group_id`).
///
///   All four are named tests with a written consequence, not silent text matches. Those scans plus
///   this harness are the whole ratchet; neither half is sufficient alone.
/// * **The seam's own blind spot** — the thin PRODUCTION layer this harness substitutes `FakeDb`
///   for: `export_rows`, `import_rows`, the `import_character_blob` shim and `CtxImportSink`'s nine
///   one-line methods. Nothing here runs any of them, and each is a single line whose damage is
///   total — no-op'ing `CtxImportSink::import_rows` means **no manifest table's rows ever arrive**,
///   and an early `return Ok(())` in the shim means the reducer the gateway calls does nothing at
///   all while every test below still passes. So that layer is pinned by EXACT-SHAPE equality in
///   `tests::the_production_adapters_are_the_pass_throughs_the_harness_assumes` — equality, not
///   `contains`, because a `contains` scan is defeated by leaving its own text in a dead branch.
///   Text scans remain defeatable in principle; that one is written as the tightest form available
///   (any edit at all fails it) precisely because it is the hinge the rest of this module hangs on.
/// * **What is still not covered anywhere headless**: SpacetimeDB's transaction rollback. A real
///   `Err` from `import_character_blob` unwinds every write it made; [`FakeDb`] keeps them. Every
///   refusal test below therefore asserts on the **in-row** — the row whose absence is what
///   actually stops `finish_transfer` from destroying the source copy — and never on "nothing was
///   written".
///
/// Deliberate simplification: three fake tables, not nineteen. The transport is table-agnostic by
/// construction (one opaque `TableRows` per arm), so a fourth fake table would exercise no new line of production
/// code. The three are chosen to cover the three SHAPES an arm can have: transports, transports
/// with a different row type, and `not_transported`.
#[cfg(test)]
mod transport_harness {
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
    /// gateway's `FakeShardDb` (issue #37).
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
        /// `game_transfer_out`: transfer_id -> the escrow's routing fields (issue #34).
        out_rows: RefCell<HashMap<u64, XOut>>,
        /// `game_group_member`: character_guid -> group_id.
        group_members: RefCell<HashMap<u64, u64>>,
        /// Parties torn down by `remove_member`'s disband — what the character-owned DELETE sweep
        /// does to a party when one of its members is deleted, and precisely what
        /// `detach_for_transfer` exists to run AHEAD of (issue #19 AC#4).
        disbanded: RefCell<HashSet<u64>>,
        /// `game_character_shard`: the forwarding receipt. CHARACTER-OWNED, so the cascade sweeps
        /// it — which is why `apply_finish` must record it AFTER the cascade, never before.
        shard_index: RefCell<HashMap<u64, (u32, u64)>>,
        accounts: RefCell<HashSet<u64>>,
        now: Cell<i64>,
        /// Makes `insert_character` silently do nothing — the ONE thing a real destination can do
        /// that this fake otherwise cannot: accept the call and materialise no row. That is the
        /// state `apply_import_blob`'s post-import PROOF exists to catch, and without a way to
        /// reach it the proof was pinned only by its own text (a #36 mutation survivor).
        swallow_inserts: Cell<bool>,
        /// `game_guid_allocator.high_water` (issue #59): what `bump_guid_high_water` has ratcheted
        /// it to. Starts at 0 (unseeded), same as a fresh database.
        guid_high_water: Cell<u64>,
        /// `game_guid_range` (issue #108/#237): the `(base, size)` THIS fake database mints from,
        /// if any. `None` (the default) models a database that never installed one — same as a
        /// fresh real database before `install_guid_range` runs.
        guid_range: Cell<Option<(u64, u64)>>,
    }

    /// The escrow out-row's routing fields, kept separately so the fake never needs `TransferOut`
    /// to be `Clone` (the blob and the position are not read by `apply_finish`).
    #[derive(Clone, Copy)]
    pub struct XOut {
        pub character_guid: u64,
        pub dest_map_id: u32,
        pub dest_instance_id: u64,
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

    /// The issue #34 seam: `apply_finish` runs here for real, ordering included.
    impl FinishSink for FakeDb {
        fn out_row(&self, transfer_id: u64) -> Option<TransferOut> {
            self.out_rows
                .borrow()
                .get(&transfer_id)
                .map(|o| TransferOut {
                    transfer_id,
                    character_guid: o.character_guid,
                    dest_map_id: o.dest_map_id,
                    dest_instance_id: o.dest_instance_id,
                    dest_x: 0.0,
                    dest_y: 0.0,
                    dest_z: 0.0,
                    dest_o: 0.0,
                    blob: Vec::new(),
                    created_micros: 0,
                    cross_database: o.cross_database,
                })
        }
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
        fn clear_escrow(&mut self, transfer_id: u64) {
            self.in_rows.borrow_mut().remove(&transfer_id);
            self.out_rows.borrow_mut().remove(&transfer_id);
        }
    }

    impl ImportSink for FakeDb {
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
        fn has_live_entity(&self, guid: u64) -> bool {
            self.live.borrow().contains(&guid)
        }
        fn has_character(&self, guid: u64) -> bool {
            self.chars.borrow().contains_key(&guid)
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
        fn file_in_row(&mut self, row: TransferIn) {
            self.in_rows.borrow_mut().insert(
                row.transfer_id,
                (row.character_guid, row.blob, row.created_micros),
            );
        }
        fn now_micros(&self) -> i64 {
            self.now.get()
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
    const XFER: u64 = GUID; // the transfer id IS the character guid (#19)
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
    /// The range a `dst` fixture installs in tests that need one (issue #237) — small and round
    /// so a "foreign" guid reads as obviously outside it at a glance, deliberately not shaped
    /// like a real shard's billion-wide slot so a failing assertion's numbers read as test
    /// fixture, never as a live shard's actual range.
    const LOCAL_RANGE: (u64, u64) = (0, 1_000_000);
    /// A guid OUTSIDE `LOCAL_RANGE` — stands for a `lyracore-world-1`-born character (a
    /// different slot entirely) crossing into a database that owns `LOCAL_RANGE`. This is the
    /// live #237 trigger: any Kalimdor-born character crossing into `lyracore`.
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
            "no shadow `game_account` row — `player_login` resolves the caller through \
             `account_by_identity`, so the arriving player cannot log in at all"
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
        let moved = export(&dst, GUID, XFER, DEST);
        let mut far = FakeDb::new();
        apply_import_blob(&mut far, XFER, wire(&moved)).expect("hop 3 commits");
        let after = export(&far, GUID, XFER, DEST);
        assert_eq!(
            after.payload, first.payload,
            "the ROWS must survive a relocating hop unchanged"
        );
        assert_eq!(after.money, first.money);
        assert_eq!(after.health, first.health);
        assert_eq!(after.level, first.level);
        assert_eq!(
            (after.map_id, after.x, after.y, after.z, after.o),
            (DEST.map_id, DEST.x, DEST.y, DEST.z, DEST.o),
            "the relocation must land on the destination"
        );
    }

    // =======================================================================================
    //  Issue #59 — importing a character ratchets the destination's guid allocator
    // =======================================================================================

    /// AC#3: an imported character bumps the destination's guid high-water mark, so a
    /// `create_character` on THIS database afterward can never hand out the same guid. This is
    /// the LOCAL-range half of issue #237's fix — `GUID` sits inside `dst`'s own `LOCAL_RANGE`,
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
    /// (and therefore its never-lowers rule) rather than being skipped by the #237 gate for an
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

    /// **Issue #237, the regression fixed here.** An arrival whose guid belongs to ANOTHER
    /// shard's range must leave THIS database's allocator untouched — ranges are disjoint by
    /// construction (#108), so `FOREIGN_GUID` can never collide
    /// with anything `dst` mints, and ratcheting past it anyway is exactly what pushed
    /// `lyracore`'s real high-water mark to its own range end with zero local characters
    /// above it — every subsequent local `create_character` then failed
    /// `GUID_RANGE_EXHAUSTED`. The import itself must still SUCCEED (a foreign arrival is not an
    /// error — only the allocator bump is skipped).
    #[test]
    fn importing_a_foreign_range_guid_leaves_the_destinations_allocator_untouched() {
        let src = FakeDb::populated(FOREIGN_GUID);
        let blob = export(&src, FOREIGN_GUID, FOREIGN_GUID, DEST); // transfer_id IS the guid (#19)

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
        // The #36 mutation: `import_rows(..)?` -> `let _ = import_rows(..)`. A partial character is
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

    /// Issue #42, the inverse of the test above and the one the drift contract forgot: a table the
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
        // #16's stated contract. Deleting this guard left 468 tests green.
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
        apply_import_blob(&mut dst, XFER, wire(&blob))
            .expect("the import commits over the stale copy");

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

    /// Issue #83: the wipe above only fired when `has_character(guid)` was true, so an ORPHANED
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
        // #30's DEFER verdict, end to end: `credit_purse` on an in-transit character folds copper
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
    /// accepted the insert and materialised nothing. Deleting the proof left 468 tests green in
    /// #36, and it stayed scan-only through the first cut of this harness — the scan is defeated by
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
    /// machinery filter. (`export_rows` passing guid `0` to every mover was a #36 survivor.)
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
    //  `apply_finish` — the delete-last body, executed (issue #34).
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

    /// Issue #19 AC#4: a shard hop is not a departure.
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

    /// Issue #34 / the #19 × #20 merge order, as a behaviour rather than a comment.
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
}
