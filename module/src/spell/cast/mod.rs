//! Targeting + effect dispatch + the cast core (381 split of the old flat 2,999-line `cast.rs`):
//!   - `targeting` — `select_targets` / `aura_apply` / `apply_effect` and the per-kind effect handlers.
//!   - `resolve`   — the cast core: `resolve_cast_at` (+ its extracted `check_cast_gates` gate sweep) /
//!                   `resolve_cast` / `begin_cast` / the passive-apply path.
//!
//! The cast-bar teardown family (`pushback_cast` / `interrupt_cast` / `break_channel` /
//! `interrupt_cast_and_lock`) moved OUT of this module entirely, into `control.rs` — it already called
//! back into `pushback_cast` (the on-damage chokepoint), so co-locating it with the rest of the
//! crowd-control model removes a cross-module callback instead of adding one.
//!
//! Both siblings glob-import each other's surface via `crate::spell::*` (not `super::*` — they are
//! siblings, not parent/child, and each needs symbols the OTHER defines: `resolve` calls
//! `targeting::{select_targets, apply_effect, aura_apply, recipe_reagents}`; `targeting`'s E_TRIGGER arm
//! calls back into `resolve::resolve_cast_at`). This `mod.rs` glob re-exports both into
//! `crate::spell::cast::*`, and `spell::mod` globs `cast::*` in turn — so every `crate::spell::<sym>`
//! call site outside this module is unchanged by the split.

mod resolve;
mod targeting;

pub(crate) use resolve::*;
pub(crate) use targeting::*;
