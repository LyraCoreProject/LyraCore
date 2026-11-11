use std::collections::{BTreeMap, BTreeSet};

use spacetimedb::{table, ReducerContext, Table};

use crate::{game_creature_ai_state, game_melee_attack, game_threat, game_world_entity};

// vmangos GridStates checks active grids every GridCleanUpDelay / 10 (300 seconds / 10).
const CHECK_INTERVAL_MS: u64 = 30_000;
const GRID_SIZE: f32 = 533.333_3;
const CELLS_PER_GRID: i32 = 16;
const CELL_SIZE: f32 = GRID_SIZE / CELLS_PER_GRID as f32;

type Grid = (u32, u64, i32, i32);

#[table(
    accessor = game_creature_combat_grid,
    index(accessor = by_grid, btree(columns = [map_id, instance_id, grid_x, grid_y]))
)]
pub struct CreatureCombatGrid {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub map_id: u32,
    pub instance_id: u64,
    pub grid_x: i32,
    pub grid_y: i32,
    pub check_at_ms: u64,
}

crate::outside_table_catalog!(game_creature_combat_grid);

pub(super) fn evade_inactive_grids(ctx: &ReducerContext) {
    let now_ms = crate::time::now_ms(ctx);
    let entities = ctx.db.game_world_entity();
    let melee = ctx.db.game_melee_attack();
    let candidates: BTreeSet<_> = ctx
        .db
        .game_threat()
        .iter()
        .map(|t| t.creature_guid)
        .collect();
    let mut grids: BTreeMap<Grid, Vec<u64>> = BTreeMap::new();
    for guid in candidates {
        let Some(c) = entities.guid().find(guid) else {
            continue;
        };
        if c.is_player()
            || c.dead
            || c.owner_guid != 0
            || melee.attacker_guid().find(guid).is_some()
        {
            continue;
        }
        if crate::threat::top_threat_target(ctx, guid).is_none() {
            super::swing::evade(ctx, guid);
            continue;
        }
        grids
            .entry((
                c.map_id,
                c.instance_id,
                coordinate(c.x, GRID_SIZE, 32),
                coordinate(c.y, GRID_SIZE, 32),
            ))
            .or_default()
            .push(guid);
    }
    let timers = ctx.db.game_creature_combat_grid();
    for row in timers.iter() {
        if !grids.contains_key(&(row.map_id, row.instance_id, row.grid_x, row.grid_y)) {
            timers.id().delete(row.id);
        }
    }
    for (grid, creatures) in grids {
        let (map_id, instance_id, grid_x, grid_y) = grid;
        let Some(mut timer) = timers.by_grid().filter(grid).next() else {
            timers.insert(CreatureCombatGrid {
                id: 0,
                map_id,
                instance_id,
                grid_x,
                grid_y,
                check_at_ms: now_ms.saturating_add(CHECK_INTERVAL_MS),
            });
            continue;
        };
        if now_ms < timer.check_at_ms {
            continue;
        }
        if active_objects_near_grid(ctx, grid) {
            timer.check_at_ms = now_ms.saturating_add(CHECK_INTERVAL_MS);
            timers.id().update(timer);
        } else {
            timers.id().delete(timer.id);
            for creature in creatures {
                super::swing::evade(ctx, creature);
            }
        }
    }
}

// GridDefines::Compute uses double intermediates and a half-cell offset.
fn coordinate(position: f32, size: f32, center: i32) -> i32 {
    ((f64::from(position) - f64::from(size / 2.0)) / f64::from(size) + f64::from(center) + 0.5)
        as i32
}

fn active_objects_near_grid(ctx: &ReducerContext, grid: Grid) -> bool {
    let (map_id, instance_id, grid_x, grid_y) = grid;
    let visibility = if lyracore_shared::instance::BATTLEGROUND_MAPS.contains(&map_id) {
        533.0
    } else if lyracore_shared::instance::is_dungeon_map(map_id) {
        170.0
    } else {
        100.0
    };
    let margin = (visibility / CELL_SIZE).ceil() as i32 + 1;
    let center_x = (grid_x as f32 - 32.0 + 0.5) * GRID_SIZE;
    let center_y = (grid_y as f32 - 32.0 + 0.5) * GRID_SIZE;
    let radius = GRID_SIZE / 2.0 + (margin + 1) as f32 * CELL_SIZE;
    crate::helpers::entities_near(ctx, map_id, instance_id, center_x, center_y, radius)
        .into_iter()
        .any(|e| {
            let active = e.is_player()
                || ctx
                    .db
                    .game_creature_ai_state()
                    .creature_guid()
                    .find(e.guid)
                    .is_some_and(|state| state.active_object);
            let x = coordinate(e.x, CELL_SIZE, 512);
            let y = coordinate(e.y, CELL_SIZE, 512);
            active
                && (grid_x * CELLS_PER_GRID - margin
                    ..=grid_x * CELLS_PER_GRID + CELLS_PER_GRID + margin)
                    .contains(&x)
                && (grid_y * CELLS_PER_GRID - margin
                    ..=grid_y * CELLS_PER_GRID + CELLS_PER_GRID + margin)
                    .contains(&y)
        })
}
