use spacetimedb::ReducerContext;

use crate::spell::{Spell, SpellEffect, P_ENTRY};
use crate::{
    game_creature_template, game_gameobject, game_gameobject_template, GameObject, WorldEntity,
};

/// Resolve the required focus in the caster's partition before a cast spends anything.
pub(crate) fn check_hostile_summon(
    ctx: &ReducerContext,
    caster: &WorldEntity,
    spell: &Spell,
    effect: &SpellEffect,
) -> Result<GameObject, String> {
    if effect.p0_kind != P_ENTRY || effect.p0 <= 0 || effect.p1 <= 0 || spell.duration_ms == 0 {
        return Err("invalid hostile summon definition".to_string());
    }
    if ctx
        .db
        .game_creature_template()
        .entry()
        .find(effect.p0 as u32)
        .is_none()
    {
        return Err(format!("summon template {} is missing", effect.p0));
    }
    let radius = spell.range_yd.min(100) as f32;
    let (gx0, gx1, gy0, gy1) =
        lyracore_shared::spatial::covering_cell_box(caster.x, caster.y, radius);
    let mut nearest: Option<(f32, GameObject)> = None;
    for gx in gx0..=gx1 {
        for gy in gy0..=gy1 {
            let cell = lyracore_shared::spatial::grid_cell_id(gx, gy);
            for focus in
                ctx.db
                    .game_gameobject()
                    .by_cell()
                    .filter((caster.map_id, caster.instance_id, cell))
            {
                let Some(template) = ctx
                    .db
                    .game_gameobject_template()
                    .entry()
                    .find(focus.template_entry)
                else {
                    continue;
                };
                if template.type_id != 8 || template.data0 != effect.p1 as u32 {
                    continue;
                }
                let distance_sq = (focus.x - caster.x).powi(2)
                    + (focus.y - caster.y).powi(2)
                    + (focus.z - caster.z).powi(2);
                let allowed = radius.min(template.data1 as f32);
                if distance_sq <= allowed * allowed
                    && nearest.as_ref().is_none_or(|(distance, previous)| {
                        (distance_sq, focus.guid) < (*distance, previous.guid)
                    })
                {
                    nearest = Some((distance_sq, focus));
                }
            }
        }
    }
    nearest
        .map(|(_, focus)| focus)
        .ok_or_else(|| "required summoning circle is out of range".to_string())
}

pub(crate) fn summon_hostile(
    ctx: &ReducerContext,
    caster_guid: u64,
    spell: &Spell,
    effect: &SpellEffect,
) -> Result<(), String> {
    let caster = crate::helpers::live_entity(ctx, caster_guid)?;
    let focus = check_hostile_summon(ctx, &caster, spell, effect)?;
    super::place_temporary_summon(
        ctx,
        &caster,
        effect.p0 as u32,
        super::SummonLocation {
            x: focus.x,
            y: focus.y,
            z: focus.z,
            orientation: focus.orientation,
            lifetime_ms: spell.duration_ms,
        },
    )?;
    Ok(())
}
