use lyracore_shared::spell::taxonomy::{
    A_DAMAGE_SHIELD, A_IMMUNITY, A_MOD_DAMAGE_TAKEN_FLAT, CAST_FLAG_MAGIC_HIT, P_SCHOOL_MASK,
};
use spacetimedb::ReducerContext;

use crate::{game_spell, game_spell_effect, game_world_entity};

// docs/schema.md#damage-shields gives the importer's A_IMMUNITY p1 encoding.
const IMMUNITY_SOURCE_AURA_MASK: i32 = 0xff;
const SOURCE_SCHOOL_IMMUNITY: i32 = 39;
const SOURCE_DAMAGE_IMMUNITY: i32 = 40;
const IMMUNE_TO_HOSTILE_AND_FRIENDLY: i32 = 0x100;
// SMSG_SPELLLOGMISS reasons.
const MISS_RESIST: u8 = 2;
const MISS_EVADE: u8 = 6;
const MISS_IMMUNE: u8 = 7;

pub(crate) fn reflect(ctx: &ReducerContext, attacker_guid: u64, bearer_guid: u64) {
    if attacker_guid == bearer_guid {
        return;
    }
    let mut reflected = std::collections::BTreeSet::new();
    while let Some(shield) = crate::spell::auras_on(ctx, bearer_guid).find(|aura| {
        aura.eff_kind == A_DAMAGE_SHIELD
            && !reflected.contains(&aura.id)
            && !crate::spell::is_due_for_expiry(
                aura.eff_kind,
                aura.expires_at,
                crate::time::now(ctx),
            )
    }) {
        reflected.insert(shield.id);
        let (Some(bearer), Some(attacker), Some(spell), Some(effect)) = (
            ctx.db.game_world_entity().guid().find(bearer_guid),
            ctx.db.game_world_entity().guid().find(attacker_guid),
            ctx.db.game_spell().spell_id().find(shield.spell_id),
            ctx.db.game_spell_effect().id().find(shield.effect_id),
        ) else {
            continue;
        };
        let mut spell_immune = false;
        let mut damage_immune = false;
        for immunity in crate::spell::auras_on(ctx, attacker_guid).filter(|aura| {
            aura.eff_kind == A_IMMUNITY
                && aura.eff_p0_kind == P_SCHOOL_MASK
                && aura.eff_p0 & i32::from(spell.school_mask) != 0
                && !crate::spell::is_due_for_expiry(
                    aura.eff_kind,
                    aura.expires_at,
                    crate::time::now(ctx),
                )
        }) {
            let source = ctx.db.game_spell().spell_id().find(immunity.spell_id);
            let both_polarities = immunity.eff_p1 & IMMUNE_TO_HOSTILE_AND_FRIENDLY != 0;
            let source_aura = immunity.eff_p1 & IMMUNITY_SOURCE_AURA_MASK;
            // vmangos IsImmuneToDamage: school immunity needs a helpful or absent Spell, or 0x100.
            damage_immune |= source_aura != SOURCE_SCHOOL_IMMUNITY
                || both_polarities
                || source.as_ref().is_none_or(|source| !source.negative());
            let blocks_spell = both_polarities
                || source.is_none_or(|source| source.negative() != spell.negative());
            spell_immune |= (source_aura == SOURCE_SCHOOL_IMMUNITY && blocks_spell)
                || (source_aura == SOURCE_DAMAGE_IMMUNITY && spell.negative());
        }
        let miss = if !attacker.is_player()
            && crate::creatures::eventai::movement::returning_home(ctx, attacker_guid)
        {
            Some(MISS_EVADE)
        } else if spell_immune {
            Some(MISS_IMMUNE)
        } else if spell.negative()
            && spell.cast_flags & CAST_FLAG_MAGIC_HIT != 0
            && crate::spell::cast::spell_misses(ctx, bearer.level as u8, attacker_guid, &spell)
        {
            Some(MISS_RESIST)
        } else {
            None
        };
        if let Some(miss) = miss {
            crate::native::combat::damage_shield_miss(
                ctx,
                bearer_guid,
                attacker_guid,
                spell.spell_id,
                miss,
            );
            continue;
        }
        if damage_immune {
            crate::native::combat::damage_shield_immune(
                ctx,
                bearer_guid,
                attacker_guid,
                spell.spell_id,
            );
            continue;
        }
        let points = crate::modifiers::stacked_amount(shield.amount, shield.stacks);
        let mut damage = crate::spell::bonuses::spell_damage_done_float(
            ctx,
            &spell,
            &effect,
            bearer_guid,
            points as f32,
            false,
        );
        if !bearer.is_player() {
            let flat_taken: i32 = crate::spell::auras_on(ctx, attacker_guid)
                .filter(|aura| {
                    aura.eff_kind == A_MOD_DAMAGE_TAKEN_FLAT
                        && aura.eff_p0 & i32::from(spell.school_mask) != 0
                })
                .map(|aura| crate::modifiers::stacked_amount(aura.amount, aura.stacks))
                .sum();
            damage += flat_taken.max(0) as f32;
        }
        let damage = crate::spell::bonuses::round_damage(ctx, attacker_guid, damage).max(0) as u32;
        let damage = if attacker.dead || attacker.godmode || attacker.is_flying() {
            0
        } else {
            damage
        };
        let damage = super::final_damage(ctx, attacker_guid, damage);
        crate::native::combat::damage_shield(
            ctx,
            bearer_guid,
            attacker_guid,
            damage.amount,
            crate::spell::school_index(spell.school_mask),
        );
        crate::spell::break_stealth(ctx, bearer_guid);
        super::apply_hit(
            ctx,
            bearer_guid,
            attacker_guid,
            damage,
            super::Hit::spell(super::HitSource::DamageShield, spell.spell_id, false),
        );
    }
}
