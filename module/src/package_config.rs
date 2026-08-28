//! `game_package_config`: one durable key-value surface any installed Package reads and the
//! Operator edits. A Package that wants an Operator-tunable value has, until now, had to invent its
//! own table and its own edit story — the module cannot read files at runtime, so a config-file
//! convention can never exist module-side. This is the one seam instead.
//!
//! Module-only and gateway-UNSUBSCRIBED, the `game_lock` precedent (`gameobject.rs`): `public` so
//! `spacetime sql` can read it, no gateway binding files and no gateway subscription, because
//! nothing in the gateway needs it — docs/danger-zones.md §1.2's rule ("a table binding is only
//! needed if the gateway subscribes to or reads that table") applies. A future full binding regen
//! picks up this table's and this reducer's bindings harmlessly, same as the `record_shard_load`
//! precedent there.
//!
//! Convention (also in `packages/README.md`): a Package seeds its own defaults idempotently, from
//! its own ensure/init path, via [`ensure_package_config_default`] — so listing this table always
//! shows real keys with live values, never a blank slate an Operator has to populate cold. The
//! Operator edits a value through [`set_package_config`] today; a CLI verb for it is tracked
//! separately.

use spacetimedb::{reducer, table, ReducerContext, Table};

use crate::helpers::require_operator;

/// One `(package_name, key)` → `value` row. Packages parse their own values; the module never
/// interprets them. PK-pair pattern (see `game_encounter_state`): an `#[auto_inc]` surrogate `id`
/// carries the primary key, and `by_package_key` — a btree over the logical `(package_name, key)`
/// pair — resolves one row, while a filter on its leading column alone (`package_name`) lists a
/// package's known keys without a full-table scan. [static]
#[table(
    accessor = game_package_config,
    public,
    index(accessor = by_package_key, btree(columns = [package_name, key]))
)]
pub struct PackageConfig {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub package_name: String,
    pub key: String,
    pub value: String,
}

fn config_row(ctx: &ReducerContext, package_name: &str, key: &str) -> Option<PackageConfig> {
    ctx.db
        .game_package_config()
        .by_package_key()
        .filter((package_name, key))
        .next()
}

/// Every key currently set for `package_name`, sorted — what the refusal message in
/// [`set_package_config`] names as "here is what does exist". A `by_package_key` prefix scan
/// (`package_name` is its leading column), not a full-table iteration.
fn known_keys(ctx: &ReducerContext, package_name: &str) -> Vec<String> {
    let mut keys: Vec<String> = ctx
        .db
        .game_package_config()
        .by_package_key()
        .filter(package_name)
        .map(|row| row.key)
        .collect();
    keys.sort();
    keys
}

/// How [`set_package_config`] should react to a write, given only whether a row already exists for
/// `(package_name, key)` and whether the caller passed `allow_new` (module — pure, ctx-free, so this
/// is unit-tested without a `ReducerContext`). An existing row always updates, regardless of
/// `allow_new`; an absent one inserts only when the caller opted in, and is refused otherwise.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConfigWrite {
    Update,
    Insert,
    Refuse,
}

pub(crate) fn decide_config_write(row_exists: bool, allow_new: bool) -> ConfigWrite {
    match (row_exists, allow_new) {
        (true, _) => ConfigWrite::Update,
        (false, true) => ConfigWrite::Insert,
        (false, false) => ConfigWrite::Refuse,
    }
}

/// The refusal text for an unknown `(package_name, key)` pair (module — pure, ctx-free): names the
/// package's existing keys so a typo reads as a loud error instead of a value nobody ever reads.
pub(crate) fn unknown_key_message(package_name: &str, key: &str, known_keys: &[String]) -> String {
    if known_keys.is_empty() {
        format!("package '{package_name}' has no config keys yet; pass allow_new to set '{key}'")
    } else {
        format!(
            "package '{package_name}' has no config key '{key}'; known keys: {}. Pass allow_new to set it anyway",
            known_keys.join(", ")
        )
    }
}

/// Operator-gated write to one `(package_name, key)` row: updates an existing row, or inserts
/// a new one only when `allow_new` is set. Refuses an absent key with its package's existing keys
/// named (sorted) — see [`decide_config_write`] and [`unknown_key_message`] for the pure decision
/// and message this reducer carries out.
#[reducer]
pub fn set_package_config(
    ctx: &ReducerContext,
    package_name: String,
    key: String,
    value: String,
    allow_new: bool,
) -> Result<(), String> {
    require_operator(ctx)?;
    let existing = config_row(ctx, &package_name, &key);
    match decide_config_write(existing.is_some(), allow_new) {
        ConfigWrite::Update => {
            let mut row = existing.expect("Update implies config_row found a row");
            row.value = value;
            ctx.db.game_package_config().id().update(row);
            Ok(())
        }
        ConfigWrite::Insert => {
            ctx.db.game_package_config().insert(PackageConfig {
                id: 0,
                package_name,
                key,
                value,
            });
            Ok(())
        }
        ConfigWrite::Refuse => Err(unknown_key_message(
            &package_name,
            &key,
            &known_keys(ctx, &package_name),
        )),
    }
}

/// Whether [`ensure_package_config_default`] should insert its default (module — pure, ctx-free):
/// only when no row exists yet, so a Package can call the seeding entry point from its ensure/init
/// path on every startup and never clobber an Operator's edit sitting in the row already.
pub(crate) fn should_seed_default(row_exists: bool) -> bool {
    !row_exists
}

/// A Package's idempotent default-seeding entry point: inserts `(package, key) = value` only
/// when no row exists yet — see [`should_seed_default`] for the decision this carries out. `pub(crate)`
/// — a Package's own defaults are seeded from inside the module build, not through a reducer an
/// external caller could hit.
pub(crate) fn ensure_package_config_default(
    ctx: &ReducerContext,
    package: &str,
    key: &str,
    value: &str,
) {
    let existing = config_row(ctx, package, key);
    if !should_seed_default(existing.is_some()) {
        return;
    }
    ctx.db.game_package_config().insert(PackageConfig {
        id: 0,
        package_name: package.to_string(),
        key: key.to_string(),
        value: value.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_row_always_updates_regardless_of_allow_new() {
        assert_eq!(decide_config_write(true, false), ConfigWrite::Update);
        assert_eq!(decide_config_write(true, true), ConfigWrite::Update);
    }

    #[test]
    fn an_absent_row_inserts_only_when_allow_new_is_set() {
        assert_eq!(decide_config_write(false, true), ConfigWrite::Insert);
        assert_eq!(decide_config_write(false, false), ConfigWrite::Refuse);
    }

    #[test]
    fn the_refusal_names_the_packages_existing_keys_sorted() {
        let known = vec!["max_bots".to_string(), "spawn_rate".to_string()];
        let message = unknown_key_message("playerbots", "spwan_rate", &known);
        assert!(message.contains("playerbots"), "{message}");
        assert!(message.contains("spwan_rate"), "{message}");
        assert!(
            message.contains("max_bots, spawn_rate"),
            "keys must be named sorted: {message}"
        );
    }

    #[test]
    fn a_package_with_no_keys_yet_still_names_itself_in_the_refusal() {
        let message = unknown_key_message("brand_new_package", "any_key", &[]);
        assert!(message.contains("brand_new_package"), "{message}");
        assert!(message.contains("any_key"), "{message}");
    }

    #[test]
    fn seeding_a_default_is_idempotent() {
        assert!(
            should_seed_default(false),
            "an absent default must be seeded"
        );
        assert!(
            !should_seed_default(true),
            "an existing row (an Operator's edit, or a prior seed) must never be reseeded"
        );
    }
}
