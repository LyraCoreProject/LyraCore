use lyracore_shared::spell::taxonomy::{
    A_DAMAGE_SHIELD, A_IMMUNITY, A_MOD_DAMAGE_TAKEN_FLAT, P_SCHOOL_MASK, SPELL_ATTR_MAGIC_HIT,
};
use spacetimedb::ReducerContext;

use crate::{game_aura, game_spell, game_spell_effect, game_world_entity};

pub(crate) fn reflect(ctx: &ReducerContext, attacker_guid: u64, bearer_guid: u64) {
    if attacker_guid == bearer_guid {
        return;
    }
    let shields: Vec<_> = crate::spell::auras_on(ctx, bearer_guid)
        .filter(|aura| {
            aura.eff_kind == A_DAMAGE_SHIELD
                && !crate::spell::is_due_for_expiry(
                    aura.eff_kind,
                    aura.expires_at,
                    crate::time::now(ctx),
                )
        })
        .collect();
    for shield in shields {
        if ctx.db.game_aura().id().find(shield.id).is_none() {
            continue;
        }
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
            damage_immune = true;
            let blocks_spell = ctx
                .db
                .game_spell()
                .spell_id()
                .find(immunity.spell_id)
                .is_none_or(|source| source.negative() != spell.negative())
                || immunity.eff_p1 & 0x100 != 0;
            spell_immune |= (immunity.eff_p1 & 0xff == 39 && blocks_spell)
                || (immunity.eff_p1 & 0xff == 40 && spell.negative());
        }
        let miss = if !attacker.is_player() && crate::creatures::returning_home(ctx, attacker_guid)
        {
            Some(6)
        } else if spell_immune {
            Some(7)
        } else if spell.negative()
            && spell.cast_flags & SPELL_ATTR_MAGIC_HIT != 0
            && crate::spell::cast::spell_misses(ctx, bearer.level as u8, attacker_guid, &spell)
        {
            Some(2)
        } else {
            None
        };
        if let Some(miss) = miss {
            crate::native::damage_shield_miss(
                ctx,
                bearer_guid,
                attacker_guid,
                spell.spell_id,
                miss,
            );
            continue;
        }
        if damage_immune {
            crate::native::damage_shield_immune(ctx, bearer_guid, attacker_guid, spell.spell_id);
            continue;
        }
        let points = lyracore_shared::aura::stacked_amount(shield.amount, shield.stacks);
        let mut damage = crate::spell::bonuses::spell_damage_done_float(
            ctx,
            &spell,
            &effect,
            bearer_guid,
            points,
            false,
        );
        if !bearer.is_player() {
            let flat_taken: i32 = crate::spell::auras_on(ctx, attacker_guid)
                .filter(|aura| {
                    aura.eff_kind == A_MOD_DAMAGE_TAKEN_FLAT
                        && aura.eff_p0 & i32::from(spell.school_mask) != 0
                })
                .map(|aura| lyracore_shared::aura::stacked_amount(aura.amount, aura.stacks))
                .sum();
            damage += flat_taken.max(0) as f32;
        }
        let damage = crate::spell::bonuses::round_damage(ctx, attacker_guid, damage).max(0) as u32;
        let damage = if attacker.dead || attacker.godmode || crate::taxi::is_flying(&attacker) {
            0
        } else {
            damage
        };
        let damage = super::final_damage(ctx, attacker_guid, damage);
        crate::native::damage_shield(
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
