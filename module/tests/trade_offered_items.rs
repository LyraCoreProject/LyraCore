mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, FIXTURE_JERKY};
use lyracore_shared::item::ItemRefusal;
use lyracore_shared::mail::SENDER_IS_TRADING;
use lyracore_shared::trade::event_kind;
use support::{refused, Standalone};

const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;
const PURSE: u32 = 1_000;
const PARTNER_GOLD: u32 = 500;
const STRANGE_DUST: u32 = 10_940;
const ENCHANT: u32 = 5_091_230;

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
    for guid in ["1", other.as_str()] {
        shard.assert_call("debug_set_money", &[guid, &PURSE.to_string()]);
    }
    let partner = format!(r#"{{"guid":{other},"ownership":null}}"#);
    Fixture {
        shard,
        other,
        partner,
    }
}

impl Fixture {
    fn grant_at(&self, entry: u32, slot: u8) -> String {
        self.shard
            .assert_call("debug_grant_item", &["1", &entry.to_string(), "1"]);
        self.shard.assert_sql(&format!(
            "UPDATE game_item_instance SET slot = {slot}, soulbound = false \
             WHERE owner_guid = 1 AND entry = {entry}"
        ));
        self.item(entry)[0]["guid"].clone()
    }

    fn item(&self, entry: u32) -> Vec<support::Row> {
        self.shard.query_rows(&format!(
            "SELECT guid, owner_guid, enchant_id FROM game_item_instance WHERE entry = {entry}"
        ))
    }

    fn money(&self, guid: &str) -> u32 {
        support::number(
            &self.shard.query_rows(&format!(
                "SELECT money FROM game_world_entity WHERE guid = {guid}"
            ))[0],
            "money",
        )
    }

    fn events(&self, recipient_guid: &str, kind: u8) -> usize {
        self.shard
            .query_rows(&format!(
                "SELECT id FROM game_trade_event \
                 WHERE recipient_guid = {recipient_guid} AND kind = {kind}"
            ))
            .len()
    }

    fn mail_args<'a>(&'a self, money: &'a str, item_guid: &'a str) -> [&'a str; 8] {
        [
            ACTOR,
            self.other.as_str(),
            "\"trading\"",
            "\"\"",
            money,
            "0",
            item_guid,
            "true",
        ]
    }

    fn offer_for_gold(&self, inv_slot: u8) {
        self.shard
            .assert_call("gw_initiate_trade", &[ACTOR, &self.other]);
        self.shard.assert_call("gw_begin_trade", &[&self.partner]);
        self.shard
            .assert_call("gw_set_trade_item", &[ACTOR, "0", &inv_slot.to_string()]);
        self.shard.assert_call(
            "gw_set_trade_gold",
            &[&self.partner, &PARTNER_GOLD.to_string()],
        );
        let sessions = self.sessions();
        assert_eq!(sessions.len(), 1, "the trade opened: {sessions:?}");
        assert_eq!(
            (
                sessions[0]["open"].as_str(),
                sessions[0]["target_gold"].as_str()
            ),
            ("true", PARTNER_GOLD.to_string().as_str())
        );
        let slots = self
            .shard
            .query_rows("SELECT owner_guid, trade_slot FROM game_trade_slot");
        assert_eq!(slots.len(), 1, "one item is offered: {slots:?}");
        assert_eq!(
            (
                slots[0]["owner_guid"].as_str(),
                slots[0]["trade_slot"].as_str()
            ),
            ("1", "0")
        );
    }

    fn sessions(&self) -> Vec<support::Row> {
        self.shard
            .query_rows("SELECT open, target_gold FROM game_trade_session")
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_character_in_a_trade_cannot_send_any_mail_until_it_ends() {
    let f = fixture("trade-mail-refused");
    let jerky = f.grant_at(FIXTURE_JERKY, 23);
    let blade = f.grant_at(FIXTURE_BLADE, 24);

    f.shard.assert_call("gw_initiate_trade", &[ACTOR, &f.other]);
    let proposed = f.sessions();
    assert_eq!(proposed.len(), 1, "the trade is proposed: {proposed:?}");
    assert_eq!(proposed[0]["open"], "false");
    refused(
        &f.shard,
        "realm_mail_send",
        &f.mail_args("0", "0"),
        SENDER_IS_TRADING,
    );

    f.shard.assert_call("gw_begin_trade", &[&f.partner]);
    f.shard
        .assert_call("gw_set_trade_item", &[ACTOR, "0", "23"]);
    assert_eq!(f.sessions()[0]["open"], "true");
    assert_eq!(
        f.shard
            .query_rows(&format!(
                "SELECT id FROM game_trade_slot WHERE item_guid = {jerky}"
            ))
            .len(),
        1,
        "the jerky is offered"
    );
    for (money, item) in [
        ("0", jerky.as_str()),
        ("0", blade.as_str()),
        ("0", "0"),
        ("100", "0"),
    ] {
        refused(
            &f.shard,
            "realm_mail_send",
            &f.mail_args(money, item),
            SENDER_IS_TRADING,
        );
    }
    assert!(f.shard.query_rows("SELECT id FROM game_mail").is_empty());
    assert_eq!(f.money("1"), PURSE);

    f.shard.assert_call("gw_cancel_trade", &[ACTOR]);
    f.shard
        .assert_call("realm_mail_send", &f.mail_args("100", &jerky));
    let mailed = f
        .shard
        .query_rows("SELECT item_entry, money FROM game_mail");
    assert_eq!(mailed.len(), 1);
    assert_eq!(
        (
            mailed[0]["item_entry"].as_str(),
            mailed[0]["money"].as_str()
        ),
        (FIXTURE_JERKY.to_string().as_str(), "100")
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_offered_item_cannot_be_enchanted_until_it_leaves_the_trade() {
    let f = fixture("trade-offered-enchant");
    f.shard.insert_row(
        "game_item_template",
        &[
            ("entry", &STRANGE_DUST),
            ("name", &"Strange Dust"),
            ("max_stack", &20u32),
        ],
    );
    f.shard.insert_row(
        "game_item_enchantment",
        &[
            ("id", &(u64::from(ENCHANT) << 8)),
            ("enchant_id", &ENCHANT),
            ("kind", &lyracore_shared::item_property::STRENGTH),
            ("amount", &1i32),
        ],
    );
    f.shard.assert_sql(&format!(
        "UPDATE game_item_template SET class = 2 WHERE entry = {FIXTURE_BLADE}"
    ));
    f.grant_at(FIXTURE_BLADE, 23);
    f.grant_at(STRANGE_DUST, 24);
    f.offer_for_gold(23);

    refused(
        &f.shard,
        "gw_enchant_item",
        &[ACTOR, "23", &ENCHANT.to_string()],
        ItemRefusal::NotRightNow.as_tag(),
    );
    assert_eq!(f.item(FIXTURE_BLADE)[0]["enchant_id"], "0");
    assert_eq!(f.item(STRANGE_DUST).len(), 1, "the reagent is kept");

    f.shard.assert_call("gw_clear_trade_item", &[ACTOR, "0"]);
    f.shard
        .assert_call("gw_enchant_item", &[ACTOR, "23", &ENCHANT.to_string()]);
    assert_eq!(f.item(FIXTURE_BLADE)[0]["enchant_id"], ENCHANT.to_string());
    assert!(f.item(STRANGE_DUST).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_commit_whose_offered_item_is_gone_or_reowned_cancels_and_moves_no_gold() {
    let f = fixture("trade-offered-gone");
    let removals = [
        "DELETE FROM game_item_instance WHERE guid = {item}",
        "UPDATE game_item_instance SET owner_guid = 999 WHERE guid = {item}",
    ];
    for (round, removal) in removals.iter().enumerate() {
        let item = f.grant_at(FIXTURE_JERKY, 23);
        f.offer_for_gold(23);
        f.shard.assert_call("gw_accept_trade", &[ACTOR]);
        f.shard.assert_sql(&removal.replace("{item}", &item));

        f.shard.assert_call("gw_accept_trade", &[&f.partner]);

        assert_eq!(f.money("1"), PURSE, "{removal}");
        assert_eq!(f.money(&f.other), PURSE, "{removal}");
        assert!(f
            .shard
            .query_rows("SELECT id FROM game_trade_session")
            .is_empty());
        for guid in ["1", f.other.as_str()] {
            assert_eq!(
                f.events(guid, event_kind::TRADE_CANCELED),
                round + 1,
                "{removal}"
            );
            assert_eq!(f.events(guid, event_kind::TRADE_COMPLETE), 0, "{removal}");
        }
        f.shard.assert_sql("DELETE FROM game_item_instance");
    }
}
