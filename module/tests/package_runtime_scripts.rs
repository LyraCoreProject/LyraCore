mod support;

use support::Standalone;

const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const HASH_B: &str = "1111111111111111111111111111111111111111111111111111111111111111";

const PLAYER: u64 = 1;
const QUEST: u32 = 50_900;

fn script(
    script_id: u32,
    name: &str,
    event: &str,
    priority: i32,
    enabled: bool,
    lua: &str,
) -> String {
    serde_json::json!({
        "script_id": script_id,
        "name": name,
        "event": event,
        "priority": priority,
        "enabled": enabled,
        "source": lua,
    })
    .to_string()
}

fn artifact(package: &str, source_hash: &str, scripts: &[String]) -> String {
    format!(
        r#"{{"kind":"script","version":1,"package":"{package}","source_hash":"{source_hash}","scripts":[{}]}}"#,
        scripts.join(",")
    )
}

fn arg(value: &str) -> String {
    serde_json::to_string(value).expect("a string encodes as JSON")
}

fn apply(standalone: &Standalone, artifacts: &[String]) {
    standalone.assert_call(
        "apply_package_deltas",
        &[&arg("script"), &arg(&artifacts.join("\n"))],
    );
}

fn scripts_on_shard(standalone: &Standalone) -> Vec<(u32, String, String, String, String)> {
    let mut rows: Vec<(u32, String, String, String, String)> = standalone
        .query_rows("SELECT * FROM game_script")
        .into_iter()
        .map(|row| {
            (
                row["script_id"].parse().expect("script_id is a number"),
                row["name"].clone(),
                row["package"].clone(),
                row["event"].clone(),
                row["content_hash"].clone(),
            )
        })
        .collect();
    rows.sort_by_key(|row| row.0);
    rows
}

fn provenance(standalone: &Standalone) -> Vec<(String, u64)> {
    let mut rows: Vec<(String, u64)> = standalone
        .query_rows("SELECT * FROM game_package_import WHERE family = 'script'")
        .into_iter()
        .map(|row| {
            (
                row["package"].clone(),
                row["inserted_rows"].parse().expect("a row count"),
            )
        })
        .collect();
    rows.sort();
    rows
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_packages_runtime_scripts_reconcile_onto_a_shard() {
    let mut standalone = Standalone::start("package-runtime-scripts");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);

    apply(
        &standalone,
        &[artifact(
            "test_fixture.bolt",
            HASH_A,
            &[
                script(100_001, "bolt.greet", "on_login", 0, true, "local a = 1"),
                script(100_002, "bolt.off", "on_logout", 0, false, "local a = 2"),
            ],
        )],
    );

    let rows = scripts_on_shard(&standalone);
    assert_eq!(
        rows.len(),
        2,
        "both scripts land, disabled or not: {rows:?}"
    );
    assert_eq!(rows[0].1, "bolt.greet");
    assert_eq!(rows[0].2, "test_fixture.bolt");
    assert_eq!(rows[0].3, "on_login");
    assert_eq!(
        rows[0].4.len(),
        64,
        "the content hash is derived at apply, not authored: {rows:?}"
    );
    assert_ne!(
        rows[0].4, rows[1].4,
        "two different sources hash differently"
    );
    assert_eq!(
        provenance(&standalone),
        [("test_fixture.bolt".to_string(), 2)]
    );

    let before = scripts_on_shard(&standalone);
    apply(
        &standalone,
        &[artifact(
            "test_fixture.bolt",
            HASH_A,
            &[
                script(100_001, "bolt.greet", "on_login", 0, true, "local a = 1"),
                script(100_002, "bolt.off", "on_logout", 0, false, "local a = 2"),
            ],
        )],
    );
    assert_eq!(
        scripts_on_shard(&standalone),
        before,
        "re-publication must not change a row"
    );

    apply(
        &standalone,
        &[
            artifact(
                "test_fixture.bolt",
                HASH_A,
                &[script(
                    100_001,
                    "bolt.greet",
                    "on_login",
                    0,
                    true,
                    "local a = 1",
                )],
            ),
            artifact(
                "test_fixture.zeta",
                HASH_B,
                &[script(
                    100_050,
                    "zeta.greet",
                    "on_login",
                    0,
                    true,
                    "local a = 3",
                )],
            ),
        ],
    );
    let rows = scripts_on_shard(&standalone);
    assert_eq!(
        rows.iter().map(|r| r.0).collect::<Vec<_>>(),
        [100_001, 100_050]
    );
    assert_eq!(
        rows.iter().map(|r| r.1.as_str()).collect::<Vec<_>>(),
        ["bolt.greet", "zeta.greet"],
        "a renamed or dropped script cannot leave an orphan row: {rows:?}"
    );
    assert_eq!(
        provenance(&standalone),
        [
            ("test_fixture.bolt".to_string(), 1),
            ("test_fixture.zeta".to_string(), 1)
        ]
    );

    apply(
        &standalone,
        &[artifact(
            "test_fixture.zeta",
            HASH_B,
            &[script(
                100_050,
                "zeta.greet",
                "on_login",
                0,
                true,
                "local a = 3",
            )],
        )],
    );
    let rows = scripts_on_shard(&standalone);
    assert_eq!(
        rows.len(),
        1,
        "test_fixture.bolt's scripts are gone: {rows:?}"
    );
    assert_eq!(rows[0].1, "zeta.greet");
    assert_eq!(
        provenance(&standalone),
        [("test_fixture.zeta".to_string(), 1)]
    );

    apply(&standalone, &[]);
    assert!(scripts_on_shard(&standalone).is_empty());
    assert!(provenance(&standalone).is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_conflicting_plan_leaves_the_shard_exactly_as_it_was() {
    let mut standalone = Standalone::start("package-runtime-scripts-conflict");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);

    apply(
        &standalone,
        &[artifact(
            "test_fixture.bolt",
            HASH_A,
            &[script(
                100_001,
                "bolt.greet",
                "on_login",
                0,
                true,
                "local a = 1",
            )],
        )],
    );
    let before = scripts_on_shard(&standalone);

    let refused = standalone.call(
        "apply_package_deltas",
        &[
            &arg("script"),
            &arg(&[
                artifact(
                    "test_fixture.first",
                    HASH_A,
                    &[script(
                        100_002,
                        "first.a",
                        "on_login",
                        0,
                        true,
                        "local a = 1",
                    )],
                ),
                artifact(
                    "test_fixture.second",
                    HASH_B,
                    &[script(
                        100_002,
                        "second.a",
                        "on_login",
                        0,
                        true,
                        "local a = 2",
                    )],
                ),
            ]
            .join("\n")),
        ],
    );
    assert!(
        !refused.status.success(),
        "a conflicting plan must be refused"
    );

    assert_eq!(
        scripts_on_shard(&standalone),
        before,
        "a refused plan writes nothing"
    );
    assert_eq!(
        provenance(&standalone),
        [("test_fixture.bolt".to_string(), 1)]
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_binds_its_own_event_and_never_another_packages() {
    let mut standalone = Standalone::start("package-runtime-scripts-package-event");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);

    apply(
        &standalone,
        &[artifact(
            "test_fixture",
            HASH_A,
            &[script(
                100_001,
                "test_fixture.chooser",
                "test_fixture.answer",
                0,
                true,
                "return 42",
            )],
        )],
    );

    let rows = scripts_on_shard(&standalone);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].3, "test_fixture.answer",
        "the Package Event is the dispatch label, so it is stored as written: {rows:?}"
    );
    let before = scripts_on_shard(&standalone);

    let refused = standalone.call(
        "apply_package_deltas",
        &[
            &arg("script"),
            &arg(&artifact(
                "test_fixture.bolt",
                HASH_B,
                &[script(
                    100_002,
                    "bolt.thief",
                    "test_fixture.answer",
                    0,
                    true,
                    "return 1",
                )],
            )),
        ],
    );
    assert!(
        !refused.status.success(),
        "a Package may only bind its own events"
    );

    assert_eq!(
        scripts_on_shard(&standalone),
        before,
        "a refused plan writes nothing"
    );
    assert_eq!(provenance(&standalone), [("test_fixture".to_string(), 1)]);
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_package_script_fires_on_a_real_event_and_a_failing_one_does_not_block_the_next() {
    let mut standalone = Standalone::start("package-runtime-scripts-fire");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);

    let xp = |standalone: &Standalone| -> u64 {
        standalone
            .query_rows(&format!(
                "SELECT * FROM game_world_entity WHERE guid = {PLAYER}"
            ))
            .first()
            .expect("the player entity exists")["xp"]
            .parse()
            .expect("xp is a number")
    };
    let before = xp(&standalone);

    apply(
        &standalone,
        &[artifact(
            "test_fixture.bolt",
            HASH_A,
            &[
                script(
                    100_001,
                    "bolt.first",
                    "on_quest_accept",
                    0,
                    true,
                    "grant_xp(event.actor, 10)",
                ),
                script(
                    100_002,
                    "bolt.broken",
                    "on_quest_accept",
                    5,
                    true,
                    "this is not lua ===",
                ),
                script(
                    100_003,
                    "bolt.last",
                    "on_quest_accept",
                    10,
                    true,
                    "grant_xp(event.actor, 25)",
                ),
                script(
                    100_004,
                    "bolt.off",
                    "on_quest_accept",
                    20,
                    false,
                    "grant_xp(event.actor, 1000)",
                ),
            ],
        )],
    );

    standalone.assert_call(
        "debug_grant_quest",
        &[&PLAYER.to_string(), &QUEST.to_string()],
    );

    assert_eq!(
        xp(&standalone) - before,
        35,
        "the two working scripts granted 10 and 25; the broken one granted nothing and the \
         disabled one was never invoked"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_typed_levelup_binding_receives_each_attained_level_from_the_core_hook() {
    let mut standalone = Standalone::start("package-runtime-scripts-typed-levelup");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_seed_scenario_fixtures", &[]);
    standalone.assert_call("debug_spawn_player_entity", &[&PLAYER.to_string()]);
    standalone.assert_sql("UPDATE game_character SET online = true WHERE guid = 1");
    standalone.assert_sql("UPDATE game_quest_template SET reward_xp = 1300 WHERE entry = 50900");
    standalone.assert_sql("DELETE FROM game_quest_objective WHERE quest_entry = 50900");
    standalone.assert_call("debug_spawn_at_feet", &[&PLAYER.to_string(), "51003", "3"]);
    let giver = standalone
        .query_rows("SELECT guid FROM game_world_entity WHERE entry = 51003")
        .into_iter()
        .next()
        .expect("the quest giver was spawned")["guid"]
        .clone();

    apply(
        &standalone,
        &[artifact(
            "test_fixture.typed",
            HASH_A,
            &[script(
                100_100,
                "typed.ding",
                "on_levelup",
                0,
                true,
                include_str!("../../datascripts/tests/fixtures/typed/ding.generated.lua"),
            )],
        )],
    );
    standalone.assert_call(
        "debug_grant_quest",
        &[&PLAYER.to_string(), &QUEST.to_string()],
    );
    standalone.assert_call(
        "debug_turn_in_quest",
        &[&PLAYER.to_string(), &giver, &QUEST.to_string(), "0"],
    );

    let entity = standalone
        .query_rows("SELECT level FROM game_world_entity WHERE guid = 1")
        .into_iter()
        .next()
        .expect("the Character remains in the world");
    assert_eq!(entity["level"], "3");
    let mut messages = standalone.query_rows(
        "SELECT id, recipient_guid, message FROM game_system_message_event WHERE recipient_guid = 1",
    );
    messages.sort_by_key(|row| {
        row["id"]
            .parse::<u64>()
            .expect("the message identifier is numeric")
    });
    assert_eq!(
        messages
            .iter()
            .map(|row| row["message"].as_str())
            .collect::<Vec<_>>(),
        ["Ding 2", "Ding 3"],
        "the typed handler receives each attained level and its Character handle"
    );
}
