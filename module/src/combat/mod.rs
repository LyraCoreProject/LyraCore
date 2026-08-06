//! Combat — melee auto-attack, **bidirectional** with **per-unit attack speed**: each unit swings on
//! its own `base_attack_time_ms`, and a creature that is attacked **retaliates** and swings back at its
//! own speed. Each swing writes a `game_combat_event` row for the gateway to relay as
//! `SMSG_ATTACKERSTATEUPDATE` (swing animation + damage text); the health drop flows through the
//! `game_world_entity` `on_update` → VALUES relay. A creature target dies at 0 HP (DESTROY + respawn);
//! a **player** target also dies at 0 HP (health=0 + `dead` → the death/release/ghost corpse-run, see
//! `world::repop`). [entity]/[event]/[server]
//!
//! Submodules (#382 split along this file's own comment banners, on top of #370's shared damage
//! pipeline):
//!   - `tables` — the ctx-free hit-table/damage/regen math (pure formulas, exhaustive tests; unchanged
//!     by the split — it is the model file for the module).
//!   - `folds` — effective-value folds over aura/gear (dodge/parry/block/crit/miss/swing-time/armor/
//!     strength/agility), seal-on-swing + queued-strike, the react-window, equipped-weapon/ammo
//!     lookups, and the `roll_swing`/`roll_ranged_swing` family.
//!   - `death` — `kill_creature`/`kill_player` (the two death chokepoints) + the shared damage
//!     pipeline (`fold_incoming_damage`/`apply_hit`) every damage resolver routes through.
//!   - `engage` — the engagement model: enter/disengage/queries over `game_melee_attack`, the
//!     engagement tables, and the start/stop-attack reducers.
//!   - `swing` — the tick passes (`tick_melee`'s leash/aggro/resolve) + the resolvers that fire a hit
//!     (`fire_melee_swing`/`resolve_offhand_swing`/`fire_ranged_shot`/`ranged_impact`).
//!
//! Everything is re-exported below so every `crate::combat::<sym>` path resolves regardless of which
//! submodule actually defines it.

mod death;
mod engage;
mod folds;
mod swing;
mod tables;

pub(crate) use death::*;
pub use engage::*;
pub use folds::*;
pub use swing::*;
pub use tables::*;
