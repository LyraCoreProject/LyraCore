//! Reconciling a Package's Runtime Scripts onto a Shard, and firing one on a real event.
//!
//! Everything here needs a live `ReducerContext`, which a native test has no way to build: the
//! apply reducer's writes, the `game_script` rows they leave, and the Event Binding dispatch that
//! reads them back at a hook chokepoint. The plan-level refusals and the dispatch ORDER are pure
//! and covered where they live (`module/src/package_import/script.rs`,
//! `module/src/script_binding.rs`); this drives the rungs those cannot reach.

mod support;

use support::Standalone;

/// A Package's source digest. The artifact only carries it; nothing recomputes it.
const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const HASH_B: &str = "1111111111111111111111111111111111111111111111111111111111111111";

const PLAYER: u64 = 1;
/// The scenario fixture's own quest, which `debug_seed_scenario_fixtures` puts on the Shard. The
/// test only needs `debug_grant_quest` to succeed so the `on_quest_accept` chokepoint fires; a
/// bare sandbox seeds no quest template at all, so one has to be asked for.
const QUEST: u32 = 50_900;

fn script(
    script_id: u32,
    name: &str,
    event: &str,
    priority: i32,
    enabled: bool,
    lua: &str,
) -> String {
    format!(
        r#"{{"script_id":{script_id},"name":"{name}","event":"{event}","priority":{priority},"enabled":{enabled},"source":"{lua}"}}"#
    )
}

fn artifact(package: &str, source_hash: &str, scripts: &[String]) -> String {
    format!(
        r#"{{"kind":"script","version":1,"package":"{package}","source_hash":"{source_hash}","scripts":[{}]}}"#,
        scripts.join(",")
    )
}

/// One reducer argument. `spacetime call` parses every argument as JSON, so a `String` parameter
/// has to arrive as a JSON string literal — bare text is a parse error, and the packed payload
/// carries quotes and newlines that need escaping either way.
fn arg(value: &str) -> String {
    serde_json::to_string(value).expect("a string encodes as JSON")
}

/// `spacetime sql` 2.7.1 prints a string column inside quotes, and the shared row parser keeps the
/// cell verbatim rather than deciding what a type looks like. Stripping here, at the one place this
/// target compares a string, keeps that decision out of a harness every other integration target
/// reads its own way.
fn unquote(value: &str) -> String {
    value.trim_matches('"').to_string()
}

/// The whole enabled plan, one canonical artifact per line — the payload shape the reducer reads.
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
                unquote(&row["name"]),
                unquote(&row["package"]),
                unquote(&row["event"]),
                unquote(&row["content_hash"]),
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
                unquote(&row["package"]),
                row["inserted_rows"].parse().expect("a row count"),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_packages_runtime_scripts_reconcile_onto_a_shard() {
    let standalone = Standalone::start("package-runtime-scripts");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);

    // --- One Package's whole set lands, disabled script included.
    apply(
        &standalone,
        &[artifact(
            "example.bolt",
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
    assert_eq!(rows[0].2, "example.bolt");
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
    assert_eq!(provenance(&standalone), [("example.bolt".to_string(), 2)]);

    // --- Re-applying the same plan is idempotent.
    let before = scripts_on_shard(&standalone);
    apply(
        &standalone,
        &[artifact(
            "example.bolt",
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

    // --- A second Package joins; neither displaces the other.
    apply(
        &standalone,
        &[
            artifact(
                "example.bolt",
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
                "example.zeta",
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
            ("example.bolt".to_string(), 1),
            ("example.zeta".to_string(), 1)
        ]
    );

    // --- A Package leaving the enabled set takes its scripts with it.
    apply(
        &standalone,
        &[artifact(
            "example.zeta",
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
    assert_eq!(rows.len(), 1, "example.bolt's scripts are gone: {rows:?}");
    assert_eq!(rows[0].1, "zeta.greet");
    assert_eq!(provenance(&standalone), [("example.zeta".to_string(), 1)]);

    // --- An empty plan is the honest "no Package ships a script any more".
    apply(&standalone, &[]);
    assert!(scripts_on_shard(&standalone).is_empty());
    assert!(provenance(&standalone).is_empty());
}

/// A plan two Packages disagree about must change nothing at all — not the scripts already on the
/// Shard, and not the provenance describing them.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_conflicting_plan_leaves_the_shard_exactly_as_it_was() {
    let standalone = Standalone::start("package-runtime-scripts-conflict");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);

    apply(
        &standalone,
        &[artifact(
            "example.bolt",
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

    // Both Packages ship script 100_002. The plan is refused whole.
    let refused = standalone.call(
        "apply_package_deltas",
        &[
            &arg("script"),
            &arg(&[
                artifact(
                    "example.first",
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
                    "example.second",
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
    assert_eq!(provenance(&standalone), [("example.bolt".to_string(), 1)]);
}

/// A Package Event is an event the Package fires itself, so the Shard has to store the binding
/// exactly as written — and refuse a Package reaching for an event another Package owns.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_package_binds_its_own_event_and_never_another_packages() {
    let standalone = Standalone::start("package-runtime-scripts-package-event");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);

    apply(
        &standalone,
        &[artifact(
            "example",
            HASH_A,
            &[script(
                100_001,
                "example.chooser",
                "example.answer",
                0,
                true,
                "return 42",
            )],
        )],
    );

    let rows = scripts_on_shard(&standalone);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        rows[0].3, "example.answer",
        "the Package Event is the dispatch label, so it is stored as written: {rows:?}"
    );
    let before = scripts_on_shard(&standalone);

    // `example.bolt` reaching for `example`'s event. Refused at the parse, before any write.
    let refused = standalone.call(
        "apply_package_deltas",
        &[
            &arg("script"),
            &arg(&artifact(
                "example.bolt",
                HASH_B,
                &[script(
                    100_002,
                    "bolt.thief",
                    "example.answer",
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
    assert_eq!(provenance(&standalone), [("example".to_string(), 1)]);
}

/// The point of the whole feature: a script a Package shipped runs at a real core chokepoint, and
/// one that fails does not stop the next one.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_package_script_fires_on_a_real_event_and_a_failing_one_does_not_block_the_next() {
    let standalone = Standalone::start("package-runtime-scripts-fire");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
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

    // Three scripts on one event: one that fails to compile between two that work. The Host is the
    // failure boundary, so the third still runs.
    apply(
        &standalone,
        &[artifact(
            "example.bolt",
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
                // Disabled: shipped, on the Shard, and never invoked.
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
