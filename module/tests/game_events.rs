mod support;

use lyracore_package_delta::fixture_ids::{
    FIXTURE_BLADE, TEST_WOLF_ELDER_ENTRY, TEST_WOLF_ENTRY, WELL_FED_SPELL,
};
use lyracore_package_delta::{packed_creature_spawn_guid, packed_gameobject_spawn_guid};
use lyracore_shared::game_event::{
    DARKMOON_FAIRE_ELWYNN, DARKMOON_FAIRE_ELWYNN_BUILDING, DARKMOON_FAIRE_EVENTS,
    DARKMOON_FAIRE_MULGORE,
};
use support::{number, refused, Standalone};

const CHARACTER: u64 = 1;
const VIRTUAL_START_SECS: i64 = 1_800_000_000;
const YEAR_MINS: u32 = 525_600;

const FESTIVAL: u32 = 5_300_001;
const LANTERN: u32 = 5_300_100;
const POOLED_LANTERN: u32 = 5_300_101;
const LANTERN_POOL: u32 = 5_300_200;
const LANTERN_POINT: u64 = 5_300_300;

const WOLF_WHILE_RUNNING: u32 = 3_000_001;
const WOLF_WHILE_STOPPED: u32 = 3_000_002;
const LANTERN_WHILE_RUNNING: u32 = 5_300_401;
const LANTERN_WHILE_STOPPED: u32 = 5_300_402;
const PARTIAL_LANTERNS: u32 = 5_300_501;

// 2026-12-06 23:59:30 UTC, thirty seconds before the Darkmoon Faire opens in Elwynn.
const FAIRE_BUILDING_SECS: i64 = 1_796_601_570;
const FAIRE_BUILDER_WOLF: u32 = 3_000_003;
const DRESSED_WOLF: u32 = 3_000_004;
const ELWYNN_FAIRE_LANTERN: u32 = 5_300_403;
const MULGORE_FAIRE_LANTERN: u32 = 5_300_404;

struct Place {
    map_id: u32,
    x: f32,
    y: f32,
    z: f32,
}

fn realm(name: &str) -> (Standalone, Place) {
    realm_at(name, VIRTUAL_START_SECS)
}

fn realm_at(name: &str, start_secs: i64) -> (Standalone, Place) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    shard.assert_call(
        "debug_start_virtual_clock",
        &[&(start_secs * 1_000_000).to_string()],
    );
    let at = shard.query_rows(&format!(
        "SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
    ))[0]
        .clone();
    // Far enough from the Character that no wolf engages it.
    let place = Place {
        map_id: number(&at, "map_id"),
        x: number::<f32>(&at, "x") + 300.0,
        y: number(&at, "y"),
        z: number(&at, "z"),
    };
    shard.insert_row(
        "game_gameobject_template",
        &[
            ("entry", &LANTERN),
            ("type_id", &5),
            ("name", &"Festival lantern"),
        ],
    );
    (shard, place)
}

#[derive(Clone, Copy)]
enum Kind {
    Calendar,
    Scripted,
    Disabled,
}

/// A yearly Game Event whose window opens at `start_secs` for `length_mins`.
fn festival(shard: &Standalone, start_secs: i64, length_mins: u32, kind: Kind) {
    shard.insert_row(
        "game_event",
        &[
            ("event_entry", &FESTIVAL),
            ("start_secs", &start_secs),
            ("end_secs", &(VIRTUAL_START_SECS + 100_000_000)),
            ("occurrence_mins", &YEAR_MINS),
            ("length_mins", &length_mins),
            ("scripted", &matches!(kind, Kind::Scripted)),
            ("disabled", &matches!(kind, Kind::Disabled)),
            ("description", &"Fixture festival"),
        ],
    );
}

fn link(shard: &Standalone, spawn_guid: u64, while_inactive: bool) {
    link_to(shard, spawn_guid, FESTIVAL, while_inactive);
}

fn link_to(shard: &Standalone, spawn_guid: u64, event_entry: u32, while_inactive: bool) {
    shard.insert_row(
        "game_event_link",
        &[
            ("link_id", &spawn_guid),
            ("spawn_guid", &spawn_guid),
            ("event_entry", &event_entry),
            ("while_inactive", &while_inactive),
        ],
    );
}

fn wolf_guid(low: u32) -> u64 {
    packed_creature_spawn_guid(TEST_WOLF_ENTRY, low)
}

fn lantern_guid(low: u32) -> u64 {
    packed_gameobject_spawn_guid(low)
}

/// Links a wolf and a lantern to the festival and another pair to its absence, then imports them.
fn festival_spawns(shard: &Standalone, at: &Place) {
    link(shard, wolf_guid(WOLF_WHILE_RUNNING), false);
    link(shard, wolf_guid(WOLF_WHILE_STOPPED), true);
    link(shard, lantern_guid(LANTERN_WHILE_RUNNING), false);
    link(shard, lantern_guid(LANTERN_WHILE_STOPPED), true);
    import_spawns(
        shard,
        at,
        &[WOLF_WHILE_RUNNING, WOLF_WHILE_STOPPED],
        &[LANTERN_WHILE_RUNNING, LANTERN_WHILE_STOPPED],
    );
}

fn import_spawns(shard: &Standalone, at: &Place, wolves: &[u32], lanterns: &[u32]) {
    import_wolves(shard, at, wolves);
    let lanterns = lanterns
        .iter()
        .map(|&low| {
            format!(
                "{},{LANTERN},{},{},{},{},0,1,0,0,0,0",
                lantern_guid(low),
                at.map_id,
                at.x,
                at.y,
                at.z
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    shard.assert_call("import_gameobjects_append", &[&lanterns]);
}

fn import_wolves(shard: &Standalone, at: &Place, wolves: &[u32]) {
    let wolves = wolves
        .iter()
        .map(|&low| {
            format!(
                "{},{TEST_WOLF_ENTRY},{},{},{},{},0,0,60",
                wolf_guid(low),
                at.map_id,
                at.x,
                at.y,
                at.z
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    shard.assert_call("import_creature_spawns_append", &[&wolves]);
}

fn in_world(shard: &Standalone, guid: u64) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE guid = {guid}"
        ))
        .is_empty()
}

fn has_spawn(shard: &Standalone, guid: u64) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT guid FROM game_creature_spawn WHERE guid = {guid}"
        ))
        .is_empty()
}

fn shown(shard: &Standalone, guid: u64) -> bool {
    let rows = shard.query_rows(&format!(
        "SELECT despawned FROM game_gameobject WHERE guid = {guid}"
    ));
    assert_eq!(rows.len(), 1, "GameObject {guid} keeps its row");
    rows[0]["despawned"] == "false"
}

/// Which of the festival's wolves and lanterns are up: the running pair, then the stopped pair.
fn festival_world(shard: &Standalone) -> [bool; 4] {
    [
        in_world(shard, wolf_guid(WOLF_WHILE_RUNNING)),
        shown(shard, lantern_guid(LANTERN_WHILE_RUNNING)),
        in_world(shard, wolf_guid(WOLF_WHILE_STOPPED)),
        shown(shard, lantern_guid(LANTERN_WHILE_STOPPED)),
    ]
}

const RUNNING: [bool; 4] = [true, true, false, false];
const STOPPED: [bool; 4] = [false, false, true, true];

/// Ten virtual seconds of scheduler ticks, enough for the respawn pass.
fn settle(shard: &Standalone) {
    shard.assert_call("debug_advance_roll_batch", &["200"]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_event_inside_its_window_when_the_world_loads_shows_its_spawns_at_once() {
    let (shard, at) = realm("game-event-loaded-inside");
    festival(&shard, VIRTUAL_START_SECS - 30, 1, Kind::Calendar);
    festival_spawns(&shard, &at);

    settle(&shard);

    assert_eq!(festival_world(&shard), RUNNING);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_minute_pass_ends_an_event_through_the_despawn_path_and_brings_back_its_absent_spawns() {
    let (shard, at) = realm("game-event-calendar-end");
    festival(&shard, VIRTUAL_START_SECS - 30, 1, Kind::Calendar);
    festival_spawns(&shard, &at);
    settle(&shard);
    assert_eq!(festival_world(&shard), RUNNING);

    shard.assert_call("debug_advance_roll_batch", &["1200"]);

    assert_eq!(festival_world(&shard), STOPPED);
    assert!(
        has_spawn(&shard, wolf_guid(WOLF_WHILE_RUNNING)),
        "the despawned wolf keeps its spawn for the next festival"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_operator_start_and_stop_hold_over_the_calendar_until_cleared() {
    let (shard, at) = realm("game-event-operator");
    festival(&shard, VIRTUAL_START_SECS + 86_400, 1, Kind::Calendar);
    festival_spawns(&shard, &at);
    settle(&shard);
    assert_eq!(festival_world(&shard), STOPPED);

    shard.assert_call("debug_event_start", &[&FESTIVAL.to_string()]);
    settle(&shard);
    assert_eq!(festival_world(&shard), RUNNING);

    shard.assert_call("debug_advance_roll_batch", &["1200"]);
    assert_eq!(
        festival_world(&shard),
        RUNNING,
        "the minute pass keeps an Operator start"
    );

    shard.assert_call("debug_event_clear", &[&FESTIVAL.to_string()]);
    settle(&shard);
    assert_eq!(
        festival_world(&shard),
        STOPPED,
        "the calendar has it stopped"
    );

    refused(&shard, "debug_event_start", &["5300999"], "does not exist");
    refused(
        &shard,
        "debug_event_clear",
        &[&FESTIVAL.to_string()],
        "has no Operator hold",
    );
    assert!(shard
        .query_rows("SELECT event_entry FROM game_event_override")
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_negative_link_spawn_shows_while_its_event_is_stopped_and_leaves_while_it_runs() {
    let (shard, at) = realm("game-event-negative-link");
    festival(&shard, VIRTUAL_START_SECS - 30, 1, Kind::Scripted);
    festival_spawns(&shard, &at);
    settle(&shard);
    assert_eq!(
        festival_world(&shard),
        STOPPED,
        "a scripted event without its script stays stopped"
    );

    shard.assert_call("debug_event_start", &[&FESTIVAL.to_string()]);
    settle(&shard);
    assert_eq!(festival_world(&shard), RUNNING);

    shard.assert_call("debug_event_stop", &[&FESTIVAL.to_string()]);
    settle(&shard);
    assert_eq!(festival_world(&shard), STOPPED);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_disabled_event_shows_neither_its_spawns_nor_its_absence_spawns() {
    let (shard, at) = realm("game-event-disabled");
    festival(&shard, VIRTUAL_START_SECS - 30, 1, Kind::Disabled);
    festival_spawns(&shard, &at);

    settle(&shard);

    assert_eq!(festival_world(&shard), [false; 4]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stop_part_way_through_a_start_takes_every_shown_spawn_down() {
    let (shard, at) = realm("game-event-partial-start");
    festival(&shard, VIRTUAL_START_SECS + 86_400, 1, Kind::Calendar);
    let lanterns: Vec<u64> = (PARTIAL_LANTERNS..PARTIAL_LANTERNS + 3)
        .map(lantern_guid)
        .collect();
    for &guid in &lanterns {
        link(&shard, guid, false);
    }
    let packed = lanterns
        .iter()
        .map(|guid| {
            format!(
                "{guid},{LANTERN},{},{},{},{},0,1,0,0,0,0",
                at.map_id, at.x, at.y, at.z
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    shard.assert_call("import_gameobjects_append", &[&packed]);
    shard.assert_call("debug_run_game_event_pass", &["500"]);
    let up = || lanterns.iter().filter(|&&guid| shown(&shard, guid)).count();
    assert_eq!(up(), 0);

    shard.insert_row(
        "game_event_override",
        &[("event_entry", &FESTIVAL), ("running", &true)],
    );
    shard.assert_call("debug_run_game_event_pass", &["1"]);
    assert_eq!(up(), 1, "a pass with room for one change shows one lantern");

    shard.assert_call("debug_event_stop", &[&FESTIVAL.to_string()]);
    assert_eq!(up(), 0, "the stop reaches the lantern the start showed");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_respawn_due_while_its_event_does_not_run_brings_nothing_back() {
    let (shard, at) = realm("game-event-respawn-gate");
    festival(&shard, VIRTUAL_START_SECS + 86_400, 1, Kind::Calendar);
    festival_spawns(&shard, &at);

    settle(&shard);

    let wolf = wolf_guid(WOLF_WHILE_RUNNING);
    assert!(!in_world(&shard, wolf), "the imported respawn is refused");
    assert!(has_spawn(&shard, wolf));
    let lantern = lantern_guid(LANTERN_WHILE_RUNNING);
    assert!(!shown(&shard, lantern), "the lantern imports hidden");

    shard.assert_sql(&format!(
        "UPDATE game_gameobject SET respawn_at_micros = 1 WHERE guid = {lantern}"
    ));
    settle(&shard);

    assert!(!shown(&shard, lantern));
    let rows = shard.query_rows(&format!(
        "SELECT respawn_at_micros FROM game_gameobject WHERE guid = {lantern}"
    ));
    assert_eq!(
        number::<u64>(&rows[0], "respawn_at_micros"),
        0,
        "the hidden lantern waits for its event, not a timer"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_pool_tied_to_an_event_rolls_only_while_the_event_runs() {
    let (shard, at) = realm("game-event-pool");
    festival(&shard, VIRTUAL_START_SECS + 86_400, 1, Kind::Calendar);
    shard.insert_row(
        "game_gameobject_template",
        &[
            ("entry", &POOLED_LANTERN),
            ("type_id", &5),
            ("name", &"Pooled festival lantern"),
        ],
    );
    shard.insert_row(
        "game_gameobject_pool",
        &[("pool_id", &LANTERN_POOL), ("max_active", &1)],
    );
    shard.insert_row(
        "game_gameobject_pool_link",
        &[
            ("pool_id", &LANTERN_POOL),
            ("parent_pool_id", &0),
            ("weight", &0),
            ("map_id", &at.map_id),
        ],
    );
    shard.insert_row(
        "game_gameobject_pool_member",
        &[
            ("point_id", &LANTERN_POINT),
            ("pool_id", &LANTERN_POOL),
            ("template_entry", &POOLED_LANTERN),
            ("map_id", &at.map_id),
            ("x", &at.x),
            ("y", &at.y),
            ("z", &at.z),
            ("weight", &1),
        ],
    );
    shard.insert_row(
        "game_event_pool",
        &[("pool_id", &LANTERN_POOL), ("event_entry", &FESTIVAL)],
    );
    let pooled = || {
        shard.query_rows(&format!(
            "SELECT despawned FROM game_gameobject WHERE template_entry = {POOLED_LANTERN}"
        ))
    };

    shard.assert_call("arm_all_pools", &[]);
    assert!(pooled().is_empty(), "the pool waits for its event");

    shard.assert_call("debug_event_start", &[&FESTIVAL.to_string()]);
    let up = pooled();
    assert_eq!(up.len(), 1, "the event rolls its pool: {up:?}");
    assert_eq!(up[0]["despawned"], "false");

    shard.assert_call("debug_event_stop", &[&FESTIVAL.to_string()]);
    assert!(pooled().is_empty(), "the event's end takes its pool down");
}

/// A Darkmoon Faire event as the pinned World Source has it: scripted, with no calendar dates.
fn faire_event(shard: &Standalone, event_entry: u32) {
    shard.insert_row(
        "game_event",
        &[
            ("event_entry", &event_entry),
            ("start_secs", &0i64),
            ("end_secs", &0i64),
            ("occurrence_mins", &YEAR_MINS),
            ("length_mins", &2_592_000u32),
            ("scripted", &true),
            ("disabled", &false),
            ("description", &"Fixture Darkmoon Faire"),
        ],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_darkmoon_faire_builds_then_opens_at_its_months_site_without_an_operator() {
    let (shard, at) = realm_at("game-event-darkmoon-faire", FAIRE_BUILDING_SECS);
    for event_entry in DARKMOON_FAIRE_EVENTS {
        faire_event(&shard, event_entry);
    }
    let builder = wolf_guid(FAIRE_BUILDER_WOLF);
    let elwynn = lantern_guid(ELWYNN_FAIRE_LANTERN);
    let mulgore = lantern_guid(MULGORE_FAIRE_LANTERN);
    link_to(&shard, builder, DARKMOON_FAIRE_ELWYNN_BUILDING, false);
    link_to(&shard, elwynn, DARKMOON_FAIRE_ELWYNN, false);
    link_to(&shard, mulgore, DARKMOON_FAIRE_MULGORE, false);
    import_spawns(
        &shard,
        &at,
        &[FAIRE_BUILDER_WOLF],
        &[ELWYNN_FAIRE_LANTERN, MULGORE_FAIRE_LANTERN],
    );
    settle(&shard);
    assert_eq!(
        [
            in_world(&shard, builder),
            shown(&shard, elwynn),
            shown(&shard, mulgore)
        ],
        [true, false, false],
        "the Faire builds in Elwynn on the Sunday before its first Monday"
    );

    shard.assert_call("debug_advance_roll_batch", &["1200"]);

    assert_eq!(
        [
            in_world(&shard, builder),
            shown(&shard, elwynn),
            shown(&shard, mulgore)
        ],
        [false, true, false],
        "the Faire opens in Elwynn on December's first Monday"
    );
}

/// While the festival runs, the spawn is a Test Wolf Elder with the fixture blade and Well Fed.
fn dress_for_festival(shard: &Standalone, spawn_guid: u64) {
    shard.insert_row(
        "game_event_creature_data",
        &[
            ("data_id", &1u64),
            ("spawn_guid", &spawn_guid),
            ("event_entry", &FESTIVAL),
            ("creature_entry", &TEST_WOLF_ELDER_ENTRY),
            ("main_hand", &FIXTURE_BLADE),
            ("off_hand", &0u32),
            ("ranged", &0u32),
            ("start_spell", &WELL_FED_SPELL),
            ("end_spell", &0u32),
        ],
    );
}

#[derive(Debug, PartialEq)]
struct Wear {
    entry: u32,
    level: u32,
    main_hand_display: u32,
    well_fed: bool,
}

fn wear(shard: &Standalone, guid: u64) -> Wear {
    let entity = shard.query_rows(&format!(
        "SELECT entry, level FROM game_world_entity WHERE guid = {guid}"
    ));
    assert_eq!(entity.len(), 1, "creature {guid} is in the world");
    let main_hand_display = shard
        .query_rows(&format!(
            "SELECT main_hand FROM game_encounter_equip WHERE creature_guid = {guid}"
        ))
        .first()
        .map_or(0, |row| number(row, "main_hand"));
    let well_fed = shard
        .query_rows(&format!(
            "SELECT spell_id FROM game_aura WHERE target_guid = {guid}"
        ))
        .iter()
        .any(|row| number::<u32>(row, "spell_id") == WELL_FED_SPELL);
    Wear {
        entry: number(&entity[0], "entry"),
        level: number(&entity[0], "level"),
        main_hand_display,
        well_fed,
    }
}

fn own_wear() -> Wear {
    Wear {
        entry: TEST_WOLF_ENTRY,
        level: 1,
        main_hand_display: 0,
        well_fed: false,
    }
}

fn festival_wear(shard: &Standalone) -> Wear {
    let blade = shard.query_rows(&format!(
        "SELECT display_id FROM game_item_template WHERE entry = {FIXTURE_BLADE}"
    ));
    Wear {
        entry: TEST_WOLF_ELDER_ENTRY,
        level: 8,
        main_hand_display: number(&blade[0], "display_id"),
        well_fed: true,
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_live_creature_takes_its_event_data_as_the_event_starts_and_its_own_as_it_stops() {
    let (shard, at) = realm("game-event-creature-data-live");
    festival(&shard, VIRTUAL_START_SECS + 86_400, 1, Kind::Calendar);
    let wolf = wolf_guid(DRESSED_WOLF);
    dress_for_festival(&shard, wolf);
    import_wolves(&shard, &at, &[DRESSED_WOLF]);
    settle(&shard);
    assert_eq!(wear(&shard, wolf), own_wear());

    shard.assert_call("debug_event_start", &[&FESTIVAL.to_string()]);
    assert_eq!(
        wear(&shard, wolf),
        festival_wear(&shard),
        "the same creature changes in place"
    );

    shard.assert_call("debug_event_stop", &[&FESTIVAL.to_string()]);
    assert_eq!(wear(&shard, wolf), own_wear());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_that_spawns_while_its_event_runs_comes_in_its_event_data() {
    let (shard, at) = realm("game-event-creature-data-spawn");
    festival(&shard, VIRTUAL_START_SECS + 86_400, 1, Kind::Calendar);
    let wolf = wolf_guid(DRESSED_WOLF);
    dress_for_festival(&shard, wolf);
    shard.assert_call("debug_event_start", &[&FESTIVAL.to_string()]);

    import_wolves(&shard, &at, &[DRESSED_WOLF]);
    settle(&shard);

    assert_eq!(wear(&shard, wolf), festival_wear(&shard));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_creature_that_respawns_while_its_event_runs_comes_back_in_its_event_data() {
    let (shard, at) = realm("game-event-creature-data-respawn");
    festival(&shard, VIRTUAL_START_SECS + 86_400, 1, Kind::Calendar);
    let wolf = wolf_guid(DRESSED_WOLF);
    dress_for_festival(&shard, wolf);
    shard.assert_call("debug_event_start", &[&FESTIVAL.to_string()]);
    import_wolves(&shard, &at, &[DRESSED_WOLF]);
    settle(&shard);
    assert_eq!(wear(&shard, wolf), festival_wear(&shard));

    shard.assert_call(
        "debug_kill_creature",
        &[&CHARACTER.to_string(), &wolf.to_string()],
    );
    // Two virtual minutes: the corpse decays after one and the 60-second respawn is then due.
    shard.assert_call("debug_advance_roll_batch", &["2400"]);

    let spawn = shard.query_rows(&format!(
        "SELECT life_seq FROM game_creature_spawn WHERE guid = {wolf}"
    ));
    assert_eq!(spawn[0]["life_seq"], "2", "the wolf lived a second life");
    assert_eq!(
        wear(&shard, wolf),
        festival_wear(&shard),
        "the new life takes its equipment and start spell again"
    );
}
