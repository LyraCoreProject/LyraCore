mod support;

use lyracore_package_delta::fixture_ids::FLIGHT_MASTER_ENTRY;
use support::{fence_subscription, subscribe_with_fence, Standalone};

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn spawn_commands_keep_quest_flags_when_clearing_immunity() {
    let mut shard = Standalone::start("scripted-immunity");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql(
        &format!("UPDATE game_creature_template SET npc_flags = 128, unit_flags = 256 WHERE entry = {FLIGHT_MASTER_ENTRY}"),
    );
    let rules = "1,spawn,100,4294967295,once,all,ordinary,npc-flags:2:add+unit-flags:clear:immune-to-players+faction:35";
    let mut hash = blake3::Hasher::new();
    hash.update(b"lyracore-eventai-definition-v1");
    hash.update(format!("entry:{FLIGHT_MASTER_ENTRY}@{rules}").as_bytes());
    let revision = u64::from_le_bytes(hash.finalize().as_bytes()[..8].try_into().unwrap());
    shard.assert_call(
        "import_creature_ai_definitions",
        &[
            &serde_json::to_string(&format!("entry:{FLIGHT_MASTER_ENTRY}@{revision}@{rules}"))
                .unwrap(),
        ],
    );
    let subscription = subscribe_with_fence(&shard, &["SELECT * FROM game_world_entity"]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &FLIGHT_MASTER_ENTRY.to_string(), "2"],
    );
    let rows = shard
        .query_rows(&format!("SELECT guid,npc_flags,unit_flags FROM game_world_entity WHERE entry = {FLIGHT_MASTER_ENTRY}"));
    let row = rows
        .iter()
        .max_by_key(|row| row["guid"].parse::<u64>().unwrap())
        .unwrap();
    assert_eq!(row["npc_flags"], "130");
    assert_eq!(row["unit_flags"].parse::<u32>().unwrap() & 256, 0);
    fence_subscription(&shard, &subscription);
    let guid = row["guid"].parse::<u64>().unwrap();
    let delivered: Vec<_> = subscription
        .inserts("game_world_entity")
        .into_iter()
        .filter(|entity| entity["guid"] == guid)
        .collect();
    assert!(!delivered.is_empty());
    assert!(delivered
        .iter()
        .all(|entity| entity["unit_flags"].as_u64().unwrap() & 256 == 0));
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT unit_flags FROM game_creature_template WHERE entry = {FLIGHT_MASTER_ENTRY}"
        ))[0]["unit_flags"],
        "256"
    );
}
