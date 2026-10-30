use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use lyracore_test_support::{core_root, Standalone};
use serde_json::json;
use wire_client::runner::Transport;
use wire_client::scenario::FixtureVerb;
use wire_client::{catalogue, compare, runner};

use super::properties::{self, Violation};
use super::transport::{MultiOperatorTransport, OperatorTransport};

#[path = "../../../conformance/support/ratchet.rs"]
mod ratchet;

static REPORT: Mutex<()> = Mutex::new(());
const VIRTUAL_START_MICROS: u64 = 1_800_000_000_000_000;

fn harness_revision() -> Result<&'static str> {
    include_str!("../../../.wire-harness-rev")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .flat_map(str::split_whitespace)
        .nth(1)
        .context("harness SHA")
}

fn git(arguments: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(core_root())
        .output()?;
    ensure!(output.status.success(), "git {arguments:?} failed");
    Ok(String::from_utf8(output.stdout)?.trim().into())
}

fn update_result(
    path: &Path,
    golden: &wire_client::trace::GoldenTrace,
    comparison: Option<&compare::Comparison>,
    violations: &[Violation],
    now: u64,
) -> Result<()> {
    let _lock = REPORT.lock().unwrap();
    let revision = harness_revision()?;
    let mut result = json!({
        "schema_version": 2,
        "tier": "tier1",
        "core_commit": git(&["rev-parse", "HEAD"] )?,
        "working_tree_dirty": !git(&["status", "--porcelain", "--untracked-files=normal"] )?.is_empty(),
        "harness_revision": revision,
        "recorded_at_unix_ms": now,
        "scenarios": [],
    });
    match std::fs::read(path) {
        Ok(bytes) => {
            let old: serde_json::Value = serde_json::from_slice(&bytes)?;
            if [
                "schema_version",
                "tier",
                "core_commit",
                "working_tree_dirty",
                "harness_revision",
            ]
            .iter()
            .all(|field| old[field] == result[field])
            {
                result["scenarios"] = old["scenarios"].clone();
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error.into()),
    }
    let scenarios = result["scenarios"]
        .as_array_mut()
        .context("result scenarios")?;
    scenarios.retain(|entry| entry["id"] != golden.scenario().id);
    if let Some(comparison) = comparison {
        scenarios.push(json!({
            "id": golden.scenario().id,
            "behavior": golden.scenario().behavior,
            "scenario_sha256": golden.trace().header.scenario_sha256,
            "comparison": comparison,
            "properties": violations,
        }));
    }
    scenarios.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let temporary = path.with_extension("json.pending");
    std::fs::write(&temporary, serde_json::to_vec_pretty(&result)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn run_scenario(
    id: &str,
    setup: impl FnOnce(&Standalone, &wire_client::scenario::Scenario) -> Result<()>,
    fresh_creature_probe: Option<u32>,
) -> Result<()> {
    let path = std::env::var_os("LYRACORE_CONFORMANCE_RESULT")
        .map(PathBuf::from)
        .unwrap_or_else(|| core_root().join(".lyracore/conformance/tier1.json"));
    std::fs::create_dir_all(path.parent().context("tier 1 result parent")?)?;
    let golden = catalogue::get(id)?;
    let recorded_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    update_result(&path, &golden, None, &[], recorded_at)?;
    let mut shard = Standalone::start(&format!("tier1-{}", id.replace('_', "-")));
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    setup(&shard, golden.scenario())?;
    let virtual_start = VIRTUAL_START_MICROS;
    shard.assert_call("debug_start_virtual_clock", &[&virtual_start.to_string()]);
    let mut transport = OperatorTransport::prepare(&shard, golden.scenario())?;
    let now = virtual_start / 1_000;
    let observations = runner::run_on(golden.scenario(), &mut transport)?;
    ensure!(
        observations
            .iter()
            .any(|observation| observation.checkpoint == "opening"),
        "world entry must remain in the opening"
    );
    let comparison = compare::compare_character_observations(
        &golden,
        &observations,
        transport.character_guid(),
        now,
        &[],
    )?;
    if let Some(entry) = fresh_creature_probe {
        shard.assert_call(
            "debug_spawn_at_feet",
            &[
                &transport.character_guid().to_string(),
                &entry.to_string(),
                "3.0",
            ],
        );
        let mut delivered = false;
        for _ in 0..16 {
            transport.receive(Duration::from_millis(50))?;
            delivered = transport.batches().iter().any(|batch| {
                batch
                    .creates
                    .values()
                    .any(|context| context.newly_added && context.layer == 0)
            });
            if delivered {
                break;
            }
        }
        ensure!(
            delivered,
            "fresh creature was not delivered to the viewing Character"
        );
        probe_door(&shard, &mut transport)?;
    }
    let violations = properties::run(transport.batches())?;
    record_and_check(id, &path, &golden, recorded_at, &comparison, &violations)
}

fn probe_door(shard: &Standalone, transport: &mut OperatorTransport) -> Result<()> {
    let character = transport.character_guid();
    let position = shard
        .query_rows(&format!(
            "SELECT map_id,x,y,z FROM game_world_entity WHERE guid = {character}"
        ))
        .into_iter()
        .next()
        .context("door probe Character")?;
    shard.assert_call(
        "debug_spawn_gameobject",
        &[
            "990001",
            "0",
            "1",
            "0",
            &position["map_id"],
            &position["x"],
            &position["y"],
            &position["z"],
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let door: u64 = shard
        .query_rows("SELECT guid FROM game_gameobject WHERE template_entry = 990001")[0]["guid"]
        .parse()?;
    transport.capture()?;
    for state in [0, 1] {
        shard.assert_call(
            "debug_use_gameobject",
            &[&character.to_string(), &door.to_string()],
        );
        transport.capture()?;
        let projection = transport
            .batches()
            .last()
            .and_then(|batch| batch.projections.get(&door))
            .context("door probe projection")?;
        ensure!(
            projection.fields.get(&14).copied().unwrap_or(0) == state,
            "door probe did not transition to client state {state}"
        );
    }
    Ok(())
}

fn record_and_check(
    id: &str,
    path: &Path,
    golden: &wire_client::trace::GoldenTrace,
    now: u64,
    comparison: &compare::Comparison,
    violations: &[Violation],
) -> Result<()> {
    update_result(path, golden, Some(comparison), violations, now)?;
    let report = serde_json::to_string_pretty(comparison)?;
    println!("{id}: {report}");
    let baseline = ratchet::Baseline::read(&core_root().join("conformance/tier1-baseline.json"))?;
    baseline.check(&ratchet::ScenarioBaseline::from_comparison(
        id,
        &golden.scenario().behavior,
        harness_revision()?,
        &golden.trace().header.scenario_sha256,
        comparison,
    ))?;
    ensure!(
        violations.is_empty(),
        "tier 1 property violations: {violations:#?}"
    );
    Ok(())
}

fn run_multi_scenario(
    id: &str,
    setup: impl FnOnce(&Standalone, &wire_client::scenario::Scenario) -> Result<()>,
) -> Result<()> {
    let path = std::env::var_os("LYRACORE_CONFORMANCE_RESULT")
        .map(PathBuf::from)
        .unwrap_or_else(|| core_root().join(".lyracore/conformance/tier1.json"));
    std::fs::create_dir_all(path.parent().context("tier 1 result parent")?)?;
    let golden = catalogue::get(id)?;
    let recorded_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)?
        .as_millis()
        .try_into()?;
    update_result(&path, &golden, None, &[], recorded_at)?;
    let mut shard = Standalone::start(&format!("tier1-{}", id.replace('_', "-")));
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    setup(&shard, golden.scenario())?;
    let virtual_start = VIRTUAL_START_MICROS;
    shard.assert_call("debug_start_virtual_clock", &[&virtual_start.to_string()]);
    let mut transport = MultiOperatorTransport::prepare(&shard, golden.scenario())?;
    let now = virtual_start / 1_000;
    let observations = runner::run_multi_on(golden.scenario(), &mut transport)?;
    let comparison = compare::compare_multi_observations(
        &golden,
        &observations,
        &transport.character_guids(),
        now,
        &[],
    )?;
    let violations = properties::run(&transport.batches())?;
    record_and_check(id, &path, &golden, recorded_at, &comparison, &violations)
}

fn seed_visibility_world(
    shard: &Standalone,
    scenario: &wire_client::scenario::Scenario,
) -> Result<()> {
    let cell = super::taxi_route::STORMWIND_CELL;
    let mut race_class = None;
    for character in &scenario.characters {
        let FixtureVerb::Character {
            race, class, at, ..
        } = character
            .fixtures
            .first()
            .context("visibility Character fixture")?
        else {
            anyhow::bail!("first fixture must be a Character");
        };
        let race = wire_client::cli::parse_race(race)?.as_int();
        let class = wire_client::cli::parse_class(class)?.as_int();
        ensure!(
            race == 1 && class == 1,
            "visibility outfit needs a human warrior"
        );
        race_class = Some(lyracore_package_delta::packed_race_class_id(race, class));
        ensure!(
            at.map == 0
                && lyracore_shared::terrain::cell_index(at.x) == Some(cell.0)
                && lyracore_shared::terrain::cell_index(at.y) == Some(cell.1),
            "visibility Character is outside the Stormwind fixture cell"
        );
    }

    shard.assert_sql(super::taxi_route::stormwind_area_statement());
    shard.assert_call(
        "import_terrain_chunks",
        &[&super::taxi_route::stormwind_terrain_chunk()],
    );
    let base = shard
        .query_rows("SELECT * FROM game_item_template WHERE entry = 51")
        .into_iter()
        .next()
        .context("seeded item template 51")?;
    let columns = "entry,class,subclass,name,display_id,quality,inventory_type,item_level,required_level,max_durability,buy_price,sell_price,max_stack,damage_min,damage_max,delay_ms,stat_strength,stat_agility,stat_stamina,stat_intellect,stat_spirit,stat_crit,stat_hit,stat_armor,block_value,restores_power,spellid_1,spelltrigger_1,spellid_2,spelltrigger_2,container_slots,sheath,bonding,holy_res,fire_res,nature_res,frost_res,shadow_res,arcane_res,spellid_3,spelltrigger_3,spellid_4,spelltrigger_4,spellid_5,spelltrigger_5,required_skill,required_skill_rank,required_reputation_faction,required_reputation_rank,max_count,item_flags,page_text,start_quest,bag_family,buy_count,food_type,allowed_class,allowed_race,random_property";
    for (entry, name, subclass, inventory_type) in [
        (38, "Outfit shirt", 0, 4),
        (39, "Outfit pants", 0, 7),
        (40, "Outfit boots", 0, 8),
        (2362, "Outfit shield", 6, 14),
    ] {
        let mut template = base.clone();
        for (field, value) in [
            ("entry", entry.to_string()),
            ("name", name.into()),
            ("subclass", subclass.to_string()),
            ("inventory_type", inventory_type.to_string()),
            ("display_id", "0".into()),
            ("quality", "0".into()),
            ("item_level", "1".into()),
            ("max_durability", "0".into()),
            ("buy_price", "0".into()),
            ("sell_price", "0".into()),
        ] {
            template.insert(field.into(), value);
        }
        let values = columns
            .split(',')
            .map(|field| {
                let value = &template[field];
                if field == "name" {
                    format!("'{value}'")
                } else {
                    value.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        shard.assert_sql(&format!(
            "INSERT INTO game_item_template ({columns}) VALUES ({values})"
        ));
    }
    let race_class = race_class.context("visibility Character fixture")?;
    for (id, entry) in [25, 38, 39, 40, 2362].into_iter().enumerate() {
        shard.assert_sql(&format!(
            "INSERT INTO game_start_item (id,race_class,item_entry) VALUES ({},{race_class},{entry})",
            id + 1
        ));
    }
    Ok(())
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn connection_identity_matches_its_golden_trace() -> Result<()> {
    run_scenario("connection_identity", |_, _| Ok(()), None)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn taxi_single_hop_matches_its_golden_trace() -> Result<()> {
    run_scenario(
        "taxi_single_hop",
        |shard, scenario| {
            for statement in super::taxi_route::statements(scenario)? {
                shard.assert_sql(&statement);
            }
            shard.assert_call(
                "import_terrain_chunks",
                &[&super::taxi_route::terrain_chunks()],
            );
            Ok(())
        },
        None,
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn bank_slot_purchase_matches_its_golden_trace() -> Result<()> {
    run_scenario(
        "bank_slot_purchase",
        |shard, _| {
            shard.assert_sql(
            "INSERT INTO game_creature_template \
             (entry,name,subname,display_id,level,health,faction_template,npc_flags,unit_flags,\
              creature_type,creature_family,type_flags,rank,scale,base_attack_time_ms,money_min,\
              money_max,max_level,max_level_health,aggro_range,damage_min,damage_max,armor,\
              pickpocket_loot_id,skin_loot_id,trainer_type,trainer_class) \
             VALUES (5099,'Bank fixture','',1,1,100,35,256,0,7,0,0,0,1,2000,0,0,1,100,0,0,0,0,0,0,0,0)",
        );
            Ok(())
        },
        Some(5099),
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn two_character_visibility_matches_its_golden_trace() -> Result<()> {
    run_multi_scenario("two_character_visibility", seed_visibility_world)
}

#[test]
fn result_updates_preserve_other_scenarios_and_clear_failed_setup() -> Result<()> {
    let directory = std::env::temp_dir().join(format!(
        "conformance-result-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    std::fs::create_dir(&directory)?;
    let path = directory.join("tier1.json");
    let identity = catalogue::get("connection_identity")?;
    let taxi = catalogue::get("taxi_single_hop")?;
    let comparison = compare::compare(&identity, identity.trace(), &[])?;
    update_result(&path, &identity, Some(&comparison), &[], 1)?;
    update_result(&path, &taxi, Some(&comparison), &[], 2)?;
    update_result(&path, &taxi, None, &[], 3)?;
    let result: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    assert_eq!(result["scenarios"].as_array().unwrap().len(), 1);
    assert_eq!(result["scenarios"][0]["id"], "connection_identity");
    assert_eq!(result["recorded_at_unix_ms"], 3);
    std::fs::remove_dir_all(directory)?;
    Ok(())
}
