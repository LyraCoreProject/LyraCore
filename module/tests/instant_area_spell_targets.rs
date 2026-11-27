mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    CAST_FLAG_IGNORE_RESISTANCES, E_DAMAGE, E_HEAL, T_AREA_ALLY, T_AREA_ENEMY, T_PARTY_AREA,
};
use support::{event_rows, fence_subscription, number, subscribe_with_fence, Field, Standalone};

const CHARACTER: u64 = 1;
const WOLF_HEALTH: u32 = 100_000;
const AREA_BURST: u32 = 5_129_001;
const CAPPED_BURST: u32 = 5_129_002;
const CAPPED_TARGETS: u32 = 1;
const PARTY_HEAL: u32 = 5_129_003;
const FRIENDLY_HEAL: u32 = 5_129_004;
const PARTY: u64 = 9_216;
const AREA_RADIUS_YD: f32 = 10.0;
// One wolf more than a cap of 20 units would hit.
const PULL_SIZE: usize = 21;
const CHARACTER_FACTION: u32 = 9_211;
const WOLF_FACTION: u32 = 9_212;
// Neutral to the Character; the last two are reputation factions.
const NEUTRAL_FACTION: u32 = 9_213;
const NEUTRAL_REPUTATION_FACTION: u32 = 9_214;
const HOSTILE_REPUTATION_FACTION: u32 = 9_215;
// A base standing at Hostile puts a Character at war with the faction.
const HOSTILE_STANDING: i32 = -6_000;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_instant_area_spell_hits_a_neutral_beast_and_draws_it_into_combat() {
    let (shard, wolf) = stage("instant-area-neutral-beast");
    set_faction(&shard, wolf, NEUTRAL_FACTION);

    cast(&shard, AREA_BURST);

    assert!(health(&shard, wolf) < WOLF_HEALTH);
    assert_eq!(
        shard
            .query_rows(&format!(
                "SELECT id FROM game_threat WHERE creature_guid = {wolf} \
                 AND source_guid = {CHARACTER}"
            ))
            .len(),
        1,
        "vmangos DoSpellHitOnUnit: a unit the spell harms attacks its caster"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_instant_area_spell_spares_a_neutral_town_guard_and_hits_a_faction_at_war() {
    let (shard, guard) = stage("instant-area-reputation");
    let enemy = spawn_wolf(&shard, 2.0);
    set_faction(&shard, guard, NEUTRAL_REPUTATION_FACTION);
    set_faction(&shard, enemy, HOSTILE_REPUTATION_FACTION);

    cast(&shard, AREA_BURST);

    assert_eq!(
        health(&shard, guard),
        WOLF_HEALTH,
        "a town guard at Neutral is spared"
    );
    assert!(
        health(&shard, enemy) < WOLF_HEALTH,
        "a faction the Character is at war with is hit"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_instant_area_spell_spares_an_evading_creature() {
    let (shard, homing) = stage("instant-area-evade");
    let fighting = spawn_wolf(&shard, 2.0);
    shard.insert_row(
        "game_creature_ai_returning_home",
        &[("creature_guid", &homing)],
    );

    cast(&shard, AREA_BURST);

    assert_eq!(
        health(&shard, homing),
        WOLF_HEALTH,
        "vmangos AddUnitTarget: SPELL_MISS_EVADE"
    );
    assert!(health(&shard, fighting) < WOLF_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_instant_area_spell_logs_the_damage_it_deals_each_unit() {
    let (shard, wolf) = stage("instant-area-damage-logs");
    let second = spawn_wolf(&shard, 2.0);
    let logs = subscribe_with_fence(
        &shard,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = {CHARACTER} \
             AND is_proc_log = true"
        )],
    );

    cast(&shard, AREA_BURST);
    fence_subscription(&shard, &logs);

    // The seeded trainer and flight master stand in the area too, neutral without faction rows.
    let mut logged: Vec<(u64, u32)> = event_rows(&logs, "game_spell_cast_event_live")
        .iter()
        .map(|row| (number(row, "target_guid"), number(row, "damage")))
        .filter(|&(guid, _)| guid == wolf || guid == second)
        .collect();
    logged.sort_unstable();
    let mut dealt = [wolf, second].map(|guid| (guid, WOLF_HEALTH - health(&shard, guid)));
    dealt.sort_unstable();
    assert_eq!(
        logged, dealt,
        "vmangos DoSpellHitOnUnit sends SMSG_SPELLNONMELEEDAMAGELOG for each unit the spell hits"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_instant_area_spell_without_max_affected_targets_hits_every_unit_in_a_large_pull() {
    let (shard, wolf) = stage("instant-area-large-pull");
    let mut pull = vec![wolf];
    while pull.len() < PULL_SIZE {
        pull.push(spawn_wolf(&shard, 2.0));
    }

    cast(&shard, AREA_BURST);

    let spared: Vec<u64> = pull
        .into_iter()
        .filter(|&guid| health(&shard, guid) == WOLF_HEALTH)
        .collect();
    assert!(
        spared.is_empty(),
        "every wolf in the area is hit: {spared:?}"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_instant_area_spell_with_max_affected_targets_hits_only_that_many_and_keeps_its_target() {
    let (shard, wolf) = stage("instant-area-max-affected-targets");
    let target = spawn_wolf(&shard, 2.0);
    let third = spawn_wolf(&shard, 3.0);

    shard.assert_call(
        "debug_force_cast_at",
        &[
            &CHARACTER.to_string(),
            &CAPPED_BURST.to_string(),
            &target.to_string(),
        ],
    );

    assert!(
        health(&shard, target) < WOLF_HEALTH,
        "the cast's target is kept"
    );
    assert_eq!(
        [wolf, third]
            .into_iter()
            .filter(|&guid| health(&shard, guid) < WOLF_HEALTH)
            .count(),
        0,
        "MaxAffectedTargets {CAPPED_TARGETS} leaves room for the target alone"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_party_area_spell_heals_the_casters_group_and_spares_every_other_unit() {
    let (shard, wolf) = stage("party-area-group");
    let member = provision_group_member(&shard);
    let friend = spawn_wolf(&shard, 2.0);
    set_faction(&shard, friend, CHARACTER_FACTION);
    set_faction(&shard, wolf, NEUTRAL_FACTION);
    for guid in [member, friend, wolf] {
        wound(&shard, guid);
    }
    let before = [member, friend, wolf].map(|guid| health(&shard, guid));

    cast(&shard, PARTY_HEAL);

    assert!(
        health(&shard, member) > before[0],
        "the Group member is healed"
    );
    assert_eq!(
        [health(&shard, friend), health(&shard, wolf)],
        [before[1], before[2]],
        "vmangos FillRaidOrPartyTargets: a unit outside the Group is spared"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_friendly_area_spell_heals_a_friendly_unit_and_spares_a_neutral_one() {
    let (shard, wolf) = stage("friendly-area-neutral");
    let friend = spawn_wolf(&shard, 2.0);
    set_faction(&shard, friend, CHARACTER_FACTION);
    set_faction(&shard, wolf, NEUTRAL_FACTION);
    wound(&shard, friend);
    wound(&shard, wolf);

    cast(&shard, FRIENDLY_HEAL);

    assert!(health(&shard, friend) > WOLF_HEALTH / 2);
    assert_eq!(
        health(&shard, wolf),
        WOLF_HEALTH / 2,
        "vmangos SPELL_TARGETS_FRIENDLY"
    );
}

fn stage(name: &str) -> (Standalone, u64) {
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
        "UPDATE game_world_entity SET level = 60 WHERE guid = {CHARACTER}"
    ));
    factions(&shard);
    set_faction(&shard, CHARACTER, CHARACTER_FACTION);
    let wolf = spawn_wolf(&shard, 1.0);
    insert_burst(&shard, AREA_BURST, &[]);
    insert_burst(
        &shard,
        CAPPED_BURST,
        &[("max_affected_targets", &CAPPED_TARGETS)],
    );
    insert_heal(&shard, PARTY_HEAL, T_PARTY_AREA);
    insert_heal(&shard, FRIENDLY_HEAL, T_AREA_ALLY);
    (shard, wolf)
}

// A second Character of the fixture Account beside the caster, in one Group with it.
fn provision_group_member(shard: &Standalone) -> u64 {
    shard.assert_call(
        "create_character",
        &["1", "\"Member\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let member: u64 = number(
        &shard.query_rows("SELECT guid FROM game_character WHERE name = 'Member'")[0],
        "guid",
    );
    let caster = shard
        .query_rows(&format!(
            "SELECT map_id, x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    shard.assert_sql(&format!(
        "UPDATE game_character SET map_id = {}, x = {}, y = {}, z = {} WHERE guid = {member}",
        caster["map_id"],
        number::<f32>(&caster, "x") + 1.0,
        caster["y"],
        caster["z"]
    ));
    shard.assert_call("debug_spawn_player_entity", &[&member.to_string()]);
    set_faction(shard, member, CHARACTER_FACTION);
    shard.insert_row(
        "game_group",
        &[("group_id", &PARTY), ("leader_guid", &CHARACTER)],
    );
    let no_identity = format!("0x{}", "00".repeat(32));
    for (id, guid) in [(9_216_001u64, CHARACTER), (9_216_002, member)] {
        shard.insert_row(
            "game_group_member",
            &[
                ("id", &id),
                ("group_id", &PARTY),
                ("character_guid", &guid),
                ("owner_identity", &no_identity),
            ],
        );
    }
    member
}

fn wound(shard: &Standalone, guid: u64) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {}, max_health = {WOLF_HEALTH} WHERE guid = {guid}",
        WOLF_HEALTH / 2
    ));
}

fn insert_heal(shard: &Standalone, spell_id: u32, target: u8) {
    let name = format!("Area heal fixture {spell_id}");
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &name),
            ("school_mask", &0x02),
            ("range_yd", &0),
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
            ("radius_yd", &AREA_RADIUS_YD),
        ],
    );
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
    for id in [
        NEUTRAL_FACTION,
        NEUTRAL_REPUTATION_FACTION,
        HOSTILE_REPUTATION_FACTION,
    ] {
        shard.insert_row(
            "game_faction_template",
            &[
                ("id", &id),
                ("faction", &id),
                ("faction_group", &4),
                ("friend_group", &0),
                ("enemy_group", &0),
            ],
        );
    }
    for (faction_id, reputation_index, base_standing) in [
        (NEUTRAL_REPUTATION_FACTION, 40, 0),
        (HOSTILE_REPUTATION_FACTION, 41, HOSTILE_STANDING),
    ] {
        shard.insert_row(
            "game_faction",
            &[
                ("faction_id", &faction_id),
                ("reputation_index", &reputation_index),
                ("base_standing", &base_standing),
            ],
        );
    }
}

fn set_faction(shard: &Standalone, guid: u64, faction: u32) {
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = {faction} WHERE guid = {guid}"
    ));
}

/// A hostile wolf `offset` yards ahead of the Character, well inside the area.
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
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    set_faction(shard, wolf, WOLF_FACTION);
    wolf
}

// No range check and no speed, so no flight; CAST_FLAG_IGNORE_RESISTANCES never misses.
fn insert_burst(shard: &Standalone, spell_id: u32, extra: &[Field]) {
    let name = format!("Instant area fixture {spell_id}");
    let mut spell_row: Vec<Field> = vec![
        ("spell_id", &spell_id),
        ("name", &name),
        ("school_mask", &0x04),
        ("range_yd", &0),
        ("is_negative", &true),
        ("gcd_ms", &0),
        ("cooldown_ms", &0),
        ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
    ];
    spell_row.extend_from_slice(extra);
    shard.insert_row("game_spell", &spell_row);
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("kind", &E_DAMAGE),
            ("target", &T_AREA_ENEMY),
            ("base_points", &10),
            ("radius_yd", &AREA_RADIUS_YD),
        ],
    );
}

fn cast(shard: &Standalone, spell_id: u32) {
    shard.assert_call(
        "debug_force_cast",
        &[&CHARACTER.to_string(), &spell_id.to_string()],
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
