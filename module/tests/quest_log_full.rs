mod support;

use support::{actor, Standalone};

const CHARACTER: &str = "1";
const FIRST_QUEST: u32 = 5_096_500;
const LOG_SIZE: u32 = 20;
const WOLF: u32 = 51_000;
const REPEATABLE: u32 = FIRST_QUEST + LOG_SIZE + 1;

fn quest_rows(shard: &Standalone) -> usize {
    shard
        .query_rows(&format!(
            "SELECT quest_entry FROM game_character_quest WHERE character_guid = {CHARACTER}"
        ))
        .len()
}

fn accept(shard: &Standalone, giver: &str, quest: u32) -> std::process::Output {
    shard.call(
        "debug_accept_quest",
        &[CHARACTER, giver, &quest.to_string()],
    )
}

fn setup(name: &str) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[CHARACTER]);
    shard.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE entry = {WOLF}"
    ));
    shard.assert_call("debug_spawn_at_feet", &[CHARACTER, &WOLF.to_string(), "3"]);
    let giver = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {WOLF}"
        ))
        .remove(0)["guid"]
        .clone();
    for offset in 0..=LOG_SIZE + 1 {
        let entry = FIRST_QUEST + offset;
        let repeatable = entry == REPEATABLE;
        shard.assert_sql(&format!(
            "INSERT INTO game_quest_template \
             (entry,min_level,quest_level,title,reward_money,reward_xp,prev_quest_id,\
             required_races,required_classes,zone_or_sort,rew_rep_faction_1,rew_rep_value_1,\
             rew_rep_faction_2,rew_rep_value_2,src_item,src_item_count,repeatable,next_quest_id,\
             limit_time,reward_money_max_level,quest_type) \
             VALUES ({entry},0,2,'Full log quest',0,0,0,0,0,12,0,0,0,0,0,0,{repeatable},0,0,0,0)"
        ));
        shard.assert_sql(&format!(
            "INSERT INTO game_creature_quest (id,creature_entry,quest_entry,role) \
             VALUES ({},{WOLF},{entry},0)",
            u64::from(entry)
        ));
    }
    (shard, giver)
}

fn grant(shard: &Standalone, quest: u32) {
    shard.assert_call("debug_grant_quest", &[CHARACTER, &quest.to_string()]);
}

fn assert_full_log_refusal(output: &std::process::Output) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("quest log is full"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_full_quest_log_refuses_a_new_accept_and_adds_no_row() {
    let (shard, giver) = setup("quest-log-full");
    for offset in 0..LOG_SIZE {
        grant(&shard, FIRST_QUEST + offset);
    }
    assert_eq!(quest_rows(&shard), LOG_SIZE as usize);

    let twenty_first = FIRST_QUEST + LOG_SIZE;
    assert_full_log_refusal(&accept(&shard, &giver, twenty_first));
    assert_eq!(
        quest_rows(&shard),
        LOG_SIZE as usize,
        "a Refusal adds no row"
    );

    shard.assert_call(
        "gw_abandon_quest",
        &[&actor(CHARACTER), &FIRST_QUEST.to_string()],
    );
    assert!(accept(&shard, &giver, twenty_first).status.success());
    assert_eq!(quest_rows(&shard), LOG_SIZE as usize);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_rewarded_repeatable_quest_needs_a_free_slot_to_return() {
    let (shard, giver) = setup("quest-log-repeatable");
    grant(&shard, REPEATABLE);
    shard.assert_sql(&format!(
        "UPDATE game_character_quest SET rewarded = true WHERE quest_entry = {REPEATABLE}"
    ));
    for offset in 0..LOG_SIZE - 1 {
        grant(&shard, FIRST_QUEST + offset);
    }
    assert!(accept(&shard, &giver, REPEATABLE).status.success());

    shard.assert_sql(&format!(
        "UPDATE game_character_quest SET rewarded = true WHERE quest_entry = {REPEATABLE}"
    ));
    grant(&shard, FIRST_QUEST + LOG_SIZE - 1);
    assert_full_log_refusal(&accept(&shard, &giver, REPEATABLE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_failed_quest_keeps_its_slot_until_it_is_taken_again() {
    let (shard, giver) = setup("quest-log-failed");
    for offset in 0..LOG_SIZE {
        grant(&shard, FIRST_QUEST + offset);
    }
    shard.assert_call("debug_expire_quest", &[CHARACTER, &FIRST_QUEST.to_string()]);

    assert_full_log_refusal(&accept(&shard, &giver, FIRST_QUEST + LOG_SIZE));
    assert!(
        accept(&shard, &giver, FIRST_QUEST).status.success(),
        "a failed quest resets in its own slot"
    );
    assert_eq!(quest_rows(&shard), LOG_SIZE as usize);
    let first = shard.query_rows(&format!(
        "SELECT failed, rewarded, deadline_micros FROM game_character_quest \
         WHERE character_guid = {CHARACTER} AND quest_entry = {FIRST_QUEST}"
    ));
    assert_eq!(first.len(), 1, "{first:?}");
    assert_eq!(first[0]["failed"], "false", "{first:?}");
    assert_eq!(first[0]["rewarded"], "false", "{first:?}");
}
