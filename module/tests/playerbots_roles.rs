//! Durable level-5 Warrior, Priest, and Mage behavior on private Module databases.

mod support;

use std::collections::BTreeMap;
use support::{poll_until, Standalone, POLL_TIMEOUT};

const WARRIOR: &str = "1";
const PRIEST: &str = "5";
const MAGE: &str = "8";
const TANK: &str = "0";
const HEALER: &str = "1";
const DAMAGE: &str = "2";

struct RolesFixture {
    node: Standalone,
    warrior: String,
    priest: String,
    mage: String,
    leader: String,
    enemies: Vec<String>,
}

fn runner(node: &Standalone, guid: &str) -> BTreeMap<String, String> {
    node.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_runner WHERE character_guid = {guid}"
    ))
    .into_iter()
    .next()
    .expect("runner explanation missing")
}

fn pass(node: &Standalone, guid: &str) {
    node.assert_call("playerbots_fixture_runner_pass_once", &[guid]);
}

fn entity(node: &Standalone, guid: &str) -> BTreeMap<String, String> {
    node.query_rows(&format!(
        "SELECT * FROM game_world_entity WHERE guid = {guid}"
    ))[0]
        .clone()
}

fn known(node: &Standalone, guid: &str, spell: u32) -> bool {
    !node
        .query_rows(&format!(
            "SELECT spell_id FROM game_player_spell WHERE character_guid = {guid} AND spell_id = {spell}"
        ))
        .is_empty()
}

fn cast_events(node: &Standalone, guid: &str, spell: u32) -> Vec<BTreeMap<String, String>> {
    node.query_rows(&format!(
        "SELECT * FROM game_spell_cast_event WHERE caster_guid = {guid} AND spell_id = {spell}"
    ))
}

fn node_evidence(node: &Standalone, case: &str) {
    let path = support::log_dir().join(format!("{}-{case}.json", node.shard_name()));
    let record = serde_json::json!({
        "case": case,
        "spacetimedb": "2.7.1",
        "supported_companion_level": 5,
        "seeded_content_identity": "playerbots-starter-roles-v1",
        "seeded_geometry_identity": "playerbots-synthetic-nav-v1",
        "content": {
            "revision": "playerbots-starter-roles-v1",
            "provenance": "curated core seeds and explicitly named private role fixture rows",
            "imported_content": null,
        },
        "geometry": {
            "revision": "playerbots-synthetic-nav-v1",
            "provenance": "private source-defined navigation cells and obstruction columns",
            "client_geometry": null,
        },
        "roles": node.query_rows("SELECT character_guid, class, role, controller FROM pkg_playerbots_bot"),
        "runners": node.query_rows("SELECT * FROM pkg_playerbots_runner"),
        "entities": node.query_rows("SELECT guid, entry, level, x, y, z, health, max_health, power, max_power, dead, target_guid FROM game_world_entity"),
        "spells": node.query_rows("SELECT character_guid, spell_id FROM game_player_spell"),
        "spell_headers": node.query_rows("SELECT spell_id, cost, spell_level, range_yd FROM game_spell WHERE spell_id = 133 OR spell_id = 139 OR spell_id = 168 OR spell_id = 355 OR spell_id = 585 OR spell_id = 1243 OR spell_id = 6673 OR spell_id = 7386"),
        "cooldowns": node.query_rows("SELECT * FROM game_spell_cooldown"),
        "provisioning": node.query_rows("SELECT * FROM pkg_playerbots_provisioning"),
        "auras": node.query_rows("SELECT id, target_guid, caster_guid, spell_id, eff_kind, eff_p0 FROM game_aura"),
        "casts": node.query_rows("SELECT * FROM game_spell_cast_event"),
        "melee": node.query_rows("SELECT * FROM game_melee_attack"),
        "threat": node.query_rows("SELECT * FROM game_threat"),
        "splines": node.query_rows("SELECT * FROM game_creature_spline"),
        "nav_chunks": node.query_rows("SELECT key, map_id, cell_x, cell_y, base_z, walk, obs FROM game_nav_chunk"),
        "coverage_manifests": node.query_rows("SELECT * FROM game_vmap_nav_coverage_manifest"),
    });
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

fn evidence(fixture: &RolesFixture, case: &str) {
    node_evidence(&fixture.node, case);
}

fn fixture(name: &str) -> RolesFixture {
    let mut node = Standalone::start(name);
    node.publish_module();
    node.assert_call("claim_operator", &[]);
    node.assert_call("install_guid_range", &["1000000"]);
    node.assert_call("playerbots_spawn", &["4", "1200", "1200", "50"]);
    node_evidence(&node, "spawned");
    let mut rows = node.query_rows("SELECT character_guid, class, role FROM pkg_playerbots_bot");
    rows.sort_by_key(|row| row["character_guid"].parse::<u64>().unwrap());
    let priest = rows
        .iter()
        .find(|row| row["class"] == PRIEST && row["role"] == HEALER)
        .unwrap()["character_guid"]
        .clone();
    let mage = rows
        .iter()
        .find(|row| row["class"] == MAGE && row["role"] == DAMAGE)
        .unwrap()["character_guid"]
        .clone();
    let warriors: Vec<_> = rows
        .iter()
        .filter(|row| row["class"] == WARRIOR && row["role"] == TANK)
        .map(|row| row["character_guid"].clone())
        .collect();
    assert_eq!(warriors.len(), 2);
    let warrior = warriors[0].clone();
    let leader = warriors[1].clone();
    node.assert_call(
        "playerbots_fixture_roles_stage",
        &[&warrior, &priest, &mage, &leader],
    );
    node_evidence(&node, "staged");
    let mut enemies: Vec<_> = node
        .query_rows(
            "SELECT guid FROM game_world_entity WHERE entry >= 5098001 AND entry <= 5098003",
        )
        .into_iter()
        .map(|row| row["guid"].clone())
        .collect();
    enemies.sort_by_key(|guid| guid.parse::<u64>().unwrap());
    assert_eq!(enemies.len(), 3);
    for guid in [&warrior, &priest, &mage] {
        node.assert_call("playerbots_fixture_provision_steps", &[guid, "32"]);
    }
    node_evidence(&node, "provisioned");
    RolesFixture {
        node,
        warrior,
        priest,
        mage,
        leader,
        enemies,
    }
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn starter_roles_use_level_five_capabilities_and_never_pull_from_selection() {
    let fixture = fixture("playerbots-roles-capabilities");
    let node = &fixture.node;
    for guid in [&fixture.warrior, &fixture.priest, &fixture.mage] {
        assert_eq!(entity(node, guid)["level"], "5");
    }
    for (guid, spells) in [
        (&fixture.warrior, &[78, 2457, 355][..]),
        (&fixture.priest, &[585, 2050, 139][..]),
        (&fixture.mage, &[133, 168][..]),
    ] {
        for spell in spells {
            assert!(known(node, guid, *spell), "{guid} does not know {spell}");
        }
    }
    for spell in [7386, 6673] {
        assert!(!known(node, &fixture.warrior, spell));
    }
    assert!(!known(node, &fixture.priest, 1243));
    assert!(node
        .query_rows("SELECT spell_id FROM game_spell WHERE spell_id = 7386")
        .is_empty());
    assert!(node
        .query_rows("SELECT spell_id FROM game_spell WHERE spell_id = 585")
        .is_empty());

    let target = &fixture.enemies[0];
    node.assert_call(
        "playerbots_fixture_roles_select",
        &[&fixture.leader, target],
    );
    for guid in [&fixture.warrior, &fixture.priest, &fixture.mage] {
        pass(node, guid);
    }
    evidence(&fixture, "selection-only");
    for guid in [&fixture.warrior, &fixture.priest, &fixture.mage] {
        assert!(runner(node, guid)["companion_fight_target_guid"].contains("none"));
    }
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE target_guid = {target}"
        ))
        .is_empty());
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_pending_cast WHERE target_guid = {target}"
        ))
        .is_empty());

    node.assert_call(
        "playerbots_fixture_roles_engage",
        &[&fixture.leader, target],
    );
    let warrior_attack = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.warrior);
        !node
            .query_rows(&format!(
                "SELECT * FROM game_melee_attack WHERE attacker_guid = {} AND target_guid = {target}",
                fixture.warrior
            ))
            .is_empty()
    });
    evidence(&fixture, "warrior-engaged");
    assert!(warrior_attack);
    let tank = runner(node, &fixture.warrior);
    assert!(tank["chosen"].contains("tankFight"), "{tank:?}");
    assert!(tank["companion_fight_target_guid"].contains(target));
    assert!(!node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {} AND target_guid = {target}",
            fixture.warrior
        ))
        .is_empty());

    pass(node, &fixture.priest);
    evidence(&fixture, "priest-engaged");
    let healer = runner(node, &fixture.priest);
    assert!(healer["chosen"].contains("damageFight"), "{healer:?}");
    assert!(cast_events(node, &fixture.priest, 585).is_empty());

    let mage_cast = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.mage);
        !cast_events(node, &fixture.mage, 133).is_empty()
    });
    evidence(&fixture, "mage-engaged");
    assert!(mage_cast);
    let damage = runner(node, &fixture.mage);
    assert!(damage["chosen"].contains("damageFight"), "{damage:?}");
    assert!(!damage["chosen"].contains("meleePosition"));
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE attacker_guid = {}",
            fixture.mage
        ))
        .is_empty());
    evidence(&fixture, "capabilities-and-consent");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn selected_engaged_target_wins_and_control_holds_every_role() {
    let fixture = fixture("playerbots-roles-target-control");
    let node = &fixture.node;
    let first = &fixture.enemies[0];
    let selected = &fixture.enemies[1];
    node.assert_call("playerbots_fixture_roles_engage", &[&fixture.leader, first]);
    node.assert_call(
        "playerbots_fixture_roles_enemy_engage",
        &[selected, &fixture.leader],
    );
    node.assert_call(
        "playerbots_fixture_roles_select",
        &[&fixture.leader, selected],
    );
    for guid in [&fixture.warrior, &fixture.priest, &fixture.mage] {
        pass(node, guid);
    }
    evidence(&fixture, "designated-engaged");
    for guid in [&fixture.warrior, &fixture.priest, &fixture.mage] {
        assert!(
            runner(node, guid)["companion_fight_target_guid"].contains(selected),
            "{}",
            runner(node, guid)["companion_fight_target_guid"]
        );
    }
    node.assert_call("playerbots_fixture_roles_despawn", &[first]);

    node.assert_call(
        "playerbots_fixture_roles_begin_control",
        &[&fixture.leader, selected, "50023"],
    );
    for guid in [&fixture.warrior, &fixture.priest, &fixture.mage] {
        pass(node, guid);
    }
    evidence(&fixture, "pending-control");
    for guid in [&fixture.warrior, &fixture.priest, &fixture.mage] {
        let held = runner(node, guid);
        assert!(
            held["chosen"].contains("crowdControl") || held["chosen"].contains("heal"),
            "{held:?}"
        );
        assert!(held["companion_fight_target_guid"].contains("none"));
        assert!(node
            .query_rows(&format!(
                "SELECT * FROM game_pending_cast WHERE caster_guid = {guid} AND target_guid = {selected}"
            ))
            .is_empty());
    }
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_melee_attack WHERE target_guid = {selected}"
        ))
        .is_empty());
    node.assert_call(
        "playerbots_fixture_roles_clear_control",
        &[&fixture.leader, selected],
    );
    for spell in [50_020, 50_021, 50_022, 50_023] {
        node.assert_call(
            "playerbots_fixture_roles_control",
            &[&fixture.leader, selected, &spell.to_string()],
        );
        for guid in [&fixture.warrior, &fixture.priest, &fixture.mage] {
            pass(node, guid);
        }
        evidence(&fixture, &format!("control-{spell}"));
        for guid in [&fixture.warrior, &fixture.priest, &fixture.mage] {
            let held = runner(node, guid);
            assert!(
                held["chosen"].contains("crowdControl") || held["chosen"].contains("heal"),
                "{held:?}"
            );
            assert!(held["companion_fight_target_guid"].contains("none"));
        }
        node.assert_call(
            "playerbots_fixture_roles_clear_control",
            &[&fixture.leader, selected],
        );
    }
    pass(node, &fixture.warrior);
    evidence(&fixture, "control-cleared");
    let resumed = runner(node, &fixture.warrior);
    assert!(resumed["companion_fight_target_guid"].contains(selected));
    assert!(
        resumed["chosen"].contains("tankFight") || resumed["chosen"].contains("meleePosition"),
        "{resumed:?}"
    );
    evidence(&fixture, "target-and-control");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn buffs_and_repeated_pulls_retain_roles_through_rest_los_and_death() {
    let fixture = fixture("playerbots-roles-repeated-pulls");
    let node = &fixture.node;

    pass(node, &fixture.mage);
    evidence(&fixture, "frost-armor-first-pass");
    assert!(!node
        .query_rows(&format!(
            "SELECT spell_id FROM game_aura WHERE target_guid = {} AND spell_id = 168",
            fixture.mage
        ))
        .is_empty());
    let frost_armor_events = cast_events(node, &fixture.mage, 168).len();
    pass(node, &fixture.mage);
    evidence(&fixture, "frost-armor-repeat");
    assert_eq!(
        cast_events(node, &fixture.mage, 168).len(),
        frost_armor_events
    );

    node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.leader, "25"],
    );
    let before = entity(node, &fixture.leader)["health"]
        .parse::<u32>()
        .unwrap();
    let heal_cast = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.priest);
        !cast_events(node, &fixture.priest, 139).is_empty()
    });
    let healed = poll_until(POLL_TIMEOUT, || {
        entity(node, &fixture.leader)["health"]
            .parse::<u32>()
            .unwrap()
            > before
    });
    evidence(&fixture, "between-fights-heal");
    assert!(heal_cast);
    assert!(healed);
    let renew = runner(node, &fixture.priest);
    assert!(renew["cast_progress"].contains("spell = 139"), "{renew:?}");
    assert!(
        renew["cast_progress"].contains(&format!("target = {}", fixture.leader)),
        "{renew:?}"
    );
    assert!(!node
        .query_rows(&format!(
            "SELECT * FROM game_aura WHERE target_guid = {} AND spell_id = 139",
            fixture.leader
        ))
        .is_empty());

    node.assert_call("playerbots_fixture_roles_priest_mana", &[&fixture.priest]);
    node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.leader, "25"],
    );
    let direct_started = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.priest);
        !node
            .query_rows(&format!(
                "SELECT * FROM game_pending_cast WHERE caster_guid = {} AND spell_id = 2050 AND target_guid = {}",
                fixture.priest, fixture.leader
            ))
            .is_empty()
    });
    evidence(&fixture, "lesser-heal-started");
    assert!(direct_started);
    assert_eq!(entity(node, &fixture.priest)["power"], "70");
    let direct_finished = poll_until(POLL_TIMEOUT, || {
        let state = runner(node, &fixture.priest);
        state["cast_progress"].contains("spell = 2050")
            && state["cast_progress"].contains(&format!("target = {}", fixture.leader))
            && entity(node, &fixture.leader)["health"]
                .parse::<u32>()
                .unwrap()
                > 30
    });
    evidence(&fixture, "lesser-heal-finished");
    assert!(direct_finished);

    let first = &fixture.enemies[0];
    node.assert_call("playerbots_fixture_roles_engage", &[&fixture.leader, first]);
    pass(node, &fixture.warrior);
    pass(node, &fixture.mage);
    let retained_role = node.query_rows(&format!(
        "SELECT role FROM pkg_playerbots_bot WHERE character_guid = {}",
        fixture.warrior
    ))[0]["role"]
        .clone();
    node.assert_call("debug_kill_creature", &[&fixture.leader, first]);
    pass(node, &fixture.mage);
    evidence(&fixture, "first-target-dead");
    assert!(node
        .query_rows(&format!(
            "SELECT * FROM game_pending_cast WHERE caster_guid = {} AND target_guid = {first}",
            fixture.mage
        ))
        .is_empty());
    pass(node, &fixture.warrior);
    assert!(runner(node, &fixture.warrior)["companion_fight_target_guid"].contains("none"));

    let second = &fixture.enemies[1];
    node.assert_call(
        "playerbots_fixture_roles_engage",
        &[&fixture.leader, second],
    );
    pass(node, &fixture.warrior);
    node.assert_call("playerbots_fixture_roles_despawn", &[second]);
    pass(node, &fixture.warrior);
    evidence(&fixture, "second-target-gone");
    assert!(runner(node, &fixture.warrior)["companion_fight_target_guid"].contains("none"));

    let third = &fixture.enemies[2];
    node.assert_call("playerbots_fixture_companion_wall", &[&fixture.mage, third]);
    node.assert_call("playerbots_fixture_roles_engage", &[&fixture.leader, third]);
    let los_repair = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.mage);
        runner(node, &fixture.mage)["chosen"].contains("castingPosition")
    });
    evidence(&fixture, "third-target-los");
    assert!(los_repair);
    let los = runner(node, &fixture.mage);
    assert!(los["chosen"].contains("castingPosition"), "{los:?}");
    assert!(los["companion_fight_target_guid"].contains(third));
    node.assert_call("playerbots_fixture_roles_despawn", &[third]);

    let objective = runner(node, &fixture.mage)["objective_sequence"].clone();
    node.assert_call("debug_set_health", &[&fixture.mage, "0"]);
    pass(node, &fixture.mage);
    evidence(&fixture, "mage-dead");
    assert!(runner(node, &fixture.mage)["chosen"].contains("resurrection"));
    pass(node, &fixture.mage);
    evidence(&fixture, "mage-resurrected");
    assert_eq!(entity(node, &fixture.mage)["dead"], "false");
    node.assert_call(
        "playerbots_fixture_companion_move",
        &[&fixture.mage, "1180", "1200"],
    );
    pass(node, &fixture.mage);
    evidence(&fixture, "mage-regrouped");
    let regrouped = runner(node, &fixture.mage);
    assert_eq!(regrouped["objective_sequence"], objective);
    assert!(regrouped["companion_leader_guid"].contains(&fixture.leader));
    assert_eq!(
        node.query_rows(&format!(
            "SELECT role FROM pkg_playerbots_bot WHERE character_guid = {}",
            fixture.warrior
        ))[0]["role"],
        retained_role
    );
    for (guid, role) in [
        (&fixture.warrior, TANK),
        (&fixture.priest, HEALER),
        (&fixture.mage, DAMAGE),
    ] {
        assert_eq!(
            node.query_rows(&format!(
                "SELECT role FROM pkg_playerbots_bot WHERE character_guid = {guid}"
            ))[0]["role"],
            role
        );
    }
    evidence(&fixture, "repeated-pulls");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn a_stronger_buff_family_member_prevents_a_weaker_maintenance_cast() {
    let fixture = fixture("playerbots-roles-buff-family");
    let node = &fixture.node;
    node.assert_call(
        "playerbots_fixture_roles_prepare_fortitude",
        &[&fixture.priest],
    );
    node.assert_call(
        "playerbots_fixture_provision_steps",
        &[&fixture.priest, "1"],
    );
    evidence(&fixture, "fortitude-provisioned");
    assert!(known(node, &fixture.priest, 1243));
    for target in [
        &fixture.leader,
        &fixture.warrior,
        &fixture.priest,
        &fixture.mage,
    ] {
        node.assert_call(
            "playerbots_fixture_roles_stronger_fortitude",
            &[&fixture.leader, target],
        );
    }
    let before = cast_events(node, &fixture.priest, 1243).len();
    pass(node, &fixture.priest);
    evidence(&fixture, "stronger-family-active");
    assert_eq!(cast_events(node, &fixture.priest, 1243).len(), before);
    assert!(!node
        .query_rows(&format!(
            "SELECT spell_id FROM game_aura WHERE target_guid = {} AND spell_id = 21562",
            fixture.leader
        ))
        .is_empty());
    evidence(&fixture, "buff-family");
}
