mod support;

use support::{event_rows, fence_subscription, subscribe_with_fence, Standalone};

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI, the Wasm toolchain, and the dungeons Package installed (lyracore packages add dungeons)"]
fn deadmines_choreography_reaches_package_owned_outcomes() {
    let mut standalone = Standalone::start("deadmines-package");
    standalone.publish_module();
    standalone.assert_call_anonymous("debug_deadmines_begin", &[]);
    standalone.assert_call_anonymous("debug_deadmines_rhahkzor_falls", &[]);
    standalone.assert_call_anonymous("debug_deadmines_shredder_ejects_sneed", &[]);
    standalone.assert_call_anonymous("debug_deadmines_sneed_falls", &[]);
    standalone.assert_call_anonymous("debug_deadmines_gilnid_falls", &[]);
    let smite = standalone.query_rows("SELECT guid FROM game_world_entity WHERE entry = 646");
    assert_eq!(smite.len(), 1);
    let smite = &smite[0]["guid"];
    let speech = subscribe_with_fence(&standalone, &["SELECT * FROM game_chat_event_live"]);
    standalone.assert_call_anonymous("debug_deadmines_smite_improvises", &[]);
    fence_subscription(&standalone, &speech);
    assert!(event_rows(&speech, "game_chat_event_live")
        .iter()
        .any(|row| {
            row["sender_guid"] == *smite
                && row["chat_type"] == "1"
                && row["message"].contains("improvise")
        }));
    standalone.assert_call_anonymous("debug_deadmines_smite_gets_angry", &[]);
    fence_subscription(&standalone, &speech);
    assert!(event_rows(&speech, "game_chat_event_live")
        .iter()
        .any(|row| {
            row["sender_guid"] == *smite
                && row["chat_type"] == "1"
                && row["message"].contains("angry")
        }));
    standalone.assert_call_anonymous("debug_deadmines_cannon_breaches", &[]);
    standalone.assert_call_anonymous("debug_deadmines_smite_falls", &[]);
}
