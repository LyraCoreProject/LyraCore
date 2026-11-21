mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, FIXTURE_JERKY, FIXTURE_REINS};
use lyracore_shared::trade::{decode_offer, event_kind, OfferSlot};
use support::{number, Standalone};

const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;
const BAG_SLOT: &str = "120";
const BANK_SLOT: &str = "39";

struct Fixture {
    shard: Standalone,
    other: String,
    partner: String,
}

fn fixture(name: &str) -> Fixture {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_sql("DELETE FROM game_item_instance");
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
    let partner = format!(r#"{{"guid":{other},"ownership":null}}"#);
    Fixture {
        shard,
        other,
        partner,
    }
}

impl Fixture {
    fn open_trade(&self) {
        self.shard
            .assert_call("gw_initiate_trade", &[ACTOR, &self.other]);
        self.shard.assert_call("gw_begin_trade", &[&self.partner]);
    }

    fn equip_bag(&self) {
        self.shard.assert_sql(&format!(
            "UPDATE game_item_template SET class = 1, subclass = 0, inventory_type = 18, \
             container_slots = 4, max_stack = 1 WHERE entry = {FIXTURE_BLADE}"
        ));
        self.grant_at(FIXTURE_BLADE, 1, "19", false);
    }

    fn grant_at(&self, entry: u32, count: u32, slot: &str, soulbound: bool) {
        self.shard.assert_call(
            "debug_grant_item",
            &["1", &entry.to_string(), &count.to_string()],
        );
        self.shard.assert_sql(&format!(
            "UPDATE game_item_instance SET slot = {slot}, soulbound = {soulbound} \
             WHERE owner_guid = 1 AND entry = {entry}"
        ));
    }

    fn events(&self, recipient_guid: &str, kind: u8) -> Vec<support::Row> {
        self.shard.query_rows(&format!(
            "SELECT * FROM game_trade_event WHERE recipient_guid = {recipient_guid} AND kind = {kind}"
        ))
    }

    fn last_offer(&self, recipient_guid: &str, kind: u8) -> Vec<OfferSlot> {
        let events = self.events(recipient_guid, kind);
        let last = events
            .iter()
            .max_by_key(|event| number::<u64>(event, "id"))
            .expect("an offer event");
        decode_offer(&last["payload"]).expect("offer payload").1
    }

    fn offered_rows(&self) -> Vec<support::Row> {
        let mut rows = self.shard.query_rows("SELECT * FROM game_trade_slot");
        rows.sort_by_key(|row| number::<u8>(row, "trade_slot"));
        rows
    }

    fn assert_canceled_only_for_actor(&self, trade_slot: &str, inv_slot: &str, canceled: usize) {
        let before = self.offered_rows();
        self.shard
            .assert_call("gw_set_trade_item", &[ACTOR, trade_slot, inv_slot]);
        assert_eq!(
            self.events("1", event_kind::TRADE_CANCELED).len(),
            canceled,
            "trade slot {trade_slot} from slot {inv_slot}"
        );
        assert!(self
            .events(&self.other, event_kind::TRADE_CANCELED)
            .is_empty());
        assert_eq!(self.offered_rows(), before);
        assert_eq!(
            self.shard
                .query_rows("SELECT * FROM game_trade_session")
                .len(),
            1,
            "the session stays open"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_item_from_a_bag_is_offered_shown_to_both_and_moves_to_the_partner() {
    let f = fixture("trade-bag-item");
    f.equip_bag();
    f.grant_at(FIXTURE_JERKY, 5, BAG_SLOT, false);
    let item = f.shard.query_rows(&format!(
        "SELECT guid FROM game_item_instance WHERE entry = {FIXTURE_JERKY}"
    ))[0]["guid"]
        .clone();
    f.open_trade();

    f.shard
        .assert_call("gw_set_trade_item", &[ACTOR, "0", BAG_SLOT]);

    let rows = f.offered_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["item_guid"], item);
    for (recipient, kind) in [
        ("1", event_kind::OFFER_SELF),
        (f.other.as_str(), event_kind::OFFER_PARTNER),
    ] {
        let offer = f.last_offer(recipient, kind);
        assert_eq!(offer.len(), 1);
        assert_eq!(
            (offer[0].trade_slot, offer[0].entry, offer[0].stack_count),
            (0, FIXTURE_JERKY, 5)
        );
    }

    f.shard.assert_call("gw_accept_trade", &[ACTOR]);
    f.shard.assert_call("gw_accept_trade", &[&f.partner]);

    assert!(f
        .shard
        .query_rows(&format!(
            "SELECT * FROM game_item_instance WHERE owner_guid = 1 AND entry = {FIXTURE_JERKY}"
        ))
        .is_empty());
    let received = f.shard.query_rows(&format!(
        "SELECT stack_count, slot FROM game_item_instance \
         WHERE owner_guid = {} AND entry = {FIXTURE_JERKY}",
        f.other
    ));
    assert_eq!(received.len(), 1);
    assert_eq!(received[0]["stack_count"], "5");
    for guid in ["1", f.other.as_str()] {
        assert_eq!(f.events(guid, event_kind::TRADE_COMPLETE).len(), 1);
    }
    assert!(f
        .shard
        .query_rows("SELECT * FROM game_trade_session")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_soulbound_item_goes_in_the_will_not_be_traded_slot_only_and_stays_with_its_owner() {
    let f = fixture("trade-soulbound-untraded");
    f.grant_at(FIXTURE_REINS, 1, "23", true);
    let item = f.shard.query_rows("SELECT guid FROM game_item_instance")[0]["guid"].clone();
    f.open_trade();

    f.assert_canceled_only_for_actor("1", "23", 1);

    f.shard
        .assert_call("gw_set_trade_item", &[ACTOR, "6", "23"]);
    let rows = f.offered_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0]["trade_slot"].as_str(),
            rows[0]["item_guid"].as_str()
        ),
        ("6", item.as_str())
    );
    for (recipient, kind) in [
        ("1", event_kind::OFFER_SELF),
        (f.other.as_str(), event_kind::OFFER_PARTNER),
    ] {
        let offer = f.last_offer(recipient, kind);
        assert_eq!(offer.len(), 1);
        assert_eq!((offer[0].trade_slot, offer[0].entry), (6, FIXTURE_REINS));
    }

    f.shard.assert_call("gw_accept_trade", &[ACTOR]);
    f.shard.assert_call("gw_accept_trade", &[&f.partner]);

    let owned = f.shard.query_rows(&format!(
        "SELECT guid, owner_guid, slot FROM game_item_instance WHERE entry = {FIXTURE_REINS}"
    ));
    assert_eq!(owned.len(), 1);
    assert_eq!(
        (
            owned[0]["guid"].as_str(),
            owned[0]["owner_guid"].as_str(),
            owned[0]["slot"].as_str()
        ),
        (item.as_str(), "1", "23")
    );
    for guid in ["1", f.other.as_str()] {
        assert_eq!(f.events(guid, event_kind::TRADE_COMPLETE).len(), 1);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_refused_offer_answers_trade_canceled_to_the_offerer_and_leaves_the_trade_as_it_was() {
    let f = fixture("trade-offer-refusals");
    f.equip_bag();
    f.grant_at(FIXTURE_JERKY, 5, BAG_SLOT, false);
    f.grant_at(FIXTURE_REINS, 1, BANK_SLOT, false);
    f.open_trade();

    f.assert_canceled_only_for_actor("0", BANK_SLOT, 1);
    f.assert_canceled_only_for_actor("0", "30", 2);
    f.assert_canceled_only_for_actor("7", BAG_SLOT, 3);
    f.assert_canceled_only_for_actor("7", "4", 4);

    f.shard
        .assert_call("gw_set_trade_item", &[ACTOR, "0", BAG_SLOT]);
    f.assert_canceled_only_for_actor("0", BAG_SLOT, 5);
    f.assert_canceled_only_for_actor("1", BAG_SLOT, 6);
}
