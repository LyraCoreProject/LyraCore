use crate::{game_character, game_operator, game_world_entity, Character, WorldEntity};
use spacetimedb::{Identity, ReducerContext, Table};

pub(crate) fn require_operator(ctx: &ReducerContext) -> Result<(), String> {
    match ctx.db.game_operator().id().find(0) {
        Some(op) if op.identity == ctx.sender() => Ok(()),
        Some(_) => Err("operator only".to_string()),
        None => Err("operator not claimed".to_string()),
    }
}

pub(crate) fn entity_by_owner(ctx: &ReducerContext, owner: Identity) -> Option<WorldEntity> {
    gate_by_guid(
        ctx,
        ctx.db.game_world_entity().by_owner().filter(&owner).next(),
        |e| e.guid,
    )
}

pub(crate) fn acting_entity_by_guid(ctx: &ReducerContext, guid: u64) -> Option<WorldEntity> {
    gate_by_guid(ctx, ctx.db.game_world_entity().guid().find(guid), |e| {
        e.guid
    })
}

pub(crate) fn character_by_guid(ctx: &ReducerContext, guid: u64) -> Option<Character> {
    gate_by_guid(ctx, ctx.db.game_character().guid().find(guid), |c| c.guid)
}

/// Credits the live purse, or the stored Character's, and returns the copper that did not fit.
pub(crate) fn credit_purse(ctx: &ReducerContext, guid: u64, copper: u32) -> u32 {
    if let Some(mut entity) = acting_entity_by_guid(ctx, guid) {
        let (money, unpaid) = purse_credit(entity.money, copper);
        entity.money = money;
        entity.update(ctx);
        unpaid
    } else if let Some(mut character) = character_by_guid(ctx, guid) {
        let (money, unpaid) = purse_credit(character.money, copper);
        character.money = money;
        ctx.db.game_character().guid().update(character);
        unpaid
    } else {
        copper
    }
}

fn purse_credit(money: u32, copper: u32) -> (u32, u32) {
    let credited = copper.min(u32::MAX - money);
    (money + credited, copper - credited)
}

pub(crate) fn character_by_name(ctx: &ReducerContext, name: &str) -> Option<Character> {
    characters_by_name(ctx, name).next()
}

pub(crate) fn characters_by_name<'a>(
    ctx: &'a ReducerContext,
    name: &'a str,
) -> impl Iterator<Item = Character> + 'a {
    let characters: Vec<_> = ctx
        .db
        .game_character()
        .iter()
        .filter(|character| character.name.eq_ignore_ascii_case(name))
        .collect();
    characters
        .into_iter()
        .filter_map(move |character| gate_by_guid(ctx, Some(character), |character| character.guid))
}

fn gate_by_guid<T>(
    ctx: &ReducerContext,
    candidate: Option<T>,
    guid_of: impl Fn(&T) -> u64,
) -> Option<T> {
    let in_transit = candidate
        .as_ref()
        .is_some_and(|row| crate::transfer::is_in_transit(ctx, guid_of(row)));
    gate_in_transit(candidate, in_transit)
}

// Transfer-fenced Characters are absent from ordinary gameplay reads.
pub(crate) fn gate_in_transit<T>(candidate: Option<T>, in_transit: bool) -> Option<T> {
    if in_transit {
        None
    } else {
        candidate
    }
}

pub(crate) fn dist_sq(a: &WorldEntity, b: &WorldEntity) -> f32 {
    let (dx, dy, dz) = (b.x - a.x, b.y - a.y, b.z - a.z);
    dx * dx + dy * dy + dz * dz
}

pub(crate) fn entities_near(
    ctx: &ReducerContext,
    map_id: u32,
    instance_id: u64,
    x: f32,
    y: f32,
    radius: f32,
) -> Vec<WorldEntity> {
    let (gx0, gx1, gy0, gy1) = lyracore_shared::spatial::covering_cell_box(x, y, radius);
    let entities = ctx.db.game_world_entity();
    let mut out = Vec::new();
    for gx in gx0..=gx1 {
        for gy in gy0..=gy1 {
            out.extend(
                entities
                    .by_grid()
                    .filter((map_id, instance_id, gx, gy))
                    .filter(|e| in_same_partition(e, map_id, instance_id)),
            );
        }
    }
    out
}

pub(crate) fn in_same_partition(e: &WorldEntity, map_id: u32, instance_id: u64) -> bool {
    e.map_id == map_id && e.instance_id == instance_id
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn nearest_entity(
    ctx: &ReducerContext,
    origin: &WorldEntity,
    mut pred: impl FnMut(&WorldEntity) -> bool,
) -> Option<WorldEntity> {
    let mut best: Option<(WorldEntity, f32)> = None;
    for e in ctx.db.game_world_entity().by_map().filter(&origin.map_id) {
        if !in_same_partition(&e, origin.map_id, origin.instance_id) || !pred(&e) {
            continue;
        }
        let d = (e.x - origin.x).powi(2) + (e.y - origin.y).powi(2);
        if best.as_ref().is_none_or(|(_, bd)| d < *bd) {
            best = Some((e, d));
        }
    }
    best.map(|(e, _)| e)
}

pub(crate) fn event_recipient_identity(bound: Option<Identity>) -> Identity {
    bound.unwrap_or(Identity::ZERO)
}

pub(crate) fn live_entity(ctx: &ReducerContext, guid: u64) -> Result<WorldEntity, String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(guid)
        .ok_or_else(|| format!("no live entity for guid {guid}"))
}

#[cfg(feature = "debug_reducers")]
pub(crate) fn require_character(ctx: &ReducerContext, guid: u64) -> Result<Character, String> {
    character_by_guid(ctx, guid).ok_or_else(|| format!("no character {guid}"))
}

pub(crate) fn grid_of(ctx: &ReducerContext, guid: u64) -> (u32, u64, i32, i32) {
    ctx.db
        .game_world_entity()
        .guid()
        .find(guid)
        .map(|e| (e.map_id, e.instance_id, e.grid_x, e.grid_y))
        .unwrap_or((0, 0, 0, 0))
}

pub(crate) fn entity_addr(e: &WorldEntity) -> (u32, u64, i32, i32) {
    (e.map_id, e.instance_id, e.grid_x, e.grid_y)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn purse_credit_pays_copper_that_fits() {
        assert_eq!(purse_credit(100, 50), (150, 0));
    }

    #[test]
    fn purse_credit_returns_copper_beyond_a_full_purse() {
        assert_eq!(purse_credit(u32::MAX - 10, 25), (u32::MAX, 15));
    }

    #[test]
    fn purse_credit_pays_nothing_into_a_full_purse() {
        assert_eq!(purse_credit(u32::MAX, 25), (u32::MAX, 25));
    }

    pub(crate) fn entity(
        guid: u64,
        map_id: u32,
        instance_id: u64,
        grid_x: i32,
        grid_y: i32,
    ) -> WorldEntity {
        WorldEntity {
            grid_x,
            grid_y,
            cell: lyracore_shared::spatial::grid_cell_id(grid_x, grid_y),
            health: 1,
            max_health: 1,
            level: 1,
            ..WorldEntity::blank(guid, map_id, instance_id, [0.0; 4])
        }
    }
}
