//! Trainer window, purchase results, and learned-spell packets.

use super::*;

/// An offering and the Character facts used to render its training state.
#[derive(Clone, Copy, Debug)]
pub struct TrainerSpellView {
    pub spell_id: u32,
    pub cost: u32,
    pub required_level: u8,
    pub player_level: u32,
    pub known: bool,
    /// Skill line taught by this offering, or 0 for a spell or recipe. Includes professions,
    /// weapon skills and riding, which need distinct trainer list types.
    pub learn_skill_line: u32,
}

/// Build the trainer window. Profession learn rows select the recipe icon lookup in build 5875.
/// Known offerings are gray, offerings above the Character's level are red, and others are green.
pub fn build_trainer_list(
    trainer_guid: u64,
    spells: &[TrainerSpellView],
    greeting: &str,
) -> SMSG_TRAINER_LIST {
    // The importer supplies learn rows for each profession a trainer teaches. The packet type is
    // distinct from the creature template type, which also labels weapon masters as tradeskills.
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
            let state = if s.known {
                TrainerSpellState::Gray
            } else if s.player_level < s.required_level as u32 {
                TrainerSpellState::Red
            } else {
                TrainerSpellState::Green
            };
            TrainerSpell {
                spell: s.spell_id,
                state,
                spell_cost: s.cost,
                talent_point_cost: if s.learn_skill_line != 0 { 1 } else { 0 },
                first_rank: if s.learn_skill_line != 0 { 1 } else { 0 },
                required_level: s.required_level,
                required_skill: Skill::default(),
                required_skill_value: 0,
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

/// Build `SMSG_TRAINER_BUY_SUCCEEDED` — confirms the purchase; pair with [`build_learned_spell`].
pub fn build_trainer_buy_succeeded(trainer_guid: u64, spell_id: u32) -> SMSG_TRAINER_BUY_SUCCEEDED {
    SMSG_TRAINER_BUY_SUCCEEDED {
        guid: Guid::new(trainer_guid),
        id: spell_id,
    }
}

/// Build `SMSG_TRAINER_BUY_FAILED`. The caller picks `error` from the Module's typed Refusal; gtker
/// vanilla carries only 3 reasons, so several Refusals share Unavailable — cosmetic, since the client
/// gates the Learn button on the Green state from the list.
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

/// Build `SMSG_LEARNED_SPELL` — the live push after a buy so the spell appears on the action bar without
/// a relog (the login `SMSG_INITIAL_SPELLS` sync is login-only).
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
                },
                TrainerSpellView {
                    spell_id: 4036,
                    cost: 0,
                    required_level: 1,
                    player_level: 10,
                    known: true,
                    learn_skill_line: line,
                },
            ];
            let mut packet = Vec::new();
            build_trainer_list(42, &spells, "Greetings")
                .write_unencrypted_server(&mut packet)
                .unwrap();
            // Four-byte server header, guid, then the list type and row count.
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
            }, // learnable
            TrainerSpellView {
                spell_id: 101,
                cost: 10,
                required_level: 6,
                player_level: 2,
                known: false,
                learn_skill_line: 0,
            }, // too low
            TrainerSpellView {
                spell_id: 102,
                cost: 10,
                required_level: 1,
                player_level: 10,
                known: true,
                learn_skill_line: 0,
            }, // already known
        ];
        let msg = build_trainer_list(42, &v, "Greetings");
        assert_eq!(msg.spells[0].state, TrainerSpellState::Green);
        assert_eq!(msg.spells[1].state, TrainerSpellState::Red);
        assert_eq!(msg.spells[2].state, TrainerSpellState::Gray);
        assert_eq!(msg.guid, Guid::new(42));
        assert_eq!(msg.greeting, "Greetings");
        // boundary: at exactly required_level → learnable (Green)
        let edge = [TrainerSpellView {
            spell_id: 1,
            cost: 0,
            required_level: 6,
            player_level: 6,
            known: false,
            learn_skill_line: 0,
        }];
        assert_eq!(
            build_trainer_list(1, &edge, "").spells[0].state,
            TrainerSpellState::Green
        );
    }

    #[test]
    fn trainer_buy_failed_addresses_the_trainer_and_the_offering() {
        let msg = build_trainer_buy_failed(42, 100, TrainingFailureReason::NotEnoughMoney);
        assert_eq!(msg.guid, Guid::new(42));
        assert_eq!(msg.id, 100);
        assert_eq!(msg.error, TrainingFailureReason::NotEnoughMoney);
    }
}
