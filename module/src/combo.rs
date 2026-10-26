use spacetimedb::{table, ReducerContext, Table};

use crate::game_world_entity;

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

pub(crate) fn finisher_damage(per_point: i32, combo: u32) -> i32 {
    per_point.saturating_mul(combo as i32)
}

pub(crate) fn finisher_duration_ms(base_ms: u32, combo: u32) -> u32 {
    base_ms + combo.min(MAX_COMBO) * 3_000
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

    #[test]
    fn finisher_scales_with_points() {
        assert_eq!(super::finisher_damage(5, 3), 15);
        assert_eq!(super::finisher_damage(5, 0), 0);
    }

    #[test]
    fn finisher_duration_follows_the_vanilla_snd_curve() {
        assert_eq!(super::finisher_duration_ms(6000, 1), 9_000);
        assert_eq!(super::finisher_duration_ms(6000, 2), 12_000);
        assert_eq!(super::finisher_duration_ms(6000, 3), 15_000);
        assert_eq!(super::finisher_duration_ms(6000, 4), 18_000);
        assert_eq!(super::finisher_duration_ms(6000, 5), 21_000);
        assert_eq!(super::finisher_duration_ms(6000, 0), 6_000);
        assert_eq!(super::finisher_duration_ms(6000, 9), 21_000);
    }
}
