mod support;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use support::{actor, Standalone};

/// The seeded Tester: it pays, sends and receives, with a live purse and bags.
const TESTER: &str = "1";
/// A Character on another Account that writes to the Tester.
const SELLER: &str = "5090070";
/// A Character that receives the Tester's letters and returns some.
const FRIEND: &str = "5090075";
const PURSE: u64 = 1_000;
const BLADE: &str = "5090050";
const COD: &str = "250";
const SEND_ESCROW: &str = "5090080";
const PAYMENT_ESCROW: &str = "5090081";
const TAKE_ESCROW: &str = "5090082";
/// vanilla's `MailDeliveryDelay` default (cmangos `World.cpp:614`).
const HOUR_MICROS: i64 = 3_600 * 1_000_000;
/// Long enough for the checks that run before the delivery to finish first.
const SHORT_DELAY_SECS: i64 = 5;

fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past 1970")
        .as_micros() as i64
}

fn quoted(text: &str) -> String {
    format!("\"{text}\"")
}

/// The one row `recipient` holds under `subject`.
fn letter(standalone: &Standalone, recipient: &str, subject: &str) -> BTreeMap<String, String> {
    let rows: Vec<_> = standalone
        .query_rows(&format!(
            "SELECT id, recipient_guid, sender_guid, money, cod, check_flags, deliver_micros \
             FROM game_mail WHERE subject = '{subject}'"
        ))
        .into_iter()
        .filter(|row| row["recipient_guid"] == recipient)
        .collect();
    assert_eq!(rows.len(), 1, "{recipient} holds one {subject}: {rows:?}");
    rows.into_iter().next().expect("one row")
}

fn deliver_micros(row: &BTreeMap<String, String>) -> i64 {
    row["deliver_micros"].parse().expect("a delivery instant")
}

fn purse(standalone: &Standalone) -> u64 {
    standalone.query_rows(&format!(
        "SELECT money FROM game_world_entity WHERE guid = {TESTER}"
    ))[0]["money"]
        .parse()
        .expect("a purse is a number")
}

fn blades(standalone: &Standalone) -> Vec<String> {
    standalone
        .query_rows(&format!(
            "SELECT guid, entry FROM game_item_instance WHERE owner_guid = {TESTER}"
        ))
        .into_iter()
        .filter(|row| row["entry"] == BLADE)
        .map(|row| row["guid"].clone())
        .collect()
}

fn arrivals_of(guid: &str) -> String {
    format!("SELECT * FROM game_mail_arrival WHERE recipient_guid = {guid}")
}

fn tester_with_purse(test_name: &str) -> Standalone {
    let mut standalone = Standalone::start(test_name);
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[TESTER]);
    standalone.assert_call("debug_set_money", &[TESTER, &PURSE.to_string()]);
    standalone
}

fn refused(standalone: &Standalone, reducer: &str, args: &[&str]) -> bool {
    !standalone.call(reducer, args).status.success()
}

/// A priced item letter committed with a Delivery Delay stays hidden from the Tester until its Mail
/// Arrival. Until then the Tester cannot read or take it, and a COD payment fenced early waits in
/// its fence: the seller is paid only after the letter arrives, and only once.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_delayed_priced_letter_is_hidden_until_it_arrives_and_its_price_is_paid_after() {
    let standalone = tester_with_purse("mail-delivery-cod");
    let tester = actor(TESTER);
    let subject = quoted("delivery: cod");
    let delay = SHORT_DELAY_SECS.to_string();
    let mut priced = BTreeMap::new();

    let arrival = standalone.capture_updates(&arrivals_of(TESTER), 1, || {
        let committed_from = now_micros();
        standalone.assert_call(
            "realm_mail_commit",
            &[
                SEND_ESCROW,
                &actor(SELLER),
                TESTER,
                &subject,
                "\"\"",
                "0",
                BLADE,
                "1",
                "17",
                "0",
                "false",
                "0",
                COD,
                "0",
                &delay,
            ],
        );
        let committed_by = now_micros();
        priced = letter(&standalone, TESTER, "delivery: cod");
        let arrives = deliver_micros(&priced);
        let hidden = SHORT_DELAY_SECS * 1_000_000;
        assert!(
            (committed_from + hidden..=committed_by + hidden).contains(&arrives),
            "the letter arrives {SHORT_DELAY_SECS} s after the commit: {priced:?}"
        );

        let id = priced["id"].clone();
        assert!(refused(
            &standalone,
            "realm_mail_take_item_fence",
            &[TAKE_ESCROW, &tester, &id, BLADE]
        ));
        assert!(refused(
            &standalone,
            "realm_mail_mark_read",
            &[&tester, &id]
        ));
        standalone.assert_call(
            "realm_mail_fence",
            &[
                PAYMENT_ESCROW,
                &tester,
                SELLER,
                &subject,
                "\"\"",
                COD,
                "0",
                "0",
                "0",
                &id,
                "false",
            ],
        );
        assert!(
            refused(
                &standalone,
                "realm_mail_commit",
                &[
                    PAYMENT_ESCROW,
                    &tester,
                    SELLER,
                    &subject,
                    "\"\"",
                    COD,
                    "0",
                    "0",
                    "0",
                    "0",
                    "false",
                    "0",
                    "0",
                    &id,
                    "0",
                ],
            ),
            "a payment for a letter that has not arrived pays nobody"
        );
        assert!(
            now_micros() < arrives,
            "the checks ran past the delivery, so they prove nothing; raise SHORT_DELAY_SECS"
        );
    });
    let inserted = arrival[0]["game_mail_arrival"]["inserts"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(inserted, 1, "one Mail Arrival, at delivery: {arrival:?}");
    assert_eq!(
        purse(&standalone),
        PURSE - 250,
        "the price waits in its fence"
    );
    assert_eq!(letter(&standalone, TESTER, "delivery: cod")["cod"], COD);

    let id = priced["id"].clone();
    standalone.assert_call(
        "realm_mail_commit",
        &[
            PAYMENT_ESCROW,
            &tester,
            SELLER,
            &subject,
            "\"\"",
            COD,
            "0",
            "0",
            "0",
            "0",
            "false",
            "0",
            "0",
            &id,
            "0",
        ],
    );
    standalone.assert_call("realm_mail_confirm_delivery", &[PAYMENT_ESCROW, &tester]);
    standalone.assert_call("realm_mail_settle", &[PAYMENT_ESCROW, &tester]);
    let payment = letter(&standalone, SELLER, "delivery: cod");
    assert_eq!(
        (payment["money"].as_str(), payment["check_flags"].as_str()),
        (COD, "8"),
        "the seller is paid once, with the COD payment bit"
    );
    assert_eq!(
        deliver_micros(&payment),
        0,
        "a COD payment arrives at once (cmangos MailHandler.cpp:475-477)"
    );
    assert_eq!(letter(&standalone, TESTER, "delivery: cod")["cod"], "0");

    let before = blades(&standalone).len();
    standalone.assert_call(
        "realm_mail_take_item_fence",
        &[TAKE_ESCROW, &tester, &id, BLADE],
    );
    standalone.assert_call(
        "realm_mail_item_payout",
        &[
            TAKE_ESCROW,
            &tester,
            &id,
            BLADE,
            "1",
            "17",
            "0",
            "false",
            "0",
        ],
    );
    assert_eq!(blades(&standalone).len(), before + 1);
    assert_eq!(purse(&standalone), PURSE - 250, "charged once");
}

/// The single-database send and the return apply the Delivery Delay on their own: only an item to
/// another Account waits an hour.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn only_an_item_sent_or_returned_to_another_account_waits_an_hour() {
    let standalone = tester_with_purse("mail-delivery-send");
    let tester = actor(TESTER);
    let friend = actor(FRIEND);
    let owned = blades(&standalone);
    for _ in 0..3 {
        standalone.assert_call("debug_grant_item", &[TESTER, BLADE, "1"]);
    }
    let swords: Vec<String> = blades(&standalone)
        .into_iter()
        .filter(|guid| !owned.contains(guid))
        .collect();
    assert_eq!(swords.len(), 3, "{swords:?}");

    let send = |subject: &str, money: &str, item: &str, same_account: &str| {
        let from = now_micros();
        standalone.assert_call(
            "realm_mail_send",
            &[
                &tester,
                FRIEND,
                &quoted(subject),
                "\"\"",
                money,
                "0",
                item,
                same_account,
            ],
        );
        (from, now_micros(), letter(&standalone, FRIEND, subject))
    };

    let (from, by, waits) = send("delivery: item", "0", &swords[0], "false");
    assert!(
        (from + HOUR_MICROS..=by + HOUR_MICROS).contains(&deliver_micros(&waits)),
        "an item to another Account waits an hour (mangoszero MailHandler.cpp:306-318): {waits:?}"
    );
    assert!(refused(
        &standalone,
        "realm_mail_take_item",
        &[&friend, &waits["id"]]
    ));
    let (_, _, alt) = send("delivery: alt", "0", &swords[1], "true");
    assert_eq!(deliver_micros(&alt), 0, "an item to an alt arrives at once");
    let (_, _, copper) = send("delivery: copper", "100", "0", "false");
    assert_eq!(
        deliver_micros(&copper),
        0,
        "copper to another Account arrives at once"
    );
    let (_, _, kept) = send("delivery: kept", "0", &swords[2], "true");

    let from = now_micros();
    standalone.assert_call("realm_mail_return", &[&friend, &alt["id"], "false"]);
    let by = now_micros();
    let back = letter(&standalone, TESTER, "delivery: alt");
    assert!(
        (from + HOUR_MICROS..=by + HOUR_MICROS).contains(&deliver_micros(&back)),
        "an item returned to another Account waits an hour (cmangos Mail.cpp:241-261): {back:?}"
    );
    assert!(refused(
        &standalone,
        "realm_mail_take_item",
        &[&tester, &back["id"]]
    ));

    let from = now_micros();
    standalone.assert_call("realm_mail_return", &[&friend, &kept["id"], "true"]);
    let by = now_micros();
    let back = letter(&standalone, TESTER, "delivery: kept");
    assert!(
        (from..=by).contains(&deliver_micros(&back)),
        "an item returned to an alt arrives at once: {back:?}"
    );
    standalone.assert_call("realm_mail_take_item", &[&tester, &back["id"]]);
}
