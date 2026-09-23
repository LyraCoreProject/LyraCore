mod support;

use support::Standalone;

/// The seeded Character.
const PAYER: u64 = 1;
/// Scenario vendor 51004, spawned at the payer's feet and made a Tabard Designer.
const VENDOR_ENTRY: &str = "51004";
const TABARDDESIGNER: u32 = 0x400;

/// A tokenless actor: Realm-core holds no Account Claim for the seeded Character.
fn actor(guid: u64) -> String {
    format!(r#"{{"guid":{guid},"ownership":null}}"#)
}

fn emblem_request(npc_guid: &str) -> String {
    format!(
        r#"{{"emblem":{{"npc_guid":{npc_guid},"emblem":{{"emblem_style":11,"emblem_color":12,"border_style":3,"border_color":14,"background_color":15}}}}}}"#
    )
}

const EMBLEM_TERMS: &str = r#"{"emblem":{"emblem_style":11,"emblem_color":12,"border_style":3,"border_color":14,"background_color":15}}"#;

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

fn holds(shard: &Standalone) -> usize {
    shard.query_rows("SELECT * FROM game_guild_fee_hold").len()
}

/// The payer in the world with 15 gold, standing at a vendor. Answers the vendor's guid.
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

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
/// One database plays both the Home Shard and Realm-core.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn the_guild_leader_pays_ten_gold_and_every_refusal_returns_the_copper() {
    let (shard, vendor) = fixture("guild-fee");
    let payer = actor(PAYER);
    let request = emblem_request(&vendor);

    refused(
        &shard,
        "gw_guild_fee_hold",
        &["5090401", &payer, &request],
        "guild:npc_refused",
    );
    assert_eq!((purse(&shard), holds(&shard)), (150_000, 0));
    make_tabard_designer(&shard, &vendor);

    // Outside any Guild: the fee is held, Realm-core refuses, the finish refunds it once.
    shard.assert_call("gw_guild_fee_hold", &["5090401", &payer, &request]);
    shard.assert_call("gw_guild_fee_hold", &["5090401", &payer, &request]);
    assert_eq!((purse(&shard), holds(&shard)), (50_000, 1));
    refused(
        &shard,
        "gw_guild_fee_hold",
        &["5090402", &payer, &request],
        "another Fee Hold is pending",
    );
    let account_id = shard.query_rows(&format!(
        "SELECT account_id FROM game_character WHERE guid = {PAYER}"
    ))[0]["account_id"]
        .clone();
    refused(
        &shard,
        "delete_character",
        &[&account_id, &payer],
        "CHAR_HAS_GUILD_FEE_HOLD",
    );
    shard.assert_call("realm_guild_fee_decide", &["5090401", &payer, EMBLEM_TERMS]);
    let decision = &shard.query_rows("SELECT * FROM game_guild_fee_decision")[0];
    assert_eq!(decision["accepted"], "false");
    assert_eq!(decision["refusal"], "guild:not_in_guild");
    shard.assert_call("gw_guild_fee_finish", &["5090401", &payer, "false"]);
    shard.assert_call("gw_guild_fee_finish", &["5090401", &payer, "false"]);
    assert_eq!((purse(&shard), holds(&shard)), (150_000, 0));

    // The Guild Leader: the fee is spent and the Guild wears the emblem.
    let founding = format!(
        r#"{{"gmCreate":{{"leader_guid":{PAYER},"leader_name":"Tester","leader_team":469,"leader_realm_account":0,"gm_level":1,"name":"Tabard Guild"}}}}"#
    );
    shard.assert_call("realm_guild_op", &[&payer, &founding]);
    shard.assert_call("gw_guild_fee_hold", &["5090403", &payer, &request]);
    shard.assert_call("realm_guild_fee_decide", &["5090403", &payer, EMBLEM_TERMS]);
    shard.assert_call("realm_guild_fee_decide", &["5090403", &payer, EMBLEM_TERMS]);
    shard.assert_call("gw_guild_fee_finish", &["5090403", &payer, "true"]);
    assert_eq!((purse(&shard), holds(&shard)), (50_000, 0));
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
    assert_eq!(
        tabard_events.len(),
        1,
        "a replayed decision broadcasts once"
    );

    refused(
        &shard,
        "gw_guild_fee_hold",
        &["5090404", &payer, &request],
        "guild:not_enough_money",
    );
    assert_eq!((purse(&shard), holds(&shard)), (50_000, 0));
}

/// A Hold crosses a Shard Boundary with its Character and is refunded on the new Home Shard.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_fee_hold_travels_with_its_character_and_finishes_on_the_new_home_shard() {
    let (source, vendor) = fixture("guild-fee-source");
    make_tabard_designer(&source, &vendor);
    let payer = actor(PAYER);
    source.assert_call(
        "gw_guild_fee_hold",
        &["5090411", &payer, &emblem_request(&vendor)],
    );
    source.assert_call(
        "begin_transfer",
        &["5090410", &payer, "0", "0", "0", "0", "0", "0", "true"],
    );
    let out = source.query_rows("SELECT blob FROM game_transfer_out WHERE transfer_id = 5090410");
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();

    let mut destination = Standalone::start("guild-fee-destination");
    destination.publish_module();
    destination.assert_call("claim_operator", &[]);
    destination.assert_call("install_guid_range", &["1000000000"]);
    let system = actor(0);
    destination.assert_call("import_character_blob", &["5090410", &blob, &system]);
    source.assert_call("confirm_import", &["5090410", &system]);
    source.assert_call("finish_transfer", &["5090410", &system]);
    destination.assert_call("release_transfer", &["5090410", &system]);

    assert_eq!(holds(&source), 0, "the Hold left with its Character");
    let arrived = destination.query_rows("SELECT * FROM game_guild_fee_hold");
    assert_eq!(arrived.len(), 1);
    assert_eq!(arrived[0]["operation_id"], "5090411");
    assert_eq!(arrived[0]["copper"], "100000");

    // The source plays Realm-core: the payer is in no Guild, so the decision refuses.
    source.assert_call("realm_guild_fee_decide", &["5090411", &payer, EMBLEM_TERMS]);
    destination.assert_call("debug_spawn_player_entity", &[&PAYER.to_string()]);
    destination.assert_call("gw_guild_fee_finish", &["5090411", &payer, "false"]);
    assert_eq!((purse(&destination), holds(&destination)), (150_000, 0));
}
