// Base standing, default flags and NPC hostility against vmangos d9173eca ReputationMgr,
// Unit::GetFactionReactionTo and Player::CanInteractWithNPC.
use super::buy_in_slot::{buy, delivered, inventory, money, request, stage};
use super::{enter, fixture};
use lyracore_package_delta::fixture_ids::FIXTURE_JERKY;
use lyracore_test_support::Standalone;
use serde_json::json;

const STORMWIND: u32 = 72;
const STORMWIND_TEMPLATE: u32 = 5_119_072;
const FIXTURE_FACTION: u32 = 50_900;
const FIXTURE_REPUTATION_INDEX: usize = 60;
const SEEDED_CHARACTER: u64 = 1;
const SMSG_INITIALIZE_FACTIONS: u16 = 0x122;
const SMSG_SET_FACTION_STANDING: u16 = 0x124;
const SMSG_BUY_FAILED: u16 = 0x1a5;
// A refused buy into a slot reports every vendor refusal as too far, from vendor guid 0.
const DISTANCE_TOO_FAR: u8 = 5;
const CMSG_PING: u32 = 0x1dc;
const CMSG_SET_FACTION_ATWAR: u32 = 0x125;
// vmangos ReputationMgr.h FactionFlags.
const VISIBLE: u32 = 0x01;
const AT_WAR: u32 = 0x02;
const HIDDEN: u32 = 0x04;
const INVISIBLE_FORCED: u32 = 0x08;
const PEACE_FORCED: u32 = 0x10;
const ORC_RACE_MASK: u32 = 2;

fn set_race(shard: &Standalone, guid: u64, race: u8) {
    shard.assert_sql(&format!(
        "UPDATE game_character SET race = {race} WHERE guid = {guid}"
    ));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stormwind_vendor_refuses_a_horde_character_with_no_stored_standing() {
    let (shard, mut connection, guid, vendor) = stage("reputation-vendor-base");
    // Faction.dbc 1.12 Stormwind: other Alliance races 3100, Horde -42000, Humans 4000.
    shard.insert_row(
        "game_faction",
        &[
            ("faction_id", &STORMWIND),
            ("reputation_index", &19),
            ("base_standing", &3100),
            ("base_race_mask_1", &76),
            ("base_standing_2", &-42_000),
            ("base_race_mask_2", &178),
            ("base_standing_3", &4000),
            ("base_race_mask_3", &1),
        ],
    );
    shard.insert_row(
        "game_faction_template",
        &[("id", &STORMWIND_TEMPLATE), ("faction", &STORMWIND)],
    );
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = {STORMWIND_TEMPLATE} WHERE guid = {vendor}"
    ));

    set_race(&shard, guid, 2);
    let packets = buy(&mut connection, vendor, FIXTURE_JERKY, guid, 30, 1);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].0, SMSG_BUY_FAILED);
    assert_eq!(&packets[0].1[..8], &[0; 8]);
    assert_eq!(*packets[0].1.last().unwrap(), DISTANCE_TOO_FAR);
    assert!(inventory(&shard, guid).is_empty());
    assert_eq!(money(&shard, guid), "50000");

    set_race(&shard, guid, 1);
    buy(&mut connection, vendor, FIXTURE_JERKY, guid, 30, 1);
    assert_eq!(inventory(&shard, guid)[0]["slot"], "30");
    assert_eq!(money(&shard, guid), "49990");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn login_and_the_delete_reset_send_the_standing_less_the_base() {
    let (shard, _, _) = fixture("reputation-wire-earned");
    // Slot 1 admits no race, so the seeded Character takes slot 2's base of 2000.
    shard.assert_sql(&format!(
        "UPDATE game_faction SET base_standing = -42000, base_race_mask_1 = 2147483648, base_standing_2 = 2000 WHERE faction_id = {FIXTURE_FACTION}"
    ));
    shard.assert_call(
        "debug_grant_reputation",
        &[
            &SEEDED_CHARACTER.to_string(),
            &FIXTURE_FACTION.to_string(),
            "500",
        ],
    );
    let stored = format!(
        "SELECT standing FROM game_player_reputation WHERE character_guid = {SEEDED_CHARACTER} AND faction_id = {FIXTURE_FACTION}"
    );
    assert_eq!(shard.query_rows(&stored)[0]["standing"], "2500");

    let mut connection = enter(&shard, SEEDED_CHARACTER);
    let login = delivered(&mut connection);
    let (_, factions) = login
        .iter()
        .find(|(opcode, _)| *opcode == SMSG_INITIALIZE_FACTIONS)
        .expect("login sends the faction list");
    // u32 count, then per slot a u8 flag and a u32 standing.
    let at = 4 + FIXTURE_REPUTATION_INDEX * 5 + 1;
    assert_eq!(
        i32::from_le_bytes(factions[at..at + 4].try_into().unwrap()),
        500
    );

    shard.assert_call("debug_stage_choice_reward_fixture", &["false"]);
    let mut reset = 1u32.to_le_bytes().to_vec();
    reset.extend((FIXTURE_REPUTATION_INDEX as u32).to_le_bytes());
    reset.extend(0i32.to_le_bytes());
    // The CLI call's packets reach this connection before the reply to its own next call.
    connection
        .call("native_world_receive", json!([CMSG_PING, vec![0u8; 8]]))
        .unwrap();
    assert!(delivered(&mut connection).contains(&(SMSG_SET_FACTION_STANDING, reset)));
    assert!(shard.query_rows(&stored).is_empty());
}

fn insert_faction(shard: &Standalone, faction_id: u32, reputation_index: i32, flags: [u32; 2]) {
    // Slot 1 admits only Orcs, so a Human Character takes slot 2's flags.
    shard.insert_row(
        "game_faction",
        &[
            ("faction_id", &faction_id),
            ("reputation_index", &reputation_index),
            ("base_standing", &0),
            ("base_race_mask_1", &ORC_RACE_MASK),
            ("reputation_flags_1", &flags[0]),
            ("reputation_flags_2", &flags[1]),
        ],
    );
}

fn grant_reputation(shard: &Standalone, guid: u64, faction_id: u32, amount: i32) {
    shard.assert_call(
        "debug_grant_reputation",
        &[
            &guid.to_string(),
            &faction_id.to_string(),
            &amount.to_string(),
        ],
    );
}

fn set_at_war(connection: &mut lyracore_test_support::ReducerConnection, index: u32) {
    let mut body = index.to_le_bytes().to_vec();
    body.push(1);
    request(connection, CMSG_SET_FACTION_ATWAR, body);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn login_sends_each_faction_the_default_flags_of_the_characters_slot() {
    let (shard, guid, _) = fixture("reputation-default-flags");
    set_race(&shard, guid, 1);
    insert_faction(&shard, 5_120_801, 50, [AT_WAR | INVISIBLE_FORCED, VISIBLE]);
    insert_faction(&shard, 5_120_802, 51, [0, HIDDEN | PEACE_FORCED]);
    insert_faction(&shard, 5_120_803, 52, [0, HIDDEN]);
    insert_faction(&shard, 5_120_804, 53, [0, AT_WAR]);
    grant_reputation(&shard, guid, 5_120_803, 500);
    grant_reputation(&shard, guid, 5_120_804, 500);

    let mut connection = enter(&shard, guid);
    let login = delivered(&mut connection);
    let (_, factions) = login
        .iter()
        .find(|(opcode, _)| *opcode == SMSG_INITIALIZE_FACTIONS)
        .expect("login sends the faction list");
    // u32 count, then per slot a u8 flag and a u32 standing.
    let flags = |index: usize| u32::from(factions[4 + index * 5]);
    assert_eq!(flags(50), VISIBLE);
    assert_eq!(flags(51), HIDDEN | PEACE_FORCED);
    assert_eq!(flags(52), HIDDEN, "a gain leaves a hidden faction hidden");
    assert_eq!(
        flags(53),
        VISIBLE | AT_WAR,
        "a gain shows a faction and keeps its default war"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_peace_forced_faction_cannot_be_set_at_war() {
    let (shard, mut connection, guid, _) = stage("reputation-peace-forced");
    set_race(&shard, guid, 1);
    insert_faction(&shard, 5_120_811, 54, [0, VISIBLE | PEACE_FORCED]);
    insert_faction(&shard, 5_120_812, 55, [0, VISIBLE]);

    set_at_war(&mut connection, 54);
    set_at_war(&mut connection, 55);

    let at_war = shard.query_rows(&format!(
        "SELECT faction_id FROM game_player_reputation WHERE character_guid = {guid} AND at_war = true"
    ));
    assert_eq!(at_war.len(), 1, "{at_war:?}");
    assert_eq!(at_war[0]["faction_id"], "5120812");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_unfriendly_vendor_at_war_serves_and_a_hostile_one_refuses() {
    const VENDOR_FACTION: u32 = 5_120_821;
    const VENDOR_TEMPLATE: u32 = 5_120_821;
    let (shard, mut connection, guid, vendor) = stage("reputation-vendor-hostile");
    set_race(&shard, guid, 1);
    insert_faction(&shard, VENDOR_FACTION, 56, [0, 0]);
    shard.insert_row(
        "game_faction_template",
        &[("id", &VENDOR_TEMPLATE), ("faction", &VENDOR_FACTION)],
    );
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = {VENDOR_TEMPLATE} WHERE guid = {vendor}"
    ));
    grant_reputation(&shard, guid, VENDOR_FACTION, -3000);
    set_at_war(&mut connection, 56);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT at_war FROM game_player_reputation WHERE character_guid = {guid}"
        ))[0]["at_war"],
        "true"
    );

    buy(&mut connection, vendor, FIXTURE_JERKY, guid, 30, 1);
    assert_eq!(inventory(&shard, guid)[0]["slot"], "30");
    assert_eq!(money(&shard, guid), "49990");

    grant_reputation(&shard, guid, VENDOR_FACTION, -1);
    let packets = buy(&mut connection, vendor, FIXTURE_JERKY, guid, 31, 1);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].0, SMSG_BUY_FAILED);
    assert_eq!(*packets[0].1.last().unwrap(), DISTANCE_TOO_FAR);
    assert_eq!(inventory(&shard, guid).len(), 1);
    assert_eq!(money(&shard, guid), "49990");
}
