//! Why a Package Delta was refused.
//!
//! Every refusal here happens before an applier sees the artifact, so no partial write is possible.
//! Each variant names the smallest thing that was wrong, because the reader is a package author who
//! has to fix one line of a Datascript.
//!
//! A refusal about an identifier band or a key shape is family-specific and says so in its name
//! (`SpellIdNotClientSafe`, `EffectIndexOutOfRange`). An Import Family adds its own variants rather
//! than widening one, so no message loses the detail an author needs to fix the claim.

use core::fmt;

use crate::ids::{
    FIXTURE_RESERVED_ID_CEIL, FIXTURE_RESERVED_ID_FLOOR, FIXTURE_SPELL_ID_CEIL,
    FIXTURE_SPELL_ID_FLOOR, MAX_SPELL_EFFECT_INDEX, PACKAGE_ITEM_ID_CEIL, PACKAGE_ITEM_ID_FLOOR,
    PACKAGE_SPELL_ID_CEIL, PACKAGE_SPELL_ID_FLOOR,
};
use crate::schema::{FieldType, Table};

/// A Package Delta that cannot be trusted, and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaError {
    /// The bytes are not JSON at all.
    Malformed(String),
    /// The artifact declares a version this build does not implement.
    UnsupportedVersion {
        /// The declared version.
        found: String,
    },
    /// A required member is absent. `path` is a dotted location inside the artifact.
    MissingMember {
        /// Where the member belonged.
        path: String,
    },
    /// A member this build does not know. Refused rather than ignored, so a typo in a Datascript
    /// never becomes a silently dropped claim.
    UnexpectedMember {
        /// Where the member appeared.
        path: String,
        /// The member's name.
        member: String,
    },
    /// A member holds the wrong JSON shape.
    WrongJsonType {
        /// Where the member appeared.
        path: String,
        /// What was expected there.
        expected: &'static str,
    },
    /// The `package` member is not a usable Package identity.
    InvalidPackageId {
        /// The rejected value.
        found: String,
    },
    /// The `source_hash` member is not a 64-character lowercase hexadecimal digest.
    InvalidSourceHash {
        /// The rejected value.
        found: String,
    },
    /// The `table` member names a table outside the closed list.
    UnknownTable {
        /// The rejected name.
        found: String,
    },
    /// The `operation` member names an operation outside the closed list.
    UnknownOperation {
        /// The rejected name.
        found: String,
    },
    /// A claim asked for row deletion. Packages override a row to inert instead; there is no
    /// pre-image ledger that could undo a delete on replay.
    DeleteNotSupported,
    /// The `key` member does not describe a row of its table.
    MalformedKey {
        /// The table the key was meant for.
        table: Table,
        /// What is wrong with it.
        detail: String,
    },
    /// An inserted spell sits outside the range a Package may invent.
    SpellIdNotClientSafe {
        /// The rejected identifier.
        spell_id: u32,
    },
    /// The claim targets a seeded fixture row.
    SpellIdFixtureReserved {
        /// The rejected identifier.
        spell_id: u32,
    },
    /// The effect index names a slot no spell has.
    EffectIndexOutOfRange {
        /// The rejected index.
        effect_index: u8,
    },
    /// An inserted item sits outside the range a Package may invent.
    ItemIdNotClientSafe {
        /// The rejected identifier.
        entry: u32,
    },
    /// The claim targets a seeded fixture row.
    ItemIdFixtureReserved {
        /// The rejected identifier.
        entry: u32,
    },
    /// A claim names a column the table does not have.
    UnknownField {
        /// The claimed table.
        table: Table,
        /// The rejected column name.
        field: String,
    },
    /// A value declares a type tag outside the closed list.
    UnknownFieldType {
        /// The claimed column.
        field: String,
        /// The rejected type tag.
        found: String,
    },
    /// A value declares a type the column does not take.
    FieldTypeMismatch {
        /// The claimed table.
        table: Table,
        /// The claimed column.
        field: String,
        /// The column's type.
        expected: FieldType,
        /// The declared type.
        found: FieldType,
    },
    /// A value does not fit the type it declares.
    ValueOutOfRange {
        /// The claimed column.
        field: String,
        /// The declared type.
        ty: FieldType,
        /// The value as written.
        literal: String,
    },
    /// A claim names a primary-key column in `fields`. The key already carries it.
    KeyColumnClaimed {
        /// The claimed table.
        table: Table,
        /// The primary-key column that was claimed.
        field: String,
    },
    /// An insert does not carry the whole row.
    IncompleteInsert {
        /// The claimed table.
        table: Table,
        /// The absent columns, in declaration order.
        missing: Vec<String>,
    },
    /// An update claims no columns, so it states no change.
    EmptyUpdate {
        /// The claimed table.
        table: Table,
    },
    /// One Package Delta claims the same row twice. A Package names each row once and lists every
    /// column it sets there.
    DuplicateClaim {
        /// The claimed table.
        table: Table,
        /// The row, as the conflict report writes it.
        key: String,
    },
}

impl fmt::Display for DeltaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpellIdNotClientSafe { .. }
            | Self::SpellIdFixtureReserved { .. }
            | Self::EffectIndexOutOfRange { .. }
            | Self::ItemIdNotClientSafe { .. }
            | Self::ItemIdFixtureReserved { .. } => fmt_identifier_policy(self, f),
            other => fmt_general(other, f),
        }
    }
}

/// The identifier-band refusals, split out of [`DeltaError`]'s `Display` so neither half of the
/// message trips `clippy::too_many_lines` as a family's bands add their own variants.
fn fmt_identifier_policy(err: &DeltaError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match err {
        DeltaError::SpellIdNotClientSafe { spell_id } => write!(
            f,
            "spell {spell_id} is outside the Package spell range \
             {PACKAGE_SPELL_ID_FLOOR}..={PACKAGE_SPELL_ID_CEIL}; an inserted spell must use an \
             identifier no client and no import can already own"
        ),
        DeltaError::SpellIdFixtureReserved { spell_id } => write!(
            f,
            "spell {spell_id} is fixture-reserved \
             ({FIXTURE_SPELL_ID_FLOOR}..={FIXTURE_SPELL_ID_CEIL} and \
             {FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        DeltaError::EffectIndexOutOfRange { effect_index } => write!(
            f,
            "effect index {effect_index} is out of range; a spell has effects \
             0..={MAX_SPELL_EFFECT_INDEX}"
        ),
        DeltaError::ItemIdNotClientSafe { entry } => write!(
            f,
            "item {entry} is outside the Package item range \
             {PACKAGE_ITEM_ID_FLOOR}..={PACKAGE_ITEM_ID_CEIL}; an inserted item must use an \
             identifier no client and no import can already own"
        ),
        DeltaError::ItemIdFixtureReserved { entry } => write!(
            f,
            "item {entry} is fixture-reserved \
             ({FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
        ),
        other => unreachable!("{other:?} is not an identifier-policy refusal"),
    }
}

/// Every refusal that is not an identifier-band policy.
fn fmt_general(err: &DeltaError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match err {
        DeltaError::Malformed(detail) => write!(f, "not valid JSON: {detail}"),
        DeltaError::UnsupportedVersion { found } => write!(
            f,
            "unsupported Package Delta version {found}; this build implements version {}",
            crate::DELTA_VERSION
        ),
        DeltaError::MissingMember { path } => write!(f, "missing member `{path}`"),
        DeltaError::UnexpectedMember { path, member } => {
            write!(f, "unexpected member `{member}` in `{path}`")
        }
        DeltaError::WrongJsonType { path, expected } => {
            write!(f, "member `{path}` must be {expected}")
        }
        DeltaError::InvalidPackageId { found } => write!(
            f,
            "`{found}` is not a Package identity: expected 1 to 64 characters of \
             a-z, 0-9, `-`, `_` or `.`"
        ),
        DeltaError::InvalidSourceHash { found } => write!(
            f,
            "`{found}` is not a source hash: expected 64 lowercase hexadecimal characters"
        ),
        DeltaError::UnknownTable { found } => write!(
            f,
            "unknown table `{found}`; a Package Delta claims {}",
            known_tables()
        ),
        DeltaError::UnknownOperation { found } => {
            write!(
                f,
                "unknown operation `{found}`; expected `insert` or `update`"
            )
        }
        DeltaError::DeleteNotSupported => f.write_str(
            "row deletion is not supported; override the row to inert with an `update` instead",
        ),
        DeltaError::MalformedKey { table, detail } => {
            write!(f, "malformed `{table}` key: {detail}")
        }
        DeltaError::UnknownField { table, field } => {
            write!(f, "`{table}` has no claimable column `{field}`")
        }
        DeltaError::UnknownFieldType { field, found } => {
            write!(f, "column `{field}` declares unknown type `{found}`")
        }
        DeltaError::FieldTypeMismatch {
            table,
            field,
            expected,
            found,
        } => write!(
            f,
            "`{table}`.`{field}` is `{expected}`, but the claim declares `{found}`"
        ),
        DeltaError::ValueOutOfRange { field, ty, literal } => {
            write!(f, "column `{field}`: `{literal}` does not fit `{ty}`")
        }
        DeltaError::KeyColumnClaimed { table, field } => write!(
            f,
            "`{table}`.`{field}` is part of the primary key; the claim's `key` already names it"
        ),
        DeltaError::IncompleteInsert { table, missing } => write!(
            f,
            "an `insert` into `{table}` must carry the whole row; {} column(s) absent: {}",
            missing.len(),
            missing.join(", ")
        ),
        DeltaError::EmptyUpdate { table } => {
            write!(f, "an `update` on `{table}` must claim at least one column")
        }
        DeltaError::DuplicateClaim { table, key } => write!(
            f,
            "`{table}` row {key} is claimed twice by one Package; merge the two claims"
        ),
        other => unreachable!("{other:?} is an identifier-policy refusal, not a general one"),
    }
}

impl std::error::Error for DeltaError {}

/// The whole catalogue as a prose list — "`a`", "`a` or `b`", "`a`, `b` or `c`" — so an Import
/// Family that adds tables extends the refusal without touching the message.
fn known_tables() -> String {
    let quoted: Vec<String> = Table::ALL
        .iter()
        .map(|table| format!("`{table}`"))
        .collect();
    match quoted.split_last() {
        None => "no table at all in this build".to_owned(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}
