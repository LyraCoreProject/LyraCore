use crate::{game_aura, game_creature_template, game_world_entity};
use lyracore_shared::spell::taxonomy::{A_TRANSFORM, P_ENTRY};
use spacetimedb::ReducerContext;

pub(crate) fn recompute_display(ctx: &ReducerContext, guid: u64) {
    let Some(mut entity) = ctx.db.game_world_entity().guid().find(guid) else {
        return;
    };
    let display = ctx
        .db
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
        .unwrap_or_else(|| super::form::display(&entity).unwrap_or(entity.native_display_id));
    if entity.display_id != display {
        entity.display_id = display;
        entity.update(ctx);
    }
}
