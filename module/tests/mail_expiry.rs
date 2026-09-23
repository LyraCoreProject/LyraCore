mod support;

use support::{actor, Standalone};

/// The fixture Character that sends every staged letter (`module/src/debug/mail.rs`).
const SENDER: &str = "5090070";
/// The recipient of the letter delivered about 2 s after staging.
const DELAYED_RECIPIENT: &str = "5090072";
/// The seeded Tester, whose live purse receives the take fixture's payout.
const TAKER: &str = "1";
const TAKE_ESCROW: &str = "5090070";

/// Rows inserted into and deleted from the captured query by each committed transaction.
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

fn timers(standalone: &Standalone) -> Vec<std::collections::BTreeMap<String, String>> {
    standalone.query_rows("SELECT * FROM game_mail_timer")
}

/// Runs only when requested because it builds and publishes the Wasm module to its own standalone.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn expired_mail_returns_or_is_deleted_once_and_each_arrival_fires_once() {
    let mut standalone = Standalone::start("mail-expiry");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);

    // The stage refuses unless each visible letter sent exactly one Mail Arrival and the delayed
    // letter sent none. Two letters return at once, so their sender hears of each one.
    let delayed = standalone.capture_updates(&arrivals_of(DELAYED_RECIPIENT), 2, || {
        let returned = standalone.capture_updates(&arrivals_of(SENDER), 2, || {
            standalone.assert_call("debug_stage_mail_expiry_fixture", &[]);
        });
        assert_eq!(changes(&returned, "inserts"), [1, 1], "{returned:?}");
    });
    // The delayed letter's arrival comes when it is delivered, alone, and the event GC reaps it.
    assert_eq!(changes(&delayed, "inserts"), [1, 0], "{delayed:?}");
    assert_eq!(changes(&delayed, "deletes"), [0, 1], "{delayed:?}");
    standalone.wait_until_call_succeeds("debug_verify_mail_expiry_fixture", &[]);

    standalone.assert_call("debug_replay_mail_timer_fixture", &[]);
    standalone.assert_call("debug_verify_mail_expiry_fixture", &[]);

    // Mail written before the Mail Timer existed gets one timer from the repair pass, and a Mail
    // past its life expires at once.
    standalone.assert_call("debug_stage_mail_legacy_fixture", &[]);
    standalone.assert_call("debug_repair_after_publish", &[]);
    standalone.wait_until_call_succeeds("debug_verify_mail_legacy_fixture", &[]);

    let armed = timers(&standalone);
    assert_eq!(
        armed.len(),
        6,
        "two returned, the fresh, the delayed and the two legacy letters: {armed:?}"
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

/// A take fence moves the copper into Escrow before Mail Expiry deletes the emptied letter. The
/// payout reads only the Escrow, so it still completes.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_take_fenced_before_expiry_still_pays_out_after_the_letter_is_deleted() {
    let mut standalone = Standalone::start("mail-expiry-take");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[TAKER]);
    standalone.assert_call("debug_stage_mail_take_fixture", &[]);

    let mail_id = standalone.query_rows("SELECT id FROM game_mail WHERE subject = 'timer: take'")
        [0]["id"]
        .clone();
    let taker = actor(TAKER);
    standalone.assert_call(
        "realm_mail_take_money_fence",
        &[TAKE_ESCROW, &taker, &mail_id, "77"],
    );
    standalone.assert_call("debug_age_mail_take_fixture", &[]);
    standalone.wait_until_call_succeeds("debug_verify_mail_take_fixture", &[]);

    let purse = || -> u64 {
        standalone.query_rows(&format!(
            "SELECT money FROM game_world_entity WHERE guid = {TAKER}"
        ))[0]["money"]
            .parse()
            .expect("a purse is a number")
    };
    let before = purse();
    standalone.assert_call("realm_mail_payout", &[TAKE_ESCROW, &taker, &mail_id, "77"]);
    assert_eq!(purse(), before + 77);
}
