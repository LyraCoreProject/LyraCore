use std::collections::BTreeMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use lyracore_test_support::{core_root, ReducerConnection, Standalone};
use serde_json::json;
use wire_client::compare::Comparison;
use wire_client::decode::decode;
use wire_client::rolls::{self, Counts, PUBLIC_SEEDS};
use wire_client::scenario::FixtureVerb;

use super::transport::{prepare_character, roll_packets, Delivery};

struct Character {
    connection: ReducerConnection,
    guid: u64,
    target: u64,
    counts: Counts,
    started: bool,
    stopped: bool,
    damage: u64,
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

struct RollFixture {
    collector: ReducerConnection,
    characters: Vec<Character>,
    deliveries: BTreeMap<u64, Delivery>,
}

fn prepare(
    shard: &Standalone,
    shard_name: &str,
    scenario: &wire_client::scenario::Scenario,
    seed: u64,
    level_index: usize,
    part: usize,
) -> Result<RollFixture> {
    let spec = scenario.rolls.as_ref().context("roll specification")?;
    ensure!(
        spec.agility == 100
            && spec.weapon_skill == 300
            && spec.defense_skill == 300
            && spec.target_faction == 7
            && spec.target_entry == 38
            && spec.target_levels == [60, 63],
        "unsupported tier 1 roll fixture context"
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
            ("name", &"White melee target"),
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
        anyhow::bail!("roll Character fixture")
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
    let mut prepared = Vec::new();
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
            class: "warrior".into(),
            level: 60,
            at,
        };
        let guid = prepare_character(shard, shard_name, &format!("ROLL{index}"), &fixture)?;
        shard.assert_call_database(
            shard_name,
            "debug_spawn_at_feet",
            &[&guid.to_string(), "38", "2"],
        );
        let target: u64 = shard
            .query_database_rows(
                shard_name,
                "SELECT guid FROM game_world_entity WHERE entry = 38",
            )
            .iter()
            .map(|row| row["guid"].parse::<u64>())
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter()
            .max()
            .context("roll target")?;
        prepared.push((index, guid, target));
    }
    let mut collector =
        shard.connect_shard_reducers(shard_name, &["SELECT * FROM game_native_packet"]);
    let mut characters = Vec::new();
    let mut deliveries = BTreeMap::new();
    for (index, guid, target) in prepared {
        let mut connection = shard.connect_shard_reducers(shard_name, &[]);
        connection
            .call("native_world_open", json!([42]))
            .map_err(anyhow::Error::msg)?;
        connection
            .call("debug_native_enter_world", json!([guid]))
            .map_err(anyhow::Error::msg)?;
        shard.assert_call_database(
            shard_name,
            "debug_prepare_roll_character",
            &[
                &guid.to_string(),
                &target.to_string(),
                &spec.target_levels[index / 8].to_string(),
                &seed.to_string(),
                &(index as u64).to_string(),
            ],
        );
        deliveries.insert(guid, Delivery::new(connection.connection_id()));
        characters.push(Character {
            connection,
            guid,
            target,
            counts: Counts::default(),
            started: false,
            stopped: false,
            damage: 0,
        });
    }
    collector
        .call("debug_advance_roll_batch", json!([1]))
        .map_err(anyhow::Error::msg)?;
    let opening = collector.drain_packets();
    for delivery in deliveries.values_mut() {
        delivery.route_rows(opening.clone())?;
    }
    Ok(RollFixture {
        collector,
        characters,
        deliveries,
    })
}

fn collect(
    shard: &Standalone,
    shard_name: &str,
    scenario: &wire_client::scenario::Scenario,
    seed: u64,
    level_index: usize,
    part: usize,
    ready: &std::sync::Barrier,
) -> Result<Counts> {
    let mut preparation = Preparation {
        ready,
        arrived: false,
    };
    let setup = Instant::now();
    let RollFixture {
        mut collector,
        mut characters,
        mut deliveries,
    } = prepare(shard, shard_name, scenario, seed, level_index, part)?;
    println!(
        "roll seed {seed}: setup {:.3}s",
        setup.elapsed().as_secs_f64()
    );
    preparation.arrived = true;
    ready.wait();
    let warm = Instant::now();
    for character in &mut characters {
        character
            .connection
            .call(
                "native_world_receive",
                json!([0x141u32, character.target.to_le_bytes().to_vec()]),
            )
            .map_err(anyhow::Error::msg)?;
    }
    let per_character = scenario.rolls.as_ref().unwrap().samples_per_level / 8;
    let mut batches = 0;
    while characters
        .iter()
        .any(|character| character.counts.total() < per_character)
    {
        batches += 1;
        ensure!(
            batches <= 125,
            "incomplete white melee sample: {:?}",
            characters
                .iter()
                .map(|c| c.counts.total())
                .collect::<Vec<_>>()
        );
        let remaining = characters
            .iter()
            .map(|c| per_character - c.counts.total())
            .min()
            .unwrap();
        let ticks = (remaining * 2).min(100);
        for character in &mut characters {
            character
                .connection
                .call("native_world_receive", json!([0x1dcu32, vec![0u8; 8]]))
                .map_err(anyhow::Error::msg)?;
        }
        collector
            .call("debug_advance_roll_batch", json!([ticks]))
            .map_err(anyhow::Error::msg)?;
        let rows = collector.drain_packets();
        if batches % 10 == 0 {
            println!(
                "roll batch {batches}, elapsed {:.3}s, counts {:?}",
                warm.elapsed().as_secs_f64(),
                characters
                    .iter()
                    .map(|c| c.counts.total())
                    .collect::<Vec<_>>()
            );
        }
        for (guid, packet) in roll_packets(rows, &mut deliveries)? {
            let character = characters
                .iter_mut()
                .find(|character| character.guid == guid)
                .unwrap();
            if !matches!(packet.opcode, 0x143 | 0x144 | 0x14a) {
                continue;
            }
            let packet = decode(packet.opcode, &packet.body)?;
            if packet.fields.get("attacker")
                != Some(&wire_client::decode::Field::Guid(character.guid))
            {
                continue;
            }
            match packet.opcode {
                0x143 => character.started = true,
                0x144 => character.stopped = true,
                0x14a => {
                    ensure!(
                        character.counts.total() < per_character,
                        "extra white melee sample"
                    );
                    character
                        .counts
                        .observe(&packet, character.guid, character.target)?;
                    if let Some(wire_client::decode::Field::Unsigned(damage)) =
                        packet.fields.get("damage")
                    {
                        character.damage += damage;
                    }
                }
                _ => unreachable!(),
            }
        }
        ensure!(
            characters.iter().all(|c| c.started && c.counts.total() > 0),
            "missing attack start or first delivered swing"
        );
    }
    for character in &mut characters {
        character
            .connection
            .call("native_world_receive", json!([0x142u32, Vec::<u8>::new()]))
            .map_err(anyhow::Error::msg)?;
    }
    collector
        .call("debug_advance_roll_batch", json!([40]))
        .map_err(anyhow::Error::msg)?;
    let rows = collector.drain_packets();
    for (guid, packet) in roll_packets(rows, &mut deliveries)? {
        let character = characters.iter_mut().find(|c| c.guid == guid).unwrap();
        if packet.opcode == 0x144 {
            let packet = decode(packet.opcode, &packet.body)?;
            if packet.fields.get("attacker")
                == Some(&wire_client::decode::Field::Guid(character.guid))
            {
                character.stopped = true;
            }
        }
        if packet.opcode == 0x14a {
            let packet = decode(packet.opcode, &packet.body)?;
            ensure!(
                packet.fields.get("attacker")
                    != Some(&wire_client::decode::Field::Guid(character.guid)),
                "swing after attack stop"
            );
        }
    }
    let counts = verified_counts(
        shard,
        shard_name,
        &characters,
        scenario.rolls.as_ref().unwrap().target_levels[level_index],
    )?;
    println!(
        "roll seed {seed}: warm {:.3}s; counts {}",
        warm.elapsed().as_secs_f64(),
        serde_json::to_string(&counts)?
    );
    Ok(counts)
}

fn verified_counts(
    shard: &Standalone,
    shard_name: &str,
    characters: &[Character],
    target_level: u32,
) -> Result<Counts> {
    ensure!(
        shard
            .query_database_rows(shard_name, "SELECT id FROM game_aura")
            .is_empty(),
        "roll participants have auras"
    );
    ensure!(
        shard
            .query_database_rows(shard_name, "SELECT guid FROM game_item_instance")
            .is_empty(),
        "roll participants have items"
    );
    let entities = shard.query_database_rows(shard_name,
        "SELECT guid,level,agility,health,max_health,dead,map_id,x,y,z,orientation FROM game_world_entity");
    let skills = shard.query_database_rows(
        shard_name,
        "SELECT character_guid,skill_line,current,max_rank FROM game_player_skill",
    );
    let identities = shard.query_database_rows(
        shard_name,
        "SELECT guid,race,class,level FROM game_character",
    );
    let mut counts = Counts::default();
    for character in characters {
        ensure!(
            character.started && character.stopped && character.damage > 0,
            "missing gameplay witness"
        );
        let entity = entities
            .iter()
            .find(|row| row["guid"] == character.guid.to_string())
            .context("roll Character disappeared")?;
        let target = entities
            .iter()
            .find(|row| row["guid"] == character.target.to_string())
            .context("roll target disappeared")?;
        let identity = identities
            .iter()
            .find(|row| row["guid"] == character.guid.to_string())
            .context("roll Character identity disappeared")?;
        ensure!(
            identity["race"] == "1" && identity["class"] == "1" && identity["level"] == "60",
            "roll Character race, class or level changed"
        );
        ensure!(
            entity["level"] == "60"
                && entity["agility"] == "100"
                && target["level"] == target_level.to_string(),
            "roll levels or agility changed"
        );
        for participant in [entity, target] {
            ensure!(
                participant["dead"] == "false"
                    && participant["health"].parse::<u32>()? > 0
                    && participant["max_health"] == "1000000000"
                    && participant["map_id"] == "1",
                "roll health or map changed"
            );
        }
        for skill in [95, 162] {
            let row = skills
                .iter()
                .find(|row| {
                    row["character_guid"] == character.guid.to_string()
                        && row["skill_line"] == skill.to_string()
                })
                .context("roll skill disappeared")?;
            ensure!(
                row["current"] == "300" && row["max_rank"] == "300",
                "roll skill changed"
            );
        }
        let coordinate = |row: &BTreeMap<String, String>, name: &str| -> Result<f32> {
            Ok(row.get(name).context("roll coordinate missing")?.parse()?)
        };
        ensure!(
            (coordinate(target, "x")? - coordinate(entity, "x")? - 2.0).abs() < 0.01
                && (coordinate(target, "y")? - coordinate(entity, "y")?).abs() < 0.01
                && (coordinate(target, "z")? - coordinate(entity, "z")?).abs() < 0.01
                && coordinate(entity, "orientation")?.cos() > 0.9999
                && coordinate(target, "orientation")?.cos() < -0.9999,
            "roll position or facing changed"
        );
        for category in 0..7 {
            counts.0[category] += character.counts.0[category];
        }
    }
    println!(
        "roll fixture {shard_name}: verified {} level-60 human warriors, agility 100, unarmed/defense 300/300, no items or auras",
        characters.len()
    );
    Ok(counts)
}

fn fresh_shards(shard: &mut Standalone, prefix: &str, count: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            let shard_name = format!("{}-{prefix}-{index}", shard.shard_name());
            shard.publish_named_module_bytes(&shard_name, lyracore_test_support::module_bytes());
            shard_name
        })
        .collect()
}

fn add_counts(first: &Counts, second: &Counts) -> Counts {
    Counts(std::array::from_fn(|category| {
        first.0[category] + second.0[category]
    }))
}

fn measure(scenario: &wire_client::scenario::Scenario) -> Result<Vec<[Counts; 2]>> {
    let mut shard = Standalone::start_persistent("white-melee-public");
    let shard_names = fresh_shards(&mut shard, "public", 8);
    let ready = std::sync::Barrier::new(8);
    let parts = std::thread::scope(|scope| -> Result<Vec<Counts>> {
        let mut jobs = Vec::new();
        for (slot, seed) in PUBLIC_SEEDS.into_iter().enumerate() {
            for level in 0..2 {
                for part in 0..2 {
                    let ready = &ready;
                    let shard = &shard;
                    let shard_name = &shard_names[slot * 4 + level * 2 + part];
                    jobs.push(scope.spawn(move || {
                        collect(shard, shard_name, scenario, seed, level, part, ready)
                    }));
                }
            }
        }
        jobs.into_iter()
            .map(|job| {
                job.join()
                    .map_err(|_| anyhow::anyhow!("roll collector panicked"))?
            })
            .collect()
    })?;
    let measurements = vec![
        [
            add_counts(&parts[0], &parts[1]),
            add_counts(&parts[2], &parts[3]),
        ],
        [
            add_counts(&parts[4], &parts[5]),
            add_counts(&parts[6], &parts[7]),
        ],
    ];
    if std::env::var_os("LYRACORE_ROLL_REPEATABILITY").is_some() {
        let shard_names = fresh_shards(&mut shard, "repeat", 4);
        let ready = std::sync::Barrier::new(4);
        std::thread::scope(|scope| -> Result<()> {
            let mut jobs = Vec::new();
            for level in 0..2 {
                for part in 0..2 {
                    let shard = &shard;
                    let shard_name = &shard_names[level * 2 + part];
                    let ready = &ready;
                    let expected = &parts[level * 2 + part];
                    jobs.push(scope.spawn(move || -> Result<()> {
                        ensure!(
                            *expected
                                == collect(
                                    shard,
                                    shard_name,
                                    scenario,
                                    PUBLIC_SEEDS[0],
                                    level,
                                    part,
                                    ready
                                )?,
                            "same seed changed delivered counts"
                        );
                        Ok(())
                    }));
                }
            }
            for job in jobs {
                job.join()
                    .map_err(|_| anyhow::anyhow!("repeat collector panicked"))??;
            }
            Ok(())
        })?;
    }
    ensure!(
        measurements[0] != measurements[1],
        "different seed did not change delivered counts"
    );
    Ok(measurements)
}

pub(super) fn public_seeds() -> Result<()> {
    let golden = wire_client::catalogue::get("white_melee_outcomes")?;
    let path = std::env::var_os("LYRACORE_CONFORMANCE_RESULT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| core_root().join(".lyracore/conformance/tier1.json"));
    std::fs::create_dir_all(path.parent().context("tier 1 result parent")?)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    super::tier1::update_result(&path, &golden, None, &[], now)?;
    let mut comparison = Comparison {
        differences: vec![],
    };
    let measurements = measure(golden.scenario())?;
    for (slot, counts) in measurements.iter().enumerate() {
        for (level, counts) in counts.iter().enumerate() {
            comparison.differences.extend(
                rolls::compare(
                    golden.scenario().rolls.as_ref().unwrap(),
                    counts,
                    level,
                    &format!("public_{}", slot + 1),
                    50_000,
                )?
                .differences,
            );
        }
    }
    let report = core_root().join(".lyracore/conformance/white-melee-rates.json");
    std::fs::create_dir_all(report.parent().context("roll rate report parent")?)?;
    std::fs::write(report, serde_json::to_vec_pretty(&measurements)?)?;
    super::tier1::record_and_check(
        "white_melee_outcomes",
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
    let golden = wire_client::catalogue::get("white_melee_outcomes")?;
    let mut shard = Standalone::start_persistent("white-melee-merge");
    let shard_names = fresh_shards(&mut shard, "merge", 4);
    let ready = std::sync::Barrier::new(4);
    let parts = std::thread::scope(|scope| -> Result<Vec<Counts>> {
        let mut jobs = Vec::new();
        for level in 0..2 {
            for part in 0..2 {
                let golden = &golden;
                let ready = &ready;
                let shard = &shard;
                let shard_name = &shard_names[level * 2 + part];
                jobs.push(scope.spawn(move || {
                    collect(
                        shard,
                        shard_name,
                        golden.scenario(),
                        seed,
                        level,
                        part,
                        ready,
                    )
                }));
            }
        }
        jobs.into_iter()
            .map(|job| {
                job.join()
                    .map_err(|_| anyhow::anyhow!("roll collector panicked"))?
            })
            .collect()
    })?;
    let counts = [
        add_counts(&parts[0], &parts[1]),
        add_counts(&parts[2], &parts[3]),
    ];
    let spec = golden.scenario().rolls.as_ref().unwrap();
    let mut comparison = Comparison {
        differences: vec![],
    };
    for (level, counts) in counts.iter().enumerate() {
        comparison
            .differences
            .extend(rolls::compare(spec, counts, level, "merge", 50_000)?.differences);
    }
    println!(
        "merge {commit}: {}",
        serde_json::to_string_pretty(
            &json!({"seed": format!("{seed:#018x}"), "counts": &counts, "comparison": &comparison})
        )?
    );
    let current = super::tier1::ratchet::ScenarioBaseline::from_comparison(
        "white_melee_outcomes",
        "auto_attack",
        lyracore_conformance::harness_pin().revision,
        &golden.trace().header.scenario_sha256,
        &comparison,
    );
    check_merge_signatures(
        &super::tier1::ratchet::Baseline::read(
            &core_root().join("conformance/tier1-baseline.json"),
        )?,
        &current,
    )?;
    Ok(())
}

pub(super) fn check_merge_signatures(
    baseline: &super::tier1::ratchet::Baseline,
    current: &super::tier1::ratchet::ScenarioBaseline,
) -> Result<()> {
    let expected = baseline
        .scenarios
        .iter()
        .find(|entry| entry.scenario == current.scenario)
        .context("no held public roll signatures")?;
    ensure!(
        expected.behavior == current.behavior
            && expected.harness_revision == current.harness_revision
            && expected.scenario_sha256 == current.scenario_sha256,
        "merge seed baseline provenance differs"
    );
    let allowed = |signatures: &[super::tier1::ratchet::Signature]| {
        signatures
            .iter()
            .cloned()
            .map(|mut signature| {
                signature.checkpoint = signature
                    .checkpoint
                    .replace("_public_1", "_merge")
                    .replace("_public_2", "_merge");
                signature
            })
            .collect::<std::collections::BTreeSet<_>>()
    };
    let rules = allowed(&expected.rule);
    let data_signatures = allowed(&expected.data);
    ensure!(
        current
            .rule
            .iter()
            .all(|signature| rules.contains(signature))
            && current
                .data
                .iter()
                .all(|signature| data_signatures.contains(signature)),
        "merge seed adds a rule signature; revert, do not retry"
    );
    Ok(())
}

#[test]
fn merge_seed_holds_known_categories_without_requiring_their_redetection() {
    use super::tier1::ratchet::{Baseline, ScenarioBaseline};
    use wire_client::compare::{Difference, DifferenceClass};
    use wire_client::scenario::Stream;
    use wire_client::trace::Sha256Digest;

    let difference = |slot: &str, category: &str| Difference {
        classification: DifferenceClass::Rule,
        character: None,
        checkpoint: format!("level_60_{slot}"),
        stream: Some(Stream::Broadcast("swings".into())),
        packet_index: None,
        field_path: vec!["outcomes".into(), category.into()],
        opcode: Some(0x14a),
        deviations: vec![],
        rule: "white_melee_distribution".into(),
        reason: String::new(),
        expected: None,
        actual: None,
    };
    let entry = |differences| {
        ScenarioBaseline::from_comparison(
            "white_melee_outcomes",
            "auto_attack",
            "harness",
            &Sha256Digest::of(b"merge seed policy"),
            &Comparison { differences },
        )
    };
    let baseline = Baseline {
        schema_version: 1,
        scenarios: vec![entry(vec![
            difference("public_1", "miss"),
            difference("public_2", "crit"),
        ])],
    };
    assert!(check_merge_signatures(&baseline, &entry(vec![])).is_ok());
    let mut known = entry(vec![difference("merge", "crit")]);
    assert!(check_merge_signatures(&baseline, &known).is_ok());
    assert!(check_merge_signatures(&baseline, &entry(vec![difference("merge", "dodge")])).is_err());
    known.harness_revision = "other harness".into();
    assert!(check_merge_signatures(&baseline, &known).is_err());
}
