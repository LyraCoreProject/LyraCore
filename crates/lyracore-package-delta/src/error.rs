//! Why a Package Delta was refused.
//!
//! Every refusal here happens before an applier sees the artifact, so no partial write is possible.
//! Each variant names the smallest thing that was wrong, because the reader is a package author who
//! has to fix one line of a Datascript.

use core::fmt;

use crate::ids::{
    FIXTURE_RESERVED_ID_CEIL, FIXTURE_RESERVED_ID_FLOOR, FIXTURE_SPELL_ID_CEIL,
    FIXTURE_SPELL_ID_FLOOR, MAX_SPELL_EFFECT_INDEX, PACKAGE_SPELL_ID_CEIL, PACKAGE_SPELL_ID_FLOOR,
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
            Self::Malformed(detail) => write!(f, "not valid JSON: {detail}"),
            Self::UnsupportedVersion { found } => write!(
                f,
                "unsupported Package Delta version {found}; this build implements version {}",
                crate::DELTA_VERSION
            ),
            Self::MissingMember { path } => write!(f, "missing member `{path}`"),
            Self::UnexpectedMember { path, member } => {
                write!(f, "unexpected member `{member}` in `{path}`")
            }
            Self::WrongJsonType { path, expected } => {
                write!(f, "member `{path}` must be {expected}")
            }
            Self::InvalidPackageId { found } => write!(
                f,
                "`{found}` is not a Package identity: expected 1 to 64 characters of \
                 a-z, 0-9, `-`, `_` or `.`"
            ),
            Self::InvalidSourceHash { found } => write!(
                f,
                "`{found}` is not a source hash: expected 64 lowercase hexadecimal characters"
            ),
            Self::UnknownTable { found } => write!(
                f,
                "unknown table `{found}`; a Package Delta claims `{}` or `{}`",
                Table::Spell,
                Table::SpellEffect
            ),
            Self::UnknownOperation { found } => {
                write!(
                    f,
                    "unknown operation `{found}`; expected `insert` or `update`"
                )
            }
            Self::DeleteNotSupported => f.write_str(
                "row deletion is not supported; override the row to inert with an `update` instead",
            ),
            Self::MalformedKey { table, detail } => {
                write!(f, "malformed `{table}` key: {detail}")
            }
            Self::SpellIdNotClientSafe { spell_id } => write!(
                f,
                "spell {spell_id} is outside the Package spell range \
                 {PACKAGE_SPELL_ID_FLOOR}..={PACKAGE_SPELL_ID_CEIL}; an inserted spell must use an \
                 identifier no client and no import can already own"
            ),
            Self::SpellIdFixtureReserved { spell_id } => write!(
                f,
                "spell {spell_id} is fixture-reserved \
                 ({FIXTURE_SPELL_ID_FLOOR}..={FIXTURE_SPELL_ID_CEIL} and \
                 {FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}); no Package may claim it"
            ),
            Self::EffectIndexOutOfRange { effect_index } => write!(
                f,
                "effect index {effect_index} is out of range; a spell has effects \
                 0..={MAX_SPELL_EFFECT_INDEX}"
            ),
            Self::UnknownField { table, field } => {
                write!(f, "`{table}` has no claimable column `{field}`")
            }
            Self::UnknownFieldType { field, found } => {
                write!(f, "column `{field}` declares unknown type `{found}`")
            }
            Self::FieldTypeMismatch {
                table,
                field,
                expected,
                found,
            } => write!(
                f,
                "`{table}`.`{field}` is `{expected}`, but the claim declares `{found}`"
            ),
            Self::ValueOutOfRange { field, ty, literal } => {
                write!(f, "column `{field}`: `{literal}` does not fit `{ty}`")
            }
            Self::KeyColumnClaimed { table, field } => write!(
                f,
                "`{table}`.`{field}` is part of the primary key; the claim's `key` already names it"
            ),
            Self::IncompleteInsert { table, missing } => write!(
                f,
                "an `insert` into `{table}` must carry the whole row; {} column(s) absent: {}",
                missing.len(),
                missing.join(", ")
            ),
            Self::EmptyUpdate { table } => {
                write!(f, "an `update` on `{table}` must claim at least one column")
            }
            Self::DuplicateClaim { table, key } => write!(
                f,
                "`{table}` row {key} is claimed twice by one Package; merge the two claims"
            ),
        }
    }
}

impl std::error::Error for DeltaError {}
