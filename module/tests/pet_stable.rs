mod support;

use lyracore_package_delta::fixture_ids::SCENARIO_VENDOR_ENTRY;
use lyracore_shared::{constants::npc_flags, guid};
use serde_json::json;
use support::{number, poll_until, ReducerConnection, Standalone, POLL_TIMEOUT};

const BOAR: u32 = 51008;
const TAME: &str = "50300";
/// CMSG_PET_ACTION data 0x07000003: a command action carrying COMMAND_DISMISS.
const DISMISS: &str = "117440515";
const MSG_LIST_STABLED_PETS: u32 = 0x026F;
const CMSG_STABLE_PET: u32 = 0x0270;
const CMSG_UNSTABLE_PET: u32 = 0x0271;
const CMSG_BUY_STABLE_SLOT: u32 = 0x0272;
const CMSG_STABLE_SWAP_PET: u32 = 0x0275;
const CMSG_LOGOUT_REQUEST: u32 = 0x004B;
const CMSG_PLAYER_LOGIN: u32 = 0x003D;
const SMSG_LOGOUT_COMPLETE: u16 = 0x004D;
const SMSG_LOGIN_VERIFY_WORLD: u16 = 0x0236;
const SMSG_STABLE_RESULT: u16 = 0x0273;
const SMSG_PET_NAME_QUERY_RESPONSE: u16 = 0x0053;
// vmangos `StableResultCode`.
const ERR_MONEY: u8 = 0x01;
const ERR_STABLE: u8 = 0x06;
const SUCCESS_STABLE: u8 = 0x08;
const SUCCESS_UNSTABLE: u8 = 0x09;
const SUCCESS_BUY_SLOT: u8 = 0x0A;
// `PetPresence` codes in `game_hunter_pet.presence`.
const DISMISSED: u8 = 0;
const DEAD: u8 = 2;

/// One MSG_LIST_STABLED_PETS entry: pet number, entry, level, name, loyalty level, client slot.
type Listed = (u32, u32, u32, String, u32, u8);

/// One `game_hunter_pet` row: pet number, stable slot and presence.
type Pet = (u32, u8, u8);

struct Stable {
    shard: Standalone,
    wire: ReducerConnection,
    hunter: u64,
    master: u64,
}

fn send(wire: &mut ReducerConnection, opcode: u32, body: Vec<u8>) -> Vec<(u16, Vec<u8>)> {
    wire.drain_packets();
    if let Err(reason) = wire.call("native_world_receive", json!([opcode, body])) {
        let replies: Vec<(u16, Vec<u8>)> =
            serde_json::from_str(reason.strip_prefix("native_reply:").expect(&reason)).unwrap();
        wire.call("native_world_reply", json!([replies])).unwrap();
    }
    let mut packets = wire.drain_packets();
    packets.sort_by_key(|packet| packet.id);
    packets
        .into_iter()
        .filter_map(|packet| packet.opcode.map(|opcode| (opcode, packet.body)))
        .collect()
}

fn spawn_beside(shard: &Standalone, owner: u64, entry: u32, distance: &str) -> u64 {
    let unowned = || {
        shard.query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {entry} AND owner_guid = 0"
        ))
    };
    let before = unowned();
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&owner.to_string(), &entry.to_string(), distance],
    );
    let added: Vec<_> = unowned()
        .into_iter()
        .filter(|row| !before.contains(row))
        .collect();
    assert_eq!(added.len(), 1);
    number(&added[0], "guid")
}

/// A level 60 Hunter in the world beside a friendly Stable Master.
fn stable(name: &str) -> Stable {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"STABLE\"", &credentials, &credentials],
    );
    let account =
        shard.query_rows("SELECT id FROM game_account WHERE username = 'STABLE'")[0]["id"].clone();
    // Race 3 (Dwarf) with class 3 (Hunter); the remaining arguments are appearance.
    shard.assert_call(
        "create_character",
        &[
            &account,
            "\"Stabler\"",
            "3",
            "3",
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    let hunter: u64 = number(
        &shard.query_rows("SELECT guid FROM game_character WHERE name = 'Stabler'")[0],
        "guid",
    );
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([hunter]))
        .unwrap();
    shard.assert_call("debug_set_level", &[&hunter.to_string(), "60"]);
    let master = spawn_beside(&shard, hunter, SCENARIO_VENDOR_ENTRY, "2");
    // Faction template 35 is friendly to every Character.
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = {}, faction_template = 35 WHERE guid = {master}",
        npc_flags::STABLEMASTER
    ));
    Stable {
        shard,
        wire,
        hunter,
        master,
    }
}

impl Stable {
    fn request(&mut self, opcode: u32, pet_number: Option<u32>) -> Vec<(u16, Vec<u8>)> {
        let mut body = self.master.to_le_bytes().to_vec();
        body.extend(pet_number.into_iter().flat_map(u32::to_le_bytes));
        send(&mut self.wire, opcode, body)
    }

    fn results(&mut self, opcode: u32, pet_number: Option<u32>) -> Vec<u8> {
        self.request(opcode, pet_number)
            .into_iter()
            .filter(|(opcode, _)| *opcode == SMSG_STABLE_RESULT)
            .map(|(_, body)| body[0])
            .collect()
    }

    /// The bought slot count and the listed pets, or `None` when the listing has no reply.
    fn list(&mut self) -> Option<(u8, Vec<Listed>)> {
        let packets = self.request(MSG_LIST_STABLED_PETS, None);
        let (_, body) = packets
            .into_iter()
            .find(|(opcode, _)| u32::from(*opcode) == MSG_LIST_STABLED_PETS)?;
        assert_eq!(body[..8], self.master.to_le_bytes());
        let word = |at: usize| u32::from_le_bytes(body[at..at + 4].try_into().unwrap());
        let mut at = 10;
        let mut pets = Vec::new();
        for _ in 0..body[8] {
            let name_end = at + 12 + body[at + 12..].iter().position(|b| *b == 0).unwrap();
            let name = String::from_utf8(body[at + 12..name_end].to_vec()).unwrap();
            pets.push((
                word(at),
                word(at + 4),
                word(at + 8),
                name,
                word(name_end + 1),
                body[name_end + 5],
            ));
            at = name_end + 6;
        }
        assert_eq!(at, body.len());
        Some((body[9], pets))
    }

    fn tame(&self) -> u32 {
        let wild = spawn_beside(&self.shard, self.hunter, BOAR, "5").to_string();
        let owner = self.hunter.to_string();
        assert!(
            poll_until(POLL_TIMEOUT, || self
                .shard
                .call("debug_force_cast_at", &[&owner, TAME, &wild])
                .status
                .success()),
            "the tame must resolve after the global cooldown"
        );
        self.current().0
    }

    /// The owner's pets in stable slot order; the Current Pet comes first.
    fn pets(&self) -> Vec<Pet> {
        let mut pets: Vec<Pet> = self
            .shard
            .query_rows(&format!(
                "SELECT pet_id, stable_slot, presence FROM game_hunter_pet WHERE owner_guid = {}",
                self.hunter
            ))
            .iter()
            .map(|row| {
                (
                    number::<u64>(row, "pet_id") as u32,
                    number(row, "stable_slot"),
                    number(row, "presence"),
                )
            })
            .collect();
        pets.sort_by_key(|pet| pet.1);
        pets
    }

    fn current(&self) -> Pet {
        let pets = self.pets();
        assert_eq!(pets[0].1, 0, "the Hunter has a Current Pet: {pets:?}");
        pets[0]
    }

    fn live_pet_level(&self) -> Option<u32> {
        let pet = guid::compose(guid::UNIT, guid::low(self.hunter));
        self.shard
            .query_rows(&format!(
                "SELECT level FROM game_world_entity WHERE guid = {pet}"
            ))
            .pop()
            .map(|row| number(&row, "level"))
    }

    fn money(&self) -> u32 {
        number(
            &self.shard.query_rows(&format!(
                "SELECT money FROM game_world_entity WHERE guid = {}",
                self.hunter
            ))[0],
            "money",
        )
    }

    fn set_money(&self, copper: u32) {
        self.shard.assert_sql(&format!(
            "UPDATE game_world_entity SET money = {copper} WHERE guid = {}",
            self.hunter
        ));
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hunter_buys_a_slot_stables_its_pet_and_swaps_it_for_a_second_tame() {
    let mut stable = stable("pet-stable-swap");
    let first = stable.tame();
    stable.shard.assert_sql(&format!(
        "UPDATE game_hunter_pet SET name = 'Fang', level = 12 WHERE owner_guid = {}",
        stable.hunter
    ));
    assert_eq!(
        stable.list(),
        Some((0, vec![(first, BOAR, 12, "Fang".to_owned(), 1, 1)]))
    );
    assert_eq!(
        stable.results(CMSG_STABLE_PET, None),
        [ERR_STABLE],
        "no stable slot is bought"
    );

    stable.set_money(600);
    assert_eq!(
        stable.results(CMSG_BUY_STABLE_SLOT, None),
        [SUCCESS_BUY_SLOT]
    );
    assert_eq!(stable.money(), 100, "the first slot costs 5 silver");
    assert_eq!(
        stable.results(CMSG_BUY_STABLE_SLOT, None),
        [ERR_MONEY],
        "the second slot costs 5 gold"
    );
    assert_eq!(stable.money(), 100);

    assert_eq!(stable.results(CMSG_STABLE_PET, None), [SUCCESS_STABLE]);
    assert_eq!(
        stable.live_pet_level(),
        None,
        "stabling removes the live pet"
    );
    assert_eq!(stable.pets(), [(first, 1, DISMISSED)]);
    assert!(stable
        .shard
        .query_rows(&format!(
            "SELECT pet_id FROM game_hunter_pet_protocol WHERE owner_guid = {}",
            stable.hunter
        ))
        .is_empty());
    assert_eq!(
        stable.list(),
        Some((1, vec![(first, BOAR, 12, "Fang".to_owned(), 1, 2)]))
    );
    assert_eq!(
        stable.results(CMSG_STABLE_PET, None),
        [ERR_STABLE],
        "the Hunter has no Current Pet"
    );

    let second = stable.tame();
    assert_ne!(second, first, "a stabled pet does not block a tame");
    let replies = stable.request(CMSG_STABLE_SWAP_PET, Some(first));
    let name = replies
        .iter()
        .position(|(opcode, body)| {
            *opcode == SMSG_PET_NAME_QUERY_RESPONSE && body[..4] == first.to_le_bytes()
        })
        .expect("the owner learns the name of the pet brought out");
    let result = replies
        .iter()
        .position(|reply| *reply == (SMSG_STABLE_RESULT, vec![SUCCESS_UNSTABLE]))
        .expect("the swap succeeds");
    assert!(name < result, "the name precedes the result: {replies:x?}");
    assert!(replies[name].1[4..].starts_with(b"Fang\0"));
    assert_eq!(
        stable.live_pet_level(),
        Some(12),
        "the stabled pet comes out"
    );
    assert_eq!(stable.pets(), [(first, 0, 1), (second, 1, DISMISSED)]);
    let (slots, listed) = stable.list().expect("a listing");
    assert_eq!(slots, 1);
    assert_eq!(
        listed.iter().map(|pet| (pet.0, pet.5)).collect::<Vec<_>>(),
        [(first, 1), (second, 2)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stabled_pet_stays_stabled_through_logout_and_a_dead_one_unstables_without_a_build() {
    let mut stable = stable("pet-stable-unstable");
    let first = stable.tame();
    stable.set_money(500);
    assert_eq!(
        stable.results(CMSG_BUY_STABLE_SLOT, None),
        [SUCCESS_BUY_SLOT]
    );
    assert_eq!(stable.results(CMSG_STABLE_PET, None), [SUCCESS_STABLE]);

    // A Character in combat cannot log out, and the tamed creature may have engaged the Hunter.
    stable.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET combat_until_ms = 0 WHERE guid = {}",
        stable.hunter
    ));
    let logout = send(&mut stable.wire, CMSG_LOGOUT_REQUEST, Vec::new());
    assert!(
        logout
            .iter()
            .any(|(opcode, _)| *opcode == SMSG_LOGOUT_COMPLETE),
        "the logout completes: {logout:x?}"
    );
    let login = send(
        &mut stable.wire,
        CMSG_PLAYER_LOGIN,
        stable.hunter.to_le_bytes().to_vec(),
    );
    assert!(
        login
            .iter()
            .any(|(opcode, _)| *opcode == SMSG_LOGIN_VERIFY_WORLD),
        "the login enters the world: {login:x?}"
    );
    assert_eq!(
        stable.live_pet_level(),
        None,
        "world entry builds no stabled pet"
    );
    assert_eq!(stable.pets(), [(first, 1, DISMISSED)]);

    let second = stable.tame();
    assert_eq!(
        stable.results(CMSG_UNSTABLE_PET, Some(first)),
        [ERR_STABLE],
        "a pet is out"
    );
    stable.shard.assert_call(
        "debug_pet_command",
        &[&stable.hunter.to_string(), DISMISS, "0"],
    );
    stable.shard.assert_sql(&format!(
        "UPDATE game_hunter_pet SET presence = {DEAD} WHERE owner_guid = {} AND stable_slot = 1",
        stable.hunter
    ));
    assert_eq!(
        stable.results(CMSG_UNSTABLE_PET, Some(first)),
        [SUCCESS_UNSTABLE]
    );
    assert_eq!(stable.live_pet_level(), None, "a dead pet is not built");
    assert_eq!(stable.pets(), [(first, 0, DEAD), (second, 1, DISMISSED)]);
    assert_eq!(
        stable.results(CMSG_UNSTABLE_PET, Some(first)),
        [ERR_STABLE],
        "the Current Pet is not stabled"
    );

    assert_eq!(
        stable.results(CMSG_UNSTABLE_PET, Some(second)),
        [SUCCESS_UNSTABLE]
    );
    assert!(stable.live_pet_level().is_some(), "a living pet is built");
    assert_eq!(stable.pets(), [(second, 0, 1), (first, 1, DEAD)]);

    stable.shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = 0 WHERE guid = {}",
        stable.master
    ));
    assert_eq!(stable.list(), None, "a refused listing has no reply");
    assert_eq!(stable.results(CMSG_STABLE_PET, None), [ERR_STABLE]);
    assert_eq!(stable.pets(), [(second, 0, 1), (first, 1, DEAD)]);
}
