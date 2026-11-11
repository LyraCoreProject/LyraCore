use lyracore_shared::spell::taxonomy::{
    A_CONTROL, A_MOD_SPEED, A_MOUNTED, M_POLY, M_ROOT, M_STUN, SPEED_MOUNTED, SPEED_MOVE,
    SPEED_MOVE_ALWAYS, SPEED_MOVE_NOT_STACK, SPEED_SWIM,
};
use spacetimedb::ReducerContext;

use super::{aura_amounts, AuraAmount};

pub(crate) fn speed_bonus(ctx: &ReducerContext, unit_guid: u64, speed_kind: u8) -> i32 {
    speed_total(aura_amounts(ctx, unit_guid), speed_kind)
}

fn speed_total(auras: impl IntoIterator<Item = AuraAmount>, speed_kind: u8) -> i32 {
    auras
        .into_iter()
        .filter(|a| a.kind == A_MOD_SPEED && a.parameter == i32::from(speed_kind))
        .map(AuraAmount::total)
        .sum()
}

pub(crate) fn effective_move_speed(ctx: &ReducerContext, guid: u64, base: f32) -> f32 {
    run_speed(base, aura_amounts(ctx, guid))
}

pub(crate) fn move_speed_with_pct(base: f32, pct: i32) -> f32 {
    base * (100 + pct).max(10) as f32 / 100.0
}

pub(crate) fn run_speed(base: f32, auras: impl IntoIterator<Item = AuraAmount>) -> f32 {
    let (mut movement, mut slow, mut mount, mut mounted) = (0, 0, 0, false);
    let (mut always, mut not_stack) = (1.0_f32, 1.0_f32);
    for aura in auras {
        mounted |= aura.kind == A_MOUNTED;
        if aura.kind == A_MOD_SPEED {
            if aura.parameter == i32::from(SPEED_MOVE) {
                let amount = aura.total();
                if amount > 0 {
                    movement = movement.max(amount);
                } else {
                    slow += amount;
                }
            } else if aura.parameter == i32::from(SPEED_MOUNTED) {
                mount += aura.total();
            } else if aura.parameter == i32::from(SPEED_MOVE_ALWAYS) {
                always *= (100 + aura.total()) as f32 / 100.0;
            } else if aura.parameter == i32::from(SPEED_MOVE_NOT_STACK) {
                not_stack = not_stack.max((100 + aura.total()) as f32 / 100.0);
            }
        }
    }
    let bonus = if mounted { 1.0 } else { always.max(not_stack) };
    move_speed_with_pct(base, mounted_move_pct(movement + slow, mount, mounted)) * bonus
}

/// Stun, root and polymorph hold a unit in place. Fear moves it, so it is absent here.
pub(crate) fn holds_in_place(kind: u8, parameter: i32) -> bool {
    kind == A_CONTROL && matches!(parameter, M_STUN | M_ROOT | M_POLY)
}

pub(crate) fn moves_run_speed(kind: u8, parameter: i32) -> bool {
    kind == A_MOD_SPEED
        && [SPEED_MOVE, SPEED_MOVE_ALWAYS, SPEED_MOVE_NOT_STACK]
            .into_iter()
            .any(|speed| parameter == i32::from(speed))
}

pub(crate) fn swim_speed(base: f32, auras: impl IntoIterator<Item = AuraAmount>) -> f32 {
    let (mut bonus, mut slow) = (0, 0);
    for aura in auras {
        if aura.kind != A_MOD_SPEED || aura.parameter != i32::from(SPEED_SWIM) {
            continue;
        }
        let amount = aura.total();
        if amount > 0 {
            bonus = bonus.max(amount);
        } else {
            slow += amount;
        }
    }
    move_speed_with_pct(base, bonus + slow)
}

fn mounted_move_pct(movement: i32, mount: i32, mounted: bool) -> i32 {
    movement + if mounted { mount } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::spell::taxonomy::{SPEED_CAST, SPEED_SWING};

    #[test]
    fn stacked_snare_reaches_the_movement_floor() {
        let auras = [
            AuraAmount {
                kind: A_MOD_SPEED,
                parameter: i32::from(SPEED_MOVE),
                amount: -30,
                stacks: 4,
            },
            AuraAmount {
                kind: A_MOD_SPEED,
                parameter: i32::from(SPEED_MOVE),
                amount: 10,
                stacks: 0,
            },
            AuraAmount {
                kind: A_MOD_SPEED,
                parameter: i32::from(SPEED_SWING),
                amount: 50,
                stacks: 1,
            },
        ];
        assert_eq!(speed_total(auras, SPEED_MOVE), -110);
        assert!((move_speed_with_pct(7.0, speed_total(auras, SPEED_MOVE)) - 0.7).abs() < 0.0001);
    }

    #[test]
    fn move_speed_pct_floors_at_ten_percent() {
        assert_eq!(move_speed_with_pct(7.0, 0), 7.0);
        assert!((move_speed_with_pct(7.0, -40) - 4.2).abs() < 1e-4);
        assert!(move_speed_with_pct(7.0, 50) > 7.0);
        assert!((move_speed_with_pct(7.0, -95) - 0.7).abs() < 1e-4);
        assert!((move_speed_with_pct(7.0, -200) - 0.7).abs() < 1e-4);
    }

    #[test]
    fn mounted_speed_folds_only_while_mounted() {
        assert_eq!(mounted_move_pct(0, 60, true), 60);
        assert_eq!(mounted_move_pct(0, 100, true), 100);
        assert_eq!(mounted_move_pct(0, 60, false), 0);
        assert_eq!(mounted_move_pct(-20, 60, true), 40);
    }

    #[test]
    fn dismounting_keeps_an_ordinary_move_speed_buff_instead_of_forcing_bare_base() {
        assert_eq!(mounted_move_pct(20, 0, false), 20);
        assert_eq!(mounted_move_pct(20, 60, false), 20);
    }

    #[test]
    fn nominal_60_and_100_percent_mount_tiers_yield_the_exact_run_speed() {
        assert!((move_speed_with_pct(7.0, mounted_move_pct(0, 60, true)) - 11.2).abs() < 1e-4);
        assert!((move_speed_with_pct(7.0, mounted_move_pct(0, 100, true)) - 14.0).abs() < 1e-4);
    }

    #[test]
    fn running_applies_mount_speed_only_while_mounted() {
        let movement = AuraAmount {
            kind: A_MOD_SPEED,
            parameter: i32::from(SPEED_MOVE),
            amount: -30,
            stacks: 1,
        };
        let mount = AuraAmount {
            kind: A_MOD_SPEED,
            parameter: i32::from(SPEED_MOUNTED),
            amount: 60,
            stacks: 1,
        };
        let mounted = AuraAmount {
            kind: A_MOUNTED,
            parameter: 1147,
            amount: 0,
            stacks: 1,
        };
        assert_eq!(run_speed(7.0, [movement, mount, mounted]), 9.1);
        assert_eq!(run_speed(7.0, [mount, mounted]), 11.2);
        assert_eq!(run_speed(7.0, [movement, mount]), 4.9);
    }

    #[test]
    fn ghost_speed_uses_the_strongest_positive_aura() {
        let ghost = AuraAmount {
            kind: A_MOD_SPEED,
            parameter: i32::from(SPEED_MOVE),
            amount: 25,
            stacks: 1,
        };
        let wisp = AuraAmount {
            amount: 50,
            ..ghost
        };
        assert_eq!(run_speed(7.0, [ghost]), 8.75);
        assert_eq!(run_speed(7.0, [ghost, wisp]), 10.5);
        assert_eq!(run_speed(7.0, [wisp, ghost]), 10.5);
    }

    #[test]
    fn running_multiplies_always_bonuses_with_the_strongest_move_bonus() {
        let movement = AuraAmount {
            kind: A_MOD_SPEED,
            parameter: i32::from(SPEED_MOVE),
            amount: 50,
            stacks: 1,
        };
        let always = AuraAmount {
            parameter: i32::from(SPEED_MOVE_ALWAYS),
            amount: 10,
            ..movement
        };
        let weaker = AuraAmount {
            amount: 25,
            ..movement
        };
        assert!((run_speed(7.0, [movement, weaker, always, always]) - 12.705).abs() < 0.00001);
    }

    #[test]
    fn running_chooses_between_always_product_and_strongest_not_stack_bonus() {
        let always = AuraAmount {
            kind: A_MOD_SPEED,
            parameter: i32::from(SPEED_MOVE_ALWAYS),
            amount: 10,
            stacks: 1,
        };
        let not_stack = AuraAmount {
            parameter: i32::from(SPEED_MOVE_NOT_STACK),
            amount: 20,
            ..always
        };
        let stronger = AuraAmount {
            amount: 30,
            ..not_stack
        };
        assert!((run_speed(7.0, [always, always, not_stack]) - 8.47).abs() < 0.00001);
        assert!((run_speed(7.0, [always, always, not_stack, stronger]) - 9.1).abs() < 0.00001);
    }

    #[test]
    fn swimming_uses_its_own_strongest_bonus_and_speed_floor() {
        let ghost = AuraAmount {
            kind: A_MOD_SPEED,
            parameter: i32::from(SPEED_SWIM),
            amount: 25,
            stacks: 1,
        };
        let wisp = AuraAmount {
            amount: 50,
            ..ghost
        };
        let run = AuraAmount {
            parameter: i32::from(SPEED_MOVE),
            amount: 100,
            ..ghost
        };
        let cast = AuraAmount {
            parameter: i32::from(SPEED_CAST),
            amount: 100,
            ..ghost
        };
        assert_eq!(swim_speed(4.0, [ghost]), 5.0);
        assert_eq!(swim_speed(4.0, [ghost, wisp, run, cast]), 6.0);
        assert_eq!(swim_speed(4.0, [run, cast]), 4.0);
        let snare = AuraAmount {
            amount: -30,
            stacks: 5,
            ..ghost
        };
        assert_eq!(swim_speed(4.0, [wisp, snare]), 0.4);
    }
}
