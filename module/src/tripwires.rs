//! Source-scan tripwires (issue #379): `lib.rs` used to open with "this is the thin index" while
//! carrying 900+ lines of `#[cfg(test)]` scan machinery below its real index — pulled out here so
//! that doc comment stays true. `#[cfg(test)] mod tripwires;` is `lib.rs`'s only mention of this
//! file; every mod below is unchanged in what it enforces, only in where it lives.
//!
//! Five tripwires, in file order:
//! - [`character_owned_tripwire`] — every character-keyed table has a `character_owned` sweep marker.
//! - [`build_scan_strip_tripwire`] — a commented-out marker invocation never registers.
//! - [`partition_discipline_tripwire`] — no raw whole-table scan of a spatial table outside the
//!   partition-scoped helpers, plus its #23 extension (no module game logic reads a shard id).
//! - [`character_fence_tripwire`] — no raw `game_character` lookup outside the by-guid chokepoint.
//! - [`print_macro_tripwire`] — no print-family macro anywhere under `module/src` (new with #432).
//! - [`gc_reap_tripwire`] — every TTL-shaped event table is named in `gc.rs` (new with #379).
//!
//! `partition_discipline_tripwire::raw_scans` and `character_fence_tripwire::raw_lookups` used to
//! be ~70-line near-clones — identical bound-handle walk-back, handle dedup, comment-line filtering
//! — that had already drifted from each other in small ways neither review caught. The exact drift
//! pattern issue #64 consolidated once already, one file down (`body_of`/`code_of` into
//! `test_scan.rs`). Both now defer to the one engine there, [`crate::test_scan::raw_table_reads`]:
//! they differ only in which accessor(s) they watch and what counts as "opens a read" (`.iter()` /
//! `.count()` for a whole-table scan vs `.guid().find(` for a raw point lookup), so the two can no
//! longer disagree about what a table read is.

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
pub(crate) mod character_owned_tripwire {
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
    /// them too). Shared with `partition_discipline_tripwire` below and `gc_reap_tripwire`.
    ///
    /// `pub(crate)`, not `pub(super)`: `creatures::tick`'s
    /// `nothing_writes_the_unsubscribed_move_event_table` (#357) walks the whole compiled tree the
    /// same way, from outside this module entirely.
    pub(crate) fn scanned_files() -> Vec<PathBuf> {
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

    pub(crate) fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
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
    ///
    /// `pub(super)`: [`gc_reap_tripwire`] (a sibling in this same file) reuses this rather than
    /// re-walking every `#[table]` struct a third time.
    pub(super) fn extract_tables(content: &str) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut search_from = 0usize;
        while let Some(rel) = content[search_from..].find("#[table(") {
            let attr_start = search_from + rel;
            // Skip a `#[table(...)]` mention inside a `//`/`///`/`//!` comment (this doc-comment text
            // itself, or `helpers.rs`'s explainer, describe the attribute without defining one) — only
            // a line that isn't a comment is a real attribute.
            if crate::test_scan::on_comment_line(content, attr_start) {
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
            // `tripwires.rs` (this file) defines no `#[table]` struct of its own — it only ever
            // MENTIONS the attribute in prose/string literals (including this very scanner's own
            // source, e.g. `extract_tables`'s `.find("#[table(")`), which would otherwise misparse
            // as malformed attributes. Every real table lives in a submodule.
            if file.file_name().and_then(|n| n.to_str()) == Some("tripwires.rs") {
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
    use crate::test_scan::{line_of, on_comment_line, repo_root};

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
        // Diagnostics and harness code — never on a gameplay path. #386 split the former single
        // `debug.rs` (budget 9, after #368 dropped it from 12) into a directory; the 9 raw scans
        // landed in two of the seven files — same total, just split along the new file boundary.
        ("module/src/debug/mod.rs", 9, "`debug_reducers`-gated test harness; compiled out of production builds entirely (2 of these are #456's `debug_backfill_cell_ids`, a deliberately whole-shard migration sweep — each shard runs it once after the publish that adds the `cell` column)"),
        ("module/src/debug/instance.rs", 4, "`debug_reducers`-gated test harness; compiled out of production builds entirely; +2 for #526's `debug_assert_floor_snap`, which scans game_creature_spawn/game_world_entity by `entry` for a guid high-water mark (template-keyed, not a spatial query) before minting a scratch spawn guid"),
        // Importers — realm-wide by definition: they wipe and rebuild every partition at once.
        ("module/src/creatures/spawn.rs", 4, "`import_creature_spawns` drops every creature entity + spawn row before reloading the world; +1 for `debug_normalize_spawn_timers`, a one-shot operator migration that must visit this database's whole spawn table by definition; +1 for `debug_retire_region_creatures` (#194), which manages the partitioning itself — it scans spawn HOMES to hand a region's population to its owning shard"),
        ("module/src/gameobject.rs", 2, "`import_gameobjects` drops every gameobject row before reloading the world; plus one by-`template_entry` lookup (an entry-keyed find, not a spatial query — no index on that column)"),
        // Deliberately cross-partition: these ARE the code that manages partitions.
        ("module/src/instance.rs", 8, "instance population spawn/teardown + the reaper's occupied-instance census — classifying every partition at once is the job"),
        ("module/src/encounter.rs", 2, "guid high-water marks over spawn/gameobject TEMPLATE rows when allocating a wave guid, not live-world reads"),
        // Unindexed lookups whose target is co-located with the caller anyway.
        ("module/src/creatures/pet.rs", 2, "pet-by-owner_guid has no index; a pet is always co-located with its owner. The SECOND is `nearest_hostile_near` — KNOWN DEBT, a textbook `entities_near` radius search kept only because pets are rare (one per online warlock)"),
        ("packages/deadmines/src/choreography.rs", 1, "single instance-scoped boss-liveness check inside one encounter"),
        // Issue #383 split tick.rs into tick/{mod,movement,lifecycle,sense}.rs; this budget-5 entry
        // splits with it, same total (3 + 2 = 5), same reasoning as the pre-split note below.
        ("module/src/creatures/tick/mod.rs", 3, "per-tick world sweep that locates every player (`active_cell_creatures`, feeding the pets/in_combat/active-cell sets) plus the 2 throttled work-item 230/233 rows-visited counters (`log_active_cell_stats`/`log_narrowed_pass_stats`) — perf-catalog Tier 1 took the OLD combined file from 10 to 7 (1.6/1.7/1.10), and the `timer_never` sentinel took corpse-decay and respawn-due (now in lifecycle.rs) off the list entirely"),
        ("module/src/creatures/tick/sense.rs", 2, "`pass_aggro`'s sense-tick player snapshot plus `pass_regen`'s health/power candidate scan — same KNOWN DEBT as mod.rs above, split along with the file"),
        ("module/src/spell/cast/targeting.rs", 1, "AoE/chain target resolution by full-iter + squared distance; a textbook `helpers::entities_near` call (perf-catalog Tier 1)"),
        // #368: both nearest-trainer scans now route through `helpers::nearest_entity` (an
        // indexed `by_map` scan, partition-scoped) — this file's raw-scan count dropped to 0, so
        // it no longer needs a whitelist entry.
    ];

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

    /// Every `(line, accessor)` where a spatial table is read whole — either straight off the
    /// accessor (`ctx.db.game_world_entity().iter()`) or off a local handle bound from it
    /// (`let entities = ctx.db.game_world_entity();` ... `entities.iter()`). Both forms count:
    /// the binding form is the one this codebase actually writes most of the time. Runs on
    /// `crate::test_scan::raw_table_reads` (issue #379) — the shared engine this and
    /// `character_fence_tripwire::raw_lookups` used to duplicate.
    fn raw_scans(content: &str) -> Vec<(usize, &'static str)> {
        crate::test_scan::raw_table_reads(content, SPATIAL_ACCESSORS, opens_whole_table_read)
    }

    #[test]
    fn no_unwhitelisted_raw_spatial_scans() {
        let root = repo_root();
        let mut violations = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            // tripwires.rs (this file) holds the whitelist and this scanner's own prose; it defines
            // no reducers, so skipping it costs nothing — same carve-out the sibling tripwire makes.
            if file.file_name().and_then(|n| n.to_str()) == Some("tripwires.rs") {
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
             file's budget in `WHITELIST` (module/src/tripwires.rs) WITH a one-line justification. \
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
            if file.file_name().and_then(|n| n.to_str()) == Some("tripwires.rs") {
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
/// a verdict each (see the verdict table in `transfer/mod.rs`'s module doc, and `docs/history/transfer-by-guid-verdict-table.md` for each verdict's reasoning); it is a ratchet, so it should only
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
    use crate::test_scan::repo_root;

    /// `(repo-relative path, allowed raw-lookup count, verdict + why)`. One line each; every entry
    /// is an audited exception from the by-guid verdict table in `module/src/transfer/mod.rs`.
    const WHITELIST: &[(&str, usize, &str)] = &[
        // THE GATE ITSELF.
        ("module/src/helpers.rs", 2, "`character_by_guid` + `character_by_name` — the fence; these two ARE the raw lookups everything else routes through"),
        // TRANSFER MACHINERY — must read the row the fence hides, or it could not move it.
        ("module/src/transfer/tests.rs", 1, "NOT a lookup: the expected-shape STRING LITERAL inside `the_production_adapter_is_the_pass_through_the_harness_assumes`, which quotes `CtxShard::has_character`'s body verbatim so any edit to it fails. This scanner reads text and is right to count it"),
        ("module/src/transfer/mod.rs", 4, "TRANSFER MACHINERY, all four reading the row the fence hides because that is the row they exist to move: `in_transit_instances` (the instance reaper's claim census — this call site exists BECAUSE the character is in transit, so a fenced reader would return None every time), `import_character`'s same-database re-partition, and `CtxShard`'s two sink methods — `has_character` (the plan probe, the materialise upsert AND the post-import durability PROOF that the destination copy exists before an in-row licenses deleting the source) and `with_character` (the escrow's own export read, taken after the freeze). #380 dropped this from 8 to 4: the other four were expected-shape STRING LITERALS inside exact-shape pins that quoted `CtxImportSink`'s and `helpers.rs`'s bodies verbatim. The `helpers.rs` ones are gone (all three chokepoints collapsed onto one `gate_by_guid`, whose mutants cargo-mutants CATCHES); the adapter's moved to `transfer/tests.rs`, which is why that file now has a budget of its own"),
        ("module/src/world.rs", 8, "player_login carries its own `is_in_transit` fence (the re-materialisation chokepoint, 2 reads); teleport_player, set_home, persist_entity and is_gm_character take a guid already resolved through a fenced entity (persist_entity is called BY begin_transfer); debug_delete_character keeps a raw existence probe so a missing character stays a no-op while an in-transit one is REFUSED, with the fence on the same expression; cascade_delete_character's raw row DELETE is the sweep that probe guards. `recall_to_home` is now FENCED (character_by_guid) — it was the one teleport_player caller needing no live entity"),
        // OPEN, not decided — spec #12 puts group MEMBERSHIP state on realm-core, settled by #22.
        ("module/src/group.rs", 6, "OPEN: group_accept/group_uninvite/group_leave have a THIRD party mutate game_group_member; if #22 lands membership on realm-core these stop being a world shard's concern entirely. Guessing a verdict here is the mistake #30 exists to correct"),
        // REGENERATE at the destination — connection-derived state, never carried in the blob.
        ("module/src/auth.rs", 4, "REGENERATE: create_character's two name checks (NAME_IN_USE, pre-insert and the race-losing retry) predate any character; its guid-allocator seed scan (`legacy_guid_seed_now`, first-ever touch only) needs the whole table; delete_character keeps a raw find so NO_SUCH_CHAR/NOT_OWNER/CHAR_IN_TRANSIT stay three answers, with the fence on the next line. establish_session's owner_identity rebind and create_character's per-account cap check now route through the `by_account` index instead of a full scan (issue #390), so they no longer count here"),
        // READS, not writes: name/class/race/identity lookups that mutate nothing on the character.
        ("module/src/chat.rs", 2, "add_contact's target-existence check writes only the CALLER's own contact row; #22's `push_whisper` reads a recipient's owner_identity to ADDRESS a whisper event row (the bridge.rs/reputation.rs verdict — no write to the character), and its ZERO fallback is the intended answer on realm-core, where no character row exists at all"),
        ("module/src/items/ops.rs", 2, "race/class reads for the starter loadout and the mana-class gate — no write to the character"),
        ("module/src/spell/cast/targeting.rs", 1, "caster NAME for the resurrect prompt — no write"),
        ("module/src/reputation.rs", 1, "owner_identity fallback for RLS visibility of a new rep row — no write to the character"),
        ("module/src/bridge.rs", 1, "recipient owner_identity for an addon-message row — no write to the character"),
        ("module/src/stats.rs", 2, "set_character_level reads (race, class) and writes level/xp for a guid the caller already resolved through a fenced entity (gm_command's entity_by_owner) or through the FENCED `debug::debug_set_level`; the core itself also serves guids with no character row, so the gate belongs at its callers"),
        ("module/src/xp.rs", 1, "rested-pool drain keyed on the ATTACKER's guid, which came from a live entity (kill credit)"),
        ("module/src/rest.rs", 2, "the rest-state flip takes a live `mover` entity; the 30s accrual pass is BACKGROUND (no entity anywhere in it) and so carries its OWN `is_in_transit` fence — begin_transfer persists with set_offline:false, which is the branch that would have stopped the rest clock, so an escrowed character keeps resting==true and stays in the scan"),
        ("module/src/talent.rs", 1, "the guid comes from the caller's own entity_by_owner-resolved entity"),
        ("module/src/gm.rs", 2, "set_gm_level is FENCED (character_by_name); the two raw reads are gm_command's own gm_level probe and its .money write, both on the caller's entity_by_owner-resolved guid"),
        ("module/src/loot/mod.rs", 1, "DEFER: credit_purse writes the durable row AFTER folding the delta into the escrowed blob (`transfer::defer_money_delta`) — refusing would drop a third party's copper"),
        // #386 split the former single `debug.rs` into a directory; this lookup lives in the one
        // reducer the #378 collapse put it in.
        ("module/src/debug/repair.rs", 1, "`debug_repair_after_publish`'s gm-tester backfill (guid 1, formerly the standalone `debug_seed_gm_tester`); every debug WRITER that touches character state is fenced"),
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

    /// The one accessor this tripwire watches.
    const ACCESSOR: &[&str] = &["game_character"];

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
    /// Runs on `crate::test_scan::raw_table_reads` (issue #379) — the shared engine this and
    /// `partition_discipline_tripwire::raw_scans` used to duplicate.
    fn raw_lookups(content: &str) -> Vec<usize> {
        let mut out: Vec<usize> =
            crate::test_scan::raw_table_reads(content, ACCESSOR, opens_raw_lookup)
                .into_iter()
                .map(|(line, _)| line)
                .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    #[test]
    fn no_unwhitelisted_raw_character_lookups() {
        let root = repo_root();
        let mut violations = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            // tripwires.rs holds this scanner's own prose and defines no reducers — same carve-out
            // both sibling tripwires make.
            if file.file_name().and_then(|n| n.to_str()) == Some("tripwires.rs") {
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
             DEFER or REGENERATE verdict instead and add the site to `WHITELIST` (module/src/tripwires.rs) \
             WITH its verdict — see the table in `module/src/transfer/mod.rs`'s module doc. \
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

/// ENFORCEMENT tripwire (issue #432): SpacetimeDB's own CLI (2.7.1) source-scans this whole crate
/// for the print family — four macro names, each followed by a `!` — and aborts the build the
/// instant one appears ANYWHERE under `module/src`, including behind `#[cfg(test)]`: the scan is
/// source-level and does not know what `cfg` means. #432 was three diagnostic notes in test-only
/// code (`publish_safety.rs` ×2, `test_scan.rs` ×1) that never compile into the wasm at all, yet
/// still failed every fresh clone's first `preflight` — the alpha's own start command, blocked for
/// everyone before a single reducer ever ran. Fixed there by swapping each to
/// `writeln!(std::io::stderr(), ...)`, which the CLI's list does not name, wrapped in `let _ = ...;`
/// so the ignored `Result` raises no warning.
///
/// This pins that fix from coming apart again: no file under `module/src` (or a drop-in
/// `packages/*/src`) may spell one of those four names immediately followed by `!`. The needles
/// are assembled from two literal halves at RUN time — the same self-exclusion
/// `creatures::tick::relay_tripwire`'s move-event needle already uses (see its doc comment) — so
/// the forbidden spelling never sits contiguously in this file's own source. That matters twice
/// over: contiguous text here would (a) match itself in the scan below, and (b) trip the very
/// SpacetimeDB build scan this test exists to keep quiet, since that scan cannot tell a
/// `#[cfg(test)]` tripwire body apart from live reducer code either.
#[cfg(test)]
pub(crate) mod print_macro_tripwire {
    /// The four forbidden spellings, each built from two halves so neither ever appears whole in
    /// this source file.
    fn needles() -> [String; 4] {
        [
            format!("{}{}", "println", "!"),
            format!("{}{}", "print", "!"),
            format!("{}{}", "eprintln", "!"),
            format!("{}{}", "eprint", "!"),
        ]
    }

    #[test]
    fn no_print_family_macro_under_module_src() {
        let mut violations = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            let content = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
            for needle in needles() {
                for (idx, _) in content.match_indices(needle.as_str()) {
                    violations.push(format!(
                        "{}:{} contains `{needle}`",
                        file.display(),
                        crate::test_scan::line_of(&content, idx)
                    ));
                }
            }
        }
        assert!(
            violations.is_empty(),
            "print-family macro found under module/src or a drop-in package (issue #432):\n  {}\n\n\
             SpacetimeDB's CLI source-scans this whole crate and aborts the build on any of these — \
             even behind #[cfg(test)], because the scan is source-level and does not understand cfg. \
             For a test-only diagnostic, use `writeln!(std::io::stderr(), ...)` (needs \
             `use std::io::Write;`) instead, wrapped in `let _ = ...;` so the ignored Result does \
             not warn.",
            violations.join("\n  ")
        );
    }
}

/// ENFORCEMENT tripwire (issue #379): `gc.rs`'s `reap!` list — the fourteen event tables reaped on
/// their 1s `EVENT_TTL_MICROS`, plus the handful of tables with their own ad-hoc TTL block right
/// below it — is hand-maintained prose with no tripwire of its own. A NEW short-lived event table
/// that forgets to add its line leaks forever, silently: nothing else ever deletes its rows.
///
/// This scans every `#[table]` struct across the compiled tree (same file set and same
/// `extract_tables` as `character_owned_tripwire`) for the `id: u64` PK + `created_at: Timestamp`
/// shape every existing TTL-reaped table uses, and fails if `gc.rs`'s own source never actually
/// reaps that accessor — either via `reap!(<accessor>)` or a direct `ctx.db.<accessor>()` call (the
/// shape the ad-hoc blocks and the sweep fns `gc.rs` calls out to use). A bare substring match on
/// the accessor NAME would be satisfied by a comment mentioning it (this file's own `gc.rs` carries
/// exactly that kind of comment about `game_creature_move_event`, explaining why it is no longer
/// reaped), so the check is deliberately narrower than "the name appears somewhere".
///
/// Caught on landing: `game_rest_state_event` (`rest.rs`) carries this exact shape and its own doc
/// comment calls it "a one-shot relay row with a GC TTL" (`transfer/transport.rs`'s
/// `NOT_TRANSPORTED` table), but no line in `gc.rs` ever reaped it — every rest-area threshold
/// crossing for the lifetime of a character left one more row behind. Fixed in the same change by
/// adding it to the `reap!` list.
#[cfg(test)]
mod gc_reap_tripwire {
    /// Accessors carrying the `id: u64` + `created_at: Timestamp` TTL shape that are deliberately
    /// NOT reaped by `gc.rs`:
    /// - `game_creature_move_event`: dead — nothing writes it any more (perf catalog 2.1 moved
    ///   creature legs onto the in-place `game_creature_spline` row, updated rather than
    ///   inserted/reaped); the table stays in the schema, empty, rather than as a separate
    ///   destructive migration to drop it. See the comment atop `reap_movement_events` in `gc.rs`.
    /// - `game_mail`: DURABLE state that merely carries a `created_at` for the client's expiry
    ///   countdown. Reaping it would destroy mail, and the design declines an expiry reaper,
    ///   because nothing should silently delete an attachment a player can still collect.
    const EXEMPT_ACCESSORS: &[&str] = &["game_creature_move_event", "game_mail"];

    /// `gc.rs` actually reaps `accessor` — via the shared `reap!(accessor)` macro invocation, or a
    /// direct `ctx.db.accessor()` call (the shape of the ad-hoc blocks: `game_group_invite`'s own
    /// TTL, and `corpse::sweep_corpse_decay`'s own `ctx.db.game_corpse()`, called from here).
    fn gc_reaps(gc_src: &str, accessor: &str) -> bool {
        gc_src.contains(&format!("reap!({accessor})"))
            || gc_src.contains(&format!("ctx.db.{accessor}()"))
    }

    #[test]
    fn every_ttl_shaped_table_is_reaped_by_gc_rs() {
        let gc_src = crate::test_scan::read_scanned("module/src/gc.rs")
            .expect("module/src/gc.rs ships in every checkout — module/ is never optional");

        let mut missing = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            // gc.rs defines no table of its own; nothing to check it against itself.
            if file.file_name().and_then(|n| n.to_str()) == Some("gc.rs") {
                continue;
            }
            let content = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
            for (accessor, fields) in super::character_owned_tripwire::extract_tables(&content) {
                // Full field names, not "id: u64" / "created_at:" bare — those are substrings of
                // *_guid: u64` fields, which are on almost every table here.
                let is_ttl_shaped = fields.contains("pub id: u64,")
                    && (fields.contains("pub created_at: Timestamp,")
                        || fields.contains("pub created_at: spacetimedb::Timestamp,"));
                if !is_ttl_shaped || EXEMPT_ACCESSORS.contains(&accessor.as_str()) {
                    continue;
                }
                if !gc_reaps(&gc_src, &accessor) {
                    missing.push(format!("{accessor} (in {})", file.display()));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "table(s) shaped `id: u64` PK + `created_at: Timestamp` — the TTL-reaped convention \
             every existing short-lived event table uses — with no reap in gc.rs: {missing:?}\n\n\
             A short-lived event table only stops growing without bound if something deletes its \
             rows. Add `reap!(<accessor>);` to `reap_movement_events` in `gc.rs` (the 1s \
             `EVENT_TTL_MICROS` default), or — if the table's lifecycle needs its own policy, like \
             `game_group_invite`'s 2-minute dialog window or `game_corpse`'s decay-to-bones — a \
             dedicated block or sweep fn called from there (see `corpse::sweep_corpse_decay` for the \
             pattern: gc.rs calls it, it owns the policy). OR add the accessor to `EXEMPT_ACCESSORS` \
             above with a comment justifying why it is intentionally unreaped."
        );
    }
}

// =================================================================================================
//  #456: every `grid_x` write is accompanied by the packed `cell` write
// =================================================================================================

/// The four AOI-scoped tables carry a `cell` column that packs `(grid_x, grid_y)` into one indexed
/// value, because that is the only shape SpacetimeDB 2.7.1's subscription planner can serve from an
/// index. It is therefore a DENORMALIZATION, and the usual denormalization hazard applies with an
/// unusually nasty failure mode: a row whose `cell` disagrees with its `grid_x`/`grid_y` is not
/// slow, it is in the WRONG PLACE. The AOI subscription probes `cell`, so such a row is invisible to
/// every player standing on it and visible to whoever happens to occupy the cell it wrongly claims.
///
/// Nothing in the type system couples them: `grid_x`, `grid_y` and `cell` are three independent
/// columns, and the census behind #456 found **24 independent write sites** across 8 files, with no
/// shared constructor for `game_world_entity` re-stamps (7 hand-rolled `e.grid_x = ..` runs) or for
/// `game_gameobject` (9 independent struct literals). Adding a 25th and forgetting the third line
/// compiles clean, passes every existing test, and shows up live as entities that vanish.
///
/// So: this scan requires every `grid_x` WRITE — a `grid_x: ..` struct-literal field or a
/// `<recv>.grid_x = ..` assignment — to have a `cell` write within the same short window of source
/// text. It is deliberately textual and deliberately local: the point is to fail the build next to
/// the line someone just wrote, not to prove a semantic property.
#[cfg(test)]
pub(crate) mod grid_cell_tripwire {
    use crate::test_scan::{line_of, on_comment_line};

    /// How far after a `grid_x` write the matching `cell` write may sit. Every real site writes the
    /// three within a few adjacent lines; 400 characters is roomy enough for a doc comment between
    /// them and far too tight to reach an unrelated statement.
    const WINDOW: usize = 400;

    /// Files whose `grid_x` mentions are reads, table DEFINITIONS, or fixtures for a table with no
    /// `cell` column at all — assembled at run time so this list cannot match itself.
    fn is_exempt(path: &str) -> bool {
        // `helpers.rs` reads the grid off rows (`grid_of`, `entity_addr`) and its ONE fixture writer
        // is covered; the exemptions below are files where `grid_x` appears only in a signature,
        // a tuple destructure or a comment.
        [
            "module/src/tripwires.rs",
            "module/src/region.rs",
        ]
        .iter()
        .any(|p| path.ends_with(p) || path.replace('\\', "/").ends_with(p))
    }

    /// The AOI-scoped tables' own `#[table]` definitions declare `pub grid_x: i32` next to a `cell`
    /// column further down the struct — further than WINDOW, and not a "write" in any case. Skip a
    /// match that is a struct FIELD DECLARATION rather than an initializer or assignment.
    fn is_field_declaration(code: &str, idx: usize) -> bool {
        let line_start = code[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line_end = code[idx..].find('\n').map(|i| idx + i).unwrap_or(code.len());
        let line = code[line_start..line_end].trim();
        line.starts_with("pub grid_x") || line.starts_with("grid_x: i32")
    }

    /// `true` if the nearest non-whitespace character before `idx` is `{` or `,` — i.e. `grid_x,` at
    /// `idx` sits in a struct-literal FIELD position (first field right after the opening brace, or
    /// any field after a preceding one), not buried inside some other expression. This is what
    /// actually distinguishes the struct-literal shorthand from an ordinary function-call argument
    /// (`grid_cell_id(grid_x, grid_y)`: the character before that `grid_x` is `(`, not `{`/`,`) —
    /// deliberately independent of whether the field shares its line with other fields, so both
    /// this repo's one-field-per-line style AND a compact `E { grid_x, grid_y }` one-liner match.
    fn is_field_position(code: &str, idx: usize) -> bool {
        let bytes = code.as_bytes();
        let mut i = idx;
        while i > 0 {
            let c = bytes[i - 1];
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                i -= 1;
                continue;
            }
            return c == b'{' || c == b',';
        }
        false
    }

    /// `impl` block a `Self { .. }` struct literal at `idx` resolves to. Walks backward for the
    /// nearest `impl` keyword, then takes `impl<..> TypeName` or, for a trait impl, the identifier
    /// after ` for ` (`impl Trait for TypeName`) — the same rule `Self` follows in the language.
    /// A near-clone of the `impl<..>` generics-skip is unavoidable here (no shared engine, unlike
    /// `raw_table_reads`) because this walks forward from a keyword instead of backward from a brace.
    fn resolve_self_type(code: &str, idx: usize) -> Option<&str> {
        let impl_pos = code[..idx].rfind("impl")?;
        let after = code[impl_pos + "impl".len()..].trim_start();
        let bytes = after.as_bytes();
        let mut i = 0usize;
        if bytes.first() == Some(&b'<') {
            let mut depth = 0i32;
            while i < bytes.len() {
                match bytes[i] {
                    b'<' => depth += 1,
                    b'>' => {
                        depth -= 1;
                        i += 1;
                        if depth == 0 {
                            break;
                        }
                        continue;
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        let rest = after[i..].trim_start();
        fn ident_at(s: &str) -> Option<&str> {
            let end = s
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(s.len());
            if end == 0 {
                None
            } else {
                Some(&s[..end])
            }
        }
        if let Some(for_pos) = rest.find(" for ") {
            return ident_at(rest[for_pos + " for ".len()..].trim_start());
        }
        ident_at(rest)
    }

    /// Walks backward from `idx` tracking `{`/`}` depth to find the nearest enclosing brace, then
    /// takes the identifier immediately before it (skipping whitespace). Returns that identifier
    /// only if it looks like a type name (starts uppercase, matching this codebase's convention) —
    /// `TypeName { .. }` or `TypeName { ..prev }` — so `None` correctly falls out for a shorthand
    /// `grid_x,` that is really an ordinary function-CALL argument (`queue_motion(.., grid_x,
    /// grid_y, ..)`: its nearest enclosing `{` is a `fn`/`if`/`match` body brace, not preceded by a
    /// type name) or any other bare code block. A literal `Self { .. }` constructor resolves through
    /// [`resolve_self_type`] to the `impl` block's real type name instead of returning `"Self"`
    /// verbatim — `struct_has_cell_field` searches for `struct Self`, which never exists.
    fn enclosing_struct_literal_type(code: &str, idx: usize) -> Option<&str> {
        let bytes = code.as_bytes();
        let mut depth = 0i32;
        let mut i = idx;
        while i > 0 {
            i -= 1;
            match bytes[i] {
                b'}' => depth += 1,
                b'{' => {
                    if depth > 0 {
                        depth -= 1;
                        continue;
                    }
                    let mut end = i;
                    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
                        end -= 1;
                    }
                    let mut start = end;
                    while start > 0 {
                        let c = bytes[start - 1];
                        if c.is_ascii_alphanumeric() || c == b'_' {
                            start -= 1;
                        } else {
                            break;
                        }
                    }
                    if start == end {
                        return None; // no identifier right before `{` — a bare code block
                    }
                    let ident = &code[start..end];
                    if ident == "Self" {
                        return resolve_self_type(code, i);
                    }
                    return ident
                        .chars()
                        .next()
                        .filter(|c| c.is_ascii_uppercase())
                        .map(|_| ident);
                }
                _ => {}
            }
        }
        None
    }

    /// `true` if `code` (a single file's text) declares `struct TYPE_NAME { .. pub cell: i64 .. }`
    /// somewhere — i.e. `TYPE_NAME` is one of the cell-bearing rows this tripwire polices. Every
    /// cell-bearing type's struct literals in this codebase live in the same file as its own `#[table]`
    /// definition, so a same-file search is enough; a type this can't find (or that genuinely has no
    /// `cell` column — several event-log tables carry `grid_x`/`grid_y` as a plain `by_grid` btree
    /// index with no packed `cell` at all, by design) is treated as "not this tripwire's business",
    /// not as a violation.
    fn struct_has_cell_field(code: &str, type_name: &str) -> bool {
        let needle = format!("struct {type_name}");
        let mut search_from = 0usize;
        while let Some(rel) = code[search_from..].find(&needle) {
            let start = search_from + rel;
            let after = start + needle.len();
            let boundary_ok = code[after..]
                .chars()
                .next()
                .map(|c| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(true);
            if boundary_ok {
                if let Some(rel_brace) = code[after..].find('{') {
                    let body_start = after + rel_brace;
                    let mut depth = 0i32;
                    let mut body_end = None;
                    for (i, c) in code[body_start..].char_indices() {
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
                    if let Some(end) = body_end {
                        if code[body_start..=end].contains("pub cell: i64") {
                            return true;
                        }
                    }
                }
            }
            search_from = start + needle.len();
        }
        false
    }

    /// Every line where `code` writes `grid_x` — via `grid_x: value` (an explicit initializer or
    /// assignment target), `.grid_x = value` (a field assignment), or bare field-init SHORTHAND
    /// `grid_x,` — with no `cell` write within `WINDOW` characters after it. Shared by the
    /// real-file scan below and its own fixture test, so both exercise the identical matching logic.
    ///
    /// The shorthand form gets two extra gates the explicit forms don't need, because unlike
    /// `grid_x: ` / `.grid_x = ` its literal text also shows up somewhere that ISN'T a struct-literal
    /// write: `grid_x,` must sit in a struct-literal FIELD position (`is_field_position` — the
    /// nearest non-whitespace character before it is `{` or `,`; excludes a read like
    /// `grid_cell_id(grid_x, grid_y)`, whose preceding character is `(`, regardless of whether the
    /// field shares a line with others), AND its enclosing struct-literal type must actually declare
    /// a `cell` column (`enclosing_struct_literal_type` + `struct_has_cell_field` — excludes both
    /// ordinary function-call arguments, whose enclosing brace is a fn/if/match body not a type name,
    /// and the several event-log tables that carry `grid_x`/`grid_y` with no `cell` column at all,
    /// by design). `enclosing_struct_literal_type` resolves a literal `Self { .. }` constructor
    /// through `resolve_self_type` to the `impl` block's real type name, so a constructor written as
    /// `Self { .. }` inside `impl E { .. }` is policed exactly like `E { .. }` would be.
    fn grid_x_writes_missing_cell(code: &str) -> Vec<usize> {
        // Assembled at run time — a contiguous literal would match this file's own text.
        let field_init = format!("{}{}", "grid_x", ": ");
        let assign = format!("{}{}", ".grid_x", " = ");
        let shorthand = format!("{}{}", "grid_x", ",");
        let cell_token = "cell";

        let mut violations = Vec::new();
        for needle in [field_init.as_str(), assign.as_str(), shorthand.as_str()] {
            for (idx, _) in code.match_indices(needle) {
                if on_comment_line(code, idx) || is_field_declaration(code, idx) {
                    continue;
                }
                if needle == shorthand.as_str() {
                    if !is_field_position(code, idx) {
                        continue;
                    }
                    let Some(type_name) = enclosing_struct_literal_type(code, idx) else {
                        continue;
                    };
                    if !struct_has_cell_field(code, type_name) {
                        continue;
                    }
                }
                let end = (idx + WINDOW).min(code.len());
                if !code[idx..end].contains(cell_token) {
                    violations.push(line_of(code, idx));
                }
            }
        }
        violations
    }

    #[test]
    fn every_grid_x_write_also_writes_the_packed_cell() {
        let mut violations = Vec::new();
        for file in super::character_owned_tripwire::scanned_files() {
            let display = file.display().to_string();
            if is_exempt(&display) {
                continue;
            }
            let code = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("cannot read {display}: {e}"));
            for line in grid_x_writes_missing_cell(&code) {
                violations.push(format!("{display}:{line}"));
            }
        }

        assert!(
            violations.is_empty(),
            "a `grid_x` write with no `cell` write beside it (#456):\n  {}\n\n\
             `cell` packs `(grid_x, grid_y)` into the one indexed value the AOI subscription probes \
             by equality — it is what makes the box query index-served instead of a full partition \
             scan. It is a plain column, so nothing updates it for you: write it in the SAME \
             statement run that writes `grid_x`/`grid_y`, via \
             `lyracore_shared::spatial::grid_cell_id(grid_x, grid_y)` (or `cell_id_at(x, y)` when \
             you have the world position rather than the cell). A row whose `cell` disagrees with \
             its grid columns is not merely slow — it is addressed to the WRONG CELL, so it is \
             invisible to the players standing on it.",
            violations.join("\n  ")
        );
    }

    /// The tripwire is only worth anything if it would actually fire. Feed it the shape it hunts.
    #[test]
    fn the_scan_rejects_a_grid_write_with_no_cell_beside_it() {
        let bad = "fn f(e: &mut E) {\n    e.grid_x = gx;\n    e.grid_y = gy;\n}\n";
        let needle = format!("{}{}", ".grid_x", " = ");
        let idx = bad.find(&needle).expect("fixture must contain the shape");
        let end = (idx + WINDOW).min(bad.len());
        assert!(
            !bad[idx..end].contains("cell"),
            "the fixture is supposed to be a VIOLATION"
        );
        let good = "fn f(e: &mut E) {\n    e.grid_x = gx;\n    e.grid_y = gy;\n    \
                    e.cell = spatial::grid_cell_id(gx, gy);\n}\n";
        let idx = good.find(&needle).unwrap();
        let end = (idx + WINDOW).min(good.len());
        assert!(
            good[idx..end].contains("cell"),
            "the accompanied form must PASS"
        );
    }

    /// Issue #467: the tripwire's original `grid_x: ` needle never saw field-init SHORTHAND
    /// (`grid_x,`, no `: value`) — what an initializer built from same-named locals uses, the
    /// idiom `motion.rs` uses throughout. This feeds `grid_x_writes_missing_cell` a struct literal
    /// written that way and asserts a broken one (no `cell` field) is CAUGHT, a correct one PASSES,
    /// and a `grid_x,` that is merely a function-call ARGUMENT (a read, not a struct-literal field)
    /// is not mistaken for a write.
    #[test]
    fn the_scan_catches_a_broken_shorthand_initializer() {
        // `struct E` declares a `cell` column, so this tripwire is E's business — mirrors a real
        // cell-bearing table's own file, where the `#[table]` struct and every construction site
        // live together.
        let cell_bearing_struct = "struct E {\n    pub grid_x: i32,\n    pub grid_y: i32,\n    pub cell: i64,\n}\n";

        let bad = format!(
            "{cell_bearing_struct}fn f() -> E {{\n    E {{\n        grid_x,\n        grid_y,\n    }}\n}}\n"
        );
        assert_eq!(
            grid_x_writes_missing_cell(&bad).len(),
            1,
            "a shorthand `grid_x,` field with no `cell` field beside it must be a VIOLATION"
        );

        let good = format!(
            "{cell_bearing_struct}fn f() -> E {{\n    E {{\n        grid_x,\n        grid_y,\n        cell,\n    }}\n}}\n"
        );
        assert!(
            grid_x_writes_missing_cell(&good).is_empty(),
            "a shorthand initializer that also sets `cell` must PASS"
        );

        let read = "fn f() {\n    let cell = grid_cell_id(grid_x, grid_y);\n}\n";
        assert!(
            grid_x_writes_missing_cell(read).is_empty(),
            "`grid_x,` as a function-call ARGUMENT (a read) must not be mistaken for a write"
        );

        // A struct with grid_x/grid_y but genuinely NO `cell` column (the event-log-table shape) —
        // shorthand there is legitimately not this tripwire's business.
        let no_cell_struct = "struct NoCell {\n    pub grid_x: i32,\n    pub grid_y: i32,\n}\n";
        let no_cell_use = format!(
            "{no_cell_struct}fn f() -> NoCell {{\n    NoCell {{\n        grid_x,\n        grid_y,\n    }}\n}}\n"
        );
        assert!(
            grid_x_writes_missing_cell(&no_cell_use).is_empty(),
            "a type with no `cell` column must not be flagged for lacking one"
        );
    }

    /// A code-review finding on #467: the shorthand path resolved its enclosing struct-literal type
    /// by searching the file for `struct <name> { .. }`, which finds nothing for the near-universal
    /// `Self { .. }` constructor idiom (`impl E { fn f() -> Self { Self { .. } } }`) — `Self` is not
    /// a struct declaration. Confirms a broken `Self { .. }` shorthand initializer is still CAUGHT.
    #[test]
    fn the_scan_catches_a_broken_self_shorthand_initializer() {
        let bad = "struct E {\n    pub grid_x: i32,\n    pub grid_y: i32,\n    pub cell: i64,\n}\n\
                   impl E {\n    fn f() -> Self {\n        Self {\n            grid_x,\n            grid_y,\n        }\n    }\n}\n";
        assert_eq!(
            grid_x_writes_missing_cell(bad).len(),
            1,
            "a broken `Self {{ .. }}` shorthand initializer must be a VIOLATION"
        );

        let good = "struct E {\n    pub grid_x: i32,\n    pub grid_y: i32,\n    pub cell: i64,\n}\n\
                    impl E {\n    fn f() -> Self {\n        Self {\n            grid_x,\n            grid_y,\n            cell,\n        }\n    }\n}\n";
        assert!(
            grid_x_writes_missing_cell(good).is_empty(),
            "a `Self {{ .. }}` shorthand initializer that also sets `cell` must PASS"
        );
    }

    /// A code-review finding on #467: `is_own_line` required the ENTIRE trimmed line to be exactly
    /// `grid_x,`, so a compact one-liner (`E { grid_x, grid_y }`, multiple fields sharing a line)
    /// was skipped outright — violation or not. Confirms a broken compact literal is still CAUGHT.
    #[test]
    fn the_scan_catches_a_broken_compact_one_line_shorthand() {
        let cell_bearing_struct = "struct E {\n    pub grid_x: i32,\n    pub grid_y: i32,\n    pub cell: i64,\n}\n";

        let bad = format!("{cell_bearing_struct}fn f() -> E {{ E {{ grid_x, grid_y }} }}\n");
        assert_eq!(
            grid_x_writes_missing_cell(&bad).len(),
            1,
            "a broken compact one-line shorthand initializer must be a VIOLATION"
        );

        let good = format!("{cell_bearing_struct}fn f() -> E {{ E {{ grid_x, grid_y, cell }} }}\n");
        assert!(
            grid_x_writes_missing_cell(&good).is_empty(),
            "a compact one-line shorthand initializer that also sets `cell` must PASS"
        );
    }
}
