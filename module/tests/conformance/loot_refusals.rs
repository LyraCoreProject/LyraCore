use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, FIXTURE_JERKY, TEST_WOLF_ENTRY};
use lyracore_shared::guid;
use lyracore_test_support::{ReducerConnection, Standalone};

use serde_json::json;

use super::buy_in_slot::request;
use super::{enter, fixture, Packet};

const CMSG_LOOT: u32 = 0x15d;
const CMSG_LOOT_MONEY: u32 = 0x15e;
const CMSG_LOOT_RELEASE: u32 = 0x15f;
const CMSG_AUTOSTORE_LOOT_ITEM: u32 = 0x108;
const SMSG_LOOT_RESPONSE: u16 = 0x160;
const SMSG_LOOT_RELEASE_RESPONSE: u16 = 0x161;
const SMSG_LOOT_REMOVED: u16 = 0x162;
const SMSG_INVENTORY_CHANGE_FAILURE: u16 = 0x112;

const INVENTORY_FULL: u8 = 0x32;
const CANT_CARRY_MORE_OF_THIS: u8 = 0x11;
const ALREADY_LOOTED: u8 = 0x31;
const LOOT_ERROR_DIDNT_KILL: u8 = 0;
const LOOT_ERROR_TOO_FAR: u8 = 4;

struct Stage {
    shard: Standalone,
    connection: ReducerConnection,
    character: u64,
    account: String,
    corpse: u64,
}

fn stage(name: &str) -> Stage {
    let (shard, character, account) = fixture(name);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&character.to_string()]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&character.to_string(), &TEST_WOLF_ENTRY.to_string(), "2.0"],
    );
    // The seeded demo wolf shares the entry; the one spawned here has the largest guid.
    let corpse = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    shard.assert_call(
        "debug_kill_creature",
        &[&character.to_string(), &corpse.to_string()],
    );
    shard.assert_sql(&format!(
        "DELETE FROM game_corpse_loot WHERE corpse_guid = {corpse}"
    ));
    shard.insert_row(
        "game_corpse_loot_eligible",
        &[
            ("id", &900_002),
            ("corpse_guid", &corpse),
            ("eligible_guid", &character),
        ],
    );
    shard.insert_row(
        "game_corpse_loot",
        &[
            ("id", &900_001),
            ("corpse_guid", &corpse),
            ("slot", &0),
            ("item_entry", &FIXTURE_JERKY),
            ("count", &1),
        ],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_stack = 1 WHERE entry = {FIXTURE_BLADE} OR entry = {FIXTURE_JERKY}"
    ));
    let mut connection = enter(&shard, character);
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {character}"
    ));
    connection.drain_packets();
    Stage {
        shard,
        connection,
        character,
        account,
        corpse,
    }
}

fn open_corpse_on(connection: &mut ReducerConnection, corpse: u64) {
    let opened = request(connection, CMSG_LOOT, corpse.to_le_bytes().into());
    // A loot error shares the opcode; a real window has a nonzero loot type after the guid.
    assert!(opened
        .iter()
        .any(|(opcode, body)| *opcode == SMSG_LOOT_RESPONSE && body[8] != 0));
}

impl Stage {
    fn open_corpse(&mut self) {
        open_corpse_on(&mut self.connection, self.corpse);
        assert_eq!(self.open_loot(), self.corpse.to_string());
    }

    fn second_looter(&self) -> ReducerConnection {
        self.shard.assert_call(
            "create_character",
            &[
                &self.account,
                "\"Second\"",
                "1",
                "1",
                "0",
                "0",
                "0",
                "0",
                "0",
                "0",
            ],
        );
        let second: u64 = self
            .shard
            .query_rows("SELECT guid FROM game_character WHERE name = 'Second'")[0]["guid"]
            .parse()
            .unwrap();
        self.shard
            .assert_call("debug_spawn_player_entity", &[&second.to_string()]);
        self.shard.insert_row(
            "game_corpse_loot_eligible",
            &[
                ("id", &900_003),
                ("corpse_guid", &self.corpse),
                ("eligible_guid", &second),
            ],
        );
        let mut connection = enter(&self.shard, second);
        connection.drain_packets();
        connection
    }

    fn removed_recipients_of_take(&mut self, slot: u8) -> Vec<u128> {
        self.connection
            .call("native_world_receive", json!([0x1dcu32, vec![0u8; 8]]))
            .unwrap();
        self.connection.drain_packets();
        self.connection
            .call(
                "native_world_receive",
                json!([CMSG_AUTOSTORE_LOOT_ITEM, vec![slot]]),
            )
            .unwrap();
        let mut recipients: Vec<u128> = self
            .connection
            .drain_packets()
            .iter()
            .filter_map(|row| {
                let Packet(_, (_, recipient), (_, opcode), body, ..) =
                    serde_json::from_str(row.get()).unwrap();
                (serde_json::from_str::<u16>(opcode.get()).unwrap() == SMSG_LOOT_REMOVED
                    && u8::from_str_radix(&body, 16) == Ok(slot))
                .then(|| serde_json::from_str::<(u128,)>(recipient.get()).unwrap().0)
            })
            .collect();
        recipients.sort_unstable();
        recipients
    }

    fn corpse_money(&self) -> String {
        self.shard.query_rows(&format!(
            "SELECT money FROM game_world_entity WHERE guid = {}",
            self.corpse
        ))[0]["money"]
            .clone()
    }

    fn take(&mut self, slot: u8) -> Vec<(u16, Vec<u8>)> {
        request(&mut self.connection, CMSG_AUTOSTORE_LOOT_ITEM, vec![slot])
    }

    fn release(&mut self, named: u64) -> Vec<(u16, Vec<u8>)> {
        request(
            &mut self.connection,
            CMSG_LOOT_RELEASE,
            named.to_le_bytes().into(),
        )
    }

    fn open_loot(&self) -> String {
        self.shard.query_rows(&format!(
            "SELECT open_loot FROM game_native_player WHERE character_guid = {}",
            self.character
        ))[0]["open_loot"]
            .clone()
    }

    fn fill_backpack(&self) {
        for _ in 0..16 {
            self.shard.assert_call(
                "debug_grant_item",
                &[&self.character.to_string(), &FIXTURE_BLADE.to_string(), "1"],
            );
        }
        assert_eq!(self.carried(FIXTURE_BLADE), 16);
    }

    fn carried(&self, entry: u32) -> usize {
        self.shard
            .query_rows(&format!(
                "SELECT guid FROM game_item_instance WHERE owner_guid = {} AND entry = {entry}",
                self.character
            ))
            .len()
    }

    fn loot_rows(&self) -> usize {
        self.shard
            .query_rows(&format!(
                "SELECT id FROM game_corpse_loot WHERE corpse_guid = {}",
                self.corpse
            ))
            .len()
    }
}

fn inventory_failure(reason: u8) -> (u16, Vec<u8>) {
    let mut body = vec![reason];
    body.extend([0; 17]);
    (SMSG_INVENTORY_CHANGE_FAILURE, body)
}

fn loot_error(target: u64, error: u8) -> (u16, Vec<u8>) {
    let mut body = target.to_le_bytes().to_vec();
    body.extend([0, error]);
    (SMSG_LOOT_RESPONSE, body)
}

fn assert_loot_error(packets: &[(u16, Vec<u8>)], target: u64, error: u8) {
    assert_eq!(packets, [loot_error(target, error)]);
}

fn release_response(target: u64) -> (u16, Vec<u8>) {
    let mut body = target.to_le_bytes().to_vec();
    body.push(1);
    (SMSG_LOOT_RELEASE_RESPONSE, body)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_take_into_a_full_backpack_answers_inventory_full_and_keeps_the_window() {
    let mut stage = stage("loot-take-full-bag");
    stage.open_corpse();
    stage.fill_backpack();

    assert_eq!(stage.take(0), [inventory_failure(INVENTORY_FULL)]);

    assert_eq!(stage.loot_rows(), 1);
    assert_eq!(stage.carried(FIXTURE_JERKY), 0);
    assert_eq!(stage.open_loot(), stage.corpse.to_string());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_take_of_a_unique_item_already_held_answers_cant_carry_more_of_this() {
    let mut stage = stage("loot-take-unique-held");
    stage.shard.assert_sql(&format!(
        "UPDATE game_item_template SET max_count = 1 WHERE entry = {FIXTURE_JERKY}"
    ));
    stage.shard.assert_call(
        "debug_grant_item",
        &[
            &stage.character.to_string(),
            &FIXTURE_JERKY.to_string(),
            "1",
        ],
    );
    stage.open_corpse();

    assert_eq!(stage.take(0), [inventory_failure(CANT_CARRY_MORE_OF_THIS)]);

    assert_eq!(stage.loot_rows(), 1);
    assert_eq!(stage.carried(FIXTURE_JERKY), 1);
    assert_eq!(stage.open_loot(), stage.corpse.to_string());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_take_from_an_empty_slot_answers_already_looted() {
    let mut stage = stage("loot-take-empty-slot");
    stage.open_corpse();

    assert_eq!(stage.take(7), [inventory_failure(ALREADY_LOOTED)]);

    assert_eq!(stage.loot_rows(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_take_from_a_vanished_creature_corpse_answers_a_loot_error() {
    let mut stage = stage("loot-take-vanished-corpse");
    stage.open_corpse();
    stage.shard.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {}",
        stage.corpse
    ));

    let packets = stage.take(0);

    assert_loot_error(&packets, stage.corpse, LOOT_ERROR_DIDNT_KILL);
    assert_eq!(stage.open_loot(), stage.corpse.to_string());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_take_from_a_creature_corpse_out_of_reach_answers_too_far() {
    let mut stage = stage("loot-take-out-of-reach");
    stage.open_corpse();
    stage.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 500 WHERE guid = {}",
        stage.character
    ));

    let packets = stage.take(0);

    assert_loot_error(&packets, stage.corpse, LOOT_ERROR_TOO_FAR);
    assert_eq!(stage.loot_rows(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_take_from_a_vanished_gameobject_releases_the_window() {
    let mut stage = stage("loot-take-vanished-gameobject");
    let chest = guid::compose(guid::GAMEOBJECT, 99);
    stage.shard.assert_sql(&format!(
        "UPDATE game_native_player SET open_loot = {chest} WHERE character_guid = {}",
        stage.character
    ));

    assert_eq!(stage.take(0), [release_response(chest)]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_take_without_an_open_window_is_silent() {
    let mut stage = stage("loot-take-no-window");

    assert!(stage.take(0).is_empty());

    assert_eq!(stage.loot_rows(), 1);
    assert_eq!(stage.carried(FIXTURE_JERKY), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_release_closes_the_stored_window_whatever_guid_the_client_names() {
    let mut stage = stage("loot-release-stored-window");
    stage.open_corpse();

    assert_eq!(stage.release(0xdead), [release_response(stage.corpse)]);
    assert_eq!(stage.open_loot(), "0");
    assert!(stage.release(stage.corpse).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_take_tells_every_looter_with_the_window_open() {
    let mut stage = stage("loot-take-removed-for-all");
    let mut second = stage.second_looter();
    stage.open_corpse();
    open_corpse_on(&mut second, stage.corpse);

    let mut expected = vec![stage.connection.connection_id(), second.connection_id()];
    expected.sort_unstable();
    assert_eq!(stage.removed_recipients_of_take(0), expected);
    assert_eq!(stage.carried(FIXTURE_JERKY), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn money_out_of_reach_is_silent_and_stays_on_the_corpse() {
    let mut stage = stage("loot-money-out-of-reach");
    stage.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET money = 17 WHERE guid = {}",
        stage.corpse
    ));
    stage.open_corpse();
    stage.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 500 WHERE guid = {}",
        stage.character
    ));

    assert!(request(&mut stage.connection, CMSG_LOOT_MONEY, Vec::new()).is_empty());
    assert_eq!(stage.corpse_money(), "17");
}
