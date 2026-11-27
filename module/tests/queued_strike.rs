mod support;

use lyracore_shared::spell::taxonomy::{
    A_INERT, CAST_FLAG_DISCOUNT_POWER_ON_MISS, E_NEXT_SWING, P_NONE, T_SELF, T_TARGET_ENEMY,
};

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use lyracore_package_delta::fixture_ids::{FIXTURE_BLADE, TEST_WOLF_ENTRY};
use lyracore_package_delta::packed_spell_effect_id;
use support::{event_rows, poll_until, Standalone, TableSubscription, POLL_TIMEOUT};

type SqlRow = BTreeMap<String, String>;

const PLAYER: u64 = 1;
const HEROIC_STRIKE: u32 = 78;
const CLEAVE: u32 = 845;
const RAPTOR_STRIKE: u32 = 2973;
const BATTLE_SHOUT: u32 = 6673;
const HEROIC_STRIKE_COST: u32 = 150;
const CLEAVE_COST: u32 = 200;
const RAPTOR_STRIKE_COST: u32 = 15;
const RAPTOR_STRIKE_COOLDOWN_MS: u32 = 6000;
const BATTLE_SHOUT_COST: u32 = 100;
const MAUL: u32 = 6807;
const MAUL_COST: u32 = 150;
// Spell.dbc Stances bits for Bear Form (5) and Dire Bear Form (8).
const BEAR_FORMS: u32 = (1 << 4) | (1 << 7);
const BEAR_FORM: u32 = 5487;
const DIRE_BEAR_FORM: u32 = 9634;
const NO_POWER: &str = "1";
const INTERRUPTED: &str = "2";
const RAGE: u32 = 1;
const MANA: u32 = 0;
const PROC_WATCH: u32 = 5_131_703;
const WATCH_CHARGES: u8 = 3;
const WATCH_DURATION_MS: u32 = 600_000;
// vmangos PROC_FLAG_TAKE_MELEE_ABILITY.
const TAKEN_MELEE_SPELL_HIT: u32 = 0x20;
// Spell.dbc Cleave 845: EffectChainTarget 2, no effect radius, the 5 yd melee range.
const CLEAVE_CHAIN_TARGETS: u8 = 2;
// vmangos SPELL_DAMAGE_CLASS_MELEE: a melee chain gathers within its radius alone.
const DAMAGE_CLASS_MELEE: u8 = 2;
// The victim at the usual 1 yd, a wolf 3 yd past it and one 6.5 yd past it, beyond Cleave's 5 yd.
const CLEAVE_WOLF_OFFSETS: [&str; 3] = ["1.0", "4.0", "7.5"];
const WOLF_HEALTH: u32 = 100_000;
const DODGE: &str = "3";
// A level 63 wolf gives the fixture blade's unskilled swing the 6000 basis point miss cap and then
// a dodge band of over 3000, so this table draw is a dodge.
const DODGE_DRAW: u32 = 7000;

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_queued_strike_fizzles_into_a_white_swing_when_the_rage_is_gone() {
    let mut standalone = Standalone::start("queued-strike-fizzle");
    standalone.publish_module();
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike::rage(HEROIC_STRIKE, "Heroic Strike", HEROIC_STRIKE_COST),
    );
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike::rage(CLEAVE, "Cleave", CLEAVE_COST),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell SET cost = {BATTLE_SHOUT_COST} WHERE spell_id = {BATTLE_SHOUT}"
    ));

    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let wolf = spawn_target(&standalone);
    engage(&standalone, wolf);

    stage_power(&standalone, HEROIC_STRIKE_COST);
    cast(&standalone, HEROIC_STRIKE, wolf);
    cast(&standalone, BATTLE_SHOUT, PLAYER);
    let staged = player(&standalone);
    assert_eq!(queued_spell(&staged), HEROIC_STRIKE.to_string());
    let rage_at_the_swing = power(&staged);
    assert!(
        rage_at_the_swing < HEROIC_STRIKE_COST,
        "the shout must leave the Warrior short of Heroic Strike's cost, had {rage_at_the_swing}"
    );

    let (casts, landed) = swing(&standalone);

    let after = player(&standalone);
    assert!(
        power(&after) >= rage_at_the_swing,
        "the fizzle must not charge the strike; rage went {rage_at_the_swing} -> {}",
        power(&after)
    );
    assert_eq!(
        queued_spell(&after),
        "0",
        "the queued strike must be cleared, so the ability can be queued again"
    );
    assert!(
        casts.iter().any(|row| is_power_failure(row, HEROIC_STRIKE)),
        "the caster must be told the strike failed for lack of rage, got {casts:?}"
    );
    assert!(
        !casts.iter().any(|row| fired(row, HEROIC_STRIKE)),
        "a fizzled strike must not report a cast, got {casts:?}"
    );
    assert_eq!(
        landed["spell_swing"], "false",
        "the attack must resolve as an ordinary white swing"
    );

    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {wolf}"
    ));

    stage_power(&standalone, CLEAVE_COST);
    cast(&standalone, CLEAVE, wolf);
    let (casts, landed) = swing(&standalone);

    let after = player(&standalone);
    assert_eq!(
        power(&after),
        0,
        "an exactly affordable strike spends it all"
    );
    assert_eq!(queued_spell(&after), "0");
    assert!(
        casts.iter().any(|row| fired(row, CLEAVE)),
        "rage equal to the cost must fire the strike, got {casts:?}"
    );
    assert!(
        !casts.iter().any(|row| is_power_failure(row, CLEAVE)),
        "an affordable strike must not report a failure, got {casts:?}"
    );
    assert_eq!(
        landed["spell_swing"], "true",
        "a fired strike replaces the white swing"
    );

    stage_power(&standalone, CLEAVE_COST);
    cast(&standalone, CLEAVE, wolf);
    stage_power(&standalone, CLEAVE_COST - 1);
    let (casts, landed) = swing(&standalone);

    let after = player(&standalone);
    assert_eq!(
        power(&after),
        CLEAVE_COST - 1,
        "a fizzle leaves the pool exactly as it found it"
    );
    assert_eq!(queued_spell(&after), "0");
    assert!(
        casts.iter().any(|row| is_power_failure(row, CLEAVE)),
        "one point short must fizzle with the same failure, got {casts:?}"
    );
    assert!(
        !casts.iter().any(|row| fired(row, CLEAVE)),
        "a fizzled strike must not report a cast, got {casts:?}"
    );
    assert_eq!(landed["spell_swing"], "false");

    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = false WHERE guid = {wolf}"
    ));

    let after = landed_swing(&standalone, || stage_power(&standalone, 0));
    assert!(
        power(&after) > 0,
        "a landed white swing must reward rage, so the strike check below is not vacuous"
    );

    let after = landed_swing(&standalone, || {
        stage_power(&standalone, HEROIC_STRIKE_COST);
        cast(&standalone, HEROIC_STRIKE, wolf);
    });
    assert_eq!(
        power(&after),
        0,
        "the landed strike must spend its cost and reward no rage"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_queued_raptor_strike_spends_mana_and_starts_its_cooldown_when_the_swing_fires() {
    let mut standalone = Standalone::start("queued-raptor-strike");
    standalone.publish_module();
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike {
            power_type: MANA,
            cooldown_ms: RAPTOR_STRIKE_COOLDOWN_MS,
            ..QueuedStrike::rage(RAPTOR_STRIKE, "Raptor Strike", RAPTOR_STRIKE_COST)
        },
    );
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let wolf = spawn_target(&standalone);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {wolf}"
    ));
    engage(&standalone, wolf);

    stage_mana(&standalone, 100);
    cast(&standalone, RAPTOR_STRIKE, wolf);
    let queued = player(&standalone);
    assert_eq!(queued_spell(&queued), RAPTOR_STRIKE.to_string());
    assert_eq!(power(&queued), 100, "queuing must not spend the mana");
    assert!(
        raptor_strike_cooldown(&standalone).is_empty(),
        "the cooldown must wait for the swing"
    );

    let (casts, landed) = swing(&standalone);

    let after = player(&standalone);
    assert_eq!(power(&after), 100 - RAPTOR_STRIKE_COST);
    assert_eq!(queued_spell(&after), "0");
    assert_eq!(landed["spell_swing"], "true");
    let fired_row = casts
        .iter()
        .find(|row| fired(row, RAPTOR_STRIKE))
        .unwrap_or_else(|| panic!("the swing must fire Raptor Strike, got {casts:?}"));
    assert_eq!(
        fired_row["cooldown_ms"],
        RAPTOR_STRIKE_COOLDOWN_MS.to_string(),
        "the client learns the cooldown from the fired strike"
    );
    assert_eq!(raptor_strike_cooldown(&standalone).len(), 1);
    let fired_ms = spacetimedb::Timestamp::parse_from_rfc3339(&fired_row["created_at"])
        .unwrap()
        .to_micros_since_unix_epoch()
        / 1000;
    assert_eq!(
        after["mana_regen_paused_until_ms"],
        (fired_ms + 5000).to_string(),
        "spending mana pauses mana regeneration for five seconds"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_queued_strike_raises_the_victims_melee_ability_proc_and_a_white_swing_does_not() {
    let mut standalone = Standalone::start("queued-strike-ability-proc");
    standalone.publish_module();
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike::rage(HEROIC_STRIKE, "Heroic Strike", HEROIC_STRIKE_COST),
    );
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let wolf = spawn_target(&standalone);
    engage(&standalone, wolf);
    stage_proc_watch(&standalone, wolf);
    assert_eq!(watch_charges(&standalone, wolf), Some(WATCH_CHARGES));

    landed_swing(&standalone, || stage_power(&standalone, 0));
    assert_eq!(
        watch_charges(&standalone, wolf),
        Some(WATCH_CHARGES),
        "a white swing raises no melee-ability event"
    );

    for _ in 0..20 {
        stage_power(&standalone, HEROIC_STRIKE_COST);
        cast(&standalone, HEROIC_STRIKE, wolf);
        let (_, landed) = swing(&standalone);
        assert_eq!(landed["spell_swing"], "true");
        if landed["damage"] != "0" {
            break;
        }
    }
    assert_eq!(
        watch_charges(&standalone, wolf),
        Some(WATCH_CHARGES - 1),
        "vmangos gives a queued strike's victim PROC_FLAG_TAKE_MELEE_ABILITY"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_dodged_heroic_strike_gives_back_82_percent_of_its_rage() {
    let mut standalone = Standalone::start("queued-strike-dodge-refund");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike::rage(HEROIC_STRIKE, "Heroic Strike", HEROIC_STRIKE_COST),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell SET cast_flags = {CAST_FLAG_DISCOUNT_POWER_ON_MISS} \
         WHERE spell_id = {HEROIC_STRIKE}"
    ));
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_call("debug_equip_weapon", &["1", &FIXTURE_BLADE.to_string()]);
    let wolf = spawn_target(&standalone);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET level = 63 WHERE guid = {wolf}"
    ));
    engage(&standalone, wolf);
    stage_power(&standalone, 1000);
    cast(&standalone, HEROIC_STRIKE, wolf);
    standalone.assert_call(
        "debug_script_swing_draws",
        &["1", &format!("[{DODGE_DRAW}]")],
    );

    let (_, landed) = swing(&standalone);

    assert_eq!(landed["spell_swing"], "true", "{landed:?}");
    assert_eq!(landed["hit_info"], DODGE, "{landed:?}");
    assert_eq!(
        power(&player(&standalone)),
        1000 - HEROIC_STRIKE_COST + 123,
        "vmangos gives back lroundf(150 * 0.82) rage on a dodge"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_queued_cleave_strikes_its_victim_and_the_enemy_in_reach_but_not_one_beyond() {
    let mut standalone = Standalone::start("queued-cleave-chain");
    standalone.publish_module();
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike::rage(CLEAVE, "Cleave", CLEAVE_COST),
    );
    standalone.assert_sql(&format!(
        "UPDATE game_spell SET damage_class = {DAMAGE_CLASS_MELEE} WHERE spell_id = {CLEAVE}"
    ));
    set_cleave_chain(&standalone, CLEAVE_CHAIN_TARGETS);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let [victim, near, far] =
        CLEAVE_WOLF_OFFSETS.map(|offset| spawn_target_at(&standalone, offset));
    engage(&standalone, victim);
    assert_eq!(
        attack_target(&standalone),
        Some(victim),
        "the Character must be attacking the victim"
    );

    for _ in 0..20 {
        let casts = cleave_swing(&standalone, victim);
        assert!(
            chain_logged(&casts, near),
            "every fired Cleave must reach the wolf 3 yd past its victim, got {casts:?}"
        );
        assert!(
            !chain_logged(&casts, far),
            "the wolf 6.5 yd past the victim is beyond Cleave's 5 yd, got {casts:?}"
        );
        if wolf_health(&standalone, near) < WOLF_HEALTH {
            break;
        }
    }
    assert_eq!(
        [victim, near, far].map(|wolf| wolf_health(&standalone, wolf) < WOLF_HEALTH),
        [true, true, false],
        "vmangos casts the queued Cleave at the victim; its target 6 chain gathers melee units \
         within 5 yd of it"
    );

    set_cleave_chain(&standalone, 3);
    let casts = cleave_swing(&standalone, victim);
    assert!(
        chain_logged(&casts, near) && !chain_logged(&casts, far),
        "a third jump could reach the far wolf 3.5 yd from the near one, but vmangos gathers a \
         melee chain only within 5 yd of the victim, got {casts:?}"
    );
}

fn set_cleave_chain(standalone: &Standalone, chain_targets: u8) {
    standalone.assert_sql(&format!(
        "UPDATE game_spell_effect SET chain_targets = {chain_targets} WHERE spell_id = {CLEAVE}"
    ));
}

/// Queues Cleave at `victim`, swings once and returns the swing's cast events.
fn cleave_swing(standalone: &Standalone, victim: u64) -> Vec<SqlRow> {
    stage_power(standalone, CLEAVE_COST);
    cast(standalone, CLEAVE, victim);
    assert_eq!(
        queued_spell(&player(standalone)),
        CLEAVE.to_string(),
        "the cast must queue Cleave on the next swing"
    );
    let (casts, landed) = swing(standalone);
    assert_eq!(
        landed["spell_swing"], "true",
        "the swing must fire the queued Cleave, got {landed:?}"
    );
    assert!(
        casts.iter().any(|row| fired(row, CLEAVE)),
        "the fired Cleave must report its GO, got {casts:?}"
    );
    casts
}

// A chain unit's Cleave log row, written whether its own roll hit or missed.
fn chain_logged(casts: &[SqlRow], unit: u64) -> bool {
    casts.iter().any(|row| {
        row["spell_id"] == CLEAVE.to_string()
            && row["is_proc_log"] == "true"
            && row["target_guid"] == unit.to_string()
    })
}

fn attack_target(standalone: &Standalone) -> Option<u64> {
    standalone
        .query_rows(&format!(
            "SELECT target_guid FROM game_melee_attack WHERE attacker_guid = {PLAYER}"
        ))
        .first()
        .and_then(|row| row["target_guid"].parse().ok())
}

fn wolf_health(standalone: &Standalone, wolf: u64) -> u32 {
    standalone
        .query_rows(&format!(
            "SELECT health FROM game_world_entity WHERE guid = {wolf}"
        ))
        .first()
        .and_then(|row| row["health"].parse().ok())
        .expect("the wolf must be live")
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn leaving_bear_form_interrupts_a_queued_maul() {
    let mut standalone = Standalone::start("queued-maul-stance");
    standalone.publish_module();
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike {
            form_mask: BEAR_FORMS,
            ..QueuedStrike::rage(MAUL, "Maul", MAUL_COST)
        },
    );
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("debug_seed_druid_forms", &[]);
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let wolf = spawn_target(&standalone);
    engage(&standalone, wolf);

    shift(&standalone, BEAR_FORM);
    stage_power(&standalone, MAUL_COST);
    cast(&standalone, MAUL, wolf);
    assert_eq!(queued_spell(&player(&standalone)), MAUL.to_string());

    assert_form_change_interrupts_maul(&standalone, BEAR_FORM, None);

    shift(&standalone, BEAR_FORM);
    stage_power(&standalone, MAUL_COST);
    let (casts, landed) = swing(&standalone);

    assert!(
        power(&player(&standalone)) >= MAUL_COST,
        "the interrupted Maul must not spend rage on the next swing"
    );
    assert!(
        !casts.iter().any(|row| fired(row, MAUL)),
        "returning to Bear Form must not revive the old Maul, got {casts:?}"
    );
    assert_eq!(landed["spell_swing"], "false");

    stage_power(&standalone, MAUL_COST);
    cast(&standalone, MAUL, wolf);
    assert_form_change_interrupts_maul(&standalone, BEAR_FORM, Some(DIRE_BEAR_FORM));
}

/// Casts a form spell once its global cooldown has passed.
fn shift(standalone: &Standalone, form: u32) {
    standalone.assert_sql(&format!(
        "DELETE FROM game_global_cooldown WHERE caster_guid = {PLAYER}"
    ));
    cast(standalone, form, PLAYER);
}

/// Changes Form as the 1.12 client does: `CMSG_CANCEL_AURA` leaves `from`, then `to` is cast.
fn assert_form_change_interrupts_maul(standalone: &Standalone, from: u32, to: Option<u32>) {
    assert_eq!(queued_spell(&player(standalone)), MAUL.to_string());
    let events =
        standalone.subscribe(&["SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1"]);
    standalone.assert_call(
        "debug_shed_spell_auras",
        &[&PLAYER.to_string(), &from.to_string()],
    );
    if let Some(to) = to {
        shift(standalone, to);
    }
    assert!(
        poll_until(POLL_TIMEOUT, || cast_events(&events).iter().any(|row| {
            row["spell_id"] == MAUL.to_string()
                && row["is_interrupted"] == "true"
                && row["failure_reason"] == INTERRUPTED
        })),
        "leaving Form spell {from} must interrupt Maul, got {:?}",
        cast_events(&events)
    );
    assert_eq!(queued_spell(&player(standalone)), "0");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn death_drops_a_queued_raptor_strike() {
    let mut standalone = Standalone::start("queued-strike-death");
    standalone.publish_module();
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike {
            power_type: MANA,
            ..QueuedStrike::rage(RAPTOR_STRIKE, "Raptor Strike", RAPTOR_STRIKE_COST)
        },
    );
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    let wolf = spawn_target(&standalone);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {wolf}"
    ));
    engage(&standalone, wolf);
    stage_mana(&standalone, 100);
    cast(&standalone, RAPTOR_STRIKE, wolf);
    assert_eq!(
        queued_spell(&player(&standalone)),
        RAPTOR_STRIKE.to_string()
    );

    hold_the_swing(&standalone);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET health = 1 WHERE guid = {PLAYER}"
    ));
    assert!(
        poll_until(POLL_TIMEOUT, || {
            hold_the_swing(&standalone);
            standalone.query_rows(&format!(
                "SELECT dead FROM game_world_entity WHERE guid = {PLAYER}"
            ))[0]["dead"]
                == "true"
        }),
        "the wolf must kill the Character"
    );
    assert_eq!(queued_spell(&player(&standalone)), "0");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_queued_raptor_strike_ends_with_its_attack() {
    let mut standalone = Standalone::start("queued-strike-attack-end");
    standalone.publish_module();
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike {
            power_type: MANA,
            cooldown_ms: RAPTOR_STRIKE_COOLDOWN_MS,
            ..QueuedStrike::rage(RAPTOR_STRIKE, "Raptor Strike", RAPTOR_STRIKE_COST)
        },
    );
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let wolf = spawn_target(&standalone);
    engage(&standalone, wolf);
    stage_mana(&standalone, 100);
    cast(&standalone, RAPTOR_STRIKE, wolf);
    assert_eq!(
        queued_spell(&player(&standalone)),
        RAPTOR_STRIKE.to_string()
    );

    let events =
        standalone.subscribe(&["SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET dead = true, health = 0 WHERE guid = {wolf}"
    ));

    assert!(
        poll_until(POLL_TIMEOUT, || cast_events(&events).iter().any(|row| {
            row["spell_id"] == RAPTOR_STRIKE.to_string()
                && row["is_interrupted"] == "true"
                && row["failure_reason"] == INTERRUPTED
        })),
        "the attack ending on a dead target must interrupt the strike, got {:?}",
        cast_events(&events)
    );
    let after = player(&standalone);
    assert_eq!(queued_spell(&after), "0");
    assert_eq!(power(&after), 100, "an interrupted strike costs nothing");
    assert!(raptor_strike_cooldown(&standalone).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn ending_auto_shot_keeps_a_queued_raptor_strike() {
    const AUTO_SHOT: u32 = 75;
    let mut standalone = Standalone::start("queued-strike-auto-shot");
    standalone.publish_module();
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike {
            power_type: MANA,
            ..QueuedStrike::rage(RAPTOR_STRIKE, "Raptor Strike", RAPTOR_STRIKE_COST)
        },
    );
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let wolf = spawn_target(&standalone);
    engage(&standalone, wolf);
    stage_mana(&standalone, 100);
    cast(&standalone, RAPTOR_STRIKE, wolf);

    hold_the_swing(&standalone);
    standalone.assert_sql(&format!(
        "UPDATE game_melee_attack SET ranged_spell_id = {AUTO_SHOT} WHERE attacker_guid = {PLAYER}"
    ));
    assert!(
        poll_until(POLL_TIMEOUT, || standalone
            .query_rows(&format!(
                "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {PLAYER}"
            ))
            .is_empty()),
        "Auto Shot without a ranged weapon must end its loop"
    );

    assert_eq!(
        queued_spell(&player(&standalone)),
        RAPTOR_STRIKE.to_string()
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_auto_shot_target_dying_interrupts_a_queued_raptor_strike() {
    const AUTO_SHOT: u32 = 75;
    const RANGED_SLOT: u8 = 17;
    let mut standalone = Standalone::start("queued-strike-auto-shot-death");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike {
            power_type: MANA,
            ..QueuedStrike::rage(RAPTOR_STRIKE, "Raptor Strike", RAPTOR_STRIKE_COST)
        },
    );
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    standalone.assert_call("debug_grant_item", &["1", &FIXTURE_BLADE.to_string(), "1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_item_instance SET slot = {RANGED_SLOT} \
         WHERE owner_guid = {PLAYER} AND entry = {FIXTURE_BLADE}"
    ));
    let wolf = spawn_target(&standalone);
    engage(&standalone, wolf);
    stage_mana(&standalone, 100);
    cast(&standalone, RAPTOR_STRIKE, wolf);

    hold_the_swing(&standalone);
    standalone.assert_sql(&format!(
        "UPDATE game_melee_attack SET ranged_spell_id = {AUTO_SHOT} WHERE attacker_guid = {PLAYER}"
    ));
    hold_the_swing(&standalone);
    let attack = standalone.query_rows(&format!(
        "SELECT ranged_spell_id FROM game_melee_attack WHERE attacker_guid = {PLAYER}"
    ));
    assert_eq!(attack[0]["ranged_spell_id"], AUTO_SHOT.to_string());
    assert_eq!(
        queued_spell(&player(&standalone)),
        RAPTOR_STRIKE.to_string()
    );

    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET dead = true, health = 0 WHERE guid = {wolf}"
    ));
    assert!(
        poll_until(POLL_TIMEOUT, || {
            hold_the_swing(&standalone);
            standalone
                .query_rows(&format!(
                    "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {PLAYER}"
                ))
                .is_empty()
        }),
        "the Auto Shot attack must end with its target"
    );
    assert_eq!(
        queued_spell(&player(&standalone)),
        "0",
        "the fight ending must interrupt the strike queued during Auto Shot"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn running_out_of_ammo_keeps_a_queued_raptor_strike() {
    const AUTO_SHOT: u32 = 75;
    const BOW: u8 = 2;
    const RANGED_SLOT: u8 = 17;
    let mut standalone = Standalone::start("queued-strike-no-ammo");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    stage_on_next_swing_spell(
        &standalone,
        QueuedStrike {
            power_type: MANA,
            ..QueuedStrike::rage(RAPTOR_STRIKE, "Raptor Strike", RAPTOR_STRIKE_COST)
        },
    );
    standalone.assert_sql(&format!(
        "UPDATE game_item_template SET subclass = {BOW} WHERE entry = {FIXTURE_BLADE}"
    ));
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    standalone.assert_call("debug_grant_item", &["1", &FIXTURE_BLADE.to_string(), "1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_item_instance SET slot = {RANGED_SLOT} \
         WHERE owner_guid = {PLAYER} AND entry = {FIXTURE_BLADE}"
    ));
    let wolf = spawn_target(&standalone);
    engage(&standalone, wolf);
    stage_mana(&standalone, 100);
    cast(&standalone, RAPTOR_STRIKE, wolf);

    hold_the_swing(&standalone);
    standalone.assert_sql(&format!(
        "UPDATE game_melee_attack SET ranged_spell_id = {AUTO_SHOT} WHERE attacker_guid = {PLAYER}"
    ));
    let wolf_row = standalone.query_rows(&format!(
        "SELECT x, y FROM game_world_entity WHERE guid = {wolf}"
    ));
    let wolf_x: f32 = wolf_row[0]["x"].parse().unwrap();
    let wolf_y: f32 = wolf_row[0]["y"].parse().unwrap();
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {}, y = {wolf_y}, orientation = {} WHERE guid = {PLAYER}",
        wolf_x + 15.0,
        std::f32::consts::PI
    ));
    let events =
        standalone.subscribe(&["SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1"]);
    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");

    assert!(
        poll_until(POLL_TIMEOUT, || standalone
            .query_rows(&format!(
                "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {PLAYER}"
            ))
            .is_empty()),
        "a bow without ammo must end its Auto Shot loop"
    );
    assert_eq!(
        queued_spell(&player(&standalone)),
        RAPTOR_STRIKE.to_string()
    );
    assert!(
        !cast_events(&events)
            .iter()
            .any(|row| row["spell_id"] == RAPTOR_STRIKE.to_string()),
        "running out of ammo must not interrupt the strike, got {:?}",
        cast_events(&events)
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_evading_creature_drops_its_queued_strike() {
    const STRIKE: u32 = 11976;
    let mut standalone = Standalone::start("queued-strike-evade");
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &["1"]);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET godmode = true WHERE guid = {PLAYER}"
    ));
    let wolf = spawn_target(&standalone);
    engage(&standalone, wolf);
    assert!(
        poll_until(POLL_TIMEOUT, || !wolf_melee(&standalone, wolf).is_empty()),
        "the wolf never fought back"
    );
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u32;
    standalone.assert_sql(&format!(
        "UPDATE game_melee_attack SET last_swing_ms = {now_ms} WHERE attacker_guid = {wolf}"
    ));
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET next_swing_spell = {STRIKE} WHERE guid = {wolf}"
    ));

    let x: f32 = standalone.query_rows(&format!(
        "SELECT x FROM game_world_entity WHERE guid = {PLAYER}"
    ))[0]["x"]
        .parse()
        .unwrap();
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET x = {} WHERE guid = {PLAYER}",
        x + 200.0
    ));

    assert!(
        poll_until(POLL_TIMEOUT, || wolf_melee(&standalone, wolf).is_empty()),
        "the wolf must evade once its target is beyond the combat backstop"
    );
    let queued = standalone.query_rows(&format!(
        "SELECT next_swing_spell FROM game_world_entity WHERE guid = {wolf}"
    ));
    assert_eq!(queued[0]["next_swing_spell"], "0");
}

fn wolf_melee(standalone: &Standalone, wolf: u64) -> Vec<SqlRow> {
    standalone.query_rows(&format!(
        "SELECT attacker_guid FROM game_melee_attack WHERE attacker_guid = {wolf}"
    ))
}

fn raptor_strike_cooldown(standalone: &Standalone) -> Vec<SqlRow> {
    standalone.query_rows(&format!(
        "SELECT ready_at FROM game_spell_cd WHERE caster_guid = {PLAYER} AND spell_id = {RAPTOR_STRIKE}"
    ))
}

fn landed_swing(standalone: &Standalone, mut stage: impl FnMut()) -> SqlRow {
    for _ in 0..20 {
        stage();
        let (_, landed) = swing(standalone);
        if landed["damage"] != "0" {
            return player(standalone);
        }
    }
    panic!("no swing landed in 20 attempts");
}

struct QueuedStrike {
    spell_id: u32,
    name: &'static str,
    power_type: u32,
    cost: u32,
    cooldown_ms: u32,
    form_mask: u32,
}

impl QueuedStrike {
    fn rage(spell_id: u32, name: &'static str, cost: u32) -> Self {
        Self {
            spell_id,
            name,
            power_type: RAGE,
            cost,
            cooldown_ms: 0,
            form_mask: 0,
        }
    }
}

fn stage_on_next_swing_spell(standalone: &Standalone, strike: QueuedStrike) {
    let QueuedStrike {
        spell_id,
        name,
        power_type,
        cost,
        cooldown_ms,
        form_mask,
    } = strike;
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &spell_id),
            ("name", &name),
            ("power_type", &power_type),
            ("cost", &cost),
            ("cooldown_ms", &cooldown_ms),
            ("range_yd", &5),
            ("school_mask", &1),
        ],
    );
    standalone.assert_call(
        "debug_set_spell_form_masks",
        &[&spell_id.to_string(), &form_mask.to_string(), "0"],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(spell_id, 0)),
            ("spell_id", &spell_id),
            ("kind", &E_NEXT_SWING),
            ("base_points", &11),
            ("target", &T_TARGET_ENEMY),
        ],
    );
}

fn stage_proc_watch(standalone: &Standalone, wolf: u64) {
    standalone.insert_row(
        "game_spell",
        &[
            ("spell_id", &PROC_WATCH),
            ("name", &"Proc watch fixture"),
            ("school_mask", &1),
            ("range_yd", &30),
            ("duration_ms", &WATCH_DURATION_MS),
            ("spell_level", &1),
            ("proc_flags", &TAKEN_MELEE_SPELL_HIT),
            ("proc_chance", &100),
            ("proc_charges", &WATCH_CHARGES),
        ],
    );
    standalone.insert_row(
        "game_spell_effect",
        &[
            ("id", &packed_spell_effect_id(PROC_WATCH, 0)),
            ("spell_id", &PROC_WATCH),
            ("effect_index", &0),
            ("kind", &A_INERT),
            ("base_points", &0),
            ("target", &T_SELF),
            ("p_0", &0),
            ("p_0_kind", &P_NONE),
        ],
    );
    standalone.assert_call(
        "debug_force_cast_at",
        &[
            &wolf.to_string(),
            &PROC_WATCH.to_string(),
            &wolf.to_string(),
        ],
    );
}

fn watch_charges(standalone: &Standalone, wolf: u64) -> Option<u8> {
    standalone
        .query_rows(&format!(
            "SELECT proc_charges FROM game_aura WHERE target_guid = {wolf} \
             AND spell_id = {PROC_WATCH}"
        ))
        .first()
        .map(|row| row["proc_charges"].parse().unwrap())
}

fn spawn_target(standalone: &Standalone) -> u64 {
    spawn_target_at(standalone, "1.0")
}

fn spawn_target_at(standalone: &Standalone, offset: &str) -> u64 {
    standalone.assert_call(
        "debug_spawn_at_feet",
        &["1", &TEST_WOLF_ENTRY.to_string(), offset],
    );
    let guid = standalone
        .query_rows(&format!(
            "SELECT guid, x FROM game_world_entity WHERE entry = {TEST_WOLF_ENTRY}"
        ))
        .iter()
        .filter_map(|row| row["guid"].parse::<u64>().ok())
        .max()
        .expect("the spawned wolf must exist");
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET simulation_indexed = false, health = 100000, max_health = 100000 WHERE guid = {guid}"
    ));
    guid
}

fn engage(standalone: &Standalone, target: u64) {
    let events =
        standalone.subscribe(&["SELECT * FROM game_combat_event_live WHERE attacker_guid = 1"]);
    standalone.assert_call("debug_engage", &["1", &target.to_string()]);
    assert!(
        poll_until(POLL_TIMEOUT, || !own_swings(&events).is_empty()),
        "the engagement never swung"
    );
}

fn stage_power(standalone: &Standalone, rage: u32) {
    hold_the_swing(standalone);
    standalone.assert_call("debug_set_power", &["1", &rage.to_string()]);
}

fn stage_mana(standalone: &Standalone, mana: u32) {
    let row = standalone
        .query_rows(&format!(
            "SELECT unit_bytes_0 FROM game_world_entity WHERE guid = {PLAYER}"
        ))
        .remove(0);
    let bytes: u32 = row["unit_bytes_0"]
        .parse()
        .expect("unit_bytes_0 is a number");
    let bytes = (bytes & 0x00FF_FFFF) | (MANA << 24);
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET unit_bytes_0 = {bytes}, max_power = {mana} WHERE guid = {PLAYER}"
    ));
    stage_power(standalone, mana);
}

fn cast(standalone: &Standalone, spell_id: u32, target: u64) {
    hold_the_swing(standalone);
    standalone.assert_call(
        "debug_cast_at",
        &["1", &spell_id.to_string(), &target.to_string()],
    );
}

fn swing(standalone: &Standalone) -> (Vec<SqlRow>, SqlRow) {
    hold_the_swing(standalone);
    let events = standalone.subscribe(&[
        "SELECT * FROM game_combat_event_live WHERE attacker_guid = 1",
        "SELECT * FROM game_spell_cast_event_live WHERE caster_guid = 1",
    ]);
    standalone.assert_sql("UPDATE game_melee_attack SET last_swing_ms = 1 WHERE attacker_guid = 1");
    assert!(
        poll_until(POLL_TIMEOUT, || !own_swings(&events).is_empty()),
        "the swing never fired"
    );
    hold_the_swing(standalone);
    (cast_events(&events), own_swing(&events))
}

fn hold_the_swing(standalone: &Standalone) {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u32;
    standalone.assert_sql(&format!(
        "UPDATE game_melee_attack SET last_swing_ms = {now_ms} WHERE attacker_guid = 1"
    ));
}

fn player(standalone: &Standalone) -> SqlRow {
    standalone
        .query_rows(&format!(
            "SELECT power, next_swing_spell, mana_regen_paused_until_ms FROM game_world_entity \
             WHERE guid = {PLAYER}"
        ))
        .pop()
        .expect("the player entity must be live")
}

fn power(row: &SqlRow) -> u32 {
    row["power"].parse().expect("power must be a number")
}

fn queued_spell(row: &SqlRow) -> String {
    row["next_swing_spell"].clone()
}

fn cast_events(events: &TableSubscription) -> Vec<SqlRow> {
    event_rows(events, "game_spell_cast_event_live")
}

fn is_power_failure(row: &SqlRow, spell_id: u32) -> bool {
    row["spell_id"] == spell_id.to_string()
        && row["is_interrupted"] == "true"
        && row["failure_reason"] == NO_POWER
}

fn fired(row: &SqlRow, spell_id: u32) -> bool {
    row["spell_id"] == spell_id.to_string() && row["is_completion"] == "true"
}

fn own_swings(events: &TableSubscription) -> Vec<SqlRow> {
    event_rows(events, "game_combat_event_live")
}

fn own_swing(events: &TableSubscription) -> SqlRow {
    own_swings(events)
        .pop()
        .expect("the swing must have been logged")
}
