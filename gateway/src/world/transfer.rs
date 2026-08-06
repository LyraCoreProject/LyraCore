//! The gateway's half of the escrowed cross-database transfer (issue #19, spec #12 Phase A).
//!
//! The module owns the state machine (`module/src/transfer.rs`); this file owns the ORDER the two
//! databases are driven in, because that ordering is the one safety property neither database can
//! check for itself — each can only see its own ledger row.
//!
//! ```text
//!   SOURCE shard                                   DESTINATION shard
//!   1 begin_transfer  ── freeze + serialize ──►    (nothing yet)
//!                                                  2 ensure_instance        (mirror the dungeon)
//!                                                  3 import_character_blob  (rows land, fenced)
//!   4 confirm_import  ◄── "it committed" ──────
//!   5 finish_transfer ── source copy destroyed
//!                                                  6 release_transfer       (arrival goes LIVE)
//!   7 evict_instance_population (the world writer stops ticking the dungeon)
//! ```
//!
//! **Every step is idempotent**, which is what makes a killed gateway recoverable without any
//! gateway-side state: the escrow row on the source carries the transfer id, the destination and the
//! blob, so a fresh process re-derives the whole plan from durable data and re-runs the sequence
//! from the top. Replayed steps are no-ops (`BeginPlan::Replay`, the in-row PK, `FinishPlan::
//! AlreadyDone`, `release_transfer`'s missing-row arm).
//!
//! **The transfer id IS the character guid.** A character can hold at most one escrow at a time
//! (`BeginPlan::AlreadyInTransit` refuses a second), and guid 0 does not exist — so using the guid
//! makes the id re-derivable by a gateway that restarted with no memory of what it was doing. That
//! is the entire recovery mechanism: nothing about an in-flight transfer lives in gateway RAM.
//!
//! Deliberate simplification: the driver is SYNCHRONOUS and runs on the world session's own
//! thread, inside the client's loading screen (the WORLDPORT_ACK handler). Ceiling: a slow or
//! unreachable destination shard stalls that one session for the reducer timeout. Upgrade path: a
//! driver task with a work queue, once transfers happen without a loading screen to hide in (spec
//! #12 Phase C's warm handoff).

use anyhow::{anyhow, Result};

use super::WorldStore;

/// Where a character is going. Derived from the character's own durable row on the source shard —
/// `world::teleport_player` writes the DESTINATION map/instance/position there before it despawns
/// the entity for a cross-map hop, so by the time the client acks the loading screen the row
/// already describes where it belongs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransferPlan {
    pub transfer_id: u64,
    pub character_guid: u64,
    pub dest_map_id: u32,
    pub dest_instance_id: u64,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_z: f32,
    pub dest_o: f32,
}

/// An escrow row read back off a source shard: the transfer's identity, its destination, and the
/// serialized character. The `blob` is opaque to the gateway — it is produced by `begin_transfer`
/// and consumed by `import_character_blob`, and the gateway only carries it.
#[derive(Clone, Debug, PartialEq)]
pub struct EscrowedTransfer {
    pub transfer_id: u64,
    pub character_guid: u64,
    pub dest_map_id: u32,
    pub dest_instance_id: u64,
    pub blob: Vec<u8>,
}

/// The transfer id for a character. See the module doc: the guid IS the id.
pub fn transfer_id_for(character_guid: u64) -> u64 {
    character_guid
}

/// The eight step boundaries `LYRACORE_TRANSFER_ABORT_AFTER` can name, in drive order. These are the
/// `WorldStore` method names — the same vocabulary the headless crash-matrix test already speaks —
/// so a step name is greppable straight to the call it follows. The list is also the drive's own
/// sequence: `an_unset_transfer_abort_injection_changes_nothing` asserts the two are equal, so a
/// step added to the drive without a crash point here is a hole in the AC#3 matrix.
pub const ABORT_STEPS: [&str; 8] = [
    "begin_transfer",
    "ensure_instance",
    "import_character_blob",
    "confirm_import",
    "finish_transfer",
    "publish_shard_index",
    "release_transfer",
    "evict_instance_population",
];

/// Deliberate, injected death — the `kill -9` half of #19 AC#3.
///
/// **Why `abort()` and not a `panic!` or an `Err`.** AC#3 is only meaningful if the process leaves
/// behind *exactly* the durable state a real `kill -9` at this boundary would leave. That rules out
/// the two cheaper options:
///
/// * a returned `Err` is a CLEAN failure: `run_transfer`'s caller unwinds the world session's normal
///   error path (log, respond to the client, drop the session and its despawn/cleanup handlers) and
///   the gateway keeps serving. State a crash would never have produced gets written.
/// * a `panic!` unwinds ONE thread. The driver runs on the world session's own thread, so the
///   process survives, every `Drop` on that thread's stack runs, and a `catch_unwind` anywhere up
///   the stack turns the "crash" into an ordinary error.
///
/// `std::process::abort()` raises SIGABRT: the whole process dies at once, no unwinding, no `Drop`,
/// no `atexit`, uncatchable. Self-sending SIGKILL proper would need a `libc` dependency; SIGABRT is
/// the faithful stand-in available from `std` alone. Nothing is flushed either, hence the explicit
/// `log::logger().flush()` first — otherwise the loud line announcing the injection is lost.
#[cfg(not(test))]
fn die_by_injection() -> ! {
    log::logger().flush();
    std::process::abort()
}

/// Note: under `cargo test` the same site panics instead, because a `cargo test` process that
/// aborts takes the whole test binary with it and proves nothing. The env gate, the step names and
/// the ordering are identical in both builds — only the manner of death differs, and the manner of
/// death is exactly the part that cannot be asserted in-process anyway.
#[cfg(test)]
fn die_by_injection() -> ! {
    panic!("LYRACORE_TRANSFER_ABORT_AFTER: injected abort");
}

/// One crash point. `abort_after` is the (already read) value of `LYRACORE_TRANSFER_ABORT_AFTER`.
///
/// Note: unconfigured, this is `None != Some(step)` and a return — no allocation, no formatting,
/// no log, no behaviour change. The one env read that produces `abort_after` happens once per
/// transfer, and a transfer happens once per loading screen.
#[inline]
fn abort_point(abort_after: Option<&str>, step: &str, transfer_id: u64) {
    if abort_after != Some(step) {
        return;
    }
    log::error!(
        "transfer {transfer_id}: LYRACORE_TRANSFER_ABORT_AFTER={step} — step committed, ABORTING BY \
         FAULT INJECTION (#19 AC#3). This is a deliberate crash, not a bug."
    );
    die_by_injection()
}

/// Drive one escrowed transfer of `plan.character_guid` from `src` to `dst` to completion.
///
/// Re-entrant: calling it again after ANY step (including after full completion) re-runs the
/// sequence with every landed step replaying as a no-op. That is how a gateway killed mid-transfer
/// recovers — it simply calls this again at the client's next world entry.
///
/// # The ordering, and what each step's failure costs
///
/// | crash after | source | destination | recovery |
/// |---|---|---|---|
/// | 1 begin   | frozen, escrowed | nothing | re-run: begin replays, import proceeds |
/// | 3 import  | frozen, escrowed | durable + fenced | re-run: import replays, confirm proceeds |
/// | 4 confirm | frozen, attested | durable + fenced | re-run, or the SOURCE reaper rolls forward |
/// | 5 finish  | gone | durable + fenced | re-run: `settle` releases the arrival copy — but realm-core's index is STALE and the re-run does not republish it (step 5b) |
/// | 5b publish | gone | durable + fenced | same as 5, and the index IS correct |
/// | 6 release | gone | LIVE | done; step 7 is cleanup |
///
/// No reachable point has zero durable copies, and no reachable point has the character LIVE on
/// both databases: the source is frozen from step 1 (its escrow row fences it) and destroyed at 5,
/// and the destination is fenced from 3 until 6.
///
/// # Fault injection (#19 AC#3)
///
/// The whole seven-step drive commits in ~17ms live, so "`kill -9` the gateway at step N" is not a
/// thing a log-watcher can land. `LYRACORE_TRANSFER_ABORT_AFTER=<step>` makes each crash point
/// deterministic instead: the named step runs, commits, and then the process aborts (see
/// [`die_by_injection`]). The accepted names are [`ABORT_STEPS`]. Unset — the only configuration any
/// real run has — costs one `env::var` per transfer and seven `Option<&str>` compares that all miss.
pub fn run_transfer(src: &dyn WorldStore, dst: &dyn WorldStore, plan: &TransferPlan) -> Result<()> {
    // Deliberate simplification: ONE env read, threaded down as a plain `Option<&str>` rather than
    // re-read at each step — which also lets the tests drive every crash point without mutating
    // process-global env underneath a parallel test runner.
    let abort_after = std::env::var("LYRACORE_TRANSFER_ABORT_AFTER").ok();
    // A typo'd step name would silently never fire, and the crash matrix would report a PASS for a
    // crash that never happened. Say so — only ever reached when the operator opted in.
    if let Some(step) = abort_after.as_deref() {
        if !ABORT_STEPS.contains(&step) {
            log::error!(
                "LYRACORE_TRANSFER_ABORT_AFTER={step} names no transfer step — NOTHING will abort. \
                 Valid steps: {ABORT_STEPS:?}"
            );
        }
    }
    run_transfer_injected(src, dst, plan, abort_after.as_deref())
}

/// `run_transfer` with the fault-injection step passed in rather than read from the environment, so
/// the tests can drive every crash point without mutating process-global env.
pub(super) fn run_transfer_injected(
    src: &dyn WorldStore,
    dst: &dyn WorldStore,
    plan: &TransferPlan,
    abort_after: Option<&str>,
) -> Result<()> {
    log::info!(
        "transfer {}: character {} {} -> {} (map {} instance {})",
        plan.transfer_id,
        plan.character_guid,
        src.shard_name(),
        dst.shard_name(),
        plan.dest_map_id,
        plan.dest_instance_id
    );

    // 1. FREEZE + SERIALIZE on the source, in one transaction. Idempotent on the transfer id.
    src.begin_transfer(plan)?;
    abort_point(abort_after, "begin_transfer", plan.transfer_id);

    // Read the escrow back rather than trusting `plan`: after a resume the row on disk is the
    // authority (it holds the blob, and its destination is the one the escrow was opened for).
    let escrow = src.escrowed_transfer(plan.character_guid).ok_or_else(|| {
        anyhow!(
            "transfer {}: begin_transfer reported success but no escrow row is readable on {} — \
             refusing to import a character whose source copy is not frozen",
            plan.transfer_id,
            src.shard_name()
        )
    })?;

    // 2. Mirror the instance BEFORE the import: the arriving character carries a
    //    `game_instance_binding` naming this id, and `player_login`'s stranding guard diverts a
    //    character whose `pending_instance_id` names an instance that does not exist here.
    if escrow.dest_instance_id != 0 {
        // Deliberate simplification: party_id 0 (solo). The destination only reads it in
        // `resolve_or_create_instance`'s "the party's live instance" arm, which nothing on the
        // instance shard reaches — every member arrives with their own binding, which resolves
        // first. Upgrade path: realm-core (#22) owns party ids across shards.
        dst.ensure_instance(escrow.dest_instance_id, escrow.dest_map_id, 0)?;
    }
    // Outside the `if`: the boundary exists whether or not the hop needed an instance mirrored, and
    // a same-map transfer must still be crashable "after ensure_instance".
    abort_point(abort_after, "ensure_instance", escrow.transfer_id);

    // 3. MATERIALISE at the destination. Idempotent on the transfer id (the in-row PK).
    dst.import_character_blob(escrow.transfer_id, &escrow.blob)?;
    abort_point(abort_after, "import_character_blob", escrow.transfer_id);

    // 4. ATTEST on the source — and ONLY here, only because step 3 returned Ok. This is the
    //    ordering obligation the module cannot check for itself: `confirm_import` files the in-row
    //    that licenses `finish_transfer` to destroy the source copy, so attesting before a
    //    successful import would delete a character with nowhere to arrive.
    src.confirm_import(escrow.transfer_id)?;
    abort_point(abort_after, "confirm_import", escrow.transfer_id);

    // 5. DELETE-LAST: the source copy is cascade-deleted and the escrow cleared.
    src.finish_transfer(escrow.transfer_id)?;
    abort_point(abort_after, "finish_transfer", escrow.transfer_id);

    // 5b. PUBLISH the character→shard index to REALM-CORE (issue #34, #20 AC#3).
    //
    // Step 5's own transaction wrote this same `(guid, map, instance)` into the SOURCE database's
    // `game_character_shard` — but there is no transaction spanning two SpacetimeDB databases, so
    // realm-core's copy (the authoritative one, the one `home_shard` reads) cannot be written from
    // inside it. This replicates it, and the placement is the whole of what makes it safe rather
    // than a stale-index generator: it runs only after `finish_transfer` returned `Ok`, and it
    // publishes the ESCROW's own destination fields — the same fields `do_finish` recorded from —
    // so it can never name a destination for a transfer that did not settle.
    //
    // `?`, not best-effort: an index that silently stops being written is exactly the rot this
    // ticket exists to remove. What a failure here costs, precisely — because `?` here is NOT free:
    // steps 6 and 7 do not run, `run_transfer` returns Err, `MSG_MOVE_WORLDPORT_ACK` answers
    // `SMSG_TRANSFER_ABORTED` and ends the session, and the character sits WHOLE BUT FENCED at the
    // destination until the next login, whose `settle_transfer` takes the holder-is-owner arm and
    // drops the fence. Nothing is lost or duplicated — but a player is kicked off a loading screen
    // for a directory write, which is a real cost rather than the "not a login this could newly
    // break" an earlier draft of this comment claimed. It fails on an unreachable realm-core (the
    // world handshake fails closed on that too, but the handshake happened earlier in the session
    // and realm-core can die in between) and on a module-side REFUSAL of `set_character_shard` —
    // it is operator-gated.
    //
    // RESIDUAL WINDOW, stated honestly (adversarial review of this PR). If the gateway dies — or
    // this call fails — between 5 and 5b, realm-core's index still names the OLD shard, and the
    // recovery above does NOT repair it: it never re-enters `run_transfer`, so step 5b never runs
    // again for that transfer. The fallback is `settle_home_shard`'s own holder lookup
    // (`realm_core::locate_home_shard`, issue #47), which probes and heals a stale entry on the
    // character's NEXT WORLD ENTRY — not the next completed transfer, as it was before #47, when
    // this fallback hung off the unreachable `WorldStore::home_shard` (`settle_home_shard` scanned
    // the connected shards unconditionally and never touched the index at all). The window is still
    // real between here and that next login; it is no longer indefinite.
    src.publish_shard_index(
        escrow.character_guid,
        escrow.dest_map_id,
        escrow.dest_instance_id,
    )?;
    abort_point(abort_after, "publish_shard_index", escrow.transfer_id);

    // 6. RELEASE: the arrival copy's fence drops and the character is live at the destination.
    dst.release_transfer(escrow.transfer_id)?;
    abort_point(abort_after, "release_transfer", escrow.transfer_id);

    // 7. The world writer stops paying for the dungeon (#19 AC#2). Deliberately LAST and
    //    best-effort: the character is already whole on the destination, so a failure here is a
    //    performance wart (an idle population on the source until its 30-minute empty reap), never
    //    a correctness one — and failing the login over it would be strictly worse for the player.
    if escrow.dest_instance_id != 0 {
        if let Err(e) = src.evict_instance_population(escrow.dest_instance_id) {
            log::warn!(
                "transfer {}: could not evict instance {} population from {} ({e:#}) — the run is \
                 fine, but that shard keeps ticking a dungeon nobody is in until the reaper takes it",
                escrow.transfer_id,
                escrow.dest_instance_id,
                src.shard_name()
            );
        }
    }
    abort_point(abort_after, "evict_instance_population", escrow.transfer_id);
    Ok(())
}

/// Put `character_guid` on the shard that owns its location, if it is not there already, and clear
/// any escrow left behind by an earlier crashed attempt. Called at every world entry.
///
/// `holder` is the shard whose durable `game_character` row the character currently lives in;
/// `owner` is the shard the shard map says owns its location. When they are the same shard this is
/// the no-op path plus one cheap `release_transfer`, which clears the arrival fence in the ONE
/// crash window that leaves it behind (killed between `finish_transfer` and `release_transfer` — the
/// source copy is already gone, so there is no escrow row anywhere to re-drive from, and without
/// this the character would be fenced out of its own login forever).
pub fn settle_transfer(
    holder: &dyn WorldStore,
    owner: &dyn WorldStore,
    character_guid: u64,
) -> Result<()> {
    let transfer_id = transfer_id_for(character_guid);
    if holder.shard_name() == owner.shard_name() {
        return owner.release_transfer(transfer_id);
    }
    let escrow = holder.escrowed_transfer(character_guid);
    if escrow.is_none() {
        // The holder has no source claim of its own — so any ledger row filed HERE under this id
        // can only be an unreleased ARRIVAL fence, left by an earlier inbound hop whose
        // `release_transfer` never ran. That matters now the transfer id IS the character guid:
        // the module's `plan_begin` reads the out-row OR the in-row as "this id is already
        // escrowed for this character" and answers `BeginPlan::Replay`, so `begin_transfer` below
        // would report success while freezing nothing, and the character could never leave this
        // shard again. Clearing it first is safe by construction — `release_transfer` refuses
        // outright while a local out-row exists, and we have just proved there is none.
        holder.release_transfer(transfer_id)?;
    }
    // Resume the escrow if one exists — its destination, not the character row's, is the
    // authority, because the escrow was opened against it and the destination may already hold an
    // imported copy filed under that id.
    let plan = match escrow {
        Some(e) => TransferPlan {
            transfer_id: e.transfer_id,
            character_guid,
            dest_map_id: e.dest_map_id,
            dest_instance_id: e.dest_instance_id,
            // The escrow already carries the destination position inside its blob; these are only
            // used by a FRESH `begin_transfer`, which this resume path never reaches (begin
            // replays).
            dest_x: 0.0,
            dest_y: 0.0,
            dest_z: 0.0,
            dest_o: 0.0,
        },
        None => holder
            .character_destination(character_guid)
            .ok_or_else(|| {
                anyhow!(
                    "character {character_guid} is not on {} and has no escrow there — nothing to \
                 transfer from",
                    holder.shard_name()
                )
            })?,
    };
    run_transfer(holder, owner, &plan)
}
