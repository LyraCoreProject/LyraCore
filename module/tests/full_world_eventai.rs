mod support;

use std::time::{Duration, Instant};
use support::{event_rows, poll_until, Standalone, POLL_TIMEOUT};

const ENTRY: u32 = 51_000;
const CREATURE: u64 = (0xF130_u64 << 48) | ((ENTRY as u64) << 24) | 1;

fn definition(enabled: bool) -> String {
    let rules = format!(
        "900001,health:0:100,100,4294967295,once,all,ordinary,any-posture,auto-attack:{}+sheath:2",
        u8::from(enabled)
    );
    packed_definition(&rules)
}

fn packed_definition(rules: &str) -> String {
    let subject = format!("entry:{ENTRY}");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-eventai-definition-v1");
    hasher.update(format!("{subject}@{rules}").as_bytes());
    let revision = u64::from_le_bytes(hasher.finalize().as_bytes()[..8].try_into().unwrap());
    format!("{subject}@{revision}@{rules}")
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn zone_speech_resolves_terrain_imported_after_the_creature_spawn() {
    let mut shard = Standalone::start("full-world-zone-speech");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let creature = shard
        .query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {CREATURE}"
        ))
        .remove(0);
    assert_eq!(creature["zone_id"], "0");
    shard.assert_call(
        "debug_teleport",
        &[
            "1",
            &creature["map_id"],
            &creature["x"],
            &creature["y"],
            &creature["z"],
            "0",
        ],
    );
    shard.assert_sql("INSERT INTO game_creature_ai_broadcast_text (id,male_text,female_text,chat_type,language_id,emote_delay_1_ms,emote_id_1,emote_delay_2_ms,emote_id_2,emote_delay_3_ms,emote_id_3) VALUES (5099000,'Zone speech','',6,0,0,0,0,0,0,0)");
    let speech = shard.subscribe(&[&format!(
        "SELECT * FROM game_chat_event_live WHERE sender_guid = {CREATURE} AND chat_type = 6"
    )]);
    let rules = "900003,health:0:100,100,4294967295,repeat:500:500,all,ordinary,any-posture,auto-attack:0+speak:zone-yell:self:5099000";
    shard.assert_call(
        "debug_stage_eventai_revision_fixture",
        &[&CREATURE.to_string(), "1", &packed_definition(rules)],
    );
    let cell_x = lyracore_shared::terrain::cell_index(creature["x"].parse().unwrap()).unwrap();
    let cell_y = lyracore_shared::terrain::cell_index(creature["y"].parse().unwrap()).unwrap();
    for zone in [5099001, 5099002] {
        shard.assert_sql(&format!("INSERT INTO game_area (id,map_id,parent_area_id,area_bit,flags,exploration_level,faction_group,name) VALUES ({zone},{},0,0,0,0,0,'Speech zone')", creature["map_id"]));
        let heights = vec!["0"; 145].join(":");
        shard.assert_call(
            "import_terrain_chunks",
            &[&format!(
                "{},{cell_x},{cell_y},{},0,0,{zone},{heights}",
                creature["map_id"], creature["z"]
            )],
        );
        assert!(poll_until(POLL_TIMEOUT, || shard.query_rows(&format!(
            "SELECT zone_id FROM game_world_entity WHERE guid = {CREATURE}"
        ))[0]["zone_id"]
            == zone.to_string()));
    }
    assert!(poll_until(POLL_TIMEOUT, || !event_rows(
        &speech,
        "game_chat_event_live"
    )
    .is_empty()));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn authored_auto_attack_switch_controls_real_scheduled_swings() {
    let mut shard = Standalone::start("full-world-eventai");
    shard.publish_module();
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let creature = shard
        .query_rows(&format!(
            "SELECT * FROM game_world_entity WHERE guid = {CREATURE}"
        ))
        .remove(0);
    shard.assert_call(
        "debug_teleport",
        &[
            "1",
            &creature["map_id"],
            &creature["x"],
            &creature["y"],
            &creature["z"],
            "0",
        ],
    );
    let stage = |enabled| {
        shard.assert_call(
            "debug_stage_eventai_revision_fixture",
            &[&CREATURE.to_string(), "1", &definition(enabled)],
        )
    };
    stage(false);
    let state_query = format!(
        "SELECT melee_enabled FROM game_creature_ai_state WHERE creature_guid = {CREATURE}"
    );
    assert!(poll_until(POLL_TIMEOUT, || shard
        .query_rows(&state_query)
        .first()
        .is_some_and(|row| row["melee_enabled"] == "false")));
    let swing_query =
        format!("SELECT last_swing_ms FROM game_melee_attack WHERE attacker_guid = {CREATURE}");
    let last_swing = shard.query_rows(&swing_query).remove(0)["last_swing_ms"].clone();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        assert_eq!(
            shard.query_rows(&swing_query)[0]["last_swing_ms"],
            last_swing
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let creature = shard
        .query_rows(&format!(
            "SELECT unit_bytes_2 FROM game_world_entity WHERE guid = {CREATURE}"
        ))
        .remove(0);
    assert_eq!(creature["unit_bytes_2"].parse::<u32>().unwrap() & 0xff, 2);
    stage(true);
    assert!(poll_until(POLL_TIMEOUT, || shard.query_rows(&swing_query)
        [0]["last_swing_ms"]
        != "0"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn instance_copies_inherit_the_source_guid_spawn_rule() {
    let mut shard = Standalone::start("full-world-guid-ai");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let source = (0xF130u64 << 48) | (u64::from(ENTRY) << 24) | 1_000_036;
    let subject = format!("guid:{source}");
    let rules = "900002,spawn:always,100,4294967295,once,all,ordinary,any-posture,sheath:2";
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-eventai-definition-v1");
    hasher.update(format!("{subject}@{rules}").as_bytes());
    let revision = u64::from_le_bytes(hasher.finalize().as_bytes()[..8].try_into().unwrap());
    shard.assert_call(
        "import_creature_ai_definitions",
        &[&format!("{subject}@{revision}@{rules}")],
    );
    shard.assert_call(
        "import_creature_spawns",
        &[&format!("{source},{ENTRY},36,10,20,30,0,0,300")],
    );
    for instance in [901, 902] {
        shard.assert_call(
            "ensure_instance",
            &[&instance.to_string(), "36", "0", &support::actor("1")],
        );
        let rows = shard.query_rows(&format!(
            "SELECT guid FROM game_creature_spawn WHERE instance_id = {instance}"
        ));
        assert_eq!(rows.len(), 1);
        let guid = &rows[0]["guid"];
        assert_ne!(guid, &source.to_string());
        let entity = shard
            .query_rows(&format!(
                "SELECT unit_bytes_2 FROM game_world_entity WHERE guid = {guid}"
            ))
            .remove(0);
        assert_eq!(entity["unit_bytes_2"].parse::<u32>().unwrap() & 0xff, 2);
    }
}
