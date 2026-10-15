use lyracore_native_host::{Environment, Schema};
use serde_json::Value;
use spacetimedb::{ReducerContext, Table, Timestamp};

use super::prototype_reference::apply_request;
use crate::creatures::cycle::native_prototype::{admit_request, snapshot, Request, Scenario};
use crate::creatures::eventai::movement;
use crate::tables::*;

pub struct TableSimulation {
    world: Environment,
    guids: Vec<u64>,
    sequence: u64,
    at_micros: u64,
    changes: Vec<lyracore_native_host::Change>,
}

impl TableSimulation {
    pub fn new(scenario: &Scenario) -> Result<Self, String> {
        let ctx = ReducerContext::__dummy();
        let mut schema = Schema::default();
        macro_rules! register {
            ($($table:ident),* $(,)?) => { $(schema.register(ctx.db.$table())?;)* };
        }
        register!(
            game_world_entity,
            game_creature_spline,
            game_melee_attack,
            game_aura,
            game_creature_ai_movement_intent,
            game_creature_ai_state,
            game_creature_ai_definition,
            game_creature_spawn,
            game_creature_template,
            game_creature_spell,
            game_creature_cast,
            game_creature_ai_returning_home,
            game_config,
            game_terrain_chunk,
            game_nav_chunk,
            game_navigation_revision,
            game_vmap_generation,
            game_vmap_generation_chunk,
            game_vmap_chunk,
            game_vmap_nav_coverage_manifest,
            game_vmap_nav_coverage,
            game_go_collider,
            game_gameobject,
            game_gameobject_template,
            game_go_model
        );
        let mut world = Environment::new(schema)?;
        world.transaction(|| {
            for row in &scenario.entities {
                ctx.db.game_world_entity().insert(row.clone());
            }
            for row in &scenario.splines {
                ctx.db.game_creature_spline().insert(row.clone());
            }
            for row in &scenario.attacks {
                ctx.db.game_melee_attack().insert(row.clone());
            }
            Ok(())
        })?;
        Ok(Self {
            world,
            guids: scenario.entities.iter().map(|row| row.guid).collect(),
            sequence: 0,
            at_micros: scenario.start_micros,
            changes: Vec::new(),
        })
    }

    pub fn apply(&mut self, request: &Request) -> Result<(), String> {
        self.apply_batch(std::slice::from_ref(request))
    }

    pub fn apply_batch(&mut self, requests: &[Request]) -> Result<(), String> {
        let mut sequence = self.sequence;
        let mut at_micros = self.at_micros;
        self.world.transaction(|| {
            for request in requests {
                let mut ctx = ReducerContext::__dummy();
                admit_request(sequence, at_micros, request, |guid| {
                    ctx.db.game_world_entity().guid().find(guid).is_some()
                })?;
                ctx.timestamp = Timestamp::from_micros_since_unix_epoch(request.at_micros as i64);
                apply_request(&ctx, request);
                sequence = request.sequence;
                at_micros = request.at_micros;
            }
            Ok(())
        })?;
        self.sequence = sequence;
        self.at_micros = at_micros;
        self.changes = self.world.changes().to_vec();
        Ok(())
    }

    pub fn step(&mut self, request: &Request) -> Result<Value, String> {
        self.apply(request)?;
        self.snapshot()
    }

    pub fn commit_batch(&self, run: u64) -> crate::native_prototype::commit::Batch {
        use crate::native_prototype::commit::{Batch, RowChange};
        Batch {
            run,
            sequence: self.sequence,
            at_micros: self.at_micros,
            changes: self
                .changes
                .iter()
                .map(|change| RowChange {
                    table: change.table.clone(),
                    before: change.before.as_deref().map(<[u8]>::to_vec),
                    after: change.after.as_deref().map(<[u8]>::to_vec),
                })
                .collect(),
        }
    }

    pub fn snapshot(&mut self) -> Result<Value, String> {
        self.world.transaction(|| {
            let mut ctx = ReducerContext::__dummy();
            ctx.timestamp = Timestamp::from_micros_since_unix_epoch(self.at_micros as i64);
            Ok(snapshot(
                self.sequence,
                self.at_micros,
                self.guids
                    .iter()
                    .filter_map(|&guid| ctx.db.game_world_entity().guid().find(guid))
                    .collect(),
                self.guids
                    .iter()
                    .filter_map(|&guid| ctx.db.game_creature_spline().guid().find(guid))
                    .collect(),
                self.guids
                    .iter()
                    .filter_map(|&guid| ctx.db.game_melee_attack().attacker_guid().find(guid))
                    .collect(),
                self.guids
                    .iter()
                    .copied()
                    .filter(|&guid| {
                        movement::intent(&ctx, guid).is_some_and(|intent| intent.immobilized)
                    })
                    .collect(),
            ))
        })
    }
}
