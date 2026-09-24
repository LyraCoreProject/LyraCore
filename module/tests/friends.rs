mod support;

use support::{actor, Standalone};

const HUMAN: &str = "1"; // Alliance
const ORC: &str = "2"; // Horde
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

/// `gw_add_friend`/`gw_add_ignore` against one Standalone Shard, whose only durable Character row
/// is the actor's own: every target guid below has no `game_character` row here at all, proving the
/// Gateway's realm-wide name resolution is now the only existence Gate.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn gw_add_friend_needs_no_local_row_and_gates_self_enemy_duplicate_and_full() {
    let mut shard = Standalone::start("friends-add");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    // The actor's own live entity carries its race (Human, Alliance) — `add_contact_core`'s Enemy
    // Gate reads it off here, not off any durable row for the target.
    shard.assert_call("debug_spawn_player_entity", &["1"]);

    // A target with no `game_character` row anywhere on this database still succeeds: the
    // existence check moved to the Gateway's name resolution.
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
    // The Enemy Gate is friends-only: the same Orc target is a legal ignore.
    shard.assert_call("gw_add_ignore", &[&actor("1"), "9002"]);

    // Fill the friend list to its cap: 9001 above plus 49 more makes 50 (`chat::MAX_FRIENDS`).
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
