mod support;

use support::Standalone;

const CHECK_MASK_COPIED: u32 = 0x04;

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

fn assert_copy_refused(shard: &Standalone, mail_id: u64, reason: &str) {
    let output = shard.call("realm_mail_copy_text", &[&actor(1), &mail_id.to_string()]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success() && text.contains(reason),
        "the copy should refuse with {reason}: {text}"
    );
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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
    let letter = shard
        .query_rows("SELECT slot FROM game_item_instance WHERE owner_guid = 1 AND entry = 8383");
    shard.assert_call("gw_destroy_item", &[&actor(1), &letter[0]["slot"], "0"]);
    assert!(shard
        .query_rows(&format!(
            "SELECT id FROM game_item_text WHERE id = {mail_id}"
        ))
        .is_empty());

    assert_copy_refused(
        &shard,
        mail_id,
        "mail: this letter was already made permanent",
    );

    let granted = shard.query_rows(&format!(
        "SELECT guid FROM game_item_instance WHERE owner_guid = 1 AND entry = 8383 AND item_text_id = {mail_id}"
    ));
    assert!(
        granted.is_empty(),
        "the refused copy grants no second letter"
    );
}

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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_letter_not_yet_delivered_cannot_be_copied() {
    let shard = fixture("mail-letter-copy-not-delivered");
    let mail_id = seed_mail(&shard, 1, 2, "left it at the inn");
    shard.assert_sql(&format!(
        "UPDATE game_mail SET deliver_micros = 9999999999999999 WHERE id = {mail_id}"
    ));

    assert_copy_refused(&shard, mail_id, "mail: not addressed to you");

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

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_transfer_keeps_the_text_of_the_letters_it_carries() {
    let source = fixture("mail-letter-copy-transfer-source");
    seed_letter_item_template(&source);
    source.assert_call("debug_spawn_player_entity", &["1"]);
    let partner = spawn_partner(&source);
    let mail_id = seed_mail(&source, 1, 2, "left it at the inn");
    let letter = copy_into_bags(&source, mail_id);
    let to_partner = mail_letter(&source, 1, partner, &letter);

    source.assert_call(
        "begin_transfer",
        &[
            "5090096",
            &actor(partner),
            "0",
            "0",
            "0",
            "0",
            "0",
            "0",
            "true",
        ],
    );
    let out = source.query_rows("SELECT blob FROM game_transfer_out WHERE transfer_id = 5090096");
    let blob = serde_json::to_string(out[0]["blob"].strip_prefix("0x").unwrap()).unwrap();
    let mut destination = Standalone::start("mail-letter-copy-transfer-destination");
    destination.publish_module();
    destination.assert_call("claim_operator", &[]);
    destination.assert_call("install_guid_range", &["1000000000"]);
    let system = actor(0);
    destination.assert_call("import_character_blob", &["5090096", &blob, &system]);
    source.assert_call("confirm_import", &["5090096", &system]);
    source.assert_call("finish_transfer", &["5090096", &system]);

    assert!(
        source
            .query_rows(&format!("SELECT id FROM game_mail WHERE id = {to_partner}"))
            .is_empty(),
        "the Mail left with its recipient"
    );
    assert_eq!(
        destination.query_rows(&format!(
            "SELECT item_text_id FROM game_mail WHERE recipient_guid = {partner}"
        ))[0]["item_text_id"],
        mail_id.to_string()
    );
    let text = source.query_rows(&format!(
        "SELECT text, letters FROM game_item_text WHERE id = {mail_id}"
    ));
    assert_eq!(
        [&text[0]["text"], &text[0]["letters"]],
        ["left it at the inn", "1"],
        "a letter that travels is not destroyed"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn deleting_a_character_destroys_its_letters_text() {
    let shard = fixture("mail-letter-copy-character-deleted");
    seed_letter_item_template(&shard);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let mail_id = seed_mail(&shard, 1, 2, "left it at the inn");
    copy_into_bags(&shard, mail_id);
    let text_query = format!("SELECT letters FROM game_item_text WHERE id = {mail_id}");
    assert_eq!(shard.query_rows(&text_query)[0]["letters"], "1");

    shard.assert_sql("DELETE FROM game_world_entity WHERE guid = 1");
    let account_id = shard.query_rows("SELECT account_id FROM game_character WHERE guid = 1")[0]
        ["account_id"]
        .clone();
    shard.assert_call("delete_character", &[&account_id, &actor(1)]);

    assert!(
        shard.query_rows(&text_query).is_empty(),
        "the text went with the deleted Character's letter"
    );
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn deleting_a_character_drops_the_text_of_the_letter_in_its_mail_once() {
    let shard = fixture("mail-letter-copy-mail-owner-deleted");
    seed_letter_item_template(&shard);
    shard.assert_call("debug_spawn_player_entity", &["1"]);
    let partner = spawn_partner(&shard);
    let mail_id = seed_mail(&shard, 1, 2, "left it at the inn");
    let letter = copy_into_bags(&shard, mail_id);
    let to_partner = mail_letter(&shard, 1, partner, &letter);
    shard.assert_sql(&format!(
        "UPDATE game_item_text SET letters = 2 WHERE id = {mail_id}"
    ));

    shard.assert_sql(&format!(
        "DELETE FROM game_world_entity WHERE guid = {partner}"
    ));
    let account_id = shard.query_rows(&format!(
        "SELECT account_id FROM game_character WHERE guid = {partner}"
    ))[0]["account_id"]
        .clone();
    shard.assert_call("delete_character", &[&account_id, &actor(partner)]);

    assert!(
        shard
            .query_rows(&format!("SELECT id FROM game_mail WHERE id = {to_partner}"))
            .is_empty(),
        "the Mail went with its recipient"
    );
    assert_eq!(
        shard.query_rows(&format!(
            "SELECT letters FROM game_item_text WHERE id = {mail_id}"
        ))[0]["letters"],
        "1"
    );
}
