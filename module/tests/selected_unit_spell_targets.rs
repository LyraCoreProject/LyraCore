mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::constants::type_mask;
use lyracore_shared::guid;
use lyracore_shared::spell::taxonomy::{
    E_HEAL, T_CHAIN_ALLY, T_TARGET_GROUP_CLASS, T_TARGET_SUBGROUP,
};
use support::{number, Standalone};

const CHARACTER: u64 = 1;
const MAX_HEALTH: u32 = 1_000;
const SUBGROUP_HEAL: u32 = 5_131_301;
const GROUP_CLASS_HEAL: u32 = 5_131_302;
const CHAIN_HEAL: u32 = 5_131_303;
const RANGED_CHAIN_HEAL: u32 = 5_131_304;
const CHAIN_HEAL_RANGE_YD: u32 = 40;
const GROUP: u64 = 13_130;
const RADIUS_YD: f32 = 5.0;
const CHARACTER_FACTION: u32 = 13_131;
const WARRIOR: u8 = 1;
const MAGE: u8 = 8;
const RAID: u8 = 1;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_subgroup_spell_reaches_the_selected_characters_subgroup_and_pets_near_it() {
    let shard = stage("selected-subgroup");
    let target = character(&shard, "Target", WARRIOR, 20.0);
    let mate = character(&shard, "Mate", WARRIOR, 23.0);
    let pet = pet(&shard, mate, 24.0);
    let other_subgroup = character(&shard, "Elsewhere", WARRIOR, 21.0);
    let outsider = character(&shard, "Outsider", WARRIOR, 21.0);
    group(&shard, &[(target, 0), (mate, 0), (other_subgroup, 1)]);

    let healed = cast_and_watch(
        &shard,
        SUBGROUP_HEAL,
        target,
        [target, mate, pet, other_subgroup, outsider],
    );

    assert_eq!(
        healed,
        [true, true, true, false, false],
        "vmangos target 37: the selected Character's Subgroup and pets near it, though 20 yd \
         from the caster; another Subgroup and a unit outside the Group are spared"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_group_class_spell_reaches_the_selected_characters_class_in_every_subgroup() {
    let shard = stage("selected-group-class");
    let target = character(&shard, "Target", WARRIOR, 20.0);
    let warrior = character(&shard, "Warrior", WARRIOR, 22.0);
    let mage = character(&shard, "Mage", MAGE, 21.0);
    let outsider = character(&shard, "Outsider", WARRIOR, 21.0);
    group(&shard, &[(target, 0), (warrior, 1), (mage, 0)]);

    let healed = cast_and_watch(
        &shard,
        GROUP_CLASS_HEAL,
        target,
        [target, warrior, mage, outsider],
    );

    assert_eq!(
        healed,
        [true, true, false, false],
        "vmangos target 61: the selected Character's class in every Subgroup; another class \
         and a unit outside the Group are spared"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_friendly_chain_jumps_to_the_targets_group_before_a_more_injured_unit() {
    let shard = stage("friendly-chain");
    let target = character(&shard, "Target", WARRIOR, 5.0);
    let mate = character(&shard, "Mate", WARRIOR, 14.0);
    group(&shard, &[(target, 0), (mate, 0)]);
    wound(&shard, mate, 100);
    let injured_most = spawn_wolf(&shard, 1.0);
    wound(&shard, injured_most, 500);
    let beyond_mate = spawn_wolf(&shard, 22.0);
    wound(&shard, beyond_mate, 200);

    let healed = cast_and_watch(
        &shard,
        CHAIN_HEAL,
        target,
        [target, mate, beyond_mate, injured_most],
    );

    assert_eq!(
        healed,
        [true, true, true, false],
        "vmangos ChainHealingOrder: the first jump goes to the target's Group mate, not to the \
         closer, more injured unit; the last jump goes from the mate to the unit 8 yd beyond it, \
         which the injured unit 13 yd from the mate could not have reached"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_friendly_chain_without_a_radius_jumps_within_the_spells_range_of_the_caster() {
    let shard = stage("friendly-chain-range");
    let target = character(&shard, "Target", WARRIOR, 35.0);
    let beyond_target = spawn_wolf(&shard, 38.0);
    wound(&shard, beyond_target, 200);

    let healed = cast_and_watch(&shard, RANGED_CHAIN_HEAL, target, [target, beyond_target]);

    assert_eq!(
        healed,
        [true, true],
        "vmangos SetTargetMap: without a radius the jumps come from the spell's 40 yd range plus \
         10 yd per jump of the caster"
    );
}

fn stage(name: &str) -> Standalone {
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
    shard.insert_row(
        "game_faction_template",
        &[
            ("id", &CHARACTER_FACTION),
            ("faction", &CHARACTER_FACTION),
            ("faction_group", &1),
            ("friend_group", &1),
            ("enemy_group", &2),
        ],
    );
    set_faction(&shard, CHARACTER, CHARACTER_FACTION);
    insert_heal(&shard, SUBGROUP_HEAL, T_TARGET_SUBGROUP, 0, RADIUS_YD, 0);
    insert_heal(
        &shard,
        GROUP_CLASS_HEAL,
        T_TARGET_GROUP_CLASS,
        0,
        RADIUS_YD,
        0,
    );
    insert_heal(&shard, CHAIN_HEAL, T_CHAIN_ALLY, 3, RADIUS_YD, 0);
    // Chain Heal 1064: no radius, a 40 yd range.
    insert_heal(
        &shard,
        RANGED_CHAIN_HEAL,
        T_CHAIN_ALLY,
        2,
        0.0,
        CHAIN_HEAL_RANGE_YD,
    );
    shard
}

/// A wounded Character of the fixture Account `offset` yards along +x from the caster.
fn character(shard: &Standalone, name: &str, class: u8, offset: f32) -> u64 {
    let quoted = format!("\"{name}\"");
    let class = class.to_string();
    shard.assert_call(
        "create_character",
        &["1", &quoted, "1", &class, "0", "0", "0", "0", "0", "0"],
    );
    let guid: u64 = number(
        &shard.query_rows(&format!(
            "SELECT guid FROM game_character WHERE name = '{name}'"
        ))[0],
        "guid",
    );
    let (map_id, x, y, z) = caster_position(shard);
    shard.assert_sql(&format!(
        "UPDATE game_character SET map_id = {map_id}, x = {}, y = {y}, z = {z} WHERE guid = {guid}",
        x + offset
    ));
    shard.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    set_faction(shard, guid, CHARACTER_FACTION);
    wound(shard, guid, MAX_HEALTH / 2);
    guid
}

/// A wounded pet of `owner` under the owner's low guid, as `pet_guid_for` names a player's pet.
fn pet(shard: &Standalone, owner: u64, offset: f32) -> u64 {
    let pet = guid::compose(guid::UNIT, guid::low(owner));
    let (map_id, x, y, z) = caster_position(shard);
    let no_identity = format!("0x{}", "00".repeat(32));
    shard.insert_row(
        "game_world_entity",
        &[
            ("guid", &pet),
            ("owner_identity", &no_identity),
            ("map_id", &map_id),
            ("x", &(x + offset)),
            ("y", &y),
            ("z", &z),
            ("type_mask", &type_mask::CREATURE),
            ("entry", &TEST_WOLF_ENTRY),
            ("scale_x", &1.0f32),
            ("level", &60),
            ("faction_template", &CHARACTER_FACTION),
            ("owner_guid", &owner),
        ],
    );
    wound(shard, pet, MAX_HEALTH / 2);
    pet
}

fn group(shard: &Standalone, members: &[(u64, u8)]) {
    shard.insert_row(
        "game_group",
        &[
            ("group_id", &GROUP),
            ("leader_guid", &members[0].0),
            ("group_type", &RAID),
        ],
    );
    let no_identity = format!("0x{}", "00".repeat(32));
    for (id, &(character_guid, subgroup)) in (GROUP * 1_000..).zip(members) {
        shard.insert_row(
            "game_group_member",
            &[
                ("id", &id),
                ("group_id", &GROUP),
                ("character_guid", &character_guid),
                ("owner_identity", &no_identity),
                ("raid_slot", &subgroup),
            ],
        );
    }
}

/// A friendly wolf `offset` yards along +x from the caster.
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
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60 WHERE guid = {wolf}"
    ));
    set_faction(shard, wolf, CHARACTER_FACTION);
    wolf
}

fn caster_position(shard: &Standalone) -> (u32, f32, f32, f32) {
    let row = shard
        .query_rows(&format!(
            "SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    (
        number(&row, "map_id"),
        number(&row, "x"),
        number(&row, "y"),
        number(&row, "z"),
    )
}

fn set_faction(shard: &Standalone, guid: u64, faction: u32) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = {faction} WHERE guid = {guid}"
    ));
}

fn wound(shard: &Standalone, guid: u64, missing: u32) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {}, max_health = {MAX_HEALTH} WHERE guid = {guid}",
        MAX_HEALTH - missing
    ));
}

// A zero range skips the range check and the flight.
fn insert_heal(
    shard: &Standalone,
    spell_id: u32,
    target: u8,
    chain_targets: u8,
    radius_yd: f32,
    range_yd: u32,
) {
    let name = format!("Selected unit heal fixture {spell_id}");
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &name),
            ("school_mask", &0x02),
            ("range_yd", &range_yd),
            ("is_negative", &false),
            ("gcd_ms", &0),
            ("cooldown_ms", &0),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("kind", &E_HEAL),
            ("target", &target),
            ("base_points", &10),
            ("radius_yd", &radius_yd),
            ("chain_targets", &chain_targets),
        ],
    );
}

fn cast_at(shard: &Standalone, spell_id: u32, target: u64) {
    shard.assert_call(
        "debug_force_cast_at",
        &[
            &CHARACTER.to_string(),
            &spell_id.to_string(),
            &target.to_string(),
        ],
    );
}

fn health(shard: &Standalone, guid: u64) -> u32 {
    number(
        &shard
            .query_rows(&format!(
                "SELECT health FROM game_world_entity WHERE guid = {guid}"
            ))
            .remove(0),
        "health",
    )
}

/// Casts `spell_id` at `target` and reports which of `watched` it healed.
fn cast_and_watch<const N: usize>(
    shard: &Standalone,
    spell_id: u32,
    target: u64,
    watched: [u64; N],
) -> [bool; N] {
    let before = watched.map(|guid| health(shard, guid));
    cast_at(shard, spell_id, target);
    let mut healed = [false; N];
    for (i, guid) in watched.into_iter().enumerate() {
        healed[i] = health(shard, guid) > before[i];
    }
    healed
}
