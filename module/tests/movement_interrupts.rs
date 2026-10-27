mod support;

use support::{actor, number, Standalone};

const CHARACTER: &str = "1";
const START_FORWARD: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_START_FORWARD;
const STOP: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_STOP;
const SET_FACING: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_SET_FACING;
const HEARTBEAT: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_HEARTBEAT;
const FALL_LAND: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_FALL_LAND;
const JUMP: u32 = lyracore_shared::opcodes::movement::MSG_MOVE_JUMP;
const JUMPING: &str = "[0,32,0,0]";
const FORWARD: &str = "[1,0,0,0]";
const STILL: &str = "[0,0,0,0]";

const ROOT_A: u32 = 5_090_901;
const ROOT_B: u32 = 5_090_902;
const SHADOWMELD: u32 = 5_090_903;
const STEALTH: u32 = 5_090_904;

// aura_interrupt bit 1 is break-on-move, mapped from the vanilla movement interrupt flag.
fn fixture(name: &str) -> (Standalone, [f32; 3]) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[CHARACTER]);
    let a_control = lyracore_shared::spell::taxonomy::A_CONTROL;
    let a_stealth = lyracore_shared::spell::taxonomy::A_STEALTH;
    let m_root = lyracore_shared::spell::taxonomy::M_ROOT;
    for (spell_id, kind, p0, aura_interrupt) in [
        (ROOT_A, a_control, m_root, 0),
        (ROOT_B, a_control, m_root, 0),
        (SHADOWMELD, a_stealth, 0, 2),
        (STEALTH, a_stealth, 0, 0),
    ] {
        shard.assert_sql(&format!(
            "INSERT INTO game_spell (spell_id, name, power_type, cost, cast_time_ms, gcd_ms, \
             cooldown_ms, range_yd, duration_ms, school_mask, dispel_type, mechanic, max_stacks, \
             aura_interrupt, attributes, spell_level, max_level, is_negative, cast_flags, stances, \
             family_name, family_flags, proc_flags, proc_chance, proc_charges, max_duration_ms) \
             VALUES ({spell_id}, 'Fixture {spell_id}', 0, 0, 0, 0, 0, 0, 600000, 1, 0, 0, 0, \
             {aura_interrupt}, 0, 0, 0, false, 0, 0, 0, 0, 0, 0, 0, 0)"
        ));
        shard.assert_sql(&format!(
            "INSERT INTO game_spell_effect (id, spell_id, effect_index, kind, base_points, \
             die_sides, per_level, period_ms, target, radius_yd, chain_targets, trigger_spell, \
             effect_mechanic, p0, p0_kind, p1, script_id, enters_combat, points_per_combo) \
             VALUES ({}, {spell_id}, 0, {kind}, 0, 0, 0.0, 0, 0, 0.0, 0, 0, 0, {p0}, 3, 0, 0, \
             false, 0.0)",
            u64::from(spell_id) << 2
        ));
    }
    let rows = shard.query_rows("SELECT x, y, z FROM game_world_entity WHERE guid = 1");
    let [x, y, z] = ["x", "y", "z"].map(|axis| number(&rows[0], axis));
    (shard, [x, y, z])
}

fn cast(shard: &Standalone, spell_id: u32) {
    shard.assert_call("debug_force_cast", &[CHARACTER, &spell_id.to_string()]);
}

fn send_move(shard: &Standalone, opcode: u32, flags: &str, at: [f32; 3], o: f32, time_ms: u32) {
    let [x, y, z] = at;
    let args = [
        actor(CHARACTER),
        opcode.to_string(),
        flags.to_string(),
        x.to_string(),
        y.to_string(),
        z.to_string(),
        o.to_string(),
        time_ms.to_string(),
    ];
    shard.assert_call(
        "gw_movement_update",
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
    );
}

fn position(shard: &Standalone) -> (f32, f32) {
    let rows = shard.query_rows("SELECT x, orientation FROM game_world_entity WHERE guid = 1");
    (number(&rows[0], "x"), number(&rows[0], "orientation"))
}

fn auras(shard: &Standalone, spell_id: u32) -> usize {
    shard
        .query_rows(&format!(
            "SELECT id FROM game_aura WHERE target_guid = 1 AND spell_id = {spell_id}"
        ))
        .len()
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_rooted_character_turns_but_moves_only_after_the_last_root_ends() {
    let (shard, [x, y, z]) = fixture("movement-root");
    cast(&shard, ROOT_A);
    cast(&shard, ROOT_B);
    let held = position(&shard);

    send_move(&shard, START_FORWARD, FORWARD, [x + 3.0, y, z], 0.5, 10);
    assert_eq!(
        position(&shard),
        held,
        "a steering heartbeat must not translate"
    );

    send_move(&shard, STOP, STILL, [x + 1.0, y, z], 1.5, 20);
    assert_eq!(position(&shard), (x + 1.0, 1.5));
    send_move(&shard, SET_FACING, STILL, [x + 1.0, y, z], 2.5, 30);
    assert_eq!(
        position(&shard),
        (x + 1.0, 2.5),
        "a rooted Character still turns"
    );

    send_move(&shard, FALL_LAND, FORWARD, [x + 2.0, y, z], 2.5, 35);
    assert_eq!(position(&shard).0, x + 2.0, "a landing is never dropped");

    shard.assert_sql(&format!("DELETE FROM game_aura WHERE spell_id = {ROOT_A}"));
    send_move(&shard, START_FORWARD, FORWARD, [x + 4.0, y, z], 2.5, 40);
    assert_eq!(position(&shard).0, x + 2.0, "the second root still holds");

    shard.assert_sql(&format!("DELETE FROM game_aura WHERE spell_id = {ROOT_B}"));
    send_move(&shard, START_FORWARD, FORWARD, [x + 4.0, y, z], 2.5, 50);
    assert_eq!(position(&shard).0, x + 4.0);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn moving_ends_shadowmeld_but_keeps_stealth() {
    let (shard, [x, y, z]) = fixture("movement-shadowmeld");
    cast(&shard, SHADOWMELD);
    send_move(&shard, SET_FACING, STILL, [x, y, z], 1.0, 10);
    assert_eq!(
        auras(&shard, SHADOWMELD),
        1,
        "turning in place keeps Shadowmeld"
    );
    send_move(&shard, HEARTBEAT, STILL, [x, y, z], 1.0, 15);
    // The stored row can lag the client; a stop that lands after the cast is not a move.
    send_move(&shard, STOP, STILL, [x + 3.0, y, z], 1.0, 18);
    assert_eq!(
        auras(&shard, SHADOWMELD),
        1,
        "standing still keeps Shadowmeld"
    );
    send_move(&shard, START_FORWARD, FORWARD, [x + 3.0, y, z], 1.0, 20);
    assert_eq!(auras(&shard, SHADOWMELD), 0);

    cast(&shard, SHADOWMELD);
    send_move(&shard, JUMP, JUMPING, [x + 3.0, y, z], 1.0, 25);
    assert_eq!(auras(&shard, SHADOWMELD), 0, "a jump in place is movement");

    cast(&shard, STEALTH);
    send_move(&shard, START_FORWARD, FORWARD, [x + 6.0, y, z], 1.0, 30);
    assert_eq!(auras(&shard, STEALTH), 1, "Rogue Stealth allows movement");
}
