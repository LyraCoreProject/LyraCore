mod support;

use support::Standalone;

const CHECK_MASK_COPIED: u32 = 0x04;

/// Is COPIED set in a `game_mail.check_flags` cell? A non-empty-body letter also carries HAS_BODY
/// (0x10), so the raw column value after a copy is 0x14, not 0x04 alone — check the bit, not the
/// whole byte.
fn has_copied(check_flags: &str) -> bool {
    check_flags.parse::<u32>().unwrap() & CHECK_MASK_COPIED != 0
}

fn fixture(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard.assert_call("install_guid_range", &["0"]);
    shard
}

fn actor(guid: u64) -> String {
    format!(r#"{{"guid":{guid},"ownership":null}}"#)
}

fn seed_mail(shard: &Standalone, recipient_guid: u64, sender_guid: u64, body: &str) -> u64 {
    shard.assert_call(
        "debug_seed_mail",
        &[
            &recipient_guid.to_string(),
            &sender_guid.to_string(),
            "\"Your sword\"",
            &format!("{body:?}"),
            "0",
        ],
    );
    shard.query_rows(&format!(
        "SELECT id FROM game_mail WHERE recipient_guid = {recipient_guid}"
    ))[0]["id"]
        .parse()
        .unwrap()
}

/// Seeds the "Plain Letter" (item 8383) template a real ClassicDB import carries, since the fixture
/// shards in these tests start with none. Every column bar `entry`/`name`/`max_stack`/`buy_count`/
/// `allowed_class`/`allowed_race` is a placeholder value — `grant_letter_item` needs the row to
/// exist, not any particular stat on it.
fn seed_letter_item_template(shard: &Standalone) {
    shard.assert_sql(
        "INSERT INTO game_item_template (entry,class,subclass,name,display_id,quality,\
         inventory_type,item_level,required_level,max_durability,buy_price,sell_price,max_stack,\
         damage_min,damage_max,delay_ms,stat_strength,stat_agility,stat_stamina,stat_intellect,\
         stat_spirit,stat_crit,stat_hit,stat_armor,block_value,restores_power,spellid_1,\
         spelltrigger_1,spellid_2,spelltrigger_2,container_slots,sheath,bonding,holy_res,fire_res,\
         nature_res,frost_res,shadow_res,arcane_res,spellid_3,spelltrigger_3,spellid_4,\
         spelltrigger_4,spellid_5,spelltrigger_5,required_skill,required_skill_rank,\
         required_reputation_faction,required_reputation_rank,max_count,item_flags,page_text,\
         start_quest,bag_family,buy_count,food_type,allowed_class,allowed_race,random_property) \
         VALUES (8383,0,0,'Plain Letter',0,0,0,0,0,0,0,0,1,0.0,0.0,0,0,0,0,0,0,0,0,0,0,false,0,0,\
         0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,1503,255,0)",
    );
}

/// `realm_mail_copy_text` (the reducer `apply_copy_text` answers through) against a real database:
/// COPIED lands on the mail and its body becomes durable item text under the mail's own id.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn copying_a_letter_sets_copied_and_files_its_body_as_item_text() {
    let shard = fixture("mail-letter-copy-basic");
    let mail_id = seed_mail(&shard, 1, 2, "left it at the inn");

    shard.assert_call("realm_mail_copy_text", &[&actor(1), &mail_id.to_string()]);

    let mail = shard.query_rows(&format!(
        "SELECT check_flags FROM game_mail WHERE id = {mail_id}"
    ));
    assert!(has_copied(&mail[0]["check_flags"]));
    let text = shard.query_rows(&format!(
        "SELECT text FROM game_item_text WHERE id = {mail_id}"
    ));
    assert_eq!(text[0]["text"], "left it at the inn");
}

/// A redundant copy click replays rather than erroring, and files no second text row.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn copying_the_same_letter_twice_replays_without_a_second_text_row() {
    let shard = fixture("mail-letter-copy-twice");
    let mail_id = seed_mail(&shard, 1, 2, "left it at the inn");

    shard.assert_call("realm_mail_copy_text", &[&actor(1), &mail_id.to_string()]);
    shard.assert_call("realm_mail_copy_text", &[&actor(1), &mail_id.to_string()]);

    let rows = shard.query_rows(&format!(
        "SELECT id FROM game_item_text WHERE id = {mail_id}"
    ));
    assert_eq!(rows.len(), 1, "one text row, not two");
}

/// `mail::returned` keeps a mail's id when it goes back to its sender. If Bob copies a letter and
/// then returns it, Alice's copy of the SAME id must not panic on the text row Bob already filed —
/// it reuses it, since a mail's body never changes.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_returned_letters_reused_id_copies_again_without_panicking() {
    let shard = fixture("mail-letter-copy-returned");
    let alice = 1u64;
    let bob = 2u64;
    let mail_id = seed_mail(&shard, bob, alice, "left it at the inn");

    shard.assert_call("realm_mail_copy_text", &[&actor(bob), &mail_id.to_string()]);
    shard.assert_call(
        "realm_mail_return",
        &[&actor(bob), &mail_id.to_string(), "true"],
    );
    let returned = shard.query_rows(&format!(
        "SELECT recipient_guid, check_flags FROM game_mail WHERE id = {mail_id}"
    ));
    assert_eq!(returned[0]["recipient_guid"], alice.to_string());
    assert_eq!(
        returned[0]["check_flags"], "2",
        "RETURNED only — COPIED does not survive a return"
    );

    // Must not panic on the primary-key conflict the reused id would hit.
    shard.assert_call(
        "realm_mail_copy_text",
        &[&actor(alice), &mail_id.to_string()],
    );

    let after = shard.query_rows(&format!(
        "SELECT check_flags FROM game_mail WHERE id = {mail_id}"
    ));
    assert!(has_copied(&after[0]["check_flags"]));
    let text = shard.query_rows(&format!(
        "SELECT text FROM game_item_text WHERE id = {mail_id}"
    ));
    assert_eq!(text.len(), 1, "still exactly one text row");
    assert_eq!(text[0]["text"], "left it at the inn");
}

/// `gw_mail_grant_letter` (`items::grant_letter_item`) is idempotent by text id: a retry after an
/// earlier grant already landed — the recovery path for a grant interrupted by bags filling,
/// logout, a timeout, or a Gateway crash — must not mint a second Plain Letter.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn granting_a_letter_a_second_time_does_not_duplicate_the_item() {
    let shard = fixture("mail-letter-grant-twice");
    seed_letter_item_template(&shard);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let mail_id = seed_mail(&shard, 1, 2, "left it at the inn");
    shard.assert_call("realm_mail_copy_text", &[&actor(1), &mail_id.to_string()]);

    shard.assert_call("gw_mail_grant_letter", &[&actor(1), &mail_id.to_string()]);
    shard.assert_call("gw_mail_grant_letter", &[&actor(1), &mail_id.to_string()]);

    let granted = shard.query_rows(&format!(
        "SELECT guid FROM game_item_instance WHERE owner_guid = 1 AND entry = 8383 AND item_text_id = {mail_id}"
    ));
    assert_eq!(granted.len(), 1, "one Plain Letter, not two");
}

/// The held-item check `grant_letter_item` runs is only the guard for the narrow window before
/// GRANTED is recorded. Once the player has destroyed the granted letter, `realm_mail_copy_text`
/// must refuse a second copy on GRANTED alone — copy, grant, destroy, copy again must not mint the
/// letter over and over. The Gateway's own `copy_letter` never calls `gw_mail_grant_letter` unless
/// the copy step just succeeded, so a refused copy is what keeps a second grant from happening.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn copying_grant_and_destroying_the_letter_refuses_a_second_grant() {
    let shard = fixture("mail-letter-copy-grant-destroy");
    seed_letter_item_template(&shard);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let mail_id = seed_mail(&shard, 1, 2, "left it at the inn");
    shard.assert_call("realm_mail_copy_text", &[&actor(1), &mail_id.to_string()]);
    shard.assert_call("gw_mail_grant_letter", &[&actor(1), &mail_id.to_string()]);
    shard.assert_call(
        "realm_mail_mark_letter_granted",
        &[&actor(1), &mail_id.to_string()],
    );
    shard.assert_sql("DELETE FROM game_item_instance WHERE owner_guid = 1 AND entry = 8383");

    let refused = shard.call("realm_mail_copy_text", &[&actor(1), &mail_id.to_string()]);
    assert!(
        !refused.status.success(),
        "GRANTED must refuse a second copy even with the item gone"
    );

    let granted = shard.query_rows(&format!(
        "SELECT guid FROM game_item_instance WHERE owner_guid = 1 AND entry = 8383 AND item_text_id = {mail_id}"
    ));
    assert!(
        granted.is_empty(),
        "the copy step refused, so the Gateway never reaches the grant — no letter is minted"
    );
}

/// Creates the second Character, "Partner", on Account 1 and spawns it next to Character 1.
fn spawn_partner(shard: &Standalone) -> u64 {
    shard.assert_call(
        "create_character",
        &["1", "\"Partner\"", "1", "1", "0", "0", "0", "0", "0", "0"],
    );
    let partner: u64 = shard.query_rows("SELECT guid FROM game_character WHERE name = 'Partner'")
        [0]["guid"]
        .parse()
        .unwrap();
    shard.assert_call("debug_spawn_player_entity", &[&partner.to_string()]);
    partner
}

/// Copies `mail_id` into a Plain Letter in Character 1's bags and answers the letter's item guid.
fn copy_into_bags(shard: &Standalone, mail_id: u64) -> String {
    shard.assert_call("realm_mail_copy_text", &[&actor(1), &mail_id.to_string()]);
    shard.assert_call("gw_mail_grant_letter", &[&actor(1), &mail_id.to_string()]);
    shard.assert_call(
        "realm_mail_mark_letter_granted",
        &[&actor(1), &mail_id.to_string()],
    );
    shard.query_rows(&format!(
        "SELECT guid FROM game_item_instance WHERE owner_guid = 1 AND item_text_id = {mail_id}"
    ))[0]["guid"]
        .clone()
}

/// Sends the Plain Letter `letter_guid` from `sender` to `recipient` on one Realm Account, so it
/// arrives at once, and answers the new mail's id.
fn mail_letter(shard: &Standalone, sender: u64, recipient: u64, letter_guid: &str) -> u64 {
    shard.assert_call("debug_set_money", &[&sender.to_string(), "1000"]);
    shard.assert_call(
        "realm_mail_send",
        &[
            &actor(sender),
            &recipient.to_string(),
            "\"A letter\"",
            "\"\"",
            "0",
            "0",
            letter_guid,
            "true",
        ],
    );
    shard.query_rows(&format!(
        "SELECT id FROM game_mail WHERE recipient_guid = {recipient} AND item_entry = 8383"
    ))[0]["id"]
        .parse()
        .unwrap()
}

/// A Trade Commit rebuilds the letter on the far side from its snapshot. The snapshot carries the
/// text id, so the letter stays readable.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_traded_plain_letter_keeps_its_text() {
    let shard = fixture("trade-letter-copy");
    seed_letter_item_template(&shard);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let partner = spawn_partner(&shard);
    let mail_id = seed_mail(&shard, 1, 2, "left it at the inn");
    copy_into_bags(&shard, mail_id);
    let letter_slot = shard.query_rows(&format!(
        "SELECT slot FROM game_item_instance WHERE owner_guid = 1 AND item_text_id = {mail_id}"
    ))[0]["slot"]
        .clone();

    shard.assert_call("gw_initiate_trade", &[&actor(1), &partner.to_string()]);
    shard.assert_call("gw_begin_trade", &[&actor(partner)]);
    shard.assert_call("gw_set_trade_item", &[&actor(1), "0", &letter_slot]);
    shard.assert_call("gw_accept_trade", &[&actor(1)]);
    shard.assert_call("gw_accept_trade", &[&actor(partner)]);

    let received = shard.query_rows(&format!(
        "SELECT item_text_id FROM game_item_instance WHERE owner_guid = {partner} AND entry = 8383"
    ));
    assert_eq!(received.len(), 1, "the letter changed hands once");
    assert_eq!(received[0]["item_text_id"], mail_id.to_string());
    assert!(shard
        .query_rows("SELECT guid FROM game_item_instance WHERE owner_guid = 1 AND entry = 8383")
        .is_empty());
}

/// A mailed Plain Letter arrives with its text id. The text row counts the one letter, and it goes
/// when that letter is deleted with the mail that carries it, although the mail it was copied from
/// is still in the mailbox.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_mailed_plain_letter_keeps_its_text_until_it_is_destroyed() {
    let shard = fixture("mail-letter-copy-mailed");
    seed_letter_item_template(&shard);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let partner = spawn_partner(&shard);
    let mail_id = seed_mail(&shard, 1, 2, "left it at the inn");
    let letter = copy_into_bags(&shard, mail_id);
    let text_query = format!("SELECT text, letters FROM game_item_text WHERE id = {mail_id}");
    assert_eq!(shard.query_rows(&text_query)[0]["letters"], "1");

    let to_partner = mail_letter(&shard, 1, partner, &letter);
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT item_text_id FROM game_mail WHERE id = {to_partner}"
        ))[0]["item_text_id"],
        mail_id.to_string()
    );
    shard.assert_call(
        "realm_mail_take_item",
        &[&actor(partner), &to_partner.to_string()],
    );
    let held = shard.query_rows(&format!(
        "SELECT guid, item_text_id FROM game_item_instance WHERE owner_guid = {partner} AND entry = 8383"
    ));
    assert_eq!(held[0]["item_text_id"], mail_id.to_string());
    let text = shard.query_rows(&text_query);
    assert_eq!(
        [&text[0]["text"], &text[0]["letters"]],
        ["left it at the inn", "1"],
        "moving a letter neither copies nor destroys its text"
    );

    let back = mail_letter(&shard, partner, 1, &held[0]["guid"]);
    shard.assert_call("realm_mail_delete", &[&actor(1), &back.to_string()]);

    assert!(
        shard.query_rows(&text_query).is_empty(),
        "the text went with its last letter"
    );
    assert_eq!(
        shard
            .query_rows(&format!("SELECT id FROM game_mail WHERE id = {mail_id}"))
            .len(),
        1,
        "the mail the letter was copied from is untouched"
    );
}

/// `apply_copy_text` reads the mail through `mail::delivered_mail`, so a mail not yet at its
/// delivery instant does not exist for `realm_mail_copy_text` either — the same rule that already
/// keeps an undelivered mail from being read, deleted, taken from or returned.
#[test]
#[ignore = "requires the SpacetimeDB 2.7.1 CLI and Wasm toolchain"]
fn a_letter_not_yet_delivered_cannot_be_copied() {
    let shard = fixture("mail-letter-copy-not-delivered");
    let mail_id = seed_mail(&shard, 1, 2, "left it at the inn");
    shard.assert_sql(&format!(
        "UPDATE game_mail SET deliver_micros = 9999999999999999 WHERE id = {mail_id}"
    ));

    let refused = shard.call("realm_mail_copy_text", &[&actor(1), &mail_id.to_string()]);
    assert!(
        !refused.status.success(),
        "an undelivered letter has no text to copy yet"
    );

    let mail = shard.query_rows(&format!(
        "SELECT check_flags FROM game_mail WHERE id = {mail_id}"
    ));
    assert!(
        !has_copied(&mail[0]["check_flags"]),
        "a refused copy must not touch the mail plane at all"
    );
    assert!(
        shard
            .query_rows(&format!(
                "SELECT id FROM game_item_text WHERE id = {mail_id}"
            ))
            .is_empty(),
        "and it must not file item text for a letter nobody has read yet"
    );
}
