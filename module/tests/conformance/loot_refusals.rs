use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, FIXTURE_JERKY, TEST_WOLF_ENTRY};
use lyracore_shared::guid;
use lyracore_test_support::{NativeAudience, ReducerConnection, Standalone};

use serde_json::json;

use super::buy_in_slot::{delivered_until, request};
use super::{enter, fixture};

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
    corpse: u64,
}

fn spawn_wolf(name: &str) -> (Standalone, u64, u64) {
    let (shard, character, _) = fixture(name);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&character.to_string()]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&character.to_string(), &TEST_WOLF_ENTRY.to_string(), "2.0"],
    );
    // The seeded demo wolf shares the entry; the one spawned here has the largest guid.
    let wolf = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    (shard, character, wolf)
}

fn stage(name: &str) -> Stage {
    let (shard, character, corpse) = spawn_wolf(name);
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
        corpse,
    }
}

/// Opens a Loot Window on `corpse` and returns the slots it lists.
fn open_corpse_on(connection: &mut ReducerConnection, corpse: u64) -> Vec<u8> {
    let opened = request(connection, CMSG_LOOT, corpse.to_le_bytes().into());
    // A loot error shares the opcode; a real window has a nonzero loot type after the guid.
    let (_, body) = opened
        .iter()
        .find(|(opcode, body)| *opcode == SMSG_LOOT_RESPONSE && body[8] != 0)
        .expect("a Loot Window opens");
    // Guid, loot type and money take 13 bytes, then the item count and 22 bytes per item.
    (0..usize::from(body[13]))
        .map(|item| body[14 + item * 22])
        .collect()
}

impl Stage {
    fn open_corpse(&mut self) -> Vec<u8> {
        let listed = open_corpse_on(&mut self.connection, self.corpse);
        assert_eq!(self.open_loot(), self.corpse.to_string());
        listed
    }

    fn second_looter(&self) -> (u64, ReducerConnection) {
        let second = provision_second(&self.shard);
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
        (second, connection)
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
            .filter_map(|packet| match packet.audience {
                NativeAudience::Connection(connection)
                    if packet.opcode == Some(SMSG_LOOT_REMOVED) && packet.body == [slot] =>
                {
                    Some(connection)
                }
                _ => None,
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
        carried_by(&self.shard, self.character, entry)
    }

    fn corpse_flag_lootable(&self) -> bool {
        let flags: u32 = self.shard.query_rows(&format!(
            "SELECT dynamic_flags FROM game_world_entity WHERE guid = {}",
            self.corpse
        ))[0]["dynamic_flags"]
            .parse()
            .unwrap();
        flags & 1 != 0
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

// A native player row is unique per Account, so the second looter needs its own.
fn provision_second(shard: &Standalone) -> u64 {
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"SECOND\"", &credentials, &credentials],
    );
    let account =
        shard.query_rows("SELECT id FROM game_account WHERE username = 'SECOND'")[0]["id"].clone();
    shard.assert_call(
        "create_character",
        &[
            &account,
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
    let second: u64 = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Second'")[0]
        ["guid"]
        .parse()
        .unwrap();
    shard.assert_call("debug_spawn_player_entity", &[&second.to_string()]);
    second
}

fn carried_by(shard: &Standalone, character: u64, entry: u32) -> usize {
    shard
        .query_rows(&format!(
            "SELECT guid FROM game_item_instance WHERE owner_guid = {character} AND entry = {entry}"
        ))
        .len()
}

const JERKY_QUEST: u32 = 9_141_001;

fn give_jerky_quest(shard: &Standalone, characters: &[u64]) {
    insert_jerky_quest(shard);
    grant_jerky_quest(shard, characters);
}

fn insert_jerky_quest(shard: &Standalone) {
    shard.insert_row(
        "game_quest_template",
        &[
            ("entry", &JERKY_QUEST),
            ("quest_level", &1),
            ("title", &"Jerky for two"),
        ],
    );
    shard.insert_row(
        "game_quest_objective",
        &[
            ("id", &9_141_002),
            ("quest_entry", &JERKY_QUEST),
            ("kind", &1),
            ("target_entry", &FIXTURE_JERKY),
            ("required_count", &1),
        ],
    );
}

fn grant_jerky_quest(shard: &Standalone, characters: &[u64]) {
    for character in characters {
        shard.assert_call(
            "debug_grant_quest",
            &[&character.to_string(), &JERKY_QUEST.to_string()],
        );
    }
}

const ITEM_FLAG_PARTY_LOOT: u32 = 0x800;
const PARTY: u64 = 9_141_010;

// Two grouped needers share the Loot Tag, so the wolf's death fills loot for both.
fn party_loot_stage(name: &str) -> (Stage, u64, ReducerConnection) {
    let (shard, character, corpse) = spawn_wolf(name);
    let second = provision_second(&shard);
    give_jerky_quest(&shard, &[character, second]);
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET item_flags = {ITEM_FLAG_PARTY_LOOT} WHERE entry = {FIXTURE_JERKY}"
    ));
    shard.assert_sql(&format!(
        "DELETE FROM game_creature_loot WHERE creature_entry = {TEST_WOLF_ENTRY}"
    ));
    shard.insert_row(
        "game_creature_loot",
        &[
            ("id", &9_141_020),
            ("creature_entry", &TEST_WOLF_ENTRY),
            ("item_entry", &FIXTURE_JERKY),
            ("chance_bp", &10_000),
            ("count", &1),
            ("quest_only", &true),
        ],
    );
    shard.insert_row(
        "game_group",
        &[
            ("group_id", &PARTY),
            ("leader_guid", &character),
            ("loot_method", &0),
        ],
    );
    let no_identity = format!("0x{}", "00".repeat(32));
    for (id, member) in [(9_141_011u64, character), (9_141_012, second)] {
        shard.insert_row(
            "game_group_member",
            &[
                ("id", &id),
                ("group_id", &PARTY),
                ("character_guid", &member),
                ("owner_identity", &no_identity),
            ],
        );
        shard.insert_row(
            "game_creature_quest_tap_member",
            &[
                ("id", &id),
                ("creature_guid", &corpse),
                ("character_guid", &member),
            ],
        );
    }
    shard.insert_row(
        "game_creature_quest_tap",
        &[("creature_guid", &corpse), ("character_guid", &character)],
    );
    shard.insert_row(
        "game_creature_loot_tag_group",
        &[("creature_guid", &corpse), ("group_id", &PARTY)],
    );
    shard.assert_call(
        "debug_kill_creature",
        &[&character.to_string(), &corpse.to_string()],
    );
    let mut connection = enter(&shard, character);
    connection.drain_packets();
    let mut second_connection = enter(&shard, second);
    second_connection.drain_packets();
    let stage = Stage {
        shard,
        connection,
        character,
        corpse,
    };
    (stage, second, second_connection)
}

const SKINNING_SKILL_LINE: u32 = 393;

fn quest_drop_stage(name: &str, needs_jerky: bool) -> Stage {
    let (shard, character, corpse) = spawn_wolf(name);
    insert_jerky_quest(&shard);
    if needs_jerky {
        grant_jerky_quest(&shard, &[character]);
    }
    shard.assert_sql(&format!(
        "UPDATE game_creature_template SET money_min = 0, money_max = 0 WHERE entry = {TEST_WOLF_ENTRY}"
    ));
    shard.assert_sql(&format!(
        "DELETE FROM game_creature_loot WHERE creature_entry = {TEST_WOLF_ENTRY}"
    ));
    shard.insert_row(
        "game_creature_loot",
        &[
            ("id", &9_164_001),
            ("creature_entry", &TEST_WOLF_ENTRY),
            ("item_entry", &FIXTURE_JERKY),
            ("chance_bp", &10_000),
            ("count", &1),
            ("quest_only", &true),
        ],
    );
    shard.assert_call(
        "debug_apply_damage",
        &[&corpse.to_string(), "1", &character.to_string()],
    );
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT character_guid FROM game_creature_quest_tap WHERE creature_guid = {corpse}"
            ))
            .len(),
        1,
        "damage must acquire the Loot Tag"
    );
    shard.assert_call(
        "debug_kill_creature",
        &[&character.to_string(), &corpse.to_string()],
    );
    let mut connection = enter(&shard, character);
    connection.drain_packets();
    Stage {
        shard,
        connection,
        character,
        corpse,
    }
}

fn inventory_failure(reason: u8) -> (u16, Vec<u8>) {
    let mut body = vec![reason];
    body.extend([0; 17]);
    (SMSG_INVENTORY_CHANGE_FAILURE, body)
}

/// Keeps the loot and inventory replies. The other Character's movement and visibility
/// broadcasts reach a second connection at any moment, so a whole-reply compare drops them.
fn loot_replies(packets: Vec<(u16, Vec<u8>)>) -> Vec<(u16, Vec<u8>)> {
    packets
        .into_iter()
        .filter(|(opcode, _)| {
            [
                SMSG_LOOT_RESPONSE,
                SMSG_LOOT_RELEASE_RESPONSE,
                SMSG_LOOT_REMOVED,
                SMSG_INVENTORY_CHANGE_FAILURE,
            ]
            .contains(opcode)
        })
        .collect()
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
fn a_character_that_dies_with_a_creature_corpse_open_has_the_window_closed() {
    let mut stage = stage("loot-death-corpse");
    stage.open_corpse();

    stage
        .shard
        .assert_call("debug_set_health", &[&stage.character.to_string(), "0"]);

    let release = release_response(stage.corpse);
    assert!(delivered_until(&mut stage.connection, &release).contains(&release));
    assert_eq!(stage.open_loot(), "0");
    assert_eq!(stage.loot_rows(), 1, "the loot stays on the corpse");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_take_tells_every_looter_with_the_window_open() {
    let mut stage = stage("loot-take-removed-for-all");
    let (_, mut second) = stage.second_looter();
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_item_taken_by_one_needer_is_gone_from_every_needers_window() {
    let mut stage = stage("loot-quest-item-shared");
    stage
        .shard
        .assert_sql("UPDATE game_corpse_loot SET quest_only = true WHERE id = 900001");
    let (second_guid, mut second) = stage.second_looter();
    give_jerky_quest(&stage.shard, &[stage.character, second_guid]);
    stage.open_corpse();
    open_corpse_on(&mut second, stage.corpse);

    let mut expected = vec![stage.connection.connection_id(), second.connection_id()];
    expected.sort_unstable();
    assert_eq!(stage.removed_recipients_of_take(0), expected);
    assert_eq!(stage.carried(FIXTURE_JERKY), 1);
    assert_eq!(stage.loot_rows(), 0);
    assert_eq!(
        loot_replies(request(&mut second, CMSG_AUTOSTORE_LOOT_ITEM, vec![0])),
        [inventory_failure(ALREADY_LOOTED)]
    );
    assert_eq!(carried_by(&stage.shard, second_guid, FIXTURE_JERKY), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn each_needer_takes_a_party_loot_quest_item_from_the_window_already_open() {
    let (mut stage, second_guid, mut second) = party_loot_stage("loot-quest-item-party");
    let slot_of = |character: u64| -> u8 {
        stage.shard.query_rows(&format!(
            "SELECT slot FROM game_corpse_loot WHERE corpse_guid = {} AND reserved_for = {character}",
            stage.corpse
        ))[0]["slot"]
            .parse()
            .unwrap()
    };
    let (first_slot, second_slot) = (slot_of(stage.character), slot_of(second_guid));
    assert_eq!(stage.loot_rows(), 2);
    assert_ne!(first_slot, second_slot);
    assert_eq!(stage.open_corpse(), [first_slot]);
    assert_eq!(open_corpse_on(&mut second, stage.corpse), [second_slot]);

    assert_eq!(
        stage.removed_recipients_of_take(first_slot),
        [stage.connection.connection_id()]
    );
    let taken = request(&mut second, CMSG_AUTOSTORE_LOOT_ITEM, vec![second_slot]);

    assert!(taken.contains(&(SMSG_LOOT_REMOVED, vec![second_slot])));
    assert_eq!(stage.carried(FIXTURE_JERKY), 1);
    assert_eq!(carried_by(&stage.shard, second_guid, FIXTURE_JERKY), 1);
    assert_eq!(stage.loot_rows(), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_needer_whose_objective_completes_cannot_take_their_party_loot_copy() {
    let (mut stage, _, _) = party_loot_stage("loot-quest-item-party-done");
    stage.open_corpse();
    stage.shard.assert_call(
        "debug_grant_item",
        &[
            &stage.character.to_string(),
            &FIXTURE_JERKY.to_string(),
            "1",
        ],
    );
    let slot: u8 = stage.shard.query_rows(&format!(
        "SELECT slot FROM game_corpse_loot WHERE corpse_guid = {} AND reserved_for = {}",
        stage.corpse, stage.character
    ))[0]["slot"]
        .parse()
        .unwrap();

    let packets = stage.take(slot);

    assert_loot_error(&packets, stage.corpse, LOOT_ERROR_DIDNT_KILL);
    assert_eq!(stage.carried(FIXTURE_JERKY), 1);
    assert_eq!(stage.loot_rows(), 2);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_corpse_whose_only_drop_is_an_unneeded_quest_item_is_not_lootable_and_can_be_skinned() {
    let stage = quest_drop_stage("loot-quest-item-unneeded", false);
    assert_eq!(stage.loot_rows(), 0);
    assert!(!stage.corpse_flag_lootable());

    stage.shard.insert_row(
        "game_player_skill",
        &[
            ("id", &9_164_002),
            ("character_guid", &stage.character),
            ("owner_identity", &format!("0x{}", "00".repeat(32))),
            ("skill_line", &SKINNING_SKILL_LINE),
            ("current", &300),
            ("max_rank", &300),
        ],
    );
    // The fixture wolf's skinning loot drops Light Leather.
    stage.shard.insert_row(
        "game_item_template",
        &[
            ("entry", &2318),
            ("name", &"Light Leather"),
            ("max_stack", &20),
        ],
    );
    stage
        .shard
        .assert_call("debug_skin_nearest", &[&stage.character.to_string()]);

    let skinned = stage.shard.query_rows(&format!(
        "SELECT skinned FROM game_world_entity WHERE guid = {}",
        stage.corpse
    ));
    assert_eq!(skinned[0]["skinned"], "true");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_accepted_after_the_kill_cannot_take_the_item_from_that_corpse() {
    let mut stage = quest_drop_stage("loot-quest-item-late", false);
    grant_jerky_quest(&stage.shard, &[stage.character]);

    let opened = request(
        &mut stage.connection,
        CMSG_LOOT,
        stage.corpse.to_le_bytes().into(),
    );

    // The refused skinning answers a window with no items; a window listing the item has count 1.
    let listed: Vec<u8> = opened
        .iter()
        .filter(|(opcode, body)| *opcode == SMSG_LOOT_RESPONSE && body[8] != 0)
        .flat_map(|(_, body)| (0..usize::from(body[13])).map(|item| body[14 + item * 22]))
        .collect();
    assert!(listed.is_empty());
    assert_eq!(stage.loot_rows(), 0);
    assert_eq!(stage.carried(FIXTURE_JERKY), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_quest_item_its_killer_needs_at_the_kill_stays_on_the_corpse() {
    let mut stage = quest_drop_stage("loot-quest-item-needed", true);
    assert_eq!(stage.loot_rows(), 1);
    assert!(stage.corpse_flag_lootable());

    assert_eq!(stage.open_corpse().len(), 1);
    stage.take(0);

    assert_eq!(stage.carried(FIXTURE_JERKY), 1);
    assert_eq!(stage.loot_rows(), 0);
}
