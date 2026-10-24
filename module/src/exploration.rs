use spacetimedb::{table, ReducerContext, Table};

use crate::xp::XpEvent;
use crate::WorldEntity;

#[table(accessor = game_character_explored, public, index(accessor = by_character, btree(columns = [character_guid])))]
pub struct CharacterExplored {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub character_guid: u64,
    pub area_bit: i32,
    #[default(0)]
    pub area_id: u32,
    #[default(0)]
    pub experience: u32,
}

crate::character_owned!(delete, fn sweep_delete_game_character_explored(ctx, character_guid) {
    let ex = ctx.db.game_character_explored();
    for r in ex.by_character().filter(&character_guid).collect::<Vec<_>>() {
        ex.id().delete(r.id);
    }
});
crate::character_owned!(transfer, fn sweep_transfer_game_character_explored(ctx, character_guid, io) {
    table = game_character_explored,
    by = by_character,
    remint = id,
});

pub(crate) fn check_area_exploration(
    ctx: &ReducerContext,
    mover: &mut WorldEntity,
    area: &crate::GameArea,
) {
    if area.area_bit < 0 {
        return;
    }
    let already = ctx
        .db
        .game_character_explored()
        .by_character()
        .filter(&mover.guid)
        .any(|r| r.area_bit == area.area_bit);
    if already {
        return;
    }
    let xp = if area.exploration_level > 0 {
        crate::xp::rated_xp(
            ctx,
            crate::xp::explore_xp(area.exploration_level as u32, mover.level),
        )
    } else {
        0
    };
    let explored = ctx.db.game_character_explored().insert(CharacterExplored {
        id: 0,
        character_guid: mover.guid,
        area_bit: area.area_bit,
        area_id: area.id,
        experience: xp,
    });
    crate::native::explored(ctx, &explored);
    if xp > 0 {
        crate::xp::grant_xp(ctx, mover, xp);
        XpEvent::insert(
            ctx,
            mover.guid,
            XpEvent {
                id: 0,
                recipient_identity: mover.owner_identity,
                killed_guid: 0,
                total_exp: xp,
                created_at: ctx.timestamp,
                is_kill: false,
                rested_bonus: 0,
            },
        );
    }
}

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_explore_at(
    ctx: &ReducerContext,
    guid: u64,
    map_id: u32,
    x: f32,
    y: f32,
) -> Result<(), String> {
    let mut e = crate::helpers::live_entity(ctx, guid)?;
    e.map_id = map_id;
    e.x = x;
    e.y = y;
    if let Some(area) = crate::terrain::area_at(ctx, map_id, x, y) {
        check_area_exploration(ctx, &mut e, &area);
    }
    e.update(ctx);
    Ok(())
}
