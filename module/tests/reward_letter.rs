mod support;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use support::{actor, Standalone};

const TESTER: &str = "1";
const GIVER: &str = "17379390972441394945";
const CARD_QUEST: &str = "509091";
const ELDER_QUEST: &str = "509092";
const PLAIN_QUEST: &str = "509093";
const CARD_BODY: &str = "Your membership card, $n.";

fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past 1970")
        .as_micros() as i64
}

fn reward_letters(standalone: &Standalone) -> Vec<BTreeMap<String, String>> {
    standalone
        .query_rows(&format!(
            "SELECT * FROM game_mail WHERE recipient_guid = {TESTER}"
        ))
        .into_iter()
        .filter(|row| row["sender_kind"] == "3")
        .collect()
}

fn letter_from(standalone: &Standalone, sender_entry: &str) -> BTreeMap<String, String> {
    let rows: Vec<_> = reward_letters(standalone)
        .into_iter()
        .filter(|row| row["sender_entry"] == sender_entry)
        .collect();
    assert_eq!(rows.len(), 1, "one letter from {sender_entry}: {rows:?}");
    rows.into_iter().next().expect("one row")
}

fn turn_in(standalone: &Standalone, quest: &str) -> std::process::Output {
    standalone.call("gw_turn_in_quest", &[&actor(TESTER), GIVER, quest, "0"])
}

#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_turn_in_files_one_reward_letter_that_arrives_from_its_quest_giver() {
    let mut standalone = Standalone::start("reward-letter");
    standalone.publish_module();
    standalone.assert_call("claim_operator", &[]);
    standalone.assert_call("install_guid_range", &["0"]);
    standalone.assert_call("debug_spawn_player_entity", &[TESTER]);
    standalone.assert_call("debug_stage_reward_letter_fixture", &[]);

    standalone.assert_call("debug_turn_in_quest", &[TESTER, GIVER, CARD_QUEST, "0"]);
    assert!(
        reward_letters(&standalone).is_empty(),
        "the Package turn-in path files no letter"
    );
    standalone.assert_call("debug_stage_reward_letter_fixture", &[]);

    assert!(turn_in(&standalone, PLAIN_QUEST).status.success());
    assert!(
        reward_letters(&standalone).is_empty(),
        "no reward mail, no letter"
    );

    let from = now_micros();
    assert!(turn_in(&standalone, CARD_QUEST).status.success());
    let by = now_micros();
    let again = turn_in(&standalone, CARD_QUEST);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&again.stdout),
        String::from_utf8_lossy(&again.stderr)
    );
    assert!(
        !again.status.success() && text.contains("quest already turned in"),
        "a quest turns in once: {text}"
    );
    let mail = letter_from(&standalone, "620");
    for (column, want) in [
        ("sender_kind", "3"),
        ("sender_guid", "0"),
        ("subject", ""),
        ("body", CARD_BODY),
        ("mail_template_id", "509091"),
        ("check_flags", "16"),
        ("item_entry", "5090050"),
        ("item_stack_count", "1"),
        ("item_durability", "70"),
        ("money", "0"),
        ("cod", "0"),
    ] {
        assert_eq!(mail[column], want, "{column}: {mail:?}");
    }
    let day = 86_400 * 1_000_000;
    let deliver: i64 = mail["deliver_micros"].parse().expect("a delivery instant");
    assert!(
        (from + day..=by + day).contains(&deliver),
        "the letter arrives 86,400 s after the turn-in: {mail:?}"
    );
    standalone.assert_sql(&format!(
        "UPDATE game_mail SET deliver_micros = 0 WHERE id = {}",
        mail["id"]
    ));
    let returned = standalone.call("realm_mail_return", &[&actor(TESTER), &mail["id"], "false"]);
    let answer = format!(
        "{}{}",
        String::from_utf8_lossy(&returned.stdout),
        String::from_utf8_lossy(&returned.stderr)
    );
    assert!(
        !returned.status.success() && answer.contains("only a character's mail can be returned"),
        "a Reward Letter has nobody to go back to: {answer}"
    );

    let from = now_micros();
    assert!(turn_in(&standalone, ELDER_QUEST).status.success());
    let by = now_micros();
    let mail = letter_from(&standalone, "11811");
    for (column, want) in [
        ("money", "1000000"),
        ("item_entry", "0"),
        ("mail_template_id", "509092"),
    ] {
        assert_eq!(mail[column], want, "{column}: {mail:?}");
    }
    let deliver: i64 = mail["deliver_micros"].parse().expect("a delivery instant");
    let delay = 129_600 * 1_000_000;
    assert!(
        (from + delay..=by + delay).contains(&deliver),
        "the letter arrives 129,600 s after the turn-in: {mail:?}"
    );
}
