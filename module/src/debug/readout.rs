use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::{game_spell, game_spell_effect, game_world_entity};

#[table(accessor = game_debug_readout, public)]
pub struct DebugReadout {
    #[primary_key]
    pub key: String,
    pub base_min: u32,
    pub base_max: u32,
    pub mitigation_pct: u32,
    pub final_min: u32,
    pub final_max: u32,
    pub note: String,
    #[default(0)]
    pub dodge_bp: u32,
    #[default(0)]
    pub crit_bp: u32,
    #[default(0)]
    pub hit_miss_bp: u32,
    #[default(0)]
    pub parry_bp: u32,
    #[default(0)]
    pub attack_time_ms: u32,
    #[default(0)]
    pub spell_crit_bp: u32,
    #[default(0)]
    pub spell_hit_normal: u32,
    #[default(0)]
    pub spell_hit_crit: u32,
    #[default(0)]
    pub damage_taken_pct: i32,
    #[default(0)]
    pub block_bp: u32,
    #[default(0)]
    pub block_value: u32,
    #[default(0)]
    pub attacker_weapon_skill: u32,
    #[default(0)]
    pub defender_defense_skill: u32,
}

#[reducer]
pub fn debug_compute_swing(ctx: &ReducerContext, attacker_guid: u64, target_guid: u64) {
    let entities = ctx.db.game_world_entity();
    let (Some(attacker), Some(target)) = (
        entities.guid().find(attacker_guid),
        entities.guid().find(target_guid),
    ) else {
        return;
    };
    let (bmin, bmax) = crate::combat::swing_range_ctx(ctx, &attacker);
    let eff_armor = crate::combat::effective_armor(ctx, &target);
    let p = crate::combat::swing_profile(bmin, bmax, eff_armor, attacker.level);
    let crit_bp = crate::combat::effective_crit_bp(ctx, &attacker);
    let sd = crate::skill::skill_diff_ctx(ctx, &attacker, &target);
    let attacker_weapon_skill = crate::skill::effective_weapon_skill(ctx, &attacker);
    let defender_defense_skill = crate::skill::effective_defense_skill(ctx, &target);
    let hit_miss_bp = crate::combat::effective_miss_bp(ctx, &attacker, sd);
    let dodge_bp = crate::combat::effective_dodge_bp(ctx, &target, sd);
    let parry_bp = crate::combat::effective_parry_bp(ctx, &target, sd);
    let block_value = crate::combat::effective_block_value(ctx, &target);
    let block_bp = crate::combat::effective_block_bp(ctx, &target, sd, block_value);
    let attack_time_ms = crate::combat::effective_swing_time(ctx, &attacker);
    let damage_done_pct = crate::spell::stance_damage_done_pct(attacker.stance)
        + crate::spell::combat_field_bonus(ctx, attacker_guid, crate::spell::COMBAT_DMG_DONE);
    let outgoing_min = crate::spell::apply_damage_pct(p.final_min, damage_done_pct);
    let outgoing_max = crate::spell::apply_damage_pct(p.final_max, damage_done_pct);
    let damage_taken_pct = crate::spell::damage_taken_bonus(ctx, target_guid);
    let final_min = crate::spell::apply_damage_pct(outgoing_min, damage_taken_pct);
    let final_max = crate::spell::apply_damage_pct(outgoing_max, damage_taken_pct);
    let readouts = ctx.db.game_debug_readout();
    let key = "swing".to_string();
    readouts.key().delete(&key);
    readouts.insert(DebugReadout {
        key,
        base_min: p.base_min,
        base_max: p.base_max,
        mitigation_pct: p.mitigation_pct,
        final_min,
        final_max,
        note: format!(
            "atk={attacker_guid}(lvl {}) tgt={target_guid}(armor {eff_armor}, dmg_taken {damage_taken_pct}%)",
            attacker.level
        ),
        dodge_bp,
        crit_bp,
        hit_miss_bp,
        parry_bp,
        attack_time_ms,
        damage_taken_pct,
        spell_crit_bp: 0,
        spell_hit_normal: 0,
        spell_hit_crit: 0,
        block_bp,
        block_value,
        attacker_weapon_skill,
        defender_defense_skill,
    });
    let offhand_key = "offhand_swing".to_string();
    readouts.key().delete(&offhand_key);
    if let Some(weapon) = crate::combat::equipped_offhand_weapon_damage(ctx, attacker_guid) {
        let (base_min, base_max) = crate::combat::offhand_swing_range(ctx, &attacker, weapon);
        readouts.insert(DebugReadout {
            key: offhand_key,
            base_min,
            base_max,
            mitigation_pct: 0,
            final_min: 0,
            final_max: 0,
            note: String::new(),
            dodge_bp: 0,
            crit_bp: 0,
            hit_miss_bp: 0,
            parry_bp: 0,
            attack_time_ms: 0,
            spell_crit_bp: 0,
            spell_hit_normal: 0,
            spell_hit_crit: 0,
            damage_taken_pct: 0,
            block_bp: 0,
            block_value: 0,
            attacker_weapon_skill: 0,
            defender_defense_skill: 0,
        });
    }
}

#[reducer]
pub fn debug_compute_spell(
    ctx: &ReducerContext,
    caster_guid: u64,
    target_guid: u64,
    spell_id: u32,
) {
    let entities = ctx.db.game_world_entity();
    let (Some(caster), Some(target)) = (
        entities.guid().find(caster_guid),
        entities.guid().find(target_guid),
    ) else {
        return;
    };
    let Some(hdr) = ctx.db.game_spell().spell_id().find(spell_id) else {
        return;
    };
    let spell_miss_bp = crate::spell::spell_miss_chance_bp(caster.level, target.level);
    let Some(eff) = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .find(|e| e.kind == crate::spell::E_DAMAGE)
    else {
        return;
    };
    let sp = crate::spell::spell_power(ctx, caster_guid, hdr.school_mask);
    let scaled =
        crate::spell::compose_magnitude(eff.base_points, sp, crate::spell::SPELL_POWER_COEFF_PCT);
    let normal =
        crate::spell::apply_resistance(ctx, target_guid, caster_guid, hdr.school_mask, scaled);
    let crit_raw = crate::spell::apply_spell_crit(scaled, true);
    let crit =
        crate::spell::apply_resistance(ctx, target_guid, caster_guid, hdr.school_mask, crit_raw);
    let spell_crit_bp = crate::spell::spell_crit_bp(ctx, caster_guid);
    let readouts = ctx.db.game_debug_readout();
    let key = "spell".to_string();
    readouts.key().delete(&key);
    readouts.insert(DebugReadout {
        key,
        base_min: 0,
        base_max: 0,
        mitigation_pct: 0,
        final_min: 0,
        final_max: 0,
        note: format!(
            "caster={caster_guid}(lvl {}) spell={spell_id} '{}' tgt={target_guid}(lvl {}) miss={spell_miss_bp}bp",
            caster.level, hdr.name, target.level
        ),
        dodge_bp: 0,
        crit_bp: 0,
        hit_miss_bp: 0,
        parry_bp: 0,
        attack_time_ms: 0,
        damage_taken_pct: 0,
        spell_crit_bp,
        spell_hit_normal: normal.max(0) as u32,
        spell_hit_crit: crit.max(0) as u32,
        block_bp: 0,
        block_value: 0,
        attacker_weapon_skill: 0,
        defender_defense_skill: 0,
    });
}
