use spacetimedb::ReducerContext;

use crate::{
    game_aura, game_creature_template, game_item_instance, game_item_template, game_spell,
    game_spell_effect, game_world_entity, ItemInstance, WorldEntity,
};

use super::*;

fn fold_band(
    ctx: &ReducerContext,
    target_guid: u64,
    base: u32,
    field: u8,
    gear: crate::items::EquipStat,
) -> u32 {
    (i64::from(base)
        + i64::from(crate::spell::combat_field_bonus(ctx, target_guid, field))
        + i64::from(crate::items::equipped_stat_bonus(ctx, target_guid, gear)))
    .clamp(0, 10_000) as u32
}

pub(crate) fn effective_dodge_bp(ctx: &ReducerContext, target: &WorldEntity, sd: u32) -> u32 {
    fold_band(
        ctx,
        target.guid,
        dodge_chance_bp(effective_agility(ctx, target), sd),
        crate::spell::COMBAT_DODGE,
        crate::items::EquipStat::Dodge,
    )
}

pub(crate) fn effective_parry_bp(ctx: &ReducerContext, target: &WorldEntity, sd: u32) -> u32 {
    fold_band(
        ctx,
        target.guid,
        parry_chance_bp(sd),
        crate::spell::COMBAT_PARRY,
        crate::items::EquipStat::Parry,
    )
}

pub(crate) fn effective_block_bp(
    ctx: &ReducerContext,
    target: &WorldEntity,
    sd: u32,
    block_value: u32,
) -> u32 {
    if block_value > 0 {
        fold_band(
            ctx,
            target.guid,
            block_chance_bp(sd),
            crate::spell::COMBAT_BLOCK,
            crate::items::EquipStat::Block,
        )
    } else {
        0
    }
}

pub(crate) fn aura_attack_power_bonus(ctx: &ReducerContext, unit_guid: u64) -> u32 {
    crate::spell::combat_field_bonus(ctx, unit_guid, crate::spell::COMBAT_ATTACK_POWER).max(0)
        as u32
}

pub(crate) fn seal_holy_on_swing(ctx: &ReducerContext, attacker: &WorldEntity) -> (u32, u32) {
    let (amount, spell) = crate::spell::seal_amount_and_spell(ctx, attacker.guid);
    (seal_swing_holy(amount, attacker.base_attack_time_ms), spell)
}

fn seal_swing_holy(seal: i32, attack_time_ms: u32) -> u32 {
    if seal <= 0 {
        return 0;
    }
    ((seal as u64 * attack_time_ms as u64) / 100_000) as u32
}

#[derive(Clone, Copy, Default)]
pub(crate) struct QueuedStrikeDamage {
    pub(crate) bonus: u32,
    pub(crate) mod_flat: i32,
    pub(crate) mod_pct: i32,
}

pub(crate) fn queued_strike_damage(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    strike: &crate::spell::Spell,
) -> QueuedStrikeDamage {
    let (mod_flat, mod_pct) =
        crate::spell::spell_mod(ctx, attacker.guid, crate::spell::SPELLMOD_OP_DAMAGE, strike);
    QueuedStrikeDamage {
        bonus: queued_strike_bonus(ctx, attacker, strike),
        mod_flat,
        mod_pct,
    }
}

// vmangos interrupts a form-bound queued strike whenever its form is lost, even for a sibling form.
pub(crate) fn interrupt_form_bound_queued_strike(ctx: &ReducerContext, unit: &WorldEntity) {
    let Some(strike) = ctx.db.game_spell().spell_id().find(unit.next_swing_spell) else {
        return;
    };
    if strike.stances != 0 {
        interrupt_queued_strike(ctx, unit);
    }
}

// vmangos AttackStop interrupts the queued melee spell.
pub(crate) fn interrupt_queued_strike_of(ctx: &ReducerContext, guid: u64) {
    if let Some(unit) = ctx.db.game_world_entity().guid().find(guid) {
        interrupt_queued_strike(ctx, &unit);
    }
}

fn interrupt_queued_strike(ctx: &ReducerContext, unit: &WorldEntity) {
    if unit.next_swing_spell == 0 {
        return;
    }
    clear_queued_strike(ctx, unit.guid);
    crate::spell::SpellCastEvent::insert(
        ctx,
        crate::spell::SpellCastEvent {
            is_interrupted: true,
            failure_reason: crate::spell::CAST_FAIL_INTERRUPTED,
            ..crate::spell::SpellCastEvent::signal_at(
                ctx,
                unit,
                unit.next_swing_spell,
                crate::spell::SpellCastEventKind::Interrupt,
            )
        },
    );
}

fn queued_strike_bonus(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    strike: &crate::spell::Spell,
) -> u32 {
    let Some(effect) = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&strike.spell_id)
        .find(|e| e.kind == crate::spell::E_NEXT_SWING)
    else {
        return 0;
    };
    strike_bonus_points(&effect, strike, attacker.level, ctx.random::<u32>())
}

fn strike_bonus_points(
    effect: &crate::spell::SpellEffect,
    strike: &crate::spell::Spell,
    attacker_level: u32,
    roll: u32,
) -> u32 {
    crate::spell::effect_amount(
        effect.base_points,
        effect.die_sides,
        effect.per_level,
        attacker_level,
        strike.spell_level,
        strike.max_level,
        roll,
    )
    .max(0) as u32
}

pub(crate) fn clear_queued_strike(ctx: &ReducerContext, guid: u64) {
    let entities = ctx.db.game_world_entity();
    if let Some(mut e) = entities.guid().find(guid) {
        if e.next_swing_spell != 0 {
            e.next_swing_spell = 0;
            e.update(ctx);
        }
    }
}

pub(crate) const REACT_WINDOW_MS: u64 = 5000;

#[derive(Clone, Copy)]
pub(crate) enum ReactKind {
    Overpower,
    Revenge,
}

pub(crate) fn react_window_active(until_ms: u64, now_ms: u64) -> bool {
    until_ms > now_ms
}

pub(crate) fn arm_react(ctx: &ReducerContext, guid: u64, kind: ReactKind) {
    let entities = ctx.db.game_world_entity();
    let now_ms = (ctx.timestamp.to_micros_since_unix_epoch() / 1000) as u64;
    if let Some(mut e) = entities.guid().find(guid) {
        let until = now_ms + REACT_WINDOW_MS;
        match kind {
            ReactKind::Overpower => e.overpower_until_ms = until,
            ReactKind::Revenge => e.revenge_until_ms = until,
        }
        e.update(ctx);
    }
}

pub(crate) fn effective_strength(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
    effective_stat(
        e.strength as i32,
        crate::spell::stat_bonus(ctx, e.guid, crate::spell::STAT_STR),
        crate::items::equipped_stat_bonus(ctx, e.guid, crate::items::EquipStat::Strength),
    )
}

pub(crate) fn effective_agility(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
    effective_stat(
        e.agility as i32,
        crate::spell::stat_bonus(ctx, e.guid, crate::spell::STAT_AGI),
        crate::items::equipped_stat_bonus(ctx, e.guid, crate::items::EquipStat::Agility),
    )
}

pub(crate) fn effective_armor(ctx: &ReducerContext, e: &WorldEntity) -> u32 {
    lyracore_shared::resistance::effective_resistance(
        e.armor,
        crate::spell::resistance_bonus(ctx, e.guid, crate::spell::RESIST_ARMOR),
    )
}

pub(crate) fn effective_block_value(ctx: &ReducerContext, defender: &WorldEntity) -> u32 {
    let Some(inst) =
        crate::items::item_in_slot(ctx, defender.guid, crate::items::equip_slot::OFFHAND)
    else {
        return 0;
    };
    let Some(tmpl) = ctx.db.game_item_template().entry().find(inst.entry) else {
        return 0;
    };
    if tmpl.inventory_type != crate::items::invtype::SHIELD
        || crate::items::item_is_broken(&tmpl, &inst)
    {
        return 0;
    }
    let from_strength = effective_strength(ctx, defender) / 20;
    (tmpl.block_value.max(0) as u32).saturating_add(from_strength)
}

pub(crate) fn effective_crit_bp(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let bonus = crate::spell::combat_field_bonus(ctx, attacker.guid, crate::spell::COMBAT_CRIT)
        + crate::items::equipped_stat_bonus(ctx, attacker.guid, crate::items::EquipStat::Crit)
        + agility_crit_bp(effective_agility(ctx, attacker), attacker.level) as i32;
    crit_bp_with_bonus(bonus)
}

pub(crate) fn effective_miss_bp(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    skill_diff: u32,
) -> u32 {
    let bonus = crate::spell::combat_field_bonus(ctx, attacker.guid, crate::spell::COMBAT_HIT)
        + crate::items::equipped_stat_bonus(ctx, attacker.guid, crate::items::EquipStat::Hit);
    miss_bp_with_bonus(miss_chance_bp(skill_diff), bonus)
}

pub(crate) fn effective_swing_time(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let speed = crate::spell::speed_bonus(ctx, attacker.guid, crate::spell::SPEED_SWING);
    swing_time_with_haste(attacker.base_attack_time_ms, speed)
}

pub(crate) fn effective_move_speed(ctx: &ReducerContext, guid: u64, base: f32) -> f32 {
    lyracore_shared::aura::run_speed(
        base,
        crate::spell::auras_on(ctx, guid).map(|aura| lyracore_shared::aura::AuraAmount {
            kind: aura.eff_kind,
            parameter: aura.eff_p0,
            amount: aura.amount,
            stacks: aura.stacks,
        }),
    )
}

#[cfg(test)]
use lyracore_shared::aura::mounted_move_pct;

fn weapon_profile_in_slot(
    ctx: &ReducerContext,
    player_guid: u64,
    slot: u8,
) -> Option<(u32, u32, u32, u8)> {
    let inst = crate::items::item_in_slot(ctx, player_guid, slot)?;
    let tmpl = ctx.db.game_item_template().entry().find(inst.entry)?;
    if tmpl.class != lyracore_shared::constants::starter_item::CLASS_WEAPON {
        return None;
    }
    if crate::items::item_is_broken(&tmpl, &inst) {
        return None;
    }
    let bonus =
        crate::items::item_stat_bonus(ctx, &inst, &tmpl, crate::items::EquipStat::WeaponDamage);
    Some((
        add_weapon_damage(tmpl.damage_min, bonus),
        add_weapon_damage(tmpl.damage_max, bonus),
        tmpl.delay_ms,
        tmpl.subclass,
    ))
}

fn add_weapon_damage(base: f32, bonus: i32) -> u32 {
    (base.round() as i64 + i64::from(bonus)).clamp(0, i64::from(u32::MAX)) as u32
}

fn equipped_weapon_damage(ctx: &ReducerContext, player_guid: u64) -> Option<(u32, u32, u32)> {
    weapon_profile_in_slot(
        ctx,
        player_guid,
        lyracore_shared::constants::starter_item::MAINHAND_SLOT,
    )
    .map(|(dmin, dmax, delay, _)| (dmin, dmax, delay))
}

pub(crate) fn equipped_offhand_weapon_damage(
    ctx: &ReducerContext,
    player_guid: u64,
) -> Option<(u32, u32, u32)> {
    weapon_profile_in_slot(ctx, player_guid, crate::items::equip_slot::OFFHAND)
        .map(|(dmin, dmax, delay, _)| (dmin, dmax, delay))
}

pub(crate) fn equipped_ranged_weapon(
    ctx: &ReducerContext,
    player_guid: u64,
) -> Option<(u32, u32, u32, u8)> {
    weapon_profile_in_slot(ctx, player_guid, crate::items::equip_slot::RANGED)
}

pub(crate) fn find_ammo(ctx: &ReducerContext, player_guid: u64) -> Option<ItemInstance> {
    let templates = ctx.db.game_item_template();
    lyracore_shared::item::loaded_ammo(
        ctx.db
            .game_item_instance()
            .by_owner_guid()
            .filter(&player_guid),
        |item| {
            (
                item.slot,
                item.stack_count,
                templates.entry().find(item.entry).map(|tmpl| tmpl.class),
            )
        },
    )
}

const DURABILITY_WEAR_CHANCE_PCT: u32 = 10;

pub(crate) fn wear_weapon(ctx: &ReducerContext, player_guid: u64) {
    if let Some(mut inst) = crate::items::item_in_slot(
        ctx,
        player_guid,
        lyracore_shared::constants::starter_item::MAINHAND_SLOT,
    ) {
        if inst.durability > 0 && ctx.random::<u32>() % 100 < DURABILITY_WEAR_CHANCE_PCT {
            inst.durability -= 1;
            inst.update(ctx);
        }
    }
}

pub(crate) fn swing_range_ctx(ctx: &ReducerContext, attacker: &WorldEntity) -> (u32, u32) {
    melee_range(ctx, attacker, false)
}

fn melee_range(ctx: &ReducerContext, attacker: &WorldEntity, normalized: bool) -> (u32, u32) {
    let disarmed = crate::spell::is_disarmed(ctx, attacker.guid);
    if !attacker.is_player() {
        let tmpl = ctx.db.game_creature_template().entry().find(attacker.entry);
        let (base_min, base_max) = match &tmpl {
            Some(t) if t.damage_max > 0 => (t.damage_min, t.damage_max),
            _ => (CREATURE_MELEE_MIN, CREATURE_MELEE_MAX),
        };
        let (base_min, base_max) =
            match (&tmpl, crate::creatures::live_pet_kind(ctx, attacker.guid)) {
                (Some(t), crate::creatures::PetKind::Hunter { .. }) => {
                    crate::creatures::scale_creature_damage_for_level(
                        base_min,
                        base_max,
                        t.level,
                        attacker.level,
                    )
                }
                _ => (base_min, base_max),
            };
        let rank = tmpl.map(|t| t.rank).unwrap_or(0);
        let (min, max) = scale_swing_for_rank(base_min, base_max, rank);
        if disarmed {
            let pct = crate::spell::DISARM_SWING_RETAINED_PCT;
            return (min * pct / 100, max * pct / 100);
        }
        return (min, max);
    }
    let class = attacker.class();
    let ap = melee_attack_power_for(
        class,
        effective_strength(ctx, attacker),
        effective_agility(ctx, attacker),
        attacker.level,
    ) + aura_attack_power_bonus(ctx, attacker.guid);
    let mainhand = lyracore_shared::constants::starter_item::MAINHAND_SLOT;
    let speed = |delay| {
        if normalized {
            normalized_speed_ms(ctx, attacker.guid, mainhand)
        } else {
            delay
        }
    };
    if !disarmed {
        if let Some((dmin, dmax, delay)) = equipped_weapon_damage(ctx, attacker.guid) {
            return weapon_swing_range_ap(ap, dmin, dmax, speed(delay));
        }
    }
    player_swing_range_ap(ap, speed(attacker.base_attack_time_ms))
}

const NORMALIZED_ONE_HAND_MS: u32 = 2400;
const NORMALIZED_TWO_HAND_MS: u32 = 3300;
const NORMALIZED_DAGGER_MS: u32 = 1700;
const NORMALIZED_RANGED_MS: u32 = 2800;

// vmangos GetAPMultiplier: normalized damage adds attack power at a fixed speed per weapon type.
fn normalized_speed_ms(ctx: &ReducerContext, player_guid: u64, slot: u8) -> u32 {
    normalized_speed_for(
        crate::items::item_in_slot(ctx, player_guid, slot)
            .and_then(|item| ctx.db.game_item_template().entry().find(item.entry))
            .map(|tmpl| (tmpl.inventory_type, tmpl.subclass)),
    )
}

fn normalized_speed_for(weapon: Option<(u8, u8)>) -> u32 {
    use crate::items::invtype as t;
    match weapon {
        Some((t::TWO_HAND_WEAPON, _)) => NORMALIZED_TWO_HAND_MS,
        Some((t::RANGED | t::RANGED_RIGHT | t::THROWN, _)) => NORMALIZED_RANGED_MS,
        Some((_, crate::items::weapon_subclass::DAGGER)) => NORMALIZED_DAGGER_MS,
        _ => NORMALIZED_ONE_HAND_MS,
    }
}

/// One weapon spell's damage terms; `percent` scales the weapon roll and `bonus` together.
#[derive(Clone, Copy)]
pub(crate) struct WeaponStrike {
    pub(crate) bonus: i32,
    pub(crate) percent: u32,
    pub(crate) normalized: bool,
    pub(crate) ranged: bool,
}

// vmangos MeleeSpellHitResult: no glancing or crushing; ranged strikes skip dodge and parry.
pub(crate) fn roll_strike_outcome(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    ranged: bool,
) -> u8 {
    let sd = crate::skill::skill_diff_ctx(ctx, attacker, target);
    let (dodge, parry) = if ranged {
        (0, 0)
    } else {
        (
            effective_dodge_bp(ctx, target, sd),
            effective_parry_bp(ctx, target, sd),
        )
    };
    let table = AttackTable {
        miss: effective_miss_bp(ctx, attacker, sd),
        dodge,
        parry,
        glancing: 0,
        block: 0,
        crit: effective_crit_bp(ctx, attacker),
        crushing: 0,
    };
    attack_outcome(ctx.random::<u32>() % 10_000, &table)
}

pub(crate) fn weapon_strike_spell_damage(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    strike: WeaponStrike,
    spell: &crate::spell::Spell,
    crit: bool,
) -> u32 {
    let (min, max) = strike_weapon_range(ctx, attacker, strike);
    let weapon = min + ctx.random::<u32>() % (max.saturating_sub(min) + 1);
    let mods = crate::spell::spell_mod(ctx, attacker.guid, crate::spell::SPELLMOD_OP_DAMAGE, spell);
    let mit_pct = if spell.school_mask == SCHOOL_MASK_PHYSICAL {
        armor_mitigation_pct(effective_armor(ctx, target), attacker.level)
    } else {
        0
    };
    strike_amount(weapon, strike.bonus, strike.percent, mods, crit, mit_pct)
}

const SCHOOL_MASK_PHYSICAL: u8 = 1;

fn strike_weapon_range(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    strike: WeaponStrike,
) -> (u32, u32) {
    if !attacker.is_player() {
        return crate::creatures::scale_hunter_pet_swing(
            ctx,
            attacker.guid,
            swing_range_ctx(ctx, attacker),
        );
    }
    if !strike.ranged {
        return melee_range(ctx, attacker, strike.normalized);
    }
    let Some((dmin, dmax, delay, subclass)) = equipped_ranged_weapon(ctx, attacker.guid) else {
        return (0, 0);
    };
    let delay = if strike.normalized {
        normalized_speed_ms(ctx, attacker.guid, crate::items::equip_slot::RANGED)
    } else {
        delay
    };
    let (_, min, max) = ranged_sheet_values(
        effective_agility(ctx, attacker),
        attacker.level,
        Some((dmin, dmax, delay, subclass)),
    );
    (min, max)
}

fn strike_amount(
    weapon: u32,
    bonus: i32,
    percent: u32,
    (mod_flat, mod_pct): (i32, i32),
    crit: bool,
    mit_pct: u32,
) -> u32 {
    let scaled = (i64::from(weapon) + i64::from(bonus)).max(0) * i64::from(percent) / 100;
    let base =
        crate::spell::apply_spell_mod(scaled.min(i64::from(u32::MAX)) as u32, mod_flat, mod_pct);
    let pre_mit = u64::from(base) * if crit { u64::from(CRIT_MULT) } else { 1 };
    (pre_mit * u64::from(100 - mit_pct.min(100)) / 100).clamp(1, u64::from(u32::MAX)) as u32
}

// vmangos TakeAmmo: a thrown stack loses one; an unstackable thrown weapon loses durability.
pub(crate) fn spend_ranged_ammo(ctx: &ReducerContext, player_guid: u64) {
    let Some(mut weapon) =
        crate::items::item_in_slot(ctx, player_guid, crate::items::equip_slot::RANGED)
    else {
        return;
    };
    let Some(tmpl) = ctx.db.game_item_template().entry().find(weapon.entry) else {
        return;
    };
    if tmpl.inventory_type == crate::items::invtype::THROWN {
        if tmpl.max_stack > 1 {
            spend_one(ctx, weapon);
        } else if weapon.durability > 0 {
            weapon.durability -= 1;
            weapon.update(ctx);
        }
    } else if tmpl.subclass != crate::items::weapon_subclass::WAND {
        if let Some(ammo) = find_ammo(ctx, player_guid) {
            spend_one(ctx, ammo);
        }
    }
}

pub(crate) fn spend_one(ctx: &ReducerContext, mut item: ItemInstance) {
    item.stack_count = item.stack_count.saturating_sub(1);
    if item.stack_count == 0 {
        crate::ItemInstance::delete(ctx, item.guid);
    } else {
        item.update(ctx);
    }
}

// vmangos WriteAmmoToPacket: a thrown weapon flies as itself; a launcher shows its loaded ammo.
pub(crate) fn ranged_projectile(ctx: &ReducerContext, player_guid: u64) -> Option<(u32, u32)> {
    let templates = ctx.db.game_item_template();
    let weapon = crate::items::item_in_slot(ctx, player_guid, crate::items::equip_slot::RANGED)?;
    let tmpl = templates.entry().find(weapon.entry)?;
    if tmpl.inventory_type == crate::items::invtype::THROWN {
        return Some((tmpl.display_id, u32::from(tmpl.inventory_type)));
    }
    if tmpl.subclass == crate::items::weapon_subclass::WAND {
        return None;
    }
    let ammo = templates.entry().find(find_ammo(ctx, player_guid)?.entry)?;
    Some((ammo.display_id, u32::from(ammo.inventory_type)))
}

pub(crate) fn weapon_strike_damage(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    target: &WorldEntity,
    flat_bonus: i32,
    (mod_flat, mod_pct): (i32, i32),
) -> (u32, bool) {
    let (min, max) = swing_range_ctx(ctx, caster);
    let base = min + ctx.random::<u32>() % (max - min + 1);
    let total = (base as i32 + flat_bonus).max(1) as u32;
    let total = crate::spell::apply_spell_mod(total, mod_flat, mod_pct).max(1);
    let crit = (ctx.random::<u32>() % 10_000) < effective_crit_bp(ctx, caster);
    let pre_mit = if crit { total * CRIT_MULT } else { total };
    let mit_pct = armor_mitigation_pct(effective_armor(ctx, target), caster.level);
    (armor_mitigated(pre_mit, mit_pct), crit)
}

pub(crate) fn retaliate_on_hit(ctx: &ReducerContext, victim_guid: u64, attacker_guid: u64) {
    let carries_retaliate = ctx
        .db
        .game_aura()
        .by_target()
        .filter(&victim_guid)
        .any(|a| a.eff_kind == crate::spell::A_RETALIATE);
    if !carries_retaliate {
        return;
    }
    let entities = ctx.db.game_world_entity();
    let (Some(victim), Some(attacker)) = (
        entities.guid().find(victim_guid),
        entities.guid().find(attacker_guid),
    ) else {
        return;
    };
    if victim.dead || attacker.dead {
        return;
    }
    let (dmg, _crit) = weapon_strike_damage(ctx, &victim, &attacker, 0, (0, 0));
    crate::spell::apply_target_damage(
        ctx,
        attacker_guid,
        victim_guid,
        dmg as i32,
        crate::combat::Hit::triggered(),
    );
}

// `strike` is a firing queued strike's damage; `None` rolls a white swing.
pub(crate) fn roll_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    strike: Option<QueuedStrikeDamage>,
) -> (u32, u8, u32) {
    let range = crate::creatures::scale_hunter_pet_swing(
        ctx,
        attacker.guid,
        swing_range_ctx(ctx, attacker),
    );
    roll_swing_with_range(ctx, attacker, target, range, strike)
}

pub(crate) fn roll_swing_with_range(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    range: (u32, u32),
    strike: Option<QueuedStrikeDamage>,
) -> (u32, u8, u32) {
    let block_value = effective_block_value(ctx, target);
    let sd = crate::skill::skill_diff_ctx(ctx, attacker, target);
    let table = AttackTable {
        miss: effective_miss_bp(ctx, attacker, sd),
        dodge: effective_dodge_bp(ctx, target, sd),
        parry: effective_parry_bp(ctx, target, sd),
        glancing: glancing_chance_bp(attacker.level, target.level),
        block: effective_block_bp(ctx, target, sd, block_value),
        crit: effective_crit_bp(ctx, attacker),
        crushing: if !attacker.is_player() && target.is_player() {
            crushing_chance_bp(attacker.level, target.level)
        } else {
            0
        },
    };
    let rolls = SwingRolls {
        outcome: ctx.random::<u32>() % 10_000,
        weapon: ctx.random::<u32>(),
        block: ctx.random::<u32>() % 10_000,
    };
    let mit_pct = armor_mitigation_pct(effective_armor(ctx, target), attacker.level);
    resolve_swing(table, rolls, range, strike, mit_pct, block_value)
}

struct SwingRolls {
    outcome: u32,
    weapon: u32,
    block: u32,
}

// Returns (damage, outcome, blocked); a blocked crit stays a crit.
fn resolve_swing(
    mut table: AttackTable,
    rolls: SwingRolls,
    (min, max): (u32, u32),
    strike: Option<QueuedStrikeDamage>,
    mit_pct: u32,
    block_value: u32,
) -> (u32, u8, u32) {
    // vmangos rolls a queued strike's partial block after the table, so it can crit and block.
    let block_bp = table.block;
    if strike.is_some() {
        table.block = 0;
    }
    let outcome = attack_outcome(rolls.outcome, &table);
    if matches!(outcome, HIT_MISS | HIT_DODGE | HIT_PARRY) {
        return (0, outcome, 0);
    }
    let weapon = min + rolls.weapon % (max - min + 1);
    let base = strike.map_or(weapon, |strike| {
        crate::spell::apply_spell_mod(
            weapon.saturating_add(strike.bonus),
            strike.mod_flat,
            strike.mod_pct,
        )
    });
    let pre_mit = match outcome {
        HIT_CRIT => base * CRIT_MULT,
        HIT_GLANCING => (base * GLANCING_DAMAGE_PCT / 100).max(1),
        HIT_CRUSHING => base * CRUSH_DAMAGE_PCT / 100,
        _ => base,
    };
    let dmg = armor_mitigated(pre_mit, mit_pct);
    let blocks = match strike {
        Some(_) => rolls.block < block_bp,
        None => outcome == HIT_BLOCK,
    };
    if !blocks {
        return (dmg, outcome, 0);
    }
    let blocked = block_value.min(dmg);
    let outcome = if outcome == HIT_CRIT {
        HIT_CRIT
    } else {
        HIT_BLOCK
    };
    (dmg - blocked, outcome, blocked)
}

pub(crate) fn roll_ranged_swing(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    dmin: u32,
    dmax: u32,
) -> (u32, u8) {
    let sd = crate::skill::skill_diff_ctx(ctx, attacker, target);
    if (ctx.random::<u32>() % 10_000) < effective_miss_bp(ctx, attacker, sd) {
        return (0, HIT_MISS);
    }
    let is_crit = (ctx.random::<u32>() % 10_000) < effective_crit_bp(ctx, attacker);
    let base = dmin + ctx.random::<u32>() % (dmax.saturating_sub(dmin) + 1);
    let pre_mit = if is_crit { base * CRIT_MULT } else { base };
    let mit_pct = armor_mitigation_pct(effective_armor(ctx, target), attacker.level);
    let dmg = armor_mitigated(pre_mit, mit_pct);
    (dmg, if is_crit { HIT_CRIT } else { HIT_NORMAL })
}

pub(crate) fn roll_money(ctx: &ReducerContext, min: u32, max: u32) -> u32 {
    if max == 0 || max < min {
        return 0;
    }
    min + ctx.random::<u32>() % (max - min + 1)
}

#[cfg(test)]
mod weapon_strike_tests {
    use super::{normalized_speed_for, strike_amount};

    #[test]
    fn a_weapon_percent_scales_the_weapon_roll_and_the_flat_bonus_together() {
        // vmangos EffectWeaponDmg: (40 weapon + 10 bonus) at Backstab's 150%.
        assert_eq!(strike_amount(40, 10, 150, (0, 0), false, 0), 75);
    }

    #[test]
    fn a_strike_crits_for_double_before_armor_mitigates_it() {
        assert_eq!(strike_amount(40, 10, 100, (0, 0), true, 20), 80);
    }

    #[test]
    fn a_damage_spell_mod_applies_after_the_weapon_percent() {
        assert_eq!(strike_amount(40, 10, 100, (5, 10), false, 0), 60);
    }

    #[test]
    fn normalized_speed_follows_the_weapon_type() {
        use crate::items::{invtype as t, weapon_subclass as ws};
        assert_eq!(normalized_speed_for(None), 2400);
        assert_eq!(normalized_speed_for(Some((t::WEAPON, ws::DAGGER))), 1700);
        assert_eq!(normalized_speed_for(Some((t::WEAPON, ws::SWORD_1H))), 2400);
        assert_eq!(
            normalized_speed_for(Some((t::TWO_HAND_WEAPON, ws::STAFF))),
            3300
        );
        assert_eq!(normalized_speed_for(Some((t::THROWN, ws::THROWN))), 2800);
    }
}

#[cfg(test)]
mod weapon_damage_tests {
    use super::add_weapon_damage;

    #[test]
    fn flat_weapon_damage_applies_after_template_rounding_and_clamps() {
        assert_eq!(add_weapon_damage(10.6, 20), 31);
        assert_eq!(add_weapon_damage(10.4, -20), 0);
        assert_eq!(add_weapon_damage(u32::MAX as f32, i32::MAX), u32::MAX);
    }
}

#[cfg(test)]
mod seal_tests {
    use super::seal_swing_holy;

    #[test]
    fn seal_swing_holy_scales_with_coeff_and_speed() {
        assert_eq!(seal_swing_holy(0, 2000), 0);
        assert_eq!(seal_swing_holy(108, 2000), 2);
        assert_eq!(seal_swing_holy(216, 2000), 4);
        assert_eq!(seal_swing_holy(216, 3600), 7);
    }
}

#[cfg(test)]
mod queued_strike_tests {
    use super::{resolve_swing, strike_bonus_points, QueuedStrikeDamage, SwingRolls};
    use crate::combat::{AttackTable, HIT_BLOCK, HIT_CRIT, HIT_GLANCING, HIT_NORMAL};
    use crate::seed::{base_effect, base_spell};

    fn bonus(base_points: i32, die_sides: i32, per_level: f32) -> crate::spell::SpellEffect {
        crate::spell::SpellEffect {
            base_points,
            die_sides,
            per_level,
            ..base_effect(1, 0)
        }
    }

    fn strike(spell_level: u8) -> crate::spell::Spell {
        crate::spell::Spell {
            spell_level,
            ..base_spell(1, "Strike")
        }
    }

    #[test]
    fn heroic_strike_rank_one_adds_its_flat_eleven() {
        assert_eq!(
            strike_bonus_points(&bonus(11, 1, 0.0), &strike(1), 60, 7),
            11
        );
    }

    #[test]
    fn a_creature_strike_scales_with_the_attacker_level() {
        // Strike 11976: 5 points plus 1.5 per level above 1; level 60 adds 5 + 88.
        assert_eq!(
            strike_bonus_points(&bonus(5, 1, 1.5), &strike(1), 60, 0),
            93
        );
    }

    fn table(block: u32, crit: u32, glancing: u32) -> AttackTable {
        AttackTable {
            miss: 0,
            dodge: 0,
            parry: 0,
            glancing,
            block,
            crit,
            crushing: 0,
        }
    }

    fn rolls(block: u32) -> SwingRolls {
        SwingRolls {
            outcome: 0,
            weapon: 0,
            block,
        }
    }

    fn heroic_strike(bonus: u32) -> Option<QueuedStrikeDamage> {
        Some(QueuedStrikeDamage {
            bonus,
            ..Default::default()
        })
    }

    #[test]
    fn a_queued_strike_can_crit_and_be_blocked() {
        // (100 + 11) doubled by the crit, halved by armor, then 30 blocked.
        assert_eq!(
            resolve_swing(
                table(10_000, 10_000, 0),
                rolls(0),
                (100, 100),
                heroic_strike(11),
                50,
                30
            ),
            (81, HIT_CRIT, 30)
        );
    }

    #[test]
    fn a_white_swing_blocks_before_it_can_crit() {
        assert_eq!(
            resolve_swing(table(10_000, 10_000, 0), rolls(0), (100, 100), None, 50, 30),
            (20, HIT_BLOCK, 30)
        );
    }

    #[test]
    fn a_queued_strike_misses_its_block_roll_above_the_block_chance() {
        assert_eq!(
            resolve_swing(
                table(5_000, 0, 0),
                rolls(5_000),
                (100, 100),
                heroic_strike(11),
                0,
                30
            ),
            (111, HIT_NORMAL, 0)
        );
    }

    #[test]
    fn damage_modifiers_scale_the_weapon_and_the_bonus_together() {
        let maul = Some(QueuedStrikeDamage {
            bonus: 1000,
            mod_flat: 0,
            mod_pct: 100,
        });
        assert_eq!(
            resolve_swing(table(0, 0, 0), rolls(9_999), (1, 1), maul, 0, 0),
            (2002, HIT_NORMAL, 0)
        );
    }

    #[test]
    fn a_glancing_strike_reduces_the_whole_hit() {
        assert_eq!(
            resolve_swing(
                table(0, 0, 10_000),
                rolls(9_999),
                (100, 100),
                heroic_strike(100),
                0,
                0
            ),
            (140, HIT_GLANCING, 0)
        );
    }
}

#[cfg(test)]
mod react_window_tests {
    use super::react_window_active;

    #[test]
    fn unarmed_window_is_closed() {
        assert!(!react_window_active(0, 1_000));
        assert!(!react_window_active(0, 0));
    }

    #[test]
    fn armed_window_is_open_until_deadline() {
        let until = 6_000u64;
        assert!(react_window_active(until, 1_000));
        assert!(react_window_active(until, 5_999));
        assert!(!react_window_active(until, 6_000));
        assert!(!react_window_active(until, 6_001));
    }
}

#[cfg(test)]
mod mounted_speed_tests {
    use super::{mounted_move_pct, move_speed_with_pct};

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
}
