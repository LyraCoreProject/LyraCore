use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, TimeDuration};

use crate::encounter::{
    self, EncounterSignal, DOOR_OPEN_STATE, ENCOUNTER_DONE, ENCOUNTER_FAILED, ENCOUNTER_IN_PROGRESS,
};
use crate::{game_creature_template, game_gameobject, game_instance, game_world_entity};

const MAP_ID: u32 = 230;
const ENCOUNTER_ID: u32 = 4;
const TOMB_DWARVES: [u32; 7] = [9034, 9035, 9036, 9037, 9038, 9039, 9040];
const TOMB_ENTRANCE: u32 = 170576;
const TOMB_EXIT: u32 = 170577;
const CHEST_OF_THE_SEVEN: u32 = 169243;
const DWARF_HOSTILE_FACTION: u32 = 754;
const ROUND_DELAY_MICROS: i64 = 30_000_000;

#[table(
    accessor = blackrock_tomb_round,
    scheduled(advance_tomb_round),
    index(accessor = by_instance, btree(columns = [instance_id]))
)]
pub struct BlackrockTombRound {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub instance_id: u64,
    pub next_round: u8,
}

crate::encounter_package!(BlackrockDepthsTombOfSeven, fn tomb_of_seven(ctx, instance_id, signal) {
    match signal {
        EncounterSignal::Begin => {
            clear_round_timer(ctx, instance_id);
            activate_dwarf(ctx, instance_id, 0)?;
            schedule_round(ctx, instance_id, 1);
            set_gameobject_state(ctx, instance_id, TOMB_ENTRANCE, 0);
            encounter::set_encounter_state(ctx, instance_id, ENCOUNTER_ID, ENCOUNTER_IN_PROGRESS)
        }
        EncounterSignal::Fail => {
            clear_round_timer(ctx, instance_id);
            revive_tomb_dwarves(ctx, instance_id);
            set_gameobject_state(ctx, instance_id, TOMB_ENTRANCE, DOOR_OPEN_STATE);
            encounter::set_encounter_state(ctx, instance_id, ENCOUNTER_ID, ENCOUNTER_FAILED)
        }
        EncounterSignal::Complete => {
            clear_round_timer(ctx, instance_id);
            set_gameobject_state(ctx, instance_id, TOMB_ENTRANCE, DOOR_OPEN_STATE);
            set_gameobject_state(ctx, instance_id, TOMB_EXIT, DOOR_OPEN_STATE);
            set_gameobject_state(ctx, instance_id, CHEST_OF_THE_SEVEN, 0);
            encounter::set_encounter_state(ctx, instance_id, ENCOUNTER_ID, ENCOUNTER_DONE)
        }
        other => Err(format!("Tomb of Seven does not accept encounter signal {other:?}")),
    }
});

crate::game_hook!(on_creature_death, fn tomb_dwarf_died(ctx, payload) {
    if !TOMB_DWARVES.contains(&payload.entry)
        || !instance_belongs_to_blackrock(ctx, payload.instance_id)
        || encounter::get_encounter_state(ctx, payload.instance_id, ENCOUNTER_ID)
            != ENCOUNTER_IN_PROGRESS
    {
        return;
    }
    let Some(timer) = ctx
        .db
        .blackrock_tomb_round()
        .by_instance()
        .filter(&payload.instance_id)
        .next()
    else {
        return;
    };
    let active_round = timer.next_round.saturating_sub(1) as usize;
    if TOMB_DWARVES.get(active_round) != Some(&payload.entry) {
        return;
    }
    ctx.db
        .blackrock_tomb_round()
        .scheduled_id()
        .delete(timer.scheduled_id);
    activate_and_schedule(ctx, payload.instance_id, timer.next_round);
});

#[reducer]
pub fn advance_tomb_round(ctx: &ReducerContext, timer: BlackrockTombRound) {
    if ctx.sender() == ctx.database_identity()
        && instance_belongs_to_blackrock(ctx, timer.instance_id)
        && encounter::get_encounter_state(ctx, timer.instance_id, ENCOUNTER_ID)
            == ENCOUNTER_IN_PROGRESS
    {
        activate_and_schedule(ctx, timer.instance_id, timer.next_round);
    }
}

fn activate_and_schedule(ctx: &ReducerContext, instance_id: u64, round: u8) {
    if let Err(error) = activate_dwarf(ctx, instance_id, round) {
        spacetimedb::log::warn!("Tomb of Seven round {} could not start: {error}", round + 1);
        schedule_round(ctx, instance_id, round);
        return;
    }
    if usize::from(round) + 1 < TOMB_DWARVES.len() {
        schedule_round(ctx, instance_id, round + 1);
    }
}

fn activate_dwarf(ctx: &ReducerContext, instance_id: u64, round: u8) -> Result<(), String> {
    let entry = *TOMB_DWARVES
        .get(usize::from(round))
        .ok_or_else(|| format!("Tomb of Seven round {round} is outside the dwarf roster"))?;
    let entities = ctx.db.game_world_entity();
    let player_guid = entities
        .by_map()
        .filter(&MAP_ID)
        .find(|entity| entity.instance_id == instance_id && entity.is_player() && !entity.dead)
        .map(|entity| entity.guid)
        .ok_or_else(|| format!("Tomb of Seven instance {instance_id} has no living player"))?;
    let mut dwarf = entities
        .by_map()
        .filter(&MAP_ID)
        .find(|entity| entity.instance_id == instance_id && entity.entry == entry && !entity.dead)
        .ok_or_else(|| format!("Tomb of Seven dwarf {entry} is missing or dead"))?;
    let original_faction = dwarf.faction_template;
    dwarf.faction_template = DWARF_HOSTILE_FACTION;
    let dwarf_guid = dwarf.guid;
    entities.guid().update(dwarf);
    if !crate::combat::arm_creature_engagement(ctx, dwarf_guid, player_guid, false) {
        if let Some(mut dwarf) = entities.guid().find(dwarf_guid) {
            dwarf.faction_template = original_faction;
            entities.guid().update(dwarf);
        }
        return Err(format!(
            "Tomb of Seven dwarf {entry} already has an outgoing engagement"
        ));
    }
    Ok(())
}

fn schedule_round(ctx: &ReducerContext, instance_id: u64, next_round: u8) {
    let scheduled_at = ScheduleAt::Time(
        ctx.timestamp
            .checked_add(TimeDuration::from_micros(ROUND_DELAY_MICROS))
            .unwrap_or(ctx.timestamp),
    );
    ctx.db.blackrock_tomb_round().insert(BlackrockTombRound {
        scheduled_id: 0,
        scheduled_at,
        instance_id,
        next_round,
    });
}

fn clear_round_timer(ctx: &ReducerContext, instance_id: u64) {
    let timers = ctx.db.blackrock_tomb_round();
    let ids: Vec<u64> = timers
        .by_instance()
        .filter(&instance_id)
        .map(|timer| timer.scheduled_id)
        .collect();
    for id in ids {
        timers.scheduled_id().delete(id);
    }
}

fn instance_belongs_to_blackrock(ctx: &ReducerContext, instance_id: u64) -> bool {
    ctx.db
        .game_instance()
        .instance_id()
        .find(instance_id)
        .is_some_and(|instance| instance.map_id == MAP_ID)
}

fn set_gameobject_state(
    ctx: &spacetimedb::ReducerContext,
    instance_id: u64,
    entry: u32,
    state: u8,
) {
    let gameobjects = ctx.db.game_gameobject();
    let guids: Vec<u64> = gameobjects
        .by_map()
        .filter(&MAP_ID)
        .filter(|go| go.instance_id == instance_id && go.template_entry == entry)
        .map(|go| go.guid)
        .collect();
    for guid in guids {
        if let Some(mut go) = gameobjects.guid().find(guid) {
            go.state = state;
            go.respawn_at_micros = 0;
            gameobjects.guid().update(go);
        }
    }
}

fn revive_tomb_dwarves(ctx: &spacetimedb::ReducerContext, instance_id: u64) {
    let entities = ctx.db.game_world_entity();
    let guids: Vec<u64> = entities
        .by_map()
        .filter(&MAP_ID)
        .filter(|entity| entity.instance_id == instance_id && TOMB_DWARVES.contains(&entity.entry))
        .map(|entity| entity.guid)
        .collect();
    for guid in guids {
        crate::combat::disengage(ctx, guid);
        crate::creatures::reset_creature_lifecycle(ctx, guid);
        if let Some(mut dwarf) = entities.guid().find(guid) {
            if dwarf.dead {
                crate::loot::reap_corpse_loot_family(ctx, guid);
                dwarf.dead = false;
                dwarf.health = dwarf.max_health;
                dwarf.dynamic_flags = 0;
                dwarf.money = 0;
            }
            if let Some(template) = ctx.db.game_creature_template().entry().find(dwarf.entry) {
                dwarf.faction_template = template.faction_template;
            }
            entities.guid().update(dwarf);
        }
    }
}
