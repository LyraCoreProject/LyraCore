use spacetimedb::{ReducerContext, Table};

use lyracore_shared::race_class::{mask_admits, ZeroMask};

use super::{game_player_skill, PlayerSkill};
use crate::skilldata::{game_skill_ability, game_skill_availability};
use crate::spell::spellbook::{game_spell_chain, game_spell_learn};
use crate::WorldEntity;

// vmangos SKILL_FLAG_UNLEARNABLE: the skill can be unlearned.
const UNLEARNABLE: i32 = 0x20;

// vmangos GetSkillRaceClassInfo: the first availability row that admits the race and class.
fn unlearnable(ctx: &ReducerContext, entity: &WorldEntity, skill_line: u32) -> bool {
    ctx.db
        .game_skill_availability()
        .by_skill_line()
        .filter(skill_line)
        .filter(|row| {
            mask_admits(row.race_mask, entity.race(), ZeroMask::AdmitsAll)
                && mask_admits(row.class_mask, entity.class(), ZeroMask::AdmitsAll)
        })
        .min_by_key(|row| row.id)
        .is_some_and(|row| row.flags & UNLEARNABLE != 0)
}

// vmangos Player::RemoveSpell order: spells it teaches, spells whose prev or req spell it is,
// then the spell itself.
fn forgetting_order(
    ctx: &ReducerContext,
    guid: u64,
    spell_id: u32,
    seen: &mut Vec<u32>,
    order: &mut Vec<u32>,
) {
    if seen.contains(&spell_id) || !crate::spell::knows_spell(ctx, guid, spell_id) {
        return;
    }
    seen.push(spell_id);
    let taught: Vec<u32> = ctx
        .db
        .game_spell_learn()
        .by_parent()
        .filter(&spell_id)
        .map(|row| row.learn_spell)
        .collect();
    let higher_ranks: Vec<u32> = ctx
        .db
        .game_spell_chain()
        .iter()
        .filter(|row| row.prev_spell == spell_id || row.req_spell == spell_id)
        .map(|row| row.spell_id)
        .collect();
    for next in taught.into_iter().chain(higher_ranks) {
        forgetting_order(ctx, guid, next, seen, order);
    }
    order.push(spell_id);
}

/// CMSG_UNLEARN_SKILL (vmangos `HandleUnlearnSkillOpcode`): drops a skill the Character's race and
/// class may unlearn, with every spell of the skill line and every quest that requires it. Any
/// other request is ignored.
pub(crate) fn unlearn_skill(ctx: &ReducerContext, entity: &WorldEntity, skill_line: u32) {
    let guid = entity.guid;
    if !unlearnable(ctx, entity, skill_line) {
        spacetimedb::log::warn!("character {guid} tried to unlearn skill {skill_line}");
        return;
    }
    let Some(row) = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(&guid)
        .find(|row| row.skill_line == skill_line)
    else {
        return;
    };
    PlayerSkill::delete(ctx, row.id);
    let abilities: Vec<u32> = ctx
        .db
        .game_skill_ability()
        .by_skill_line()
        .filter(skill_line)
        .map(|ability| ability.spell_id)
        .collect();
    let (mut seen, mut order) = (Vec::new(), Vec::new());
    for spell_id in abilities {
        forgetting_order(ctx, guid, spell_id, &mut seen, &mut order);
    }
    crate::spell::strip_spell_auras(ctx, guid, &order);
    for spell_id in order {
        crate::spell::forget_spell(ctx, guid, spell_id);
    }
    crate::quest::forget_quests_requiring_skill(ctx, guid, skill_line);
    crate::spell::recompute_sheet(ctx, guid);
}
