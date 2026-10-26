mod support;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use support::{actor, Standalone};

const TESTER: &str = "1";
const FRIEND: &str = "5090075";
const PURSE: u64 = 1_000;
const BLADE: &str = "5090050";
const HOUR_MICROS: i64 = 3_600 * 1_000_000;

fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past 1970")
        .as_micros() as i64
}

fn quoted(text: &str) -> String {
    format!("\"{text}\"")
}

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
