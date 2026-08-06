//! The combat-readback scratchpad table + its two writers: `debug_compute_swing` (melee) and
//! `debug_compute_spell` (spell crit/damage). Both write `game_debug_readout` so the harness can
//! assert combat-stat features via SQL with no live fight/cast.

use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::{game_spell, game_spell_effect, game_world_entity};

/// A read-only scratchpad the combat-readback harness reads via SQL (the keystone that flips
/// combat-stat features from "needs a finicky live kill" to machine-solo). `debug_compute_swing`
/// writes the deterministic swing profile here; the harness asserts on it without a fight. Public so
/// `spacetime sql` can read it; debug-only (the whole module is behind `debug_reducers`). [server]
#[table(accessor = game_debug_readout, public)]
pub struct DebugReadout {
    #[primary_key]
    pub key: String, // a single "swing" slot today; keyed so future readouts can coexist
    pub base_min: u32,
    pub base_max: u32,
    pub mitigation_pct: u32,
    pub final_min: u32,
    pub final_max: u32,
    pub note: String,
    /// The target's chance (basis points) to dodge the attacker's swing, from its EFFECTIVE agility
    /// (base + any A_MOD_STAT(AGI) buff) — the avoidance readback. END-appended + defaulted so adding
    /// it auto-migrates the existing row.
    #[default(0)]
    pub dodge_bp: u32,
    /// The attacker's EFFECTIVE crit chance (basis points): `CRIT_BP` + any A_MOD_COMBAT(CRIT) aura —
    /// the crit-rating readback. END-appended + defaulted so adding it auto-migrates the existing row.
    #[default(0)]
    pub crit_bp: u32,
    /// The attacker's EFFECTIVE miss chance (basis points) vs the target: level-derived `miss_chance_bp`
    /// MINUS any A_MOD_COMBAT(HIT) aura (hit rating reduces miss) — the hit-rating readback. END-appended
    /// + defaulted so adding it auto-migrates the existing row.
    #[default(0)]
    pub hit_miss_bp: u32,
    /// The target's chance (basis points) to PARRY the attacker's swing: `PARRY_BP` floor plus the
    /// defense-vs-weapon-skill term (a higher-level defender parries more), agility-independent — the
    /// parry-scaling readback. END-appended + defaulted so adding it auto-migrates the existing row.
    #[default(0)]
    pub parry_bp: u32,
    /// The attacker's EFFECTIVE swing interval (ms): `base_attack_time_ms` adjusted by any
    /// `A_MOD_SPEED(SPEED_SWING)` aura (haste shortens, an attack-speed slow lengthens) — the swing-speed
    /// readback. END-appended + defaulted.
    #[default(0)]
    pub attack_time_ms: u32,
    /// SPELL crit readbacks (written by `debug_compute_spell` to the "spell" row). The caster's spell
    /// crit chance (bp) = `SPELL_CRIT_BP` + any A_MOD_COMBAT(CRIT) aura. END-appended + defaulted.
    #[default(0)]
    pub spell_crit_bp: u32,
    /// The spell's NON-crit damage (post spell-power scaling + magic resistance) — the baseline hit.
    /// END-appended + defaulted.
    #[default(0)]
    pub spell_hit_normal: u32,
    /// The spell's CRIT damage (×1.5 of the scaled base, then magic resistance) — proves the crit fold.
    /// END-appended + defaulted.
    #[default(0)]
    pub spell_hit_crit: u32,
    /// The TARGET's incoming-damage modifier (signed %, A_MOD_DAMAGE_TAKEN) folded into the swing's
    /// final_min/final_max — so a defensive cooldown like Shield Wall (−75) shows the swing landing for
    /// 25% here, with no fight. 0 = no such aura (final_min/max are armor-only). END-appended + defaulted.
    #[default(0)]
    pub damage_taken_pct: i32,
    /// The TARGET's chance (basis points) to BLOCK the attacker's swing: 0 unless it has a shield
    /// equipped, else `block_chance_bp` (the defense-vs-weapon-skill band). The block-chance readback,
    /// the avoidance/mitigation twin of `parry_bp`. END-appended + defaulted (auto-migrates). [server]
    #[default(0)]
    pub block_bp: u32,
    /// The TARGET's flat shield BLOCK VALUE (the damage a blocked swing absorbs): the shield's base
    /// `block_value` + Str/20, or 0 if unshielded — so a shield's effect is verifiable with no fight.
    /// END-appended + defaulted (auto-migrates). [server]
    #[default(0)]
    pub block_value: u32,
    /// The ATTACKER's effective weapon skill (equipped weapon's line, else level*5) and the TARGET's
    /// effective defense skill (else level*5). Their difference drives the miss/dodge/parry/block bands —
    /// the skill readback, so a below-cap skill set via `debug_set_skill` is verifiable with no fight.
    /// END-appended + defaulted (auto-migrates). [server]
    #[default(0)]
    pub attacker_weapon_skill: u32,
    #[default(0)]
    pub defender_defense_skill: u32,
}

/// Compute the SERVER's swing profile for `attacker` vs `target` (the exact `swing_range_ctx` +
/// `armor_mitigation_pct` live combat uses) and write it to `game_debug_readout` — so combat-stat
/// features (functional armor, weapon damage, …) are verifiable via SQL, with NO live kill. Keyed by
/// "swing" (each call overwrites). No-op if either guid is absent.
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
    // EFFECTIVE armor (base + any A_MOD_RESISTANCE(armor) aura) — the exact value live combat mitigates
    // against, so an armor buff shows up in mitigation_pct here without a fight.
    let eff_armor = crate::combat::effective_armor(ctx, &target);
    let p = crate::combat::swing_profile(bmin, bmax, eff_armor, attacker.level);
    // EFFECTIVE crit/miss (base + any A_MOD_COMBAT(CRIT)/(HIT) aura on the attacker) — the exact bands
    // roll_swing builds the attack table from, so a crit/hit-rating buff shows up here without a fight.
    let crit_bp = crate::combat::effective_crit_bp(ctx, &attacker);
    // The skill difference (defender defense skill − attacker weapon skill) feeding the skill-based bands,
    // exactly as roll_swing computes it; plus the raw weapon/defense skill values for the readback. The
    // defense skill now folds A_MOD_COMBAT(COMBAT_DEFENSE) auras (Anticipation), so it shifts sd here.
    let sd = crate::skill::skill_diff_ctx(ctx, &attacker, &target);
    let attacker_weapon_skill = crate::skill::effective_weapon_skill(ctx, &attacker);
    let defender_defense_skill = crate::skill::effective_defense_skill(ctx, &target);
    let hit_miss_bp = crate::combat::effective_miss_bp(ctx, &attacker, sd);
    // Defender dodge/parry/block bands — the EXACT effective bands roll_swing uses (the agility/skill base
    // PLUS any A_MOD_COMBAT(DODGE/PARRY/BLOCK) talent aura), so a Deflection/Shield-Spec buff shows up here
    // without a fight. Block is shielded-only.
    let dodge_bp = crate::combat::effective_dodge_bp(ctx, &target, sd);
    let parry_bp = crate::combat::effective_parry_bp(ctx, &target, sd);
    let block_value = crate::combat::effective_block_value(ctx, &target);
    let block_bp = crate::combat::effective_block_bp(ctx, &target, sd, block_value);
    // Effective swing interval (ms): base attack time shortened by any melee-haste aura on the attacker.
    let attack_time_ms = crate::combat::effective_swing_time(ctx, &attacker);
    // Outgoing-damage modifier on the ATTACKER (stance + A_MOD_COMBAT(COMBAT_DMG_DONE) — e.g. Curse of
    // Weakness) folded into the armor-mitigated range FIRST, exactly as resolve_swing/apply_target_damage
    // apply it, so a cursed attacker's swing readout shows the reduced hit — server-verifiable with no fight.
    let damage_done_pct = crate::spell::stance_damage_done_pct(attacker.stance)
        + crate::spell::combat_field_bonus(ctx, attacker_guid, crate::spell::COMBAT_DMG_DONE);
    let outgoing_min = crate::spell::apply_damage_pct(p.final_min, damage_done_pct);
    let outgoing_max = crate::spell::apply_damage_pct(p.final_max, damage_done_pct);
    // Incoming-damage modifier on the TARGET (A_MOD_DAMAGE_TAKEN — e.g. Shield Wall −75%) folded in NEXT,
    // exactly as live combat applies it after armor (combat tick_melee). So a shielded target's swing
    // readout shows the reduced hit — server-verifiable with no fight.
    let damage_taken_pct = crate::spell::damage_taken_bonus(ctx, target_guid);
    let final_min = crate::spell::apply_damage_pct(outgoing_min, damage_taken_pct);
    let final_max = crate::spell::apply_damage_pct(outgoing_max, damage_taken_pct);
    let readouts = ctx.db.game_debug_readout();
    let key = "swing".to_string();
    readouts.key().delete(&key); // upsert
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
        spell_crit_bp: 0, // melee row — spell fields unused
        spell_hit_normal: 0,
        spell_hit_crit: 0,
        block_bp,
        block_value,
        attacker_weapon_skill,
        defender_defense_skill,
    });
}

/// Compute the SERVER's SPELL hit profile for `caster` casting `spell_id` at `target` and write it to
/// `game_debug_readout` (key "spell") — so spell crit is verifiable via SQL with NO live cast/RNG. Reports
/// the caster's spell crit chance + the NORMAL and CRIT (×1.5) damage, both through the exact
/// spell-power-scale + magic-resistance path `apply_effect`'s E_DAMAGE arm uses (die variance excluded —
/// the readout uses the effect's authored `base_points` so the crit ratio is exact). Uses the spell's
/// FIRST E_DAMAGE effect. No-op if a guid / the spell / a damage effect is absent. Each call overwrites.
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
    // Binary spell-miss chance for this caster→target level pair — surfaced in the note so it's
    // server-verifiable via SQL with no live cast (no readout column needed).
    let spell_miss_bp = crate::spell::spell_miss_chance_bp(caster.level, target.level);
    // The spell's first direct-damage (E_DAMAGE) effect — the one the crit fold applies to.
    let Some(eff) = ctx
        .db
        .game_spell_effect()
        .by_spell()
        .filter(&spell_id)
        .find(|e| e.kind == crate::spell::E_DAMAGE)
    else {
        return;
    };
    // The SAME scale → crit → resist pipeline as the live E_DAMAGE arm, minus the random die + crit roll:
    // use the authored base_points so NORMAL vs CRIT is an exact ×1.5 readback.
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
    readouts.key().delete(&key); // upsert
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
        damage_taken_pct: 0, // spell row — the swing-row field
        spell_crit_bp,
        spell_hit_normal: normal.max(0) as u32,
        spell_hit_crit: crit.max(0) as u32,
        block_bp: 0, // spell row — the swing-row fields
        block_value: 0,
        attacker_weapon_skill: 0,
        defender_defense_skill: 0,
    });
}
