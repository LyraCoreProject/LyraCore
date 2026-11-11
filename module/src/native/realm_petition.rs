use lyracore_protocol::messages::{
    vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        GuildCommand, GuildCommandResult, GuildEmblemResult, PetitionResult, MSG_PETITION_RENAME,
    },
    Guid,
};
use lyracore_shared::{
    faction::team_for_race,
    guild::{GuildRefusal, GUILD_CHARTER_ENTRY},
};
use spacetimedb::{
    reducer, sats::bsatn, table, ConnectionId, ReducerContext, ScheduleAt, SpacetimeType, Table,
    TimeDuration,
};

use super::{
    game_native_player, guild::Actor, petition as wire, realm_guild,
    realm_guild::game_guild_character, NativeWorldConnection, Rollback,
};
use crate::game_item_instance;
use crate::guild::{
    self,
    fee::{
        self, GuildCharterPurchase, GuildEmblem, GuildEmblemPurchase, GuildFeeRequest, GuildPayment,
    },
    petition::{
        self, game_guild_petition, game_guild_petition_signature, GuildPetition,
        GuildPetitionOffer, GuildPetitionRename, GuildPetitionSign, GuildPetitionSignature,
    },
    GuildOp,
};

// Opcode one is private to Realm guild commands; client opcodes start above it in this path.
pub(super) const TRANSACTION: u32 = 1;
const ABSENT: &str = "guild charter has no Realm petition";

#[derive(SpacetimeType)]
pub enum Operation {
    Payment(GuildPayment),
    TurnIn(u64),
    Close(u64),
    Verify(u64),
    Import(GuildPetitionImport),
    Reconcile(Vec<u64>),
}

impl Operation {
    fn charter(&self) -> Option<u64> {
        match self {
            Self::Payment(GuildPayment::Charter(terms)) => Some(terms.charter_item_guid),
            Self::TurnIn(charter) | Self::Close(charter) | Self::Verify(charter) => Some(*charter),
            Self::Import(state) => Some(state.petition.charter_item_guid),
            Self::Payment(GuildPayment::Emblem(_)) | Self::Reconcile(_) => None,
        }
    }
    pub(super) fn background(&self) -> bool {
        matches!(
            self,
            Self::Close(_) | Self::Verify(_) | Self::Import(_) | Self::Reconcile(_)
        )
    }
}

#[table(accessor = game_guild_transaction)]
pub struct GuildTransaction {
    #[primary_key]
    pub guid: u64,
    pub call_id: u64,
    pub operation: Operation,
    pub copper: u32,
    pub outcome: Option<Result<(), String>>,
}

#[table(accessor = game_guild_transaction_retry, scheduled(retry_guild_transaction))]
pub struct GuildTransactionRetry {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub scheduled_at: ScheduleAt,
    pub guid: u64,
}

crate::outside_table_catalog!(game_guild_transaction, game_guild_transaction_retry);
crate::virtual_clock_exempt!(game_guild_transaction_retry);

pub(crate) fn pending(ctx: &ReducerContext, guid: u64) -> bool {
    ctx.db.game_guild_transaction().guid().find(guid).is_some()
}

fn start(ctx: &ReducerContext, guid: u64, operation: Operation, copper: u32) -> Result<(), String> {
    if pending(ctx, guid) {
        return Err("guild transaction is settling".into());
    }
    let body = bsatn::to_vec(&operation).expect("guild transaction encodes");
    let call = realm_guild::queue(ctx, guid, TRANSACTION, body)?;
    if !matches!(operation, Operation::Verify(_)) {
        ctx.db.game_guild_transaction().insert(GuildTransaction {
            guid,
            call_id: call.id,
            operation,
            copper,
            outcome: None,
        });
    }
    Ok(())
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: &NativeWorldConnection,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Rollback> {
    if pending(ctx, guid)
        || realm_guild::has_local_guilds(ctx)
        || ctx
            .db
            .game_guild_petition()
            .owner_guid()
            .find(guid)
            .is_some()
    {
        return Err(Rollback::reply(
            realm_guild::unavailable(),
            "guild migration or transaction is pending",
        ));
    }
    use ClientOpcodeMessage as C;
    match message {
        C::MSG_SAVE_GUILD_EMBLEM(request) => {
            let (payment, copper) = fee::prepare(
                ctx,
                guid,
                GuildFeeRequest::Emblem(GuildEmblemPurchase {
                    npc_guid: request.vendor.guid(),
                    emblem: GuildEmblem {
                        emblem_style: request.emblem_style,
                        emblem_color: request.emblem_color,
                        border_style: request.border_style,
                        border_color: request.border_color,
                        background_color: request.background_color,
                    },
                }),
            )
            .map_err(|cause| wire::refused(cause, &|refusal| Some(emblem_refusal(refusal))))?;
            start(ctx, guid, Operation::Payment(payment), copper)?;
        }
        C::CMSG_PETITION_BUY(request) => {
            let npc = request.npc.guid();
            let name = request.name;
            let (payment, copper) = fee::prepare(
                ctx,
                guid,
                GuildFeeRequest::Charter(GuildCharterPurchase {
                    npc_guid: npc,
                    name: name.clone(),
                }),
            )
            .map_err(|cause| {
                wire::refused(cause, &|refusal| {
                    wire::purchase_refusal(refusal, npc, &name)
                })
            })?;
            start(ctx, guid, Operation::Payment(payment), copper)?;
        }
        C::CMSG_TURN_IN_PETITION(request) => {
            let charter = request.petition.guid();
            if petition::holds_charter(ctx, guid, charter) {
                start(ctx, guid, Operation::TurnIn(charter), 0)?;
            }
        }
        message => {
            match &message {
                C::MSG_PETITION_RENAME(request)
                    if !petition::holds_charter(ctx, guid, request.petition.guid()) =>
                {
                    return Ok(())
                }
                C::CMSG_OFFER_PETITION(request)
                    if !petition::holds_charter(ctx, guid, request.petition.guid())
                        || super::available_entity(ctx, request.target.guid()).is_err() =>
                {
                    return Ok(())
                }
                _ => {}
            }
            realm_guild::submit(ctx, connection.connection_id, guid, &message)?;
        }
    }
    Ok(())
}

fn emblem_refusal(refusal: GuildRefusal) -> ServerOpcodeMessage {
    wire::emblem_result(match refusal {
        GuildRefusal::NotInGuild => GuildEmblemResult::NoGuild,
        GuildRefusal::NotLeader => GuildEmblemResult::NotGuildMaster,
        GuildRefusal::NotEnoughMoney => GuildEmblemResult::NotEnoughMoney,
        _ => GuildEmblemResult::NoMessage,
    })
}

pub(super) fn apply(
    ctx: &ReducerContext,
    actor: &Actor,
    operation: Operation,
) -> Result<Option<ServerOpcodeMessage>, Rollback> {
    match operation {
        Operation::Payment(payment) => {
            if let GuildPayment::Charter(terms) = &payment {
                if terms.payer_name != actor.name
                    || terms.payer_team != actor.team
                    || terms.charter_item_guid == 0
                {
                    return Err("charter payer facts differ from the World Session".into());
                }
            }
            fee::accept(ctx, actor.guid, &payment).map_err(|refusal| {
                wire::refused(refusal.as_tag().into(), &|refusal| match &payment {
                    GuildPayment::Emblem(_) => Some(emblem_refusal(refusal)),
                    GuildPayment::Charter(terms) => wire::purchase_refusal(refusal, 0, &terms.name),
                })
            })?;
            Ok(matches!(payment, GuildPayment::Emblem(_))
                .then(|| wire::emblem_result(GuildEmblemResult::Success)))
        }
        Operation::TurnIn(charter) => {
            let name = wire::view(ctx, charter).map_or(String::new(), |petition| petition.name);
            guild::apply_op(
                ctx,
                actor.guid,
                actor.account_id,
                GuildOp::TurnInPetition {
                    charter_item_guid: charter,
                },
            )
            .map_err(|refusal| {
                wire::refused(refusal.as_tag().into(), &|refusal| match refusal {
                    GuildRefusal::AlreadyInGuild => {
                        Some(wire::turn_in_result(PetitionResult::AlreadyInGuild))
                    }
                    GuildRefusal::NeedMoreSignatures => {
                        Some(wire::turn_in_result(PetitionResult::NeedMore))
                    }
                    GuildRefusal::NameExists => wire::name_refusal(refusal, &name),
                    _ => None,
                })
            })?;
            let packet = super::bounded(wire::command(
                GuildCommand::Create,
                &name,
                GuildCommandResult::PlayerNoMoreInGuild,
            ));
            realm_guild::deliver(ctx, actor.guid, vec![(packet.opcode, packet.body)], None);
            Ok(Some(wire::turn_in_result(PetitionResult::Ok)))
        }
        Operation::Close(charter) => {
            if let Some(row) = ctx
                .db
                .game_guild_petition()
                .charter_item_guid()
                .find(charter)
            {
                petition::close(ctx, actor.guid, row.petition_id)
                    .map_err(|error| error.as_tag().to_string())?;
            }
            Ok(None)
        }
        Operation::Verify(charter) => {
            let row = ctx
                .db
                .game_guild_petition()
                .charter_item_guid()
                .find(charter)
                .filter(|row| row.owner_guid == actor.guid)
                .ok_or(ABSENT)?;
            super::petition::charter_opened(ctx, &row);
            Ok(None)
        }
        Operation::Import(state) => {
            import(ctx, actor, state)?;
            Ok(None)
        }
        Operation::Reconcile(items) => {
            if let Some(row) = ctx.db.game_guild_petition().owner_guid().find(actor.guid) {
                if items.contains(&row.charter_item_guid) {
                    super::petition::charter_opened(ctx, &row);
                } else {
                    petition::close(ctx, actor.guid, row.petition_id)
                        .map_err(|error| error.as_tag().to_string())?;
                }
            }
            Ok(None)
        }
    }
}

pub(super) fn answered(
    ctx: &ReducerContext,
    call: &realm_guild::RealmGuildCommand,
    guid: u64,
    connection: u128,
    result: Result<(), String>,
) -> Result<(), String> {
    let operation: Operation = bsatn::from_slice(&call.body).map_err(|error| error.to_string())?;
    if let Operation::Verify(charter) = operation {
        if result.as_ref().is_err_and(|error| error == ABSENT)
            && petition::holds_charter(ctx, guid, charter)
            && !pending(ctx, guid)
            && !crate::transfer::is_in_transit(ctx, guid)
        {
            crate::ItemInstance::delete(ctx, charter);
        }
        return Ok(());
    }
    let table = ctx.db.game_guild_transaction();
    let Some(mut row) = table
        .guid()
        .find(guid)
        .filter(|row| row.call_id == call.id && row.outcome.is_none())
    else {
        return Ok(());
    };
    if let Err(error) = &result {
        let connection = ConnectionId::from_u128(connection);
        if ctx
            .db
            .game_native_player()
            .connection_id()
            .find(connection)
            .is_some_and(|player| player.character_guid == guid)
        {
            if let Some(packets) = super::rollback::replies_in(error) {
                for (opcode, body) in packets {
                    super::send_raw(ctx, connection, opcode, body, false);
                }
            } else {
                super::send(ctx, connection, realm_guild::unavailable(), false)?;
            }
        }
    }
    row.outcome = Some(result);
    table.guid().update(row);
    settle(ctx, guid);
    Ok(())
}

fn settle(ctx: &ReducerContext, guid: u64) {
    let table = ctx.db.game_guild_transaction();
    let Some(mut row) = table.guid().find(guid) else {
        return;
    };
    let Some(result) = &row.outcome else { return };
    let accepted = result.is_ok();
    if !accepted && matches!(row.operation, Operation::Close(_) | Operation::Reconcile(_)) {
        retry_later(ctx, guid);
        return;
    }
    if !accepted && row.copper != 0 {
        row.copper = crate::helpers::credit_purse(ctx, guid, row.copper);
        if row.copper != 0 {
            table.guid().update(row);
            retry_later(ctx, guid);
            return;
        }
    }
    let charter = row.operation.charter();
    if let Some(charter) = charter {
        let consume = matches!(row.operation, Operation::TurnIn(_)) && accepted
            || matches!(row.operation, Operation::Payment(GuildPayment::Charter(_))) && !accepted;
        if consume && petition::holds_charter(ctx, guid, charter) {
            crate::ItemInstance::delete(ctx, charter);
        }
    }
    if accepted {
        if let Operation::Import(state) = &row.operation {
            let original = ctx
                .db
                .game_guild_petition()
                .petition_id()
                .find(state.petition.petition_id);
            if original
                .is_some_and(|old| bsatn::to_vec(&old).ok() == bsatn::to_vec(&state.petition).ok())
            {
                ctx.db
                    .game_guild_petition_signature()
                    .by_petition()
                    .delete(state.petition.petition_id);
                ctx.db
                    .game_guild_petition()
                    .petition_id()
                    .delete(state.petition.petition_id);
            }
        }
    }
    table.guid().delete(guid);
    if let Operation::Reconcile(items) = row.operation {
        if charter_items(ctx, guid) != items {
            replay(ctx, guid);
        } else {
            for charter in items {
                if let Err(error) = start(ctx, guid, Operation::Verify(charter), 0) {
                    spacetimedb::log::error!(
                        "guild charter {charter} verification could not start: {error}"
                    );
                }
            }
        }
    } else if !matches!(row.operation, Operation::Import(_)) {
        replay(ctx, guid);
    }
}

fn retry_later(ctx: &ReducerContext, guid: u64) {
    ctx.db
        .game_guild_transaction_retry()
        .insert(GuildTransactionRetry {
            id: 0,
            scheduled_at: ScheduleAt::Time(ctx.timestamp + TimeDuration::from_micros(1_000_000)),
            guid,
        });
}

#[reducer]
pub fn retry_guild_transaction(ctx: &ReducerContext, retry: GuildTransactionRetry) {
    if !crate::time::scheduler_authorized(ctx) {
        return;
    }
    let Some(mut row) = ctx.db.game_guild_transaction().guid().find(retry.guid) else {
        return;
    };
    if row.outcome.as_ref().is_some_and(Result::is_err)
        && matches!(row.operation, Operation::Close(_) | Operation::Reconcile(_))
    {
        let body = bsatn::to_vec(&row.operation).expect("guild transaction encodes");
        match realm_guild::queue(ctx, row.guid, TRANSACTION, body) {
            Ok(call) => {
                row.call_id = call.id;
                row.outcome = None;
                ctx.db.game_guild_transaction().guid().update(row);
            }
            Err(_) => retry_later(ctx, row.guid),
        }
    } else {
        settle(ctx, retry.guid);
    }
}

pub(crate) fn item_removed(ctx: &ReducerContext, item: &crate::ItemInstance) {
    if item.entry != GUILD_CHARTER_ENTRY
        || crate::peer::realm_shard(ctx).is_none()
        || pending(ctx, item.owner_guid)
        || crate::transfer::is_in_transit(ctx, item.owner_guid)
        || !super::has_world_session(ctx, item.owner_guid)
    {
        return;
    }
    if let Err(error) = start(ctx, item.owner_guid, Operation::Close(item.guid), 0) {
        spacetimedb::log::error!(
            "guild charter {} cleanup could not start: {error}",
            item.guid
        );
    }
}

fn charter_items(ctx: &ReducerContext, guid: u64) -> Vec<u64> {
    let mut items: Vec<_> = ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(guid)
        .filter(|item| item.entry == GUILD_CHARTER_ENTRY)
        .map(|item| item.guid)
        .chain(
            crate::items::buyback_entries(ctx, guid)
                .into_iter()
                .filter(|row| row.item.entry == GUILD_CHARTER_ENTRY)
                .map(|row| row.item_guid),
        )
        .collect();
    items.sort_unstable();
    items
}

pub(super) fn replay(ctx: &ReducerContext, guid: u64) {
    if pending(ctx, guid)
        || ctx
            .db
            .game_guild_petition()
            .owner_guid()
            .find(guid)
            .is_some()
    {
        return;
    }
    if let Err(error) = start(ctx, guid, Operation::Reconcile(charter_items(ctx, guid)), 0) {
        spacetimedb::log::error!(
            "Character {guid} guild charter reconciliation could not start: {error}"
        );
    }
}

pub(super) fn is_request(message: &ClientOpcodeMessage) -> bool {
    use ClientOpcodeMessage as C;
    matches!(
        message,
        C::CMSG_PETITION_QUERY(_)
            | C::CMSG_PETITION_SHOW_SIGNATURES(_)
            | C::MSG_PETITION_RENAME(_)
            | C::CMSG_OFFER_PETITION(_)
            | C::CMSG_PETITION_SIGN(_)
            | C::MSG_PETITION_DECLINE(_)
    )
}

pub(super) fn request(
    ctx: &ReducerContext,
    actor: &Actor,
    message: ClientOpcodeMessage,
) -> Result<Option<ServerOpcodeMessage>, Rollback> {
    use ClientOpcodeMessage as C;
    let run = |op, reply: &dyn Fn(GuildRefusal) -> Option<ServerOpcodeMessage>| {
        guild::apply_op(ctx, actor.guid, actor.account_id, op)
            .map_err(|refusal| wire::refused(refusal.as_tag().into(), reply))
    };
    match message {
        C::CMSG_PETITION_QUERY(request) => {
            Ok(wire::view(ctx, request.petition.guid()).map(|view| wire::query(&view)))
        }
        C::CMSG_PETITION_SHOW_SIGNATURES(request) => {
            Ok(if guild::member(ctx, actor.guid).is_none() {
                wire::view(ctx, request.item.guid()).map(|view| wire::signatures(&view))
            } else {
                None
            })
        }
        C::MSG_PETITION_RENAME(request) => {
            let charter = request.petition.guid();
            let name = request.new_name;
            run(
                GuildOp::RenamePetition(GuildPetitionRename {
                    charter_item_guid: charter,
                    name: name.clone(),
                }),
                &|refusal| wire::name_refusal(refusal, &name),
            )?;
            Ok(Some(ServerOpcodeMessage::MSG_PETITION_RENAME(Box::new(
                MSG_PETITION_RENAME {
                    petition: Guid::new(charter),
                    new_name: name,
                },
            ))))
        }
        C::CMSG_OFFER_PETITION(request) => {
            let target = ctx
                .db
                .game_guild_character()
                .guid()
                .find(request.target.guid())
                .filter(|row| {
                    row.facts.character.online
                        && super::realm_party::current(ctx, &row.facts.character)
                            == Some(row.facts.character.world)
                })
                .ok_or_else(|| Rollback::silent("petition target is not in world"))?;
            run(
                GuildOp::OfferPetition(GuildPetitionOffer {
                    charter_item_guid: request.petition.guid(),
                    target_guid: target.guid,
                    target_team: team_for_race(target.facts.character.race),
                }),
                &|refusal| wire::standing_refusal(refusal, &target.facts.character.name),
            )?;
            Ok(None)
        }
        C::CMSG_PETITION_SIGN(request) => {
            let charter = request.petition.guid();
            run(
                GuildOp::SignPetition(GuildPetitionSign {
                    charter_item_guid: charter,
                    actor_name: actor.name.clone(),
                    actor_team: actor.team,
                }),
                &|refusal| match refusal {
                    GuildRefusal::CantSignOwn => Some(wire::sign_result(
                        charter,
                        actor.guid,
                        PetitionResult::CantSignOwn,
                    )),
                    _ => wire::standing_refusal(refusal, &actor.name),
                },
            )?;
            Ok(None)
        }
        C::MSG_PETITION_DECLINE(request) => {
            run(
                GuildOp::DeclinePetition {
                    charter_item_guid: request.petition.guid(),
                },
                &|_| None,
            )?;
            Ok(None)
        }
        _ => Err("opcode is not a Realm petition request".into()),
    }
}

#[derive(SpacetimeType)]
pub struct GuildPetitionImport {
    pub petition: GuildPetition,
    pub signatures: Vec<GuildPetitionSignature>,
}

#[reducer]
pub fn operator_report_guild_petitions(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if crate::peer::realm_shard(ctx).is_none()
        || ctx.db.game_native_player().iter().next().is_some()
    {
        return Err("petition migration requires a World Shard with gameplay closed".into());
    }
    for petition in ctx.db.game_guild_petition().iter() {
        if !petition::holds_charter(ctx, petition.owner_guid, petition.charter_item_guid) {
            return Err("legacy petition has no owned charter item".into());
        }
        let signatures = ctx
            .db
            .game_guild_petition_signature()
            .by_petition()
            .filter(petition.petition_id)
            .collect();
        start(
            ctx,
            petition.owner_guid,
            Operation::Import(GuildPetitionImport {
                petition,
                signatures,
            }),
            0,
        )?;
    }
    Ok(())
}

fn import(ctx: &ReducerContext, actor: &Actor, state: GuildPetitionImport) -> Result<(), String> {
    use super::{directory::game_character_profile, realm_shard::game_native_away};
    use lyracore_shared::guild::{
        petition_signature_key, validate_guild_name, MAX_PETITION_SIGNATURES,
    };
    if ctx
        .db
        .game_native_away()
        .iter()
        .any(|away| away.character_guid != 0)
    {
        return Err("petition migration requires a Realm Shard with gameplay closed".into());
    }
    let row = &state.petition;
    let profile = ctx
        .db
        .game_character_profile()
        .guid()
        .find(row.owner_guid)
        .filter(|profile| !profile.deleted)
        .ok_or("petition owner needs a Character profile")?;
    if row.petition_id == 0
        || row.charter_item_guid == 0
        || row.owner_guid != actor.guid
        || row.owner_name != profile.name
        || profile.account_id != actor.account_id
        || row.team != team_for_race(profile.race)
        || validate_guild_name(&row.name).is_err()
        || guild::member(ctx, row.owner_guid).is_some()
        || state.signatures.len() > MAX_PETITION_SIGNATURES
    {
        return Err("legacy petition facts are inconsistent".into());
    }
    let table = ctx.db.game_guild_petition();
    if table.petition_id().find(row.petition_id).is_some()
        || table.owner_guid().find(row.owner_guid).is_some()
        || table
            .charter_item_guid()
            .find(row.charter_item_guid)
            .is_some()
    {
        return Err("legacy petition conflicts with Realm state".into());
    }
    let mut signers = std::collections::BTreeSet::new();
    let mut accounts = std::collections::BTreeSet::new();
    let mut slots = std::collections::BTreeSet::new();
    for signature in &state.signatures {
        let profile = ctx
            .db
            .game_character_profile()
            .guid()
            .find(signature.signer_guid)
            .filter(|profile| !profile.deleted)
            .ok_or("petition signer needs a Character profile")?;
        if signature.petition_id != row.petition_id
            || signature.signer_guid == row.owner_guid
            || signature.signer_name != profile.name
            || signature.signer_realm_account != profile.account_id
            || profile.account_id == 0
            || team_for_race(profile.race) != row.team
            || !signers.insert(signature.signer_guid)
            || !accounts.insert(profile.account_id)
            || !slots.insert(signature.signature_key)
            || !(0..MAX_PETITION_SIGNATURES).any(|slot| {
                petition_signature_key(row.petition_id, slot) == signature.signature_key
            })
        {
            return Err("legacy petition signatures are inconsistent".into());
        }
    }
    crate::guild::IdSequence::Petition.observe(ctx, row.petition_id);
    state.petition.insert(ctx);
    for signature in state.signatures {
        ctx.db.game_guild_petition_signature().insert(signature);
    }
    Ok(())
}
