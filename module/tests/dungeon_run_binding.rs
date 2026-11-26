mod support;

use support::{group_op, Standalone};

const LEADER: u64 = 1;
const DEADMINES: u32 = 36;
const TO_DEADMINES: u32 = 5_272_001;

const INVITE: &str = "0";
const ACCEPT: &str = "1";
const LEAVE: &str = "3";
const SET_LEADER: &str = "7";

fn realm(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.insert_row(
        "game_map",
        &[
            ("id", &DEADMINES),
            ("internal_name", &"Fixture"),
            ("name", &"Fixture"),
            ("map_type", &1),
        ],
    );
    shard.insert_row(
        "game_area_trigger",
        &[
            ("id", &TO_DEADMINES),
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
            ("trigger_id", &TO_DEADMINES),
            ("target_map", &DEADMINES),
            ("x", &10.0f32),
            ("y", &20.0f32),
            ("z", &30.0f32),
            ("o", &0.0f32),
            ("name", &"Fixture entrance"),
        ],
    );
    shard.assert_call("debug_spawn_player_entity", &[&LEADER.to_string()]);
    shard
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
    let guid: u64 = shard.query_rows(&format!(
        "SELECT guid FROM game_character WHERE name = '{name}'"
    ))[0]["guid"]
        .parse()
        .unwrap();
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    guid
}

fn join(shard: &Standalone, inviter: u64, guid: u64) {
    group_op(shard, INVITE, inviter, guid, 0);
    group_op(shard, ACCEPT, guid, 0, 0);
}

fn group_of(shard: &Standalone, guid: u64) -> String {
    shard.query_rows(&format!(
        "SELECT group_id FROM game_group_member WHERE character_guid = {guid}"
    ))[0]["group_id"]
        .clone()
}

fn enter(shard: &Standalone, guid: u64) -> String {
    shard.assert_call(
        "debug_enter_areatrigger",
        &[&guid.to_string(), &TO_DEADMINES.to_string()],
    );
    let entity = shard
        .query_rows(&format!(
            "SELECT map_id, instance_id FROM game_world_entity WHERE guid = {guid}"
        ))
        .remove(0);
    assert_eq!(entity["map_id"], DEADMINES.to_string(), "{guid} entered");
    entity["instance_id"].clone()
}

fn walk_out(shard: &Standalone, guid: u64) {
    shard.assert_call(
        "debug_teleport",
        &[&guid.to_string(), "0", "0", "0", "0", "0"],
    );
}

fn owner_of(shard: &Standalone, run: &str) -> String {
    shard.query_rows(&format!(
        "SELECT party_id FROM game_instance WHERE instance_id = {run}"
    ))[0]["party_id"]
        .clone()
}

fn holders_of(shard: &Standalone, run: &str) -> Vec<u64> {
    let mut holders: Vec<u64> = shard
        .query_rows(&format!(
            "SELECT character_guid FROM game_instance_binding WHERE instance_id = {run}"
        ))
        .into_iter()
        .map(|row| row["character_guid"].parse().unwrap())
        .collect();
    holders.sort_unstable();
    holders
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_leaders_solo_run_becomes_the_groups_run_when_the_group_forms() {
    let shard = realm("dungeon-run-formation");
    let member = create_character(&shard, "Member");
    let solo = enter(&shard, LEADER);
    walk_out(&shard, LEADER);

    join(&shard, LEADER, member);
    let group = group_of(&shard, LEADER);

    assert_eq!(
        owner_of(&shard, &solo),
        group,
        "the Group takes the leader's run"
    );
    assert_eq!(
        enter(&shard, member),
        solo,
        "a member who enters first resumes it"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_new_leader_takes_the_group_to_its_own_run_and_the_old_leader_keeps_the_groups_run() {
    let shard = realm("dungeon-run-leader-change");
    let heir = create_character(&shard, "Heir");
    let other = create_character(&shard, "Other");
    let heirs_run = enter(&shard, heir);
    join(&shard, LEADER, heir);
    join(&shard, LEADER, other);
    let group = group_of(&shard, LEADER);
    let groups_run = enter(&shard, LEADER);
    assert_ne!(groups_run, heirs_run, "the heir joined inside its own run");
    walk_out(&shard, LEADER);

    group_op(&shard, SET_LEADER, LEADER, heir, 0);

    assert_eq!(owner_of(&shard, &heirs_run), group);
    assert_eq!(owner_of(&shard, &groups_run), "0");
    assert_eq!(holders_of(&shard, &groups_run), [LEADER]);
    assert_eq!(
        enter(&shard, other),
        heirs_run,
        "the Group enters the new leader's run"
    );
    group_op(&shard, LEAVE, LEADER, 0, 0);
    assert_eq!(
        enter(&shard, LEADER),
        groups_run,
        "the old leader keeps the Group's former run"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_last_member_in_the_groups_run_keeps_it_when_the_group_disbands() {
    let shard = realm("dungeon-run-disband");
    let survivor = create_character(&shard, "Survivor");
    join(&shard, LEADER, survivor);
    let run = enter(&shard, LEADER);
    assert_eq!(enter(&shard, survivor), run);
    walk_out(&shard, LEADER);

    group_op(&shard, LEAVE, LEADER, 0, 0);

    assert_eq!(owner_of(&shard, &run), "0", "the run is the survivor's own");
    assert_eq!(holders_of(&shard, &run), [survivor]);
    walk_out(&shard, survivor);
    assert_eq!(enter(&shard, survivor), run);
    assert_ne!(enter(&shard, LEADER), run, "the leaver starts a fresh run");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_character_who_left_the_group_enters_a_fresh_run_alone() {
    let shard = realm("dungeon-run-left-group");
    let leaver = create_character(&shard, "Leaver");
    let stays = create_character(&shard, "Stays");
    join(&shard, LEADER, leaver);
    join(&shard, LEADER, stays);
    let group = group_of(&shard, LEADER);
    let run = enter(&shard, LEADER);
    assert_eq!(enter(&shard, leaver), run);

    walk_out(&shard, leaver);
    group_op(&shard, LEAVE, leaver, 0, 0);
    let fresh = enter(&shard, leaver);

    assert_ne!(fresh, run, "the Group's run stays with the Group");
    assert_eq!(owner_of(&shard, &fresh), "0", "the leaver's run is its own");
    assert_eq!(owner_of(&shard, &run), group);
    assert_eq!(enter(&shard, stays), run);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_character_who_joined_another_group_enters_that_groups_run() {
    let shard = realm("dungeon-run-other-group");
    let leaver = create_character(&shard, "Leaver");
    let stays = create_character(&shard, "Stays");
    let other_leader = create_character(&shard, "Other");
    join(&shard, LEADER, leaver);
    join(&shard, LEADER, stays);
    let run = enter(&shard, LEADER);
    assert_eq!(enter(&shard, leaver), run);

    walk_out(&shard, leaver);
    group_op(&shard, LEAVE, leaver, 0, 0);
    join(&shard, other_leader, leaver);
    let other_group = group_of(&shard, leaver);
    let other_run = enter(&shard, leaver);

    assert_ne!(other_run, run, "the former Group's run is not entered");
    assert_eq!(owner_of(&shard, &other_run), other_group);
    assert_eq!(enter(&shard, other_leader), other_run);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_member_who_joins_a_group_leaves_its_solo_run_behind() {
    let shard = realm("dungeon-run-join-group");
    let joiner = create_character(&shard, "Joiner");
    let solo = enter(&shard, joiner);
    walk_out(&shard, joiner);

    join(&shard, LEADER, joiner);
    let group = group_of(&shard, joiner);
    assert!(
        shard
            .query_rows(&format!(
                "SELECT id FROM game_instance_binding WHERE character_guid = {joiner}"
            ))
            .is_empty(),
        "joining the Group resets the joiner's own runs"
    );
    let run = enter(&shard, joiner);

    assert_ne!(run, solo, "the Group does not take the joiner's solo run");
    assert_eq!(owner_of(&shard, &run), group);
    assert_eq!(enter(&shard, LEADER), run);
}
