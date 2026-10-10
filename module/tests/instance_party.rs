//! A party formed after one member entered an Instance alone shares that Instance, so the members
//! who follow never mint a second one.

mod support;

use support::{actor, Standalone};

const OWNER: &str = "1";
const MEMBER: &str = "2";

const INVITE: &str = "0";
const ACCEPT: &str = "1";

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_solo_instance_becomes_the_instance_of_the_party_its_owner_forms() {
    let mut shard = Standalone::start("instance-party-adoption");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call(
        "create_character",
        &["1", "\"Member\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    for guid in [OWNER, MEMBER] {
        shard.assert_call("debug_spawn_player_entity", &[guid]);
    }
    shard.assert_call("debug_create_fixture_instance", &[OWNER]);
    let solo = instance_of(&shard, OWNER);

    shard.assert_call(
        "realm_group_op",
        &[INVITE, &actor(OWNER), MEMBER, "0", "0", "0"],
    );
    shard.assert_call(
        "realm_group_op",
        &[ACCEPT, &actor(MEMBER), "0", "0", "0", "0"],
    );
    let group = shard.query_rows("SELECT group_id FROM game_group")[0]["group_id"].clone();

    shard.assert_call("debug_create_fixture_instance", &[OWNER]);
    shard.assert_call("debug_create_fixture_instance", &[MEMBER]);

    assert_eq!(instance_of(&shard, MEMBER), solo);
    let instances: Vec<(String, String)> = shard
        .query_rows("SELECT instance_id, party_id FROM game_instance")
        .into_iter()
        .map(|row| (row["instance_id"].clone(), row["party_id"].clone()))
        .collect();
    assert_eq!(instances, [(solo, group)]);
}

fn instance_of(shard: &Standalone, guid: &str) -> String {
    shard.query_rows(&format!(
        "SELECT instance_id FROM game_world_entity WHERE guid = {guid}"
    ))[0]["instance_id"]
        .clone()
}
