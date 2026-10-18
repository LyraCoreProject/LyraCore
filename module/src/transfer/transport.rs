use spacetimedb::{log, ReducerContext, SpacetimeType};

use super::TransferOut;
use crate::bridge::{
    game_party_command_issuer, game_party_command_receipt, PartyCommandIssuer, PartyCommandReceipt,
};
use crate::items::{game_item_instance, ItemInstance};

#[path = "legacy_item_rows.rs"]
mod legacy_item_rows;

// Hot marks describe first-frame state; every table still travels in one blob.
pub(crate) const HOT_TABLES: &[&str] = &[
    "game_item_instance",
    "game_player_action",
    "game_player_skill",
    "game_player_spell",
    "game_character_talent",
    "game_party_command_issuer",
    "game_party_command_receipt",
    "game_aura",
];

pub(crate) const MANIFEST_EXCLUDE: &[&str] = &["game_transfer_out"];

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const NOT_TRANSPORTED: &[&str] = &[
    "game_rest_state_event",
    "game_breath_relay_event",
    "game_breath_state",
    "game_group_invite",
    "game_group_member",
    "game_group_member_partition",
    "game_group_target_icon",
    "game_instance_removal",
    // These rules require a local Package Instance and never follow a Character through Transfer.
    "game_package_combat_rule",
    // Transfer persists caster mana in the Character row and drops the current form.
    "game_druid_form_mana",
    "game_pet_command",
    "game_mail_delivery",
    "game_character_shard",
    "game_active_taxi_flight",
    "game_taxi_flight_schedule",
    "game_taxi_passenger_spline",
    "game_taxi_service_reply",
    "game_trade_session",
    "game_trade_slot",
    "game_duel",
    "game_duel_event",
];

#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntry {
    pub table: String,
    pub hot: bool,
}

// Item row-format suffixes make older Shards refuse fields they cannot preserve.
#[derive(SpacetimeType, Clone, Debug, PartialEq, Eq)]
pub struct TableRows {
    pub table: String,
    pub rows: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CommandIssuerImportRefusal {
    TooManyRows { count: usize },
    OwnerMismatch { expected: u64, actual: u64 },
    DestinationConflict { character_guid: u64 },
}

impl std::fmt::Display for CommandIssuerImportRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooManyRows { count } => {
                write!(
                    f,
                    "COMMAND_ISSUER_CARDINALITY: payload contains {count} rows"
                )
            }
            Self::OwnerMismatch { expected, actual } => write!(
                f,
                "COMMAND_ISSUER_OWNER_MISMATCH: expected Character {expected}, found {actual}"
            ),
            Self::DestinationConflict { character_guid } => write!(
                f,
                "COMMAND_ISSUER_CONFLICT: destination already has Character {character_guid}"
            ),
        }
    }
}

pub(crate) fn admit_command_issuer_import(
    character_guid: u64,
    rows: &[PartyCommandIssuer],
    destination_exists: bool,
) -> Result<(), CommandIssuerImportRefusal> {
    if rows.len() > 1 {
        return Err(CommandIssuerImportRefusal::TooManyRows { count: rows.len() });
    }
    if let Some(row) = rows.first() {
        if row.character_guid != character_guid {
            return Err(CommandIssuerImportRefusal::OwnerMismatch {
                expected: character_guid,
                actual: row.character_guid,
            });
        }
    }
    if destination_exists {
        return Err(CommandIssuerImportRefusal::DestinationConflict { character_guid });
    }
    Ok(())
}

pub enum RowIo<'a> {
    Export(&'a mut Vec<u8>),
    Import(&'a [u8], &'a mut Result<(), String>),
}

// The import caller must clear existing rows before inserting this table's payload.
pub(crate) fn move_rows<C, R>(
    ctx: &C,
    io: &mut RowIo<'_>,
    export: impl FnOnce() -> Vec<R>,
    insert: impl Fn(&C, R),
) where
    R: spacetimedb::SpacetimeType
        + spacetimedb::sats::Serialize
        + for<'de> spacetimedb::sats::Deserialize<'de>,
{
    match io {
        RowIo::Export(out) => **out = encode_rows(export()),
        RowIo::Import(bytes, outcome) => {
            for r in decode_rows::<R>(bytes, outcome) {
                insert(ctx, r);
            }
        }
    }
}

pub(crate) fn encode_rows<R>(rows: Vec<R>) -> Vec<u8>
where
    R: spacetimedb::SpacetimeType + spacetimedb::sats::Serialize,
{
    match spacetimedb::sats::bsatn::to_vec(&rows) {
        Ok(bytes) => bytes,
        Err(e) => {
            log::error!("transfer export: cannot serialize rows: {e}");
            Vec::new()
        }
    }
}

pub(crate) fn decode_rows<R>(bytes: &[u8], outcome: &mut Result<(), String>) -> Vec<R>
where
    R: spacetimedb::SpacetimeType + for<'de> spacetimedb::sats::Deserialize<'de>,
{
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut remaining = bytes;
    match spacetimedb::sats::bsatn::from_reader::<Vec<R>>(&mut remaining) {
        Ok(rows) if remaining.is_empty() => rows,
        Ok(_) => {
            if outcome.is_ok() {
                *outcome = Err("arriving rows contain trailing bytes".to_owned());
            }
            Vec::new()
        }
        Err(e) => {
            if outcome.is_ok() {
                *outcome = Err(format!("cannot deserialize arriving rows: {e}"));
            }
            Vec::new()
        }
    }
}

pub(crate) fn not_transported(io: &mut RowIo<'_>) {
    match io {
        RowIo::Export(out) => out.clear(),
        RowIo::Import(..) => {}
    }
}

pub(crate) type TransportArm<'a, C> = (&'a str, fn(&C, u64, &mut RowIo<'_>));

pub(crate) fn export_rows_via<C>(
    ctx: &C,
    character_guid: u64,
    arms: &[TransportArm<'_, C>],
) -> Vec<TableRows> {
    let mut out = Vec::new();
    for (table, mover) in arms {
        if MANIFEST_EXCLUDE.contains(table) {
            continue;
        }
        let mut rows = Vec::new();
        mover(ctx, character_guid, &mut RowIo::Export(&mut rows));
        out.push(TableRows {
            table: (*table).to_string(),
            rows,
        });
    }
    out
}

pub(crate) fn export_rows(ctx: &ReducerContext, character_guid: u64) -> Vec<TableRows> {
    let mut payload = export_rows_via(ctx, character_guid, crate::CHARACTER_OWNED_TRANSFERS);
    legacy_item_rows::mark_current(&mut payload);
    payload
}

pub(crate) fn import_rows_via<C>(
    ctx: &C,
    character_guid: u64,
    payload: &[TableRows],
    arms: &[TransportArm<'_, C>],
) -> Result<(), String> {
    // Check full payload coverage before applying any rows.
    let missing: Vec<&str> = arms
        .iter()
        .map(|(table, _)| *table)
        .filter(|table| !MANIFEST_EXCLUDE.contains(table))
        .filter(|table| !payload.iter().any(|entry| entry.table == *table))
        .collect();
    if !missing.is_empty() {
        log::error!(
            "transfer import: arriving payload is MISSING {} manifest table(s) for character \
             {character_guid}: {} — refusing the import so the source copy survives",
            missing.len(),
            missing.join(", ")
        );
        return Err(format!(
            "arriving payload does not carry manifest table(s) {} which this shard expects — \
             refusing a partial import",
            missing.join(", ")
        ));
    }
    for entry in payload {
        let Some((_, mover)) = arms.iter().find(|(t, _)| *t == entry.table) else {
            return Err(format!(
                "arriving payload names table {} which this shard has no transport arm for — \
                 refusing a partial import",
                entry.table
            ));
        };
        let mut outcome = Ok(());
        mover(
            ctx,
            character_guid,
            &mut RowIo::Import(&entry.rows, &mut outcome),
        );
        outcome.map_err(|e| format!("table {}: {e}", entry.table))?;
    }
    Ok(())
}

pub(crate) fn import_rows(
    ctx: &ReducerContext,
    character_guid: u64,
    payload: &[TableRows],
) -> Result<(), String> {
    let payload = legacy_item_rows::prepare(payload)?;
    if let Some(entry) = payload
        .iter()
        .find(|entry| entry.table == "game_party_command_issuer")
    {
        let mut outcome = Ok(());
        let rows = decode_rows::<PartyCommandIssuer>(&entry.rows, &mut outcome);
        outcome?;
        admit_command_issuer_import(
            character_guid,
            &rows,
            ctx.db
                .game_party_command_issuer()
                .character_guid()
                .find(character_guid)
                .is_some(),
        )
        .map_err(|refusal| refusal.to_string())?;
    }
    if let Some(entry) = payload
        .iter()
        .find(|entry| entry.table == "game_item_instance")
    {
        let mut outcome = Ok(());
        let rows = decode_rows::<ItemInstance>(&entry.rows, &mut outcome);
        outcome?;
        let mut guids = std::collections::BTreeSet::new();
        for row in rows {
            if row.owner_guid != character_guid {
                return Err(format!(
                    "ITEM_GUID_OWNER_MISMATCH: item {} belongs to Character {}",
                    row.guid, row.owner_guid
                ));
            }
            if !guids.insert(row.guid)
                || ctx.db.game_item_instance().guid().find(row.guid).is_some()
            {
                return Err(format!(
                    "ITEM_GUID_CONFLICT: item {} for Character {character_guid}",
                    row.guid
                ));
            }
        }
    }
    if let Some(entry) = payload
        .iter()
        .find(|entry| entry.table == "game_party_command_receipt")
    {
        let mut outcome = Ok(());
        let rows = decode_rows::<PartyCommandReceipt>(&entry.rows, &mut outcome);
        outcome?;
        let current_count = ctx
            .db
            .game_party_command_receipt()
            .by_bot()
            .filter(&character_guid)
            .take(crate::bridge::RECEIPT_CAPACITY + 1)
            .count();
        if rows.len() > crate::bridge::RECEIPT_CAPACITY
            || current_count.saturating_add(rows.len()) > crate::bridge::RECEIPT_CAPACITY
        {
            return Err(format!(
                "COMMAND_RECEIPT_CAPACITY: Character {character_guid} has {current_count} local and {} arriving receipts",
                rows.len()
            ));
        }
        let mut keys = std::collections::BTreeSet::new();
        for row in rows {
            if row.bot_guid != character_guid {
                return Err(format!(
                    "COMMAND_RECEIPT_OWNER_MISMATCH: receipt {} belongs to Character {}",
                    row.receipt_key, row.bot_guid
                ));
            }
            if !keys.insert(row.receipt_key.clone())
                || ctx
                    .db
                    .game_party_command_receipt()
                    .receipt_key()
                    .find(&row.receipt_key)
                    .is_some()
            {
                return Err(format!(
                    "COMMAND_RECEIPT_KEY_CONFLICT: receipt {} for Character {character_guid}",
                    row.receipt_key
                ));
            }
        }
    }
    import_rows_via(
        ctx,
        character_guid,
        &payload,
        crate::CHARACTER_OWNED_TRANSFERS,
    )
}

#[derive(SpacetimeType, Clone, Debug, PartialEq)]
pub struct ExportBlob {
    pub transfer_id: u64,
    pub character_guid: u64,
    // Includes post-freeze credits absent from character_row.money.
    pub money: u32,
    pub manifest: Vec<ManifestEntry>,
    pub dest_map_id: u32,
    pub dest_instance_id: u64,
    pub dest_x: f32,
    pub dest_y: f32,
    pub dest_z: f32,
    pub dest_o: f32,
    pub character_row: Vec<u8>,
    pub payload: Vec<TableRows>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Destination {
    pub map_id: u32,
    pub instance_id: u64,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub o: f32,
}

impl TransferOut {
    pub(crate) fn destination(&self) -> Destination {
        Destination {
            map_id: self.dest_map_id,
            instance_id: self.dest_instance_id,
            x: self.dest_x,
            y: self.dest_y,
            z: self.dest_z,
            o: self.dest_o,
        }
    }
}

impl ExportBlob {
    pub(crate) fn destination(&self) -> Destination {
        Destination {
            map_id: self.dest_map_id,
            instance_id: self.dest_instance_id,
            x: self.dest_x,
            y: self.dest_y,
            z: self.dest_z,
            o: self.dest_o,
        }
    }
}

impl crate::character::Character {
    pub(crate) fn relocate(&mut self, dest: Destination) {
        self.map_id = dest.map_id;
        self.pending_instance_id = dest.instance_id;
        self.x = dest.x;
        self.y = dest.y;
        self.z = dest.z;
        self.orientation = dest.o;
    }
}

pub(crate) fn decode_blob(transfer_id: u64, bytes: &[u8]) -> Result<ExportBlob, String> {
    spacetimedb::sats::bsatn::from_slice(bytes)
        .map_err(|e| format!("transfer {transfer_id}: corrupt export blob: {e}"))
}

// Only a missing final watched-slot i32 receives its legacy default.
pub(crate) fn decode_character_row(
    transfer_id: u64,
    bytes: &[u8],
) -> Result<crate::character::Character, String> {
    use spacetimedb::sats::bsatn::{from_reader, DecodeError};

    let refusal = |reason| {
        format!("transfer {transfer_id}: cannot decode the arriving character row: {reason}")
    };
    let mut remaining = bytes;
    match from_reader::<crate::character::Character>(&mut remaining) {
        Ok(character) if remaining.is_empty() => Ok(character),
        Ok(_) => Err(refusal("trailing bytes".to_string())),
        Err(DecodeError::BufferLength {
            for_type: "i32",
            expected: 4,
            given: 0,
        }) if remaining.is_empty() => {
            let mut upgraded = bytes.to_vec();
            upgraded.extend_from_slice(&(-1i32).to_le_bytes());
            let mut remaining = upgraded.as_slice();
            let character = from_reader::<crate::character::Character>(&mut remaining)
                .map_err(|e| refusal(e.to_string()))?;
            if !remaining.is_empty() {
                return Err(refusal("trailing bytes".to_string()));
            }
            Ok(character)
        }
        Err(e) => Err(refusal(e.to_string())),
    }
}

// Keep each compatibility entry until every Shard can export that table.
pub(crate) const TRANSFER_TABLES_OLD_BLOBS_MAY_LACK: &[&str] = &[
    "game_auction_bid_hold",
    "game_auction_hold",
    "game_character_away",
    "game_group_target_icon",
    "game_guild_fee_hold",
    "game_instance_removal",
];

pub(crate) fn check_manifest(transfer_id: u64, arriving: &[ManifestEntry]) -> Result<(), String> {
    let mine = manifest();
    let named = |table: &str| arriving.iter().any(|entry| entry.table == table);
    let expected: Vec<ManifestEntry> = mine
        .iter()
        .filter(|entry| {
            named(&entry.table)
                || !TRANSFER_TABLES_OLD_BLOBS_MAY_LACK.contains(&entry.table.as_str())
        })
        .cloned()
        .collect();
    if arriving != expected {
        return Err(format!(
            "transfer {transfer_id}: manifest mismatch — source exported {} character-owned tables, \
             this shard knows {}",
            arriving.len(),
            mine.len()
        ));
    }
    Ok(())
}

pub(crate) fn payload_for_this_build(
    arriving: &[ManifestEntry],
    payload: &[TableRows],
) -> Vec<TableRows> {
    let mut complete = payload.to_vec();
    for table in TRANSFER_TABLES_OLD_BLOBS_MAY_LACK {
        if !arriving.iter().any(|entry| entry.table == *table)
            && !payload.iter().any(|entry| entry.table == *table)
        {
            complete.push(TableRows {
                table: (*table).to_owned(),
                rows: Vec::new(),
            });
        }
    }
    complete
}

pub(crate) fn build_export_blob(
    transfer_id: u64,
    character: &crate::character::Character,
    dest: Destination,
    payload: Vec<TableRows>,
) -> Result<ExportBlob, String> {
    let character_row = spacetimedb::sats::bsatn::to_vec(&character)
        .map_err(|e| format!("transfer {transfer_id}: cannot serialize the character row: {e}"))?;
    Ok(ExportBlob {
        transfer_id,
        character_guid: character.guid,
        money: character.money,
        manifest: manifest(),
        dest_map_id: dest.map_id,
        dest_instance_id: dest.instance_id,
        dest_x: dest.x,
        dest_y: dest.y,
        dest_z: dest.z,
        dest_o: dest.o,
        character_row,
        payload,
    })
}

pub(crate) fn manifest() -> Vec<ManifestEntry> {
    crate::CHARACTER_OWNED_TABLES
        .iter()
        .filter(|t| !MANIFEST_EXCLUDE.contains(t))
        .map(|t| ManifestEntry {
            table: (*t).to_string(),
            hot: HOT_TABLES.contains(t),
        })
        .collect()
}
