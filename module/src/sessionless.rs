//! Package-owned consent for core actions performed without a Session.

use lyracore_shared::group::GroupRefusal;
use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::game_world_entity;

/// Absent consent preserves legacy behavior. A Package updates this row when selecting its controller.
/// [entity]
#[table(accessor = game_sessionless_action_consent)]
pub struct SessionlessActionConsent {
    #[primary_key]
    pub character_guid: u64,
    pub allowed: bool,
}

crate::character_owned!(delete, fn sweep_delete_game_sessionless_action_consent(ctx, character_guid) {
    ctx.db.game_sessionless_action_consent().character_guid().delete(character_guid);
});

crate::character_owned!(transfer, fn sweep_transfer_game_sessionless_action_consent(ctx, character_guid, io) {
    table = game_sessionless_action_consent,
    primary_key = character_guid,
});

/// Every selection, including the same value, clears unclaimed Group Intents in this transaction.
/// An action admitted before this transaction may still finish on Realm-core afterwards.
#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn set_sessionless_action_consent(
    ctx: &ReducerContext,
    character_guid: u64,
    allowed: bool,
) {
    let consent = ctx.db.game_sessionless_action_consent();
    let row = SessionlessActionConsent {
        character_guid,
        allowed,
    };
    if consent.character_guid().find(character_guid).is_some() {
        consent.character_guid().update(row);
    } else {
        consent.insert(row);
    }
    crate::group::clear_unclaimed_group_intents(ctx, character_guid);
}

/// Check current ownership before Package gameplay. A missing body is allowed so Legacy can
/// restore it; group admission additionally requires a live entity and controller consent.
pub(crate) fn action_gate(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), crate::actor::ActionRefusal> {
    use crate::actor::{ActionRefusal, ActionRefusalKind};
    crate::account_ownership::require_actor(
        ctx,
        crate::SessionActor {
            guid: character_guid,
            ownership: None,
        },
    )
    .map_err(|detail| ActionRefusal::new(ActionRefusalKind::CannotAct, detail))?;
    let character = crate::helpers::character_by_guid(ctx, character_guid).ok_or_else(|| {
        ActionRefusal::new(ActionRefusalKind::MissingActor, "Character unavailable")
    })?;
    if character.online {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            "Character has a World Session",
        ));
    }
    Ok(())
}

pub(crate) fn group_action_gate(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), GroupRefusal> {
    action_gate(ctx, character_guid).map_err(|_| GroupRefusal::ActorUnavailable)?;
    if ctx
        .db
        .game_world_entity()
        .guid()
        .find(character_guid)
        .is_none()
    {
        return Err(GroupRefusal::ActorUnavailable);
    }
    if ctx
        .db
        .game_sessionless_action_consent()
        .character_guid()
        .find(character_guid)
        .is_some_and(|consent| !consent.allowed)
    {
        return Err(GroupRefusal::ActionSuppressed);
    }
    Ok(())
}

/// Admit one automatic group action against current World Shard state. Completion acknowledges
/// this transaction only. Realm-core membership commits separately and can follow a later selection.
#[reducer]
pub fn admit_sessionless_group_action(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    group_action_gate(ctx, character_guid)
        .map_err(|refusal| crate::group::refused(refusal, "session-less group admission"))
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_set_sessionless_action_consent(
    ctx: &ReducerContext,
    character_guid: u64,
    allowed: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::helpers::require_character(ctx, character_guid)?;
    set_sessionless_action_consent(ctx, character_guid, allowed);
    Ok(())
}

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_emit_sessionless_group_intent(
    ctx: &ReducerContext,
    character_guid: u64,
    target_guid: u64,
    leave: bool,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if leave {
        crate::group::emit_bot_leave_intent(ctx, character_guid);
    } else {
        crate::group::emit_bot_invite_intent(ctx, character_guid, target_guid);
    }
    Ok(())
}
