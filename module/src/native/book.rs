use lyracore_protocol::{
    codec,
    messages::vanilla::{opcodes::ServerOpcodeMessage, SMSG_SUPERCEDED_SPELL},
};
use spacetimedb::ReducerContext;

use crate::{
    game_world_entity,
    skill::{game_player_skill, PlayerSkill},
    spell::spellbook::{game_player_spell, game_spell_chain},
    talent::game_character_talent,
};

pub(super) fn cooldowns(
    ctx: &ReducerContext,
    guid: u64,
) -> Vec<lyracore_protocol::messages::vanilla::CooldownSpell> {
    use crate::game_spell_cd;
    let now = crate::time::now(ctx).to_micros_since_unix_epoch();
    ctx.db
        .game_spell_cd()
        .by_caster()
        .filter(guid)
        .filter_map(|row| {
            let remaining = |deadline: i64| {
                std::time::Duration::from_millis(
                    (deadline.saturating_sub(now) / 1000).clamp(0, u32::MAX as i64) as u64,
                )
            };
            let cooldown = remaining(row.ready_at.to_micros_since_unix_epoch());
            let category_cooldown = remaining(row.category_ready_micros);
            (!cooldown.is_zero() || !category_cooldown.is_zero()).then_some(
                lyracore_protocol::messages::vanilla::CooldownSpell {
                    spell_id: row.spell_id as u16,
                    item_id: 0,
                    spell_category: if category_cooldown.is_zero() {
                        0
                    } else {
                        row.category as u16
                    },
                    cooldown,
                    category_cooldown,
                },
            )
        })
        .collect()
}

pub(crate) fn talent_points(ctx: &ReducerContext, guid: u64, level: u32) {
    if !super::has_world_session(ctx, guid) {
        return;
    }
    let spent = ctx
        .db
        .game_character_talent()
        .by_character()
        .filter(guid)
        .fold(0u32, |total, row| total.saturating_add(u32::from(row.rank)));
    super::send_to_character(
        ctx,
        guid,
        ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(codec::build_talent_points_values(
            guid,
            crate::talent::talent_points_available(level, spent),
        ))),
        super::EncodeFailure::Panic,
    );
}

pub(crate) fn learned(ctx: &ReducerContext, guid: u64, spell: u32) {
    if !super::has_world_session(ctx, guid) {
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
    super::send_to_character(ctx, guid, message, super::EncodeFailure::Panic);
    proficiency(ctx, guid);
}

pub(crate) fn forgotten(ctx: &ReducerContext, guid: u64, spell: u32) {
    // SMSG_REMOVED_SPELL uses a u16 spell id in build 5875.
    if super::send_raw_to_character(
        ctx,
        guid,
        codec::SMSG_REMOVED_SPELL_OPCODE,
        (spell as u16).to_le_bytes().to_vec(),
    ) {
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
        super::send_to_character(
            ctx,
            guid,
            codec::build_armor_proficiency_msg(character.class(), &learned),
            super::EncodeFailure::Panic,
        );
        super::send_to_character(
            ctx,
            guid,
            codec::build_weapon_proficiency_msg(character.class(), &learned),
            super::EncodeFailure::Panic,
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
    if !super::has_world_session(ctx, row.character_guid)
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
                super::send_to_character(
                    ctx,
                    row.character_guid,
                    ServerOpcodeMessage::SMSG_UPDATE_OBJECT(Box::new(values)),
                    super::EncodeFailure::Panic,
                );
            }
        }
    }
}
