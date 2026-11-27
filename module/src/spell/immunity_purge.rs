//! vmangos `SPELL_ATTR_EX_IMMUNITY_PURGES_EFFECT`: an immunity Aura that lands removes the Auras
//! it makes its unit immune to. See docs/schema.md#immunity-purge.

use std::collections::HashSet;

use lyracore_shared::spell::taxonomy::{
    A_CONTROL, A_FLAG, A_IMMUNITY, M_FEAR, M_POLY, M_STUN, P_MECHANIC, P_RAW, P_SCHOOL_MASK,
};
use spacetimedb::ReducerContext;

use super::control::{is_mechanic_immunity_mask, SOURCE_AURA_MASK, SPELL_ATTR_NO_IMMUNITIES};
use super::school_immunity::SOURCE_SCHOOL_IMMUNITY;
use super::{auras_on, is_action_blocked, shed_auras};
use crate::{game_spell, game_spell_effect, Aura, Spell, SpellEffect};

const SPELL_ATTR_EX_IMMUNITY_PURGES_EFFECT: u32 = 0x8000;
const SOURCE_STATE_IMMUNITY: i32 = 38;
const SOURCE_DISPEL_IMMUNITY: i32 = 41;
const DISPEL_ALL: i32 = 7;
// vmangos DISPEL_ALL_MASK: Magic, Curse, Disease and Poison.
const DISPEL_ALL_MASK: u32 = 0b1_1110;

enum Purge {
    Mechanics(u32),
    AuraType(i32),
    DispelTypes(u32),
    Schools(u8),
}

impl Purge {
    // A school immunity purges only from a helpful Spell.
    fn of(spell: &Spell, effect: &SpellEffect) -> Option<Self> {
        Self::from_effect(effect)
            .filter(|purge| !matches!(purge, Self::Schools(_)) || !spell.negative())
    }

    fn from_effect(effect: &SpellEffect) -> Option<Self> {
        match (effect.kind, effect.p0_kind, effect.p1 & SOURCE_AURA_MASK) {
            (A_IMMUNITY, P_MECHANIC, _) if is_mechanic_immunity_mask(effect.p1) => {
                Some(Self::Mechanics(effect.p0 as u32))
            }
            (A_IMMUNITY, P_MECHANIC, _) => Some(Self::Mechanics(mechanic_bit(effect.p0))),
            (A_IMMUNITY, P_RAW, SOURCE_STATE_IMMUNITY) => Some(Self::AuraType(effect.p0)),
            (A_IMMUNITY, P_SCHOOL_MASK, SOURCE_SCHOOL_IMMUNITY) => {
                Some(Self::Schools(effect.p0 as u8))
            }
            (A_FLAG, _, SOURCE_DISPEL_IMMUNITY) => dispel_mask(effect.p0).map(Self::DispelTypes),
            _ => None,
        }
    }

    fn takes(&self, ctx: &ReducerContext, aura: &Aura, spell: &Spell) -> bool {
        let unaffected = spell.attributes & SPELL_ATTR_NO_IMMUNITIES != 0;
        match *self {
            Self::Mechanics(mask) => !unaffected && mechanics_of(ctx, aura, spell) & mask != 0,
            Self::Schools(mask) => !unaffected && spell.school_mask & mask != 0 && spell.negative(),
            Self::AuraType(source_aura) => {
                matches!(aura.eff_kind, A_CONTROL | A_FLAG | A_IMMUNITY)
                    && aura.eff_p1 & SOURCE_AURA_MASK == source_aura
            }
            Self::DispelTypes(mask) => dispel_in(mask, spell),
        }
    }
}

fn dispel_in(mask: u32, spell: &Spell) -> bool {
    1u32.checked_shl(u32::from(spell.dispel_type)).unwrap_or(0) & mask != 0
}

fn mechanic_bit(mechanic: i32) -> u32 {
    if (1..=32).contains(&mechanic) {
        1 << (mechanic - 1)
    } else {
        0
    }
}

fn dispel_mask(dispel_type: i32) -> Option<u32> {
    match dispel_type {
        DISPEL_ALL => Some(DISPEL_ALL_MASK),
        1..=31 => Some(1 << dispel_type),
        _ => None,
    }
}

// vmangos `SpellAuraHolder::HasMechanicMask`: the Spell's mechanic or the Aura effect's own.
fn mechanics_of(ctx: &ReducerContext, aura: &Aura, spell: &Spell) -> u32 {
    let effect_mechanic = ctx
        .db
        .game_spell_effect()
        .id()
        .find(aura.effect_id)
        .map_or(0, |effect| effect.effect_mechanic);
    mechanic_bit(i32::from(spell.mechanic)) | mechanic_bit(i32::from(effect_mechanic))
}

/// Removes the Auras that the immunity Aura of `effect` covers, with every Aura of their Spells.
/// The immunity's own Spell stays.
pub(crate) fn purge_on_land(
    ctx: &ReducerContext,
    spell: &Spell,
    effect: &SpellEffect,
    target_guid: u64,
) {
    if spell.attributes_ex & SPELL_ATTR_EX_IMMUNITY_PURGES_EFFECT == 0 {
        return;
    }
    let Some(purge) = Purge::of(spell, effect) else {
        return;
    };
    let spells = ctx.db.game_spell();
    let held: Vec<Aura> = auras_on(ctx, target_guid)
        .filter(|aura| aura.spell_id != spell.spell_id)
        .collect();
    let mut purged = HashSet::new();
    for aura in &held {
        if !purged.contains(&aura.spell_id)
            && spells
                .spell_id()
                .find(aura.spell_id)
                .is_some_and(|held_spell| purge.takes(ctx, aura, &held_spell))
        {
            purged.insert(aura.spell_id);
        }
    }
    shed_auras(
        ctx,
        held.into_iter()
            .filter(|aura| purged.contains(&aura.spell_id))
            .collect(),
    );
}

// vmangos CONFUSED_MECHANIC_MASK: disoriented and polymorph.
const CONFUSED_MECHANIC_MASK: u32 = (1 << 1) | (1 << 16);
const MECHANIC_FEAR_BIT: u32 = 1 << 4;
const MECHANIC_STUN_BIT: u32 = 1 << 11;

/// What a purging Spell makes its caster immune to, for `caster_is_blocked`.
#[derive(Default)]
struct Covered {
    schools: u8,
    mechanics: u32,
    dispel_types: u32,
}

impl Covered {
    fn of(spell: &Spell, effects: &[SpellEffect]) -> Self {
        let mut covered = Self::default();
        if spell.attributes_ex & SPELL_ATTR_EX_IMMUNITY_PURGES_EFFECT == 0 {
            return covered;
        }
        for purge in effects.iter().filter_map(Purge::from_effect) {
            match purge {
                Purge::Mechanics(mask) => covered.mechanics |= mask,
                Purge::Schools(mask) => covered.schools |= mask,
                Purge::DispelTypes(mask) => covered.dispel_types |= mask,
                Purge::AuraType(_) => {}
            }
        }
        covered
    }

    fn is_empty(&self) -> bool {
        self.schools == 0 && self.mechanics == 0 && self.dispel_types == 0
    }

    fn covers(&self, ctx: &ReducerContext, aura: &Aura, held: &Spell) -> bool {
        let state_mechanics = match aura.eff_p0 {
            M_STUN => MECHANIC_STUN_BIT,
            M_FEAR => MECHANIC_FEAR_BIT,
            _ => CONFUSED_MECHANIC_MASK,
        };
        held.school_mask & self.schools != 0
            || dispel_in(self.dispel_types, held)
            || (mechanics_of(ctx, aura, held) | state_mechanics) & self.mechanics != 0
    }
}

/// vmangos `Spell::CheckCasterAuras`: a stun, fear or polymorph stops a cast, unless the Spell
/// purges and its immunity covers every Aura that causes the state.
pub(crate) fn caster_is_blocked(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell: &Spell,
    effects: &[SpellEffect],
) -> bool {
    if !is_action_blocked(ctx, caster_guid) {
        return false;
    }
    let covered = Covered::of(spell, effects);
    if covered.is_empty() {
        return true;
    }
    let spells = ctx.db.game_spell();
    auras_on(ctx, caster_guid)
        .filter(|aura| {
            aura.eff_kind == A_CONTROL && matches!(aura.eff_p0, M_STUN | M_FEAR | M_POLY)
        })
        .any(|aura| {
            !spells
                .spell_id()
                .find(aura.spell_id)
                .is_some_and(|held| covered.covers(ctx, &aura, &held))
        })
}
