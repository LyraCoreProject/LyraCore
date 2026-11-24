mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, FIXTURE_JERKY, FIXTURE_REINS};
use lyracore_shared::constants::unit_flags::IN_COMBAT;
use lyracore_shared::mail::{ITEM_CANNOT_BE_UNEQUIPPED, ITEM_IN_BANK_OR_BAG_SLOT};
use support::{refused, Standalone};

const ACTOR: &str = r#"{"guid":1,"ownership":null}"#;
const CHEST_SLOT: u8 = 4;
const MAIN_HAND_SLOT: u8 = 15;
const FIRST_BAG_SLOT: u8 = 19;
const BACKPACK_SLOT: u8 = 23;
const BANK_SLOT: u8 = 39;
const BANK_BAG_SLOT: u8 = 63;
const FIRST_BAG_CONTENT_SLOT: u8 = 120;
const MAGE: u8 = 8;

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
    shard.assert_sql(&format!(
        "UPDATE game_character SET class = {MAGE} WHERE guid = 1"
    ));
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call(
        "create_character",
        &["1", "\"Other\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let other =
        shard.query_rows("SELECT guid FROM game_character WHERE name = 'Other'")[0]["guid"].clone();
    shard.assert_sql("DELETE FROM game_item_instance");
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET bonding = 0 \
         WHERE entry = {FIXTURE_BLADE} OR entry = {FIXTURE_REINS}"
    ));
    shard.assert_call("debug_set_money", &["1", "1000"]);
    Fixture { shard, other }
}

impl Fixture {
    fn grant_at(&self, entry: u32, slot: u8) -> String {
        self.shard
            .assert_call("debug_grant_item", &["1", &entry.to_string(), "1"]);
        self.shard.assert_sql(&format!(
            "UPDATE game_item_instance SET slot = {slot}, soulbound = false \
             WHERE owner_guid = 1 AND entry = {entry}"
        ));
        self.shard.query_rows(&format!(
            "SELECT guid FROM game_item_instance WHERE entry = {entry}"
        ))[0]["guid"]
            .clone()
    }

    fn move_to(&self, item_guid: &str, slot: u8) {
        self.shard.assert_sql(&format!(
            "UPDATE game_item_instance SET slot = {slot} WHERE guid = {item_guid}"
        ));
    }

    fn max_health(&self) -> u32 {
        support::number(
            &self
                .shard
                .query_rows("SELECT max_health FROM game_world_entity WHERE guid = 1")[0],
            "max_health",
        )
    }

    fn mail_args<'a>(&'a self, item_guid: &'a str) -> [&'a str; 8] {
        [
            ACTOR,
            self.other.as_str(),
            "\"attachment\"",
            "\"\"",
            "0",
            "0",
            item_guid,
            "true",
        ]
    }

    fn assert_refused_in_place(&self, item_guid: &str, slot: u8, refusal: &str) {
        refused(
            &self.shard,
            "realm_mail_send",
            &self.mail_args(item_guid),
            refusal,
        );
        let held = self.shard.query_rows(&format!(
            "SELECT owner_guid, slot FROM game_item_instance WHERE guid = {item_guid}"
        ));
        assert_eq!(held.len(), 1, "item {item_guid} is kept");
        assert_eq!(
            (held[0]["owner_guid"].as_str(), held[0]["slot"].as_str()),
            ("1", slot.to_string().as_str())
        );
        assert!(self.shard.query_rows("SELECT id FROM game_mail").is_empty());
    }

    fn assert_mailed(&self, item_guid: &str, entry: u32) {
        self.shard
            .assert_call("realm_mail_send", &self.mail_args(item_guid));
        assert!(self
            .shard
            .query_rows(&format!(
                "SELECT guid FROM game_item_instance WHERE guid = {item_guid}"
            ))
            .is_empty());
        let mailed = self.shard.query_rows(&format!(
            "SELECT item_entry FROM game_mail WHERE recipient_guid = {}",
            self.other
        ));
        assert!(
            mailed
                .iter()
                .any(|row| row["item_entry"] == entry.to_string()),
            "{mailed:?}"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_equipped_bag_cannot_be_mailed_empty_or_with_contents() {
    let f = fixture("mail-equipped-bag");
    f.shard.assert_sql(&format!(
        "UPDATE game_item_template SET class = 1, subclass = 0, inventory_type = 18, \
         container_slots = 4, max_stack = 1 WHERE entry = {FIXTURE_BLADE}"
    ));
    let bag = f.grant_at(FIXTURE_BLADE, FIRST_BAG_SLOT);
    f.assert_refused_in_place(&bag, FIRST_BAG_SLOT, ITEM_IN_BANK_OR_BAG_SLOT);

    let jerky = f.grant_at(FIXTURE_JERKY, FIRST_BAG_CONTENT_SLOT);
    f.assert_refused_in_place(&bag, FIRST_BAG_SLOT, ITEM_IN_BANK_OR_BAG_SLOT);
    let contents = f.shard.query_rows(&format!(
        "SELECT slot FROM game_item_instance WHERE guid = {jerky}"
    ));
    assert_eq!(contents[0]["slot"], FIRST_BAG_CONTENT_SLOT.to_string());

    f.move_to(&jerky, BACKPACK_SLOT + 1);
    f.move_to(&bag, BACKPACK_SLOT);
    f.assert_mailed(&bag, FIXTURE_BLADE);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_equipped_weapon_is_mailed_and_its_stats_leave_the_character() {
    let f = fixture("mail-equipped-weapon");
    f.shard.assert_sql(&format!(
        "UPDATE game_item_template SET stat_stamina = 10 WHERE entry = {FIXTURE_BLADE}"
    ));
    let bare_health = f.max_health();
    let blade = f.grant_at(FIXTURE_BLADE, BACKPACK_SLOT);
    f.shard
        .assert_call("debug_equip_item", &["1", &BACKPACK_SLOT.to_string()]);
    assert!(f.max_health() > bare_health, "the blade's stamina applies");

    f.assert_mailed(&blade, FIXTURE_BLADE);
    assert_eq!(f.max_health(), bare_health);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn in_combat_equipped_armour_is_refused_and_an_equipped_weapon_is_mailed() {
    let f = fixture("mail-equipped-in-combat");
    f.shard.assert_sql(&format!(
        "UPDATE game_item_template SET class = 4, subclass = 1, inventory_type = 5 \
         WHERE entry = {FIXTURE_REINS}"
    ));
    let robe = f.grant_at(FIXTURE_REINS, CHEST_SLOT);
    let blade = f.grant_at(FIXTURE_BLADE, MAIN_HAND_SLOT);
    f.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {IN_COMBAT}, combat_active = true, \
         combat_until_ms = 1800000000000 WHERE guid = 1"
    ));

    f.assert_refused_in_place(&robe, CHEST_SLOT, ITEM_CANNOT_BE_UNEQUIPPED);
    f.assert_mailed(&blade, FIXTURE_BLADE);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_bank_item_and_a_bank_bag_cannot_be_mailed() {
    let f = fixture("mail-bank-item");
    let reins = f.grant_at(FIXTURE_REINS, BANK_SLOT);
    f.assert_refused_in_place(&reins, BANK_SLOT, ITEM_IN_BANK_OR_BAG_SLOT);
    f.move_to(&reins, BANK_BAG_SLOT);
    f.assert_refused_in_place(&reins, BANK_BAG_SLOT, ITEM_IN_BANK_OR_BAG_SLOT);

    f.move_to(&reins, BACKPACK_SLOT);
    f.assert_mailed(&reins, FIXTURE_REINS);
}
