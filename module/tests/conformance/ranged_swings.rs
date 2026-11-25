use std::sync::Barrier;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use lyracore_test_support::{NativePacket, ReducerConnection, Standalone};
use serde_json::json;
use wire_client::compare::Difference;
use wire_client::decode::{decode, Packet};
use wire_client::rolls::PUBLIC_SEEDS;
use wire_client::scenario::{FixtureVerb, Scenario};

use super::ranged_swing_spec::{self as spec, Counts, Flight, RangedSwing};
use super::rolls::Pose;
use super::strike_collector::{cast_request, Measurement, Preparation, Sample, CMSG_CAST_SPELL};
use super::strike_rolls::{SAMPLES_PER_LEVEL, TARGET_LEVELS};
use super::transport::{prepare_character, Delivery};

const READY_SHOTS: u32 = 2;
const READY_BATCHES: u32 = 12;

/// One ranged swing's Tier 1 setup; each batch spans one weapon delay, longer than the flight.
struct Shooter {
    swing: &'static RangedSwing,
    class: &'static str,
    class_id: u8,
    account: &'static str,
    prepare: &'static str,
    items: &'static [(u32, u8)],
    skill: u32,
    distance: f32,
    ticks_per_shot: u32,
}

// A 300 ms gun delay outlasts Auto Shot's 250 ms flight over 10 yd at 40 yd/s.
const AUTO_SHOT: Shooter = Shooter {
    swing: &spec::AUTO_SHOT,
    class: "warrior",
    class_id: 1,
    account: "AUTOSHOT",
    prepare: "debug_prepare_auto_shot_character",
    items: &[(2508, 17), (2516, 23)],
    skill: 46,
    distance: 10.0,
    ticks_per_shot: 6,
};

// A 400 ms wand delay outlasts Shoot's 300 ms flight over 6 yd at 20 yd/s.
const WAND_SHOT: Shooter = Shooter {
    swing: &spec::WAND_SHOT,
    class: "priest",
    class_id: 5,
    account: "WANDSHOT",
    prepare: "debug_prepare_wand_shot_character",
    items: &[(11287, 17)],
    skill: 228,
    distance: 6.0,
    ticks_per_shot: 8,
};

struct Character {
    connection: ReducerConnection,
    guid: u64,
    target: u64,
    delivery: Delivery,
    flight: Flight,
    counts: Counts,
}

impl Character {
    fn observe(&mut self, shooter: &Shooter, packets: &[Packet], quota: u32) -> Result<()> {
        shooter.swing.observe(
            &mut self.flight,
            &mut self.counts,
            packets,
            (self.guid, self.target),
            quota,
        )
    }
}

fn own_packets(character: &mut Character, rows: &[NativePacket]) -> Result<Vec<Packet>> {
    character
        .delivery
        .route_rows(rows)?
        .into_iter()
        .filter(|packet| matches!(packet.opcode, 0x132 | 0x14a | 0x24b | 0x250))
        .map(|packet| match packet.opcode {
            spec::SPELL_LOG_MISS => super::weapon_strike_spec::spell_log_miss(&packet.body),
            opcode => decode(opcode, &packet.body),
        })
        .collect()
}

fn advance_and_drain(receiver: &mut ReducerConnection, ticks: u32) -> Result<Vec<NativePacket>> {
    receiver
        .call("debug_advance_roll_batch", json!([ticks]))
        .map_err(anyhow::Error::msg)?;
    Ok(receiver.drain_packets())
}

// Unretained readiness: two shots per Character; one still in flight is counted in the sample.
fn ready_shots(
    shooter: &Shooter,
    receiver: &mut ReducerConnection,
    characters: &mut [Character],
) -> Result<()> {
    for character in characters.iter_mut() {
        character
            .connection
            .call(
                "native_world_receive",
                json!([
                    CMSG_CAST_SPELL,
                    cast_request(shooter.swing.spell, character.target)
                ]),
            )
            .map_err(anyhow::Error::msg)?;
    }
    for _ in 0..READY_BATCHES {
        let rows = advance_and_drain(receiver, shooter.ticks_per_shot)?;
        for character in characters.iter_mut() {
            let packets = own_packets(character, &rows)?;
            character.observe(shooter, &packets, u32::MAX)?;
        }
        if characters
            .iter()
            .all(|character| character.counts.trials() >= READY_SHOTS)
        {
            for character in characters.iter_mut() {
                character.counts = Counts::default();
            }
            return Ok(());
        }
    }
    let shots: Vec<_> = characters
        .iter()
        .map(|character| character.counts.trials())
        .collect();
    anyhow::bail!(
        "{} readiness counted {shots:?} shots in {READY_BATCHES} batches",
        shooter.swing.context.label()
    )
}

fn prepare(
    shooter: &Shooter,
    shard: &Standalone,
    shard_name: &str,
    scenario: &Scenario,
    (seed, level_index, part): (u64, usize, usize),
) -> Result<(ReducerConnection, Vec<Character>)> {
    shard.assert_call_database(shard_name, "claim_operator", &[]);
    shard.assert_call_database(shard_name, "install_guid_range", &["0"]);
    shard.assert_call_database(
        shard_name,
        "debug_start_virtual_clock",
        &["1800000000000000"],
    );
    shard.insert_row_database(
        shard_name,
        "game_creature_template",
        &[
            ("entry", &38),
            ("name", &"Ranged swing target"),
            ("display_id", &1),
            ("level", &60),
            ("health", &1000000000),
            ("faction_template", &7),
            ("creature_type", &7),
            ("scale", &1),
            ("base_attack_time_ms", &2000),
            ("max_level", &60),
            ("max_level_health", &1000000000),
        ],
    );
    let FixtureVerb::Character { at, .. } = &scenario.fixtures[0] else {
        anyhow::bail!("ranged swing Character fixture")
    };
    let mut participants = Vec::new();
    for (index, name) in super::rolls::NAMES
        .iter()
        .enumerate()
        .filter(|(index, _)| index / 8 == level_index && index % 8 / 4 == part)
    {
        let mut at = at.clone();
        at.y -= index as f32 * 200.0;
        let fixture = FixtureVerb::Character {
            name: (*name).into(),
            race: "human".into(),
            class: shooter.class.into(),
            level: 60,
            at,
        };
        let account = format!("{}{index}", shooter.account);
        let guid = prepare_character(shard, shard_name, &account, &fixture)?;
        shard.assert_call_database(
            shard_name,
            "debug_spawn_at_feet",
            &[&guid.to_string(), "38", &shooter.distance.to_string()],
        );
        let target = shard
            .query_database_rows(
                shard_name,
                "SELECT guid FROM game_world_entity WHERE entry = 38",
            )
            .iter()
            .map(|row| row["guid"].parse::<u64>())
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .max()
            .context("ranged swing target")?;
        participants.push((index, guid, target));
    }
    let mut receiver =
        shard.connect_shard_reducers(shard_name, &["SELECT * FROM game_native_packet"]);
    let mut characters = Vec::new();
    for (index, guid, target) in participants {
        let mut connection = shard.connect_shard_reducers(shard_name, &[]);
        connection
            .call("native_world_open", json!([42]))
            .map_err(anyhow::Error::msg)?;
        connection
            .call("debug_native_enter_world", json!([guid]))
            .map_err(anyhow::Error::msg)?;
        shard.assert_call_database(
            shard_name,
            shooter.prepare,
            &[
                &guid.to_string(),
                &target.to_string(),
                &TARGET_LEVELS[level_index].to_string(),
                &seed.to_string(),
                &(index as u64).to_string(),
            ],
        );
        characters.push(Character {
            delivery: Delivery::new(connection.connection_id()),
            connection,
            guid,
            target,
            flight: Flight::default(),
            counts: Counts::default(),
        });
    }
    let opening = advance_and_drain(&mut receiver, 1)?;
    for character in &mut characters {
        character.delivery.route_rows(&opening)?;
    }
    ready_shots(shooter, &mut receiver, &mut characters)?;
    verify_context(shooter, shard, shard_name, &characters, level_index)?;
    Ok((receiver, characters))
}

fn verify_context(
    shooter: &Shooter,
    shard: &Standalone,
    shard_name: &str,
    characters: &[Character],
    level_index: usize,
) -> Result<()> {
    let items = shard.query_database_rows(
        shard_name,
        "SELECT owner_guid,entry,slot FROM game_item_instance",
    );
    ensure!(
        items.len() == characters.len() * shooter.items.len()
            && characters.iter().all(|character| {
                shooter.items.iter().all(|(entry, slot)| {
                    items.iter().any(|item| {
                        item["owner_guid"] == character.guid.to_string()
                            && item["entry"] == entry.to_string()
                            && item["slot"] == slot.to_string()
                    })
                })
            }),
        "each {} Character must hold only its ranged weapon and ammo: {items:?}",
        shooter.swing.context.label()
    );
    let participants: Vec<_> = characters
        .iter()
        .map(|character| (character.guid, character.target))
        .collect();
    super::rolls::verify_participants(
        shard,
        shard_name,
        &participants,
        TARGET_LEVELS[level_index],
        &[95, 162, shooter.skill],
        None,
        &Pose {
            class: shooter.class_id,
            distance: shooter.distance,
        },
    )
}

fn collect(
    shooter: &Shooter,
    shard: &Standalone,
    shard_name: &str,
    scenario: &Scenario,
    (seed, level_index, part): (u64, usize, usize),
    ready: &Barrier,
) -> Result<Sample<4>> {
    let label = shooter.swing.context.label();
    let mut preparation = Preparation {
        ready,
        arrived: false,
    };
    let setup = Instant::now();
    let (mut receiver, mut characters) = prepare(
        shooter,
        shard,
        shard_name,
        scenario,
        (seed, level_index, part),
    )?;
    println!(
        "{label} {shard_name}: setup {:.3}s",
        setup.elapsed().as_secs_f64()
    );
    preparation.arrived = true;
    ready.wait();
    let warm = Instant::now();
    let total = |characters: &[Character]| {
        let mut counts = Counts::default();
        for character in characters {
            counts.add(&character.counts);
        }
        counts
    };
    let quota = SAMPLES_PER_LEVEL / 8;
    for batch in 1..=quota + READY_BATCHES {
        if characters
            .iter()
            .all(|character| character.counts.trials() == quota)
        {
            break;
        }
        for character in &mut characters {
            character
                .connection
                .keep_alive()
                .map_err(anyhow::Error::msg)?;
        }
        let rows = advance_and_drain(&mut receiver, shooter.ticks_per_shot)?;
        for character in &mut characters {
            let packets = own_packets(character, &rows)?;
            if let Err(gap) = character.observe(shooter, &packets, quota) {
                let reason = format!(
                    "batch {batch} for Character {}: {gap:#}; packets {packets:?}",
                    character.guid
                );
                println!("{label} {shard_name}: blocked: {reason}");
                return Ok(Sample::Blocked {
                    counts: total(&characters),
                    reason,
                });
            }
        }
        if batch % 1250 == 0 {
            println!(
                "{label} {shard_name}: {batch} batches; {:.3}s warm",
                warm.elapsed().as_secs_f64()
            );
        }
    }
    let shots: Vec<_> = characters
        .iter()
        .map(|character| character.counts.trials())
        .collect();
    if shots.iter().any(|shots| *shots < quota) {
        return Ok(Sample::Blocked {
            counts: total(&characters),
            reason: format!("the auto-repeat stopped after {shots:?} counted shots"),
        });
    }
    verify_context(shooter, shard, shard_name, &characters, level_index)?;
    let counts = total(&characters);
    println!(
        "{label} {shard_name}: seed {seed}; warm {:.3}s; counts {}",
        warm.elapsed().as_secs_f64(),
        serde_json::to_string(&counts)?
    );
    Ok(Sample::Complete { counts })
}

fn collect_auto_shot(
    shard: &Standalone,
    shard_name: &str,
    scenario: &Scenario,
    seed: u64,
    level_index: usize,
    part: usize,
    ready: &Barrier,
) -> Result<Sample<4>> {
    let run = (seed, level_index, part);
    collect(&AUTO_SHOT, shard, shard_name, scenario, run, ready)
}

fn collect_wand_shot(
    shard: &Standalone,
    shard_name: &str,
    scenario: &Scenario,
    seed: u64,
    level_index: usize,
    part: usize,
    ready: &Barrier,
) -> Result<Sample<4>> {
    let run = (seed, level_index, part);
    collect(&WAND_SHOT, shard, shard_name, scenario, run, ready)
}

const MEASUREMENTS: [Measurement<4>; 2] = [
    Measurement {
        context: &spec::AUTO_SHOT.context,
        shards: "autoshot",
        collect: collect_auto_shot,
    },
    Measurement {
        context: &spec::WAND_SHOT.context,
        shards: "wandshot",
        collect: collect_wand_shot,
    },
];

pub(super) fn public_seeds(scenario: &Scenario) -> Result<Vec<Difference>> {
    let mut differences = Vec::new();
    for measurement in &MEASUREMENTS {
        differences.extend(measurement.public_seeds(scenario)?);
    }
    Ok(differences)
}

pub(super) fn merge_seed(scenario: &Scenario, seed: u64) -> Result<Vec<Difference>> {
    let mut differences = Vec::new();
    for measurement in &MEASUREMENTS {
        differences.extend(measurement.merge_seed(scenario, seed)?);
    }
    Ok(differences)
}

// SpacetimeDB 2.14 drops a WebSocket 40 s after the last frame from a client that reads nothing.
const PAST_IDLE_TEARDOWN: Duration = Duration::from_secs(45);

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_shooter_whose_connection_only_waits_keeps_its_auto_repeat_past_the_idle_teardown() -> Result<()>
{
    let golden = wire_client::catalogue::get("white_melee_outcomes")?;
    let mut shard = Standalone::start("ranged-swing-idle-connection");
    shard.publish_module();
    let shard_name = shard.shard_name().to_string();
    let (mut receiver, mut characters) = prepare(
        &AUTO_SHOT,
        &shard,
        &shard_name,
        golden.scenario(),
        (PUBLIC_SEEDS[0], 0, 0),
    )?;
    let waiting = Instant::now();
    while waiting.elapsed() < PAST_IDLE_TEARDOWN {
        receiver.keep_alive().map_err(anyhow::Error::msg)?;
        for character in &mut characters {
            character
                .connection
                .keep_alive()
                .map_err(anyhow::Error::msg)?;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    for _ in 0..2 {
        let rows = advance_and_drain(&mut receiver, AUTO_SHOT.ticks_per_shot)?;
        for character in &mut characters {
            let packets = own_packets(character, &rows)?;
            character.observe(&AUTO_SHOT, &packets, u32::MAX)?;
        }
    }
    let shots: Vec<_> = characters
        .iter()
        .map(|character| character.counts.trials())
        .collect();
    ensure!(
        shots.iter().all(|shots| *shots > 0),
        "a shooter stopped while its connection waited: {shots:?}"
    );
    Ok(())
}
