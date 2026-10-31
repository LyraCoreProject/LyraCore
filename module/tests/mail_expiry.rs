mod support;

use std::collections::BTreeMap;

use support::Standalone;

const SENDER: &str = "5090070";
const DELAYED_RECIPIENT: &str = "5090072";

fn changes(updates: &[serde_json::Value], side: &str) -> Vec<usize> {
    updates
        .iter()
        .map(|update| {
            update["game_mail_arrival"][side]
                .as_array()
                .map_or(0, Vec::len)
        })
        .collect()
}

fn arrivals_of(guid: &str) -> String {
    format!("SELECT * FROM game_mail_arrival WHERE recipient_guid = {guid}")
}

fn timers(standalone: &Standalone) -> Vec<BTreeMap<String, String>> {
    standalone.query_rows("SELECT * FROM game_mail_timer")
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn expired_mail_returns_or_is_deleted_once_and_each_arrival_fires_once() {
    let mut standalone = Standalone::start("mail-expiry");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);

    let delayed = standalone.capture_updates(&arrivals_of(DELAYED_RECIPIENT), 2, || {
        let returned = standalone.capture_updates(&arrivals_of(SENDER), 2, || {
            standalone.assert_call("debug_stage_mail_expiry_fixture", &[]);
        });
        assert_eq!(changes(&returned, "inserts"), [1, 1], "{returned:?}");
    });
    assert_eq!(changes(&delayed, "inserts"), [1, 0], "{delayed:?}");
    assert_eq!(changes(&delayed, "deletes"), [0, 1], "{delayed:?}");
    standalone.wait_until_call_succeeds("debug_verify_mail_expiry_fixture", &[]);

    standalone.assert_call("debug_replay_mail_timer_fixture", &[]);
    standalone.assert_call("debug_verify_mail_expiry_fixture", &[]);

    standalone.assert_call("debug_stage_mail_legacy_fixture", &[]);
    standalone.assert_call("debug_repair_after_publish", &[]);
    standalone.wait_until_call_succeeds("debug_verify_mail_legacy_fixture", &[]);

    let armed = timers(&standalone);
    assert_eq!(
        armed.len(),
        7,
        "two returned, two inside their life, the delayed and two legacy letters: {armed:?}"
    );
    standalone.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(
        timers(&standalone),
        armed,
        "a second repair pass must arm nothing"
    );

    standalone.publish_module();
    standalone.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(
        timers(&standalone),
        armed,
        "a republish keeps every timer and arms no second one"
    );
    for reducer in [
        "debug_verify_mail_expiry_fixture",
        "debug_verify_mail_legacy_fixture",
    ] {
        standalone.assert_call(reducer, &[]);
    }
}
