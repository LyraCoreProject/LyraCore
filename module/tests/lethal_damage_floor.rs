mod support;

use std::thread;
use std::time::Duration;

use support::Standalone;

const PLAYER_GUID: u64 = 1;
const WOLF_ENTRY: u32 = 51_000;
const WOLF_GUID: u64 = (0xF130_u64 << 48) | ((WOLF_ENTRY as u64) << 24) | 1;
const SECOND_WOLF_GUID: u64 = (0xF130_u64 << 48) | ((WOLF_ENTRY as u64) << 24) | 2;
const THIRD_WOLF_GUID: u64 = (0xF130_u64 << 48) | ((WOLF_ENTRY as u64) << 24) | 3;
const SOURCE_RULE_ID: u64 = 900_001;

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn authoritative_damage_honors_the_lethal_floor_and_its_lifetime() {
    let standalone = Standalone::start("lethal-damage-floor");
    standalone.publish_module();
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER_GUID.to_string()]);
    let first_definition = definition("phase:1");
    install_definition(&standalone, WOLF_GUID, &first_definition);

    stage(&standalone, WOLF_GUID);
    standalone.assert_call(
        "debug_cast_at",
        &[&WOLF_GUID.to_string(), "50072", &WOLF_GUID.to_string()],
    );
    verify(&standalone, WOLF_GUID, 10, false, true, false, 50);

    apply(&standalone, WOLF_GUID, 55);
    verify(&standalone, WOLF_GUID, 10, false, true, false, 1);

    apply(&standalone, WOLF_GUID, 20);
    verify(&standalone, WOLF_GUID, 1, false, true, true, 0);
    apply(&standalone, WOLF_GUID, 20);
    verify(&standalone, WOLF_GUID, 1, false, true, true, 0);

    set_floor(&standalone, WOLF_GUID, true);
    verify(&standalone, WOLF_GUID, 1, false, true, false, 0);
    apply(&standalone, WOLF_GUID, 2);
    verify(&standalone, WOLF_GUID, 1, false, true, true, 0);
    standalone.assert_call(
        "debug_force_lethal_damage_floor_fixture",
        &[&WOLF_GUID.to_string()],
    );
    verify(&standalone, WOLF_GUID, 0, true, false, false, 0);

    standalone.assert_call(
        "debug_spawn_at_feet",
        &[&PLAYER_GUID.to_string(), &WOLF_ENTRY.to_string(), "1.0"],
    );
    stage(&standalone, SECOND_WOLF_GUID);
    let replacement = definition("phase:2");
    standalone.assert_call(
        "debug_replace_definition_then_apply_damage_floor_fixture",
        &[
            &PLAYER_GUID.to_string(),
            &SECOND_WOLF_GUID.to_string(),
            &replacement,
            "100",
        ],
    );
    verify(&standalone, SECOND_WOLF_GUID, 0, true, false, false, 0);

    standalone.assert_call(
        "debug_respawn_lethal_damage_floor_fixture",
        &[&SECOND_WOLF_GUID.to_string()],
    );
    verify(
        &standalone,
        SECOND_WOLF_GUID,
        u32::MAX,
        false,
        false,
        false,
        0,
    );

    stage(&standalone, SECOND_WOLF_GUID);
    standalone.assert_call(
        "debug_respawn_lethal_damage_floor_fixture",
        &[&SECOND_WOLF_GUID.to_string()],
    );
    verify(
        &standalone,
        SECOND_WOLF_GUID,
        u32::MAX,
        false,
        false,
        false,
        0,
    );

    stage(&standalone, SECOND_WOLF_GUID);
    set_floor(&standalone, SECOND_WOLF_GUID, false);
    verify(&standalone, SECOND_WOLF_GUID, 10, false, false, false, 0);
    apply(&standalone, SECOND_WOLF_GUID, 100);
    verify(&standalone, SECOND_WOLF_GUID, 0, true, false, false, 0);

    standalone.assert_call(
        "debug_spawn_at_feet",
        &[&PLAYER_GUID.to_string(), &WOLF_ENTRY.to_string(), "1.0"],
    );
    stage(&standalone, THIRD_WOLF_GUID);
    standalone.assert_call(
        "debug_stage_ranged_lethal_damage_floor_fixture",
        &[
            &PLAYER_GUID.to_string(),
            &THIRD_WOLF_GUID.to_string(),
            "20",
            "1000",
        ],
    );
    standalone.assert_call(
        "debug_set_lethal_damage_floor_health_fixture",
        &[&THIRD_WOLF_GUID.to_string(), "5"],
    );
    thread::sleep(Duration::from_millis(1_300));
    standalone.assert_call(
        "debug_verify_ranged_lethal_damage_floor_fixture",
        &[
            &PLAYER_GUID.to_string(),
            &THIRD_WOLF_GUID.to_string(),
            "1",
            "4",
        ],
    );
}

fn definition(instruction: &str) -> String {
    let rules = format!(
        "{SOURCE_RULE_ID},aggro,100,4294967295,once,all,ordinary,any-posture,{instruction}"
    );
    packed_definition(&rules)
}

fn install_definition(standalone: &Standalone, creature_guid: u64, packed: &str) {
    standalone.assert_call(
        "debug_stage_eventai_revision_fixture",
        &[&creature_guid.to_string(), &PLAYER_GUID.to_string(), packed],
    );
}

fn stage(standalone: &Standalone, creature_guid: u64) {
    standalone.assert_call(
        "debug_stage_lethal_damage_floor_fixture",
        &[&creature_guid.to_string()],
    );
}

fn set_floor(standalone: &Standalone, creature_guid: u64, enabled: bool) {
    standalone.assert_call(
        "debug_set_lethal_damage_floor_fixture",
        &[&creature_guid.to_string(), &enabled.to_string()],
    );
}

fn apply(standalone: &Standalone, target_guid: u64, rolled: u32) {
    standalone.assert_call(
        "debug_apply_lethal_damage_floor_fixture",
        &[
            &PLAYER_GUID.to_string(),
            &target_guid.to_string(),
            &rolled.to_string(),
        ],
    );
}

fn verify(
    standalone: &Standalone,
    creature_guid: u64,
    health: u32,
    dead: bool,
    floor: bool,
    notification: bool,
    absorb: i32,
) {
    standalone.assert_call(
        "debug_verify_lethal_damage_floor_fixture",
        &[
            &creature_guid.to_string(),
            &health.to_string(),
            &dead.to_string(),
            &floor.to_string(),
            &notification.to_string(),
            &absorb.to_string(),
        ],
    );
}

fn packed_definition(rules: &str) -> String {
    let subject = format!("entry:{WOLF_ENTRY}");
    let material = format!("{subject}@{rules}");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-eventai-definition-v1");
    hasher.update(material.as_bytes());
    let revision = u64::from_le_bytes(
        hasher.finalize().as_bytes()[..8]
            .try_into()
            .expect("a BLAKE3 digest has at least eight bytes"),
    );
    format!("{subject}@{revision}@{rules}")
}
