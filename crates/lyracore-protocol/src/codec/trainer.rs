use lyracore_shared::trainer::OfferingGates;
use wow_world_messages::vanilla::{
    Skill, TrainerSpell, TrainerSpellState, TrainingFailureReason, SMSG_LEARNED_SPELL,
    SMSG_TRAINER_BUY_FAILED, SMSG_TRAINER_BUY_SUCCEEDED, SMSG_TRAINER_LIST,
};
use wow_world_messages::Guid;

#[derive(Clone, Copy, Debug, Default)]
pub struct TrainerSpellView {
    pub spell_id: u32,
    pub cost: u32,
    pub required_level: u8,
    pub player_level: u32,
    pub known: bool,
    pub learn_skill_line: u32,
    pub required_skill: u32,
    pub required_skill_value: u32,
    pub gates: OfferingGates,
}

pub fn build_trainer_list(
    trainer_guid: u64,
    spells: &[TrainerSpellView],
    greeting: &str,
) -> SMSG_TRAINER_LIST {
    let teaches_profession = spells.iter().any(|s| {
        matches!(
            Skill::try_from(s.learn_skill_line),
            Ok(Skill::Alchemy
                | Skill::Blacksmithing
                | Skill::Cooking
                | Skill::Enchanting
                | Skill::Engineering
                | Skill::FirstAid
                | Skill::Fishing
                | Skill::Herbalism
                | Skill::Leatherworking
                | Skill::Mining
                | Skill::Skinning
                | Skill::Tailoring)
        )
    });
    let spells = spells
        .iter()
        .map(|s| {
            use lyracore_shared::trainer::{offering_state, OfferingState};
            let state = match offering_state(
                s.known,
                s.player_level,
                u32::from(s.required_level),
                s.gates,
            ) {
                OfferingState::Known => TrainerSpellState::Gray,
                OfferingState::LevelTooLow
                | OfferingState::PreviousRankMissing
                | OfferingState::SkillTooLow => TrainerSpellState::Red,
                OfferingState::PrimaryProfessionLimit | OfferingState::Available => {
                    TrainerSpellState::Green
                }
            };
            let first_rank = u32::from(s.gates.primary_first_rank);
            TrainerSpell {
                spell: s.spell_id,
                state,
                spell_cost: s.cost,
                talent_point_cost: first_rank & u32::from(s.gates.primary_slot_free),
                first_rank,
                required_level: s.required_level,
                required_skill: Skill::try_from(s.required_skill).unwrap_or_default(),
                required_skill_value: s.required_skill_value,
                required_spells: [0, 0, 0],
            }
        })
        .collect();
    SMSG_TRAINER_LIST {
        guid: Guid::new(trainer_guid),
        trainer_type: if teaches_profession { 2 } else { 0 },
        spells,
        greeting: greeting.to_string(),
    }
}

pub fn build_trainer_buy_succeeded(trainer_guid: u64, spell_id: u32) -> SMSG_TRAINER_BUY_SUCCEEDED {
    SMSG_TRAINER_BUY_SUCCEEDED {
        guid: Guid::new(trainer_guid),
        id: spell_id,
    }
}

pub fn build_trainer_buy_failed(
    trainer_guid: u64,
    spell_id: u32,
    error: TrainingFailureReason,
) -> SMSG_TRAINER_BUY_FAILED {
    SMSG_TRAINER_BUY_FAILED {
        guid: Guid::new(trainer_guid),
        id: spell_id,
        error,
    }
}

pub fn build_learned_spell(spell_id: u32) -> SMSG_LEARNED_SPELL {
    SMSG_LEARNED_SPELL { id: spell_id }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profession_lists_enable_recipe_item_icons_without_changing_offering_ids() {
        use wow_world_messages::vanilla::ServerMessage;

        for line in [129, 164, 165, 171, 182, 185, 186, 197, 202, 333, 356, 393] {
            let spells = [
                TrainerSpellView {
                    spell_id: 3984,
                    cost: 100,
                    required_level: 1,
                    player_level: 10,
                    known: false,
                    learn_skill_line: 0,
                    ..Default::default()
                },
                TrainerSpellView {
                    spell_id: 4036,
                    cost: 0,
                    required_level: 1,
                    player_level: 10,
                    known: true,
                    learn_skill_line: line,
                    ..Default::default()
                },
            ];
            let mut packet = Vec::new();
            build_trainer_list(42, &spells, "Greetings")
                .write_unencrypted_server(&mut packet)
                .unwrap();
            assert_eq!(&packet[12..16], &2u32.to_le_bytes(), "skill line {line}");
            assert_eq!(&packet[16..20], &2u32.to_le_bytes());
            assert_eq!(&packet[20..24], &3984u32.to_le_bytes());
            assert_eq!(&packet[58..62], &4036u32.to_le_bytes());
        }
    }

    #[test]
    fn other_skill_lists_keep_the_spell_icon_type() {
        for line in [0, 43, 173, 762, u32::MAX] {
            let spells = [TrainerSpellView {
                spell_id: 100,
                cost: 10,
                required_level: 1,
                player_level: 10,
                known: false,
                learn_skill_line: line,
                ..Default::default()
            }];
            assert_eq!(build_trainer_list(42, &spells, "").trainer_type, 0);
        }
        assert_eq!(build_trainer_list(42, &[], "").trainer_type, 0);
    }

    #[test]
    fn trainer_list_state_mapping() {
        let v = [
            TrainerSpellView {
                spell_id: 100,
                cost: 10,
                required_level: 6,
                player_level: 10,
                known: false,
                learn_skill_line: 0,
                ..Default::default()
            },
            TrainerSpellView {
                spell_id: 101,
                cost: 10,
                required_level: 6,
                player_level: 2,
                known: false,
                learn_skill_line: 0,
                ..Default::default()
            },
            TrainerSpellView {
                spell_id: 102,
                cost: 10,
                required_level: 1,
                player_level: 10,
                known: true,
                learn_skill_line: 0,
                ..Default::default()
            },
        ];
        let msg = build_trainer_list(42, &v, "Greetings");
        assert_eq!(msg.spells[0].state, TrainerSpellState::Green);
        assert_eq!(msg.spells[1].state, TrainerSpellState::Red);
        assert_eq!(msg.spells[2].state, TrainerSpellState::Gray);
        assert_eq!(msg.guid, Guid::new(42));
        assert_eq!(msg.greeting, "Greetings");
        let edge = [TrainerSpellView {
            spell_id: 1,
            cost: 0,
            required_level: 6,
            player_level: 6,
            known: false,
            learn_skill_line: 0,
            ..Default::default()
        }];
        assert_eq!(
            build_trainer_list(1, &edge, "").spells[0].state,
            TrainerSpellState::Green
        );
    }

    #[test]
    fn a_trainer_list_marks_unmet_gates_red_and_sends_the_skill_requirement() {
        let offer = |spell_id, gates| TrainerSpellView {
            spell_id,
            required_level: 1,
            player_level: 10,
            learn_skill_line: 0,
            required_skill: 197,
            required_skill_value: 50,
            gates,
            ..Default::default()
        };
        let msg = build_trainer_list(
            42,
            &[
                offer(1, OfferingGates::default()),
                offer(
                    2,
                    OfferingGates {
                        skill_requirement_unmet: true,
                        ..Default::default()
                    },
                ),
                offer(
                    3,
                    OfferingGates {
                        previous_rank_missing: true,
                        ..Default::default()
                    },
                ),
            ],
            "",
        );
        let states: Vec<_> = msg.spells.iter().map(|spell| spell.state).collect();
        assert_eq!(
            states,
            [
                TrainerSpellState::Green,
                TrainerSpellState::Red,
                TrainerSpellState::Red
            ]
        );
        assert!(msg
            .spells
            .iter()
            .all(|spell| spell.required_skill == Skill::Tailoring
                && spell.required_skill_value == 50));
    }

    #[test]
    fn a_primary_first_rank_is_green_with_a_free_slot_and_green_without_the_learn_button() {
        let first_rank = |primary_slot_free| TrainerSpellView {
            spell_id: 2259,
            required_level: 1,
            player_level: 10,
            learn_skill_line: 171,
            gates: OfferingGates {
                primary_first_rank: true,
                primary_slot_free,
                ..Default::default()
            },
            ..Default::default()
        };
        let free = &build_trainer_list(42, &[first_rank(true)], "").spells[0];
        assert_eq!(free.state, TrainerSpellState::Green);
        assert_eq!((free.talent_point_cost, free.first_rank), (1, 1));
        let none_left = &build_trainer_list(42, &[first_rank(false)], "").spells[0];
        assert_eq!(none_left.state, TrainerSpellState::Green);
        assert_eq!((none_left.talent_point_cost, none_left.first_rank), (0, 1));
    }

    #[test]
    fn a_later_rank_and_a_secondary_profession_send_no_confirmation() {
        let spell = |learn_skill_line| TrainerSpellView {
            spell_id: 3101,
            required_level: 1,
            player_level: 10,
            learn_skill_line,
            ..Default::default()
        };
        for line in [171, 185] {
            let sent = &build_trainer_list(42, &[spell(line)], "").spells[0];
            assert_eq!((sent.talent_point_cost, sent.first_rank), (0, 0), "{line}");
        }
    }

    #[test]
    fn trainer_buy_failed_addresses_the_trainer_and_the_offering() {
        let msg = build_trainer_buy_failed(42, 100, TrainingFailureReason::NotEnoughMoney);
        assert_eq!(msg.guid, Guid::new(42));
        assert_eq!(msg.id, 100);
        assert_eq!(msg.error, TrainingFailureReason::NotEnoughMoney);
    }
}
