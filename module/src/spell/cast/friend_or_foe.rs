use lyracore_shared::spell::taxonomy::E_CAST_FRIEND_OR_FOE;
use spacetimedb::{log, ReducerContext};

use super::resolve::{resolve_trigger_cast, CastRequest};
use crate::{game_world_entity, SpellEffect, WorldEntity};

// vmangos IsFriendlyTo, taken as the caster itself or a unit the caster may not harm.
fn is_friend(ctx: &ReducerContext, caster: &WorldEntity, target: &WorldEntity) -> bool {
    caster.guid == target.guid || !crate::combat::may_harm(ctx, caster, target)
}

/// Casts the effect's friend spell at a friend and its foe spell (`p0`) at any other target.
pub(super) fn apply(
    ctx: &ReducerContext,
    e: &SpellEffect,
    caster_guid: u64,
    target_guid: u64,
    level: u8,
) {
    let entities = ctx.db.game_world_entity();
    let (Some(caster), Some(target)) = (
        entities.guid().find(caster_guid),
        entities.guid().find(target_guid),
    ) else {
        return;
    };
    let spell_id = if is_friend(ctx, &caster, &target) {
        e.trigger_spell
    } else {
        e.p0 as u32
    };
    if spell_id == 0 {
        return;
    }
    if let Err(err) = resolve_trigger_cast(
        ctx,
        &CastRequest::new(caster_guid, spell_id, level, target_guid),
    ) {
        log::info!(
            "spell {} cast {spell_id} at {target_guid}, which did not resolve: {err}",
            e.spell_id
        );
    }
}

/// vmangos PaladinHolyShockScript::OnCheckCast: only a target that is not a friend must be in front.
pub(super) fn needs_facing(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    target: &WorldEntity,
    effects: &[SpellEffect],
) -> bool {
    effects.iter().any(|e| e.kind == E_CAST_FRIEND_OR_FOE) && !is_friend(ctx, caster, target)
}
