mod support;

use lyracore_shared::constants::player_flags::{HIDE_CLOAK, HIDE_HELM};
use lyracore_test_support::{NativeAudience, NativePacket};
use serde_json::json;
use support::{number, ReducerConnection, Standalone};
use wire_client::decode::{self, Field};

const CMSG_TOGGLE_HELM: u32 = 0x02B9;
const CMSG_TOGGLE_CLOAK: u32 = 0x02BA;
const SMSG_UPDATE_OBJECT: u16 = 0x00A9;
const PLAYER_FLAGS_FIELD: &str = "field_190";

fn realm(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard
}

/// Enters the world as a new human on its own Account; an Account holds one World Session.
fn enter(shard: &Standalone, name: &str) -> (u64, ReducerConnection) {
    let username = name.to_uppercase();
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &[&format!("\"{username}\""), &credentials, &credentials],
    );
    let account = shard.query_rows(&format!(
        "SELECT id FROM game_account WHERE username = '{username}'"
    ))[0]["id"]
        .clone();
    shard.assert_call(
        "create_character",
        &[
            &account,
            &format!("\"{name}\""),
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
    let guid: u64 = number(
        &shard.query_rows(&format!(
            "SELECT guid FROM game_character WHERE name = '{name}'"
        ))[0],
        "guid",
    );
    let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    connection.call("native_world_open", json!([42])).unwrap();
    connection
        .call("debug_native_enter_world", json!([guid]))
        .unwrap();
    (guid, connection)
}

fn toggle(connection: &mut ReducerConnection, opcode: u32) -> Vec<NativePacket> {
    connection
        .call("native_world_receive", json!([opcode, []]))
        .unwrap()
        .packets()
        .to_vec()
}

fn player_flags(shard: &Standalone, guid: u64) -> u32 {
    number(
        &shard.query_rows(&format!(
            "SELECT player_flags FROM game_world_entity WHERE guid = {guid}"
        ))[0],
        "player_flags",
    )
}

fn stored_flags(shard: &Standalone, guid: u64) -> u32 {
    number(
        &shard.query_rows(&format!(
            "SELECT hide_equipment_flags FROM game_character WHERE guid = {guid}"
        ))[0],
        "hide_equipment_flags",
    )
}

/// PLAYER_FLAGS values that VALUES updates for `guid` carried to its viewers.
fn broadcast_player_flags(packets: &[NativePacket], guid: u64) -> Vec<u64> {
    packets
        .iter()
        .filter(|packet| {
            packet.opcode == Some(SMSG_UPDATE_OBJECT)
                && matches!(packet.audience, NativeAudience::Aoi { .. })
        })
        .flat_map(|packet| {
            let decoded = decode::decode(SMSG_UPDATE_OBJECT, &packet.body).unwrap();
            let Some(Field::List(objects)) = decoded.fields.get("objects") else {
                return Vec::new();
            };
            objects
                .iter()
                .filter_map(|object| {
                    let Field::Object(object) = object else {
                        return None;
                    };
                    let Some(Field::Guid(subject)) = object.get("guid") else {
                        return None;
                    };
                    let Some(Field::Object(values)) = object.get("values") else {
                        return None;
                    };
                    match values.get(PLAYER_FLAGS_FIELD) {
                        Some(Field::Unsigned(flags)) if *subject == guid => Some(*flags),
                        _ => None,
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn toggling_the_helm_and_the_cloak_flips_their_flags_and_viewers_see_player_flags() {
    let shard = realm("equipment-display-toggle");
    let (_, mut watcher) = enter(&shard, "Watcher");
    let (guid, mut toggler) = enter(&shard, "Toggler");
    let before = player_flags(&shard, guid);
    watcher.drain_packets();

    let hidden_helm = toggle(&mut toggler, CMSG_TOGGLE_HELM);
    assert_eq!(player_flags(&shard, guid), before | HIDE_HELM);
    assert_eq!(
        broadcast_player_flags(&hidden_helm, guid),
        [u64::from(before | HIDE_HELM)]
    );

    toggle(&mut toggler, CMSG_TOGGLE_CLOAK);
    assert_eq!(player_flags(&shard, guid), before | HIDE_HELM | HIDE_CLOAK);

    let shown_helm = toggle(&mut toggler, CMSG_TOGGLE_HELM);
    assert_eq!(player_flags(&shard, guid), before | HIDE_CLOAK);
    assert_eq!(
        broadcast_player_flags(&shown_helm, guid),
        [u64::from(before | HIDE_CLOAK)]
    );

    toggle(&mut toggler, CMSG_TOGGLE_CLOAK);
    assert_eq!(player_flags(&shard, guid), before);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hidden_helm_and_cloak_survive_logout_and_login() {
    let shard = realm("equipment-display-relog");
    let (guid, mut connection) = enter(&shard, "Hider");
    toggle(&mut connection, CMSG_TOGGLE_HELM);
    toggle(&mut connection, CMSG_TOGGLE_CLOAK);

    shard.assert_call("debug_logout_character", &[&guid.to_string()]);
    assert_eq!(stored_flags(&shard, guid), HIDE_HELM | HIDE_CLOAK);

    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    assert_eq!(
        player_flags(&shard, guid) & (HIDE_HELM | HIDE_CLOAK),
        HIDE_HELM | HIDE_CLOAK
    );
}
