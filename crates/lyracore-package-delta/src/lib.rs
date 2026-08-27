//! The **Package Delta** artifact and the claim conflict tracer.
//!
//! A Package Delta is the durable record of what one Package changes in the spell tables. It names
//! the Package, the source it was generated from, and one claim per row: the table, the typed
//! primary key, the operation, and the columns the Package sets there. A base import replaces whole
//! data families, so a Package's row edits have to be a replayable artifact rather than a one-shot
//! edit that the next import silently reverts.
//!
//! This library is pure and deterministic. It reads no files, keeps no state, reads no clock, and
//! writes nothing. It parses, canonicalizes, and traces; applying a delta to a shard is a separate
//! concern. The crate also ships `lyracore-delta-check`, a command that reads named artifact files
//! and prints the plan they form — the IO lives there, outside everything a caller links against.
//!
//! # The artifact
//!
//! ```json
//! {
//!   "version": 1,
//!   "package": "example.sharper-fireballs",
//!   "source_hash": "0000000000000000000000000000000000000000000000000000000000000000",
//!   "claims": [
//!     {
//!       "table": "game_spell",
//!       "key": { "spell_id": 133 },
//!       "operation": "update",
//!       "fields": {
//!         "cooldown_ms": { "type": "u32", "value": 1500 },
//!         "family_flags": { "type": "u64", "value": "18446744073709551615" }
//!       }
//!     },
//!     {
//!       "table": "game_spell_effect",
//!       "key": { "spell_id": 133, "effect_index": 0 },
//!       "operation": "update",
//!       "fields": { "base_points": { "type": "i32", "value": 120 } }
//!     }
//!   ]
//! }
//! ```
//!
//! Every value states its type. An unsigned 64-bit value is a decimal **string**, because a JSON
//! number loses precision above 2^53 and would come back silently wrong. A `game_spell_effect` key
//! names the spell and the effect index; the packed durable key
//! (`(spell_id << 2) | effect_index`) is derived, never authored.
//!
//! # What it refuses
//!
//! Unknown versions, tables, columns, type tags and malformed keys fail at [`PackageDelta::parse`],
//! before an applier sees the artifact. So does row deletion: uninstalling a Package replays the
//! affected families without its claims, and there is no pre-image a delete could be undone from.
//! An `update` names only the columns it changes; an `insert` carries the whole row.
//!
//! # What it traces
//!
//! [`trace`] merges the claims of several Packages. Different columns of one row merge. The same
//! column, or two inserts at one primary key, is a [`ClaimConflict`] naming both Packages and the
//! exact claim. There are no priority numbers: the tracer reports that a human has to choose.
//!
//! # Import Families
//!
//! Every claimable table belongs to one Import Family, the unit a base import clears and reloads
//! and the unit an apply runs for. [`Table`] is one closed enum with its variants grouped by
//! family, and [`Table::family`] names the owner, so an applier called for one family can tell that
//! a claim belongs to the import it is running. This build's catalogue is the spell family and the
//! item family.
//!
//! # Identifier policy
//!
//! An inserted row must sit in its family's Package identifier band, which no client and no import
//! can reach — for spells, [`PACKAGE_SPELL_ID_FLOOR`]..=[`PACKAGE_SPELL_ID_CEIL`]; for items,
//! [`PACKAGE_ITEM_ID_FLOOR`]..=[`PACKAGE_ITEM_ID_CEIL`]. An update may name any row — tuning real
//! data is the point — except a fixture-reserved one, which no Package may touch under any
//! operation. See [`ids`] for the bands, the formula a family's band follows, and why they sit
//! where they do.

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::pedantic)]
// The parser narrows every integer with an explicit bound check immediately before the cast, so the
// `as` conversions in `delta.rs` are the checked ones, not the lossy ones this lint looks for.
#![allow(clippy::cast_possible_truncation)]

mod canonical;
mod delta;
mod error;
pub mod ids;
mod schema;
mod trace;

pub use delta::{
    Claim, ClaimCounts, Operation, PackageDelta, PackageId, PrimaryKey, SourceHash, DELTA_VERSION,
};
pub use error::DeltaError;
pub use ids::{
    is_fixture_reserved_item_id, is_fixture_reserved_spell_id, is_package_item_id,
    is_package_spell_id, packed_spell_effect_id, MAX_SPELL_EFFECT_INDEX, PACKAGE_ITEM_ID_CEIL,
    PACKAGE_ITEM_ID_FLOOR, PACKAGE_SPELL_ID_CEIL, PACKAGE_SPELL_ID_FLOOR,
};
pub use schema::{Column, FieldType, FieldValue, Table, ITEM_FAMILY, SPELL_FAMILY};
pub use trace::{trace, ClaimConflict, ClaimTrace, ClaimedField, TracedRow};

/// Reads an artifact and writes it back in canonical form.
///
/// Equivalent input produces identical bytes, whatever member order, whitespace or number spelling
/// it arrived with.
///
/// # Errors
/// Any [`DeltaError`] the parse would raise.
pub fn canonicalize(json: &str) -> Result<String, DeltaError> {
    Ok(PackageDelta::parse(json)?.to_canonical_json())
}
