use spacetimedb::ReducerContext;

use crate::graveyard;
use crate::world::teleport;

use crate::{game_aura, game_world_entity, WorldEntity};
use lyracore_shared::constants::{npc_flags, player_flags, unit_vis_flags};

const GHOST: u32 = 8326;
const WISP_SPIRIT: u32 = 20584;
const NIGHT_ELF: u8 = 4;

pub(crate) fn spirit_healer_gate(
    ctx: &ReducerContext,
    ghost_guid: u64,
    healer_guid: u64,
) -> Result<(WorldEntity, WorldEntity), crate::Refusal> {
    let entities = ctx.db.game_world_entity();
    let ghost = entities
        .guid()
        .find(ghost_guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !ghost.dead || ghost.player_flags & player_flags::GHOST == 0 {
        return Err("caller is not a ghost".into());
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
        return Err("Spirit Healer on another map".into());
    }
    if crate::helpers::dist_sq(&ghost, &healer) > crate::npc_interaction::INTERACTION_RANGE_SQ {
        return Err("Spirit Healer out of range".into());
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
    if character.race() == NIGHT_ELF {
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
    crate::spell::shed_auras(ctx, auras);
    crate::spell::recompute_display(ctx, guid);
    crate::Corpse::delete(ctx, crate::corpse::corpse_guid_for(guid));
    crate::spell::clear_resurrect_request_and_option(ctx, guid);
}

pub(crate) fn do_repop(ctx: &ReducerContext, guid: u64) -> Result<(), crate::Refusal> {
    use lyracore_shared::constants::{player_flags, unit_vis_flags};
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !player.dead {
        return Ok(());
    }
    if player.player_flags & player_flags::GHOST != 0 {
        return Ok(());
    }

    let (corpse_bytes_1, corpse_bytes_2) = crate::corpse::corpse_appearance_bytes(
        player.unit_bytes_0,
        player.player_bytes,
        player.player_bytes_2,
    );
    let now_micros = crate::time::now(ctx).to_micros_since_unix_epoch();
    let (new_expire, delay_micros) =
        crate::corpse::escalated_reclaim(player.death_expire_micros, now_micros);

    let corpse_guid = crate::corpse::corpse_guid_for(player.guid);
    crate::Corpse::delete(ctx, corpse_guid);
    crate::Corpse {
        guid: corpse_guid,
        owner_guid: player.guid,
        map_id: player.map_id,
        x: player.x,
        y: player.y,
        z: player.z,
        orientation: player.orientation,
        display_id: player.native_display_id,
        bytes_1: corpse_bytes_1,
        bytes_2: corpse_bytes_2,
        created_at: crate::time::now(ctx),
        reclaim_delay_micros: delay_micros,
        is_bones: false,
        instance_id: player.instance_id,
        cell: 0,
    }
    .insert(ctx);

    player.health = 1;
    player.player_flags |= player_flags::GHOST;
    player.unit_bytes_1 |= unit_vis_flags::GHOST;
    player.death_expire_micros = new_expire;
    let player_guid = player.guid;
    let (death_x, death_y, death_map_id, death_race) =
        (player.x, player.y, player.map_id, player.race());
    player.update(ctx);
    restore(ctx, player_guid);

    crate::creatures::despawn_pets(ctx, player_guid);

    let gy = graveyard::resolve_graveyard(ctx, death_map_id, death_x, death_y, death_race);
    // vmangos never refuses a release; like a failed graveyard teleport there, the ghost stays put.
    if let Err(refusal) = teleport(ctx, player_guid, gy.map, 0, [gy.x, gy.y, gy.z, gy.o]) {
        spacetimedb::log::warn!(
            "repop: {player_guid} released at its corpse; graveyard teleport refused: {refusal}"
        );
    }

    crate::spell::clear_resurrect_request_and_option(ctx, player_guid);
    Ok(())
}

pub(crate) fn ghost_restored_fields(player_flags: u32, unit_bytes_1: u32) -> (bool, u32, u32, u32) {
    use lyracore_shared::constants::{player_flags as pf, unit_vis_flags};
    (
        true,
        1,
        player_flags | pf::GHOST,
        unit_bytes_1 | unit_vis_flags::GHOST,
    )
}

pub const RESURRECTION_SICKNESS_SPELL: u32 = 15007;

fn spirit_res_vitals(max_health: u32, max_power: u32) -> (u32, u32) {
    ((max_health / 2).max(1), max_power / 2)
}

// None below level 11; one minute per level above 10, up to the spell's ten minutes.
fn resurrection_sickness_micros(level: u32) -> Option<i64> {
    const MINUTE_MICROS: i64 = 60_000_000;
    (level >= 11).then(|| i64::from(level.min(20) - 10) * MINUTE_MICROS)
}

pub(crate) fn do_spirit_healer_res(ctx: &ReducerContext, guid: u64) -> Result<(), crate::Refusal> {
    use lyracore_shared::constants::player_flags;
    let entities = ctx.db.game_world_entity();
    let mut player = entities
        .guid()
        .find(guid)
        .ok_or_else(|| "caller not in world".to_string())?;
    if !player.dead || player.player_flags & player_flags::GHOST == 0 {
        return Err("caller is not a ghost".into());
    }
    let (health, power) = spirit_res_vitals(player.max_health, player.max_power);
    let player_level = player.level;
    player.health = health;
    player.power = power;
    player.update(ctx);
    finish_resurrection(ctx, guid);
    crate::items::apply_spirit_healer_durability_loss(ctx, guid);
    let Some(sickness_micros) = resurrection_sickness_micros(player_level) else {
        return Ok(());
    };
    crate::spell::apply_spell_auras(ctx, RESURRECTION_SICKNESS_SPELL, guid, 1, 1);
    let expires_at = spacetimedb::Timestamp::from_micros_since_unix_epoch(
        crate::time::now(ctx).to_micros_since_unix_epoch() + sickness_micros,
    );
    let sickness: Vec<_> = ctx
        .db
        .game_aura()
        .by_target()
        .filter(&guid)
        .filter(|aura| aura.spell_id == RESURRECTION_SICKNESS_SPELL)
        .collect();
    for mut aura in sickness {
        aura.expires_at = expires_at;
        aura.update(ctx);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ghost_restored_fields, resurrection_sickness_micros, spirit_res_vitals};

    #[test]
    fn ghost_restored_fields_reapply_exactly_the_release_state_over_the_fresh_build() {
        use lyracore_shared::constants::{player_flags, unit_vis_flags};
        let (dead, health, pf, ub1) = ghost_restored_fields(0, 17u32 << 16);
        assert!(dead, "a preserved ghost rebuilds dead");
        assert_eq!(
            health, 1,
            "ghost HP is 1 (vanilla), matching do_repop's release"
        );
        assert_eq!(pf, player_flags::GHOST, "the GHOST player flag is set");
        assert_eq!(
            ub1,
            (17u32 << 16) | unit_vis_flags::GHOST,
            "the ghost render bit ORs over the stance byte"
        );
        let (_, _, pf2, ub2) = ghost_restored_fields(pf, ub1);
        assert_eq!((pf2, ub2), (pf, ub1));
    }

    #[test]
    fn spirit_healer_res_restores_half_vitals() {
        assert_eq!(spirit_res_vitals(200, 80), (100, 40));
        assert_eq!(spirit_res_vitals(199, 81), (99, 40));
        assert_eq!(spirit_res_vitals(1, 0), (1, 0));
        assert_eq!(
            spirit_res_vitals(0, 0),
            (1, 0),
            "the .max(1) floor keeps a 0-max res alive"
        );
    }

    #[test]
    fn resurrection_sickness_lasts_a_minute_per_level_above_ten_up_to_ten_minutes() {
        const MINUTE: i64 = 60_000_000;
        assert_eq!(resurrection_sickness_micros(10), None);
        assert_eq!(resurrection_sickness_micros(11), Some(MINUTE));
        assert_eq!(resurrection_sickness_micros(15), Some(5 * MINUTE));
        assert_eq!(resurrection_sickness_micros(20), Some(10 * MINUTE));
        assert_eq!(resurrection_sickness_micros(60), Some(10 * MINUTE));
    }
}
