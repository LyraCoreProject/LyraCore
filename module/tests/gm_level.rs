mod support;

use support::Standalone;

fn tester_gm_levels(shard: &Standalone) -> (String, String) {
    let account = shard.query_rows("SELECT gm_level FROM game_account WHERE username = 'TEST'");
    let character = shard.query_rows("SELECT gm_level FROM game_character WHERE guid = 1");
    (
        account[0]["gm_level"].clone(),
        character[0]["gm_level"].clone(),
    )
}

fn levels(account: &str, character: &str) -> (String, String) {
    (account.to_string(), character.to_string())
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_repair_moves_a_character_gm_level_onto_its_account_once() {
    let mut shard = Standalone::start("gm-level");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    assert_eq!(
        tester_gm_levels(&shard),
        levels("3", "0"),
        "the seeded Tester's Account is a GM"
    );

    shard.assert_sql("UPDATE game_account SET gm_level = 0 WHERE username = 'TEST'");
    shard.assert_sql("UPDATE game_character SET gm_level = 2 WHERE guid = 1");
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(
        tester_gm_levels(&shard),
        levels("2", "0"),
        "a Character's GM level moves onto its Account"
    );

    shard.assert_call("set_gm_level", &["\"Tester\"", "0"]);
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(
        tester_gm_levels(&shard),
        levels("0", "0"),
        "an Operator's demotion survives the next repair"
    );

    let refused = shard.call("set_gm_level", &["\"Nobody\"", "1"]);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        !refused.status.success() && output.contains("no player named Nobody"),
        "set_gm_level should refuse an unknown Character: {output}"
    );
}
