mod support;

use std::io::Cursor;

use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{A_SELF_RESURRECT, P_SPELL_ID, T_TARGET_ALLY};
use serde_json::json;
use support::Standalone;
use wow_world_messages::vanilla::{
    ClientMessage, SpellCastTargets, SpellCastTargets_SpellCastTargetFlags,
    SpellCastTargets_SpellCastTargetFlags_Unit, CMSG_USE_ITEM,
};
use wow_world_messages::Guid;

const HEARTHSTONE: u32 = 6948;
const SOULSTONE_AURA: u32 = 20707;
const SOULSTONE_SELF_RES: u32 = 3026;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_soulstone_used_on_a_friendly_player_places_the_aura_on_that_player() {
    let mut shard = Standalone::start("item-use-friendly-target");
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"SOULSTONE\"", &credentials, &credentials],
    );
    let account = shard.query_rows("SELECT id FROM game_account WHERE username = 'SOULSTONE'")[0]
        ["id"]
        .clone();
    let warlock = create_character(&shard, &account, "Warlock", "9");
    let ally = create_character(&shard, &account, "Ally", "1");
    shard.assert_call("debug_spawn_player_entity", &[&ally.to_string()]);

    let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    connection.call("native_world_open", json!([42])).unwrap();
    connection
        .call("debug_native_enter_world", json!([warlock]))
        .unwrap();
    soulstone_item(&shard, warlock);
    connection.drain_packets();
    let slot: u8 = shard.query_rows(&format!(
        "SELECT slot FROM game_item_instance WHERE owner_guid = {warlock} AND entry = {HEARTHSTONE}"
    ))[0]["slot"]
        .parse()
        .unwrap();
    let mut frame = Vec::new();
    CMSG_USE_ITEM {
        bag_index: 255,
        bag_slot: slot,
        spell_index: 0,
        targets: SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
                SpellCastTargets_SpellCastTargetFlags_Unit {
                    unit_target: Guid::new(ally),
                },
            ),
        },
    }
    .write_unencrypted_client(&mut Cursor::new(&mut frame))
    .unwrap();
    // A client frame is a two-byte size and a four-byte opcode before the body.
    connection
        .call("native_world_receive", json!([0x0abu32, &frame[6..]]))
        .unwrap();

    assert!(
        has_aura(&shard, ally),
        "the targeted ally holds the Soulstone"
    );
    assert!(!has_aura(&shard, warlock), "the Warlock does not");
}

fn create_character(shard: &Standalone, account: &str, name: &str, class: &str) -> u64 {
    shard.assert_call(
        "create_character",
        &[
            account,
            &format!("\"{name}\""),
            "1",
            class,
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
        ],
    );
    shard.query_rows(&format!(
        "SELECT guid FROM game_character WHERE name = '{name}'"
    ))[0]["guid"]
        .parse()
        .unwrap()
}

fn soulstone_item(shard: &Standalone, owner: u64) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &SOULSTONE_AURA),
            ("name", &"Soulstone Resurrection"),
            ("range_yd", &30),
            ("duration_ms", &1800000),
            ("school_mask", &32),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(SOULSTONE_AURA, 0)),
            ("spell_id", &SOULSTONE_AURA),
            ("kind", &A_SELF_RESURRECT),
            ("die_sides", &1),
            ("target", &T_TARGET_ALLY),
            ("p_0", &SOULSTONE_SELF_RES),
            ("p_0_kind", &P_SPELL_ID),
        ],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET spellid_1 = {SOULSTONE_AURA}, spelltrigger_1 = 0 \
         WHERE entry = {HEARTHSTONE}"
    ));
    shard.assert_call(
        "debug_grant_item",
        &[&owner.to_string(), &HEARTHSTONE.to_string(), "1"],
    );
}

fn has_aura(shard: &Standalone, target: u64) -> bool {
    !shard
        .query_rows(&format!(
            "SELECT id FROM game_aura WHERE target_guid = {target} AND spell_id = {SOULSTONE_AURA}"
        ))
        .is_empty()
}
