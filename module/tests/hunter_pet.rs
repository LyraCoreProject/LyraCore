mod support;

use lyracore_shared::pet::pet_guid_for;
use support::{number, poll_until, Standalone, POLL_TIMEOUT};

const HUNTER: &str = "1";
const BOAR: &str = "51006";
const TAME: &str = "50300";

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

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn tame_dismiss_and_retame_preserve_one_durable_identity_and_clean_live_state() {
    let mut shard = Standalone::start("hunter-pet-lifetime");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    // Entry 51006 also seeds a Flight Master. This private fixture supplies the tameable template.
    shard.assert_sql("UPDATE game_creature_template SET creature_family = 5, level = 5, health = 100, npc_flags = 0 WHERE entry = 51006");
    shard.assert_sql("UPDATE game_character SET class = 3, level = 10 WHERE guid = 1");
    shard.assert_call("debug_spawn_player_entity", &[HUNTER]);
    shard.assert_call("debug_set_level", &[HUNTER, "60"]);
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
