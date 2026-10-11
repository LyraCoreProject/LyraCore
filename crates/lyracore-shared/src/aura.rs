//! Pure folds over active aura values.

use crate::spell::taxonomy::*;

#[derive(Clone, Copy, Debug)]
pub struct AuraAmount {
    pub kind: u8,
    pub parameter: i32,
    pub amount: i32,
    pub stacks: u8,
}

/// Legacy zero-stack rows contribute once. Arithmetic follows the Module's signed aura fold.
pub fn stacked_amount(amount: i32, stacks: u8) -> i32 {
    amount * i32::from(stacks.max(1))
}

pub fn moves_sheet(kind: u8, parameter: i32) -> bool {
    (kind == A_MOD_STAT
        && [STAT_STR, STAT_AGI, STAT_STA, STAT_INT, STAT_SPI, STAT_ALL]
            .into_iter()
            .any(|stat| parameter == i32::from(stat)))
        || (kind == A_MOD_COMBAT
            && [COMBAT_ATTACK_POWER, COMBAT_CRIT]
                .into_iter()
                .any(|field| parameter == i32::from(field)))
}

pub fn speed_bonus(auras: impl IntoIterator<Item = AuraAmount>, speed_kind: u8) -> i32 {
    auras
        .into_iter()
        .filter(|a| a.kind == A_MOD_SPEED && a.parameter == i32::from(speed_kind))
        .map(|a| stacked_amount(a.amount, a.stacks))
        .sum()
}

/// Movement retains at least ten percent of its base speed.
pub fn move_speed_with_pct(base: f32, pct: i32) -> f32 {
    base * (100 + pct).max(10) as f32 / 100.0
}

/// Running combines movement auras with mounted speed only while a mount aura is active.
/// Unmounted always bonuses multiply; their product competes with the strongest not-stack bonus.
pub fn run_speed(base: f32, auras: impl IntoIterator<Item = AuraAmount>) -> f32 {
    let (mut movement, mut slow, mut mount, mut mounted) = (0, 0, 0, false);
    let (mut always, mut not_stack) = (1.0_f32, 1.0_f32);
    for aura in auras {
        mounted |= aura.kind == A_MOUNTED;
        if aura.kind == A_MOD_SPEED {
            if aura.parameter == i32::from(SPEED_MOVE) {
                let amount = stacked_amount(aura.amount, aura.stacks);
                if amount > 0 {
                    movement = movement.max(amount);
                } else {
                    slow += amount;
                }
            } else if aura.parameter == i32::from(SPEED_MOUNTED) {
                mount += stacked_amount(aura.amount, aura.stacks);
            } else if aura.parameter == i32::from(SPEED_MOVE_ALWAYS) {
                always *= (100 + stacked_amount(aura.amount, aura.stacks)) as f32 / 100.0;
            } else if aura.parameter == i32::from(SPEED_MOVE_NOT_STACK) {
                not_stack =
                    not_stack.max((100 + stacked_amount(aura.amount, aura.stacks)) as f32 / 100.0);
            }
        }
    }
    let bonus = if mounted { 1.0 } else { always.max(not_stack) };
    move_speed_with_pct(base, mounted_move_pct(movement + slow, mount, mounted)) * bonus
}

pub fn moves_run_speed(kind: u8, parameter: i32) -> bool {
    kind == A_MOD_SPEED
        && [SPEED_MOVE, SPEED_MOVE_ALWAYS, SPEED_MOVE_NOT_STACK]
            .into_iter()
            .any(|speed| parameter == i32::from(speed))
}

/// Swimming uses its strongest positive speed aura and retains the movement floor.
pub fn swim_speed(base: f32, auras: impl IntoIterator<Item = AuraAmount>) -> f32 {
    let (mut bonus, mut slow) = (0, 0);
    for aura in auras {
        if aura.kind != A_MOD_SPEED || aura.parameter != i32::from(SPEED_SWIM) {
            continue;
        }
        let amount = stacked_amount(aura.amount, aura.stacks);
        if amount > 0 {
            bonus = bonus.max(amount);
        } else {
            slow += amount;
        }
    }
    move_speed_with_pct(base, bonus + slow)
}

pub fn mounted_move_pct(movement: i32, mount: i32, mounted: bool) -> i32 {
    movement + if mounted { mount } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_changes_include_crit_and_only_valid_stats() {
        assert!(moves_sheet(A_MOD_COMBAT, i32::from(COMBAT_CRIT)));
        assert!(moves_sheet(A_MOD_COMBAT, i32::from(COMBAT_ATTACK_POWER)));
        assert!(!moves_sheet(A_MOD_COMBAT, i32::from(COMBAT_HIT)));
        assert!(moves_sheet(A_MOD_STAT, i32::from(STAT_ALL)));
        assert!(moves_sheet(A_MOD_STAT, i32::from(STAT_SPI)));
        assert!(!moves_sheet(A_MOD_STAT, 5));
        assert!(!moves_sheet(A_MOD_STAT, -1));
    }

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
        assert_eq!(speed_bonus(auras, SPEED_MOVE), -110);
        assert!((move_speed_with_pct(7.0, speed_bonus(auras, SPEED_MOVE)) - 0.7).abs() < 0.0001);
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
        let snare = AuraAmount {
            amount: -30,
            ..ghost
        };
        assert_eq!(run_speed(7.0, [ghost, wisp, snare]), 8.4);
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
