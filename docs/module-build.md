# Module build

`module/build.rs` discovers Package modules and generates registration arrays in `$OUT_DIR`.
It scans Rust files in `module/src/` and each installed `packages/<name>/src/` directory.
Any Package with a `src/` directory enables the `has_packages` cfg. Core ships no enabled Packages.

A Package supplies `src/mod.rs` and any sibling modules. A `src/` without `mod.rs` fails the build.
Discovery generates `package_mods.rs` with a `pkg_<name>` module for each Package, so installing one
requires no Core source edit. The `package_test_fixture` feature adds `tests/fixtures/test_fixture.rs`
as one more Package module.

`character_owned!` markers generate `character_sweeps.rs`:

- `CHARACTER_OWNED_DELETE_SWEEPS` and `CHARACTER_OWNED_RESTAMP_SWEEPS` contain sweep functions.
- `CHARACTER_OWNED_TABLES` names the tables in the Character transport manifest.
- `CHARACTER_OWNED_TRANSFERS` pairs each table name with its row mover.
- `CHARACTER_OWNED_TRANSFER_NAMES` exposes those names without function pointers for native tests.
- `CHARACTER_OWNED_NOT_TRANSPORTED` names the deliberate exclusions. No test restates the list or
  checks a reason: the `character_owned!(not_transported, ..)` marker in the declaring file is the
  one statement of each decision, with a comment beside it when the reason is not obvious.

A delete marker must use `sweep_delete_<table_accessor>`; a transport marker must use
`sweep_transfer_<table_accessor>`. Prefix stripping derives the table names, keeping the
manifest and mover registry tied to the declarations. A `not_transported` marker still
registers a transport arm, which exports an empty entry and absorbs arriving rows.

`game_tick_pass!`, `game_hook!`, `game_client_command!`, `encounter_package!` and
`game_package_characters!` generate `package_registries.rs`. The registries contain periodic passes
(`GAME_TICK_PASSES`), notify hooks, the client command handler, and map-specific Encounter Authority.
Registration wrappers stop Package work after Package Teardown. `GAME_PACKAGES` lists compiled
Packages, their tables, and their Character reads. A Package table that sets its own `name` fails
the build, because Package Teardown addresses tables by accessor.

Every `#[table(.., scheduled(REDUCER))]` declaration under `module/src/` generates one arm of
`virtual_schedules.rs`. `module/src/time.rs` includes it in conformance builds, so the virtual clock
captures, parks and fires the table's rows. Each arm names the table by its declaring module and
the reducer by the module that defines `#[reducer] fn REDUCER`, so both modules must be visible
from the crate root. The table needs a `#[primary_key]` field and the default `scheduled_at`
column. A table that schedules from `ctx.timestamp` instead of `time::schedule_time` or
`time::schedule_interval` opts out with `crate::virtual_clock_exempt!(table_accessor);` in its
declaring file. A marker that names no scheduled table in that file fails the build. Packages
cannot reach the crate-private Realm Clock, so their scheduled tables are not registered. Each arm
sits under the `cfg` gates of its struct and of its module path, as in the table catalog below.

Schedules due at the same virtual instant fire in arm order, and conformance traces observe that
order. `SAME_INSTANT_FIRING_ORDER` in `build.rs` fixes it for the tables that existed when the
registry became generated; a table it does not list fires after them, ordered by accessor. Adding a
scheduled table therefore needs no edit there. Removing or reordering a listed entry changes
recorded traces.

Every `#[table]` declaration under `module/src/` generates one line of `table_catalog.rs`, which
`module/src/tables.rs` includes: a re-export of the row type and its accessors from the declaring
module, under the `cfg` gates of the struct, the file and each `mod` declaration on its path.
`lib.rs` re-exports `tables::*`, so a new table needs no hand edit to be named as
`crate::tables::<name>`, `crate::<name>` or `lyracore_module::<name>`. The declaring module must be
visible from the crate root and the row type must be `pub`. The catalog is the table surface the
Package API promises, so a table Packages must not rely on opts out with
`crate::outside_table_catalog!(table_accessor, ..);` in its declaring file. A marker that names no
table declared in that file fails the build.

`datascripts/runtime-scripts/events.json` supplies the event catalogue. The build uses it to
emit `hook_dispatch.rs`, including payload aliases, each `fire_*` function and the Runtime Script
payload fields. `module/src/hooks.rs` includes it, so callers keep the same dispatch paths.
`generate-events.ts` reads the same catalogue to emit TS/Lua editor declarations and the names
accepted by the Script Artifact parser. The Runtime Script tests check these generated files.

The marker scanner ignores comments and string literals. It reads invocation heads, without
interpreting function bodies. A live marker must match the supported grammar; malformed markers
fail the build instead of disappearing from the registry.

Core file paths resolve through their top-level module. Nested marker files must have a visible
facade re-export, which the scanner checks. Package paths resolve through their generated
`pkg_<name>` module. A debug-only source file must start with
`#![cfg(feature = "debug_reducers")]`; discovery observes that gate so an ordinary build cannot
reference a function omitted by Rust.

The separate `package_api_lint` test checks Package source against `docs/package-api.md`. Code
generation does not run it. CI runs it after installing the Official Packages.
