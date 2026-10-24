use codec::update_mask::idx::EXPLORED_ZONES_WORDS;
use lyracore_protocol::{codec, messages::vanilla::opcodes::ServerOpcodeMessage};
use spacetimedb::{ConnectionId, ReducerContext};

use super::game_native_player;
use crate::exploration::{game_character_explored, CharacterExplored};

pub(crate) fn xp(ctx: &ReducerContext, guid: u64, row: &crate::xp::XpEvent) {
    if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
        let (opcode, body) = codec::build_log_xpgain_raw(
            row.killed_guid,
            row.total_exp,
            row.is_kill,
            row.rested_bonus,
        );
        super::send_raw(ctx, owner.connection_id, opcode, body, false);
    }
}

pub(crate) fn levelup(ctx: &ReducerContext, guid: u64, row: &crate::xp::LevelupEvent) {
    if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
        let packet = lyracore_protocol::world::encode(ServerOpcodeMessage::SMSG_LEVELUP_INFO(
            Box::new(codec::build_levelup_info(
                row.new_level,
                row.health_gained,
                row.mana_gained,
                row.strength_gained,
                row.agility_gained,
                row.stamina_gained,
                row.intellect_gained,
                row.spirit_gained,
            )),
        ))
        .expect("fixed-size level-up packet fits a world frame");
        super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
    }
}

pub(crate) fn explored(ctx: &ReducerContext, row: &CharacterExplored) {
    let Some(owner) = ctx
        .db
        .game_native_player()
        .character_guid()
        .find(row.character_guid)
    else {
        return;
    };
    if !(0..i32::from(EXPLORED_ZONES_WORDS) * 32).contains(&row.area_bit) {
        return;
    }
    let first_bit = row.area_bit / 32 * 32;
    let word = ctx
        .db
        .game_character_explored()
        .by_character()
        .filter(row.character_guid)
        .filter(|area| (first_bit..first_bit + 32).contains(&area.area_bit))
        .fold(0, |word, area| word | (1 << (area.area_bit - first_bit)));
    let (opcode, body) =
        codec::build_explored_zones_values(row.character_guid, (first_bit / 32) as u16, word);
    super::send_raw(ctx, owner.connection_id, opcode, body, false);
    let (opcode, body) = codec::build_exploration_experience_raw(row.area_id, row.experience);
    super::send_raw(ctx, owner.connection_id, opcode, body, false);
}

pub(super) fn replay_exploration(ctx: &ReducerContext, connection: ConnectionId, guid: u64) {
    let mut words = [0u32; EXPLORED_ZONES_WORDS as usize];
    for row in ctx.db.game_character_explored().by_character().filter(guid) {
        if (0..i32::from(EXPLORED_ZONES_WORDS) * 32).contains(&row.area_bit) {
            words[(row.area_bit / 32) as usize] |= 1 << (row.area_bit % 32);
        }
    }
    if words.iter().all(|word| *word == 0) {
        return;
    }
    let mut mask = codec::update_mask::UpdateMaskValues::new();
    for (index, word) in words.into_iter().enumerate().filter(|(_, word)| *word != 0) {
        mask.set_u32(
            codec::update_mask::idx::PLAYER_EXPLORED_ZONES_1 + index as u16,
            word,
        );
    }
    let (opcode, body) = codec::build_values_update_raw(guid, &mask);
    super::send_raw(ctx, connection, opcode, body, false);
}
