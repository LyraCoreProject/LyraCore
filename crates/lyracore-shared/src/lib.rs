//! Protocol-agnostic constants and helpers shared by the SpacetimeDB module and the
//! gateway. No external dependencies, no IO — safe to compile into the wasm module and the
//! native gateway alike.
//!
//! This crate deliberately holds only *values and pure functions* (opcode numbers,
//! bit-packing, spatial math). The byte-level (de)serialization lives in the gateway via
//! `wow_world_messages` (which owns the descriptor field indices); the table/reducer logic
//! lives in the module. See `docs/` for the design these constants implement.

pub mod auction;
pub mod bank;
pub mod constants;
pub mod env;
pub mod faction;
pub mod group;
pub mod instance;
pub mod item;
pub mod loot_roll;
pub mod mail;
pub mod nav;
pub mod opcodes;
pub mod packing;
pub mod quest;
pub mod region;
pub mod spatial;
pub mod terrain;
pub mod trade;
pub mod trainer;
pub mod values_mask;
pub mod vmap;
pub mod whisper;

/// The exact error text the module's `start_attack` reducer returns when the target is a corpse.
/// The gateway pattern-matches this specific failure to reply `SMSG_ATTACKSWING_DEADTARGET` (so the
/// client leaves combat stance with proper feedback), while other failures stay silently ignored.
/// Shared here so the producer (module) and matcher (gateway) cannot drift out of sync.
pub const ERR_ATTACK_TARGET_DEAD: &str = "target is dead";

/// The exact error text `start_attack` returns when the target is FRIENDLY to the attacker (the
/// faction gate — neutral + hostile targets are attackable; only friendly are protected). The gateway
/// maps it to `SMSG_ATTACKSWING_CANT_ATTACK` so the client leaves stance cleanly. Shared so producer
/// (module) and matcher (gateway) can't drift.
pub const ERR_ATTACK_FRIENDLY: &str = "target is friendly";
