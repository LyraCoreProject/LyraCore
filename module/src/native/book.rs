use lyracore_protocol::{
    codec,
    messages::vanilla::{opcodes::ServerOpcodeMessage, SMSG_SUPERCEDED_SPELL},
};
use spacetimedb::ReducerContext;

use super::game_native_player;
use crate::{
    game_world_entity,
    skill::{game_player_skill, PlayerSkill},
    spell::spellbook::{game_player_spell, game_spell_chain},
    talent::game_character_talent,
};

pub(crate) fn talent_points(ctx: &ReducerContext, guid: u64, level: u32) {
    if !super::is_player(ctx, guid) {
        return;
    }
    let spent = ctx
        .db
        .game_character_talent()
        .by_character()
        .filter(guid)
        .fold(0u32, |total, row| total.saturating_add(u32::from(row.rank)));
    send(
        ctx,
        guid,
        ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_talent_points_values(
            guid,
            crate::talent::talent_points_available(level, spent),
        ))),
    );
}

fn send(ctx: &ReducerContext, guid: u64, message: ServerOpcodeMessage) {
    if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
        let packet = lyracore_protocol::world::encode(message)
            .expect("bounded spellbook message fits a world frame");
        super::send_raw(ctx, owner.connection_id, packet.opcode, packet.body, false);
    }
}

pub(crate) fn learned(ctx: &ReducerContext, guid: u64, spell: u32) {
    if !super::is_player(ctx, guid) {
        return;
    }
    use crate::game_spell;
    let ranks_stack = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell)
        .is_none_or(|row| {
            row.attributes & lyracore_shared::spell::taxonomy::SPELL_ATTR_PASSIVE == 0
                && row.power_type == 0
        });
    let previous = (!ranks_stack)
        .then(|| ctx.db.game_spell_chain().spell_id().find(spell))
        .flatten()
        .map(|row| row.prev_spell)
        .filter(|previous| {
            *previous != 0 && *previous != spell && crate::spell::knows_spell(ctx, guid, *previous)
        });
    let message = match previous {
        Some(old) => ServerOpcodeMessage::SMSG_SUPERCEDED_SPELL(SMSG_SUPERCEDED_SPELL {
            // Build 5875 expects old then new; the generated field names are reversed.
            new_spell_id: old as u16,
            old_spell_id: spell as u16,
        }),
        None => ServerOpcodeMessage::SMSG_LEARNED_SPELL(codec::build_learned_spell(spell)),
    };
    send(ctx, guid, message);
    proficiency(ctx, guid);
}

pub(crate) fn forgotten(ctx: &ReducerContext, guid: u64, spell: u32) {
    if let Some(owner) = ctx.db.game_native_player().character_guid().find(guid) {
        // SMSG_REMOVED_SPELL uses a u16 spell id in build 5875.
        super::send_raw(
            ctx,
            owner.connection_id,
            0x0203,
            (spell as u16).to_le_bytes().to_vec(),
            false,
        );
        proficiency(ctx, guid);
    }
}

fn proficiency(ctx: &ReducerContext, guid: u64) {
    if let Some(character) = ctx.db.game_world_entity().guid().find(guid) {
        let learned: Vec<_> = ctx
            .db
            .game_player_spell()
            .by_character()
            .filter(guid)
            .map(|row| row.spell_id)
            .collect();
        send(
            ctx,
            guid,
            codec::build_armor_proficiency_msg(character.class(), &learned),
        );
        send(
            ctx,
            guid,
            codec::build_weapon_proficiency_msg(character.class(), &learned),
        );
    }
}

pub(crate) fn skill_changed(
    ctx: &ReducerContext,
    previous: Option<&PlayerSkill>,
    current: Option<&PlayerSkill>,
) {
    let Some(row) = current.or(previous) else {
        return;
    };
    if !super::is_player(ctx, row.character_guid)
        || previous.map(|row| (row.skill_line, row.current, row.max_rank))
            == current.map(|row| (row.skill_line, row.current, row.max_rank))
    {
        return;
    }
    let Some(character) = ctx.db.game_world_entity().guid().find(row.character_guid) else {
        return;
    };
    let mut stored: Vec<_> = ctx
        .db
        .game_player_skill()
        .by_character()
        .filter(row.character_guid)
        .map(|row| (row.id, row.skill_line, row.current, row.max_rank))
        .collect();
    let layout = |stored: &mut Vec<(u64, u32, u16, u16)>| {
        stored.sort_by_key(|row| row.0);
        codec::skill_slot_layout(
            character.class(),
            &stored
                .iter()
                .map(|row| (row.1, row.2, row.3))
                .collect::<Vec<_>>(),
        )
    };
    let after = layout(&mut stored);
    stored.retain(|entry| entry.0 != row.id);
    if let Some(old) = previous {
        stored.push((old.id, old.skill_line, old.current, old.max_rank));
    }
    let before = layout(&mut stored);
    for slot in 0..before.len().max(after.len()).min(128) {
        if before.get(slot) != after.get(slot) {
            let (skill, current, cap) = after.get(slot).copied().unwrap_or_default();
            if let Some(values) =
                codec::build_skill_values(row.character_guid, slot as u8, skill, current, cap)
            {
                send(
                    ctx,
                    row.character_guid,
                    ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(values)),
                );
            }
        }
    }
}
