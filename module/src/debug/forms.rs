use spacetimedb::{reducer, ReducerContext, Table};

use crate::form::tables::FormSpell;
use crate::form::{game_form, game_form_spell, Form};
use crate::{game_spell, game_spell_effect};
use lyracore_shared::spell::taxonomy::A_SHAPESHIFT;

// Build-5875 SpellShapeshiftForm.dbc and Spell.dbc values the `spell` import reads for Forms 17-19.
const STANCE_NAMES: [(u32, &str); 3] = [
    (17, "Battle Stance"),
    (18, "Defensive Stance"),
    (19, "Berserker Stance"),
];
const STANCE_ATTRIBUTES: u32 = 0x0905_0010;
const STANCE_CATEGORY: u32 = 47;
const STANCE_COOLDOWN_MS: u32 = 1000;
const ALLOW_AURA_WHILE_DEAD: u32 = 0x0010_0000;
const PASSIVE_ATTRIBUTES: u32 = 0x0005_00D0;
type PassiveEffect = (u8, i32, u8, u8);
const PASSIVES: [(u32, &str, &[PassiveEffect]); 3] = {
    use lyracore_shared::spell::taxonomy::{
        A_MOD_COMBAT, A_MOD_DAMAGE_DONE_PCT, A_MOD_DAMAGE_TAKEN, COMBAT_CRIT, COMBAT_THREAT,
        P_COMBAT_FIELD, P_COMBAT_PERCENT, P_SCHOOL_MASK,
    };
    [
        (
            21156,
            "Battle Stance Passive",
            &[(A_MOD_COMBAT, -20, COMBAT_THREAT, P_COMBAT_FIELD)],
        ),
        (
            7376,
            "Defensive Stance Passive",
            &[
                (A_MOD_DAMAGE_TAKEN, -10, 127, P_SCHOOL_MASK),
                (A_MOD_DAMAGE_DONE_PCT, -10, 127, P_SCHOOL_MASK),
                (A_MOD_COMBAT, 30, COMBAT_THREAT, P_COMBAT_FIELD),
            ],
        ),
        (
            7381,
            "Berserker Stance Passive",
            &[
                (A_MOD_COMBAT, 3, COMBAT_CRIT, P_COMBAT_PERCENT),
                (A_MOD_DAMAGE_TAKEN, 10, 127, P_SCHOOL_MASK),
                (A_MOD_COMBAT, -20, COMBAT_THREAT, P_COMBAT_FIELD),
            ],
        ),
    ]
};

/// Writes Forms 17-19 with their stance spells and passives, as the `spell` import writes them.
#[reducer]
pub fn debug_seed_warrior_forms(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    use lyracore_shared::spell::taxonomy::{CAST_FLAG_NO_AURA_CANCEL, P_FORM, T_SELF};
    let upsert_effect = |effect: crate::SpellEffect| {
        ctx.db.game_spell_effect().id().delete(effect.id);
        ctx.db.game_spell_effect().insert(effect);
    };
    for fact in lyracore_shared::form::FORM_FACTS {
        let Some(&(_, name)) = STANCE_NAMES.iter().find(|(form, _)| *form == fact.form_id) else {
            continue;
        };
        if ctx.db.game_spell().spell_id().find(fact.spell_id).is_none() {
            ctx.db.game_spell().insert(crate::Spell {
                attributes: STANCE_ATTRIBUTES,
                attributes_ex_3: ALLOW_AURA_WHILE_DEAD,
                cast_flags: CAST_FLAG_NO_AURA_CANCEL,
                duration_ms: u32::MAX,
                school_mask: 1,
                cooldown_ms: STANCE_COOLDOWN_MS,
                category: STANCE_CATEGORY,
                category_cooldown_ms: STANCE_COOLDOWN_MS,
                ..crate::Spell::blank(fact.spell_id, name)
            });
        }
        upsert_effect(crate::SpellEffect {
            kind: A_SHAPESHIFT,
            target: T_SELF,
            p0: fact.form_id as i32,
            p0_kind: P_FORM,
            ..crate::SpellEffect::blank(fact.spell_id, 0)
        });
        ctx.db.game_form().form_id().delete(fact.form_id);
        ctx.db.game_form().insert(Form {
            form_id: fact.form_id,
            name: name.to_string(),
            spell_id: fact.spell_id,
            acts_as_shapeshifted: false,
            power_type: fact.power_type,
            power_on_entry: fact.power_on_entry as u8,
            default_for_class: fact.default_for_class,
            client_form: fact.form_id as u8,
        });
        for &spell_id in fact.passives {
            let id = lyracore_package_delta::packed_form_spell_id(fact.form_id, spell_id);
            ctx.db.game_form_spell().id().delete(id);
            ctx.db.game_form_spell().insert(FormSpell {
                id,
                form_id: fact.form_id,
                spell_id,
            });
        }
    }
    for (spell_id, name, effects) in PASSIVES {
        if ctx.db.game_spell().spell_id().find(spell_id).is_none() {
            ctx.db.game_spell().insert(crate::Spell {
                attributes: PASSIVE_ATTRIBUTES,
                duration_ms: u32::MAX,
                gcd_ms: 0,
                school_mask: 1,
                ..crate::Spell::blank(spell_id, name)
            });
        }
        for (index, &(kind, base_points, p0, p0_kind)) in effects.iter().enumerate() {
            upsert_effect(crate::SpellEffect {
                kind,
                base_points,
                target: T_SELF,
                p0: i32::from(p0),
                p0_kind,
                ..crate::SpellEffect::blank(spell_id, index as u8)
            });
        }
    }
    Ok(())
}
