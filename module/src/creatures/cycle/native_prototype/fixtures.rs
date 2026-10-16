use super::*;
use crate::{CreatureSpawn, CreatureTemplate};
use lyracore_shared::constants::{type_mask, unit_flags};
use spacetimedb::Timestamp;

pub const SCENARIOS: &[&str] = &[
    "idle_drift",
    "grid_crossing",
    "combat_advance",
    "stopped_target",
    "moving_target_turn_stop",
    "immobilized",
    "missing_mover",
    "clock_wrap",
];

pub fn scenario(name: &str) -> Result<Scenario, String> {
    let name = SCENARIOS
        .iter()
        .copied()
        .find(|n| *n == name)
        .ok_or_else(|| format!("unknown prototype scenario: {name}"))?;
    let start_micros = if name == "clock_wrap" {
        (u64::from(u32::MAX) - 500) * 1_000
    } else {
        60_000_000
    };
    let x = if name == "grid_crossing" { 16.0 } else { 10.0 };
    let mut creature = entity(CREATURE_GUID, x, 0.0);
    if name == "combat_advance" {
        creature.unit_flags |= unit_flags::IN_COMBAT;
    }
    let pursuing = matches!(name, "stopped_target" | "moving_target_turn_stop");
    let splines = if pursuing {
        Vec::new()
    } else {
        vec![CreatureSpline {
            guid: creature.guid,
            start_micros,
            dur_ms: 3_000,
            sx: creature.x,
            sy: creature.y,
            sz: creature.z,
            dx: creature.x + 12.0,
            dy: 0.0,
            dz: 0.0,
            map_id: creature.map_id,
            instance_id: creature.instance_id,
            grid_x: creature.grid_x,
            grid_y: creature.grid_y,
            spline_id: (start_micros / 1_000) as u32,
            run: false,
            cell: creature.cell,
            facing: false,
            facing_angle: 0.0,
            path: None,
        }]
    };
    let mut entities = vec![creature];
    let attacks = if pursuing {
        let mut target = entity(TARGET_GUID, 30.0, 0.0);
        target.type_mask = type_mask::PLAYER;
        if name == "moving_target_turn_stop" {
            target.movement_flags = crate::combat::MOVE_FLAG_FORWARD;
        }
        entities.push(target);
        vec![MeleeAttack {
            attacker_guid: CREATURE_GUID,
            target_guid: TARGET_GUID,
            last_swing_ms: 0,
            ranged_spell_id: 0,
            last_offhand_swing_ms: 0,
            rout_ends_ms: 0,
            pursuit_ends_ms: 0,
            leash_x: 10.0,
            leash_y: 0.0,
            swing_error: 0,
        }]
    } else {
        Vec::new()
    };
    let actions = match name {
        "moving_target_turn_stop" => vec![
            (0, Action::Tick),
            (500, Action::Tick),
            (500, target(22.0, 20.0, 1)),
            (1_000, Action::Tick),
            (1_000, target(18.0, 5.0, 0)),
            (1_500, Action::Tick),
            (3_000, Action::Tick),
            (3_500, Action::Tick),
        ],
        "immobilized" => vec![
            (500, Action::Tick),
            (750, Action::Immobilize { enabled: true }),
            (1_000, Action::Tick),
            (1_500, Action::Tick),
            (1_500, Action::Immobilize { enabled: false }),
            (2_000, Action::Tick),
        ],
        "missing_mover" => vec![
            (500, Action::Tick),
            (750, Action::RemoveCreature),
            (1_000, Action::Tick),
        ],
        _ => [0, 500, 1_000, 1_500, 2_000, 3_000, 3_500]
            .into_iter()
            .map(|ms| (ms, Action::Tick))
            .collect(),
    };
    let requests = actions
        .into_iter()
        .enumerate()
        .map(|(i, (ms, action))| Request {
            sequence: i as u64 + 1,
            at_micros: start_micros + ms * 1_000,
            action,
        })
        .collect();
    Ok(Scenario {
        name,
        requests,
        entities,
        splines,
        attacks,
        start_micros,
    })
}

fn target(x: f32, y: f32, movement_flags: u32) -> Action {
    Action::MoveTarget {
        x,
        y,
        z: 0.0,
        orientation: 0.0,
        movement_flags,
    }
}

fn entity(guid: u64, x: f32, y: f32) -> WorldEntity {
    let spawn = CreatureSpawn {
        guid,
        entry: 900_000_001,
        map_id: 900_000_001,
        x,
        y,
        z: 0.0,
        orientation: 0.0,
        respawn_at: Timestamp::UNIX_EPOCH,
        despawn_at: Timestamp::UNIX_EPOCH,
        movement_type: 0,
        respawn_secs: 0,
        life_seq: 1,
        instance_id: INSTANCE_ID,
        source_guid: 0,
    };
    let template = CreatureTemplate {
        entry: spawn.entry,
        name: "Native simulation prototype".into(),
        subname: String::new(),
        display_id: 1,
        level: 1,
        health: 100,
        faction_template: 0,
        npc_flags: 0,
        unit_flags: 0,
        creature_type: 0,
        creature_family: 0,
        type_flags: 0,
        rank: 0,
        scale: 1.0,
        base_attack_time_ms: 2_000,
        money_min: 0,
        money_max: 0,
        max_level: 1,
        max_level_health: 100,
        aggro_range: 0,
        damage_min: 0,
        damage_max: 0,
        armor: 0,
        pickpocket_loot_id: 0,
        skin_loot_id: 0,
        trainer_type: 0,
        trainer_class: 0,
    };
    crate::creatures::build_creature_entity(&spawn, &template, 0, INSTANCE_ID)
}
