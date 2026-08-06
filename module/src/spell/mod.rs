//! Spells & auras — the **data-driven effect-row engine** (v2). A spell is a `game_spell` header plus
//! an ordered list of `game_spell_effect` rows; each effect carries a `kind` (a deduplicated
//! effect/aura taxonomy), a magnitude, a target, and two **single-meaning** typed params (`p0`/`p1`,
//! tagged by `p0_kind`) — the mangos `misc_value` union resolved once, at author/import time, never at
//! runtime. `resolve_cast_at` iterates the effects and dispatches each `kind` to a handler; auras are
//! placed on `game_aura` with a **frozen typed snapshot** so periodic ticks + combat stat-reads never
//! re-join the template. Importable from `Spell.dbc` (the importer fills header + effect rows); a
//! `Scripted` kind + graceful-degradation no-op are the escape hatch for the irreducible tail. [static]
//!
//! Runtime tables: `game_aura` (active auras, gateway-relayed as `UNIT_FIELD_AURA`), `game_spell_cooldown`
//! (GCD), `game_pending_cast` (cast bar), `game_spell_cast_event` (the cast visual). The aura VALUES
//! relay reuses the proven dirty_reset discipline (the gateway reads slot/spell_id/level/flags). [event]
//!
//! Submodules:
//!   - `tables`    — the spell/aura/cooldown/cast/schedule table structs (+ their generated accessors).
//!   - `taxonomy`  — the KIND/MECHANIC/param/target/combat-field/stat/resistance consts + tuning.
//!   - `control`   — the crowd-control model (the 7 CC predicates + immunity + break-on-damage).
//!   - `math`      — the pure helpers + aura-read folds + damage/resist/dispel/vitals/absorb helpers.
//!   - `cast`      — targeting + effect dispatch + the cast core (`resolve_cast_at` / `begin_cast`).
//!   - `scheduler` — the `#[reducer]`s (`tick_auras` / `fire_pending_cast` / `cast_spell`).
//!
//! Everything is re-exported below so every `crate::spell::<sym>` path resolves regardless of which
//! submodule actually defines it.

mod cast;
mod control;
mod math;
mod scheduler;
pub(crate) mod spellbook;
pub(crate) mod stacking;
mod tables;
mod taxonomy;

#[cfg(test)]
mod tests;

// Re-export the full surface. `pub use` for submodules exposing at least one `pub` item; `pub(crate) use`
// for submodules whose surface is entirely `pub(crate)` (avoids the "glob reexports nothing public"
// warning).
pub(crate) use cast::*;
pub use control::*;
pub use math::*;
pub use scheduler::*;
pub use spellbook::*;
pub use tables::*;
pub(crate) use taxonomy::*;
// `stacking` is reached explicitly via `crate::spell::stacking::<sym>` (its tables' generated
// accessor traits — `game_spell_group`/`game_spell_group_rule`/`game_dr_state` — collide-free but its
// pure types (`ApplyDecision`, `AuraSummary`, ...) are deliberately NOT globbed into this module's flat
// namespace, since `cast.rs`/`scheduler.rs`/`math.rs` reference them qualified for readability at each
// of the few call sites).
