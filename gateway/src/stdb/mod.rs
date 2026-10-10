//! SpacetimeDB client wiring. The gateway is just a client: it calls reducers and reads
//! state via subscriptions. Each database has one privileged coordinator connection for the
//! shared cache and callbacks, plus a small reducer-only call-pipe pool. Player verbs name their
//! actor explicitly through the operator-gated reducer surface.
//!
//! Wired via `spacetimedb_sdk` (connect, subscribe, reducer calls).
//!
//! Each Store family trait is implemented on `Coordinator` in `store/<family>.rs`; its trait
//! methods call the reducer directly. `classify` decides whether a failed call is a Refusal or a
//! Transport Loss.
//!   - `connection`: the `Coordinator` facade + inner state + live connections + watchdog +
//!     the `call_reducer!` macro, `classify` + lifecycle constructors.
//!   - `account_sessions`: durable Account ownership, the Actor's signed `SessionActor`, and
//!     socket closure on renewal failure.
//!   - `reads`: cache reads that several families, relays or logon share.
//!   - `reducers`: reducer calls no Store trait owns (relays, logon, provisioning, heartbeat).
//!   - `realm_db`: `RealmDb` for `Coordinator`, the Realm-core routing seam.
//!   - `store`: one Store family adapter per file.
//!   - `subscriptions`: `PlayerSubscriptions`, viewer setup, and shared packet builders.
//!   - `views`: row→view converters + the thin `RealmRow`/`AccountRow` mirrors.

// The SpacetimeDB codegen writes one function per table subscription; several run past any
// reasonable length ceiling and no edit here survives a regeneration.
#[allow(clippy::too_many_lines)]
pub mod bindings;

mod account_sessions;
pub(crate) mod aoi; // `world/mod.rs`'s 10s task reads `aoi::AOI_RECENTERS` for the AOISTAT line
mod armor; // the gateway-side EFFECTIVE-armor fold for the character sheet (Approach B)
mod auction_holds;
mod connection;
mod member_stats_relay; // Member Stats for group mates outside the viewer's AOI
mod movement_batch;
mod reads;
mod realm_db; // impl RealmDb for Coordinator
mod reducers;
mod store; // one Store family adapter per file
pub(crate) mod subscriptions;
mod views;
pub(crate) mod world_index;
pub(crate) mod world_view; // shared per-shard spatial, broadcast, private, and owner dispatch

pub use connection::Coordinator;
pub(crate) use connection::ReducerCallError;
pub(crate) use connection::{classify, ignore_refusal, DurableFailure};
pub use subscriptions::PlayerSubscriptions;
// Re-exported so `crate::stdb::{RealmRow, AccountRow}` resolves (they are the return types of
// `Coordinator::realm` / `account_by_username`). `allow(unused_imports)` because in this *binary*
// crate the re-export has no external consumer to mark it used, yet the path must stay resolvable.
#[allow(unused_imports)]
pub use views::{AccountRow, RealmRow};
