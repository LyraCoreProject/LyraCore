mod support;

use support::{actor, Standalone};

const HUMAN: &str = "1";
const ORC: &str = "2";
const CONTACT_ROWS: &str =
    "SELECT target_guid, is_ignore FROM game_character_contact WHERE owner_guid = 1";

fn refused(shard: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let output = shard.call(reducer, args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && text.contains(tag),
        "{reducer} should refuse with {tag}: {text}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn gw_add_friend_needs_no_local_row_and_gates_self_enemy_duplicate_and_full() {
    let mut shard = Standalone::start("friends-add");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);

    shard.assert_call("gw_add_friend", &[&actor("1"), "9001", HUMAN]);
    let rows = shard.query_rows(CONTACT_ROWS);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["target_guid"], "9001");
    assert_eq!(rows[0]["is_ignore"], "false");

    refused(
        &shard,
        "gw_add_friend",
        &[&actor("1"), "1", HUMAN],
        "social:add_self",
    );
    refused(
        &shard,
        "gw_add_friend",
        &[&actor("1"), "9001", HUMAN],
        "social:already_on_list",
    );
    refused(
        &shard,
        "gw_add_friend",
        &[&actor("1"), "9002", ORC],
        "social:enemy",
    );
    shard.assert_call("gw_add_ignore", &[&actor("1"), "9002"]);

    for i in 0..49u64 {
        shard.assert_call(
            "gw_add_friend",
            &[&actor("1"), &(9100 + i).to_string(), HUMAN],
        );
    }
    refused(
        &shard,
        "gw_add_friend",
        &[&actor("1"), "9200", HUMAN],
        "social:list_full",
    );
}
