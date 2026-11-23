mod support;

use std::io::Cursor;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::{player_flags, unit_flags};
use lyracore_shared::spell::taxonomy::{
    A_ABSORB, E_DAMAGE, E_DUEL, E_HEAL, P_GAMEOBJECT_ENTRY, P_SCHOOL_MASK, T_AREA_ENEMY, T_SELF,
    T_TARGET_ALLY, T_TARGET_ENEMY,
};
use lyracore_test_support::ReducerConnection;
use serde_json::json;
use support::{number, poll_until, Standalone, POLL_TIMEOUT};
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
const ABSORB_SHIELD: u32 = 5_091_408;
const RAGE_BURST: u32 = 5_091_409;
const HUMAN: &str = "1";
const ORC: &str = "2";
const CMSG_TOGGLE_PVP: u32 = 0x253;
const CMSG_CAST_SPELL_OPCODE: u32 = 0x12e;
const CMSG_ATTACKSWING: u32 = 0x141;
// 1.12.1 SpellCastResult values from vmangos SpellDefines.h.
const NO_DUELING: u8 = 0x47;
const TARGET_DUELING: u8 = 0x68;
// `state::REQUESTED` and `state::ACTIVE` in `module/src/duel.rs`.
const REQUESTED: u8 = 0;
const ACTIVE: u8 = 2;
// vmangos Player::UpdatePvP holds the flag 300 s, 6000 ticks of the 50 ms virtual clock.
const PVP_FLAG_TICKS: u32 = 6000;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_unflagged_enemy_character_is_outside_an_area_spell_until_it_flags() {
    let (shard, account) = realm("pvp-area-spell");
    let mage = character(&shard, &account, "Mage", HUMAN);
    let rival = character(&shard, &account, "Rival", HUMAN);
    for guid in [mage, rival] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    }
    hostile_factions(&shard, mage, rival);
    area_spell(&shard, ARCANE_BURST, 0);
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
    shard.assert_call(
        "debug_force_cast",
        &[&mage.to_string(), &ARCANE_BURST.to_string()],
    );
    assert!(
        timer(&shard, rival).is_some(),
        "harm from a flagged Character refreshes the victim's flag"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_spell_with_a_cost_on_a_flagged_enemy_flags_its_caster() {
    let (shard, account) = realm("pvp-cost-spell");
    let warrior = character(&shard, &account, "Warrior", HUMAN);
    let rival = character(&shard, &account, "Rival", HUMAN);
    for guid in [warrior, rival] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    }
    hostile_factions(&shard, warrior, rival);
    area_spell(&shard, RAGE_BURST, 100);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_flags = {} WHERE guid = {rival}",
        flags(&shard, rival).0 | unit_flags::PVP
    ));
    shard.assert_call("debug_set_power", &[&warrior.to_string(), "1000"]);

    shard.assert_call(
        "debug_force_cast",
        &[&warrior.to_string(), &RAGE_BURST.to_string()],
    );
    assert!(power(&shard, warrior) <= 900, "the cast spends its rage");
    assert_ne!(
        flags(&shard, warrior).0 & unit_flags::PVP,
        0,
        "vmangos DoAllEffectOnTarget flags the caster after it takes the cost"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_swing_the_flagged_target_absorbs_still_flags_the_attacker() {
    let (shard, account) = realm("pvp-absorbed-swing");
    shard.clear_schedule("game_creature_move_schedule");
    let warrior = character(&shard, &account, "Warrior", HUMAN);
    let rival = character(&shard, &account, "Shielded", HUMAN);
    for guid in [warrior, rival] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    }
    hostile_factions(&shard, warrior, rival);
    absorb_spell(&shard);
    shard.assert_call(
        "debug_force_cast",
        &[&rival.to_string(), &ABSORB_SHIELD.to_string()],
    );
    let at = &shard.query_rows(&format!(
        "SELECT x FROM game_world_entity WHERE guid = {warrior}"
    ))[0];
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, x = {}, unit_flags = {} \
         WHERE guid = {rival}",
        number::<f32>(at, "x") + 1.0,
        flags(&shard, rival).0 | unit_flags::PVP
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET orientation = 0.0 WHERE guid = {warrior}"
    ));
    let rival_health = health(&shard, rival);

    shard.assert_call("debug_engage", &[&warrior.to_string(), &rival.to_string()]);
    assert!(
        poll_until(POLL_TIMEOUT, || {
            shard.assert_sql(&format!(
                "UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = {warrior}"
            ));
            flags(&shard, warrior).0 & unit_flags::PVP != 0
        }),
        "vmangos UpdateMeleeAttackingState flags the attacker at the swing"
    );
    assert_eq!(
        health(&shard, rival),
        rival_health,
        "no swing got through the absorb"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn neither_a_character_nor_its_pet_may_attack_an_unflagged_enemy_character() {
    let (shard, account) = realm("pvp-unflagged-melee");
    shard.clear_schedule("game_creature_move_schedule");
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    let hunter = character(&shard, &account, "Hunter", HUMAN);
    let rival = character(&shard, &account, "Rival", HUMAN);
    let mut connection = enter(&shard, hunter);
    shard.assert_call("debug_spawn_player_entity", &[&rival.to_string()]);
    hostile_factions(&shard, hunter, rival);

    let _ = connection.call(
        "native_world_receive",
        json!([CMSG_ATTACKSWING, rival.to_le_bytes()]),
    );
    assert!(
        melee_target(&shard, hunter).is_none(),
        "vmangos IsValidAttackTarget refuses CMSG_ATTACKSWING"
    );

    shard.assert_call(
        "debug_spawn_at_feet",
        &[&hunter.to_string(), &TEST_WOLF_ENTRY.to_string(), "5"],
    );
    let pet: u64 = number(
        &shard.query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))[0],
        "guid",
    );
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET owner_guid = {hunter}, \
         faction_template = {ALLIANCE_FACTION} WHERE guid = {pet}"
    ));
    let rival_health = health(&shard, rival);
    shard.assert_call("debug_engage", &[&pet.to_string(), &rival.to_string()]);
    assert!(
        poll_until(POLL_TIMEOUT, || melee_target(&shard, pet).is_none()),
        "the pet's melee at an unflagged enemy Character ends"
    );
    assert_eq!(health(&shard, rival), rival_health);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_enemy_capital_holds_the_flag_and_it_drops_300_s_after_leaving() {
    let (shard, account) = realm("pvp-enforced-zone");
    shard.clear_schedule("game_creature_move_schedule");
    let raider = character(&shard, &account, "Raider", ORC);
    shard.assert_call("debug_spawn_player_entity", &[&raider.to_string()]);
    stormwind(&shard, 0.0, 0.0, 312);
    let unflagged = flags(&shard, raider).0;
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    let check_zone_at = |x: &str, y: &str| {
        shard.assert_call("debug_check_rest_at", &[&raider.to_string(), "0", x, y]);
    };

    check_zone_at("0", "0");
    let flagged = flags(&shard, raider).0;
    assert_eq!(
        flagged,
        unflagged | unit_flags::PVP,
        "vmangos Player::UpdateZone"
    );
    let countdown = timer(&shard, raider);
    check_zone_at("0", "0");
    assert_eq!(
        timer(&shard, raider),
        countdown,
        "a held zone writes no timer"
    );
    advance(&shard, PVP_FLAG_TICKS);
    assert_eq!(flags(&shard, raider).0, flagged, "the zone holds the flag");

    check_zone_at("5000", "5000");
    advance(&shard, PVP_FLAG_TICKS - 1);
    assert_eq!(flags(&shard, raider).0, flagged);
    advance(&shard, 1);
    assert_eq!(flags(&shard, raider).0, unflagged);

    check_zone_at("0", "0");
    // A move with no zone check, such as a near teleport or a ghost's run, leaves the zone too.
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET x = 5000.0, y = 5000.0 WHERE guid = {raider}"
    ));
    advance(&shard, PVP_FLAG_TICKS);
    assert_eq!(
        flags(&shard, raider).0,
        flagged,
        "the countdown starts at the expiry"
    );
    advance(&shard, PVP_FLAG_TICKS);
    assert_eq!(
        flags(&shard, raider).0,
        unflagged,
        "a stale zone cannot hold the flag"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn toggling_pvp_flags_at_once_and_the_flag_drops_five_minutes_after_desire_clears() {
    let (shard, account) = realm("pvp-toggle");
    shard.clear_schedule("game_creature_move_schedule");
    let guid = character(&shard, &account, "Toggler", HUMAN);
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
    let challenger = character(&shard, &account, "Challenger", HUMAN);
    let rival = character(&shard, &account, "Rival", HUMAN);
    let third = character(&shard, &account, "Third", HUMAN);
    for guid in [rival, third] {
        shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    }
    duel_spell(&shard);
    let mut connection = enter(&shard, challenger);
    shard.assert_call(
        "debug_learn_spell",
        &[&challenger.to_string(), &DUEL_CHALLENGE.to_string()],
    );

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
        refused.contains(&cast_result(TARGET_DUELING)),
        "vmangos Spell::CheckCast refuses the own opponent before EffectDuel: {refused}"
    );

    shard.assert_sql("DELETE FROM game_duel");
    shard.insert_row(
        "game_character_contact",
        &[
            ("id", &IGNORE_CONTACT),
            ("owner_guid", &rival),
            ("owner_identity", &format!("0x{}", "00".repeat(32))),
            ("target_guid", &challenger),
            ("is_ignore", &true),
        ],
    );
    challenge(&mut connection, rival).expect("an ignored challenge drops without a cast result");
    assert_eq!(duels(&shard), 0);

    shard.assert_sql(&format!(
        "DELETE FROM game_character_contact WHERE id = {IGNORE_CONTACT}"
    ));
    let at = &shard.query_rows(&format!(
        "SELECT x, y FROM game_world_entity WHERE guid = {challenger}"
    ))[0];
    // Stormwind's AreaTable.dbc flags lack AREA_FLAG_DUEL (0x40).
    stormwind(&shard, number(at, "x"), number(at, "y"), 312);
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
    let duelist = character(&shard, &account, "Duelist", HUMAN);
    let opponent = character(&shard, &account, "Opponent", HUMAN);
    let healer = character(&shard, &account, "Healer", HUMAN);
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

fn character(shard: &Standalone, account: &str, name: &str, race: &str) -> u64 {
    shard.assert_call(
        "create_character",
        &[
            account,
            &format!("\"{name}\""),
            race,
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

/// Stormwind City (area 1519, an Alliance capital) under the terrain cell holding `x`, `y`.
fn stormwind(shard: &Standalone, x: f32, y: f32, flags: u32) {
    shard.insert_row(
        "game_area",
        &[
            ("id", &1519),
            ("area_bit", &1),
            ("flags", &flags),
            ("faction_group", &2),
            ("name", &"Stormwind City"),
        ],
    );
    let cell = |at: f32| lyracore_shared::terrain::cell_index(at).unwrap();
    let heights = vec!["0"; 145].join(":");
    shard.assert_call(
        "import_terrain_chunks",
        &[&format!("0,{},{},0,0,0,1519,{heights}", cell(x), cell(y))],
    );
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

/// An enemy-area damage spell; a nonzero cost is in rage, power type 1.
fn area_spell(shard: &Standalone, spell: u32, rage_cost: u32) {
    let power_type = u8::from(rage_cost > 0);
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell),
            ("name", &"PvP area burst fixture"),
            ("school_mask", &64),
            ("is_negative", &true),
            ("gcd_ms", &0),
            ("cooldown_ms", &0),
            ("spell_level", &1),
            ("power_type", &power_type),
            ("cost", &rage_cost),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell, 0)),
            ("spell_id", &spell),
            ("kind", &E_DAMAGE),
            ("base_points", &5),
            ("target", &T_AREA_ENEMY),
            ("radius_yd", &10.0),
        ],
    );
}

fn absorb_spell(shard: &Standalone) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &ABSORB_SHIELD),
            ("name", &"PvP absorb shield fixture"),
            ("duration_ms", &600_000),
            ("spell_level", &1),
            ("gcd_ms", &0),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(ABSORB_SHIELD, 0)),
            ("spell_id", &ABSORB_SHIELD),
            ("kind", &A_ABSORB),
            ("base_points", &100_000),
            ("target", &T_SELF),
            ("p_0", &1),
            ("p_0_kind", &P_SCHOOL_MASK),
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

fn timer(shard: &Standalone, guid: u64) -> Option<String> {
    shard
        .query_rows(&format!(
            "SELECT scheduled_id FROM game_pvp_flag_timer WHERE character_guid = {guid}"
        ))
        .first()
        .map(|row| row["scheduled_id"].clone())
}

fn melee_target(shard: &Standalone, attacker: u64) -> Option<String> {
    shard
        .query_rows(&format!(
            "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {attacker}"
        ))
        .first()
        .map(|row| row["target_guid"].clone())
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

fn power(shard: &Standalone, guid: u64) -> u32 {
    number(
        &shard.query_rows(&format!(
            "SELECT power FROM game_world_entity WHERE guid = {guid}"
        ))[0],
        "power",
    )
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
