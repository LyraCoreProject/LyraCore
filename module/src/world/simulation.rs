use spacetimedb::{ReducerContext, Table};

use super::{game_world_entity, WorldEntity};

impl WorldEntity {
    pub(crate) fn delete(ctx: &ReducerContext, guid: impl std::borrow::Borrow<u64>) -> bool {
        let guid = *guid.borrow();
        let removed = ctx.db.game_world_entity().guid().delete(guid);
        crate::breath::clear(ctx, guid);
        crate::combo::forget(ctx, guid);
        crate::transport::leave(ctx, guid);
        crate::aoi::entity_removed(ctx, guid);
        removed
    }

    fn stamp_simulation(&mut self) {
        use lyracore_shared::packing::power_type;

        self.combat_active =
            self.unit_flags & lyracore_shared::constants::unit_flags::IN_COMBAT != 0;
        let power_pending = self.max_power > 0
            && match self.power_type() {
                power_type::MANA | power_type::ENERGY => self.power != self.max_power,
                power_type::RAGE => self.power > 0,
                _ => false,
            };
        self.regeneration_pending = !self.dead && (self.health < self.max_health || power_pending);
        self.simulation_indexed = true;
    }

    pub(crate) fn insert(mut self, ctx: &ReducerContext) -> Self {
        self.stamp_simulation();
        let entity = ctx.db.game_world_entity().insert(self);
        crate::aoi::entity_changed(ctx, None, &entity);
        entity
    }

    pub(crate) fn update(mut self, ctx: &ReducerContext) -> Self {
        self.stamp_simulation();
        let old = ctx.db.game_world_entity().guid().find(self.guid);
        let entity = ctx.db.game_world_entity().guid().update(self);
        crate::aoi::entity_changed(ctx, old.as_ref(), &entity);
        if let Some(old) = old {
            if crate::breath::eligible(&old) && !crate::breath::eligible(&entity) {
                crate::breath::clear(ctx, entity.guid);
            }
            crate::native::entity_changed(ctx, &old, &entity);
        }
        entity
    }
}

const BACKFILL_BATCH_SIZE: usize = 512;

// Scheduled phases can resume only after this returns true.
pub(crate) fn backfill_simulation(ctx: &ReducerContext) -> bool {
    let entities = ctx.db.game_world_entity();
    let pending: Vec<_> = entities
        .by_simulation_indexed()
        .filter(false)
        .take(BACKFILL_BATCH_SIZE)
        .collect();
    for entity in pending {
        entity.update(ctx);
    }
    entities
        .by_simulation_indexed()
        .filter(false)
        .next()
        .is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::packing::power_type;

    fn entity() -> WorldEntity {
        let mut entity = crate::helpers::tests::entity(7, 0, 0, 0, 0);
        entity.health = 100;
        entity.max_health = 100;
        entity.max_power = 100;
        entity
    }

    #[test]
    fn regeneration_selects_deficits_and_rage_without_revisiting_full_pools() {
        for (kind, power, pending) in [
            (power_type::MANA, 100, false),
            (power_type::MANA, 99, true),
            (power_type::MANA, 101, true),
            (power_type::ENERGY, 100, false),
            (power_type::ENERGY, 0, true),
            (power_type::RAGE, 0, false),
            (power_type::RAGE, 100, true),
        ] {
            let mut entity = entity();
            entity.unit_bytes_0 = u32::from(kind) << 24;
            entity.power = power;
            entity.stamp_simulation();
            assert_eq!(entity.regeneration_pending, pending, "{kind}, {power}");
        }
    }

    #[test]
    fn health_and_death_transitions_replace_the_regeneration_predicate() {
        let mut entity = entity();
        entity.max_power = 0;
        entity.health = 99;
        entity.stamp_simulation();
        assert!(entity.regeneration_pending);
        entity.dead = true;
        entity.stamp_simulation();
        assert!(!entity.regeneration_pending);
        entity.dead = false;
        entity.health = 100;
        entity.stamp_simulation();
        assert!(!entity.regeneration_pending);
    }

    #[test]
    fn migration_defaults_are_replaced_from_the_live_gameplay_fields() {
        let mut entity = entity();
        entity.unit_flags = lyracore_shared::constants::unit_flags::IN_COMBAT;
        entity.health = 10;
        assert!(!entity.simulation_indexed);
        entity.stamp_simulation();
        assert!(entity.simulation_indexed);
        assert!(entity.regeneration_pending);
        assert!(entity.combat_active);
        entity.unit_flags = 0;
        entity.stamp_simulation();
        assert!(!entity.combat_active);
    }
}
