mod support;

use serde_json::Value;
use support::{actor, Standalone};

const CHAT_ROWS: &str = "SELECT * FROM game_chat_event";
/// The seeded Tester (`module/src/seed.rs`): a Human with a durable Character row the standalone
/// publishes with every fresh database.
const CHARACTER: u64 = 1;

fn refused(standalone: &Standalone, args: &[&str], tag: &str) {
    let result = standalone.call("gw_send_chat", args);
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !result.status.success() && output.contains(tag),
        "gw_send_chat should refuse with {tag}: {output}"
    );
}

/// The inserted `game_chat_event` rows of one captured transaction.
fn inserted_rows(update: &Value) -> Vec<Value> {
    update["game_chat_event"]["inserts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn an_e_line_is_one_universal_row_and_a_dead_or_creature_only_type_is_refused() {
    let mut standalone = Standalone::start("proximity-emotes");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);

    // EMOTE (type 3) with a requested non-Universal language (7, Common) still stores Universal
    // (cm:Player.cpp:16594).
    let updates = standalone.capture_updates(CHAT_ROWS, 1, || {
        standalone.assert_call(
            "gw_send_chat",
            &[
                &actor(&CHARACTER.to_string()),
                "3",
                "7",
                "\"waves wildly.\"",
            ],
        );
    });
    let rows = inserted_rows(&updates[0]);
    assert_eq!(rows.len(), 1, "one /e is one row: {updates:?}");
    assert_eq!(rows[0]["chat_type"], 3);
    assert_eq!(rows[0]["sender_guid"], CHARACTER);
    assert_eq!(rows[0]["message"], "waves wildly.");
    assert_eq!(
        rows[0]["language"], 0,
        "EMOTE always stores Universal, whatever was requested"
    );

    // Criterion 6: a Character can no longer submit the creature-only text-emote type (2).
    refused(
        &standalone,
        &[&actor(&CHARACTER.to_string()), "2", "0", "\"dances.\""],
        "unsupported chat type",
    );

    // Criterion 4: a dead Character's `/e` produces no row.
    standalone.assert_sql(&format!(
        "UPDATE game_world_entity SET dead = true WHERE guid = {CHARACTER}"
    ));
    refused(
        &standalone,
        &[&actor(&CHARACTER.to_string()), "3", "0", "\"waves again.\""],
        "dead players cannot speak",
    );
}

/// cm:ChatHandler.cpp:100-111: a Human saying or yelling in Orcish is refused before anything is
/// broadcast. Wire languages from gtker vanilla `language.rs`: Orcish 1, Common 7.
#[test]
#[ignore = "requires SpacetimeDB 2.7.1 and the Wasm toolchain"]
fn a_say_or_yell_line_needs_a_language_the_speakers_race_knows() {
    let mut standalone = Standalone::start("say-language");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[&CHARACTER.to_string()]);

    // Refusals roll back: the only row the next transaction carries is the Common sentinel.
    let updates = standalone.capture_updates(CHAT_ROWS, 1, || {
        refused(
            &standalone,
            &[&actor(&CHARACTER.to_string()), "0", "1", "\"zug zug\""],
            "chat:unknown_language",
        );
        refused(
            &standalone,
            &[&actor(&CHARACTER.to_string()), "1", "1", "\"ZUG ZUG\""],
            "chat:unknown_language",
        );
        standalone.assert_call(
            "gw_send_chat",
            &[&actor(&CHARACTER.to_string()), "0", "7", "\"well met\""],
        );
    });
    let rows = inserted_rows(&updates[0]);
    assert_eq!(rows.len(), 1, "{updates:?}");
    assert_eq!(rows[0]["chat_type"], 0);
    assert_eq!(rows[0]["language"], 7, "a Human's Common stays Common");
    assert_eq!(rows[0]["message"], "well met");
}
