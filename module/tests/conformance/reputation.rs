// Base standing on the wire and in reactions against vmangos d9173eca ReputationMgr and
// Unit::GetFactionReactionTo.
use super::buy_in_slot::{buy, delivered, inventory, money, stage};
use super::{enter, fixture};
use lyracore_package_delta::fixture_ids::FIXTURE_JERKY;
use lyracore_test_support::Standalone;

const STORMWIND: u32 = 72;
const STORMWIND_TEMPLATE: u32 = 5_119_072;
const FIXTURE_FACTION: u32 = 50_900;
const FIXTURE_REPUTATION_INDEX: usize = 60;
const SEEDED_CHARACTER: u64 = 1;
const SMSG_INITIALIZE_FACTIONS: u16 = 0x122;
const SMSG_SET_FACTION_STANDING: u16 = 0x124;
const SMSG_BUY_FAILED: u16 = 0x1a5;
const CANT_FIND_ITEM: u8 = 0;

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
    assert_eq!(*packets[0].1.last().unwrap(), CANT_FIND_ITEM);
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
    assert!(delivered(&mut connection).contains(&(SMSG_SET_FACTION_STANDING, reset)));
    assert!(shard.query_rows(&stored).is_empty());
}
