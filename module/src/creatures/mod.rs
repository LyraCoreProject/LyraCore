//! Tier 2 (NPCs): server-authored creatures. Static template + patrol waypoints, the broadcast
//! creature-move event the gateway relays as `SMSG_MONSTER_MOVE`, and the scheduled patrol tick.
//! The live creature is a `game_world_entity` row of type Unit (see `world.rs`). [static]/[event]
//!
//! Split into submodules — every `crate::creatures::<sym>` path still resolves via the re-exports
//! below:
//!   - [`spawn`] — the creature tables + entity builders + stat rolls + the content importer
//!   - [`ai`]    — pure movement/flee/chase/wander geometry + the AI tuning consts (+ the unit tests)
//!   - [`tick`]  — the move-event tables + the `tick_creatures` pass pipeline
//!   - [`pet`]   — the warlock pet system (summon/despawn/follow), riding the creature machinery above

mod ai;
mod eventai;
mod pet;
mod spawn;
pub(crate) mod tick;

pub use ai::*;
pub use eventai::*;
pub use pet::*;
pub use spawn::*;
pub use tick::*;
