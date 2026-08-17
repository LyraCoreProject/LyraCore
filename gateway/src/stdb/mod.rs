//! SpacetimeDB client wiring. The gateway is just a client: it calls reducers and reads
//! state via subscriptions. Each database has one privileged coordinator connection for the
//! shared cache and callbacks, plus a small reducer-only call-pipe pool. Player verbs name their
//! actor explicitly through the operator-gated reducer surface.
//!
//! Wired via `spacetimedb_sdk` (connect, subscribe, reducer calls).
//!
//! This module is split by concern behind a thin facade (pure code-motion):
//!   - `connection`: the `Coordinator` facade + inner state + live connections + watchdog +
//!     the `call_reducer!` macro + lifecycle constructors + session-epoch arbitration.
//!   - `reads`: cache-accessor `Coordinator` methods (RLS-bypass reads → codec views).
//!   - `reducers`: reducer-call wrapper `Coordinator` methods.
//!   - `subscriptions`: `PlayerSubscriptions`, viewer setup, and shared packet builders.
//!   - `views`: row→view converters + the thin `RealmRow`/`AccountRow` mirrors.

pub mod bindings;

mod account_sessions; // per-account session-epoch + live-socket registry, split out of `connection`
pub(crate) mod aoi; // `world/mod.rs`'s 10s task reads `aoi::AOI_RECENTERS` for the AOISTAT line
mod armor; // the gateway-side EFFECTIVE-armor fold for the character sheet (Approach B)
mod connection;
mod movement_batch;
mod reads;
mod reducers;
pub(crate) mod subscriptions;
mod views;
mod world_store; // impl WorldStore for Coordinator (replaces the former WorldCoordinatorStore newtype)
pub(crate) mod world_index;
pub(crate) mod world_view; // shared per-shard spatial, broadcast, private, and owner dispatch

pub use connection::Coordinator;
pub(crate) use connection::is_reducer_refusal;
pub use subscriptions::PlayerSubscriptions;
// Re-exported so `crate::stdb::{RealmRow, AccountRow}` resolves (they are the return types of
// `Coordinator::realm` / `account_by_username`). `allow(unused_imports)` because in this *binary*
// crate the re-export has no external consumer to mark it used, yet the path must stay resolvable.
#[allow(unused_imports)]
pub use views::{AccountRow, RealmRow};
