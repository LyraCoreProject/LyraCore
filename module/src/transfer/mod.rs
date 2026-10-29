//! Before changing the Transfer ledger, fence or reducers, read docs/character-transfer.md.

mod transport;
#[cfg(test)]
pub(crate) use transport::{
    admit_command_issuer_import, decode_rows, encode_rows, export_rows_via, import_rows_via,
    manifest, CommandIssuerImportRefusal, ManifestEntry, TransportArm, HOT_TABLES,
    MANIFEST_EXCLUDE, NOT_TRANSPORTED, TRANSFER_TABLES_OLD_BLOBS_MAY_LACK,
};
pub(crate) use transport::{
    build_export_blob, check_manifest, decode_blob, decode_character_row, export_rows, import_rows,
    move_rows, not_transported, payload_for_this_build, Destination, ExportBlob, RowIo, TableRows,
};

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

pub(crate) const TRANSFER_STALE_MICROS: i64 = 30_000_000;

const TRANSFER_REAP_INTERVAL_MICROS: u64 = 5_000_000;

#[table(
    accessor = game_transfer_out,
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct TransferOut {
    // The caller-chosen idempotency key must remain stable across retries.
    #[primary_key]
    pub transfer_id: u64,
    pub character_guid: u64,
    pub dest_map_id: u32,
    pub dest_instance_id: u64,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_z: f32,
    pub dest_o: f32,
    pub blob: Vec<u8>,
    pub created_micros: i64,
    // Cross-Shard recovery cannot infer a missing destination copy from an absent local receipt.
    #[default(false)]
    pub cross_database: bool,
}

#[table(
    accessor = game_transfer_in,
    index(accessor = by_character, btree(columns = [character_guid]))
)]
pub struct TransferIn {
    #[primary_key]
    pub transfer_id: u64,
    pub character_guid: u64,
    pub blob: Vec<u8>,
    pub created_micros: i64,
    #[default(0u64)]
    pub bot_intent_id: u64,
    #[default(0u64)]
    pub bot_controller_generation: u64,
    #[default(0i64)]
    pub bot_intent_created_micros: i64,
    #[default(Identity::ZERO)]
    pub bot_intent_source: Identity,
    #[default(0u32)]
    pub source_map_id: u32,
    #[default(0u64)]
    pub source_instance_id: u64,
    #[default(0u64)]
    pub source_locator_revision: u64,
}

#[table(accessor = game_transfer_reaper_schedule, scheduled(reap_transfers))]
pub struct TransferReaperSchedule {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
}

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
    pub destination_instance: u64,
    pub reason: String,
    pub created_at: Timestamp,
    #[default(0u64)]
    pub controller_generation: u64,
    #[default(0u64)]
    pub claim_token: u64,
    #[default(0i64)]
    pub claim_until_micros: i64,
    // Record preparation before releasing the arrival fence; the Character may cross onward.
    #[default(false)]
    pub arrival_ready: bool,
    #[default(Identity::ZERO)]
    pub source_module_identity: Identity,
    #[default(0u32)]
    pub source_map: u32,
    #[default(0u64)]
    pub source_instance: u64,
    #[default(0u64)]
    pub source_locator_revision: u64,
}

// Destination placement and the Transfer Intent must commit in one transaction.
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
        created_at: crate::time::now(ctx),
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

#[reducer]
#[allow(clippy::too_many_arguments)]
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
    let now = crate::time::now(ctx).to_micros_since_unix_epoch();
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Phase {
    Resident,
    Escrowed,
    Imported,
}

pub(crate) fn phase(has_out: bool, has_in: bool) -> Phase {
    match (has_out, has_in) {
        (false, _) => Phase::Resident,
        (true, false) => Phase::Escrowed,
        (true, true) => Phase::Imported,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BeginPlan {
    Replay,
    NoSource,
    IdCollision,
    AlreadyInTransit,
    Escrow,
}

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

pub(crate) fn escrowed_guid(out_row: Option<u64>, in_row: Option<u64>) -> Option<u64> {
    out_row.or(in_row)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ImportPlan {
    Replay,
    NoEscrow,
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum FinishPlan {
    AlreadyDone,
    NotImported,
    Complete,
}

pub(crate) fn plan_finish(has_out: bool, has_in: bool) -> FinishPlan {
    match phase(has_out, has_in) {
        Phase::Resident => FinishPlan::AlreadyDone,
        Phase::Escrowed => FinishPlan::NotImported,
        Phase::Imported => FinishPlan::Complete,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Recovery {
    Hold,
    Rollback,
    RollForward,
}

// Unknown destination commitment holds Escrow; an imported Character can only roll forward.
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

pub(crate) fn login_allowed(has_out: bool, has_in: bool) -> bool {
    !has_out && !has_in
}

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
    !login_allowed(has_out, has_in)
}

pub(crate) struct TransferClaimReadLimit;

pub(crate) fn in_transit_instances(
    ctx: &ReducerContext,
) -> Result<Vec<u64>, TransferClaimReadLimit> {
    let chars = ctx.db.game_character();
    let mut out = Vec::new();
    for row in ctx.db.game_transfer_out().iter() {
        if row.dest_instance_id != 0 {
            out.push(row.dest_instance_id);
        }
        // The fenced Character reader hides the in-transit row this recovery path needs.
        if let Some(c) = chars.guid().find(row.character_guid) {
            if c.pending_instance_id != 0 {
                out.push(c.pending_instance_id);
            }
        }
    }
    let pending: Vec<_> = ctx
        .db
        .game_bot_transfer_intent()
        .iter()
        .take(lyracore_shared::transfer::BOT_TRANSFER_PENDING_LIMIT + 1)
        .collect();
    if pending.len() > lyracore_shared::transfer::BOT_TRANSFER_PENDING_LIMIT {
        return Err(TransferClaimReadLimit);
    }
    for intent in pending {
        if intent.source_instance != 0 {
            out.push(intent.source_instance);
        }
        if intent.destination_instance != 0 {
            out.push(intent.destination_instance);
        }
    }
    Ok(out)
}

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
        Err(e) => log::error!(
            "defer_money_delta: character {character_guid} in transit but its blob is unreadable \
             ({e}) — {amount} copper NOT deferred"
        ),
    }
    true
}

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

pub(crate) trait ShardLedger {
    fn out_row(&self, transfer_id: u64) -> Option<TransferOut>;
    fn in_row(&self, transfer_id: u64) -> Option<TransferIn>;
    fn file_out_row(&mut self, row: TransferOut);
    fn file_in_row(&mut self, row: TransferIn);
    fn delete_out_row(&mut self, transfer_id: u64);
    fn delete_in_row(&mut self, transfer_id: u64);
    fn has_character(&self, guid: u64) -> bool;
    fn now_micros(&self) -> i64;

    // Clear the out-row last because its absence releases the source claim.
    fn clear_escrow(&mut self, transfer_id: u64) {
        self.delete_in_row(transfer_id);
        self.delete_out_row(transfer_id);
    }
}

pub(crate) trait BeginSink: ShardLedger {
    fn has_active_taxi_flight(&self, guid: u64) -> bool;
    fn is_in_transit(&self, guid: u64) -> bool;
    fn freeze_live_entity(&mut self, guid: u64);
    fn export_rows(&self, guid: u64) -> Vec<TableRows>;
    fn with_character<T>(
        &self,
        guid: u64,
        f: impl FnOnce(&crate::character::Character) -> T,
    ) -> Option<T>;
    fn arm_reaper(&mut self);
}

pub(crate) trait ImportSink: ShardLedger {
    fn has_live_entity(&self, guid: u64) -> bool;
    fn detach_for_transfer(&mut self, guid: u64);
    fn cascade_delete_character(&mut self, guid: u64, listing_holds: ListingHolds);
    fn insert_character(&mut self, c: crate::character::Character);
    fn import_rows(&mut self, guid: u64, payload: &[TableRows]) -> Result<(), String>;
    fn ensure_shadow_account(&mut self, account_id: u64);
    fn bump_guid_high_water(&mut self, guid: u64);
    fn own_guid_range(&self) -> Option<(u64, u64)>;
}

// Cascade before writing the forwarding receipt, which is Character-owned.
pub(crate) trait FinishSink: ShardLedger {
    fn detach_for_transfer(&mut self, guid: u64);
    fn cascade_delete_character(&mut self, guid: u64, listing_holds: ListingHolds);
    fn record_shard(&mut self, guid: u64, map_id: u32, instance_id: u64);
}

pub(crate) trait ReapSink: FinishSink {
    fn escrows(&self) -> Vec<(u64, u64, i64, bool)>;
}

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
        crate::time::now(self.ctx).to_micros_since_unix_epoch()
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
        // Persist vitals, position and ghost state before deleting the live entity.
        let entities = self.ctx.db.game_world_entity();
        if let Some(e) = entities.guid().find(guid) {
            crate::world::persist_entity(self.ctx, &e, false);
            crate::WorldEntity::delete(self.ctx, guid);
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
    fn detach_for_transfer(&mut self, guid: u64) {
        crate::group::detach_for_transfer(self.ctx, guid);
        crate::bridge::detach_command_receipts_for_transfer(self.ctx, guid);
    }
    fn cascade_delete_character(&mut self, guid: u64, listing_holds: ListingHolds) {
        cascade_for_transfer(self.ctx, guid, listing_holds);
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
    fn cascade_delete_character(&mut self, guid: u64, listing_holds: ListingHolds) {
        cascade_for_transfer(self.ctx, guid, listing_holds);
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

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn begin_transfer(
    ctx: &ReducerContext,
    transfer_id: u64,
    request_actor: crate::account_ownership::SessionActor,
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
    // End live sessions before fencing Transfer while partner Relays can resolve the Character.
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

    sink.freeze_live_entity(character_guid);

    // Freeze and export in one transaction so the payload is a consistent snapshot.
    let payload = sink.export_rows(character_guid);
    let blob = sink
        .with_character(character_guid, |c| {
            build_export_blob(transfer_id, c, dest, payload)
        })
        .ok_or_else(|| {
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
        created_micros: crate::time::now(ctx).to_micros_since_unix_epoch(),
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
    actor: crate::account_ownership::SessionActor,
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

#[reducer]
pub fn import_character_blob(
    ctx: &ReducerContext,
    transfer_id: u64,
    blob: Vec<u8>,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_transfer_actor(ctx, transfer_id, request_actor)?;
    let character_guid = decode_blob(transfer_id, &blob)?.character_guid;
    crate::account_ownership::require_actor_for(ctx, request_actor, character_guid)?;
    apply_import_blob(&mut CtxShard { ctx }, transfer_id, blob)
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn import_player_character_blob(
    ctx: &ReducerContext,
    transfer_id: u64,
    blob: Vec<u8>,
    source_map_id: u32,
    source_instance_id: u64,
    source_locator_revision: u64,
    request_actor: crate::account_ownership::SessionActor,
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

#[reducer]
#[allow(clippy::too_many_arguments)]
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
    request_actor: crate::account_ownership::SessionActor,
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

pub(crate) fn apply_import_blob<S: ImportSink>(
    sink: &mut S,
    transfer_id: u64,
    blob: Vec<u8>,
) -> Result<(), String> {
    if transfer_id == 0 {
        return Err("transfer_id 0 is reserved (it is the \"no transfer\" sentinel)".to_string());
    }
    if let Some(existing) = sink.in_row(transfer_id) {
        // A reused transfer id must not authorize deletion of another Character at the source.
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

    if sink.has_live_entity(guid) {
        return Err(format!(
            "transfer {transfer_id}: character {guid} already has a LIVE entity on this shard — \
             refusing to import a second copy"
        ));
    }

    let mut c = decode_character_row(transfer_id, &decoded.character_row)?;
    if c.guid != guid {
        return Err(format!(
            "transfer {transfer_id}: the arriving character row is guid {} but the blob names {guid}",
            c.guid
        ));
    }
    c.relocate(decoded.destination());
    c.money = decoded.money;
    let account_id = c.account_id;
    // Detach Realm-owned party mirrors before the cascade can interpret cleanup as a departure.
    sink.detach_for_transfer(guid);
    sink.cascade_delete_character(guid, ListingHolds::Keep);
    sink.insert_character(c);
    // Advance the allocator atomically with import only for guids inside this Shard's own range.
    if crate::auth::in_guid_range(sink.own_guid_range(), guid) {
        sink.bump_guid_high_water(guid);
    }
    let payload = payload_for_this_build(&decoded.manifest, &decoded.payload);
    sink.import_rows(guid, &payload)?;

    sink.ensure_shadow_account(account_id);

    // Prove the arrival exists before filing the receipt that permits deletion of the source.
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

#[reducer]
pub fn confirm_import(
    ctx: &ReducerContext,
    transfer_id: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_transfer_actor(ctx, transfer_id, request_actor)?;
    apply_confirm(&mut CtxShard { ctx }, transfer_id)
}

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
        return Ok(());
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

#[reducer]
pub fn release_transfer(
    ctx: &ReducerContext,
    transfer_id: u64,
    request_actor: crate::account_ownership::SessionActor,
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

#[reducer]
pub fn release_player_transfer_arrival(
    ctx: &ReducerContext,
    transfer_id: u64,
    character_guid: u64,
    source_map_id: u32,
    source_instance_id: u64,
    source_locator_revision: u64,
    request_actor: crate::account_ownership::SessionActor,
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

#[reducer]
#[allow(clippy::too_many_arguments)]
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

pub(crate) fn apply_release<S: ShardLedger>(sink: &mut S, transfer_id: u64) -> Result<(), String> {
    if sink.out_row(transfer_id).is_some() {
        return Err(format!(
            "transfer {transfer_id}: this database holds the SOURCE out-row — call finish_transfer, \
             not release_transfer (releasing alone would leave the source's claim behind)"
        ));
    }
    let Some(row) = sink.in_row(transfer_id) else {
        return Ok(());
    };
    sink.delete_in_row(transfer_id);
    log::info!(
        "release_transfer: {transfer_id} — character {} is live at the destination",
        row.character_guid
    );
    Ok(())
}

#[reducer]
pub fn finish_transfer(
    ctx: &ReducerContext,
    transfer_id: u64,
    request_actor: crate::account_ownership::SessionActor,
) -> Result<(), String> {
    require_operator(ctx)?;
    require_transfer_actor(ctx, transfer_id, request_actor)?;
    apply_finish_step(&mut CtxShard { ctx }, transfer_id)
}

pub(crate) fn apply_finish_step<S: FinishSink>(
    sink: &mut S,
    transfer_id: u64,
) -> Result<(), String> {
    let has_out = sink.out_row(transfer_id).is_some();
    let has_in = sink.in_row(transfer_id).is_some();
    match plan_finish(has_out, has_in) {
        // Log this successful no-op because a call to the wrong Shard also leaves no Escrow here.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListingHolds {
    Delete,
    Keep,
}

pub(crate) fn listing_holds_after(blob: &[u8]) -> ListingHolds {
    let carried = decode_blob(0, blob).is_ok_and(|blob| {
        blob.manifest
            .iter()
            .any(|entry| entry.table == "game_auction_hold")
    });
    if carried {
        ListingHolds::Delete
    } else {
        ListingHolds::Keep
    }
}

// Restore listing Holds omitted by older blobs so the cascade cannot destroy their value.
fn cascade_for_transfer(ctx: &ReducerContext, guid: u64, listing_holds: ListingHolds) {
    use crate::auction::game_auction_hold;
    let kept: Vec<_> = match listing_holds {
        ListingHolds::Delete => Vec::new(),
        ListingHolds::Keep => ctx
            .db
            .game_auction_hold()
            .by_seller()
            .filter(&guid)
            .collect(),
    };
    crate::world::cascade_delete_character(ctx, guid);
    for hold in kept {
        ctx.db.game_auction_hold().insert(hold);
    }
}

pub(crate) fn apply_finish<S: FinishSink>(sink: &mut S, transfer_id: u64) {
    let mut cascaded = false;
    if let Some(out) = sink.out_row(transfer_id) {
        cascaded = out.cross_database;
        if out.cross_database {
            // Detach transported Command Receipts and party mirrors before deletion sweeps.
            sink.detach_for_transfer(out.character_guid);
            sink.cascade_delete_character(out.character_guid, listing_holds_after(&out.blob));
        }
        // The forwarding receipt and Escrow release must commit in the same transaction.
        sink.record_shard(out.character_guid, out.dest_map_id, out.dest_instance_id);
    }
    sink.clear_escrow(transfer_id);
    log::info!(
        "finish_transfer: {transfer_id} complete — escrow cleared, source copy {}",
        if cascaded {
            "DELETED (cross-database)"
        } else {
            "kept (same-database transfer)"
        }
    );
}

#[reducer]
pub fn reap_transfers(ctx: &ReducerContext, _schedule: TransferReaperSchedule) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    apply_reap(&mut CtxShard { ctx });
}

pub(crate) fn apply_reap<S: ReapSink>(sink: &mut S) {
    let now = sink.now_micros();
    for (transfer_id, character_guid, created_micros, cross_database) in sink.escrows() {
        let age = now - created_micros;
        let has_in = sink.in_row(transfer_id).is_some();
        // No local arrival receipt means unattested, not unimported, on a cross-Shard Transfer.
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
