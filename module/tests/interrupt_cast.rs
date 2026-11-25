mod support;

use lyracore_package_delta::fixture_ids::TEST_WOLF_ENTRY;
use lyracore_package_delta::packed_spell_effect_id;
use lyracore_shared::spell::taxonomy::{
    A_ABSORB, A_CONTROL, A_PERIODIC_DAMAGE, CAST_FLAG_CHANNELED, CAST_FLAG_IGNORE_RESISTANCES,
    E_DAMAGE, E_INTERRUPT, E_PERSISTENT_AREA, M_ROOT, P_MECHANIC, P_SCHOOL_MASK, T_SELF,
    T_TARGET_ENEMY,
};
use spacetimedb::Timestamp;
use support::{
    event_rows, fence_subscription, number, subscribe_with_fence, Field, Row, Standalone,
    TableSubscription,
};

const CHARACTER: u64 = 1;
const FIRE: u8 = 0x04;
const CHANNEL_AURA: u32 = 5_124_101;
const CHANNELLED_AREA: u32 = 5_124_102;
const SLOW_CAST: u32 = 5_124_103;
const KICK: u32 = 5_124_104;
const BITE: u32 = 5_124_105;
const DAMAGE_CANCELLED_CHANNEL: u32 = 5_124_106;
const SHORT_CHANNEL: u32 = 5_124_107;
const UNSILENCEABLE_CHANNEL: u32 = 5_124_108;
const UNPUSHED_CAST: u32 = 5_124_109;
const SHIELD: u32 = 5_124_110;
const DAMAGE_CANCELLED_CAST: u32 = 5_124_111;
const DAMAGE_BROKEN_ROOT: u32 = 5_124_112;
const PROC_ROOT: u32 = 5_124_113;
// vmangos PROC_FLAG_TAKEN_MELEE_HIT | PROC_FLAG_TAKEN_MELEE_SPELL_HIT.
const TAKEN_MELEE_PROC_FLAGS: u32 = 0x28;
// vmangos spell_template interrupt fields of Fireball 133, Drain Life 689 and First Aid 746.
const FIREBALL_INTERRUPT_FLAGS: u32 = 0x0F;
const DRAIN_LIFE_CHANNEL_FLAGS: u32 = 0x7C0C;
const FIRST_AID_CHANNEL_FLAGS: u32 = 0x3C0E;
const WITHOUT_PUSHBACK_INTERRUPT_FLAGS: u32 = 0x09;
// vmangos spell_template InterruptFlags of Hearthstone 8690.
const HEARTHSTONE_INTERRUPT_FLAGS: u32 = 0x1F;
// AURA_INTERRUPT_DAMAGE_CANCELS, kept in `aura_interrupt` bit 0.
const BREAKS_ON_DAMAGE: u16 = 0x1;
const SHIELD_POINTS: i32 = 1_000_000;
const ALL_SCHOOLS: i32 = 0x7F;
const SLOW_CAST_MS: u32 = 10_000;
const CAST_EVENTS: &str = "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1";
// `SpellCastEventKind::Pushback`.
const PUSHBACK: &str = "4";
const PREVENTION_SILENCE: u8 = 1;
const PREVENTION_NONE: u8 = 0;
const SHORT_CHANNEL_MS: u32 = 900;
const KICK_LOCKOUT_MS: u32 = 5_000;
const CHANNEL_DURATION_MS: u32 = 6_000;
const PERIOD_MS: u32 = 1_000;
// A cast at an equal level target misses at most 4% of the time.
const CAST_ATTEMPTS: usize = 20;
const CHARACTER_HEALTH: u32 = 100_000;
const CHARACTER_FACTION: u32 = 9_301;
const WOLF_FACTION: u32 = 9_302;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_ends_a_channel_aura_and_locks_its_school() {
    let (shard, wolf) = stage("interrupt-channel-aura");
    cast_until(&shard, CHARACTER, CHANNEL_AURA, wolf, || {
        !channel_auras(&shard).is_empty()
    });
    assert!(!channel_auras(&shard).is_empty(), "the channel must start");

    cast_until(&shard, wolf, KICK, CHARACTER, || {
        channel_auras(&shard).is_empty()
    });

    assert!(
        channel_auras(&shard).is_empty(),
        "the interrupt must end the channel"
    );
    assert_eq!(locked_schools(&shard), [FIRE]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_ends_a_channelled_ground_area_and_locks_its_school() {
    let (shard, wolf) = stage("interrupt-channelled-area");
    cast_area(&shard);
    assert_eq!(areas(&shard).len(), 1, "the channelled area must be placed");

    cast_until(&shard, wolf, KICK, CHARACTER, || areas(&shard).is_empty());

    assert!(
        areas(&shard).is_empty(),
        "the interrupt must end the channelled area"
    );
    assert_eq!(locked_schools(&shard), [FIRE]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_cancels_a_cast_in_progress_and_locks_its_school() {
    let (shard, wolf) = stage("interrupt-cast-in-progress");
    begin_cast(&shard, SLOW_CAST, wolf);

    cast_until(&shard, wolf, KICK, CHARACTER, || {
        pending_casts(&shard).is_empty()
    });

    assert!(
        pending_casts(&shard).is_empty(),
        "the interrupt must cancel the cast"
    );
    assert_eq!(locked_schools(&shard), [FIRE]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_on_a_unit_that_is_not_casting_writes_no_lockout() {
    let (shard, wolf) = stage("interrupt-idle-unit");

    for _ in 0..5 {
        shard.assert_call(
            "debug_force_cast_at",
            &[&wolf.to_string(), &KICK.to_string(), &CHARACTER.to_string()],
        );
    }

    assert!(locked_schools(&shard).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_leaves_a_channel_without_silence_prevention_running() {
    let (shard, wolf) = stage("interrupt-unsilenceable-channel");
    cast_until(&shard, CHARACTER, UNSILENCEABLE_CHANNEL, wolf, || {
        !auras_of(&shard, UNSILENCEABLE_CHANNEL).is_empty()
    });
    assert!(
        !auras_of(&shard, UNSILENCEABLE_CHANNEL).is_empty(),
        "the channel must start"
    );

    for _ in 0..5 {
        shard.assert_call(
            "debug_force_cast_at",
            &[&wolf.to_string(), &KICK.to_string(), &CHARACTER.to_string()],
        );
    }

    assert!(!auras_of(&shard, UNSILENCEABLE_CHANNEL).is_empty());
    assert!(locked_schools(&shard).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_interrupt_leaves_a_cast_that_damage_does_not_push_back_running() {
    let (shard, wolf) = stage("interrupt-unpushed-cast");
    begin_cast(&shard, UNPUSHED_CAST, wolf);

    for _ in 0..5 {
        shard.assert_call(
            "debug_force_cast_at",
            &[&wolf.to_string(), &KICK.to_string(), &CHARACTER.to_string()],
        );
    }

    assert_eq!(pending_casts(&shard).len(), 1);
    assert!(locked_schools(&shard).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn damage_shortens_a_channel_aura_by_a_second_and_then_by_less() {
    let (shard, wolf) = stage("damage-delays-channel-aura");
    cast_until(&shard, CHARACTER, CHANNEL_AURA, wolf, || {
        !channel_auras(&shard).is_empty()
    });
    let start = channel_end(&shard, CHANNEL_AURA);

    bite_until(&shard, wolf, || channel_end(&shard, CHANNEL_AURA) != start);
    let first = channel_end(&shard, CHANNEL_AURA);
    bite_until(&shard, wolf, || channel_end(&shard, CHANNEL_AURA) != first);
    let second = channel_end(&shard, CHANNEL_AURA);

    assert_eq!((start - first, first - second), (1_000_000, 800_000));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn damage_shortens_a_channelled_ground_area_by_a_second() {
    let (shard, wolf) = stage("damage-delays-channelled-area");
    cast_area(&shard);
    let start = area_end(&shard);

    bite_until(&shard, wolf, || area_end(&shard) != start);

    assert_eq!(start - area_end(&shard), 1_000_000);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn damage_ends_a_channel_with_less_time_left_than_its_delay() {
    let (shard, wolf) = stage("damage-ends-short-channel");
    cast_until(&shard, CHARACTER, SHORT_CHANNEL, wolf, || {
        !auras_of(&shard, SHORT_CHANNEL).is_empty()
    });
    assert!(
        !auras_of(&shard, SHORT_CHANNEL).is_empty(),
        "the channel must start"
    );

    bite_until(&shard, wolf, || auras_of(&shard, SHORT_CHANNEL).is_empty());

    assert!(auras_of(&shard, SHORT_CHANNEL).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn damage_cancels_a_channel_whose_flags_say_damage_cancels_it() {
    let (shard, wolf) = stage("damage-cancels-channel");
    cast_until(&shard, CHARACTER, DAMAGE_CANCELLED_CHANNEL, wolf, || {
        !auras_of(&shard, DAMAGE_CANCELLED_CHANNEL).is_empty()
    });
    assert!(
        !auras_of(&shard, DAMAGE_CANCELLED_CHANNEL).is_empty(),
        "the channel must start"
    );

    bite_until(&shard, wolf, || {
        auras_of(&shard, DAMAGE_CANCELLED_CHANNEL).is_empty()
    });

    assert!(auras_of(&shard, DAMAGE_CANCELLED_CHANNEL).is_empty());
    assert!(health(&shard, CHARACTER) < CHARACTER_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn damage_leaves_a_cast_without_the_pushback_flag_on_time() {
    let (shard, wolf) = stage("damage-leaves-unpushed-cast");
    begin_cast(&shard, UNPUSHED_CAST, wolf);
    let due = pending_casts(&shard)[0]["scheduled_at"].clone();

    bite_until(&shard, wolf, || {
        health(&shard, CHARACTER) < CHARACTER_HEALTH
    });

    assert!(health(&shard, CHARACTER) < CHARACTER_HEALTH);
    assert_eq!(pending_casts(&shard)[0]["scheduled_at"], due);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn damage_pushes_a_cast_back_a_second_and_then_less_with_each_hit() {
    let (shard, wolf) = stage("damage-pushes-back-cast");
    let events = subscribe_with_fence(&shard, &[CAST_EVENTS]);
    begin_cast(&shard, SLOW_CAST, wolf);
    // Three seconds into the cast, three pushbacks fit below the full cast time.
    for _ in 0..60 {
        shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    }

    for hits in 1..=3 {
        bite_until(&shard, wolf, || pushbacks(&shard, &events).len() == hits);
    }

    assert_eq!(pushbacks(&shard, &events), [1000, 800, 600]);
    assert_eq!(pending_casts(&shard).len(), 1);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hit_on_a_cast_with_its_full_cast_time_left_pushes_it_back_nothing() {
    let (shard, wolf) = stage("damage-pushback-capped");
    let events = subscribe_with_fence(&shard, &[CAST_EVENTS]);
    begin_cast(&shard, SLOW_CAST, wolf);

    bite_until(&shard, wolf, || !pushbacks(&shard, &events).is_empty());

    assert_eq!(pushbacks(&shard, &events), [0]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hit_a_shield_absorbs_whole_cancels_a_cast_that_damage_cancels() {
    let (shard, wolf) = stage("absorbed-hit-cancels-cast");
    let raised = raise_shield(&shard);
    begin_cast(&shard, DAMAGE_CANCELLED_CAST, wolf);

    bite_until(&shard, wolf, || pending_casts(&shard).is_empty());

    assert!(pending_casts(&shard).is_empty());
    assert!(shield_left(&shard) < raised, "a bite must land");
    assert_eq!(health(&shard, CHARACTER), CHARACTER_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hit_a_shield_absorbs_whole_pushes_back_no_cast() {
    let (shard, wolf) = stage("absorbed-hit-pushes-back-nothing");
    let events = subscribe_with_fence(&shard, &[CAST_EVENTS]);
    let raised = raise_shield(&shard);
    begin_cast(&shard, SLOW_CAST, wolf);
    for _ in 0..60 {
        shard.assert_call("debug_advance_virtual_clock", &["50000"]);
    }

    bite_until(&shard, wolf, || shield_left(&shard) < raised);

    assert!(shield_left(&shard) < raised, "a bite must land");
    assert_eq!(health(&shard, CHARACTER), CHARACTER_HEALTH);
    assert_eq!(pending_casts(&shard).len(), 1);
    assert!(pushbacks(&shard, &events).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hit_a_shield_absorbs_whole_breaks_an_aura_that_damage_breaks() {
    let (shard, wolf) = stage("absorbed-hit-breaks-aura");
    let raised = raise_shield(&shard);
    root_character(&shard, wolf, DAMAGE_BROKEN_ROOT);

    bite_until(&shard, wolf, || {
        roots(&shard, DAMAGE_BROKEN_ROOT).is_empty()
    });

    assert!(roots(&shard, DAMAGE_BROKEN_ROOT).is_empty());
    assert!(shield_left(&shard) < raised, "a bite must land");
    assert_eq!(health(&shard, CHARACTER), CHARACTER_HEALTH);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_hit_a_shield_absorbs_whole_leaves_a_root_whose_spell_has_proc_flags() {
    let (shard, wolf) = stage("absorbed-hit-keeps-proc-root");
    let raised = raise_shield(&shard);
    root_character(&shard, wolf, PROC_ROOT);

    bite_until(&shard, wolf, || shield_left(&shard) < raised);

    assert!(shield_left(&shard) < raised, "a bite must land");
    assert_eq!(roots(&shard, PROC_ROOT).len(), 1);
}

fn stage(name: &str) -> (Standalone, u64) {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard.assert_call("debug_seed_scenario_fixtures", &[]);
    shard.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);
    shard.assert_call("debug_start_virtual_clock", &["1800000000000000"]);
    shard.assert_call(
        "debug_spawn_at_feet",
        &[&CHARACTER.to_string(), &TEST_WOLF_ENTRY.to_string(), "1.0"],
    );
    shard.clear_schedule("game_creature_move_schedule");
    shard.clear_schedule("game_melee_schedule");
    let wolf = shard
        .query_rows(&format!(
            "SELECT guid FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = false, level = 60, \
         health = {CHARACTER_HEALTH}, max_health = {CHARACTER_HEALTH} WHERE guid = {CHARACTER}"
    ));
    shard.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, level = 60, \
         health = 100000, max_health = 100000 WHERE guid = {wolf}"
    ));
    hostile_factions(&shard, wolf);
    let channelled = CAST_FLAG_IGNORE_RESISTANCES | CAST_FLAG_CHANNELED;
    let channel_aura_effect: &[Field] = &[
        ("kind", &A_PERIODIC_DAMAGE),
        ("target", &T_TARGET_ENEMY),
        ("base_points", &10),
        ("period_ms", &PERIOD_MS),
    ];
    insert_drain_life_channel(
        &shard,
        CHANNEL_AURA,
        CHANNEL_DURATION_MS,
        channel_aura_effect,
    );
    insert_drain_life_channel(&shard, SHORT_CHANNEL, SHORT_CHANNEL_MS, channel_aura_effect);
    insert_spell(
        &shard,
        DAMAGE_CANCELLED_CHANNEL,
        &[
            ("duration_ms", &CHANNEL_DURATION_MS),
            ("cast_flags", &channelled),
            ("channel_interrupt_flags", &FIRST_AID_CHANNEL_FLAGS),
            ("prevention_type", &PREVENTION_SILENCE),
        ],
        channel_aura_effect,
    );
    insert_spell(
        &shard,
        UNSILENCEABLE_CHANNEL,
        &[
            ("duration_ms", &CHANNEL_DURATION_MS),
            ("cast_flags", &channelled),
            ("channel_interrupt_flags", &DRAIN_LIFE_CHANNEL_FLAGS),
            ("prevention_type", &PREVENTION_NONE),
        ],
        channel_aura_effect,
    );
    insert_drain_life_channel(
        &shard,
        CHANNELLED_AREA,
        CHANNEL_DURATION_MS,
        &[
            ("kind", &E_PERSISTENT_AREA),
            ("target", &T_SELF),
            ("base_points", &10),
            ("period_ms", &PERIOD_MS),
            ("radius_yd", &10.0),
        ],
    );
    let damage_effect: &[Field] = &[
        ("kind", &E_DAMAGE),
        ("target", &T_TARGET_ENEMY),
        ("base_points", &10),
    ];
    insert_spell(
        &shard,
        SLOW_CAST,
        &[
            ("cast_time_ms", &SLOW_CAST_MS),
            ("interrupt_flags", &FIREBALL_INTERRUPT_FLAGS),
            ("prevention_type", &PREVENTION_SILENCE),
        ],
        damage_effect,
    );
    insert_spell(
        &shard,
        UNPUSHED_CAST,
        &[
            ("cast_time_ms", &10_000),
            ("interrupt_flags", &WITHOUT_PUSHBACK_INTERRUPT_FLAGS),
            ("prevention_type", &PREVENTION_SILENCE),
        ],
        damage_effect,
    );
    insert_spell(
        &shard,
        DAMAGE_CANCELLED_CAST,
        &[
            ("cast_time_ms", &SLOW_CAST_MS),
            ("interrupt_flags", &HEARTHSTONE_INTERRUPT_FLAGS),
            ("prevention_type", &PREVENTION_NONE),
        ],
        damage_effect,
    );
    insert_spell(
        &shard,
        DAMAGE_BROKEN_ROOT,
        &[
            ("duration_ms", &CHANNEL_DURATION_MS),
            ("aura_interrupt", &BREAKS_ON_DAMAGE),
        ],
        &[
            ("kind", &A_CONTROL),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &M_ROOT),
            ("p_0_kind", &P_MECHANIC),
        ],
    );
    insert_spell(
        &shard,
        PROC_ROOT,
        &[
            ("duration_ms", &CHANNEL_DURATION_MS),
            ("aura_interrupt", &BREAKS_ON_DAMAGE),
            ("proc_flags", &TAKEN_MELEE_PROC_FLAGS),
        ],
        &[
            ("kind", &A_CONTROL),
            ("target", &T_TARGET_ENEMY),
            ("p_0", &M_ROOT),
            ("p_0_kind", &P_MECHANIC),
        ],
    );
    insert_spell(&shard, BITE, &[], damage_effect);
    insert_spell(
        &shard,
        KICK,
        &[("duration_ms", &KICK_LOCKOUT_MS)],
        &[("kind", &E_INTERRUPT), ("target", &T_TARGET_ENEMY)],
    );
    (shard, wolf)
}

// An enemy spell picks only hostile units, and the bare Standalone has no faction rows.
fn hostile_factions(shard: &Standalone, wolf: u64) {
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
    for (guid, faction) in [(CHARACTER, CHARACTER_FACTION), (wolf, WOLF_FACTION)] {
        shard.assert_sql(&format!(
            "UPDATE game_world_entity SET faction_template = {faction} WHERE guid = {guid}"
        ));
    }
}

// A zero range skips the range check and the projectile flight, so a direct hit lands in its cast.
fn insert_spell(shard: &Standalone, spell_id: u32, spell: &[Field], effect: &[Field]) {
    let name = format!("Interrupt fixture {spell_id}");
    let mut spell_row: Vec<Field> = vec![
        ("spell_id", &spell_id),
        ("name", &name),
        ("school_mask", &FIRE),
        ("range_yd", &0),
        ("is_negative", &true),
        ("gcd_ms", &0),
        ("cooldown_ms", &0),
    ];
    if !spell.iter().any(|(column, _)| *column == "cast_flags") {
        spell_row.push(("cast_flags", &CAST_FLAG_IGNORE_RESISTANCES));
    }
    spell_row.extend_from_slice(spell);
    shard.insert_row("game_spell", &spell_row);
    let effect_id = packed_spell_effect_id(spell_id, 0);
    let mut effect_row: Vec<Field> = vec![("id", &effect_id), ("spell_id", &spell_id)];
    effect_row.extend_from_slice(effect);
    shard.insert_row("game_spell_effect", &effect_row);
}

fn insert_drain_life_channel(
    shard: &Standalone,
    spell_id: u32,
    duration_ms: u32,
    effect: &[Field],
) {
    insert_spell(
        shard,
        spell_id,
        &[
            ("duration_ms", &duration_ms),
            (
                "cast_flags",
                &(CAST_FLAG_IGNORE_RESISTANCES | CAST_FLAG_CHANNELED),
            ),
            ("interrupt_flags", &FIREBALL_INTERRUPT_FLAGS),
            ("channel_interrupt_flags", &DRAIN_LIFE_CHANNEL_FLAGS),
            ("prevention_type", &PREVENTION_SILENCE),
        ],
        effect,
    );
}

fn roots(shard: &Standalone, spell_id: u32) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT id FROM game_aura WHERE target_guid = {CHARACTER} AND spell_id = {spell_id}"
    ))
}

fn root_character(shard: &Standalone, wolf: u64, spell_id: u32) {
    cast_until(shard, wolf, spell_id, CHARACTER, || {
        !roots(shard, spell_id).is_empty()
    });
    assert!(!roots(shard, spell_id).is_empty(), "the root must land");
    // An Aura survives damage in the Durable Request that applies it.
    shard.assert_call("debug_advance_virtual_clock", &["50000"]);
}

fn raise_shield(shard: &Standalone) -> i32 {
    shard.insert_row(
        "game_spell",
        &[
            ("spell_id", &SHIELD),
            ("name", &"Interrupt fixture shield"),
            ("duration_ms", &600_000),
            ("spell_level", &1),
            ("gcd_ms", &0),
        ],
    );
    shard.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(SHIELD, 0)),
            ("spell_id", &SHIELD),
            ("kind", &A_ABSORB),
            ("base_points", &SHIELD_POINTS),
            ("target", &T_SELF),
            ("p_0", &ALL_SCHOOLS),
            ("p_0_kind", &P_SCHOOL_MASK),
        ],
    );
    shard.assert_call(
        "debug_force_cast",
        &[&CHARACTER.to_string(), &SHIELD.to_string()],
    );
    let raised = shield_left(shard);
    assert!(raised > 0, "the shield must be up");
    raised
}

fn shield_left(shard: &Standalone) -> i32 {
    number(
        &shard.query_rows(&format!(
            "SELECT amount_remaining FROM game_aura WHERE target_guid = {CHARACTER} \
             AND spell_id = {SHIELD}"
        ))[0],
        "amount_remaining",
    )
}

fn pushbacks(shard: &Standalone, events: &TableSubscription) -> Vec<u32> {
    fence_subscription(shard, events);
    event_rows(events, "game_spell_cast_event_live")
        .into_iter()
        .filter(|row| row["kind"] == PUSHBACK)
        .map(|row| number(&row, "delay_ms"))
        .collect()
}

// A miss changes nothing, so a cast repeats until its outcome shows.
fn cast_until(
    shard: &Standalone,
    caster: u64,
    spell_id: u32,
    target: u64,
    landed: impl Fn() -> bool,
) {
    for _ in 0..CAST_ATTEMPTS {
        shard.assert_call(
            "debug_force_cast_at",
            &[
                &caster.to_string(),
                &spell_id.to_string(),
                &target.to_string(),
            ],
        );
        if landed() {
            return;
        }
    }
}

fn cast_area(shard: &Standalone) {
    let row = shard
        .query_rows(&format!(
            "SELECT x, y, z FROM game_world_entity WHERE guid = {CHARACTER}"
        ))
        .remove(0);
    shard.assert_call(
        "debug_cast_spell_at",
        &[
            &CHARACTER.to_string(),
            &CHANNELLED_AREA.to_string(),
            "0",
            &number::<f32>(&row, "x").to_string(),
            &number::<f32>(&row, "y").to_string(),
            &number::<f32>(&row, "z").to_string(),
        ],
    );
}

fn begin_cast(shard: &Standalone, spell_id: u32, target: u64) {
    shard.assert_call(
        "debug_begin_cast",
        &[
            &CHARACTER.to_string(),
            &spell_id.to_string(),
            &target.to_string(),
        ],
    );
    assert_eq!(
        pending_casts(shard).len(),
        1,
        "the cast must be in progress"
    );
}

// A miss deals no damage, so the wolf bites the Character until the hit shows.
fn bite_until(shard: &Standalone, wolf: u64, landed: impl Fn() -> bool) {
    cast_until(shard, wolf, BITE, CHARACTER, landed);
}

fn channel_auras(shard: &Standalone) -> Vec<Row> {
    auras_of(shard, CHANNEL_AURA)
}

fn auras_of(shard: &Standalone, spell_id: u32) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT id, expires_at FROM game_aura WHERE caster_guid = {CHARACTER} AND spell_id = {spell_id}"
    ))
}

fn channel_end(shard: &Standalone, spell_id: u32) -> i64 {
    auras_of(shard, spell_id)
        .iter()
        .map(|row| timestamp(&row["expires_at"]))
        .max()
        .expect("the channel must run")
}

fn area_end(shard: &Standalone) -> i64 {
    let rows = shard.query_rows(&format!(
        "SELECT expires_at FROM game_ground_area WHERE caster_guid = {CHARACTER}"
    ));
    assert_eq!(rows.len(), 1, "the channelled area must run");
    timestamp(&rows[0]["expires_at"])
}

fn timestamp(value: &str) -> i64 {
    Timestamp::parse_from_rfc3339(value)
        .unwrap_or_else(|error| panic!("invalid durable timestamp {value:?}: {error}"))
        .to_micros_since_unix_epoch()
}

fn health(shard: &Standalone, guid: u64) -> u32 {
    number(
        &shard.query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {guid}"
        ))[0],
        "health",
    )
}

fn areas(shard: &Standalone) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT id FROM game_ground_area WHERE caster_guid = {CHARACTER}"
    ))
}

fn pending_casts(shard: &Standalone) -> Vec<Row> {
    shard.query_rows(&format!(
        "SELECT scheduled_id, scheduled_at FROM game_pending_cast WHERE caster_guid = {CHARACTER}"
    ))
}

fn locked_schools(shard: &Standalone) -> Vec<u8> {
    shard
        .query_rows(&format!(
            "SELECT school FROM game_school_lockout WHERE caster_guid = {CHARACTER}"
        ))
        .iter()
        .map(|row| number(row, "school"))
        .collect()
}
