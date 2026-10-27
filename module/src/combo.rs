use spacetimedb::{table, ReducerContext, Table};

use crate::game_world_entity;
use crate::spell::{Spell, SpellEffect, A_PERIODIC_DAMAGE, E_FINISHER_DAMAGE};
use lyracore_shared::spell::taxonomy::{
    FAMILY_FLAG_DRUID_RIP_BITE, FAMILY_FLAG_ROGUE_EVISCERATE, FAMILY_FLAG_ROGUE_RUPTURE,
    SPELL_FAMILY_DRUID, SPELL_FAMILY_ROGUE,
};

pub const MAX_COMBO: u32 = 5;

#[table(accessor = game_combo_point, public, index(accessor = by_owner, btree(columns = [owner_guid])))]
pub struct ComboPoint {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub owner_guid: u64,
    pub target_guid: u64,
    pub points: u32,
}

pub(crate) fn capped_inc(points: u32) -> u32 {
    (points + 1).min(MAX_COMBO)
}

/// Adds the effect's per-point amount and 1.12 attack-power share for the points on `target_guid`.
pub(crate) fn effect_points(
    ctx: &ReducerContext,
    hdr: &Spell,
    e: &SpellEffect,
    caster_guid: u64,
    target_guid: u64,
    points: i32,
) -> i32 {
    let combo = combo_points(ctx, caster_guid, target_guid);
    if combo == 0 {
        return points;
    }
    let ap_pct = attack_power_pct(hdr.family_name, hdr.family_flags, e.kind, combo);
    let ap_share = if ap_pct == 0 {
        0
    } else {
        ctx.db
            .game_world_entity()
            .guid()
            .find(caster_guid)
            .map(|caster| crate::combat::melee_attack_power(ctx, &caster) * ap_pct / 100)
            .unwrap_or(0)
    };
    combo_amount(points, e.points_per_combo, combo).saturating_add(ap_share as i32)
}

fn combo_amount(points: i32, points_per_combo: f32, combo: u32) -> i32 {
    points.saturating_add((points_per_combo * combo as f32) as i32)
}

// vmangos `spell_rogue.cpp`, `spell_druid.cpp` and `Aura::CalculateDotDamage` for client 1.12.
fn attack_power_pct(family_name: u8, family_flags: u64, kind: u8, combo: u32) -> u32 {
    let has = |flag: u64| family_flags & flag != 0;
    match (family_name, kind) {
        (SPELL_FAMILY_ROGUE, E_FINISHER_DAMAGE) if has(FAMILY_FLAG_ROGUE_EVISCERATE) => 3 * combo,
        (SPELL_FAMILY_ROGUE, A_PERIODIC_DAMAGE) if has(FAMILY_FLAG_ROGUE_RUPTURE) => combo.min(3),
        (SPELL_FAMILY_DRUID, E_FINISHER_DAMAGE) if has(FAMILY_FLAG_DRUID_RIP_BITE) => 3 * combo,
        (SPELL_FAMILY_DRUID, A_PERIODIC_DAMAGE) if has(FAMILY_FLAG_DRUID_RIP_BITE) => combo.min(4),
        _ => 0,
    }
}

/// Grows a finisher's duration by a fifth of the gap to its maximum per combo point.
pub(crate) fn finisher_duration_ms(base_ms: u32, max_ms: u32, combo: u32) -> u32 {
    if base_ms == u32::MAX || max_ms <= base_ms {
        return base_ms;
    }
    base_ms + (max_ms - base_ms) * combo.min(MAX_COMBO) / MAX_COMBO
}

pub(crate) fn held(ctx: &ReducerContext, owner_guid: u64) -> Option<ComboPoint> {
    ctx.db
        .game_combo_point()
        .by_owner()
        .filter(&owner_guid)
        .next()
}

pub(crate) fn combo_points(ctx: &ReducerContext, owner_guid: u64, target_guid: u64) -> u32 {
    ctx.db
        .game_combo_point()
        .by_owner()
        .filter(&owner_guid)
        .find(|r| r.target_guid == target_guid)
        .map(|r| r.points)
        .unwrap_or(0)
}

pub(crate) fn held_points(ctx: &ReducerContext, owner_guid: u64) -> u32 {
    held(ctx, owner_guid).map(|r| r.points).unwrap_or(0)
}

/// Combo points sit on one live target; a point on another target restarts the count there.
pub(crate) fn add_combo_point(ctx: &ReducerContext, owner_guid: u64, target_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let (Some(owner), Some(target)) = (
        entities.guid().find(owner_guid),
        entities.guid().find(target_guid),
    ) else {
        return;
    };
    if target.dead {
        return;
    }
    let player_side = if target.is_player() {
        Some(target.guid)
    } else {
        entities
            .guid()
            .find(target.owner_guid)
            .filter(|side| side.is_player())
            .map(|side| side.guid)
    };
    // A builder that ends a Duel leaves the loser alive but no longer an enemy.
    if player_side.is_some_and(|side| {
        !crate::duel::active_opponents(ctx, owner_guid, side)
            && !crate::faction::is_hostile(ctx, owner.faction_template, target.faction_template)
    }) {
        return;
    }
    let points = capped_inc(combo_points(ctx, owner_guid, target_guid));
    delete_rows(ctx, owner_guid);
    ctx.db.game_combo_point().insert(ComboPoint {
        id: 0,
        owner_guid,
        target_guid,
        points,
    });
    crate::native::combo_points_changed(ctx, owner_guid, target_guid, points);
}

pub(crate) fn spend_combo(ctx: &ReducerContext, owner_guid: u64) {
    if let Some(target_guid) = delete_rows(ctx, owner_guid) {
        crate::native::combo_points_changed(ctx, owner_guid, target_guid, 0);
    }
}

/// Clears the combo points `guid` holds and every combo point placed on it.
pub(crate) fn forget(ctx: &ReducerContext, guid: u64) {
    let owners: Vec<u64> = ctx
        .db
        .game_combo_point()
        .iter()
        .filter(|r| r.owner_guid == guid || r.target_guid == guid)
        .map(|r| r.owner_guid)
        .collect();
    for owner_guid in owners {
        spend_combo(ctx, owner_guid);
    }
}

/// Clears `owner_guid`'s points when any sit on `side_guid` or on a unit it owns.
pub(crate) fn forget_side(ctx: &ReducerContext, owner_guid: u64, side_guid: u64) {
    let on_side = ctx
        .db
        .game_combo_point()
        .by_owner()
        .filter(&owner_guid)
        .any(|r| {
            r.target_guid == side_guid
                || ctx
                    .db
                    .game_world_entity()
                    .guid()
                    .find(r.target_guid)
                    .is_some_and(|target| target.owner_guid == side_guid)
        });
    if on_side {
        spend_combo(ctx, owner_guid);
    }
}

fn delete_rows(ctx: &ReducerContext, owner_guid: u64) -> Option<u64> {
    let combos = ctx.db.game_combo_point();
    let rows: Vec<ComboPoint> = combos.by_owner().filter(&owner_guid).collect();
    for row in &rows {
        combos.id().delete(row.id);
    }
    rows.first().map(|r| r.target_guid)
}

#[cfg(test)]
mod tests {
    use super::capped_inc;

    #[test]
    fn combo_caps_at_five() {
        let mut p = 0;
        for _ in 0..8 {
            p = capped_inc(p);
        }
        assert_eq!(p, super::MAX_COMBO);
    }

    // Eviscerate rank 1 tooltip: 6-10 at one point, 26-30 at five; the rolled base is 1-5.
    #[test]
    fn eviscerate_adds_five_damage_per_combo_point() {
        assert_eq!(super::combo_amount(1, 5.0, 1), 6);
        assert_eq!(super::combo_amount(5, 5.0, 1), 10);
        assert_eq!(super::combo_amount(1, 5.0, 5), 26);
        assert_eq!(super::combo_amount(5, 5.0, 5), 30);
    }

    // Expose Armor rank 5 removes 340 armor per point from a zero base.
    #[test]
    fn expose_armor_scales_its_reduction_by_combo_points() {
        assert_eq!(super::combo_amount(0, -340.0, 5), -1700);
    }

    #[test]
    fn attack_power_shares_follow_the_1_12_finishers() {
        use super::*;
        let rogue = |flags, kind, combo| attack_power_pct(SPELL_FAMILY_ROGUE, flags, kind, combo);
        let druid = |flags, kind, combo| attack_power_pct(SPELL_FAMILY_DRUID, flags, kind, combo);
        assert_eq!(rogue(0x0082_0000, E_FINISHER_DAMAGE, 5), 15);
        assert_eq!(rogue(FAMILY_FLAG_ROGUE_RUPTURE, A_PERIODIC_DAMAGE, 5), 3);
        assert_eq!(druid(FAMILY_FLAG_DRUID_RIP_BITE, A_PERIODIC_DAMAGE, 5), 4);
        assert_eq!(druid(FAMILY_FLAG_DRUID_RIP_BITE, E_FINISHER_DAMAGE, 5), 15);
        assert_eq!(rogue(0x0004_0000, E_FINISHER_DAMAGE, 5), 0);
    }

    #[test]
    fn slice_and_dice_lasts_nine_to_twenty_one_seconds() {
        let durations: Vec<u32> = (0..=6)
            .map(|combo| super::finisher_duration_ms(6_000, 21_000, combo))
            .collect();
        assert_eq!(
            durations,
            [6_000, 9_000, 12_000, 15_000, 18_000, 21_000, 21_000]
        );
    }

    #[test]
    fn kidney_shot_rank_one_stuns_one_second_per_combo_point() {
        assert_eq!(super::finisher_duration_ms(0, 5_000, 1), 1_000);
        assert_eq!(super::finisher_duration_ms(0, 5_000, 5), 5_000);
    }

    #[test]
    fn a_fixed_duration_ignores_combo_points() {
        assert_eq!(super::finisher_duration_ms(30_000, 30_000, 5), 30_000);
        assert_eq!(super::finisher_duration_ms(12_000, 0, 5), 12_000);
    }
}
