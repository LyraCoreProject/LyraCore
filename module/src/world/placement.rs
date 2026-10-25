use lyracore_shared::spatial;
use spacetimedb::ReducerContext;

use super::WorldEntity;

/// The value a placement writes to `last_move_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MoveClock {
    At(u32),
    Unchanged,
}

impl MoveClock {
    /// Native World Sessions supply client time; bots and creatures use the Realm Clock.
    pub(crate) fn realm(ctx: &ReducerContext, guid: u64, now_ms: u32) -> Self {
        if crate::native::is_player(ctx, guid) {
            Self::Unchanged
        } else {
            Self::At(now_ms)
        }
    }
}

/// Whether a placement moved the entity into another cell; `explore` applies the consequences.
#[must_use = "explore the entered cell, or call `unexplored` to name why not"]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CellCrossing(bool);

impl WorldEntity {
    /// Moves to a position and derives its grid address and packed cell together.
    pub(crate) fn place(&mut self, x: f32, y: f32, z: f32, clock: MoveClock) -> CellCrossing {
        let previous_grid = (self.grid_x, self.grid_y);
        let (grid_x, grid_y) = spatial::grid_cell(x, y);
        self.x = x;
        self.y = y;
        self.z = z;
        self.grid_x = grid_x;
        self.grid_y = grid_y;
        self.cell = spatial::grid_cell_id(grid_x, grid_y);
        if let MoveClock::At(ms) = clock {
            self.last_move_ms = ms;
        }
        CellCrossing((grid_x, grid_y) != previous_grid)
    }
}

impl CellCrossing {
    /// A living Character that entered a cell explores its area and moves into its zone.
    pub(crate) fn explore(self, ctx: &ReducerContext, entity: &mut WorldEntity) {
        if !self.0 || !entity.is_player() || entity.dead {
            return;
        }
        if let Some(area) = crate::terrain::area_at(ctx, entity.map_id, entity.x, entity.y) {
            crate::exploration::check_area_exploration(ctx, entity, &area);
            super::apply_zone_transition(ctx, entity, Some(crate::terrain::zone_of(&area)));
        }
    }

    pub(crate) fn unexplored(self) {}
}

#[cfg(test)]
mod tests {
    use super::{CellCrossing, MoveClock};
    use lyracore_shared::spatial;

    fn entity_at(x: f32) -> crate::WorldEntity {
        let (grid_x, grid_y) = spatial::grid_cell(x, 0.0);
        let mut entity = crate::helpers::tests::entity(7, 0, 0, grid_x, grid_y);
        entity.x = x;
        entity.cell = spatial::grid_cell_id(grid_x, grid_y);
        entity
    }

    #[test]
    fn a_placement_past_a_cell_edge_moves_the_grid_address_and_reports_the_crossing() {
        let mut entity = entity_at(15.0);
        let before = entity.cell;
        assert_eq!(
            entity.place(20.0, 0.0, 1.0, MoveClock::Unchanged),
            CellCrossing(true)
        );
        assert_ne!(entity.cell, before);
        assert_eq!(
            entity.cell,
            spatial::grid_cell_id(entity.grid_x, entity.grid_y)
        );
        assert_eq!((entity.x, entity.y, entity.z), (20.0, 0.0, 1.0));
        assert_eq!(
            entity.place(21.0, 0.0, 1.0, MoveClock::Unchanged),
            CellCrossing(false)
        );
    }

    #[test]
    fn a_placement_writes_the_move_clock_only_when_given_one() {
        let mut entity = entity_at(15.0);
        entity.last_move_ms = 100;
        entity
            .place(16.0, 0.0, 0.0, MoveClock::Unchanged)
            .unexplored();
        assert_eq!(entity.last_move_ms, 100);
        entity
            .place(16.0, 0.0, 0.0, MoveClock::At(250))
            .unexplored();
        assert_eq!(entity.last_move_ms, 250);
    }
}
