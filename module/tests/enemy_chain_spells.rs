mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::item::weapon_subclass::THROWN;
use lyracore_shared::spell::taxonomy::{
    CAST_FLAG_ALWAYS_HIT, CAST_FLAG_CANNOT_CRIT, CAST_FLAG_IGNORE_RESISTANCES, E_DAMAGE,
    E_NEXT_SWING, E_WEAPON_STRIKE, T_CHAIN_ENEMY, T_TARGET_ENEMY,
};
use serde_json::json;
use support::{number, Field, NativePacket, Standalone, WorldSessionObserver};

const CHARACTER: u64 = 1;
const WOLF_HEALTH: u32 = 100_000;
const CHAIN_LIGHTNING: u32 = 5_133_201;
const MULTI_SHOT: u32 = 5_133_202;
// Spell.dbc EffectChainTarget of Chain Lightning 421 and Multi-Shot 2643.
const CHAIN_TARGETS: u8 = 3;
const CHARACTER_FACTION: u32 = 13_321;
const WOLF_FACTION: u32 = 13_322;
// Spell.dbc Attributes of Multi-Shot 2643; 0x02 marks the ranged slot.
const MULTI_SHOT_ATTRIBUTES: u32 = 0x0001_0002;
const NATURE: u32 = 0x08;
const PHYSICAL: u32 = 0x01;
// vmangos SpellDamageClass.
const DAMAGE_CLASS_MAGIC: u8 = 1;
const DAMAGE_CLASS_MELEE: u8 = 2;
const DAMAGE_CLASS_RANGED: u8 = 3;
const RANGED_SLOT: u8 = 17;
const INVTYPE_THROWN: u8 = 25;
const THROWN_STACK: u32 = 20;
// 20 ticks of 50 ms pass the longest flight in these tests.
const FLIGHT_TICKS: &str = "20";
// Four wolves 4 yd apart in a line ahead of the Character; the cast aims at the nearest.
const WOLF_OFFSETS: [f32; 4] = [4.0, 8.0, 12.0, 16.0];
const SMSG_SPELL_GO: u16 = 0x0132;
// vmangos SpellMissInfo SPELL_MISS_EVADE.
const MISS_EVADE: u8 = 6;
const CLEAVE: u32 = 845;
// Spell.dbc Cleave 845: EffectChainTarget 2, no effect radius, the 5 yd melee range.
const CLEAVE_CHAIN_TARGETS: u8 = 2;
// The victim in melee reach, a wolf 3 yd past it and one 6.5 yd past it, beyond Cleave's 5 yd.
const CLEAVE_WOLF_OFFSETS: [f32; 3] = [1.5, 4.5, 8.0];
// Two 50 ms batches pass a 100 ms melee tick, which swings the 50 ms fist.
const SWING_TICKS: &str = "2";
// A dodge or miss can spare a wolf; 20 Cleaves leave it a negligible chance.
const CLEAVE_ATTEMPTS: usize = 20;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn chain_lightning_jumps_from_its_target_to_two_more_enemies() {
    let (shard, wolves) = stage("enemy-chain-lightning");

    cast_and_land(&shard, CHAIN_LIGHTNING, wolves[0]);

    assert_eq!(
        hit(&shard, wolves),
        [true, true, true, false],
        "vmangos SetTargetMap target 6: the target, then the nearest enemy within 10 yd of the \
         last, until the 3 targets of Chain Lightning are hit"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn multi_shot_strikes_its_target_and_two_more_enemies_for_one_thrown_weapon() {
    let (shard, wolves) = stage("enemy-chain-multi-shot");
    equip_thrown(&shard);

    cast_and_land(&shard, MULTI_SHOT, wolves[0]);

    assert_eq!(
        hit(&shard, wolves),
        [true, true, true, false],
        "vmangos SetTargetMap target 6 gives Multi-Shot 3 targets"
    );
    assert_eq!(
        thrown_stack(&shard),
        THROWN_STACK - 1,
        "vmangos Spell::TakeAmmo spends one per cast, not one per target"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_chain_lightning_go_lists_its_hits_in_jump_order_and_the_unit_that_evaded() {
    let (shard, wolves) = stage("enemy-chain-go");
    shard.insert_row(
        "game_creature_ai_returning_home",
        &[("creature_guid", &wolves[1])],
    );
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([CHARACTER]))
        .unwrap();

    wire.call(
        "debug_force_cast_at",
        json!([CHARACTER, CHAIN_LIGHTNING, wolves[0]]),
    )
    .unwrap();

    let mut session = WorldSessionObserver::new(wire.connection_id());
    let go = session
        .received(&wire.drain_packets())
        .into_iter()
        .find(|packet| packet.opcode == Some(SMSG_SPELL_GO))
        .expect("the caster's World Session receives the GO");
    assert_eq!(
        go_units(&go),
        (vec![wolves[0], wolves[2]], vec![(wolves[1], MISS_EVADE)]),
        "vmangos WriteSpellGoTargets: every unit of the target map in jump order, the evading \
         link as a miss; the fourth wolf is past the 3 targets"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_queued_cleave_strikes_its_victim_and_the_enemy_in_reach_but_not_one_beyond() {
    let shard = open_shard("queued-cleave-chain");
    let [victim, near, far] = CLEAVE_WOLF_OFFSETS.map(|offset| spawn_wolf(&shard, offset));
    insert_queued_cleave(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true, base_attack_time_ms = 50 \
         WHERE guid = {CHARACTER}"
    ));
    let mut wire = shard.connect_reducers(&["SELECT * FROM game_native_packet"]);
    wire.call("native_world_open", json!([42])).unwrap();
    wire.call("debug_native_enter_world", json!([CHARACTER]))
        .unwrap();
    let mut session = WorldSessionObserver::new(wire.connection_id());
    shard.assert_call(
        "debug_engage",
        &[&CHARACTER.to_string(), &victim.to_string()],
    );

    let mut gos = Vec::new();
    for _ in 0..CLEAVE_ATTEMPTS {
        if hit(&shard, [victim, near]) == [true, true] {
            break;
        }
        shard.assert_call(
            "debug_force_cast_at",
            &[
                &CHARACTER.to_string(),
                &CLEAVE.to_string(),
                &victim.to_string(),
            ],
        );
        shard.assert_call("debug_advance_roll_batch", &[SWING_TICKS]);
        gos.extend(
            session
                .received(&wire.drain_packets())
                .into_iter()
                .filter(|packet| {
                    packet.opcode == Some(SMSG_SPELL_GO) && go_spell(packet) == CLEAVE
                }),
        );
    }

    assert_eq!(
        hit(&shard, [victim, near, far]),
        [true, true, false],
        "vmangos casts the queued Cleave at the victim; its target 6 chain gathers melee units \
         within 5 yd of it"
    );
    let go = gos.first().expect("the swing sends the Cleave's GO");
    let (hits, misses) = go_units(go);
    let mut units: Vec<u64> = hits
        .into_iter()
        .chain(misses.into_iter().map(|(unit, _)| unit))
        .collect();
    units.sort_unstable();
    let mut expected = vec![victim, near];
    expected.sort_unstable();
    assert_eq!(
        units, expected,
        "vmangos WriteSpellGoTargets lists the victim and the chained wolf, hit or missed"
    );
}

fn open_shard(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.clear_schedule("game_creature_move_schedule");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60, orientation = 0 WHERE guid = {CHARACTER}"
    ));
    factions(&shard);
    set_faction(&shard, CHARACTER, CHARACTER_FACTION);
    shard
}

fn stage(name: &str) -> (Standalone, [u64; 4]) {
    let shard = open_shard(name);
    shard.clear_schedule("game_melee_schedule");
    let wolves = WOLF_OFFSETS.map(|offset| spawn_wolf(&shard, offset));
    // Chain Lightning 421: nature damage at 30 yd with no effect radius.
    insert_chain(
        &shard,
        CHAIN_LIGHTNING,
        E_DAMAGE,
        &[
            ("school_mask", &NATURE),
            ("damage_class", &DAMAGE_CLASS_MAGIC),
            ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
        ],
        &[("base_points", &10)],
    );
    // Multi-Shot 2643: a ranged weapon strike at 35 yd with no effect radius.
    insert_chain(
        &shard,
        MULTI_SHOT,
        E_WEAPON_STRIKE,
        &[
            ("school_mask", &PHYSICAL),
            ("attributes", &MULTI_SHOT_ATTRIBUTES),
            ("damage_class", &DAMAGE_CLASS_RANGED),
            (
                "cast_flags",
                &(CAST_FLAG_ALWAYS_HIT | CAST_FLAG_CANNOT_CRIT),
            ),
        ],
        &[("base_points", &0), ("p_0", &100), ("p_0_kind", &255)],
    );
    (shard, wolves)
}

// The bare Standalone has no faction rows; the wolves are the Character's enemies.
fn factions(shard: &Standalone) {
    shard.insert_rows(
        "game_faction_template",
        &[
            &[
                ("id", &CHARACTER_FACTION),
                ("faction", &CHARACTER_FACTION),
                ("faction_group", &1),
                ("friend_group", &1),
                ("enemy_group", &2),
            ],
            &[
                ("id", &WOLF_FACTION),
                ("faction", &WOLF_FACTION),
                ("faction_group", &2),
                ("friend_group", &2),
                ("enemy_group", &1),
            ],
        ],
    );
}

fn set_faction(shard: &Standalone, guid: u64, faction: u32) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = {faction} WHERE guid = {guid}"
    ));
}

/// A hostile wolf `offset` yards ahead of the Character, facing away from it.
fn spawn_wolf(shard: &Standalone, offset: f32) -> u64 {
    let wolves = |shard: &Standalone| -> Vec<u64> {
        shard
            .query_rows(&format!(
                "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
            ))
            .iter()
            .map(|row| number(row, "guid"))
            .collect()
    };
    let before = wolves(shard);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[
            &CHARACTER.to_string(),
            &TEST_WOLF_ENTRY.to_string(),
            &offset.to_string(),
        ],
    );
    let wolf = wolves(shard)
        .into_iter()
        .find(|guid| !before.contains(guid))
        .expect("the spawned wolf must exist");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60, armor = 0, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    set_faction(shard, wolf, WOLF_FACTION);
    wolf
}

/// An imported-shape target 6 chain: `kind` at the selected enemy and 2 more.
fn insert_chain(
    shard: &Standalone,
    spell_id: u32,
    kind: u8,
    spell_fields: &[Field],
    effect_fields: &[Field],
) {
    let name = format!("Enemy chain fixture {spell_id}");
    let mut spell_row: Vec<Field> = vec![
        ("spell_id", &spell_id),
        ("name", &name),
        ("range_yd", &30),
        ("is_negative", &true),
        ("gcd_ms", &0),
        ("cooldown_ms", &0),
    ];
    spell_row.extend_from_slice(spell_fields);
    shard.insert_row("game_spell", &spell_row);
    let id = packed_spell_effect_id(spell_id, 0);
    let mut effect_row: Vec<Field> = vec![
        ("id", &id),
        ("spell_id", &spell_id),
        ("kind", &kind),
        ("target", &T_CHAIN_ENEMY),
        ("chain_targets", &CHAIN_TARGETS),
    ];
    effect_row.extend_from_slice(effect_fields);
    shard.insert_row("game_spell_effect", &effect_row);
}

// Cleave 845 as the importer writes it: a queued strike that keeps its chain count.
fn insert_queued_cleave(shard: &Standalone) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &CLEAVE),
            ("name", &"Cleave"),
            ("range_yd", &5),
            ("is_negative", &true),
            ("school_mask", &PHYSICAL),
            ("damage_class", &DAMAGE_CLASS_MELEE),
            ("gcd_ms", &0),
            ("cooldown_ms", &0),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(CLEAVE, 0)),
            ("spell_id", &CLEAVE),
            ("kind", &E_NEXT_SWING),
            ("base_points", &5),
            ("target", &T_TARGET_ENEMY),
            ("chain_targets", &CLEAVE_CHAIN_TARGETS),
        ],
    );
}

fn equip_thrown(shard: &Standalone) {
    shard.assert_call(
        "debug_equip_weapon",
        &[&CHARACTER.to_string(), &FIXTURE_BLADE.to_string()],
    );
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET subclass = {THROWN}, inventory_type = {INVTYPE_THROWN}, \
         max_stack = 200, damage_min = 1000.0, damage_max = 1000.0 WHERE entry = {FIXTURE_BLADE}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_item_instance SET slot = {RANGED_SLOT}, stack_count = {THROWN_STACK} \
         WHERE owner_guid = {CHARACTER} AND entry = {FIXTURE_BLADE}"
    ));
}

fn thrown_stack(shard: &Standalone) -> u32 {
    number(
        &shard
            .query_rows(&format!(
                "SELECT stack_count FROM game_item_instance WHERE owner_guid = {CHARACTER} \
                 AND entry = {FIXTURE_BLADE}"
            ))
            .remove(0),
        "stack_count",
    )
}

/// Casts `spell_id` at `target` and lets every shot or bolt land.
fn cast_and_land(shard: &Standalone, spell_id: u32, target: u64) {
    shard.assert_call(
        "debug_force_cast_at",
        &[
            &CHARACTER.to_string(),
            &spell_id.to_string(),
            &target.to_string(),
        ],
    );
    shard.assert_call("debug_advance_roll_batch", &[FLIGHT_TICKS]);
}

/// The spell id of an SMSG_SPELL_GO body.
fn go_spell(go: &NativePacket) -> u32 {
    let body = &go.body;
    let mut at = 0;
    for _ in 0..2 {
        at += 1 + body[at].count_ones() as usize;
    }
    u32::from_le_bytes(body[at..at + 4].try_into().unwrap())
}

/// The hit guids and the (guid, miss) pairs of an SMSG_SPELL_GO body.
fn go_units(go: &NativePacket) -> (Vec<u64>, Vec<(u64, u8)>) {
    let body = &go.body;
    let mut at = 0;
    // Two packed guids, then spell u32 and flags u16.
    for _ in 0..2 {
        at += 1 + body[at].count_ones() as usize;
    }
    at += 6;
    let guid = |at: &mut usize| {
        let value = u64::from_le_bytes(body[*at..*at + 8].try_into().unwrap());
        *at += 8;
        value
    };
    let hit_count = usize::from(body[at]);
    at += 1;
    let hits = (0..hit_count).map(|_| guid(&mut at)).collect();
    let miss_count = usize::from(body[at]);
    at += 1;
    let mut misses = Vec::new();
    for _ in 0..miss_count {
        let unit = guid(&mut at);
        misses.push((unit, body[at]));
        at += 1;
    }
    (hits, misses)
}

fn hit<const N: usize>(shard: &Standalone, wolves: [u64; N]) -> [bool; N] {
    wolves.map(|wolf| {
        number::<u32>(
            &shard
                .query_rows(&format!(
                    "SELECT health FROM game_world_entity WHERE guid = {wolf}"
                ))
                .remove(0),
            "health",
        ) < WOLF_HEALTH
    })
}
