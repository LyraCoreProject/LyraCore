mod support;

use std::time::{SystemTime, UNIX_EPOCH};

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_INERT, E_HEAL, E_NEXT_SWING, P_NONE, T_TARGET_ALLY, T_TARGET_ENEMY,
};
use lyracore_shared::spell_custom::FIXED_DAMAGE;
use support::{event_rows, number, poll_until, Standalone, POLL_TIMEOUT};

const PLAYER: u64 = 1;
const PALADIN_GUID: u64 = 5_096_011;
const HUMAN: u8 = 1;
const PALADIN: u8 = 2;
// vmangos spell_threat rows: Sunder Armor rank 5 (11597) and Heroic Strike rank 9 (25286).
const SUNDER_ARMOR_RANK_5_THREAT: i64 = 261;
const HEROIC_STRIKE_RANK_9_THREAT: i64 = 175;
const FIXTURE_SUNDER_ARMOR: u32 = 509_1110;
const FIXTURE_SUNDER_ARMOR_RANK_TWO: u32 = 509_1111;
const FIXTURE_CURSE: u32 = 509_1112;
const FIXTURE_HEROIC_STRIKE: u32 = 509_1113;
const FIXTURE_HOLY_LIGHT: u32 = 509_1114;
const FIXTURE_FORTITUDE: u32 = 509_1115;
const FIXTURE_HEAL_POINTS: i32 = 400;
const FORTITUDE_THREAT: i64 = 60;
const HEROIC_STRIKE_MULTIPLIER: f32 = 2.0;

fn stage(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    shard
}

fn wolf(shard: &Standalone) -> u64 {
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&PLAYER.to_string(), &TEST_WOLF_ENTRY.to_string(), "2"],
    );
    let wolf = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .map(|row| number::<u64>(row, "guid"))
        .max()
        .expect("the spawned wolf must exist");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, health = 100000, \
         max_health = 100000 WHERE guid = {wolf}"
    ));
    wolf
}

fn spell(shard: &Standalone, spell_id: u32, name: &str, negative: bool) {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &name),
            ("range_yd", &30),
            ("school_mask", &1),
            ("is_negative", &negative),
            ("duration_ms", &30_000),
            ("custom_flags", &FIXED_DAMAGE),
        ],
    );
}

fn effect(shard: &Standalone, spell_id: u32, kind: u8, base_points: i32, target: u8) {
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("kind", &kind),
            ("base_points", &base_points),
            ("target", &target),
            ("p_0_kind", &P_NONE),
        ],
    );
}

fn debuff(shard: &Standalone, spell_id: u32, name: &str) {
    spell(shard, spell_id, name, true);
    effect(shard, spell_id, A_INERT, 0, T_TARGET_ENEMY);
}

fn table_threat(shard: &Standalone, spell_id: u32, threat: i64, multiplier: f32) {
    shard.insert_row(
        "game_spell_threat",
        &[
            ("spell_id", &spell_id),
            ("threat", &threat),
            ("multiplier", &multiplier),
            ("inverse_effect_mask", &0),
        ],
    );
}

fn cast(shard: &Standalone, caster: u64, spell_id: u32, target: u64) {
    shard.assert_call(
        "debug_cast_at",
        &[
            &caster.to_string(),
            &spell_id.to_string(),
            &target.to_string(),
        ],
    );
}

fn threat(shard: &Standalone, creature: u64, source: u64) -> Option<i64> {
    shard
        .query_rows(&format!(
            "SELECT threat FROM game_threat WHERE creature_guid = {creature} \
             AND source_guid = {source}"
        ))
        .first()
        .map(|row| number(row, "threat"))
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn sunder_armor_adds_its_table_threat_without_damage() {
    let shard = stage("threat-table-sunder");
    let wolf = wolf(&shard);
    debuff(&shard, FIXTURE_SUNDER_ARMOR, "Fixture Sunder Armor");
    table_threat(
        &shard,
        FIXTURE_SUNDER_ARMOR,
        SUNDER_ARMOR_RANK_5_THREAT,
        1.0,
    );
    cast(&shard, PLAYER, FIXTURE_SUNDER_ARMOR, wolf);
    assert_eq!(
        threat(&shard, wolf, PLAYER),
        Some(SUNDER_ARMOR_RANK_5_THREAT)
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_rank_without_its_own_row_adds_its_first_ranks_table_threat() {
    let shard = stage("threat-table-rank");
    let wolf = wolf(&shard);
    debuff(&shard, FIXTURE_SUNDER_ARMOR, "Fixture Sunder Armor");
    debuff(
        &shard,
        FIXTURE_SUNDER_ARMOR_RANK_TWO,
        "Fixture Sunder Armor Rank Two",
    );
    shard.insert_row(
        "game_spell_chain",
        &[
            ("spell_id", &FIXTURE_SUNDER_ARMOR_RANK_TWO),
            ("prev_spell", &FIXTURE_SUNDER_ARMOR),
            ("first_spell", &FIXTURE_SUNDER_ARMOR),
            ("rank", &2),
            ("req_spell", &0),
        ],
    );
    table_threat(
        &shard,
        FIXTURE_SUNDER_ARMOR,
        SUNDER_ARMOR_RANK_5_THREAT,
        1.0,
    );
    cast(&shard, PLAYER, FIXTURE_SUNDER_ARMOR_RANK_TWO, wolf);
    assert_eq!(
        threat(&shard, wolf, PLAYER),
        Some(SUNDER_ARMOR_RANK_5_THREAT)
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_harmful_aura_without_a_table_row_lists_its_caster_with_no_threat() {
    let shard = stage("threat-table-curse");
    let wolf = wolf(&shard);
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 60 WHERE guid = {PLAYER}"
    ));
    debuff(&shard, FIXTURE_CURSE, "Fixture Curse");
    cast(&shard, PLAYER, FIXTURE_CURSE, wolf);
    assert_eq!(
        threat(&shard, wolf, PLAYER),
        Some(0),
        "vmangos adds no level x 5 threat for a harmful Aura"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_landed_heroic_strike_adds_its_table_threat_and_scaled_damage_threat() {
    let shard = stage("threat-table-heroic-strike");
    let wolf = wolf(&shard);
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &FIXTURE_HEROIC_STRIKE),
            ("name", &"Fixture Heroic Strike"),
            ("power_type", &1),
            ("range_yd", &5),
            ("school_mask", &1),
        ],
    );
    effect(
        &shard,
        FIXTURE_HEROIC_STRIKE,
        E_NEXT_SWING,
        11,
        T_TARGET_ENEMY,
    );
    table_threat(
        &shard,
        FIXTURE_HEROIC_STRIKE,
        HEROIC_STRIKE_RANK_9_THREAT,
        HEROIC_STRIKE_MULTIPLIER,
    );
    let swings = shard.subscribe(&["SELECT * FROM game_combat_event_live WHERE attacker_guid = 1"]);
    shard.assert_call("debug_engage", &[&PLAYER.to_string(), &wolf.to_string()]);
    assert!(
        poll_until(POLL_TIMEOUT, || !event_rows(
            &swings,
            "game_combat_event_live"
        )
        .is_empty()),
        "the engagement never swung"
    );
    for _ in 0..20 {
        hold_the_swing(&shard);
        shard.assert_sql(&format!(
            "UPDATE game_threat SET threat = 0 WHERE creature_guid = {wolf} \
             AND source_guid = {PLAYER}"
        ));
        cast(&shard, PLAYER, FIXTURE_HEROIC_STRIKE, wolf);
        let swings =
            shard.subscribe(&["SELECT * FROM game_combat_event_live WHERE attacker_guid = 1"]);
        shard.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
        assert!(
            poll_until(POLL_TIMEOUT, || !event_rows(
                &swings,
                "game_combat_event_live"
            )
            .is_empty()),
            "the strike never swung"
        );
        hold_the_swing(&shard);
        let swing = event_rows(&swings, "game_combat_event_live").remove(0);
        assert_eq!(swing["spell_swing"], "true");
        let damage: i64 = number(&swing, "damage");
        let observed = threat(&shard, wolf, PLAYER).unwrap_or(0);
        if damage == 0 {
            assert_eq!(observed, 0, "an avoided strike adds no table threat");
            continue;
        }
        assert_eq!(
            observed,
            HEROIC_STRIKE_RANK_9_THREAT + damage * HEROIC_STRIKE_MULTIPLIER as i64
        );
        return;
    }
    panic!("no Heroic Strike landed in 20 swings");
}

fn hold_the_swing(shard: &Standalone) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u32;
    shard.assert_sql(&format!(
        "UPDATE game_melee_attack SET last_swing_ms = {now_ms} WHERE attacker_guid = {PLAYER}"
    ));
}

struct Healing {
    shard: Standalone,
    wolves: [u64; 2],
}

impl Healing {
    fn stage(name: &str) -> Self {
        let shard = stage(name);
        let wolves = [wolf(&shard), wolf(&shard)];
        for wolf in wolves {
            shard.assert_call(
                "debug_add_threat",
                &[&wolf.to_string(), &PLAYER.to_string(), "100"],
            );
        }
        let at = &shard.query_rows(&format!(
            "SELECT x, y, z, map_id FROM game_world_entity WHERE guid = {PLAYER}"
        ))[0];
        shard.assert_call(
            "debug_stage_meeting_stone_character",
            &[
                &PALADIN_GUID.to_string(),
                &PALADIN_GUID.to_string(),
                &HUMAN.to_string(),
                &PALADIN.to_string(),
                "60",
                &at["map_id"],
                &at["x"],
                &at["y"],
                &at["z"],
            ],
        );
        Self { shard, wolves }
    }

    fn paladin_threat(&self) -> [Option<i64>; 2] {
        self.wolves
            .map(|wolf| threat(&self.shard, wolf, PALADIN_GUID))
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_paladins_heal_adds_a_quarter_threat_split_among_the_creatures_fighting_its_target() {
    let healing = Healing::stage("threat-table-paladin-heal");
    let shard = &healing.shard;
    spell(shard, FIXTURE_HOLY_LIGHT, "Fixture Holy Light", false);
    effect(
        shard,
        FIXTURE_HOLY_LIGHT,
        E_HEAL,
        FIXTURE_HEAL_POINTS,
        T_TARGET_ALLY,
    );
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 100, max_health = 1000 WHERE guid = {PLAYER}"
    ));
    cast(shard, PALADIN_GUID, FIXTURE_HOLY_LIGHT, PLAYER);
    let quarter_each = i64::from(FIXTURE_HEAL_POINTS) / 4 / 2;
    assert_eq!(healing.paladin_threat(), [Some(quarter_each); 2]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_buffs_table_threat_is_split_among_the_creatures_fighting_its_target() {
    let healing = Healing::stage("threat-table-buff");
    let shard = &healing.shard;
    spell(shard, FIXTURE_FORTITUDE, "Fixture Fortitude", false);
    effect(shard, FIXTURE_FORTITUDE, A_INERT, 0, T_TARGET_ALLY);
    table_threat(shard, FIXTURE_FORTITUDE, FORTITUDE_THREAT, 0.0);
    cast(shard, PALADIN_GUID, FIXTURE_FORTITUDE, PLAYER);
    assert_eq!(healing.paladin_threat(), [Some(FORTITUDE_THREAT / 2); 2]);
}
