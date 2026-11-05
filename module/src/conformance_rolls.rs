use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::{game_item_instance, game_world_entity};

#[table(accessor = game_conformance_swing_random)]
pub struct SwingRandom {
    #[primary_key]
    pub character_guid: u64,
    pub state: u64,
}

#[table(accessor = game_conformance_spell_random)]
pub struct SpellRandom {
    #[primary_key]
    pub character_guid: u64,
    #[unique]
    pub target_guid: u64,
    pub state: u64,
}

fn next(state: &mut u64) -> u32 {
    *state = state.wrapping_add(0x9e3779b97f4a7c15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    (value ^ (value >> 31)) as u32
}

fn next_spell(ctx: &ReducerContext, mut row: SpellRandom) -> u32 {
    let draw = next(&mut row.state);
    ctx.db
        .game_conformance_spell_random()
        .character_guid()
        .update(row);
    draw
}

pub(crate) fn spell_draw(ctx: &ReducerContext, character_guid: u64) -> Option<u32> {
    ctx.db
        .game_conformance_spell_random()
        .character_guid()
        .find(character_guid)
        .map(|row| next_spell(ctx, row))
}

pub(crate) fn spell_target_draw(ctx: &ReducerContext, target_guid: u64) -> Option<u32> {
    ctx.db
        .game_conformance_spell_random()
        .target_guid()
        .find(target_guid)
        .map(|row| next_spell(ctx, row))
}

#[reducer]
pub fn debug_prepare_spell_roll_character(
    ctx: &ReducerContext,
    character_guid: u64,
    target_guid: u64,
    target_level: u32,
    seed: u64,
    stream: u64,
) -> Result<(), String> {
    use crate::{game_spell, game_spell_effect};

    crate::helpers::require_operator(ctx)?;
    if !matches!(target_level, 60 | 63) || !crate::time::is_virtual(ctx) {
        return Err("spell rolls need a virtual clock and target level 60 or 63".into());
    }
    let entities = ctx.db.game_world_entity();
    let mut character = entities
        .guid()
        .find(character_guid)
        .ok_or("spell Character missing")?;
    let mut target = entities
        .guid()
        .find(target_guid)
        .ok_or("spell target missing")?;
    if !character.is_player() || target.is_player() || target.entry != 38 || character.level != 60 {
        return Err("invalid spell roll participants".into());
    }
    for item in ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&character_guid)
        .collect::<Vec<_>>()
    {
        crate::ItemInstance::delete(ctx, item.guid);
    }
    character.intellect = 125;
    character.health = 1_000_000_000;
    character.max_health = 1_000_000_000;
    character.power = 1_000_000_000;
    character.max_power = 1_000_000_000;
    character.update(ctx);
    target.level = target_level;
    target.health = 1_000_000_000;
    target.max_health = 1_000_000_000;
    target.orientation = std::f32::consts::PI;
    target.update(ctx);

    let mut spell = crate::seed::base_spell(2948, "Fixture spell 2948");
    spell.cost = 50;
    spell.cast_time_ms = 1500;
    spell.range_yd = 30;
    spell.school_mask = 4;
    spell.attributes = 65536;
    spell.spell_level = 22;
    spell.max_level = 26;
    spell.is_negative = true;
    spell.family_name = 3;
    spell.family_flags = 1073741840;
    spell.proc_chance = 101;
    ctx.db.game_spell().spell_id().delete(2948);
    ctx.db.game_spell().insert(spell);
    let effects = ctx.db.game_spell_effect();
    for effect in effects.by_spell().filter(&2948).collect::<Vec<_>>() {
        effects.id().delete(effect.id);
    }
    let mut effect = crate::seed::base_effect(2948, 0);
    effect.kind = 1;
    effect.base_points = 53;
    effect.die_sides = 13;
    effect.per_level = 0.9;
    effect.target = 1;
    effect.bonus_coefficient = 0.429;
    effects.insert(effect);
    crate::debug::debug_learn_spell(ctx, character_guid, 2948)?;
    let table = ctx.db.game_conformance_spell_random();
    table.character_guid().delete(character_guid);
    table.insert(SpellRandom {
        character_guid,
        target_guid,
        state: seed ^ stream.wrapping_mul(0xd1b54a32d192ed03),
    });
    Ok(())
}

pub(crate) fn swing_draws(ctx: &ReducerContext, character_guid: u64) -> Option<[u32; 3]> {
    let table = ctx.db.game_conformance_swing_random();
    let mut row = table.character_guid().find(character_guid)?;
    let draws = [
        next(&mut row.state),
        next(&mut row.state),
        next(&mut row.state),
    ];
    table.character_guid().update(row);
    Some(draws)
}

#[reducer]
pub fn debug_prepare_roll_character(
    ctx: &ReducerContext,
    character_guid: u64,
    target_guid: u64,
    target_level: u32,
    seed: u64,
    stream: u64,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if !matches!(target_level, 60 | 63) || !crate::time::is_virtual(ctx) {
        return Err("white melee needs a virtual clock and target level 60 or 63".into());
    }
    let entities = ctx.db.game_world_entity();
    let mut character = entities
        .guid()
        .find(character_guid)
        .ok_or("roll Character missing")?;
    let mut target = entities
        .guid()
        .find(target_guid)
        .ok_or("roll target missing")?;
    if !character.is_player() || target.is_player() || target.entry != 38 || character.level != 60 {
        return Err("invalid white melee participants".into());
    }
    for item in ctx
        .db
        .game_item_instance()
        .by_owner_guid()
        .filter(&character_guid)
        .collect::<Vec<_>>()
    {
        crate::ItemInstance::delete(ctx, item.guid);
    }
    character.agility = 100;
    character.health = 1_000_000_000;
    character.max_health = 1_000_000_000;
    character.base_attack_time_ms = 50;
    character.next_swing_spell = 0;
    character.update(ctx);
    target.level = target_level;
    target.health = 1_000_000_000;
    target.max_health = 1_000_000_000;
    target.orientation = std::f32::consts::PI;
    target.update(ctx);
    crate::skill::debug_set_skill(ctx, character_guid, 162, 300)?;
    crate::skill::debug_set_skill(ctx, character_guid, 95, 300)?;
    let table = ctx.db.game_conformance_swing_random();
    table.character_guid().delete(character_guid);
    table.insert(SwingRandom {
        character_guid,
        state: seed ^ stream.wrapping_mul(0xd1b54a32d192ed03),
    });
    Ok(())
}

#[reducer]
pub fn debug_advance_roll_batch(ctx: &ReducerContext, ticks: u32) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if !(1..=4000).contains(&ticks) {
        return Err("roll batch needs 1 through 4000 scheduler ticks".into());
    }
    crate::time::advance_virtual_clock(ctx, i64::from(ticks) * 50_000)
}
