mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, FIXTURE_JERKY};
use lyracore_shared::trade::event_kind;
use support::{refused, Standalone};

const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;

struct Fixture {
    shard: Standalone,
    other: String,
}

fn fixture(name: &str) -> Fixture {
    let mut shard = Standalone::start(name);
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
    shard.assert_call("debug_spawn_player_entity", &[&other]);
    shard.assert_sql("UPDATE game_world_entity SET x = 100, y = 100, z = 100 WHERE guid = 1");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 100, y = 100, z = 100 WHERE guid = {other}"
    ));
    shard.assert_sql("DELETE FROM game_item_instance");
    shard.clear_schedule("game_event_reaper_schedule");
    Fixture { shard, other }
}

impl Fixture {
    fn grant_at(&self, entry: u32, count: u32, slot: u8) -> String {
        self.shard.assert_call(
            "debug_grant_item",
            &["1", &entry.to_string(), &count.to_string()],
        );
        self.shard.assert_sql(&format!(
            "UPDATE game_item_instance SET slot = {slot} WHERE owner_guid = 1 AND entry = {entry}"
        ));
        self.shard.query_rows(&format!(
            "SELECT guid FROM game_item_instance WHERE owner_guid = 1 AND entry = {entry}"
        ))[0]["guid"]
            .clone()
    }

    // A purse row is what an open leaves on an item whose loot is rolled.
    fn open(&self, item: &str) {
        self.shard.insert_row(
            "game_loot_money",
            &[("source_guid", &item), ("owner_guid", &1), ("money", &5)],
        );
    }

    // A spent-charge row is what a cast that spends a charge leaves on an item that cannot stack.
    fn spend_charge(&self, item: &str) {
        self.shard.insert_row(
            "game_item_spell_charges",
            &[("item_guid", &item), ("owner_guid", &1), ("spent", &1)],
        );
    }

    fn owner_of(&self, item: &str) -> Vec<String> {
        self.shard
            .query_rows(&format!(
                "SELECT owner_guid FROM game_item_instance WHERE guid = {item}"
            ))
            .into_iter()
            .map(|row| row["owner_guid"].clone())
            .collect()
    }

    fn canceled(&self, recipient: &str) -> usize {
        self.shard
            .query_rows(&format!(
                "SELECT id FROM game_trade_event WHERE recipient_guid = {recipient} AND kind = {}",
                event_kind::TRADE_CANCELED
            ))
            .len()
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_opened_item_cannot_be_mailed() {
    let f = fixture("opened-item-mail");
    let item = f.grant_at(FIXTURE_JERKY, 1, 23);
    f.open(&item);
    f.shard.assert_call("debug_set_money", &["1", "1000"]);

    refused(
        &f.shard,
        "realm_mail_send",
        &[
            ACTOR,
            &f.other,
            "\"Pearls\"",
            "\"\"",
            "0",
            "0",
            &item,
            "true",
        ],
        lyracore_shared::mail::ITEM_HAS_LOOT,
    );

    assert_eq!(f.owner_of(&item), ["1"]);
    assert!(f
        .shard
        .query_rows(&format!(
            "SELECT id FROM game_mail WHERE item_entry = {FIXTURE_JERKY}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_opened_item_can_be_neither_offered_nor_traded_at_accept() {
    let f = fixture("opened-item-trade");
    let opened = f.grant_at(FIXTURE_JERKY, 1, 23);
    let offered = f.grant_at(FIXTURE_BLADE, 1, 24);
    f.open(&opened);
    let partner = format!(r#"{{"guid":{},"ownership":null}}"#, f.other);
    f.shard.assert_call("gw_initiate_trade", &[ACTOR, &f.other]);
    f.shard.assert_call("gw_begin_trade", &[&partner]);

    f.shard
        .assert_call("gw_set_trade_item", &[ACTOR, "0", "23"]);
    assert_eq!(f.canceled("1"), 1, "offering an opened item cancels it");
    assert!(f
        .shard
        .query_rows("SELECT id FROM game_trade_slot")
        .is_empty());

    f.shard
        .assert_call("gw_set_trade_item", &[ACTOR, "0", "24"]);
    assert_eq!(
        f.shard.query_rows("SELECT id FROM game_trade_slot").len(),
        1,
        "the unopened blade is offered"
    );
    f.open(&offered);

    f.shard.assert_call("gw_accept_trade", &[ACTOR]);
    assert_eq!(
        (f.canceled("1"), f.canceled(&f.other)),
        (2, 0),
        "only the accepting player is told"
    );
    f.shard.assert_call("gw_accept_trade", &[&partner]);
    assert_eq!((f.canceled("1"), f.canceled(&f.other)), (2, 1));

    assert_eq!(f.owner_of(&offered), ["1"], "the trade does not complete");
    assert_eq!(
        f.shard
            .query_rows("SELECT id FROM game_trade_session")
            .len(),
        1,
        "vmangos leaves the trade open"
    );
    let session = &f
        .shard
        .query_rows("SELECT initiator_accepted, target_accepted FROM game_trade_session")[0];
    assert_eq!(
        (
            session["initiator_accepted"].as_str(),
            session["target_accepted"].as_str()
        ),
        ("true", "true"),
        "vmangos keeps a refused accept"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_opened_stack_cannot_be_split() {
    let f = fixture("opened-item-split");
    let stack = f.grant_at(FIXTURE_JERKY, 5, 23);
    f.open(&stack);

    refused(
        &f.shard,
        "debug_split_item",
        &["1", "23", "2", "24"],
        "item:couldnt_split_items",
    );

    let rows = f.shard.query_rows(&format!(
        "SELECT guid, stack_count FROM game_item_instance WHERE owner_guid = 1 AND entry = {FIXTURE_JERKY}"
    ));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["stack_count"], "5");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_item_with_spent_charges_cannot_be_mailed() {
    let f = fixture("spent-charge-mail");
    let item = f.grant_at(FIXTURE_BLADE, 1, 23);
    f.spend_charge(&item);
    f.shard.assert_call("debug_set_money", &["1", "1000"]);

    refused(
        &f.shard,
        "realm_mail_send",
        &[
            ACTOR,
            &f.other,
            "\"Charges\"",
            "\"\"",
            "0",
            "0",
            &item,
            "true",
        ],
        lyracore_shared::mail::ITEM_HAS_SPENT_CHARGES,
    );

    assert_eq!(f.owner_of(&item), ["1"]);
    assert!(f
        .shard
        .query_rows(&format!(
            "SELECT id FROM game_mail WHERE item_entry = {FIXTURE_BLADE}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_traded_item_keeps_its_spent_charges_under_its_new_guid() {
    let f = fixture("spent-charge-trade");
    let item = f.grant_at(FIXTURE_BLADE, 1, 23);
    f.spend_charge(&item);
    let partner = format!(r#"{{"guid":{},"ownership":null}}"#, f.other);
    f.shard.assert_call("gw_initiate_trade", &[ACTOR, &f.other]);
    f.shard.assert_call("gw_begin_trade", &[&partner]);
    f.shard
        .assert_call("gw_set_trade_item", &[ACTOR, "0", "23"]);
    f.shard.assert_call("gw_accept_trade", &[ACTOR]);
    f.shard.assert_call("gw_accept_trade", &[&partner]);

    let received = f.shard.query_rows(&format!(
        "SELECT guid FROM game_item_instance WHERE owner_guid = {} AND entry = {FIXTURE_BLADE}",
        f.other
    ));
    assert_eq!(received.len(), 1, "the trade completes");
    let new_guid = &received[0]["guid"];
    assert_ne!(new_guid, &item, "a traded item gets a new guid");
    let charges = f
        .shard
        .query_rows("SELECT item_guid, owner_guid, spent FROM game_item_spell_charges");
    assert_eq!(charges.len(), 1, "{charges:?}");
    assert_eq!(
        (
            charges[0]["item_guid"].as_str(),
            charges[0]["owner_guid"].as_str(),
            charges[0]["spent"].as_str()
        ),
        (new_guid.as_str(), f.other.as_str(), "1")
    );
}
