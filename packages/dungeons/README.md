# dungeons

One Package for the encounter choreography of every scripted dungeon. Each dungeon is one submodule
under `src/`, and `src/mod.rs` re-exports them all, so the whole Package installs, enables, and
disables as one unit.

## Boundary

In this Package:

- Encounter choreography: the `encounter_package!` handler per `EncounterBinding`, plus the
  `game_hook!`, `game_tick_pass!`, and scheduled-reducer support that choreography needs.
- Package-owned tables for that choreography (for example `SunkenTempleSuppression`,
  `WailingEscortSchedule`).
- The `debug_reducers` verification harness (`src/eventai_instance_test.rs`): the `debug_verify_*`
  reducers the durable encounter tests call. It compiles only under that feature.

Not in this Package:

- The encounter kernel itself (`module/src/encounter.rs`): bindings, door state, and signal
  dispatch stay core.
- Base dungeon data: creatures, spawns, and EventAI rows come from the importer, not from here.
- Playerbots: bot behavior is its own Package.

The Package covers many dungeons and grows by one submodule per dungeon; it is not scoped to a
single encounter. To add one: create `src/<dungeon>.rs`, add its `mod` and `pub use <dungeon>::*;`
pair to `src/mod.rs`, and register its handlers with `encounter_package!`.
The registry scan resolves every file through the Package facade, so keep item names
dungeon-prefixed.
