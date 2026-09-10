#![cfg(target_os = "linux")]

#[path = "support/playerbots_companion_acceptance.rs"]
mod companion;
#[path = "../../module/tests/support/mod.rs"]
mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use companion::{one, parse_u64, wait_until, CompanionTopology, WireControl};
use lyracore_shared::constants::player_flags::GHOST;
use serde_json::{json, Value};

fn assert_login_owner(topology: &CompanionTopology) {
    let guid = topology.party.leader;
    let characters = topology.query(
        &topology.source,
        &format!("SELECT account_id, owner_identity FROM game_character WHERE guid = {guid}"),
    );
    let character = one(&characters, "authenticated Character");
    let bodies = topology.query(
        &topology.source,
        &format!("SELECT owner_identity FROM game_world_entity WHERE guid = {guid}"),
    );
    let body = one(&bodies, "authenticated Character body");
    let accounts = topology.query(
        &topology.source,
        &format!(
            "SELECT identity FROM game_account WHERE id = {}",
            character["account_id"]
        ),
    );
    let account = one(&accounts, "authenticated Account");
    let evidence = json!({"character": character, "body": body, "account": account});
    std::fs::write(
        topology.evidence_dir.join("login-ownership.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .expect("failed to save authenticated ownership evidence");
    assert_eq!(
        body["owner_identity"],
        sats_field(sats_field(&account["identity"], "some"), "__identity__"),
        "the live body must use the authenticated Account identity: {evidence}"
    );
    assert_eq!(
        character["owner_identity"], body["owner_identity"],
        "addon replies must address the authenticated Character owner: {evidence}"
    );
}

fn position(topology: &CompanionTopology, database: &str, guid: u64) -> (f32, f32, f32) {
    let rows = topology.query(
        database,
        &format!("SELECT x, y, z FROM game_world_entity WHERE guid = {guid}"),
    );
    let row = one(&rows, "live Character body");
    (
        row["x"].parse().unwrap(),
        row["y"].parse().unwrap(),
        row["z"].parse().unwrap(),
    )
}

fn distance(left: (f32, f32, f32), right: (f32, f32, f32)) -> f32 {
    let dx = left.0 - right.0;
    let dy = left.1 - right.1;
    let dz = left.2 - right.2;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

fn parse_value_u64(value: &Value, field: &str) -> u64 {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing {field} in {value}"))
        .parse()
        .unwrap_or_else(|_| panic!("invalid {field} in {value}"))
}

fn parse_value_f32(value: &Value, field: &str) -> f32 {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("missing {field} in {value}"))
        .parse()
        .unwrap_or_else(|_| panic!("invalid {field} in {value}"))
}

fn sats_field<'a>(value: &'a str, field: &str) -> &'a str {
    let marker = format!("{field} = ");
    let start = value
        .match_indices(&marker)
        .find(|(index, _)| {
            value[..*index]
                .trim_end()
                .as_bytes()
                .last()
                .is_none_or(|byte| matches!(byte, b'(' | b','))
        })
        .unwrap_or_else(|| panic!("missing {field} in {value}"))
        .0;
    let tail = &value[start + marker.len()..];
    let mut depth = 0usize;
    for (index, byte) in tail.bytes().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' if depth > 0 => depth -= 1,
            b',' if depth == 0 => return &tail[..index],
            b')' if depth == 0 => return &tail[..index],
            _ => {}
        }
    }
    tail
}

fn sats_f32(value: &str, field: &str) -> f32 {
    sats_field(value, field)
        .parse()
        .unwrap_or_else(|_| panic!("invalid {field} in {value}"))
}

#[test]
fn sats_coordinates_do_not_match_the_order_name() {
    let order = "(stay = (map_id = 0, instance_id = 0, x = -11192.5, y = 1685.34, z = 25.7612))";
    assert_eq!(sats_f32(order, "x"), -11192.5);
    assert_eq!(sats_f32(order, "y"), 1685.34);
    assert_eq!(sats_f32(order, "z"), 25.7612);
    assert_eq!(sats_field(order, "map_id"), "0");
    assert_eq!(sats_field(order, "instance_id"), "0");
}

#[test]
fn sats_fields_preserve_nested_values_and_complete_names() {
    let objective =
        "(source_identity = 91, identity = 7, target = (entity = (guid = 42, map_id = 0)))";
    assert_eq!(sats_field(objective, "identity"), "7");
    assert_eq!(
        sats_field(objective, "target"),
        "(entity = (guid = 42, map_id = 0))"
    );
    let account = "(some = (__identity__ = 0x594c))";
    assert_eq!(
        sats_field(sats_field(account, "some"), "__identity__"),
        "0x594c"
    );
}

fn assert_stable_companion_objective(before: &Value, after: &Value, context: &str) {
    let before = before
        .as_str()
        .unwrap_or_else(|| panic!("{context}: prior objective is not text"));
    let after = after
        .as_str()
        .unwrap_or_else(|| panic!("{context}: current objective is not text"));
    assert_ne!(
        before, "(none = ())",
        "{context}: prior objective is absent"
    );
    assert_ne!(
        after, "(none = ())",
        "{context}: current objective is absent"
    );
    assert_eq!(sats_field(before, "kind"), "(companion = ())", "{context}");
    assert_eq!(
        sats_field(before, "deadline_micros"),
        i64::MAX.to_string(),
        "{context}: Companion objective acquired a finite deadline"
    );
    for field in [
        "identity",
        "kind",
        "deadline_micros",
        "started_micros",
        "catalog_revision",
    ] {
        assert_eq!(
            sats_field(before, field),
            sats_field(after, field),
            "{context}: retained objective changed {field}"
        );
    }
}

fn command(wire: &mut WireControl, payload: &str) -> Value {
    let evidence = wire.addon(payload, false);
    let reply = evidence["result"]["reply"]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("addon reply missing from {evidence}"));
    assert!(
        reply.starts_with("STC\tv1|playerbots.order.result|0|1/1|"),
        "unexpected addon command reply: {reply}"
    );
    evidence
}

fn wait_order(topology: &CompanionTopology, guid: u64, expected: &str) {
    wait_until("companion order did not settle", || {
        let database = topology.current_world(guid);
        topology
            .query(
                &database,
                &format!(
                    "SELECT * FROM pkg_playerbots_companion_order WHERE \
                     character_guid = {guid}"
                ),
            )
            .first()
            .is_some_and(|row| row["active"] == "true" && row["order"] == expected)
    });
}

fn wait_stay_order(topology: &CompanionTopology, guid: u64) -> (f32, f32, f32) {
    let mut settled = None;
    wait_until("Stay order did not settle", || {
        let database = topology.current_world(guid);
        let rows = topology.query(
            &database,
            &format!(
                "SELECT * FROM pkg_playerbots_companion_order WHERE character_guid = \
                 {guid}"
            ),
        );
        let Some(row) = rows.first() else {
            return false;
        };
        if row["active"] != "true" || !row["order"].starts_with("(stay = (") {
            return false;
        }
        settled = Some(row["order"].clone());
        true
    });
    let order = settled.expect("settled Stay order missing");
    let anchor = (
        sats_f32(&order, "x"),
        sats_f32(&order, "y"),
        sats_f32(&order, "z"),
    );
    let database = topology.current_world(guid);
    let bodies = topology.query(
        &database,
        &format!("SELECT map_id, instance_id FROM game_world_entity WHERE guid = {guid}"),
    );
    let body = one(&bodies, "Stay Character body");
    assert_eq!(sats_field(&order, "map_id"), body["map_id"]);
    assert_eq!(sats_field(&order, "instance_id"), body["instance_id"]);
    let mut settled_position = None;
    wait_until(
        "Stay Character did not settle at its committed anchor",
        || {
            let database = topology.current_world(guid);
            let current = position(topology, &database, guid);
            let stopped = topology
                .query(
                    &database,
                    &format!("SELECT guid FROM game_creature_spline WHERE guid = {guid}"),
                )
                .is_empty();
            if stopped && distance(current, anchor) <= 3.05 {
                settled_position = Some(current);
                true
            } else {
                false
            }
        },
    );
    settled_position.expect("settled Stay position missing")
}

fn send_follow_all(topology: &CompanionTopology, wire: &mut WireControl) -> Vec<Value> {
    topology
        .party
        .bots()
        .into_iter()
        .map(|guid| {
            let evidence = command(wire, &format!("follow|{guid}"));
            wait_order(
                topology,
                guid,
                &format!("(follow = (leader_guid = {}))", topology.party.leader),
            );
            evidence
        })
        .collect()
}

fn assert_no_unrequested_pull(topology: &CompanionTopology, phase: &str) {
    let database = topology.current_world(topology.party.warrior);
    let enemy_set: BTreeSet<_> = topology.party.enemies.into_iter().collect();
    for guid in topology.party.bots() {
        let bodies = topology.query(
            &database,
            &format!("SELECT target_guid FROM game_world_entity WHERE guid = {guid}"),
        );
        let body = one(&bodies, "companion body");
        let target = parse_u64(body, "target_guid");
        assert!(
            target == 0 || !enemy_set.contains(&target),
            "{phase}: bot {guid} acquired hostile {target} without Target or Assist consent"
        );
        assert!(
            topology
                .query(
                    &database,
                    &format!(
                        "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {guid}"
                    ),
                )
                .is_empty(),
            "{phase}: bot {guid} started unrequested melee"
        );
        for row in topology.query(
            &database,
            &format!("SELECT target_guid FROM game_pending_cast WHERE caster_guid = {guid}"),
        ) {
            let target = parse_u64(&row, "target_guid");
            assert!(
                !enemy_set.contains(&target),
                "{phase}: bot {guid} started an unrequested cast at {target}"
            );
        }
        for (table, owner) in [
            ("pkg_playerbots_companion_combat_receipt", "attacker_guid"),
            ("pkg_playerbots_companion_cast_receipt", "caster_guid"),
        ] {
            for row in topology.query(
                &database,
                &format!("SELECT target_guid FROM {table} WHERE {owner} = {guid}"),
            ) {
                let target = parse_u64(&row, "target_guid");
                assert!(
                    !enemy_set.contains(&target),
                    "{phase}: bot {guid} produced an unrequested receipt for {target}"
                );
            }
        }
    }
}

fn wait_for_party_progress(topology: &CompanionTopology, before: &BTreeMap<u64, (f32, f32, f32)>) {
    wait_until("all four companions did not make Follow progress", || {
        topology.party.bots().into_iter().all(|guid| {
            let now = position(topology, &topology.current_world(guid), guid);
            distance(now, before[&guid]) > 0.1
        })
    });
}

fn wait_for_party_live(topology: &CompanionTopology, database: &str) {
    wait_until("party did not rebuild its live destination state", || {
        topology.party.all().into_iter().all(|guid| {
            topology
                .query(
                    database,
                    &format!("SELECT guid FROM game_world_entity WHERE guid = {guid}"),
                )
                .len()
                == 1
        }) && topology.party.bots().into_iter().all(|guid| {
            topology
                .query(
                    database,
                    &format!(
                        "SELECT character_guid FROM pkg_playerbots_runner WHERE character_guid = \
                         {guid}"
                    ),
                )
                .len()
                == 1
        })
    });
}

fn wait_enemy_dead(topology: &CompanionTopology, enemy: u64) {
    let started = Instant::now();
    let path = topology
        .evidence_dir
        .join(format!("pull-{enemy}-observations.json"));
    let mut observations = Vec::new();
    let completed = support::poll_until(Duration::from_secs(60), || {
        let database = topology.current_world(topology.party.warrior);
        let rows = topology.query(
            &database,
            &format!(
                "SELECT guid, map_id, instance_id, x, y, z, health, max_health, dead, target_guid \
                 FROM game_world_entity WHERE guid = {enemy}"
            ),
        );
        let dead = rows
            .first()
            .is_some_and(|row| row["health"] == "0" && row["dead"] == "true");
        observations.push(json!({
            "elapsed_micros": started.elapsed().as_micros() as u64,
            "database": database,
            "enemy": rows,
        }));
        std::fs::write(&path, serde_json::to_vec_pretty(&observations).unwrap())
            .expect("failed to save pull progress");
        dead
    });
    if !completed {
        let database = topology.current_world(topology.party.warrior);
        topology.save(
            &format!("fixed-pull-{enemy}-timeout"),
            json!({
                "enemy_guid": enemy,
                "observations": observations,
                "threat": topology.query(
                    &database,
                    &format!("SELECT * FROM game_threat WHERE creature_guid = {enemy}"),
                ),
            }),
        );
    }
    assert!(
        completed,
        "declared pull did not finish through Core combat"
    );
}

fn owned_combat_handles(topology: &CompanionTopology, target: u64) -> Vec<Value> {
    let database = topology.current_world(topology.party.warrior);
    let mut rows = Vec::new();
    for guid in topology.party.bots() {
        rows.extend(
            topology
                .query(
                    &database,
                    &format!(
                        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid} AND \
                         target_guid = {target}"
                    ),
                )
                .into_iter()
                .map(|row| json!({"kind": "melee", "row": row})),
        );
        rows.extend(
            topology
                .query(
                    &database,
                    &format!(
                        "SELECT * FROM game_pending_cast WHERE caster_guid = {guid} AND \
                         target_guid = {target}"
                    ),
                )
                .into_iter()
                .map(|row| json!({"kind": "cast", "row": row})),
        );
    }
    rows.sort_by_key(Value::to_string);
    rows
}

fn exact_control_auras(topology: &CompanionTopology, target: u64) -> Vec<BTreeMap<String, String>> {
    topology.query(
        &topology.current_world(topology.party.warrior),
        &format!("SELECT * FROM game_aura WHERE target_guid = {target} AND spell_id = 50020"),
    )
}

fn target_receipts(topology: &CompanionTopology, target: u64) -> Value {
    let database = topology.current_world(topology.party.warrior);
    json!({
        "physical": topology.query(
            &database,
            &format!(
                "SELECT * FROM pkg_playerbots_companion_combat_receipt WHERE target_guid = {target}"
            ),
        ),
        "casts": topology.query(
            &database,
            &format!(
                "SELECT * FROM pkg_playerbots_companion_cast_receipt WHERE target_guid = {target}"
            ),
        ),
    })
}

fn exact_completed_heal(
    topology: &CompanionTopology,
    target: u64,
    not_before_micros: u64,
) -> Option<BTreeMap<String, String>> {
    topology
        .query(
            &topology.current_world(topology.party.priest),
            &format!(
                "SELECT * FROM pkg_playerbots_companion_cast_receipt WHERE caster_guid = {} AND \
                 target_guid = {target} AND spell_id = 2050",
                topology.party.priest
            ),
        )
        .into_iter()
        .find(|row| {
            parse_u64(row, "resolved_micros") >= not_before_micros
                && parse_u64(row, "healed") > 0
                && parse_u64(row, "target_health_after") > 35
                && parse_u64(row, "source_event_id") > 0
                && parse_u64(row, "scheduled_id") > 0
                && row["finished_micros"] == row["resolved_micros"]
        })
}

fn observed_casts(
    topology: &CompanionTopology,
    caster: u64,
    target: u64,
    spell: u32,
    not_before_micros: u64,
) -> Vec<BTreeMap<String, String>> {
    topology
        .query(
            &topology.current_world(caster),
            &format!(
                "SELECT * FROM pkg_playerbots_companion_cast_receipt WHERE caster_guid = {caster} \
                 AND target_guid = {target} AND spell_id = {spell}"
            ),
        )
        .into_iter()
        .filter(|row| {
            parse_u64(row, "source_event_id") > 0
                && parse_u64(row, "resolved_micros") >= not_before_micros
        })
        .collect()
}

fn pull_boundary(topology: &CompanionTopology, phase: &str, prior_target: Option<u64>) -> Value {
    let evidence = topology.save(phase, json!({"prior_target": prior_target}));
    let database = topology.current_world(topology.party.warrior);
    for guid in topology.party.bots() {
        let roles = topology.query(
            &database,
            &format!("SELECT class, role FROM pkg_playerbots_bot WHERE character_guid = {guid}"),
        );
        assert_eq!(roles.len(), 1, "{phase}: role row changed for {guid}");
        let (class, role) = if guid == topology.party.warrior {
            ("1", "0")
        } else if guid == topology.party.priest {
            ("5", "1")
        } else {
            ("8", "2")
        };
        assert_eq!(
            roles[0]["class"], class,
            "{phase}: class changed for {guid}"
        );
        assert_eq!(roles[0]["role"], role, "{phase}: role changed for {guid}");
        if let Some(target) = prior_target {
            assert!(
                topology
                    .query(
                        &database,
                        &format!(
                            "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid} AND \
                             target_guid = {target}"
                        ),
                    )
                    .is_empty(),
                "{phase}: obsolete melee target {target} survived for {guid}"
            );
            assert!(
                topology
                    .query(
                        &database,
                        &format!(
                            "SELECT * FROM game_pending_cast WHERE caster_guid = {guid} AND \
                             target_guid = {target}"
                        ),
                    )
                    .is_empty(),
                "{phase}: obsolete cast target {target} survived for {guid}"
            );
        }
    }
    evidence
}

#[test]
#[ignore = "requires SpacetimeDB, the playerbots Package, Gateway, and the pinned Headless Client"]
#[allow(
    clippy::too_many_lines,
    reason = "the evidence follows one ordered five-Character route"
)]
fn playerbots_acceptance_human_and_four_companions_complete_the_fixed_route() {
    let topology = CompanionTopology::stage("playerbots-companion-fixed-route");
    let fixed_before = topology.save(
        "fixed-before",
        json!({
            "lesser_heal_effect": topology.query(
                &topology.source,
                "SELECT * FROM game_spell_effect WHERE spell_id = 2050 AND kind = 2",
            ),
        }),
    );
    assert_eq!(
        fixed_before["extra"]["lesser_heal_effect"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "Lesser Heal does not have one declared Core healing effect"
    );
    let mut gateway = topology.gateway(false, "fixed-route");
    let mut wire = topology.wire("fixed-route");
    assert_login_owner(&topology);
    topology.begin();

    assert_no_unrequested_pull(&topology, "before-follow");
    let follow_commands = send_follow_all(&topology, &mut wire);
    let follow_positions: BTreeMap<_, _> = topology
        .party
        .bots()
        .into_iter()
        .map(|guid| (guid, position(&topology, &topology.source, guid)))
        .collect();
    let leader_start = position(&topology, &topology.source, topology.party.leader);
    let leader_ahead = (leader_start.0 + 18.0, leader_start.1, leader_start.2);
    let follow_move = wire.move_to(leader_start, leader_ahead);
    wait_for_party_progress(&topology, &follow_positions);
    topology.save(
        "fixed-follow-progress",
        json!({"commands": follow_commands, "leader_move": follow_move}),
    );

    let stay = command(&mut wire, &format!("stay|{}", topology.party.mage_two));
    let held_before = wait_stay_order(&topology, topology.party.mage_two);
    let held_leader = position(&topology, &topology.source, topology.party.leader);
    let next_leader = (held_leader.0 + 18.0, held_leader.1, held_leader.2);
    let stay_move = wire.move_to(held_leader, next_leader);
    std::thread::sleep(Duration::from_secs(3));
    let held_after = position(&topology, &topology.source, topology.party.mage_two);
    assert!(
        distance(held_before, held_after) < 0.1,
        "Stay Mage moved with the leader"
    );
    assert_no_unrequested_pull(&topology, "stay-held");
    let resume = command(&mut wire, &format!("follow|{}", topology.party.mage_two));
    wait_until("Stay Mage did not resume Follow", || {
        distance(
            position(&topology, &topology.source, topology.party.mage_two),
            held_after,
        ) > 0.1
    });
    topology.save(
        "fixed-stay-and-resume",
        json!({"stay": stay, "move": stay_move, "resume": resume}),
    );

    let first = topology.party.enemies[0];
    let mut first_commands = Vec::new();
    for guid in [
        topology.party.warrior,
        topology.party.mage_one,
        topology.party.mage_two,
    ] {
        first_commands.push(command(&mut wire, &format!("target|{guid}|{first}")));
    }
    wait_enemy_dead(&topology, first);
    let first_boundary = pull_boundary(&topology, "fixed-first-pull", Some(first));

    let priest_before_repair = position(&topology, &topology.source, topology.party.priest);
    let wound_attempts = topology.apply_fault_when_due(0);
    let wound = one(
        &topology.query(
            &topology.source,
            "SELECT applied_micros FROM pkg_playerbots_companion_fault WHERE id = 0",
        ),
        "applied wound fault",
    )
    .clone();
    let wound_applied_micros = parse_u64(&wound, "applied_micros");
    assert_ne!(wound_applied_micros, 0, "wound fault was not applied");
    wait_until("Priest did not heal the declared wounds", || {
        let leader = topology.query(
            &topology.source,
            &format!(
                "SELECT health FROM game_world_entity WHERE guid = {}",
                topology.party.leader
            ),
        );
        let mage = topology.query(
            &topology.source,
            &format!(
                "SELECT health FROM game_world_entity WHERE guid = {}",
                topology.party.mage_one
            ),
        );
        let fortitude = topology.query(
            &topology.source,
            &format!(
                "SELECT spell_id FROM game_aura WHERE target_guid = {} AND spell_id = 1243",
                topology.party.mage_one
            ),
        );
        one(&leader, "wounded leader")["health"]
            .parse::<u32>()
            .unwrap()
            > 35
            && one(&mage, "wounded Mage")["health"].parse::<u32>().unwrap() > 35
            && fortitude.len() == 1
            && [topology.party.leader, topology.party.mage_one]
                .into_iter()
                .all(|target| {
                    exact_completed_heal(&topology, target, wound_applied_micros).is_some()
                })
            && observed_casts(
                &topology,
                topology.party.priest,
                topology.party.mage_one,
                1243,
                wound_applied_micros,
            )
            .len()
                == 1
            && distance(
                priest_before_repair,
                position(&topology, &topology.source, topology.party.priest),
            ) > 0.1
    });
    let repaired = topology.save(
        "fixed-repair-observed",
        json!({"wound_attempts": wound_attempts}),
    );
    let mage_guid = topology.party.mage_one.to_string();
    let retained_fortitude: Vec<_> = repaired["source"]["auras"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row["target_guid"].as_str() == Some(mage_guid.as_str())
                && row["spell_id"].as_str() == Some("1243")
        })
        .cloned()
        .collect();
    let retained_cast = repaired["source"]["cast_receipts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row["target_guid"].as_str() == Some(topology.party.mage_one.to_string().as_str())
                && row["spell_id"].as_str() == Some("1243")
                && row["source_event_id"].as_str().is_some_and(|id| id != "0")
                && row["scheduled_id"] == "0"
                && parse_value_u64(row, "resolved_micros") >= wound_applied_micros
        })
        .cloned()
        .expect("Mage Fortitude receipt missing from saved evidence");
    for target in [topology.party.leader, topology.party.mage_one] {
        let heal = exact_completed_heal(&topology, target, wound_applied_micros)
            .unwrap_or_else(|| panic!("exact Lesser Heal completion missing for {target}"));
        assert!(parse_u64(&heal, "healed") > 0);
        assert!(parse_u64(&heal, "target_health_after") > 35);
        assert_ne!(parse_u64(&heal, "scheduled_id"), 0);
        assert_eq!(heal["finished_micros"], heal["resolved_micros"]);
    }
    std::thread::sleep(Duration::from_secs(2));
    let repair_stable = topology.save("fixed-repair-stable", json!({}));
    assert_eq!(
        observed_casts(
            &topology,
            topology.party.priest,
            topology.party.mage_one,
            1243,
            wound_applied_micros,
        )
        .len(),
        1,
        "completed Fortitude restarted"
    );
    let later_fortitude: Vec<_> = repair_stable["source"]["auras"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| {
            row["target_guid"].as_str() == Some(mage_guid.as_str())
                && row["spell_id"].as_str() == Some("1243")
        })
        .cloned()
        .collect();
    assert_eq!(
        retained_fortitude, later_fortitude,
        "completed Fortitude restarted instead of remaining durable"
    );
    let later_cast = repair_stable["source"]["cast_receipts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row["target_guid"].as_str() == Some(mage_guid.as_str())
                && row["spell_id"].as_str() == Some("1243")
                && parse_value_u64(row, "resolved_micros") >= wound_applied_micros
        })
        .cloned()
        .expect("stable Mage repair receipt missing");
    assert_eq!(retained_cast, later_cast, "completed Fortitude restarted");
    assert!(
        repair_stable["source"]["casts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| {
                row["caster_guid"].as_str() != Some(topology.party.priest.to_string().as_str())
                    || row["target_guid"].as_str() != Some(mage_guid.as_str())
            }),
        "completed Mage repair retained a second pending cast"
    );
    let second = topology.party.enemies[1];
    let priest_target = command(
        &mut wire,
        &format!("target|{}|{second}", topology.party.priest),
    );
    let assist = command(
        &mut wire,
        &format!(
            "assist|{}|{}",
            topology.party.mage_one, topology.party.priest
        ),
    );
    wait_order(
        &topology,
        topology.party.mage_one,
        &format!("(assist = (member_guid = {}))", topology.party.priest),
    );
    wait_until("Assist Mage did not take the named Priest's target", || {
        topology
            .query(
                &topology.source,
                &format!(
                    "SELECT companion_fight_target_guid FROM pkg_playerbots_runner WHERE \
                     character_guid = {}",
                    topology.party.mage_one
                ),
            )
            .first()
            .is_some_and(|row| row["companion_fight_target_guid"] == format!("(some = {second})"))
    });
    for guid in [topology.party.warrior, topology.party.mage_two] {
        command(&mut wire, &format!("target|{guid}|{second}"));
    }
    wait_until(
        "second pull produced no owned combat handle before control",
        || !owned_combat_handles(&topology, second).is_empty(),
    );
    let before_control = topology.save(
        "fixed-before-control",
        json!({"handles": owned_combat_handles(&topology, second)}),
    );
    let control_attempts = topology.apply_fault_when_due(1);
    wait_until(
        "ordinary decisions did not cancel work against the controlled target",
        || {
            exact_control_auras(&topology, second).len() == 1
                && owned_combat_handles(&topology, second).is_empty()
        },
    );
    let controlled_receipts = target_receipts(&topology, second);
    let controlled = topology.save(
        "fixed-controlled-target-held",
        json!({
            "attempts": control_attempts,
            "before_handles": before_control["extra"]["handles"],
            "control_aura": exact_control_auras(&topology, second),
            "receipts": controlled_receipts,
        }),
    );
    std::thread::sleep(Duration::from_secs(2));
    let control_stable = topology.save(
        "fixed-controlled-target-stable",
        json!({
            "control_aura": exact_control_auras(&topology, second),
            "handles": owned_combat_handles(&topology, second),
            "receipts": target_receipts(&topology, second),
        }),
    );
    assert_eq!(
        control_stable["extra"]["control_aura"], controlled["extra"]["control_aura"],
        "control aura did not remain durable through the cancellation window"
    );
    assert_eq!(control_stable["extra"]["handles"], json!([]));
    assert_eq!(
        control_stable["extra"]["receipts"], controlled["extra"]["receipts"],
        "a companion damaged the controlled target"
    );
    let clear_attempts = topology.apply_fault_when_due(3);
    wait_enemy_dead(&topology, second);
    let second_boundary = pull_boundary(&topology, "fixed-second-pull", Some(second));
    let combat_receipts = topology.query(
        &topology.source,
        "SELECT attacker_guid, target_guid, tank_is_top_threat FROM \
         pkg_playerbots_companion_combat_receipt",
    );
    let cast_receipts = topology.query(
        &topology.source,
        "SELECT caster_guid, target_guid, spell_id, damage FROM \
         pkg_playerbots_companion_cast_receipt",
    );
    for mage in [topology.party.mage_one, topology.party.mage_two] {
        assert!(
            cast_receipts.iter().any(|row| {
                parse_u64(row, "caster_guid") == mage
                    && [first, second].contains(&parse_u64(row, "target_guid"))
                    && parse_u64(row, "damage") > 0
            }),
            "Mage {mage} dealt no recorded damage in the repeated pulls"
        );
    }
    assert!(
        cast_receipts.iter().any(|row| {
            parse_u64(row, "caster_guid") == topology.party.mage_one
                && parse_u64(row, "target_guid") == second
                && parse_u64(row, "damage") > 0
        }),
        "Assist Mage never acted on the named Priest's target"
    );
    assert!(
        combat_receipts.iter().any(|row| {
            parse_u64(row, "attacker_guid") == topology.party.warrior
                && parse_u64(row, "target_guid") == second
                && row["tank_is_top_threat"] == "true"
        }),
        "Warrior was not the second creature's authoritative top threat"
    );
    let regroup_follow = command(&mut wire, &format!("follow|{}", topology.party.mage_two));
    wait_order(
        &topology,
        topology.party.mage_two,
        &format!("(follow = (leader_guid = {}))", topology.party.leader),
    );
    let before_death = topology.save(
        "fixed-before-death",
        json!({"regroup_follow": regroup_follow}),
    );
    let death_attempts = topology.apply_fault_when_due(2);
    let regrouping = topology.save(
        "fixed-death-regrouping",
        json!({"attempts": death_attempts}),
    );
    let death = fault_in(&regrouping, "source", 2);
    assert_eq!(death["death_dead"], "true");
    assert_eq!(death["death_was_ghost"], "false");
    assert_eq!(death["death_health"], "0");
    assert_mage_recovery_identity(&topology, &before_death, &regrouping);
    for guid in [
        topology.party.warrior,
        topology.party.priest,
        topology.party.mage_one,
    ] {
        assert!(
            topology
                .query(
                    &topology.source,
                    &format!(
                        "SELECT * FROM game_melee_attack WHERE attacker_guid = {guid} AND \
                         target_guid = {}",
                        topology.party.enemies[2]
                    ),
                )
                .is_empty(),
            "party pulled while the dead Mage was regrouping"
        );
    }
    wait_until("dead Mage never became a released ghost", || {
        topology
            .query(
                &topology.source,
                "SELECT ghost_observed_micros FROM pkg_playerbots_companion_fault WHERE id = 2",
            )
            .first()
            .is_some_and(|row| parse_u64(row, "ghost_observed_micros") > 0)
    });
    let ghost = topology.save("fixed-released-ghost", json!({}));
    let ghost_fault = fault_in(&ghost, "source", 2);
    assert!(parse_value_u64(ghost_fault, "ghost_observed_micros") > 0);
    assert_mage_recovery_identity(&topology, &before_death, &ghost);
    let ghost_position = (
        parse_value_f32(ghost_fault, "ghost_x"),
        parse_value_f32(ghost_fault, "ghost_y"),
        parse_value_f32(ghost_fault, "ghost_z"),
    );
    wait_until(
        "resurrected Mage did not make physical Follow progress",
        || {
            let fault = topology.query(
                &topology.source,
                "SELECT alive_observed_micros FROM pkg_playerbots_companion_fault WHERE id = 2",
            );
            if fault
                .first()
                .is_none_or(|row| parse_u64(row, "alive_observed_micros") == 0)
            {
                return false;
            }
            let leader = position(&topology, &topology.source, topology.party.leader);
            let mage = position(&topology, &topology.source, topology.party.mage_two);
            distance(mage, leader) + 0.1 < distance(ghost_position, leader)
        },
    );
    let resurrected = topology.save("fixed-resurrected", json!({}));
    let alive = fault_in(&resurrected, "source", 2);
    assert!(
        parse_value_u64(alive, "alive_observed_micros")
            > parse_value_u64(alive, "ghost_observed_micros")
    );
    let alive_body = value_row_by_guid(
        &resurrected,
        "source",
        "bodies",
        "guid",
        topology.party.mage_two,
    );
    assert_eq!(alive_body["dead"], "false");
    assert!(parse_value_u64(alive_body, "health") > 0);
    assert_eq!(
        parse_value_u64(alive_body, "player_flags") & u64::from(GHOST),
        0
    );
    assert_mage_recovery_identity(&topology, &before_death, &resurrected);
    let third = topology.party.enemies[2];
    assert!(owned_combat_handles(&topology, third).is_empty());
    assert_eq!(target_receipts(&topology, third)["physical"], json!([]));
    assert_eq!(target_receipts(&topology, third)["casts"], json!([]));
    let priest_third = command(
        &mut wire,
        &format!("target|{}|{third}", topology.party.priest),
    );
    wait_until("Assist Mage retained the Priest's obsolete target", || {
        topology
            .query(
                &topology.source,
                &format!(
                    "SELECT companion_fight_target_guid FROM pkg_playerbots_runner WHERE \
                     character_guid = {}",
                    topology.party.mage_one
                ),
            )
            .first()
            .is_some_and(|row| row["companion_fight_target_guid"] == format!("(some = {third})"))
    });
    let mut third_commands = vec![priest_third];
    for guid in [topology.party.warrior, topology.party.mage_two] {
        third_commands.push(command(&mut wire, &format!("target|{guid}|{third}")));
    }
    wait_enemy_dead(&topology, third);
    wait_until("third pull cast observations did not settle", || {
        observed_casts(&topology, topology.party.mage_one, third, 133, 0)
            .iter()
            .any(|row| parse_u64(row, "damage") > 0)
    });
    let third_boundary = pull_boundary(&topology, "fixed-third-pull", Some(third));
    assert!(
        third_boundary["source"]["cast_receipts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| {
                row["caster_guid"].as_str() == Some(topology.party.mage_one.to_string().as_str())
                    && row["target_guid"].as_str() == Some(third.to_string().as_str())
                    && row["damage"].as_str().is_some_and(|value| value != "0")
            }),
        "Assist Mage never acted after the Priest changed target"
    );
    let retained_quest = third_boundary["source"]["quests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row["character_guid"].as_str() == Some(topology.party.warrior.to_string().as_str())
                && row["quest_entry"].as_str() == Some("50911")
        })
        .expect("retained Quest vanished during the fixed route");
    assert_eq!(retained_quest["counts"], "1");
    assert_eq!(retained_quest["rewarded"], "false");
    assert_eq!(retained_quest["failed"], "false");
    assert_fixed_orders(&topology, &third_boundary, "source", third);
    let crossing_follow = send_follow_all(&topology, &mut wire);
    topology.save(
        "fixed-recovery",
        json!({
            "first_commands": first_commands,
            "first_boundary": first_boundary["phase"],
            "repair_phase": repaired["phase"],
            "repair_stable_phase": repair_stable["phase"],
            "priest_target": priest_target,
            "assist": assist,
            "controlled": controlled["phase"],
            "clear_attempts": clear_attempts,
            "second_boundary": second_boundary["phase"],
            "death_phase": regrouping["phase"],
            "ghost_phase": ghost["phase"],
            "resurrection_phase": resurrected["phase"],
            "third_commands": third_commands,
            "third_boundary": third_boundary["phase"],
            "crossing_follow": crossing_follow,
        }),
    );

    let outside = position(&topology, &topology.source, topology.party.leader);
    let to_entry = wire.move_to(outside, companion::ENTRY_SOURCE);
    let entry = wire.area_trigger(companion::ENTRY_TRIGGER);
    topology.wait_for_map(companion::DUNGEON_MAP);
    wait_for_party_live(&topology, &topology.destination);
    let entered = topology.save(
        "fixed-instance-entered",
        json!({"move": to_entry, "entry": entry}),
    );
    assert_old_world_frozen(&entered, "source", "destination");
    assert_party_landed(&topology, &entered, "destination");
    assert_follow_orders(&topology, &entered, "destination");

    let inside = position(&topology, &topology.destination, topology.party.leader);
    let to_exit = wire.move_to(inside, companion::EXIT_SOURCE);
    let exit = wire.area_trigger(companion::EXIT_TRIGGER);
    topology.wait_for_map(0);
    wait_for_party_live(&topology, &topology.source);
    let completed = topology.save(
        "fixed-instance-exited",
        json!({"entry_phase": entered["phase"], "move": to_exit, "exit": exit}),
    );
    assert_old_world_frozen(&completed, "destination", "source");
    assert_party_whole(&topology, &completed);
    assert_inventory_retained(
        &topology,
        &fixed_before,
        &completed,
        Some(topology.party.mage_two),
    );
    assert_follow_orders(&topology, &completed, "source");
    wire.stop();
    gateway.stop();
}

#[test]
#[ignore = "requires SpacetimeDB, the playerbots Package, Gateway, and the pinned Headless Client"]
#[allow(
    clippy::too_many_lines,
    reason = "the evidence follows one ordered restart and Transfer sequence"
)]
fn playerbots_acceptance_restart_transfer_and_lost_ack_apply_once() {
    let mut topology = CompanionTopology::stage("playerbots-companion-restart-transfer");
    let before = topology.save("composition-before", json!({}));
    let mut gateway = topology.gateway(true, "command-abort");
    let mut wire = topology.wire("command-abort");
    assert_login_owner(&topology);
    topology.begin();
    let sent_a = wire.addon(&format!("follow|{}", topology.party.warrior), true);
    let abort = gateway.wait_for_abort();
    wire.terminate();
    let crash_boundary = topology.save(
        "composition-command-applied-before-finalize",
        json!({"sent": sent_a, "abort": abort}),
    );
    assert_gateway_abort(&crash_boundary["extra"]["abort"]);
    let intent_a = latest_intent(&topology, &topology.source);
    assert_eq!(
        intent_a["pending"], "true",
        "intent A finalized before the crash"
    );
    let intent_a_id = parse_u64(&intent_a, "id");
    let applied_receipt = topology.query(
        &topology.source,
        &format!("SELECT * FROM game_party_command_receipt WHERE intent_id = {intent_a_id}"),
    );
    assert_eq!(
        applied_receipt.len(),
        1,
        "intent A did not commit exactly one target receipt"
    );
    assert_eq!(applied_receipt[0]["outcome"], "(applied = ())");
    let mut gateway = topology.gateway(false, "command-recovery");
    wait_until("intent A did not finalize after Gateway restart", || {
        topology
            .query(
                &topology.source,
                &format!("SELECT pending FROM game_party_command_intent WHERE id = {intent_a_id}"),
            )
            .first()
            .is_some_and(|row| row["pending"] == "false")
    });
    let recovered_intent = one(
        &topology.query(
            &topology.source,
            &format!("SELECT * FROM game_party_command_intent WHERE id = {intent_a_id}"),
        ),
        "recovered intent A",
    )
    .clone();
    assert_eq!(recovered_intent["state"], "(finished = (applied = ()))");
    let recovered_receipts = topology.query(
        &topology.source,
        &format!("SELECT * FROM game_party_command_receipt WHERE intent_id = {intent_a_id}"),
    );
    assert_eq!(
        recovered_receipts.len(),
        1,
        "Gateway recovery applied intent A more than once"
    );
    assert_eq!(recovered_receipts[0]["outcome"], "(applied = ())");
    let applied_payload = format!("{intent_a_id}|Applied");
    let applied_results = topology.query(
        &topology.source,
        &format!(
            "SELECT * FROM game_addon_message WHERE cmd = 'playerbots.order.result' AND payload = \
             '{applied_payload}'"
        ),
    );
    topology.save(
        "composition-command-recovered",
        json!({
            "intent_a": recovered_intent,
            "receipt_count": recovered_receipts.len(),
            "result_count": applied_results.len(),
        }),
    );
    assert_eq!(
        applied_results.len(),
        1,
        "Gateway recovery did not publish exactly one terminal result for intent A"
    );
    let mut wire = topology.wire("composition-route");
    let remaining_follow = topology
        .party
        .bots()
        .into_iter()
        .filter(|guid| *guid != topology.party.warrior)
        .map(|guid| command(&mut wire, &format!("follow|{guid}")))
        .collect::<Vec<_>>();

    let leader = position(&topology, &topology.source, topology.party.leader);
    wire.move_to(leader, companion::ENTRY_SOURCE);
    wire.area_trigger(companion::ENTRY_TRIGGER);
    topology.wait_for_map(companion::DUNGEON_MAP);
    wait_for_party_live(&topology, &topology.destination);
    let dungeon_leader = position(&topology, &topology.destination, topology.party.leader);
    let entered = topology.save(
        "composition-entered",
        json!({"remaining_follow": remaining_follow}),
    );
    assert_old_world_frozen(&entered, "source", "destination");
    assert_party_landed(&topology, &entered, "destination");
    assert_follow_orders(&topology, &entered, "destination");
    let retained_move = wire.move_to(
        dungeon_leader,
        (dungeon_leader.0 + 18.0, dungeon_leader.1, dungeon_leader.2),
    );
    wait_until(
        "no retained Follow foreground existed before Module restart",
        || {
            topology
                .query(
                    &topology.destination,
                    &format!(
                        "SELECT foreground FROM pkg_playerbots_runner WHERE character_guid = {}",
                        topology.party.warrior
                    ),
                )
                .first()
                .is_some_and(|row| row["foreground"] != "(none = ())")
                && topology
                    .query(
                        &topology.destination,
                        &format!(
                            "SELECT guid FROM game_creature_spline WHERE guid = {}",
                            topology.party.warrior
                        ),
                    )
                    .len()
                    == 1
        },
    );
    let restart_point = topology.save_restart_point(
        "composition-before-module-restart",
        topology.party.warrior,
        retained_move,
    );
    wire.terminate();
    let process_ids = topology.restart_module_process();
    gateway.stop();
    let restarted = topology.save(
        "composition-module-restarted",
        json!({
            "entered_phase": entered["phase"],
            "restart_point": restart_point["phase"],
            "process_ids": process_ids,
        }),
    );
    assert_restart_retained(
        &restart_point,
        &restarted,
        topology.party.warrior,
        topology.party.leader,
    );
    let mut gateway = topology.gateway(false, "after-module-restart");
    let mut wire = topology.wire("after-module-restart");

    let warrior_before_resume = position(&topology, &topology.destination, topology.party.warrior);
    let leader_before_resume = position(&topology, &topology.destination, topology.party.leader);
    let leader_resume_destination = (
        leader_before_resume.0 + 18.0,
        leader_before_resume.1,
        leader_before_resume.2,
    );
    let resumed_leader_move = wire.move_to(leader_before_resume, leader_resume_destination);
    wait_until(
        "Warrior did not physically resume Follow after Module restart",
        || {
            let warrior = position(&topology, &topology.destination, topology.party.warrior);
            distance(warrior, warrior_before_resume) > 0.1
                && distance(warrior, leader_resume_destination)
                    < distance(warrior_before_resume, leader_resume_destination)
        },
    );
    let resumed = topology.save(
        "composition-follow-resumed",
        json!({"leader_move": resumed_leader_move}),
    );
    assert_order_in_evidence(
        &resumed,
        "destination",
        topology.party.warrior,
        &format!("(follow = (leader_guid = {}))", topology.party.leader),
    );

    let leader = position(&topology, &topology.destination, topology.party.leader);
    wire.move_to(leader, companion::EXIT_SOURCE);
    wire.area_trigger(companion::EXIT_TRIGGER);
    topology.wait_for_map(0);
    wait_for_party_live(&topology, &topology.source);
    let exited = topology.save(
        "composition-exited",
        json!({
            "restart_phase": restarted["phase"],
            "resumed_phase": resumed["phase"],
        }),
    );
    assert_old_world_frozen(&exited, "destination", "source");

    let sent_b = command(&mut wire, &format!("follow|{}", topology.party.warrior));
    let intent_b = latest_intent(&topology, &topology.source);
    let intent_b_id = parse_u64(&intent_b, "id");
    assert_ne!(
        intent_a_id, intent_b_id,
        "the second client command reused intent A"
    );
    let result = topology.save(
        "composition-distinct-unchanged-command",
        json!({
            "exit_phase": exited["phase"],
            "sent_b": sent_b,
            "intent_a": intent_a_id,
            "intent_b": intent_b,
        }),
    );
    let unchanged_payload = format!("{intent_b_id}|Unchanged");
    assert!(
        result["source"]["addon_results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["payload"].as_str() == Some(unchanged_payload.as_str())),
        "intent B did not report Unchanged: {result}"
    );
    assert_distinct_unchanged_command(
        &topology,
        &exited,
        &result,
        topology.party.warrior,
        intent_b_id,
    );
    assert_inventory_retained(&topology, &before, &result, None);
    assert_quest_unchanged(&before, &result, topology.party.warrior, 50_911);
    assert_party_whole(&topology, &result);
    for guid in topology.party.bots() {
        assert_order_in_evidence(
            &result,
            "source",
            guid,
            &format!("(follow = (leader_guid = {}))", topology.party.leader),
        );
    }
    wire.stop();
    gateway.stop();
}

fn latest_intent(topology: &CompanionTopology, database: &str) -> BTreeMap<String, String> {
    let mut intents = topology.query(database, "SELECT * FROM game_party_command_intent");
    intents.sort_by_key(|row| parse_u64(row, "id"));
    intents.pop().expect("party command intent missing")
}

fn assert_gateway_abort(abort: &Value) {
    assert!(
        abort["success"].as_bool() == Some(false),
        "injected Gateway exit succeeded: {abort}"
    );
    assert!(
        abort["log"]
            .as_str()
            .is_some_and(|log| log.contains("target apply committed")),
        "Gateway did not reach the requested command boundary: {abort}"
    );
    assert_eq!(
        abort["signal"].as_u64(),
        Some(6),
        "Gateway did not stop through the requested SIGABRT: {abort}"
    );
}

fn assert_order_in_evidence(evidence: &Value, world: &str, guid: u64, expected: &str) {
    let order = value_row_by_guid(evidence, world, "orders", "character_guid", guid);
    assert_eq!(order["active"], "true", "order inactive for {guid}");
    assert_eq!(order["order"], expected, "order changed for {guid}");
}

fn assert_fixed_orders(topology: &CompanionTopology, evidence: &Value, world: &str, target: u64) {
    for (guid, order) in [
        (
            topology.party.warrior,
            format!("(target = (target_guid = {target}))"),
        ),
        (
            topology.party.priest,
            format!("(target = (target_guid = {target}))"),
        ),
        (
            topology.party.mage_one,
            format!("(assist = (member_guid = {}))", topology.party.priest),
        ),
        (
            topology.party.mage_two,
            format!("(target = (target_guid = {target}))"),
        ),
    ] {
        assert_order_in_evidence(evidence, world, guid, &order);
    }
}

fn assert_follow_orders(topology: &CompanionTopology, evidence: &Value, world: &str) {
    for guid in topology.party.bots() {
        assert_order_in_evidence(
            evidence,
            world,
            guid,
            &format!("(follow = (leader_guid = {}))", topology.party.leader),
        );
    }
}

fn assert_party_whole(topology: &CompanionTopology, evidence: &Value) {
    let source_characters = evidence["source"]["characters"].as_array().unwrap();
    let destination_characters = evidence["destination"]["characters"].as_array().unwrap();
    assert_eq!(
        source_characters.len() + destination_characters.len(),
        5,
        "party Character multiplicity changed: {evidence}"
    );
    assert_party_landed(topology, evidence, "source");
    assert!(
        evidence["destination"]["bodies"]
            .as_array()
            .unwrap()
            .is_empty(),
        "dungeon bodies survived the exit"
    );
}

fn assert_party_landed(topology: &CompanionTopology, evidence: &Value, world: &str) {
    assert_eq!(
        evidence[world]["characters"].as_array().unwrap().len(),
        5,
        "landing did not retain five Characters: {evidence}"
    );
    assert_eq!(
        evidence[world]["bodies"].as_array().unwrap().len(),
        5,
        "landing did not rebuild five Character bodies: {evidence}"
    );
    let expected_guids: BTreeSet<_> = topology
        .party
        .all()
        .into_iter()
        .map(|guid| guid.to_string())
        .collect();
    let actual_guids: BTreeSet<_> = evidence[world]["characters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["guid"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(actual_guids, expected_guids, "landing changed party GUIDs");
    for (guid, class, role) in [
        (topology.party.warrior, "1", "0"),
        (topology.party.priest, "5", "1"),
        (topology.party.mage_one, "8", "2"),
        (topology.party.mage_two, "8", "2"),
    ] {
        let expected_guid = guid.to_string();
        let actual = evidence[world]["roles"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["character_guid"].as_str() == Some(expected_guid.as_str()))
            .unwrap_or_else(|| panic!("role missing after exit for {guid}: {evidence}"));
        assert_eq!(
            actual["class"], class,
            "class changed after exit for {guid}"
        );
        assert_eq!(actual["role"], role, "role changed after exit for {guid}");
        assert!(
            value_row_by_guid(evidence, world, "bots", "character_guid", guid)["controller"]
                .as_str()
                .is_some_and(|controller| controller.contains("cohort")),
            "Cohort controller changed after landing for {guid}"
        );
    }
    let group = evidence[world]["group"].as_array().unwrap();
    assert_eq!(group.len(), 1, "landing lost the party mirror");
    assert_eq!(
        group[0]["leader_guid"],
        topology.party.leader.to_string(),
        "landing changed the party leader"
    );
    let member_guids: BTreeSet<_> = evidence[world]["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["character_guid"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        member_guids, expected_guids,
        "landing changed party membership"
    );
}

fn assert_old_world_frozen(evidence: &Value, old: &str, current: &str) {
    assert_eq!(
        evidence[current]["characters"].as_array().unwrap().len(),
        5,
        "current holder does not own all five Characters: {evidence}"
    );
    for table in [
        "characters",
        "bodies",
        "bots",
        "roles",
        "orders",
        "runners",
        "actions",
        "splines",
        "casts",
        "auras",
        "melee",
        "items",
        "provisioning",
        "quests",
        "command_receipts",
    ] {
        assert!(
            evidence[old][table].as_array().unwrap().is_empty(),
            "old holder retained {table}: {evidence}"
        );
    }
}

fn canonical_rows(value: &Value, world: &str, table: &str) -> Vec<String> {
    let mut rows: Vec<_> = value[world][table]
        .as_array()
        .unwrap()
        .iter()
        .map(Value::to_string)
        .collect();
    rows.sort();
    rows
}

fn value_row_by_guid<'a>(
    value: &'a Value,
    world: &str,
    table: &str,
    guid_field: &str,
    guid: u64,
) -> &'a Value {
    let expected_guid = guid.to_string();
    value[world][table]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row[guid_field].as_str() == Some(expected_guid.as_str()))
        .unwrap_or_else(|| panic!("{table} row for {guid} missing from {value}"))
}

fn fault_in<'a>(value: &'a Value, world: &str, id: u64) -> &'a Value {
    let expected_id = id.to_string();
    value[world]["faults"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"].as_str() == Some(expected_id.as_str()))
        .unwrap_or_else(|| panic!("fault {id} missing from {value}"))
}

fn assert_mage_recovery_identity(topology: &CompanionTopology, before: &Value, after: &Value) {
    let guid = topology.party.mage_two;
    let before_bot = value_row_by_guid(before, "source", "bots", "character_guid", guid);
    let after_bot = value_row_by_guid(after, "source", "bots", "character_guid", guid);
    for field in ["class", "role", "controller"] {
        assert_eq!(
            before_bot[field], after_bot[field],
            "Mage recovery changed {field}"
        );
    }
    assert_eq!(after_bot["class"], "8");
    assert_eq!(after_bot["role"], "2");
    assert!(after_bot["controller"].as_str().unwrap().contains("cohort"));
    let before_order = value_row_by_guid(before, "source", "orders", "character_guid", guid);
    let after_order = value_row_by_guid(after, "source", "orders", "character_guid", guid);
    for field in ["issuer_guid", "group_id", "active", "revision", "order"] {
        assert_eq!(
            before_order[field], after_order[field],
            "Mage recovery changed Companion Order {field}"
        );
    }
    assert_eq!(
        after_order["order"],
        format!("(follow = (leader_guid = {}))", topology.party.leader)
    );
}

fn assert_restart_retained(before: &Value, after: &Value, warrior: u64, leader: u64) {
    let before_runner = before["runner"]
        .as_array()
        .and_then(|rows| rows.first())
        .expect("restart point Runner missing");
    let after_runner =
        value_row_by_guid(after, "destination", "runners", "character_guid", warrior);
    assert_ne!(
        before_runner["foreground"], "(none = ())",
        "restart point had no active Follow foreground"
    );
    assert_eq!(
        before["spline"].as_array().unwrap().len(),
        1,
        "restart point had no active Core movement leg"
    );
    let foreground = before_runner["foreground"].as_str().unwrap();
    assert!(
        foreground.contains(&format!("action = (move = (entity = {leader}))"))
            && foreground.contains("reason = (follow = ())"),
        "restart point foreground was not the retained Follow action: {foreground}"
    );
    let spline = &before["spline"][0];
    assert_eq!(parse_value_u64(spline, "guid"), warrior);
    let start = (
        parse_value_f32(spline, "sx"),
        parse_value_f32(spline, "sy"),
        parse_value_f32(spline, "sz"),
    );
    let destination = (
        parse_value_f32(spline, "dx"),
        parse_value_f32(spline, "dy"),
        parse_value_f32(spline, "dz"),
    );
    let leader_body = before["leader_body"]
        .as_array()
        .and_then(|rows| rows.first())
        .expect("restart point leader body missing");
    assert_eq!(parse_value_u64(leader_body, "guid"), leader);
    let leader_position = (
        parse_value_f32(leader_body, "x"),
        parse_value_f32(leader_body, "y"),
        parse_value_f32(leader_body, "z"),
    );
    assert!(
        distance(destination, leader_position) + 0.1 < distance(start, leader_position),
        "restart point movement leg did not approach the Follow leader"
    );
    for field in [
        "generation",
        "objective_sequence",
        "companion_order_revision",
    ] {
        assert_eq!(
            before_runner[field], after_runner[field],
            "Module restart changed retained Runner {field}"
        );
    }
    assert_stable_companion_objective(
        &before_runner["objective"],
        &after_runner["objective"],
        "Module restart",
    );
    let active = |value: &Value| {
        value["destination"]["quests"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| {
                row["character_guid"].as_str() == Some(warrior.to_string().as_str())
                    && row["quest_entry"].as_str() == Some("50911")
            })
            .cloned()
            .expect("retained Quest missing")
    };
    let before_quest = before["quest"]
        .as_array()
        .and_then(|rows| rows.first())
        .expect("restart point retained Quest missing");
    let retained = active(after);
    assert_eq!(
        before_quest, &retained,
        "Module restart changed retained Quest"
    );
}

fn assert_distinct_unchanged_command(
    topology: &CompanionTopology,
    before: &Value,
    after: &Value,
    warrior: u64,
    intent_id: u64,
) {
    let before_order = value_row_by_guid(before, "source", "orders", "character_guid", warrior);
    let after_order = value_row_by_guid(after, "source", "orders", "character_guid", warrior);
    for field in ["issuer_guid", "group_id", "active", "revision", "order"] {
        assert_eq!(
            before_order[field], after_order[field],
            "Unchanged intent B changed Companion Order field {field}"
        );
    }
    let before_bot = value_row_by_guid(before, "source", "bots", "character_guid", warrior);
    let after_bot = value_row_by_guid(after, "source", "bots", "character_guid", warrior);
    for field in ["controller", "class", "role"] {
        assert_eq!(
            before_bot[field], after_bot[field],
            "Unchanged intent B changed bot field {field}"
        );
    }
    let before_runner = value_row_by_guid(before, "source", "runners", "character_guid", warrior);
    let after_runner = value_row_by_guid(after, "source", "runners", "character_guid", warrior);
    for field in [
        "generation",
        "objective_sequence",
        "companion_order_revision",
    ] {
        assert_eq!(
            before_runner[field], after_runner[field],
            "Unchanged intent B changed retained work field {field}"
        );
    }
    assert_stable_companion_objective(
        &before_runner["objective"],
        &after_runner["objective"],
        "Unchanged intent B",
    );
    let intent = one(
        &topology.query(
            &topology.source,
            &format!("SELECT * FROM game_party_command_intent WHERE id = {intent_id}"),
        ),
        "finished intent B",
    )
    .clone();
    assert_eq!(intent["pending"], "false");
    assert_eq!(intent["state"], "(finished = (unchanged = ()))");
    let receipts = topology.query(
        &topology.source,
        &format!("SELECT * FROM game_party_command_receipt WHERE intent_id = {intent_id}"),
    );
    assert_eq!(receipts.len(), 1, "intent B has duplicate target receipts");
    assert_eq!(receipts[0]["outcome"], "(unchanged = ())");
}

fn assert_inventory_retained(
    topology: &CompanionTopology,
    before: &Value,
    after: &Value,
    death_guid: Option<u64>,
) {
    assert_eq!(
        canonical_rows(before, "source", "provisioning"),
        canonical_rows(after, "source", "provisioning"),
        "provisioning receipt changed across restart and Transfer"
    );
    let before_items = before["source"]["items"].as_array().unwrap();
    let items = after["source"]["items"].as_array().unwrap();
    let before_item_guids: BTreeSet<_> = before_items
        .iter()
        .map(|row| row["guid"].as_str().expect("item guid missing"))
        .collect();
    let item_guids: BTreeSet<_> = items
        .iter()
        .map(|row| row["guid"].as_str().expect("item guid missing"))
        .collect();
    assert_eq!(
        item_guids.len(),
        items.len(),
        "duplicate item GUID after exit"
    );
    assert_eq!(
        before_item_guids, item_guids,
        "item identity changed across the route"
    );
    let mut equipped = BTreeSet::new();
    for item in items {
        let slot = item["slot"].as_str().unwrap().parse::<u16>().unwrap();
        if slot <= 18 {
            let owner = item["owner_guid"].as_str().unwrap();
            assert!(
                equipped.insert((owner, slot)),
                "duplicate equipped slot {slot} for {owner}"
            );
        }
    }
    let expected_owners: BTreeSet<_> = topology
        .party
        .bots()
        .into_iter()
        .map(|guid| guid.to_string())
        .collect();
    let equipped_owners: BTreeSet<_> = equipped
        .iter()
        .map(|(owner, _)| (*owner).to_string())
        .collect();
    assert_eq!(
        equipped_owners, expected_owners,
        "one or more companions lost all equipped items"
    );
    for guid in topology.party.bots() {
        let owner = guid.to_string();
        for entry in ["1251", "6948"] {
            assert!(
                before_items.iter().any(|item| {
                    item["owner_guid"].as_str() == Some(owner.as_str())
                        && item["entry"].as_str() == Some(entry)
                }),
                "bot {guid} lacked provisioned supply {entry} before the route"
            );
        }
    }
    let before_templates = before["source"]["item_templates"].as_array().unwrap();
    let after_templates = after["source"]["item_templates"].as_array().unwrap();
    let canonical_templates = |rows: &[Value]| {
        let mut rows = rows.iter().map(Value::to_string).collect::<Vec<_>>();
        rows.sort();
        rows
    };
    assert_eq!(
        canonical_templates(before_templates),
        canonical_templates(after_templates),
        "item template durability facts changed across the route"
    );
    let max_durability = |entry: &str| {
        let template = before_templates
            .iter()
            .find(|row| row["entry"].as_str() == Some(entry))
            .unwrap_or_else(|| panic!("item template {entry} missing"));
        parse_value_u64(template, "max_durability")
    };
    for prior in before_items {
        let guid = prior["guid"].as_str().expect("item guid missing");
        let current = items
            .iter()
            .find(|row| row["guid"].as_str() == Some(guid))
            .unwrap_or_else(|| panic!("item {guid} disappeared"));
        for field in [
            "guid",
            "entry",
            "owner_identity",
            "owner_guid",
            "slot",
            "stack_count",
            "created_at",
            "enchant_id",
            "soulbound",
            "random_property_id",
        ] {
            assert_eq!(
                prior[field], current[field],
                "item {guid} changed retained field {field}"
            );
        }
        let before_durability = parse_value_u64(prior, "durability");
        let after_durability = parse_value_u64(current, "durability");
        let owner = parse_value_u64(prior, "owner_guid");
        let slot = parse_value_u64(prior, "slot");
        let entry = prior["entry"].as_str().expect("item entry missing");
        if death_guid == Some(owner) && slot <= 18 {
            let max = max_durability(entry);
            let loss = if max == 0 || before_durability == 0 {
                0
            } else {
                (max / 10).max(1)
            };
            assert_eq!(
                after_durability,
                before_durability.saturating_sub(loss),
                "item {guid} did not retain the exact Core death durability loss"
            );
        } else if death_guid.is_some() && owner == topology.party.warrior && slot == 15 {
            assert!(
                after_durability <= before_durability,
                "Warrior main-hand durability increased during ordinary combat"
            );
        } else {
            assert_eq!(
                after_durability, before_durability,
                "item {guid} changed durability without its ordinary owner"
            );
        }
    }
    let before_reward: Vec<_> = before["source"]["quests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["quest_entry"].as_str() == Some("50910"))
        .collect();
    let after_reward: Vec<_> = after["source"]["quests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["quest_entry"].as_str() == Some("50910"))
        .collect();
    assert_eq!(
        before_reward, after_reward,
        "reward receipt reapplied or changed"
    );
}

fn assert_quest_unchanged(before: &Value, after: &Value, guid: u64, quest_entry: u32) {
    let expected_guid = guid.to_string();
    let expected_quest = quest_entry.to_string();
    let exact = |value: &Value| {
        value["source"]["quests"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| {
                row["character_guid"].as_str() == Some(expected_guid.as_str())
                    && row["quest_entry"].as_str() == Some(expected_quest.as_str())
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        exact(before),
        exact(after),
        "retained Quest changed across restart and Transfer"
    );
}
