// Starting standing against vmangos d9173eca ReputationMgr::GetBaseReputation and
// FactionEntry::GetIndexFitTo.
mod support;

use support::{number, Standalone};

const CHARACTER: &str = "1";
const CENARION_CIRCLE: u32 = 609;
const WINTERSABER_TRAINERS: u32 = 589;
const NO_HUMAN_SLOT: u32 = 5_119_400;
const HUMAN: u8 = 1;
const ORC: u8 = 2;
const NIGHT_ELF: u8 = 4;
const TAUREN: u8 = 6;
const WARRIOR: u8 = 1;
const DRUID: u8 = 11;

// Faction.dbc 1.12 slots as (race mask, class mask, base standing).
const FACTIONS: [(u32, [(u32, u32, i32); 4]); 3] = [
    (
        CENARION_CIRCLE,
        [(255, 479, 0), (40, 1024, 2000), (0, 0, 0), (0, 0, 0)],
    ),
    (
        WINTERSABER_TRAINERS,
        [(178, 0, -42000), (77, 0, 0), (0, 0, 0), (0, 0, 0)],
    ),
    (
        NO_HUMAN_SLOT,
        [(2, 0, 500), (4, 0, 600), (8, 0, 700), (0, 1024, 800)],
    ),
];

fn start(name: &str, race: u8, class: u8) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[CHARACTER]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET race = {race}, class = {class} WHERE guid = {CHARACTER}"
    ));
    for (reputation_index, (faction_id, slots)) in (40_i32..).zip(FACTIONS) {
        let [one, two, three, four] = slots;
        shard.insert_row(
            "game_faction",
            &[
                ("faction_id", &faction_id),
                ("reputation_index", &reputation_index),
                ("base_standing", &one.2),
                ("base_standing_2", &two.2),
                ("base_standing_3", &three.2),
                ("base_standing_4", &four.2),
                ("base_race_mask_1", &one.0),
                ("base_race_mask_2", &two.0),
                ("base_race_mask_3", &three.0),
                ("base_race_mask_4", &four.0),
                ("base_class_mask_1", &one.1),
                ("base_class_mask_2", &two.1),
                ("base_class_mask_3", &three.1),
                ("base_class_mask_4", &four.1),
            ],
        );
    }
    shard
}

// A zero grant stores the standing the Character starts with.
fn starting_standing(shard: &Standalone, faction_id: u32) -> i32 {
    shard.assert_call(
        "debug_grant_reputation",
        &[CHARACTER, &faction_id.to_string(), "0"],
    );
    number(
        &shard.query_rows(&format!(
            "SELECT standing FROM game_player_reputation WHERE character_guid = {CHARACTER} AND faction_id = {faction_id}"
        ))[0],
        "standing",
    )
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_tauren_druid_starts_cenarion_circle_at_2000() {
    let shard = start("base-reputation-tauren-druid", TAUREN, DRUID);
    assert_eq!(starting_standing(&shard, CENARION_CIRCLE), 2000);
    assert_eq!(starting_standing(&shard, WINTERSABER_TRAINERS), -42000);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_alliance_character_starts_wintersaber_trainers_neutral() {
    let shard = start("base-reputation-night-elf-warrior", NIGHT_ELF, WARRIOR);
    assert_eq!(starting_standing(&shard, WINTERSABER_TRAINERS), 0);
    assert_eq!(starting_standing(&shard, CENARION_CIRCLE), 0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_character_no_slot_admits_starts_at_0() {
    let shard = start("base-reputation-no-slot", HUMAN, WARRIOR);
    assert_eq!(starting_standing(&shard, NO_HUMAN_SLOT), 0);
    let orc = start("base-reputation-orc-slot", ORC, WARRIOR);
    assert_eq!(starting_standing(&orc, NO_HUMAN_SLOT), 500);
}
