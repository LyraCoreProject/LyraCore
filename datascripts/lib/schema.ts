import type { Spell, SpellEffect } from "../generated/types";

export type FieldType =
  | "u8"
  | "u16"
  | "u32"
  | "u64"
  | "i32"
  | "f32"
  | "bool"
  | "string";

// Keys use generated names; column values use durable names.
export interface Column {
  readonly column: string;
  readonly type: FieldType;
}

// The Package API does not claim the combo-point columns.
export type SpellField = Exclude<keyof Spell, "spellId" | "maxDurationMs">;

export type SpellEffectField = Exclude<
  keyof SpellEffect,
  "id" | "spellId" | "effectIndex" | "pointsPerCombo"
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

export const SPELL_TABLE = "game_spell";
export const SPELL_EFFECT_TABLE = "game_spell_effect";

export const SPELL_KEY_COLUMNS = ["spell_id"];
export const SPELL_EFFECT_KEY_COLUMNS = ["id", "spell_id", "effect_index"];
