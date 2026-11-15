use core::fmt;

use crate::ids::{
    PackageRange, FIXTURE_RESERVED_ID_CEIL, FIXTURE_RESERVED_ID_FLOOR, MAX_QUEST_OBJECTIVE_INDEX,
    MAX_QUEST_REWARD_CHOICE_INDEX, MAX_SPELL_EFFECT_INDEX, MAX_STATS_LEVEL, PACKAGE_SCRIPT_ID_CEIL,
    PACKAGE_SCRIPT_ID_FLOOR,
};
use crate::schema::{FieldType, Table};
use crate::script::HOOK_EVENT_NAMES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaError {
    Malformed(String),
    WrongArtifactKind {
        expected: &'static str,
        found: String,
    },
    UnsupportedVersion {
        found: String,
    },
    MissingMember {
        path: String,
    },
    UnexpectedMember {
        path: String,
        member: String,
    },
    WrongJsonType {
        path: String,
        expected: &'static str,
    },
    InvalidPackageId {
        found: String,
    },
    InvalidSourceHash {
        found: String,
    },
    UnknownTable {
        found: String,
    },
    UnknownOperation {
        found: String,
    },
    DeleteNotSupported,
    MalformedKey {
        table: Table,
        detail: String,
    },
    IdOutsidePackageRange {
        range: PackageRange,
        id: u64,
    },
    IdFixtureReserved {
        range: PackageRange,
        id: u64,
    },
    EffectIndexOutOfRange {
        effect_index: u8,
    },
    QuestObjectiveIndexOutOfRange {
        obj_index: u8,
    },
    QuestRewardChoiceIndexOutOfRange {
        choice_index: u8,
    },
    StatsLevelOutOfRange {
        level: u8,
    },
    InsertNotSupported {
        table: Table,
    },
    UnknownField {
        table: Table,
        field: String,
    },
    UnknownFieldType {
        field: String,
        found: String,
    },
    FieldTypeMismatch {
        table: Table,
        field: String,
        expected: FieldType,
        found: FieldType,
    },
    ValueOutOfRange {
        field: String,
        ty: FieldType,
        literal: String,
    },
    KeyColumnClaimed {
        table: Table,
        field: String,
    },
    IncompleteInsert {
        table: Table,
        missing: Vec<String>,
    },
    EmptyUpdate {
        table: Table,
    },
    DuplicateClaim {
        table: Table,
        key: String,
    },

    Script(ScriptRefusal),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptRefusal {
    IdNotInPackageBand { script_id: u32 },
    InvalidName { found: String },
    UnknownEvent { found: String, package: String },
    EmptySource { name: String },
    DuplicateId { script_id: u32 },
    DuplicateName { name: String },
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
            Self::UnknownEvent { found, package } => write!(
                f,
                "unknown event `{found}`; a Runtime Script binds to one of {}, or to a Package \
                 Event of its own Package spelled `{package}.<name>`, where `<name>` is a \
                 lowercase letter followed by lowercase letters, digits or `_`",
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
            Self::IdOutsidePackageRange { .. }
            | Self::IdFixtureReserved { .. }
            | Self::EffectIndexOutOfRange { .. }
            | Self::QuestObjectiveIndexOutOfRange { .. }
            | Self::QuestRewardChoiceIndexOutOfRange { .. }
            | Self::StatsLevelOutOfRange { .. } => fmt_identifier_policy(self, f),
            Self::Script(refusal) => refusal.fmt(f),
            other => fmt_general(other, f),
        }
    }
}

fn fmt_identifier_policy(err: &DeltaError, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match err {
        DeltaError::IdOutsidePackageRange { range, id } => {
            let band = range.band();
            write!(
                f,
                "{} {id} is outside the Package {} range {}..={}; an inserted {} must use an \
                 identifier no client and no import can already own",
                band.row,
                band.name,
                range.floor(),
                range.ceil(),
                band.inserted
            )
        }
        DeltaError::IdFixtureReserved { range, id } => {
            let shared = format!("{FIXTURE_RESERVED_ID_FLOOR}..={FIXTURE_RESERVED_ID_CEIL}");
            let reserved = match (range, range.fixture_cluster()) {
                (_, None) => shared,
                (PackageRange::Creature, Some((floor, ceil))) => {
                    format!("{floor}..={ceil} for a template entry, {shared} project-wide")
                }
                (_, Some((floor, ceil))) => format!("{floor}..={ceil} and {shared}"),
            };
            write!(
                f,
                "{} {id} is fixture-reserved ({reserved}); no Package may claim it",
                range.band().row
            )
        }
        DeltaError::EffectIndexOutOfRange { effect_index } => write!(
            f,
            "effect index {effect_index} is out of range; a spell has effects \
             0..={MAX_SPELL_EFFECT_INDEX}"
        ),
        DeltaError::QuestObjectiveIndexOutOfRange { obj_index } => write!(
            f,
            "objective index {obj_index} is out of range; a quest has objectives \
             0..={MAX_QUEST_OBJECTIVE_INDEX}"
        ),
        DeltaError::QuestRewardChoiceIndexOutOfRange { choice_index } => write!(
            f,
            "reward choice index {choice_index} is out of range; a quest has reward choices \
             0..={MAX_QUEST_REWARD_CHOICE_INDEX}"
        ),
        DeltaError::StatsLevelOutOfRange { level } => write!(
            f,
            "level {level} is outside 1..={MAX_STATS_LEVEL}; no stat curve carries a row for it"
        ),
        other => unreachable!("{other:?} is not an identifier-policy refusal"),
    }
}

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
            crate::delta::DELTA_VERSION
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
        DeltaError::InsertNotSupported { table } => write!(
            f,
            "`{table}` rows cannot be inserted: the primary key names an entity another Import \
             Family owns, which no Package may invent; an existing row may still be updated"
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

fn known_tables() -> String {
    prose_list(
        Table::ALL.iter().map(|table| format!("`{table}`")),
        "no table at all in this build",
    )
}

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
