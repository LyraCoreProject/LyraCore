use spacetimedb::ReducerContext;

use crate::game_aura;
use crate::game_player_skill;
use crate::game_skill_ability;
use crate::game_spell_effect;
use crate::game_world_entity;
use crate::spell::{
    CastGateRefusal, CastRefusalReason, SpellEffect, A_MOD_SPEED, A_MOUNTED, SPEED_MOUNTED,
};
use crate::WorldEntity;

pub(crate) fn mount_aura_moves_mount(eff_kind: u8, eff_p0: i32) -> bool {
    eff_kind == A_MOUNTED || (eff_kind == A_MOD_SPEED && eff_p0 == SPEED_MOUNTED as i32)
}

pub(crate) fn projected_mount_display(effects: impl IntoIterator<Item = (u8, i32)>) -> u32 {
    effects
        .into_iter()
        .find(|(kind, _)| *kind == A_MOUNTED)
        .map(|(_, p0)| p0.max(0) as u32)
        .unwrap_or(0)
}

// The aura set owns mounted state; mount_display_id is only its projection.
pub(crate) fn active_mount_spell(ctx: &ReducerContext, guid: u64) -> Option<u32> {
    ctx.db
        .game_aura()
        .by_target()
        .filter(&guid)
        .filter(|a| a.eff_kind == A_MOUNTED)
        .min_by_key(|a| a.id)
        .map(|a| a.spell_id)
}

pub(crate) fn recompute_mount(ctx: &ReducerContext, guid: u64) {
    if crate::taxi::is_in_flight(ctx, guid) {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let Some(mut e) = entities.guid().find(guid) else {
        return;
    };
    let display = projected_mount_display(
        ctx.db
            .game_aura()
            .by_target()
            .filter(&guid)
            .map(|a| (a.eff_kind, a.eff_p0)),
    );
    let speed_bp = (crate::combat::effective_move_speed(ctx, guid, 1.0) * 10_000.0).round() as u32;
    if e.mount_display_id == display && e.run_speed_mult_bp == speed_bp {
        return;
    }
    e.mount_display_id = display;
    e.run_speed_mult_bp = speed_bp;
    e.update(ctx);
}

pub(crate) fn dismount(ctx: &ReducerContext, guid: u64) {
    if crate::taxi::is_in_flight(ctx, guid) {
        return;
    }
    let Some(spell_id) = active_mount_spell(ctx, guid) else {
        return;
    };
    let auras = ctx.db.game_aura();
    let ids: Vec<u64> = auras
        .by_target()
        .filter(&guid)
        .filter(|a| a.spell_id == spell_id)
        .map(|a| a.id)
        .collect();
    for id in ids {
        crate::Aura::delete(ctx, id);
    }
    recompute_mount(ctx, guid);
}

pub(crate) fn restore_mount_on_rebuild(ctx: &ReducerContext, guid: u64) {
    if active_mount_spell(ctx, guid).is_some() {
        recompute_mount(ctx, guid);
    }
}

pub(crate) fn spell_is_mount(ctx: &ReducerContext, spell_id: u32) -> bool {
    ctx.db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .any(|e| e.kind == A_MOUNTED)
}

fn mask_admits(mask: u32, id: u8) -> bool {
    mask == 0 || ((1..=32).contains(&id) && mask & (1 << (id - 1)) != 0)
}

fn rank_satisfies(rank: u16, min_skill: i32) -> bool {
    i32::from(rank) >= min_skill.max(0)
}

fn trained_for_spell(ctx: &ReducerContext, guid: u64, spell_id: u32, race: u8, class: u8) -> bool {
    let abilities = ctx.db.game_skill_ability();
    ctx.db
        .game_player_skill()
        .by_character()
        .filter(&guid)
        .any(|learned| {
            abilities
                .by_skill_line()
                .filter(&learned.skill_line)
                .any(|a| {
                    a.spell_id == spell_id
                        && mask_admits(a.race_mask, race)
                        && mask_admits(a.class_mask, class)
                        && rank_satisfies(learned.current, a.min_skill)
                })
        })
}

pub(crate) fn indoor_check_is_due(old_move_ms: u32, move_time_ms: u32) -> bool {
    move_time_ms / 100 != old_move_ms / 100
}

pub(crate) fn mount_cast_allowed_here(
    ctx: &ReducerContext,
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
) -> bool {
    !crate::vmap::is_indoor(ctx, map_id, x, y, z)
}

fn is_submerged_here(ctx: &ReducerContext, entity: &WorldEntity) -> bool {
    let level = crate::terrain::liquid_level_at(ctx, entity.map_id, entity.x, entity.y);
    lyracore_shared::env::is_submerged(
        entity.z,
        level.unwrap_or_default(),
        level.is_some(),
        entity.movement_flags,
    )
}

pub(crate) fn check_mount_cast(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    effects: &[SpellEffect],
    spell_id: u32,
) -> Result<(), CastGateRefusal> {
    if !effects.iter().any(|e| e.kind == A_MOUNTED) || !caster.is_player() {
        return Ok(());
    }
    if !trained_for_spell(ctx, caster.guid, spell_id, caster.race(), caster.class()) {
        return Err(format!("spell {spell_id} requires riding training").into());
    }
    if caster.dead || caster.player_flags & lyracore_shared::constants::player_flags::GHOST != 0 {
        return Err(CastGateRefusal::other(
            CastRefusalReason::MountWhileDead,
            "you cannot mount while dead",
        ));
    }
    if crate::combat::is_engaged(ctx, caster.guid) {
        return Err(CastGateRefusal::other(
            CastRefusalReason::MountInCombat,
            "you cannot mount while in combat",
        ));
    }
    if !mount_cast_allowed_here(ctx, caster.map_id, caster.x, caster.y, caster.z) {
        return Err(CastGateRefusal::other(
            CastRefusalReason::MountIndoors,
            "you cannot mount indoors",
        ));
    }
    if is_submerged_here(ctx, caster) {
        return Err(CastGateRefusal::other(
            CastRefusalReason::MountInDeepWater,
            "you cannot mount in deep water",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::{
        A_MOD_SPEED, A_MOD_STAT, A_MOUNTED, A_PERIODIC_DAMAGE, SPEED_MOUNTED, SPEED_MOVE,
        SPEED_SWING,
    };

    #[test]
    fn mount_aura_moves_mount_covers_the_mount_pair_only() {
        assert!(mount_aura_moves_mount(A_MOUNTED, 0));
        assert!(mount_aura_moves_mount(A_MOUNTED, 1147));
        assert!(mount_aura_moves_mount(A_MOD_SPEED, SPEED_MOUNTED as i32));

        assert!(!mount_aura_moves_mount(A_MOD_SPEED, SPEED_MOVE as i32));
        assert!(!mount_aura_moves_mount(A_MOD_SPEED, SPEED_SWING as i32));
        assert!(!mount_aura_moves_mount(A_MOD_STAT, SPEED_MOUNTED as i32));
        assert!(!mount_aura_moves_mount(A_PERIODIC_DAMAGE, 0));
    }

    #[test]
    fn projected_mount_display_reads_the_mount_aura_only() {
        assert_eq!(projected_mount_display([]), 0);
        assert_eq!(
            projected_mount_display([(A_MOD_STAT, 5), (A_PERIODIC_DAMAGE, 12)]),
            0
        );
        assert_eq!(
            projected_mount_display([(A_MOD_SPEED, SPEED_MOUNTED as i32), (A_MOUNTED, 1147)]),
            1147
        );
        assert_eq!(projected_mount_display([(A_MOUNTED, -3)]), 0);
    }

    #[test]
    fn projected_mount_display_is_deterministic_with_a_stray_second_mount() {
        assert_eq!(
            projected_mount_display([(A_MOUNTED, 1147), (A_MOUNTED, 2404)]),
            1147
        );
    }

    #[test]
    fn rank_satisfies_is_inclusive_at_the_vanilla_riding_tiers() {
        assert!(!rank_satisfies(74, 75));
        assert!(rank_satisfies(75, 75));
        assert!(rank_satisfies(150, 75));

        assert!(!rank_satisfies(149, 150));
        assert!(rank_satisfies(150, 150));

        assert!(rank_satisfies(0, 0));
        assert!(rank_satisfies(0, -1));
    }

    #[test]
    fn mask_admits_treats_race_and_class_ids_as_one_based_bits() {
        assert!(mask_admits(0, 1));
        assert!(mask_admits(0, 11));

        let human = 1u32 << 0;
        let orc = 1u32 << 1;
        assert!(mask_admits(human, 1));
        assert!(!mask_admits(human, 2));
        assert!(mask_admits(human | orc, 2));

        assert!(!mask_admits(human, 0));
        assert!(!mask_admits(u32::MAX, 33));
    }

    #[test]
    fn indoor_check_is_due_only_on_a_100ms_boundary() {
        assert!(!indoor_check_is_due(0, 0));
        assert!(!indoor_check_is_due(1_000, 1_099));
        assert!(indoor_check_is_due(1_099, 1_100));
        assert!(!indoor_check_is_due(1_100, 1_199));
        assert!(indoor_check_is_due(1_100, 5_000));
        assert!(indoor_check_is_due(5_000, 1_100));
    }
}
