// The tables a Package Delta may claim, their claimable columns, and the wire type each column
// takes. Hand-written against the Rust catalogue in `crates/lyracore-package-delta/src/schema.rs`,
// which is itself hand-written against `module/src/spell/tables.rs`.
//
// Two names exist for every column, and both are load-bearing:
//
//   * the KEY of each entry is the generated TypeScript name (`cooldownMs`), which is what an author
//     writes and what the editor completes;
//   * `column` is the DURABLE name (`cooldown_ms`), which is what the artifact and the Base Snapshot
//     carry.
//
// The catalogue's keys are typed as exactly the generated `Spell` / `SpellEffect` columns minus the
// primary key, so a column added, removed or renamed in the Module fails `tsc --noEmit` HERE. That
// is the same standing check the Reference Datascript performs, applied to the authoring surface.

import type { Spell, SpellEffect } from "../generated/types";

/// The wire type tag a claimed value carries. Mirrors `FieldType` in the Rust catalogue.
export type FieldType =
  | "u8"
  | "u16"
  | "u32"
  | "u64"
  | "i32"
  | "f32"
  | "bool"
  | "string";

/// One claimable column: its durable name and the type a claim on it must declare.
export interface Column {
  readonly column: string;
  readonly type: FieldType;
}

/// The `game_spell` columns an author may set, keyed by their generated names. `spellId` is absent:
/// a claim carries it in the key, so naming it in `fields` would let a Package state one identifier
/// twice and disagree with itself.
export type SpellField = Exclude<keyof Spell, "spellId">;

/// The `game_spell_effect` columns an author may set. `id` is absent for the key reason plus one
/// more: it is derived from the spell and the effect index, never authored.
export type SpellEffectField = Exclude<
  keyof SpellEffect,
  "id" | "spellId" | "effectIndex"
>;

export const SPELL_COLUMNS: { readonly [K in SpellField]: Column } = {
  name: { column: "name", type: "string" },
  powerType: { column: "power_type", type: "u8" },
  cost: { column: "cost", type: "u32" },
  castTimeMs: { column: "cast_time_ms", type: "u32" },
  gcdMs: { column: "gcd_ms", type: "u32" },
  cooldownMs: { column: "cooldown_ms", type: "u32" },
  rangeYd: { column: "range_yd", type: "u32" },
  durationMs: { column: "duration_ms", type: "u32" },
  schoolMask: { column: "school_mask", type: "u8" },
  dispelType: { column: "dispel_type", type: "u8" },
  mechanic: { column: "mechanic", type: "u8" },
  maxStacks: { column: "max_stacks", type: "u8" },
  auraInterrupt: { column: "aura_interrupt", type: "u16" },
  attributes: { column: "attributes", type: "u32" },
  spellLevel: { column: "spell_level", type: "u8" },
  maxLevel: { column: "max_level", type: "u8" },
  isNegative: { column: "is_negative", type: "bool" },
  castFlags: { column: "cast_flags", type: "u32" },
  stances: { column: "stances", type: "u8" },
  familyName: { column: "family_name", type: "u8" },
  familyFlags: { column: "family_flags", type: "u64" },
  procFlags: { column: "proc_flags", type: "u32" },
  procChance: { column: "proc_chance", type: "u8" },
  procCharges: { column: "proc_charges", type: "u8" },
};

export const SPELL_EFFECT_COLUMNS: {
  readonly [K in SpellEffectField]: Column;
} = {
  kind: { column: "kind", type: "u8" },
  basePoints: { column: "base_points", type: "i32" },
  dieSides: { column: "die_sides", type: "i32" },
  perLevel: { column: "per_level", type: "f32" },
  periodMs: { column: "period_ms", type: "u32" },
  target: { column: "target", type: "u8" },
  radiusYd: { column: "radius_yd", type: "f32" },
  chainTargets: { column: "chain_targets", type: "u8" },
  triggerSpell: { column: "trigger_spell", type: "u32" },
  effectMechanic: { column: "effect_mechanic", type: "u8" },
  p0: { column: "p0", type: "i32" },
  p0Kind: { column: "p0_kind", type: "u8" },
  p1: { column: "p1", type: "i32" },
  scriptId: { column: "script_id", type: "u32" },
  entersCombat: { column: "enters_combat", type: "bool" },
};

/// The durable table names, and the values an artifact's `table` member carries.
export const SPELL_TABLE = "game_spell";
export const SPELL_EFFECT_TABLE = "game_spell_effect";

/// The primary-key columns of each table, by their durable names. A claim never carries one in
/// `fields`; the key already names it.
export const SPELL_KEY_COLUMNS = ["spell_id"];
export const SPELL_EFFECT_KEY_COLUMNS = ["id", "spell_id", "effect_index"];
