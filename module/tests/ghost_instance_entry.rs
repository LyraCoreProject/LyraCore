mod support;

use support::{number, Row, Standalone};

const DEADMINES_GHOST: u64 = 1;

const DEADMINES: u32 = 36;
const RAID_MAP: u32 = 409;
const TO_DEADMINES: u32 = 5_340_001;
const TO_RAID: u32 = 5_340_002;

fn entity(shard: &Standalone, guid: u64) -> Row {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {guid}"
        ))
        .remove(0)
}

fn character(shard: &Standalone, guid: u64) -> Row {
    shard
        .query_rows(&format!("SELECT * FROM game_character WHERE guid = {guid}"))
        .remove(0)
}

fn release_with_corpse_on(shard: &Standalone, guid: u64, corpse_map: u32) -> u32 {
    let max_health = number(&entity(shard, guid), "max_health");
    shard.assert_call("debug_set_health", &[&guid.to_string(), "0"]);
    shard.assert_call("debug_repop", &[&guid.to_string()]);
    assert_eq!(entity(shard, guid)["dead"], "true");
    shard.assert_sql(&format!(
        "UPDATE game_corpse SET map_id = {corpse_map} WHERE owner_guid = {guid}"
    ));
    max_health
}

fn has_corpse(shard: &Standalone, guid: u64) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT guid FROM game_corpse WHERE owner_guid = {guid}"
        ))
        .is_empty()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ghost_revives_at_the_entrance_of_the_map_holding_its_corpse() {
    let mut shard = Standalone::start("ghost-instance-entry");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("set_gm_level", &["\"Tester\"", "0"]);
    shard.assert_call(
        "create_character",
        &["1", "\"RaidGhost\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let raid_ghost: u64 = shard
        .query_rows("SELECT guid FROM game_character WHERE name = 'RaidGhost'")[0]["guid"]
        .parse()
        .unwrap();
    for guid in [DEADMINES_GHOST, raid_ghost] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    }
    for (map, map_type) in [(RAID_MAP, 2), (DEADMINES, 1)] {
        shard.insert_row(
            "game_map",
            &[
                ("id", &map),
                ("internal_name", &"Fixture"),
                ("name", &"Fixture"),
                ("map_type", &map_type),
            ],
        );
    }
    for (trigger, target_map) in [(TO_DEADMINES, DEADMINES), (TO_RAID, RAID_MAP)] {
        shard.insert_row(
            "game_area_trigger",
            &[
                ("id", &trigger),
                ("map_id", &0),
                ("x", &0.0f32),
                ("y", &0.0f32),
                ("z", &0.0f32),
                ("radius", &5.0f32),
            ],
        );
        shard.insert_row(
            "game_areatrigger_teleport",
            &[
                ("trigger_id", &trigger),
                ("target_map", &target_map),
                ("x", &10.0f32),
                ("y", &20.0f32),
                ("z", &30.0f32),
                ("o", &0.0f32),
                ("name", &"Fixture entrance"),
            ],
        );
    }

    let max_health = release_with_corpse_on(&shard, DEADMINES_GHOST, DEADMINES);
    shard.assert_call(
        "debug_enter_areatrigger",
        &[&DEADMINES_GHOST.to_string(), &TO_DEADMINES.to_string()],
    );
    let entered = character(&shard, DEADMINES_GHOST);
    assert_eq!(entered["map_id"], DEADMINES.to_string());
    assert_eq!(entered["pending_ghost"], "false");
    assert_eq!(number::<u32>(&entered, "health"), max_health / 2);
    assert!(!has_corpse(&shard, DEADMINES_GHOST));

    let max_health = release_with_corpse_on(&shard, raid_ghost, RAID_MAP);
    shard.assert_call(
        "debug_enter_areatrigger",
        &[&raid_ghost.to_string(), &TO_RAID.to_string()],
    );
    let refused = entity(&shard, raid_ghost);
    assert_eq!(refused["map_id"], "0", "a solo Character stays outside");
    assert_eq!(refused["dead"], "false", "the refused ghost is revived");
    let health: u32 = number(&refused, "health");
    assert!(
        (max_health / 2..max_health).contains(&health),
        "{health} of {max_health}"
    );
    assert!(!has_corpse(&shard, raid_ghost));
}
