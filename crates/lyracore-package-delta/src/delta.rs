//! The Package Delta artifact: its types, its one refusing constructor per level, and the parser
//! that builds it from untrusted JSON.
//!
//! Nothing in this module writes anywhere. A `PackageDelta` that exists has already passed every
//! check the artifact promises, so an applier reads it without re-deciding anything.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value};

use crate::canonical;
use crate::error::DeltaError;
use crate::ids::{
    is_fixture_reserved_item_id, is_fixture_reserved_spell_id, is_package_item_id,
    is_package_spell_id, packed_spell_effect_id, MAX_SPELL_EFFECT_INDEX,
};
use crate::schema::{FieldType, FieldValue, Table};

/// The artifact version this build reads and writes.
pub const DELTA_VERSION: u64 = 1;

/// The identity of the Package a claim came from.
///
/// Lowercase on purpose: two Packages differing only in case would canonicalize to two rows that
/// look like the same Package in a conflict report.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId(String);

impl PackageId {
    /// Accepts 1 to 64 characters of `a-z`, `0-9`, `-`, `_` or `.`.
    ///
    /// # Errors
    /// [`DeltaError::InvalidPackageId`] when the identity is empty, too long, or uses any other
    /// character.
    pub fn new(id: impl Into<String>) -> Result<Self, DeltaError> {
        let id = id.into();
        let shaped = !id.is_empty()
            && id.len() <= 64
            && id.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
            });
        if shaped {
            Ok(Self(id))
        } else {
            Err(DeltaError::InvalidPackageId { found: id })
        }
    }

    /// The identity as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The digest of the Datascript source a Package Delta was generated from.
///
/// The artifact only carries it; nothing here recomputes it. It exists so a replay can tell that a
/// Package's source changed without diffing every claim, and so a conflict report can name the
/// source that produced a claim.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceHash(String);

impl SourceHash {
    /// Accepts exactly 64 lowercase hexadecimal characters, the shape of the BLAKE3 digest the
    /// importer already uses.
    ///
    /// # Errors
    /// [`DeltaError::InvalidSourceHash`] for any other length or character.
    pub fn new(hex: impl Into<String>) -> Result<Self, DeltaError> {
        let hex = hex.into();
        let shaped = hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if shaped {
            Ok(Self(hex))
        } else {
            Err(DeltaError::InvalidSourceHash { found: hex })
        }
    }

    /// The digest as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SourceHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a claim does to its row.
///
/// There is no `Delete`. Uninstalling a Package replays the affected families without its claims, so
/// there is no pre-image a delete could be undone from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Operation {
    /// The row does not exist yet; the claim carries every column.
    Insert,
    /// The row exists; the claim carries only the columns it changes.
    Update,
}

impl Operation {
    /// The value the artifact's `operation` member carries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
        }
    }

    /// Resolves an `operation` member.
    ///
    /// # Errors
    /// [`DeltaError::DeleteNotSupported`] for `delete`, which is a refusal rather than a typo, and
    /// [`DeltaError::UnknownOperation`] for anything else.
    pub fn parse(name: &str) -> Result<Self, DeltaError> {
        match name {
            "insert" => Ok(Self::Insert),
            "update" => Ok(Self::Update),
            "delete" => Err(DeltaError::DeleteNotSupported),
            other => Err(DeltaError::UnknownOperation {
                found: other.to_owned(),
            }),
        }
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The row a claim names, typed by its table.
///
/// One variant per [`Table`] variant, in the same family-grouped order, so the derived ordering is
/// the canonical one: `game_spell` before `game_spell_effect`, then by spell, then by effect index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrimaryKey {
    // ---- spell ----
    /// A `game_spell` header row.
    Spell {
        /// The spell.
        spell_id: u32,
    },
    /// A `game_spell_effect` child row. The durable key is derived, never authored.
    SpellEffect {
        /// The spell the effect belongs to.
        spell_id: u32,
        /// Which of the spell's effects, 0..=[`MAX_SPELL_EFFECT_INDEX`].
        effect_index: u8,
    },
    // ---- items ----
    /// A `game_item_template` row.
    Item {
        /// The item.
        entry: u32,
    },
}

impl PrimaryKey {
    /// Names a spell header row.
    ///
    /// # Errors
    /// [`DeltaError::MalformedKey`] for spell 0, and [`DeltaError::SpellIdFixtureReserved`] for a
    /// seeded fixture spell.
    pub fn spell(spell_id: u32) -> Result<Self, DeltaError> {
        check_claimable_spell_id(Table::Spell, spell_id)?;
        Ok(Self::Spell { spell_id })
    }

    /// Names one effect of a spell. The packed durable key follows from the two components.
    ///
    /// # Errors
    /// The same identifier refusals as [`PrimaryKey::spell`], plus
    /// [`DeltaError::EffectIndexOutOfRange`] for a slot no spell has.
    pub fn spell_effect(spell_id: u32, effect_index: u8) -> Result<Self, DeltaError> {
        check_claimable_spell_id(Table::SpellEffect, spell_id)?;
        if effect_index > MAX_SPELL_EFFECT_INDEX {
            return Err(DeltaError::EffectIndexOutOfRange { effect_index });
        }
        Ok(Self::SpellEffect {
            spell_id,
            effect_index,
        })
    }

    /// Names an item row.
    ///
    /// # Errors
    /// [`DeltaError::MalformedKey`] for entry 0, and [`DeltaError::ItemIdFixtureReserved`] for a
    /// seeded fixture item.
    pub fn item(entry: u32) -> Result<Self, DeltaError> {
        check_claimable_item_id(entry)?;
        Ok(Self::Item { entry })
    }

    /// The table this row lives in.
    #[must_use]
    pub const fn table(self) -> Table {
        match self {
            Self::Spell { .. } => Table::Spell,
            Self::SpellEffect { .. } => Table::SpellEffect,
            Self::Item { .. } => Table::Item,
        }
    }

    /// The spell this row belongs to, for a spell-family key.
    ///
    /// Every caller sits inside the spell family's own dispatch (`module/src/package_import/
    /// spell.rs`), where `check_claims_belong_to` has already refused a foreign-family row before
    /// this could run — so a foreign-family key reaching here is an internal invariant break, not
    /// untrusted input.
    ///
    /// # Panics
    /// If called on an item key.
    #[must_use]
    pub fn spell_id(self) -> u32 {
        match self {
            Self::Spell { spell_id } | Self::SpellEffect { spell_id, .. } => spell_id,
            Self::Item { .. } => {
                unreachable!(
                    "`spell_id` is a spell-family accessor; a foreign-family key never reaches it"
                )
            }
        }
    }

    /// The durable primary-key value: `spell_id` for a header, the packed
    /// `(spell_id << 2) | effect_index` for an effect, `entry` for an item.
    #[must_use]
    pub const fn row_id(self) -> u64 {
        match self {
            Self::Spell { spell_id } => spell_id as u64,
            Self::SpellEffect {
                spell_id,
                effect_index,
            } => packed_spell_effect_id(spell_id, effect_index),
            Self::Item { entry } => entry as u64,
        }
    }
}

impl fmt::Display for PrimaryKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spell { spell_id } => write!(f, "{{spell_id={spell_id}}}"),
            Self::SpellEffect {
                spell_id,
                effect_index,
            } => write!(f, "{{spell_id={spell_id}, effect_index={effect_index}}}"),
            Self::Item { entry } => write!(f, "{{entry={entry}}}"),
        }
    }
}

/// Refuses an `insert` at an identifier outside its family's Package band.
///
/// The band is a per-family policy, so this match carries one arm per family and no wildcard: a
/// family that arrives without stating its band cannot compile.
fn check_inventable(key: PrimaryKey) -> Result<(), DeltaError> {
    match key {
        PrimaryKey::Spell { spell_id } | PrimaryKey::SpellEffect { spell_id, .. } => {
            if is_package_spell_id(spell_id) {
                Ok(())
            } else {
                Err(DeltaError::SpellIdNotClientSafe { spell_id })
            }
        }
        PrimaryKey::Item { entry } => {
            if is_package_item_id(entry) {
                Ok(())
            } else {
                Err(DeltaError::ItemIdNotClientSafe { entry })
            }
        }
    }
}

/// A spell identifier is refused the same way under every operation when it is 0 or fixture-owned.
fn check_claimable_spell_id(table: Table, spell_id: u32) -> Result<(), DeltaError> {
    if spell_id == 0 {
        return Err(DeltaError::MalformedKey {
            table,
            detail: "`spell_id` 0 is not a spell".to_owned(),
        });
    }
    if is_fixture_reserved_spell_id(spell_id) {
        return Err(DeltaError::SpellIdFixtureReserved { spell_id });
    }
    Ok(())
}

/// An item identifier is refused the same way under every operation when it is 0 or fixture-owned.
fn check_claimable_item_id(entry: u32) -> Result<(), DeltaError> {
    if entry == 0 {
        return Err(DeltaError::MalformedKey {
            table: Table::Item,
            detail: "`entry` 0 is not an item".to_owned(),
        });
    }
    if is_fixture_reserved_item_id(entry) {
        return Err(DeltaError::ItemIdFixtureReserved { entry });
    }
    Ok(())
}

/// One Package's claim on one row.
///
/// The table follows from the key, so a claim cannot name a table its key does not describe.
#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    key: PrimaryKey,
    operation: Operation,
    fields: BTreeMap<String, FieldValue>,
}

impl Claim {
    /// Builds a claim, refusing everything the artifact promises it will refuse.
    ///
    /// The identifier policy is operation-dependent, and this is the only place it is applied:
    ///
    ///  * An `insert` invents a row, so its identifier must sit in its family's Package band — for
    ///    a spell, the Package spell range ([`is_package_spell_id`]). Nothing else is safe: a real
    ///    client identifier would collide with the next base import.
    ///  * An `update` changes a row someone else owns, so any identifier is allowed except a
    ///    fixture-reserved one, which [`PrimaryKey`] already refused. Tuning real spells is the
    ///    common case and must stay possible.
    ///
    /// # Errors
    /// [`DeltaError::SpellIdNotClientSafe`] for an out-of-range insert;
    /// [`DeltaError::KeyColumnClaimed`], [`DeltaError::UnknownField`] or
    /// [`DeltaError::FieldTypeMismatch`] for a bad column; [`DeltaError::IncompleteInsert`] for a
    /// partial insert; [`DeltaError::EmptyUpdate`] for an update that states no change.
    pub fn new(
        key: PrimaryKey,
        operation: Operation,
        fields: BTreeMap<String, FieldValue>,
    ) -> Result<Self, DeltaError> {
        let table = key.table();
        if operation == Operation::Insert {
            check_inventable(key)?;
        }

        for (name, value) in &fields {
            let Some(expected) = table.column_type(name) else {
                return Err(if is_key_column(table, name) {
                    DeltaError::KeyColumnClaimed {
                        table,
                        field: name.clone(),
                    }
                } else {
                    DeltaError::UnknownField {
                        table,
                        field: name.clone(),
                    }
                });
            };
            let found = value.field_type();
            if found != expected {
                return Err(DeltaError::FieldTypeMismatch {
                    table,
                    field: name.clone(),
                    expected,
                    found,
                });
            }
        }

        match operation {
            Operation::Insert => {
                let missing: Vec<String> = table
                    .columns()
                    .iter()
                    .filter(|column| !fields.contains_key(column.name))
                    .map(|column| column.name.to_owned())
                    .collect();
                if !missing.is_empty() {
                    return Err(DeltaError::IncompleteInsert { table, missing });
                }
            }
            Operation::Update => {
                if fields.is_empty() {
                    return Err(DeltaError::EmptyUpdate { table });
                }
            }
        }

        Ok(Self {
            key,
            operation,
            fields,
        })
    }

    /// The row this claim names.
    #[must_use]
    pub const fn key(&self) -> PrimaryKey {
        self.key
    }

    /// The table this claim's row lives in.
    #[must_use]
    pub const fn table(&self) -> Table {
        self.key.table()
    }

    /// What this claim does to the row.
    #[must_use]
    pub const fn operation(&self) -> Operation {
        self.operation
    }

    /// The claimed columns, in canonical (name-sorted) order.
    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<String, FieldValue> {
        &self.fields
    }
}

/// True for a column that is part of the row's primary key, and therefore never claimable.
fn is_key_column(table: Table, name: &str) -> bool {
    match table {
        Table::Spell => name == "spell_id",
        Table::SpellEffect => matches!(name, "id" | "spell_id" | "effect_index"),
        Table::Item => name == "entry",
    }
}

/// One Package's whole set of claims: its identity, the source it was generated from, and the rows
/// it changes.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageDelta {
    package: PackageId,
    source_hash: SourceHash,
    claims: Vec<Claim>,
}

impl PackageDelta {
    /// Builds a delta and puts its claims in canonical order.
    ///
    /// # Errors
    /// [`DeltaError::DuplicateClaim`] when the Package names one row twice. A Package states each
    /// row once and lists every column it sets there, so ordering the claims can never lose one.
    pub fn new(
        package: PackageId,
        source_hash: SourceHash,
        mut claims: Vec<Claim>,
    ) -> Result<Self, DeltaError> {
        claims.sort_by_key(Claim::key);
        for pair in claims.windows(2) {
            if pair[0].key == pair[1].key {
                return Err(DeltaError::DuplicateClaim {
                    table: pair[0].table(),
                    key: pair[0].key.to_string(),
                });
            }
        }
        Ok(Self {
            package,
            source_hash,
            claims,
        })
    }

    /// Reads an artifact from untrusted JSON.
    ///
    /// Every refusal happens here, before any caller could act on a claim.
    ///
    /// # Errors
    /// Any [`DeltaError`]. The parse stops at the first problem.
    pub fn parse(json: &str) -> Result<Self, DeltaError> {
        let root: Value =
            serde_json::from_str(json).map_err(|e| DeltaError::Malformed(e.to_string()))?;
        let root = object(&root, "")?;
        expect_members(root, "", &["claims", "package", "source_hash", "version"])?;

        let version = member(root, "", "version")?;
        let version = version.as_u64().ok_or(DeltaError::WrongJsonType {
            path: "version".to_owned(),
            expected: "a version number",
        })?;
        if version != DELTA_VERSION {
            return Err(DeltaError::UnsupportedVersion {
                found: version.to_string(),
            });
        }

        let package = PackageId::new(string(member(root, "", "package")?, "package")?)?;
        let source_hash =
            SourceHash::new(string(member(root, "", "source_hash")?, "source_hash")?)?;

        let claims = member(root, "", "claims")?
            .as_array()
            .ok_or(DeltaError::WrongJsonType {
                path: "claims".to_owned(),
                expected: "an array of claims",
            })?;
        let claims = claims
            .iter()
            .enumerate()
            .map(|(index, claim)| parse_claim(claim, index))
            .collect::<Result<Vec<_>, _>>()?;

        Self::new(package, source_hash, claims)
    }

    /// Writes the artifact back as canonical JSON.
    ///
    /// Byte-stable: two artifacts that say the same thing produce the same bytes, whatever member
    /// order, whitespace or number spelling they were written with.
    #[must_use]
    pub fn to_canonical_json(&self) -> String {
        canonical::write_delta(self)
    }

    /// The Package this delta belongs to.
    #[must_use]
    pub const fn package(&self) -> &PackageId {
        &self.package
    }

    /// The digest of the Datascript source this delta was generated from.
    #[must_use]
    pub const fn source_hash(&self) -> &SourceHash {
        &self.source_hash
    }

    /// The claims, in canonical order.
    #[must_use]
    pub fn claims(&self) -> &[Claim] {
        &self.claims
    }

    /// How many rows of each kind this Package claims.
    ///
    /// Counted from the claims alone, so it describes this Package on its own — an applier records
    /// it as provenance and a dry-run check prints it, and both read the same numbers.
    #[must_use]
    pub fn claim_counts(&self) -> ClaimCounts {
        let mut counts = ClaimCounts::default();
        for claim in &self.claims {
            match (claim.operation(), claim.table()) {
                (Operation::Update, _) => counts.updated_rows += 1,
                (Operation::Insert, Table::Spell) => {
                    counts.inserted_spells += 1;
                    counts.inserted_rows += 1;
                }
                (Operation::Insert, Table::SpellEffect) => {
                    counts.inserted_effects += 1;
                    counts.inserted_rows += 1;
                }
                (Operation::Insert, Table::Item) => counts.inserted_rows += 1,
            }
        }
        counts
    }
}

/// How many rows of each kind one Package Delta claims.
///
/// These are the counts `game_package_import` records, so the field names are durable column names
/// and a family adds counters rather than renaming these. [`PackageDelta::claim_counts`] tallies
/// them with a match over every table, which is where a new family states what its inserts count
/// as.
///
/// `inserted_spells`/`inserted_effects` are the spell family's own counts, kept under their
/// original names rather than renamed for a later family. `inserted_rows` is the family-generic
/// successor: every family's inserts count here, spell's included, so a reader that knows only this
/// column still sees the whole picture. A non-spell family (items today) leaves the legacy pair at
/// zero and reports through `inserted_rows` alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClaimCounts {
    /// Rows the Package changes but does not own, across every table.
    pub updated_rows: u64,
    /// `game_spell` rows the Package invents.
    pub inserted_spells: u64,
    /// `game_spell_effect` rows the Package invents.
    pub inserted_effects: u64,
    /// Rows the Package invents, across every table its family owns.
    pub inserted_rows: u64,
}

fn parse_claim(value: &Value, index: usize) -> Result<Claim, DeltaError> {
    let path = format!("claims[{index}]");
    let claim = object(value, &path)?;
    expect_members(claim, &path, &["fields", "key", "operation", "table"])?;

    let table_name = string(member(claim, &path, "table")?, &format!("{path}.table"))?;
    let table = Table::parse(table_name).ok_or_else(|| DeltaError::UnknownTable {
        found: table_name.to_owned(),
    })?;

    let operation = Operation::parse(string(
        member(claim, &path, "operation")?,
        &format!("{path}.operation"),
    )?)?;

    let key = parse_key(member(claim, &path, "key")?, table)?;

    let fields_path = format!("{path}.fields");
    let fields = object(member(claim, &path, "fields")?, &fields_path)?;
    let fields = fields
        .iter()
        .map(|(name, value)| {
            parse_field_value(value, &format!("{fields_path}.{name}"), name)
                .map(|value| (name.clone(), value))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;

    Claim::new(key, operation, fields)
}

fn parse_key(value: &Value, table: Table) -> Result<PrimaryKey, DeltaError> {
    let key = value.as_object().ok_or(DeltaError::MalformedKey {
        table,
        detail: "expected an object".to_owned(),
    })?;
    let expected: &[&str] = match table {
        Table::Spell => &["spell_id"],
        Table::SpellEffect => &["effect_index", "spell_id"],
        Table::Item => &["entry"],
    };
    for name in key.keys() {
        if !expected.contains(&name.as_str()) {
            return Err(DeltaError::MalformedKey {
                table,
                detail: format!("unexpected member `{name}`"),
            });
        }
    }
    for name in expected {
        if !key.contains_key(*name) {
            return Err(DeltaError::MalformedKey {
                table,
                detail: format!("missing member `{name}`"),
            });
        }
    }

    match table {
        Table::Spell => {
            let spell_id = key_number(key, table, "spell_id", u64::from(u32::MAX))? as u32;
            PrimaryKey::spell(spell_id)
        }
        Table::SpellEffect => {
            let spell_id = key_number(key, table, "spell_id", u64::from(u32::MAX))? as u32;
            let effect_index = key_number(key, table, "effect_index", u64::from(u8::MAX))? as u8;
            PrimaryKey::spell_effect(spell_id, effect_index)
        }
        Table::Item => {
            let entry = key_number(key, table, "entry", u64::from(u32::MAX))? as u32;
            PrimaryKey::item(entry)
        }
    }
}

fn key_number(
    key: &Map<String, Value>,
    table: Table,
    name: &str,
    max: u64,
) -> Result<u64, DeltaError> {
    let value = &key[name];
    value
        .as_u64()
        .filter(|n| *n <= max)
        .ok_or_else(|| DeltaError::MalformedKey {
            table,
            detail: format!("`{name}` must be a whole number in 0..={max}, found `{value}`"),
        })
}

fn parse_field_value(value: &Value, path: &str, field: &str) -> Result<FieldValue, DeltaError> {
    let tagged = object(value, path)?;
    expect_members(tagged, path, &["type", "value"])?;

    let tag = string(member(tagged, path, "type")?, &format!("{path}.type"))?;
    let ty = FieldType::parse(tag).ok_or_else(|| DeltaError::UnknownFieldType {
        field: field.to_owned(),
        found: tag.to_owned(),
    })?;
    let raw = member(tagged, path, "value")?;

    match ty {
        FieldType::U8 => {
            unsigned(raw, field, ty, u64::from(u8::MAX)).map(|n| FieldValue::U8(n as u8))
        }
        FieldType::U16 => {
            unsigned(raw, field, ty, u64::from(u16::MAX)).map(|n| FieldValue::U16(n as u16))
        }
        FieldType::U32 => {
            unsigned(raw, field, ty, u64::from(u32::MAX)).map(|n| FieldValue::U32(n as u32))
        }
        FieldType::U64 => unsigned_string(raw, field).map(FieldValue::U64),
        FieldType::I32 => signed(raw, field).map(FieldValue::I32),
        FieldType::F32 => float(raw, field).map(FieldValue::F32),
        FieldType::Bool => raw
            .as_bool()
            .map(FieldValue::Bool)
            .ok_or_else(|| out_of_range(raw, field, ty)),
        FieldType::Str => raw
            .as_str()
            .map(|s| FieldValue::Str(s.to_owned()))
            .ok_or_else(|| out_of_range(raw, field, ty)),
    }
}

fn unsigned(raw: &Value, field: &str, ty: FieldType, max: u64) -> Result<u64, DeltaError> {
    raw.as_u64()
        .filter(|n| *n <= max)
        .ok_or_else(|| out_of_range(raw, field, ty))
}

/// An unsigned 64-bit value travels as a decimal string, because a JSON number loses precision
/// above 2^53 and would come back silently wrong.
fn unsigned_string(raw: &Value, field: &str) -> Result<u64, DeltaError> {
    let text = raw
        .as_str()
        .ok_or_else(|| out_of_range(raw, field, FieldType::U64))?;
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(out_of_range(raw, field, FieldType::U64));
    }
    text.parse::<u64>()
        .map_err(|_| out_of_range(raw, field, FieldType::U64))
}

fn signed(raw: &Value, field: &str) -> Result<i32, DeltaError> {
    raw.as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| out_of_range(raw, field, FieldType::I32))
}

/// JSON carries one float width, so a claimed `f32` arrives as an `f64` and is narrowed here. A
/// value that narrows to an infinity, or that was never finite, does not survive a round trip
/// through the artifact and is refused rather than stored.
fn float(raw: &Value, field: &str) -> Result<f32, DeltaError> {
    let wide = raw
        .as_f64()
        .ok_or_else(|| out_of_range(raw, field, FieldType::F32))?;
    let narrow = wide as f32;
    if narrow.is_finite() {
        Ok(narrow)
    } else {
        Err(out_of_range(raw, field, FieldType::F32))
    }
}

fn out_of_range(raw: &Value, field: &str, ty: FieldType) -> DeltaError {
    DeltaError::ValueOutOfRange {
        field: field.to_owned(),
        ty,
        literal: raw.to_string(),
    }
}

fn object<'a>(value: &'a Value, path: &str) -> Result<&'a Map<String, Value>, DeltaError> {
    value.as_object().ok_or(DeltaError::WrongJsonType {
        path: path.to_owned(),
        expected: "an object",
    })
}

fn string<'a>(value: &'a Value, path: &str) -> Result<&'a str, DeltaError> {
    value.as_str().ok_or(DeltaError::WrongJsonType {
        path: path.to_owned(),
        expected: "a string",
    })
}

fn member<'a>(
    map: &'a Map<String, Value>,
    path: &str,
    name: &str,
) -> Result<&'a Value, DeltaError> {
    map.get(name).ok_or_else(|| DeltaError::MissingMember {
        path: join(path, name),
    })
}

/// Refuses any member outside the closed list. `allowed` is sorted, matching the parsed map, so the
/// member named in the error is the same one for the same input.
fn expect_members(
    map: &Map<String, Value>,
    path: &str,
    allowed: &[&str],
) -> Result<(), DeltaError> {
    for name in map.keys() {
        if !allowed.contains(&name.as_str()) {
            return Err(DeltaError::UnexpectedMember {
                path: if path.is_empty() {
                    "the artifact root".to_owned()
                } else {
                    path.to_owned()
                },
                member: name.clone(),
            });
        }
    }
    Ok(())
}

fn join(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_owned()
    } else {
        format!("{path}.{name}")
    }
}
