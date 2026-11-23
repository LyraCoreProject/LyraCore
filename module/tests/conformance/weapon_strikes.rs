use std::time::Instant;

use anyhow::{ensure, Context, Result};
use lyracore_test_support::{core_root, ReducerConnection, Standalone};
use serde_json::json;
use wire_client::compare::Difference;
use wire_client::decode::{decode, Field, Packet};
use wire_client::rolls::PUBLIC_SEEDS;
use wire_client::scenario::{FixtureVerb, Scenario};

use super::properties::Packet as RoutedPacket;
use super::queued_strikes::{cast_request, Preparation, CMSG_CAST_SPELL};
use super::transport::{prepare_character, Delivery};
use super::weapon_strike_spec::{self as spec, Counts};

struct Character {
    connection: ReducerConnection,
    guid: u64,
    target: u64,
    delivery: Delivery,
    counts: Counts,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum Sample {
    Complete { counts: Counts },
    Blocked { counts: Counts, reason: String },
}

impl Sample {
    fn counts(&self) -> &Counts {
        match self {
            Self::Complete { counts } | Self::Blocked { counts, .. } => counts,
        }
    }
}

fn cast(character: &mut Character) -> std::result::Result<(), String> {
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

fn own_packets(
    character: &mut Character,
    rows: &[Box<serde_json::value::RawValue>],
) -> Result<Vec<Packet>> {
    decoded(&character.delivery.route_rows(rows.to_vec())?)
}

fn decoded(routed: &[RoutedPacket]) -> Result<Vec<Packet>> {
    routed
        .iter()
        .filter(|packet| matches!(packet.opcode, 0x132 | 0x14a | 0x24b | 0x250))
        .map(|packet| match packet.opcode {
            spec::SPELL_LOG_MISS => spec::spell_log_miss(&packet.body),
            opcode => decode(opcode, &packet.body),
        })
        .collect()
}

fn advance_and_drain(
    receiver: &mut ReducerConnection,
) -> Result<Vec<Box<serde_json::value::RawValue>>> {
    receiver
        .call("debug_advance_roll_batch", json!([1]))
        .map_err(anyhow::Error::msg)?;
    Ok(receiver.drain_packets())
}

// Unretained readiness.
fn ready_casts(receiver: &mut ReducerConnection, characters: &mut [Character]) -> Result<()> {
    for character in characters.iter_mut() {
        cast(character).map_err(anyhow::Error::msg)?;
    }
    let rows = advance_and_drain(receiver)?;
    for character in characters.iter_mut() {
        let packets = own_packets(character, &rows)?;
        ensure!(
            packets.iter().any(|packet| packet.opcode == spec::SPELL_GO
                && packet.fields.get("caster") == Some(&Field::Guid(character.guid))
                && packet.fields.get("spell") == Some(&Field::Unsigned(spec::SPELL_ID.into()))),
            "Mortal Strike reached no SMSG_SPELL_GO: {packets:?}"
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
            ("name", &"Weapon strike target"),
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
        anyhow::bail!("weapon strike Character fixture")
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
        let guid = prepare_character(shard, shard_name, &format!("WEAPON{index}"), &fixture)?;
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
            .context("weapon strike target")?;
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
            "debug_prepare_weapon_strike_character",
            &[
                &guid.to_string(),
                &target.to_string(),
                &spec::TARGET_LEVELS[level_index].to_string(),
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
    let opening = advance_and_drain(&mut receiver)?;
    for character in &mut characters {
        character.delivery.route_rows(opening.clone())?;
    }
    ready_casts(&mut receiver, &mut characters)?;
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
        "each weapon strike Character must hold only its main-hand axe: {items:?}"
    );
    let participants: Vec<_> = characters
        .iter()
        .map(|character| (character.guid, character.target))
        .collect();
    super::rolls::verify_participants(
        shard,
        shard_name,
        &participants,
        spec::TARGET_LEVELS[level_index],
        &[44, 95, 162],
        Some(spec::SPELL_ID),
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
) -> Result<Sample> {
    let mut preparation = Preparation {
        ready,
        arrived: false,
    };
    let setup = Instant::now();
    let (mut receiver, mut characters) =
        prepare(shard, shard_name, scenario, seed, level_index, part)?;
    println!(
        "weapon strike {shard_name}: setup {:.3}s",
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
    for trial in 1..=spec::SAMPLES_PER_LEVEL / 8 {
        for character in &mut characters {
            if let Err(refusal) = cast(character) {
                return Ok(Sample::Blocked {
                    counts: total(&characters),
                    reason: format!("trial {trial}: Mortal Strike was refused: {refusal}"),
                });
            }
        }
        let rows = advance_and_drain(&mut receiver)?;
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
                println!("weapon strike {shard_name}: blocked: {reason}");
                return Ok(Sample::Blocked {
                    counts: total(&characters),
                    reason,
                });
            }
        }
        if trial % 1250 == 0 {
            println!(
                "weapon strike {shard_name}: {trial} casts each; {:.3}s warm",
                warm.elapsed().as_secs_f64()
            );
        }
    }
    verify_context(shard, shard_name, &characters, level_index)?;
    let counts = total(&characters);
    println!(
        "weapon strike {shard_name}: seed {seed}; warm {:.3}s; counts {}",
        warm.elapsed().as_secs_f64(),
        serde_json::to_string(&counts)?
    );
    Ok(Sample::Complete { counts })
}

fn measure(scenario: &Scenario, seed: u64, prefix: &str) -> Result<[Sample; 2]> {
    let mut shard = Standalone::start_persistent(prefix);
    let names = super::rolls::fresh_shards(&mut shard, "weapon", 4);
    let ready = std::sync::Barrier::new(4);
    let parts = std::thread::scope(|scope| -> Result<Vec<Sample>> {
        let mut jobs = Vec::new();
        for level in 0..2 {
            for part in 0..2 {
                let shard = &shard;
                let name = &names[level * 2 + part];
                let ready = &ready;
                jobs.push(
                    scope.spawn(move || collect(shard, name, scenario, seed, level, part, ready)),
                );
            }
        }
        jobs.into_iter()
            .map(|job| {
                job.join()
                    .map_err(|_| anyhow::anyhow!("weapon strike collector panicked"))?
            })
            .collect()
    })?;
    Ok(std::array::from_fn(|level| {
        let pair = &parts[level * 2..level * 2 + 2];
        let mut counts = pair[0].counts().clone();
        counts.add(pair[1].counts());
        let blockers: Vec<_> = pair
            .iter()
            .filter_map(|sample| match sample {
                Sample::Blocked { reason, .. } => Some(reason.as_str()),
                Sample::Complete { .. } => None,
            })
            .collect();
        if blockers.is_empty() {
            Sample::Complete { counts }
        } else {
            Sample::Blocked {
                counts,
                reason: blockers.join("; "),
            }
        }
    }))
}

fn compare_samples(samples: &[Sample; 2], slot: &str) -> Result<Vec<Difference>> {
    let mut differences = Vec::new();
    for (level, sample) in samples.iter().enumerate() {
        match sample {
            Sample::Complete { counts } => differences.extend(spec::compare(counts, level, slot)?),
            Sample::Blocked { counts, reason } => {
                differences.push(spec::blocked(counts, level, slot, reason));
            }
        }
    }
    Ok(differences)
}

/// Collects both public seeds; their rule signatures join `white_melee_outcomes`.
pub(super) fn public_seeds(scenario: &Scenario) -> Result<Vec<Difference>> {
    let mut measurements = Vec::new();
    for seed in PUBLIC_SEEDS {
        measurements.push(measure(scenario, seed, "weapon-strike-public")?);
    }
    let report = core_root().join(".lyracore/conformance/weapon-strike-rates.json");
    std::fs::create_dir_all(report.parent().context("weapon strike report parent")?)?;
    std::fs::write(&report, serde_json::to_vec_pretty(&measurements)?)?;
    let complete = measurements
        .iter()
        .flatten()
        .all(|sample| matches!(sample, Sample::Complete { .. }));
    if complete && std::env::var_os("LYRACORE_ROLL_REPEATABILITY").is_some() {
        ensure!(
            measure(scenario, PUBLIC_SEEDS[0], "weapon-strike-repeat")? == measurements[0],
            "same seed changed delivered weapon strike counts"
        );
    }
    if complete {
        ensure!(
            measurements[0] != measurements[1],
            "different seed did not change delivered weapon strike counts"
        );
    }
    let mut differences = Vec::new();
    for (slot, samples) in measurements.iter().enumerate() {
        differences.extend(compare_samples(samples, &format!("public_{}", slot + 1))?);
    }
    Ok(differences)
}

pub(super) fn merge_seed(scenario: &Scenario, seed: u64) -> Result<Vec<Difference>> {
    let samples = measure(scenario, seed, "weapon-strike-merge")?;
    println!(
        "weapon strike merge {seed:#018x}: {}",
        serde_json::to_string(&samples)?
    );
    compare_samples(&samples, "merge")
}

const CERTAIN_HIT: u32 = 5_098_601;
const LIKELY_MISS: u32 = 5_098_602;
const CERTAIN_DODGE: u32 = 5_098_603;
const CERTAIN_PARRY: u32 = 5_098_604;
const NO_AVOIDANCE: u32 = 5_098_605;
// A full miss band still leaves 40%, since the attack table caps miss at 60%.
const MISS_ATTEMPTS: usize = 40;

/// Self Auras whose combat-field changes, in whole percent, force a band of the attack table.
fn stage_forcing_auras(shard: &Standalone, shard_name: &str) {
    use lyracore_package_delta::packed_spell_effect_id;
    use lyracore_shared::spell::taxonomy::{
        A_MOD_COMBAT, COMBAT_DODGE, COMBAT_HIT, COMBAT_PARRY, P_COMBAT_PERCENT, T_SELF,
    };

    let spells: [(u32, &[(u8, i32)]); 5] = [
        (CERTAIN_HIT, &[(COMBAT_HIT, 100)]),
        (LIKELY_MISS, &[(COMBAT_HIT, -100)]),
        (CERTAIN_DODGE, &[(COMBAT_DODGE, 100)]),
        (CERTAIN_PARRY, &[(COMBAT_DODGE, -100), (COMBAT_PARRY, 100)]),
        (NO_AVOIDANCE, &[(COMBAT_DODGE, -100), (COMBAT_PARRY, -100)]),
    ];
    for (spell, changes) in spells {
        let name = format!("Attack table fixture {spell}");
        shard.insert_row_database(
            shard_name,
            "game_spell",
            &[
                ("spell_id", &spell),
                ("name", &name),
                ("duration_ms", &3_600_000u32),
                ("max_duration_ms", &3_600_000u32),
                ("spell_level", &1u32),
            ],
        );
        for (index, (field, percent)) in changes.iter().enumerate() {
            let index = index as u8;
            shard.insert_row_database(
                shard_name,
                "game_spell_effect",
                &[
                    ("id", &packed_spell_effect_id(spell, index)),
                    ("spell_id", &spell),
                    ("effect_index", &index),
                    ("kind", &A_MOD_COMBAT),
                    ("base_points", percent),
                    ("target", &T_SELF),
                    ("p_0", &i32::from(*field)),
                    ("p_0_kind", &P_COMBAT_PERCENT),
                ],
            );
        }
    }
}

fn force(shard: &Standalone, shard_name: &str, unit: u64, spell: u32) {
    shard.assert_call_database(
        shard_name,
        "debug_force_cast",
        &[&unit.to_string(), &spell.to_string()],
    );
}

/// Casts for each listed Character, routes the batch to every Character and returns their packets.
fn cast_round(
    receiver: &mut ReducerConnection,
    characters: &mut [Character],
    casting: &[usize],
) -> Result<Vec<Vec<RoutedPacket>>> {
    for &index in casting {
        cast(&mut characters[index]).map_err(anyhow::Error::msg)?;
    }
    let rows = advance_and_drain(receiver)?;
    characters
        .iter_mut()
        .map(|character| character.delivery.route_rows(rows.clone()))
        .collect()
}

fn observe(character: &mut Character, routed: &[RoutedPacket]) -> Result<()> {
    let packets = decoded(routed)?;
    character
        .counts
        .observe(&packets, character.guid, character.target)
        .with_context(|| format!("Character {}: {packets:?}", character.guid))
}

/// Whether the packet is an SMSG_UPDATE_OBJECT that holds one values block for `guid`.
fn values_update_of(packet: &RoutedPacket, guid: u64) -> bool {
    // Block count u32 and no transport, then UPDATETYPE_VALUES and the packed guid.
    let Some((mask, rest)) = packet.body.get(6..).and_then(|body| body.split_first()) else {
        return false;
    };
    let mut bytes = rest.iter();
    let unpacked: [u8; 8] = std::array::from_fn(|bit| {
        if mask & (1 << bit) != 0 {
            bytes.next().copied().unwrap_or(0)
        } else {
            0
        }
    });
    packet.opcode == 0xa9
        && packet.body.starts_with(&[1, 0, 0, 0, 0, 0])
        && u64::from_le_bytes(unpacked) == guid
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_cast_weapon_strike_reaches_its_caster_as_one_go_that_names_its_outcome() -> Result<()> {
    let golden = wire_client::catalogue::get("white_melee_outcomes")?;
    let mut shard = Standalone::start("weapon-strike-outcome-go");
    shard.publish_module();
    let shard_name = shard.shard_name().to_string();
    let (mut receiver, mut characters) = prepare(
        &shard,
        &shard_name,
        golden.scenario(),
        PUBLIC_SEEDS[0],
        0,
        0,
    )?;
    stage_forcing_auras(&shard, &shard_name);
    let forcing = [
        (CERTAIN_HIT, CERTAIN_DODGE),
        (CERTAIN_HIT, CERTAIN_PARRY),
        (LIKELY_MISS, NO_AVOIDANCE),
        (CERTAIN_HIT, NO_AVOIDANCE),
    ];
    ensure!(
        characters.len() == forcing.len(),
        "four weapon strike Characters"
    );
    for (character, (caster_aura, target_aura)) in characters.iter().zip(forcing) {
        force(&shard, &shard_name, character.guid, caster_aura);
        force(&shard, &shard_name, character.target, target_aura);
    }

    let packets = cast_round(&mut receiver, &mut characters, &[0, 1, 3])?;
    for index in [0, 1, 3] {
        observe(&mut characters[index], &packets[index])?;
    }
    ensure!(
        characters[0].counts.0 == [0, 1, 0, 0, 0, 0],
        "a certain dodge"
    );
    ensure!(
        characters[1].counts.0 == [0, 0, 1, 0, 0, 0],
        "a certain parry"
    );
    ensure!(characters[3].counts.trials() == 1 && characters[3].counts.0[..3] == [0, 0, 0]);
    let landed = &packets[3];
    let position = |found: &dyn Fn(&RoutedPacket) -> bool| landed.iter().position(found);
    let go = position(&|packet| packet.opcode == spec::SPELL_GO);
    let log = position(&|packet| packet.opcode == 0x0250);
    let debuff = position(&|packet| values_update_of(packet, characters[3].target));
    // The GO precedes the effects' packets, or the client keeps its cast slot busy.
    ensure!(
        go.zip(debuff).is_some_and(|(go, debuff)| go < debuff)
            && go.zip(log).is_some_and(|(go, log)| go < log),
        "vmangos sends the GO before the target's values update and the damage log: {landed:?}"
    );

    for _ in 0..MISS_ATTEMPTS {
        let packets = cast_round(&mut receiver, &mut characters, &[2])?;
        observe(&mut characters[2], &packets[2])?;
        if characters[2].counts.0[0] > 0 {
            return Ok(());
        }
    }
    anyhow::bail!(
        "no miss in {MISS_ATTEMPTS} casts: {:?}",
        characters[2].counts
    )
}
