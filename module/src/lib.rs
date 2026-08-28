//! SpacetimeDB module — **all** WoW 1.12 game state and logic.
//!
//! This is the thin index. The actual tables and reducers live in per-domain submodules — the
//! `mod` declarations below are the authoritative list of what exists (47 of them at last count,
//! from `action_bar` to `xp`); this doc comment doesn't duplicate that list because a hand-kept
//! copy of it only goes stale as the game grows. A few load-bearing ones to start from: `world`
//! is the live entity/movement relay, `combat`/`spell`/`items`/`quest`/`instance` are the bulk of
//! the gameplay logic, `transfer` is the escrowed cross-database character-transport subsystem,
//! and `seed` is the `init` lifecycle reducer that populates a fresh database.
//!
//! Each module is re-exported below so submodules can reference any table uniformly via
//! `crate::{<accessor_trait>, <RowType>}`. The `#[table(accessor = X)]` macro generates a trait
//! named `X` (inheriting the struct's `pub` visibility) that provides `ctx.db.X()`; the glob
//! re-exports carry those traits to the crate root. Scheduled tables are co-located with the
//! reducer they name (`scheduled(...)` resolves the reducer in its own module's scope).
//!
//! Built against SpacetimeDB 2.5 (`accessor =` table syntax; `ctx.sender()` /
//! `ctx.db.<accessor>()`; RLS via the `unstable` `client_visibility_filter`). Categories,
//! visibility, and field-index annotations follow `docs/schema.md`.

/// Marks a table's rows as keyed by `character_guid` and defines the sweep fn invoked when a
/// character is deleted (`world::cascade_delete_character`) or re-owned across an identity change
/// (`world::restamp_owned_data`). Write the invocation right next to the table it sweeps — `build.rs`
/// text-scans every file under `src/` for this marker and generates the two registries `world.rs`
/// iterates, `CHARACTER_OWNED_DELETE_SWEEPS` and `CHARACTER_OWNED_RESTAMP_SWEEPS` (see
/// `$OUT_DIR/character_sweeps.rs`, `include!`d below). A new character-keyed table therefore needs
/// exactly ONE addition — this marker, colocated with the table, in the table's own file — and never
/// an edit to `world.rs` (the modularity goal: an out-of-tree custom-content table gets the same
/// one-line hookup).
///
/// Because the scan is a text match, not a real parser, the invocation MUST be written in exactly
/// one of these two shapes or `build.rs` panics (a malformed marker fails the build loudly; it
/// never skips silently). Comments and strings are stripped before scanning, which is why this doc
/// can show the real syntax:
///
///   `character_owned!(delete, fn <name>(<ctx>, <guid>) { <body> })` — two params (ctx, the
///   character guid); expands to `pub(crate) fn <name>(ctx: &ReducerContext, guid: u64)`.
///
///   `character_owned!(restamp, fn <name>(<ctx>, <guid>, <identity>) { <body> })` — three
///   params (ctx, guid, the new owning identity); expands to `pub(crate) fn <name>(ctx:
///   &ReducerContext, guid: u64, identity: Identity)`.
///
///   `character_owned!(transfer, fn <name>(<ctx>, <guid>, <io>) { table = .., by = .., .. })` and
///   `character_owned!(not_transported, fn <name>())` — the CROSS-DATABASE row transport (issue
///   #19), which is what lets a table's rows actually leave one SpacetimeDB database and arrive in
///   another. Both expand to `pub(crate) fn <name>(ctx: &ReducerContext, guid: u64, io: &mut
///   crate::transfer::RowIo)`. Required for every `game_*` manifest table —
///   `transfer::every_manifest_table_can_cross_a_database_boundary` fails the test suite if one is
///   missing, because a table with no transport arm is silent data LOSS the first time a character
///   crosses a real shard boundary. See the GRAMMAR section below: these two are the ONLY shapes
///   that parse, deliberately.
///
/// See `crate::reputation::sweep_delete_game_player_reputation` for a worked example.
///
/// # The transport GRAMMAR, and why it is a grammar (#380)
///
/// A transport arm used to be a free-form `$body:block`, so "does this arm actually transport, or
/// does it silently drop the table's rows?" was a question only a source SCANNER could answer — and
/// a scanner is an arms race it eventually loses. The #36 review proved it: repointing
/// `sweep_transfer_game_item_instance` at `not_transported` deleted every character's gear on every
/// hop with 468 module tests green, and each hardening of the scan (`contains("move_rows")`, then
/// "exactly once", then "at the top", then "and it filters by the guid") was defeated by the next
/// dead-branch trick.
///
/// So the arm has no body any more. It DECLARES the two facts the transport needs and the macro
/// writes the code, which makes every one of those mutations a parse error instead of a test the
/// scanner has to be clever enough to fail:
///
/// ```text
/// // Transports. Exports the rows `<index>` yields for the transferring guid, and re-inserts each
/// // arriving row at the destination with the surrogate primary key `<pk>` re-minted (an
/// // `#[auto_inc]` id is local to ONE database, so carrying it verbatim either collides with a row
/// // the destination already minted or means nothing there).
/// crate::character_owned!(transfer, fn sweep_transfer_game_player_skill(ctx, character_guid, io) {
///     table = game_player_skill,
///     by = by_character,
///     remint = id,
/// });
///
/// // Transports, KEEPING the row's primary key — for the handful of tables whose key is derived
/// // from the OWNER guid (so it is already namespaced per character and cannot collide) and is the
/// // id the CLIENT knows the row by. Re-minting one of those makes the row look brand new.
/// crate::character_owned!(transfer, fn sweep_transfer_game_item_instance(ctx, character_guid, io) {
///     table = game_item_instance,
///     by = by_owner_guid,
///     keep_key,
/// });
///
/// // Declines. A DECISION written at the table (one-shot relay rows whose durable half lives on the
/// // character row, or state another database is authoritative for) — and it must ALSO be listed on
/// // `transfer::NOT_TRANSPORTED` with its reason, which build.rs's generated
/// // `CHARACTER_OWNED_NOT_TRANSPORTED` cross-checks.
/// crate::character_owned!(not_transported, fn sweep_transfer_game_rest_state_event());
/// ```
///
/// `ctx`/`character_guid`/`io` stay in the `transfer` head because they are the expansion's own
/// parameter names, and writing them keeps the marker's shape identical to the `delete`/`restamp`
/// forms build.rs scans for.
#[macro_export]
macro_rules! character_owned {
    (delete, fn $name:ident($ctx:ident, $guid:ident) $body:block) => {
        pub(crate) fn $name($ctx: &spacetimedb::ReducerContext, $guid: u64) $body
    };
    (restamp, fn $name:ident($ctx:ident, $guid:ident, $identity:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $identity: spacetimedb::Identity,
        ) $body
    };
    (transfer, fn $name:ident($ctx:ident, $guid:ident, $io:ident) {
        table = $table:ident,
        by = $index:ident,
        remint = $pk:ident $(,)?
    }) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::move_rows(
                $ctx,
                $io,
                || $ctx.db.$table().$index().filter(&$guid).collect::<Vec<_>>(),
                |ctx: &spacetimedb::ReducerContext, mut row| {
                    // Surrogate `#[auto_inc]` PK: local to ONE database, so the destination mints
                    // its own rather than inheriting a number that means something else there.
                    row.$pk = 0;
                    ctx.db.$table().insert(row);
                },
            );
        }
    };
    (transfer, fn $name:ident($ctx:ident, $guid:ident, $io:ident) {
        table = $table:ident,
        by = $index:ident,
        keep_key $(,)?
    }) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::move_rows(
                $ctx,
                $io,
                || $ctx.db.$table().$index().filter(&$guid).collect::<Vec<_>>(),
                |ctx: &spacetimedb::ReducerContext, row| {
                    ctx.db.$table().insert(row);
                },
            );
        }
    };
    (not_transported, fn $name:ident()) => {
        pub(crate) fn $name(
            _ctx: &spacetimedb::ReducerContext,
            _character_guid: u64,
            io: &mut $crate::transfer::RowIo<'_>,
        ) {
            $crate::transfer::not_transported(io);
        }
    };
}

/// Registers a periodic pass with the core scheduler tick pipeline. Same build.rs
/// text-scan mechanism as the `character_owned` marker: write the invocation next to the code it
/// drives (core src/ or a drop-in `packages/<name>/src/` file — every checkout has at least the
/// reference Package, `packages/example/`) and the generated `GAME_TICK_PASSES` registry picks it
/// up with zero core-file edits.
///
/// The ONE accepted shape (anything else fails the build loudly; commented-out/quoted marker
/// syntax is inert — build.rs strips comments and strings before scanning):
///
///   `game_tick_pass!(fn <name>(<ctx>) { <body> })` — expands to
///   `pub(crate) fn <name>(ctx: &ReducerContext)`.
///
/// CADENCE: every registered pass runs at the END of every `tick_creatures` tick (`MOVE_TICK_SECS`,
/// 0.5s), AFTER all core passes — a package pass can never wedge itself between load-bearing core
/// pass orderings. A pass needing a slower cadence must self-quantize on `ctx.timestamp` (the
/// `is_sense_tick` pattern in `creatures/tick/mod.rs`).
#[macro_export]
macro_rules! game_tick_pass {
    (fn $name:ident($ctx:ident) $body:block) => {
        pub(crate) fn $name($ctx: &spacetimedb::ReducerContext) $body
    };
}

/// Registers a NOTIFY-ONLY handler on a core gameplay event. Same build.rs
/// text-scan mechanism as the `character_owned` marker; the generated per-event `GAME_HOOKS_*`
/// arrays are dispatched at the core chokepoints via `hooks::fire_*` (one dispatch line each — see
/// `src/hooks.rs` for the event catalog, payload shapes, and firing semantics).
///
/// The ONE accepted shape (anything else — including an unknown event name — fails the build
/// loudly; commented-out/quoted marker syntax is inert):
///
///   `game_hook!(<event>, fn <name>(<ctx>, <payload>) { <body> })` — expands to
///   `pub(crate) fn <name>(ctx: &ReducerContext, payload: &<EventPayload>)` where the payload type
///   is resolved from the event name via `hooks::payload_for` (generated from build.rs's
///   `HOOK_EVENTS` catalog).
///
/// Handlers OBSERVE and may act through the same reducer-internal fns core code uses; they must not
/// assume any ordering relative to other handlers of the same event beyond the registry's
/// deterministic (sorted-by-path) order. Mutating/decorator semantics (veto, fold) are explicitly
/// Phase 2 and NOT this mechanism.
#[macro_export]
macro_rules! game_hook {
    ($event:ident, fn $name:ident($ctx:ident, $payload:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $payload: &$crate::hooks::payload_for::$event,
        ) $body
    };
}

/// Installs one package as the sole authority for a map-scoped encounter binding. `build.rs`
/// rejects duplicate bindings, and the generated registry keeps routing out of core code.
#[macro_export]
macro_rules! encounter_package {
    ($binding:ident, fn $name:ident($ctx:ident, $instance_id:ident, $signal:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $instance_id: u64,
            $signal: $crate::encounter::EncounterSignal,
        ) -> Result<(), String> $body
    };
}

// GENERATED by module/build.rs from the `character_owned` marker invocations scattered across src/
// and packages/*/src/. Defines
// `CHARACTER_OWNED_DELETE_SWEEPS: &[fn(&ReducerContext, u64)]` and
// `CHARACTER_OWNED_RESTAMP_SWEEPS: &[fn(&ReducerContext, u64, Identity)]` at crate scope
// (`crate::CHARACTER_OWNED_DELETE_SWEEPS`). `build.rs` panics at compile time if a marker is
// malformed, so an empty/short slice here means "no markers found", not "scan failed silently".
include!(concat!(env!("OUT_DIR"), "/character_sweeps.rs"));

// GENERATED by module/build.rs from packages/*/src/mod.rs discovery: one
// `#[path = ...] pub mod pkg_<name>;` per drop-in package, compiling the package's Rust into this
// crate. Empty when packages/ holds no Rust packages.
include!(concat!(env!("OUT_DIR"), "/package_mods.rs"));

// GENERATED by module/build.rs from package marker invocations.
// Defines `GAME_TICK_PASSES: &[(&str, fn(&ReducerContext))]` (name string kept for logging)
// and one `GAME_HOOKS_<EVENT>: &[fn(&ReducerContext, &Payload)]` per event in build.rs's
// HOOK_EVENTS catalog, plus the map-scoped encounter authority registry.
include!(concat!(env!("OUT_DIR"), "/package_registries.rs"));

mod action_bar;
mod actor;
mod auction;
mod auth;
mod breath_relay;
mod bridge;
mod breath;
mod character;
mod chat;
mod combat;
mod config;
mod corpse;
mod creatures;
mod exploration;
// Layer 1 test-only reducers (debug_teleport / set_health / spawn_at_feet / force_cast / set_level /
// clear_creatures) for the client-automation harness. PROD-SAFE: the whole module is behind the
// `debug_reducers` Cargo feature (default OFF) — a plain build / production publish compiles it out
// entirely. Enable for the test build via `--build-options='--features=debug_reducers'`. See
// debug/mod.rs (#386 split this into a directory along its section banners: mod/readout/audit/
// repair/encounter/instance/fingerprint).
#[cfg(feature = "debug_reducers")]
mod debug;
// Encounter kernel (work-item 228): instance-scoped encounter state, HP-threshold/death/GO-use
// hooks, and the shared choreography primitives packages consume (packages/<dungeon>/ own content).
mod encounter;
mod duel;
mod faction;
mod gameobject;
mod gc;
mod gm;
// Module-private DOOR/BUTTON collision-mesh store: the import reducers `--go-models` calls.
// Consumption (the state-gated ray merge) is the `game_go_collider` registry.
mod go_model;
// Graveyard resolution (work-item 209/226): the death-release subsystem `world::do_repop` calls to
// pick where a ghost teleports. Extracted from `world.rs` (issue #385).
mod graveyard;
mod group;
mod gw;
mod helpers;
pub mod hooks;
mod import_meta;
// Dungeon-instancing lifecycle (work-item 190 slices 2+3): game_instance/game_instance_binding,
// the areatrigger resolve-or-create entry, per-instance population spawn, and the reset/reap loop.
mod combo;
mod instance;
mod items;
mod load;
mod loot;
/// Mail: the durable `game_mail` row, its sweeps, and the shared insert core. Realm-core is
/// authoritative; a single-database gateway reads and writes its own copy through the same rules.
mod mail;
/// Mail attachment escrow: the source-side fence, the mail-plane commit keyed by the same
/// caller-chosen id, and the reaper. The mechanism for moving value into a mail row across a
/// database boundary no transaction spans; the single-database plane deliberately bypasses it.
mod mail_escrow;
/// Batched movement republish (#461): the PRIVATE `game_entity_motion_pending` staging table that
/// `movement_update` writes, and the 20 Hz `publish_motion` tick that drains it into the public
/// `game_entity_motion` relay in one transaction.
mod motion;
/// Land mounts: the `A_MOUNTED` state model, the `mount_display_id` projection, and the one shared
/// idempotent dismount every removal path converges on.
mod mount;
pub mod nav;
/// Package Deltas applied to the spell tables as the last stage of their base import, and the
/// per-Package provenance that records what landed. Not re-exported below, for the same reason
/// `import_meta` is not: nothing outside this module reads its table.
mod package_import;
mod professions;
/// Deploy-safety tripwire (#223): source-scans `scripts/**` + `tools/**` for a destructive
/// `spacetime publish -c`, and pins the sanctioned deploy script's own argv guard and required
/// flags. Test-only; reads files, never runs the CLI.
#[cfg(test)]
mod publish_safety;
mod quest;
mod realm_core;
mod region;
mod reputation;
mod rest;
// The Runtime Script Host. Called from `script_binding`'s dispatch on every hook event in a
// default build, so it no longer needs the "no caller yet" allow the host shipped with.
mod runtime_script;
/// The `game_script` table and the Event Binding dispatch: which Runtime Scripts run for which
/// event. Not re-exported below, for the same reason `package_import` is not — nothing outside this
/// module reads its table.
mod script_binding;
mod seed;
mod skill;
mod skilldata;
mod spell;
mod stats;
mod talent;
mod taxi;
pub mod terrain;
#[cfg(test)]
mod test_scan;
mod threat;
mod trade;
mod trainer;
mod transfer;
/// Exact per-cell vmap collision-triangle store + LoS/collision ray queries (issue #521, part of
/// the #169 full-vmap epic). Builds on `nav`'s obstruction-grid approximation with an exact
/// triangle store; see the module doc comment for the split.
pub mod vmap;
// Source-scan tripwires (issue #379 pulled these out of this file, which had grown to 1,146 lines —
// four fifths of it cfg(test) scan machinery — so the doc comment atop this file ("this is the thin
// index") stayed true). See `tripwires.rs`'s own module doc for the roster and the shared engine in
// `test_scan.rs` it now runs on.
#[cfg(test)]
mod tripwires;
/// Per-zone weather: the seasonal climate weights, the current-weather row the gateway relays, the
/// ten-minute roll that advances it, and the operator's forced-weather lever.
mod weather;
mod world;
mod xp;

pub use action_bar::*;
pub use auction::*;
pub use auth::*;
pub use breath::*;
pub use breath_relay::BreathRelayEvent; // gateway schema-parity relay (#141)
pub use bridge::*;
pub use character::*;
pub use chat::*;
pub use combat::*;
pub use config::*;
pub use corpse::*;
pub use creatures::*;
#[cfg(feature = "debug_reducers")]
pub use debug::*;
pub use duel::*;
pub use encounter::*;
pub use exploration::CharacterExplored; // re-exported for the gateway schema-parity test (282)
pub use rest::RestStateEvent; // re-exported for the gateway schema-parity test (#468 4c)
pub use spell::stacking::SpellGroupRule; // Keeps this generated table in schema-parity coverage.
pub use faction::*;
pub use gameobject::*;
pub use gw::*;
pub use gc::*;
pub use gm::*;
pub use graveyard::*;
pub use group::*;
pub use instance::*;
pub use items::*;
pub use load::*;
pub use loot::*;
pub use mail::Mail; // re-exported for the gateway schema-parity test
pub use mail_escrow::MailEscrow; // re-exported for the gateway schema-parity test
pub use motion::*;
pub use quest::*;
pub use realm_core::*;
pub use region::*;
pub use reputation::*;
pub use skill::*;
pub use skilldata::*;
pub use spell::*;
pub use stats::*;
pub use talent::*;
pub use taxi::*;
pub use threat::*;
pub use trade::*;
pub use trainer::*;
pub use transfer::TransferOut; // re-exported for the gateway schema-parity test (#19)
pub use weather::*;
pub use world::*;
pub use xp::*;

/// How long a movement / creature-move event lives before the reaper GCs it. Events are delivered
/// to subscribers in real time on insert, so a short TTL is safe (see docs).
pub(crate) const EVENT_TTL_MICROS: i64 = 1_000_000; // 1s

/// How long a never-answered `game_group_invite` row lives. NOT the event TTL — a human is
/// looking at the invite dialog, so give them two minutes before the reaper treats it as dead.
pub(crate) const INVITE_TTL_MICROS: i64 = 120 * 1_000_000; // 2min
