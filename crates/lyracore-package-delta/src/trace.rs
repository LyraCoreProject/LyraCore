//! The conflict tracer: what several Packages together would do to one Import Family, and where
//! they disagree.
//!
//! Read-only and pure. It decides nothing about applying; it produces the merged picture and the
//! list of disagreements, which is exactly what a dry-run check prints and what an applier refuses
//! on.

use std::collections::BTreeMap;
use std::fmt;

use crate::delta::{Operation, PackageDelta, PackageId, PrimaryKey};
use crate::schema::{FieldValue, Table};

/// One column of a merged row, and the Package that set it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedField {
    /// The value the Package claimed.
    pub value: FieldValue,
    /// The Package that claimed it.
    pub claimed_by: PackageId,
}

/// One row after every Package's claims on it have been merged.
#[derive(Debug, Clone, PartialEq)]
pub struct TracedRow {
    key: PrimaryKey,
    operation: Operation,
    fields: BTreeMap<String, ClaimedField>,
}

impl TracedRow {
    /// The row this describes.
    #[must_use]
    pub const fn key(&self) -> PrimaryKey {
        self.key
    }

    /// The table the row lives in.
    #[must_use]
    pub const fn table(&self) -> Table {
        self.key.table()
    }

    /// The durable primary-key value, packed for an effect row.
    #[must_use]
    pub const fn row_id(&self) -> u64 {
        self.key.row_id()
    }

    /// `Insert` when any Package invents this row, `Update` when they all change an existing one.
    #[must_use]
    pub const fn operation(&self) -> Operation {
        self.operation
    }

    /// The merged columns, in canonical (name-sorted) order, each with the Package that set it.
    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<String, ClaimedField> {
        &self.fields
    }
}

/// Two Packages disagreeing about one row.
///
/// Each variant names both Packages and the exact claim, because the package author reading it has
/// to find one line in someone else's Datascript.
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimConflict {
    /// Both Packages set the same column of the same row.
    Column {
        /// The row.
        key: PrimaryKey,
        /// The column both Packages set.
        field: String,
        /// The Package that claimed the column first.
        holder: PackageId,
        /// What the holder set it to.
        holder_value: FieldValue,
        /// The Package that claimed it afterwards.
        challenger: PackageId,
        /// What the challenger set it to.
        challenger_value: FieldValue,
    },
    /// Both Packages invent a row at the same primary key.
    InsertedRow {
        /// The row.
        key: PrimaryKey,
        /// The Package that inserted first.
        holder: PackageId,
        /// The Package that inserted afterwards.
        challenger: PackageId,
    },
}

impl ClaimConflict {
    /// The row the two Packages disagree about.
    #[must_use]
    pub const fn key(&self) -> PrimaryKey {
        match self {
            Self::Column { key, .. } | Self::InsertedRow { key, .. } => *key,
        }
    }
}

impl fmt::Display for ClaimConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Column {
                key,
                field,
                holder,
                holder_value,
                challenger,
                challenger_value,
            } => write!(
                f,
                "family `{}`, table `{}`, row {key}: packages `{holder}` and `{challenger}` both claim column \
                 `{field}` (`{holder}` sets {holder_value}, `{challenger}` sets {challenger_value})",
                key.table().family(),
                key.table()
            ),
            Self::InsertedRow {
                key,
                holder,
                challenger,
            } => write!(
                f,
                "family `{}`, table `{}`, row {key}: packages `{holder}` and `{challenger}` both insert this row",
                key.table().family(),
                key.table()
            ),
        }
    }
}

/// What a set of Package Deltas together would do, and where they disagree.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimTrace {
    rows: Vec<TracedRow>,
    conflicts: Vec<ClaimConflict>,
}

impl ClaimTrace {
    /// The merged rows, in canonical order. Present whether or not there are conflicts, so a
    /// dry-run check can print the whole picture alongside the problems.
    #[must_use]
    pub fn rows(&self) -> &[TracedRow] {
        &self.rows
    }

    /// Every disagreement found, in canonical order. All of them, not just the first: an author
    /// fixing package conflicts wants the whole list in one pass.
    #[must_use]
    pub fn conflicts(&self) -> &[ClaimConflict] {
        &self.conflicts
    }

    /// True when the Packages can be applied together.
    #[must_use]
    pub fn is_clear(&self) -> bool {
        self.conflicts.is_empty()
    }
}

/// Merges the claims of several Package Deltas and reports every disagreement.
///
/// Two Packages claiming different columns of one row merge. Claiming the same column, or inserting
/// the same primary key, is a conflict. There are no priority numbers: the tracer does not pick a
/// winner, it reports that a human has to.
///
/// The Package that claims a column first becomes its `holder` in the report. That decides only how
/// the disagreement reads, never the outcome — a conflict is a conflict from either direction.
#[must_use]
pub fn trace(deltas: &[PackageDelta]) -> ClaimTrace {
    let mut rows: BTreeMap<PrimaryKey, TracedRow> = BTreeMap::new();
    let mut inserted_by: BTreeMap<PrimaryKey, PackageId> = BTreeMap::new();
    let mut conflicts = Vec::new();

    for delta in deltas {
        let package = delta.package();
        for claim in delta.claims() {
            let key = claim.key();

            if claim.operation() == Operation::Insert {
                match inserted_by.get(&key) {
                    Some(holder) => conflicts.push(ClaimConflict::InsertedRow {
                        key,
                        holder: holder.clone(),
                        challenger: package.clone(),
                    }),
                    None => {
                        inserted_by.insert(key, package.clone());
                    }
                }
            }

            let row = rows.entry(key).or_insert_with(|| TracedRow {
                key,
                operation: Operation::Update,
                fields: BTreeMap::new(),
            });
            if claim.operation() == Operation::Insert {
                row.operation = Operation::Insert;
            }

            for (name, value) in claim.fields() {
                match row.fields.get(name) {
                    Some(held) => conflicts.push(ClaimConflict::Column {
                        key,
                        field: name.clone(),
                        holder: held.claimed_by.clone(),
                        holder_value: held.value.clone(),
                        challenger: package.clone(),
                        challenger_value: value.clone(),
                    }),
                    None => {
                        row.fields.insert(
                            name.clone(),
                            ClaimedField {
                                value: value.clone(),
                                claimed_by: package.clone(),
                            },
                        );
                    }
                }
            }
        }
    }

    conflicts.sort_by(|a, b| conflict_order(a).cmp(&conflict_order(b)));

    ClaimTrace {
        rows: rows.into_values().collect(),
        conflicts,
    }
}

/// Conflicts sort by row, then by column, then by the two Packages, so the report reads the same
/// way for the same input. An inserted-row conflict sorts before the column conflicts on that row,
/// because it is the reason the row is contested at all.
fn conflict_order(conflict: &ClaimConflict) -> (PrimaryKey, &str, &str, &str) {
    match conflict {
        ClaimConflict::InsertedRow {
            key,
            holder,
            challenger,
        } => (*key, "", holder.as_str(), challenger.as_str()),
        ClaimConflict::Column {
            key,
            field,
            holder,
            challenger,
            ..
        } => (*key, field.as_str(), holder.as_str(), challenger.as_str()),
    }
}
