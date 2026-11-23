// Starting standing against vmangos d9173eca ReputationMgr::GetBaseReputation and
// FactionEntry::GetIndexFitTo.
mod support;

use support::{actor, number, Standalone};

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
type Slots = [(u32, u32, i32); 4];

const FACTIONS: [(u32, Slots); 3] = [
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

fn character(name: &str, race: u8, class: u8) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[CHARACTER]);
    shard.assert_sql(&format!(
        "UPDATE game_character SET race = {race}, class = {class} WHERE guid = {CHARACTER}"
    ));
    shard
}

fn start(name: &str, race: u8, class: u8) -> Standalone {
    let shard = character(name, race, class);
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

fn grant(shard: &Standalone, faction_id: u32, amount: i32) {
    shard.assert_call(
        "debug_grant_reputation",
        &[CHARACTER, &faction_id.to_string(), &amount.to_string()],
    );
}

// A zero grant stores the standing the Character starts with.
fn starting_standing(shard: &Standalone, faction_id: u32) -> i32 {
    grant(shard, faction_id, 0);
    standing(shard, faction_id)
}

fn standing(shard: &Standalone, faction_id: u32) -> i32 {
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

fn stamp_dbc(shard: &Standalone) {
    shard.assert_call("stamp_import_meta", &["\"dbc\"", "\"\"", "\"\"", "0"]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_dbc_stamp_moves_a_row_stored_on_slot_1_to_the_characters_slot_once() {
    let shard = character("base-reputation-repair", NIGHT_ELF, WARRIOR);
    shard.insert_row(
        "game_faction",
        &[
            ("faction_id", &WINTERSABER_TRAINERS),
            ("reputation_index", &40),
            ("base_standing", &-42_000),
        ],
    );
    grant(&shard, WINTERSABER_TRAINERS, 10_000);
    assert_eq!(standing(&shard, WINTERSABER_TRAINERS), -32_000);
    stamp_dbc(&shard);
    assert_eq!(
        standing(&shard, WINTERSABER_TRAINERS),
        -32_000,
        "the repair waits until the masks are imported"
    );

    shard.assert_sql(&format!(
        "UPDATE game_faction SET base_race_mask_1 = 178, base_race_mask_2 = 77 WHERE faction_id = {WINTERSABER_TRAINERS}"
    ));
    stamp_dbc(&shard);
    assert_eq!(standing(&shard, WINTERSABER_TRAINERS), 10_000);
    stamp_dbc(&shard);
    assert_eq!(standing(&shard, WINTERSABER_TRAINERS), 10_000);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_repair_waits_while_a_character_is_in_transfer() {
    let shard = character("base-reputation-repair-transfer", NIGHT_ELF, WARRIOR);
    shard.insert_row(
        "game_faction",
        &[
            ("faction_id", &WINTERSABER_TRAINERS),
            ("reputation_index", &40),
            ("base_standing", &-42_000),
        ],
    );
    grant(&shard, WINTERSABER_TRAINERS, 10_000);
    shard.assert_sql(&format!(
        "UPDATE game_faction SET base_race_mask_1 = 178, base_race_mask_2 = 77 WHERE faction_id = {WINTERSABER_TRAINERS}"
    ));
    shard.assert_call(
        "begin_transfer",
        &[
            "5119401",
            &actor(CHARACTER),
            "0",
            "0",
            "1200",
            "1200",
            "50",
            "0",
            "true",
        ],
    );
    stamp_dbc(&shard);
    assert_eq!(standing(&shard, WINTERSABER_TRAINERS), -32_000);

    shard.assert_sql("DELETE FROM game_transfer_out WHERE transfer_id = 5119401");
    stamp_dbc(&shard);
    assert_eq!(standing(&shard, WINTERSABER_TRAINERS), 10_000);
    stamp_dbc(&shard);
    assert_eq!(standing(&shard, WINTERSABER_TRAINERS), 10_000);
}
