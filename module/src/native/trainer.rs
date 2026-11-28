use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        TrainingFailureReason,
    },
};
use lyracore_shared::trainer::TrainerRefusal;
use spacetimedb::{ConnectionId, ReducerContext};

use super::{Rollback, Unhandled};
use crate::{trainer::game_trainer_spell, Refusal};

pub(super) fn list(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    trainer: u64,
) -> Result<(), Refusal> {
    let character = super::actor(ctx, guid)?;
    let trainer = crate::trainer::validate_trainer_interaction(ctx, &character, trainer)?;
    let trainee = crate::trainer::Trainee::load(ctx, guid);
    let mut spells: Vec<_> = ctx
        .db
        .game_trainer_spell()
        .by_trainer()
        .filter(trainer.entry)
        .map(|row| {
            let assessment = trainee.assess(
                ctx,
                &row,
                crate::trainer::resolve_learn_target(ctx, row.spell_id),
            );
            codec::TrainerSpellView {
                spell_id: row.spell_id,
                cost: row.cost,
                required_level: row.required_level,
                player_level: character.level,
                known: assessment.known,
                learn_skill_line: row.learn_skill_line,
                required_skill: row.required_skill,
                required_skill_value: row.required_skill_value,
                previous_spell: assessment.previous_spell,
                required_spell: assessment.required_spell,
                gates: assessment.gates,
            }
        })
        .collect();
    spells.sort_by_key(|row| row.spell_id);
    Ok(super::send(
        ctx,
        connection,
        ServerOpcodeMessage::SMSG_TRAINER_LIST(Box::new(codec::build_trainer_list(
            trainer.guid,
            &spells,
            "I can teach you a thing or two.",
        ))),
        false,
    )?)
}

pub(super) fn receive(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    message: ClientOpcodeMessage,
) -> Result<(), Unhandled> {
    let handled: Result<(), Rollback> = match message {
        ClientOpcodeMessage::CMSG_LEARN_TALENT(c) => {
            let actor = super::actor(ctx, guid)?;
            crate::talent::do_learn_talent(ctx, guid, actor.owner_identity, c.talent.as_int())?;
            Ok(())
        }
        ClientOpcodeMessage::CMSG_TRAINER_LIST(c) => {
            Ok(list(ctx, connection, guid, c.guid.guid())?)
        }
        ClientOpcodeMessage::CMSG_TRAINER_BUY_SPELL(c) => {
            let buy_failed = |reason, cause: &str| {
                Rollback::reply(
                    ServerOpcodeMessage::SMSG_TRAINER_BUY_FAILED(Box::new(
                        codec::build_trainer_buy_failed(c.guid.guid(), c.id, reason),
                    )),
                    cause,
                )
            };
            super::actor(ctx, guid).map_err(|refusal| {
                buy_failed(TrainingFailureReason::Unavailable, &refusal.to_string())
            })?;
            crate::trainer::apply_trainer_buy(ctx, guid, c.guid.guid(), c.id)
                .map_err(|refusal| buy_failed(failure_reason(refusal), refusal.as_tag()))?;
            Ok(super::send(
                ctx,
                connection,
                ServerOpcodeMessage::SMSG_TRAINER_BUY_SUCCEEDED(Box::new(
                    codec::build_trainer_buy_succeeded(c.guid.guid(), c.id),
                )),
                false,
            )?)
        }
        ClientOpcodeMessage::CMSG_SET_ACTION_BUTTON(c) => {
            let actor = super::actor(ctx, guid)?;
            Ok(crate::action_bar::apply_set_action_button(
                ctx,
                actor,
                c.button,
                u32::from(c.action) | (u32::from(c.misc) << 16),
                c.action_type,
            )?)
        }
        other => return Err(Unhandled::Declined(other)),
    };
    Ok(handled?)
}

fn failure_reason(refusal: TrainerRefusal) -> TrainingFailureReason {
    match refusal {
        TrainerRefusal::NotEnoughMoney => TrainingFailureReason::NotEnoughMoney,
        TrainerRefusal::AlreadyKnown
        | TrainerRefusal::LevelTooLow
        | TrainerRefusal::PreviousRankMissing
        | TrainerRefusal::RequiredSpellMissing
        | TrainerRefusal::SkillTooLow
        | TrainerRefusal::PrimaryProfessionLimit => TrainingFailureReason::NotEnoughSkill,
        TrainerRefusal::Unavailable | TrainerRefusal::NotOffered => {
            TrainingFailureReason::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_refusal_but_money_and_admission_answers_not_enough_skill() {
        for refusal in [
            TrainerRefusal::AlreadyKnown,
            TrainerRefusal::LevelTooLow,
            TrainerRefusal::PreviousRankMissing,
            TrainerRefusal::RequiredSpellMissing,
            TrainerRefusal::SkillTooLow,
            TrainerRefusal::PrimaryProfessionLimit,
        ] {
            assert_eq!(
                failure_reason(refusal),
                TrainingFailureReason::NotEnoughSkill,
                "{refusal:?}"
            );
        }
        assert_eq!(
            failure_reason(TrainerRefusal::NotEnoughMoney),
            TrainingFailureReason::NotEnoughMoney
        );
        for refusal in [TrainerRefusal::Unavailable, TrainerRefusal::NotOffered] {
            assert_eq!(
                failure_reason(refusal),
                TrainingFailureReason::Unavailable,
                "{refusal:?}"
            );
        }
    }
}
