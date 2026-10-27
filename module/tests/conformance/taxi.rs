use lyracore_test_support::Standalone;
use serde_json::json;

fn fixture(name: &str) -> (Standalone, u64) {
    let (shard, guid, _) = super::fixture(name);
    shard.assert_sql(
        "INSERT INTO game_taxi_node \
         (id,client_node_id,map_id,x,y,z,name,mount_display_horde,mount_display_alliance) \
         VALUES (5090099,99,0,0,0,0,'Discovery fixture',0,0)",
    );
    (shard, guid)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn learning_a_taxi_node_requires_the_operator() {
    let (shard, guid) = fixture("taxi-learn-operator");
    let mut anonymous = shard.connect_reducers_anonymous(&[]);
    let refusal = anonymous
        .call("debug_learn_taxi_node", json!([guid, 5090099]))
        .unwrap_err();
    assert!(refusal.contains("operator"), "{refusal}");
    assert!(shard
        .query_rows("SELECT * FROM game_character_taxi_node")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn learning_a_taxi_node_requires_a_character_and_an_existing_node() {
    let (shard, guid) = fixture("taxi-learn-input");
    let mut connection = shard.connect_reducers(&[]);
    let refusal = connection
        .call("debug_learn_taxi_node", json!([u64::MAX, 5090099]))
        .unwrap_err();
    assert!(refusal.contains("no character"), "{refusal}");
    let refusal = connection
        .call("debug_learn_taxi_node", json!([guid, 99]))
        .unwrap_err();
    assert!(refusal.contains("taxi node 99 does not exist"), "{refusal}");
    assert!(shard
        .query_rows("SELECT * FROM game_character_taxi_node")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn learning_a_taxi_node_is_idempotent_without_flight_or_packets() {
    let (shard, guid) = fixture("taxi-learn-state");
    let mut connection = super::enter(&shard, guid);
    connection.drain_packets();
    let character = shard.query_rows("SELECT * FROM game_character");
    let entity_query = format!("SELECT * FROM game_world_entity WHERE guid = {guid}");
    let entity = shard.query_rows(&entity_query);
    for _ in 0..2 {
        let transaction = connection
            .call("debug_learn_taxi_node", json!([guid, 5090099]))
            .unwrap();
        assert!(transaction.inserts("game_native_packet").is_empty());
    }
    let known = shard.query_rows("SELECT * FROM game_character_taxi_node");
    assert_eq!(known.len(), 1);
    assert_eq!(known[0]["character_guid"], guid.to_string());
    assert_eq!(known[0]["node_id"], "5090099");
    for table in [
        "game_active_taxi_flight",
        "game_taxi_flight_schedule",
        "game_taxi_passenger_spline",
        "game_taxi_service_reply",
    ] {
        assert!(
            shard
                .query_rows(&format!("SELECT * FROM {table}"))
                .is_empty(),
            "{table}"
        );
    }
    assert_eq!(shard.query_rows("SELECT * FROM game_character"), character);
    assert_eq!(shard.query_rows(&entity_query), entity);
}
