mod support;

use support::Standalone;

const PAYER: u64 = 1;
const VENDOR_ENTRY: &str = "51004";
const TABARDDESIGNER: u32 = 0x400;

fn actor(guid: u64) -> String {
    format!(r#"{{"guid":{guid},"ownership":null}}"#)
}

fn emblem_request(npc_guid: &str) -> String {
    format!(
        r#"{{"emblem":{{"npc_guid":{npc_guid},"emblem":{{"emblem_style":11,"emblem_color":12,"border_style":3,"border_color":14,"background_color":15}}}}}}"#
    )
}

fn refused(shard: &Standalone, reducer: &str, args: &[&str], tag: &str) {
    let result = shard.call(reducer, args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(tag),
        "{reducer}: expected {tag}: {output}"
    );
}

fn purse(shard: &Standalone) -> u32 {
    shard.query_rows(&format!(
        "SELECT money FROM game_world_entity WHERE guid = {PAYER}"
    ))[0]["money"]
        .parse()
        .unwrap()
}

fn fixture(name: &str) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&PAYER.to_string()]);
    shard.assert_call("debug_set_money", &[&PAYER.to_string(), "150000"]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&PAYER.to_string(), VENDOR_ENTRY, "1"],
    );
    let vendor = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {VENDOR_ENTRY}"
    ))[0]["guid"]
        .clone();
    (shard, vendor)
}

fn make_tabard_designer(shard: &Standalone, vendor: &str) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {TABARDDESIGNER} WHERE guid = {vendor}"
    ));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_guild_leader_pays_ten_gold_and_every_refusal_keeps_the_copper() {
    let (shard, vendor) = fixture("guild-fee");
    let payer = actor(PAYER);
    let request = emblem_request(&vendor);

    refused(
        &shard,
        "debug_guild_fee_purchase",
        &[&payer, &request],
        "guild:not_in_guild",
    );
    assert_eq!(purse(&shard), 150_000);
    let founding = format!(
        r#"{{"gmCreate":{{"leader_guid":{PAYER},"leader_name":"Tester","leader_team":469,"leader_realm_account":0,"gm_level":1,"name":"Tabard Guild"}}}}"#
    );
    shard.assert_call("realm_guild_op", &[&payer, &founding]);
    refused(
        &shard,
        "debug_guild_fee_purchase",
        &[&payer, &request],
        "guild:npc_refused",
    );
    assert_eq!(purse(&shard), 150_000);
    make_tabard_designer(&shard, &vendor);

    shard.assert_call("debug_guild_fee_purchase", &[&payer, &request]);
    assert_eq!(purse(&shard), 50_000);
    let guild = &shard.query_rows("SELECT * FROM game_guild")[0];
    let emblem: Vec<&str> = [
        "emblem_style",
        "emblem_color",
        "border_style",
        "border_color",
        "background_color",
    ]
    .iter()
    .map(|column| guild[*column].as_str())
    .collect();
    assert_eq!(emblem, ["11", "12", "3", "14", "15"]);
    let tabard_events =
        shard.query_rows("SELECT * FROM game_guild_event WHERE kind = 9 AND recipient_guid = 0");
    assert_eq!(tabard_events.len(), 1);

    refused(
        &shard,
        "debug_guild_fee_purchase",
        &[&payer, &request],
        "guild:not_enough_money",
    );
    assert_eq!(purse(&shard), 50_000);
}
