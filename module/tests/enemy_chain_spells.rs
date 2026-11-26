mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::item::weapon_subclass::THROWN;
use lyracore_shared::spell::taxonomy::{
    CAST_FLAG_ALWAYS_HIT, CAST_FLAG_CANNOT_CRIT, CAST_FLAG_IGNORE_RESISTANCES, E_DAMAGE,
    E_WEAPON_STRIKE, T_CHAIN_ENEMY,
};
use support::{number, Field, Standalone};

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
const DAMAGE_CLASS_RANGED: u8 = 3;
const RANGED_SLOT: u8 = 17;
const INVTYPE_THROWN: u8 = 25;
const THROWN_STACK: u32 = 20;
// 20 ticks of 50 ms pass the longest flight in these tests.
const FLIGHT_TICKS: &str = "20";
// Four wolves 4 yd apart in a line ahead of the Character; the cast aims at the nearest.
const WOLF_OFFSETS: [f32; 4] = [4.0, 8.0, 12.0, 16.0];

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

fn stage(name: &str) -> (Standalone, [u64; 4]) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.clear_schedule("game_creature_move_schedule");
    shard.clear_schedule("game_melee_schedule");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60, orientation = 0 WHERE guid = {CHARACTER}"
    ));
    factions(&shard);
    set_faction(&shard, CHARACTER, CHARACTER_FACTION);
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
