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
    use lyracore_shared::spell::taxonomy::CAST_FLAG_NO_AURA_CANCEL;
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
        seed_form(ctx, fact, name, false, false);
    }
    seed_passives(ctx, &PASSIVES);
    Ok(())
}

// Build-5875 SpellShapeshiftForm.dbc names and flags and Spell.dbc values the `spell` import reads
// for Forms 1, 5 and 8.
const DRUID_FORMS: [(u32, &str, bool); 3] = [
    (1, "Cat Form", true),
    (5, "Bear Form", false),
    (8, "Dire Bear Form", false),
];
const DRUID_FORM_ATTRIBUTES: u32 = 0x0005_0010;
const DRUID_FORM_GCD_CATEGORY: u32 = 133;
const DRUID_PASSIVES: [(u32, &str, &[PassiveEffect]); 4] = {
    use lyracore_shared::spell::taxonomy::{
        A_MOD_COMBAT, COMBAT_ATTACK_POWER, COMBAT_THREAT, P_COMBAT_FIELD,
    };
    [
        (
            3025,
            "Cat Form (Passive)",
            &[
                (A_MOD_COMBAT, 40, COMBAT_ATTACK_POWER, P_COMBAT_FIELD),
                (A_MOD_COMBAT, -29, COMBAT_THREAT, P_COMBAT_FIELD),
            ],
        ),
        (
            1178,
            "Bear Form (Passive)",
            &[(A_MOD_COMBAT, 30, COMBAT_ATTACK_POWER, P_COMBAT_FIELD)],
        ),
        (
            9635,
            "Dire Bear Form (Passive)",
            &[(A_MOD_COMBAT, 120, COMBAT_ATTACK_POWER, P_COMBAT_FIELD)],
        ),
        (
            21178,
            "Bear Form (Passive2)",
            &[(A_MOD_COMBAT, 30, COMBAT_THREAT, P_COMBAT_FIELD)],
        ),
    ]
};

/// Writes Forms 1, 5 and 8 with their form spells, as the `spell` import writes them, passives
/// that carry only their attack power and threat effects, and Leader of the Pack and the Heart of
/// the Wild effects.
#[reducer]
pub fn debug_seed_druid_forms(ctx: &ReducerContext) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    for fact in lyracore_shared::form::FORM_FACTS {
        let Some(&(_, name, agility)) = DRUID_FORMS.iter().find(|(form, ..)| *form == fact.form_id)
        else {
            continue;
        };
        if ctx.db.game_spell().spell_id().find(fact.spell_id).is_none() {
            ctx.db.game_spell().insert(crate::Spell {
                attributes: DRUID_FORM_ATTRIBUTES,
                duration_ms: u32::MAX,
                school_mask: 1,
                gcd_category: DRUID_FORM_GCD_CATEGORY,
                ..crate::Spell::blank(fact.spell_id, name)
            });
        }
        seed_form(ctx, fact, name, true, agility);
    }
    seed_passives(ctx, &DRUID_PASSIVES);
    seed_talent_passives(ctx);
    Ok(())
}

// Build-5875 Spell.dbc values the `spell` import reads for Leader of the Pack 24932 and the Heart of
// the Wild effects 24900 and 24899: `Stances`, attributes, and the one effect with its base points.
const TALENT_PASSIVES: [(u32, &str, u32, u32, PassiveEffect); 3] = {
    use lyracore_shared::spell::taxonomy::{
        A_MOD_COMBAT, A_MOD_STAT_PCT, COMBAT_CRIT, P_COMBAT_PERCENT, P_STAT_ID, STAT_STA, STAT_STR,
    };
    [
        (
            24932,
            "Leader of the Pack",
            0x91,
            0,
            (A_MOD_COMBAT, 3, COMBAT_CRIT, P_COMBAT_PERCENT),
        ),
        (
            24900,
            "Heart of the Wild Cat Effect",
            0x1,
            0x190,
            (A_MOD_STAT_PCT, 0, STAT_STR, P_STAT_ID),
        ),
        (
            24899,
            "Heart of the Wild Bear Effect",
            0x90,
            0x190,
            (A_MOD_STAT_PCT, 0, STAT_STA, P_STAT_ID),
        ),
    ]
};
// Spell.dbc radius index 11 of Leader of the Pack's party Aura.
const LEADER_OF_THE_PACK_RADIUS_YD: f32 = 45.0;

fn seed_talent_passives(ctx: &ReducerContext) {
    use lyracore_shared::spell::taxonomy::{CAST_FLAG_AREA_AURA, T_SELF};
    for (spell_id, name, stances, attributes, (kind, base_points, p0, p0_kind)) in TALENT_PASSIVES {
        let area = spell_id == 24932;
        if ctx.db.game_spell().spell_id().find(spell_id).is_none() {
            ctx.db.game_spell().insert(crate::Spell {
                attributes,
                cast_flags: if area { CAST_FLAG_AREA_AURA } else { 0 },
                duration_ms: u32::MAX,
                gcd_ms: 0,
                school_mask: 1,
                form_mask: stances,
                ..crate::Spell::blank(spell_id, name)
            });
            crate::spell::write_dbc_form_set(ctx, spell_id, stances, 0);
        }
        upsert_effect(
            ctx,
            crate::SpellEffect {
                kind,
                base_points,
                target: T_SELF,
                radius_yd: if area {
                    LEADER_OF_THE_PACK_RADIUS_YD
                } else {
                    0.0
                },
                p0: i32::from(p0),
                p0_kind,
                ..crate::SpellEffect::blank(spell_id, 0)
            },
        );
    }
}

fn upsert_effect(ctx: &ReducerContext, effect: crate::SpellEffect) {
    ctx.db.game_spell_effect().id().delete(effect.id);
    ctx.db.game_spell_effect().insert(effect);
}

fn seed_form(
    ctx: &ReducerContext,
    fact: &lyracore_shared::form::FormFact,
    name: &str,
    acts_as_shapeshifted: bool,
    attack_power_from_agility: bool,
) {
    use lyracore_shared::spell::taxonomy::{P_FORM, T_SELF};
    upsert_effect(
        ctx,
        crate::SpellEffect {
            kind: A_SHAPESHIFT,
            target: T_SELF,
            p0: fact.form_id as i32,
            p0_kind: P_FORM,
            ..crate::SpellEffect::blank(fact.spell_id, 0)
        },
    );
    ctx.db.game_form().form_id().delete(fact.form_id);
    ctx.db.game_form().insert(Form {
        form_id: fact.form_id,
        name: name.to_string(),
        spell_id: fact.spell_id,
        acts_as_shapeshifted,
        power_type: fact.power_type,
        power_on_entry: fact.power_on_entry as u8,
        default_for_class: fact.default_for_class,
        client_form: fact.form_id as u8,
        natural_attack_ms: fact.natural_attack_ms,
        attack_power_from_agility,
        clears_snares: fact.clears_snares,
        entry_chance_spell: fact.entry_chance_spell,
        blocks_mounting: fact.blocks_mounting,
        alliance_display_id: fact.alliance_display_id,
        horde_display_id: fact.horde_display_id,
        display_scale: fact.display_scale,
    });
    for passive in fact.passives {
        let id = lyracore_package_delta::packed_form_spell_id(fact.form_id, passive.spell_id);
        ctx.db.game_form_spell().id().delete(id);
        ctx.db.game_form_spell().insert(FormSpell {
            id,
            form_id: fact.form_id,
            spell_id: passive.spell_id,
            required_spell: passive.required_spell,
            amount_spell: passive.amount_spell,
        });
    }
}

fn seed_passives(ctx: &ReducerContext, passives: &[(u32, &str, &[PassiveEffect])]) {
    use lyracore_shared::spell::taxonomy::T_SELF;
    for &(spell_id, name, effects) in passives {
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
            upsert_effect(
                ctx,
                crate::SpellEffect {
                    kind,
                    base_points,
                    target: T_SELF,
                    p0: i32::from(p0),
                    p0_kind,
                    ..crate::SpellEffect::blank(spell_id, index as u8)
                },
            );
        }
    }
}

/// Sets a spell's DBC Form masks and the Form Set rows they import as.
#[reducer]
pub fn debug_set_spell_form_masks(
    ctx: &ReducerContext,
    spell_id: u32,
    required: u32,
    excluded: u32,
) -> Result<(), String> {
    let mut spell = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .ok_or_else(|| format!("no spell {spell_id}"))?;
    spell.form_mask = required;
    spell.form_exclude = excluded;
    ctx.db.game_spell().spell_id().update(spell);
    crate::spell::write_dbc_form_set(ctx, spell_id, required, excluded);
    Ok(())
}
