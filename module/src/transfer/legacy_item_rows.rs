use std::collections::BTreeSet;

use spacetimedb::{Identity, SpacetimeType, Timestamp};

use super::TableRows;
use crate::mail::Mail;
use crate::mail_escrow::MailEscrow;
use crate::{BuybackEntry, ItemInstance};

const FORMATS: &[(&str, &str)] = &[
    ("game_item_instance", "game_item_instance@item-text-1"),
    (
        "game_character_buyback",
        "game_character_buyback@random-property-1",
    ),
    ("game_mail", "game_mail@letter-text-1"),
    ("game_mail_escrow", "game_mail_escrow@letter-text-1"),
    ("game_auction_hold", "game_auction_hold@letter-text-1"),
];
const RANDOM_PROPERTY_ITEM_INSTANCE: &str = "game_item_instance@random-property-1";
const RANDOM_PROPERTY_MAIL: &str = "game_mail@random-property-1";
const MAIL_HEADER_MAIL: &str = "game_mail@mail-header-1";

// Older Modules refuse these payload tags before applying rows.
pub(super) fn mark_current(payload: &mut [TableRows]) {
    for entry in payload {
        if let Some((_, tagged)) = FORMATS.iter().find(|(table, _)| *table == entry.table) {
            entry.table = (*tagged).to_owned();
        }
    }
}

// Table defaults do not rewrite Escrow bytes; choose the old codec by tag, never by decode failure.
pub(super) fn prepare(payload: &[TableRows]) -> Result<Vec<TableRows>, String> {
    let mut seen = BTreeSet::new();
    let mut prepared = Vec::with_capacity(payload.len());
    for entry in payload {
        let mut entry = entry.clone();
        if let Some((table, _)) = FORMATS.iter().find(|(_, tagged)| *tagged == entry.table) {
            entry.table = (*table).to_owned();
        } else if entry.table == RANDOM_PROPERTY_MAIL {
            entry.rows = upgrade::<RandomPropertyMail, Mail>(&entry.rows)
                .map_err(|error| format!("table {}: {error}", entry.table))?;
            entry.table = "game_mail".to_owned();
        } else if entry.table == MAIL_HEADER_MAIL {
            entry.rows = upgrade::<MailHeaderMail, Mail>(&entry.rows)
                .map_err(|error| format!("table {}: {error}", entry.table))?;
            entry.table = "game_mail".to_owned();
        } else if entry.table == RANDOM_PROPERTY_ITEM_INSTANCE {
            entry.rows = upgrade::<RandomPropertyItemInstance, ItemInstance>(&entry.rows)
                .map_err(|error| format!("table {}: {error}", entry.table))?;
            entry.table = "game_item_instance".to_owned();
        } else {
            entry.rows = match entry.table.as_str() {
                "game_item_instance" => upgrade::<LegacyItemInstance, ItemInstance>(&entry.rows),
                "game_character_buyback" => {
                    upgrade::<LegacyBuybackEntry, BuybackEntry>(&entry.rows)
                }
                "game_mail" => upgrade::<LegacyMail, Mail>(&entry.rows),
                "game_mail_escrow" => upgrade::<RewardHeaderMailEscrow, MailEscrow>(&entry.rows),
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

// These exact row shapes decode already persisted Escrow bytes.
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
            item_text_id: 0,
        }
    }
}

#[derive(SpacetimeType)]
struct RandomPropertyItemInstance {
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
    random_property_id: u32,
}

impl From<RandomPropertyItemInstance> for ItemInstance {
    fn from(row: RandomPropertyItemInstance) -> Self {
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
            random_property_id: row.random_property_id,
            item_text_id: 0,
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
        RandomPropertyMail {
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
        .into()
    }
}

#[derive(SpacetimeType)]
struct RandomPropertyMail {
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
    random_property_id: u32,
}

impl From<RandomPropertyMail> for Mail {
    fn from(row: RandomPropertyMail) -> Self {
        MailHeaderMail {
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
            random_property_id: row.random_property_id,
            sender_kind: 0,
            sender_entry: 0,
            check_flags: 0,
            mail_template_id: 0,
            deliver_micros: 0,
        }
        .into()
    }
}

#[derive(SpacetimeType)]
struct MailHeaderMail {
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
    random_property_id: u32,
    sender_kind: u8,
    sender_entry: u32,
    check_flags: u32,
    mail_template_id: u32,
    deliver_micros: i64,
}

impl From<MailHeaderMail> for Mail {
    fn from(row: MailHeaderMail) -> Self {
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
            random_property_id: row.random_property_id,
            sender_kind: row.sender_kind,
            sender_entry: row.sender_entry,
            check_flags: row.check_flags,
            mail_template_id: row.mail_template_id,
            deliver_micros: row.deliver_micros,
            item_text_id: 0,
        }
    }
}

#[derive(SpacetimeType)]
struct RewardHeaderMailEscrow {
    escrow_id: u64,
    sender_guid: u64,
    recipient_guid: u64,
    subject: String,
    body: String,
    money: u32,
    postage: u32,
    created_micros: i64,
    delivered: bool,
    payout: bool,
    mail_id: u64,
    item_entry: u32,
    item_stack_count: u32,
    item_durability: u32,
    item_enchant_id: u32,
    item_soulbound: bool,
    cod: u32,
    random_property_id: u32,
    delivery_delay_secs: u32,
    sender_kind: u8,
    sender_entry: u32,
    mail_template_id: u32,
}

impl From<RewardHeaderMailEscrow> for MailEscrow {
    fn from(row: RewardHeaderMailEscrow) -> Self {
        Self {
            escrow_id: row.escrow_id,
            sender_guid: row.sender_guid,
            recipient_guid: row.recipient_guid,
            subject: row.subject,
            body: row.body,
            money: row.money,
            postage: row.postage,
            created_micros: row.created_micros,
            delivered: row.delivered,
            payout: row.payout,
            mail_id: row.mail_id,
            item_entry: row.item_entry,
            item_stack_count: row.item_stack_count,
            item_durability: row.item_durability,
            item_enchant_id: row.item_enchant_id,
            item_soulbound: row.item_soulbound,
            cod: row.cod,
            random_property_id: row.random_property_id,
            delivery_delay_secs: row.delivery_delay_secs,
            sender_kind: row.sender_kind,
            sender_entry: row.sender_entry,
            mail_template_id: row.mail_template_id,
            item_text_id: 0,
        }
    }
}

#[cfg(test)]
#[path = "legacy_item_rows_tests.rs"]
mod tests;
