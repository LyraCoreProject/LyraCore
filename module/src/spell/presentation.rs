use crate::{game_aura, game_creature_template, game_world_entity, WorldEntity};
use lyracore_shared::faction::{team_for_race, TEAM_HORDE};
use lyracore_shared::spell::taxonomy::{A_TRANSFORM, P_ENTRY};
use spacetimedb::ReducerContext;

/// Shows the unit's transform Aura model, else its Form's model for its team, else its own model,
/// each at its scale over the unit's base scale. See docs/schema.md "Forms".
pub(crate) fn recompute_display(ctx: &ReducerContext, guid: u64) {
    let Some(mut entity) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let (display, model_scale) = transform_display(ctx, guid)
        .or_else(|| form_display(ctx, &entity))
        .unwrap_or((entity.native_display_id, 1.0));
    let scale = base_scale(ctx, &entity).map_or(entity.scale_x, |base| base * model_scale);
    if entity.display_id != display || entity.scale_x != scale {
        entity.display_id = display;
        entity.scale_x = scale;
        entity.update(ctx);
    }
}

/// The newest transform Aura's model and its creature's scale, as vmangos `HandleAuraTransform`.
fn transform_display(ctx: &ReducerContext, guid: u64) -> Option<(u32, f32)> {
    ctx.db
        .game_aura()
        .by_target()
        .filter(&guid)
        .filter(|a| a.eff_kind == A_TRANSFORM && a.eff_p0_kind == P_ENTRY)
        .filter_map(|a| {
            let entry = u32::try_from(a.eff_p0).ok()?;
            let template = ctx.db.game_creature_template().entry().find(entry)?;
            let scale = if template.scale > 0.0 {
                template.scale
            } else {
                1.0
            };
            (template.display_id != 0).then_some((a.applied_at, a.id, template.display_id, scale))
        })
        .max_by_key(|&(applied_at, id, ..)| (applied_at, id))
        .map(|(_, _, display, scale)| (display, scale))
}

/// The model and scale of the unit's Form: a creature takes the Alliance model, as vmangos
/// `GetShapeshiftDisplayInfo` does.
fn form_display(ctx: &ReducerContext, unit: &WorldEntity) -> Option<(u32, f32)> {
    let form = crate::form::active_form(ctx, unit.guid)?;
    let horde = unit.is_player() && team_for_race(unit.race()) == TEAM_HORDE;
    let model = if horde {
        form.horde_display_id
    } else {
        form.alliance_display_id
    };
    (model != 0).then_some((model, form.display_scale))
}

/// The scale a model's scale multiplies: a creature's from its template, None if the template is
/// gone, and 1 for a Character (a known gap, see the character_visibility ledger entry).
fn base_scale(ctx: &ReducerContext, unit: &WorldEntity) -> Option<f32> {
    if unit.is_player() {
        return Some(1.0);
    }
    let template = ctx.db.game_creature_template().entry().find(unit.entry)?;
    Some(crate::creatures::catalogue::native_scale(
        ctx,
        &template,
        unit.native_display_id,
    ))
}
