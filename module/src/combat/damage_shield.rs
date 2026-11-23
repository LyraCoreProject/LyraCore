use lyracore_shared::spell::taxonomy::{
    A_DAMAGE_SHIELD, A_MOD_DAMAGE_TAKEN_FLAT, CAST_FLAG_MAGIC_HIT,
};
use spacetimedb::ReducerContext;

use crate::{game_spell, game_spell_effect, game_world_entity};

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
        let miss = if !attacker.is_player()
            && crate::creatures::eventai::movement::returning_home(ctx, attacker_guid)
        {
            Some(MISS_EVADE)
        } else if crate::spell::school_refuses_spell(
            ctx,
            bearer_guid,
            &attacker,
            &spell,
            spell.negative(),
        ) {
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
            crate::native::combat::spell_log_miss(
                ctx,
                bearer_guid,
                attacker_guid,
                spell.spell_id,
                miss,
            );
            continue;
        }
        if crate::spell::immune_to_school(ctx, &attacker, spell.school_mask) {
            crate::native::combat::spell_or_damage_immune(
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
