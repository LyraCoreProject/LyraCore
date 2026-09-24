mod support;

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use support::{actor, Standalone};

/// The seeded Tester, who turns the fixture quests in.
const TESTER: &str = "1";
/// The fixture's quest ender: a clone of the seed chicken, creature 620, next to the Tester.
const GIVER: &str = "17379390972441394945";
/// Shaped like quest 3645, Membership Card Renewal: the quest ender sends one Tempered Blade
/// (5090050, max durability 70) after 86,400 s.
const CARD_QUEST: &str = "509091";
/// Shaped like quest 8728: creature 11811 sends 1,000,000 copper after 129,600 s.
const ELDER_QUEST: &str = "509092";
/// A quest with no reward mail.
const PLAIN_QUEST: &str = "509093";
const CARD_BODY: &str = "Your membership card, $n.";
const ELDER_BODY: &str = "The elders thank you, $n.";

fn now_micros() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past 1970")
        .as_micros() as i64
}

fn quoted(text: &str) -> String {
    format!("\"{text}\"")
}

fn escrows(standalone: &Standalone) -> Vec<BTreeMap<String, String>> {
    standalone.query_rows(&format!(
        "SELECT * FROM game_mail_escrow WHERE sender_guid = {TESTER}"
    ))
}

/// The one Reward Letter from `sender_entry` in the Tester's mailbox.
fn letter_from(standalone: &Standalone, sender_entry: &str) -> BTreeMap<String, String> {
    let rows: Vec<_> = standalone
        .query_rows(&format!(
            "SELECT * FROM game_mail WHERE recipient_guid = {TESTER}"
        ))
        .into_iter()
        .filter(|row| row["sender_entry"] == sender_entry)
        .collect();
    assert_eq!(rows.len(), 1, "one letter from {sender_entry}: {rows:?}");
    rows.into_iter().next().expect("one row")
}

fn turn_in(standalone: &Standalone, quest: &str) -> std::process::Output {
    standalone.call("gw_turn_in_quest", &[&actor(TESTER), GIVER, quest, "0"])
}

/// The Gateway's drive of a held Reward Letter, with the arguments its escrow row holds. Returns
/// the commit's wall-clock window.
#[allow(clippy::too_many_arguments)]
fn drive(
    standalone: &Standalone,
    escrow_id: &str,
    body: &str,
    money: &str,
    item: [&str; 3],
    delay_secs: &str,
    sender_entry: &str,
    template: &str,
) -> (i64, i64) {
    let tester = actor(TESTER);
    let body = quoted(body);
    let commit = [
        escrow_id,
        &tester,
        TESTER,
        "\"\"",
        &body,
        money,
        item[0],
        item[1],
        item[2],
        "0",
        "false",
        "0",
        "0",
        "0",
        delay_secs,
        "3",
        sender_entry,
        template,
    ];
    let from = now_micros();
    standalone.assert_call("realm_mail_commit", &commit);
    let by = now_micros();
    standalone.assert_call("realm_mail_commit", &commit);
    standalone.assert_call("realm_mail_confirm_delivery", &[escrow_id, &tester]);
    standalone.assert_call("realm_mail_settle", &[escrow_id, &tester]);
    (from, by)
}

/// A World Session turn-in files its Reward Letter as Escrow in the same transaction, and the
/// drive on one database writes one letter from the quest giver that arrives after the quest's
/// delay. A playerbot's turn-in, a quest with no reward mail and a refused turn-in file nothing.
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
        escrows(&standalone).is_empty(),
        "the Package turn-in path files no letter"
    );
    standalone.assert_call("debug_stage_reward_letter_fixture", &[]);

    assert!(turn_in(&standalone, PLAIN_QUEST).status.success());
    assert!(escrows(&standalone).is_empty(), "no reward mail, no letter");

    assert!(turn_in(&standalone, CARD_QUEST).status.success());
    assert!(
        !turn_in(&standalone, CARD_QUEST).status.success(),
        "a quest turns in once"
    );
    let held = escrows(&standalone);
    assert_eq!(held.len(), 1, "one letter for one turn-in: {held:?}");
    let card = &held[0];
    for (column, want) in [
        ("recipient_guid", TESTER),
        ("subject", ""),
        ("body", CARD_BODY),
        ("money", "0"),
        ("postage", "0"),
        ("item_entry", "5090050"),
        ("item_stack_count", "1"),
        ("item_durability", "70"),
        ("cod", "0"),
        ("delivery_delay_secs", "86400"),
        ("sender_kind", "3"),
        ("sender_entry", "620"),
        ("mail_template_id", "509091"),
    ] {
        assert_eq!(card[column], want, "{column}: {card:?}");
    }

    let (from, by) = drive(
        &standalone,
        &card["escrow_id"],
        CARD_BODY,
        "0",
        ["5090050", "1", "70"],
        "86400",
        "620",
        "509091",
    );
    assert!(escrows(&standalone).is_empty(), "settled");
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
        "the letter arrives 86,400 s after the commit: {mail:?}"
    );
    // Deliver it now, so the return reaches the sender Gate rather than the delivery one.
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

    assert!(turn_in(&standalone, ELDER_QUEST).status.success());
    let held = escrows(&standalone);
    assert_eq!(held.len(), 1, "{held:?}");
    let elder = &held[0];
    for (column, want) in [
        ("money", "1000000"),
        ("item_entry", "0"),
        ("delivery_delay_secs", "129600"),
        ("sender_kind", "3"),
        ("sender_entry", "11811"),
        ("mail_template_id", "509092"),
    ] {
        assert_eq!(elder[column], want, "{column}: {elder:?}");
    }
    let (from, by) = drive(
        &standalone,
        &elder["escrow_id"],
        ELDER_BODY,
        "1000000",
        ["0", "0", "0"],
        "129600",
        "11811",
        "509092",
    );
    let mail = letter_from(&standalone, "11811");
    assert_eq!(mail["money"], "1000000", "{mail:?}");
    let deliver: i64 = mail["deliver_micros"].parse().expect("a delivery instant");
    let delay = 129_600 * 1_000_000;
    assert!(
        (from + delay..=by + delay).contains(&deliver),
        "the letter arrives 129,600 s after the commit: {mail:?}"
    );
}

/// Commit the card renewal letter held under `escrow_id` on `mail_plane`, with the arguments its
/// escrow row holds.
fn commit_card(mail_plane: &Standalone, escrow_id: &str) {
    let body = quoted(CARD_BODY);
    mail_plane.assert_call(
        "realm_mail_commit",
        &[
            escrow_id,
            &actor(TESTER),
            TESTER,
            "\"\"",
            &body,
            "0",
            "5090050",
            "1",
            "70",
            "0",
            "false",
            "0",
            "0",
            "0",
            "86400",
            "3",
            "620",
            "509091",
        ],
    );
}

/// A Reward Letter still held when its Character crosses a Shard Boundary travels with the
/// Character, keeps its escrow id and is delivered from the new Home Shard once. The drive before
/// the hop reached the commit, so the drive after it replays the commit and writes no second
/// letter. The source standalone also plays Realm-core.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_held_reward_letter_travels_with_its_character_and_arrives_once() {
    let mut source = Standalone::start("reward-letter-source");
    source.publish_module();
    source.assert_call("claim_operator", &[]);
    source.assert_call("install_guid_range", &["0"]);
    source.assert_call("debug_spawn_player_entity", &[TESTER]);
    source.assert_call("debug_stage_reward_letter_fixture", &[]);
    assert!(turn_in(&source, CARD_QUEST).status.success());
    let held = escrows(&source);
    assert_eq!(held.len(), 1, "{held:?}");
    let escrow_id = held[0]["escrow_id"].clone();

    // The Gateway stopped after the commit, before the attestation.
    commit_card(&source, &escrow_id);

    let tester = actor(TESTER);
    source.assert_call(
        "begin_transfer",
        &["5090095", &tester, "0", "0", "0", "0", "0", "0", "true"],
    );
    let out = source.query_rows("SELECT blob FROM game_transfer_out WHERE transfer_id = 5090095");
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();
    let mut destination = Standalone::start("reward-letter-destination");
    destination.publish_module();
    destination.assert_call("claim_operator", &[]);
    destination.assert_call("install_guid_range", &["1000000000"]);
    let system = actor("0");
    destination.assert_call("import_character_blob", &["5090095", &blob, &system]);
    source.assert_call("confirm_import", &["5090095", &system]);
    source.assert_call("finish_transfer", &["5090095", &system]);
    destination.assert_call("release_transfer", &["5090095", &system]);

    assert!(
        escrows(&source).is_empty(),
        "the letter left with its Character"
    );
    let arrived = escrows(&destination);
    assert_eq!(arrived.len(), 1, "{arrived:?}");
    for (column, want) in [
        ("escrow_id", escrow_id.as_str()),
        ("delivered", "false"),
        ("sender_kind", "3"),
        ("sender_entry", "620"),
        ("mail_template_id", "509091"),
        ("item_entry", "5090050"),
        ("delivery_delay_secs", "86400"),
    ] {
        assert_eq!(arrived[0][column], want, "{column}: {arrived:?}");
    }

    destination.assert_call("debug_spawn_player_entity", &[TESTER]);
    commit_card(&source, &escrow_id);
    destination.assert_call("realm_mail_confirm_delivery", &[&escrow_id, &tester]);
    destination.assert_call("realm_mail_settle", &[&escrow_id, &tester]);
    assert!(
        escrows(&destination).is_empty(),
        "settled on the new Home Shard"
    );
    let mail = letter_from(&source, "620");
    assert_eq!(mail["mail_template_id"], "509091", "{mail:?}");
}
