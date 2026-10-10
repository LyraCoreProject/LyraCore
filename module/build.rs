//! Module registry generation and Package discovery. See `docs/module-build.md`.
//!
//! The Package API lint is the `package_api_lint` test target, not part of the build.

use std::fs;
use std::path::{Path, PathBuf};

// The lint test target reads the parts of this module the build does not.
#[allow(dead_code)]
#[path = "build_support/source.rs"]
mod source;

use source::{collect_rs_files, file_gate, strip_source, DEBUG_REDUCERS_FILE_CFG};

/// One row of the notify-hook event catalog.
///
/// `actor`/`target` are the Runtime Script Event Binding half: Rust expressions, evaluated
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
    hook(
        "on_cast_finished",
        "crate::hooks::CastFinishedPayload",
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
    hook(
        "on_character_relocated",
        "crate::hooks::CharacterRelocatedPayload",
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
    // Encounter kernel: entry-keyed creature death, once-per-instance HP-threshold
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

#[allow(clippy::too_many_lines)] // One scan-and-emit step per generated registry.
fn main() {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo");
    let src_dir = Path::new(&manifest_dir).join("src");
    println!("cargo:rerun-if-changed={}", src_dir.display());

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
                if !["client", "data", "scripts", "datascripts"]
                    .iter()
                    .any(|part| pkg.join(part).is_dir())
                {
                    println!("cargo:warning=packages/{name}: none of src/, client/, data/, scripts/ or datascripts/; nothing registered");
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

    // Package-only actor verbs need no caller in a Core checkout with no installed Rust Package.
    println!("cargo::rustc-check-cfg=cfg(has_packages)");
    if !pkg_mods.is_empty() {
        println!("cargo::rustc-cfg=has_packages");
    }

    // Durable tests need a registered name for ownership and teardown, without installing content.
    if std::env::var_os("CARGO_FEATURE_PACKAGE_TEST_FIXTURE").is_some() {
        pkg_mods.push((
            "test_fixture".to_string(),
            Path::new(&manifest_dir).join("tests/fixtures/test_fixture.rs"),
        ));
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
    // The CROSS-DATABASE row transport, keyed by table accessor so
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
    // The SAME names again, as plain strings. `CHARACTER_OWNED_TRANSFERS` above cannot be
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
    // either direction. Sorted by TABLE name so the assertion compares two stable lists.
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
    if registries.client_commands.len() > 1 {
        panic!("build.rs: more than one installed package registered `game_client_command!`");
    }
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
        let pass = gated(path, "ctx: &spacetimedb::ReducerContext", "ctx", "");
        out.push_str(&format!("    (\"{path}\", {pass}),\n"));
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
            let params = format!("ctx: &spacetimedb::ReducerContext, payload: &{payload_ty}");
            out.push_str(&format!(
                "    {},\n",
                gated(path, &params, "ctx, payload", "")
            ));
        }
        out.push_str("];\n");
    }
    out.push_str(
        "pub const GAME_ENCOUNTER_PACKAGES: &[(crate::encounter::EncounterBinding, crate::encounter::EncounterPackageHandler)] = &[\n",
    );
    for (binding, path) in &registries.encounter_packages {
        let handler = gated(
            path,
            "ctx: &spacetimedb::ReducerContext, instance_id: u64, signal: crate::encounter::EncounterSignal",
            "ctx, instance_id, signal",
            "Ok(())",
        );
        out.push_str(&format!(
            "    (crate::encounter::EncounterBinding::{binding}, {handler}),\n"
        ));
    }
    out.push_str("];\n");
    out.push_str("pub const GAME_ENCOUNTER_PACKAGE_BINDING_NAMES: &[&str] = &[\n");
    for (binding, _) in &registries.encounter_packages {
        out.push_str(&format!("    \"{binding}\",\n"));
    }
    out.push_str("];\n");
    match registries.client_commands.first() {
        Some((parse, apply, reply)) => {
            let apply = gated(
                apply,
                "ctx: &spacetimedb::ReducerContext, command: &crate::bridge::AdmittedClientCommand",
                "ctx, command",
                "crate::bridge::CommandOutcome::Suppressed",
            );
            out.push_str(&format!(
                "pub const GAME_CLIENT_COMMAND: Option<crate::bridge::ClientCommandHandler> = Some(crate::bridge::ClientCommandHandler {{ parse: {parse}, apply: {apply}, reply: {reply} }});\n"
            ))
        }
        None => out.push_str(
            "pub const GAME_CLIENT_COMMAND: Option<crate::bridge::ClientCommandHandler> = None;\n",
        ),
    }
    // Every event name, as plain strings a NATIVE test binary can read without materializing the
    // fn-pointer arrays above — the same reason `CHARACTER_OWNED_TRANSFER_NAMES` exists. This is
    // what `script_binding.rs` asserts the Package Delta crate's mirror of the catalog against.
    out.push_str("pub const GAME_HOOK_EVENT_NAMES: &[&str] = &[\n");
    for HookEvent { event, .. } in HOOK_EVENTS {
        out.push_str(&format!("    \"{event}\",\n"));
    }
    out.push_str("];\n");
    registries.package_characters.sort();
    for duplicate in registries.package_characters.windows(2) {
        if duplicate[0].0 == duplicate[1].0 {
            panic!(
                "build.rs: Package {} registers `game_package_characters!` twice: {} and {}",
                duplicate[0].0, duplicate[0].1, duplicate[1].1
            );
        }
    }
    out.push_str("pub const GAME_PACKAGES: &[crate::package_teardown::InstalledPackage] = &[\n");
    for (ident, _) in &pkg_mods {
        let mut tables: Vec<&str> = registries
            .package_tables
            .iter()
            .filter(|(package, _)| package == ident)
            .map(|(_, table)| table.as_str())
            .collect();
        tables.sort_unstable();
        tables.dedup();
        let characters = registries
            .package_characters
            .iter()
            .find(|(package, _)| package == ident)
            .map_or("None".to_string(), |(_, path)| format!("Some({path})"));
        out.push_str(&format!(
            "    crate::package_teardown::InstalledPackage {{ name: \"{ident}\", tables: &[{}], characters: {characters} }},\n",
            tables
                .iter()
                .map(|table| format!("\"{table}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
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
    /// that deliberately carry nothing. Emitted as `CHARACTER_OWNED_NOT_TRANSPORTED`.
    not_transported: Vec<String>,
    tick_passes: Vec<String>,
    hooks: Vec<(String, String)>, // (event, fully-qualified fn path)
    encounter_packages: Vec<(String, String)>, // (binding variant, fully-qualified fn path)
    client_commands: Vec<(String, String, String)>, // (parser, admitted apply, reply name) paths
    package_tables: Vec<(String, String)>, // (package ident, table accessor)
    package_characters: Vec<(String, String)>, // (package ident, fully-qualified fn path)
}

/// The registry entry for `path`. A Package's fn is wrapped so it stops running once Package
/// Teardown has run for that Package; the wrapper then returns `otherwise`.
fn gated(path: &str, params: &str, args: &str, otherwise: &str) -> String {
    match path
        .strip_prefix("crate::pkg_")
        .and_then(|rest| rest.split("::").next())
    {
        Some(package) => format!(
            "|{params}| if crate::package_teardown::runs(ctx, \"{package}\") {{ {path}({args}) }} else {{ {otherwise} }}"
        ),
        None => path.to_string(),
    }
}

/// The accessor of every `#[table(..)]` and `#[spacetimedb::table(..)]` in stripped source.
/// Only the attribute's top-level `accessor`, so an index's own `accessor = ..` is never taken.
fn table_accessors(content: &str) -> Vec<String> {
    let mut accessors = Vec::new();
    for (start, _) in content.match_indices("#[") {
        let attribute = content[start + 2..].trim_start();
        let Some(args) = attribute
            .strip_prefix("table")
            .or_else(|| attribute.strip_prefix("spacetimedb::table"))
            .and_then(|rest| rest.trim_start().strip_prefix('('))
        else {
            continue;
        };
        let mut depth = 0usize;
        let mut item_start = 0usize;
        for (index, c) in args.char_indices() {
            match c {
                '(' | '[' => depth += 1,
                ')' | ']' if depth == 0 => {
                    accessors.extend(accessor_item(&args[item_start..index]));
                    break;
                }
                ')' | ']' => depth -= 1,
                ',' if depth == 0 => {
                    accessors.extend(accessor_item(&args[item_start..index]));
                    item_start = index + 1;
                }
                _ => {}
            }
        }
    }
    accessors
}

fn accessor_item(item: &str) -> Option<String> {
    let item = item.trim();
    // Teardown clears a table by name, and `name = ..` would make the name differ from the
    // accessor this scan reads.
    if item
        .strip_prefix("name")
        .is_some_and(|rest| rest.trim_start().starts_with('='))
    {
        panic!(
            "build.rs: a Package table sets `name`; keep the accessor as its table name so \
             Package Teardown can empty it"
        );
    }
    let value = item
        .strip_prefix("accessor")?
        .trim_start()
        .strip_prefix('=')?
        .trim();
    (!value.is_empty() && value.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .then(|| value.to_string())
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
    let stripped = strip_source(&content).code;
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

/// `game_client_command!` head: `(PARSE, APPLY, REPLY)` — package files only.
fn try_match_client_command(head: &str) -> Option<(String, String, String)> {
    let mut rest = head.strip_prefix('(')?;
    let mut ident_before = |separator: char| {
        let trimmed = rest.trim_start();
        let end = trimmed.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
        if end == 0 {
            return None;
        }
        rest = trimmed[end..].trim_start().strip_prefix(separator)?;
        Some(trimmed[..end].to_string())
    };
    Some((ident_before(',')?, ident_before(',')?, ident_before(')')?))
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

/// Scan one file for every marker kind, registering each hit under `prefix` (the file's
/// collapsed crate path). The scan runs on the comment/string-stripped text, so quoted or
/// commented-out marker syntax is inert; on real code, any occurrence of a marker's literal
/// substring that doesn't parse panics — never skip silently. Every registered marker in a nested
/// submodule also has its facade re-export verified (`check_facade_reexport`).
fn scan_file(file: &Path, scan_root: &Path, in_package: bool, prefix: &str, reg: &mut Registries) {
    let raw = fs::read_to_string(file)
        .unwrap_or_else(|e| panic!("build.rs: cannot read {}: {e}", file.display()));
    if !registry_file_enabled(
        &raw,
        std::env::var_os("CARGO_FEATURE_DEBUG_REDUCERS").is_some(),
    ) {
        return;
    }
    let content = strip_source(&raw).code;

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
                    // instead of being read back out of the arm's source text.
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

    scan_marker(&content, file, "game_client_command!", |head, line| {
        match try_match_client_command(head) {
            Some((parse, apply, reply)) => {
                if !in_package {
                    panic!(
                        "build.rs: `game_client_command!` in {}:{line} is core code; command meaning belongs to a Package",
                        file.display()
                    );
                }
                check_facade_reexport(file, scan_root, in_package, &parse);
                check_facade_reexport(file, scan_root, in_package, &apply);
                check_facade_reexport(file, scan_root, in_package, &reply);
                reg.client_commands.push((
                    format!("{prefix}::{parse}"),
                    format!("{prefix}::{apply}"),
                    format!("{prefix}::{reply}"),
                ));
            }
            None => panic!(
                "build.rs: malformed `game_client_command!` marker in {}:{line} — expected `game_client_command!(PARSE, APPLY, REPLY)`",
                file.display()
            ),
        }
    });

    if let Some(package) = prefix.strip_prefix("crate::pkg_") {
        for table in table_accessors(&content) {
            reg.package_tables.push((package.to_string(), table));
        }
    }

    scan_marker(&content, file, "game_package_characters!", |head, line| {
        match (try_match_tick_pass(head), prefix.strip_prefix("crate::pkg_")) {
            (Some(name), Some(package)) => {
                check_facade_reexport(file, scan_root, in_package, &name);
                reg.package_characters
                    .push((package.to_string(), format!("{prefix}::{name}")));
            }
            (Some(_), None) => panic!(
                "build.rs: `game_package_characters!` in {}:{line} is core code; only a Package names its Characters",
                file.display()
            ),
            (None, _) => panic!(
                "build.rs: malformed `game_package_characters!` marker in {}:{line} — expected exactly \
                 `game_package_characters!(fn NAME(ctx) {{ .. }})`. A marker must never be silently \
                 skipped.",
                file.display()
            ),
        }
    });

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

/// Keep registry discovery on the same whole-file feature boundary as rustc.
///
/// The scanner deliberately supports one exact leading inner attribute instead of interpreting
/// general Rust `cfg` expressions. A parent module may repeat the gate, but the file owns the
/// registry contract so recursive discovery can decide without reconstructing the module tree.
fn registry_file_enabled(source: &str, debug_reducers: bool) -> bool {
    debug_reducers || !debug_only_file(source)
}

/// Whether the file's first non-blank line is `DEBUG_REDUCERS_FILE_CFG`.
fn debug_only_file(source: &str) -> bool {
    file_gate(source) == Some(DEBUG_REDUCERS_FILE_CFG)
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
mod tests {
    use super::*;

    #[test]
    fn debug_only_registry_file_follows_the_feature() {
        let source =
            "#![cfg(feature = \"debug_reducers\")]\ncrate::game_tick_pass!(fn pass(ctx) {});\n";
        assert!(!registry_file_enabled(source, false));
        assert!(registry_file_enabled(source, true));
    }

    #[test]
    fn ordinary_registry_file_is_always_enabled() {
        let source = "crate::game_tick_pass!(fn pass(ctx) {});\n";
        assert!(registry_file_enabled(source, false));
        assert!(registry_file_enabled(source, true));
    }

    #[test]
    fn debug_cfg_lookalikes_do_not_disable_registry_discovery() {
        let comment = "// #![cfg(feature = \"debug_reducers\")]\nfn ordinary() {}\n";
        let string = "const NOTE: &str = \"#![cfg(feature = \\\"debug_reducers\\\")]\";\n";
        assert!(registry_file_enabled(comment, false));
        assert!(registry_file_enabled(string, false));
    }

    #[test]
    fn client_command_marker_names_one_parser_apply_operation_and_reply() {
        assert_eq!(
            try_match_client_command("(parse_order, apply_order, ORDER_RESULT);"),
            Some((
                "parse_order".to_string(),
                "apply_order".to_string(),
                "ORDER_RESULT".to_string()
            ))
        );
        assert_eq!(
            try_match_client_command("(parse_order, apply_order);"),
            None
        );
        assert_eq!(
            try_match_client_command("(, apply_order, ORDER_RESULT);"),
            None
        );
        assert_eq!(
            try_match_client_command("(parse_order, apply_order, );"),
            None
        );
        assert_eq!(
            try_match_client_command("(parse_order, apply_order, ORDER_RESULT, extra);"),
            None
        );
    }

    #[test]
    fn teardown_finds_each_table_by_its_own_accessor() {
        let source = "#[table(accessor = pkg_demo_one, public)]\nstruct One;\n\
                      #[spacetimedb::table(\n    index(accessor = by_due, btree(columns = [due, id])),\n    accessor = pkg_demo_two,\n)]\nstruct Two;\n\
                      #[derive(Clone)]\n#[tables(accessor = not_a_table)]\nstruct Three;\n";
        assert_eq!(table_accessors(source), ["pkg_demo_one", "pkg_demo_two"]);
    }

    #[test]
    #[should_panic(expected = "sets `name`")]
    fn a_package_table_with_its_own_name_fails_the_build() {
        table_accessors("#[table(accessor = pkg_demo_one, name = other)]\nstruct One;\n");
    }

    #[test]
    fn a_package_registration_stops_after_teardown_and_core_code_never_does() {
        assert_eq!(
            gated("crate::pkg_demo::pass", "ctx: &C", "ctx", ""),
            "|ctx: &C| if crate::package_teardown::runs(ctx, \"demo\") { crate::pkg_demo::pass(ctx) } else {  }"
        );
        assert_eq!(
            gated("crate::hooks::pass", "ctx: &C", "ctx", ""),
            "crate::hooks::pass"
        );
    }
}
