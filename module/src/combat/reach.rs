use spacetimedb::ReducerContext;

use crate::creatures::catalogue::{game_display_combat_reach, DisplayCombatReach};
use crate::WorldEntity;

// vmangos DEFAULT_COMBAT_REACH, BASE_MELEERANGE_OFFSET, ATTACK_DISTANCE and LEEWAY_BONUS_RANGE.
pub(crate) const DEFAULT_COMBAT_REACH_YD: f32 = 1.5;
const BASE_MELEERANGE_OFFSET_YD: f32 = 4.0 / 3.0;
pub(crate) const MELEE_REACH_FLOOR_YD: f32 = 5.0;
const LEEWAY_BONUS_YD: f32 = 2.66;
// UNIT_DEFAULT_MELEE_Z_LIMIT; vmangos compares it with the squared height difference.
const MELEE_Z_LIMIT_SQ: f32 = 36.0;
// Deliberate Deviation `moving_character_reach_floor`, conformance/features/combat/auto_attack.toml
const CHARACTER_PURSUIT_REACH_YD: f32 = 9.0;
const LEEWAY_WINDOW_MS: u32 = 1200;

/// One side of a Melee Reach test.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct MeleeBody {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub combat_reach: f32,
    pub is_character: bool,
    pub moving: bool,
}

impl MeleeBody {
    pub(crate) fn of(ctx: &ReducerContext, unit: &WorldEntity, now_ms: u32) -> Self {
        Self {
            x: unit.x,
            y: unit.y,
            z: unit.z,
            combat_reach: combat_reach(ctx, unit),
            is_character: unit.is_player(),
            moving: moved_within_leeway_window(unit.last_move_ms, now_ms),
        }
    }
}

pub(crate) fn moved_within_leeway_window(last_move_ms: u32, now_ms: u32) -> bool {
    now_ms.wrapping_sub(last_move_ms) < LEEWAY_WINDOW_MS
}

/// The display's reach scaled by the unit's scale over the display's (`Unit::UpdateModelData`).
pub(crate) fn combat_reach(ctx: &ReducerContext, unit: &WorldEntity) -> f32 {
    let reaches = ctx.db.game_display_combat_reach();
    let Some(display) = reaches.display_id().find(unit.display_id) else {
        return DEFAULT_COMBAT_REACH_YD;
    };
    let native_scale = reaches
        .display_id()
        .find(unit.native_display_id)
        .map(|native| native.display_scale);
    scaled_reach(&display, native_scale, unit)
}

fn scaled_reach(
    display: &DisplayCombatReach,
    native_display_scale: Option<f32>,
    unit: &WorldEntity,
) -> f32 {
    // vmangos `Unit::InitPlayerDisplayIds` gives a Character its native display's scale, and a Form
    // or transform multiplies it; `scale_x` holds only that multiplier for a Character.
    let unit_scale = if unit.is_player() {
        native_display_scale.unwrap_or(1.0) * unit.scale_x
    } else if unit.scale_x > 0.0 {
        unit.scale_x
    } else {
        display.display_scale
    };
    display.combat_reach * unit_scale / display.display_scale
}

/// The 2D distance inside which `attacker`'s melee reaches `target`.
pub(crate) fn melee_reach_yd(attacker: &MeleeBody, target: &MeleeBody) -> f32 {
    let reach = (attacker.combat_reach.max(DEFAULT_COMBAT_REACH_YD)
        + target.combat_reach.max(DEFAULT_COMBAT_REACH_YD)
        + BASE_MELEERANGE_OFFSET_YD)
        .max(MELEE_REACH_FLOOR_YD);
    let leeway = (attacker.is_character || target.is_character) && attacker.moving && target.moving;
    match (leeway, attacker.is_character && !target.is_character) {
        (false, _) => reach,
        (true, false) => reach + LEEWAY_BONUS_YD,
        (true, true) => (reach + LEEWAY_BONUS_YD).max(CHARACTER_PURSUIT_REACH_YD),
    }
}

/// A flat test under 6 yd of height, as vmangos `Unit::CanReachWithMeleeAutoAttackAtPosition`.
pub(crate) fn in_melee_reach(attacker: &MeleeBody, target: &MeleeBody) -> bool {
    let (dx, dy, dz) = (
        attacker.x - target.x,
        attacker.y - target.y,
        attacker.z - target.z,
    );
    let reach = melee_reach_yd(attacker, target);
    dx * dx + dy * dy < reach * reach && dz * dz < MELEE_Z_LIMIT_SQ
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creature(x: f32, combat_reach: f32) -> MeleeBody {
        MeleeBody {
            x,
            y: 0.0,
            z: 0.0,
            combat_reach,
            is_character: false,
            moving: false,
        }
    }

    fn character(x: f32) -> MeleeBody {
        MeleeBody {
            is_character: true,
            ..creature(x, 1.5)
        }
    }

    fn moving(body: MeleeBody) -> MeleeBody {
        MeleeBody {
            moving: true,
            ..body
        }
    }

    fn reach_row(combat_reach: f32, display_scale: f32) -> DisplayCombatReach {
        DisplayCombatReach {
            display_id: 1,
            combat_reach,
            display_scale,
        }
    }

    #[test]
    fn a_character_in_a_form_reaches_by_the_form_scale_over_the_form_models_scale() {
        let mut cat = crate::helpers::tests::entity(1, 0, 0, 0, 0);
        cat.type_mask = lyracore_shared::constants::type_mask::PLAYER_BIT;
        let cat_model = reach_row(2.0, 1.0);
        assert_eq!(scaled_reach(&cat_model, Some(1.25), &cat), 2.5, "no Form");
        cat.scale_x = 0.8;
        let reach = scaled_reach(&cat_model, Some(1.25), &cat);
        assert!(
            (reach - 2.0).abs() < 1e-6,
            "Cat Form scales the native scale: {reach}"
        );
    }

    #[test]
    fn a_normal_pair_reaches_five_yards() {
        let reach = melee_reach_yd(&character(0.0), &creature(0.0, 1.5));
        assert_eq!(reach, 5.0, "1.5 + 1.5 + 4/3 yd is under the 5 yd floor");
        assert!(in_melee_reach(&character(0.0), &creature(4.9, 1.5)));
        assert!(!in_melee_reach(&character(0.0), &creature(5.1, 1.5)));
    }

    #[test]
    fn a_large_creature_reaches_past_five_yards() {
        // vmangos creature_display_info_addon display 589 has a combat reach of 10 yd.
        let large = creature(0.0, 10.0);
        let reach = melee_reach_yd(&large, &character(12.0));
        assert!((reach - (10.0 + 1.5 + 4.0 / 3.0)).abs() < 1e-5, "{reach}");
        assert!(in_melee_reach(&large, &character(12.5)));
        assert!(in_melee_reach(&character(12.5), &large));
        assert!(!in_melee_reach(&large, &character(13.0)));
    }

    #[test]
    fn a_reach_under_the_default_counts_as_the_default() {
        assert_eq!(
            melee_reach_yd(&creature(0.0, 0.0), &creature(0.0, 4.0)),
            1.5 + 4.0 + 4.0 / 3.0
        );
    }

    #[test]
    fn the_test_is_flat_under_a_six_yard_height_limit() {
        let attacker = character(0.0);
        let above = |z| MeleeBody {
            z,
            ..creature(3.0, 1.5)
        };
        assert!(in_melee_reach(&attacker, &above(5.9)));
        assert!(!in_melee_reach(&attacker, &above(6.0)));
    }

    #[test]
    fn leeway_needs_both_units_moving_and_a_character_in_the_pair() {
        let cases = [
            ("both still", character(0.0), creature(0.0, 1.5), 5.0),
            ("one still", moving(character(0.0)), creature(0.0, 1.5), 5.0),
            (
                "creature at a moving Character",
                moving(creature(0.0, 1.5)),
                moving(character(0.0)),
                5.0 + 2.66,
            ),
            (
                "two creatures",
                moving(creature(0.0, 1.5)),
                moving(creature(0.0, 1.5)),
                5.0,
            ),
            (
                "two moving Characters",
                moving(character(0.0)),
                moving(character(0.0)),
                5.0 + 2.66,
            ),
        ];
        for (case, attacker, target, reach) in cases {
            assert_eq!(melee_reach_yd(&attacker, &target), reach, "{case}");
        }
    }

    #[test]
    fn a_moving_character_covers_the_post_flee_tick_gap_for_a_pinned_chaser() {
        use crate::creatures::MOVE_TICK_SECS;
        use lyracore_shared::constants::speeds::RUN;
        let worst_case_gap = 5.0 + RUN * MOVE_TICK_SECS;
        let reach = melee_reach_yd(&moving(character(0.0)), &moving(creature(0.0, 1.5)));
        assert!(
            reach >= worst_case_gap,
            "reach {reach} yd must cover the worst-case post-flee-tick gap {worst_case_gap} yd"
        );
        let large = melee_reach_yd(&moving(character(0.0)), &moving(creature(0.0, 10.0)));
        assert_eq!(large, 10.0 + 1.5 + 4.0 / 3.0 + 2.66);
    }
}
