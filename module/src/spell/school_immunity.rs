use lyracore_shared::spell::taxonomy::{A_IMMUNITY, P_SCHOOL_MASK};
use spacetimedb::ReducerContext;

use super::control::{IMMUNE_TO_HOSTILE_AND_FRIENDLY, SOURCE_AURA_MASK};
use crate::{game_creature_template, game_spell, Spell, WorldEntity};

pub(crate) const SCHOOL_MASK_PHYSICAL: u8 = 1;

// Source aura ids in the low byte of an immunity Aura's p1: docs/schema.md#damage-shields.
const SOURCE_SCHOOL_IMMUNITY: i32 = 39;
const SOURCE_DAMAGE_IMMUNITY: i32 = 40;

/// vmangos `Unit::IsImmuneToDamage` without a Spell, as a white swing or a damage shield asks it.
pub(crate) fn immune_to_school(ctx: &ReducerContext, unit: &WorldEntity, school_mask: u8) -> bool {
    template_immune_to_school(ctx, unit, school_mask)
        || immunity_auras(ctx, unit.guid, school_mask).any(|aura| aura.refuses_damage())
}

/// vmangos `IsImmuneToSpell` by school, for a Spell that is not the unit's own.
pub(crate) fn school_refuses_spell(
    ctx: &ReducerContext,
    unit: &WorldEntity,
    spell: &Spell,
) -> bool {
    (spell.negative() && template_immune_to_school(ctx, unit, spell.school_mask))
        || immunity_auras(ctx, unit.guid, spell.school_mask)
            .any(|aura| aura.refuses_spell(spell.negative()))
}

fn template_immune_to_school(ctx: &ReducerContext, unit: &WorldEntity, school_mask: u8) -> bool {
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

    // vmangos `IsImmuneToSchool` refuses a Spell of the other polarity; damage immunity a harmful one.
    fn refuses_spell(&self, negative: bool) -> bool {
        match self.source_aura {
            SOURCE_SCHOOL_IMMUNITY => {
                self.both_polarities || self.source_negative != Some(negative)
            }
            SOURCE_DAMAGE_IMMUNITY => negative,
            _ => false,
        }
    }
}

fn immunity_auras(
    ctx: &ReducerContext,
    unit_guid: u64,
    school_mask: u8,
) -> impl Iterator<Item = ImmunityAura> + '_ {
    let now = crate::time::now(ctx);
    super::auras_on(ctx, unit_guid)
        .filter(move |aura| {
            aura.eff_kind == A_IMMUNITY
                && aura.eff_p0_kind == P_SCHOOL_MASK
                && aura.eff_p0 & i32::from(school_mask) != 0
                && !super::is_due_for_expiry(aura.eff_kind, aura.expires_at, now)
        })
        .map(move |aura| ImmunityAura {
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
