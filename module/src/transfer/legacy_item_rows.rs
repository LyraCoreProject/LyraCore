//! Preserve plain items in Escrow created before Random Property fields existed.

use std::collections::BTreeSet;

use spacetimedb::{Identity, SpacetimeType, Timestamp};

use super::TableRows;
use crate::items::{BuybackEntry, ItemInstance};
use crate::mail::Mail;

const FORMATS: &[(&str, &str)] = &[
    ("game_item_instance", "game_item_instance@random-property-1"),
    (
        "game_character_buyback",
        "game_character_buyback@random-property-1",
    ),
    ("game_mail", "game_mail@random-property-1"),
];

/// Older Modules require canonical payload names and refuse these tags before applying rows.
pub(super) fn mark_current(payload: &mut [TableRows]) {
    for entry in payload {
        if let Some((_, tagged)) = FORMATS.iter().find(|(table, _)| *table == entry.table) {
            entry.table = (*tagged).to_owned();
        }
    }
}

/// Defaults migrate table rows, but do not rewrite the opaque bytes in existing Transfer Escrow.
/// Decode each untagged item table with its old shape; never guess its format from a decode error.
pub(super) fn prepare(payload: &[TableRows]) -> Result<Vec<TableRows>, String> {
    let mut seen = BTreeSet::new();
    let mut prepared = Vec::with_capacity(payload.len());
    for entry in payload {
        let mut entry = entry.clone();
        if let Some((table, _)) = FORMATS.iter().find(|(_, tagged)| *tagged == entry.table) {
            entry.table = (*table).to_owned();
        } else {
            entry.rows = match entry.table.as_str() {
                "game_item_instance" => upgrade::<LegacyItemInstance, ItemInstance>(&entry.rows),
                "game_character_buyback" => {
                    upgrade::<LegacyBuybackEntry, BuybackEntry>(&entry.rows)
                }
                "game_mail" => upgrade::<LegacyMail, Mail>(&entry.rows),
                _ => Ok(entry.rows),
            }
            .map_err(|error| format!("table {}: {error}", entry.table))?;
        }
        if !seen.insert(entry.table.clone()) {
            return Err(format!("arriving payload repeats table {}", entry.table));
        }
        prepared.push(entry);
    }
    Ok(prepared)
}

fn upgrade<Old, Current>(bytes: &[u8]) -> Result<Vec<u8>, String>
where
    Old: for<'de> spacetimedb::sats::Deserialize<'de>,
    Current: From<Old> + spacetimedb::sats::Serialize,
{
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut remaining = bytes;
    let rows: Vec<Old> = spacetimedb::sats::bsatn::from_reader(&mut remaining)
        .map_err(|error| format!("cannot decode legacy item rows: {error}"))?;
    if !remaining.is_empty() {
        return Err("legacy item rows contain trailing bytes".to_owned());
    }
    let rows: Vec<Current> = rows.into_iter().map(Current::from).collect();
    spacetimedb::sats::bsatn::to_vec(&rows)
        .map_err(|error| format!("cannot encode upgraded item rows: {error}"))
}

// These exact row shapes are the compatibility boundary for already persisted Escrow bytes.
#[derive(SpacetimeType)]
struct LegacyItemInstance {
    guid: u64,
    entry: u32,
    owner_identity: Identity,
    owner_guid: u64,
    slot: u8,
    stack_count: u32,
    durability: u32,
    created_at: Timestamp,
    enchant_id: u32,
    soulbound: bool,
}

impl From<LegacyItemInstance> for ItemInstance {
    fn from(row: LegacyItemInstance) -> Self {
        Self {
            guid: row.guid,
            entry: row.entry,
            owner_identity: row.owner_identity,
            owner_guid: row.owner_guid,
            slot: row.slot,
            stack_count: row.stack_count,
            durability: row.durability,
            created_at: row.created_at,
            enchant_id: row.enchant_id,
            soulbound: row.soulbound,
            random_property_id: 0,
        }
    }
}

#[derive(SpacetimeType)]
struct LegacyBuybackEntry {
    id: u64,
    player_guid: u64,
    item_entry: u32,
    stack_count: u32,
    price: u32,
    soulbound: bool,
}

impl From<LegacyBuybackEntry> for BuybackEntry {
    fn from(row: LegacyBuybackEntry) -> Self {
        Self {
            id: row.id,
            player_guid: row.player_guid,
            item_entry: row.item_entry,
            stack_count: row.stack_count,
            price: row.price,
            soulbound: row.soulbound,
            random_property_id: 0,
        }
    }
}

#[derive(SpacetimeType)]
struct LegacyMail {
    id: u64,
    recipient_guid: u64,
    sender_guid: u64,
    subject: String,
    body: String,
    item_entry: u32,
    item_stack_count: u32,
    item_durability: u32,
    item_enchant_id: u32,
    item_soulbound: bool,
    money: u32,
    cod: u32,
    was_read: bool,
    created_at: Timestamp,
}

impl From<LegacyMail> for Mail {
    fn from(row: LegacyMail) -> Self {
        Self {
            id: row.id,
            recipient_guid: row.recipient_guid,
            sender_guid: row.sender_guid,
            subject: row.subject,
            body: row.body,
            item_entry: row.item_entry,
            item_stack_count: row.item_stack_count,
            item_durability: row.item_durability,
            item_enchant_id: row.item_enchant_id,
            item_soulbound: row.item_soulbound,
            money: row.money,
            cod: row.cod,
            was_read: row.was_read,
            created_at: row.created_at,
            random_property_id: 0,
        }
    }
}

#[cfg(test)]
#[path = "legacy_item_rows_tests.rs"]
mod tests;
