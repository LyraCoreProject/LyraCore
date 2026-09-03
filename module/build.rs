//! Build-time codegen for the module's self-registration registries.
//!
//! Scans every `.rs` file under `src/` AND under each drop-in package's `packages/<name>/src/`
//! (repo root — `packages/` ships the maintained reference Package, `packages/example/`, even in
//! the public mirror; every OTHER package is an operator's own private addition) for the three
//! marker invocations and generates, into `$OUT_DIR`, the files the crate `include!`s:
//!
//! - `character_sweeps.rs` — from `character_owned!` markers: the two `&[fn(...)]`
//!   const arrays `CHARACTER_OWNED_DELETE_SWEEPS` and `CHARACTER_OWNED_RESTAMP_SWEEPS`, plus
//!   `CHARACTER_OWNED_TABLES: &[&str]` — the same enumeration as NAMES, which is what the escrowed
//!   transfer manifest (`src/transfer.rs`, issue #16) exports. The names are derived from the
//!   `delete` sweep fn names by stripping the mandatory `sweep_delete_` prefix (every marker in the
//!   tree follows it), so the manifest can never drift from the sweep registry — there is exactly
//!   ONE list, and a table that gets a sweep gets a manifest entry in the same edit. A `delete`
//!   marker whose fn is not named `sweep_delete_<table_accessor>` panics below: the transfer
//!   manifest would silently name a non-existent table. The `transfer` and `not_transported` marker
//!   kinds add `CHARACTER_OWNED_TRANSFERS: &[(&str, fn(..))]` (issue #19) — the cross-DATABASE row
//!   transport, keyed by the same table-accessor name so a mover can never be paired with the wrong
//!   manifest entry (`sweep_transfer_<table_accessor>`, same prefix-strip rule) — plus two
//!   plain-string views of it (#380): `CHARACTER_OWNED_TRANSFER_NAMES` (every transported table, so
//!   a NATIVE test binary can read the registry without materializing fn pointers it cannot link)
//!   and `CHARACTER_OWNED_NOT_TRANSPORTED` (the subset registered through the `not_transported`
//!   kind, cross-checked against `transfer::NOT_TRANSPORTED`'s written reasons). A declining arm is
//!   still a transport arm — it lands in `CHARACTER_OWNED_TRANSFERS` too — so "this table has an
//!   arm" and "this table's rows actually cross" stay two separate, separately-ratcheted facts.
//! - `package_mods.rs` — one `#[path = ...] pub mod pkg_<name>;` per discovered package, so a
//!   folder dropped into `packages/` compiles into the module wasm with ZERO core-file edits.
//!   A package = `packages/<name>/src/mod.rs` (+ sibling submodule files).
//! - `package_registries.rs` — from `game_tick_pass!`, `game_hook!`, and
//!   `encounter_package!` markers:
//!   `GAME_TICK_PASSES` (periodic passes run by the core scheduler tick) and one
//!   `GAME_HOOKS_<EVENT>` array per known notify-hook event, dispatched at the core chokepoints
//!   (see `src/hooks.rs`), plus the map-scoped encounter authority registry.
//! - `hook_dispatch.rs` — from `HOOK_EVENTS` below: the `payload_for` alias mod
//!   and one `fire_*` fn per event, included INSIDE `src/hooks.rs` so the paths every chokepoint
//!   already uses (`hooks::fire_*`, `hooks::payload_for::*`) are unchanged. This is what keeps the
//!   event catalog to TWO homes: the payload struct in hooks.rs and the `HOOK_EVENTS` row here.
//!
//! This is a text scan, not a real Rust parser — it never inspects a marker's body, only the shape
//! of the invocation head. The scan runs on a COMMENT- AND STRING-STRIPPED copy of each
//! file (newlines preserved, so panic line numbers stay true): a commented-out marker, or marker
//! syntax quoted in a doc example or string literal, neither registers nor breaks the build — the
//! macro docs in `src/lib.rs` show real invocation syntax precisely because of this. On the CODE
//! that remains, the scan is deliberately strict: every occurrence of a marker's literal substring
//! must match the exact expected shape, or the build panics (loudly, at `cargo build`/`cargo
//! check` time) instead of silently registering nothing. A typo'd or reshaped marker must never
//! just vanish from the registry.
//!
//! The fully-qualified path is derived from the file's location, using plain Rust module
//! conventions collapsed to the top segment: `src/foo.rs` -> `crate::foo`, `src/foo/bar.rs` ->
//! `crate::foo` (every directory module in this crate is a thin facade whose `mod.rs` does
//! `pub use bar::*;`). Package files map to the package's generated root module:
//! `packages/<name>/src/**` -> `crate::pkg_<name>`. The facade assumption is CHECKED:
//! a marker in a nested submodule file whose facade does not visibly re-export it panics at build
//! time naming the missing `pub use` — instead of failing later as an opaque rustc error inside
//! `$OUT_DIR`.
//!
//! The same pass also lints each package file against the Package API surface
//! (`PACKAGE_API_ROOTS`, documented at `docs/package-api.md`): a path that reaches the crate root
//! outside it fails the build naming the Package, the file, the line and the path, unless the line
//! carries `// package-api: exempt <reason>`. Core `src/` is never linted.
//!
//! EVALUATED AND REJECTED: replacing the marker scan with an explicit per-package `register()`
//! convention. The registries are const fn-pointer arrays (no allocator-dependent init order in
//! wasm), and `character_owned!` markers are deliberately scattered NEXT TO their tables across
//! core files — a tripwire test enforces that locality. A central register() would keep codegen
//! anyway (package discovery) while losing the write-the-marker-where-the-code-lives property the
//! substrate is built on. Hardening the scan (strip + facade check) buys the same safety without
//! the churn.

use std::fs;
use std::path::{Path, PathBuf};

/// The folder name of the maintained reference Package (`packages/example/`) that
/// `lyracore packages new` scaffolds from. It ships in every checkout, including the public
/// mirror, and is deliberately excluded from the `has_packages` cfg — see its use below.
const REFERENCE_PACKAGE: &str = "example";

/// One row of the notify-hook event catalog.
///
/// `actor`/`target` are the Runtime Script Event Binding half (#318): Rust expressions, evaluated
/// against `payload` inside the generated `fire_*`, naming the guid that CAUSED the event and the
/// guid it acted ON. `"0"` means the event has no such participant, which reaches a Runtime Script
/// as an absent `event.actor`/`event.target` rather than as an error.
///
/// The mapping is a judgement per event, which is why it lives beside the event rather than being
/// derived: `on_loot`'s target is the corpse, `on_hp_threshold` has no actor at all, and `on_death`
/// is victim-centric while `on_kill` names the same two guids the other way round.
struct HookEvent {
    event: &'static str,
    payload_ty: &'static str,
    actor: &'static str,
    target: &'static str,
}

const fn hook(
    event: &'static str,
    payload_ty: &'static str,
    actor: &'static str,
    target: &'static str,
) -> HookEvent {
    HookEvent {
        event,
        payload_ty,
        actor,
        target,
    }
}

/// The notify-hook event catalog: event name -> the payload type the
/// handler receives (the struct lives in `src/hooks.rs`) -> the actor and target guids a Runtime
/// Script bound to it receives. This row is HALF of an event's
/// definition; the payload struct is the other half. From these rows build.rs generates the
/// per-event registry array (`package_registries.rs`) AND the `payload_for` alias + `fire_*`
/// dispatch fn (`hook_dispatch.rs`) — so adding an event is: payload struct in hooks.rs, one row
/// here, plus the dispatch call at the new core chokepoint. A `game_hook!` naming any other event
/// panics below with this list.
///
/// The event NAMES are mirrored by `lyracore_package_delta::script::HOOK_EVENT_NAMES`, which a pure
/// crate needs to refuse a Package binding to an event that does not exist. This build emits
/// `GAME_HOOK_EVENT_NAMES` from these rows and `module/src/script_binding.rs` asserts the two lists
/// are identical, so the catalog still cannot drift.
const HOOK_EVENTS: &[HookEvent] = &[
    hook(
        "on_damage_taken",
        "crate::hooks::DamageTakenPayload",
        "payload.attacker_guid",
        "payload.target_guid",
    ),
    hook(
        "on_death_prevented",
        "crate::hooks::DeathPreventedPayload",
        "payload.attacker_guid",
        "payload.creature_guid",
    ),
    // The spawning creature is the subject of its own spawn, and there is nothing it acted on.
    hook(
        "on_creature_spawn",
        "crate::hooks::CreatureSpawnPayload",
        "payload.guid",
        "0",
    ),
    // NOTE: `grant_xp` persists the mutated entity AFTER its ding loop, so a script reading
    // `event.actor.level` here sees the level BEFORE the ding. Read the level from the payload's
    // own consumer, not from the actor, until that site is reordered.
    hook(
        "on_levelup",
        "crate::hooks::LevelupPayload",
        "payload.character_guid",
        "0",
    ),
    hook(
        "on_group_invite",
        "crate::hooks::GroupInvitePayload",
        "payload.inviter_guid",
        "payload.target_guid",
    ),
    hook(
        "on_death",
        "crate::hooks::DeathPayload",
        "payload.killer_guid",
        "payload.victim_guid",
    ),
    hook(
        "on_kill",
        "crate::hooks::KillPayload",
        "payload.killer_guid",
        "payload.victim_guid",
    ),
    hook(
        "on_aggro",
        "crate::hooks::AggroPayload",
        "payload.creature_guid",
        "payload.target_guid",
    ),
    hook(
        "on_cast_resolved",
        "crate::hooks::CastResolvedPayload",
        "payload.caster_guid",
        "payload.target_guid",
    ),
    // The corpse is a loot container, not a live entity, so `event.target` is normally absent here.
    hook(
        "on_loot",
        "crate::hooks::LootPayload",
        "payload.looter_guid",
        "payload.corpse_guid",
    ),
    hook(
        "on_quest_accept",
        "crate::hooks::QuestAcceptPayload",
        "payload.character_guid",
        "0",
    ),
    hook(
        "on_quest_turnin",
        "crate::hooks::QuestTurninPayload",
        "payload.character_guid",
        "0",
    ),
    hook(
        "on_login",
        "crate::hooks::LoginPayload",
        "payload.character_guid",
        "0",
    ),
    // Fired BEFORE the live entity row is deleted, so the actor still reads.
    hook(
        "on_logout",
        "crate::hooks::LogoutPayload",
        "payload.character_guid",
        "0",
    ),
    hook(
        "on_gossip_select",
        "crate::hooks::GossipSelectPayload",
        "payload.character_guid",
        "payload.npc_guid",
    ),
    // Work-item 228 (encounter kernel): entry-keyed creature death, once-per-instance HP-threshold
    // crossings (fired by encounter::encounter_hp_probe, not a new core chokepoint), and GO use.
    hook(
        "on_creature_death",
        "crate::hooks::CreatureDeathPayload",
        "payload.killer_guid",
        "payload.creature_guid",
    ),
    // A threshold crossing has no actor: the probe fires it, not a unit.
    hook(
        "on_hp_threshold",
        "crate::hooks::HpThresholdPayload",
        "0",
        "payload.creature_guid",
    ),
    // The gameobject is not a world entity, so `event.target` is normally absent here.
    hook(
        "on_go_used",
        "crate::hooks::GoUsedPayload",
        "payload.user_guid",
        "payload.go_guid",
    ),
];

/// The Package API surface, as the module roots a Package may name. `docs/package-api.md` is the
/// contract; this list is what enforces it. Root granularity: everything under a listed root is on
/// the surface, and a root that is absent is core's own business. Adding one here means adding it
/// to the document in the same change.
const PACKAGE_API_ROOTS: &[&str] = &[
    "actor",
    "chat",
    "combat",
    "creatures",
    "encounter",
    "faction",
    "gameobject",
    "group",
    "helpers",
    "hooks",
    "items",
    "loot",
    "nav",
    "package_config",
    "quest",
    "script_binding",
    "spell",
    "stats",
    "terrain",
    "transfer",
    "world",
    "xp",
];

/// Crate-root names on the surface that are neither a module nor a type: the two marker macros the
/// `game_` prefix below does not already cover, and the generated character-owned table manifest.
const PACKAGE_API_ROOT_ITEMS: &[&str] = &[
    "CHARACTER_OWNED_TABLES",
    "character_owned",
    "encounter_package",
];

/// The comment that clears one out-of-surface path, written on the line that names it. It must
/// carry a reason, so an exemption is always readable where it is used and greppable across a tree.
const PACKAGE_API_EXEMPT: &str = "// package-api: exempt";

fn main() {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let src_dir = Path::new(&manifest_dir).join("src");
    println!("cargo:rerun-if-changed={}", src_dir.display());

    // packages/ lives at the repo root (module/'s parent), holding at minimum the reference
    // Package (`example/`) — present in every checkout including the public mirror — so
    // rerun-if-changed never points at a missing path here.
    let packages_dir = Path::new(&manifest_dir)
        .parent()
        .expect("module/ has a parent (the repo root)")
        .join("packages");
    println!("cargo:rerun-if-changed={}", packages_dir.display());

    // Scan roots: (crate-path prefix, dir). Core src/ derives per-file prefixes; each package
    // collapses to its generated root module.
    let mut registries = Registries::default();

    // ---- core src/ ----
    let mut files = Vec::new();
    collect_rs_files(&src_dir, &mut files);
    files.sort();
    for file in &files {
        let prefix = core_module_path(&src_dir, file);
        scan_file(file, &src_dir, false, &prefix, &mut registries);
    }

    // ---- packages/*/src/ ----
    let mut pkg_mods: Vec<(String, PathBuf)> = Vec::new(); // (ident, abs path to src/mod.rs)
    if packages_dir.is_dir() {
        let mut pkg_dirs: Vec<PathBuf> = fs::read_dir(&packages_dir)
            .unwrap_or_else(|e| panic!("build.rs: cannot read {}: {e}", packages_dir.display()))
            .map(|e| e.expect("readable dir entry").path())
            .filter(|p| p.is_dir())
            .collect();
        pkg_dirs.sort();
        for pkg in pkg_dirs {
            let name = pkg.file_name().unwrap().to_string_lossy().into_owned();
            let pkg_src = pkg.join("src");
            if !pkg_src.is_dir() {
                // A package needs no Rust half. A CLIENT-ONLY package (client/ — addons/FrameXML
                // shipped by --pack-client) and a DATA-ONLY package (data/ — the Package Deltas a
                // Datascript generates, applied by the importer) are both legal, quiet shapes. A
                // package with none of the three is probably a typo'd `scr/` folder — warn loudly.
                if !pkg.join("client").is_dir() && !pkg.join("data").is_dir() {
                    println!("cargo:warning=packages/{name}: none of src/, client/ or data/ — nothing registered (typo'd folder?)");
                }
                continue;
            }
            let mod_rs = pkg_src.join("mod.rs");
            if !mod_rs.is_file() {
                panic!(
                    "build.rs: packages/{name}/src/ exists but has no mod.rs — a package's Rust root \
                     must be src/mod.rs. It must never be silently skipped."
                );
            }
            let ident = package_ident(&name);
            let prefix = format!("crate::pkg_{ident}");
            let mut pkg_files = Vec::new();
            collect_rs_files(&pkg_src, &mut pkg_files);
            pkg_files.sort();
            for file in &pkg_files {
                lint_package_api(&name, &pkg_src, file);
                scan_file(file, &pkg_src, true, &prefix, &mut registries);
            }
            pkg_mods.push((ident, mod_rs));
        }
    }

    // `has_packages`: at least one drop-in package with Rust in it compiled into this crate, OTHER
    // than the reference Package (`packages/example/`). A handful of `actor.rs` verbs and one
    // `group.rs` emitter exist SOLELY for a real package to call (the same shape as the
    // `debug_only!` verbs, whose only consumer is the feature-gated debug harness), so without this
    // cfg a build with no such package — which, since the reference Package ships in every
    // checkout, is now the common case rather than the packages-less one — reports them as unused
    // imports and dead code. `packages/example/` never calls them, by design (it stays inert), so it
    // is excluded here rather than counted. The check-cfg line is unconditional so rustc's
    // `unexpected_cfgs` lint knows the name either way.
    println!("cargo::rustc-check-cfg=cfg(has_packages)");
    if pkg_mods.iter().any(|(ident, _)| ident != REFERENCE_PACKAGE) {
        println!("cargo::rustc-cfg=has_packages");
    }

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo");

    // ---- character_sweeps.rs (also fed by package markers) ----
    registries.delete.sort();
    registries.restamp.sort();
    let mut out = String::new();
    out.push_str(
        "// GENERATED by module/build.rs from `character_owned!` markers under src/ and packages/*/src/. DO NOT EDIT.\n",
    );
    out.push_str(
        "pub const CHARACTER_OWNED_DELETE_SWEEPS: &[fn(&spacetimedb::ReducerContext, u64)] = &[\n",
    );
    for path in &registries.delete {
        out.push_str(&format!("    {path},\n"));
    }
    out.push_str("];\n");
    out.push_str(
        "pub const CHARACTER_OWNED_RESTAMP_SWEEPS: &[fn(&spacetimedb::ReducerContext, u64, spacetimedb::Identity)] = &[\n",
    );
    for path in &registries.restamp {
        out.push_str(&format!("    {path},\n"));
    }
    out.push_str("];\n");
    // The SAME enumeration as table-accessor names — the escrowed-transfer manifest (src/transfer.rs).
    // Derived from the delete-sweep fn names so there is no parallel list to hand-maintain.
    out.push_str("pub const CHARACTER_OWNED_TABLES: &[&str] = &[\n");
    for path in &registries.delete {
        let fn_name = path
            .rsplit("::")
            .next()
            .expect("rsplit yields at least one segment");
        let table = fn_name.strip_prefix("sweep_delete_").unwrap_or_else(|| {
            panic!(
                "build.rs: `character_owned!(delete, fn {fn_name}(..))` must be named \
                 `sweep_delete_<table_accessor>` — the transfer manifest (CHARACTER_OWNED_TABLES) \
                 derives the table name by stripping that prefix, so any other spelling would put a \
                 non-existent table in the export blob."
            )
        });
        out.push_str(&format!("    \"{table}\",\n"));
    }
    out.push_str("];\n");
    // The CROSS-DATABASE row transport (issue #19), keyed by table accessor so
    // `transfer::export_rows`/`import_rows` can pair a manifest entry with its mover. Derived from
    // the `sweep_transfer_<table_accessor>` fn names by the same prefix-strip rule as the delete
    // sweeps — so a transport arm can never name a table that isn't in the manifest.
    registries.transfer.sort();
    // ONE table, ONE arm. A second arm for a table that already has one exports its rows twice,
    // and — when the second one declines — is how a drop-in could stop a CORE table from crossing.
    // `the_not_transported_allowlist_matches_the_arms_that_decline` used to catch that from the
    // package side; it no longer sees a package-registered decline, so the shape check moves here.
    let mut claimed: Vec<(&str, &String)> = Vec::new();
    for path in &registries.transfer {
        let table = transfer_table_name(path);
        if let Some((_, first)) = claimed.iter().find(|(claimed, _)| *claimed == table) {
            panic!(
                "build.rs: two `character_owned!` transport arms name the table `{table}` \
                 (`{first}` and `{path}`). A table has exactly one arm — a second one either \
                 carries its rows twice or overrides the first arm's decision to carry them at all."
            );
        }
        claimed.push((table, path));
    }
    out.push_str(
        // The `(name, fn)` pair trips `clippy::type_complexity` in the GENERATED file, where nobody
        // can annotate it — emit the allow with it. The pair is the registry's row shape, not an
        // accidental type: each entry is one table's name plus its transfer sweep.
        "// The `(&str, fn(..))` pair is this registry's row shape: one table name + its sweep.\n\
         #[allow(clippy::type_complexity)]\n\
         pub const CHARACTER_OWNED_TRANSFERS: &[(&str, fn(&spacetimedb::ReducerContext, u64, &mut crate::transfer::RowIo<'_>))] = &[\n",
    );
    for path in &registries.transfer {
        out.push_str(&format!(
            "    (\"{}\", {path}),\n",
            transfer_table_name(path)
        ));
    }
    out.push_str("];\n");
    // The SAME names again, as plain strings (#380). `CHARACTER_OWNED_TRANSFERS` above cannot be
    // named from a NATIVE test binary — referencing it materializes every registered fn's POINTER,
    // which drags the SpacetimeDB host imports (`datastore_insert_bsatn`, …) in and they cannot
    // link outside wasm. The transfer ratchet used to work around that by string-parsing this very
    // generated file at test time; it reads this array instead.
    out.push_str("// The transported-table names, in `CHARACTER_OWNED_TRANSFERS` order.\n");
    out.push_str("pub const CHARACTER_OWNED_TRANSFER_NAMES: &[&str] = &[\n");
    for path in &registries.transfer {
        out.push_str(&format!("    \"{}\",\n", transfer_table_name(path)));
    }
    out.push_str("];\n");
    // The DECLINING subset — the arms written with the `not_transported` marker kind. This is the
    // mechanical half of the decision; the reasoned half is `transfer::NOT_TRANSPORTED`, and
    // `the_not_transported_allowlist_matches_the_arms_that_decline` fails if they disagree in
    // either direction (#380). Sorted by TABLE name so the assertion compares two stable lists.
    registries.not_transported.sort();
    let mut declines: Vec<String> = registries
        .not_transported
        .iter()
        .map(|p| transfer_table_name(p).to_string())
        .collect();
    declines.sort();
    out.push_str(
        "// Tables whose transport arm deliberately carries NOTHING (the `not_transported` marker\n\
         // kind), sorted by table name. Cross-checked against `transfer::NOT_TRANSPORTED`.\n",
    );
    out.push_str("pub const CHARACTER_OWNED_NOT_TRANSPORTED: &[&str] = &[\n");
    for table in &declines {
        out.push_str(&format!("    \"{table}\",\n"));
    }
    out.push_str("];\n");
    write_out(&out_dir, "character_sweeps.rs", &out);

    // ---- package_mods.rs ----
    let mut out = String::new();
    out.push_str(
        "// GENERATED by module/build.rs from packages/*/src/mod.rs discovery. DO NOT EDIT.\n",
    );
    for (ident, mod_rs) in &pkg_mods {
        out.push_str(&format!("#[path = \"{}\"]\n", mod_rs.display()));
        out.push_str(&format!("pub mod pkg_{ident};\n"));
    }
    write_out(&out_dir, "package_mods.rs", &out);

    // ---- package_registries.rs ----
    registries.tick_passes.sort();
    registries
        .encounter_packages
        .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    for duplicate in registries.encounter_packages.windows(2) {
        if duplicate[0].0 == duplicate[1].0 {
            panic!(
                "build.rs: encounter binding {} has more than one installed package authority: {} and {}",
                duplicate[0].0, duplicate[0].1, duplicate[1].1
            );
        }
    }
    let mut out = String::new();
    out.push_str(
        "// GENERATED by module/build.rs from package markers under src/ and packages/*/src/. DO NOT EDIT.\n",
    );
    // Same reason as `CHARACTER_OWNED_TRANSFERS` above: the allow has to be emitted, because the
    // file it lands in is generated and `DO NOT EDIT`.
    out.push_str(
        "// The `(&str, fn(..))` pair is this registry's row shape: one pass name + the pass.\n\
         #[allow(clippy::type_complexity)]\n\
         pub const GAME_TICK_PASSES: &[(&str, fn(&spacetimedb::ReducerContext))] = &[\n",
    );
    for path in &registries.tick_passes {
        out.push_str(&format!("    (\"{path}\", {path}),\n"));
    }
    out.push_str("];\n");
    for HookEvent {
        event, payload_ty, ..
    } in HOOK_EVENTS
    {
        let mut hooks: Vec<String> = registries
            .hooks
            .iter()
            .filter(|(e, _)| e == event)
            .map(|(_, p)| p.clone())
            .collect();
        hooks.sort();
        out.push_str(&format!(
            "pub const GAME_HOOKS_{}: &[fn(&spacetimedb::ReducerContext, &{payload_ty})] = &[\n",
            event.to_uppercase()
        ));
        for path in &hooks {
            out.push_str(&format!("    {path},\n"));
        }
        out.push_str("];\n");
    }
    out.push_str(
        "pub const GAME_ENCOUNTER_PACKAGES: &[(crate::encounter::EncounterBinding, crate::encounter::EncounterPackageHandler)] = &[\n",
    );
    for (binding, path) in &registries.encounter_packages {
        out.push_str(&format!(
            "    (crate::encounter::EncounterBinding::{binding}, {path}),\n"
        ));
    }
    out.push_str("];\n");
    out.push_str("pub const GAME_ENCOUNTER_PACKAGE_BINDING_NAMES: &[&str] = &[\n");
    for (binding, _) in &registries.encounter_packages {
        out.push_str(&format!("    \"{binding}\",\n"));
    }
    out.push_str("];\n");
    // Every event name, as plain strings a NATIVE test binary can read without materializing the
    // fn-pointer arrays above — the same reason `CHARACTER_OWNED_TRANSFER_NAMES` exists. This is
    // what `script_binding.rs` asserts the Package Delta crate's mirror of the catalog against.
    out.push_str("pub const GAME_HOOK_EVENT_NAMES: &[&str] = &[\n");
    for HookEvent { event, .. } in HOOK_EVENTS {
        out.push_str(&format!("    \"{event}\",\n"));
    }
    out.push_str("];\n");
    write_out(&out_dir, "package_registries.rs", &out);

    // ---- hook_dispatch.rs ---- included INSIDE src/hooks.rs, so `payload_for` and the
    // `fire_*` fns keep their `crate::hooks::` paths. Generated from the same HOOK_EVENTS rows as
    // the registry arrays above — the catalog cannot drift between alias, dispatch, and array.
    let mut out = String::new();
    out.push_str("// GENERATED by module/build.rs from HOOK_EVENTS. DO NOT EDIT.\n");
    out.push_str(
        "// Included inside src/hooks.rs — see the module doc there for firing semantics.\n",
    );
    out.push_str(
        "/// Event-name -> payload-type aliases so the `game_hook` marker can resolve the handler\n\
         /// signature from the event ident alone. The names ARE the event names, hence the\n\
         /// non-camel-case carve-out.\n",
    );
    out.push_str("#[allow(non_camel_case_types)]\npub mod payload_for {\n");
    for HookEvent {
        event, payload_ty, ..
    } in HOOK_EVENTS
    {
        out.push_str(&format!("    pub type {event} = {payload_ty};\n"));
    }
    out.push_str("}\n");
    for HookEvent {
        event,
        payload_ty,
        actor,
        target,
    } in HOOK_EVENTS
    {
        // Two dispatches per event, in this order. The Rust handlers registered by `game_hook!`
        // are compiled into the build and run first; the Runtime Scripts bound to the event are
        // data, reconciled onto the shard by a Package, and run after. A Package cannot displace
        // engine code by shipping a script.
        out.push_str(&format!(
            "pub(crate) fn fire_{event}(ctx: &spacetimedb::ReducerContext, payload: &{payload_ty}) {{\n    \
                 for f in crate::GAME_HOOKS_{} {{\n        f(ctx, payload);\n    }}\n    \
                 crate::script_binding::fire(ctx, \"{event}\", {actor}, {target});\n}}\n",
            event.to_uppercase()
        ));
    }
    write_out(&out_dir, "hook_dispatch.rs", &out);
}

#[derive(Default)]
struct Registries {
    delete: Vec<String>,
    restamp: Vec<String>,
    transfer: Vec<String>,
    /// The subset of `transfer` registered through the `not_transported` marker kind — the arms
    /// that deliberately carry nothing. Emitted as `CHARACTER_OWNED_NOT_TRANSPORTED` (#380).
    not_transported: Vec<String>,
    tick_passes: Vec<String>,
    hooks: Vec<(String, String)>, // (event, fully-qualified fn path)
    encounter_packages: Vec<(String, String)>, // (binding variant, fully-qualified fn path)
}

/// The table accessor a transport arm's fully-qualified fn path names: the same
/// `sweep_transfer_<table_accessor>` prefix-strip rule the delete sweeps use, so a mover can never
/// be paired with the wrong manifest entry.
fn transfer_table_name(path: &str) -> &str {
    let fn_name = path
        .rsplit("::")
        .next()
        .expect("rsplit yields at least one segment");
    fn_name.strip_prefix("sweep_transfer_").unwrap_or_else(|| {
        panic!(
            "build.rs: `character_owned!(transfer, fn {fn_name}(..))` must be named \
             `sweep_transfer_<table_accessor>` — the transfer payload pairs each mover with its \
             manifest entry by stripping that prefix, so any other spelling would ship rows \
             under a table name that does not exist."
        )
    })
}

fn write_out(out_dir: &str, name: &str, content: &str) {
    let dest = Path::new(out_dir).join(name);
    fs::write(&dest, content)
        .unwrap_or_else(|e| panic!("build.rs: cannot write {}: {e}", dest.display()));
}

/// `my-package` -> `my_package`, validated as a Rust identifier — anything else panics (a package
/// folder name must map cleanly onto the generated `pkg_<name>` module).
fn package_ident(name: &str) -> String {
    let ident: String = name
        .chars()
        .map(|c| if c == '-' { '_' } else { c })
        .collect();
    let valid = !ident.is_empty()
        && ident.chars().next().unwrap().is_ascii_alphabetic()
        && ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        panic!(
            "build.rs: package folder name {name:?} does not map to a valid Rust identifier \
             (want [a-zA-Z][a-zA-Z0-9_-]*)"
        );
    }
    ident
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("build.rs: cannot read dir {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("build.rs: cannot read dir entry: {e}"));
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
}

/// `src/foo.rs` -> `crate::foo`. For a directory module (`src/foo/bar.rs`, `src/foo/mod.rs`), this
/// collapses to just `crate::foo`: every directory module in this crate is a thin facade whose
/// `mod.rs` does `pub use bar::*;` for each of its private submodules (see `items/mod.rs`,
/// `spell/mod.rs`, `combat/mod.rs`, `creatures/mod.rs`) — the submodules themselves (`tables`,
/// `spellbook`, ...) are private, so `crate::foo::bar::sweep_fn` would fail to resolve even though
/// the glob re-export makes `crate::foo::sweep_fn` reachable. That facade re-export is
/// VERIFIED per registered marker (`check_facade_reexport`), not assumed.
fn core_module_path(src_root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(src_root).unwrap_or_else(|_| {
        panic!(
            "build.rs: {} is not under {}",
            file.display(),
            src_root.display()
        )
    });
    let no_ext = rel.with_extension("");
    let segs: Vec<String> = no_ext
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let top = segs
        .first()
        .unwrap_or_else(|| panic!("build.rs: empty path under src/"))
        .clone();
    format!("crate::{top}")
}

struct StrippedSource {
    code: String,
    package_api_exempt_lines: Vec<usize>,
}

/// Blank out comments (line + nested block), string literals (plain, byte, raw), and char
/// literals, PRESERVING newlines and byte-for-char positions — so the marker scan sees only real
/// code and panic line numbers stay true. Lifetimes (`'a`) are left intact (only a real char
/// literal — quote, optional escape, closing quote — is blanked). This is what makes a
/// commented-out marker inert and lets doc comments show real marker syntax.
///
/// Package API exemptions are collected while the scanner knows it is inside a real line comment.
/// Marker text inside a string cannot clear a finding.
fn strip_source(src: &str) -> StrippedSource {
    let b: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(b.len());
    let mut package_api_exempt_lines = Vec::new();
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        // Line comment (also covers /// and //!): blank to end of line.
        if c == '/' && b.get(i + 1) == Some(&'/') {
            let comment_start = i;
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
            }
            let comment: String = b[comment_start..i].iter().collect();
            if comment
                .split_once(PACKAGE_API_EXEMPT)
                .is_some_and(|(_, reason)| !reason.trim().is_empty())
            {
                package_api_exempt_lines.push(
                    b[..comment_start]
                        .iter()
                        .filter(|character| **character == '\n')
                        .count()
                        + 1,
                );
            }
            continue;
        }
        // Block comment, nested per Rust.
        if c == '/' && b.get(i + 1) == Some(&'*') {
            let mut depth = 0usize;
            while i < b.len() {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    out.push(' ');
                    out.push(' ');
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
            }
            continue;
        }
        // Raw string r"..." / r#"..."# / br#"..."# — only when the r/b starts a token (previous
        // char is not part of an identifier), so `for` / `attr` never false-trigger.
        let prev_is_ident = i > 0 && (b[i - 1].is_alphanumeric() || b[i - 1] == '_');
        if !prev_is_ident && (c == 'r' || (c == 'b' && b.get(i + 1) == Some(&'r'))) {
            let r_at = if c == 'b' { i + 1 } else { i };
            let mut j = r_at + 1;
            while b.get(j) == Some(&'#') {
                j += 1;
            }
            if b.get(j) == Some(&'"') {
                let hashes = j - (r_at + 1);
                // Emit the opener as blanks, then scan for `"` + hashes.
                for c in &b[i..=j] {
                    out.push(blank(*c));
                }
                i = j + 1;
                'raw: while i < b.len() {
                    if b[i] == '"' {
                        let mut h = 0usize;
                        while h < hashes && b.get(i + 1 + h) == Some(&'#') {
                            h += 1;
                        }
                        if h == hashes {
                            for c in &b[i..=(i + hashes)] {
                                out.push(blank(*c));
                            }
                            i += hashes + 1;
                            break 'raw;
                        }
                    }
                    out.push(blank(b[i]));
                    i += 1;
                }
                continue;
            }
            // not a raw string — fall through to emit `c` normally below
        }
        // Plain / byte string literal with escapes.
        if c == '"' || (!prev_is_ident && c == 'b' && b.get(i + 1) == Some(&'"')) {
            if c == 'b' {
                out.push(' ');
                i += 1;
            }
            out.push(' '); // the opening quote
            i += 1;
            while i < b.len() {
                if b[i] == '\\' {
                    out.push(' ');
                    if i + 1 < b.len() {
                        out.push(blank(b[i + 1]));
                    }
                    i += 2;
                    continue;
                }
                if b[i] == '"' {
                    out.push(' ');
                    i += 1;
                    break;
                }
                out.push(blank(b[i]));
                i += 1;
            }
            continue;
        }
        // Char literal vs lifetime: a char literal is `'` + (escape | one char) + `'`.
        if c == '\'' {
            let is_char_lit = match b.get(i + 1) {
                Some('\\') => true,
                Some(_) => b.get(i + 2) == Some(&'\''),
                None => false,
            };
            if is_char_lit {
                out.push(' ');
                i += 1;
                if b.get(i) == Some(&'\\') {
                    // Escape: consume the backslash AND its escaped char UNCONDITIONALLY first —
                    // for '\'' the escaped char IS a quote, and a terminator-scan alone would
                    // stop on it one char early, leaking the real closing quote. The tail loop
                    // then covers multi-char escapes ('\u{...}').
                    out.push(' ');
                    i += 1;
                    if i < b.len() {
                        out.push(blank(b[i]));
                        i += 1;
                    }
                    while i < b.len() && b[i] != '\'' {
                        out.push(blank(b[i]));
                        i += 1;
                    }
                } else {
                    out.push(blank(b[i]));
                    i += 1;
                }
                if b.get(i) == Some(&'\'') {
                    out.push(' ');
                    i += 1;
                }
                continue;
            }
            // lifetime — keep the quote so positions stay aligned
        }
        out.push(c);
        i += 1;
    }
    StrippedSource {
        code: out.into_iter().collect(),
        package_api_exempt_lines,
    }
}

fn strip_comments_and_strings(src: &str) -> String {
    strip_source(src).code
}

/// Verify that a marker registered in nested submodule `file` (e.g. `src/spell/spellbook.rs`) is
/// actually re-exported by its facade (`src/spell/mod.rs` or `src/spell.rs`), so the generated
/// `crate::spell::<name>` path resolves. Accepts `pub use <sub>::*`, `pub(crate) use <sub>::*`,
/// and non-glob forms naming `name` (optionally through `self::`). Panics naming the missing
/// re-export — a build.rs panic beats the opaque rustc error inside `$OUT_DIR` it prevents.
fn check_facade_reexport(file: &Path, scan_root: &Path, in_package: bool, name: &str) {
    let rel = match file.strip_prefix(scan_root) {
        Ok(r) => r,
        Err(_) => return,
    };
    let segs: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if rel.file_stem().map(|s| s == "mod").unwrap_or(false) {
        return; // the facade itself
    }
    // Which facade must re-export this file's items depends on the collapse rule:
    // - core `src/foo.rs` -> crate::foo resolves DIRECTLY (no facade); `src/foo/bar.rs` needs
    //   foo's facade (foo/mod.rs or foo.rs) to re-export bar.
    // - a package's EVERY file collapses to crate::pkg_<name>, so even a depth-1 `src/foo.rs`
    //   needs the package's src/mod.rs to re-export foo. Deeper nesting than one directory does
    //   not exist today; if it ever does, the per-level check would need to walk the chain.
    let (facade, sub, collapsed) = if in_package && segs.len() == 1 {
        let sub = rel.file_stem().unwrap().to_string_lossy().into_owned();
        (
            scan_root.join("mod.rs"),
            sub,
            "the package root".to_string(),
        )
    } else if segs.len() == 2 {
        let dir = segs[0].clone();
        let sub = rel.file_stem().unwrap().to_string_lossy().into_owned();
        let facade_mod = scan_root.join(&dir).join("mod.rs");
        let facade_file = scan_root.join(format!("{dir}.rs"));
        let facade = if facade_mod.is_file() {
            facade_mod
        } else if facade_file.is_file() {
            facade_file
        } else {
            panic!(
                "build.rs: marker `{name}` in {} needs a facade module for `{dir}/`, but neither \
                 {dir}/mod.rs nor {dir}.rs exists under {}",
                file.display(),
                scan_root.display()
            );
        };
        (facade, sub, format!("`{dir}`"))
    } else {
        return; // core depth-1 file: crate::foo::name resolves directly
    };
    let content = fs::read_to_string(&facade)
        .unwrap_or_else(|e| panic!("build.rs: cannot read facade {}: {e}", facade.display()));
    let stripped = strip_comments_and_strings(&content);
    // Normalize whitespace so multi-line use statements match.
    let norm: String = stripped.split_whitespace().collect::<Vec<_>>().join(" ");
    for vis in ["pub use ", "pub(crate) use "] {
        for path_head in [format!("{vis}{sub}::"), format!("{vis}self::{sub}::")] {
            let mut from = 0usize;
            while let Some(idx) = norm[from..].find(&path_head) {
                let stmt_start = from + idx + path_head.len();
                let stmt_end = norm[stmt_start..]
                    .find(';')
                    .map(|e| stmt_start + e)
                    .unwrap_or(norm.len());
                let tail = &norm[stmt_start..stmt_end];
                // `*` re-exports everything; otherwise the statement must name the fn (as a path
                // segment or inside a brace list — a substring check bounded by non-ident chars).
                // A bare `*` re-exports everything. Otherwise the statement must name the fn —
                // but `name as other` does NOT count (the original spelling is renamed away;
                // only `other as name`, where `name` is the rename TARGET, keeps it reachable).
                let toks: Vec<&str> = tail
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .filter(|t| !t.is_empty())
                    .collect();
                let names_it = tail.contains('*')
                    || toks.iter().enumerate().any(|(i, t)| {
                        *t == name && toks.get(i + 1).map(|n| *n != "as").unwrap_or(true)
                    });
                if names_it {
                    return;
                }
                from = stmt_end;
            }
        }
    }
    panic!(
        "build.rs: marker fn `{name}` in {} is NOT re-exported by its facade {} — the generated \
         registry path (collapsed to {collapsed}) would not resolve. Add `pub(crate) use \
         {sub}::*;` (or re-export `{name}` explicitly) to the facade.",
        file.display(),
        facade.display()
    );
}

/// The exact two shapes a `character_owned!` invocation head may take (see the macro doc in
/// `src/lib.rs`). A `(kind, name)` match is only recorded for input matching ONE of these; any other
/// occurrence of the literal substring in the file is treated as malformed and panics below.
fn try_match_character_owned(head: &str) -> Option<(&'static str, String)> {
    for kind in ["delete", "restamp", "transfer", "not_transported"] {
        let prefix = format!("({kind},");
        let Some(rest) = head.strip_prefix(prefix.as_str()) else {
            continue;
        };
        let Some(name) = match_fn_name(rest) else {
            continue;
        };
        return Some((kind, name));
    }
    None
}

/// `game_tick_pass!` head: `(fn NAME(...` — one shape only.
fn try_match_tick_pass(head: &str) -> Option<String> {
    let rest = head.strip_prefix('(')?;
    match_fn_name(rest)
}

/// `game_hook!` head: `(EVENT, fn NAME(...` — EVENT must be in the known catalog (checked by the
/// caller so the panic can list the valid names).
fn try_match_hook(head: &str) -> Option<(String, String)> {
    let rest = head.strip_prefix('(')?.trim_start();
    let ev_end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    if ev_end == 0 {
        return None;
    }
    let event = rest[..ev_end].to_string();
    let rest = rest[ev_end..].trim_start().strip_prefix(',')?;
    let name = match_fn_name(rest)?;
    Some((event, name))
}

/// `encounter_package!` head: `(BINDING, fn NAME(...` — package files only.
fn try_match_encounter_package(head: &str) -> Option<(String, String)> {
    let rest = head.strip_prefix('(')?.trim_start();
    let binding_end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    if binding_end == 0 {
        return None;
    }
    let binding = rest[..binding_end].to_string();
    let rest = rest[binding_end..].trim_start().strip_prefix(',')?;
    let name = match_fn_name(rest)?;
    Some((binding, name))
}

/// Shared tail matcher: optional whitespace, `fn NAME`, then (after optional whitespace) `(` — the
/// fn's own param list.
fn match_fn_name(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("fn ")?;
    let rest = rest.trim_start();
    let name_end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    if name_end == 0 {
        return None;
    }
    let name = &rest[..name_end];
    if rest[name_end..].trim_start().starts_with('(') {
        Some(name.to_string())
    } else {
        None
    }
}

/// Scan one file for all four marker kinds, registering each hit under `prefix` (the file's
/// collapsed crate path). The scan runs on the comment/string-stripped text, so quoted or
/// commented-out marker syntax is inert; on real code, any occurrence of a marker's literal
/// substring that doesn't parse panics — never skip silently. Every registered marker in a nested
/// submodule also has its facade re-export verified (`check_facade_reexport`).
fn scan_file(file: &Path, scan_root: &Path, in_package: bool, prefix: &str, reg: &mut Registries) {
    let raw = fs::read_to_string(file)
        .unwrap_or_else(|e| panic!("build.rs: cannot read {}: {e}", file.display()));
    let content = strip_comments_and_strings(&raw);

    scan_marker(
        &content,
        file,
        "character_owned!",
        |head, line| match try_match_character_owned(head) {
            Some((kind, name)) => {
                check_facade_reexport(file, scan_root, in_package, &name);
                let path = format!("{prefix}::{name}");
                match kind {
                    "delete" => reg.delete.push(path),
                    "restamp" => reg.restamp.push(path),
                    "transfer" => reg.transfer.push(path),
                    // A DECLINING arm is still a transport arm — it is registered in the same
                    // registry, so `every_manifest_table_can_cross_a_database_boundary` keeps
                    // seeing an arm for the table — but it is ALSO recorded separately, because
                    // "these rows deliberately do not cross" is a decision that must be
                    // cross-checkable against `transfer::NOT_TRANSPORTED`'s written reasons
                    // instead of being read back out of the arm's source text (#380).
                    // A PACKAGE's decline is registered as a transport arm like any other, but it
                    // is NOT cross-checked against `transfer::NOT_TRANSPORTED`. That list is the
                    // core's written decision about core tables, and it cannot name a table that
                    // is absent from most builds — a Package is a drop-in. A Package writes its
                    // reason where the reader looks for it: at its own table.
                    "not_transported" => {
                        reg.transfer.push(path.clone());
                        if !in_package {
                            reg.not_transported.push(path);
                        }
                    }
                    _ => unreachable!(),
                }
            }
            None => panic!(
                "build.rs: malformed `character_owned!` marker in {}:{line} — expected exactly \
                 `character_owned!(delete, fn NAME(ctx, character_guid) {{ .. }})`, the 3-arg \
                 `restamp` form, the declarative `transfer` form, or \
                 `character_owned!(not_transported, fn NAME())` (see the macro doc in src/lib.rs). \
                 A marker must never be silently skipped.",
                file.display()
            ),
        },
    );

    scan_marker(
        &content,
        file,
        "game_tick_pass!",
        |head, line| match try_match_tick_pass(head) {
            Some(name) => {
                check_facade_reexport(file, scan_root, in_package, &name);
                reg.tick_passes.push(format!("{prefix}::{name}"));
            }
            None => panic!(
                "build.rs: malformed `game_tick_pass!` marker in {}:{line} — expected exactly \
                 `game_tick_pass!(fn NAME(ctx) {{ .. }})` (see the macro doc in src/lib.rs). A \
                 marker must never be silently skipped.",
                file.display()
            ),
        },
    );

    scan_marker(
        &content,
        file,
        "game_hook!",
        |head, line| match try_match_hook(head) {
            Some((event, name)) => {
                if !HOOK_EVENTS.iter().any(|h| h.event == event) {
                    panic!(
                        "build.rs: `game_hook!` in {}:{line} names unknown event {event:?} — known \
                         events: {:?}. Extending the catalog means adding the payload struct + \
                         dispatch site in src/hooks.rs and the HOOK_EVENTS row in module/build.rs \
                         (payload_for aliases and fire_* fns are generated from that row).",
                        file.display(),
                        HOOK_EVENTS.iter().map(|h| h.event).collect::<Vec<_>>()
                    );
                }
                check_facade_reexport(file, scan_root, in_package, &name);
                reg.hooks.push((event, format!("{prefix}::{name}")));
            }
            None => panic!(
                "build.rs: malformed `game_hook!` marker in {}:{line} — expected exactly \
                 `game_hook!(EVENT, fn NAME(ctx, payload) {{ .. }})` (see the macro doc in \
                 src/lib.rs). A marker must never be silently skipped.",
                file.display()
            ),
        },
    );

    scan_marker(
        &content,
        file,
        "encounter_package!",
        |head, line| match try_match_encounter_package(head) {
            Some((binding, name)) => {
                if !in_package {
                    panic!(
                        "build.rs: `encounter_package!` in {}:{line} is core code; encounter authority must live under packages/*/src/",
                        file.display()
                    );
                }
                check_facade_reexport(file, scan_root, in_package, &name);
                reg.encounter_packages
                    .push((binding, format!("{prefix}::{name}")));
            }
            None => panic!(
                "build.rs: malformed `encounter_package!` marker in {}:{line} — expected exactly \
                 `encounter_package!(BINDING, fn NAME(ctx, instance_id, signal) {{ .. }})`. A \
                 marker must never be silently skipped.",
                file.display()
            ),
        },
    );
}

/// Whether `root`, the first segment of a crate-root path, is on the Package API surface.
///
/// Three families beyond the listed roots: `game_*` covers the table accessor traits and the
/// `game_hook!`/`game_tick_pass!` markers, `pkg_*` covers a Package's own generated root (and its
/// siblings'), and an UpperCamelCase name is a row or payload type re-exported at the crate root.
fn on_package_api(root: &str) -> bool {
    let upper_camel = root.starts_with(|c: char| c.is_ascii_uppercase())
        && root.chars().any(|c| c.is_ascii_lowercase());
    PACKAGE_API_ROOTS.contains(&root)
        || PACKAGE_API_ROOT_ITEMS.contains(&root)
        || root.starts_with("game_")
        || root.starts_with("pkg_")
        || upper_camel
}

#[derive(Clone, Copy)]
struct SourceToken<'a> {
    text: &'a str,
    start: usize,
}

/// The Rust tokens this lint needs, with byte positions for diagnostics. This is deliberately much
/// smaller than a parser: identifiers, `::`, and punctuation stay distinct; whitespace vanishes.
fn source_tokens(source: &str) -> Vec<SourceToken<'_>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0usize;
    while at < bytes.len() {
        if bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }
        let start = at;
        if bytes[at] == b'r'
            && bytes.get(at + 1) == Some(&b'#')
            && bytes
                .get(at + 2)
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            at += 3;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
        } else if bytes[at].is_ascii_alphabetic() || bytes[at] == b'_' {
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
        } else if bytes[at] == b':' && bytes.get(at + 1) == Some(&b':') {
            at += 2;
        } else {
            at += source[at..]
                .chars()
                .next()
                .expect("at is before the end of source")
                .len_utf8();
        }
        tokens.push(SourceToken {
            text: &source[start..at],
            start,
        });
    }
    tokens
}

fn is_ident(token: SourceToken<'_>) -> bool {
    token
        .text
        .as_bytes()
        .first()
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
}

fn ident_name(token: SourceToken<'_>) -> Option<&str> {
    is_ident(token).then(|| token.text.strip_prefix("r#").unwrap_or(token.text))
}

/// Every matched brace pair in the stripped source, as byte positions of the braces.
fn brace_pairs(tokens: &[SourceToken<'_>]) -> Vec<(usize, usize)> {
    let mut open = Vec::new();
    let mut pairs = Vec::new();
    for token in tokens {
        match token.text {
            "{" => open.push(token.start),
            "}" => {
                if let Some(start) = open.pop() {
                    pairs.push((start, token.start));
                }
            }
            _ => {}
        }
    }
    pairs.sort_unstable();
    pairs
}

/// Inline `mod name { .. }` bodies. File position supplies the rest of a Package module's depth.
fn inline_modules(tokens: &[SourceToken<'_>], pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut modules = Vec::new();
    for window in tokens.windows(3) {
        if window[0].text != "mod" || !is_ident(window[1]) || window[2].text != "{" {
            continue;
        }
        if let Some(pair) = pairs.iter().find(|(open, _)| *open == window[2].start) {
            modules.push(*pair);
        }
    }
    modules.sort_unstable();
    modules
}

fn inline_module_stack(modules: &[(usize, usize)], at: usize) -> Vec<usize> {
    modules
        .iter()
        .enumerate()
        .filter_map(|(index, (open, close))| (*open < at && at < *close).then_some(index))
        .collect()
}

fn module_depth(file_depth: usize, modules: &[(usize, usize)], at: usize) -> usize {
    file_depth + inline_module_stack(modules, at).len()
}

/// `mod.rs` is its directory module; any other file adds its stem as one module segment.
fn package_file_module_depth(package_src: &Path, file: &Path) -> usize {
    let relative = file.strip_prefix(package_src).unwrap_or_else(|_| {
        panic!(
            "build.rs: {} is not under Package source {}",
            file.display(),
            package_src.display()
        )
    });
    let components = relative.components().count();
    if relative.file_name().is_some_and(|name| name == "mod.rs") {
        components.saturating_sub(1)
    } else {
        components
    }
}

/// Alias names introduced for the whole crate by one `crate` use-tree root. The direct form is
/// `crate as core`; the grouped form is `crate::{self as core}`.
fn aliases_after_use_root(
    tokens: &[SourceToken<'_>],
    root: usize,
    statement_end: usize,
) -> Vec<usize> {
    if root + 2 < statement_end
        && tokens[root + 1].text == "as"
        && is_ident(tokens[root + 2])
        && tokens[root + 2].text != "_"
    {
        return vec![root + 2];
    }
    if root + 2 >= statement_end || tokens[root + 1].text != "::" || tokens[root + 2].text != "{" {
        return Vec::new();
    }

    let mut aliases = Vec::new();
    let mut depth = 0usize;
    let mut at_entry = true;
    let mut index = root + 3;
    while index < statement_end {
        match tokens[index].text {
            "{" => depth += 1,
            "}" if depth == 0 => break,
            "}" => depth -= 1,
            "," if depth == 0 => at_entry = true,
            "self" if depth == 0 && at_entry => {
                if index + 2 < statement_end
                    && tokens[index + 1].text == "as"
                    && is_ident(tokens[index + 2])
                    && tokens[index + 2].text != "_"
                {
                    aliases.push(index + 2);
                }
                at_entry = false;
            }
            _ if depth == 0 && at_entry => at_entry = false,
            _ => {}
        }
        index += 1;
    }
    aliases
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnsupportedPackageSyntax {
    WholeCrateAlias,
    PathAttribute,
    IncludeMacro,
}

/// Syntax whose meaning the Package API lint cannot track reliably. Refuse it where it is
/// declared instead of guessing at Rust name resolution or filesystem-to-module mapping.
fn unsupported_package_syntax(
    tokens: &[SourceToken<'_>],
    stripped: &str,
) -> Vec<(usize, usize, UnsupportedPackageSyntax, String)> {
    let mut found = Vec::new();
    for (use_index, token) in tokens.iter().enumerate() {
        if token.text != "use" {
            continue;
        }
        let statement_end = tokens[use_index + 1..]
            .iter()
            .position(|token| token.text == ";")
            .map_or(tokens.len(), |offset| use_index + 1 + offset);
        let mut index = use_index + 1;
        while index < statement_end {
            if tokens[index].text == "crate" {
                for name_index in aliases_after_use_root(tokens, index, statement_end) {
                    let offset = tokens[index].start;
                    found.push((
                        offset,
                        0,
                        UnsupportedPackageSyntax::WholeCrateAlias,
                        format!("crate alias `{}`", tokens[name_index].text),
                    ));
                }
            }
            index += 1;
        }
    }

    // `extern crate self as core` is the older spelling of the same file-local crate alias.
    for window in tokens.windows(5) {
        if window[0].text == "extern"
            && window[1].text == "crate"
            && window[2].text == "self"
            && window[3].text == "as"
            && is_ident(window[4])
            && window[4].text != "_"
        {
            found.push((
                window[0].start,
                0,
                UnsupportedPackageSyntax::WholeCrateAlias,
                format!("crate alias `{}`", window[4].text),
            ));
        }
    }

    for (index, window) in tokens.windows(3).enumerate() {
        if window[0].text != "#" || window[1].text != "[" {
            continue;
        }
        let Some(attribute_end) = tokens[index + 2..]
            .iter()
            .position(|token| token.text == "]")
            .map(|offset| index + 2 + offset)
        else {
            continue;
        };
        let attribute = &tokens[index + 2..attribute_end];
        let is_path = attribute.first().is_some_and(|token| {
            ident_name(*token) == Some("path")
                && attribute.get(1).is_some_and(|token| token.text == "=")
        });
        let is_conditional_path = attribute.first().is_some_and(|token| {
            ident_name(*token) == Some("cfg_attr")
                && attribute
                    .windows(2)
                    .any(|pair| ident_name(pair[0]) == Some("path") && pair[1].text == "=")
        });
        if is_path || is_conditional_path {
            found.push((
                window[0].start,
                0,
                UnsupportedPackageSyntax::PathAttribute,
                if is_path {
                    "`#[path]`".to_string()
                } else {
                    "`#[cfg_attr(..., path = ...)]`".to_string()
                },
            ));
        }
    }

    // `include!` parses another file as Rust in this module. The lint only discovers `.rs` files,
    // so following it would require a second source-discovery rule. Refuse code inclusion while
    // keeping data inclusion (`include_str!` and `include_bytes!`) available to Packages.
    for window in tokens.windows(2) {
        if ident_name(window[0]) == Some("include") && window[1].text == "!" {
            found.push((
                window[0].start,
                0,
                UnsupportedPackageSyntax::IncludeMacro,
                "`include!`".to_string(),
            ));
        }
    }

    for finding in &mut found {
        finding.1 = stripped[..finding.0].matches('\n').count() + 1;
    }
    found.sort_by_key(|finding| finding.0);
    found.dedup_by(|left, right| left.0 == right.0 && left.2 == right.2);
    found
}

/// The root and diagnostic spelling of every path introduced after one `::`. A braced group emits
/// one result per entry at its first depth; a glob is one forbidden root of `*`.
fn roots_after_separator(
    tokens: &[SourceToken<'_>],
    separator: usize,
    prefix: &str,
) -> Vec<(usize, String, String)> {
    let Some(first) = tokens.get(separator + 1) else {
        return Vec::new();
    };
    if first.text != "{" {
        if !is_ident(*first) && first.text != "*" {
            return Vec::new();
        }
        let mut root_index = separator + 1;
        let mut root = first.text;
        let mut written = format!("{prefix}::{root}");
        if root == "self"
            && tokens
                .get(root_index + 1)
                .is_some_and(|token| token.text == "::")
            && tokens
                .get(root_index + 2)
                .is_some_and(|token| is_ident(*token))
        {
            root_index += 2;
            root = tokens[root_index].text;
            written.push_str("::");
            written.push_str(root);
        }
        let mut end = root_index;
        while tokens.get(end + 1).is_some_and(|token| token.text == "::")
            && tokens.get(end + 2).is_some_and(|token| is_ident(*token))
        {
            end += 2;
            written.push_str("::");
            written.push_str(tokens[end].text);
        }
        return vec![(root_index, root.to_string(), written)];
    }

    let mut roots = Vec::new();
    let mut depth = 0usize;
    let mut at_entry = true;
    let mut index = separator + 2;
    while index < tokens.len() {
        match tokens[index].text {
            "{" => depth += 1,
            "}" if depth == 0 => break,
            "}" => depth -= 1,
            "," if depth == 0 => at_entry = true,
            _ if depth == 0 && at_entry => {
                at_entry = false;
                if tokens[index].text == "self" {
                    if tokens
                        .get(index + 1)
                        .is_some_and(|token| token.text == "::")
                        && tokens.get(index + 2).is_some_and(|token| is_ident(*token))
                    {
                        let root = tokens[index + 2].text.to_string();
                        roots.push((index + 2, root.clone(), format!("{prefix}::self::{root}")));
                    }
                } else if is_ident(tokens[index]) || tokens[index].text == "*" {
                    roots.push((
                        index,
                        tokens[index].text.to_string(),
                        format!("{prefix}::{}", tokens[index].text),
                    ));
                }
            }
            _ => {}
        }
        index += 1;
    }
    roots
}

fn record_rooted_paths(
    tokens: &[SourceToken<'_>],
    separator: usize,
    prefix: &str,
    stripped: &str,
    exempt_lines: &[usize],
    found: &mut Vec<(usize, usize, String)>,
) {
    for (root_index, root, written) in roots_after_separator(tokens, separator, prefix) {
        let normalized_root = root.strip_prefix("r#").unwrap_or(&root);
        if normalized_root == "self" || on_package_api(normalized_root) {
            continue;
        }
        let offset = tokens[root_index].start;
        let line = stripped[..offset].matches('\n').count() + 1;
        if !exempt_lines.contains(&line) {
            found.push((offset, line, written));
        }
    }
}

/// Every Package path in `source` that reaches a crate root outside the Package API surface, as
/// (1-based line, path as written). `file_depth` is zero for `src/mod.rs`, one for `src/foo.rs` or
/// `src/foo/mod.rs`, and so on. Inline modules add to it at the occurrence.
///
/// The scan runs on the comment- and string-stripped copy. It recognizes `crate`, `$crate`, and
/// enough leading `super` segments to leave the Package. The exemption lines come from real line
/// comments recorded while stripping source.
fn out_of_surface_paths(source: &str, file_depth: usize) -> Vec<(usize, String)> {
    let stripped_source = strip_source(source);
    let stripped = stripped_source.code;
    let tokens = source_tokens(&stripped);
    let pairs = brace_pairs(&tokens);
    let modules = inline_modules(&tokens, &pairs);
    let mut found = Vec::new();

    let mut index = 0usize;
    while index < tokens.len() {
        if tokens[index].text == "$"
            && tokens
                .get(index + 1)
                .is_some_and(|token| token.text == "crate")
            && tokens
                .get(index + 2)
                .is_some_and(|token| token.text == "::")
        {
            record_rooted_paths(
                &tokens,
                index + 2,
                "$crate",
                &stripped,
                &stripped_source.package_api_exempt_lines,
                &mut found,
            );
            index += 3;
            continue;
        }
        if tokens[index].text == "crate"
            && (index == 0 || tokens[index - 1].text != "$")
            && tokens
                .get(index + 1)
                .is_some_and(|token| token.text == "::")
        {
            record_rooted_paths(
                &tokens,
                index + 1,
                "crate",
                &stripped,
                &stripped_source.package_api_exempt_lines,
                &mut found,
            );
        } else if tokens[index].text == "super" && (index == 0 || tokens[index - 1].text != "::") {
            let mut levels = 1usize;
            let mut root_end = index;
            while tokens
                .get(root_end + 1)
                .is_some_and(|token| token.text == "::")
                && tokens
                    .get(root_end + 2)
                    .is_some_and(|token| token.text == "super")
            {
                levels += 1;
                root_end += 2;
            }
            if levels == module_depth(file_depth, &modules, tokens[index].start) + 1
                && tokens
                    .get(root_end + 1)
                    .is_some_and(|token| token.text == "::")
            {
                let prefix = std::iter::repeat_n("super", levels)
                    .collect::<Vec<_>>()
                    .join("::");
                record_rooted_paths(
                    &tokens,
                    root_end + 1,
                    &prefix,
                    &stripped,
                    &stripped_source.package_api_exempt_lines,
                    &mut found,
                );
            }
            index = root_end;
        }
        index += 1;
    }

    found.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.2.cmp(&right.2)));
    found.dedup_by(|left, right| left.0 == right.0 && left.2 == right.2);
    found
        .into_iter()
        .map(|(_, line, path)| (line, path))
        .collect()
}

/// What the build says when a Package names a core path outside the surface.
fn out_of_surface_message(package: &str, file: &Path, line: usize, path: &str) -> String {
    format!(
        "build.rs: Package `{package}` names `{path}` at {}:{line}, which is outside the Package \
         API surface (docs/package-api.md, version 1). Use a path under a documented root, or, if \
         the Package genuinely needs this one, write `{PACKAGE_API_EXEMPT} <reason>` on that line \
         and raise the gap with the maintainers.",
        file.display()
    )
}

fn unsupported_package_syntax_message(
    package: &str,
    file: &Path,
    line: usize,
    kind: UnsupportedPackageSyntax,
    written: &str,
) -> String {
    let instruction = match kind {
        UnsupportedPackageSyntax::WholeCrateAlias => {
            "Whole-crate aliases can hide core paths across Rust scopes. Spell each core path as \
             `crate::<Package API root>` instead."
        }
        UnsupportedPackageSyntax::PathAttribute => {
            "Path attributes break Package source discovery and module-depth checks. Use Rust's \
             normal `mod.rs`, `<name>.rs`, or `<name>/mod.rs` layout instead."
        }
        UnsupportedPackageSyntax::IncludeMacro => {
            "`include!` can add Rust source that the Package API lint cannot discover. Put Package \
             Rust in a normal `.rs` source file instead."
        }
    };
    format!(
        "build.rs: Package `{package}` uses unsupported Package API syntax {written} at \
         {}:{line}. {instruction}",
        file.display()
    )
}

/// Fail the build for the first out-of-surface path in one Package file. Core `src/` is never
/// linted: the surface is a promise core makes to Packages, not to itself.
fn lint_package_api(package: &str, package_src: &Path, file: &Path) {
    let source = fs::read_to_string(file)
        .unwrap_or_else(|e| panic!("build.rs: cannot read {}: {e}", file.display()));
    let stripped = strip_comments_and_strings(&source);
    let tokens = source_tokens(&stripped);
    if let Some((_, line, kind, written)) = unsupported_package_syntax(&tokens, &stripped).first() {
        panic!(
            "{}",
            unsupported_package_syntax_message(package, file, *line, *kind, written)
        );
    }
    let file_depth = package_file_module_depth(package_src, file);
    if let Some((line, path)) = out_of_surface_paths(&source, file_depth).first() {
        panic!("{}", out_of_surface_message(package, file, *line, path));
    }
}

/// Find every occurrence of `marker` in `content` and hand its head (text after the marker, left-
/// trimmed) plus 1-based line number to `on_hit`.
fn scan_marker(content: &str, _file: &Path, marker: &str, mut on_hit: impl FnMut(&str, usize)) {
    let mut search_from = 0usize;
    while let Some(rel_idx) = content[search_from..].find(marker) {
        let idx = search_from + rel_idx;
        let head_start = idx + marker.len();
        let head = content[head_start..].trim_start();
        let line = content[..idx].matches('\n').count() + 1;
        on_hit(head, line);
        search_from = head_start;
    }
}

#[cfg(test)]
mod package_api_lint_tests {
    use super::*;

    fn reported(source: &str) -> Vec<String> {
        reported_at_depth(source, 0)
    }

    fn reported_at_depth(source: &str, file_depth: usize) -> Vec<String> {
        out_of_surface_paths(source, file_depth)
            .into_iter()
            .map(|(line, path)| format!("{line}:{path}"))
            .collect()
    }

    fn unsupported(source: &str) -> Vec<String> {
        let stripped = strip_comments_and_strings(source);
        let tokens = source_tokens(&stripped);
        unsupported_package_syntax(&tokens, &stripped)
            .into_iter()
            .map(|(_, line, _, written)| format!("{line}:{written}"))
            .collect()
    }

    #[test]
    fn a_documented_root_and_its_depth_are_on_the_surface() {
        let source = "use crate::helpers::live_entity;\nfn f(e: &crate::WorldEntity) {\n    crate::creatures::tick::emit_creature_leg(e);\n    crate::game_hook!(a);\n}\n";
        assert!(reported(source).is_empty(), "{:?}", reported(source));
    }

    #[test]
    fn an_undocumented_root_is_reported_with_its_line_and_path() {
        let source = "fn f() {\n    let _ = 1;\n    crate::auth::create_character();\n}\n";
        assert_eq!(reported(source), vec!["3:crate::auth::create_character"]);
    }

    #[test]
    fn the_failure_names_the_package_the_file_the_line_and_the_path() {
        let message = out_of_surface_message(
            "playerbots",
            Path::new("packages/playerbots/src/mod.rs"),
            618,
            "crate::auth::create_character",
        );
        assert!(message.contains("playerbots"), "{message}");
        assert!(
            message.contains("packages/playerbots/src/mod.rs:618"),
            "{message}"
        );
        assert!(
            message.contains("crate::auth::create_character"),
            "{message}"
        );
        assert!(message.contains("docs/package-api.md"), "{message}");
    }

    #[test]
    fn an_exemption_clears_its_own_line_and_no_other() {
        let source = "fn f() {\n    crate::auth::create_character(); // package-api: exempt bots fabricate their own characters\n    crate::auth::Account::default();\n}\n";
        assert_eq!(reported(source), vec!["3:crate::auth::Account::default"]);
    }

    #[test]
    fn an_exemption_without_a_reason_does_not_clear() {
        let source = "crate::auth::Account; // package-api: exempt\n";
        assert_eq!(reported(source), vec!["1:crate::auth::Account"]);
    }

    #[test]
    fn a_braced_import_is_read_entry_by_entry() {
        let source = "use crate::{\n    game_world_entity,\n    auth::Account,\n    quest::{objective_kind, quest_role},\n    test_scan,\n};\n";
        assert_eq!(
            reported(source),
            vec!["3:crate::auth", "5:crate::test_scan"]
        );
    }

    #[test]
    fn a_path_in_a_comment_or_a_string_is_inert() {
        let source =
            "// crate::auth::create_character is core's own\nlet s = \"crate::test_scan\";\n";
        assert!(reported(source).is_empty(), "{:?}", reported(source));
    }

    #[test]
    fn another_crates_path_is_not_this_crates_path() {
        let source = "use lyracore_crate::auth::Account;\n";
        assert!(reported(source).is_empty(), "{:?}", reported(source));
    }

    #[test]
    fn a_relative_path_that_leaves_the_package_root_is_reported() {
        let source = "use super::auth::Account;\n";
        assert_eq!(reported(source), vec!["1:super::auth::Account"]);
    }

    #[test]
    fn only_enough_super_segments_to_leave_the_package_are_linted() {
        let source = "mod tests {\n    use super::super::package_sibling;\n    use super::super::super::auth::Account;\n}\n";
        assert_eq!(
            reported_at_depth(source, 1),
            vec!["3:super::super::super::auth::Account"]
        );
    }

    #[test]
    fn package_relative_siblings_and_submodules_stay_inside_the_package() {
        let source =
            "use super::sibling;\nmod tests {\n    use super::super::package_root_item;\n}\n";
        assert!(
            reported_at_depth(source, 1).is_empty(),
            "{:?}",
            reported_at_depth(source, 1)
        );
    }

    #[test]
    fn a_whole_crate_alias_is_unsupported_before_it_can_hide_a_path() {
        let source = "use crate as core;\nuse core::auth::Account;\n";
        assert_eq!(unsupported(source), vec!["1:crate alias `core`"]);
        assert!(reported(source).is_empty(), "{:?}", reported(source));
    }

    #[test]
    fn a_package_alias_named_core_is_not_reclassified_as_the_crate() {
        let source =
            "mod owned { pub mod auth {} }\nfn f() { use self::owned as core; use core::auth; }\n";
        assert!(unsupported(source).is_empty(), "{:?}", unsupported(source));
        assert!(reported(source).is_empty(), "{:?}", reported(source));
    }

    #[test]
    fn grouped_whole_crate_aliases_are_unsupported() {
        let source = "use crate::{self as core, helpers};\nuse core::auth;\n";
        assert_eq!(unsupported(source), vec!["1:crate alias `core`"]);
    }

    #[test]
    fn every_whole_crate_alias_spelling_is_unsupported() {
        let source =
            "use {crate as first};\nuse crate::{self as second};\nextern crate self as third;\n";
        assert_eq!(
            unsupported(source),
            vec![
                "1:crate alias `first`",
                "2:crate alias `second`",
                "3:crate alias `third`"
            ]
        );
    }

    #[test]
    fn raw_whole_crate_aliases_are_unsupported() {
        let source = "use crate as r#core;\nuse r#core::auth;\n";
        assert_eq!(unsupported(source), vec!["1:crate alias `r#core`"]);
        assert!(reported(source).is_empty(), "{:?}", reported(source));
    }

    #[test]
    fn crate_root_globs_are_outside_the_named_surface() {
        let source = "use crate::*;\nuse super::*;\n";
        assert_eq!(reported(source), vec!["1:crate::*", "2:super::*"]);
    }

    #[test]
    fn dollar_crate_and_whitespace_around_separators_do_not_bypass_the_lint() {
        let source = "macro_rules! hidden { () => { $crate::auth::Account } }\nuse crate /* gap */ :: test_scan;\n";
        assert_eq!(
            reported(source),
            vec!["1:$crate::auth::Account", "2:crate::test_scan"]
        );
    }

    #[test]
    fn an_exemption_remains_line_local_for_relative_and_direct_paths() {
        let source = "use super::auth; // package-api: exempt Package setup needs Accounts\nuse crate::test_scan;\n";
        assert_eq!(reported(source), vec!["2:crate::test_scan"]);
    }

    #[test]
    fn an_exemption_cannot_enable_a_whole_crate_alias() {
        let source = "use crate as core; // package-api: exempt legacy spelling\n";
        assert_eq!(unsupported(source), vec!["1:crate alias `core`"]);
    }

    #[test]
    fn marker_text_inside_a_string_is_not_an_exemption() {
        let source = "let note = \"// package-api: exempt not a comment\"; crate::auth::Account;\n";
        assert_eq!(reported(source), vec!["1:crate::auth::Account"]);
    }

    #[test]
    fn raw_identifiers_are_normalized_for_the_package_api() {
        let source = "use crate::r#helpers::live_entity;\nuse crate::r#auth::Account;\n";
        assert_eq!(reported(source), vec!["2:crate::r#auth::Account"]);
    }

    #[test]
    fn a_raw_inline_module_counts_toward_relative_depth() {
        let source = "mod r#nested { use super::package_item; use super::super::auth::Account; }\n";
        assert_eq!(reported(source), vec!["1:super::super::auth::Account"]);
    }

    #[test]
    fn path_attributes_are_unsupported_even_when_conditional_or_exempted() {
        let source = "#[path = \"layout/hidden.rs\"] // package-api: exempt legacy layout\nmod hidden;\n#[cfg_attr(unix, path = \"unix.rs\")]\nmod platform;\n";
        assert_eq!(
            unsupported(source),
            vec!["1:`#[path]`", "3:`#[cfg_attr(..., path = ...)]`"]
        );
    }

    #[test]
    fn include_cannot_inject_an_unblessed_path_from_a_non_rust_file() {
        let package_src = std::env::temp_dir().join(format!(
            "lyracore-package-api-include-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the system clock is after the Unix epoch")
                .as_nanos(),
        ));
        fs::create_dir_all(&package_src).expect("create temporary Package source");
        let module = package_src.join("mod.rs");
        fs::write(
            &module,
            "include /* whitespace is allowed */ ! (\"private.inc\");\n",
        )
        .expect("write Package module");
        fs::write(
            package_src.join("private.inc"),
            "crate::auth::Account::default();\n",
        )
        .expect("write included Package source");

        let failure = std::panic::catch_unwind(|| lint_package_api("bots", &package_src, &module))
            .expect_err("include! must fail before a non-.rs file can add a core path");
        let _ = fs::remove_dir_all(&package_src);
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .expect("lint panic has a message");
        assert!(message.contains("`include!`"), "{message}");
        assert!(message.contains("Package `bots`"), "{message}");
    }

    #[test]
    fn raw_or_spaced_include_names_cannot_bypass_the_refusal() {
        assert_eq!(
            unsupported("r#include ! (\"private.inc\");\ninclude /* gap */ ! (\"hidden.inc\");"),
            vec!["1:`include!`", "2:`include!`"]
        );
    }

    #[test]
    fn data_include_macros_remain_available() {
        assert!(
            unsupported("let text = include_str!(\"fixture.txt\");\nlet bytes = include_bytes!(\"fixture.bin\");")
                .is_empty()
        );
    }

    #[test]
    fn unsupported_syntax_diagnostics_tell_the_author_what_to_write() {
        let alias = unsupported_package_syntax_message(
            "bots",
            Path::new("packages/bots/src/mod.rs"),
            3,
            UnsupportedPackageSyntax::WholeCrateAlias,
            "crate alias `core`",
        );
        assert!(alias.contains("packages/bots/src/mod.rs:3"), "{alias}");
        assert!(alias.contains("`crate::<Package API root>`"), "{alias}");
        assert!(!alias.contains(PACKAGE_API_EXEMPT), "{alias}");

        let path = unsupported_package_syntax_message(
            "bots",
            Path::new("packages/bots/src/mod.rs"),
            5,
            UnsupportedPackageSyntax::PathAttribute,
            "`#[path]`",
        );
        assert!(path.contains("packages/bots/src/mod.rs:5"), "{path}");
        assert!(path.contains("normal `mod.rs`"), "{path}");
        assert!(!path.contains(PACKAGE_API_EXEMPT), "{path}");

        let include = unsupported_package_syntax_message(
            "bots",
            Path::new("packages/bots/src/mod.rs"),
            7,
            UnsupportedPackageSyntax::IncludeMacro,
            "`include!`",
        );
        assert!(include.contains("packages/bots/src/mod.rs:7"), "{include}");
        assert!(include.contains("normal `.rs` source file"), "{include}");
        assert!(!include.contains(PACKAGE_API_EXEMPT), "{include}");
    }
}
