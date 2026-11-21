mod support;

use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{A_CONTROL, A_INERT, M_STUN, P_MECHANIC, T_SELF};
use serde_json::json;
use support::{number, Standalone};

const FLIGHT_MASTER_ENTRY: u32 = 51_004;
const SOURCE_NODE: u32 = 5_090_301;
const DESTINATION_NODE: u32 = 5_090_302;
const SOURCE_CLIENT_NODE: u32 = 253;
const DESTINATION_CLIENT_NODE: u32 = 254;
const FARE: u32 = 25;
// A flight of 200 s at 32 yd/s outlasts a slow test.
const FLIGHT_YARDS: f32 = 6400.0;
const MISSING_MASTER: u64 = 0xf130_00ff_ffff_ffff;
const ACTIVATE_TAXI: u32 = 0x1ad;
const ACTIVATE_TAXI_EXPRESS: u32 = 0x312;
const ACTIVATE_TAXI_REPLY: u64 = 0x1ae;
const TAXI_NODE_STATUS_QUERY: u32 = 0x1aa;
const TAXI_NODE_STATUS: u64 = 0x1ab;
const REVERSE_PATH: u32 = 5_090_306;
const UNCATALOGUED_CLIENT_NODE: u32 = 9_999;
const UNVISITED_NODE: u32 = 5_090_307;
const UNVISITED_CLIENT_NODE: u32 = 252;
const UNVISITED_PATH: u32 = 5_090_308;
const ONE_TEAM_NODE: u32 = 5_090_309;
const ONE_TEAM_CLIENT_NODE: u32 = 251;
const OFF_NETWORK_NODE: u32 = 5_090_310;
const OFF_NETWORK_CLIENT_NODE: u32 = 250;
const TAXI_QUERY_AVAILABLE_NODES: u32 = 0x1ac;
const SHOW_TAXI_NODES: u64 = 0x1a9;
const NEW_TAXI_PATH: u64 = 0x1af;
const GOSSIP_HELLO: u32 = 0x17b;
const GOSSIP_MESSAGE: u64 = 0x17d;
const TAXI_GOSSIP_OPTION: u32 = 5_090_393;
const FIRST_VISIT_NODE: u32 = 5_090_311;
const FIRST_VISIT_CLIENT_NODE: u32 = 249;
const FAR_YARDS: f32 = 500.0;
const HORDE_RACE: u32 = 2;
const ALLIANCE_RACE: u32 = 1;
const MOUNT_DISPLAY: u32 = 1147;
const SLOW_CAST: u32 = 5_090_391;
const STUN_SELF: u32 = 5_090_392;
const CAT_FORM: u32 = 768;
const HOSTILE_FACTION_TEMPLATE: u32 = 14;
const FRIENDLY_FACTION_TEMPLATE: u32 = 1;

// vmangos UnitDefines.h UnitFlags.
const REMOVE_CLIENT_CONTROL: u32 = 0x0000_0004;
const IN_COMBAT: u32 = 0x0008_0000;
const NOT_SELECTABLE: u32 = 0x0200_0000;

fn stage(name: &str, money: u32) -> (Standalone, support::ReducerConnection, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    shard.assert_call("debug_set_money", &["1", &money.to_string()]);
    shard.assert_sql(
        "UPDATE game_world_entity SET movement_flags = 0, unit_flags = 0 WHERE guid = 1",
    );
    shard.assert_call(
        "debug_spawn_at_feet",
        &["1", &FLIGHT_MASTER_ENTRY.to_string(), "2"],
    );
    let master = shard.query_rows(&format!(
        "SELECT guid FROM game_world_entity WHERE entry = {FLIGHT_MASTER_ENTRY}"
    ))[0]["guid"]
        .clone();
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET npc_flags = 9 WHERE guid = {master}"
    ));
    let player =
        &shard.query_rows("SELECT map_id, x, y, z FROM game_world_entity WHERE guid = 1")[0];
    let (map_id, x, y, z): (u32, f32, f32, f32) = (
        number(player, "map_id"),
        number(player, "x"),
        number(player, "y"),
        number(player, "z"),
    );
    // A node at the origin has no position, so the nodes stand one yard off the Character.
    let node_y = y + 1.0;
    for (id, client_node, node_x) in [
        (SOURCE_NODE, SOURCE_CLIENT_NODE, x),
        (DESTINATION_NODE, DESTINATION_CLIENT_NODE, x + FLIGHT_YARDS),
    ] {
        shard.insert_row(
            "game_taxi_node",
            &[
                ("id", &id),
                ("client_node_id", &client_node),
                ("map_id", &map_id),
                ("x", &node_x),
                ("y", &node_y),
                ("z", &z),
                ("name", &"Activation fixture"),
                ("mount_display_horde", &295),
                ("mount_display_alliance", &MOUNT_DISPLAY),
            ],
        );
    }
    insert_path(&shard);
    for (id, index, node_x) in [(5_090_304u32, 0u32, x), (5_090_305, 1, x + FLIGHT_YARDS)] {
        shard.insert_row(
            "game_taxi_path_node",
            &[
                ("id", &id),
                ("path_id", &5_090_303),
                ("node_index", &index),
                ("map_id", &map_id),
                ("x", &node_x),
                ("y", &y),
                ("z", &z),
                ("flags", &0),
                ("delay_ms", &0),
            ],
        );
    }
    for node in [SOURCE_NODE, DESTINATION_NODE] {
        know(&shard, node);
    }
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([1])).unwrap();
    wire.drain_packets();
    (shard, wire, master)
}

fn insert_path(shard: &Standalone) {
    shard.insert_row(
        "game_taxi_path",
        &[
            ("id", &5_090_303),
            ("source_node_id", &SOURCE_NODE),
            ("destination_node_id", &DESTINATION_NODE),
            ("fare", &FARE),
        ],
    );
}

fn know(shard: &Standalone, node: u32) {
    shard.insert_row(
        "game_character_taxi_node",
        &[("id", &0), ("character_guid", &1), ("node_id", &node)],
    );
}

fn activate(wire: &mut support::ReducerConnection, master: &str) -> Vec<serde_json::Value> {
    activate_guid(wire, master.parse().unwrap())
}

fn activate_guid(wire: &mut support::ReducerConnection, master: u64) -> Vec<serde_json::Value> {
    activate_nodes(wire, master, SOURCE_CLIENT_NODE, DESTINATION_CLIENT_NODE)
}

fn activate_nodes(
    wire: &mut support::ReducerConnection,
    master: u64,
    source: u32,
    destination: u32,
) -> Vec<serde_json::Value> {
    let mut body = master.to_le_bytes().to_vec();
    body.extend(source.to_le_bytes());
    body.extend(destination.to_le_bytes());
    wire.call("native_world_receive", json!([ACTIVATE_TAXI, body]))
        .unwrap()
        .inserts("game_native_packet")
        .iter()
        .map(|row| serde_json::from_str(row.get()).unwrap())
        .collect()
}

fn express(
    wire: &mut support::ReducerConnection,
    master: u64,
    nodes: &[u32],
) -> Vec<serde_json::Value> {
    let mut body = master.to_le_bytes().to_vec();
    body.extend(0u32.to_le_bytes());
    body.extend((nodes.len() as u32).to_le_bytes());
    body.extend(nodes.iter().flat_map(|node| node.to_le_bytes()));
    wire.call("native_world_receive", json!([ACTIVATE_TAXI_EXPRESS, body]))
        .unwrap()
        .inserts("game_native_packet")
        .iter()
        .map(|row| serde_json::from_str(row.get()).unwrap())
        .collect()
}

fn node_status_replies(wire: &mut support::ReducerConnection, master: u64) -> Vec<String> {
    wire.call(
        "native_world_receive",
        json!([TAXI_NODE_STATUS_QUERY, master.to_le_bytes().to_vec()]),
    )
    .unwrap()
    .inserts("game_native_packet")
    .iter()
    .map(|row| serde_json::from_str::<serde_json::Value>(row.get()).unwrap())
    .filter(|packet| packet[2][1] == TAXI_NODE_STATUS)
    .map(|packet| packet[3].as_str().unwrap().to_owned())
    .collect()
}

fn node_status(master: u64, known: bool) -> String {
    let guid: String = master
        .to_le_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{guid}{:02x}", u8::from(known))
}

fn known_node_status(master: u64) -> String {
    node_status(master, true)
}

fn stun_the_character(shard: &Standalone) {
    insert_self_spell(shard, STUN_SELF, 0, A_CONTROL, M_STUN);
    shard.assert_call("debug_force_cast", &["1", &STUN_SELF.to_string()]);
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT id FROM game_aura WHERE target_guid = 1 AND spell_id = {STUN_SELF}"
            ))
            .len(),
        1
    );
}

fn insert_reverse_path(shard: &Standalone) {
    shard.insert_row(
        "game_taxi_path",
        &[
            ("id", &REVERSE_PATH),
            ("source_node_id", &DESTINATION_NODE),
            ("destination_node_id", &SOURCE_NODE),
            ("fare", &FARE),
        ],
    );
}

fn replies(packets: &[serde_json::Value]) -> Vec<String> {
    packets
        .iter()
        .filter(|packet| packet[2][1] == ACTIVATE_TAXI_REPLY)
        .map(|packet| packet[3].as_str().unwrap().to_owned())
        .collect()
}

fn status(code: u8) -> String {
    format!("{code:02x}000000")
}

fn money(shard: &Standalone) -> u32 {
    number(
        &shard.query_rows("SELECT money FROM game_world_entity WHERE guid = 1")[0],
        "money",
    )
}

fn flights(shard: &Standalone) -> usize {
    shard
        .query_rows("SELECT character_guid FROM game_active_taxi_flight")
        .len()
}

fn assert_unchanged(shard: &Standalone, what: &str) {
    assert_eq!((flights(shard), money(shard)), (0, 1000), "{what}");
}

fn column(shard: &Standalone, table: &str, filter: &str, column: &str) -> String {
    shard.query_rows(&format!("SELECT {column} FROM {table} WHERE {filter}"))[0][column].clone()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_flying_character_gets_no_reply_and_no_second_charge() {
    let (shard, mut wire, master) = stage("taxi-activation-flying", 1000);
    assert_eq!(replies(&activate(&mut wire, &master)), [status(0)]);
    assert_eq!((flights(&shard), money(&shard)), (1, 1000 - FARE));

    assert!(replies(&activate(&mut wire, &master)).is_empty());
    assert_eq!((flights(&shard), money(&shard)), (1, 1000 - FARE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_missing_flight_master_gets_no_reply() {
    let (shard, mut wire, _master) = stage("taxi-activation-missing", 1000);
    assert!(replies(&activate_guid(&mut wire, MISSING_MASTER)).is_empty());
    assert_unchanged(&shard, "a missing flight master");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_request_out_of_range_of_the_flight_master_gets_no_reply() {
    let (shard, mut wire, master) = stage("taxi-activation-range", 1000);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 500 WHERE guid = {master}"
    ));
    assert!(replies(&activate(&mut wire, &master)).is_empty());
    assert_eq!((flights(&shard), money(&shard)), (0, 1000));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_express_request_gets_no_reply_where_vmangos_is_silent() {
    let (shard, mut wire, master) = stage("taxi-activation-express", 1000);
    let master: u64 = master.parse().unwrap();
    let nodes = [SOURCE_CLIENT_NODE, DESTINATION_CLIENT_NODE];
    assert!(replies(&express(&mut wire, MISSING_MASTER, &nodes)).is_empty());
    assert!(replies(&express(&mut wire, master, &[])).is_empty());
    assert!(replies(&express(&mut wire, master, &nodes[..1])).is_empty());
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 500 WHERE guid = {master}"
    ));
    assert!(replies(&express(&mut wire, master, &nodes)).is_empty());
    assert_unchanged(&shard, "an express request");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_two_node_express_request_flies() {
    let (shard, mut wire, master) = stage("taxi-activation-express-flies", 1000);
    let nodes = [SOURCE_CLIENT_NODE, DESTINATION_CLIENT_NODE];
    assert_eq!(
        replies(&express(&mut wire, master.parse().unwrap(), &nodes)),
        [status(0)]
    );
    assert_eq!((flights(&shard), money(&shard)), (1, 1000 - FARE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_multi_hop_express_request_in_combat_gets_status_7() {
    let (shard, mut wire, master) = stage("taxi-activation-express-combat", 1000);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {IN_COMBAT} WHERE guid = 1"
    ));
    let nodes = [
        SOURCE_CLIENT_NODE,
        DESTINATION_CLIENT_NODE,
        SOURCE_CLIENT_NODE,
    ];
    assert_eq!(
        replies(&express(&mut wire, master.parse().unwrap(), &nodes)),
        [status(7)]
    );
    assert_eq!((flights(&shard), money(&shard)), (0, 1000));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_multi_hop_express_request_with_an_unvisited_node_gets_no_reply() {
    let (shard, mut wire, master) = stage("taxi-activation-express-unvisited", 1000);
    let destination = &shard.query_rows(&format!(
        "SELECT map_id, x, y, z FROM game_taxi_node WHERE id = {DESTINATION_NODE}"
    ))[0];
    let (map_id, x, y, z): (u32, f32, f32, f32) = (
        number(destination, "map_id"),
        number(destination, "x"),
        number(destination, "y"),
        number(destination, "z"),
    );
    // The third node is catalogued and has a path from the second, so only the visit check
    // can keep this request silent.
    shard.insert_row(
        "game_taxi_node",
        &[
            ("id", &UNVISITED_NODE),
            ("client_node_id", &UNVISITED_CLIENT_NODE),
            ("map_id", &map_id),
            ("x", &(x + FLIGHT_YARDS)),
            ("y", &y),
            ("z", &z),
            ("name", &"Unvisited fixture"),
            ("mount_display_horde", &295),
            ("mount_display_alliance", &MOUNT_DISPLAY),
        ],
    );
    shard.insert_row(
        "game_taxi_path",
        &[
            ("id", &UNVISITED_PATH),
            ("source_node_id", &DESTINATION_NODE),
            ("destination_node_id", &UNVISITED_NODE),
            ("fare", &FARE),
        ],
    );
    let nodes = [
        SOURCE_CLIENT_NODE,
        DESTINATION_CLIENT_NODE,
        UNVISITED_CLIENT_NODE,
    ];
    assert!(replies(&express(&mut wire, master.parse().unwrap(), &nodes)).is_empty());
    assert_eq!((flights(&shard), money(&shard)), (0, 1000));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_multi_hop_express_request_with_a_leg_without_a_path_gets_no_reply() {
    let (shard, mut wire, master) = stage("taxi-activation-express-no-path", 1000);
    let nodes = [
        SOURCE_CLIENT_NODE,
        DESTINATION_CLIENT_NODE,
        SOURCE_CLIENT_NODE,
    ];
    assert!(replies(&express(&mut wire, master.parse().unwrap(), &nodes)).is_empty());
    assert_eq!((flights(&shard), money(&shard)), (0, 1000));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_multi_hop_express_request_the_module_cannot_fly_gets_status_1() {
    let (shard, mut wire, master) = stage("taxi-activation-express-multi-hop", 1000);
    insert_reverse_path(&shard);
    let nodes = [
        SOURCE_CLIENT_NODE,
        DESTINATION_CLIENT_NODE,
        SOURCE_CLIENT_NODE,
    ];
    assert_eq!(
        replies(&express(&mut wire, master.parse().unwrap(), &nodes)),
        [status(1)]
    );
    assert_eq!((flights(&shard), money(&shard)), (0, 1000));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn not_enough_money_gets_status_3() {
    let (shard, mut wire, master) = stage("taxi-activation-fare", FARE - 1);
    assert_eq!(replies(&activate(&mut wire, &master)), [status(3)]);
    assert_eq!((flights(&shard), money(&shard)), (0, FARE - 1));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn each_refusal_vmangos_answers_gets_its_status() {
    let (shard, mut wire, master) = stage("taxi-activation-statuses", 1000);
    let source_at = |axis: &str| -> f32 {
        column(
            &shard,
            "game_taxi_node",
            &format!("id = {SOURCE_NODE}"),
            axis,
        )
        .parse()
        .unwrap()
    };
    let (source_x, source_y, source_z) = (source_at("x"), source_at("y"), source_at("z"));
    let player_map = column(&shard, "game_world_entity", "guid = 1", "map_id");
    let node = |assignment: String| {
        format!("UPDATE game_taxi_node SET {assignment} WHERE id = {SOURCE_NODE}")
    };
    let player =
        |assignment: &str| format!("UPDATE game_world_entity SET {assignment} WHERE guid = 1");
    let cases = [
        (
            "in combat",
            player(&format!("unit_flags = {IN_COMBAT}")),
            player("unit_flags = 0"),
            7,
        ),
        (
            "in combat and mounted, which vmangos checks first",
            player(&format!(
                "unit_flags = {IN_COMBAT}, mount_display_id = {MOUNT_DISPLAY}"
            )),
            player("unit_flags = 0, mount_display_id = 0"),
            7,
        ),
        (
            "mounted",
            player(&format!("mount_display_id = {MOUNT_DISPLAY}")),
            player("mount_display_id = 0"),
            8,
        ),
        (
            "source node out of reach",
            node(format!("x = {}", source_x + 100.0)),
            node(format!("x = {source_x}")),
            4,
        ),
        (
            "source node on another map",
            node("map_id = 999".into()),
            node(format!("map_id = {player_map}")),
            4,
        ),
        (
            "source node without a position",
            node("x = 0, y = 0, z = 0".into()),
            node(format!("x = {source_x}, y = {source_y}, z = {source_z}")),
            1,
        ),
        (
            "no mount display",
            node("mount_display_horde = 0, mount_display_alliance = 0".into()),
            node(format!(
                "mount_display_horde = 295, mount_display_alliance = {MOUNT_DISPLAY}"
            )),
            1,
        ),
        (
            "an unsupported route",
            "UPDATE game_taxi_path_node SET delay_ms = 1 WHERE id = 5090304".into(),
            "UPDATE game_taxi_path_node SET delay_ms = 0 WHERE id = 5090304".into(),
            2,
        ),
    ];
    for (what, set, reset, expected) in cases {
        shard.assert_sql(&set);
        assert_eq!(
            replies(&activate(&mut wire, &master)),
            [status(expected)],
            "{what}"
        );
        assert_unchanged(&shard, what);
        shard.assert_sql(&reset);
    }
    assert_eq!(replies(&activate(&mut wire, &master)), [status(0)]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_casting_character_gets_status_7() {
    let (shard, mut wire, master) = stage("taxi-activation-casting", 1000);
    insert_self_spell(&shard, SLOW_CAST, 60_000, A_INERT, 0);
    shard.assert_call("debug_begin_cast", &["1", &SLOW_CAST.to_string(), "1"]);
    assert_eq!(
        shard
            .query_rows("SELECT caster_guid FROM game_pending_cast")
            .len(),
        1
    );
    assert_eq!(replies(&activate(&mut wire, &master)), [status(7)]);
    assert_unchanged(&shard, "a casting Character");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_shapeshifted_character_gets_status_9() {
    let (shard, mut wire, master) = stage("taxi-activation-shapeshifted", 1000);
    shard.assert_call("debug_seed_druid_forms", &[]);
    shard.assert_call("debug_force_cast", &["1", &CAT_FORM.to_string()]);
    assert_eq!(replies(&activate(&mut wire, &master)), [status(9)]);
    assert_unchanged(&shard, "a shapeshifted Character");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_moving_character_still_gets_its_flight() {
    let (shard, mut wire, master) = stage("taxi-activation-moving", 1000);
    shard.assert_sql("UPDATE game_world_entity SET movement_flags = 1 WHERE guid = 1");
    assert_eq!(replies(&activate(&mut wire, &master)), [status(0)]);
    assert_eq!((flights(&shard), money(&shard)), (1, 1000 - FARE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_sitting_character_still_gets_its_flight() {
    let (shard, mut wire, master) = stage("taxi-activation-sitting", 1000);
    shard.assert_sql("UPDATE game_world_entity SET unit_bytes_1 = 1 WHERE guid = 1");
    assert_eq!(replies(&activate(&mut wire, &master)), [status(0)]);
    assert_eq!((flights(&shard), money(&shard)), (1, 1000 - FARE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_flight_master_that_cannot_be_used_gets_no_reply() {
    let (shard, mut wire, master) = stage("taxi-activation-master", 1000);
    let entity = |assignment: &str| {
        format!("UPDATE game_world_entity SET {assignment} WHERE guid = {master}")
    };
    let health = column(
        &shard,
        "game_world_entity",
        &format!("guid = {master}"),
        "health",
    );
    let faction = column(
        &shard,
        "game_world_entity",
        &format!("guid = {master}"),
        "faction_template",
    );
    seed_hostile_factions(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = {FRIENDLY_FACTION_TEMPLATE} WHERE guid = 1"
    ));
    let cases = [
        (
            "dead",
            entity("dead = true, health = 0"),
            entity(&format!("dead = false, health = {health}")),
        ),
        (
            "not a flight master",
            entity("npc_flags = 0"),
            entity("npc_flags = 9"),
        ),
        (
            "not selectable",
            entity(&format!("unit_flags = {NOT_SELECTABLE}")),
            entity("unit_flags = 0"),
        ),
        (
            "in combat",
            entity(&format!("unit_flags = {IN_COMBAT}")),
            entity("unit_flags = 0"),
        ),
        (
            "hostile",
            entity(&format!("faction_template = {HOSTILE_FACTION_TEMPLATE}")),
            entity(&format!("faction_template = {faction}")),
        ),
    ];
    for (what, set, reset) in cases {
        shard.assert_sql(&set);
        assert!(
            replies(&activate(&mut wire, &master)).is_empty(),
            "a flight master that is {what}"
        );
        assert_unchanged(&shard, what);
        shard.assert_sql(&reset);
    }
    assert_eq!(replies(&activate(&mut wire, &master)), [status(0)]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_character_that_cannot_interact_gets_no_reply() {
    let (shard, mut wire, master) = stage("taxi-activation-character", 1000);
    let health = column(&shard, "game_world_entity", "guid = 1", "health");
    let player =
        |assignment: &str| format!("UPDATE game_world_entity SET {assignment} WHERE guid = 1");
    let cases = [
        (
            "dead",
            player("dead = true, health = 0"),
            player(&format!("dead = false, health = {health}")),
        ),
        (
            "without client control",
            player(&format!("unit_flags = {REMOVE_CLIENT_CONTROL}")),
            player("unit_flags = 0"),
        ),
    ];
    for (what, set, reset) in cases {
        shard.assert_sql(&set);
        assert!(
            replies(&activate(&mut wire, &master)).is_empty(),
            "a Character that is {what}"
        );
        assert_unchanged(&shard, what);
        shard.assert_sql(&reset);
    }
    assert_eq!(replies(&activate(&mut wire, &master)), [status(0)]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stunned_character_gets_no_reply() {
    let (shard, mut wire, master) = stage("taxi-activation-stunned", 1000);
    stun_the_character(&shard);
    assert!(replies(&activate(&mut wire, &master)).is_empty());
    assert_unchanged(&shard, "a stunned Character");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_node_vmangos_does_not_accept_gets_no_reply() {
    let (shard, mut wire, master) = stage("taxi-activation-nodes", 1000);
    let master: u64 = master.parse().unwrap();
    let silent = |wire: &mut support::ReducerConnection, source, destination, what: &str| {
        assert!(
            replies(&activate_nodes(wire, master, source, destination)).is_empty(),
            "{what}"
        );
        assert_unchanged(&shard, what);
    };
    silent(
        &mut wire,
        SOURCE_CLIENT_NODE,
        SOURCE_CLIENT_NODE,
        "the same node twice",
    );
    silent(
        &mut wire,
        SOURCE_CLIENT_NODE,
        UNCATALOGUED_CLIENT_NODE,
        "a node outside the catalogue",
    );

    shard.assert_sql("DELETE FROM game_taxi_path WHERE id = 5090303");
    silent(
        &mut wire,
        SOURCE_CLIENT_NODE,
        DESTINATION_CLIENT_NODE,
        "two nodes without a path",
    );
    insert_path(&shard);

    shard.assert_sql(&format!(
        "DELETE FROM game_character_taxi_node WHERE node_id = {DESTINATION_NODE}"
    ));
    silent(
        &mut wire,
        SOURCE_CLIENT_NODE,
        DESTINATION_CLIENT_NODE,
        "an unvisited destination",
    );
    know(&shard, DESTINATION_NODE);

    shard.assert_sql(&format!(
        "DELETE FROM game_character_taxi_node WHERE node_id = {SOURCE_NODE}"
    ));
    silent(
        &mut wire,
        SOURCE_CLIENT_NODE,
        DESTINATION_CLIENT_NODE,
        "an unvisited source",
    );
    know(&shard, SOURCE_NODE);

    assert_eq!(
        replies(&activate(&mut wire, &master.to_string())),
        [status(0)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn refusals_come_in_the_vmangos_order() {
    let (shard, mut wire, master) = stage("taxi-activation-order", 1000);
    let player = |assignment: &str| {
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET {assignment} WHERE guid = 1"
        ))
    };
    let source_x: f32 = column(
        &shard,
        "game_taxi_node",
        &format!("id = {SOURCE_NODE}"),
        "x",
    )
    .parse()
    .unwrap();

    player(&format!("unit_flags = {IN_COMBAT}"));
    shard.assert_sql(&format!(
        "DELETE FROM game_character_taxi_node WHERE node_id = {DESTINATION_NODE}"
    ));
    assert_eq!(
        replies(&activate(&mut wire, &master)),
        [status(7)],
        "combat comes before an unknown node"
    );
    know(&shard, DESTINATION_NODE);

    player(&format!(
        "unit_flags = {}",
        IN_COMBAT | REMOVE_CLIENT_CONTROL
    ));
    assert_eq!(
        replies(&activate(&mut wire, &master)),
        [status(7)],
        "combat comes before the lack of client control"
    );

    player(&format!(
        "unit_flags = 0, mount_display_id = {MOUNT_DISPLAY}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_taxi_node SET x = {} WHERE id = {SOURCE_NODE}",
        source_x + 100.0
    ));
    assert_eq!(
        replies(&activate(&mut wire, &master)),
        [status(8)],
        "a mount comes before the reach of the source node"
    );
    assert_unchanged(&shard, "an ordered refusal");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_stunned_character_still_gets_the_node_status() {
    let (shard, mut wire, master) = stage("taxi-activation-status-stunned", 1000);
    stun_the_character(&shard);
    let master: u64 = master.parse().unwrap();
    assert_eq!(
        node_status_replies(&mut wire, master),
        [known_node_status(master)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_flight_master_in_combat_still_gets_the_node_status() {
    let (shard, mut wire, master) = stage("taxi-activation-status-combat", 1000);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {IN_COMBAT} WHERE guid = {master}"
    ));
    let master: u64 = master.parse().unwrap();
    assert_eq!(
        node_status_replies(&mut wire, master),
        [known_node_status(master)]
    );
}

fn insert_node_at_master(
    shard: &Standalone,
    master: u64,
    (id, client_node): (u32, u32),
    own_mount_alliance: bool,
    in_taxi_network: bool,
) {
    let flight_master = &shard.query_rows(&format!(
        "SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {master}"
    ))[0];
    let (map_id, x, y, z): (u32, f32, f32, f32) = (
        number(flight_master, "map_id"),
        number(flight_master, "x"),
        number(flight_master, "y"),
        number(flight_master, "z"),
    );
    // The importer fills the Alliance display from the Horde entry; only the flags differ.
    shard.insert_row(
        "game_taxi_node",
        &[
            ("id", &id),
            ("client_node_id", &client_node),
            ("map_id", &map_id),
            ("x", &x),
            ("y", &y),
            ("z", &z),
            ("name", &"Nearest-node fixture"),
            ("mount_display_horde", &295),
            ("mount_display_alliance", &295),
            ("own_mount_horde", &true),
            ("own_mount_alliance", &own_mount_alliance),
            ("in_taxi_network", &in_taxi_network),
        ],
    );
}

fn set_race(shard: &Standalone, race: u32) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {race} WHERE guid = 1"
    ));
}

fn received_packets(
    wire: &mut support::ReducerConnection,
    opcode: u32,
    master: u64,
) -> Vec<(u64, String)> {
    wire.call(
        "native_world_receive",
        json!([opcode, master.to_le_bytes().to_vec()]),
    )
    .unwrap()
    .inserts("game_native_packet")
    .iter()
    .map(|row| serde_json::from_str::<serde_json::Value>(row.get()).unwrap())
    .map(|packet| {
        (
            packet[2][1].as_u64().unwrap(),
            packet[3].as_str().unwrap().to_owned(),
        )
    })
    .collect()
}

fn taxi_menu_prefixes(wire: &mut support::ReducerConnection, master: u64) -> Vec<String> {
    received_packets(wire, TAXI_QUERY_AVAILABLE_NODES, master)
        .into_iter()
        .filter(|(opcode, _)| *opcode == SHOW_TAXI_NODES)
        .map(|(_, body)| body[..32].to_owned())
        .collect()
}

fn taxi_menu(master: u64, source_client_node: u32) -> String {
    let mut prefix = 1u32.to_le_bytes().to_vec();
    prefix.extend(master.to_le_bytes());
    prefix.extend(source_client_node.to_le_bytes());
    prefix.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn learned(shard: &Standalone, node: u32) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT id FROM game_character_taxi_node WHERE character_guid = 1 AND node_id = {node}"
        ))
        .is_empty()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_node_status_skips_a_node_without_a_mount_for_the_characters_team() {
    let (shard, mut wire, master) = stage("taxi-activation-status-team", 1000);
    let master: u64 = master.parse().unwrap();
    insert_node_at_master(
        &shard,
        master,
        (ONE_TEAM_NODE, ONE_TEAM_CLIENT_NODE),
        false,
        true,
    );
    for (race, known) in [(HORDE_RACE, false), (ALLIANCE_RACE, true)] {
        set_race(&shard, race);
        assert_eq!(
            node_status_replies(&mut wire, master),
            [node_status(master, known)],
            "race {race}"
        );
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_node_status_skips_a_node_outside_the_taxi_network() {
    let (shard, mut wire, master) = stage("taxi-activation-status-network", 1000);
    let master: u64 = master.parse().unwrap();
    insert_node_at_master(
        &shard,
        master,
        (OFF_NETWORK_NODE, OFF_NETWORK_CLIENT_NODE),
        true,
        false,
    );
    assert_eq!(
        node_status_replies(&mut wire, master),
        [known_node_status(master)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_taxi_menu_opens_on_the_nearest_node_however_far() {
    let (shard, mut wire, master) = stage("taxi-activation-menu-far", 1000);
    let master: u64 = master.parse().unwrap();
    // The seeded Northshire nodes stand within 10 yards of the master; without them only the
    // far node qualifies, which the old 10-yard lookup refused.
    shard.assert_sql("DELETE FROM game_taxi_node WHERE id = 5090100");
    shard.assert_sql("DELETE FROM game_taxi_node WHERE id = 5090101");
    let x: f32 = number(
        &shard.query_rows(&format!(
            "SELECT x FROM game_taxi_node WHERE id = {SOURCE_NODE}"
        ))[0],
        "x",
    );
    shard.assert_sql(&format!(
        "UPDATE game_taxi_node SET x = {} WHERE id = {SOURCE_NODE}",
        x + FAR_YARDS
    ));
    assert_eq!(
        taxi_menu_prefixes(&mut wire, master),
        [taxi_menu(master, SOURCE_CLIENT_NODE)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_taxi_menu_opens_on_and_teaches_the_node_with_the_characters_team_mount() {
    let (shard, mut wire, master) = stage("taxi-activation-menu-team", 1000);
    let master: u64 = master.parse().unwrap();
    insert_node_at_master(
        &shard,
        master,
        (ONE_TEAM_NODE, ONE_TEAM_CLIENT_NODE),
        false,
        true,
    );
    set_race(&shard, ALLIANCE_RACE);
    assert_eq!(
        taxi_menu_prefixes(&mut wire, master),
        [taxi_menu(master, SOURCE_CLIENT_NODE)]
    );
    assert!(!learned(&shard, ONE_TEAM_NODE));
    set_race(&shard, HORDE_RACE);
    assert!(taxi_menu_prefixes(&mut wire, master).is_empty());
    assert!(learned(&shard, ONE_TEAM_NODE));
    assert_eq!(
        taxi_menu_prefixes(&mut wire, master),
        [taxi_menu(master, ONE_TEAM_CLIENT_NODE)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn the_taxi_menu_opens_on_and_teaches_only_a_node_in_the_taxi_network() {
    let (shard, mut wire, master) = stage("taxi-activation-menu-network", 1000);
    let master: u64 = master.parse().unwrap();
    insert_node_at_master(
        &shard,
        master,
        (OFF_NETWORK_NODE, OFF_NETWORK_CLIENT_NODE),
        true,
        false,
    );
    assert_eq!(
        taxi_menu_prefixes(&mut wire, master),
        [taxi_menu(master, SOURCE_CLIENT_NODE)]
    );
    assert!(!learned(&shard, OFF_NETWORK_NODE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_first_visit_teaches_the_node_and_announces_the_path_without_a_menu() {
    let (shard, mut wire, master) = stage("taxi-activation-first-visit", 1000);
    let master: u64 = master.parse().unwrap();
    insert_node_at_master(
        &shard,
        master,
        (FIRST_VISIT_NODE, FIRST_VISIT_CLIENT_NODE),
        true,
        true,
    );
    assert!(!learned(&shard, FIRST_VISIT_NODE));
    assert_eq!(
        received_packets(&mut wire, TAXI_QUERY_AVAILABLE_NODES, master),
        [
            (NEW_TAXI_PATH, String::new()),
            (TAXI_NODE_STATUS, known_node_status(master)),
        ]
    );
    assert!(learned(&shard, FIRST_VISIT_NODE));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_second_visit_opens_the_menu_and_announces_nothing() {
    let (shard, mut wire, master) = stage("taxi-activation-second-visit", 1000);
    let master: u64 = master.parse().unwrap();
    insert_node_at_master(
        &shard,
        master,
        (FIRST_VISIT_NODE, FIRST_VISIT_CLIENT_NODE),
        true,
        true,
    );
    received_packets(&mut wire, TAXI_QUERY_AVAILABLE_NODES, master);
    let visit = received_packets(&mut wire, TAXI_QUERY_AVAILABLE_NODES, master);
    assert_eq!(
        visit.iter().map(|(opcode, _)| *opcode).collect::<Vec<_>>(),
        [SHOW_TAXI_NODES]
    );
    assert!(visit[0]
        .1
        .starts_with(&taxi_menu(master, FIRST_VISIT_CLIENT_NODE)));
}

fn give_the_flight_master_a_taxi_gossip_option(shard: &Standalone) {
    shard.insert_row(
        "game_gossip_option",
        &[
            ("row_id", &TAXI_GOSSIP_OPTION),
            ("entry", &FLIGHT_MASTER_ENTRY),
            ("text", &"I'd like to fly."),
            ("action", &lyracore_shared::constants::gossip_option::TAXI),
        ],
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_first_gossip_hello_teaches_the_node_and_opens_no_gossip_menu() {
    let (shard, mut wire, master) = stage("taxi-activation-first-hello", 1000);
    let master: u64 = master.parse().unwrap();
    give_the_flight_master_a_taxi_gossip_option(&shard);
    insert_node_at_master(
        &shard,
        master,
        (FIRST_VISIT_NODE, FIRST_VISIT_CLIENT_NODE),
        true,
        true,
    );
    assert_eq!(
        received_packets(&mut wire, GOSSIP_HELLO, master),
        [
            (NEW_TAXI_PATH, String::new()),
            (TAXI_NODE_STATUS, known_node_status(master)),
        ]
    );
    assert!(learned(&shard, FIRST_VISIT_NODE));
    let hello = received_packets(&mut wire, GOSSIP_HELLO, master);
    assert_eq!(
        hello.iter().map(|(opcode, _)| *opcode).collect::<Vec<_>>(),
        [GOSSIP_MESSAGE]
    );
}

fn insert_self_spell(
    shard: &Standalone,
    spell_id: u32,
    cast_time_ms: u32,
    kind: u8,
    mechanic: i32,
) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &format!("Taxi fixture {spell_id}")),
            ("school_mask", &1u8),
            ("range_yd", &30),
            ("is_negative", &false),
            ("cast_time_ms", &cast_time_ms),
            ("duration_ms", &600_000u32),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("effect_index", &0u8),
            ("kind", &kind),
            ("target", &T_SELF),
            ("p_0", &mechanic),
            ("p_0_kind", &P_MECHANIC),
        ],
    );
}

fn seed_hostile_factions(shard: &Standalone) {
    for (id, faction, group, friends, enemies) in [(1, 1, 3, 2, 12), (14, 16, 8, 0, 1)] {
        shard.assert_sql(&format!(
            "DELETE FROM game_faction_template WHERE id = {id}"
        ));
        shard.insert_row(
            "game_faction_template",
            &[
                ("id", &id),
                ("faction", &faction),
                ("faction_group", &group),
                ("friend_group", &friends),
                ("enemy_group", &enemies),
            ],
        );
    }
}
