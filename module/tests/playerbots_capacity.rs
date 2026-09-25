//! Capacity measurements run only on a build host against an isolated Module.

mod support;

use std::io::Write;
use std::time::{Duration, Instant};
use support::Standalone;

fn capture_timings(node: &Standalone, captured: &mut Vec<String>) {
    let output = node.module_logs(100_000);
    node.assert_output_success(&output, "read capacity timing logs");
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    let path = support::log_dir()
        .join(node.shard_name())
        .with_extension("timings.jsonl");
    let next = captured.last().map_or(0, |last| {
        lines
            .iter()
            .rposition(|line| *line == last)
            .expect("capacity log capture lost its previous checkpoint")
            + 1
    });
    let mut journal = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    for line in &lines[next..] {
        writeln!(journal, "{line}").unwrap();
    }
    captured.extend(lines[next..].iter().map(|line| (*line).to_string()));
}

#[test]
#[ignore = "requires SpacetimeDB and the playerbots Package on a build host"]
fn playerbots_capacity_starting_areas_use_imported_starts_and_refuse_partial_batches() {
    let mut node = Standalone::start("playerbots-capacity-starts");
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    let areas: &[(&str, u32, u32, &[(u8, u8)])] = &[
        ("northshire", 0, 12, &[(1, 1), (1, 5), (1, 8)]),
        ("coldridge", 0, 1, &[(3, 1), (3, 5), (7, 8)]),
        ("deathknell", 0, 85, &[(5, 1), (5, 5), (5, 8)]),
        ("shadowglen", 1, 141, &[(4, 1), (4, 5)]),
        ("valley-of-trials", 1, 14, &[(2, 1), (8, 5), (8, 8)]),
        ("red-cloud-mesa", 1, 215, &[(6, 1)]),
    ];
    for (area_index, &(name, map, zone, classes)) in areas.iter().enumerate() {
        let x = 1200.0 + area_index as f32 * 200.0;
        let y = 1200.0;
        for &(race, class) in classes {
            let key = (u16::from(race) << 8) | u16::from(class);
            node.assert_sql(&format!(
                "DELETE FROM game_start_position WHERE race_class = {key}"
            ));
            node.assert_sql(&format!("INSERT INTO game_start_position (race_class,race,class,map_id,zone_id,x,y,z,orientation,display_id) VALUES ({key},{race},{class},{map},{zone},{x},{y},50,0,49)"));
        }
        let cx = lyracore_shared::terrain::cell_index(x).unwrap();
        let cy = lyracore_shared::terrain::cell_index(y).unwrap();
        let heights = vec!["50"; 145].join(":");
        for gx in cx - 1..=cx + 1 {
            for gy in cy - 1..=cy + 1 {
                let terrain = format!("{map},{gx},{gy},0,0,0,{zone},{heights}");
                let navigation = format!("{map},{gx},{gy},50,,");
                node.assert_call(
                    "import_terrain_chunks_append",
                    &[&serde_json::to_string(&terrain).unwrap()],
                );
                node.assert_call(
                    "import_nav_chunks_append",
                    &[&serde_json::to_string(&navigation).unwrap()],
                );
            }
        }
        if area_index == 0 {
            let characters_before = node.query_rows("SELECT guid, name FROM game_character");
            node.assert_sql("DELETE FROM game_start_position WHERE race_class = 264");
            let output = node.call(
                "playerbots_spawn_starting_area",
                &["\"northshire\"", "3", "{\"frozen\":[]}"],
            );
            assert!(!output.status.success());
            let refusal = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                refusal.contains("missing an imported race and class start"),
                "{refusal}"
            );
            assert_eq!(
                node.query_rows("SELECT guid, name FROM game_character"),
                characters_before,
                "refused batch changed Characters"
            );
            node.assert_sql(&format!("INSERT INTO game_start_position (race_class,race,class,map_id,zone_id,x,y,z,orientation,display_id) VALUES (264,1,8,0,12,{x},{y},50,0,49)"));
        }
        let before = node.query_rows("SELECT guid FROM game_character").len();
        node.assert_call(
            "playerbots_spawn_starting_area",
            &[
                &serde_json::to_string(name).unwrap(),
                "6",
                "{\"frozen\":[]}",
            ],
        );
        let rows = node
            .query_rows("SELECT guid, race, class, map_id, x, y, level, xp FROM game_character");
        assert_eq!(rows.len(), before + 6);
        let area_rows: Vec<_> = rows
            .iter()
            .filter(|row| (row["x"].parse::<f32>().unwrap() - x).abs() < 20.0)
            .collect();
        assert_eq!(area_rows.len(), 6);
        for row in area_rows {
            assert!(
                classes.contains(&(row["race"].parse().unwrap(), row["class"].parse().unwrap()))
            );
            assert_eq!(row["map_id"], map.to_string());
            assert_eq!(row["level"], "1");
            assert_eq!(row["xp"], "0");
        }
    }
    assert_eq!(
        node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")
            .len(),
        36
    );
    assert!(!node
        .call(
            "playerbots_spawn_starting_area",
            &["\"northshire\"", "51", "{\"cohort\":[]}"]
        )
        .status
        .success());
    assert_eq!(
        node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")
            .len(),
        36
    );
}

#[test]
#[ignore = "requires SpacetimeDB and the playerbots Package on a build host"]
fn playerbots_capacity_pending_search_preserves_foreign_motion() {
    let (node, guid) = pending_travel_request();
    node.assert_call("playerbots_load_foreign_motion", &[&guid, "30000"]);
    let query = format!("SELECT * FROM game_creature_spline WHERE guid = {guid}");
    let foreign = node.query_rows(&query);
    assert_eq!(foreign.len(), 1);
    assert!(support::poll_until(Duration::from_secs(8), || {
        node.assert_call("playerbots_fixture_runner_pass", &[]);
        let runner = node.query_rows(&format!("SELECT path_pending, foreground, last_outcome FROM pkg_playerbots_runner WHERE character_guid = {guid}"));
        runner[0]["path_pending"] == "false" && runner[0]["last_outcome"].contains("cancelled")
    }));
    assert_eq!(
        node.query_rows(&query),
        foreign,
        "pending search overwrote another motion owner"
    );
}

fn movement_setup_evidence(node: &Standalone) -> serde_json::Value {
    let rows = node.query_rows("SELECT * FROM pkg_playerbots_runner");
    let record = serde_json::json!({
        "runners": rows,
        "actions": node.query_rows("SELECT * FROM pkg_playerbots_action"),
        "splines": node.query_rows("SELECT * FROM game_creature_spline"),
        "bots": node.query_rows("SELECT * FROM pkg_playerbots_bot"),
    });
    let path = support::log_dir()
        .join(node.shard_name())
        .with_extension("setup.json");
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    serde_json::json!({"first_runners": rows.iter().take(3).collect::<Vec<_>>()})
}

fn pending_travel_request() -> (Standalone, String) {
    let node = prepare_travel(100);
    node.assert_sql("DELETE FROM game_creature_move_schedule");
    node.assert_call("playerbots_load_begin", &[]);
    node.assert_call("playerbots_fixture_runner_due", &[]);
    let mut guid = None;
    let prepared = support::poll_until(Duration::from_secs(12), || {
        node.assert_call("playerbots_fixture_runner_pass", &[]);
        guid = node
            .query_rows(
                "SELECT character_guid FROM pkg_playerbots_runner WHERE path_pending = true",
            )
            .into_iter()
            .map(|row| row["character_guid"].clone())
            .find(|guid| {
                node.query_rows(&format!(
                    "SELECT kind FROM pkg_playerbots_action WHERE character_guid = {guid}"
                ))
                .iter()
                .all(|row| !row["kind"].contains("move"))
            });
        guid.is_some()
    });
    assert!(prepared, "{}", movement_setup_evidence(&node));
    let guid = guid.unwrap();
    (node, guid)
}

#[test]
#[ignore = "requires SpacetimeDB and the playerbots Package on a build host"]
fn playerbots_capacity_new_request_can_replace_a_stop_spline() {
    let (node, guid) = pending_travel_request();
    node.assert_call("playerbots_load_foreign_motion", &[&guid, "0"]);
    let query =
        format!("SELECT start_micros, dur_ms FROM game_creature_spline WHERE guid = {guid}");
    let stopped = node.query_rows(&query);
    assert_eq!(stopped[0]["dur_ms"], "0");
    assert!(support::poll_until(Duration::from_secs(8), || {
        node.assert_call("playerbots_fixture_runner_pass", &[]);
        node.query_rows(&query).first().is_some_and(|row| {
            row["dur_ms"].parse::<u32>().unwrap() > 0
                && row["start_micros"] != stopped[0]["start_micros"]
        })
    }));
}

#[test]
#[ignore = "requires SpacetimeDB and the playerbots Package on a build host"]
fn playerbots_capacity_continuing_action_cancels_after_a_foreign_stop() {
    let node = prepare_travel(10);
    node.assert_sql("DELETE FROM game_creature_move_schedule");
    node.assert_call("playerbots_load_begin", &[]);
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    let prepared = support::poll_until(Duration::from_secs(12), || {
        node.assert_call("playerbots_fixture_runner_pass", &[]);
        node.query_rows(&format!(
            "SELECT dur_ms FROM game_creature_spline WHERE guid = {guid}"
        ))
        .first()
        .is_some_and(|row| row["dur_ms"].parse::<u32>().unwrap() > 0)
    });
    assert!(prepared, "{}", movement_setup_evidence(&node));
    node.assert_call("playerbots_load_foreign_motion", &[&guid, "0"]);
    let query = format!("SELECT * FROM game_creature_spline WHERE guid = {guid}");
    let stopped = node.query_rows(&query);
    assert!(support::poll_until(Duration::from_secs(8), || {
        node.assert_call("playerbots_fixture_runner_pass", &[]);
        let runner = node.query_rows(&format!("SELECT foreground, last_outcome FROM pkg_playerbots_runner WHERE character_guid = {guid}"));
        runner[0]["foreground"].contains("none") && runner[0]["last_outcome"].contains("cancelled")
    }));
    assert_eq!(node.query_rows(&query), stopped);
}

#[test]
#[ignore = "requires SpacetimeDB and the playerbots Package on a build host"]
fn playerbots_capacity_crowded_characters_do_not_hide_creature_targets() {
    let mut node = Standalone::start("playerbots-capacity-targets");
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_fixture_solo_target_claim", &["\"crowded\""]);
    assert_eq!(
        node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")
            .len(),
        100
    );
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package on a build host"]
fn playerbots_capacity_profile_travel() {
    let count: usize = std::env::var("PLAYERBOTS_CAPACITY_COUNT")
        .unwrap_or_else(|_| "25".to_string())
        .parse()
        .unwrap();
    assert!(matches!(count, 10 | 25 | 100 | 250 | 500 | 1_000));
    let worlds: usize = std::env::var("PLAYERBOTS_CAPACITY_WORLDS")
        .unwrap_or_else(|_| "1".to_string())
        .parse()
        .unwrap();
    assert!((1..=2).contains(&worlds));
    let nodes: Vec<_> = (0..worlds).map(|_| prepare_travel(count)).collect();
    std::thread::scope(|scope| {
        let measurements: Vec<_> = nodes
            .into_iter()
            .map(|node| scope.spawn(move || measure_travel(node, count)))
            .collect();
        for measurement in measurements {
            measurement.join().unwrap();
        }
    });
}

fn prepare_travel(count: usize) -> Standalone {
    let mut node = Standalone::start("playerbots-capacity-travel");
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    for key in ["decision_timing", "decision_profile"] {
        node.assert_call(
            "set_package_config",
            &[
                "\"playerbots\"",
                &serde_json::to_string(key).unwrap(),
                "\"true\"",
                "true",
            ],
        );
    }
    node.assert_call("playerbots_load_stage", &[&count.to_string(), "1"]);
    node
}

fn measure_travel(node: Standalone, count: usize) {
    node.assert_call("playerbots_load_begin", &[]);

    let started = Instant::now();
    let mut samples = Vec::new();
    let mut timings = Vec::new();
    capture_timings(&node, &mut timings);
    while started.elapsed() < Duration::from_secs(45) {
        std::thread::sleep(Duration::from_secs(1));
        samples.push(serde_json::json!({
            "elapsed_seconds": started.elapsed().as_secs_f64(),
            "scheduler": node.query_rows("SELECT * FROM pkg_playerbots_scheduler"),
            "bots": node.query_rows("SELECT character_guid, scheduler_lag_micros FROM pkg_playerbots_bot"),
            "runners": node.query_rows("SELECT character_guid, observed_micros, route_expansions, last_outcome, path_pending, movement_due_micros FROM pkg_playerbots_runner"),
        }));
        capture_timings(&node, &mut timings);
    }
    let positions = node.query_rows("SELECT guid, x, y FROM game_world_entity WHERE entry = 0");
    capture_timings(&node, &mut timings);
    let output = support::log_dir().join(node.shard_name());
    std::fs::write(output.with_extension("timings.jsonl"), timings.join("\n")).unwrap();
    std::fs::write(
        output.with_extension("samples.json"),
        serde_json::to_vec_pretty(&samples).unwrap(),
    )
    .unwrap();
    std::fs::write(
        output.with_extension("positions.json"),
        serde_json::to_vec_pretty(&positions).unwrap(),
    )
    .unwrap();
    eprintln!("capacity evidence: {}", output.display());
    assert_eq!(positions.len(), count);
    let stopped: Vec<_> = positions
        .iter()
        .filter(|row| row["x"].parse::<f32>().unwrap() <= 1250.0)
        .collect();
    assert!(
        stopped.is_empty(),
        "bots did not make travel progress: {stopped:?}"
    );
    let mut decisions = std::collections::BTreeMap::<u64, Vec<i64>>::new();
    let mut paths = std::collections::BTreeMap::<i64, (usize, u32)>::new();
    for line in &timings {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        if let Some(label) = row["message"]
            .as_str()
            .and_then(|message| message.strip_prefix("Timing span \"playerbots_decision "))
            .and_then(|message| message.split('"').next())
        {
            let fields: std::collections::HashMap<_, _> = label
                .split_whitespace()
                .filter_map(|part| part.split_once('='))
                .collect();
            decisions
                .entry(fields["guid"].parse().unwrap())
                .or_default()
                .push(fields["observed_micros"].parse().unwrap());
        }
        let Some(message) = row["message"]
            .as_str()
            .filter(|m| m.starts_with("playerbots_path "))
        else {
            continue;
        };
        let fields: std::collections::HashMap<_, _> = message
            .split_whitespace()
            .filter_map(|part| part.split_once('='))
            .collect();
        let pass = paths
            .entry(fields["observed_micros"].parse().unwrap())
            .or_default();
        pass.0 += 1;
        pass.1 += fields["expansions"].parse::<u32>().unwrap();
    }
    assert_eq!(
        decisions.len(),
        count,
        "missing decision timings for staged bots"
    );
    let mut intervals = Vec::new();
    for times in decisions.values_mut() {
        times.sort_unstable();
        assert!(times.len() >= 2, "bot did not receive recurring decisions");
        intervals.extend(times.windows(2).map(|pair| pair[1] - pair[0]));
    }
    intervals.sort_unstable();
    let p95 = intervals[(intervals.len() * 95).div_ceil(100) - 1];
    assert!(
        p95 <= 2_000_000,
        "decision interval p95 {p95} exceeds 2 seconds"
    );
    assert!(
        *intervals.last().unwrap() <= 5_000_000,
        "maximum decision interval exceeds 5 seconds"
    );
    assert!(!paths.is_empty(), "missing path planning evidence");
    for (at, (searches, expansions)) in paths {
        assert!(
            searches <= 32 && expansions <= 65_536,
            "path budget exceeded at {at}: {searches} searches, {expansions} expansions"
        );
    }
}

#[test]
#[ignore = "requires the preceding capacity Wasm and SpacetimeDB on a build host"]
fn playerbots_capacity_upgrade_preserves_pending_movement() {
    let previous = std::env::var_os("PLAYERBOTS_CAPACITY_PRECEDING_WASM")
        .expect("PLAYERBOTS_CAPACITY_PRECEDING_WASM must name the preceding Module");
    let mut node = Standalone::start("playerbots-capacity-upgrade");
    node.publish_module_bytes(&std::fs::read(previous).unwrap());
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("debug_set_nav_enabled", &["true"]);
    node.assert_call("playerbots_load_stage", &["10", "1"]);
    node.assert_call("playerbots_load_begin", &[]);
    let guid = node.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    assert!(support::poll_until(Duration::from_secs(12), || {
        !node
            .query_rows(&format!(
                "SELECT guid FROM game_creature_spline WHERE guid = {guid}"
            ))
            .is_empty()
    }));
    node.assert_sql("DELETE FROM game_creature_move_schedule");
    let query = format!("SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}");
    let before = node.query_rows(&query);
    assert_eq!(before.len(), 1);
    assert!(!before[0].contains_key("path_pending"));
    let characters = node.query_rows("SELECT guid, name FROM game_character");
    node.publish_module();
    let after = node.query_rows(&query);
    assert_eq!(after.len(), 1);
    for (field, value) in &before[0] {
        assert_eq!(&after[0][field], value, "migration changed {field}");
    }
    assert_eq!(after[0]["path_pending"], "false");
    assert_eq!(
        node.query_rows("SELECT guid, name FROM game_character"),
        characters
    );
    node.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
    assert!(
        node.query_rows(&format!(
            "SELECT guid FROM game_creature_spline WHERE guid = {guid}"
        ))
        .len()
            == 1
    );
    std::fs::write(
        support::log_dir().join(format!("{}-migration.json", node.shard_name())),
        serde_json::to_vec_pretty(&serde_json::json!({"before": before, "after": after})).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "requires an Operator-owned PLAYERBOTS_CAPACITY_NAV_SAMPLE on the build host"]
fn playerbots_capacity_profile_imported_navigation() {
    use lyracore_shared::nav::{find_leg_in_range_ex, LegOutcome, NavCellData};
    use std::collections::{HashMap, HashSet};

    let path = std::env::var_os("PLAYERBOTS_CAPACITY_NAV_SAMPLE")
        .expect("PLAYERBOTS_CAPACITY_NAV_SAMPLE must name the retained navigation input");
    let bytes = std::fs::read(path).unwrap();
    let input: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let queried: HashSet<u64> = input["queried_keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_u64().unwrap())
        .collect();
    let mut cells = HashMap::new();
    let blob = |value: &serde_json::Value| -> Vec<u8> {
        if let Some(text) = value.as_str() {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
                .collect()
        } else {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect()
        }
    };
    for row in input["nav"].as_array().unwrap() {
        cells.insert(
            (
                row["cell_x"].as_u64().unwrap() as u16,
                row["cell_y"].as_u64().unwrap() as u16,
            ),
            NavCellData {
                base_z: row["base_z"].as_f64().unwrap() as f32,
                walk: blob(&row["walk"]),
                obs: blob(&row["obs"]),
            },
        );
    }
    let mut measurements = Vec::new();
    for recorded in input["paths"].as_array().unwrap() {
        let movement = &recorded["movement"];
        let number = |row: &serde_json::Value, key: &str| row[key].as_f64().unwrap() as f32;
        let from = (number(movement, "from_x"), number(movement, "from_y"));
        let to = (
            number(&movement["destination"], "x"),
            number(&movement["destination"], "y"),
        );
        let stop_dist = number(recorded, "stop_dist");
        let map = movement["destination"]["map_id"].as_u64().unwrap() as u32;
        for budget in [1_024, 4_096, 16_384] {
            let mut reads = 0;
            let started = Instant::now();
            let result = find_leg_in_range_ex(
                &mut |x, y| {
                    reads += 1;
                    assert!(
                        queried.contains(&lyracore_shared::terrain::cell_key(map, x, y)),
                        "route left the captured navigation region at {x},{y}"
                    );
                    cells.get(&(x, y)).cloned()
                },
                from,
                to,
                stop_dist,
                budget,
            );
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            let (outcome, points) = match result.outcome {
                LegOutcome::Complete(points) => ("complete", points),
                LegOutcome::Partial(points) => ("partial", points),
                LegOutcome::Blocked => ("blocked", Vec::new()),
            };
            measurements.push(serde_json::json!({
                "guid": recorded["guid"], "budget": budget, "elapsed_ms": elapsed,
                "cell_reads": reads, "expansions": result.expansions,
                "from": from, "destination": to, "outcome": outcome, "points": points,
                "stop_dist": stop_dist,
            }));
        }
    }
    let result = serde_json::json!({
        "input_blake3": blake3::hash(&bytes).to_hex().to_string(),
        "measurements": measurements,
    });
    let output = std::env::var_os("PLAYERBOTS_CAPACITY_PROFILE_OUTPUT")
        .expect("PLAYERBOTS_CAPACITY_PROFILE_OUTPUT must name the result file");
    std::fs::write(output, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
    eprintln!("{result}");
}
