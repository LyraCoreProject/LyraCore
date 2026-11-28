mod support;

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_SPELLMOD_FLAT, CAST_FLAG_ALWAYS_HIT, CAST_FLAG_IGNORE_RESISTANCES, E_CAST_FRIEND_OR_FOE,
    E_DAMAGE, E_HEAL, P_SPELLMOD_OP, P_SPELL_ID, SPELLMOD_OP_CRITICAL_CHANCE, T_SELF,
    T_TARGET_ALLY, T_TARGET_ANY, T_TARGET_ENEMY,
};
use support::{event_rows, fence_subscription, subscribe_with_fence, Standalone};

const PLAYER: u64 = 1;
const PLAYER_HEALTH: u32 = 10_000;
const WOLF_HEALTH: u32 = 100_000;
const HOLY: u8 = 0x02;
const DAMAGE_CLASS_MAGIC: u8 = 1;
const SPELL_FAMILY_PALADIN: u8 = 10;
const RANGE_YD: u32 = 30;
const SEPARATION_YD: f32 = 5.0;
const EFFECT_POINTS: i32 = 100;
const DURATION_MS: u32 = 600_000;
const MANA: u32 = 1000;
const COST: u32 = 10;
const COOLDOWN_MS: u32 = 30_000;
const CERTAIN_CRIT_PERCENT: i32 = 100;
// Faction groups: the Paladin and its ally, the other faction's Characters, and the wolf.
const ALLIED_FACTION: u32 = 5_141;
const HORDE_FACTION: u32 = 5_142;
const WOLF_FACTION: u32 = 5_143;
const ALLIED_GROUP: u32 = 0x1;
const HORDE_GROUP: u32 = 0x2;
const WOLF_GROUP: u32 = 0x4;
// vmangos Holy Shock rank 1 20473 and the family flag its heal 25914 and damage 25912 carry.
const HOLY_SHOCK_FAMILY_FLAG: u64 = 0x20_0000;
// Divine Favor's mask as vmangos SpellModMgr sets it.
const DIVINE_FAVOR_MASK: u32 = 0x8020_2000;

const HOLY_SHOCK: u32 = 5_141_001;
const HOLY_SHOCK_HEAL: u32 = 5_141_002;
const HOLY_SHOCK_DAMAGE: u32 = 5_141_003;
const DIVINE_FAVOR: u32 = 5_141_004;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn holy_shock_heals_a_friend_and_leaves_the_damage_spell_uncast() {
    let mut standalone = Standalone::start("holy-shock-friend");
    let wolf = stage(&mut standalone);
    let ally = spawn_ally(&standalone);
    set_health(&standalone, ally, 1);

    let cast_spells = cast_spells(&standalone, ally);

    assert_eq!(cast_spells, [HOLY_SHOCK, HOLY_SHOCK_HEAL]);
    assert!(health(&standalone, ally) > 1, "the heal raises the friend");
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn holy_shock_damages_an_enemy_and_leaves_the_heal_uncast() {
    let mut standalone = Standalone::start("holy-shock-enemy");
    let wolf = stage(&mut standalone);
    set_health(&standalone, PLAYER, 1);

    let cast_spells = cast_spells(&standalone, wolf);

    assert_eq!(cast_spells, [HOLY_SHOCK, HOLY_SHOCK_DAMAGE]);
    assert!(
        health(&standalone, wolf) < WOLF_HEALTH,
        "the damage hurts the enemy"
    );
    assert_eq!(health(&standalone, PLAYER), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn holy_shock_on_the_caster_heals_the_caster() {
    let mut standalone = Standalone::start("holy-shock-self");
    stage(&mut standalone);
    set_health(&standalone, PLAYER, 1);

    let cast_spells = cast_spells(&standalone, PLAYER);

    assert_eq!(cast_spells, [HOLY_SHOCK, HOLY_SHOCK_HEAL]);
    assert!(health(&standalone, PLAYER) > 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_enemy_behind_the_caster_refuses_holy_shock_and_a_friend_behind_does_not() {
    let mut standalone = Standalone::start("holy-shock-facing");
    let wolf = stage(&mut standalone);
    let ally = spawn_ally(&standalone);
    set_health(&standalone, ally, 1);
    place_beside_caster(&standalone, wolf, -SEPARATION_YD);
    place_beside_caster(&standalone, ally, -SEPARATION_YD);

    support::refused(
        &standalone,
        "debug_cast_at",
        &["1", &HOLY_SHOCK.to_string(), &wolf.to_string()],
        "target must be in front of caster",
    );
    assert_eq!(health(&standalone, wolf), WOLF_HEALTH);
    standalone.assert_call(
        "debug_cast_at",
        &["1", &HOLY_SHOCK.to_string(), &ally.to_string()],
    );
    assert!(health(&standalone, ally) > 1);

    place_beside_caster(&standalone, wolf, SEPARATION_YD);
    standalone.assert_call(
        "debug_cast_at",
        &["1", &HOLY_SHOCK.to_string(), &wolf.to_string()],
    );
    assert!(health(&standalone, wolf) < WOLF_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_unflagged_character_of_the_other_faction_behind_the_caster_refuses_holy_shock_and_spends_nothing(
) {
    let mut standalone = Standalone::start("holy-shock-other-faction");
    stage(&mut standalone);
    let orc = spawn_character(
        &standalone,
        "holy-shock-horde",
        "Orc",
        (2, 1),
        HORDE_FACTION,
    );
    place_beside_caster(&standalone, orc, -SEPARATION_YD);
    standalone.assert_sql(&format!(
        "UPDATE game_spell SET cost = {COST}, cooldown_ms = {COOLDOWN_MS} \
         WHERE spell_id = {HOLY_SHOCK}"
    ));
    use_mana(&standalone);
    set_health(&standalone, orc, 1);

    support::refused(
        &standalone,
        "debug_cast_at",
        &["1", &HOLY_SHOCK.to_string(), &orc.to_string()],
        "target must be in front of caster",
    );

    assert_eq!(mana(&standalone), MANA);
    assert!(standalone
        .query_rows(&format!(
            "SELECT id FROM game_spell_cd WHERE spell_id = {HOLY_SHOCK}"
        ))
        .is_empty());
    assert_eq!(health(&standalone, orc), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn divine_favor_makes_the_heal_holy_shock_casts_crit_and_ends_with_it() {
    let mut standalone = Standalone::start("holy-shock-divine-favor");
    stage(&mut standalone);
    divine_favor(&standalone);
    standalone.assert_call("debug_force_cast", &["1", &DIVINE_FAVOR.to_string()]);
    standalone.assert_call("debug_advance_virtual_clock", &["50000"]);
    set_health(&standalone, PLAYER, 1);

    let heal = go_event(&standalone, HOLY_SHOCK_HEAL, PLAYER);

    assert_eq!(heal["is_crit"], "true");
    assert_eq!(charges(&standalone), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn divine_favor_makes_the_damage_holy_shock_casts_crit_and_ends_with_it() {
    let mut standalone = Standalone::start("holy-shock-divine-favor-damage");
    let wolf = stage(&mut standalone);
    divine_favor(&standalone);
    standalone.assert_call("debug_force_cast", &["1", &DIVINE_FAVOR.to_string()]);
    standalone.assert_call("debug_advance_virtual_clock", &["50000"]);

    let damage = go_event(&standalone, HOLY_SHOCK_DAMAGE, wolf);

    assert_eq!(damage["is_crit"], "true");
    assert_eq!(charges(&standalone), None);
}

fn stage(standalone: &mut Standalone) -> u64 {
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {PLAYER_HEALTH}, max_health = {PLAYER_HEALTH}, \
         orientation = 0 WHERE guid = {PLAYER}"
    ));
    standalone.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    let wolf = standalone
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, armor = 0, level = 1, \
         health = {WOLF_HEALTH}, max_health = {WOLF_HEALTH} WHERE guid = {wolf}"
    ));
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    factions(standalone);
    set_faction(standalone, PLAYER, ALLIED_FACTION);
    set_faction(standalone, wolf, WOLF_FACTION);
    holy_shock(standalone);
    place_beside_caster(standalone, wolf, SEPARATION_YD);
    wolf
}

fn factions(standalone: &Standalone) {
    for (id, group, enemy_groups) in [
        (ALLIED_FACTION, ALLIED_GROUP, HORDE_GROUP | WOLF_GROUP),
        (HORDE_FACTION, HORDE_GROUP, ALLIED_GROUP),
        (WOLF_FACTION, WOLF_GROUP, ALLIED_GROUP),
    ] {
        standalone.insert_row(
            "game_faction_template",
            &[
                ("id", &id),
                ("faction", &id),
                ("faction_group", &group),
                ("friend_group", &group),
                ("enemy_group", &enemy_groups),
            ],
        );
    }
}

fn set_faction(standalone: &Standalone, guid: u64, faction: u32) {
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET faction_template = {faction} WHERE guid = {guid}"
    ));
}

fn holy_shock(standalone: &Standalone) {
    spell(standalone, HOLY_SHOCK, 0, 0);
    spell(standalone, HOLY_SHOCK_HEAL, DAMAGE_CLASS_MAGIC, 0);
    spell(
        standalone,
        HOLY_SHOCK_DAMAGE,
        DAMAGE_CLASS_MAGIC,
        CAST_FLAG_ALWAYS_HIT | CAST_FLAG_IGNORE_RESISTANCES,
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(HOLY_SHOCK, 0)),
            ("spell_id", &HOLY_SHOCK),
            ("kind", &E_CAST_FRIEND_OR_FOE),
            ("target", &T_TARGET_ANY),
            ("trigger_spell", &HOLY_SHOCK_HEAL),
            ("p_0", &(HOLY_SHOCK_DAMAGE as i32)),
            ("p_0_kind", &P_SPELL_ID),
        ],
    );
    for (spell_id, kind, target) in [
        (HOLY_SHOCK_HEAL, E_HEAL, T_TARGET_ALLY),
        (HOLY_SHOCK_DAMAGE, E_DAMAGE, T_TARGET_ENEMY),
    ] {
        standalone.insert_row(
            "game_spell_effect",
            &[
                ("id", &packed_spell_effect_id(spell_id, 0)),
                ("spell_id", &spell_id),
                ("kind", &kind),
                ("base_points", &EFFECT_POINTS),
                ("target", &target),
            ],
        );
    }
}

fn spell(standalone: &Standalone, spell_id: u32, damage_class: u8, cast_flags: u32) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &format!("Holy Shock fixture {spell_id}")),
            ("school_mask", &HOLY),
            ("range_yd", &RANGE_YD),
            ("family_name", &SPELL_FAMILY_PALADIN),
            ("family_flags", &HOLY_SHOCK_FAMILY_FLAG),
            ("damage_class", &damage_class),
            ("cast_flags", &cast_flags),
        ],
    );
}

fn divine_favor(standalone: &Standalone) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &DIVINE_FAVOR),
            ("name", &"Divine Favor fixture"),
            ("school_mask", &HOLY),
            ("range_yd", &RANGE_YD),
            ("duration_ms", &DURATION_MS),
            ("spell_level", &1),
            ("proc_chance", &100),
            ("proc_charges", &1),
            ("family_name", &SPELL_FAMILY_PALADIN),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(DIVINE_FAVOR, 0)),
            ("spell_id", &DIVINE_FAVOR),
            ("kind", &A_SPELLMOD_FLAT),
            ("base_points", &CERTAIN_CRIT_PERCENT),
            ("target", &T_SELF),
            ("p_0", &SPELLMOD_OP_CRITICAL_CHANCE),
            ("p_0_kind", &P_SPELLMOD_OP),
            ("p_1", &(DIVINE_FAVOR_MASK as i32)),
        ],
    );
}

fn spawn_ally(standalone: &Standalone) -> u64 {
    spawn_character(
        standalone,
        "holy-shock-test",
        "Ally",
        (1, 2),
        ALLIED_FACTION,
    )
}

fn spawn_character(
    standalone: &Standalone,
    package: &str,
    name: &str,
    (race, class): (u8, u8),
    faction: u32,
) -> u64 {
    standalone.assert_call(
        "debug_create_package_character",
        &[
            &format!("\"{package}\""),
            &format!("\"{name}\""),
            &race.to_string(),
            &class.to_string(),
        ],
    );
    let guid: u64 = standalone
        .query_rows(&format!(
            "SELECT guid FROM game_character WHERE name = '{name}'"
        ))
        .remove(0)["guid"]
        .parse()
        .unwrap();
    standalone.assert_call("debug_spawn_player_entity", &[&guid.to_string()]);
    set_faction(standalone, guid, faction);
    place_beside_caster(standalone, guid, SEPARATION_YD);
    guid
}

fn use_mana(standalone: &Standalone) {
    let bytes: u32 = standalone
        .query_rows(&format!(
            "SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .remove(0)["unit_bytes_0"]
        .parse()
        .expect("unit_bytes_0 is a number");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {}, power = {MANA}, max_power = {MANA} \
         WHERE guid = {PLAYER}",
        bytes & 0x00FF_FFFF
    ));
}

fn mana(standalone: &Standalone) -> u32 {
    standalone.query_rows(&format!(
        "SELECT power FROM game_world_entity WHERE guid = {PLAYER}"
    ))[0]["power"]
        .parse()
        .expect("power is a number")
}

fn place_beside_caster(standalone: &Standalone, guid: u64, offset_x: f32) {
    let caster = standalone
        .query_rows(&format!(
            "SELECT x, y, z, map_id, instance_id FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .remove(0);
    let x: f32 = caster["x"].parse().unwrap();
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {}, y = {}, z = {}, map_id = {}, instance_id = {} \
         WHERE guid = {guid}",
        x + offset_x,
        caster["y"],
        caster["z"],
        caster["map_id"],
        caster["instance_id"]
    ));
}

fn set_health(standalone: &Standalone, guid: u64, health: u32) {
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = {health} WHERE guid = {guid}"
    ));
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .expect("health is a number")
}

fn charges(standalone: &Standalone) -> Option<u8> {
    standalone
        .query_rows(&format!(
            "SELECT proc_charges FROM game_aura WHERE target_guid = {PLAYER} \
             AND spell_id = {DIVINE_FAVOR}"
        ))
        .first()
        .map(|row| row["proc_charges"].parse().unwrap())
}

// The spells whose SMSG_SPELL_GO the cast sends, in the order of the event ids.
fn cast_spells(standalone: &Standalone, target: u64) -> Vec<u32> {
    let events = subscribe_with_fence(
        standalone,
        &["SELECT * FROM game_spell_cast_event_live WHERE kind = 2"],
    );
    standalone.assert_call(
        "debug_cast_at",
        &["1", &HOLY_SHOCK.to_string(), &target.to_string()],
    );
    fence_subscription(standalone, &events);
    let mut spells: Vec<u32> = event_rows(&events, "game_spell_cast_event_live")
        .iter()
        .map(|row| support::number(row, "spell_id"))
        .collect();
    // These tests check which spells cast, not the order of their cast events.
    spells.sort_unstable();
    spells
}

fn go_event(standalone: &Standalone, spell_id: u32, target: u64) -> support::Row {
    let events = subscribe_with_fence(
        standalone,
        &[&format!(
            "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {spell_id} AND kind = 2"
        )],
    );
    standalone.assert_call(
        "debug_cast_at",
        &["1", &HOLY_SHOCK.to_string(), &target.to_string()],
    );
    fence_subscription(standalone, &events);
    event_rows(&events, "game_spell_cast_event_live")
        .pop()
        .expect("the heal's cast completes")
}
