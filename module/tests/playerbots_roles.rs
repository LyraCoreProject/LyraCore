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
const PRECEDING_CORE: &str = "dae05c1b78cfd4b59f24968ec8be41f75ce896d1";
const PRECEDING_CORE_TREE: &str = "432809ddefbc666d8dfe9ccde288eca0488bd8c7";
const PRECEDING_COLLECTION: &str = "2028771d15c8efe87b50d2b6795315221953055d";
const PRECEDING_COLLECTION_TREE: &str = "c27a460ab7cdcdd2072a90aa3275bc68e53b15c0";
const PRECEDING_PLAYERBOTS_TREE: &str = "05aaf2452beb3c4e18073b29deb978482351fc91";
const PRECEDING_PACKAGE_IDENTITY: &str =
    "573c5856e3a261e073a7bd414e2183efa7f6dcec823a47d0c190d29b1aab3db4";

struct RolesFixture {
    node: Standalone,
    warrior: String,
    priest: String,
    mage: String,
    leader: String,
    enemies: Vec<String>,
}

fn git(path: &std::path::Path, args: &[&str]) -> String {
    let result = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout).unwrap().trim().to_string()
}

fn digest_files(path: &std::path::Path, digest: &mut blake3::Hasher) {
    let mut children: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    children.sort();
    digest.update(&(children.len() as u64).to_le_bytes());
    for child in children {
        let name = child.file_name().unwrap().as_encoded_bytes();
        digest.update(&(name.len() as u64).to_le_bytes());
        digest.update(name);
        digest.update(&[u8::from(child.is_dir())]);
        if child.is_dir() {
            digest_files(&child, digest);
        } else {
            let contents = std::fs::read(&child).unwrap();
            digest.update(&(contents.len() as u64).to_le_bytes());
            digest.update(&contents);
        }
    }
}

struct PrecedingRoles {
    wasm: Vec<u8>,
    manifest: serde_json::Value,
}

fn preceding_roles() -> PrecedingRoles {
    let wasm_path = std::env::var_os("PLAYERBOTS_ROLES_PRECEDING_WASM")
        .expect("PLAYERBOTS_ROLES_PRECEDING_WASM must name the merged PB-006 Wasm");
    let manifest_path = std::env::var_os("PLAYERBOTS_ROLES_PRECEDING_MANIFEST")
        .expect("PLAYERBOTS_ROLES_PRECEDING_MANIFEST must describe that Wasm build");
    let core_path = std::env::var_os("PLAYERBOTS_ROLES_PRECEDING_CORE")
        .expect("PLAYERBOTS_ROLES_PRECEDING_CORE must name the clean merged Core checkout");
    let collection_path = std::env::var_os("PLAYERBOTS_ROLES_PRECEDING_COLLECTION").expect(
        "PLAYERBOTS_ROLES_PRECEDING_COLLECTION must name the clean merged Package checkout",
    );
    let core_path = std::path::Path::new(&core_path);
    let collection_path = std::path::Path::new(&collection_path);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest_path).unwrap()).unwrap();
    let wasm = std::fs::read(&wasm_path).unwrap();

    assert_eq!(manifest["core"], PRECEDING_CORE);
    assert_eq!(manifest["collection"], PRECEDING_COLLECTION);
    assert_eq!(manifest["core_tree"], PRECEDING_CORE_TREE);
    assert_eq!(manifest["collection_tree"], PRECEDING_COLLECTION_TREE);
    assert_eq!(manifest["playerbots_tree"], PRECEDING_PLAYERBOTS_TREE);
    assert_eq!(manifest["core_dirty"], false);
    assert_eq!(manifest["collection_dirty"], false);
    assert_eq!(manifest["rust"], "1.93.0");
    assert_eq!(manifest["spacetimedb"], "2.7.1");
    assert_eq!(manifest["target"], "wasm32-unknown-unknown");
    assert_eq!(manifest["profile"], "release");
    assert_eq!(manifest["features"], serde_json::json!(["debug_reducers"]));
    assert_eq!(
        manifest["installed_packages"],
        serde_json::json!(["dungeons", "example", "fire_nova", "playerbots"])
    );
    assert_eq!(
        manifest["package_content_identity"],
        PRECEDING_PACKAGE_IDENTITY
    );
    assert_eq!(manifest["wasm_bytes"].as_u64(), Some(wasm.len() as u64));

    assert_eq!(git(core_path, &["rev-parse", "HEAD"]), PRECEDING_CORE);
    assert_eq!(
        git(core_path, &["rev-parse", "HEAD^{tree}"]),
        PRECEDING_CORE_TREE
    );
    assert!(git(core_path, &["status", "--porcelain"]).is_empty());
    assert_eq!(
        git(collection_path, &["rev-parse", "HEAD"]),
        PRECEDING_COLLECTION
    );
    assert_eq!(
        git(collection_path, &["rev-parse", "HEAD^{tree}"]),
        PRECEDING_COLLECTION_TREE
    );
    assert_eq!(
        git(collection_path, &["rev-parse", "HEAD:playerbots"]),
        PRECEDING_PLAYERBOTS_TREE
    );
    assert!(git(collection_path, &["status", "--porcelain"]).is_empty());
    let mut package_digest = blake3::Hasher::new();
    digest_files(&collection_path.join("playerbots"), &mut package_digest);
    assert_eq!(
        package_digest.finalize().to_hex().as_str(),
        PRECEDING_PACKAGE_IDENTITY
    );

    let sha256 = std::process::Command::new("sha256sum")
        .arg(&wasm_path)
        .output()
        .unwrap();
    assert!(sha256.status.success());
    let sha256 = String::from_utf8(sha256.stdout).unwrap();
    assert_eq!(
        sha256.split_whitespace().next().unwrap(),
        manifest["wasm_sha256"].as_str().unwrap()
    );
    if let Some(expected) = manifest["wasm_blake3"].as_str() {
        assert_eq!(blake3::hash(&wasm).to_hex().as_str(), expected);
    }
    PrecedingRoles { wasm, manifest }
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
    let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let package = core.join("packages/playerbots");
    let mut package_digest = blake3::Hasher::new();
    digest_files(&package, &mut package_digest);
    let path = support::log_dir().join(format!("{}-{case}.json", node.shard_name()));
    let record = serde_json::json!({
        "case": case,
        "spacetimedb": "2.7.1",
        "rust": "1.93.0",
        "tested_core": git(core, &["rev-parse", "HEAD"]),
        "tested_collection": git(&package, &["rev-parse", "HEAD"]),
        "core_dirty": !git(core, &["status", "--porcelain"]).is_empty(),
        "collection_dirty": !git(&package, &["status", "--porcelain"]).is_empty(),
        "module_wasm_identity": blake3::hash(support::module_bytes()).to_hex().to_string(),
        "package_content_identity": package_digest.finalize().to_hex().to_string(),
        "validated_companion_levels": [5, 10],
        "seeded_content_identity": "playerbots-starter-roles-v1",
        "seeded_geometry_identity": "playerbots-synthetic-nav-v1",
        "content": {
            "revision": "playerbots-starter-roles-v1",
            "provenance": "curated core seeds and explicitly named private role fixture rows",
            "imported_content": null,
            "training_level_reference": {
                "source": "local classic-db-full.sql",
                "sha256": "d2083bcd2670451279cbf93af138eadae04c6d183a4cd0ff0357047e4a565de6",
                "use": "source-derived level facts only; not an imported World Shard",
                "levels": {"133": 1, "139": 8, "168": 1, "355": 10, "585": 1, "1243": 1, "2050": 1, "6673": 1, "7386": 10},
            },
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
        "spell_headers": node.query_rows("SELECT spell_id, cost, spell_level, range_yd FROM game_spell WHERE spell_id = 133 OR spell_id = 139 OR spell_id = 168 OR spell_id = 355 OR spell_id = 585 OR spell_id = 1243 OR spell_id = 2050 OR spell_id = 6673 OR spell_id = 7386"),
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

fn fixture(name: &str, level: u32) -> RolesFixture {
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
    if level != 5 {
        for guid in [&warrior, &priest, &mage, &leader] {
            node.assert_call("debug_set_level", &[guid, &level.to_string()]);
        }
        node_evidence(&node, &format!("level-{level}"));
    }
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
fn level_five_roles_obey_classic_level_gates_and_never_pull_from_selection() {
    let fixture = fixture("playerbots-roles-capabilities", 5);
    let node = &fixture.node;
    for guid in [&fixture.warrior, &fixture.priest, &fixture.mage] {
        assert_eq!(entity(node, guid)["level"], "5");
    }
    for (guid, spells) in [
        (&fixture.warrior, &[78, 2457][..]),
        (&fixture.priest, &[585, 2050][..]),
        (&fixture.mage, &[133, 168][..]),
    ] {
        for spell in spells {
            assert!(known(node, guid, *spell), "{guid} does not know {spell}");
        }
    }
    for spell in [355, 7386, 6673] {
        assert!(!known(node, &fixture.warrior, spell));
    }
    for spell in [139, 1243] {
        assert!(!known(node, &fixture.priest, spell));
    }
    assert_eq!(
        node.query_rows("SELECT spell_level FROM game_spell WHERE spell_id = 355")[0]
            ["spell_level"],
        "10"
    );
    assert_eq!(
        node.query_rows("SELECT spell_level FROM game_spell WHERE spell_id = 139")[0]
            ["spell_level"],
        "8"
    );
    let warrior_provisioning = node.query_rows(&format!(
        "SELECT history FROM pkg_playerbots_provisioning WHERE character_guid = {}",
        fixture.warrior
    ));
    let priest_provisioning = node.query_rows(&format!(
        "SELECT history FROM pkg_playerbots_provisioning WHERE character_guid = {}",
        fixture.priest
    ));
    assert!(warrior_provisioning[0]["history"].contains("spell 355 requires level 10"));
    assert!(priest_provisioning[0]["history"].contains("spell 139 requires level 8"));
    assert!(node
        .query_rows("SELECT spell_id FROM game_spell WHERE spell_id = 7386")
        .is_empty());
    assert!(node
        .query_rows("SELECT spell_id FROM game_spell WHERE spell_id = 585")
        .is_empty());

    // Model a populated PB-005 spellbook that learned the old level-zero Taunt header. The current
    // header Gate must keep it out of the level-five rotation without deleting the owned spell.
    node.assert_call("debug_learn_spell", &[&fixture.warrior, "355"]);
    assert!(known(node, &fixture.warrior, 355));

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
    assert!(cast_events(node, &fixture.warrior, 355).is_empty());
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
    let fixture = fixture("playerbots-roles-target-control", 10);
    let node = &fixture.node;
    for (guid, spells) in [
        (&fixture.warrior, &[78, 2457, 355][..]),
        (&fixture.priest, &[585, 2050, 139][..]),
        (&fixture.mage, &[133, 168][..]),
    ] {
        for spell in spells {
            assert!(known(node, guid, *spell), "{guid} does not know {spell}");
        }
    }
    assert!(!known(node, &fixture.warrior, 7386));
    let first = &fixture.enemies[0];
    let selected = &fixture.enemies[1];
    node.assert_call(
        "playerbots_select_controller",
        &[&fixture.warrior, "{\"recordOnly\":[]}"],
    );
    node.assert_call(
        "playerbots_fixture_roles_control",
        &[&fixture.leader, first, "50020"],
    );
    node.assert_call(
        "playerbots_fixture_roles_record_only_attack",
        &[&fixture.warrior, first],
    );
    let recorded_attack = node.query_rows(&format!(
        "SELECT * FROM game_melee_attack WHERE attacker_guid = {}",
        fixture.warrior
    ));
    evidence(&fixture, "record-only-controlled-attack");
    assert_eq!(recorded_attack.len(), 1);
    assert_eq!(recorded_attack[0]["target_guid"], *first);
    assert_eq!(recorded_attack[0]["last_swing_ms"], "4242");
    assert_eq!(recorded_attack[0]["last_offhand_swing_ms"], "2121");
    let recorded = runner(node, &fixture.warrior);
    assert!(recorded["last_outcome"].contains("recorded"));
    assert!(recorded["chosen"].contains("crowdControl"), "{recorded:?}");
    node.assert_call(
        "playerbots_fixture_roles_clear_control",
        &[&fixture.leader, first],
    );
    node.assert_call(
        "playerbots_select_controller",
        &[&fixture.warrior, "{\"cohort\":[]}"],
    );
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
fn tank_repairs_range_and_completes_a_real_taunt() {
    let fixture = fixture("playerbots-roles-tank-range", 10);
    let node = &fixture.node;
    let target = &fixture.enemies[0];
    node.assert_call("playerbots_fixture_roles_short_taunt", &[]);
    assert_eq!(
        node.query_rows("SELECT range_yd FROM game_spell WHERE spell_id = 355")[0]["range_yd"],
        "8"
    );
    node.assert_call("playerbots_fixture_roles_move", &[target, "1240", "1200"]);
    node.assert_call(
        "playerbots_fixture_roles_move",
        &[&fixture.leader, "1260", "1200"],
    );
    node.assert_call(
        "playerbots_fixture_roles_enemy_engage",
        &[target, &fixture.leader],
    );
    node.assert_call(
        "playerbots_fixture_roles_select",
        &[&fixture.leader, target],
    );
    let started_x = entity(node, &fixture.warrior)["x"].parse::<f32>().unwrap();
    assert_eq!(started_x, 1200.0);
    assert!(poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.warrior);
        entity(node, &fixture.warrior)["x"].parse::<f32>().unwrap() > started_x + 1.0
    }));
    let cast = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.warrior);
        cast_events(node, &fixture.warrior, 355)
            .iter()
            .any(|event| event["target_guid"] == *target)
    });
    evidence(&fixture, "tank-range-repair");
    assert!(cast);
    let tank = runner(node, &fixture.warrior);
    assert!(tank["chosen"].contains("tankFight"), "{tank:?}");
    assert!(entity(node, &fixture.warrior)["x"].parse::<f32>().unwrap() > started_x + 1.0);
    assert!(poll_until(POLL_TIMEOUT, || {
        entity(node, target)["target_guid"] == fixture.warrior
            && !node
                .query_rows(&format!(
                    "SELECT * FROM game_threat WHERE creature_guid = {target} AND source_guid = {}",
                    fixture.warrior
                ))
                .is_empty()
    }));
    evidence(&fixture, "tank-taunt-threat");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn buffs_and_repeated_pulls_retain_roles_through_rest_los_and_death() {
    let fixture = fixture("playerbots-roles-repeated-pulls", 10);
    let node = &fixture.node;

    node.assert_call("debug_learn_spell", &[&fixture.warrior, "6673"]);
    pass(node, &fixture.warrior);
    evidence(&fixture, "battle-shout-first-pass");
    assert!(!node
        .query_rows(&format!(
            "SELECT spell_id FROM game_aura WHERE target_guid = {} AND spell_id = 6673",
            fixture.warrior
        ))
        .is_empty());
    let battle_shout_events = cast_events(node, &fixture.warrior, 6673).len();
    pass(node, &fixture.warrior);
    evidence(&fixture, "battle-shout-repeat");
    assert_eq!(
        cast_events(node, &fixture.warrior, 6673).len(),
        battle_shout_events
    );

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

    node.assert_call("playerbots_fixture_roles_cancel_renew", &[&fixture.leader]);
    node.assert_call("playerbots_fixture_roles_priest_mana", &[&fixture.priest]);
    node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.leader, "25"],
    );
    let before_direct = entity(node, &fixture.leader)["health"]
        .parse::<u32>()
        .unwrap();
    let mut pending = None;
    let direct_started = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.priest);
        pending = node
            .query_rows(&format!(
                "SELECT * FROM game_pending_cast WHERE caster_guid = {} AND spell_id = 2050 AND target_guid = {}",
                fixture.priest, fixture.leader
            ))
            .into_iter()
            .next();
        pending.is_some()
    });
    let power_at_start = entity(node, &fixture.priest)["power"].clone();
    evidence(&fixture, "lesser-heal-started");
    assert!(direct_started);
    let pending = pending.expect("Lesser Heal pending cast missing after admission");
    assert_eq!(pending["caster_guid"], fixture.priest);
    assert_eq!(pending["spell_id"], "2050");
    assert_eq!(pending["target_guid"], fixture.leader);
    assert_eq!(power_at_start, "100");
    let scheduled_id = pending["scheduled_id"].clone();
    let direct_finished = poll_until(POLL_TIMEOUT, || {
        let state = runner(node, &fixture.priest);
        state["cast_progress"].contains(&format!("scheduled_id = {scheduled_id}"))
            && state["cast_progress"].contains("spell = 2050")
            && state["cast_progress"].contains(&format!("target = {}", fixture.leader))
            && state["last_outcome"].contains("castFinished = (resolved")
            && entity(node, &fixture.priest)["power"] == "70"
            && entity(node, &fixture.leader)["health"]
                .parse::<u32>()
                .unwrap()
                > before_direct
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
fn ally_buff_retains_its_target_through_range_repair_and_does_not_repeat() {
    let fixture = fixture("playerbots-roles-ally-buff", 10);
    let node = &fixture.node;
    node.assert_call(
        "playerbots_fixture_roles_prepare_fortitude",
        &[&fixture.priest],
    );
    node.assert_call(
        "playerbots_fixture_provision_steps",
        &[&fixture.priest, "1"],
    );
    assert!(known(node, &fixture.priest, 1243));
    for target in [&fixture.warrior, &fixture.priest, &fixture.mage] {
        node.assert_call(
            "playerbots_fixture_roles_stronger_fortitude",
            &[&fixture.leader, target],
        );
    }
    node.assert_call(
        "playerbots_fixture_roles_move",
        &[&fixture.leader, "1240", "1200"],
    );
    assert_eq!(
        node.query_rows("SELECT range_yd FROM game_spell WHERE spell_id = 1243")[0]["range_yd"],
        "30"
    );
    let started_x = entity(node, &fixture.priest)["x"].parse::<f32>().unwrap();
    assert!(poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.priest);
        let state = runner(node, &fixture.priest);
        state["chosen"].contains("buffPosition")
            && state["companion_buff_target_guid"].contains(&fixture.leader)
    }));
    evidence(&fixture, "fortitude-range-repair");
    node.assert_call(
        "playerbots_fixture_roles_buff_wall",
        &[&fixture.priest, &fixture.leader],
    );
    assert!(poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.priest);
        let state = runner(node, &fixture.priest);
        state["chosen"].contains("castingPosition")
            && state["companion_buff_target_guid"].contains(&fixture.leader)
            && node
                .query_rows(&format!(
                    "SELECT spell_id FROM game_aura WHERE target_guid = {} AND spell_id = 1243",
                    fixture.leader
                ))
                .is_empty()
    }));
    evidence(&fixture, "fortitude-los-repair");
    node.assert_call(
        "playerbots_fixture_runner_clear_navigation",
        &[&fixture.priest],
    );
    let completed = poll_until(POLL_TIMEOUT, || {
        pass(node, &fixture.priest);
        !node
            .query_rows(&format!(
                "SELECT spell_id FROM game_aura WHERE target_guid = {} AND spell_id = 1243",
                fixture.leader
            ))
            .is_empty()
    });
    evidence(&fixture, "fortitude-completed");
    assert!(completed);
    assert!(entity(node, &fixture.priest)["x"].parse::<f32>().unwrap() > started_x);
    let completed_state = runner(node, &fixture.priest);
    assert!(completed_state["cast_progress"].contains("spell = 1243"));
    assert!(completed_state["cast_progress"].contains(&fixture.leader));
    let casts = cast_events(node, &fixture.priest, 1243).len();
    pass(node, &fixture.priest);
    evidence(&fixture, "fortitude-repeat");
    assert_eq!(cast_events(node, &fixture.priest, 1243).len(), casts);
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn bounded_role_reads_record_typed_holds() {
    let fixture = fixture("playerbots-roles-read-limits", 10);
    let node = &fixture.node;
    node.assert_call(
        "playerbots_fixture_roles_prepare_fortitude",
        &[&fixture.priest],
    );
    node.assert_call(
        "playerbots_fixture_provision_steps",
        &[&fixture.priest, "1"],
    );

    for (kind, staged_guid, actor, case, expected) in [
        (
            0,
            &fixture.priest,
            &fixture.priest,
            "rotation-limit",
            "rotationLimit",
        ),
        (
            1,
            &fixture.warrior,
            &fixture.priest,
            "buff-aura-limit",
            "buffAuraLimit",
        ),
        (
            2,
            &fixture.warrior,
            &fixture.priest,
            "buff-family-unavailable",
            "buffFamilyUnavailable",
        ),
    ] {
        node.assert_call(
            "playerbots_fixture_roles_overflow",
            &[staged_guid, &kind.to_string()],
        );
        pass(node, actor);
        evidence(&fixture, case);
        let state = runner(node, actor);
        assert!(state["chosen"].contains("roleUnavailable"), "{state:?}");
        assert!(state["failures"].contains(expected), "{state:?}");
        node.assert_call("playerbots_fixture_roles_clear_overflow", &[]);
    }

    node.assert_call("playerbots_fixture_roles_priest_mana", &[&fixture.priest]);
    node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.leader, "40"],
    );
    node.assert_call(
        "playerbots_fixture_roles_overflow",
        &[&fixture.warrior, "1"],
    );
    pass(node, &fixture.priest);
    evidence(&fixture, "buff-aura-limit-heal");
    let state = runner(node, &fixture.priest);
    assert!(state["chosen"].contains("heal"), "{state:?}");
    assert!(
        state["companion_heal_target_guid"].contains(&fixture.leader),
        "{state:?}"
    );
    assert!(state["failures"].contains("buffAuraLimit"), "{state:?}");
    node.assert_call("playerbots_fixture_roles_clear_overflow", &[]);

    node.assert_call(
        "playerbots_fixture_roles_overflow",
        &[&fixture.warrior, "3"],
    );
    pass(node, &fixture.warrior);
    evidence(&fixture, "party-fight-limit");
    let state = runner(node, &fixture.warrior);
    assert!(state["chosen"].contains("partyUnavailable"), "{state:?}");
    assert!(state["failures"].contains("fightLimit"), "{state:?}");
    node.assert_call("playerbots_fixture_roles_clear_overflow", &[]);

    node.assert_call(
        "playerbots_fixture_roles_move",
        &[&fixture.warrior, "1100", "1200"],
    );
    node.assert_call(
        "playerbots_fixture_companion_health",
        &[&fixture.warrior, "50"],
    );
    node.assert_call("playerbots_fixture_runner_survival", &[&fixture.warrior]);
    node.assert_call(
        "playerbots_fixture_roles_overflow",
        &[&fixture.warrior, "0"],
    );
    pass(node, &fixture.warrior);
    evidence(&fixture, "rotation-limit-survival");
    let state = runner(node, &fixture.warrior);
    assert!(state["chosen"].contains("survival"), "{state:?}");
    assert!(state["failures"].contains("rotationLimit"), "{state:?}");
}

#[test]
#[ignore = "requires SpacetimeDB, Wasm, and the playerbots Package"]
fn a_stronger_buff_family_member_prevents_a_weaker_maintenance_cast() {
    let fixture = fixture("playerbots-roles-buff-family", 10);
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

fn sorted_catalog(node: &Standalone, table: &str) -> Vec<BTreeMap<String, String>> {
    let mut rows = node.query_rows(&format!("SELECT * FROM {table}"));
    rows.sort_by_key(|row| row["id"].parse::<u64>().unwrap());
    rows
}

#[test]
#[ignore = "requires the merged PB-006 Wasm, SpacetimeDB, and the playerbots Package"]
fn populated_pb006_state_upgrades_roles_without_replacing_operator_catalogue() {
    let preceding = preceding_roles();
    assert_ne!(
        blake3::hash(&preceding.wasm),
        blake3::hash(support::module_bytes())
    );

    let mut defaults = Standalone::start("playerbots-roles-pb006-default-migration");
    defaults.publish_module_bytes(&preceding.wasm);
    defaults.assert_call("claim_operator", &[]);
    defaults.assert_call("install_guid_range", &["1000000"]);
    defaults.assert_call("playerbots_spawn_role", &["1", "1200", "1200", "50", TANK]);
    let guid = defaults.query_rows("SELECT character_guid FROM pkg_playerbots_bot")[0]
        ["character_guid"]
        .clone();
    defaults.assert_call("playerbots_select_controller", &[&guid, "{\"cohort\":[]}"]);
    defaults.assert_call("debug_learn_spell", &[&guid, "355"]);
    defaults.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    defaults.assert_sql(&format!(
        "UPDATE pkg_playerbots_provisioning SET next_repair_micros = 9223372036854775807 WHERE character_guid = {guid}"
    ));
    defaults.assert_call("playerbots_fixture_runner_stage", &[&guid, "false"]);
    assert!(poll_until(POLL_TIMEOUT, || {
        defaults.assert_call("playerbots_fixture_runner_pass_once", &[&guid]);
        runner(&defaults, &guid)["foreground"].contains("some")
    }));
    defaults.assert_call("playerbots_fixture_freeze", &[&guid]);
    let preceding_runner = runner(&defaults, &guid);
    let preceding_provisioning = defaults.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"
    ))[0]
        .clone();
    let preceding_rotations = sorted_catalog(&defaults, "pkg_playerbots_rotation");
    let preceding_kit = sorted_catalog(&defaults, "pkg_playerbots_kit");
    let preceding_levels = defaults.query_rows(
        "SELECT spell_id, spell_level FROM game_spell WHERE spell_id = 139 OR spell_id = 355",
    );
    let preceding_known_taunt = known(&defaults, &guid, 355);

    let mut customized = Standalone::start("playerbots-roles-pb006-custom-migration");
    customized.publish_module_bytes(&preceding.wasm);
    customized.assert_call("claim_operator", &[]);
    customized.assert_call("playerbots_spawn", &["0", "1200", "1200", "50"]);
    customized.assert_sql(
        "UPDATE pkg_playerbots_rotation SET priority = 77 WHERE class = 8 AND role = 2 AND spell_id = 133",
    );
    customized.assert_sql("UPDATE game_spell SET range_yd = 20 WHERE spell_id = 355");
    let customized_rotations = sorted_catalog(&customized, "pkg_playerbots_rotation");
    let customized_kit = sorted_catalog(&customized, "pkg_playerbots_kit");

    defaults.publish_module();
    let migrated_runner = runner(&defaults, &guid);
    defaults.assert_call("playerbots_spawn", &["0", "1200", "1200", "50"]);
    defaults.assert_call("playerbots_fixture_provision_steps", &[&guid, "1"]);
    let upgraded_runner = runner(&defaults, &guid);
    let upgraded_provisioning = defaults.query_rows(&format!(
        "SELECT * FROM pkg_playerbots_provisioning WHERE character_guid = {guid}"
    ))[0]
        .clone();
    let upgraded_rotations = sorted_catalog(&defaults, "pkg_playerbots_rotation");
    let upgraded_kit = sorted_catalog(&defaults, "pkg_playerbots_kit");
    let upgraded_levels = defaults.query_rows(
        "SELECT spell_id, spell_level FROM game_spell WHERE spell_id = 139 OR spell_id = 355",
    );
    defaults.assert_call("playerbots_spawn", &["0", "1200", "1200", "50"]);
    let repeated_rotations = sorted_catalog(&defaults, "pkg_playerbots_rotation");
    let repeated_kit = sorted_catalog(&defaults, "pkg_playerbots_kit");

    customized.publish_module();
    customized.assert_call("playerbots_spawn", &["0", "1200", "1200", "50"]);
    let preserved_rotations = sorted_catalog(&customized, "pkg_playerbots_rotation");
    let preserved_kit = sorted_catalog(&customized, "pkg_playerbots_kit");
    let preserved_custom_header = customized
        .query_rows("SELECT spell_level, range_yd FROM game_spell WHERE spell_id = 355")[0]
        .clone();

    node_evidence(&defaults, "pb006-default-migration-current");
    node_evidence(&customized, "pb006-custom-migration-current");
    let path = support::log_dir().join(format!(
        "{}-pb006-populated-migration.json",
        defaults.shard_name()
    ));
    std::fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "preceding_build": preceding.manifest,
            "preceding_runner": preceding_runner,
            "migrated_runner_before_any_pass": migrated_runner,
            "upgraded_runner": upgraded_runner,
            "preceding_provisioning": preceding_provisioning,
            "upgraded_provisioning": upgraded_provisioning,
            "preceding_rotations": preceding_rotations,
            "upgraded_rotations": upgraded_rotations,
            "preceding_kit": preceding_kit,
            "upgraded_kit": upgraded_kit,
            "preceding_curated_spell_levels": preceding_levels,
            "upgraded_curated_spell_levels": upgraded_levels,
            "preceding_level_five_knows_taunt": preceding_known_taunt,
            "upgraded_level_five_knows_taunt": known(&defaults, &guid, 355),
            "customized_rotations": customized_rotations,
            "preserved_rotations": preserved_rotations,
            "customized_kit": customized_kit,
            "preserved_kit": preserved_kit,
            "preserved_non_curated_header": preserved_custom_header,
            "current_wasm_blake3": blake3::hash(support::module_bytes()).to_hex().to_string(),
        }))
        .unwrap(),
    )
    .unwrap();

    assert!(preceding_runner["objective"].contains("some"));
    assert!(preceding_runner["foreground"].contains("some"));
    for field in ["objective_sequence", "objective", "foreground", "chosen"] {
        assert_eq!(
            migrated_runner[field], preceding_runner[field],
            "runner field {field}"
        );
        assert_eq!(
            upgraded_runner[field], preceding_runner[field],
            "runner field {field}"
        );
    }
    assert!(migrated_runner["companion_fight_target_guid"].contains("none"));
    assert!(migrated_runner["companion_buff_target_guid"].contains("none"));
    assert_eq!(preceding_provisioning["revision"], "1");
    assert!(!preceding_provisioning["history"].is_empty());
    assert_eq!(upgraded_provisioning["revision"], "2");
    assert!(upgraded_provisioning["cause"].contains("periodic"));
    assert!(upgraded_provisioning["history"].contains(preceding_provisioning["history"].as_str()));
    assert!(preceding_levels.iter().all(|row| row["spell_level"] == "0"));
    assert!(preceding_known_taunt);
    assert!(known(&defaults, &guid, 355));
    for (spell_id, spell_level) in [("139", "8"), ("355", "10")] {
        assert!(upgraded_levels
            .iter()
            .any(|row| { row["spell_id"] == spell_id && row["spell_level"] == spell_level }));
    }

    assert_eq!(upgraded_rotations.len(), preceding_rotations.len() + 4);
    assert_eq!(upgraded_kit.len(), preceding_kit.len() + 4);
    assert!(preceding_rotations
        .iter()
        .all(|row| upgraded_rotations.contains(row)));
    assert!(preceding_kit.iter().all(|row| upgraded_kit.contains(row)));
    for (class, role, priority, spell, condition) in [
        ("1", TANK, "20", "6673", "2"),
        (PRIEST, HEALER, "10", "585", "0"),
        (PRIEST, HEALER, "20", "1243", "4"),
        (MAGE, DAMAGE, "20", "168", "2"),
    ] {
        assert!(upgraded_rotations.iter().any(|row| {
            row["class"] == class
                && row["role"] == role
                && row["priority"] == priority
                && row["spell_id"] == spell
                && row["condition"] == condition
                && row["threshold_pct"] == "0"
        }));
        assert!(upgraded_kit.iter().any(|row| {
            row["class"] == class && row["role"] == role && row["spell_id"] == spell
        }));
    }
    assert_eq!(repeated_rotations, upgraded_rotations);
    assert_eq!(repeated_kit, upgraded_kit);
    assert_eq!(preserved_rotations, customized_rotations);
    assert_eq!(preserved_kit, customized_kit);
    assert_eq!(preserved_custom_header["spell_level"], "0");
    assert_eq!(preserved_custom_header["range_yd"], "20");
}
