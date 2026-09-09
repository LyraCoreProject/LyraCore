//! Escrowed character transfer — the ONE primitive behind every sharding granularity (
//! spec instance entry, continent travel, seam crossings, bulk region migration).
//!
//! # Where the pieces live
//!
//! | file | what is in it |
//! |---|---|
//! | `mod.rs` (here) | the ledger tables, the pure decision core, the SINKS every step is written against, and the reducers that bind them to a `ReducerContext` |
//! | `transport.rs` | the row transport: `RowIo`/`move_rows`/the bsatn codec, the manifest, and `ExportBlob` — everything that answers "what crosses, and in what shape" |
//! | `tests.rs` | the crash matrix, the pure-planner enumerations, and the source-scan tripwires |
//! | `harness.rs` | `FakeDb` — two in-memory "databases" the real protocol bodies are EXECUTED against |
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
//! (the real cross-database move) is built on top of this claim:
//!
//! 1. `helpers::entity_by_owner` — the ACTOR side. Every player-fired reducer resolves "who is
//!    acting" through it, so an in-transit character can take no action itself.
//! 2. `world::player_login` — the RE-MATERIALISATION side. The one path that could put a live entity
//!    back on a shard the character has left (the dual-liveness dupe).
//! 3. `begin_transfer`'s delete of the live `game_world_entity` row — the TARGET side. The ~50
//!    hand-rolled `map_id`/`instance_id` gates, the aggro candidate scan, the threat lists and the
//!    AOI relay all resolve through that row, so they stop seeing the character by construction.
//! 4. `helpers::character_by_guid` / `character_by_name` — the BY-GUID side. Reducers
//!    that reach a character by guid or by name straight into `game_character` (or into a
//!    character-owned table) touch none of the first three; this pair is the gate they route
//!    through, and it reads an in-transit character as ABSENT so each caller's existing
//!    "no such character" arm fires (no new error string, no gateway edit).
//!
//! # The by-guid verdict table
//!
//! REFUSE is not the right answer everywhere, so the class is settled per path. Four verdicts exist:
//!
//! * **REFUSE** — route through `character_by_guid`/`character_by_name` (or, for the two background
//!   tick passes, carry an [`is_in_transit`] gate directly). The default, and what every fenced
//!   reducer in the tree does.
//! * **DEFER into the blob** — `loot::credit_purse`, via [`defer_money_delta`]. Refusing would drop
//!   a THIRD party's copper: the recipient is a party member collecting their share of someone
//!   else's kill and cannot know why they were shorted.
//! * **REGENERATE at the destination** — `auth::establish_session`, deliberately unfenced.
//!   `Character.owner_identity` is per-CONNECTION derived state; a carried copy would arrive stale
//!   and be overwritten immediately, and a field that is always wrong on arrival is worse than no
//!   field. Pinned by `owner_identity_is_regenerated_at_the_destination_never_carried`.
//! * **NOT A TRANSFER CONCERN** — the group verbs, settled: party membership is
//!   authoritative on realm-core, so there is no source-copy write left to lose. What replaced the
//!   fence question is a REPLICATION one, answered by putting `game_group_member` on
//!   [`NOT_TRANSPORTED`] and having the gateway re-push realm-core's roster at world entry.
//!
//! The audited exception list is EMPTY: every by-guid path in the tree is either fenced or holds one
//! of the three other verdicts. **The issue-by-issue reasoning for each one — which path, which
//! field it would have lost, why its verdict is what it is — is archived at
//! `docs/history/transfer-by-guid-verdict-table.md`.** Read it before deciding a NEW path's
//! verdict; nothing in it is stale, it is simply not something the protocol's own source has to
//! restate.
//!
//! Enforcement: each fenced call site is pinned by a named source-scan tripwire in `tests.rs`
//! (deleting a fence turns a NAMED test red — a pure model has no reducers in it and sees none of
//! them), and `tripwires.rs`'s `character_fence_tripwire` is the ratchet that stops a NEW unfenced
//! by-guid path from being added.
//!
//! # The CROSS-DATABASE protocol
//!
//! Shipped the state machine within ONE database (two `instance_id` partitions, so
//! `import_character` re-partitions a row rather than materialising a second). The cross-database work makes the move
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
//! What is still parked: the LIVE two-database run. Every acceptance criterion needs a real
//! 1.12.1 client, two published databases and an operator — What is proven
//! headlessly is the state machine (the crash matrix in `tests.rs`, including the cross-database
//! sequence), the transport ratchet, and the Escrow sequence executed across two `FakeDb`s in
//! `harness.rs`.
//!
//! # Why every step is written against a SINK
//!
//! `ReducerContext` cannot be constructed in a unit test, so before the sinks existed the only thing
//! a test in this crate could do to a reducer body was read its TEXT. The review ran 21
//! mutations against that surface and 17 left the suite green. Each step therefore has its body
//! written against a trait — [`BeginSink`], [`ImportSink`], [`FinishSink`], [`ReapSink`], all over
//! the shared [`ShardLedger`] — with [`CtxShard`] as the one production adapter and
//! `harness::FakeDb` as the test one. The reducers are two lines each; everything they used to
//! contain is executed for real in `harness.rs`.
//!
//! Everything in the "pure core" section below is `ReducerContext`-free on purpose: it is the whole
//! decision surface of the protocol, so the crash matrix drives the SAME functions the reducers
//! execute. [server]

mod transport;
pub(crate) use transport::*;

#[cfg(test)]
mod harness;
#[cfg(test)]
mod tests;

use spacetimedb::{
    log, reducer, table, Identity, ReducerContext, ScheduleAt, Table, TimeDuration, Timestamp,
};

use crate::auth::game_guid_range;
use crate::helpers::require_operator;
use crate::{game_character, game_creature_spline, game_world_entity};

const BOT_TRANSFER_CLAIM_MICROS: i64 = 5_000_000;

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

// ===========================================================================================
//  Ledger tables
// ===========================================================================================

/// SOURCE-side escrow: "this character is frozen here, its state is serialized in `blob`, and it is
/// on its way to `dest_*`". Its existence IS the in-transit flag.
///
/// Note: the spec asks for the in-transit flag as a COLUMN on `game_character`. It lives here
/// instead because `game_character` is gateway-subscribed with a HAND-MAINTAINED binding
/// (`docs/danger-zones.md` §1.2 / §1.6) — a column there needs a lockstep gateway edit, which is
/// the tree, not this one's. The ledger row is a strictly stronger flag anyway (it carries
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
    /// **Is the destination a different DATABASE?** Two things hang off it, and both
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
    /// END-appended + `#[default(false)]` → additive auto-migration; every earlier escrow row reads
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
    /// Exact session-less crossing identity. Zero values describe a human crossing or a row from
    /// before bot arrival identity was added.
    #[default(0u64)]
    pub bot_intent_id: u64,
    #[default(0u64)]
    pub bot_controller_generation: u64,
    #[default(0i64)]
    pub bot_intent_created_micros: i64,
    #[default(Identity::ZERO)]
    pub bot_intent_source: Identity,
    /// Realm-core predecessor captured before this arrival was imported. New Gateway-driven
    /// crossings always set these fields; zero describes a row from before exact recovery.
    #[default(0u32)]
    pub source_map_id: u32,
    #[default(0u64)]
    pub source_instance_id: u64,
    #[default(0u64)]
    pub source_locator_revision: u64,
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
// per file is what the `tripwires.rs` tripwire requires. NOTE: this puts `game_transfer_out` into the
// generated `CHARACTER_OWNED_TABLES` enumeration, which `MANIFEST_EXCLUDE` above then filters back
// out of the export blob — the escrow is machinery, not character data.
crate::character_owned!(delete, fn sweep_delete_game_transfer_out(ctx, character_guid) {
    let out = ctx.db.game_transfer_out();
    let rows: Vec<_> = out.by_character().filter(character_guid).collect();
    let crossing = rows.iter().any(|row| row.cross_database);
    if !crossing {
        let intents = ctx.db.game_bot_transfer_intent();
        let ids: Vec<_> = intents
            .by_bot()
            .filter(character_guid)
            .take(lyracore_shared::transfer::BOT_TRANSFER_PENDING_LIMIT + 1)
            .map(|intent| intent.id)
            .collect();
        for id in ids {
            intents.id().delete(id);
        }
    }
    let ids: Vec<u64> = rows.into_iter().map(|r| r.transfer_id).collect();
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
//  Session-less crossings — the intent a Package writes and the Gateway executes
// ===========================================================================================

/// A Shard crossing decided for a Character that has no Session: a playerbot whose party walked
/// through a dungeon portal, and the same row again when the party walks back out.
///
/// The escrowed transfer needs no client, but it does need a driver, and every driver we had was a
/// World Session: the client acks its loading screen (`MSG_MOVE_WORLDPORT_ACK`) and the Gateway
/// drives the nine steps inside it. A bot has nobody to ack, so this row is its ack:
/// the bounded Coordinator dispatcher picks it up and drives the identical sequence against the
/// Shard the Shard Map gives the destination.
///
/// Private — no client ever needs to see this, only the Gateway's owner-token Coordinator
/// connection. The row remains until the Gateway completes its exact claim, so reconnect and
/// process restart can resume the crossing from durable state. [server]
#[table(
    accessor = game_bot_transfer_intent,
    index(accessor = by_bot, btree(columns = [bot_guid]))
)]
pub struct BotTransferIntent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub bot_guid: u64,
    pub destination_map: u32,
    /// 0 for the open world.
    pub destination_instance: u64,
    /// Why the Package decided to move this bot, for the Gateway's log line. Never parsed.
    pub reason: String,
    pub created_at: Timestamp,
    /// The Package runner generation normalized for this crossing. Zero is the legacy/debug
    /// generation and is also the compatible value for rows written before this field existed.
    #[default(0u64)]
    pub controller_generation: u64,
    /// The Gateway worker that currently owns this attempt. Zero means unclaimed.
    #[default(0u64)]
    pub claim_token: u64,
    /// End of the claim lease in Module time. An expired claim may be replaced after restart.
    #[default(0i64)]
    pub claim_until_micros: i64,
    /// The exact claimed crossing completed destination preparation. The Gateway writes this on
    /// the source before it drops the destination's arrival fence, so a retry can finish the
    /// source intent even after the Character has already crossed onward.
    #[default(false)]
    pub arrival_ready: bool,
    /// Module identity of the World Shard that owns this durable intent. Intent ids are only
    /// Shard-local, so this is part of the crossing identity on the destination fence.
    #[default(Identity::ZERO)]
    pub source_module_identity: Identity,
    /// Realm locator predecessor captured before this source is escrowed. The Gateway binds the
    /// revision under the active claim, then Realm-core compare-and-sets from this exact partition.
    #[default(0u32)]
    pub source_map: u32,
    #[default(0u64)]
    pub source_instance: u64,
    #[default(0u64)]
    pub source_locator_revision: u64,
}

/// Send a session-less Character to `destination`, through the Gateway.
///
/// Two writes, and they must be one transaction, which is why this is a helper and not two calls a
/// Package makes in order:
///
/// 1. [`crate::world::teleport_player`] moves the durable `game_character` row to the destination
///    and despawns the live entity, exactly as a player's portal step does. That row is where
///    `begin_transfer` reads the destination position from, so the Gateway may never observe the
///    intent before it.
/// 2. The intent row, which is what the Gateway observes.
///
/// Admission uses the ordinary session-less ownership Gate and requires a live body before it
/// cancels source work, places the Character, and records the intent. A Refusal therefore changes
/// none of those rows.
///
/// On a realm with one Shard the crossing is already complete when this returns: the character row
/// names the new map, and the relay finds nothing to cross. The Package writes the same code for
/// both topologies.
///
/// Its callers are the `packages/` drop-ins (see the `package_only!` macro in `actor.rs`) and the
/// `debug_bot_transfer` lever, which is how the crossing is accepted without a Package installed. A
/// build with neither is a designed state, not dead code.
#[cfg_attr(
    all(not(has_packages), not(feature = "debug_reducers")),
    allow(dead_code)
)]
pub(crate) fn emit_bot_transfer_intent(
    ctx: &ReducerContext,
    bot_guid: u64,
    destination: Destination,
    reason: &str,
    controller_generation: u64,
) -> Result<u64, crate::actor::ActionRefusal> {
    let existing = bot_transfer_intent_gate(ctx, bot_guid, destination, controller_generation)?;
    if let Some(intent_id) = existing {
        return Ok(intent_id);
    }
    let source = ctx
        .db
        .game_world_entity()
        .guid()
        .find(bot_guid)
        .expect("the Transfer Intent Gate requires a live Character");
    let intents = ctx.db.game_bot_transfer_intent();
    if let Some(cast) = crate::spell::pending_cast(ctx, bot_guid) {
        crate::spell::cancel_cast_attempt(ctx, bot_guid, cast.scheduled_id);
    }
    let _ = crate::actor::stop_attack(ctx, bot_guid);
    ctx.db.game_creature_spline().guid().delete(bot_guid);
    crate::world::teleport_player(
        ctx,
        bot_guid,
        destination.map_id,
        destination.instance_id,
        destination.x,
        destination.y,
        destination.z,
        destination.o,
    );
    let intent = intents.insert(BotTransferIntent {
        id: 0,
        bot_guid,
        destination_map: destination.map_id,
        destination_instance: destination.instance_id,
        reason: reason.to_string(),
        created_at: ctx.timestamp,
        controller_generation,
        claim_token: 0,
        claim_until_micros: 0,
        arrival_ready: false,
        source_module_identity: ctx.database_identity(),
        source_map: source.map_id,
        source_instance: source.instance_id,
        source_locator_revision: 0,
    });
    Ok(intent.id)
}

/// Check every fallible Transfer Intent condition before a caller writes prerequisite state.
/// Returning an existing id preserves the writer's idempotence without repeating the placement.
pub(crate) fn bot_transfer_intent_gate(
    ctx: &ReducerContext,
    bot_guid: u64,
    destination: Destination,
    controller_generation: u64,
) -> Result<Option<u64>, crate::actor::ActionRefusal> {
    use crate::actor::{ActionRefusal, ActionRefusalKind};
    let intents = ctx.db.game_bot_transfer_intent();
    let pending: Vec<_> = intents.by_bot().filter(bot_guid).take(2).collect();
    if pending.len() > 1 {
        return Err(ActionRefusal::new(
            ActionRefusalKind::TransferPending,
            "Character has conflicting Transfer Intents",
        ));
    }
    if let Some(existing) = pending.first() {
        if existing.destination_map == destination.map_id
            && existing.destination_instance == destination.instance_id
            && existing.controller_generation == controller_generation
        {
            return Ok(Some(existing.id));
        }
        return Err(ActionRefusal::new(
            ActionRefusalKind::TransferPending,
            "Character already has a pending Transfer Intent",
        ));
    }
    if intents
        .iter()
        .take(lyracore_shared::transfer::BOT_TRANSFER_PENDING_LIMIT)
        .count()
        == lyracore_shared::transfer::BOT_TRANSFER_PENDING_LIMIT
    {
        return Err(ActionRefusal::new(
            ActionRefusalKind::TransferPending,
            "World Shard has too many pending Transfer Intents",
        ));
    }
    crate::sessionless::action_gate(ctx, bot_guid)?;
    ctx.db
        .game_world_entity()
        .guid()
        .find(bot_guid)
        .ok_or_else(|| {
            ActionRefusal::new(ActionRefusalKind::MissingActor, "Character is not in world")
        })?;
    Ok(None)
}

/// Bind the Realm locator predecessor while this worker owns the Transfer Intent. This happens
/// before escrow begins, so a later Realm compare-and-set can reject a worker from an old crossing.
#[reducer]
#[allow(clippy::too_many_arguments)] // Exact intent claim and Realm predecessor are one wire Gate.
pub fn bind_bot_transfer_locator(
    ctx: &ReducerContext,
    intent_id: u64,
    bot_guid: u64,
    controller_generation: u64,
    claim_token: u64,
    source_map: u32,
    source_instance: u64,
    source_locator_revision: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    if source_locator_revision == 0 {
        return Err("Realm locator revision 0 is invalid".to_string());
    }
    let intents = ctx.db.game_bot_transfer_intent();
    let mut intent = intents
        .id()
        .find(intent_id)
        .ok_or_else(|| "Transfer Intent is gone".to_string())?;
    if (
        intent.bot_guid,
        intent.controller_generation,
        intent.claim_token,
    ) != (bot_guid, controller_generation, claim_token)
        || claim_token == 0
    {
        return Err("Transfer Intent claim changed".to_string());
    }
    if (intent.source_map, intent.source_instance) != (source_map, source_instance) {
        return Err("Transfer Intent source partition changed".to_string());
    }
    if intent.source_locator_revision != 0
        && intent.source_locator_revision != source_locator_revision
    {
        return Err("Transfer Intent Realm locator changed".to_string());
    }
    intent.source_locator_revision = source_locator_revision;
    intents.id().update(intent);
    Ok(())
}

/// Claim one exact Transfer Intent. A live foreign lease normally spaces competing drivers; after
/// expiry, overlapping retries remain safe because the Escrow sequence is idempotent and only the
/// exact current claim can advance or complete this row.
#[reducer]
pub fn claim_bot_transfer_intent(
    ctx: &ReducerContext,
    intent_id: u64,
    bot_guid: u64,
    controller_generation: u64,
    claim_token: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    if claim_token == 0 {
        return Err("Transfer claim token 0 is invalid".to_string());
    }
    let intents = ctx.db.game_bot_transfer_intent();
    let mut pending: Vec<_> = intents.by_bot().filter(bot_guid).take(2).collect();
    if pending.len() != 1 || pending[0].id != intent_id {
        return Err("Character does not have one exact Transfer Intent".to_string());
    }
    let mut intent = pending.pop().expect("one Transfer Intent checked above");
    if (intent.bot_guid, intent.controller_generation) != (bot_guid, controller_generation) {
        return Err("Transfer Intent identity changed".to_string());
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    if intent.source_module_identity == Identity::ZERO {
        intent.source_module_identity = ctx.database_identity();
    }
    if intent.claim_token != 0
        && intent.claim_token != claim_token
        && intent.claim_until_micros > now
    {
        return Err("Transfer Intent is claimed".to_string());
    }
    intent.claim_token = claim_token;
    intent.claim_until_micros = now.saturating_add(BOT_TRANSFER_CLAIM_MICROS);
    intents.id().update(intent);
    Ok(())
}

/// Record that destination party state is ready before its arrival fence drops.
#[reducer]
pub fn mark_bot_transfer_arrival_ready(
    ctx: &ReducerContext,
    intent_id: u64,
    bot_guid: u64,
    controller_generation: u64,
    claim_token: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let intents = ctx.db.game_bot_transfer_intent();
    let mut intent = intents
        .id()
        .find(intent_id)
        .ok_or_else(|| "Transfer Intent is gone".to_string())?;
    if (
        intent.bot_guid,
        intent.controller_generation,
        intent.claim_token,
    ) != (bot_guid, controller_generation, claim_token)
        || claim_token == 0
    {
        return Err("Transfer Intent claim changed".to_string());
    }
    intent.arrival_ready = true;
    intents.id().update(intent);
    Ok(())
}

/// Complete only the Transfer Intent claimed by this worker and runner generation.
#[reducer]
pub fn complete_bot_transfer_intent(
    ctx: &ReducerContext,
    intent_id: u64,
    bot_guid: u64,
    controller_generation: u64,
    claim_token: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    let intents = ctx.db.game_bot_transfer_intent();
    let intent = intents
        .id()
        .find(intent_id)
        .ok_or_else(|| "Transfer Intent is gone".to_string())?;
    if (
        intent.bot_guid,
        intent.controller_generation,
        intent.claim_token,
    ) != (bot_guid, controller_generation, claim_token)
        || claim_token == 0
    {
        return Err("Transfer Intent claim changed".to_string());
    }
    if !intent.arrival_ready {
        return Err("Transfer Intent arrival is not ready".to_string());
    }
    intents.id().delete(intent_id);
    Ok(())
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
/// The fallback is not tidiness. Now that the transfer id IS the character guid, a database
/// holding only an unreleased ARRIVAL in-row — the state left by a driver killed between
/// `finish_transfer` and `release_transfer` — would otherwise read as an UNUSED id. `plan_begin`
/// would then answer `Escrow`... except it never gets there, because [`is_in_transit`] sees the
/// in-row and answers `AlreadyInTransit`, so the character is refused a transfer OUT of the shard
/// it is stuck on, permanently, with no operator recourse (the review's blocker 2). Reading the
/// in-row here is what turns that into the `Replay` the `settle_transfer` fence-clear then repairs.
///
/// Pure, so it is pinned by a real assertion rather than by the reducer's text — the whole line was
/// a mutation survivor.
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
/// Note: cross-database, the answer comes from the gateway querying the destination
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
/// `instance::occupied_instances` (REFUSE verdict) so the instance reaper cannot tear
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

/// DEFER verdict — fold a post-`begin_transfer` `money` credit into the escrowed export
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
/// need a destination-side replay hop, and that hop belongs to the cross-database work (it owns
/// the gateway leg that would carry it). Folding unconditionally here means the final blob is
/// already correct when that work wires `finish_transfer` to ship residual deltas — no call-site
/// edit at that point.
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
//  The SINKS — every step's database, reduced to exactly what that step touches
// ===========================================================================================

/// The escrow LEDGER on whichever database a step is being driven against, plus the one fact about
/// the character row every step needs: does it exist here?
///
/// Supertrait of the four per-step sinks, so a row one step writes means the same thing to the next
/// and no method is declared twice. [`CtxShard`] is the production adapter, `harness::FakeDb` the
/// test one — and the harness runs the REAL step bodies, not models of them.
pub(crate) trait ShardLedger {
    /// `game_transfer_out` under this id, if any.
    fn out_row(&self, transfer_id: u64) -> Option<TransferOut>;
    /// `game_transfer_in` under this id, if any.
    fn in_row(&self, transfer_id: u64) -> Option<TransferIn>;
    fn file_out_row(&mut self, row: TransferOut);
    fn file_in_row(&mut self, row: TransferIn);
    fn delete_out_row(&mut self, transfer_id: u64);
    fn delete_in_row(&mut self, transfer_id: u64);
    /// Is there a durable `game_character` row for this guid here?
    fn has_character(&self, guid: u64) -> bool;
    fn now_micros(&self) -> i64;

    /// Clear BOTH escrow rows. **The out-row goes LAST**: it is the source's claim on the
    /// character, and it is the row whose absence unfreezes the character everywhere
    /// ([`is_in_transit`] reads either one). Provided here rather than per-adapter so that order is
    /// written once and EXECUTED by the harness, instead of being a property of two one-line
    /// adapter bodies no test could reach.
    fn clear_escrow(&mut self, transfer_id: u64) {
        self.delete_in_row(transfer_id);
        self.delete_out_row(transfer_id);
    }
}

/// What **step 1** touches, on the SOURCE database.
pub(crate) trait BeginSink: ShardLedger {
    /// A direct taxi flight is module-owned movement and cannot cross a shard boundary.
    fn has_active_taxi_flight(&self, guid: u64) -> bool;
    /// Is this character mid-transfer under ANY id? The by-character lookup no per-id check can do
    /// (see [`plan_begin`]'s `AlreadyInTransit`).
    fn is_in_transit(&self, guid: u64) -> bool;
    /// Persist the live `game_world_entity` into the durable character row and DELETE it — the
    /// single delete that makes the character invisible to every targeting, aggro, threat and
    /// AOI-relay gate in the module. A no-op when there is no live entity here (an offline
    /// character is a perfectly normal thing to escrow).
    fn freeze_live_entity(&mut self, guid: u64);
    /// [`export_rows`] against this database's transport registry.
    fn export_rows(&self, guid: u64) -> Vec<TableRows>;
    /// Run `f` against the durable character row, if there is one. A closure rather than a returned
    /// row because `Character` is a table struct and is deliberately not `Clone`.
    fn with_character<T>(
        &self,
        guid: u64,
        f: impl FnOnce(&crate::character::Character) -> T,
    ) -> Option<T>;
    /// Arm the reaper schedule if it is not armed yet.
    fn arm_reaper(&mut self);
}

/// What **step 2, cross-database** touches, on the DESTINATION database.
///
/// This trait IS the seam. `ReducerContext` cannot be constructed in a unit test, so
/// before it existed nothing in the crate could execute the import — its guards were pinned by
/// source scans that matched their own text, and 17 of 21 mutations against them left the suite
/// green.
pub(crate) trait ImportSink: ShardLedger {
    /// Is there a LIVE `game_world_entity` for this guid here?
    fn has_live_entity(&self, guid: u64) -> bool;
    fn cascade_delete_character(&mut self, guid: u64);
    fn insert_character(&mut self, c: crate::character::Character);
    /// The payload half — [`import_rows`] against this database's transport registry.
    fn import_rows(&mut self, guid: u64, payload: &[TableRows]) -> Result<(), String>;
    fn ensure_shadow_account(&mut self, account_id: u64);
    /// Ratchet THIS database's guid high-water mark up to at least `guid` (AC#3) — so a
    /// locally created character can never later collide with one this database received by import.
    /// Unconditional, same as `world::cascade_delete_character`'s call through this same method —
    /// the GATE on whether an arriving guid is even allowed to reach here lives in
    /// [`apply_import_blob`], not in this method, precisely so the two sinks can never
    /// diverge on it (see `own_guid_range`'s doc comment).
    fn bump_guid_high_water(&mut self, guid: u64);
    /// THIS database's own `game_guid_range` (`(base, size)`), if one is installed. A FACT query,
    /// not a decision — the [`apply_import_blob`] is what turns it into "should this
    /// arrival ratchet the allocator" via `auth::in_guid_range`, so both sinks share the one
    /// decision instead of each re-implementing it. Putting the gate here instead would let a
    /// `FakeDb` that forgot to reimplement it silently validate a shard-poisoning import as green.
    fn own_guid_range(&self) -> Option<(u64, u64)>;
}

/// What **step 3** touches, on the SOURCE database.
///
/// The same seam as [`ImportSink`], for the same reason (extended).
/// [`apply_finish`]'s ORDER — detach, then cascade, then write the forwarding receipt, then delete
/// the escrow — was pinned only by source scans that matched their own text, and the ordering
/// constraint is not cosmetic: `game_character_shard` is itself character-owned, so a cascade that
/// ran after `record_shard` would wipe the receipt it had just written.
pub(crate) trait FinishSink: ShardLedger {
    /// `group::detach_for_transfer` — raw membership removal: no leader transfer, no disband.
    fn detach_for_transfer(&mut self, guid: u64);
    fn cascade_delete_character(&mut self, guid: u64);
    /// `realm_core::record_shard` — this database's forwarding receipt for the character.
    fn record_shard(&mut self, guid: u64, map_id: u32, instance_id: u64);
}

/// What the REAPER touches: everything [`apply_finish`] does (it rolls forward by calling it), plus
/// the census of escrows to consider.
pub(crate) trait ReapSink: FinishSink {
    /// Every escrow row on this database, as `(transfer_id, character_guid, created_micros,
    /// cross_database)`. Collected up front because the loop mutates the table it walks.
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)>;
}

/// The ONE production adapter: the real `ReducerContext`, wearing every sink at once. It used to be
/// three separate structs (merged them, so there is one place a `ctx.db` line can be wrong
/// instead of three).
///
/// **This layer is the seam's own blind spot and there is no headless way to close it.** The harness
/// substitutes a `FakeDb` for every line of it, so a no-op'd method here is invisible to every test
/// in the crate — and cargo-mutants cannot help either, because it can only ask whether a test
/// FAILS and no test can execute a `ReducerContext` at all (measured: 54 missed mutants across this
/// struct on the first full run, which is why `.cargo/mutants.toml` excludes it BY NAME with that
/// number written down). So it stays pinned by exact-shape equality, in
/// `tests::the_production_adapter_is_the_pass_through_the_harness_assumes` — the one string pin
/// Kept. Every method below must remain a single expression; if this stops being a layer of
/// pass-throughs, the harness underneath it stops meaning what it claims.
struct CtxShard<'a> {
    ctx: &'a ReducerContext,
}

impl ShardLedger for CtxShard<'_> {
    fn out_row(&self, transfer_id: u64) -> Option<TransferOut> {
        self.ctx
            .db
            .game_transfer_out()
            .transfer_id()
            .find(transfer_id)
    }
    fn in_row(&self, transfer_id: u64) -> Option<TransferIn> {
        self.ctx
            .db
            .game_transfer_in()
            .transfer_id()
            .find(transfer_id)
    }
    fn file_out_row(&mut self, row: TransferOut) {
        self.ctx.db.game_transfer_out().insert(row);
    }
    fn file_in_row(&mut self, row: TransferIn) {
        self.ctx.db.game_transfer_in().insert(row);
    }
    fn delete_out_row(&mut self, transfer_id: u64) {
        self.ctx
            .db
            .game_transfer_out()
            .transfer_id()
            .delete(transfer_id);
    }
    fn delete_in_row(&mut self, transfer_id: u64) {
        self.ctx
            .db
            .game_transfer_in()
            .transfer_id()
            .delete(transfer_id);
    }
    fn has_character(&self, guid: u64) -> bool {
        self.ctx.db.game_character().guid().find(guid).is_some()
    }
    fn now_micros(&self) -> i64 {
        self.ctx.timestamp.to_micros_since_unix_epoch()
    }
}

impl BeginSink for CtxShard<'_> {
    fn has_active_taxi_flight(&self, guid: u64) -> bool {
        crate::taxi::is_in_flight(self.ctx, guid)
    }
    fn is_in_transit(&self, guid: u64) -> bool {
        is_in_transit(self.ctx, guid)
    }
    fn freeze_live_entity(&mut self, guid: u64) {
        // Persist the live entity into the durable character row FIRST: it is the source copy a
        // rollback restores, so anything the entity is holding (position, vitals, coin, XP) must be
        // in it before the entity row goes away.
        //
        // `set_offline: false` — the CROSS-MAP TELEPORT precedent (`world::teleport_player`), not
        // the logout one: a shard hop is a loading screen. `true` would force `pending_ghost` false
        // (a free resurrect for a transferring ghost, work-item 226's landmine) and start the
        // rested-XP clock.
        let entities = self.ctx.db.game_world_entity();
        if let Some(e) = entities.guid().find(guid) {
            crate::world::persist_entity(self.ctx, &e, false);
            entities.guid().delete(guid);
        }
    }
    fn export_rows(&self, guid: u64) -> Vec<TableRows> {
        export_rows(self.ctx, guid)
    }
    fn with_character<T>(
        &self,
        guid: u64,
        f: impl FnOnce(&crate::character::Character) -> T,
    ) -> Option<T> {
        self.ctx
            .db
            .game_character()
            .guid()
            .find(guid)
            .map(|c| f(&c))
    }
    fn arm_reaper(&mut self) {
        // Deliberate simplification: arm the reaper lazily, here, instead of in `seed.rs`'s `init`.
        // `init` only runs on a FRESH database, so a schedule row added there would never arm the
        // already-published live DB (it would need a `debug_rearm_*` ritual — see
        // docs/danger-zones.md §3). First transfer arms it; idempotent because the table holds at
        // most one row.
        let sched = self.ctx.db.game_transfer_reaper_schedule();
        if sched.iter().next().is_none() {
            sched.insert(TransferReaperSchedule {
                scheduled_id: 0,
                scheduled_at: ScheduleAt::Interval(TimeDuration::from_micros(
                    TRANSFER_REAP_INTERVAL_MICROS as i64,
                )),
            });
        }
    }
}

impl ImportSink for CtxShard<'_> {
    fn has_live_entity(&self, guid: u64) -> bool {
        self.ctx.db.game_world_entity().guid().find(guid).is_some()
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

impl FinishSink for CtxShard<'_> {
    fn detach_for_transfer(&mut self, guid: u64) {
        crate::group::detach_for_transfer(self.ctx, guid);
        crate::bridge::detach_command_receipts_for_transfer(self.ctx, guid);
    }
    fn cascade_delete_character(&mut self, guid: u64) {
        crate::world::cascade_delete_character(self.ctx, guid);
    }
    fn record_shard(&mut self, guid: u64, map_id: u32, instance_id: u64) {
        crate::realm_core::record_shard(self.ctx, guid, map_id, instance_id);
    }
}

impl ReapSink for CtxShard<'_> {
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)> {
        self.ctx
            .db
            .game_transfer_out()
            .iter()
            .map(|o| {
                (
                    o.transfer_id,
                    o.character_guid,
                    o.created_micros,
                    o.cross_database,
                )
            })
            .collect()
    }
}

// ===========================================================================================
//  Reducers — each one an operator gate plus its `apply_*` body
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
    request_actor: crate::SessionActor,
    dest_map_id: u32,
    dest_instance_id: u64,
    dest_x: f32,
    dest_y: f32,
    dest_z: f32,
    dest_o: f32,
    cross_database: bool,
) -> Result<(), String> {
    require_operator(ctx)?;
    let character_guid = crate::account_ownership::require_actor(ctx, request_actor)?;
    if crate::taxi::is_in_flight(ctx, character_guid) {
        return Err("PLAYER_IN_TAXI_FLIGHT".to_string());
    }
    // Live sessions end before the escrow write flips the in-transit fence: their partner-facing
    // relay rows still address a resolvable character, and the not_transported sweep cannot drop
    // them silently. This remains shard-side, outside the sink-abstracted `apply_begin` harness.
    crate::trade::cancel_trade_for(ctx, character_guid);
    crate::duel::interrupt_duel_for(ctx, character_guid);
    apply_begin(
        &mut CtxShard { ctx },
        transfer_id,
        character_guid,
        Destination {
            map_id: dest_map_id,
            instance_id: dest_instance_id,
            x: dest_x,
            y: dest_y,
            z: dest_z,
            o: dest_o,
        },
        cross_database,
    )
}

/// The whole of [`begin_transfer`] bar the operator gate, over a [`BeginSink`]. Executed for real by
/// `harness` — before that it was 120 lines of `ReducerContext` code whose only coverage was
/// three `.contains()` scans of its own text.
pub(crate) fn apply_begin<S: BeginSink>(
    sink: &mut S,
    transfer_id: u64,
    character_guid: u64,
    dest: Destination,
    cross_database: bool,
) -> Result<(), String> {
    if sink.has_active_taxi_flight(character_guid) {
        return Err("PLAYER_IN_TAXI_FLIGHT".to_string());
    }
    if transfer_id == 0 {
        return Err("transfer_id 0 is reserved (it is the \"no transfer\" sentinel)".to_string());
    }
    // Whose escrow is filed under this id, if any.
    let escrowed = escrowed_guid(
        sink.out_row(transfer_id).map(|o| o.character_guid),
        sink.in_row(transfer_id).map(|i| i.character_guid),
    );
    match plan_begin(
        escrowed,
        character_guid,
        sink.has_character(character_guid),
        sink.is_in_transit(character_guid),
    ) {
        BeginPlan::Replay => return Ok(()),
        BeginPlan::NoSource => return Err(format!("no such character: {character_guid}")),
        BeginPlan::IdCollision => {
            return Err(format!(
                "transfer {transfer_id} is already escrowed for character {} — refusing to reuse the \
                 id for character {character_guid}",
                escrowed.expect("IdCollision implies an escrow exists")
            ));
        }
        BeginPlan::AlreadyInTransit => {
            return Err(format!("character {character_guid} is already in transit"));
        }
        BeginPlan::Escrow => {}
    }

    // Freeze: persist whatever the live entity was holding, then delete it. That single delete is
    // what makes the character invisible to every targeting/aggro/threat/AOI gate in the module,
    // because those all resolve through `game_world_entity`.
    sink.freeze_live_entity(character_guid);

    // The ROWS. Serialized here, in the same transaction that froze the character, so
    // the payload is a consistent snapshot: nothing can write a manifest table between the freeze
    // and the export (the in-transit fence covers every writer bar the audited exceptions in this
    // module's verdict table). The character row is read AFTER the freeze — `freeze_live_entity`
    // just rewrote it.
    let payload = sink.export_rows(character_guid);
    let blob = sink
        .with_character(character_guid, |c| {
            build_export_blob(transfer_id, c, dest, payload)
        })
        .ok_or_else(|| {
            // Unreachable: `BeginPlan::Escrow` implies the row existed a moment ago and nothing
            // between here and there deletes it. Returning Err rather than panicking keeps the
            // failure a clean transaction abort — the escrow row below is then never committed.
            format!(
                "transfer {transfer_id}: character {character_guid} vanished between the plan and \
                 the export — nothing escrowed"
            )
        })??;
    let blob = spacetimedb::sats::bsatn::to_vec(&blob)
        .map_err(|e| format!("transfer {transfer_id}: cannot serialize export blob: {e}"))?;

    let created_micros = sink.now_micros();
    sink.file_out_row(TransferOut {
        transfer_id,
        character_guid,
        dest_map_id: dest.map_id,
        dest_instance_id: dest.instance_id,
        dest_x: dest.x,
        dest_y: dest.y,
        dest_z: dest.z,
        dest_o: dest.o,
        blob,
        created_micros,
        cross_database,
    });
    sink.arm_reaper();

    log::info!(
        "begin_transfer: {transfer_id} escrowed character {character_guid} -> map {} instance {}",
        dest.map_id,
        dest.instance_id
    );
    Ok(())
}

/// **Step 2 — commit the arrival copy. Idempotent on `transfer_id` (unique PK).**
///
/// Same-database tracer: the escrowed blob is read from the local out-row. Cross-database,
/// the gateway ships the blob and [`import_character_blob`] takes it as an argument — the ONLY
/// difference, which is why the state machine is provable now.
///
/// The ONE reducer in this module still written straight against `ReducerContext` rather than a
/// sink, and deliberately so: same-database "materialise the destination copy" IS a re-partition of
/// the single shared `game_character` row, so there is nothing here a fake second database could
/// model. Everything it does that is not the re-partition — the blob decode, the manifest gate, the
/// six-field relocation — is shared with [`apply_import_blob`] and executed by the harness there.
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

    let blob = decode_blob(transfer_id, &out.blob)?;
    check_manifest(transfer_id, &blob.manifest)?;

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
    c.relocate(out.destination());
    chars.guid().update(c);

    ctx.db.game_transfer_in().insert(TransferIn {
        transfer_id,
        character_guid: out.character_guid,
        blob: out.blob.clone(),
        created_micros: ctx.timestamp.to_micros_since_unix_epoch(),
        bot_intent_id: 0,
        bot_controller_generation: 0,
        bot_intent_created_micros: 0,
        bot_intent_source: Identity::ZERO,
        source_map_id: 0,
        source_instance_id: 0,
        source_locator_revision: 0,
    });

    log::info!(
        "import_character: {transfer_id} committed character {} at map {} instance {}",
        out.character_guid,
        out.dest_map_id,
        out.dest_instance_id
    );
    Ok(())
}

fn require_transfer_actor(
    ctx: &ReducerContext,
    transfer_id: u64,
    actor: crate::SessionActor,
) -> Result<(), String> {
    crate::account_ownership::require_actor(ctx, actor)?;
    let character = ctx
        .db
        .game_transfer_out()
        .transfer_id()
        .find(transfer_id)
        .map(|row| row.character_guid)
        .or_else(|| {
            ctx.db
                .game_transfer_in()
                .transfer_id()
                .find(transfer_id)
                .map(|row| row.character_guid)
        });
    if let Some(guid) = character {
        crate::account_ownership::require_actor_for(ctx, actor, guid)?;
    }
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
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_transfer_actor(ctx, transfer_id, request_actor)?;
    let character_guid = decode_blob(transfer_id, &blob)?.character_guid;
    crate::account_ownership::require_actor_for(ctx, request_actor, character_guid)?;
    apply_import_blob(&mut CtxShard { ctx }, transfer_id, blob)
}

/// Commit a human arrival with the Realm locator predecessor that began this crossing.
/// Recovery may settle only that exact pending phase, even when the Character later returns to
/// the same destination under a newer Realm revision.
#[reducer]
#[allow(clippy::too_many_arguments)] // The source predecessor and Actor are the reducer wire Gate.
pub fn import_player_character_blob(
    ctx: &ReducerContext,
    transfer_id: u64,
    blob: Vec<u8>,
    source_map_id: u32,
    source_instance_id: u64,
    source_locator_revision: u64,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_transfer_actor(ctx, transfer_id, request_actor)?;
    if source_locator_revision == 0 {
        return Err("player Transfer arrival has no Realm locator predecessor".to_string());
    }
    let character_guid = decode_blob(transfer_id, &blob)?.character_guid;
    crate::account_ownership::require_actor_for(ctx, request_actor, character_guid)?;
    let expected = (source_map_id, source_instance_id, source_locator_revision);
    if let Some(existing) = ctx.db.game_transfer_in().transfer_id().find(transfer_id) {
        if existing.bot_intent_id != 0
            || (
                existing.source_map_id,
                existing.source_instance_id,
                existing.source_locator_revision,
            ) != expected
        {
            return Err(format!(
                "transfer {transfer_id}: destination fence belongs to another crossing"
            ));
        }
    }
    apply_import_blob(&mut CtxShard { ctx }, transfer_id, blob)?;
    let arrivals = ctx.db.game_transfer_in();
    let mut arrival = arrivals
        .transfer_id()
        .find(transfer_id)
        .ok_or_else(|| format!("transfer {transfer_id}: imported without an arrival fence"))?;
    arrival.source_map_id = source_map_id;
    arrival.source_instance_id = source_instance_id;
    arrival.source_locator_revision = source_locator_revision;
    arrivals.transfer_id().update(arrival);
    Ok(())
}

/// Commit a session-less arrival with its exact source intent identity already on the destination
/// fence. An existing blank fence is never relabelled: it may belong to a newer human crossing that
/// reused the Character-guid transfer id.
#[reducer]
#[allow(clippy::too_many_arguments)] // The exact crossing identity is the reducer wire contract.
pub fn import_bot_character_blob(
    ctx: &ReducerContext,
    transfer_id: u64,
    blob: Vec<u8>,
    source_module_identity: Identity,
    intent_id: u64,
    controller_generation: u64,
    intent_created_micros: i64,
    source_map_id: u32,
    source_instance_id: u64,
    source_locator_revision: u64,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_transfer_actor(ctx, transfer_id, request_actor)?;
    if source_module_identity == Identity::ZERO
        || intent_id == 0
        || intent_created_micros <= 0
        || source_locator_revision == 0
    {
        return Err("bot Transfer arrival identity is invalid".to_string());
    }
    let character_guid = decode_blob(transfer_id, &blob)?.character_guid;
    if transfer_id != character_guid {
        return Err(format!(
            "transfer {transfer_id}: session-less arrival belongs to character {character_guid}"
        ));
    }
    crate::account_ownership::require_actor_for(ctx, request_actor, character_guid)?;
    let expected = (
        source_module_identity,
        intent_id,
        controller_generation,
        intent_created_micros,
        source_map_id,
        source_instance_id,
        source_locator_revision,
    );
    if let Some(existing) = ctx.db.game_transfer_in().transfer_id().find(transfer_id) {
        let current = (
            existing.bot_intent_source,
            existing.bot_intent_id,
            existing.bot_controller_generation,
            existing.bot_intent_created_micros,
            existing.source_map_id,
            existing.source_instance_id,
            existing.source_locator_revision,
        );
        if current != expected {
            return Err(format!(
                "transfer {transfer_id}: destination fence belongs to another crossing"
            ));
        }
    }
    apply_import_blob(&mut CtxShard { ctx }, transfer_id, blob)?;
    let arrivals = ctx.db.game_transfer_in();
    let mut arrival = arrivals
        .transfer_id()
        .find(transfer_id)
        .ok_or_else(|| format!("transfer {transfer_id}: imported without an arrival fence"))?;
    arrival.bot_intent_source = source_module_identity;
    arrival.bot_intent_id = intent_id;
    arrival.bot_controller_generation = controller_generation;
    arrival.bot_intent_created_micros = intent_created_micros;
    arrival.source_map_id = source_map_id;
    arrival.source_instance_id = source_instance_id;
    arrival.source_locator_revision = source_locator_revision;
    arrivals.transfer_id().update(arrival);
    Ok(())
}

/// The whole of [`import_character_blob`] bar the operator gate — every guard, in order, over an
/// [`ImportSink`]. Executed for real by `harness`.
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
        let decoded = decode_blob(transfer_id, &blob)?;
        if existing.character_guid != decoded.character_guid {
            return Err(format!(
                "transfer {transfer_id} is already imported for character {} — refusing to reuse \
                 the id for character {}",
                existing.character_guid, decoded.character_guid
            ));
        }
        return Ok(());
    }

    let decoded = decode_blob(transfer_id, &blob)?;
    if decoded.transfer_id != transfer_id {
        return Err(format!(
            "transfer {transfer_id}: blob names transfer {} — refusing to import a blob filed \
             under a different id",
            decoded.transfer_id
        ));
    }
    check_manifest(transfer_id, &decoded.manifest)?;
    let guid = decoded.character_guid;

    // A live entity for this guid here means the character is ALREADY resident on this database —
    // importing on top of it would be the dual-liveness dupe wearing a different hat. Refuse.
    if sink.has_live_entity(guid) {
        return Err(format!(
            "transfer {transfer_id}: character {guid} already has a LIVE entity on this shard — \
             refusing to import a second copy"
        ));
    }

    // Materialise `game_character`. Decoded with THIS build's `Character` type, then relocated to
    // the destination coordinates and given the money the blob carries (which is the escrowed value
    // PLUS any `defer_money_delta` folded in after the freeze — the residual, replayed here
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
    c.relocate(decoded.destination());
    c.money = decoded.money;
    let account_id = c.account_id;
    // Remove stale rows for this Character even when its Character row is absent. Item import
    // separately checks foreign GUID collisions because legacy packing could overlap.
    sink.cascade_delete_character(guid);
    sink.insert_character(c);
    // AC#3: ratchet this database's guid allocator past `guid` NOW, in the same
    // transaction as the materialisation — so a `create_character` racing this import (or run any
    // time after) can never hand the same guid to a brand-new local character.
    //
    // ONLY when `guid` is actually inside THIS database's own range. Ranges are
    // disjoint by construction — a foreign-range arrival (e.g. a world-1 character crossing
    // into core) can never collide with anything core mints, so ratcheting past it protects
    // nothing and instead walks core's own mark toward, and eventually past, its own range end.
    // That is exactly what was hit live: `lyracore`'s high_water sat at its range end with
    // zero local characters above it, and every local `create_character` failed
    // GUID_RANGE_EXHAUSTED. A local-range arrival still ratchets, unchanged AC#3.
    if crate::auth::in_guid_range(sink.own_guid_range(), guid) {
        sink.bump_guid_high_water(guid);
    }
    sink.import_rows(guid, &decoded.payload)?;

    // The destination has no `game_account` row (accounts are realm-scoped and live on the default
    // database until realm-core). `gw::gw_player_login` resolves the account by id, so
    // without one the arriving player cannot log in at all.
    // Deliberate simplification: a SHADOW account — id + a synthetic username, no credentials. The
    // gateway rebinds `identity` through `establish_session` on this shard at every world entry, which is the only
    // field this row exists to hold. Upgrade path: realm-core owns accounts (Phase B) and this
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
        bot_intent_id: 0,
        bot_controller_generation: 0,
        bot_intent_created_micros: 0,
        bot_intent_source: Identity::ZERO,
        source_map_id: 0,
        source_instance_id: 0,
        source_locator_revision: 0,
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
/// `plan_finish`, `recovery`, the whole crash matrix — then works unchanged, because the in-row
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
pub fn confirm_import(
    ctx: &ReducerContext,
    transfer_id: u64,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_transfer_actor(ctx, transfer_id, request_actor)?;
    apply_confirm(&mut CtxShard { ctx }, transfer_id)
}

/// The whole of [`confirm_import`] bar the operator gate, over a [`ShardLedger`].
pub(crate) fn apply_confirm<S: ShardLedger>(sink: &mut S, transfer_id: u64) -> Result<(), String> {
    let Some(out) = sink.out_row(transfer_id) else {
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
    if sink.in_row(transfer_id).is_some() {
        return Ok(()); // already attested — replay-safe like every other step
    }
    let created_micros = sink.now_micros();
    let character_guid = out.character_guid;
    sink.file_in_row(TransferIn {
        transfer_id,
        character_guid,
        blob: out.blob,
        created_micros,
        bot_intent_id: 0,
        bot_controller_generation: 0,
        bot_intent_created_micros: 0,
        bot_intent_source: Identity::ZERO,
        source_map_id: 0,
        source_instance_id: 0,
        source_locator_revision: 0,
    });
    log::info!(
        "confirm_import: {transfer_id} — destination copy of character {character_guid} attested \
         durable; the source copy may now be released"
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
/// Refuses while a local out-row exists, or while the destination fence carries a new exact
/// crossing identity. Only a migrated blank fence uses this reducer; new human and session-less
/// crossings use their exact release reducers.
#[reducer]
pub fn release_transfer(
    ctx: &ReducerContext,
    transfer_id: u64,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_transfer_actor(ctx, transfer_id, request_actor)?;
    if ctx
        .db
        .game_transfer_in()
        .transfer_id()
        .find(transfer_id)
        .is_some_and(|arrival| arrival.bot_intent_id != 0 || arrival.source_locator_revision != 0)
    {
        return Err(format!(
            "transfer {transfer_id}: identified arrival requires its exact release reducer"
        ));
    }
    apply_release(&mut CtxShard { ctx }, transfer_id)
}

/// Release only the human arrival carrying this Realm locator predecessor. A different exact
/// predecessor belongs to another crossing and remains fenced.
#[reducer]
pub fn release_player_transfer_arrival(
    ctx: &ReducerContext,
    transfer_id: u64,
    character_guid: u64,
    source_map_id: u32,
    source_instance_id: u64,
    source_locator_revision: u64,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_transfer_actor(ctx, transfer_id, request_actor)?;
    if transfer_id != character_guid || source_locator_revision == 0 {
        return Err("player Transfer arrival identity is invalid".to_string());
    }
    let Some(arrival) = ctx.db.game_transfer_in().transfer_id().find(transfer_id) else {
        return Ok(());
    };
    if arrival.character_guid != character_guid {
        return Err(format!(
            "transfer {transfer_id}: arrival belongs to character {}, not {character_guid}",
            arrival.character_guid
        ));
    }
    if arrival.bot_intent_id != 0
        || (
            arrival.source_map_id,
            arrival.source_instance_id,
            arrival.source_locator_revision,
        ) != (source_map_id, source_instance_id, source_locator_revision)
    {
        return Ok(());
    }
    apply_release(&mut CtxShard { ctx }, transfer_id)
}

/// Release only this intent's destination fence. An absent fence or a differently identified
/// newer fence proves this crossing already released and is left untouched.
#[reducer]
pub fn release_bot_transfer_arrival(
    ctx: &ReducerContext,
    transfer_id: u64,
    bot_guid: u64,
    source_module_identity: Identity,
    intent_id: u64,
    controller_generation: u64,
    intent_created_micros: i64,
    source_map_id: u32,
    source_instance_id: u64,
    source_locator_revision: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    if transfer_id != bot_guid
        || source_module_identity == Identity::ZERO
        || intent_id == 0
        || intent_created_micros <= 0
        || source_locator_revision == 0
    {
        return Err("bot Transfer arrival identity is invalid".to_string());
    }
    let Some(arrival) = ctx.db.game_transfer_in().transfer_id().find(transfer_id) else {
        return Ok(());
    };
    if arrival.character_guid != bot_guid {
        return Err(format!(
            "transfer {transfer_id}: arrival belongs to character {}, not {bot_guid}",
            arrival.character_guid
        ));
    }
    if (
        arrival.bot_intent_source,
        arrival.bot_intent_id,
        arrival.bot_controller_generation,
        arrival.bot_intent_created_micros,
        arrival.source_map_id,
        arrival.source_instance_id,
        arrival.source_locator_revision,
    ) != (
        source_module_identity,
        intent_id,
        controller_generation,
        intent_created_micros,
        source_map_id,
        source_instance_id,
        source_locator_revision,
    ) {
        return Ok(());
    }
    apply_release(&mut CtxShard { ctx }, transfer_id)
}

/// The whole of [`release_transfer`] bar the operator gate, over a [`ShardLedger`].
pub(crate) fn apply_release<S: ShardLedger>(sink: &mut S, transfer_id: u64) -> Result<(), String> {
    if sink.out_row(transfer_id).is_some() {
        return Err(format!(
            "transfer {transfer_id}: this database holds the SOURCE out-row — call finish_transfer, \
             not release_transfer (releasing alone would leave the source's claim behind)"
        ));
    }
    let Some(row) = sink.in_row(transfer_id) else {
        return Ok(()); // already released — replay-safe, like every other step
    };
    sink.delete_in_row(transfer_id);
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
pub fn finish_transfer(
    ctx: &ReducerContext,
    transfer_id: u64,
    request_actor: crate::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_transfer_actor(ctx, transfer_id, request_actor)?;
    apply_finish_step(&mut CtxShard { ctx }, transfer_id)
}

/// The whole of [`finish_transfer`] bar the operator gate: the `plan_finish` verdict plus, when it
/// says so, [`apply_finish`]'s delete-last body.
pub(crate) fn apply_finish_step<S: FinishSink>(
    sink: &mut S,
    transfer_id: u64,
) -> Result<(), String> {
    let has_out = sink.out_row(transfer_id).is_some();
    let has_in = sink.in_row(transfer_id).is_some();
    match plan_finish(has_out, has_in) {
        // NO-OP, and it must SAY so. The gateway's drive calls `finish_transfer` on a specific
        // database and reads `Ok(())` as "the source released the character", then proceeds to
        // publish_shard_index / release_transfer / evict. Returning success silently here makes a
        // finish driven against the WRONG database indistinguishable from one that did the work —
        // which is exactly the question anyone debugging a stranded copy is asking. Cheap to log:
        // the fan-out makes this a normal, expected occurrence, not an error, so it is `info!`.
        FinishPlan::AlreadyDone => {
            log::info!(
                "finish_transfer: {transfer_id} — no escrow on this database, nothing to finish \
                 (already settled here, or this call reached the wrong shard)"
            );
            Ok(())
        }
        FinishPlan::NotImported => Err(format!(
            "transfer {transfer_id}: not imported — refusing to release the source copy"
        )),
        FinishPlan::Complete => {
            apply_finish(sink, transfer_id);
            Ok(())
        }
    }
}

/// The delete-last body, shared by [`finish_transfer`] and the reaper's roll-forward. Executed for
/// real — order and all — by `harness`.
pub(crate) fn apply_finish<S: FinishSink>(sink: &mut S, transfer_id: u64) {
    let mut cascaded = false;
    if let Some(out) = sink.out_row(transfer_id) {
        // CROSS-DATABASE: the source shard's teardown is `cascade_delete_character` —
        // the existing character-owned delete sweep, the exact counterpart of the payload this blob
        // exported. Skipping it leaves a full second copy of the character on the source, which is a
        // dupe the moment anything logs it in. Same-database there is nothing to cascade: the two
        // partitions share `game_character`, so destroying the "source copy" would destroy the
        // destination copy too. What IS deleted last, in both deployments, is the source's escrow
        // row — the source's claim on the character.
        cascaded = out.cross_database;
        if out.cross_database {
            // Source rows that crossed in the Escrow snapshot detach HERE, before the cascade, so
            // their removal depends on no generated sweep ordering. The group mirror uses raw
            // removal because a shard hop is not a party departure. Command Receipts leave the old
            // Shard here because their transported copies now guard retry at the destination.
            sink.detach_for_transfer(out.character_guid);
            sink.cascade_delete_character(out.character_guid);
        }
        // AC#3: the character→shard index entry is written HERE, inside the same transaction that
        // releases the escrow, from the out-row's own destination fields — so "the escrow settled" and
        // "the directory says where it settled" can never disagree on this database.
        //
        // Cross-database, this is only HALF the index. There is no transaction spanning
        // two SpacetimeDB databases, so realm-core's copy — the one `home_shard` actually reads —
        // cannot be written from here at all. What the gateway does instead is REPLICATE this row:
        // `world::transfer::run_transfer` publishes the same `(guid, map, instance)` to realm-core
        // as a required step of the drive, strictly AFTER `finish_transfer` returned Ok, so it can
        // never name a destination for a transfer that did not settle. That is the strongest form
        // available across two databases. It is NOT a guarantee: if the gateway dies (or the publish
        // fails) between `finish_transfer` and step 6, realm-core's copy keeps naming the old
        // shard, and the recovery path does not re-drive `run_transfer`, so nothing republishes it.
        // The index is therefore still specified as a HINT the gateway CONFIRMS by probing rather
        // than trusts, and the login self-heal is still the terminal fallback — except that the
        // self-heal is currently unreachable in production (the gateway's `settle_home_shard`
        // overrides the `home_shard` that hosts it and locates the character by scanning instead).
        // Nothing in production reads this directory yet either, so the two cancel out today; both
        // have to be fixed together before anything routes on it. See
        // `gateway/src/realm_core.rs::publish_shard_index`.
        //
        // MERGE ORDER: this runs AFTER the cascade, never before. `game_character_shard`
        // is itself character-owned (`realm_core.rs`'s delete sweep), so a cascade that ran second
        // would wipe the forwarding receipt the source shard is supposed to keep — the receipt that
        // lets a gateway whose realm-core is unconfigured (`realm_core()` == the default shard) still
        // find a character that moved off it.
        sink.record_shard(out.character_guid, out.dest_map_id, out.dest_instance_id);
    }
    sink.clear_escrow(transfer_id);
    // Name what actually happened: "complete" alone read identically whether the source copy
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
    apply_reap(&mut CtxShard { ctx });
}

/// The whole of [`reap_transfers`] bar the scheduler-identity gate, over a [`ReapSink`]. Executed
/// for real by `harness` — the `cross_database && !has_in` rule below used to be pinned by a
/// `.contains()` scan of the reducer's own text.
pub(crate) fn apply_reap<S: ReapSink>(sink: &mut S) {
    let now = sink.now_micros();
    for (transfer_id, character_guid, created_micros, cross_database) in sink.escrows() {
        let age = now - created_micros;
        let has_in = sink.in_row(transfer_id).is_some();
        // CROSS-DATABASE: the destination's arrival copy is on ANOTHER database, so an
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
                sink.delete_out_row(transfer_id);
                log::warn!(
                    "reap_transfers: {transfer_id} abandoned before import — rolled BACK, character \
                     {character_guid} unfrozen at the source"
                );
            }
            Recovery::RollForward => {
                apply_finish(sink, transfer_id);
                log::warn!(
                    "reap_transfers: {transfer_id} abandoned after import — rolled FORWARD, character \
                     {character_guid} released at the destination"
                );
            }
        }
    }
}
