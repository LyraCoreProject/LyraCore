use lyracore_shared::spell::taxonomy::{
    CAST_FLAG_BINARY, CAST_FLAG_IGNORE_RESISTANCES, RESIST_ARMOR,
};
use spacetimedb::ReducerContext;

use crate::{game_spell, game_world_entity};

pub(crate) fn apply_resistance(
    ctx: &ReducerContext,
    target_guid: u64,
    caster_guid: u64,
    school_mask: u8,
    dmg: i32,
    spell_id: u32,
) -> i32 {
    let caster_level = ctx
        .db
        .game_world_entity()
        .guid()
        .find(caster_guid)
        .map(|c| c.level)
        .unwrap_or(0);
    resist_magic_damage(
        ctx,
        target_guid,
        school_mask,
        caster_level,
        dmg,
        spell_id,
        false,
    )
}

pub(crate) fn resist_magic_damage(
    ctx: &ReducerContext,
    target_guid: u64,
    school_mask: u8,
    level: u32,
    dmg: i32,
    spell_id: u32,
    periodic: bool,
) -> i32 {
    if dmg <= 0 {
        return dmg;
    }
    if school_mask == 0 || school_mask & RESIST_ARMOR != 0 {
        return dmg;
    }
    let flags = ctx
        .db
        .game_spell()
        .spell_id()
        .find(spell_id)
        .map_or(0, |spell| spell.cast_flags);
    if flags & CAST_FLAG_IGNORE_RESISTANCES != 0 {
        return dmg;
    }
    let target = ctx.db.game_world_entity().guid().find(target_guid);
    let chance = super::rolls::resistance_chance(
        crate::modifiers::resistance_bonus(ctx, target_guid, school_mask),
        level,
        target.as_ref().map_or(level, |target| target.level),
        target.as_ref().is_some_and(|target| target.is_player()),
        true,
    );
    if flags & CAST_FLAG_BINARY != 0 && chance >= 0.0 {
        return dmg;
    }
    let periodic = periodic && !matches!(spell_id, 23461 | 24818 | 25812 | 28531);
    let roll = super::rolls::integer_roll(ctx, target_guid, 99);
    let quarters = super::rolls::resistance_quarters(chance, periodic, roll);
    let dither = crate::conformance_rolls::draw(
        ctx,
        crate::conformance_rolls::RollStream::Target(target_guid),
    );
    super::rolls::damage_after_resistance(dmg, quarters, dither)
}
