//! SpacetimeDB client wiring. The gateway is just a client: it calls reducers and reads
//! state via subscriptions. There are two kinds of connection:
//!
//!   - **Coordinator** (privileged / owner identity): reads `game_account` + `game_session`
//!     (private), and config tables; calls `provision_account` / `establish_session`.
//!   - **Player** (per-account identity): one per in-world player; subscribes to self,
//!     nearby entities, and addressed movement events; calls `player_login` /
//!     `movement_update` / `logout`. Authorized in reducers via `ctx.sender`.
//!
//! Wired via `spacetimedb_sdk` (connect, subscribe, reducer calls).
//!
//! This module is split by concern behind a thin facade (pure code-motion):
//!   - `connection`: the `Coordinator` facade + inner state + live/player connections + watchdog +
//!     the `call_reducer!` macro + lifecycle constructors + session-epoch arbitration.
//!   - `reads`: cache-accessor `Coordinator` methods (RLS-bypass reads → codec views).
//!   - `reducers`: reducer-call wrapper `Coordinator` methods.
//!   - `subscriptions`: `PlayerSubscriptions` + the `subscribe_player_events` relay.
//!   - `views`: row→view converters + the thin `RealmRow`/`AccountRow` mirrors.

pub mod bindings;

pub(crate) mod aoi; // #184: `world/mod.rs`'s 10s task reads `aoi::AOI_RECENTERS` for the AOISTAT line
mod armor; // the gateway-side EFFECTIVE-armor fold for the character sheet (Approach B)
mod connection;
mod reads;
mod reducers;
pub(crate) mod subscriptions;
mod views;
mod world_store; // impl WorldStore for Coordinator (replaces the former WorldCoordinatorStore newtype)
pub(crate) mod world_index;
pub(crate) mod world_view; // #468: the shared per-shard AOI dispatch that replaced the per-player subscriptions // #468: the in-process AOI cell index that replaced the per-player box subscriptions

/// The per-account live-socket refcount behind the #447 connection release. Re-exported so the
/// world-session tests can drive the REAL arbitration through their fake store instead of a
/// re-implementation of it (a fake that reimplements the gate only ever tests the fake).
/// Test-only: production reaches it through `Coordinator::attach_account_session`,
/// `detach_account_session` (the world tier's immediate release) and
/// `detach_account_session_deferred` + `reap_idle_account_sessions` (the logon tier's, #269).
#[cfg(test)]
pub(crate) use connection::AccountSessions;
pub use connection::Coordinator;
pub use subscriptions::PlayerSubscriptions;
// Re-exported so `crate::stdb::{RealmRow, AccountRow}` resolves (they are the return types of
// `Coordinator::realm` / `account_by_username`). `allow(unused_imports)` because in this *binary*
// crate the re-export has no external consumer to mark it used, yet the path must stay resolvable.
#[allow(unused_imports)]
pub use views::{AccountRow, RealmRow};
