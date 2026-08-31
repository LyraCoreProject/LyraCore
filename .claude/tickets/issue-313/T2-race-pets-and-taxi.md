# T2 — the remaining Client Mirror Tables: race info, creature family, taxi

Model: sonnet

Depends on: T1. Rebase onto T1 before starting.

## Goal

Add the five remaining update-only tables to the `dbc` family. Mechanical after T1: the shell, the
hook, the provenance stamp and the refusal all exist. No new machinery, no identifier range.

## Read first

- `.claude/tickets/issue-313/README.md`, the Client Mirror rule and the client-divergence section.
- T1's `module/src/package_import/dbc.rs`. Follow its per-table block layout exactly.
- `crates/lyracore-shared/src/constants.rs`, `taxi_protocol` and `taxi_fixture`. The node mask and
  the reserved fixture ids are the reason the taxi tables cannot take inserts.
- `importer/src/dbc.rs::taxi_catalogue_sql`, for the three tables' validation rules and the
  fixture rows it appends after the client rows.
- `module/src/taxi.rs` lines 388 and 524: the Module reads `game_taxi_path_node` to build the
  passenger spline, which is why a geometry claim IS visible to the player.

## Tables

Five, all update-only. Struct sources are `module/src/config.rs` (`RaceInfo`, `GameTaxiNode`,
`GameTaxiPath`, `GameTaxiPathNode`) and `module/src/creatures/spawn.rs` (`CreatureFamily`).

**`game_race_info`** — `ChrRaces.dbc`, importer `dbc::race_info_sql`

- Claim key: `{ race: u8 }`.
- Claimable columns: `male_display` (u32), `female_display` (u32), `faction_template` (u32).
- Read at login by `module/src/creatures/spawn.rs` to build the player's world entity.
- Two reasons for update-only, and the doc comment states both: it is a Client Mirror Table (the
  creation screen enumerates races from the client's own `ChrRaces.dbc`), AND the key is a `u8`, so
  no range value could fit even if the first reason were repealed.
- Display ids ARE honoured: the wire carries the id and the client renders it from its own
  `CreatureDisplayInfo.dbc`. So a claim here changes what other players see.

**`game_creature_family`** — `CreatureFamily.dbc`, importer `dbc::creature_family_sql`

- Claim key: `{ family_id: u32 }`.
- Claimable columns: `name` (string), `pet_food_mask` (i32), `pet_talent_type` (i32),
  `category` (i32).
- Read by `module/src/creatures/hunter_pet.rs` (tameability: `pet_talent_type != -1`) and
  `module/src/creatures/pet_care.rs` (the feeding food-mask gate). Both fully server-authoritative,
  so making a family tameable or widening its diet works end to end.

**`game_taxi_node`** — `TaxiNodes.dbc`, importer `dbc::taxi_catalogue_sql`

- Claim key: `{ id: u32 }`, the `TaxiNodes.dbc` record id, which the importer copies verbatim into
  the storage id.
- Claimable columns: `map_id` (u32), `x`, `y`, `z` (f32), `name` (string),
  `mount_display_horde` (u32), `mount_display_alliance` (u32).
- `client_node_id` is NOT claimable. It is a one-based bit position in the vanilla 256-bit
  known-node mask and carries a `#[unique]` index; a claim that moved it would either collide with
  another node's bit or fall outside the mask. State that in the doc comment.
- Position and mount display are honoured; `name` diverges from the client's own file.

**`game_taxi_path`** — `TaxiPath.dbc`, importer `dbc::taxi_catalogue_sql`

- Claim key: `{ id: u32 }`.
- Claimable columns: `fare` (u32) only. `source_node_id` and `destination_node_id` stay out: the
  Module indexes routes by `(source_node_id, destination_node_id)` and a claim that re-pointed an
  endpoint would silently retarget an existing route rather than create one, which is the operation
  the Client Mirror rule forbids.
- Fare tuning is the plain case for including the taxi tables at all, and it is honoured end to end.
- The doc comment carries the third divergence kind from the README: a route the client's own
  `TaxiPath.dbc` does not connect is never offered on the flight map, which is why this table takes
  no inserts.

**`game_taxi_path_node`** — `TaxiPathNode.dbc`, importer `dbc::taxi_catalogue_sql`

- Claim key: `{ id: u32 }`, the `TaxiPathNode.dbc` point id.
- Claimable columns: `map_id` (u32), `x`, `y`, `z` (f32), `flags` (i32), `delay_ms` (i32).
- `path_id` and `node_index` stay out: together they are the row's natural identity, and the Module
  reads the path's points through the `by_path` index in `node_index` order. A claim that changed
  either would reorder or reparent a waypoint rather than edit it.
- This is the one taxi table with NO client divergence at all: the Module builds the passenger
  spline from these rows, so rerouting a flight is exactly what the player flies. Say so.

### The key-shape note this ticket owes the reader

Every claim key in this ticket is the DBC record id, and for all five tables the importer copies
that id straight into the durable primary key. That is what makes a claim on a DBC catalogue
nameable at all, and it is worth one sentence in the family doc comment, because two tables in T3
do NOT have that property.

### The taxi fixture

`taxi_catalogue_sql` appends the headless-flight fixture rows (`taxi_fixture::*`, storage ids
5,090,100 to 5,090,105) after the client rows, and refuses any client id that reaches the reserved
namespace. A claim on a fixture id must be refused the same way every other family refuses a
fixture-reserved identifier, so reuse the existing `is_fixture_reserved_*` shape rather than
inventing a taxi-specific check. There is no range to check against here, only the fixture span.

## Files owned

Same set as T1, minus `importer/src/main.rs` (no validation change), plus nothing new.

- `crates/lyracore-package-delta/src/schema.rs`, `delta.rs`, `error.rs`
- `crates/lyracore-package-delta/tests/families.rs`, `tests/dbc_identifiers.rs`
- `module/src/package_import/dbc.rs`
- `CONTEXT.md`, only if a term needs sharpening

## Out of scope

- `game_graveyard`, `game_skill_line`, `game_skill_ability`, `game_lock`. T3 owns them.
- The Package DBC Range. Every table here says "never" in `check_inventable`.
- Making `client_node_id`, `source_node_id`, `destination_node_id`, `path_id` or `node_index`
  claimable. They are identity, not data.
- Any change to `taxi_catalogue_sql`'s validation or to the fixture constants.

## Acceptance tests

1. `tests/dbc_identifiers.rs`: all five tables refuse every insert with `InsertNotSupported` naming
   the table; a claim on a taxi fixture id is refused as fixture-reserved.
2. `tests/families.rs`: five new names parse back to themselves, report family `dbc`, each has
   claimable columns.
3. Setter and column-coverage tests per table in `module/src/package_import/dbc.rs`.
4. A test that `client_node_id`, `source_node_id`, `destination_node_id`, `path_id` and `node_index`
   are refused as unknown columns, so the identity decision above is pinned rather than implied.
5. `check_references`: `game_taxi_path.fare` needs none; assert that `game_race_info.faction_template`
   must name a `game_faction_template` row, reusing T1's check.
6. Crate, module lib and importer suites pass; wasm release build succeeds; clippy and rustfmt clean
   on touched files.

## Definition of done

A Package can move a flight master's landing point, change a route's fare, reroute a flight's
waypoints, make a creature family tameable or change its diet, and change a race's display or
nameplate faction, and all of it survives the next `--dbc` reload. Eight tables now belong to the
`dbc` family, none of them inventable.
