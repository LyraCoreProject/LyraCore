use super::tables::AttackTable;

pub(super) struct Skills {
    pub weapon: i32,
    pub maximum: i32,
    pub defense: i32,
    pub defense_maximum: i32,
    pub target_level: u32,
    pub target_is_character: bool,
}

pub(super) fn attack_table(
    skills: Skills,
    mut base: AttackTable,
    hit_bonus: i32,
    can_glance: bool,
) -> AttackTable {
    let full = skills.weapon - skills.defense;
    let difference = skills.weapon - skills.defense_maximum;
    let capped = skills.weapon.min(skills.maximum) - skills.defense_maximum;
    let low_level = if !skills.target_is_character && skills.target_level < 10 {
        skills.target_level as f32 / 10.0
    } else {
        1.0
    };
    let avoidance_per_skill = if skills.target_is_character { 4 } else { 10 };
    let miss_per_skill = if skills.target_is_character {
        4
    } else if full < -10 {
        20
    } else {
        10
    };
    let hit_bonus = if full < -10 && hit_bonus > 0 {
        hit_bonus - 100
    } else {
        hit_bonus
    };
    base.miss = (((base.miss as i32 - full * miss_per_skill) as f32 * low_level) - hit_bonus as f32)
        .clamp(0.0, 6000.0) as u32;
    base.dodge =
        ((base.dodge as i32 - difference * avoidance_per_skill) as f32 * low_level).max(0.0) as u32;
    if base.parry > 0 {
        let parry_bonus = if skills.target_is_character {
            difference * 4
        } else {
            capped * if capped < -10 { 60 } else { 20 }
        };
        base.parry = ((base.parry as i32 - parry_bonus) as f32 * low_level).max(0.0) as u32;
    }
    base.glancing = if can_glance {
        (1000 + (skills.defense - skills.weapon.min(skills.maximum)) * 200).clamp(0, 4000) as u32
    } else {
        0
    };
    if base.block > 0 {
        let block = base.block as i32 - difference * avoidance_per_skill;
        let block = if skills.target_is_character {
            block
        } else {
            block.min(500)
        };
        base.block = (block as f32 * low_level).max(0.0) as u32;
    }
    let crit_bonus = if skills.target_is_character || full > 0 {
        full * 4
    } else {
        (skills.weapon.min(skills.maximum) - skills.defense) * 20
    };
    base.crit = (base.crit as i32 + crit_bonus).max(0) as u32;
    base
}

pub(super) fn sheet_crit_bp(
    agility: u32,
    class: u8,
    rate: Option<f32>,
    weapon: i32,
    maximum: i32,
    bonus: i32,
) -> u32 {
    let offset = match class {
        2 => 70,
        5 => 300,
        7 => 170,
        8 => 320,
        9 => 200,
        11 => 90,
        _ => 0,
    };
    let agility = rate
        .filter(|rate| rate.is_finite() && *rate > 0.0)
        .map_or(0.0, |rate| agility as f32 / rate * 100.0);
    (agility + (offset + bonus + (weapon - maximum) * 4) as f32).clamp(0.0, 10_000.0) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::tables::{attack_outcome, HIT_BLOCK, HIT_CRIT, HIT_GLANCING, HIT_NORMAL};

    fn creature(weapon: i32, maximum: i32, defense: i32, level: u32, hit: i32) -> AttackTable {
        attack_table(
            Skills {
                weapon,
                maximum,
                defense,
                defense_maximum: level as i32 * 5,
                target_level: level,
                target_is_character: false,
            },
            AttackTable {
                miss: 500,
                dodge: 500,
                parry: 500,
                glancing: 0,
                block: 500,
                crit: 500,
                crushing: 0,
            },
            hit,
            true,
        )
    }

    #[test]
    fn white_melee_intervals_follow_skill_caps_and_low_level_rules() {
        let equal = creature(300, 300, 300, 60, 0);
        let boss = creature(300, 300, 315, 63, 0);
        assert_eq!(
            [
                equal.miss,
                equal.dodge,
                equal.parry,
                equal.glancing,
                equal.block,
                equal.crit
            ],
            [500, 500, 500, 1000, 500, 500]
        );
        assert_eq!(
            [
                boss.miss,
                boss.dodge,
                boss.parry,
                boss.glancing,
                boss.block,
                boss.crit
            ],
            [800, 650, 1400, 4000, 500, 200]
        );
        assert_eq!(attack_outcome(1500, &equal), HIT_GLANCING);
        assert_eq!(attack_outcome(2500, &equal), HIT_BLOCK);
        assert_eq!(attack_outcome(3000, &equal), HIT_CRIT);
        assert_eq!(attack_outcome(3500, &equal), HIT_NORMAL);
        let trained = creature(305, 300, 315, 63, 100);
        assert_eq!(
            [trained.miss, trained.parry, trained.glancing, trained.crit],
            [500, 1400, 4000, 200]
        );
        let low = creature(25, 25, 25, 5, 0);
        assert_eq!(
            [low.miss, low.dodge, low.parry, low.glancing, low.block],
            [250, 250, 250, 1000, 250]
        );
        let over = creature(310, 300, 300, 60, 0);
        assert_eq!(
            [
                over.miss,
                over.dodge,
                over.parry,
                over.glancing,
                over.block,
                over.crit
            ],
            [400, 400, 500, 1000, 400, 540]
        );
    }

    #[test]
    fn character_defenders_use_four_basis_points_per_skill_and_never_glance() {
        let table = attack_table(
            Skills {
                weapon: 315,
                maximum: 315,
                defense: 300,
                defense_maximum: 300,
                target_level: 60,
                target_is_character: true,
            },
            AttackTable {
                miss: 500,
                dodge: 500,
                parry: 500,
                glancing: 0,
                block: 500,
                crit: 500,
                crushing: 0,
            },
            0,
            false,
        );
        assert_eq!(
            [
                table.miss,
                table.dodge,
                table.parry,
                table.block,
                table.crit,
                table.glancing
            ],
            [440, 440, 440, 440, 560, 0]
        );
    }

    #[test]
    fn crit_uses_class_rates_offsets_and_actual_weapon_skill() {
        assert_eq!(sheet_crit_bp(100, 1, Some(20.0), 300, 300, 0), 500);
        assert_eq!(sheet_crit_bp(20, 1, Some(4.0), 5, 5, 0), 500);
        assert_eq!(sheet_crit_bp(100, 4, Some(28.9855), 300, 300, 0), 345);
        assert_eq!(sheet_crit_bp(100, 8, Some(19.4553), 300, 300, 0), 833);
        assert_eq!(sheet_crit_bp(100, 1, Some(20.0), 295, 300, 0), 480);
        assert_eq!(sheet_crit_bp(100, 1, Some(20.0), 305, 300, 0), 520);
        assert_eq!(sheet_crit_bp(100, 1, None, 300, 300, 0), 0);
    }
}
