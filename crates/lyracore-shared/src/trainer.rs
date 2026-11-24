// CLASS is also the column default; require a populated trainer_class to restrict it.
pub mod trainer_type {
    pub const CLASS: u8 = 0;
    pub const MOUNTS: u8 = 1;
    pub const TRADESKILLS: u8 = 2;
    pub const PETS: u8 = 3;
}

pub const RIDING_SKILL_LINE: u32 = 762;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrainerRefusal {
    Unavailable,
    NotOffered,
    AlreadyKnown,
    LevelTooLow,
    PreviousRankMissing,
    SkillTooLow,
    PrimaryProfessionLimit,
    NotEnoughMoney,
}

impl TrainerRefusal {
    pub fn as_tag(self) -> &'static str {
        match self {
            Self::Unavailable => "trainer:unavailable",
            Self::NotOffered => "trainer:not_offered",
            Self::AlreadyKnown => "trainer:already_known",
            Self::LevelTooLow => "trainer:level_too_low",
            Self::PreviousRankMissing => "trainer:previous_rank_missing",
            Self::SkillTooLow => "trainer:skill_too_low",
            Self::PrimaryProfessionLimit => "trainer:primary_profession_limit",
            Self::NotEnoughMoney => "trainer:not_enough_money",
        }
    }
}

pub const fn serves(player_class: u8, trainer_type: u8, trainer_class: u8) -> bool {
    trainer_type != self::trainer_type::CLASS || trainer_class == 0 || trainer_class == player_class
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASSES: [u8; 9] = [1, 2, 3, 4, 5, 7, 8, 9, 11];

    #[test]
    fn a_class_trainer_serves_its_own_class_and_refuses_every_other() {
        for &trainer_class in &CLASSES {
            for &player_class in &CLASSES {
                assert_eq!(
                    serves(player_class, trainer_type::CLASS, trainer_class),
                    player_class == trainer_class,
                    "class trainer {trainer_class} vs player {player_class}"
                );
            }
        }
        assert!(!serves(1, trainer_type::CLASS, 2));
        assert!(serves(2, trainer_type::CLASS, 2));
    }

    #[test]
    fn an_unpopulated_trainer_class_serves_every_class() {
        for &player_class in &CLASSES {
            assert!(
                serves(player_class, trainer_type::CLASS, 0),
                "un-reimported trainer must still serve class {player_class}"
            );
        }
    }

    #[test]
    fn non_class_trainer_types_serve_every_class_even_with_a_populated_trainer_class() {
        for ty in [
            trainer_type::MOUNTS,
            trainer_type::TRADESKILLS,
            trainer_type::PETS,
        ] {
            for &player_class in &CLASSES {
                assert!(
                    serves(player_class, ty, 0),
                    "type {ty} serves {player_class}"
                );
                for &trainer_class in &CLASSES {
                    assert!(
                        serves(player_class, ty, trainer_class),
                        "type {ty} (class {trainer_class}) still serves {player_class}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_dump_verified_trainers_gate_as_ingested() {
        assert!(serves(2, 0, 2));
        assert!(!serves(1, 0, 2));
        for &c in &CLASSES {
            assert!(serves(c, 2, 0), "every class trains weapons at Woo Ping");
        }
        assert!(serves(8, 0, 8));
        assert!(!serves(1, 0, 8));
    }
}

#[derive(Clone, Copy)]
pub struct LearnEffect {
    pub index: u8,
    pub id: u64,
    pub kind: u8,
    pub trigger_spell: u32,
}

// vmangos trainers resolve every wrapper, a pet-learn one included, to effect 0's trigger spell.
pub fn learn_kind(kind: u8, target: u8) -> u8 {
    use crate::spell::taxonomy::{E_LEARN_SPELL, E_SCRIPTED, T_CASTER_PET};
    if kind == E_SCRIPTED && target == T_CASTER_PET {
        E_LEARN_SPELL
    } else {
        kind
    }
}

// Stances and forms carry a trigger spell too, so only a learn effect names a learn target.
pub fn learn_target_trigger(kind: u8, trigger_spell: u32) -> Option<u32> {
    (trigger_spell != 0 && kind == crate::spell::taxonomy::E_LEARN_SPELL).then_some(trigger_spell)
}

pub fn resolve_learn_target(spell_id: u32, effects: impl IntoIterator<Item = LearnEffect>) -> u32 {
    effects
        .into_iter()
        .filter_map(|effect| {
            learn_target_trigger(effect.kind, effect.trigger_spell)
                .map(|target| (effect.index, effect.id, target))
        })
        .min_by_key(|&(index, id, _)| (index, id))
        .map_or(spell_id, |(_, _, target)| target)
}

// Weapon, ranged, unarmed and defense skill lines; their caps follow the level.
pub const COMBAT_SKILL_LINES: [u32; 17] = [
    44, 172, 54, 160, 229, 43, 55, 136, 473, 173, 162, 95, 45, 46, 176, 226, 228,
];

pub fn is_combat_skill_line(line: u32) -> bool {
    COMBAT_SKILL_LINES.contains(&line)
}

pub fn offering_known(
    skill_line: u32,
    offered_cap: u32,
    stored_cap: Option<u32>,
    knows_spell: bool,
) -> bool {
    if skill_line == 0 {
        knows_spell
    } else if is_combat_skill_line(skill_line) {
        stored_cap.is_some()
    } else {
        stored_cap.is_some_and(|cap| cap >= offered_cap)
    }
}

pub const MAX_PRIMARY_PROFESSIONS: u32 = 2;
pub const APPRENTICE_SKILL_CAP: u32 = 75;

pub const PRIMARY_PROFESSION_LINES: [u32; 9] = [171, 164, 333, 202, 182, 165, 186, 393, 197];

pub fn is_primary_profession_line(line: u32) -> bool {
    PRIMARY_PROFESSION_LINES.contains(&line)
}

pub fn is_primary_first_rank(skill_line: u32, offered_cap: u32) -> bool {
    is_primary_profession_line(skill_line) && offered_cap == APPRENTICE_SKILL_CAP
}

pub fn free_primary_profession_slots(held: usize) -> u32 {
    MAX_PRIMARY_PROFESSIONS.saturating_sub(u32::try_from(held).unwrap_or(u32::MAX))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferingState {
    Known,
    LevelTooLow,
    PreviousRankMissing,
    SkillTooLow,
    PrimaryProfessionLimit,
    Available,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OfferingGates {
    pub previous_rank_missing: bool,
    pub skill_requirement_unmet: bool,
    pub primary_first_rank: bool,
    pub primary_slot_free: bool,
}

// Gate order follows vmangos Player::GetTrainerSpellState.
pub fn offering_state(
    known: bool,
    level: u32,
    required_level: u32,
    gates: OfferingGates,
) -> OfferingState {
    if known {
        OfferingState::Known
    } else if level < required_level {
        OfferingState::LevelTooLow
    } else if gates.previous_rank_missing {
        OfferingState::PreviousRankMissing
    } else if gates.skill_requirement_unmet {
        OfferingState::SkillTooLow
    } else if gates.primary_first_rank && !gates.primary_slot_free {
        OfferingState::PrimaryProfessionLimit
    } else {
        OfferingState::Available
    }
}

#[cfg(test)]
mod rule_tests {
    use super::*;
    use crate::spell::taxonomy::{
        A_FLAG, A_MOD_STAT, A_PERIODIC_TRIGGER, A_PROC_DAMAGE, A_PROC_TRIGGER, A_SHAPESHIFT,
        A_STEALTH, E_DAMAGE, E_LEARN_SPELL, E_SCRIPTED, E_SET_STANCE, E_TRIGGER,
    };

    #[test]
    fn a_pet_learn_effect_names_its_trigger_spell_as_learn_target_but_other_scripted_effects_do_not(
    ) {
        use crate::spell::taxonomy::{T_CASTER_PET, T_SCRIPTED};
        // vmangos spell 23100 Dash teaches 23099 to the pet.
        assert_eq!(
            learn_target_trigger(learn_kind(E_SCRIPTED, T_CASTER_PET), 23099),
            Some(23099)
        );
        assert_eq!(
            learn_target_trigger(learn_kind(E_SCRIPTED, T_SCRIPTED), 23099),
            None
        );
    }

    #[test]
    fn wrapper_resolution_orders_eligible_effects_by_index_then_id() {
        let effects = [
            LearnEffect {
                index: 2,
                id: 1,
                kind: E_LEARN_SPELL,
                trigger_spell: 100,
            },
            LearnEffect {
                index: 1,
                id: 8,
                kind: E_LEARN_SPELL,
                trigger_spell: 200,
            },
            LearnEffect {
                index: 1,
                id: 7,
                kind: E_LEARN_SPELL,
                trigger_spell: 300,
            },
        ];
        assert_eq!(resolve_learn_target(1873, effects), 300);
        assert_eq!(resolve_learn_target(1873, effects.into_iter().rev()), 300);
    }

    #[test]
    fn a_spell_that_only_triggers_another_spell_resolves_to_itself() {
        const BATTLE_STANCE: u32 = 2457;
        const BATTLE_STANCE_PASSIVE: u32 = 21156;
        let triggering_kinds = [
            ("stance", A_SHAPESHIFT),
            ("legacy form", E_SET_STANCE),
            ("stealth", A_STEALTH),
            ("periodic trigger", E_TRIGGER),
            ("periodic trigger aura", A_PERIODIC_TRIGGER),
            ("flag aura", A_FLAG),
            ("proc trigger", A_PROC_TRIGGER),
            ("proc damage", A_PROC_DAMAGE),
            ("stat aura", A_MOD_STAT),
            ("damage", E_DAMAGE),
        ];
        for (label, kind) in triggering_kinds {
            let effect = LearnEffect {
                index: 0,
                id: 1,
                kind,
                trigger_spell: BATTLE_STANCE_PASSIVE,
            };
            assert_eq!(
                resolve_learn_target(BATTLE_STANCE, [effect]),
                BATTLE_STANCE,
                "{label}"
            );
        }
    }

    #[test]
    fn a_stance_offering_ignores_a_trigger_spell_beside_its_stance_effect() {
        let effects = [
            LearnEffect {
                index: 0,
                id: 1,
                kind: A_SHAPESHIFT,
                trigger_spell: 21156,
            },
            LearnEffect {
                index: 1,
                id: 2,
                kind: A_FLAG,
                trigger_spell: 0,
            },
        ];
        assert_eq!(resolve_learn_target(2457, effects), 2457);
    }

    #[test]
    fn a_learn_effect_resolves_to_the_spell_it_teaches() {
        let learn = |trigger_spell| LearnEffect {
            index: 0,
            id: 1,
            kind: E_LEARN_SPELL,
            trigger_spell,
        };
        assert_eq!(resolve_learn_target(1873, [learn(639)]), 639);
        assert_eq!(resolve_learn_target(1873, [learn(0)]), 1873);
    }

    #[test]
    fn a_scripted_effect_with_a_trigger_spell_is_not_a_learn_effect() {
        let scripted = LearnEffect {
            index: 0,
            id: 1,
            kind: E_SCRIPTED,
            trigger_spell: 639,
        };
        assert_eq!(resolve_learn_target(1873, [scripted]), 1873);
    }

    #[test]
    fn weapon_proficiency_uses_row_presence_while_riding_and_professions_use_tiers() {
        assert!(offering_known(173, 300, Some(1), false));
        assert!(!offering_known(173, 0, None, true));
        for line in [185, 762] {
            assert!(offering_known(line, 75, Some(75), false));
            assert!(!offering_known(line, 150, Some(75), false));
            assert!(!offering_known(line, 75, None, true));
        }
        assert!(offering_known(0, 0, None, true));
        assert!(!offering_known(0, 0, Some(300), false));
    }

    #[test]
    fn known_offerings_stay_gray_even_below_the_required_level() {
        let gates = OfferingGates::default();
        assert_eq!(offering_state(true, 1, 10, gates), OfferingState::Known);
        assert_eq!(
            offering_state(false, 9, 10, gates),
            OfferingState::LevelTooLow
        );
        assert_eq!(
            offering_state(false, 10, 10, gates),
            OfferingState::Available
        );
    }

    #[test]
    fn offering_gates_apply_in_vmangos_order() {
        let all = OfferingGates {
            previous_rank_missing: true,
            skill_requirement_unmet: true,
            primary_first_rank: true,
            primary_slot_free: false,
        };
        assert_eq!(offering_state(true, 1, 9, all), OfferingState::Known);
        assert_eq!(offering_state(false, 1, 9, all), OfferingState::LevelTooLow);
        assert_eq!(
            offering_state(false, 9, 9, all),
            OfferingState::PreviousRankMissing
        );
        let all = OfferingGates {
            previous_rank_missing: false,
            ..all
        };
        assert_eq!(offering_state(false, 9, 9, all), OfferingState::SkillTooLow);
        let all = OfferingGates {
            skill_requirement_unmet: false,
            ..all
        };
        assert_eq!(
            offering_state(false, 9, 9, all),
            OfferingState::PrimaryProfessionLimit
        );
        let all = OfferingGates {
            primary_slot_free: true,
            ..all
        };
        assert_eq!(offering_state(false, 9, 9, all), OfferingState::Available);
    }

    #[test]
    fn only_the_nine_primary_lines_count_and_only_their_apprentice_rank_takes_a_slot() {
        for line in PRIMARY_PROFESSION_LINES {
            assert!(is_primary_first_rank(line, 75), "line {line}");
            assert!(!is_primary_first_rank(line, 150), "line {line}");
        }
        for secondary in [129, 185, 356, 762, 0] {
            assert!(!is_primary_first_rank(secondary, 75), "line {secondary}");
        }
        assert_eq!(free_primary_profession_slots(0), 2);
        assert_eq!(free_primary_profession_slots(1), 1);
        assert_eq!(free_primary_profession_slots(2), 0);
        assert_eq!(free_primary_profession_slots(9), 0);
    }
}
