use lyracore_shared::group::COMMAND_RESULT_WINDOW_MICROS;
use spacetimedb::{reducer, table, Identity, ReducerContext, Table, Timestamp};

use crate::game_group;

const COMMAND_LIFETIME_MICROS: i64 = 30_000_000;
const CLAIM_LEASE_MICROS: i64 = 2_000_000;
pub(crate) const RECEIPT_CAPACITY: usize = 32;

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandOutcome {
    Applied,
    Unchanged,
    Malformed,
    NotLeader,
    NotMember,
    StalePartyMirror,
    WrongAccount,
    MissingBot,
    WrongPartition,
    Suppressed,
    TargetDead,
    TargetUnavailable,
    TargetControlled,
    Expired,
    WaitingForCapacity,
    OutcomeUnknown,
    Superseded,
}

impl CommandOutcome {
    pub(crate) fn tag(&self) -> &'static str {
        match self {
            Self::Applied => "Applied",
            Self::Unchanged => "Unchanged",
            Self::Malformed => "Malformed",
            Self::NotLeader => "NotLeader",
            Self::NotMember => "NotMember",
            Self::StalePartyMirror => "StalePartyMirror",
            Self::WrongAccount => "WrongAccount",
            Self::MissingBot => "MissingBot",
            Self::WrongPartition => "WrongPartition",
            Self::Suppressed => "Suppressed",
            Self::TargetDead => "TargetDead",
            Self::TargetUnavailable => "TargetUnavailable",
            Self::TargetControlled => "TargetControlled",
            Self::Expired => "Expired",
            Self::WaitingForCapacity => "WaitingForCapacity",
            Self::OutcomeUnknown => "OutcomeUnknown",
            Self::Superseded => "Superseded",
        }
    }
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParsedClientCommand {
    pub kind: u8,
    pub bot_guid: u64,
    pub authority_member_guid: u64,
    pub exact_target_guid: u64,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug)]
pub struct AdmittedClientCommand {
    pub source_identity: Identity,
    pub intent_id: u64,
    pub issuer_guid: u64,
    pub issuer_sequence: u64,
    pub group_id: u64,
    pub leader_guid: u64,
    pub members: Vec<u64>,
    pub command: ParsedClientCommand,
    pub expires_micros: i64,
    pub receipt_retain_until_micros: i64,
}

#[derive(Clone, Copy)]
pub struct ClientCommandHandler {
    pub parse: fn(&str, &str) -> Option<Result<ParsedClientCommand, CommandOutcome>>,
    pub apply: fn(&ReducerContext, &AdmittedClientCommand) -> CommandOutcome,
    pub reply: &'static str,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub enum CommandIntentState {
    Pending,
    Finished(CommandOutcome),
}

#[table(
    accessor = game_party_command_intent,
    index(accessor = by_pending, btree(columns = [pending])),
    index(accessor = by_result_reap, btree(columns = [result_reap_micros]))
)]
pub struct PartyCommandIntent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub source_identity: Identity,
    pub issuer_guid: u64,
    pub reply_identity: Identity,
    pub issuer_sequence: u64,
    pub command: ParsedClientCommand,
    pub created_micros: i64,
    pub expires_micros: i64,
    pub result_reap_micros: i64,
    pub claim_token: u64,
    pub claim_until_micros: i64,
    pub pending: bool,
    pub state: CommandIntentState,
    pub dispatch_lane: u8,
    pub dispatch_next: u64,
}

#[table(accessor = game_party_command_dispatch_lane)]
pub struct PartyCommandDispatchLane {
    #[primary_key]
    pub lane: u8,
    pub head_intent_id: u64,
    pub tail_intent_id: u64,
}

#[table(accessor = game_party_command_issuer)]
pub struct PartyCommandIssuer {
    #[primary_key]
    pub character_guid: u64,
    pub last_sequence: u64,
}

crate::character_owned!(delete, fn sweep_delete_game_party_command_issuer(ctx, character_guid) {
    ctx.db
.game_party_command_issuer()
.character_guid()
.delete(character_guid);
});

crate::character_owned!(transfer, fn sweep_transfer_game_party_command_issuer(ctx, character_guid, io) {
    table = game_party_command_issuer,
    primary_key = character_guid,
});

#[table(
    accessor = game_party_command_receipt,
    index(accessor = by_source_intent, btree(columns = [source_identity, intent_id])),
    index(accessor = by_bot, btree(columns = [bot_guid])),
    index(accessor = by_retention, btree(columns = [retain_until_micros]))
)]
pub struct PartyCommandReceipt {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[unique]
    pub receipt_key: String,
    pub source_identity: Identity,
    pub intent_id: u64,
    pub bot_guid: u64,
    pub outcome: CommandOutcome,
    pub retain_until_micros: i64,
}

// Retain receipts through deletion until expiry so a lost acknowledgement cannot apply twice.
crate::character_owned!(delete, fn sweep_delete_game_party_command_receipt(_ctx, _character_guid) {});

pub(crate) fn detach_command_receipts_for_transfer(ctx: &ReducerContext, character_guid: u64) {
    let receipts = ctx.db.game_party_command_receipt();
    for row in receipts
        .by_bot()
        .filter(&character_guid)
        .take(RECEIPT_CAPACITY)
        .collect::<Vec<_>>()
    {
        receipts.id().delete(row.id);
    }
}

crate::character_owned!(transfer, fn sweep_transfer_game_party_command_receipt(ctx, character_guid, io) {
    table = game_party_command_receipt,
    by = by_bot,
    remint = id,
});

pub(crate) fn party_command_receipt_key(source_identity: Identity, intent_id: u64) -> String {
    format!("{source_identity}:{intent_id}")
}

#[table(accessor = game_addon_message, public)]
pub struct AddonMessage {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    pub cmd: String,
    pub payload: String,
    pub created_at: Timestamp,
}

pub(crate) fn send(ctx: &ReducerContext, character_guid: u64, cmd: &str, payload: &str) {
    let Some(c) = crate::helpers::character_by_guid(ctx, character_guid) else {
        return;
    };
    send_to_identity(ctx, c.owner_identity, cmd, payload);
}

fn send_to_identity(ctx: &ReducerContext, recipient_identity: Identity, cmd: &str, payload: &str) {
    let row = ctx.db.game_addon_message().insert(AddonMessage {
        id: 0,
        recipient_identity,
        cmd: cmd.to_string(),
        payload: payload.to_string(),
        created_at: ctx.timestamp,
    });
    crate::native::addon_message(ctx, &row);
}

#[cfg(not(feature = "debug_reducers"))]
fn client_command_handler() -> Option<ClientCommandHandler> {
    crate::GAME_CLIENT_COMMAND
}

#[cfg(feature = "debug_reducers")]
fn client_command_handler() -> Option<ClientCommandHandler> {
    crate::GAME_CLIENT_COMMAND.or(Some(FIXTURE_COMMAND))
}

#[cfg(feature = "debug_reducers")]
const FIXTURE_COMMAND: ClientCommandHandler = ClientCommandHandler {
    parse: parse_fixture_command,
    apply: |_, _| CommandOutcome::Applied,
    reply: "fixture.command.result",
};

#[cfg(feature = "debug_reducers")]
fn parse_fixture_command(
    cmd: &str,
    payload: &str,
) -> Option<Result<ParsedClientCommand, CommandOutcome>> {
    (cmd == "fixture.command").then(|| {
        let mut fields = payload.split('|');
        let bot_guid = fields
            .next()
            .and_then(|field| field.parse::<u64>().ok())
            .filter(|guid| *guid != 0)
            .ok_or(CommandOutcome::Malformed)?;
        let mut optional_guid = || {
            fields
                .next()
                .map_or(Ok(0), str::parse::<u64>)
                .map_err(|_| CommandOutcome::Malformed)
        };
        let authority_member_guid = optional_guid()?;
        let exact_target_guid = optional_guid()?;
        if fields.next().is_some() {
            return Err(CommandOutcome::Malformed);
        }
        Ok(ParsedClientCommand {
            kind: 0,
            bot_guid,
            authority_member_guid,
            exact_target_guid,
        })
    })
}

pub(crate) fn apply_client_command(
    ctx: &ReducerContext,
    character_guid: u64,
    cmd: &str,
    payload: &str,
) -> Result<(), String> {
    match cmd {
        "ping" => send(ctx, character_guid, "pong", payload),
        other => {
            let parsed = client_command_handler().and_then(|handler| {
                (handler.parse)(other, payload).map(|parsed| (handler, parsed))
            });
            match parsed {
                Some((handler, Ok(command))) => {
                    let now = ctx.timestamp.to_micros_since_unix_epoch();
                    let Some(reply_identity) =
                        crate::helpers::character_by_guid(ctx, character_guid)
                            .map(|character| character.owner_identity)
                    else {
                        return Ok(());
                    };
                    let issuers = ctx.db.game_party_command_issuer();
                    let current_issuer = issuers.character_guid().find(character_guid);
                    let issuer_sequence = current_issuer
                        .as_ref()
                        .map_or(1, |row| row.last_sequence.saturating_add(1));
                    let issuer = PartyCommandIssuer {
                        character_guid,
                        last_sequence: issuer_sequence,
                    };
                    if current_issuer.is_some() {
                        issuers.character_guid().update(issuer);
                    } else {
                        issuers.insert(issuer);
                    }
                    let dispatch_lane = (command.bot_guid
                        % u64::from(lyracore_shared::group::COMMAND_DISPATCH_LANES))
                        as u8;
                    let inserted = ctx
                        .db
                        .game_party_command_intent()
                        .insert(PartyCommandIntent {
                            id: 0,
                            source_identity: ctx.database_identity(),
                            issuer_guid: character_guid,
                            reply_identity,
                            issuer_sequence,
                            command,
                            created_micros: now,
                            expires_micros: now.saturating_add(COMMAND_LIFETIME_MICROS),
                            result_reap_micros: i64::MAX,
                            claim_token: 0,
                            claim_until_micros: 0,
                            pending: true,
                            state: CommandIntentState::Pending,
                            dispatch_lane,
                            dispatch_next: 0,
                        });
                    if crate::native::is_player(ctx, character_guid) {
                        let outcome = apply_local_party_command(ctx, &inserted, handler);
                        if outcome == CommandOutcome::WaitingForCapacity {
                            return Err(outcome.tag().to_string());
                        }
                        complete_party_command(ctx, inserted, outcome);
                    } else {
                        enqueue_party_command_intent(ctx, &inserted);
                    }
                }
                Some((handler, Err(outcome))) => {
                    send(ctx, character_guid, handler.reply, outcome.tag())
                }
                None => {
                    spacetimedb::log::info!(
                        "bridge: unknown client command {other:?} from {character_guid} (dropped)"
                    );
                }
            }
        }
    }
    Ok(())
}

fn apply_local_party_command(
    ctx: &ReducerContext,
    intent: &PartyCommandIntent,
    handler: ClientCommandHandler,
) -> CommandOutcome {
    if crate::helpers::character_by_guid(ctx, intent.command.bot_guid).is_none() {
        return CommandOutcome::MissingBot;
    }
    let Some(member) = crate::group::group_of(ctx, intent.issuer_guid) else {
        return CommandOutcome::NotMember;
    };
    let Some(group) = ctx.db.game_group().group_id().find(member.group_id) else {
        return CommandOutcome::StalePartyMirror;
    };
    if group.leader_guid != intent.issuer_guid {
        return CommandOutcome::NotLeader;
    }
    let Some(roster) =
        crate::group::bounded_roster(ctx, group.group_id, crate::group::group_kind_of(&group))
    else {
        return CommandOutcome::StalePartyMirror;
    };
    let members: Vec<_> = roster
        .into_iter()
        .map(|member| member.character_guid)
        .collect();
    if !members.contains(&intent.command.bot_guid)
        || (intent.command.authority_member_guid != 0
            && !members.contains(&intent.command.authority_member_guid))
    {
        return CommandOutcome::NotMember;
    }
    let partition = |guid| {
        use crate::game_world_entity;
        ctx.db
            .game_world_entity()
            .guid()
            .find(guid)
            .map(|entity| (entity.map_id, entity.instance_id))
    };
    let bot_partition = partition(intent.command.bot_guid);
    if bot_partition.is_none()
        || partition(intent.issuer_guid) != bot_partition
        || (intent.command.authority_member_guid != 0
            && partition(intent.command.authority_member_guid) != bot_partition)
    {
        return CommandOutcome::WrongPartition;
    }
    let command = AdmittedClientCommand {
        source_identity: intent.source_identity,
        intent_id: intent.id,
        issuer_guid: intent.issuer_guid,
        issuer_sequence: intent.issuer_sequence,
        group_id: group.group_id,
        leader_guid: group.leader_guid,
        members,
        command: intent.command,
        expires_micros: intent.expires_micros,
        receipt_retain_until_micros: intent
            .expires_micros
            .saturating_add(COMMAND_RESULT_WINDOW_MICROS),
    };
    crate::group::admit_party_command(ctx, &command)
        .unwrap_or_else(|| (handler.apply)(ctx, &command))
}

fn complete_party_command(
    ctx: &ReducerContext,
    mut intent: PartyCommandIntent,
    outcome: CommandOutcome,
) {
    intent.state = CommandIntentState::Finished(outcome);
    intent.pending = false;
    intent.claim_until_micros = 0;
    intent.result_reap_micros = ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .saturating_add(COMMAND_RESULT_WINDOW_MICROS);
    let response_id = intent.id;
    let reply_identity = intent.reply_identity;
    ctx.db.game_party_command_intent().id().update(intent);
    if let Some(handler) = client_command_handler() {
        send_to_identity(
            ctx,
            reply_identity,
            handler.reply,
            &format!("{response_id}|{}", outcome.tag()),
        );
    }
}

fn enqueue_party_command_intent(ctx: &ReducerContext, intent: &PartyCommandIntent) {
    let lanes = ctx.db.game_party_command_dispatch_lane();
    let intents = ctx.db.game_party_command_intent();
    match lanes.lane().find(intent.dispatch_lane) {
        Some(mut lane) => {
            let mut tail = intents
                .id()
                .find(lane.tail_intent_id)
                .expect("party command dispatch tail must name an intent");
            tail.dispatch_next = intent.id;
            intents.id().update(tail);
            lane.tail_intent_id = intent.id;
            lanes.lane().update(lane);
        }
        None => {
            lanes.insert(PartyCommandDispatchLane {
                lane: intent.dispatch_lane,
                head_intent_id: intent.id,
                tail_intent_id: intent.id,
            });
        }
    }
}

fn retire_party_command_intent(
    ctx: &ReducerContext,
    intent: &PartyCommandIntent,
) -> Result<(), String> {
    let lanes = ctx.db.game_party_command_dispatch_lane();
    let mut lane = lanes
        .lane()
        .find(intent.dispatch_lane)
        .ok_or_else(|| "MissingDispatchLane".to_string())?;
    if lane.head_intent_id != intent.id {
        return Err("IntentNotDispatchHead".to_string());
    }
    if intent.dispatch_next == 0 {
        lanes.lane().delete(intent.dispatch_lane);
    } else {
        lane.head_intent_id = intent.dispatch_next;
        lanes.lane().update(lane);
    }
    Ok(())
}

#[reducer]
pub fn defer_party_command_intent(
    ctx: &ReducerContext,
    intent_id: u64,
    claim_token: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let intents = ctx.db.game_party_command_intent();
    let mut intent = intents
        .id()
        .find(intent_id)
        .ok_or_else(|| "MissingIntent".to_string())?;
    if !matches!(intent.state, CommandIntentState::Pending) {
        return Err("Finished".to_string());
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let foreign_live_claim = intent.claim_token != 0
        && intent.claim_token != claim_token
        && intent.claim_until_micros > now;
    if intent.claim_token != claim_token && now < intent.expires_micros {
        return Err("ClaimLost".to_string());
    }
    let lanes = ctx.db.game_party_command_dispatch_lane();
    let mut lane = lanes
        .lane()
        .find(intent.dispatch_lane)
        .ok_or_else(|| "MissingDispatchLane".to_string())?;
    if lane.head_intent_id != intent.id {
        return Err("IntentNotDispatchHead".to_string());
    }
    // Expiry rotates the lane; keep its lease while the prior target transaction may finish.
    if !foreign_live_claim {
        intent.claim_token = 0;
        intent.claim_until_micros = 0;
    }
    if intent.dispatch_next == 0 {
        intents.id().update(intent);
        return Ok(());
    }
    let next_head = intent.dispatch_next;
    intent.dispatch_next = 0;
    let mut tail = intents
        .id()
        .find(lane.tail_intent_id)
        .ok_or_else(|| "MissingDispatchTail".to_string())?;
    tail.dispatch_next = intent.id;
    intents.id().update(tail);
    lane.head_intent_id = next_head;
    lane.tail_intent_id = intent.id;
    lanes.lane().update(lane);
    intents.id().update(intent);
    Ok(())
}

#[reducer]
pub fn claim_party_command_intent(
    ctx: &ReducerContext,
    intent_id: u64,
    claim_token: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    let table = ctx.db.game_party_command_intent();
    let mut intent = table
        .id()
        .find(intent_id)
        .ok_or_else(|| "MissingIntent".to_string())?;
    if !matches!(intent.state, CommandIntentState::Pending) {
        return Err("Finished".to_string());
    }
    if now >= intent.expires_micros {
        return Err("Expired".to_string());
    }
    if ctx
        .db
        .game_party_command_dispatch_lane()
        .lane()
        .find(intent.dispatch_lane)
        .is_none_or(|lane| lane.head_intent_id != intent.id)
    {
        return Err("IntentNotDispatchHead".to_string());
    }
    if intent.claim_token != 0
        && intent.claim_token != claim_token
        && intent.claim_until_micros > now
    {
        return Err("Claimed".to_string());
    }
    intent.claim_token = claim_token;
    intent.claim_until_micros = now.saturating_add(CLAIM_LEASE_MICROS);
    table.id().update(intent);
    Ok(())
}

#[reducer]
pub fn finish_party_command_intent(
    ctx: &ReducerContext,
    intent_id: u64,
    claim_token: u64,
    outcome: CommandOutcome,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if outcome == CommandOutcome::WaitingForCapacity {
        return Err("WaitingForCapacity is not terminal".to_string());
    }
    let table = ctx.db.game_party_command_intent();
    let intent = table
        .id()
        .find(intent_id)
        .ok_or_else(|| "MissingIntent".to_string())?;
    if let CommandIntentState::Finished(existing) = &intent.state {
        return (*existing == outcome)
            .then_some(())
            .ok_or_else(|| "Intent already finished with another outcome".to_string());
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    if intent.claim_token != claim_token
        && (now < intent.expires_micros || intent.claim_until_micros > now)
    {
        return Err("ClaimLost".to_string());
    }
    retire_party_command_intent(ctx, &intent)?;
    complete_party_command(ctx, intent, outcome);
    Ok(())
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn apply_admitted_party_command(
    ctx: &ReducerContext,
    source_identity: Identity,
    intent_id: u64,
    issuer_guid: u64,
    issuer_sequence: u64,
    group_id: u64,
    leader_guid: u64,
    members: Vec<u64>,
    kind: u8,
    bot_guid: u64,
    authority_member_guid: u64,
    exact_target_guid: u64,
    expires_micros: i64,
    receipt_retain_until_micros: i64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let receipts = ctx.db.game_party_command_receipt();
    if receipts
        .by_source_intent()
        .filter((source_identity, intent_id))
        .next()
        .is_some()
    {
        return Ok(());
    }
    if crate::transfer::is_in_transit(ctx, bot_guid) {
        return Err("TransferInProgress".to_string());
    }
    if crate::helpers::character_by_guid(ctx, bot_guid).is_none() {
        return Err("NotCharacterHolder".to_string());
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    if now >= expires_micros {
        return Err(CommandOutcome::Expired.tag().to_string());
    }
    for stale in receipts
        .by_bot()
        .filter(&bot_guid)
        .take(RECEIPT_CAPACITY + 1)
        .filter(|row| row.retain_until_micros <= now)
        .collect::<Vec<_>>()
    {
        receipts.id().delete(stale.id);
    }
    if receipts
        .by_bot()
        .filter(&bot_guid)
        .take(RECEIPT_CAPACITY)
        .count()
        == RECEIPT_CAPACITY
    {
        return Err(CommandOutcome::WaitingForCapacity.tag().to_string());
    }
    let command = AdmittedClientCommand {
        source_identity,
        intent_id,
        issuer_guid,
        issuer_sequence,
        group_id,
        leader_guid,
        members,
        command: ParsedClientCommand {
            kind,
            bot_guid,
            authority_member_guid,
            exact_target_guid,
        },
        expires_micros,
        receipt_retain_until_micros,
    };
    let outcome = crate::group::admit_party_command(ctx, &command).unwrap_or_else(|| {
        client_command_handler().map_or(CommandOutcome::Suppressed, |handler| {
            (handler.apply)(ctx, &command)
        })
    });
    if outcome == CommandOutcome::WaitingForCapacity {
        return Err(outcome.tag().to_string());
    }
    receipts.insert(PartyCommandReceipt {
        id: 0,
        receipt_key: party_command_receipt_key(source_identity, intent_id),
        source_identity,
        intent_id,
        bot_guid,
        outcome,
        retain_until_micros: receipt_retain_until_micros,
    });
    Ok(())
}

#[reducer]
pub fn confirm_party_command_receipt(
    ctx: &ReducerContext,
    source_identity: Identity,
    intent_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    match ctx
        .db
        .game_party_command_receipt()
        .by_source_intent()
        .filter((source_identity, intent_id))
        .next()
    {
        Some(receipt) => Err(receipt.outcome.tag().to_string()),
        None => Ok(()),
    }
}

#[reducer]
pub fn confirm_party_command_holder(ctx: &ReducerContext, bot_guid: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if crate::transfer::is_in_transit(ctx, bot_guid) {
        return Err("TransferInProgress".to_string());
    }
    crate::helpers::character_by_guid(ctx, bot_guid)
        .is_some()
        .then_some(())
        .ok_or_else(|| CommandOutcome::MissingBot.tag().to_string())
}

#[cfg(feature = "debug_reducers")]
fn fixture_command_intent(
    ctx: &ReducerContext,
    intent_id: u64,
) -> Result<PartyCommandIntent, String> {
    ctx.db
        .game_party_command_intent()
        .id()
        .find(intent_id)
        .ok_or_else(|| "fixture command intent missing".to_string())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn party_command_fixture_apply(
    ctx: &ReducerContext,
    intent_id: u64,
    claim_token: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    fixture_command_apply_after_authority(ctx, intent_id, claim_token, 0)
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn party_command_fixture_apply_after_gate_change(
    ctx: &ReducerContext,
    intent_id: u64,
    claim_token: u64,
    mode: u8,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if !matches!(mode, 1 | 2) {
        return Err("unknown command Gate fixture mode".to_string());
    }
    fixture_command_apply_after_authority(ctx, intent_id, claim_token, mode)
}

#[cfg(feature = "debug_reducers")]
fn fixture_command_apply_after_authority(
    ctx: &ReducerContext,
    intent_id: u64,
    claim_token: u64,
    gate_change: u8,
) -> Result<(), String> {
    claim_party_command_intent(ctx, intent_id, claim_token)?;
    let intent = fixture_command_intent(ctx, intent_id)?;
    let member = crate::group::group_of(ctx, intent.issuer_guid)
        .ok_or_else(|| CommandOutcome::NotMember.tag().to_string())?;
    let group = ctx
        .db
        .game_group()
        .group_id()
        .find(member.group_id)
        .ok_or_else(|| CommandOutcome::NotMember.tag().to_string())?;
    let members: Vec<_> =
        crate::group::bounded_roster(ctx, member.group_id, crate::group::group_kind_of(&group))
            .ok_or_else(|| CommandOutcome::StalePartyMirror.tag().to_string())?
            .into_iter()
            .map(|row| row.character_guid)
            .collect();
    crate::group::admit_party_command_authority(
        ctx,
        member.group_id,
        intent.issuer_guid,
        intent.command.bot_guid,
        intent.command.authority_member_guid,
        members.clone(),
    )?;
    match gate_change {
        0 => {}
        1 => {
            crate::WorldEntity::delete(ctx, intent.command.bot_guid);
        }
        2 => {
            crate::sessionless::set_sessionless_action_consent(ctx, intent.command.bot_guid, false)
        }
        _ => unreachable!("fixture mode checked by reducer"),
    }
    let result = apply_admitted_party_command(
        ctx,
        intent.source_identity,
        intent.id,
        intent.issuer_guid,
        intent.issuer_sequence,
        member.group_id,
        group.leader_guid,
        members,
        intent.command.kind,
        intent.command.bot_guid,
        intent.command.authority_member_guid,
        intent.command.exact_target_guid,
        intent.expires_micros,
        intent
            .expires_micros
            .saturating_add(COMMAND_RESULT_WINDOW_MICROS),
    );
    match result {
        Err(refusal) if refusal == CommandOutcome::WaitingForCapacity.tag() => Ok(()),
        other => other,
    }
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn party_command_fixture_finish(
    ctx: &ReducerContext,
    intent_id: u64,
    claim_token: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let intent = fixture_command_intent(ctx, intent_id)?;
    let receipt = ctx
        .db
        .game_party_command_receipt()
        .by_source_intent()
        .filter((intent.source_identity, intent.id))
        .next()
        .ok_or_else(|| "fixture command has no terminal target receipt".to_string())?;
    finish_party_command_intent(ctx, intent.id, claim_token, receipt.outcome)
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn party_command_fixture_drive(
    ctx: &ReducerContext,
    intent_id: u64,
    claim_token: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    party_command_fixture_apply(ctx, intent_id, claim_token)?;
    if ctx
        .db
        .game_party_command_receipt()
        .by_source_intent()
        .filter((ctx.database_identity(), intent_id))
        .next()
        .is_some()
    {
        party_command_fixture_finish(ctx, intent_id, claim_token)?;
    }
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn party_command_fixture_release_receipt(
    ctx: &ReducerContext,
    bot_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let receipts = ctx.db.game_party_command_receipt();
    let oldest = receipts
        .by_bot()
        .filter(&bot_guid)
        .take(RECEIPT_CAPACITY + 1)
        .min_by_key(|row| row.id)
        .ok_or("fixture command receipt missing")?;
    receipts.id().delete(oldest.id);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn party_command_fixture_expire(ctx: &ReducerContext, intent_id: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let table = ctx.db.game_party_command_intent();
    let mut intent = fixture_command_intent(ctx, intent_id)?;
    if !matches!(intent.state, CommandIntentState::Pending) {
        return Err("fixture command is already terminal".to_string());
    }
    intent.expires_micros = ctx.timestamp.to_micros_since_unix_epoch().saturating_sub(1);
    table.id().update(intent);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn party_command_fixture_expire_after_receipt_window(
    ctx: &ReducerContext,
    intent_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let table = ctx.db.game_party_command_intent();
    let mut intent = fixture_command_intent(ctx, intent_id)?;
    if !matches!(intent.state, CommandIntentState::Pending) {
        return Err("fixture command is already terminal".to_string());
    }
    intent.expires_micros = ctx
        .timestamp
        .to_micros_since_unix_epoch()
        .saturating_sub(COMMAND_RESULT_WINDOW_MICROS)
        .saturating_sub(1);
    table.id().update(intent);
    Ok(())
}
