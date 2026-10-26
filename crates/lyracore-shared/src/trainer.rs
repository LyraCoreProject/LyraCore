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

    #[test]
    fn trainer_type_constants_match_the_source_enum() {
        assert_eq!(trainer_type::CLASS, 0);
        assert_eq!(trainer_type::MOUNTS, 1);
        assert_eq!(trainer_type::TRADESKILLS, 2);
        assert_eq!(trainer_type::PETS, 3);
    }
}

#[derive(Clone, Copy)]
pub struct LearnEffect {
    pub index: u8,
    pub id: u64,
    pub kind: u8,
    pub trigger_spell: u32,
}

// The importer turns LEARN_SPELL into E_SCRIPTED. Stances and forms carry a trigger spell too.
pub fn learn_target_trigger(kind: u8, trigger_spell: u32) -> Option<u32> {
    (trigger_spell != 0 && kind == crate::spell::taxonomy::E_SCRIPTED).then_some(trigger_spell)
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

pub fn is_combat_skill_line(line: u32) -> bool {
    matches!(
        line,
        44 | 172 | 54 | 160 | 229 | 43 | 55 | 136 | 473 | 173 | 162 | 95
    )
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfferingState {
    Known,
    LevelTooLow,
    Available,
}

pub fn offering_state(known: bool, level: u32, required_level: u32) -> OfferingState {
    if known {
        OfferingState::Known
    } else if level < required_level {
        OfferingState::LevelTooLow
    } else {
        OfferingState::Available
    }
}

#[cfg(test)]
mod rule_tests {
    use super::*;
    use crate::spell::taxonomy::{
        A_FLAG, A_MOD_STAT, A_PERIODIC_TRIGGER, A_PROC_DAMAGE, A_PROC_TRIGGER, A_STEALTH, E_DAMAGE,
        E_SCRIPTED, E_SET_STANCE, E_TRIGGER,
    };

    #[test]
    fn wrapper_resolution_orders_eligible_effects_by_index_then_id() {
        let effects = [
            LearnEffect {
                index: 2,
                id: 1,
                kind: E_SCRIPTED,
                trigger_spell: 100,
            },
            LearnEffect {
                index: 1,
                id: 8,
                kind: E_SCRIPTED,
                trigger_spell: 200,
            },
            LearnEffect {
                index: 1,
                id: 7,
                kind: E_SCRIPTED,
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
            ("stance or form", E_SET_STANCE),
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
                kind: E_SET_STANCE,
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
            kind: E_SCRIPTED,
            trigger_spell,
        };
        assert_eq!(resolve_learn_target(1873, [learn(639)]), 639);
        assert_eq!(resolve_learn_target(1873, [learn(0)]), 1873);
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
        assert_eq!(offering_state(true, 1, 10), OfferingState::Known);
        assert_eq!(offering_state(false, 9, 10), OfferingState::LevelTooLow);
        assert_eq!(offering_state(false, 10, 10), OfferingState::Available);
    }
}
