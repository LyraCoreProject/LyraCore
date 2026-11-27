use crate::{game_aura, game_creature_template, game_world_entity, WorldEntity};
use lyracore_shared::faction::{team_for_race, TEAM_HORDE};
use lyracore_shared::spell::taxonomy::{A_TRANSFORM, P_ENTRY};
use spacetimedb::ReducerContext;

/// Shows the unit's transform Aura model, else its Form's model for its team, else its own model.
/// A transform leaves the scale as it is.
pub(crate) fn recompute_display(ctx: &ReducerContext, guid: u64) {
    let Some(mut entity) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let (display, scale) = match transform_display(ctx, guid) {
        Some(display) => (display, entity.scale_x),
        None => form_display(ctx, &entity)
            .unwrap_or_else(|| (entity.native_display_id, native_scale(ctx, &entity))),
    };
    if entity.display_id != display || entity.scale_x != scale {
        entity.display_id = display;
        entity.scale_x = scale;
        entity.update(ctx);
    }
}

fn transform_display(ctx: &ReducerContext, guid: u64) -> Option<u32> {
    ctx.db
        .game_aura()
        .by_target()
        .filter(&guid)
        .filter(|a| a.eff_kind == A_TRANSFORM && a.eff_p0_kind == P_ENTRY)
        .filter_map(|a| {
            let entry = u32::try_from(a.eff_p0).ok()?;
            let template = ctx.db.game_creature_template().entry().find(entry)?;
            (template.display_id != 0).then_some((a.applied_at, a.id, template.display_id))
        })
        .max_by_key(|&(applied_at, id, _)| (applied_at, id))
        .map(|(_, _, display)| display)
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

/// A Character is drawn at scale 1; a creature at the scale of its native model.
fn native_scale(ctx: &ReducerContext, unit: &WorldEntity) -> f32 {
    if unit.is_player() {
        return 1.0;
    }
    let Some(template) = ctx.db.game_creature_template().entry().find(unit.entry) else {
        return unit.scale_x;
    };
    let scale = crate::creatures::hunter_pet::model_scale(ctx, &template, unit.native_display_id);
    if scale > 0.0 {
        scale
    } else {
        1.0
    }
}
