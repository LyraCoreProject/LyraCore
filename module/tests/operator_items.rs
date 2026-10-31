mod support;

use serde_json::{json, Value};
use support::{number, Standalone};

const FIRST_ITEM: u32 = 5_200_500;
const VENDOR: u32 = 620;

fn realm(name: &str) -> Standalone {
    let mut shard = Standalone::start(name);
    shard.publish_module();
    shard.assert_call("claim_operator", &[]);
    shard
}

fn item(shard: &Standalone, entry: u32) -> Option<support::Row> {
    shard
        .query_rows(&format!(
            "SELECT * FROM game_item_template WHERE entry = {entry}"
        ))
        .pop()
}

fn fields(shard: &Standalone) -> Value {
    Value::Object(
        item(shard, 25)
            .unwrap()
            .into_iter()
            .filter(|(key, _)| key != "entry" && !key.starts_with("spellppmrate_"))
            .map(|(key, value)| {
                let value = if key == "name" {
                    json!(value)
                } else {
                    serde_json::from_str(&value).unwrap()
                };
                (key, value)
            })
            .collect(),
    )
}

fn apply(shard: &Standalone, edits: Value) {
    shard.assert_call("operator_apply_item_edits", &[&edits.to_string()]);
}

fn refusal(shard: &Standalone, edits: Value, expected: &str) {
    let output = shard.call("operator_apply_item_edits", &[&edits.to_string()]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success(), "{text}");
    assert!(text.contains(expected), "{text}");
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn only_the_operator_can_edit_items_even_in_an_empty_batch() {
    let shard = realm("operator-items-authority");
    let output = shard.call_anonymous("operator_apply_item_edits", &["[]"]);
    assert!(!output.status.success());
    apply(&shard, json!([]));
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn item_create_and_full_update_use_the_operator_template_range() {
    let shard = realm("operator-items-create");
    let mut fields = fields(&shard);
    apply(
        &shard,
        json!([
            {"createItem": {"fields": fields}},
            {"createItem": {"fields": fields}}
        ]),
    );
    assert_eq!(
        number::<u32>(&item(&shard, FIRST_ITEM).unwrap(), "entry"),
        FIRST_ITEM
    );
    assert!(item(&shard, FIRST_ITEM + 1).is_some());
    fields["name"] = json!("Meld blade");
    fields["stat_strength"] = json!(17);
    apply(
        &shard,
        json!([{"updateItem": {"entry": FIRST_ITEM, "fields": fields}}]),
    );
    let row = item(&shard, FIRST_ITEM).unwrap();
    assert_eq!(row["name"], "Meld blade");
    assert_eq!(number::<i32>(&row, "stat_strength"), 17);
    apply(&shard, json!([{"createItem": {"fields": fields}}]));
    assert!(item(&shard, FIRST_ITEM + 2).is_some());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn protected_items_missing_spells_and_invalid_fields_refuse() {
    let shard = realm("operator-items-refusals");
    let fields = fields(&shard);
    for (entry, reason) in [
        (7_000_000, "Package Item Range"),
        (5_090_000, "Fixture-Reserved"),
        (999_999, "does not exist"),
    ] {
        refusal(
            &shard,
            json!([{"updateItem": {"entry": entry, "fields": fields}}]),
            reason,
        );
    }
    let mut bad = fields.clone();
    bad["spellid_5"] = json!(4_000_000_000u32);
    refusal(
        &shard,
        json!([{"createItem": {"fields": bad}}]),
        "spell 4000000000 does not exist",
    );
    bad = fields.clone();
    bad["damage_min"] = json!(100);
    bad["damage_max"] = json!(1);
    refusal(
        &shard,
        json!([{"updateItem": {"entry": 25, "fields": bad}}]),
        "damage",
    );
    refusal(
        &shard,
        Value::Array(vec![json!({"createItem": {"fields": fields}}); 65]),
        "at most 64 item edits",
    );
    assert!(item(&shard, FIRST_ITEM).is_none());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn a_refused_item_batch_rolls_back_templates_vendors_and_success_logs() {
    let shard = realm("operator-items-rollback");
    let before = item(&shard, 25).unwrap();
    let mut fields = fields(&shard);
    fields["name"] = json!("Must roll back");
    refusal(
        &shard,
        json!([
            {"createItem": {"fields": fields}},
            {"updateItem": {"entry": 25, "fields": fields}},
            {"setVendorOffers": {"creature_entry": VENDOR, "offers": [{"item_entry": FIRST_ITEM, "max_count": 0}]}},
            {"setVendorOffers": {"creature_entry": VENDOR, "offers": [{"item_entry": 999999, "max_count": 0}]}}
        ]),
        "item edit 3",
    );
    assert!(item(&shard, FIRST_ITEM).is_none());
    assert_eq!(item(&shard, 25).unwrap(), before);
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_npc_vendor WHERE creature_entry = {VENDOR}"
        ))
        .is_empty());
    let logs = shard.module_logs(100);
    assert!(!String::from_utf8_lossy(&logs.stdout).contains("create item 5200500"));
    apply(&shard, json!([{"createItem": {"fields": fields}}]));
    assert!(item(&shard, FIRST_ITEM).is_some());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_replacement_allocates_above_imported_ids_and_preserves_list_order() {
    let shard = realm("operator-items-vendor");
    shard.assert_sql(&format!("INSERT INTO game_npc_vendor (id, creature_entry, item_entry, slot, max_count) VALUES (90000000, {VENDOR}, 25, 1, 0)"));
    apply(
        &shard,
        json!([{"setVendorOffers": {"creature_entry": VENDOR, "offers": [
            {"item_entry": 50, "max_count": 3}, {"item_entry": 25, "max_count": 0}
        ]}}]),
    );
    let mut rows = shard.query_rows(&format!(
        "SELECT * FROM game_npc_vendor WHERE creature_entry = {VENDOR}"
    ));
    rows.sort_by_key(|row| number::<u32>(row, "slot"));
    assert_eq!(rows.len(), 2);
    assert_eq!(number::<u64>(&rows[0], "id"), 90_000_001);
    assert_eq!(number::<u32>(&rows[0], "item_entry"), 50);
    assert_eq!(number::<u32>(&rows[0], "slot"), 0);
    assert_eq!(number::<u32>(&rows[0], "max_count"), 3);
    assert_eq!(number::<u64>(&rows[1], "id"), 90_000_002);
    assert_eq!(number::<u32>(&rows[1], "item_entry"), 25);
    assert_eq!(number::<u32>(&rows[1], "slot"), 1);
    apply(
        &shard,
        json!([{"setVendorOffers": {"creature_entry": VENDOR, "offers": []}}]),
    );
    assert!(shard
        .query_rows(&format!(
            "SELECT * FROM game_npc_vendor WHERE creature_entry = {VENDOR}"
        ))
        .is_empty());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn vendor_refusals_preserve_the_old_offer_list() {
    let shard = realm("operator-items-vendor-refusal");
    let offer = json!({"item_entry": 25, "max_count": 0});
    apply(
        &shard,
        json!([{"setVendorOffers": {"creature_entry": VENDOR, "offers": [offer]}}]),
    );
    let query = format!("SELECT * FROM game_npc_vendor WHERE creature_entry = {VENDOR}");
    let before = shard.query_rows(&query);
    for (entry, offers, reason) in [
        (
            999_999,
            json!([]),
            "creature template 999999 does not exist",
        ),
        (VENDOR, json!([offer, offer]), "duplicate vendor item 25"),
        (
            VENDOR,
            json!([{"item_entry": 999999, "max_count": 0}]),
            "item 999999 does not exist",
        ),
        (
            VENDOR,
            Value::Array(vec![offer.clone(); 129]),
            "at most 128 offers",
        ),
        (
            VENDOR,
            Value::Array(vec![offer; 513]),
            "at most 512 vendor offers",
        ),
    ] {
        refusal(
            &shard,
            json!([{"setVendorOffers": {"creature_entry": entry, "offers": offers}}]),
            reason,
        );
        assert_eq!(shard.query_rows(&query), before);
    }
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn item_edits_survive_plain_publish() {
    let mut shard = realm("operator-items-publish");
    apply(
        &shard,
        json!([
            {"createItem": {"fields": fields(&shard)}},
            {"setVendorOffers": {"creature_entry": VENDOR, "offers": [{"item_entry": FIRST_ITEM, "max_count": 0}]}}
        ]),
    );
    let before = item(&shard, FIRST_ITEM).unwrap();
    let query = format!("SELECT * FROM game_npc_vendor WHERE creature_entry = {VENDOR}");
    let offers = shard.query_rows(&query);
    let mut wasm = support::module_bytes().to_vec();
    // An empty Wasm custom section changes the program hash without changing its definition.
    wasm.extend_from_slice(&[0, 2, 1, b'x']);
    shard.publish_module_bytes(&wasm);
    shard.assert_call("debug_repair_after_publish", &[]);
    assert_eq!(item(&shard, FIRST_ITEM).unwrap(), before);
    assert_eq!(shard.query_rows(&query), offers);
    apply(&shard, json!([{"createItem": {"fields": fields(&shard)}}]));
    assert!(item(&shard, FIRST_ITEM + 1).is_some());
}

#[test]
#[ignore = "requires the SpacetimeDB 2.14.0 CLI and Wasm toolchain"]
fn an_update_keeps_an_imported_spell_reference_the_realm_lacks() {
    let shard = realm("operator-items-held-spell");
    let mut fields = fields(&shard);
    apply(&shard, json!([{"createItem": {"fields": fields}}]));
    // An imported row may name a spell the realm never imported.
    shard.assert_sql(&format!(
        "UPDATE game_item_template SET spellid_2 = 4000000000 WHERE entry = {FIRST_ITEM}"
    ));
    fields["spellid_2"] = json!(4_000_000_000u32);
    fields["name"] = json!("Renamed");
    apply(
        &shard,
        json!([{"updateItem": {"entry": FIRST_ITEM, "fields": fields}}]),
    );
    assert_eq!(item(&shard, FIRST_ITEM).unwrap()["name"], "Renamed");
    fields["spellid_3"] = json!(4_000_000_001u32);
    refusal(
        &shard,
        json!([{"updateItem": {"entry": FIRST_ITEM, "fields": fields}}]),
        "spell 4000000001 does not exist",
    );
}
