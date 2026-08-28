//! Why a Package artifact was refused.
//!
//! Every refusal here happens before an applier sees the artifact, so no partial write is possible.
//! Each variant names the smallest thing that was wrong, because the reader is a package author who
//! has to fix one line of a Datascript.
//!
//! A refusal about an identifier band or a key shape is family-specific and says so in its name
//! (`SpellIdNotClientSafe`, `EffectIndexOutOfRange`, `ScriptIdNotInPackageBand`). An Import Family
//! adds its own variants rather than widening one, so no message loses the detail an author needs
//! to fix the claim. The envelope refusals above them — malformed JSON, an unknown member, a bad
//! Package identity — are shared, because every artifact kind carries the same envelope.

use core::fmt;

use crate::ids::{
    FIXTURE_RESERVED_ID_CEIL, FIXTURE_RESERVED_ID_FLOOR, FIXTURE_SPELL_ID_CEIL,
    FIXTURE_SPELL_ID_FLOOR, MAX_SPELL_EFFECT_INDEX, PACKAGE_ITEM_ID_CEIL, PACKAGE_ITEM_ID_FLOOR,
    PACKAGE_SCRIPT_ID_CEIL, PACKAGE_SCRIPT_ID_FLOOR, PACKAGE_SPELL_ID_CEIL, PACKAGE_SPELL_ID_FLOOR,
};
use crate::schema::{FieldType, Table};
use crate::script::HOOK_EVENT_NAMES;

/// A Package artifact that cannot be trusted, and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaError {
    /// The bytes are not JSON at all.
    Malformed(String),
    /// The bytes hold a different artifact kind than the parser that read them.
    ///
    /// One Package's generated directory holds every kind it ships, side by side, so a router that
    /// hands the wrong file to the wrong parser must be told exactly that rather than shown a
    /// member-by-member complaint about an artifact that was never meant for it.
    WrongArtifactKind {
        /// What the parser reads.
        expected: &'static str,
        /// What the bytes actually are.
        found: String,
    },
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

    /// A refusal only the script family can raise.
    ///
    /// Grouped rather than flattened in beside the claim refusals: the script family shares this
    /// enum's envelope variants — malformed JSON, an unknown member, a bad Package identity — and
    /// nothing else, so its own refusals read better as one named group than as six more arms a
    /// claim reader has to skip past. A family with refusals of its own follows this shape.
    Script(ScriptRefusal),
}

/// Why a Script Artifact was refused, for the refusals no other Import Family can raise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptRefusal {
    /// A shipped Runtime Script sits outside the range a Package may ship in.
    IdNotInPackageBand {
        /// The rejected identifier.
        script_id: u32,
    },
    /// The `name` member is not a usable Runtime Script name.
    InvalidName {
        /// The rejected value.
        found: String,
    },
    /// The `event` member names an event outside the Module's hook catalogue.
    UnknownEvent {
        /// The rejected name.
        found: String,
    },
    /// A Runtime Script ships no Lua at all. An empty script is a Datascript that emitted nothing,
    /// not a script that does nothing.
    EmptySource {
        /// The script that carries no source.
        name: String,
    },
    /// One Script Artifact ships two scripts at one identifier.
    DuplicateId {
        /// The repeated identifier.
        script_id: u32,
    },
    /// One Script Artifact ships two scripts under one name.
    DuplicateName {
        /// The repeated name.
        name: String,
    },
}

impl fmt::Display for ScriptRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdNotInPackageBand { script_id } => write!(
                f,
                "script {script_id} is outside the Package script range \
                 {PACKAGE_SCRIPT_ID_FLOOR}..={PACKAGE_SCRIPT_ID_CEIL}; a shipped Runtime Script \
                 must use an identifier the range clears for Packages"
            ),
            Self::InvalidName { found } => write!(
                f,
                "`{found}` is not a Runtime Script name: expected 1 to 64 characters of \
                 a-z, 0-9, `-`, `_` or `.`"
            ),
            Self::UnknownEvent { found } => write!(
                f,
                "unknown event `{found}`; a Runtime Script binds to one of {}",
                known_events()
            ),
            Self::EmptySource { name } => write!(
                f,
                "Runtime Script `{name}` carries no source; a script that should do nothing is \
                 shipped disabled, not empty"
            ),
            Self::DuplicateId { script_id } => write!(
                f,
                "script {script_id} is shipped twice by one Package; every Runtime Script needs \
                 its own identifier"
            ),
            Self::DuplicateName { name } => write!(
                f,
                "Runtime Script name `{name}` is shipped twice by one Package; every Runtime \
                 Script needs its own name"
            ),
        }
    }
}

impl fmt::Display for DeltaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpellIdNotClientSafe { .. }
            | Self::SpellIdFixtureReserved { .. }
            | Self::EffectIndexOutOfRange { .. }
            | Self::ItemIdNotClientSafe { .. }
            | Self::ItemIdFixtureReserved { .. } => fmt_identifier_policy(self, f),
            // The script family groups its own refusals behind one variant, so it delegates as a
            // whole rather than adding six arms here.
            Self::Script(refusal) => refusal.fmt(f),
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
        DeltaError::WrongArtifactKind { expected, found } => write!(
            f,
            "these bytes are {found}, not {expected}; one Package ships each artifact kind in \
             its own file and each is read by its own parser"
        ),
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
    prose_list(
        Table::ALL.iter().map(|table| format!("`{table}`")),
        "no table at all in this build",
    )
}

/// The hook catalogue as the same prose list, so an event added to the Module extends this refusal
/// without touching the message.
fn known_events() -> String {
    prose_list(
        HOOK_EVENT_NAMES.iter().map(|event| format!("`{event}`")),
        "no event at all in this build",
    )
}

fn prose_list(items: impl Iterator<Item = String>, empty: &str) -> String {
    let quoted: Vec<String> = items.collect();
    match quoted.split_last() {
        None => empty.to_owned(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
    }
}
