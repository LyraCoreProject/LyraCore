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
//! - `package_registries.rs` — from `game_tick_pass!` and `game_hook!` markers:
//!   `GAME_TICK_PASSES` (periodic passes run by the core scheduler tick) and one
//!   `GAME_HOOKS_<EVENT>` array per known notify-hook event, dispatched at the core chokepoints
//!   (see `src/hooks.rs`).
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

/// The notify-hook event catalog: event name -> the payload type the
/// handler receives (the struct lives in `src/hooks.rs`). This row is HALF of an event's
/// definition; the payload struct is the other half. From these rows build.rs generates the
/// per-event registry array (`package_registries.rs`) AND the `payload_for` alias + `fire_*`
/// dispatch fn (`hook_dispatch.rs`) — so adding an event is: payload struct in hooks.rs, one row
/// here, plus the dispatch call at the new core chokepoint. A `game_hook!` naming any other event
/// panics below with this list.
const HOOK_EVENTS: &[(&str, &str)] = &[
    ("on_damage_taken", "crate::hooks::DamageTakenPayload"),
    ("on_creature_spawn", "crate::hooks::CreatureSpawnPayload"),
    ("on_levelup", "crate::hooks::LevelupPayload"),
    ("on_group_invite", "crate::hooks::GroupInvitePayload"),
    ("on_death", "crate::hooks::DeathPayload"),
    ("on_kill", "crate::hooks::KillPayload"),
    ("on_aggro", "crate::hooks::AggroPayload"),
    ("on_cast_resolved", "crate::hooks::CastResolvedPayload"),
    ("on_loot", "crate::hooks::LootPayload"),
    ("on_quest_accept", "crate::hooks::QuestAcceptPayload"),
    ("on_quest_turnin", "crate::hooks::QuestTurninPayload"),
    ("on_login", "crate::hooks::LoginPayload"),
    ("on_logout", "crate::hooks::LogoutPayload"),
    ("on_gossip_select", "crate::hooks::GossipSelectPayload"),
    // Work-item 228 (encounter kernel): entry-keyed creature death, once-per-instance HP-threshold
    // crossings (fired by encounter::encounter_hp_probe, not a new core chokepoint), and GO use.
    ("on_creature_death", "crate::hooks::CreatureDeathPayload"),
    ("on_hp_threshold", "crate::hooks::HpThresholdPayload"),
    ("on_go_used", "crate::hooks::GoUsedPayload"),
];

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
                // A CLIENT-ONLY package (client/ with no src/ — addons/FrameXML shipped by
                // --pack-client) is a legal, quiet shape. A package with NEITHER is probably a
                // typo'd `scr/` folder — warn loudly.
                if !pkg.join("client").is_dir() {
                    println!("cargo:warning=packages/{name}: neither src/ nor client/ — nothing registered (typo'd folder?)");
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
    let mut out = String::new();
    out.push_str(
        "// GENERATED by module/build.rs from `game_tick_pass!` / `game_hook!` markers under src/ and packages/*/src/. DO NOT EDIT.\n",
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
    for (event, payload_ty) in HOOK_EVENTS {
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
    for (event, payload_ty) in HOOK_EVENTS {
        out.push_str(&format!("    pub type {event} = {payload_ty};\n"));
    }
    out.push_str("}\n");
    for (event, payload_ty) in HOOK_EVENTS {
        out.push_str(&format!(
            "pub(crate) fn fire_{event}(ctx: &spacetimedb::ReducerContext, payload: &{payload_ty}) {{\n    \
                 for f in crate::GAME_HOOKS_{} {{\n        f(ctx, payload);\n    }}\n}}\n",
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

/// Blank out comments (line + nested block), string literals (plain, byte, raw), and char
/// literals, PRESERVING newlines and byte-for-char positions — so the marker scan sees only real
/// code and panic line numbers stay true. Lifetimes (`'a`) are left intact (only a real char
/// literal — quote, optional escape, closing quote — is blanked). This is what makes a
/// commented-out marker inert and lets doc comments show real marker syntax.
fn strip_comments_and_strings(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out: Vec<char> = Vec::with_capacity(b.len());
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        // Line comment (also covers /// and //!): blank to end of line.
        if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                out.push(' ');
                i += 1;
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
    out.into_iter().collect()
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

/// Scan one file for all three marker kinds, registering each hit under `prefix` (the file's
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
                    "not_transported" => {
                        reg.transfer.push(path.clone());
                        reg.not_transported.push(path);
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
                if !HOOK_EVENTS.iter().any(|(e, _)| *e == event) {
                    panic!(
                        "build.rs: `game_hook!` in {}:{line} names unknown event {event:?} — known \
                         events: {:?}. Extending the catalog means adding the payload struct + \
                         dispatch site in src/hooks.rs and the HOOK_EVENTS row in module/build.rs \
                         (payload_for aliases and fire_* fns are generated from that row).",
                        file.display(),
                        HOOK_EVENTS.iter().map(|(e, _)| *e).collect::<Vec<_>>()
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
