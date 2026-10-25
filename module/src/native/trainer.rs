use lyracore_protocol::{
    codec,
    messages::vanilla::{
        opcodes::{ClientOpcodeMessage, ServerOpcodeMessage},
        TrainingFailureReason,
    },
};
use lyracore_shared::trainer::TrainerRefusal;
use spacetimedb::{ConnectionId, ReducerContext};

use super::Rollback;
use crate::{
    skill::game_player_skill, spell::spellbook::game_player_spell, trainer::game_trainer_spell,
    Refusal,
};

pub(super) fn list(
    ctx: &ReducerContext,
    connection: ConnectionId,
    guid: u64,
    trainer: u64,
) -> Result<(), Refusal> {
    let character = super::actor(ctx, guid)?;
    let trainer = crate::trainer::validate_trainer_interaction(ctx, &character, trainer)?;
    let learned: std::collections::HashSet<_> = ctx
        .db
        .game_player_spell()
        .by_character()
        .filter(guid)
        .map(|row| row.spell_id)
        .collect();
    let caps: std::collections::HashMap<_, _> = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(guid)
        .map(|row| (row.skill_line, u32::from(row.max_rank)))
        .collect();
    let mut spells: Vec<_> = ctx
        .db
        .game_trainer_spell()
        .by_trainer()
        .filter(trainer.entry)
        .map(|row| codec::TrainerSpellView {
            spell_id: row.spell_id,
            cost: row.cost,
            required_level: row.required_level,
            player_level: character.level,
            known: lyracore_shared::trainer::offering_known(
                row.learn_skill_line,
                row.learn_skill_cap,
                caps.get(&row.learn_skill_line).copied(),
                learned.contains(&crate::trainer::resolve_learn_target(ctx, row.spell_id)),
            ),
            learn_skill_line: row.learn_skill_line,
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
) -> Result<(), Rollback> {
    match message {
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
        _ => Err("invalid trainer opcode".into()),
    }
}

fn failure_reason(refusal: TrainerRefusal) -> TrainingFailureReason {
    match refusal {
        TrainerRefusal::NotEnoughMoney => TrainingFailureReason::NotEnoughMoney,
        TrainerRefusal::LevelTooLow | TrainerRefusal::PreviousRankMissing => {
            TrainingFailureReason::NotEnoughSkill
        }
        TrainerRefusal::Unavailable | TrainerRefusal::NotOffered | TrainerRefusal::AlreadyKnown => {
            TrainingFailureReason::Unavailable
        }
    }
}
