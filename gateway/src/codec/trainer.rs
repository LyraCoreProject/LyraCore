//! Class-trainer wire: the trainer window (`SMSG_TRAINER_LIST`) + the buy result
//! (`SMSG_TRAINER_BUY_SUCCEEDED`/`_FAILED`) + the live `SMSG_LEARNED_SPELL` push so a bought ability
//! shows on the action bar without a relog. All gtker-typed vanilla (SMSG_TRAINER_LIST IS vanilla-complete
//! in the crate, unlike the raw-encoded vendor list). The per-spell Green/Red/Gray state is computed here
//! from the player's level + what they already know.

use super::*;

/// A trainer-list row the store fills for [`build_trainer_list`] — the static spell facts plus the two
/// player-dependent bits (`player_level`, `known`) the codec folds into the Green/Red/Gray state.
#[derive(Clone, Copy, Debug)]
pub struct TrainerSpellView {
    pub spell_id: u32,
    pub cost: u32,
    pub required_level: u8,
    pub player_level: u32,
    pub known: bool,
    /// This offering is a PROFESSION learn-row (learn_skill_line != 0). Drives the two
    /// misleadingly-named gtker u32s (really cmangos primary_prof_first_rank / primary_prof) —
    /// sending first_rank=1 on CLASS spells made the 5875 client treat every spell as a primary
    /// profession and DISABLE the Train button (live find 2026-07-11, screenshot: green row,
    /// funded purse, dead button). Class spells must send (0, 0).
    pub profession: bool,
}

/// Build `SMSG_TRAINER_LIST` — the trainer window. Each spell's state: GRAY if already known, RED if the
/// player is below `required_level`, else GREEN (learnable now). `first_rank = 1` enables the client's
/// Learn button (cmangos convention for a standalone rank). Deliberate simplification: no
/// rank-chain / required_skill — class spells need neither; `required_skill = Skill::default()`
/// (None) + empty prereqs.
pub fn build_trainer_list(
    trainer_guid: u64,
    spells: &[TrainerSpellView],
    greeting: &str,
) -> SMSG_TRAINER_LIST {
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
                // Real 1.12 semantics (cmangos SendTrainerList): these are
                // (primary_prof_first_rank, primary_prof) — NOT talent cost / rank. Class spells
                // send (0,0); profession learn-rows flag both so the client runs its
                // profession-slot handling only where it belongs.
                talent_point_cost: if s.profession { 1 } else { 0 },
                first_rank: if s.profession { 1 } else { 0 },
                required_level: s.required_level,
                required_skill: Skill::default(),
                required_skill_value: 0,
                required_spells: [0, 0, 0],
            }
        })
        .collect();
    SMSG_TRAINER_LIST {
        guid: Guid::new(trainer_guid),
        trainer_type: 0, // unused in vanilla
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

/// Build `SMSG_TRAINER_BUY_FAILED`. `reason` is the gtker `TrainingFailureReason` int the module tagged
/// onto its `Err` ([1]=money, [2]=level/req, else generic). gtker vanilla has only 3 reasons, so
/// already-known/range/flag failures fall to Unavailable — cosmetic, since the client gates the Learn
/// button on the Green state from the list.
pub fn build_trainer_buy_failed(
    trainer_guid: u64,
    spell_id: u32,
    reason: u32,
) -> SMSG_TRAINER_BUY_FAILED {
    let error = match reason {
        1 => TrainingFailureReason::NotEnoughMoney,
        2 => TrainingFailureReason::NotEnoughSkill,
        _ => TrainingFailureReason::Unavailable,
    };
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
    fn trainer_list_state_mapping() {
        let v = [
            TrainerSpellView {
                spell_id: 100,
                cost: 10,
                required_level: 6,
                player_level: 10,
                known: false,
                profession: false,
            }, // learnable
            TrainerSpellView {
                spell_id: 101,
                cost: 10,
                required_level: 6,
                player_level: 2,
                known: false,
                profession: false,
            }, // too low
            TrainerSpellView {
                spell_id: 102,
                cost: 10,
                required_level: 1,
                player_level: 10,
                known: true,
                profession: false,
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
            profession: false,
        }];
        assert_eq!(
            build_trainer_list(1, &edge, "").spells[0].state,
            TrainerSpellState::Green
        );
    }

    #[test]
    fn trainer_buy_failed_maps_reason_code_to_the_closest_failure_reason() {
        let money = build_trainer_buy_failed(42, 100, 1);
        assert_eq!(money.error, TrainingFailureReason::NotEnoughMoney);
        assert_eq!(money.guid, Guid::new(42));
        assert_eq!(money.id, 100);
        let skill = build_trainer_buy_failed(42, 100, 2);
        assert_eq!(skill.error, TrainingFailureReason::NotEnoughSkill);
        // gtker vanilla has only 3 reasons; any other code (already-known/range/flag failures)
        // degrades to Unavailable — cosmetic, since the client gates the Learn button on the list's
        // Green state anyway.
        for other in [0u32, 3, 99] {
            let msg = build_trainer_buy_failed(42, 100, other);
            assert_eq!(
                msg.error,
                TrainingFailureReason::Unavailable,
                "reason {other} must degrade to Unavailable"
            );
        }
    }
}
