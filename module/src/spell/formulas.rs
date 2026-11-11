use super::taxonomy::{INT_PER_SPELL_POWER, INT_SPELL_POWER_BASE};
use crate::Aura;

pub(crate) fn cost_gate_blocks(is_player: bool, power: u32, cost: u32) -> bool {
    is_player && power < cost
}

pub(crate) fn level_gate_blocks(is_player: bool, spell_level: u8, caster_level: u8) -> bool {
    is_player && spell_level > 0 && (caster_level as u32) < spell_level as u32
}

pub(crate) fn healed_value(health: u32, max_health: u32, basis: i32) -> u32 {
    let gain = basis.max(0) as u32;
    health.saturating_add(gain).min(max_health)
}

pub(crate) fn damaged_value(health: u32, basis: i32) -> u32 {
    health.saturating_sub(basis.max(0) as u32).max(1)
}

pub(crate) fn energized_value(power: u32, max_power: u32, basis: i32) -> u32 {
    let gain = basis.max(0) as u32;
    power.saturating_add(gain).min(max_power)
}

pub(crate) fn effect_amount(
    base_points: i32,
    die_sides: i32,
    per_level: f32,
    level: u32,
    spell_level: u8,
    max_level: u8,
    roll: u32,
) -> i32 {
    let floor = spell_level as u32;
    let capped = if max_level == 0 {
        level.max(floor)
    } else {
        level.clamp(floor, max_level as u32)
    };
    let levels_above = capped.saturating_sub(floor);
    let scaled = base_points + (per_level * levels_above as f32) as i32;
    let die = if die_sides > 0 {
        (roll % die_sides as u32) as i32
    } else {
        0
    };
    scaled + die
}

pub(crate) fn int_spell_power(effective_int: i32) -> i32 {
    let above = (effective_int - INT_SPELL_POWER_BASE).max(0);
    if INT_PER_SPELL_POWER <= 0 {
        0
    } else {
        above / INT_PER_SPELL_POWER
    }
}

pub(crate) fn compose_magnitude(base_points: i32, spell_power: i32, coeff_pct: i32) -> i32 {
    let sp = spell_power.max(0);
    base_points + sp * coeff_pct / 100
}

pub(crate) fn apply_spell_crit(dmg: i32, is_crit: bool) -> i32 {
    if is_crit {
        dmg * 3 / 2
    } else {
        dmg
    }
}

pub(crate) fn should_arm_spell_retaliation(
    caster_is_player: bool,
    target_is_player: bool,
    target_already_engaged: bool,
) -> bool {
    caster_is_player && !target_is_player && !target_already_engaged
}

// Damage logs carry one school index, so a multi-school mask uses its lowest bit.
pub(crate) fn school_index(school_mask: u8) -> u8 {
    if school_mask == 0 {
        0
    } else {
        school_mask.trailing_zeros() as u8
    }
}

pub(crate) fn dispel_category_matches(dispel_cat: u8, aura_cat: u8) -> bool {
    dispel_cat == 0 || dispel_cat == aura_cat
}

pub(crate) fn apply_spell_mod(base: u32, flat: i32, pct: i32) -> u32 {
    let v = base as i64 + flat as i64;
    let v = v * (100 + pct as i64) / 100;
    v.max(0) as u32
}

pub(crate) fn apply_damage_pct(dmg: u32, pct: i32) -> u32 {
    ((dmg as i64 * (100 + pct).max(0) as i64) / 100) as u32
}

pub(crate) fn rederive_pool(
    login_max: u32,
    base_attr: u32,
    bonus: i32,
    per_point: fn(u32) -> u32,
) -> u32 {
    let effective = (base_attr as i64 + bonus as i64).max(0) as u32;
    let from_base = per_point(base_attr);
    let from_eff = per_point(effective);
    // Signed arithmetic preserves a zero-bonus no-op even when login_max is below from_base.
    (i128::from(login_max) - i128::from(from_base) + i128::from(from_eff))
        .clamp(0, i128::from(u32::MAX)) as u32
}

pub(crate) fn add_flat_pool_bonus(pool: u32, bonus: i32) -> u32 {
    (i128::from(pool) + i128::from(bonus)).clamp(0, i128::from(u32::MAX)) as u32
}

pub(crate) fn absorb_draw(remaining: i32, incoming: u32) -> (u32, i32, u32) {
    let pool = remaining.max(0) as u32;
    let absorbed = pool.min(incoming);
    let pool_left = (pool - absorbed) as i32;
    let leftover = incoming - absorbed;
    (absorbed, pool_left, leftover)
}

// shields must be sorted by aura id, oldest first.
pub(crate) fn drain_shields(shields: Vec<Aura>, incoming: u32) -> (Vec<Aura>, Vec<u64>, u32) {
    let mut remaining_damage = incoming;
    let mut updated = Vec::new();
    let mut deleted = Vec::new();
    for mut shield in shields {
        if remaining_damage == 0 {
            break;
        }
        let (_absorbed, pool_left, leftover) =
            absorb_draw(shield.amount_remaining, remaining_damage);
        remaining_damage = leftover;
        if pool_left <= 0 {
            deleted.push(shield.id);
        } else {
            shield.amount_remaining = pool_left;
            updated.push(shield);
        }
    }
    (updated, deleted, remaining_damage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::spell::taxonomy::{
        A_ABSORB, A_MOD_RESISTANCE, P_NONE, P_SCHOOL_MASK, RESIST_ARMOR,
    };
    use spacetimedb::Timestamp;

    fn aura(amount: i32, stacks: u8) -> Aura {
        Aura {
            id: 0,
            target_guid: 0,
            caster_guid: 0,
            spell_id: 0,
            slot: 0,
            level: 0,
            flags: 0,
            applied_at: Timestamp::UNIX_EPOCH,
            expires_at: Timestamp::UNIX_EPOCH,
            effect_id: 0,
            eff_kind: A_MOD_RESISTANCE,
            amount,
            eff_p0: RESIST_ARMOR as i32,
            eff_p0_kind: P_SCHOOL_MASK,
            eff_p1: 0,
            period_ms: 0,
            amount_remaining: 0,
            stacks,
            next_tick_micros: 0,
            channel_target: 0,
            enters_combat: false,
            proc_flags: 0,
            proc_chance: 0,
            proc_ppm: 0.0,
            proc_ex: 0,
            proc_school_mask: 0,
            proc_family_name: 0,
            proc_family_flags: 0,
            proc_charges: 0,
            proc_icd_ms: 0,
            proc_ready_micros: 0,
        }
    }

    #[test]
    fn should_arm_spell_retaliation_gates_player_vs_unengaged_creature() {
        assert!(should_arm_spell_retaliation(true, false, false));
        assert!(!should_arm_spell_retaliation(true, false, true));
        assert!(!should_arm_spell_retaliation(true, true, false));
        assert!(!should_arm_spell_retaliation(false, false, false));
    }

    #[test]
    fn school_index_names_the_lowest_school_in_the_mask() {
        use super::school_index;
        assert_eq!(school_index(0), 0);
        assert_eq!(school_index(0x01), 0);
        assert_eq!(school_index(0x02), 1);
        assert_eq!(school_index(0x04), 2);
        assert_eq!(school_index(0x08), 3);
        assert_eq!(school_index(0x10), 4);
        assert_eq!(school_index(0x20), 5);
        assert_eq!(school_index(0x40), 6);
        assert_eq!(school_index(0x14), 2);
    }

    #[test]
    fn apply_spell_mod_folds_flat_then_pct_and_floors_at_zero() {
        use super::apply_spell_mod;
        assert_eq!(apply_spell_mod(3000, 0, 0), 3000);
        assert_eq!(apply_spell_mod(3000, -200, 0), 2800);
        assert_eq!(apply_spell_mod(3000, -500, 0), 2500);
        assert_eq!(apply_spell_mod(100, -200, 0), 0);
        assert_eq!(apply_spell_mod(1000, 0, -10), 900);
        assert_eq!(apply_spell_mod(1000, -100, -10), 810);
    }

    fn shield(id: u64, amount_remaining: i32) -> Aura {
        Aura {
            id,
            amount_remaining,
            eff_kind: A_ABSORB,
            eff_p0: 0,
            eff_p0_kind: P_NONE,
            ..aura(0, 1)
        }
    }

    fn ids_and_pools(shields: &[Aura]) -> Vec<(u64, i32)> {
        shields.iter().map(|a| (a.id, a.amount_remaining)).collect()
    }

    #[test]
    fn drain_shields_single_shield_partial_and_exact_deplete() {
        let (updated, deleted, remaining) = drain_shields(vec![shield(5, 100)], 20);
        assert_eq!(ids_and_pools(&updated), vec![(5, 80)]);
        assert!(deleted.is_empty());
        assert_eq!(remaining, 0);
        let (updated, deleted, remaining) = drain_shields(vec![shield(7, 20)], 20);
        assert!(updated.is_empty());
        assert_eq!(deleted, vec![7]);
        assert_eq!(remaining, 0);
        let (updated, deleted, remaining) = drain_shields(vec![shield(9, 5)], 20);
        assert!(updated.is_empty());
        assert_eq!(deleted, vec![9]);
        assert_eq!(remaining, 15);
    }

    #[test]
    fn drain_shields_oldest_first_with_early_stop() {
        let (updated, deleted, remaining) = drain_shields(vec![shield(1, 30), shield(2, 50)], 60);
        assert_eq!(ids_and_pools(&updated), vec![(2, 20)]);
        assert_eq!(deleted, vec![1]);
        assert_eq!(remaining, 0);
        let (updated, deleted, remaining) =
            drain_shields(vec![shield(1, 50), shield(2, 50), shield(3, 50)], 40);
        assert_eq!(ids_and_pools(&updated), vec![(1, 10)]);
        assert!(deleted.is_empty());
        assert_eq!(remaining, 0);
    }

    #[test]
    fn drain_shields_empty_list_passes_incoming_through() {
        let (updated, deleted, remaining) = drain_shields(vec![], 42);
        assert!(updated.is_empty());
        assert!(deleted.is_empty());
        assert_eq!(remaining, 42);
    }
}
