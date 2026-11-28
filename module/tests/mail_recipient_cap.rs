mod support;

use lyracore_shared::mail::RECIPIENT_MAILBOX_FULL;
use support::{refused, Standalone};

const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;
const SENDER_MONEY: u32 = 1000;
const POSTAGE: u32 = 30;
const MAILS_AT_THE_LIMIT: usize = 100;

fn mails_of(shard: &Standalone, guid: &str) -> usize {
    shard
        .query_rows(&format!(
            "SELECT id FROM game_mail WHERE recipient_guid = {guid}"
        ))
        .len()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_send_to_a_mailbox_holding_more_than_a_hundred_mails_is_refused_and_charges_nothing() {
    let mut shard = Standalone::start("mail-recipient-cap");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call(
        "create_character",
        &["1", "\"Other\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let other =
        shard.query_rows("SELECT guid FROM game_character WHERE name = 'Other'")[0]["guid"].clone();
    shard.assert_call("debug_set_money", &["1", &SENDER_MONEY.to_string()]);
    for _ in 0..MAILS_AT_THE_LIMIT {
        shard.assert_call("debug_seed_mail", &[&other, "1", "\"Filler\"", "\"\"", "0"]);
    }
    let send_args = [ACTOR, &other, "\"hello\"", "\"\"", "0", "0", "0", "true"];

    shard.assert_call("realm_mail_send", &send_args);
    assert_eq!(mails_of(&shard, &other), MAILS_AT_THE_LIMIT + 1);
    let paid =
        shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1")[0]["money"].clone();
    assert_eq!(paid, (SENDER_MONEY - POSTAGE).to_string());

    refused(
        &shard,
        "realm_mail_send",
        &send_args,
        RECIPIENT_MAILBOX_FULL,
    );
    assert_eq!(mails_of(&shard, &other), MAILS_AT_THE_LIMIT + 1);
    let kept =
        shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1")[0]["money"].clone();
    assert_eq!(kept, paid, "a refused send charges no postage");
}
