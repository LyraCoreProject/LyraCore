mod support;

use support::{
    event_rows, fence_subscription, lone_wolf, number, poll_until, subscribe_with_fence,
    Standalone, POLL_TIMEOUT,
};

#[test]
#[ignore = "requires a healthy machine with the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn migration_defaults_are_repaired_before_recovery_outside_active_cells() {
    let mut shard = Standalone::start("simulation-backfill");
    let wolf = lone_wolf(&mut shard);
    let guid = number::<u64>(&wolf, "guid").to_string();
    let rage = u32::from(lyracore_shared::packing::power_type::RAGE) << 24;
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 1, max_health = 100, power = 100, \
         max_power = 100, unit_bytes_0 = {rage}, unit_flags = 0, \
         regeneration_pending = false, combat_active = false, simulation_indexed = false \
         WHERE guid = {guid}"
    ));

    assert!(poll_until(POLL_TIMEOUT, || {
        let rows = shard.query_rows(&format!(
            "SELECT health, power, simulation_indexed, regeneration_pending \
             FROM game_world_entity WHERE guid = {guid}"
        ));
        let row = &rows[0];
        row["simulation_indexed"] == "true"
            && row["regeneration_pending"] == "true"
            && number::<u32>(row, "health") > 1
            && number::<u32>(row, "power") < 100
    }));

    shard.assert_call("debug_set_health", &[&guid, "100"]);
    shard.assert_call("debug_set_power", &[&guid, "0"]);
    let rows = shard.query_rows(&format!(
        "SELECT regeneration_pending, simulation_indexed FROM game_world_entity WHERE guid = {guid}"
    ));
    assert_eq!(rows[0]["simulation_indexed"], "true");
    assert_eq!(rows[0]["regeneration_pending"], "false");
}

#[test]
#[ignore = "requires a healthy machine with the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn backfill_recovers_combat_flags_without_restarting_the_combat_deadline() {
    let mut shard = Standalone::start("simulation-combat-backfill");
    let wolf = lone_wolf(&mut shard);
    let guid = number::<u64>(&wolf, "guid").to_string();
    let in_combat = lyracore_shared::constants::unit_flags::IN_COMBAT;
    let updates = subscribe_with_fence(
        &shard,
        &[&format!(
            "SELECT * FROM game_world_entity WHERE guid = {guid}"
        )],
    );
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {in_combat}, combat_until_ms = 0, \
         combat_active = false, simulation_indexed = false WHERE guid = {guid}"
    ));
    assert!(poll_until(POLL_TIMEOUT, || {
        let rows = shard.query_rows(&format!(
            "SELECT unit_flags, combat_active, simulation_indexed, combat_until_ms \
             FROM game_world_entity WHERE guid = {guid}"
        ));
        let row = &rows[0];
        row["simulation_indexed"] == "true"
            && row["combat_active"] == "false"
            && number::<u32>(row, "unit_flags") & in_combat == 0
            && number::<u64>(row, "combat_until_ms") == 0
    }));
    fence_subscription(&shard, &updates);
    let rows = event_rows(&updates, "game_world_entity");
    assert!(!rows.is_empty());
    for row in rows {
        assert_eq!(
            row["combat_until_ms"], "0",
            "the deadline restarted: {row:?}"
        );
    }
}

#[test]
#[ignore = "requires a healthy machine with the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn backfill_batches_pause_regeneration_until_every_entity_is_indexed() {
    let mut shard = Standalone::start("simulation-batched-backfill");
    let wolf = lone_wolf(&mut shard);
    let packed = (1..=512)
        .map(|index| {
            let guid = (0xF130u64 << 48) | (51000u64 << 24) | (1_000_000 + index);
            format!(
                "{guid},51000,{},{},{},{},0,0",
                wolf["map_id"], wolf["x"], wolf["y"], wolf["z"]
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    shard.assert_call(
        "import_creature_spawns_append",
        &[&serde_json::to_string(&packed).unwrap()],
    );
    assert!(poll_until(POLL_TIMEOUT, || {
        shard
            .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000")
            .len()
            == 513
    }));
    shard.assert_sql("DELETE FROM game_creature_move_schedule");
    shard.assert_sql("DELETE FROM game_creature_spawn");
    shard.assert_sql("DELETE FROM game_world_entity WHERE entry != 51000");
    assert_eq!(
        shard.query_rows("SELECT guid FROM game_world_entity").len(),
        513
    );

    shard.assert_call("debug_arm_instance_tick", &["0", "4000"]);
    shard.assert_sql(&format!(
        "UPDATE game_creature_move_schedule SET instance_id = {}",
        u64::MAX
    ));
    let updates = shard.subscribe(&["SELECT * FROM game_world_entity"]);
    shard.assert_sql(
        "UPDATE game_world_entity SET health = 1, max_health = 100, power = 0, max_power = 0, \
         unit_flags = 0, regeneration_pending = false, combat_active = false, \
         simulation_indexed = false",
    );
    assert!(poll_until(POLL_TIMEOUT, || {
        shard
            .query_rows("SELECT health, simulation_indexed FROM game_world_entity")
            .iter()
            .all(|row| row["simulation_indexed"] == "true" && number::<u32>(row, "health") > 1)
    }));
    assert!(poll_until(POLL_TIMEOUT, || {
        updates
            .inserts("game_world_entity")
            .iter()
            .any(|row| row["simulation_indexed"] == true && row["health"].as_u64().unwrap() > 1)
    }));
    let first_batch: Vec<_> = updates
        .inserts("game_world_entity")
        .into_iter()
        .filter(|row| row["simulation_indexed"] == true)
        .take(512)
        .collect();
    assert_eq!(first_batch.len(), 512);
    assert!(first_batch.iter().all(|row| row["health"] == 1));
}
