use spacetimedb::{reducer, sys, table, ReducerContext, Table, Timestamp};

use crate::creatures::cycle::TickContext;
use crate::creatures::TickScope;
use crate::tables::*;

const START_MICROS: u64 = 9_000_000_000_000_000;
const ENTRY: u32 = 900_001;

#[table(accessor = game_native_gameplay_state, public)]
pub struct NativeGameplayState {
    #[primary_key]
    pub id: u8,
    pub scenario: String,
    pub creatures: u32,
    pub sequence: u64,
    pub at_micros: u64,
    pub awake: u64,
    pub phases: Vec<String>,
    pub visited: Vec<u64>,
}

#[reducer]
pub fn debug_native_gameplay_setup(
    ctx: &ReducerContext,
    scenario: String,
    creatures: u32,
    tables: Vec<String>,
    schedules: Vec<String>,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if !matches!(scenario.as_str(), "periodic_deaths" | "creature_cycle")
        || !(2..=5_000).contains(&creatures)
    {
        return Err("unknown gameplay fixture or creature count outside 2..=5000".into());
    }
    if ctx.db.game_native_gameplay_state().count() != 0 || ctx.db.game_session().count() != 0 {
        return Err("gameplay fixtures require a fresh disposable Shard without Sessions".into());
    }
    clear_tables(&tables)?;
    let clock = fixture_clock(START_MICROS);
    crate::seed::init(&clock);
    clear_tables(&schedules)?;
    for name in [
        "game_world_entity",
        "game_creature_spawn",
        "game_creature_spline",
        "game_creature_waypoint",
    ] {
        clear_tables(&[name.to_owned()])?;
    }
    crate::debug::debug_spawn_player_entity(&clock, 1)?;
    let mut character = clock.db.game_world_entity().guid().find(1).unwrap();
    character.health = 1_000_000;
    character.max_health = 1_000_000;
    clock
        .db
        .game_world_entity()
        .guid()
        .update(character.clone());
    populate(&clock, &scenario, creatures, &character)?;
    ctx.db
        .game_native_gameplay_state()
        .insert(NativeGameplayState {
            id: 0,
            scenario,
            creatures,
            sequence: 0,
            at_micros: START_MICROS,
            awake: 0,
            phases: Vec::new(),
            visited: Vec::new(),
        });
    Ok(())
}

fn clear_tables(names: &[String]) -> Result<(), String> {
    for name in names {
        if !name.starts_with("game_") || name == "game_operator" {
            return Err(format!("not a disposable gameplay table: {name}"));
        }
        let id = sys::table_id_from_name(name).map_err(|error| error.to_string())?;
        sys::datastore_clear(id).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn populate(
    ctx: &ReducerContext,
    scenario: &str,
    creatures: u32,
    character: &WorldEntity,
) -> Result<(), String> {
    if scenario == "creature_cycle" {
        for (id, group, enemy) in [(1, 1, 2), (14, 2, 1)] {
            ctx.db.game_faction_template().insert(FactionTemplate {
                id,
                faction: id,
                faction_group: group,
                friend_group: group,
                enemy_group: enemy,
                enemy_0: 0,
                enemy_1: 0,
                enemy_2: 0,
                enemy_3: 0,
                friend_0: 0,
                friend_1: 0,
                friend_2: 0,
                friend_3: 0,
            });
        }
    }
    let mut template = ctx
        .db
        .game_creature_template()
        .entry()
        .find(51_000)
        .unwrap();
    template.entry = ENTRY;
    template.health = if scenario == "periodic_deaths" {
        1
    } else {
        10_000
    };
    template.money_min = 1;
    template.money_max = 1_000;
    template.aggro_range = 15;
    ctx.db.game_creature_template().insert(template);
    let template = ctx.db.game_creature_template().entry().find(ENTRY).unwrap();
    ctx.db.game_creature_loot().insert(CreatureLoot {
        id: 0,
        creature_entry: ENTRY,
        item_entry: 50,
        chance_bp: 5_000,
        count: 1,
        group_id: 0,
        quest_only: false,
    });
    for ordinal in 1..=creatures {
        let guid = (0xF130_u64 << 48) | (u64::from(ENTRY) << 24) | u64::from(ordinal);
        let offset = if ordinal % 2 == 0 { 4.0 } else { 35.0 };
        let spawn = CreatureSpawn {
            guid,
            entry: ENTRY,
            map_id: character.map_id,
            x: character.x + offset,
            y: character.y + (ordinal % 8) as f32,
            z: character.z,
            orientation: std::f32::consts::PI,
            respawn_at: crate::creatures::timer_never(ctx),
            despawn_at: crate::creatures::timer_never(ctx),
            movement_type: crate::creatures::MOVEMENT_RANDOM,
            respawn_secs: 15,
            life_seq: 0,
            instance_id: 0,
            source_guid: 0,
        };
        let entity = crate::creatures::build_creature_entity(&spawn, &template, 0, 0);
        ctx.db.game_creature_spawn().insert(spawn);
        crate::creatures::insert_creature_entity(ctx, entity);
        if scenario == "periodic_deaths" {
            crate::loot::tag::record_first_threat(ctx, guid, 1);
            crate::debug::debug_fill_aura_slots(ctx, guid, 1, true, 1)?;
            let mut aura = ctx.db.game_aura().by_target().filter(guid).next().unwrap();
            aura.eff_kind = crate::spell::A_PERIODIC_DAMAGE;
            aura.amount = 7;
            aura.period_ms = 1_000;
            aura.next_tick_micros = START_MICROS as i64 + 1_000_000;
            ctx.db.game_aura().id().update(aura);
        }
    }
    Ok(())
}

// Synthetic time controls rule inputs; the real transaction still owns table access.
fn fixture_clock(at_micros: u64) -> ReducerContext {
    let mut clock = ReducerContext::__dummy();
    clock.timestamp = Timestamp::from_micros_since_unix_epoch(at_micros as i64);
    clock
}

#[reducer]
pub fn debug_native_gameplay_step(ctx: &ReducerContext, sequence: u64) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    let mut state = ctx
        .db
        .game_native_gameplay_state()
        .id()
        .find(0)
        .ok_or("gameplay fixture is not initialized")?;
    if sequence != state.sequence + 1 || sequence > 32 {
        return Err("gameplay fixture requires consecutive steps in 1..=32".into());
    }
    state.sequence = sequence;
    state.at_micros = START_MICROS + sequence * 1_000_000;
    let clock = fixture_clock(state.at_micros);
    match state.scenario.as_str() {
        "periodic_deaths" => crate::spell::run_auras(&clock),
        "creature_cycle" => {
            let outcome = crate::creatures::cycle::run(
                &clock,
                TickContext {
                    now_micros: state.at_micros,
                    now_ms: (state.at_micros / 1_000) as u32,
                    tick_secs: 1.0,
                    sense: true,
                    sense_secs: 1.0,
                    scope: TickScope::CatchAll {
                        dedicated: Default::default(),
                    },
                },
            );
            state.awake = outcome.awake as u64;
            (state.phases, state.visited) = outcome
                .rows_visited
                .into_iter()
                .map(|(name, visited)| (name.to_owned(), visited))
                .unzip();
            crate::combat::run_melee(&clock);
            crate::spell::run_auras(&clock);
        }
        _ => return Err("unknown gameplay fixture".into()),
    }
    ctx.db.game_native_gameplay_state().id().update(state);
    Ok(())
}

#[reducer]
pub fn debug_native_gameplay_process_fault(ctx: &ReducerContext, spin: bool) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    if ctx.db.game_session().count() != 0 || ctx.db.game_native_gameplay_state().count() != 0 {
        return Err("process faults require an unused disposable gameplay fixture".into());
    }
    let mut character =
        crate::helpers::character_by_guid(ctx, 1).ok_or("fixture Character is missing")?;
    character.money = character
        .money
        .checked_add(1)
        .ok_or("fixture money overflow")?;
    ctx.db.game_character().guid().update(character);
    ctx.db
        .game_native_gameplay_state()
        .insert(NativeGameplayState {
            id: 0,
            scenario: "process_fault".into(),
            creatures: 0,
            sequence: 1,
            at_micros: START_MICROS,
            awake: 0,
            phases: Vec::new(),
            visited: Vec::new(),
        });
    if spin {
        spacetimedb::log::info!("LYRACORE_NATIVE_PROCESS_SPIN_READY");
        loop {
            std::hint::spin_loop();
        }
    }
    std::process::abort()
}
