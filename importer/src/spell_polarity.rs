//! Port of vmangos `SpellEntry::IsPositiveSpell` for a caster that targets itself, at d9173eca.
//! `WorldSession::HandleCastSpellOpcode` refuses an explicitly selected unit target that is the
//! caster when this returns false.

use crate::spell_dbc::Spell;
use lyracore_shared::spell_custom::{NEGATIVE, POSITIVE};

const EFFECT_INSTAKILL: i32 = 1;
const EFFECT_DUMMY: i32 = 3;
const EFFECT_APPLY_AURA: i32 = 6;
const EFFECT_HEAL: i32 = 10;
const EFFECT_LEARN_SPELL: i32 = 36;
const EFFECT_DISPEL: i32 = 38;
const EFFECT_SKILL_STEP: i32 = 44;

const SPELLFAMILY_GENERIC: u32 = 0;
const SPELLFAMILY_WARLOCK: u32 = 5;
const WARLOCK_HELLFIRE: u64 = 1 << 6;
const WARLOCK_VOIDWALKER_SPELLS: u64 = 1 << 25;
const ATTR_AURA_IS_DEBUFF: u32 = 0x0400_0000;

const TARGET_UNIT_CASTER: i32 = 1;
const TARGET_UNIT_ENEMY: i32 = 6;
const TARGET_SCRIPT_AOE_AT_SRC: i32 = 7;
const TARGET_ENEMY_AOE_AT_SRC: i32 = 15;
const TARGET_ENEMY_AOE_AT_DEST: i32 = 16;
const TARGET_UNIT_FRIEND: i32 = 21;
const TARGET_CASTER_SRC: i32 = 22;
const TARGET_ENEMY_IN_CONE_24: i32 = 24;
const TARGET_UNIT: i32 = 25;
const TARGET_ENEMY_AOE_AT_DYNOBJ: i32 = 28;
const TARGET_FRIEND_AOE_AT_SRC: i32 = 30;
const TARGET_PARTY_AOE_AT_SRC: i32 = 33;
const TARGET_UNIT_PARTY: i32 = 35;
const TARGET_UNIT_FRIEND_CHAIN_HEAL: i32 = 45;
const TARGET_CASTER_TARGET_POSITION: i32 = 53;
const TARGET_UNIT_RAID: i32 = 57;
const TARGET_UNIT_RAID_AND_CLASS: i32 = 61;

const MECHANICS_THAT_ARE_NOT_POSITIVE_IMMUNITIES: [u32; 4] = [16, 19, 21, 25];
const SPELLMOD_COST: u32 = 14;

pub(crate) fn refuses_self_cast(spell: &Spell, spells: &[Spell]) -> bool {
    is_explicitly_selected_unit_target(spell.implicit_target_a[0])
        && !is_positive_spell(spell, spells)
}

fn is_explicitly_selected_unit_target(target: i32) -> bool {
    matches!(
        target,
        TARGET_UNIT_ENEMY
            | TARGET_UNIT_FRIEND
            | TARGET_UNIT
            | TARGET_UNIT_PARTY
            | TARGET_UNIT_FRIEND_CHAIN_HEAL
            | TARGET_CASTER_TARGET_POSITION
            | TARGET_UNIT_RAID
            | TARGET_UNIT_RAID_AND_CLASS
    )
}

fn is_positive_target(target_a: i32, target_b: i32) -> bool {
    match target_a {
        TARGET_UNIT_ENEMY
        | TARGET_ENEMY_AOE_AT_SRC
        | TARGET_ENEMY_AOE_AT_DEST
        | TARGET_ENEMY_IN_CONE_24
        | TARGET_ENEMY_AOE_AT_DYNOBJ
        | TARGET_CASTER_TARGET_POSITION => false,
        TARGET_CASTER_SRC => matches!(
            target_b,
            TARGET_PARTY_AOE_AT_SRC | TARGET_FRIEND_AOE_AT_SRC | TARGET_SCRIPT_AOE_AT_SRC
        ),
        _ if target_b != 0 => is_positive_target(target_b, 0),
        _ => true,
    }
}

fn is_positive_spell(spell: &Spell, spells: &[Spell]) -> bool {
    spell.attributes & ATTR_AURA_IS_DEBUFF == 0
        && (0..3)
            .all(|index| spell.effect[index] == 0 || is_positive_effect(spell, index, true, spells))
}

fn simple_value(spell: &Spell, index: usize) -> i32 {
    spell.effect_base_points[index].saturating_add(spell.effect_base_dice[index] as i32)
}

fn is_positive_effect(spell: &Spell, index: usize, self_cast: bool, spells: &[Spell]) -> bool {
    if spell.custom_flags & POSITIVE != 0 {
        return true;
    }
    if spell.custom_flags & NEGATIVE != 0 {
        return false;
    }
    let family_flag = |family, flag| spell.family_name == family && spell.family_flags & flag != 0;
    if family_flag(SPELLFAMILY_WARLOCK, WARLOCK_HELLFIRE)
        && spell.icon_id == 937
        && spell.visual_id == 5423
    {
        return true;
    }
    let target_a = spell.implicit_target_a[index];
    let target_b = spell.implicit_target_b[index];
    match spell.effect[index] {
        EFFECT_DUMMY if matches!(spell.id, 10258 | 18153) => return true,
        EFFECT_HEAL | EFFECT_LEARN_SPELL | EFFECT_SKILL_STEP => return true,
        EFFECT_INSTAKILL => {
            return (target_a == TARGET_UNIT_CASTER && target_b == 0)
                || family_flag(SPELLFAMILY_WARLOCK, WARLOCK_VOIDWALKER_SPELLS);
        }
        EFFECT_DISPEL if self_cast => return true,
        EFFECT_DISPEL | EFFECT_APPLY_AURA => {
            if let Some(positive) = aura_polarity(spell, index, spells) {
                return positive;
            }
        }
        _ => {}
    }
    is_positive_target(target_a, target_b) && spell.attributes & ATTR_AURA_IS_DEBUFF == 0
}

// Some(polarity) is an early return in vmangos; None falls through to the target check.
fn aura_polarity(spell: &Spell, index: usize, spells: &[Spell]) -> Option<bool> {
    let value = simple_value(spell, index);
    let target_a = spell.implicit_target_a[index];
    match spell.effect_aura[index] {
        4 => match spell.id {
            13139 | 23445 => Some(false),
            27184 | 27190 | 27191 | 27201 | 27202 | 27203 => Some(true),
            _ => None,
        },
        13 | 22 | 29 | 30 | 49 | 118 | 135 => (value < 0).then_some(false),
        14 => (value < 0).then_some(true),
        57 | 79 | 133 => (value > 0).then_some(true),
        34 | 109 => Some(true),
        23 => {
            let triggered = spells
                .iter()
                .find(|other| other.id == spell.effect_trigger_spell[index]);
            triggered
                .filter(|other| other.id != spell.id)
                .and_then(|other| {
                    (0..3)
                        .any(|i| {
                            other.effect[i] != 0
                                && is_positive_target(
                                    other.implicit_target_a[i],
                                    other.implicit_target_b[i],
                                )
                                && !is_positive_effect(other, i, false, spells)
                        })
                        .then_some(false)
                })
        }
        12 => (index == 0 && spell.effect[1] == 0 && spell.effect[2] == 0).then_some(false),
        60 => Some(spell.id == 24740),
        27 => Some(spell.id == 24732),
        26 | 95 | 53 | 68 | 89 | 91 => Some(false),
        3 => (target_a == TARGET_UNIT_CASTER).then_some(false),
        33 => ((target_a == TARGET_UNIT_CASTER && spell.family_name == SPELLFAMILY_GENERIC)
            || (spell.attributes & ATTR_AURA_IS_DEBUFF != 0 && index == 0))
            .then_some(false),
        61 => (spell.id == 802).then_some(true),
        77 => MECHANICS_THAT_ARE_NOT_POSITIVE_IMMUNITIES
            .contains(&spell.effect_misc_value[index])
            .then_some(false),
        107 | 108 => {
            (spell.effect_misc_value[index] == SPELLMOD_COST && value > 0).then_some(false)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spell(
        id: u32,
        effect: [i32; 3],
        aura: [u32; 3],
        base: [i32; 3],
        target_a: [i32; 3],
    ) -> Spell {
        Spell {
            id,
            effect,
            effect_aura: aura,
            effect_base_points: base,
            implicit_target_a: target_a,
            ..Default::default()
        }
    }

    #[test]
    fn the_handler_refuses_what_vmangos_calls_non_positive_for_a_selected_unit() {
        let refused = [
            spell(133, [2, 6, 0], [0, 3, 0], [0; 3], [6, 6, 0]),
            spell(53, [17, 0, 0], [0; 3], [0; 3], [6, 0, 0]),
            spell(1130, [6, 0, 0], [102, 0, 0], [0; 3], [6, 0, 0]),
            spell(2139, [68, 0, 0], [0; 3], [0; 3], [6, 0, 0]),
            spell(100, [96, 0, 0], [0; 3], [0; 3], [6, 0, 0]),
            spell(605, [6, 0, 0], [2, 0, 0], [0; 3], [6, 0, 0]),
        ];
        for spell in &refused {
            assert!(refuses_self_cast(spell, &refused), "spell {}", spell.id);
        }
    }

    #[test]
    fn the_handler_lets_positive_and_self_dispelling_spells_target_the_caster() {
        let allowed = [
            spell(1038, [6, 0, 0], [10, 0, 0], [-31, 0, 0], [57, 0, 0]),
            spell(25895, [6, 0, 0], [10, 0, 0], [-31, 0, 0], [61, 0, 0]),
            spell(370, [38, 0, 0], [0; 3], [0; 3], [6, 0, 0]),
            spell(4987, [38, 0, 0], [0; 3], [0; 3], [21, 0, 0]),
            spell(2050, [10, 0, 0], [0; 3], [0; 3], [21, 0, 0]),
            spell(168, [6, 0, 0], [15, 0, 0], [0; 3], [1, 0, 0]),
        ];
        for spell in &allowed {
            assert!(!refuses_self_cast(spell, &allowed), "spell {}", spell.id);
        }
    }

    #[test]
    fn a_source_custom_flag_decides_polarity_before_the_target_does() {
        let mut positive = spell(1, [2, 0, 0], [0; 3], [0; 3], [6, 0, 0]);
        positive.custom_flags = POSITIVE;
        assert!(!refuses_self_cast(&positive, &[]));
        let mut negative = spell(2, [10, 0, 0], [0; 3], [0; 3], [21, 0, 0]);
        negative.custom_flags = NEGATIVE;
        assert!(refuses_self_cast(&negative, &[]));
    }

    #[test]
    fn a_periodic_trigger_is_negative_when_it_aims_a_negative_effect_at_a_positive_target() {
        let trigger = || {
            let mut trigger = spell(1, [6, 0, 0], [23, 0, 0], [0; 3], [21, 0, 0]);
            trigger.effect_trigger_spell = [2, 0, 0];
            trigger
        };
        let hostile = spell(2, [6, 0, 0], [3, 0, 0], [0; 3], [1, 0, 0]);
        assert!(refuses_self_cast(&trigger(), &[trigger(), hostile]));
        let friendly = spell(2, [10, 0, 0], [0; 3], [0; 3], [21, 0, 0]);
        assert!(!refuses_self_cast(&trigger(), &[trigger(), friendly]));
    }
}
