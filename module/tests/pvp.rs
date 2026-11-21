mod support;

use std::io::Cursor;

use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::{player_flags, unit_flags};
use lyracore_shared::spell::taxonomy::{
    E_DAMAGE, E_DUEL, E_HEAL, P_GAMEOBJECT_ENTRY, T_AREA_ENEMY, T_TARGET_ALLY, T_TARGET_ENEMY,
};
use lyracore_test_support::ReducerConnection;
use serde_json::json;
use support::{number, Standalone};
use wow_world_messages::vanilla::{
    ClientMessage, SpellCastTargets, SpellCastTargets_SpellCastTargetFlags,
    SpellCastTargets_SpellCastTargetFlags_Unit, CMSG_CAST_SPELL,
};
use wow_world_messages::Guid;

const ALLIANCE_FACTION: u32 = 5_091_400;
const HORDE_FACTION: u32 = 5_091_401;
const ARCANE_BURST: u32 = 5_091_402;
const DUEL_CHALLENGE: u32 = 5_091_403;
const DUEL_FLAG: u32 = 5_091_404;
const MENDING_TOUCH: u32 = 5_091_405;
const IGNORE_CONTACT: u32 = 5_091_406;
const DUEL: u64 = 5_091_407;
const CMSG_TOGGLE_PVP: u32 = 0x253;
const CMSG_CAST_SPELL_OPCODE: u32 = 0x12e;
// 1.12.1 SpellCastResult values from vmangos SpellDefines.h.
const NO_DUELING: u8 = 0x47;
const TARGET_DUELING: u8 = 0x68;
const TARGET_ENEMY: u8 = 0x69;
// `state::REQUESTED` and `state::ACTIVE` in `module/src/duel.rs`.
const REQUESTED: u8 = 0;
const ACTIVE: u8 = 2;
// vmangos Player::UpdatePvP holds the flag 300 s, 6000 ticks of the 50 ms virtual clock.
const PVP_FLAG_TICKS: u32 = 6000;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_unflagged_enemy_character_is_outside_an_area_spell_until_it_flags() {
    let (shard, account) = realm("pvp-area-spell");
    let mage = character(&shard, &account, "Mage");
    let rival = character(&shard, &account, "Rival");
    for guid in [mage, rival] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    }
    hostile_factions(&shard, mage, rival);
    area_spell(&shard);
    let mage_flags = flags(&shard, mage).0;
    let rival_flags = flags(&shard, rival).0;
    let rival_health = health(&shard, rival);

    for _ in 0..5 {
        shard.assert_call(
            "debug_force_cast",
            &[&mage.to_string(), &ARCANE_BURST.to_string()],
        );
    }
    assert_eq!(
        health(&shard, rival),
        rival_health,
        "vmangos IsValidAttackTarget"
    );
    assert_eq!(flags(&shard, mage).0, mage_flags);

    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {} WHERE guid = {rival}",
        rival_flags | unit_flags::PVP
    ));
    let mut casts = 0;
    while health(&shard, rival) == rival_health {
        casts += 1;
        assert!(casts <= 20, "the flagged rival never took the area damage");
        shard.assert_call(
            "debug_force_cast",
            &[&mage.to_string(), &ARCANE_BURST.to_string()],
        );
    }
    assert_ne!(
        flags(&shard, mage).0 & unit_flags::PVP,
        0,
        "harm to a flagged enemy flags the attacker"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn toggling_pvp_flags_at_once_and_the_flag_drops_five_minutes_after_desire_clears() {
    let (shard, account) = realm("pvp-toggle");
    shard.clear_schedule("game_creature_move_schedule");
    let guid = character(&shard, &account, "Toggler");
    let mut connection = enter(&shard, guid);
    let (unit_flags_before, player_flags_before) = flags(&shard, guid);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    let flagged = (
        unit_flags_before | unit_flags::PVP,
        player_flags_before | player_flags::PVP_DESIRED,
    );

    toggle(&mut connection, &[]);
    assert_eq!(
        flags(&shard, guid),
        flagged,
        "a bare toggle sets PVP_DESIRED"
    );
    advance(&shard, PVP_FLAG_TICKS);
    assert_eq!(
        flags(&shard, guid),
        flagged,
        "PVP_DESIRED freezes the countdown"
    );

    toggle(&mut connection, &[0]);
    let undesired = (flagged.0, player_flags_before);
    assert_eq!(flags(&shard, guid), undesired);
    advance(&shard, PVP_FLAG_TICKS - 1);
    assert_eq!(flags(&shard, guid), undesired);
    advance(&shard, 1);
    assert_eq!(
        flags(&shard, guid),
        (unit_flags_before, player_flags_before)
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_duel_challenge_refuses_with_the_vmangos_cast_result() {
    let (shard, account) = realm("duel-challenge");
    shard.clear_schedule("game_duel_schedule");
    let challenger = character(&shard, &account, "Challenger");
    let rival = character(&shard, &account, "Rival");
    let third = character(&shard, &account, "Third");
    for guid in [rival, third] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    }
    duel_spell(&shard);
    shard.assert_call(
        "debug_learn_spell",
        &[&challenger.to_string(), &DUEL_CHALLENGE.to_string()],
    );
    let mut connection = enter(&shard, challenger);

    insert_duel(&shard, third, rival, REQUESTED);
    let refused = challenge(&mut connection, rival).expect_err("the rival is dueling");
    assert!(
        refused.contains(&cast_result(TARGET_DUELING)),
        "vmangos Spell::CheckCast: {refused}"
    );
    assert_eq!(duels(&shard), 1);

    shard.assert_sql("DELETE FROM game_duel");
    insert_duel(&shard, challenger, rival, ACTIVE);
    let refused = challenge(&mut connection, rival).expect_err("they already duel");
    assert!(
        refused.contains(&cast_result(TARGET_ENEMY)),
        "vmangos Spell::EffectDuel: {refused}"
    );

    shard.assert_sql("DELETE FROM game_duel");
    shard.assert_sql(&format!(
        "INSERT INTO game_character_contact (id,owner_guid,owner_identity,target_guid,is_ignore) \
         VALUES ({IGNORE_CONTACT},{rival},0x{},{challenger},true)",
        "00".repeat(32)
    ));
    challenge(&mut connection, rival).expect("an ignored challenge drops without a cast result");
    assert_eq!(duels(&shard), 0);

    shard.assert_sql(&format!(
        "DELETE FROM game_character_contact WHERE id = {IGNORE_CONTACT}"
    ));
    let at = shard.query_rows(&format!(
        "SELECT x, y FROM game_world_entity WHERE guid = {challenger}"
    ));
    // Stormwind's AreaTable.dbc flags lack AREA_FLAG_DUEL (0x40).
    shard.insert_row(
        "game_area",
        &[
            ("id", &1519),
            ("area_bit", &1),
            ("flags", &312),
            ("faction_group", &2),
            ("name", &"Stormwind City"),
        ],
    );
    let cell =
        |axis: &str| lyracore_shared::terrain::cell_index(number::<f32>(&at[0], axis)).unwrap();
    let heights = vec!["0"; 145].join(":");
    shard.assert_call(
        "import_terrain_chunks",
        &[&format!(
            "0,{},{},0,0,0,1519,{heights}",
            cell("x"),
            cell("y")
        )],
    );
    let refused = challenge(&mut connection, rival).expect_err("a capital forbids Duels");
    assert!(
        refused.contains(&cast_result(NO_DUELING)),
        "vmangos Spell::EffectDuel: {refused}"
    );
    assert_eq!(duels(&shard), 0);

    shard.assert_sql("UPDATE game_area SET flags = 376 WHERE id = 1519");
    challenge(&mut connection, rival).expect("AREA_FLAG_DUEL allows the Duel");
    assert_eq!(duels(&shard), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_third_character_cannot_heal_a_duelist_once_the_duel_starts() {
    let (shard, account) = realm("duel-outside-heal");
    shard.clear_schedule("game_duel_schedule");
    let duelist = character(&shard, &account, "Duelist");
    let opponent = character(&shard, &account, "Opponent");
    let healer = character(&shard, &account, "Healer");
    for guid in [duelist, opponent, healer] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    }
    heal_spell(&shard);
    shard.assert_call("debug_set_health", &[&duelist.to_string(), "1"]);
    insert_duel(&shard, duelist, opponent, ACTIVE);

    let refused = shard.call(
        "debug_force_cast_at",
        &[
            &healer.to_string(),
            &MENDING_TOUCH.to_string(),
            &duelist.to_string(),
        ],
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(!refused.status.success(), "vmangos IsValidHelpfulTarget");
    assert!(text.contains("targets allies"), "{text}");
    assert_eq!(health(&shard, duelist), 1);

    shard.assert_sql(&format!("UPDATE game_duel SET state = {REQUESTED}"));
    shard.assert_call(
        "debug_force_cast_at",
        &[
            &healer.to_string(),
            &MENDING_TOUCH.to_string(),
            &duelist.to_string(),
        ],
    );
    assert!(
        health(&shard, duelist) > 1,
        "a Duel not yet started allows help"
    );
}

fn realm(name: &str) -> (Standalone, String) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    let credentials = serde_json::to_string(&vec![0u8; 32]).unwrap();
    shard.assert_call(
        "provision_account",
        &["\"PVP\"", &credentials, &credentials],
    );
    let account =
        shard.query_rows("SELECT id FROM game_account WHERE username = 'PVP'")[0]["id"].clone();
    (shard, account)
}

fn character(shard: &Standalone, account: &str, name: &str) -> u64 {
    shard.assert_call(
        "create_character",
        &[
            account,
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
    number(
        &shard.query_rows(&format!(
            "SELECT guid FROM game_character WHERE name = '{name}'"
        ))[0],
        "guid",
    )
}

fn enter(shard: &Standalone, guid: u64) -> ReducerConnection {
    let mut connection = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    connection.call("native_world_open", json!([42])).unwrap();
    connection
        .call("debug_native_enter_world", json!([guid]))
        .unwrap();
    connection
}

fn hostile_factions(shard: &Standalone, alliance: u64, horde: u64) {
    shard.insert_rows(
        "game_faction_template",
        &[
            &[
                ("id", &ALLIANCE_FACTION),
                ("faction", &ALLIANCE_FACTION),
                ("faction_group", &1),
                ("friend_group", &1),
                ("enemy_group", &2),
            ],
            &[
                ("id", &HORDE_FACTION),
                ("faction", &HORDE_FACTION),
                ("faction_group", &2),
                ("friend_group", &2),
                ("enemy_group", &1),
            ],
        ],
    );
    for (guid, faction) in [(alliance, ALLIANCE_FACTION), (horde, HORDE_FACTION)] {
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET faction_template = {faction} WHERE guid = {guid}"
        ));
    }
}

fn area_spell(shard: &Standalone) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &ARCANE_BURST),
            ("name", &"PvP area burst fixture"),
            ("school_mask", &64),
            ("is_negative", &true),
            ("gcd_ms", &0),
            ("cooldown_ms", &0),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(ARCANE_BURST, 0)),
            ("spell_id", &ARCANE_BURST),
            ("kind", &E_DAMAGE),
            ("base_points", &5),
            ("target", &T_AREA_ENEMY),
            ("radius_yd", &10.0),
        ],
    );
}

fn duel_spell(shard: &Standalone) {
    shard.insert_row(
        "game_gameobject_template",
        &[
            ("entry", &DUEL_FLAG),
            ("type_id", &16),
            ("display_id", &787),
            ("name", &"PvP duel flag fixture"),
        ],
    );
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &DUEL_CHALLENGE),
            ("name", &"PvP duel challenge fixture"),
            ("range_yd", &30),
            ("gcd_ms", &0),
            ("cooldown_ms", &0),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(DUEL_CHALLENGE, 0)),
            ("spell_id", &DUEL_CHALLENGE),
            ("kind", &E_DUEL),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &DUEL_FLAG),
            ("p_0_kind", &P_GAMEOBJECT_ENTRY),
        ],
    );
}

fn heal_spell(shard: &Standalone) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &MENDING_TOUCH),
            ("name", &"PvP mending touch fixture"),
            ("school_mask", &2),
            ("range_yd", &30),
            ("gcd_ms", &0),
            ("cooldown_ms", &0),
            ("spell_level", &1),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(MENDING_TOUCH, 0)),
            ("spell_id", &MENDING_TOUCH),
            ("kind", &E_HEAL),
            ("base_points", &20),
            ("target", &T_TARGET_ALLY),
        ],
    );
}

fn insert_duel(shard: &Standalone, initiator: u64, challenged: u64, state: u8) {
    let at = shard.query_rows(&format!(
        "SELECT map_id, instance_id, x, y, z FROM game_world_entity WHERE guid = {challenged}"
    ));
    shard.insert_row(
        "game_duel",
        &[
            ("id", &DUEL),
            ("flag_entry", &DUEL_FLAG),
            ("initiator_guid", &initiator),
            ("challenged_guid", &challenged),
            ("map_id", &at[0]["map_id"]),
            ("instance_id", &at[0]["instance_id"]),
            ("flag_x", &at[0]["x"]),
            ("flag_y", &at[0]["y"]),
            ("flag_z", &at[0]["z"]),
            ("state", &state),
        ],
    );
}

fn duels(shard: &Standalone) -> usize {
    shard.query_rows("SELECT id FROM game_duel").len()
}

fn toggle(connection: &mut ReducerConnection, body: &[u8]) {
    connection
        .call("native_world_receive", json!([CMSG_TOGGLE_PVP, body]))
        .unwrap();
}

fn challenge(connection: &mut ReducerConnection, target: u64) -> Result<(), String> {
    let mut frame = Vec::new();
    CMSG_CAST_SPELL {
        spell: DUEL_CHALLENGE,
        targets: SpellCastTargets {
            target_flags: SpellCastTargets_SpellCastTargetFlags::new_unit(
                SpellCastTargets_SpellCastTargetFlags_Unit {
                    unit_target: Guid::new(target),
                },
            ),
        },
    }
    .write_unencrypted_client(&mut Cursor::new(&mut frame))
    .unwrap();
    // A client frame is a two-byte size and a four-byte opcode before the body.
    connection
        .call(
            "native_world_receive",
            json!([CMSG_CAST_SPELL_OPCODE, &frame[6..]]),
        )
        .map(|_| ())
}

/// The refused cast's last reply: SMSG_CAST_RESULT (0x130) with the spell, status 2 and `result`.
fn cast_result(result: u8) -> String {
    let body = DUEL_CHALLENGE
        .to_le_bytes()
        .iter()
        .chain(&[2, result])
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("[304, [{body}]]]")
}

fn flags(shard: &Standalone, guid: u64) -> (u32, u32) {
    let row = &shard.query_rows(&format!(
        "SELECT unit_flags, player_flags FROM game_world_entity WHERE guid = {guid}"
    ))[0];
    (number(row, "unit_flags"), number(row, "player_flags"))
}

fn health(shard: &Standalone, guid: u64) -> u32 {
    number(
        &shard.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {guid}"
        ))[0],
        "health",
    )
}

fn advance(shard: &Standalone, ticks: u32) {
    let mut left = ticks;
    while left > 0 {
        let batch = left.min(4000);
        shard.assert_call("debug_advance_roll_batch", &[&batch.to_string()]);
        left -= batch;
    }
}
