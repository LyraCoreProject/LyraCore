mod support;

use lyracore_shared::pet::{pet_guid_for, PET_NAME_INVALID};
use support::{actor, number, poll_until, Row, Standalone, POLL_TIMEOUT};

const HUNTER: &str = "1";
const BOAR: &str = "51006";
const TAME: &str = "50300";
/// `UNIT_FLAG_PET_RENAME`: the 1.12 client offers "Rename Pet" while the live pet carries it.
const PET_RENAME_FLAG: u32 = 0x10;

fn wild_boar(shard: &Standalone) -> String {
    let before = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51006 AND owner_guid = 0");
    shard.assert_call("debug_spawn_at_feet", &[HUNTER, BOAR, "5"]);
    let rows = shard
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51006 AND owner_guid = 0");
    let added: Vec<_> = rows
        .into_iter()
        .filter(|row| !before.contains(row))
        .collect();
    assert_eq!(added.len(), 1);
    added[0]["guid"].clone()
}

fn hunter(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_sql("UPDATE game_creature_template SET creature_family = 5, level = 5, health = 100, npc_flags = 0 WHERE entry = 51006");
    shard.assert_sql("UPDATE game_character SET class = 3, level = 10 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &[HUNTER]);
    shard.assert_call("debug_set_level", &[HUNTER, "60"]);
    shard
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn tame_dismiss_and_retame_preserve_one_durable_identity_and_clean_live_state() {
    let shard = hunter("hunter-pet-lifetime");
    let wild = wild_boar(&shard);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    let pet = pet_guid_for(1).to_string();
    assert!(shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE guid = {wild}"
        ))
        .is_empty());
    let live = shard.query_rows(&format!(
        "SELECT owner_guid, entry FROM game_world_entity WHERE guid = {pet}"
    ));
    assert_eq!(live.len(), 1);
    assert_eq!(live[0]["owner_guid"], HUNTER);
    assert_eq!(live[0]["entry"], BOAR);
    let identities =
        shard.query_rows("SELECT pet_id, creature_entry FROM game_hunter_pet WHERE owner_guid = 1");
    assert_eq!(identities.len(), 1);
    assert_eq!(identities[0]["pet_id"], HUNTER);
    let spawn = shard.query_rows(&format!(
        "SELECT respawn_at, despawn_at FROM game_creature_spawn WHERE guid = {wild}"
    ));
    assert_eq!(spawn.len(), 1, "taming preserves the authored spawn");
    let respawn = spacetimedb::Timestamp::parse_from_rfc3339(&spawn[0]["respawn_at"])
        .expect("respawn timestamp")
        .to_micros_since_unix_epoch();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64;
    assert!(
        (now..now + 600_000_000).contains(&respawn),
        "taming rearms a finite future respawn"
    );
    let despawn = spacetimedb::Timestamp::parse_from_rfc3339(&spawn[0]["despawn_at"])
        .expect("despawn timestamp")
        .to_micros_since_unix_epoch();
    assert!(
        despawn > now + 100 * 365 * 86_400 * 1_000_000,
        "taming disarms the despawn timer"
    );
    let protocol =
        shard.query_rows("SELECT live_pet_guid FROM game_hunter_pet_protocol WHERE owner_guid = 1");
    assert_eq!(
        number::<u64>(&protocol[0], "live_pet_guid"),
        pet_guid_for(1)
    );

    shard.assert_call("debug_pet_command", &[HUNTER, "117440515", "0"]);
    for table in [
        "game_world_entity",
        "game_entity_motion",
        "game_live_pet_kind",
    ] {
        let key = if table == "game_live_pet_kind" {
            "pet_guid"
        } else {
            "guid"
        };
        assert!(
            shard
                .query_rows(&format!("SELECT * FROM {table} WHERE {key} = {pet}"))
                .is_empty(),
            "{table} retains the dismissed pet"
        );
    }
    assert!(shard
        .query_rows("SELECT * FROM game_pet_command WHERE owner_guid = 1")
        .is_empty());
    assert_eq!(
        shard.query_rows("SELECT pet_id, creature_entry FROM game_hunter_pet WHERE owner_guid = 1"),
        identities
    );

    let replacement = wild_boar(&shard);
    assert!(
        poll_until(POLL_TIMEOUT, || shard
            .call("debug_force_cast_at", &[HUNTER, TAME, &replacement])
            .status
            .success()),
        "retaming must succeed after the global cooldown"
    );
    assert_eq!(
        shard.query_rows("SELECT pet_id, creature_entry FROM game_hunter_pet WHERE owner_guid = 1"),
        identities
    );
    assert_eq!(
        shard.query_rows("SELECT * FROM game_live_pet_kind").len(),
        1
    );
    let protocol =
        shard.query_rows("SELECT live_pet_guid FROM game_hunter_pet_protocol WHERE owner_guid = 1");
    assert_eq!(
        number::<u64>(&protocol[0], "live_pet_guid"),
        pet_guid_for(1)
    );
}

fn identity(shard: &Standalone) -> Row {
    shard
        .query_rows(
            "SELECT name, rename_allowed, name_timestamp FROM game_hunter_pet WHERE owner_guid = 1",
        )
        .remove(0)
}

fn protocol_name(shard: &Standalone) -> (String, u32) {
    let row = shard
        .query_rows(
            "SELECT name, name_timestamp FROM game_hunter_pet_protocol WHERE owner_guid = 1",
        )
        .remove(0);
    (row["name"].clone(), number(&row, "name_timestamp"))
}

fn live_pet_flags(shard: &Standalone) -> u32 {
    let row = shard
        .query_rows(&format!(
            "SELECT unit_flags FROM game_world_entity WHERE guid = {}",
            pet_guid_for(1)
        ))
        .remove(0);
    number(&row, "unit_flags")
}

fn rename(shard: &Standalone, owner: &str, pet_guid: u64, name: &str) -> std::process::Output {
    shard.call(
        "gw_pet_rename",
        &[
            &actor(owner),
            &pet_guid.to_string(),
            &serde_json::to_string(name).unwrap(),
        ],
    )
}

fn assert_refused(output: std::process::Output, reason: &str) {
    assert!(!output.status.success(), "the rename must be refused");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(reason), "expected {reason:?} in {stderr}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hunter_renames_its_own_pet_once_and_the_name_outlives_logout() {
    let shard = hunter("hunter-pet-rename");
    let wild = wild_boar(&shard);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    let pet = pet_guid_for(1);
    let template_name = shard
        .query_rows("SELECT name FROM game_creature_template WHERE entry = 51006")
        .remove(0)["name"]
        .clone();
    let tamed = identity(&shard);
    assert_eq!(tamed["name"], template_name);
    assert_eq!(tamed["rename_allowed"], "true");
    let tamed_at: u32 = number(&tamed, "name_timestamp");
    assert!(tamed_at > 0, "taming stamps the name");
    assert_eq!(protocol_name(&shard), (template_name.clone(), tamed_at));
    assert_ne!(live_pet_flags(&shard) & PET_RENAME_FLAG, 0);

    for (pet_guid, name, reason) in [
        (wild.parse().unwrap(), "Fang", "has no live pet"),
        (pet, "Fang 2", PET_NAME_INVALID),
        (pet, "F", PET_NAME_INVALID),
        (pet, "Fangfangfangf", PET_NAME_INVALID),
    ] {
        assert_refused(rename(&shard, HUNTER, pet_guid, name), reason);
        assert_eq!(identity(&shard), tamed, "{name:?} changed the identity");
        assert_eq!(protocol_name(&shard), (template_name.clone(), tamed_at));
        assert_ne!(live_pet_flags(&shard) & PET_RENAME_FLAG, 0);
    }

    assert!(rename(&shard, HUNTER, pet, "Fang").status.success());
    let renamed = identity(&shard);
    assert_eq!(renamed["name"], "Fang");
    assert_eq!(renamed["rename_allowed"], "false");
    let renamed_at: u32 = number(&renamed, "name_timestamp");
    assert!(
        renamed_at > tamed_at,
        "the client asks again only for a new stamp"
    );
    assert_eq!(protocol_name(&shard), ("Fang".to_string(), renamed_at));
    assert_eq!(live_pet_flags(&shard) & PET_RENAME_FLAG, 0);

    assert_refused(rename(&shard, HUNTER, pet, "Claw"), "already renamed");
    assert_eq!(identity(&shard), renamed);

    shard.assert_call("debug_logout_character", &[HUNTER]);
    assert!(shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE guid = {pet}"
        ))
        .is_empty());
    assert_eq!(identity(&shard), renamed);
    assert_eq!(protocol_name(&shard), ("Fang".to_string(), renamed_at));
    assert_refused(rename(&shard, HUNTER, pet, "Claw"), "not in world");
    assert_eq!(identity(&shard), renamed);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_summoned_pet_cannot_be_renamed_as_a_hunter_pet() {
    use lyracore_shared::spell::taxonomy::{E_SUMMON_PET, T_SELF};

    let shard = hunter("summoned-pet-rename");
    shard.assert_sql(&format!(
        "UPDATE game_spell SET cast_flags = 0, duration_ms = 0 WHERE spell_id = {TAME}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_spell_effect SET kind = {E_SUMMON_PET}, target = {T_SELF}, p0 = {BOAR} WHERE spell_id = {TAME}"
    ));
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, HUNTER]);
    let pet = pet_guid_for(1);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT hunter_pet_id FROM game_live_pet_kind WHERE pet_guid = {pet}"
        ))[0]["hunter_pet_id"],
        "0"
    );
    for name in ["Fang", "Fang 2"] {
        assert_refused(rename(&shard, HUNTER, pet, name), "not a Hunter pet");
        assert!(shard.query_rows("SELECT * FROM game_hunter_pet").is_empty());
        assert!(shard
            .query_rows("SELECT * FROM game_hunter_pet_protocol")
            .is_empty());
        assert_eq!(
            shard.query_rows(&format!(
                "SELECT owner_guid FROM game_world_entity WHERE guid = {pet}"
            ))[0]["owner_guid"],
            HUNTER
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn existing_live_pet_recovers_rename_eligibility_without_losing_other_flags() {
    let shard = hunter("hunter-pet-name-upgrade");
    let wild = wild_boar(&shard);
    shard.assert_call("debug_force_cast_at", &[HUNTER, TAME, &wild]);
    let pet = pet_guid_for(1);
    let original_name = protocol_name(&shard);
    shard.assert_sql("UPDATE game_hunter_pet SET name_timestamp = 0 WHERE owner_guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, unit_flags = 512 WHERE guid = {pet}"
    ));
    assert!(poll_until(POLL_TIMEOUT, || live_pet_flags(&shard)
        & PET_RENAME_FLAG
        != 0));
    assert_ne!(live_pet_flags(&shard) & 512, 0);
    assert_eq!(protocol_name(&shard), original_name);

    assert!(rename(&shard, HUNTER, pet, "Fang").status.success());
    let renamed_name = protocol_name(&shard);
    assert!(renamed_name.1 > original_name.1);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, unit_flags = 528 WHERE guid = {pet}"
    ));
    assert!(poll_until(POLL_TIMEOUT, || live_pet_flags(&shard)
        & PET_RENAME_FLAG
        == 0));
    assert_ne!(live_pet_flags(&shard) & 512, 0);
    assert_eq!(protocol_name(&shard), renamed_name);
}
