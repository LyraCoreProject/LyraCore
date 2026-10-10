mod support;

use support::{Row, Standalone};

const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;
const ZERO_IDENTITY: &str = "0x0000000000000000000000000000000000000000000000000000000000000000";

fn realm(name: &str) -> Standalone {
    let mut realm = Standalone::start(name);
    realm.publish_module();
    realm.assert_call("claim_operator", &[]);
    assert!(locator(&realm).is_empty());
    realm
}

fn locator(realm: &Standalone) -> Vec<Row> {
    realm.query_rows("SELECT * FROM game_character_shard WHERE character_guid = 1")
}

fn begin(revision: &str) -> [&str; 10] {
    [
        "1",
        "1",
        "0",
        revision,
        "1",
        "0",
        ZERO_IDENTITY,
        "0",
        "0",
        ACTOR,
    ]
}

fn assert_begin_refused(realm: &Standalone, args: &[&str]) {
    let before = locator(realm);
    let output = realm.call("begin_character_shard_transfer", args);
    assert!(
        !output.status.success(),
        "the competing Transfer must be refused"
    );
    assert_eq!(
        locator(realm),
        before,
        "a Refusal must leave the locator intact"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_first_human_transfer_creates_only_a_pending_locator_and_replays_exactly() {
    let realm = realm("first-human-transfer-locator");
    realm.assert_call("begin_character_shard_transfer", &begin("0"));
    let pending = locator(&realm);
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["map_id"], "1");
    assert_eq!(pending[0]["instance_id"], "0");
    assert_eq!(pending[0]["revision"], "1");
    assert_eq!(pending[0]["transfer_pending"], "true");
    assert_eq!(pending[0]["pending_destination_map"], "1");
    assert_eq!(pending[0]["pending_destination_instance"], "0");

    realm.assert_call("begin_character_shard_transfer", &begin("0"));
    realm.assert_call("begin_character_shard_transfer", &begin("1"));
    assert_eq!(locator(&realm), pending);

    realm.assert_call("finish_character_shard_transfer", &begin("1"));
    let settled = locator(&realm);
    assert_eq!(settled[0]["revision"], "2");
    assert_eq!(settled[0]["transfer_pending"], "false");
    assert_begin_refused(&realm, &begin("0"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn first_transfer_initialization_cannot_replace_a_competing_pending_locator() {
    let realm = realm("competing-first-transfer-locator");
    realm.assert_call("begin_character_shard_transfer", &begin("0"));
    for (column, value) in [(1, "0"), (2, "7"), (3, "2"), (4, "36"), (5, "7")] {
        let mut competing = begin("0");
        competing[column] = value;
        assert_begin_refused(&realm, &competing);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn first_transfer_initialization_cannot_replace_a_settled_locator() {
    let realm = realm("settled-first-transfer-locator");
    realm.assert_call("set_character_shard", &["1", "1", "0", ACTOR]);
    assert_eq!(locator(&realm)[0]["revision"], "1");
    assert_begin_refused(&realm, &begin("0"));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_bot_cannot_create_a_missing_transfer_locator() {
    let realm = realm("bot-missing-transfer-locator");
    for revision in ["0", "1"] {
        let mut bot = begin(revision);
        bot[6] = "0x0101010101010101010101010101010101010101010101010101010101010101";
        bot[7] = "10";
        bot[8] = "4";
        assert_begin_refused(&realm, &bot);
    }
    let mut incomplete = begin("0");
    incomplete[8] = "4";
    assert_begin_refused(&realm, &incomplete);
}
