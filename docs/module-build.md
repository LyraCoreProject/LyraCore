# Module build

`module/build.rs` discovers Package modules and generates registration arrays in `$OUT_DIR`.
It scans Rust files in `module/src/` and each installed `packages/<name>/src/` directory.
`packages/example/` is the maintained reference Package and does not enable `has_packages`.

A Package supplies `src/mod.rs` and any sibling modules. Discovery generates `package_mods.rs`
with a `pkg_<name>` module for each Package, so installing one requires no Core source edit.

`character_owned!` markers generate `character_sweeps.rs`:

- `CHARACTER_OWNED_DELETE_SWEEPS` and `CHARACTER_OWNED_RESTAMP_SWEEPS` contain sweep functions.
- `CHARACTER_OWNED_TABLES` names the tables in the Character transport manifest.
- `CHARACTER_OWNED_TRANSFERS` pairs each table name with its row mover.
- `CHARACTER_OWNED_TRANSFER_NAMES` exposes those names without function pointers for native tests.
- `CHARACTER_OWNED_NOT_TRANSPORTED` names the deliberate exclusions whose reasons the tests check.

A delete marker must use `sweep_delete_<table_accessor>`; a transport marker must use
`sweep_transfer_<table_accessor>`. Prefix stripping derives the table names, keeping the
manifest and mover registry tied to the declarations. A `not_transported` marker still
registers a transport arm, which exports an empty entry and absorbs arriving rows.

`game_tick_pass!`, `game_hook!`, `game_client_command!`, and `encounter_package!` generate
`package_registries.rs`. The registries contain periodic passes, notify hooks, client command
handlers, and map-specific Encounter Authority. Registration wrappers stop Package work after
Package Teardown. `GAME_PACKAGES` lists compiled Packages, their tables, and their Character reads.

`HOOK_EVENTS` generates `hook_dispatch.rs`, including the payload aliases and a `fire_*` function
for each event. `module/src/hooks.rs` includes it, so callers use the same dispatch paths.

The marker scanner ignores comments and string literals. It reads invocation heads, without
interpreting function bodies. A live marker must match the supported grammar; malformed markers
fail the build instead of disappearing from the registry.

Core file paths resolve through their top-level module. Nested marker files must have a visible
facade re-export, which the scanner checks. Package paths resolve through their generated
`pkg_<name>` module. A debug-only source file must start with
`#![cfg(feature = "debug_reducers")]`; discovery observes that gate so an ordinary build cannot
reference a function omitted by Rust.

Package source also passes the Package API lint described in `docs/package-api.md`. A path outside
`PACKAGE_API_ROOTS` fails with the Package, file, line, and path. An explicit
`// package-api: exempt <reason>` permits an ordinary boundary exception. A gated API root still
requires its debug or test gate, and exemptions cannot bypass that requirement. Core source does
not pass through this lint.
