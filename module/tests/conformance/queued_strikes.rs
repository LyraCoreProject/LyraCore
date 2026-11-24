use std::time::Instant;

use anyhow::{ensure, Context, Result};
use lyracore_test_support::{NativePacket, ReducerConnection, Standalone};
use serde_json::json;
use wire_client::compare::Difference;
use wire_client::decode::{decode, Field, Packet};
use wire_client::rolls::PUBLIC_SEEDS;
use wire_client::scenario::{FixtureVerb, Scenario};

use super::queued_strike_spec::{self as spec, Counts};
use super::strike_collector::{cast_request, Measurement, Preparation, Sample, CMSG_CAST_SPELL};
use super::strike_rolls::{SAMPLES_PER_LEVEL, TARGET_LEVELS};
use super::transport::{prepare_character, Delivery};

const SMSG_SPELL_GO: u16 = 0x132;
const SMSG_ATTACKERSTATEUPDATE: u16 = 0x14a;
const SMSG_SPELLNONMELEEDAMAGELOG: u16 = 0x250;
// Each 100 ms melee tick swings the 50 ms fixture axe once.
const TICKS_PER_SWING: u32 = 2;

struct Character {
    connection: ReducerConnection,
    guid: u64,
    target: u64,
    delivery: Delivery,
    counts: Counts,
}

fn queue_strike(character: &mut Character) -> std::result::Result<(), String> {
    character
        .connection
        .call(
            "native_world_receive",
            json!([
                CMSG_CAST_SPELL,
                cast_request(spec::SPELL_ID, character.target)
            ]),
        )
        .map(|_| ())
}

fn own_packets(character: &mut Character, rows: &[NativePacket]) -> Result<Vec<Packet>> {
    character
        .delivery
        .route_rows(rows)?
        .into_iter()
        .filter(|packet| {
            matches!(
                packet.opcode,
                SMSG_SPELL_GO | SMSG_ATTACKERSTATEUPDATE | SMSG_SPELLNONMELEEDAMAGELOG
            )
        })
        .map(|packet| decode(packet.opcode, &packet.body))
        .collect()
}

fn queued_spells(shard: &Standalone, shard_name: &str, characters: &[Character]) -> Vec<String> {
    let rows = shard.query_database_rows(
        shard_name,
        "SELECT guid,next_swing_spell FROM game_world_entity",
    );
    characters
        .iter()
        .map(|character| {
            rows.iter()
                .find(|row| row["guid"] == character.guid.to_string())
                .map_or_else(String::new, |row| row["next_swing_spell"].clone())
        })
        .collect()
}

// Unretained readiness: each Character's queued strike must replace its next white swing.
fn ready_strikes(
    shard: &Standalone,
    shard_name: &str,
    receiver: &mut ReducerConnection,
    characters: &mut [Character],
) -> Result<()> {
    for character in characters.iter_mut() {
        character
            .connection
            .call(
                "native_world_receive",
                json!([0x141u32, character.target.to_le_bytes().to_vec()]),
            )
            .map_err(anyhow::Error::msg)?;
    }
    receiver
        .call("debug_advance_roll_batch", json!([TICKS_PER_SWING]))
        .map_err(anyhow::Error::msg)?;
    let rows = receiver.drain_packets();
    for character in characters.iter_mut() {
        own_packets(character, &rows)?;
    }
    for _ in 0..2 {
        for character in characters.iter_mut() {
            queue_strike(character).map_err(anyhow::Error::msg)?;
        }
        ensure!(
            queued_spells(shard, shard_name, characters)
                .iter()
                .all(|spell| *spell == spec::SPELL_ID.to_string()),
            "Heroic Strike was not queued"
        );
        receiver
            .call("debug_advance_roll_batch", json!([TICKS_PER_SWING]))
            .map_err(anyhow::Error::msg)?;
        let rows = receiver.drain_packets();
        for character in characters.iter_mut() {
            let guid = Field::Guid(character.guid);
            let packets = own_packets(character, &rows)?;
            ensure!(
                packets.iter().any(|packet| packet.opcode == SMSG_SPELL_GO
                    && packet.fields.get("caster") == Some(&guid)
                    && packet.fields.get("spell") == Some(&Field::Unsigned(spec::SPELL_ID.into())))
                    && !packets
                        .iter()
                        .any(|packet| packet.opcode == SMSG_ATTACKERSTATEUPDATE
                            && packet.fields.get("attacker") == Some(&guid)
                            && packet.fields.get("spell") == Some(&Field::Unsigned(0))),
                "the swing did not fire the queued strike: {packets:?}"
            );
        }
        ensure!(
            queued_spells(shard, shard_name, characters)
                .iter()
                .all(|spell| spell == "0"),
            "the fired strike stayed queued"
        );
    }
    Ok(())
}

fn prepare(
    shard: &Standalone,
    shard_name: &str,
    scenario: &Scenario,
    seed: u64,
    level_index: usize,
    part: usize,
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
            ("name", &"Queued strike target"),
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
        anyhow::bail!("queued strike Character fixture")
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
            class: "warrior".into(),
            level: 60,
            at,
        };
        let guid = prepare_character(shard, shard_name, &format!("STRIKE{index}"), &fixture)?;
        shard.assert_call_database(
            shard_name,
            "debug_spawn_at_feet",
            &[&guid.to_string(), "38", "2"],
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
            .context("queued strike target")?;
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
            "debug_prepare_queued_strike_character",
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
            counts: Counts::default(),
        });
    }
    receiver
        .call("debug_advance_roll_batch", json!([1]))
        .map_err(anyhow::Error::msg)?;
    let opening = receiver.drain_packets();
    for character in &mut characters {
        character.delivery.route_rows(&opening)?;
    }
    ready_strikes(shard, shard_name, &mut receiver, &mut characters)?;
    verify_context(shard, shard_name, &characters, level_index)?;
    Ok((receiver, characters))
}

fn verify_context(
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
        items.len() == characters.len()
            && characters.iter().all(|character| items.iter().any(|item| {
                item["owner_guid"] == character.guid.to_string()
                    && item["entry"] == "37"
                    && item["slot"] == "15"
            })),
        "each queued strike Character must hold only its main-hand axe: {items:?}"
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
        &[44, 95, 162],
        None,
        &super::rolls::MELEE_POSE,
    )
}

fn collect(
    shard: &Standalone,
    shard_name: &str,
    scenario: &Scenario,
    seed: u64,
    level_index: usize,
    part: usize,
    ready: &std::sync::Barrier,
) -> Result<Sample<7>> {
    let mut preparation = Preparation {
        ready,
        arrived: false,
    };
    let setup = Instant::now();
    let (mut receiver, mut characters) =
        prepare(shard, shard_name, scenario, seed, level_index, part)?;
    println!(
        "queued strike {shard_name}: setup {:.3}s",
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
    for trial in 1..=SAMPLES_PER_LEVEL / 8 {
        for character in &mut characters {
            if let Err(refusal) = queue_strike(character) {
                return Ok(Sample::Blocked {
                    counts: total(&characters),
                    reason: format!("trial {trial}: Heroic Strike was refused: {refusal}"),
                });
            }
        }
        receiver
            .call("debug_advance_roll_batch", json!([TICKS_PER_SWING]))
            .map_err(anyhow::Error::msg)?;
        let rows = receiver.drain_packets();
        for character in &mut characters {
            let packets = own_packets(character, &rows)?;
            if let Err(gap) = character
                .counts
                .observe(&packets, character.guid, character.target)
            {
                let reason = format!(
                    "trial {trial} for Character {}: {gap:#}; packets {packets:?}",
                    character.guid
                );
                println!("queued strike {shard_name}: blocked: {reason}");
                return Ok(Sample::Blocked {
                    counts: total(&characters),
                    reason,
                });
            }
        }
        if trial % 1250 == 0 {
            println!(
                "queued strike {shard_name}: {trial} strikes each; {:.3}s warm",
                warm.elapsed().as_secs_f64()
            );
        }
    }
    verify_context(shard, shard_name, &characters, level_index)?;
    let counts = total(&characters);
    println!(
        "queued strike {shard_name}: seed {seed}; warm {:.3}s; counts {}",
        warm.elapsed().as_secs_f64(),
        serde_json::to_string(&counts)?
    );
    Ok(Sample::Complete { counts })
}

const MEASUREMENT: Measurement<7> = Measurement {
    context: &spec::CONTEXT,
    shards: "strike",
    collect,
};

pub(super) fn public_seeds(scenario: &Scenario) -> Result<Vec<Difference>> {
    MEASUREMENT.public_seeds(scenario)
}

pub(super) fn merge_seed(scenario: &Scenario, seed: u64) -> Result<Vec<Difference>> {
    MEASUREMENT.merge_seed(scenario, seed)
}

struct ForcedStrike {
    attacker: u64,
    target: u64,
    packets: Vec<Packet>,
}

impl ForcedStrike {
    fn counts(&self) -> Result<Counts> {
        let mut counts = Counts::default();
        counts.observe(&self.packets, self.attacker, self.target)?;
        Ok(counts)
    }
}

/// Fires one queued strike per Character from scripted (table, weapon, partial block) draws.
fn forced_strike(test_name: &str, draws: [u32; 3]) -> Result<Vec<ForcedStrike>> {
    let golden = wire_client::catalogue::get("white_melee_outcomes")?;
    let mut shard = Standalone::start_persistent(test_name);
    let name = super::rolls::fresh_shards(&mut shard, "forced", 1).remove(0);
    let (mut receiver, mut characters) =
        prepare(&shard, &name, golden.scenario(), PUBLIC_SEEDS[0], 0, 0)?;
    for character in &mut characters {
        receiver
            .call("debug_script_swing_draws", json!([character.guid, draws]))
            .map_err(anyhow::Error::msg)?;
        queue_strike(character).map_err(anyhow::Error::msg)?;
    }
    receiver
        .call("debug_advance_roll_batch", json!([TICKS_PER_SWING]))
        .map_err(anyhow::Error::msg)?;
    let rows = receiver.drain_packets();
    characters
        .into_iter()
        .map(|mut character| -> Result<ForcedStrike> {
            let guid = Field::Guid(character.guid);
            let packets = own_packets(&mut character, &rows)?
                .into_iter()
                .filter(|packet| {
                    packet.fields.get("caster") == Some(&guid)
                        || packet.fields.get("attacker") == Some(&guid)
                })
                .collect();
            Ok(ForcedStrike {
                attacker: character.guid,
                target: character.target,
                packets,
            })
        })
        .collect()
}

fn unsigned(packet: &Packet, name: &str) -> u64 {
    match packet.fields.get(name) {
        Some(Field::Unsigned(value)) => *value,
        other => panic!("opcode {:#06x} {name}: {other:?}", packet.opcode),
    }
}

fn opcodes(packets: &[Packet]) -> Vec<u16> {
    packets.iter().map(|packet| packet.opcode).collect()
}

// Level 60 bands from ADR 0007: dodge 500..1000, glancing 1500..2500; partial block below 500.
#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_dodged_queued_strike_sends_its_go_miss_then_a_dodge_swing() -> Result<()> {
    for strike in forced_strike("queued-strike-dodge", [700, 0, 0])? {
        let packets = &strike.packets;
        assert_eq!(
            opcodes(packets),
            [SMSG_SPELL_GO, SMSG_ATTACKERSTATEUPDATE],
            "{packets:?}"
        );
        let Some(Field::List(misses)) = packets[0].fields.get("misses") else {
            anyhow::bail!("the GO lists no misses: {:?}", packets[0]);
        };
        let dodge = Field::Object(std::collections::BTreeMap::from([
            ("guid".into(), Field::Guid(strike.target)),
            ("reason".into(), Field::Unsigned(3)),
        ]));
        assert_eq!(misses, &vec![dodge]);
        let swing = &packets[1];
        assert_eq!(unsigned(swing, "victim_state"), 2);
        // vmangos HITINFO_NOACTION | HITINFO_SUPPRESS_MISS_TEXT; ROLLED_* flags are not sent.
        assert_eq!(unsigned(swing, "hit_info"), 0x11000);
        assert_eq!(unsigned(swing, "spell"), u64::from(spec::SPELL_ID));
        assert_eq!(strike.counts()?, Counts::new([0, 1, 0, 0, 0, 0, 0]));
    }
    Ok(())
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_glancing_queued_strike_sends_go_then_a_glancing_swing_then_its_damage_log() -> Result<()> {
    for strike in forced_strike("queued-strike-glancing", [2000, 0, 9_999])? {
        let packets = &strike.packets;
        assert_eq!(
            opcodes(packets),
            [
                SMSG_SPELL_GO,
                SMSG_ATTACKERSTATEUPDATE,
                SMSG_SPELLNONMELEEDAMAGELOG
            ],
            "{packets:?}"
        );
        let (swing, log) = (&packets[1], &packets[2]);
        assert_eq!(unsigned(swing, "victim_state"), 1);
        // Heroic Strike's ON_NEXT_SWING_NO_DAMAGE leaves the swing's damage to the log.
        assert_eq!(unsigned(swing, "hit_info"), 0x15002);
        assert_eq!(unsigned(swing, "damage"), 0);
        assert!(unsigned(log, "damage") > 0);
        assert_eq!(unsigned(log, "blocked"), 0);
        assert_eq!(strike.counts()?, Counts::new([0, 0, 0, 1, 0, 0, 0]));
    }
    Ok(())
}

// Heroic Strike rank 1 with the fixture axe never exceeds a level 60 creature's block value.
#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_fully_blocked_queued_strike_still_sends_its_swing_and_a_damage_log() -> Result<()> {
    for strike in forced_strike("queued-strike-blocked", [5000, 0, 0])? {
        let packets = &strike.packets;
        assert_eq!(
            opcodes(packets),
            [
                SMSG_SPELL_GO,
                SMSG_ATTACKERSTATEUPDATE,
                SMSG_SPELLNONMELEEDAMAGELOG
            ],
            "{packets:?}"
        );
        let (swing, log) = (&packets[1], &packets[2]);
        assert_eq!(unsigned(swing, "victim_state"), 1);
        assert_eq!(unsigned(swing, "hit_info"), 0x11c02);
        assert_eq!(unsigned(log, "damage"), 0);
        assert!(unsigned(log, "blocked") > 0);
        assert_eq!(strike.counts()?, Counts::new([0, 0, 0, 0, 0, 1, 1]));
    }
    Ok(())
}
