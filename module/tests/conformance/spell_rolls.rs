use std::collections::BTreeMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use lyracore_test_support::{core_root, ReducerConnection, Standalone};
use serde_json::json;
use wire_client::compare::{Comparison, Difference, DifferenceClass};
use wire_client::decode::{decode, Field};
use wire_client::rolls::PUBLIC_SEEDS;
use wire_client::scenario::Stream;
use wire_client::scenario::{FixtureVerb, Scenario};
use wire_client::spell_rolls::{self, Collector, Counts};
use wire_client::trace::RecordedField;

use super::transport::{prepare_character, Delivery};

struct Character {
    connection: ReducerConnection,
    guid: u64,
    target: u64,
    cast: (u32, Vec<u8>),
    collector: Collector,
    delivery: Delivery,
    damage: u64,
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

fn total_counts(characters: &[Character]) -> Counts {
    let mut counts = Counts::default();
    for character in characters {
        for (sum, count) in counts.0.iter_mut().zip(character.collector.counts.0) {
            *sum += count;
        }
    }
    counts
}

struct Preparation<'a> {
    ready: &'a std::sync::Barrier,
    arrived: bool,
}

impl Drop for Preparation<'_> {
    fn drop(&mut self) {
        if !self.arrived {
            self.ready.wait();
        }
    }
}

fn prepare(
    shard: &Standalone,
    shard_name: &str,
    scenario: &Scenario,
    seed: u64,
    level_index: usize,
    part: usize,
) -> Result<(ReducerConnection, Vec<Character>)> {
    let spec = scenario
        .spell_rolls
        .as_ref()
        .context("spell specification")?;
    ensure!(
        spec.spell_id == 2948
            && spec.intellect == 125
            && spec.target_entry == 38
            && spec.target_faction == 7
            && spec.target_levels == [60, 63],
        "unsupported spell roll context"
    );
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
            ("name", &"Fixture spell target"),
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
        anyhow::bail!("spell Character fixture")
    };
    let names = [
        "Rollalpha",
        "Rollbeta",
        "Rollgamma",
        "Rolldelta",
        "Rollepsilon",
        "Rollzeta",
        "Rolleta",
        "Rolltheta",
        "Rolliota",
        "Rollkappa",
        "Rolambda",
        "Rollmu",
        "Rollnu",
        "Rollxi",
        "Rollomicron",
        "Rollpi",
    ];
    let mut participants = Vec::new();
    for (index, name) in names
        .iter()
        .enumerate()
        .filter(|(index, _)| index / 8 == level_index && index % 8 / 4 == part)
    {
        let mut at = at.clone();
        at.y -= index as f32 * 200.0;
        let fixture = FixtureVerb::Character {
            name: (*name).into(),
            race: "human".into(),
            class: "mage".into(),
            level: 60,
            at,
        };
        let guid = prepare_character(shard, shard_name, &format!("SPELL{index}"), &fixture)?;
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
            .context("spell target")?;
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
            "debug_prepare_spell_roll_character",
            &[
                &guid.to_string(),
                &target.to_string(),
                &spec.target_levels[level_index].to_string(),
                &seed.to_string(),
                &(index as u64).to_string(),
            ],
        );
        characters.push(Character {
            delivery: Delivery::new(connection.connection_id()),
            connection,
            guid,
            target,
            cast: scenario.steps[0]
                .send
                .as_ref()
                .context("spell cast request")?
                .encode(guid, &BTreeMap::from([("target".into(), target)]))?,
            collector: Collector::new(guid, target, spec.spell_id, 2, 1500)?,
            damage: 0,
        });
    }
    receiver
        .call("debug_advance_roll_batch", json!([1]))
        .map_err(anyhow::Error::msg)?;
    let opening = receiver.drain_packets();
    for character in &mut characters {
        character.delivery.route_rows(opening.clone())?;
    }
    verify_context(
        shard,
        shard_name,
        &characters,
        spec.target_levels[level_index],
        false,
    )?;
    Ok((receiver, characters))
}

fn verify_context(
    shard: &Standalone,
    shard_name: &str,
    characters: &[Character],
    target_level: u32,
    measured: bool,
) -> Result<()> {
    for (table, column) in [
        ("game_aura", "id"),
        ("game_item_instance", "guid"),
        ("game_character_talent", "id"),
    ] {
        ensure!(
            shard
                .query_database_rows(shard_name, &format!("SELECT {column} FROM {table}"))
                .is_empty(),
            "spell participants have items or auras"
        );
    }
    let entities = shard.query_database_rows(shard_name,
        "SELECT guid,level,intellect,health,max_health,power,max_power,dead,map_id,x,y,z,orientation,faction_template FROM game_world_entity");
    let identities = shard.query_database_rows(
        shard_name,
        "SELECT guid,race,class,level FROM game_character",
    );
    let reactions = shard.query_database_rows(
        shard_name,
        "SELECT creature_guid,react_state FROM game_creature_ai_state",
    );
    for character in characters {
        let entity = entities
            .iter()
            .find(|row| row["guid"] == character.guid.to_string())
            .context("spell Character disappeared")?;
        let target = entities
            .iter()
            .find(|row| row["guid"] == character.target.to_string())
            .context("spell target disappeared")?;
        let identity = identities
            .iter()
            .find(|row| row["guid"] == character.guid.to_string())
            .context("spell identity disappeared")?;
        let reaction = reactions
            .iter()
            .find(|row| row["creature_guid"] == character.target.to_string())
            .context("spell target has no passive reaction state")?;
        ensure!(
            reaction["react_state"] == "0",
            "spell target is not passive: {}",
            reaction["react_state"]
        );
        ensure!(
            identity["race"] == "1" && identity["class"] == "8" && identity["level"] == "60",
            "spell race, class or Character level changed"
        );
        ensure!(
            entity["level"] == "60"
                && entity["intellect"] == "125"
                && target["level"] == target_level.to_string()
                && target["faction_template"] == "7"
                && entity["power"].parse::<u32>()? > 50
                && entity["max_power"] == "1000000000",
            "spell level, intellect, faction or mana changed"
        );
        for participant in [entity, target] {
            ensure!(
                participant["dead"] == "false"
                    && participant["health"].parse::<u32>()? > 0
                    && participant["max_health"] == "1000000000"
                    && participant["map_id"] == "1",
                "spell health or map changed"
            );
        }
        let coordinate = |row: &BTreeMap<String, String>, name: &str| -> Result<f32> {
            Ok(row.get(name).context("spell coordinate missing")?.parse()?)
        };
        ensure!(
            (coordinate(target, "x")? - coordinate(entity, "x")? - 2.0).abs() < 0.01
                && (coordinate(target, "y")? - coordinate(entity, "y")?).abs() < 0.01
                && (coordinate(target, "z")? - coordinate(entity, "z")?).abs() < 0.01
                && coordinate(entity, "orientation")?.cos() > 0.9999
                && coordinate(target, "orientation")?.cos() < -0.9999,
            "spell position or facing changed"
        );
        if measured {
            ensure!(
                character.collector.idle()
                    && character.damage > 0
                    && u64::from(target["health"].parse::<u32>()?) + character.damage
                        == 1_000_000_000,
                "delivered spell damage disagrees with target health"
            );
        }
    }
    Ok(())
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
        "spell {shard_name}: setup {:.3}s",
        setup.elapsed().as_secs_f64()
    );
    preparation.arrived = true;
    ready.wait();
    let warm = Instant::now();
    let per_character = scenario.spell_rolls.as_ref().unwrap().samples_per_level / 8;
    for trial in 0..per_character {
        for character in &mut characters {
            character.collector.begin_cast()?;
            character
                .connection
                .call(
                    "native_world_receive",
                    json!([character.cast.0, &character.cast.1]),
                )
                .map_err(anyhow::Error::msg)?;
        }
        receiver
            .call("debug_advance_roll_batch", json!([32]))
            .map_err(anyhow::Error::msg)?;
        let rows = receiver.drain_packets();
        let mut blocked = None;
        for character in &mut characters {
            let mut observed = Vec::new();
            for packet in character.delivery.route_rows(rows.clone())? {
                if !matches!(packet.opcode, 0x130 | 0x131 | 0x132 | 0x250 | 0x14a) {
                    continue;
                }
                let packet = decode(packet.opcode, &packet.body)?;
                observed.push(packet.clone());
                if packet.opcode == 0x14a {
                    continue;
                }
                if character.collector.observe(&packet)? && packet.opcode == 0x250 {
                    let Some(Field::Unsigned(damage)) = packet.fields.get("damage") else {
                        anyhow::bail!("spell damage missing")
                    };
                    character.damage += damage;
                }
            }
            if !character.collector.idle()
                || character.collector.counts.total() != u64::from(trial + 1)
            {
                blocked = Some(format!(
                    "incomplete or duplicate spell trial {} for Character {}; counts {:?}; packets {:?}; pending {:?}; impact {:?}; health {:?}",
                    trial + 1, character.guid, character.collector.counts.0, observed,
                    shard.query_database_rows(shard_name, &format!("SELECT spell_id,pushback_count FROM game_pending_cast WHERE caster_guid = {}", character.guid)),
                    shard.query_database_rows(shard_name, &format!("SELECT spell_id FROM game_pending_spell_impact WHERE caster_guid = {}", character.guid)),
                    shard.query_database_rows(shard_name, &format!("SELECT guid,health FROM game_world_entity WHERE guid = {} OR guid = {}", character.guid, character.target)),
                ));
                break;
            }
        }
        if let Some(reason) = blocked {
            println!(
                "spell {shard_name}: blocked after {:.3}s warm: {reason}",
                warm.elapsed().as_secs_f64()
            );
            return Ok(Sample::Blocked {
                counts: total_counts(&characters),
                reason,
            });
        }
        if (trial + 1) % 1250 == 0 {
            println!(
                "spell {shard_name}: {} casts each; {:.3}s warm",
                trial + 1,
                warm.elapsed().as_secs_f64()
            );
        }
    }
    verify_context(
        shard,
        shard_name,
        &characters,
        scenario.spell_rolls.as_ref().unwrap().target_levels[level_index],
        true,
    )?;
    let counts = total_counts(&characters);
    println!(
        "spell {shard_name}: seed {seed}; warm {:.3}s; counts {}",
        warm.elapsed().as_secs_f64(),
        serde_json::to_string(&counts)?
    );
    Ok(Sample::Complete { counts })
}

fn measure(seed: u64, scenario: &Scenario, prefix: &str) -> Result<[Sample; 2]> {
    let mut shard = Standalone::start_persistent(prefix);
    let names: Vec<_> = (0..4)
        .map(|index| {
            let name = format!("{}-{index}", shard.shard_name());
            shard.publish_named_module_bytes(&name, lyracore_test_support::module_bytes());
            name
        })
        .collect();
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
                    .map_err(|_| anyhow::anyhow!("spell collector panicked"))?
            })
            .collect()
    })?;
    Ok(std::array::from_fn(|level| {
        let pair = &parts[level * 2..level * 2 + 2];
        let counts = Counts(std::array::from_fn(|category| {
            pair[0].counts().0[category] + pair[1].counts().0[category]
        }));
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

fn compare_samples(
    spec: &spell_rolls::Specification,
    samples: &[Sample; 2],
    slot: &str,
) -> Result<Comparison> {
    let mut comparison = Comparison {
        differences: vec![],
    };
    for (level, sample) in samples.iter().enumerate() {
        match sample {
            Sample::Complete { counts } => comparison
                .differences
                .extend(spell_rolls::compare(spec, counts, level, slot, 50_000)?.differences),
            Sample::Blocked { counts, reason } => comparison.differences.push(Difference {
                classification: DifferenceClass::Rule,
                character: None,
                checkpoint: format!("level_{}_{}", spec.target_levels[level], slot),
                stream: Some(Stream::Broadcast("casts".into())),
                packet_index: None,
                field_path: vec!["outcomes".into()],
                opcode: Some(0x250),
                deviations: vec![],
                rule: "spell_sample".into(),
                reason: format!("full spell sample is blocked; rates are unmeasured: {reason}"),
                expected: Some(RecordedField::Unsigned(u64::from(spec.samples_per_level))),
                actual: Some(RecordedField::Unsigned(counts.total())),
            }),
        }
    }
    Ok(comparison)
}

pub(super) fn public_seeds() -> Result<()> {
    let golden = wire_client::catalogue::get("spell_direct_damage_outcomes")?;
    let path = std::env::var_os("LYRACORE_CONFORMANCE_RESULT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| core_root().join(".lyracore/conformance/tier1.json"));
    std::fs::create_dir_all(path.parent().context("spell result parent")?)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    super::tier1::update_result(&path, &golden, None, &[], now)?;
    let mut measurements = Vec::new();
    for seed in PUBLIC_SEEDS {
        measurements.push(measure(seed, golden.scenario(), "spell-roll-public")?);
    }
    let report = core_root().join(".lyracore/conformance/spell-rates.json");
    std::fs::create_dir_all(report.parent().context("spell report parent")?)?;
    std::fs::write(&report, serde_json::to_vec_pretty(&measurements)?)?;
    let complete = measurements
        .iter()
        .flatten()
        .all(|sample| matches!(sample, Sample::Complete { .. }));
    if complete && std::env::var_os("LYRACORE_ROLL_REPEATABILITY").is_some() {
        ensure!(
            measure(PUBLIC_SEEDS[0], golden.scenario(), "spell-roll-repeat")? == measurements[0],
            "same seed changed delivered spell counts"
        );
    }
    if complete {
        ensure!(
            measurements[0] != measurements[1],
            "different seed did not change delivered spell counts"
        );
    }
    let mut comparison = Comparison {
        differences: vec![],
    };
    for (slot, levels) in measurements.iter().enumerate() {
        comparison.differences.extend(
            compare_samples(
                golden.scenario().spell_rolls.as_ref().unwrap(),
                levels,
                &format!("public_{}", slot + 1),
            )?
            .differences,
        );
    }
    super::tier1::record_and_check(
        "spell_direct_damage_outcomes",
        &path,
        &golden,
        now,
        &comparison,
        &[],
    )
}

pub(super) fn merge_seed(commit: &str) -> Result<()> {
    ensure!(
        commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "merge seed needs a full commit SHA"
    );
    let seed = u64::from_str_radix(&commit[..16], 16)?;
    let golden = wire_client::catalogue::get("spell_direct_damage_outcomes")?;
    let counts = measure(seed, golden.scenario(), "spell-roll-merge")?;
    let comparison = compare_samples(
        golden.scenario().spell_rolls.as_ref().unwrap(),
        &counts,
        "merge",
    )?;
    println!(
        "spell merge {commit}: {}",
        serde_json::to_string_pretty(
            &json!({"seed": format!("{seed:#018x}"), "counts": counts, "comparison": comparison})
        )?
    );
    let current = super::tier1::ratchet::ScenarioBaseline::from_comparison(
        "spell_direct_damage_outcomes",
        "cast_and_interrupt",
        super::tier1::harness_revision()?,
        &golden.trace().header.scenario_sha256,
        &comparison,
    );
    super::rolls::check_merge_signatures(
        &super::tier1::ratchet::Baseline::read(
            &core_root().join("conformance/tier1-baseline.json"),
        )?,
        &current,
    )
}

#[test]
fn blocked_spell_samples_never_claim_rate_evidence() -> Result<()> {
    let golden = wire_client::catalogue::get("spell_direct_damage_outcomes")?;
    let samples = std::array::from_fn(|_| Sample::Blocked {
        counts: Counts([0, 0, 0, 2, 0]),
        reason: "passive target caused cast pushback".into(),
    });
    let comparison = compare_samples(
        golden.scenario().spell_rolls.as_ref().unwrap(),
        &samples,
        "public_1",
    )?;
    assert_eq!(comparison.differences.len(), 2);
    for difference in comparison.differences {
        assert_eq!(difference.classification, DifferenceClass::Rule);
        assert_eq!(difference.rule, "spell_sample");
        assert_eq!(difference.expected, Some(RecordedField::Unsigned(50_000)));
        assert_eq!(difference.actual, Some(RecordedField::Unsigned(2)));
        assert_eq!(difference.field_path, ["outcomes"]);
    }
    Ok(())
}
