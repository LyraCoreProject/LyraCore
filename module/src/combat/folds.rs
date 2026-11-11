use spacetimedb::ReducerContext;

use crate::{
    game_aura, game_creature_template, game_item_instance, game_item_template, game_world_entity,
    ItemInstance, WorldEntity,
};

use super::queued_strike::QueuedStrikeDamage;
use super::*;

fn fold_band(
    ctx: &ReducerContext,
    target_guid: u64,
    base: u32,
    field: u8,
    gear: crate::items::EquipStat,
) -> u32 {
    (i64::from(base)
        + i64::from(crate::modifiers::combat_field_bonus(
            ctx,
            target_guid,
            field,
        ))
        + i64::from(crate::items::equipped_stat_bonus(ctx, target_guid, gear)))
    .clamp(0, 10_000) as u32
}

pub(crate) fn effective_dodge_bp(ctx: &ReducerContext, target: &WorldEntity, sd: u32) -> u32 {
    fold_band(
        ctx,
        target.guid,
        dodge_chance_bp(
            crate::modifiers::effective_stat(ctx, target, crate::spell::STAT_AGI),
            sd,
        ),
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

pub(crate) fn attack_power_modifiers(ctx: &ReducerContext, guid: u64, field: u8) -> u32 {
    let (positive, negative) = crate::spell::auras_on(ctx, guid)
        .filter(|aura| {
            aura.eff_kind == crate::spell::A_MOD_COMBAT && aura.eff_p0 == i32::from(field)
        })
        .map(|aura| crate::modifiers::stacked_amount(aura.amount, aura.stacks))
        .fold((0i32, 0i32), |(positive, negative), amount| {
            (
                positive.saturating_add(amount.max(0)),
                negative.saturating_add(amount.min(0)),
            )
        });
    u32::from(positive as u16) | (u32::from(negative as u16) << 16)
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
    let now_ms = crate::time::now_ms(ctx);
    if let Some(mut e) = entities.guid().find(guid) {
        let until = now_ms + REACT_WINDOW_MS;
        match kind {
            ReactKind::Overpower => e.overpower_until_ms = until,
            ReactKind::Revenge => e.revenge_until_ms = until,
        }
        e.update(ctx);
    }
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
    let from_strength =
        crate::modifiers::effective_stat(ctx, defender, crate::spell::STAT_STR) / 20;
    (tmpl.block_value.max(0) as u32).saturating_add(from_strength)
}

pub(crate) fn effective_crit_bp(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    crit_bp_for_weapon_skill(
        ctx,
        attacker,
        crate::skill::effective_weapon_skill(ctx, attacker),
    )
}

pub(crate) fn effective_ranged_crit_bp(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    crit_bp_for_weapon_skill(
        ctx,
        attacker,
        crate::skill::effective_ranged_weapon_skill(ctx, attacker),
    )
}

fn crit_bp_for_weapon_skill(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    weapon_skill: u32,
) -> u32 {
    let bonus = crate::modifiers::combat_field_bonus(ctx, attacker.guid, crate::spell::COMBAT_CRIT)
        + crate::items::equipped_stat_bonus(ctx, attacker.guid, crate::items::EquipStat::Crit);
    if !attacker.is_player() {
        return (500 + bonus).clamp(0, 10_000) as u32;
    }
    let rate = crate::stats::crit_per_agility_for(ctx, attacker.class(), attacker.level);
    super::white_melee::sheet_crit_bp(
        crate::modifiers::effective_stat(ctx, attacker, crate::spell::STAT_AGI),
        attacker.class(),
        attacker.level,
        rate,
        weapon_skill as i32,
        crate::skill::skill_cap_for_level(attacker.level) as i32,
        bonus,
    )
}

pub(crate) fn effective_miss_bp(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    skill_diff: u32,
) -> u32 {
    let bonus = crate::modifiers::combat_field_bonus(ctx, attacker.guid, crate::spell::COMBAT_HIT)
        + crate::items::equipped_stat_bonus(ctx, attacker.guid, crate::items::EquipStat::Hit);
    miss_bp_with_bonus(miss_chance_bp(skill_diff), bonus)
}

pub(crate) fn effective_swing_time(ctx: &ReducerContext, attacker: &WorldEntity) -> u32 {
    let speed = crate::modifiers::speed_bonus(ctx, attacker.guid, crate::spell::SPEED_SWING);
    swing_time_with_haste(attacker.base_attack_time_ms, speed)
}

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
        let stats = tmpl
            .as_ref()
            .and_then(|t| crate::creatures::catalogue::level_stats(ctx, t, attacker.level));
        let derived = stats.is_some()
            || tmpl.as_ref().is_some_and(|t| {
                crate::creatures::catalogue::level_stats(ctx, t, t.level).is_some()
            });
        let (base_min, base_max) = stats.map_or((base_min, base_max), |stats| {
            (stats.damage_min, stats.damage_max)
        });
        let rank = tmpl.filter(|_| !derived).map(|t| t.rank).unwrap_or(0);
        let (min, max) = scale_swing_for_rank(base_min, base_max, rank);
        if disarmed {
            let pct = crate::spell::DISARM_SWING_RETAINED_PCT;
            return (min * pct / 100, max * pct / 100);
        }
        return (min, max);
    }
    let ap = melee_attack_power(ctx, attacker);
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
    spell: &crate::Spell,
    crit: bool,
) -> u32 {
    let (min, max) = strike_weapon_range(ctx, attacker, strike);
    let weapon = min + ctx.random::<u32>() % (max.saturating_sub(min) + 1);
    let mods = crate::spell::spell_mod(ctx, attacker.guid, crate::spell::SPELLMOD_OP_DAMAGE, spell);
    let mit_pct = if spell.school_mask == SCHOOL_MASK_PHYSICAL
        && spell.custom_flags & lyracore_shared::spell_custom::IGNORE_ARMOR == 0
    {
        armor_mitigation_pct(
            crate::modifiers::effective_armor(ctx, target),
            attacker.level,
        )
    } else {
        0
    };
    strike_amount(weapon, strike.bonus, strike.percent, mods, crit, mit_pct)
}

/// A finisher's damage: `amount` with the damage spell mods, the strike's crit and the target's armor.
pub(crate) fn finisher_strike_damage(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    spell: &crate::Spell,
    amount: i32,
    crit: bool,
) -> u32 {
    let mods = crate::spell::spell_mod(ctx, attacker.guid, crate::spell::SPELLMOD_OP_DAMAGE, spell);
    let mit_pct = if spell.custom_flags & lyracore_shared::spell_custom::IGNORE_ARMOR != 0 {
        0
    } else {
        armor_mitigation_pct(
            crate::modifiers::effective_armor(ctx, target),
            attacker.level,
        )
    };
    strike_amount(0, amount, 100, mods, crit, mit_pct)
}

pub(crate) fn melee_attack_power(ctx: &ReducerContext, unit: &WorldEntity) -> u32 {
    melee_attack_power_for(
        unit.class(),
        crate::modifiers::effective_stat(ctx, unit, crate::spell::STAT_STR),
        crate::modifiers::effective_stat(ctx, unit, crate::spell::STAT_AGI),
        unit.level,
    )
    .saturating_add_signed(crate::modifiers::combat_field_bonus(
        ctx,
        unit.guid,
        crate::spell::COMBAT_ATTACK_POWER,
    ))
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
        attacker.class(),
        crate::modifiers::effective_stat(ctx, attacker, crate::spell::STAT_AGI),
        attacker.level,
        Some((dmin, dmax, delay, subclass)),
        crate::modifiers::combat_field_bonus(
            ctx,
            attacker.guid,
            lyracore_shared::spell::taxonomy::COMBAT_RANGED_ATTACK_POWER,
        ),
    );
    (min as u32, max as u32)
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
    let mit_pct =
        armor_mitigation_pct(crate::modifiers::effective_armor(ctx, target), caster.level);
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
    deal_damage(
        ctx,
        victim_guid,
        attacker_guid,
        dmg as i32,
        Hit::triggered(),
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
    let block_value = if strike.is_none() {
        white_block_value(ctx, target)
    } else {
        effective_block_value(ctx, target)
    };
    let table = if strike.is_none() {
        white_swing_table(ctx, attacker, target, block_value)
    } else {
        let sd = crate::skill::skill_diff_ctx(ctx, attacker, target);
        AttackTable {
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
        }
    };
    let stream = crate::conformance_rolls::RollStream::Swing(attacker.guid);
    let rolls = SwingRolls {
        outcome: crate::conformance_rolls::draw(ctx, stream) % 10_000,
        weapon: crate::conformance_rolls::draw(ctx, stream),
        block: crate::conformance_rolls::draw(ctx, stream) % 10_000,
    };
    let mit_pct = armor_mitigation_pct(
        crate::modifiers::effective_armor(ctx, target),
        attacker.level,
    );
    resolve_swing(table, rolls, range, strike, mit_pct, block_value)
}

pub(crate) fn white_swing_table(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    block_value: u32,
) -> AttackTable {
    use crate::creatures::catalogue::game_creature_defense;

    let target_is_character = target.is_player();
    let weapon = crate::skill::effective_weapon_skill(ctx, attacker) as i32;
    let maximum = crate::skill::skill_cap_for_level(attacker.level) as i32;
    let defense_maximum = crate::skill::skill_cap_for_level(target.level) as i32;
    let flags = (!target_is_character)
        .then(|| ctx.db.game_creature_defense().entry().find(target.entry))
        .flatten();
    let defense = white_defense_skill(
        ctx,
        attacker,
        target,
        flags.as_ref().is_some_and(|flags| flags.no_defense),
    );
    let facing = crate::geometry::is_facing(
        target.x,
        target.y,
        target.orientation,
        attacker.x,
        attacker.y,
    );
    let (dodge, parry, block) =
        white_defense_bands(ctx, target, block_value, facing, flags.as_ref());
    let hit_bonus =
        crate::modifiers::combat_field_bonus(ctx, attacker.guid, crate::spell::COMBAT_HIT)
            + crate::items::equipped_stat_bonus(ctx, attacker.guid, crate::items::EquipStat::Hit);
    let mut table = super::white_melee::attack_table(
        super::white_melee::Skills {
            weapon,
            maximum,
            defense,
            defense_maximum,
            target_level: target.level,
            target_is_character,
        },
        AttackTable {
            miss: if attacker.is_player()
                && attacker.next_swing_spell == 0
                && equipped_offhand_weapon_damage(ctx, attacker.guid).is_some()
            {
                2400
            } else {
                500
            },
            dodge,
            parry,
            glancing: 0,
            block,
            crit: effective_crit_bp(ctx, attacker),
            crushing: if !attacker.is_player() && target_is_character {
                crushing_chance_bp(attacker.level, target.level)
            } else {
                0
            },
        },
        hit_bonus,
        character_controls(ctx, attacker) && !character_controls(ctx, target),
    );
    if target_is_character && !facing {
        table.dodge = 0;
    }
    if target.unit_bytes_1 & 0xff != 0 {
        table.miss = 0;
    }
    table
}

fn character_controls(ctx: &ReducerContext, unit: &WorldEntity) -> bool {
    unit.is_player()
        || unit.owner_guid != 0
            && ctx
                .db
                .game_world_entity()
                .guid()
                .find(unit.owner_guid)
                .is_some_and(|owner| owner.is_player())
}

fn white_defense_bands(
    ctx: &ReducerContext,
    target: &WorldEntity,
    block_value: u32,
    facing: bool,
    flags: Option<&crate::creatures::catalogue::CreatureDefense>,
) -> (u32, u32, u32) {
    let target_is_character = target.is_player();
    let totem = !target_is_character
        && ctx
            .db
            .game_creature_template()
            .entry()
            .find(target.entry)
            .is_some_and(|template| {
                template.creature_type == lyracore_shared::spell::taxonomy::TOTEM_TYPE
            });
    let can_defend = !totem
        && !crate::spell::is_stunned(ctx, target.guid)
        && !crate::spell::is_non_melee_spell_casting(ctx, target.guid);
    let dodge = if target_is_character {
        effective_dodge_bp(ctx, target, 0)
    } else {
        fold_band(
            ctx,
            target.guid,
            500,
            crate::spell::COMBAT_DODGE,
            crate::items::EquipStat::Dodge,
        )
    };
    let parry = effective_parry_bp(ctx, target, 0);
    let block = if target_is_character {
        effective_block_bp(ctx, target, 0, block_value)
    } else {
        fold_band(
            ctx,
            target.guid,
            500,
            crate::spell::COMBAT_BLOCK,
            crate::items::EquipStat::Block,
        )
    };
    let defense_bonus = if target_is_character {
        (i64::from(crate::skill::effective_defense_skill(ctx, target))
            - i64::from(crate::skill::skill_cap_for_level(target.level)))
            * 4
    } else {
        0
    };
    let with_defense = |chance| (i64::from(chance) + defense_bonus).clamp(0, 10_000) as u32;
    let dodge = with_defense(dodge);
    let parry = with_defense(parry);
    let block = if !target_is_character || block_value > 0 {
        with_defense(block)
    } else {
        0
    };
    (
        if can_defend { dodge } else { 0 },
        if can_defend && facing && !flags.is_some_and(|flags| flags.no_parry) {
            parry
        } else {
            0
        },
        if can_defend && facing && !flags.is_some_and(|flags| flags.no_block) {
            block
        } else {
            0
        },
    )
}

pub(crate) fn white_block_value(ctx: &ReducerContext, target: &WorldEntity) -> u32 {
    if target.is_player() {
        effective_block_value(ctx, target)
    } else {
        target.level / 2
            + crate::modifiers::effective_stat(ctx, target, crate::spell::STAT_STR) / 20
    }
}

fn white_defense_skill(
    ctx: &ReducerContext,
    attacker: &WorldEntity,
    target: &WorldEntity,
    no_defense: bool,
) -> i32 {
    use crate::skill::game_player_skill;
    let maximum = crate::skill::skill_cap_for_level(target.level) as i32;
    if no_defense {
        0
    } else if target.is_player() && attacker.is_player() {
        let maximum = ctx
            .db
            .game_player_skill()
            .by_character()
            .filter(target.guid)
            .find(|skill| skill.skill_line == crate::skill::skill_line::DEFENSE)
            .map_or(maximum, |skill| i32::from(skill.max_rank));
        let aura =
            crate::modifiers::combat_field_bonus(ctx, target.guid, crate::spell::COMBAT_DEFENSE);
        let gear =
            crate::items::equipped_stat_bonus(ctx, target.guid, crate::items::EquipStat::Defense);
        (i64::from(maximum) + i64::from(aura) + i64::from(gear)).clamp(0, i64::from(i32::MAX))
            as i32
    } else if target.is_player() {
        crate::skill::effective_defense_skill(ctx, target) as i32
    } else {
        maximum
    }
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
    let is_crit = (ctx.random::<u32>() % 10_000) < effective_ranged_crit_bp(ctx, attacker);
    let base = dmin + ctx.random::<u32>() % (dmax.saturating_sub(dmin) + 1);
    let pre_mit = if is_crit { base * CRIT_MULT } else { base };
    let mit_pct = armor_mitigation_pct(
        crate::modifiers::effective_armor(ctx, target),
        attacker.level,
    );
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
    use super::{resolve_swing, QueuedStrikeDamage, SwingRolls};
    use crate::combat::{AttackTable, HIT_BLOCK, HIT_CRIT, HIT_GLANCING, HIT_NORMAL};

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
