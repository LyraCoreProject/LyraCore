use spacetimedb::ReducerContext;

use crate::game_world_entity;

use super::*;

pub(crate) fn can_afford(power: u32, cost: u32) -> bool {
    power >= cost
}

pub(crate) fn cost_gate_blocks(is_player: bool, power: u32, cost: u32) -> bool {
    is_player && !can_afford(power, cost)
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

pub(crate) const SPELL_CRIT_BP: u32 = 0;

// Low-level approximation; 100 and 5 are tuning constants rather than an exact vanilla curve.
pub(crate) fn intellect_crit_bp(intellect: u32, level: u32) -> u32 {
    intellect * 100 / (level + 5)
}

pub(crate) fn apply_spell_crit(dmg: i32, is_crit: bool) -> i32 {
    if is_crit {
        dmg * 3 / 2
    } else {
        dmg
    }
}

pub(crate) fn spell_crit_bp_with_bonus(bonus: i32) -> u32 {
    (SPELL_CRIT_BP as i32 + bonus).clamp(0, 10_000) as u32
}

pub(crate) fn spell_miss_chance_bp(attacker_level: u32, target_level: u32) -> u32 {
    let diff = target_level.saturating_sub(attacker_level);
    let bp = match diff {
        0 => 400,
        1 => 500,
        2 => 600,
        d => 600 + (d - 2) * 1100,
    };
    bp.min(9900)
}

pub(crate) fn spell_crit_bp(ctx: &ReducerContext, caster_guid: u64) -> u32 {
    let level = ctx
        .db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .map(|c| c.level)
        .unwrap_or(0);
    let int_crit =
        intellect_crit_bp(effective_intellect(ctx, caster_guid).max(0) as u32, level) as i32;
    spell_crit_bp_with_bonus(int_crit + combat_field_bonus(ctx, caster_guid, COMBAT_CRIT))
}

pub(crate) fn is_on_cooldown(now_micros: i64, ready_micros: i64) -> bool {
    now_micros < ready_micros
}

pub(crate) fn is_ready_at(now_micros: i64, ready_micros: i64) -> bool {
    now_micros >= ready_micros
}

pub(crate) fn school_locked(now_micros: i64, until_micros: i64) -> bool {
    until_micros > now_micros
}

pub(crate) fn distance_3d(ax: f32, ay: f32, az: f32, bx: f32, by: f32, bz: f32) -> f32 {
    let (dx, dy, dz) = (ax - bx, ay - by, az - bz);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

pub(crate) fn projectile_travel_ms(distance_yd: f32, speed_yps: f32) -> u32 {
    if speed_yps <= 0.0 || distance_yd <= 0.0 {
        return 0;
    }
    ((distance_yd / speed_yps) * 1000.0).round().max(0.0) as u32
}

// Orientation is radians from +X, increasing counterclockwise toward +Y.
pub(crate) fn is_behind(ax: f32, ay: f32, tx: f32, ty: f32, target_orientation: f32) -> bool {
    let (dx, dy) = (ax - tx, ay - ty);
    if dx == 0.0 && dy == 0.0 {
        return false;
    }
    // The dot product keeps the side boundary symmetric under f32 rounding.
    dx * target_orientation.cos() + dy * target_orientation.sin() <= 0.0
}

// Uses a 180-degree facing arc; vanilla uses roughly 120 degrees.
pub(crate) fn is_facing(ax: f32, ay: f32, attacker_orientation: f32, tx: f32, ty: f32) -> bool {
    let (dx, dy) = (tx - ax, ty - ay);
    if dx == 0.0 && dy == 0.0 {
        return true;
    }
    dx * attacker_orientation.cos() + dy * attacker_orientation.sin() > 0.0
}

#[cfg(test)]
pub(crate) fn cast_finish_micros(now_micros: i64, cast_time_ms: u32) -> i64 {
    now_micros + (cast_time_ms as i64) * 1000
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

pub(crate) fn apply_resistance(
    ctx: &ReducerContext,
    target_guid: u64,
    caster_guid: u64,
    school_mask: u8,
    dmg: i32,
) -> i32 {
    let caster_level = ctx
        .db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .map(|c| c.level)
        .unwrap_or(0);
    resist_magic_damage(ctx, target_guid, school_mask, caster_level, dmg)
}

pub(crate) fn resist_magic_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    school_mask: u8,
    level: u32,
    dmg: i32,
) -> i32 {
    if dmg <= 0 {
        return dmg;
    }
    let magic_mask = school_mask & !RESIST_ARMOR;
    if magic_mask == 0 {
        return dmg;
    }
    let resistance = resistance_bonus(ctx, target_guid, magic_mask).max(0) as u32;
    if resistance == 0 {
        return dmg;
    }
    let pct = crate::combat::resist_mitigation_pct(resistance, level);
    apply_resist_pct(dmg, pct)
}

pub(crate) fn apply_resist_pct(dmg: i32, pct: u32) -> i32 {
    (dmg - dmg * pct as i32 / 100).max(1)
}

pub(crate) fn dispel_category_matches(dispel_cat: u8, aura_cat: u8) -> bool {
    dispel_cat == 0 || dispel_cat == aura_cat
}

pub(crate) fn seal_amount(ctx: &ReducerContext, unit_guid: u64) -> i32 {
    sum_active_auras(ctx, unit_guid, |a| a.eff_kind == A_SEAL)
}

pub(crate) fn seal_amount_and_spell(ctx: &ReducerContext, unit_guid: u64) -> (i32, u32) {
    let spell = auras_on(ctx, unit_guid)
        .find(|a| a.eff_kind == A_SEAL)
        .map(|a| a.spell_id)
        .unwrap_or(0);
    (seal_amount(ctx, unit_guid), spell)
}

pub(crate) fn remove_seal_auras(ctx: &ReducerContext, unit_guid: u64) -> u32 {
    let auras = ctx.db.game_aura();
    let seal_spell_ids: std::collections::HashSet<u32> = auras
        .by_target()
        .filter(&unit_guid)
        .filter(|a| a.eff_kind == A_SEAL)
        .map(|a| a.spell_id)
        .collect();
    let ids: Vec<u64> = auras
        .by_target()
        .filter(&unit_guid)
        .filter(|a| seal_spell_ids.contains(&a.spell_id))
        .map(|a| a.id)
        .collect();
    let removed = ids.len() as u32;
    for id in ids {
        crate::Aura::delete(ctx, id);
    }
    removed
}

pub(crate) fn is_stealthed(ctx: &ReducerContext, unit_guid: u64) -> bool {
    ctx.db
        .game_aura()
        .by_target()
        .filter(&unit_guid)
        .any(|a| a.eff_kind == A_STEALTH)
}

pub(crate) fn break_stealth(ctx: &ReducerContext, unit_guid: u64) -> u32 {
    let auras = ctx.db.game_aura();
    let ids: Vec<u64> = auras
        .by_target()
        .filter(&unit_guid)
        .filter(|a| a.eff_kind == A_STEALTH)
        .map(|a| a.id)
        .collect();
    let removed = ids.len() as u32;
    for id in ids {
        crate::Aura::delete(ctx, id);
    }
    removed
}

pub(crate) fn spell_mod(
    ctx: &ReducerContext,
    caster_guid: u64,
    op: i32,
    hdr: &Spell,
) -> (i32, i32) {
    if hdr.family_name == 0 || hdr.family_flags == 0 {
        return (0, 0);
    }
    let spells = ctx.db.game_spell();
    let family_ok = |a: &Aura| {
        a.eff_p0 == op
            && spells.spell_id().find(a.spell_id).is_some_and(|src| {
                src.family_name == hdr.family_name
                    && hdr.family_flags & (a.eff_p1 as u32 as u64) != 0
            })
    };
    let flat = sum_active_auras(ctx, caster_guid, |a| {
        a.eff_kind == A_SPELLMOD_FLAT && family_ok(a)
    });
    let pct = sum_active_auras(ctx, caster_guid, |a| {
        a.eff_kind == A_SPELLMOD_PCT && family_ok(a)
    });
    (flat, pct)
}

pub(crate) fn apply_spell_mod(base: u32, flat: i32, pct: i32) -> u32 {
    let v = base as i64 + flat as i64;
    let v = v * (100 + pct as i64) / 100;
    v.max(0) as u32
}

pub(crate) fn combat_field_bonus(ctx: &ReducerContext, unit_guid: u64, field: u8) -> i32 {
    sum_active_auras(ctx, unit_guid, |a| {
        a.eff_kind == A_MOD_COMBAT && a.eff_p0 == field as i32
    })
}

pub(crate) fn combat_health_regen_pct(ctx: &ReducerContext, unit_guid: u64) -> i32 {
    sum_active_auras(ctx, unit_guid, |a| a.eff_kind == A_COMBAT_HEALTH_REGEN_PCT)
}

pub(crate) fn power_regen_pct(ctx: &ReducerContext, unit_guid: u64, power_type: u8) -> u32 {
    compound_regen_pct(
        auras_on(ctx, unit_guid)
            .filter(|a| a.eff_kind == A_MOD_POWER_REGEN_PCT && a.eff_p0 == i32::from(power_type))
            .map(|a| stacked_amount(&a)),
    )
}

// Each aura multiplies the rate, as cmangos `GetTotalAuraMultiplierByMiscValue` does.
fn compound_regen_pct(amounts: impl Iterator<Item = i32>) -> u32 {
    amounts
        .fold(100u64, |pct, amount| {
            pct.saturating_mul((100 + i64::from(amount)).max(0) as u64) / 100
        })
        .min(u64::from(u32::MAX)) as u32
}

pub(crate) fn detect_range_mod(ctx: &ReducerContext, unit_guid: u64) -> f32 {
    sum_active_auras(ctx, unit_guid, |a| a.eff_kind == A_MOD_DETECT_RANGE) as f32
}

pub(crate) fn damage_taken_bonus(ctx: &ReducerContext, target_guid: u64) -> i32 {
    let aura_pct = sum_active_auras(ctx, target_guid, |a| a.eff_kind == A_MOD_DAMAGE_TAKEN);
    let stance_pct = ctx
        .db
        .game_world_entity()
        .guid()
        .find(target_guid)
        .map(|t| stance_damage_taken_pct(t.stance))
        .unwrap_or(0);
    aura_pct + stance_pct
}

pub(crate) fn apply_damage_pct(dmg: u32, pct: i32) -> u32 {
    ((dmg as i64 * (100 + pct).max(0) as i64) / 100) as u32
}

pub(crate) fn stance_allows(mask: u8, stance: u8) -> bool {
    mask == 0 || (stance < 8 && (mask & (1u8 << stance)) != 0)
}

// Client build 5875 expects a ShapeshiftForm id in UNIT_FIELD_BYTES_1 byte 2.
pub(crate) fn client_form_for_stance(stance: u8) -> u32 {
    match stance {
        STANCE_BEAR => 5,
        STANCE_CAT => 1,
        STANCE_DIRE_BEAR => 8,
        _ => stance as u32 + 17,
    }
}

pub(crate) fn stance_switch_clears_power(old_stance: u8, new_stance: u8, power_type: u8) -> bool {
    old_stance != new_stance && power_type == lyracore_shared::packing::power_type::RAGE
}

pub(crate) fn form_recast_toggles_off(current: u8, recast: u8) -> bool {
    current == recast && recast >= STANCE_BEAR
}

pub(crate) fn stance_damage_taken_pct(stance: u8) -> i32 {
    if stance == STANCE_DEFENSIVE {
        STANCE_DEFENSIVE_DR_PCT
    } else {
        0
    }
}

pub(crate) fn stance_damage_done_pct(stance: u8) -> i32 {
    if stance == STANCE_DEFENSIVE {
        STANCE_DEFENSIVE_DMG_DONE_PCT
    } else {
        0
    }
}

pub(crate) fn stance_threat_pct(stance: u8) -> i32 {
    if stance == STANCE_DEFENSIVE {
        STANCE_DEFENSIVE_THREAT_PCT
    } else {
        0
    }
}

pub(crate) fn speed_bonus(ctx: &ReducerContext, unit_guid: u64, speed_kind: u8) -> i32 {
    lyracore_shared::aura::speed_bonus(
        auras_on(ctx, unit_guid).map(|a| lyracore_shared::aura::AuraAmount {
            kind: a.eff_kind,
            parameter: a.eff_p0,
            amount: a.amount,
            stacks: a.stacks,
        }),
        speed_kind,
    )
}

pub(crate) fn auras_on(ctx: &ReducerContext, unit_guid: u64) -> impl Iterator<Item = Aura> {
    ctx.db.game_aura().by_target().filter(unit_guid)
}

// Legacy zero-stack rows count once.
fn stacked_amount(a: &Aura) -> i32 {
    lyracore_shared::aura::stacked_amount(a.amount, a.stacks)
}

fn sum_active_auras(ctx: &ReducerContext, unit_guid: u64, pred: impl Fn(&Aura) -> bool) -> i32 {
    auras_on(ctx, unit_guid)
        .filter(|a| pred(a))
        .map(|a| stacked_amount(&a))
        .sum()
}

pub(crate) fn spell_power(ctx: &ReducerContext, caster_guid: u64, school_mask: u8) -> i32 {
    int_spell_power(effective_intellect(ctx, caster_guid))
        .saturating_add(combat_field_bonus(ctx, caster_guid, COMBAT_SPELL_POWER))
        .saturating_add(crate::items::equipped_school_stat_bonus(
            ctx,
            caster_guid,
            crate::items::EquipStat::SpellPower,
            school_mask,
        ))
}

pub(crate) fn healing_power(ctx: &ReducerContext, caster_guid: u64, school_mask: u8) -> i32 {
    spell_power(ctx, caster_guid, school_mask).saturating_add(
        crate::items::equipped_school_stat_bonus(
            ctx,
            caster_guid,
            crate::items::EquipStat::HealingPower,
            school_mask,
        ),
    )
}

pub(crate) fn effective_intellect(ctx: &ReducerContext, caster_guid: u64) -> i32 {
    let base_int = ctx
        .db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .map(|c| c.intellect as i32)
        .unwrap_or(0);
    (i64::from(base_int)
        + i64::from(stat_bonus(ctx, caster_guid, STAT_INT))
        + i64::from(crate::items::equipped_stat_bonus(
            ctx,
            caster_guid,
            crate::items::EquipStat::Intellect,
        )))
    .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

pub(crate) fn stat_bonus(ctx: &ReducerContext, unit_guid: u64, stat: u8) -> i32 {
    sum_active_auras(ctx, unit_guid, |a| {
        a.eff_kind == A_MOD_STAT && (a.eff_p0 == stat as i32 || a.eff_p0 == STAT_ALL as i32)
    })
}

pub(crate) fn stat_pct_bonus(ctx: &ReducerContext, unit_guid: u64, stat: u8) -> i32 {
    sum_active_auras(ctx, unit_guid, |a| {
        a.eff_kind == A_MOD_STAT_PCT && (a.eff_p0 == stat as i32 || a.eff_p0 == STAT_ALL as i32)
    })
}

pub(crate) fn resistance_bonus(ctx: &ReducerContext, unit_guid: u64, school_mask: u8) -> i32 {
    use lyracore_shared::aura::AuraAmount;
    lyracore_shared::resistance::resistance_bonus(
        school_mask,
        auras_on(ctx, unit_guid).map(|a| AuraAmount {
            kind: a.eff_kind,
            parameter: a.eff_p0,
            amount: a.amount,
            stacks: a.stacks,
        }),
        crate::items::resistance_items(ctx, unit_guid),
        |id, kind| crate::items::enchant_stat(ctx, id, kind),
    )
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

pub(crate) fn aura_moves_vitals(kind: u8, p0: i32) -> bool {
    kind == A_MOD_STAT && (p0 == STAT_STA as i32 || p0 == STAT_INT as i32 || p0 == STAT_ALL as i32)
}

pub(crate) use lyracore_shared::aura::moves_sheet as aura_moves_sheet;

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
    fn power_regen_auras_multiply_and_floor_at_zero() {
        assert_eq!(compound_regen_pct([].into_iter()), 100);
        assert_eq!(compound_regen_pct([100].into_iter()), 200);
        assert_eq!(compound_regen_pct([100, 50].into_iter()), 300);
        assert_eq!(compound_regen_pct([-150].into_iter()), 0);
    }

    #[test]
    fn stacked_amount_compounds_with_stacks() {
        assert_eq!(stacked_amount(&aura(-90, 5)), -450);
        assert_eq!(stacked_amount(&aura(-90, 1)), -90);
        assert_eq!(stacked_amount(&aura(50, 1)), 50);
        assert_eq!(stacked_amount(&aura(50, 0)), 50);
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

    #[test]
    fn projectile_travel_ms_scales_distance_over_speed() {
        assert_eq!(projectile_travel_ms(21.0, 21.0), 1000);
        assert_eq!(projectile_travel_ms(10.5, 21.0), 500);
        assert_eq!(projectile_travel_ms(0.0, 21.0), 0);
        assert_eq!(projectile_travel_ms(30.0, 0.0), 0);
        assert_eq!(projectile_travel_ms(30.0, -1.0), 0);
    }

    #[test]
    fn apply_resist_pct_scales_and_floors_at_one() {
        assert_eq!(apply_resist_pct(100, 0), 100);
        assert_eq!(apply_resist_pct(100, 50), 50);
        assert_eq!(apply_resist_pct(100, 75), 25);
        assert_eq!(apply_resist_pct(4, 75), 1);
        assert_eq!(apply_resist_pct(100, 100), 1);
        assert_eq!(apply_resist_pct(1, 75), 1);
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

#[cfg(test)]
mod facing_tests {
    use super::is_facing;
    use std::f32::consts::PI;

    #[test]
    fn front_and_behind_at_the_cardinal_orientations() {
        assert!(is_facing(0.0, 0.0, 0.0, 5.0, 0.0));
        assert!(!is_facing(0.0, 0.0, 0.0, -5.0, 0.0));
        assert!(is_facing(0.0, 0.0, PI, -5.0, 0.0));
        assert!(!is_facing(0.0, 0.0, PI, 5.0, 0.0));
        assert!(is_facing(0.0, 0.0, PI / 2.0, 0.0, 5.0));
        assert!(!is_facing(0.0, 0.0, PI / 2.0, 0.0, -5.0));
    }

    #[test]
    fn the_90_degree_seam_is_exclusive_and_diagonals_resolve() {
        assert!(!is_facing(0.0, 0.0, 0.0, 0.0, 5.0));
        assert!(!is_facing(0.0, 0.0, 0.0, 0.0, -5.0));
        assert!(is_facing(0.0, 0.0, 0.0, 0.1, 5.0));
        assert!(!is_facing(0.0, 0.0, 0.0, -0.1, 5.0));
    }

    #[test]
    fn wrapped_and_negative_orientations_behave() {
        assert!(is_facing(0.0, 0.0, 2.0 * PI, 5.0, 0.001));
        assert!(is_facing(0.0, 0.0, -PI / 2.0, 0.0, -5.0));
        assert!(!is_facing(0.0, 0.0, -PI / 2.0, 0.0, 5.0));
    }

    #[test]
    fn coincident_points_always_face() {
        assert!(is_facing(3.0, -4.0, 1.234, 3.0, -4.0));
    }
}
