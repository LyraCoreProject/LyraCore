//! Package Config lever: an operator stand-in for the seeding half of
//! `crate::package_config`, until an in-tree Package calls `ensure_package_config_default` for
//! real. A thin `?`-wrapper over that fn, so a live node can be seeded and re-seeded from the
//! runbook without a Package present, the same role `debug/encounter.rs`'s levers play for
//! `crate::encounter`'s package-facing primitives.

use spacetimedb::{reducer, ReducerContext};

/// Seed `(package_name, key) = value` if absent (`package_config::ensure_package_config_default`).
/// Idempotent: re-running this never overwrites a value already set, whether that value came from a
/// prior seed or from an Operator's own `set_package_config` edit.
#[reducer]
pub fn debug_seed_package_config(
    ctx: &ReducerContext,
    package_name: String,
    key: String,
    value: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    crate::package_config::ensure_package_config_default(ctx, &package_name, &key, &value);
    Ok(())
}
