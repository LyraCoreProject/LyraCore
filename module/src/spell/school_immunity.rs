use lyracore_shared::spell::taxonomy::{A_IMMUNITY, P_SCHOOL_MASK};
use spacetimedb::ReducerContext;

use super::control::{
    IMMUNE_TO_HOSTILE_AND_FRIENDLY, SOURCE_AURA_MASK, SPELL_ATTR_EX3_IGNORE_RESTRICTIONS,
    SPELL_ATTR_NO_IMMUNITIES,
};
use crate::{game_creature_template, game_spell, Aura, Spell, WorldEntity};

pub(crate) const SCHOOL_MASK_PHYSICAL: u8 = 1;

// Source aura ids in the low byte of an immunity Aura's p1: docs/schema.md#damage-shields.
const SOURCE_SCHOOL_IMMUNITY: i32 = 39;
const SOURCE_DAMAGE_IMMUNITY: i32 = 40;
// vmangos SPELL_ATTR_EX2_NO_SCHOOL_IMMUNITIES, kept in `attributes_ex_2`.
const SPELL_ATTR_EX2_NO_SCHOOL_IMMUNITIES: u32 = 0x0400_0000;

/// vmangos `Unit::IsImmuneToDamage` without a Spell: true when damage of the school does not land.
pub(crate) fn immune_to_school(ctx: &ReducerContext, unit: &WorldEntity, school_mask: u8) -> bool {
    template_immune_to_school(ctx, unit, school_mask)
        || super::auras_on(ctx, unit.guid).any(|aura| aura_refuses_damage(ctx, &aura, school_mask))
}

/// `immune_to_school` for one Aura, for a caller that already scans the unit's Auras.
pub(crate) fn aura_refuses_damage(ctx: &ReducerContext, aura: &Aura, school_mask: u8) -> bool {
    immunity_aura(ctx, aura, school_mask).is_some_and(|immunity| immunity.refuses_damage())
}

/// vmangos `SpellHitResult` by school: `IsImmuneToSpell`, then `IsImmuneToDamage` for a harmful
/// Spell on another unit. A unit's own Spell on itself skips the template mask and damage immunity.
pub(crate) fn school_refuses_spell(
    ctx: &ReducerContext,
    caster_guid: u64,
    unit: &WorldEntity,
    spell: &Spell,
    harmful: bool,
) -> bool {
    if spell.attributes & SPELL_ATTR_NO_IMMUNITIES != 0
        || spell.attributes_ex_3 & SPELL_ATTR_EX3_IGNORE_RESTRICTIONS != 0
    {
        return false;
    }
    let on_self = unit.guid == caster_guid;
    let skips_school_immunity = spell.attributes_ex_2 & SPELL_ATTR_EX2_NO_SCHOOL_IMMUNITIES != 0;
    (harmful && !on_self && template_immune_to_school(ctx, unit, spell.school_mask))
        || super::auras_on(ctx, unit.guid).any(|aura| {
            immunity_aura(ctx, &aura, spell.school_mask).is_some_and(|immunity| {
                !(skips_school_immunity && immunity.source_aura == SOURCE_SCHOOL_IMMUNITY)
                    && immunity.refuses_spell(harmful, on_self)
            })
        })
}

pub(crate) fn template_immune_to_school(
    ctx: &ReducerContext,
    unit: &WorldEntity,
    school_mask: u8,
) -> bool {
    !unit.is_player()
        && ctx
            .db
            .game_creature_template()
            .entry()
            .find(unit.entry)
            .is_some_and(|template| template.school_immune_mask & u32::from(school_mask) != 0)
}

struct ImmunityAura {
    source_aura: i32,
    both_polarities: bool,
    // None when the source Spell is missing.
    source_negative: Option<bool>,
}

impl ImmunityAura {
    // A school immunity refuses damage only from a helpful or missing source Spell, or with 0x100.
    fn refuses_damage(&self) -> bool {
        self.source_aura != SOURCE_SCHOOL_IMMUNITY
            || self.both_polarities
            || self.source_negative != Some(true)
    }

    // vmangos `IsImmuneToSpell`: other polarity; damage immunity: a harmful Spell on another unit.
    fn refuses_spell(&self, harmful: bool, on_self: bool) -> bool {
        match self.source_aura {
            SOURCE_SCHOOL_IMMUNITY => self.both_polarities || self.source_negative != Some(harmful),
            SOURCE_DAMAGE_IMMUNITY => harmful && !on_self,
            _ => false,
        }
    }
}

fn immunity_aura(ctx: &ReducerContext, aura: &Aura, school_mask: u8) -> Option<ImmunityAura> {
    (aura.eff_kind == A_IMMUNITY
        && aura.eff_p0_kind == P_SCHOOL_MASK
        && aura.eff_p0 & i32::from(school_mask) != 0
        && !super::is_due_for_expiry(aura.eff_kind, aura.expires_at, crate::time::now(ctx)))
    .then(|| ImmunityAura {
        source_aura: aura.eff_p1 & SOURCE_AURA_MASK,
        both_polarities: aura.eff_p1 & IMMUNE_TO_HOSTILE_AND_FRIENDLY != 0,
        source_negative: ctx
            .db
            .game_spell()
            .spell_id()
            .find(aura.spell_id)
            .map(|spell| spell.negative()),
    })
}
