mod support;

use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_shared::constants::unit_flags::IN_COMBAT;
use lyracore_shared::guid;
use lyracore_shared::spell::taxonomy::CAST_FLAG_CHANNELED;
use lyracore_shared::spell::taxonomy::{A_ABSORB, A_PERIODIC_DAMAGE, CAST_FLAG_IGNORE_RESISTANCES};
use lyracore_shared::spell_custom::CHAN_NO_DIST_LIMIT;
use support::{event_rows, number, poll_until, Row, Standalone, POLL_TIMEOUT};

const CHARACTER: u64 = 1;
const WOLF: u64 = guid::creature(TEST_WOLF_ENTRY, 1);
/// `debug_fill_aura_slots` gives buff slot n spell id `9_000_000 + n` and debuff slots start at 32.
const SHIELD: u32 = 9_000_000;
const DOT: u32 = 9_000_032;
const DAMAGE_BROKEN_ROOT: u32 = 9_000_033;
const PHYSICAL: i32 = 1;
const FIRE: i32 = 4;
const SHADOW: i32 = 32;
const ALL_SCHOOLS: i32 = 127;
// Mana Shield's EffectMultipleValue 2.0 in hundredths, as an Aura's `eff_p1` keeps it.
const MANA_SHIELD_COST: i32 = 200;
const SECOND_ABSORB: u32 = SHIELD + 1;
const MANA_SHIELD_POOL: i32 = 500;
const PROC_ROOT: u32 = 9_000_034;
/// `SpellCastEventKind::PeriodicLog`.
const PERIODIC_LOG: u8 = 6;
// AURA_INTERRUPT_DAMAGE_CANCELS, kept in `aura_interrupt` bit 0.
const BREAKS_ON_DAMAGE: u16 = 0x1;
// vmangos PROC_FLAG_TAKEN_MELEE_HIT.
const TAKEN_MELEE_PROC_FLAGS: u32 = 0x8;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_periodic_kill_commits_the_death_and_an_unrelated_heal() {
    let mut standalone = Standalone::start("aura-tick-death");
    standalone.publish_module();
    standalone.clear_schedule("game_creature_move_schedule");
    standalone.clear_schedule("game_melee_schedule");
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, health = 1, max_health = 1 WHERE guid = {WOLF}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, health = 50, max_health = 100 WHERE guid = {CHARACTER}"
    ));
    assert_eq!(health(&standalone, WOLF), 1);
    standalone.assert_call(
        "debug_fill_aura_slots",
        &[&WOLF.to_string(), "1", "true", "1"],
    );
    standalone.assert_call("debug_fill_aura_slots", &["1", "1", "false", "1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_aura SET eff_kind = 144, amount = 7 WHERE target_guid = {WOLF}"
    ));
    standalone.assert_sql("UPDATE game_aura SET eff_kind = 145, amount = 7 WHERE target_guid = 1");
    let due = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
        - 1_000_000;
    standalone.assert_sql(&format!(
        "UPDATE game_aura SET period_ms = 600000, next_tick_micros = {due}"
    ));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let wolf = standalone.query_rows(&format!(
            "SELECT health, dead FROM game_world_entity WHERE guid = {WOLF}"
        ));
        if wolf[0]["dead"] == "true" {
            assert_eq!(wolf[0]["health"], "0");
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the periodic kill did not commit; wolf health={}, unrelated heal health={}",
            wolf[0]["health"],
            health(&standalone, CHARACTER),
        );
        thread::sleep(Duration::from_millis(50));
    }

    assert_eq!(health(&standalone, CHARACTER), 57);
    assert!(standalone
        .query_rows(&format!(
            "SELECT id FROM game_aura WHERE target_guid = {WOLF}"
        ))
        .is_empty());
    let surviving =
        standalone.query_rows("SELECT next_tick_micros FROM game_aura WHERE target_guid = 1");
    assert_eq!(surviving.len(), 1);
    assert_eq!(
        surviving[0]["next_tick_micros"].parse::<i64>().unwrap(),
        due + 600_000_000,
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_shield_takes_a_periodic_tick_it_covers_whole_and_the_tick_kills_nothing() {
    let standalone = stage_shielded_character("periodic-absorb-whole", 20, 100, 30, 1);

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick, "damage"), 0);
    assert_eq!(number::<u32>(&tick, "absorbed"), 30);
    assert_eq!(absorb_left(&standalone, SHIELD), Some(70));
    assert_eq!(health(&standalone, CHARACTER), 20);
    assert_eq!(
        standalone.query_rows(&format!(
            "SELECT dead FROM game_world_entity WHERE guid = {CHARACTER}"
        ))[0]["dead"],
        "false"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_periodic_tick_larger_than_a_shield_spends_it_and_deals_the_rest() {
    let standalone = stage_shielded_character("periodic-absorb-part", 10_000, 40, 100, 1);

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick, "damage"), 60);
    assert_eq!(number::<u32>(&tick, "absorbed"), 40);
    assert_eq!(
        absorb_left(&standalone, SHIELD),
        None,
        "the spent shield ends"
    );
    assert_eq!(health(&standalone, CHARACTER), 9_940);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_periodic_tick_a_shield_takes_whole_breaks_auras_as_an_absorbed_hit_does() {
    let standalone = stage_shielded_character("periodic-absorb-breaks", 10_000, 100, 30, 3);
    standalone.insert_rows(
        "game_spell",
        &[
            &[
                ("spell_id", &DAMAGE_BROKEN_ROOT),
                ("name", &"Periodic absorb fixture root"),
                ("aura_interrupt", &BREAKS_ON_DAMAGE),
            ],
            &[
                ("spell_id", &PROC_ROOT),
                ("name", &"Periodic absorb fixture proc root"),
                ("aura_interrupt", &BREAKS_ON_DAMAGE),
                ("proc_flags", &TAKEN_MELEE_PROC_FLAGS),
            ],
        ],
    );

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick, "damage"), 0);
    assert!(aura_ids(&standalone, DAMAGE_BROKEN_ROOT).is_empty());
    assert_eq!(
        aura_ids(&standalone, PROC_ROOT).len(),
        1,
        "vmangos checkProcFlags keeps an Aura whose spell has proc flags"
    );
    assert_eq!(health(&standalone, CHARACTER), 10_000);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_channelled_periodic_tick_a_shield_takes_whole_puts_both_units_in_combat() {
    let standalone = stage_shielded_character("periodic-channel-absorbed", 10_000, 100, 30, 1);
    stage_dot_spell(&standalone, CAST_FLAG_CHANNELED);
    assert!(!in_combat(&standalone, CHARACTER) && !in_combat(&standalone, WOLF));

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick, "absorbed"), 30);
    assert_eq!(number::<u32>(&tick, "damage"), 0);
    assert!(in_combat(&standalone, CHARACTER));
    assert!(in_combat(&standalone, WOLF));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_channelled_periodic_tick_that_lands_puts_both_units_in_combat() {
    let standalone = stage_shielded_character("periodic-channel-landed", 10_000, 0, 30, 1);
    stage_dot_spell(&standalone, CAST_FLAG_CHANNELED);

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick, "damage"), 30);
    assert!(in_combat(&standalone, CHARACTER));
    assert!(in_combat(&standalone, WOLF));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_periodic_tick_of_a_spell_that_is_not_channelled_puts_neither_unit_in_combat() {
    let standalone = stage_shielded_character("periodic-plain-no-combat", 10_000, 0, 30, 1);
    stage_dot_spell(&standalone, 0);

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick, "damage"), 30);
    assert!(!in_combat(&standalone, CHARACTER));
    assert!(!in_combat(&standalone, WOLF));
}

/// Gives the DOT a Spell row with `cast_flags` and `CHAN_NO_DIST_LIMIT`, and puts both units on map 0
/// with the caster outside simulation indexing.
fn stage_dot_spell(standalone: &Standalone, cast_flags: u32) {
    standalone.insert_rows(
        "game_spell",
        &[&[
            ("spell_id", &DOT),
            ("name", &"Periodic combat fixture"),
            ("cast_flags", &cast_flags),
            ("custom_flags", &CHAN_NO_DIST_LIMIT),
        ]],
    );
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, map_id = 0, instance_id = 0 \
         WHERE guid = {WOLF}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET map_id = 0, instance_id = 0 WHERE guid = {CHARACTER}"
    ));
}

fn in_combat(standalone: &Standalone, guid: u64) -> bool {
    let flags: u32 = standalone.query_rows(&format!(
        "SELECT unit_flags FROM game_world_entity WHERE guid = {guid}"
    ))[0]["unit_flags"]
        .parse()
        .unwrap();
    flags & IN_COMBAT != 0
}

/// An absorb Aura staged on the Character: the school mask it covers, its pool and the mana it
/// costs per point absorbed in hundredths, 0 for a school absorb.
struct Absorb {
    mask: i32,
    pool: i32,
    mana_cost: i32,
}

/// A Character at `health` of 10,000 behind a `shield` point absorb, under `debuffs` wolf Auras;
/// the first ticks for `tick` damage once made due.
fn stage_shielded_character(
    name: &str,
    health: u32,
    shield: i32,
    tick: i32,
    debuffs: u8,
) -> Standalone {
    let shield = Absorb {
        mask: ALL_SCHOOLS,
        pool: shield,
        mana_cost: 0,
    };
    stage_absorbing_character(name, health, &[shield], tick, debuffs)
}

/// A Character at `health` of 10,000 under `absorbs`, buff slot n holding the nth at spell id
/// `SHIELD + n`, and `debuffs` wolf Auras; the first ticks for `tick` damage once made due.
fn stage_absorbing_character(
    name: &str,
    health: u32,
    absorbs: &[Absorb],
    tick: i32,
    debuffs: u8,
) -> Standalone {
    let mut standalone = Standalone::start(name);
    standalone.publish_module();
    standalone.clear_schedule("game_creature_move_schedule");
    standalone.clear_schedule("game_melee_schedule");
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, godmode = false, \
         health = {health}, max_health = 10000 WHERE guid = {CHARACTER}"
    ));
    standalone.assert_call(
        "debug_fill_aura_slots",
        &[
            &CHARACTER.to_string(),
            &CHARACTER.to_string(),
            "false",
            &absorbs.len().to_string(),
        ],
    );
    for (slot, absorb) in absorbs.iter().enumerate() {
        standalone.assert_sql(&format!(
            "UPDATE game_aura SET eff_kind = {A_ABSORB}, eff_p0 = {}, eff_p1 = {}, \
             amount_remaining = {} WHERE spell_id = {}",
            absorb.mask,
            absorb.mana_cost,
            absorb.pool,
            SHIELD as usize + slot
        ));
    }
    standalone.assert_call(
        "debug_fill_aura_slots",
        &[
            &CHARACTER.to_string(),
            &WOLF.to_string(),
            "true",
            &debuffs.to_string(),
        ],
    );
    standalone.assert_sql(&format!(
        "UPDATE game_aura SET eff_kind = {A_PERIODIC_DAMAGE}, amount = {tick} WHERE spell_id = {DOT}"
    ));
    standalone
}

/// Gives the DOT a Spell row of `school_mask` that no resistance reduces.
fn stage_dot_school(standalone: &Standalone, school_mask: i32) {
    standalone.insert_rows(
        "game_spell",
        &[&[
            ("spell_id", &DOT),
            ("name", &"Periodic school fixture"),
            ("school_mask", &school_mask),
            ("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES),
        ]],
    );
}

fn set_mana(standalone: &Standalone, mana: u32) {
    let bytes: u32 = standalone
        .query_rows(&format!(
            "SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0)["unit_bytes_0"]
        .parse()
        .expect("unit_bytes_0 is a number");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {}, power = {mana}, max_power = 1000 \
         WHERE guid = {CHARACTER}",
        bytes & 0x00FF_FFFF
    ));
}

fn mana(standalone: &Standalone) -> u32 {
    standalone.query_rows(&format!(
        "SELECT power FROM game_world_entity WHERE guid = {CHARACTER}"
    ))[0]["power"]
        .parse()
        .expect("power is a number")
}

fn absorb_left(standalone: &Standalone, spell_id: u32) -> Option<i32> {
    standalone
        .query_rows(&format!(
            "SELECT amount_remaining FROM game_aura WHERE spell_id = {spell_id}"
        ))
        .first()
        .map(|row| number(row, "amount_remaining"))
}

fn physical_hit(standalone: &Standalone, rolled: u32) {
    standalone.assert_call(
        "debug_apply_lethal_damage_floor_fixture",
        &[
            &WOLF.to_string(),
            &CHARACTER.to_string(),
            &rolled.to_string(),
        ],
    );
}

fn mana_shield() -> Absorb {
    Absorb {
        mask: PHYSICAL,
        pool: MANA_SHIELD_POOL,
        mana_cost: MANA_SHIELD_COST,
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_fire_ward_takes_a_fire_tick() {
    let ward = Absorb {
        mask: FIRE,
        pool: 100,
        mana_cost: 0,
    };
    let standalone = stage_absorbing_character("absorb-fire-ward-fire", 10_000, &[ward], 30, 1);
    stage_dot_school(&standalone, FIRE);

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick, "absorbed"), 30);
    assert_eq!(number::<u32>(&tick, "damage"), 0);
    assert_eq!(absorb_left(&standalone, SHIELD), Some(70));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_fire_ward_takes_no_shadow_tick() {
    let ward = Absorb {
        mask: FIRE,
        pool: 100,
        mana_cost: 0,
    };
    let standalone = stage_absorbing_character("absorb-fire-ward-shadow", 10_000, &[ward], 30, 1);
    stage_dot_school(&standalone, SHADOW);

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick, "absorbed"), 0);
    assert_eq!(number::<u32>(&tick, "damage"), 30);
    assert_eq!(absorb_left(&standalone, SHIELD), Some(100));
    assert_eq!(health(&standalone, CHARACTER), 9_970);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_mana_shield_costs_two_mana_per_point_of_physical_damage_it_absorbs() {
    let standalone =
        stage_absorbing_character("absorb-mana-shield-cost", 10_000, &[mana_shield()], 30, 1);
    set_mana(&standalone, 1_000);

    physical_hit(&standalone, 100);

    assert_eq!(health(&standalone, CHARACTER), 10_000);
    assert_eq!(mana(&standalone), 800);
    assert_eq!(absorb_left(&standalone, SHIELD), Some(400));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_mana_shield_absorbs_no_more_than_the_mana_can_pay() {
    let standalone =
        stage_absorbing_character("absorb-mana-shield-low", 10_000, &[mana_shield()], 30, 1);
    set_mana(&standalone, 31);

    physical_hit(&standalone, 100);

    assert_eq!(health(&standalone, CHARACTER), 9_915);
    assert_eq!(mana(&standalone), 1);
    assert_eq!(absorb_left(&standalone, SHIELD), Some(485));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_mana_shield_ends_when_its_pool_is_spent() {
    let shield = Absorb {
        pool: 40,
        ..mana_shield()
    };
    let standalone =
        stage_absorbing_character("absorb-mana-shield-spent", 10_000, &[shield], 30, 1);
    set_mana(&standalone, 1_000);

    physical_hit(&standalone, 100);

    assert_eq!(health(&standalone, CHARACTER), 9_940);
    assert_eq!(mana(&standalone), 920);
    assert_eq!(absorb_left(&standalone, SHIELD), None);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_mana_shield_takes_no_spell_damage() {
    let standalone =
        stage_absorbing_character("absorb-mana-shield-spell", 10_000, &[mana_shield()], 30, 1);
    stage_dot_school(&standalone, FIRE);
    set_mana(&standalone, 1_000);

    let tick = tick_once(&standalone);

    assert_eq!(number::<u32>(&tick, "absorbed"), 0);
    assert_eq!(number::<u32>(&tick, "damage"), 30);
    assert_eq!(mana(&standalone), 1_000);
    assert_eq!(absorb_left(&standalone, SHIELD), Some(MANA_SHIELD_POOL));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_power_word_shield_takes_damage_before_an_older_mana_shield() {
    let power_word_shield = Absorb {
        mask: ALL_SCHOOLS,
        pool: 40,
        mana_cost: 0,
    };
    let standalone = stage_absorbing_character(
        "absorb-shield-before-mana-shield",
        10_000,
        &[mana_shield(), power_word_shield],
        30,
        1,
    );
    set_mana(&standalone, 1_000);

    physical_hit(&standalone, 100);

    assert_eq!(health(&standalone, CHARACTER), 10_000);
    assert_eq!(absorb_left(&standalone, SECOND_ABSORB), None);
    assert_eq!(mana(&standalone), 880);
    assert_eq!(absorb_left(&standalone, SHIELD), Some(440));
}

fn tick_once(standalone: &Standalone) -> Row {
    let ticks = standalone.subscribe(&[&format!(
        "SELECT * FROM game_spell_cast_event_live WHERE spell_id = {DOT} AND kind = {PERIODIC_LOG}"
    )]);
    let due = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as i64
        - 1_000_000;
    standalone.assert_sql(&format!(
        "UPDATE game_aura SET period_ms = 600000, next_tick_micros = {due} WHERE spell_id = {DOT}"
    ));
    let mut rows = Vec::new();
    assert!(
        poll_until(POLL_TIMEOUT, || {
            rows = event_rows(&ticks, "game_spell_cast_event_live");
            !rows.is_empty()
        }),
        "the periodic damage must tick"
    );
    assert_eq!(rows.len(), 1);
    rows.remove(0)
}

fn aura_ids(standalone: &Standalone, spell_id: u32) -> Vec<Row> {
    standalone.query_rows(&format!(
        "SELECT id FROM game_aura WHERE target_guid = {CHARACTER} AND spell_id = {spell_id}"
    ))
}

fn health(standalone: &Standalone, guid: u64) -> u32 {
    standalone.query_rows(&format!(
        "SELECT health FROM game_world_entity WHERE guid = {guid}"
    ))[0]["health"]
        .parse()
        .unwrap()
}
