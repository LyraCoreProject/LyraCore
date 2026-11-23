mod support;

use lyracore_shared::constants::player_flags;
use lyracore_shared::spell::taxonomy::*;
use support::{number, Row, Standalone};

const DEADMINES_GHOST: u64 = 1;

const DEADMINES: u32 = 36;
const RAID_MAP: u32 = 409;
const TO_DEADMINES: u32 = 5_340_001;
const TO_RAID: u32 = 5_340_002;
const GHOST_SPELL: u32 = 8326;

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

fn create_character(shard: &Standalone, name: &str) -> u64 {
    shard.assert_call(
        "create_character",
        &[
            "1",
            &format!("\"{name}\""),
            "1",
            "1",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    shard.query_rows(&format!(
        "SELECT guid FROM game_character WHERE name = '{name}'"
    ))[0]["guid"]
        .parse()
        .unwrap()
}

fn insert_ghost_spell(shard: &Standalone) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &GHOST_SPELL),
            ("name", &"Ghost"),
            ("duration_ms", &4294967295u64),
            ("school_mask", &1),
        ],
    );
    for (index, (kind, amount, parameter, parameter_kind)) in [
        (A_FLAG, -20, 0, P_FLAG),
        (A_MOD_SPEED, 25, i32::from(SPEED_MOVE), P_SPEED_KIND),
        (A_MOD_SPEED, 25, i32::from(SPEED_SWIM), P_SPEED_KIND),
    ]
    .into_iter()
    .enumerate()
    {
        shard.insert_row(
            "game_spell_effect",
            &[
                ("id", &(u64::from(GHOST_SPELL) * 4 + index as u64)),
                ("spell_id", &GHOST_SPELL),
                ("effect_index", &index),
                ("kind", &kind),
                ("base_points", &amount),
                ("die_sides", &1),
                ("p_0", &parameter),
                ("p_0_kind", &parameter_kind),
            ],
        );
    }
}

fn has_ghost_aura(shard: &Standalone, guid: u64) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT spell_id FROM game_aura WHERE target_guid = {guid} AND spell_id = {GHOST_SPELL}"
        ))
        .is_empty()
}

fn release_with_corpse_on(shard: &Standalone, guid: u64, corpse_map: u32, is_bones: bool) -> u32 {
    let max_health = number(&entity(shard, guid), "max_health");
    shard.assert_call("debug_set_health", &[&guid.to_string(), "0"]);
    shard.assert_call("debug_repop", &[&guid.to_string()]);
    assert_eq!(entity(shard, guid)["dead"], "true");
    assert!(has_ghost_aura(shard, guid));
    shard.assert_sql(&format!(
        "UPDATE game_corpse SET map_id = {corpse_map}, is_bones = {is_bones} WHERE owner_guid = {guid}"
    ));
    max_health
}

fn has_resurrectable_corpse(shard: &Standalone, guid: u64) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT guid FROM game_corpse WHERE owner_guid = {guid} AND is_bones = false"
        ))
        .is_empty()
}

fn assert_revived_inside_deadmines(shard: &Standalone, guid: u64, max_health: u32) {
    let entered = character(shard, guid);
    assert_eq!(entered["map_id"], DEADMINES.to_string());
    assert_eq!(entered["pending_ghost"], "false");
    assert_eq!(number::<u32>(&entered, "health"), max_health / 2);
    assert!(!has_resurrectable_corpse(shard, guid));
}

fn assert_revived_outside(shard: &Standalone, guid: u64, max_health: u32) {
    let refused = entity(shard, guid);
    assert_eq!(refused["map_id"], "0", "a solo Character stays outside");
    assert_eq!(refused["dead"], "false", "the refused ghost is revived");
    assert_eq!(
        number::<u32>(&refused, "player_flags") & player_flags::GHOST,
        0
    );
    assert!(!has_ghost_aura(shard, guid));
    let health: u32 = number(&refused, "health");
    assert!(
        (max_health / 2..max_health).contains(&health),
        "{health} of {max_health}"
    );
    assert!(!has_resurrectable_corpse(shard, guid));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_ghost_revives_at_the_entrance_of_the_map_holding_its_corpse() {
    let mut shard = Standalone::start("ghost-instance-entry");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("set_gm_level", &["\"Tester\"", "0"]);
    let raid_ghost = create_character(&shard, "RaidGhost");
    let teleported_ghost = create_character(&shard, "TeleportedGhost");
    for guid in [DEADMINES_GHOST, raid_ghost, teleported_ghost] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    }
    insert_ghost_spell(&shard);
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

    let max_health = release_with_corpse_on(&shard, DEADMINES_GHOST, DEADMINES, true);
    shard.assert_call(
        "debug_enter_areatrigger",
        &[&DEADMINES_GHOST.to_string(), &TO_DEADMINES.to_string()],
    );
    assert_revived_inside_deadmines(&shard, DEADMINES_GHOST, max_health);

    let max_health = release_with_corpse_on(&shard, raid_ghost, RAID_MAP, false);
    shard.assert_call(
        "debug_enter_areatrigger",
        &[&raid_ghost.to_string(), &TO_RAID.to_string()],
    );
    assert_revived_outside(&shard, raid_ghost, max_health);

    let max_health = release_with_corpse_on(&shard, teleported_ghost, DEADMINES, false);
    shard.assert_call(
        "debug_teleport",
        &[
            &teleported_ghost.to_string(),
            &DEADMINES.to_string(),
            "10",
            "20",
            "30",
            "0",
        ],
    );
    assert_revived_inside_deadmines(&shard, teleported_ghost, max_health);
}
