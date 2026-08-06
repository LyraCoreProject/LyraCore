//! SpacetimeDB module — **all** WoW 1.12 game state and logic.
//!
//! This is the thin index. The actual tables and reducers live in per-domain submodules — the
//! `mod` declarations below are the authoritative list of what exists (46 of them at last count,
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
///   `character_owned!(transfer, fn <name>(<ctx>, <guid>, <io>) { <body> })` — three params
///   (ctx, guid, a `&mut transfer::RowIo`); expands to `pub(crate) fn <name>(ctx:
///   &ReducerContext, guid: u64, io: &mut crate::transfer::RowIo)`. This is the CROSS-DATABASE
///   row transport (issue #19): one body serves BOTH directions through
///   `transfer::move_rows(ctx, io, export, insert)`, so a table's rows can actually leave one
///   SpacetimeDB database and arrive in another. Required for every `game_*` manifest table —
///   `transfer::every_manifest_table_can_cross_a_database_boundary` fails the build's test suite
///   if one is missing, because a table with no transport arm is silent data LOSS the first time
///   a character crosses a real shard boundary.
///
/// See `crate::reputation::sweep_delete_game_player_reputation` for a worked example.
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
    (transfer, fn $name:ident($ctx:ident, $guid:ident, $io:ident) $body:block) => {
        pub(crate) fn $name(
            $ctx: &spacetimedb::ReducerContext,
            $guid: u64,
            $io: &mut $crate::transfer::RowIo<'_>,
        ) $body
    };
}

/// Registers a periodic pass with the core scheduler tick pipeline. Same build.rs
/// text-scan mechanism as the `character_owned` marker: write the invocation next to the code it
/// drives (core src/ or a drop-in `packages/<name>/src/` file, if this checkout has one — the
/// packages contract, `packages/` itself, is not part of the public mirror) and the
/// generated `GAME_TICK_PASSES` registry picks it up with zero core-file edits.
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
/// `is_sense_tick` pattern in `creatures/tick.rs`).
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

// GENERATED by module/build.rs from the `game_tick_pass` / `game_hook` marker invocations.
// Defines `GAME_TICK_PASSES: &[(&str, fn(&ReducerContext))]` (name string kept for logging)
// and one `GAME_HOOKS_<EVENT>: &[fn(&ReducerContext, &Payload)]` per event in build.rs's
// HOOK_EVENTS catalog. Iterated by `hooks::run_package_tick_passes` / `hooks::fire_*`.
include!(concat!(env!("OUT_DIR"), "/package_registries.rs"));

mod action_bar;
mod actor;
mod auth;
mod bridge;
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
// entirely. Enable for the test build via `--build-options='--features=debug_reducers'`. See debug.rs.
#[cfg(feature = "debug_reducers")]
mod debug;
// Encounter kernel (work-item 228): instance-scoped encounter state, HP-threshold/death/GO-use
// hooks, and the shared choreography primitives packages consume (packages/<dungeon>/ own content).
mod encounter;
mod faction;
mod gameobject;
mod gc;
mod gm;
mod group;
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
pub mod nav;
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
mod seed;
mod skill;
mod skilldata;
mod spell;
mod stats;
mod talent;
pub mod terrain;
#[cfg(test)]
mod test_scan;
mod threat;
mod trainer;
mod transfer;
mod world;
mod xp;

pub use action_bar::*;
pub use auth::*;
pub use bridge::*;
pub use character::*;
pub use chat::*;
pub use combat::*;
pub use config::*;
pub use corpse::*;
pub use creatures::*;
#[cfg(feature = "debug_reducers")]
pub use debug::*;
pub use encounter::*;
pub use exploration::CharacterExplored; // re-exported for the gateway schema-parity test (282)
pub use faction::*;
pub use gameobject::*;
pub use gc::*;
pub use gm::*;
pub use group::*;
pub use instance::*;
pub use items::*;
pub use load::*;
pub use loot::*;
pub use quest::*;
pub use realm_core::*;
pub use region::*;
pub use reputation::*;
pub use skill::*;
pub use skilldata::*;
pub use spell::*;
pub use stats::*;
pub use talent::*;
pub use threat::*;
pub use trainer::*;
pub use transfer::TransferOut; // re-exported for the gateway schema-parity test (#19)
pub use world::*;
pub use xp::*;

/// How long a movement / creature-move event lives before the reaper GCs it. Events are delivered
/// to subscribers in real time on insert, so a short TTL is safe (see docs).
pub(crate) const EVENT_TTL_MICROS: i64 = 1_000_000; // 1s

/// How long a never-answered `game_group_invite` row lives. NOT the event TTL — a human is
/// looking at the invite dialog, so give them two minutes before the reaper treats it as dead.
pub(crate) const INVITE_TTL_MICROS: i64 = 120 * 1_000_000; // 2min

/// ENFORCEMENT tripwire: `build.rs`'s codegen only registers tables that remembered to
/// add a `character_owned` marker — it can't catch a table that forgot to. This test independently
/// source-scans `src/**/*.rs` for `#[table]` structs carrying a `character_guid: u64`, `player_guid:
/// u64`, or `owner_guid: u64` field (the three field-name conventions every existing character-owned
/// table uses) and fails if the SAME FILE doesn't also contain a `character_owned` marker invocation.
/// A handful of tables use one of those field names for something that is deliberately NOT durable
/// character-owned data (transient combat rows, or the character's own identity/corpse row handled
/// directly in `world::cascade_delete_character`); those are the explicit `EXEMPT_ACCESSORS` below —
/// adding a new exemption should be rare and each one needs its own justifying comment, mirroring how
/// `world.rs` already explains its 3 hardcoded (non-swept) deletes.
#[cfg(test)]
mod character_owned_tripwire {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Accessor names that carry a `character_guid`/`player_guid`/`owner_guid` field but are
    /// deliberately NOT swept via a `character_owned` marker:
    /// - `game_world_entity`: the live entity IS the character (deleted directly); its `owner_guid`
    ///   field is a transient PET-ownership pointer (which player owns this summoned creature), not a
    ///   row owned BY a character.
    /// - `game_corpse`: the character's own corpse row, deleted directly via the deterministic
    ///   `corpse_guid_for(character_guid)` lookup (not a `by_owner`-style scan).
    /// - `game_combo_point`: transient combat state (docs' "auras, combo points, lockouts, pending
    ///   casts are exempt" carve-out) — dies with the live session, not the durable character.
    const EXEMPT_ACCESSORS: &[&str] = &["game_world_entity", "game_corpse", "game_combo_point"];

    /// Every `.rs` file that compiles into this crate: core `src/` plus each drop-in
    /// `packages/*/src/` (build.rs compiles those in, so every source-scanning tripwire must see
    /// them too). Shared with `partition_discipline_tripwire` below.
    pub(super) fn scanned_files() -> Vec<PathBuf> {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let mut files = Vec::new();
        collect_rs_files(&Path::new(manifest_dir).join("src"), &mut files);
        let packages_dir = Path::new(manifest_dir)
            .parent()
            .expect("module/ has a parent (the repo root)")
            .join("packages");
        if packages_dir.is_dir() {
            for entry in fs::read_dir(&packages_dir).expect("readable packages/") {
                let pkg_src = entry.expect("readable dir entry").path().join("src");
                if pkg_src.is_dir() {
                    collect_rs_files(&pkg_src, &mut files);
                }
            }
        }
        files
    }

    pub(super) fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in
            fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read dir {}: {e}", dir.display()))
        {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                out.push(path);
            }
        }
    }

    /// Extract every `#[table(accessor = NAME` ... its struct's field block, as `(accessor, fields_text)`.
    /// A plain text scan (mirrors `build.rs`'s approach): find `#[table(`, pull the accessor name out of
    /// the attribute, then find the following `struct ... { ... }` and take everything up to the matching
    /// close brace (braces don't nest inside a plain field list here, so a naive depth counter is exact).
    fn extract_tables(content: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut search_from = 0usize;
        while let Some(rel) = content[search_from..].find("#[table(") {
            let attr_start = search_from + rel;
            // Skip a `#[table(...)]` mention inside a `//`/`///`/`//!` comment (this doc-comment text
            // itself, or `helpers.rs`'s explainer, describe the attribute without defining one) — only
            // a line that isn't a comment is a real attribute.
            let line_start = content[..attr_start]
                .rfind('\n')
                .map(|i| i + 1)
                .unwrap_or(0);
            if content[line_start..attr_start]
                .trim_start()
                .starts_with("//")
            {
                search_from = attr_start + "#[table(".len();
                continue;
            }
            let attr_rest = &content[attr_start..];
            let accessor = attr_rest
                .find("accessor")
                .and_then(|i| attr_rest[i..].find('=').map(|eq| i + eq + 1))
                .map(|name_start| {
                    let name_rest = attr_rest[name_start..].trim_start();
                    let end = name_rest
                        .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .unwrap_or(name_rest.len());
                    name_rest[..end].to_string()
                })
                .unwrap_or_else(|| {
                    panic!(
                        "malformed #[table(...)] (no accessor=NAME found) near byte {attr_start}"
                    )
                });
            let struct_kw = attr_rest.find("struct ").unwrap_or_else(|| {
                panic!("#[table(accessor = {accessor}, ...)] not followed by a struct")
            });
            let after_struct = &attr_rest[struct_kw..];
            let body_start = after_struct
                .find('{')
                .unwrap_or_else(|| panic!("struct for accessor {accessor} has no body"));
            let mut depth = 0i32;
            let mut body_end = None;
            for (i, c) in after_struct[body_start..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            body_end = Some(body_start + i);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let body_end = body_end
                .unwrap_or_else(|| panic!("unterminated struct body for accessor {accessor}"));
            let fields = after_struct[body_start..=body_end].to_string();
            out.push((accessor, fields));
            search_from = attr_start + "#[table(".len();
        }
        out
    }

    fn has_character_owned_marker(content: &str) -> bool {
        let marker = format!("{}{}", "character_owned", "!");
        content.contains(&format!("{marker}(delete"))
            || content.contains(&format!("{marker}(restamp"))
    }

    #[test]
    fn every_character_keyed_table_has_a_character_owned_marker() {
        let files = scanned_files();

        let mut missing = Vec::new();
        for file in &files {
            // `lib.rs` (this file) defines no `#[table]` struct of its own — it only ever MENTIONS the
            // attribute in prose/string literals (including this very scanner's own source), which
            // would otherwise misparse as malformed attributes. Every real table lives in a submodule.
            if file.file_name().and_then(|n| n.to_str()) == Some("lib.rs") {
                continue;
            }
            let content = fs::read_to_string(file)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
            let tables = extract_tables(&content);
            if tables.is_empty() {
                continue;
            }
            let marker_present = has_character_owned_marker(&content);
            for (accessor, fields) in &tables {
                let is_character_keyed = fields.contains("character_guid: u64")
                    || fields.contains("player_guid: u64")
                    || fields.contains("owner_guid: u64");
                if !is_character_keyed || EXEMPT_ACCESSORS.contains(&accessor.as_str()) {
                    continue;
                }
                if !marker_present {
                    missing.push(format!("{accessor} (in {})", file.display()));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "character-keyed table(s) with no `character_owned` sweep marker in their file: {missing:?}\n\
             Add a `crate::character_owned` `delete` marker invocation (and usually the `restamp` form \
             too, see the macro doc at the top of lib.rs) next to the table, OR add the accessor to \
             `EXEMPT_ACCESSORS` above with a comment justifying why it's intentionally unswept."
        );
    }
}

/// ENFORCEMENT tripwire for the marker scan itself: a commented-out marker invocation must
/// neither register nor break the build. The comment right inside this mod IS the fixture — it is
/// real, well-formed marker syntax that would have panicked a naive substring scan (no `fn`
/// body brace games, the genuine article). If comment-stripping ever regresses, one of two things
/// happens loudly: build.rs panics on the "malformed" comment text, or the ghost pass registers
/// and this test fails.
#[cfg(test)]
mod build_scan_strip_tripwire {
    // game_tick_pass!(fn ghost_commented_out_pass(ctx) { unreachable!() })
    // character_owned!(delete, fn ghost_commented_out_sweep(ctx, character_guid) { unreachable!() })

    #[test]
    fn commented_out_markers_do_not_register() {
        // Assert on the GENERATED TEXT, not the registry consts: the ghost fn never compiles
        // into anything, so there is no fn-pointer value to look for — and referencing the const
        // arrays would materialize every REAL registered fn's pointer, dragging SpacetimeDB host
        // imports into this native test binary (which cannot link them).
        let registries = include_str!(concat!(env!("OUT_DIR"), "/package_registries.rs"));
        assert!(
            !registries.contains("ghost_commented_out_pass"),
            "a commented-out game_tick_pass registered — build.rs comment-stripping regressed"
        );
        let sweeps = include_str!(concat!(env!("OUT_DIR"), "/character_sweeps.rs"));
        assert!(
            !sweeps.contains("ghost_commented_out_sweep"),
            "a commented-out character_owned marker registered — build.rs comment-stripping regressed"
        );
    }
}

/// ENFORCEMENT tripwire (issue #14, part of the elastic-world-sharding spec #12): a full
/// `.iter()` over a SPATIAL table reads the whole world, so it silently breaks shardability —
/// once the world is cut into region/continent/instance shards, a whole-table scan can only ever
/// see the rows on the caller's own shard, and the code that relied on seeing everything goes
/// quietly wrong. Every spatial read must instead go through the partition-scoped helper family
/// (`helpers::entities_near` + `helpers::in_same_partition`), which is `(map_id, instance_id)`-
/// scoped and grid-indexed and therefore keeps meaning the same thing after a shard split.
///
/// This test source-scans `src/**/*.rs` + `packages/*/src/**/*.rs` (same file set as
/// `character_owned_tripwire`) for raw scans and fails on any file over its whitelisted budget.
/// The whitelist FREEZES today's legitimate scans; it is a ratchet, so it should only ever shrink
/// as perf-catalog Tier 1 work replaces scans with indexed helpers.
///
// Deliberate simplification: whitelist granularity is (file, count), not (file, line) — line
// numbers churn on every unrelated edit above them and would make this test a merge-conflict
// generator. The ceiling: swapping one whitelisted scan for a different one inside an
// already-whitelisted file slips through. Upgrade path if that ever bites: key on the enclosing
// `fn` name instead.
//
// Deliberate simplification: "raw" is a whole-table read (`.iter()` or `.count()`) reached either
// straight off the accessor OR off a local handle bound from it one line earlier — `let entities =
// ctx.db.game_world_entity();` is this codebase's DOMINANT idiom (~120 bindings vs ~18 inline
// calls), so a scanner that only matched the inline form would wave through most of the real
// scans and would fail exactly nobody who copied the style of the code next to them. Index reads
// (`.by_grid().filter(..)`, `.guid().find(..)`) are already partition-safe or point reads.
// Ceiling: still a text scan — a handle passed to another fn, or an accessor name pasted together
// by a macro, defeats it. Upgrade path: none planned — the tripwire's job is to make the lazy
// paths fail, not to be a sound analysis.
#[cfg(test)]
mod partition_discipline_tripwire {
    /// Tables whose rows have a world position, i.e. the ones a shard split partitions.
    /// Non-spatial tables (accounts, items, quests, templates) are realm-wide by construction and
    /// are none of this test's business.
    const SPATIAL_ACCESSORS: &[&str] = &[
        "game_world_entity",
        "game_creature_spawn",
        "game_gameobject",
        "game_dynamic_object",
    ];

    /// `(repo-relative path, allowed raw-scan count, why)`. One line of justification each.
    const WHITELIST: &[(&str, usize, &str)] = &[
        // Diagnostics and harness code — never on a gameplay path.
        ("module/src/debug.rs", 12, "`debug_reducers`-gated test harness; compiled out of production builds entirely"),
        // Importers — realm-wide by definition: they wipe and rebuild every partition at once.
        ("module/src/creatures/spawn.rs", 4, "`import_creature_spawns` drops every creature entity + spawn row before reloading the world; +1 for `debug_normalize_spawn_timers`, a one-shot operator migration that must visit this database's whole spawn table by definition; +1 for `debug_retire_region_creatures` (#194), which manages the partitioning itself — it scans spawn HOMES to hand a region's population to its owning shard"),
        ("module/src/gameobject.rs", 2, "`import_gameobjects` drops every gameobject row before reloading the world; plus one by-`template_entry` lookup (an entry-keyed find, not a spatial query — no index on that column)"),
        // Deliberately cross-partition: these ARE the code that manages partitions.
        ("module/src/instance.rs", 8, "instance population spawn/teardown + the reaper's occupied-instance census — classifying every partition at once is the job"),
        ("module/src/encounter.rs", 2, "guid high-water marks over spawn/gameobject TEMPLATE rows when allocating a wave guid, not live-world reads"),
        // Unindexed lookups whose target is co-located with the caller anyway.
        ("module/src/creatures/pet.rs", 2, "pet-by-owner_guid has no index; a pet is always co-located with its owner. The SECOND is `nearest_hostile_near` — KNOWN DEBT, a textbook `entities_near` radius search kept only because pets are rare (one per online warlock)"),
        ("packages/deadmines/src/choreography.rs", 1, "single instance-scoped boss-liveness check inside one encounter"),
        // KNOWN DEBT — replace with `entities_near` / a grid-index read, do not copy.
        ("module/src/creatures/tick.rs", 5, "per-tick world sweeps (sense-tick player list) plus 2 throttled rows-visited counters — perf-catalog Tier 1 took this from 10 to 7 (1.6/1.7/1.10), and the `timer_never` sentinel took corpse-decay and respawn-due off the list entirely (7 -> 5): both now range-scan `by_despawn_at`/`by_respawn_at` instead of the whole spawn table"),
        ("module/src/spell/cast.rs", 1, "AoE/chain target resolution by full-iter + squared distance; a textbook `helpers::entities_near` call (perf-catalog Tier 1)"),
        ("module/src/skill.rs", 2, "nearest-trainer resolution; a plain radius search that should move to `helpers::entities_near` (perf-catalog Tier 1)"),
    ];

    /// `byte_idx` sits on a `//` line, i.e. it is prose rather than code (same rule
    /// `character_owned_tripwire` uses for its `#[table(` scan).
    pub(super) fn on_comment_line(content: &str, byte_idx: usize) -> bool {
        let line_start = content[..byte_idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
        content[line_start..byte_idx].trim_start().starts_with("//")
    }

    pub(super) fn line_of(content: &str, byte_idx: usize) -> usize {
        content[..byte_idx].matches('\n').count() + 1
    }

    /// The text at `byte_idx` opens a whole-table read: `.iter()`, or `.count()` (the same read
    /// with the rows thrown away — `Table::count` is no more partition-scoped than `iter`).
    /// Whitespace is collapsed so a rustfmt-wrapped method chain still matches.
    fn opens_whole_table_read(content: &str, byte_idx: usize) -> bool {
        let tail: String = content[byte_idx..]
            .chars()
            .take(64)
            .filter(|c| !c.is_whitespace())
            .collect();
        tail.starts_with(".iter()") || tail.starts_with(".count()")
    }

    /// `name` at `byte_idx` is a standalone identifier, not a substring of a longer one and not a
    /// field access (`self.entities` is somebody else's business).
    pub(super) fn is_standalone_ident(content: &str, byte_idx: usize, name: &str) -> bool {
        let before_ok = content[..byte_idx]
            .chars()
            .next_back()
            .map(|c| !(c.is_alphanumeric() || c == '_' || c == '.'))
            .unwrap_or(true);
        let after_ok = content[byte_idx + name.len()..]
            .chars()
            .next()
            .map(|c| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(true);
        before_ok && after_ok
    }

    /// Every `(line, accessor)` where a spatial table is read whole — either straight off the
    /// accessor (`ctx.db.game_world_entity().iter()`) or off a local handle bound from it
    /// (`let entities = ctx.db.game_world_entity();` ... `entities.iter()`). Both forms count:
    /// the binding form is the one this codebase actually writes most of the time.
    fn raw_scans(content: &str) -> Vec<(usize, &'static str)> {
        let mut out = Vec::new();
        // `(local handle name, accessor it was bound from)`, deduped by name so a file that binds
        // `entities` a dozen times doesn't count each downstream `entities.iter()` a dozen times.
        let mut handles: Vec<(String, &'static str)> = Vec::new();
        for accessor in SPATIAL_ACCESSORS {
            let call = format!("{accessor}()");
            for (idx, _) in content.match_indices(&call) {
                if on_comment_line(content, idx) {
                    continue;
                }
                if opens_whole_table_read(content, idx + call.len()) {
                    out.push((line_of(content, idx), *accessor));
                }
            }
            let bind = format!("ctx.db.{accessor}();");
            for (idx, _) in content.match_indices(&bind) {
                if on_comment_line(content, idx) {
                    continue;
                }
                // Walk back over `let [mut] NAME =` on the same line to name the handle.
                let line_start = content[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let Some(head) = content[line_start..idx].trim_end().strip_suffix('=') else {
                    continue;
                };
                let mut words = head.split_whitespace();
                if words.next() != Some("let") {
                    continue;
                }
                let Some(name) = words.next_back() else {
                    continue;
                };
                if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                if !handles.iter().any(|(n, _)| n == name) {
                    handles.push((name.to_string(), *accessor));
                }
            }
        }
        for (name, accessor) in &handles {
            for (idx, _) in content.match_indices(name.as_str()) {
                if on_comment_line(content, idx) || !is_standalone_ident(content, idx, name) {
                    continue;
                }
                if opens_whole_table_read(content, idx + name.len()) {
                    out.push((line_of(content, idx), *accessor));
                }
            }
        }
        out.sort();
        out
    }

    pub(super) fn repo_root() -> std::path::PathBuf {
        crate::test_scan::repo_root()
    }

    #[test]
    fn no_unwhitelisted_raw_spatial_scans() {
        let root = repo_root();
        let mut violations = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            // lib.rs (this file) holds the whitelist and this scanner's own prose; it defines no
            // reducers, so skipping it costs nothing — same carve-out the sibling tripwire makes.
            if file.file_name().and_then(|n| n.to_str()) == Some("lib.rs") {
                continue;
            }
            let rel = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let content =
                std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
            let found = raw_scans(&content);
            let allowed = WHITELIST
                .iter()
                .find(|(p, _, _)| *p == rel)
                .map(|(_, n, _)| *n)
                .unwrap_or(0);
            if found.len() > allowed {
                for (line, accessor) in found {
                    violations.push(format!("{rel}:{line} scans {accessor} (budget {allowed})"));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "raw spatial-table scan(s) outside the partition-scoped helpers:\n  {}\n\n\
             A whole-table `.iter()` over a spatial table is not shardable (issue #14 / spec #12): \
             after a region or continent split it silently sees only the caller's own shard. Use \
             `crate::helpers::entities_near(ctx, map_id, instance_id, x, y, radius)` for a radius \
             search (grid-indexed, `(map_id, instance_id)`-scoped) and \
             `crate::helpers::in_same_partition(&e, map_id, instance_id)` to test partition \
             membership. If the scan is genuinely realm-wide or partition-managing, raise that \
             file's budget in `WHITELIST` (module/src/lib.rs) WITH a one-line justification. \
             Whitelisted today: {} scan(s) across {} file(s).",
            violations.join("\n  "),
            WHITELIST.iter().map(|(_, n, _)| n).sum::<usize>(),
            WHITELIST.len(),
        );
    }

    /// The whitelist is a ratchet: an entry whose file no longer needs its full budget (or no
    /// longer exists) must be trimmed, or the budget quietly re-opens the door it was closed for.
    ///
    /// An entry naming an OPTIONAL drop-in package that isn't installed is skipped, not counted as
    /// zero — `crate::test_scan::is_installed` draws that line at the package DIRECTORY, so a file
    /// missing from an INSTALLED package still reports `actual 0` and fails here.
    #[test]
    fn whitelist_has_no_stale_entries() {
        let root = repo_root();
        let mut stale = Vec::new();
        for (rel, allowed, _) in WHITELIST {
            if !crate::test_scan::is_installed(rel) {
                continue;
            }
            let actual = std::fs::read_to_string(root.join(rel))
                .map(|c| raw_scans(&c).len())
                .unwrap_or(0);
            if actual < *allowed {
                stale.push(format!("{rel}: budget {allowed}, actual {actual}"));
            }
        }
        assert!(
            stale.is_empty(),
            "WHITELIST budget(s) larger than the scans that actually exist — lower them:\n  {}",
            stale.join("\n  ")
        );
    }

    // ==========================================================================================
    //  #23 EXTENSION: no module game logic may read a SHARD ID.
    // ==========================================================================================

    /// The only files allowed to touch a shard column: the one that DEFINES the assignment table
    /// and the operator reducer that writes it (`region.rs`), and the one that DEFINES the
    /// per-shard load-sample table and ITS operator reducer (`load.rs`, issue #78) — `game_shard_load
    /// .shard` is a database-name LABEL an ops sample is filed under, ring-evicted and compared for
    /// equality, exactly like `RegionAssignment.shard`. Both write or compare; neither branches on
    /// the value to decide anything gameplay-visible.
    const SHARD_ID_OWNERS: &[&str] = &["module/src/region.rs", "module/src/load.rs"];

    /// The ways module code could reach a shard id: the assignment table's accessor, the `.shard`
    /// column itself (`row.shard`), and the ROW TYPE — because `let RegionAssignment { shard, .. }
    /// = row;` and `match row { RegionAssignment { shard: db, .. } => … }` bind the column without
    /// ever writing a dot (both forms were confirmed to slip past the first two). You cannot
    /// destructure a type you may not name. `game_character_shard` / `CharacterShard` deliberately
    /// do NOT count — that table stores a `(map_id, instance_id)` LOCATION precisely so nothing has
    /// to name a database. `game_shard_load` / `ShardLoad` (issue #78) get the same destructuring
    /// protection as `RegionAssignment` — they carry a shard-name LABEL an ops sample is filed
    /// under, not a location, but the same "cannot destructure a type you may not name" argument
    /// applies to it too.
    const SHARD_ID_FORMS: &[&str] = &[
        "game_region_assignment",
        ".shard",
        "RegionAssignment",
        "game_shard_load",
        "ShardLoad",
    ];

    /// Every `(line, form)` where a file names a shard id. `.shard` must be a whole field access,
    /// so `.shard_name`/`.shard_index` (gateway vocabulary, if it ever appears in prose) and
    /// `game_character_shard` (no dot before `shard`) are not hits.
    fn shard_id_reads(content: &str) -> Vec<(usize, &'static str)> {
        let mut out = Vec::new();
        for form in SHARD_ID_FORMS {
            for (idx, _) in content.match_indices(form) {
                if on_comment_line(content, idx) {
                    continue;
                }
                let after_ok = content[idx + form.len()..]
                    .chars()
                    .next()
                    .map(|c| !(c.is_alphanumeric() || c == '_'))
                    .unwrap_or(true);
                if after_ok {
                    out.push((line_of(content, idx), *form));
                }
            }
        }
        out.sort();
        out
    }

    /// AC#1 of #23: **region definitions are data; shard ids are the gateway's business.** The
    /// module stores `region_assignment { map_id, region_id, shard, epoch }` (it has to — realm-core
    /// is this same wasm under another database name), but the instant a reducer *branches* on
    /// `shard` the module stops being relocatable: the same code on two databases would take two
    /// different paths, and a region migration would change gameplay instead of changing routing.
    /// Regions and cells are fair game anywhere; the database name is not.
    /// The module's own `src` + `packages/*` (what the sibling tripwires scan) **plus
    /// `crates/lyracore-shared/src`**, which is compiled INTO this module and therefore just as capable
    /// of naming a database.
    ///
    /// Issue #48 moved `DUNGEON_MAPS` out of `module/src/instance.rs` into `lyracore_shared::instance` so
    /// the gateway and the module could not disagree about which maps are dungeons. A map-id set is a
    /// world fact and entirely fine here — but the move quietly put a piece of module logic in a crate
    /// this tripwire did not look at, so "no module game logic reads a shard id" would have stopped
    /// being enforceable for anything that followed it there. It is scanned now. (Only THIS tripwire
    /// is extended: the spatial-scan and character-owned tripwires are about `#[table]` definitions
    /// and spatial iteration, neither of which exists in a `no_std`-shaped shared crate.)
    fn shard_id_scanned_files() -> Vec<std::path::PathBuf> {
        let mut files = super::character_owned_tripwire::scanned_files();
        let shared = repo_root().join("crates/lyracore-shared/src");
        if shared.is_dir() {
            super::character_owned_tripwire::collect_rs_files(&shared, &mut files);
        }
        files
    }

    #[test]
    fn no_module_game_logic_reads_a_shard_id() {
        let root = repo_root();
        let mut violations = Vec::new();
        for file in shard_id_scanned_files() {
            if file.file_name().and_then(|n| n.to_str()) == Some("lib.rs") {
                continue; // this file holds the scanner's own vocabulary (see the sibling tests)
            }
            let rel = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            if SHARD_ID_OWNERS.contains(&rel.as_str()) {
                continue;
            }
            let content =
                std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
            for (line, form) in shard_id_reads(&content) {
                violations.push(format!("{rel}:{line} names a shard id (`{form}`)"));
            }
        }
        assert!(
            violations.is_empty(),
            "module code outside {SHARD_ID_OWNERS:?} reads a SHARD ID:\n  {}\n\n\
             A shard is a database name — a GATEWAY routing fact (issue #23 / spec #12). Module \
             game logic may read cells (`lyracore_shared::spatial`) and regions \
             (`lyracore_shared::region::RegionMap`), and may partition by `(map_id, instance_id)` via \
             `crate::helpers`, but must never branch on which database owns a region: that is what \
             lets the same wasm run on every shard and lets a region move between shards without a \
             code change. Route in `gateway/src/config.rs` instead.",
            violations.join("\n  ")
        );
    }

    /// The owner list is a ratchet too: a file that no longer touches a shard id must come off it,
    /// or it keeps a hole open for the next person who copies the code next to them.
    #[test]
    fn shard_id_owner_list_has_no_stale_entries() {
        let root = repo_root();
        let stale: Vec<&str> = SHARD_ID_OWNERS
            .iter()
            .copied()
            .filter(|rel| {
                std::fs::read_to_string(root.join(rel)).is_ok_and(|c| shard_id_reads(&c).is_empty())
            })
            .collect();
        assert!(
            stale.is_empty(),
            "SHARD_ID_OWNERS names file(s) that no longer touch a shard id — remove them: {stale:?}"
        );
    }
}

/// ENFORCEMENT tripwire (issue #30, part of the elastic-world-sharding spec #12): a reducer that
/// reaches a character by GUID or by NAME straight into `game_character` touches none of the
/// escrowed-transfer chokepoints — not `helpers::entity_by_owner` (the actor side), not
/// `world::player_login` (re-materialisation), not `begin_transfer`'s delete of the live entity
/// (the target side). Same-database that is harmless: the write lands on the SAME row the
/// destination reads. Cross-database (#19) each one is a LOST WRITE, because the export blob was
/// serialized at `begin_transfer` and the mutation dies with the source copy.
///
/// The fence is `helpers::character_by_guid` / `character_by_name`, which read an in-transit
/// character as ABSENT so each caller's existing "no such character" arm fires. This test
/// source-scans the same file set as its two sibling tripwires for RAW character lookups and fails
/// on any file over its whitelisted budget. The whitelist FREEZES today's audited exceptions with
/// a verdict each (see the table in `transfer.rs`'s module doc); it is a ratchet, so it should only
/// ever shrink.
///
// Note: same (file, count) granularity, same text-scan mechanism, and the same two ceilings as
// `partition_discipline_tripwire` — swapping one whitelisted lookup for a different one inside an
// already-whitelisted file slips through, and a handle passed to another fn defeats the scan
// entirely. Upgrade path if it bites: key on the enclosing `fn` name. What it DOES catch is the
// case this ticket exists for: somebody adds a fresh `ctx.db.game_character().guid().find(..)` in
// a file that had none, copying the style of the code next to them, and CI turns red.
#[cfg(test)]
mod character_fence_tripwire {
    use super::partition_discipline_tripwire::{
        is_standalone_ident, line_of, on_comment_line, repo_root,
    };

    /// `(repo-relative path, allowed raw-lookup count, verdict + why)`. One line each; every entry
    /// is an audited exception from the by-guid verdict table in `module/src/transfer.rs`.
    const WHITELIST: &[(&str, usize, &str)] = &[
        // THE GATE ITSELF.
        ("module/src/helpers.rs", 2, "`character_by_guid` + `character_by_name` — the fence; these two ARE the raw lookups everything else routes through"),
        // TRANSFER MACHINERY — must read the row the fence hides, or it could not move it.
        ("module/src/transfer.rs", 8, "begin_transfer (2: the plan probe + the post-persist re-read), import_character, in_transit_instances, and CtxImportSink::has_character (the blob import's ONE character probe, used for both the materialise upsert and the post-import durability PROOF) all operate ON the escrowed character; a fenced reader would return None every time — the blob import's second call is precisely the check that the destination copy exists before an in-row licenses deleting the source. The 6th is NOT a lookup: it is the expected-shape STRING LITERAL inside `tests::the_production_adapters_are_the_pass_throughs_the_harness_assumes`, which quotes CtxImportSink's body verbatim so any edit to it fails — this scanner reads text and is right to count it. The 7th and 8th (issue #64) are the same kind of exact-shape STRING LITERAL, inside `tests::the_by_guid_chokepoint_is_exactly_the_pinned_shape` — one each for `character_by_guid`'s and `character_by_name`'s `want` strings, which quote `helpers.rs`'s bodies verbatim so a mutation anywhere in them (not just a `!` glued onto `is_in_transit`) fails"),
        ("module/src/world.rs", 8, "player_login carries its own `is_in_transit` fence (the re-materialisation chokepoint, 2 reads); teleport_player, set_home, persist_entity and is_gm_character take a guid already resolved through a fenced entity (persist_entity is called BY begin_transfer); debug_delete_character keeps a raw existence probe so a missing character stays a no-op while an in-transit one is REFUSED, with the fence on the same expression; cascade_delete_character's raw row DELETE is the sweep that probe guards. `recall_to_home` is now FENCED (character_by_guid) — it was the one teleport_player caller needing no live entity"),
        // OPEN, not decided — spec #12 puts group MEMBERSHIP state on realm-core, settled by #22.
        ("module/src/group.rs", 6, "OPEN: group_accept/group_uninvite/group_leave have a THIRD party mutate game_group_member; if #22 lands membership on realm-core these stop being a world shard's concern entirely. Guessing a verdict here is the mistake #30 exists to correct"),
        // REGENERATE at the destination — connection-derived state, never carried in the blob.
        ("module/src/auth.rs", 6, "REGENERATE: establish_session rebinds owner_identity from the LIVE connection; create_character's name/cap/guid-allocation checks predate any character; delete_character keeps a raw find so NO_SUCH_CHAR/NOT_OWNER/CHAR_IN_TRANSIT stay three answers, with the fence on the next line"),
        // READS, not writes: name/class/race/identity lookups that mutate nothing on the character.
        ("module/src/chat.rs", 2, "add_contact's target-existence check writes only the CALLER's own contact row; #22's `push_whisper` reads a recipient's owner_identity to ADDRESS a whisper event row (the bridge.rs/reputation.rs verdict — no write to the character), and its ZERO fallback is the intended answer on realm-core, where no character row exists at all"),
        ("module/src/items/ops.rs", 2, "race/class reads for the starter loadout and the mana-class gate — no write to the character"),
        ("module/src/spell/cast.rs", 1, "caster NAME for the resurrect prompt — no write"),
        ("module/src/reputation.rs", 1, "owner_identity fallback for RLS visibility of a new rep row — no write to the character"),
        ("module/src/bridge.rs", 1, "recipient owner_identity for an addon-message row — no write to the character"),
        ("module/src/stats.rs", 2, "set_character_level reads (race, class) and writes level/xp for a guid the caller already resolved through a fenced entity (gm_command's entity_by_owner) or through the FENCED `debug::debug_set_level`; the core itself also serves guids with no character row, so the gate belongs at its callers"),
        ("module/src/xp.rs", 1, "rested-pool drain keyed on the ATTACKER's guid, which came from a live entity (kill credit)"),
        ("module/src/rest.rs", 2, "the rest-state flip takes a live `mover` entity; the 30s accrual pass is BACKGROUND (no entity anywhere in it) and so carries its OWN `is_in_transit` fence — begin_transfer persists with set_offline:false, which is the branch that would have stopped the rest clock, so an escrowed character keeps resting==true and stays in the scan"),
        ("module/src/talent.rs", 1, "the guid comes from the caller's own entity_by_owner-resolved entity"),
        ("module/src/gm.rs", 2, "set_gm_level is FENCED (character_by_name); the two raw reads are gm_command's own gm_level probe and its .money write, both on the caller's entity_by_owner-resolved guid"),
        ("module/src/loot.rs", 1, "DEFER: credit_purse writes the durable row AFTER folding the delta into the escrowed blob (`transfer::defer_money_delta`) — refusing would drop a third party's copper"),
        ("module/src/debug.rs", 1, "`debug_seed_gm_tester` (guid 1, a dev-DB gm_level backfill); every debug WRITER that touches character state is fenced"),
        ("packages/playerbots/src/goals.rs", 1, "a bot reads a nearby player's NAME for goal selection — no write"),
        ("packages/playerbots/src/mod.rs", 4, "bot roster bookkeeping over rows this same reducer just created — free-name probe, post-create fetch, post-update re-read; +1 for `ensure_bot_account`'s per-account character COUNT, which reaches no character (it decides which bot account still has room under the 10-character cap) — an in-transit bot counting or not counting toward that cap is harmless either way"),
    ];

    /// The lookup forms that reach a character row raw. `.guid().find(` and `.name().find(` are the
    /// indexed point reads; `.iter()` is the case-folding name scan (`send_whisper`'s convention)
    /// and the account-scoped counts. `.guid().update(` is deliberately NOT here: the row it writes
    /// was already resolved by one of these, so the READ is the chokepoint.
    ///
    /// `.guid().delete(` IS here, and is the exception that proves the "the READ is the chokepoint"
    /// rule: a raw delete resolves nothing first, so no read gates it. It was added after a review
    /// mutant — a bare `ctx.db.game_character().guid().delete(&guid)` dropped into a ZERO-budget
    /// file — left the suite green, which is the annihilation case (`delete_character`'s verdict)
    /// walking straight past its own ratchet.
    const LOOKUP_FORMS: &[&str] = &[
        ".guid().find(",
        ".name().find(",
        ".guid().delete(",
        ".iter()",
    ];

    fn opens_raw_lookup(content: &str, byte_idx: usize) -> bool {
        let tail: String = content[byte_idx..]
            .chars()
            .take(64)
            .filter(|c| !c.is_whitespace())
            .collect();
        LOOKUP_FORMS.iter().any(|f| tail.starts_with(f))
    }

    /// Every line where `game_character` is read raw — inline off the accessor, or off a local
    /// handle bound from it (`let chars = ctx.db.game_character();`, this codebase's dominant idiom).
    fn raw_lookups(content: &str) -> Vec<usize> {
        const ACCESSOR: &str = "game_character";
        let mut out = Vec::new();
        let call = format!("{ACCESSOR}()");
        for (idx, _) in content.match_indices(&call) {
            if on_comment_line(content, idx) {
                continue;
            }
            if opens_raw_lookup(content, idx + call.len()) {
                out.push(line_of(content, idx));
            }
        }
        // Bound handles, deduped by name (a file that binds `chars` a dozen times counts each
        // downstream read once, not once per binding).
        let mut handles: Vec<String> = Vec::new();
        let bind = format!("ctx.db.{ACCESSOR}();");
        for (idx, _) in content.match_indices(&bind) {
            if on_comment_line(content, idx) {
                continue;
            }
            let line_start = content[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
            let Some(head) = content[line_start..idx].trim_end().strip_suffix('=') else {
                continue;
            };
            let mut words = head.split_whitespace();
            if words.next() != Some("let") {
                continue;
            }
            let Some(name) = words.next_back() else {
                continue;
            };
            if !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            if !handles.iter().any(|n| n == name) {
                handles.push(name.to_string());
            }
        }
        for name in &handles {
            for (idx, _) in content.match_indices(name.as_str()) {
                if on_comment_line(content, idx) || !is_standalone_ident(content, idx, name) {
                    continue;
                }
                if opens_raw_lookup(content, idx + name.len()) {
                    out.push(line_of(content, idx));
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    #[test]
    fn no_unwhitelisted_raw_character_lookups() {
        let root = repo_root();
        let mut violations = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            // lib.rs holds this scanner's own prose and defines no reducers — same carve-out both
            // sibling tripwires make.
            if file.file_name().and_then(|n| n.to_str()) == Some("lib.rs") {
                continue;
            }
            let rel = file
                .strip_prefix(&root)
                .unwrap_or(&file)
                .to_string_lossy()
                .replace('\\', "/");
            let content =
                std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
            let found = raw_lookups(&content);
            let allowed = WHITELIST
                .iter()
                .find(|(p, _, _)| *p == rel)
                .map(|(_, n, _)| *n)
                .unwrap_or(0);
            if found.len() > allowed {
                violations.push(format!(
                    "{rel}: {} raw game_character lookup(s) (budget {allowed}) at line(s) {found:?}",
                    found.len()
                ));
            }
        }

        assert!(
            violations.is_empty(),
            "raw `game_character` lookup(s) outside the by-guid chokepoint:\n  {}\n\n\
             A reducer that reaches a character by guid or by name passes NONE of the escrowed-\
             transfer chokepoints (issue #30 / spec #12), so cross-database it can write to a source \
             copy the destination already serialized past — a lost write. Route the READ through \
             `crate::helpers::character_by_guid(ctx, guid)` or \
             `crate::helpers::character_by_name(ctx, name)`; both read an in-transit character as \
             absent, so your existing not-found arm fires and no new error string reaches the \
             gateway. If refusal is the WRONG answer for your path (it would drop a third party's \
             value, or the field is connection-derived and regenerated at the destination), pick the \
             DEFER or REGENERATE verdict instead and add the site to `WHITELIST` (module/src/lib.rs) \
             WITH its verdict — see the table in `module/src/transfer.rs`'s module doc. \
             Whitelisted today: {} lookup(s) across {} file(s).",
            violations.join("\n  "),
            WHITELIST.iter().map(|(_, n, _)| n).sum::<usize>(),
            WHITELIST.len(),
        );
    }

    /// Ratchet: an entry whose file no longer needs its full budget must be trimmed, or the budget
    /// quietly re-opens the door it was closed for.
    ///
    /// Same optional-drop-in rule as its sibling: an entry under a `packages/<name>/` this checkout
    /// does not have is skipped; one under an INSTALLED package that has lost the file still counts
    /// zero and fails.
    #[test]
    fn character_whitelist_has_no_stale_entries() {
        let root = repo_root();
        let mut stale = Vec::new();
        for (rel, allowed, _) in WHITELIST {
            if !crate::test_scan::is_installed(rel) {
                continue;
            }
            let actual = std::fs::read_to_string(root.join(rel))
                .map(|c| raw_lookups(&c).len())
                .unwrap_or(0);
            if actual < *allowed {
                stale.push(format!("{rel}: budget {allowed}, actual {actual}"));
            }
        }
        assert!(
            stale.is_empty(),
            "WHITELIST budget(s) larger than the lookups that actually exist — lower them:\n  {}",
            stale.join("\n  ")
        );
    }
}
