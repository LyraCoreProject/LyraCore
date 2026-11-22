use lyracore_package_delta::fixture_ids::FIXTURE_JERKY;
use lyracore_test_support::{ReducerConnection, Standalone};

use super::buy_in_slot::request;
use super::{enter, fixture};

const CMSG_OPEN_ITEM: u32 = 0x0ac;
const CMSG_GAMEOBJ_USE: u32 = 0x0b1;
const CMSG_AUTOSTORE_LOOT_ITEM: u32 = 0x108;
const CMSG_LOOT_MONEY: u32 = 0x15e;
const CMSG_LOOT_RELEASE: u32 = 0x15f;
const SMSG_INVENTORY_CHANGE_FAILURE: u16 = 0x112;
const SMSG_LOOT_RESPONSE: u16 = 0x160;
const SMSG_LOOT_RELEASE_RESPONSE: u16 = 0x161;

const ITEM_LOCKED: u8 = 0x24;
const CANT_DO_RIGHT_NOW: u8 = 0x27;
const ITEM_FLAG_LOOTABLE: u32 = 0x4;
const BACKPACK: u8 = 255;
const LOOT_CORPSE: u8 = 1;

const CLAM: u32 = 900_501;
const LOCKBOX: u32 = 900_502;
const LOCKBOX_LOCK: u32 = 900_503;
const COIN_CHEST: u32 = 900_601;
const CLAM_COPPER: u32 = 37;
const CHEST_COPPER: u32 = 25;

struct Stage {
    shard: Standalone,
    connection: ReducerConnection,
    character: u64,
}

fn stage(name: &str) -> Stage {
    let (shard, character, _) = fixture(name);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&character.to_string()]);
    let mut connection = enter(&shard, character);
    shard.assert_sql(&format!(
        "DELETE FROM game_item_instance WHERE owner_guid = {character}"
    ));
    connection.drain_packets();
    Stage {
        shard,
        connection,
        character,
    }
}

impl Stage {
    fn lootable_template(&self, entry: u32, lock_id: u32, copper: u32) {
        self.shard.insert_row(
            "game_item_template",
            &[
                ("entry", &entry),
                ("class", &15),
                ("name", &"Fixture lootable"),
                ("max_stack", &1),
                ("item_flags", &ITEM_FLAG_LOOTABLE),
                ("lock_id", &lock_id),
                ("money_min", &copper),
                ("money_max", &copper),
            ],
        );
    }

    fn grant(&self, entry: u32) -> (u64, u8) {
        self.shard.assert_call(
            "debug_grant_item",
            &[&self.character.to_string(), &entry.to_string(), "1"],
        );
        let row = self.shard.query_rows(&format!(
            "SELECT guid, slot FROM game_item_instance WHERE owner_guid = {} AND entry = {entry}",
            self.character
        ))[0]
            .clone();
        (row["guid"].parse().unwrap(), row["slot"].parse().unwrap())
    }

    fn open(&mut self, slot: u8) -> Vec<(u16, Vec<u8>)> {
        request(&mut self.connection, CMSG_OPEN_ITEM, vec![BACKPACK, slot])
    }

    fn money(&self) -> u32 {
        self.shard.query_rows(&format!(
            "SELECT money FROM game_world_entity WHERE guid = {}",
            self.character
        ))[0]["money"]
            .parse()
            .unwrap()
    }

    fn held(&self, entry: u32) -> usize {
        self.shard
            .query_rows(&format!(
                "SELECT guid FROM game_item_instance WHERE owner_guid = {} AND entry = {entry}",
                self.character
            ))
            .len()
    }

    fn open_loot(&self) -> String {
        self.shard.query_rows(&format!(
            "SELECT open_loot FROM game_native_player WHERE character_guid = {}",
            self.character
        ))[0]["open_loot"]
            .clone()
    }
}

fn window(packets: &[(u16, Vec<u8>)], source: u64) -> Vec<u8> {
    packets
        .iter()
        .find(|(opcode, body)| {
            *opcode == SMSG_LOOT_RESPONSE && body[..8] == source.to_le_bytes() && body[8] != 0
        })
        .unwrap_or_else(|| panic!("no Loot Window on {source}: {packets:?}"))
        .1
        .clone()
}

fn window_money(window: &[u8]) -> u32 {
    u32::from_le_bytes(window[9..13].try_into().unwrap())
}

fn inventory_failure(reason: u8, item: u64) -> (u16, Vec<u8>) {
    let mut body = vec![reason];
    body.extend(item.to_le_bytes());
    body.extend([0; 9]);
    (SMSG_INVENTORY_CHANGE_FAILURE, body)
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn opening_a_clam_gives_its_item_and_money_and_destroys_the_emptied_clam() {
    let mut stage = stage("item-loot-clam");
    stage.lootable_template(CLAM, 0, CLAM_COPPER);
    stage.shard.insert_row(
        "game_item_loot",
        &[
            ("id", &900_501),
            ("lootable_entry", &CLAM),
            ("item_entry", &FIXTURE_JERKY),
            ("chance_bp", &10_000),
            ("count", &1),
            ("min_count", &1),
        ],
    );
    let (clam, slot) = stage.grant(CLAM);
    let money_before = stage.money();

    let window = window(&stage.open(slot), clam);
    assert_eq!(window[8], LOOT_CORPSE);
    assert_eq!(window_money(&window), CLAM_COPPER);
    assert_eq!(window[13], 1, "one item in the window");
    assert_eq!(window[14], 0, "the item sits in loot slot 0");
    assert_eq!(
        u32::from_le_bytes(window[15..19].try_into().unwrap()),
        FIXTURE_JERKY
    );
    assert_eq!(stage.open_loot(), clam.to_string());

    request(&mut stage.connection, CMSG_LOOT_MONEY, Vec::new());
    request(&mut stage.connection, CMSG_AUTOSTORE_LOOT_ITEM, vec![0]);
    let released = request(
        &mut stage.connection,
        CMSG_LOOT_RELEASE,
        clam.to_le_bytes().into(),
    );

    let mut release_body = clam.to_le_bytes().to_vec();
    release_body.push(1);
    assert!(released.contains(&(SMSG_LOOT_RELEASE_RESPONSE, release_body)));
    assert_eq!(stage.money(), money_before + CLAM_COPPER);
    assert_eq!(stage.held(FIXTURE_JERKY), 1);
    assert_eq!(stage.held(CLAM), 0, "the emptied clam is destroyed");
    assert!(stage
        .shard
        .query_rows(&format!(
            "SELECT id FROM game_corpse_loot WHERE corpse_guid = {clam}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn opening_an_item_that_is_not_lootable_is_refused() {
    let mut stage = stage("item-loot-not-lootable");
    let (jerky, slot) = stage.grant(FIXTURE_JERKY);

    assert_eq!(
        stage.open(slot),
        [inventory_failure(CANT_DO_RIGHT_NOW, jerky)]
    );
    assert_eq!(stage.open_loot(), "0");
    assert_eq!(stage.held(FIXTURE_JERKY), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn opening_a_lockbox_that_needs_lockpicking_answers_item_locked() {
    let mut stage = stage("item-loot-locked");
    stage.lootable_template(LOCKBOX, LOCKBOX_LOCK, 0);
    stage.shard.insert_row(
        "game_lock",
        &[
            ("id", &900_503),
            ("lock_id", &LOCKBOX_LOCK),
            ("index", &0),
            ("kind", &2),
            ("property", &633),
            ("required_skill", &1),
        ],
    );
    let (lockbox, slot) = stage.grant(LOCKBOX);

    assert_eq!(stage.open(slot), [inventory_failure(ITEM_LOCKED, lockbox)]);
    assert_eq!(stage.open_loot(), "0");
    assert!(stage
        .shard
        .query_rows(&format!(
            "SELECT source_guid FROM game_loot_money WHERE source_guid = {lockbox}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_chest_with_a_fixed_money_range_shows_that_money() {
    let mut stage = stage("item-loot-chest-money");
    stage.shard.insert_row(
        "game_gameobject_template",
        &[
            ("entry", &COIN_CHEST),
            ("type_id", &3),
            ("name", &"Fixture coin chest"),
            ("money_min", &CHEST_COPPER),
            ("money_max", &CHEST_COPPER),
        ],
    );
    let at = stage.shard.query_rows(&format!(
        "SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {}",
        stage.character
    ))[0]
        .clone();
    stage.shard.assert_call(
        "debug_spawn_gameobject",
        &[
            &COIN_CHEST.to_string(),
            "3",
            "0",
            "0",
            &at["map_id"],
            &at["x"],
            &at["y"],
            &at["z"],
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let chest: u64 = stage.shard.query_rows(&format!(
        "SELECT guid FROM game_gameobject WHERE template_entry = {COIN_CHEST}"
    ))[0]["guid"]
        .parse()
        .unwrap();
    let money_before = stage.money();

    let window = window(
        &request(
            &mut stage.connection,
            CMSG_GAMEOBJ_USE,
            chest.to_le_bytes().into(),
        ),
        chest,
    );
    assert_eq!(window_money(&window), CHEST_COPPER);
    assert_eq!(window[13], 0, "the chest holds no items");

    request(&mut stage.connection, CMSG_LOOT_MONEY, Vec::new());
    assert_eq!(stage.money(), money_before + CHEST_COPPER);
}
