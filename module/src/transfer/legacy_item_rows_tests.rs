use std::cell::Cell;

use super::*;
use crate::transfer::{decode_rows, encode_rows, import_rows_via, RowIo, TransportArm};

fn entry<R: SpacetimeType + spacetimedb::sats::Serialize>(table: &str, rows: Vec<R>) -> TableRows {
    TableRows {
        table: table.to_owned(),
        rows: encode_rows(rows),
    }
}

fn arrive<R>(entry: TableRows) -> Vec<R>
where
    R: SpacetimeType + for<'de> spacetimedb::sats::Deserialize<'de>,
{
    let payload = prepare(&[entry]).unwrap();
    let mut outcome = Ok(());
    let rows = decode_rows(&payload[0].rows, &mut outcome);
    outcome.unwrap();
    rows
}

#[test]
fn legacy_item_rows_keep_every_value_and_arrive_plain() {
    let rows = (1..=2)
        .map(|id| LegacyItemInstance {
            guid: id,
            entry: 509_0000 + id as u32,
            owner_identity: Identity::ZERO,
            owner_guid: 73,
            slot: 22 + id as u8,
            stack_count: id as u32,
            durability: 51,
            created_at: Timestamp::from_micros_since_unix_epoch(1234),
            enchant_id: 7745,
            soulbound: id == 1,
        })
        .collect();
    let arrived: Vec<ItemInstance> = arrive(entry("game_item_instance", rows));
    assert_eq!(arrived.len(), 2);
    for (index, row) in arrived.iter().enumerate() {
        let id = index as u64 + 1;
        assert_eq!(row.guid, id);
        assert_eq!(row.entry, 509_0000 + id as u32);
        assert_eq!(row.owner_identity, Identity::ZERO);
        assert_eq!(row.owner_guid, 73);
        assert_eq!(row.slot, 22 + id as u8);
        assert_eq!(row.stack_count, id as u32);
        assert_eq!(row.durability, 51);
        assert_eq!(
            row.created_at,
            Timestamp::from_micros_since_unix_epoch(1234)
        );
        assert_eq!(row.enchant_id, 7745);
        assert_eq!(row.soulbound, id == 1);
        assert_eq!(row.random_property_id, 0);
    }
}

#[test]
fn legacy_buyback_rows_keep_value_order_and_binding() {
    let rows = vec![
        LegacyBuybackEntry {
            id: 1,
            player_guid: 73,
            item_entry: 509_0001,
            stack_count: 3,
            price: 192,
            soulbound: true,
        },
        LegacyBuybackEntry {
            id: 2,
            player_guid: 73,
            item_entry: 509_0002,
            stack_count: 1,
            price: 42,
            soulbound: false,
        },
    ];
    let arrived: Vec<BuybackEntry> = arrive(entry("game_character_buyback", rows));
    let values: Vec<_> = arrived
        .iter()
        .map(|row| {
            (
                row.id,
                row.player_guid,
                row.item_entry,
                row.stack_count,
                row.price,
                row.soulbound,
                row.random_property_id,
            )
        })
        .collect();
    assert_eq!(
        values,
        [
            (1, 73, 509_0001, 3, 192, true, 0),
            (2, 73, 509_0002, 1, 42, false, 0)
        ]
    );
}

#[test]
fn legacy_mail_decodes_multiple_variable_length_rows_without_guessing() {
    let rows = [(1, "", "First body"), (2, "Second subject", "")]
        .into_iter()
        .map(|(id, subject, body)| LegacyMail {
            id,
            recipient_guid: 73,
            sender_guid: 91,
            subject: subject.to_owned(),
            body: body.to_owned(),
            item_entry: 509_0001,
            item_stack_count: 3,
            item_durability: 51,
            item_enchant_id: 7748,
            item_soulbound: false,
            money: 173,
            cod: 12,
            was_read: id == 1,
            created_at: Timestamp::from_micros_since_unix_epoch(1234),
        })
        .collect();
    let arrived: Vec<Mail> = arrive(entry("game_mail", rows));
    assert_eq!(arrived.len(), 2);
    assert_eq!(
        (&*arrived[0].subject, &*arrived[0].body),
        ("", "First body")
    );
    assert_eq!(
        (&*arrived[1].subject, &*arrived[1].body),
        ("Second subject", "")
    );
    for (index, row) in arrived.iter().enumerate() {
        assert_eq!(row.id, index as u64 + 1);
        assert_eq!(row.recipient_guid, 73);
        assert_eq!(row.sender_guid, 91);
        assert_eq!(row.item_entry, 509_0001);
        assert_eq!(row.item_stack_count, 3);
        assert_eq!(row.item_durability, 51);
        assert_eq!(row.item_enchant_id, 7748);
        assert!(!row.item_soulbound);
        assert_eq!(row.money, 173);
        assert_eq!(row.cod, 12);
        assert_eq!(row.was_read, index == 0);
        assert_eq!(
            row.created_at,
            Timestamp::from_micros_since_unix_epoch(1234)
        );
        assert_eq!(row.random_property_id, 0);
    }
}

#[test]
fn current_item_formats_preserve_the_exact_encoded_values() {
    for (table, _) in FORMATS {
        let mut payload = vec![TableRows {
            table: (*table).to_owned(),
            rows: vec![1, 2, 3, 4],
        }];
        let original = payload.clone();
        mark_current(&mut payload);
        assert_ne!(payload[0].table, *table);
        assert_eq!(prepare(&payload).unwrap(), original);
    }
    let mut payload = vec![entry(
        "game_character_buyback",
        vec![BuybackEntry {
            id: 1,
            player_guid: 73,
            item_entry: 509_0001,
            stack_count: 1,
            price: 192,
            soulbound: true,
            random_property_id: 1182,
        }],
    )];
    mark_current(&mut payload);
    let arrived: Vec<BuybackEntry> = arrive(payload.remove(0));
    assert_eq!(arrived[0].random_property_id, 1182);
}

fn count_import(ctx: &Cell<usize>, _: u64, _: &mut RowIo<'_>) {
    ctx.set(ctx.get() + 1);
}

#[test]
fn the_previous_import_contract_refuses_new_formats_before_applying_rows() {
    let applied = Cell::new(0);
    let arms: &[TransportArm<'_, Cell<usize>>] = &[("game_item_instance", count_import)];
    let mut payload = vec![TableRows {
        table: "game_item_instance".to_owned(),
        rows: Vec::new(),
    }];
    mark_current(&mut payload);
    let error = import_rows_via(&applied, 73, &payload, arms).unwrap_err();
    assert!(error.contains("game_item_instance"));
    assert_eq!(applied.get(), 0);
}

#[test]
fn malformed_legacy_rows_and_duplicate_format_aliases_are_refused() {
    let malformed = TableRows {
        table: "game_mail".to_owned(),
        rows: vec![1],
    };
    assert!(prepare(&[malformed])
        .unwrap_err()
        .contains("legacy item rows"));
    let old = TableRows {
        table: "game_item_instance".to_owned(),
        rows: Vec::new(),
    };
    let mut new = vec![old.clone()];
    mark_current(&mut new);
    assert!(prepare(&[old, new.remove(0)])
        .unwrap_err()
        .contains("repeats table"));
}

#[test]
fn neither_row_format_accepts_an_unconsumed_suffix() {
    let mut bytes = encode_rows(Vec::<LegacyMail>::new());
    bytes.extend_from_slice(&[1, 2, 3, 4]);
    let entry = TableRows {
        table: "game_mail".to_owned(),
        rows: bytes.clone(),
    };
    assert!(prepare(&[entry]).unwrap_err().contains("trailing bytes"));
    let mut outcome = Ok(());
    let rows = decode_rows::<Mail>(&bytes, &mut outcome);
    assert!(rows.is_empty());
    assert!(outcome.unwrap_err().contains("trailing bytes"));
}

#[test]
fn unrelated_and_empty_legacy_tables_keep_their_payloads() {
    let payload = vec![
        TableRows {
            table: "game_aura".to_owned(),
            rows: vec![9, 1, 7],
        },
        TableRows {
            table: "game_mail".to_owned(),
            rows: Vec::new(),
        },
    ];
    assert_eq!(prepare(&payload).unwrap(), payload);
}
