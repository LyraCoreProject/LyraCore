//! The row TRANSPORT: what crosses a database boundary, and in what shape.
//!
//! `mod.rs` owns the escrow protocol — when a character may move and what each step is allowed to
//! do. This file owns the cargo: the manifest that says WHICH tables travel, the [`RowIo`] direction
//! marker and [`move_rows`] shim every `character_owned!(transfer, ..)` arm flows through, the bsatn
//! codec underneath it, and [`ExportBlob`], the one value that crosses the wire.
//!
//! Everything here is `ReducerContext`-generic or `ReducerContext`-free, which is what lets
//! `harness.rs` execute the real export/import loops against a fake store (issue #37). [server]

use spacetimedb::{log, ReducerContext, SpacetimeType};

use super::TransferOut;

/// The subset of [`crate::CHARACTER_OWNED_TABLES`] marked **hot**: state the destination needs in
/// the player's first frame (worn gear, castable abilities, trained skills, the action bar). The
/// rest is **cold** — correct to stream in behind the loading screen.
///
/// Deliberate simplification: in v1 the mark is CARRIED but not ACTED ON — one blob ships
/// everything, because same-database transfers have nothing to stream. It becomes load-bearing
/// when the seam-crossing warm handoff has to fit a ~1s budget (spec #12, Phase C): cold tables
/// move after the handshake.
/// Verified against the generated enumeration by `hot_marks_name_only_real_manifest_tables`.
pub(crate) const HOT_TABLES: &[&str] = &[
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
pub(crate) const MANIFEST_EXCLUDE: &[&str] = &["game_transfer_out"];

/// The manifest tables whose rows deliberately do NOT cross a database boundary — the ONLY tables
/// whose arm may be written with the `character_owned!(not_transported, ..)` marker kind (issue #19
/// review).
///
/// The arm-exists ratchet (`every_manifest_table_can_cross_a_database_boundary`) cannot tell a
/// transport arm from a declining one, so on its own it is defeated by the one edit it exists to
/// stop: swapping a real table's arm for a decline keeps the ratchet green while every character
/// silently arrives without that table's rows. Verified by mutation — pointing
/// `sweep_transfer_game_item_instance` at `not_transported` left all 468 module tests passing while
/// deleting every player's gear on every hop.
///
/// So the "not transported" decision is written HERE, with its REASON, as well as at the table,
/// where build.rs reads it off the marker kind into `CHARACTER_OWNED_NOT_TRANSPORTED`.
/// `the_not_transported_allowlist_matches_the_arms_that_decline` fails if the two disagree in
/// either direction. Each entry needs its reason, exactly like `EXEMPT_ACCESSORS` in `tripwires.rs`:
///
/// - `game_rest_state_event` — a one-shot relay row with a GC TTL; the DURABLE rest state
///   (`resting` / `rested_xp` / `rested_since_micros`) lives on the character row and travels in
///   `character_row`.
/// - `game_breath_relay_event` — a one-shot timer/damage relay with a GC TTL; private breath
///   state is re-armed from movement after arrival, so carrying a packet would replay stale UI.
/// - `game_breath_state` — transient live-world state. The destination derives a fresh timer from
///   movement after arrival, rather than resuming a source-side underwater snapshot.
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
pub(crate) const NOT_TRANSPORTED: &[&str] = &[
    "game_rest_state_event",
    "game_breath_relay_event",
    "game_breath_state",
    "game_group_invite",
    "game_group_member",
    "game_pet_command",
    "game_character_shard",
    // A Trade Session (+ its slot rows) is a live dialog with a partner who is by definition NOT
    // transferring too — carrying it would import a negotiation the destination's partner copy
    // cannot see. It dies with the source, exactly as the logout teardown would (#120).
    "game_trade_session",
    "game_trade_slot",
];

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
/// harness in `mod harness` can drive this exact body against a fake store. See that
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
/// The registry is a PARAMETER so this loop can be executed by `mod harness` — it is the
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

/// What crosses the seam. The character itself travels whole, as `character_row` — the
/// `game_character` row serialized bsatn — so a column added to `Character` travels without anyone
/// remembering to add a field here. The character-owned TABLES travel as the manifest, derived from
/// `CHARACTER_OWNED_TABLES` (the build-time enumeration generated from the `character_owned!` delete
/// markers, so it can never drift from the sweep registry) plus the `payload` of actual rows.
///
/// The manifest is the load-bearing SCHEMA half: the destination compares it against its OWN build
/// (`manifest()`) and refuses an import from a shard whose character-owned table set differs. The
/// `payload` alongside it is the DATA half (issue #19) — the actual rows, one entry per manifest
/// table, produced by that table's `character_owned!(transfer, ..)` arm.
///
/// **Nothing that is already inside `character_row` gets a second field here (#380).** Until then
/// the blob ALSO carried `name`/`level`/`map_id`/`instance_id`/`x`/`y`/`z`/`o`/`health`/`power` as
/// typed copies — pre-#19 remnants from when the blob was a manifest and there was no serialized
/// row to read them off. Not one of them was read by anything: `apply_import_blob` builds the
/// arrival copy from `character_row` and overwrites the position from `dest_*`, `import_character`
/// (same-database) reads only the manifest, and the gateway treats the whole blob as opaque bytes.
/// A duplicated value that nothing reads is strictly worse than no value, because the next reader
/// cannot tell which copy is authoritative — the same reasoning that keeps `owner_identity` out
/// (see the REGENERATE verdict). `money` stays, and is the exception that shows the rule: it is the
/// one field that is deliberately NOT just a copy of the row, because `defer_money_delta` folds
/// post-freeze credits into it (#30's DEFER verdict) and `apply_import_blob` replays it at the
/// destination.
#[derive(SpacetimeType, Clone, Debug, PartialEq)]
pub struct ExportBlob {
    pub transfer_id: u64,
    pub character_guid: u64,
    /// The escrowed purse PLUS every `defer_money_delta` folded in after the freeze. Not redundant
    /// with `character_row`'s own `money` — that one is the value at `begin_transfer`.
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

impl TransferOut {
    /// The escrow's destination, as one value. Same-database `import_character` reads it off the
    /// local out-row; cross-database the blob carries its own copy (below), because the out-row is
    /// on the other node.
    pub(crate) fn destination(&self) -> Destination {
        Destination {
            map_id: self.dest_map_id,
            instance_id: self.dest_instance_id,
            x: self.dest_x,
            y: self.dest_y,
            z: self.dest_z,
            o: self.dest_o,
        }
    }
}

impl ExportBlob {
    /// Where the blob says the character is going.
    pub(crate) fn destination(&self) -> Destination {
        Destination {
            map_id: self.dest_map_id,
            instance_id: self.dest_instance_id,
            x: self.dest_x,
            y: self.dest_y,
            z: self.dest_z,
            o: self.dest_o,
        }
    }
}

impl crate::character::Character {
    /// Move the durable row to `dest` — the SIX fields an arrival overwrites, and no others (#380).
    ///
    /// Both halves of step 2 land here: same-database `import_character` re-partitions the shared
    /// row, cross-database `apply_import_blob` relocates the freshly decoded arrival copy. They used
    /// to be two hand-written six-line blocks a hundred lines apart, which is exactly the shape
    /// where one of them silently keeps writing `y` twice and never writes `z`. It is inherent on
    /// `Character` rather than free in this module because "relocate a character row" is a fact
    /// about the row; the transfer protocol is only its most dangerous caller.
    pub(crate) fn relocate(&mut self, dest: Destination) {
        self.map_id = dest.map_id;
        self.pending_instance_id = dest.instance_id;
        self.x = dest.x;
        self.y = dest.y;
        self.z = dest.z;
        self.orientation = dest.o;
    }
}

/// Decode an escrowed [`ExportBlob`]. One spelling of the error, in one place, for the two callers
/// that read a blob (`import_character` off the local out-row, `apply_import_blob` off the wire).
pub(crate) fn decode_blob(transfer_id: u64, bytes: &[u8]) -> Result<ExportBlob, String> {
    spacetimedb::sats::bsatn::from_slice(bytes)
        .map_err(|e| format!("transfer {transfer_id}: corrupt export blob: {e}"))
}

/// The SCHEMA-DRIFT gate: the destination compares the arriving manifest against its OWN build and
/// refuses an import from a shard whose character-owned table set differs, because such a shard
/// would otherwise silently drop the tables it does not know — with the source copy cascade-deleted
/// moments later.
///
/// Shared by both step-2 reducers (#380). It was two identical inline blocks, and the one in
/// `import_character_blob` was reachable only through a source-scan assertion on its text.
pub(crate) fn check_manifest(transfer_id: u64, arriving: &[ManifestEntry]) -> Result<(), String> {
    let mine = manifest();
    if arriving != mine {
        return Err(format!(
            "transfer {transfer_id}: manifest mismatch — source exported {} character-owned tables, \
             this shard knows {}",
            arriving.len(),
            mine.len()
        ));
    }
    Ok(())
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
