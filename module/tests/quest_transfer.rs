//! Durable quest progress survives a cross-shard Transfer and world-port entry.

mod support;

use support::{actor, number, Row, Standalone};

const CHARACTER: &str = "1";
const TRANSFER: &str = "509647";
const FIRST_QUEST: u32 = 5_096_470;
const DEADLINE_MICROS: i64 = 9_999_999_999_999_999;

fn shard(name: &str, guid_base: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &[guid_base]);
    shard
}

fn quests(shard: &Standalone) -> Vec<Row> {
    let mut rows = shard.query_rows(&format!(
        "SELECT character_guid, quest_entry, counts, rewarded, deadline_micros, failed \
         FROM game_character_quest WHERE character_guid = {CHARACTER}"
    ));
    rows.sort_by_key(|row| number::<u32>(row, "quest_entry"));
    rows
}

fn seed_quests(shard: &Standalone) {
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[CHARACTER]);
    shard.assert_sql("DELETE FROM game_world_entity WHERE entry = 51000");
    shard.assert_sql("UPDATE game_creature_template SET faction_template = 35 WHERE entry = 51000");

    for offset in 0..4 {
        let entry = FIRST_QUEST + offset;
        shard.assert_sql(&format!(
            "INSERT INTO game_quest_template \
             (entry,min_level,quest_level,title,reward_money,reward_xp,prev_quest_id,\
             required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,\
             rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,\
             limit_time,reward_money_max_level,quest_type) \
             VALUES ({entry},0,2,'Transfer quest',0,0,0,0,0,12,0,0,0,0,0,0,false,0,0,0,0)"
        ));
        for objective in 0..4 {
            let id = u64::from(entry) * 4 + objective;
            let required_count = 4 - objective;
            shard.assert_sql(&format!(
                "INSERT INTO game_quest_objective \
                 (id,quest_entry,obj_index,kind,target_entry,required_count) \
                 VALUES ({id},{entry},{objective},0,51000,{required_count})"
            ));
        }
        let objectives = shard.query_rows(&format!(
            "SELECT * FROM game_quest_objective WHERE quest_entry = {entry}"
        ));
        assert_eq!(objectives.len(), 4, "{objectives:?}");
        shard.assert_call("debug_grant_quest", &[CHARACTER, &entry.to_string()]);
        shard.assert_call("debug_spawn_at_feet", &[CHARACTER, "51000", "5"]);
        let wolf = shard
            .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51000 AND dead = false");
        shard.assert_call("debug_apply_damage", &[&wolf[0]["guid"], "1", CHARACTER]);
        shard.assert_call("debug_kill_nearest", &[CHARACTER, "51000"]);
    }

    shard.assert_sql(&format!(
        "UPDATE game_character_quest SET rewarded = true WHERE quest_entry = {FIRST_QUEST}"
    ));
    shard.assert_call(
        "debug_expire_quest",
        &[CHARACTER, &(FIRST_QUEST + 1).to_string()],
    );
    shard.assert_sql(&format!(
        "UPDATE game_character_quest SET deadline_micros = {DEADLINE_MICROS} \
         WHERE quest_entry = {}",
        FIRST_QUEST + 2
    ));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn quest_progress_survives_cross_shard_transfer_and_world_port() {
    let source = shard("quest-transfer-source", "0");
    let destination = shard("quest-transfer-destination", "1000000000");
    seed_quests(&source);
    let before = quests(&source);
    assert_eq!(before.len(), 4);
    // The pinned CLI prints a numeric array with its values concatenated.
    for (row, expected_counts) in before.iter().zip(["4321", "3321", "2221", "1111"]) {
        assert_eq!(row["counts"], expected_counts, "{before:?}");
    }
    for (row, (rewarded, failed, deadline)) in before.iter().zip([
        (true, false, 0),
        (false, true, 0),
        (false, false, DEADLINE_MICROS),
        (false, false, 0),
    ]) {
        assert_eq!(row["rewarded"], rewarded.to_string());
        assert_eq!(row["failed"], failed.to_string());
        assert_eq!(number::<i64>(row, "deadline_micros"), deadline);
    }

    source.assert_call(
        "begin_transfer",
        &[
            TRANSFER,
            &actor(CHARACTER),
            "0",
            "0",
            "1200",
            "1200",
            "50",
            "0",
            "true",
        ],
    );
    assert_eq!(
        quests(&source),
        before,
        "Escrow preserves the source quests"
    );
    let out = source.query_rows(&format!(
        "SELECT blob FROM game_transfer_out WHERE transfer_id = {TRANSFER}"
    ));
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();
    let system = actor("0");
    destination.assert_call("import_character_blob", &[TRANSFER, &blob, &system]);
    assert_eq!(quests(&destination), before, "import preserves every quest");
    source.assert_call("confirm_import", &[TRANSFER, &system]);
    source.assert_call("finish_transfer", &[TRANSFER, &system]);
    assert!(quests(&source).is_empty(), "the source quests left once");
    destination.assert_call("release_transfer", &[TRANSFER, &system]);
    assert_eq!(
        quests(&destination),
        before,
        "release preserves every quest"
    );

    let key = serde_json::to_string(&vec![7u8; 40]).unwrap();
    destination.assert_call(
        "establish_session",
        &["1", &key, r#"{"__identity__":"0x1"}"#],
    );
    destination.assert_call("gw_heartbeat", &[]);
    destination.assert_call("gw_player_world_port", &["1", &actor(CHARACTER)]);
    assert_eq!(
        quests(&destination),
        before,
        "world-port entry preserves every quest"
    );
    assert_eq!(
        destination
            .query_rows("SELECT guid FROM game_world_entity WHERE guid = 1")
            .len(),
        1,
        "the transferred Character entered the world"
    );
}
