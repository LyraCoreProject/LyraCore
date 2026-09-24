mod support;

use std::collections::BTreeMap;

use support::{actor, Standalone};

/// The fixture Character that sends every staged letter (`module/src/debug/mail.rs`).
const SENDER: &str = "5090070";
/// The recipient of the letter delivered about 2 s after staging.
const DELAYED_RECIPIENT: &str = "5090072";
/// The seeded Tester, whose live purse receives the take fixture's payout.
const TAKER: &str = "1";
const TAKE_MONEY_ESCROW: &str = "5090070";
const TAKE_ITEM_ESCROW: &str = "5090071";
const COD_ESCROW: &str = "5090072";
/// The COD fixture's price, the Tester's purse before the payment, and the blade it carries.
const COD: &str = "250";
const PURSE: u64 = 1_000;
const BLADE: &str = "5090050";
/// A Mail with a COD price lives 3 days (cmangos `Mail.cpp:311`), and 30 days without one.
const PAST_COD_LIFE_SECS: &str = "259260";
const PAST_LIFE_SECS: &str = "2592060";

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

fn timers(standalone: &Standalone) -> Vec<BTreeMap<String, String>> {
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

fn quoted(text: &str) -> String {
    format!("\"{text}\"")
}

/// The row `recipient` holds under `subject`.
fn letter(standalone: &Standalone, recipient: &str, subject: &str) -> BTreeMap<String, String> {
    standalone
        .query_rows(&format!(
            "SELECT id, recipient_guid, sender_guid, money, check_flags FROM game_mail \
             WHERE subject = '{subject}'"
        ))
        .into_iter()
        .find(|row| row["recipient_guid"] == recipient)
        .unwrap_or_else(|| panic!("{recipient} holds no {subject}"))
}

fn purse(standalone: &Standalone) -> u64 {
    standalone.query_rows(&format!(
        "SELECT money FROM game_world_entity WHERE guid = {TAKER}"
    ))[0]["money"]
        .parse()
        .expect("a purse is a number")
}

fn blades(standalone: &Standalone) -> usize {
    standalone
        .query_rows(&format!(
            "SELECT entry FROM game_item_instance WHERE owner_guid = {TAKER}"
        ))
        .iter()
        .filter(|row| row["entry"] == BLADE)
        .count()
}

fn tester_at_the_mailbox(test_name: &str) -> Standalone {
    let mut standalone = Standalone::start(test_name);
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[TAKER]);
    standalone.assert_call("debug_set_money", &[TAKER, &PURSE.to_string()]);
    standalone.assert_call("debug_stage_mail_take_fixture", &[]);
    standalone
}

/// A take fence moves the copper or the item into Escrow before Mail Expiry deletes the emptied
/// letter. The payout reads only the Escrow, so it still completes.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_take_fenced_before_expiry_still_pays_out_after_the_letter_is_deleted() {
    let standalone = tester_at_the_mailbox("mail-expiry-take");
    let taker = actor(TAKER);
    let money = letter(&standalone, TAKER, "timer: take money")["id"].clone();
    let item = letter(&standalone, TAKER, "timer: take item")["id"].clone();
    standalone.assert_call(
        "realm_mail_take_money_fence",
        &[TAKE_MONEY_ESCROW, &taker, &money, "77"],
    );
    standalone.assert_call(
        "realm_mail_take_item_fence",
        &[TAKE_ITEM_ESCROW, &taker, &item, BLADE],
    );
    for subject in ["timer: take money", "timer: take item"] {
        let subject = quoted(subject);
        standalone.assert_call("debug_age_mail_fixture", &[TAKER, &subject, PAST_LIFE_SECS]);
        standalone.wait_until_call_succeeds(
            "debug_verify_mail_fixture_held",
            &[TAKER, &subject, "false"],
        );
    }

    let (purse_before, blades_before) = (purse(&standalone), blades(&standalone));
    standalone.assert_call(
        "realm_mail_payout",
        &[TAKE_MONEY_ESCROW, &taker, &money, "77"],
    );
    standalone.assert_call(
        "realm_mail_item_payout",
        &[
            TAKE_ITEM_ESCROW,
            &taker,
            &item,
            BLADE,
            "1",
            "17",
            "0",
            "false",
            "0",
            "0",
        ],
    );
    assert_eq!(purse(&standalone), purse_before + 77);
    assert_eq!(blades(&standalone), blades_before + 1);
}

/// The payment fence takes the payer's copper on their Shard before Realm-core commits it. When
/// Mail Expiry sends the priced letter back in between, the commit returns the copper to the payer
/// as a Returned Mail, and the Gateway can confirm and settle the fence.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_cod_payment_fenced_before_the_letter_expires_comes_back_to_the_payer() {
    let standalone = tester_at_the_mailbox("mail-expiry-cod");
    let taker = actor(TAKER);
    let subject = quoted("timer: cod race");
    let priced = letter(&standalone, TAKER, "timer: cod race")["id"].clone();
    standalone.assert_call(
        "realm_mail_fence",
        &[
            COD_ESCROW, &taker, SENDER, &subject, "\"\"", COD, "0", "0", "0", &priced, "false",
        ],
    );
    assert_eq!(purse(&standalone), PURSE - 250, "the fence took the price");

    standalone.assert_call(
        "debug_age_mail_fixture",
        &[TAKER, &subject, PAST_COD_LIFE_SECS],
    );
    standalone.wait_until_call_succeeds(
        "debug_verify_mail_fixture_held",
        &[SENDER, &subject, "true"],
    );

    standalone.assert_call(
        "realm_mail_commit",
        &[
            COD_ESCROW, &taker, SENDER, &subject, "\"\"", COD, "0", "0", "0", "0", "false", "0",
            "0", &priced, "0", "0", "0", "0", "0",
        ],
    );
    standalone.assert_call("realm_mail_confirm_delivery", &[COD_ESCROW, &taker]);
    standalone.assert_call("realm_mail_settle", &[COD_ESCROW, &taker]);
    assert!(
        standalone
            .query_rows(&format!(
                "SELECT escrow_id FROM game_mail_escrow WHERE escrow_id = {COD_ESCROW}"
            ))
            .is_empty(),
        "the fence settled"
    );

    let refund = letter(&standalone, TAKER, "timer: cod race");
    assert_eq!(
        (
            refund["money"].as_str(),
            refund["check_flags"].as_str(),
            refund["sender_guid"].as_str()
        ),
        (COD, "2", SENDER),
        "the payment comes back from the seller as a Returned Mail"
    );
    let bought = standalone.call(
        "realm_mail_take_item_fence",
        &[TAKE_ITEM_ESCROW, &taker, &priced, BLADE],
    );
    assert!(
        !bought.status.success(),
        "the returned payment bought nothing"
    );
    standalone.assert_call("realm_mail_take_money", &[&taker, &refund["id"]]);
    assert_eq!(purse(&standalone), PURSE, "no copper was lost");
}
