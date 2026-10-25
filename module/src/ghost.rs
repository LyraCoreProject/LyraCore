use spacetimedb::ReducerContext;

use crate::{game_aura, game_world_entity, WorldEntity};
use lyracore_shared::constants::{npc_flags, player_flags, unit_vis_flags};

const GHOST: u32 = 8326;
const WISP_SPIRIT: u32 = 20584;
const SPIRIT_HEALER_RANGE_SQ: f32 = 100.0;

pub(crate) fn spirit_healer_gate(
    ctx: &ReducerContext,
    ghost_guid: u64,
    healer_guid: u64,
) -> Result<(WorldEntity, WorldEntity), String> {
    let entities = ctx.db.game_world_entity();
    let ghost = entities
        .guid()
        .find(ghost_guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !ghost.dead || ghost.player_flags & player_flags::GHOST == 0 {
        return Err("caller is not a ghost".to_string());
    }
    let healer = entities
        .guid()
        .find(healer_guid)
        .filter(|healer| {
            !healer.is_player()
                && !healer.dead
                && healer.health != 0
                && healer.npc_flags & npc_flags::SPIRITHEALER != 0
        })
        .ok_or_else(|| "target is not a Spirit Healer".to_string())?;
    if !crate::helpers::in_same_partition(&healer, ghost.map_id, ghost.instance_id) {
        return Err("Spirit Healer on another map".to_string());
    }
    if crate::helpers::dist_sq(&ghost, &healer) > SPIRIT_HEALER_RANGE_SQ {
        return Err("Spirit Healer out of range".to_string());
    }
    Ok((ghost, healer))
}

pub(crate) fn is_ghost_spell(spell_id: u32) -> bool {
    matches!(spell_id, GHOST | WISP_SPIRIT)
}

pub(crate) fn restore(ctx: &ReducerContext, guid: u64) {
    let Some(character) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    if !character.dead || character.player_flags & player_flags::GHOST == 0 {
        return;
    }
    crate::spell::apply_spell_auras(ctx, GHOST, guid, character.level as u8, 1);
    if character.race() == 4 {
        crate::spell::apply_spell_auras(ctx, WISP_SPIRIT, guid, character.level as u8, 1);
    }
    crate::spell::recompute_display(ctx, guid);
}

pub(crate) fn finish_resurrection(ctx: &ReducerContext, guid: u64) {
    if let Some(mut character) = ctx.db.game_world_entity().guid().find(guid) {
        character.dead = false;
        character.player_flags &= !player_flags::GHOST;
        character.unit_bytes_1 &= !unit_vis_flags::GHOST;
        character.update(ctx);
    }
    let auras = ctx
        .db
        .game_aura()
        .by_target()
        .filter(&guid)
        .filter(|a| is_ghost_spell(a.spell_id))
        .collect();
    crate::spell::shed_auras(ctx, guid, auras);
    crate::spell::recompute_display(ctx, guid);
    crate::Corpse::delete(ctx, crate::corpse::corpse_guid_for(guid));
    crate::spell::clear_resurrect_request_and_option(ctx, guid);
}
