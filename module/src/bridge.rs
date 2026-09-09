//! Addon⇄server message bridge (work-item 184) — the UI↔server RPC channel custom features ride.
//!
//! Client→server: the 1.12 client's `SendAddonMessage` is `CMSG_MESSAGECHAT` with language
//! `LANG_ADDON` (0xFFFFFFFF) and text `"<prefix>\t<message>"`. The GATEWAY intercepts those frames
//! raw (gtker's Language enum can't decode 0xFFFFFFFF), parses the `STC` envelope, and calls the
//! [`client_command`] reducer AS the player — a command handler has exactly a reducer's authority.
//!
//! Server→client: [`send`] inserts a [`AddonMessage`] row; the gateway relays it (COORDINATOR
//! connection — the 279 delivery law: addon UI state must survive fat transactions) as an
//! addon-language whisper the client surfaces to addons as `CHAT_MSG_ADDON`.
//!
//! Core owns `ping`; one installed Package may register a parser and admitted apply operation with
//! `game_client_command!`. Parsing queues only a pending intent. Gateway certifies Realm-core party
//! authority before the target World Shard applies gameplay.

use spacetimedb::{reducer, table, Identity, ReducerContext, Table, Timestamp};

use crate::{game_character, game_group, game_group_member};

const COMMAND_LIFETIME_MICROS: i64 = 30_000_000;
const COMMAND_RESULT_WINDOW_MICROS: i64 = 30_000_000;
const CLAIM_LEASE_MICROS: i64 = 2_000_000;
const RECEIPT_CAPACITY: usize = 32;

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
}

impl CommandOutcome {
    pub fn tag(&self) -> &'static str {
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
/// An authenticated client command awaiting Gateway party-authority admission. [entity]
pub struct PartyCommandIntent {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub source_identity: Identity,
    pub issuer_guid: u64,
    pub command: ParsedClientCommand,
    pub created_micros: i64,
    pub expires_micros: i64,
    pub result_reap_micros: i64,
    pub claim_token: u64,
    pub claim_until_micros: i64,
    pub pending: bool,
    pub state: CommandIntentState,
}

#[table(
    accessor = game_party_command_receipt,
    index(accessor = by_source_intent, btree(columns = [source_identity, intent_id])),
    index(accessor = by_bot, btree(columns = [bot_guid])),
    index(accessor = by_retention, btree(columns = [retain_until_micros]))
)]
/// The target Shard's idempotency result for one source Module and intent id. [entity]
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

pub(crate) fn party_command_receipt_key(source_identity: Identity, intent_id: u64) -> String {
    format!("{source_identity}:{intent_id}")
}

/// One server→client addon message: relayed by the gateway as an addon-language whisper, arriving
/// in the recipient's addons as `CHAT_MSG_ADDON` with `prefix == "STC"`. TTL-reaped with the other
/// event tables (same `created_at` shape). [event]
#[table(accessor = game_addon_message, public)]
pub struct AddonMessage {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub recipient_identity: Identity,
    /// Command name, e.g. `"pong"` or `"event.progress"` — the addon routes on it.
    pub cmd: String,
    /// Command payload, a command-defined micro-format (no JSON — 255-byte wire budget).
    pub payload: String,
    pub created_at: Timestamp,
}

/// Queue an addon message for `character_guid`'s live session. The recipient identity comes off
/// the durable character row (its owner binding) — a logged-out recipient's row is inserted and
/// simply reaped unread (the 1s event TTL), which is the right semantics for UI state.
pub(crate) fn send(ctx: &ReducerContext, character_guid: u64, cmd: &str, payload: &str) {
    let Some(c) = ctx.db.game_character().guid().find(character_guid) else {
        return;
    };
    ctx.db.game_addon_message().insert(AddonMessage {
        id: 0,
        recipient_identity: c.owner_identity,
        cmd: cmd.to_string(),
        payload: payload.to_string(),
        created_at: ctx.timestamp,
    });
}

/// The shared core behind [`client_command`] and its gateway twin `gw_client_command`:
/// dispatch with the sender already resolved to a guid.
pub(crate) fn apply_client_command(
    ctx: &ReducerContext,
    character_guid: u64,
    cmd: &str,
    payload: &str,
) -> Result<(), String> {
    dispatch(ctx, character_guid, cmd, payload);
    Ok(())
}

/// Command dispatch — one arm per command, handlers live next to their features.
fn dispatch(ctx: &ReducerContext, character_guid: u64, cmd: &str, payload: &str) {
    match cmd {
        // The 184 acceptance round-trip: `STC ping` → `STC pong` (payload echoed).
        "ping" => send(ctx, character_guid, "pong", payload),
        other => {
            match crate::GAME_CLIENT_COMMAND.and_then(|handler| (handler.parse)(other, payload)) {
                Some(Ok(command)) => {
                    let now = ctx.timestamp.to_micros_since_unix_epoch();
                    ctx.db
                        .game_party_command_intent()
                        .insert(PartyCommandIntent {
                            id: 0,
                            source_identity: ctx.database_identity(),
                            issuer_guid: character_guid,
                            command,
                            created_micros: now,
                            expires_micros: now.saturating_add(COMMAND_LIFETIME_MICROS),
                            result_reap_micros: i64::MAX,
                            claim_token: 0,
                            claim_until_micros: 0,
                            pending: true,
                            state: CommandIntentState::Pending,
                        });
                }
                Some(Err(outcome)) => send(
                    ctx,
                    character_guid,
                    "playerbots.order.result",
                    outcome.tag(),
                ),
                None => {
                    spacetimedb::log::info!(
                        "bridge: unknown client command {other:?} from {character_guid} (dropped)"
                    );
                }
            }
        }
    }
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
    let mut intent = table
        .id()
        .find(intent_id)
        .ok_or_else(|| "MissingIntent".to_string())?;
    if let CommandIntentState::Finished(existing) = &intent.state {
        return (*existing == outcome)
            .then_some(())
            .ok_or_else(|| "Intent already finished with another outcome".to_string());
    }
    let now = ctx.timestamp.to_micros_since_unix_epoch();
    if intent.claim_token != claim_token && now < intent.expires_micros {
        return Err("ClaimLost".to_string());
    }
    intent.state = CommandIntentState::Finished(outcome);
    intent.pending = false;
    intent.claim_until_micros = 0;
    intent.result_reap_micros = now.saturating_add(COMMAND_RESULT_WINDOW_MICROS);
    let issuer_guid = intent.issuer_guid;
    let response_id = intent.id;
    table.id().update(intent);
    send(
        ctx,
        issuer_guid,
        "playerbots.order.result",
        &format!("{response_id}|{}", outcome.tag()),
    );
    Ok(())
}

#[reducer]
#[allow(clippy::too_many_arguments)]
pub fn apply_admitted_party_command(
    ctx: &ReducerContext,
    source_identity: Identity,
    intent_id: u64,
    issuer_guid: u64,
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
        crate::GAME_CLIENT_COMMAND.map_or(CommandOutcome::Suppressed, |handler| {
            (handler.apply)(ctx, &command)
        })
    });
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

/// Drive Realm-core admission and target application on one private unsharded fixture, stopping
/// before source finalization so durable tests can model a Gateway loss at that boundary.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn playerbots_fixture_command_apply(
    ctx: &ReducerContext,
    intent_id: u64,
    claim_token: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
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
    let members: Vec<_> = ctx
        .db
        .game_group_member()
        .by_group()
        .filter(&member.group_id)
        .take(crate::group::GROUP_MAX_MEMBERS + 1)
        .map(|row| row.character_guid)
        .collect();
    if members.len() > crate::group::GROUP_MAX_MEMBERS {
        return Err(CommandOutcome::StalePartyMirror.tag().to_string());
    }
    crate::group::admit_party_command_authority(
        ctx,
        member.group_id,
        intent.issuer_guid,
        intent.command.bot_guid,
        intent.command.authority_member_guid,
    )?;
    let result = apply_admitted_party_command(
        ctx,
        intent.source_identity,
        intent.id,
        intent.issuer_guid,
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

/// Finish a private fixture command from the exact target receipt without applying it again.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn playerbots_fixture_command_finish(
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

/// Apply and finish one private unsharded fixture command through the production phase operations.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn playerbots_fixture_command_drive(
    ctx: &ReducerContext,
    intent_id: u64,
    claim_token: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    playerbots_fixture_command_apply(ctx, intent_id, claim_token)?;
    if ctx
        .db
        .game_party_command_receipt()
        .by_source_intent()
        .filter((ctx.database_identity(), intent_id))
        .next()
        .is_some()
    {
        playerbots_fixture_command_finish(ctx, intent_id, claim_token)?;
    }
    Ok(())
}

/// Release the oldest receipt owned by one private command fixture to exercise capacity recovery.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn playerbots_fixture_command_release_receipt(
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

/// Expire one still-pending private fixture command without changing its target receipt.
#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn playerbots_fixture_command_expire(
    ctx: &ReducerContext,
    intent_id: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let table = ctx.db.game_party_command_intent();
    let mut intent = fixture_command_intent(ctx, intent_id)?;
    if !matches!(intent.state, CommandIntentState::Pending) {
        return Err("fixture command is already terminal".to_string());
    }
    intent.expires_micros = 0;
    intent.claim_until_micros = 0;
    table.id().update(intent);
    Ok(())
}
