use lyracore_package_delta::packed_creature_spawn_guid;
use spacetimedb::{ReducerContext, TimeDuration};

use crate::{game_corpse_loot, game_gameobject, game_gameobject_pool_member, game_loot_roll};

use super::*;

pub(crate) fn despawn_creature_entity(ctx: &ReducerContext, guid: u64) {
    crate::combat::disengage(ctx, guid);
    crate::threat::clear_taunt_lock(ctx, guid);
    crate::combat::clear_lethal_damage_floor(ctx, guid);
    crate::creatures::cancel_relay_runs_for_source(ctx, guid);
    crate::creatures::reset_creature_lifecycle(ctx, guid);
    crate::creatures::drop_summon_expiry(ctx, guid);
    let _ = crate::creatures::set_creature_gossip_menu(ctx, guid, None);
    crate::loot::tag::clear(ctx, guid);
    ctx.db.game_creature_spline().guid().delete(guid);
    crate::motion::drop_pending(ctx, guid);
    ctx.db.game_entity_motion().guid().delete(guid);
    crate::loot::reap_corpse_loot_family(ctx, guid);
    // Delete last; its Relay destroys the client object.
    crate::WorldEntity::delete(ctx, guid);
}

pub(crate) fn pass_decay(ctx: &ReducerContext) -> usize {
    let now_ts = ctx.timestamp;
    let entities = ctx.db.game_world_entity();
    let spawns = ctx.db.game_creature_spawn();

    let mut visited = 0usize;
    let decaying: Vec<u64> = spawns
        .by_despawn_at()
        .filter(..=now_ts)
        .inspect(|_| visited += 1)
        .filter(|s| {
            entities
                .guid()
                .find(s.guid)
                .map(|e| e.dead)
                .unwrap_or(false)
        })
        .map(|s| s.guid)
        .collect();

    if !decaying.is_empty() {
        crate::loot::sweep_loot_rolls(ctx);
    }
    for guid in decaying {
        if ctx
            .db
            .game_loot_roll()
            .by_corpse()
            .filter(&guid)
            .any(|r| !r.resolved)
            || ctx
                .db
                .game_corpse_loot()
                .by_corpse()
                .filter(&guid)
                .any(|l| l.withheld)
        {
            continue;
        }

        crate::creatures::despawn_creature_entity(ctx, guid);
        if let Some(mut spawn) = spawns.guid().find(guid) {
            spawn.respawn_at = if spawn.respawn_secs == 0 {
                now_ts
                    .checked_add(TimeDuration::from_micros(crate::combat::RESPAWN_MICROS))
                    .unwrap_or(now_ts)
            } else {
                let death_ts = spawn
                    .despawn_at
                    .checked_sub(TimeDuration::from_micros(
                        crate::combat::CORPSE_DECAY_MICROS,
                    ))
                    .unwrap_or(spawn.despawn_at);
                let armed = death_ts
                    .checked_add(TimeDuration::from_micros(
                        spawn.respawn_secs as i64 * 1_000_000,
                    ))
                    .unwrap_or(now_ts);
                // Arm respawn in the future so decay cannot respawn the creature in this firing.
                if armed > now_ts {
                    armed
                } else {
                    now_ts
                }
            };
            spawn.despawn_at = crate::creatures::timer_never(ctx);
            spawns.guid().update(spawn);
        }
    }
    visited
}

pub(crate) fn pass_respawn(ctx: &ReducerContext) -> usize {
    let now_ts = ctx.timestamp;
    let entities = ctx.db.game_world_entity();
    let spawns = ctx.db.game_creature_spawn();
    let templates = ctx.db.game_creature_template();

    let mut visited = 0usize;
    const MAX_RESPAWNS_PER_TICK: usize = 256;
    let due: Vec<CreatureSpawn> = spawns
        .by_respawn_at()
        .filter(..=now_ts)
        .take(MAX_RESPAWNS_PER_TICK)
        .inspect(|_| visited += 1)
        .collect();
    let mut next_waypoint_id = None;
    for mut spawn in due {
        let guid = spawn.guid;
        if (spawn.instance_id == 0
            && lyracore_shared::instance::has_instance_population(spawn.map_id))
            || entities.guid().find(guid).is_some()
        {
            spawn.respawn_at = crate::creatures::timer_never(ctx);
            spawns.guid().update(spawn);
            continue;
        }
        let Some(entry) = crate::creatures::population::entry_for_spawn(ctx, &spawn) else {
            spawn.respawn_at = crate::creatures::timer_never(ctx);
            spawns.guid().update(spawn);
            continue;
        };
        if spawn.source_guid == 0 && entry != spawn.entry {
            spawn.respawn_at = crate::creatures::timer_never(ctx);
            spawns.guid().update(spawn.clone());
            spawn.source_guid = guid;
            spawn.guid = packed_creature_spawn_guid(entry, guid as u32);
            spawn.entry = entry;
            if spawns.guid().find(spawn.guid).is_some() {
                continue;
            }
            if let Err(reason) =
                crate::instance::copy_spawn_waypoints(ctx, guid, spawn.guid, &mut next_waypoint_id)
            {
                spacetimedb::log::error!("creature source {guid} cannot copy its patrol: {reason}");
                continue;
            }
            spawns.insert(spawn.clone());
        }
        let guid = spawn.guid;
        crate::combat::clear_lethal_damage_floor(ctx, guid);
        crate::creatures::cancel_relay_runs_for_new_life(ctx, guid);
        // Advance and disarm the spawn before the new entity exposes this life.
        if let Some(mut s) = spawns.guid().find(guid) {
            s.respawn_at = crate::creatures::timer_never(ctx);
            s.life_seq = s.life_seq.saturating_add(1);
            spawns.guid().update(s);
        }
        if let Some(tmpl) = templates.entry().find(spawn.entry) {
            super::spawn::insert_creature_entity(
                ctx,
                build_creature_entity(&spawn, &tmpl, ctx.random(), spawn.instance_id),
            );
        }
    }
    visited
}

pub(crate) fn pass_gameobject_respawn(ctx: &ReducerContext) -> usize {
    let now = ctx.timestamp.to_micros_since_unix_epoch() as u64;
    let gos = ctx.db.game_gameobject();
    let members = ctx.db.game_gameobject_pool_member();
    let mut visited = 0usize;
    let due: Vec<u64> = gos
        .by_respawn_at()
        .filter(1..=now)
        .inspect(|_| visited += 1)
        .filter(|g| crate::gameobject::respawn_due(g.state, g.respawn_at_micros, now))
        .map(|g| g.guid)
        .collect();
    for guid in due {
        match crate::gameobject::pool_point_id_of(guid).and_then(|pid| members.point_id().find(pid))
        {
            Some(member) => crate::gameobject::reroll_pool(ctx, member.pool_id, guid),
            None => {
                if let Some(mut go) = gos.guid().find(guid) {
                    go.state = 0;
                    go.respawn_at_micros = 0;
                    go.update(ctx);
                }
            }
        }
    }
    visited
}
