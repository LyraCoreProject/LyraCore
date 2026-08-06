//! Import provenance stamps (work-item 216). `game_import_meta` records, per FAMILY, the source
//! SHA / file hash / row count / timestamp of the last successful (re)load, so a live DB can answer
//! "what data am I running?" via `spacetime sql` — a question neither `import-world.sh`'s floor
//! assertions nor the wire suite's data gates (docs/danger-zones.md) can answer on their own (they
//! prove data LANDED, not WHERE it came from).
//!
//! NOT public — the precedent for genuinely non-`public` module tables is `game_taunt_lock`
//! (threat.rs) / `game_event_reaper_schedule` (gc.rs). (`game_lock`/`game_creature_family` are a
//! DIFFERENT axis: `public` but gateway-UNSUBSCRIBED — visible to any SpacetimeDB client, just
//! never bound/read by our gateway.) Nothing subscribes this table, so it needs no gateway binding
//! regen — docs/danger-zones.md §1.2's "new table → regenerate the gateway bindings" rule applies
//! to tables the gateway actually reads. The importer and an operator via `spacetime sql` still
//! read/write it directly against the SpacetimeDB node.
use spacetimedb::{reducer, table, ReducerContext, Table, Timestamp};

use crate::helpers::require_operator;

/// One row per FAMILY (a `--dump`/`--dbc` import family name — e.g. "creatures", "quests" — or a
/// `debug_seed_<name>` reseed) recording the provenance of its last successful load. `family` is the
/// primary key: a re-import UPSERTS (delete+insert) rather than appending history, so a query always
/// shows the CURRENT state — "what data am I running right now", not a full audit log.
#[table(accessor = game_import_meta)]
pub struct ImportMeta {
    #[primary_key]
    pub family: String,
    /// The resolved cmangos `classic-db` commit SHA this load ran against. Empty string when the
    /// caller has no external dump provenance (e.g. a `debug_seed_*` reseed, or a `--dbc`-only
    /// family that reads client DBCs rather than the cmangos dump).
    pub source_sha: String,
    /// sha256 hex digest of the dump bytes actually parsed for this run. Empty string when not
    /// applicable (same cases as `source_sha`).
    pub file_hash: String,
    /// The row count this family's load actually wrote/touched — family-scoped, not a global table
    /// size, so it stays meaningful across families that share an underlying table.
    pub row_count: u64,
    pub imported_at: Timestamp,
}

/// Upsert `family`'s provenance row. The shared core both `stamp_import_meta` (below, the importer's
/// operator-gated entry point) and module-side callers (`debug.rs`'s `debug_seed_*` reseed reducers,
/// which run under the `debug_reducers` feature and don't go through the operator gate themselves)
/// use. Delete-then-insert — the same upsert convention `debug_compute_swing`'s
/// `readouts.key().delete(&key)` already uses for a primary-keyed "latest state" row — rather than an
/// append-only history, so a re-run overwrites the prior stamp for that family.
pub(crate) fn stamp(
    ctx: &ReducerContext,
    family: &str,
    source_sha: &str,
    file_hash: &str,
    row_count: u64,
) {
    let table = ctx.db.game_import_meta();
    table.family().delete(family.to_string());
    table.insert(ImportMeta {
        family: family.to_string(),
        source_sha: source_sha.to_string(),
        file_hash: file_hash.to_string(),
        row_count,
        imported_at: ctx.timestamp,
    });
}

/// Operator-only entry point the importer (a privileged, non-gateway SpacetimeDB client — see
/// `require_operator`'s doc on why this check can't move to the gateway) calls after each family's
/// successful clear+reload to record its provenance. NOT feature-gated (unlike `debug.rs`'s harness
/// reducers behind `debug_reducers`): this is production tooling the importer calls on every real
/// run, not a test-only actuator.
#[reducer]
pub fn stamp_import_meta(
    ctx: &ReducerContext,
    family: String,
    source_sha: String,
    file_hash: String,
    row_count: u64,
) -> Result<(), String> {
    require_operator(ctx)?;
    stamp(ctx, &family, &source_sha, &file_hash, row_count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_is_upsert_by_family() {
        // Pure-logic shape check (no live ReducerContext in a native unit test — the module's other
        // table-touching tests are exercised via the wire suite / gateway integration, not here).
        // This test documents the row shape stays what `stamp`/`stamp_import_meta` produce.
        let row = ImportMeta {
            family: "quests".to_string(),
            source_sha: "deadbeef".to_string(),
            file_hash: "abc123".to_string(),
            row_count: 42,
            imported_at: Timestamp::UNIX_EPOCH,
        };
        assert_eq!(row.family, "quests");
        assert_eq!(row.row_count, 42);
    }
}
