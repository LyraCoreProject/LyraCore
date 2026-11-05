use std::collections::BTreeMap;

use spacetimedb::ReducerContext;

use super::transport::{decode_rows, encode_rows, TableRows};
use crate::mail_text::{game_item_text, ItemText};

const TEXT_TABLE: &str = "game_item_text";

fn remap(
    payload: &mut [TableRows],
    mut map: impl FnMut(u32) -> Result<u32, String>,
) -> Result<(), String> {
    macro_rules! rows {
        ($entry:expr, $row:ty, $($field:ident).+) => {{
            let mut outcome = Ok(());
            let mut rows = decode_rows::<$row>(&$entry.rows, &mut outcome);
            outcome?;
            for row in &mut rows {
                if row.$($field).+ != 0 {
                    row.$($field).+ = map(row.$($field).+)?;
                }
            }
            $entry.rows = encode_rows(rows);
        }};
    }
    for entry in payload {
        match entry.table.as_str() {
            "game_item_instance" => rows!(entry, crate::ItemInstance, item_text_id),
            "game_buyback_item" => rows!(entry, crate::items::BuybackItem, item.item_text_id),
            "game_mail" => rows!(entry, crate::mail::Mail, item_text_id),
            "game_mail_escrow" => rows!(entry, crate::mail_escrow::MailEscrow, item_text_id),
            "game_auction_hold" => rows!(entry, crate::auction::AuctionHold, item_text_id),
            _ => {}
        }
    }
    Ok(())
}

pub(super) fn export(ctx: &ReducerContext, payload: &mut Vec<TableRows>) {
    let mut ids = std::collections::BTreeSet::new();
    remap(payload, |id| {
        ids.insert(id);
        Ok(id)
    })
    .expect("exported item rows decode");
    if ids.is_empty() {
        return;
    }
    payload.push(TableRows {
        table: TEXT_TABLE.into(),
        rows: encode_rows(
            ids.into_iter()
                .filter_map(|id| ctx.db.game_item_text().id().find(id))
                .collect::<Vec<_>>(),
        ),
    });
}

pub(super) fn import(ctx: &ReducerContext, payload: &mut Vec<TableRows>) -> Result<(), String> {
    let mut counts = BTreeMap::<u32, u32>::new();
    remap(payload, |id| {
        *counts.entry(id).or_default() += 1;
        Ok(id)
    })?;
    let mut texts = BTreeMap::new();
    if let Some(index) = payload.iter().position(|entry| entry.table == TEXT_TABLE) {
        let entry = payload.remove(index);
        let mut outcome = Ok(());
        for text in decode_rows::<ItemText>(&entry.rows, &mut outcome) {
            if texts.insert(text.id, text.text).is_some() {
                return Err("Transfer repeats an Item Text id".into());
            }
        }
        outcome?;
    }
    let mut ids = BTreeMap::new();
    for (old, count) in counts {
        let text = texts
            .remove(&old)
            .ok_or_else(|| format!("Transfer is missing Item Text {old}"))?;
        ids.insert(old, crate::mail_text::import_text(ctx, text, count)?);
    }
    remap(payload, |id| {
        ids.get(&id)
            .copied()
            .ok_or_else(|| "Transfer Item Text mapping is incomplete".into())
    })
}

pub(super) fn release(ctx: &ReducerContext, guid: u64, include_holds: bool) {
    use super::game_transfer_out;
    let carried = ctx
        .db
        .game_transfer_out()
        .by_character()
        .filter(guid)
        .any(|out| {
            super::decode_blob(out.transfer_id, &out.blob)
                .ok()
                .is_some_and(|blob| blob.payload.iter().any(|entry| entry.table == TEXT_TABLE))
        });
    if !carried {
        return;
    }
    let mut payload =
        super::transport::export_rows_via(ctx, guid, crate::CHARACTER_OWNED_TRANSFERS);
    // The buyback deletion sweep releases its own Item Text references.
    payload.retain(|entry| entry.table != "game_buyback_item");
    if !include_holds {
        payload.retain(|entry| entry.table != "game_auction_hold");
    }
    remap(&mut payload, |id| {
        crate::mail_text::drop_letter_text(ctx, id);
        Ok(id)
    })
    .expect("exported item rows decode");
}
