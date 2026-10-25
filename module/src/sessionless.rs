use lyracore_shared::group::GroupRefusal;
use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::game_world_entity;
use crate::transfer::game_bot_transfer_intent;

#[table(accessor = game_sessionless_action_consent)]
pub struct SessionlessActionConsent {
    #[primary_key]
    pub character_guid: u64,
    pub allowed: bool,
}

#[table(accessor = game_character_relocation)]
pub struct CharacterRelocation {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[unique]
    pub character_guid: u64,
    pub trigger_id: u32,
    pub controller_generation: u64,
    pub map_id: u32,
    pub instance_id: u64,
}

crate::character_owned!(delete, fn sweep_delete_game_character_relocation(ctx, character_guid) {
    ctx.db.game_character_relocation().character_guid().delete(character_guid);
});
crate::character_owned!(not_transported, fn sweep_transfer_game_character_relocation());

pub(crate) fn completed_relocation(
    ctx: &ReducerContext,
    character_guid: u64,
    trigger_id: u32,
    partition: (u32, u64),
    controller_generation: u64,
) -> Option<u64> {
    let receipt = ctx
        .db
        .game_character_relocation()
        .character_guid()
        .find(character_guid)?;
    let entity = crate::helpers::acting_entity_by_guid(ctx, character_guid)?;
    (receipt.trigger_id == trigger_id
        && receipt.controller_generation == controller_generation
        && (receipt.map_id, receipt.instance_id) == partition
        && (entity.map_id, entity.instance_id) == partition)
        .then_some(receipt.id)
}

// Call after body, consent, destination and Instance admission Gates pass.
pub(crate) fn relocate(
    ctx: &ReducerContext,
    character_guid: u64,
    trigger_id: u32,
    destination: crate::transfer::Destination,
    controller_generation: u64,
) -> u64 {
    if let Some(cast) = crate::spell::pending_cast(ctx, character_guid) {
        crate::spell::cancel_cast_attempt(ctx, character_guid, cast.scheduled_id);
    }
    let _ = crate::actor::stop_attack(ctx, character_guid);
    crate::actor::stop_movement(ctx, character_guid);
    crate::world::teleport_player(
        ctx,
        character_guid,
        destination.map_id,
        destination.instance_id,
        destination.x,
        destination.y,
        destination.z,
        destination.o,
    );
    let receipts = ctx.db.game_character_relocation();
    receipts.character_guid().delete(character_guid);
    receipts
        .insert(CharacterRelocation {
            id: 0,
            character_guid,
            trigger_id,
            controller_generation,
            map_id: destination.map_id,
            instance_id: destination.instance_id,
        })
        .id
}

crate::character_owned!(delete, fn sweep_delete_game_sessionless_action_consent(ctx, character_guid) {
    ctx.db.game_sessionless_action_consent().character_guid().delete(character_guid);
});

crate::character_owned!(transfer, fn sweep_transfer_game_sessionless_action_consent(ctx, character_guid, io) {
    table = game_sessionless_action_consent,
    primary_key = character_guid,
});

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
    ctx.db
        .game_character_relocation()
        .character_guid()
        .delete(character_guid);
}

pub(crate) fn action_gate(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), crate::actor::ActionRefusal> {
    use crate::actor::{ActionRefusal, ActionRefusalKind};
    crate::account_ownership::require_actor(
        ctx,
        crate::account_ownership::SessionActor {
            guid: character_guid,
            ownership: None,
        },
    )
    .map_err(|detail| ActionRefusal::new(ActionRefusalKind::CannotAct, detail))?;
    if ctx
        .db
        .game_bot_transfer_intent()
        .by_bot()
        .filter(character_guid)
        .next()
        .is_some()
    {
        return Err(ActionRefusal::new(
            ActionRefusalKind::TransferPending,
            "Character has a pending Transfer Intent",
        ));
    }
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

#[cfg_attr(not(has_packages), allow(dead_code))]
pub(crate) fn movement_gate(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), crate::actor::ActionRefusal> {
    use crate::actor::{ActionRefusal, ActionRefusalKind};
    action_gate(ctx, character_guid)?;
    if ctx
        .db
        .game_sessionless_action_consent()
        .character_guid()
        .find(character_guid)
        .is_some_and(|consent| !consent.allowed)
    {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            "session-less movement is disabled for this Character",
        ));
    }
    let me = crate::helpers::live_entity(ctx, character_guid)
        .map_err(|detail| ActionRefusal::new(ActionRefusalKind::MissingActor, detail))?;
    if me.dead
        || me.health == 0
        || crate::spell::pending_cast(ctx, character_guid).is_some()
        || crate::spell::is_self_movement_suppressed(ctx, character_guid)
    {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            "Character cannot continue self-directed movement",
        ));
    }
    Ok(())
}

pub(crate) fn transfer_authority_gate(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), crate::actor::ActionRefusal> {
    use crate::actor::{ActionRefusal, ActionRefusalKind};
    crate::account_ownership::require_actor(
        ctx,
        crate::account_ownership::SessionActor {
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
    if ctx
        .db
        .game_sessionless_action_consent()
        .character_guid()
        .find(character_guid)
        .is_some_and(|consent| !consent.allowed)
    {
        return Err(ActionRefusal::new(
            ActionRefusalKind::CannotAct,
            "session-less Transfer is disabled for this Character",
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

#[cfg(feature = "debug_reducers")]
#[reducer]
pub fn debug_admit_sessionless_action(
    ctx: &ReducerContext,
    character_guid: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    action_gate(ctx, character_guid).map_err(|refusal| format!("{:?}: {}", refusal.kind, refusal))
}
