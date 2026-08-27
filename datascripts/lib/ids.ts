// Reserved spell identifier bands, and the rule that decides which of them a Package may claim.
//
// A strict mirror of `crates/lyracore-package-delta/src/ids.rs`. Every constant is a policy number,
// not a derived one; the reasoning behind each band lives in the Rust file and is not repeated here,
// because a second copy of the reasoning is a second thing to get out of date.

/// Packed `game_spell_effect` primary key: `(spell_id << 2) | effect_index`. A Package Delta never
/// authors the packed value; it names the spell and the effect index, and this derives the key.
export function packedSpellEffectId(spellId: number, effectIndex: number): bigint {
  return (BigInt(spellId) << 2n) | BigInt(effectIndex);
}

/// Highest `effect_index` a claim may name. A vanilla spell carries three effect slots.
export const MAX_SPELL_EFFECT_INDEX = 2;

/// The Package Spell Range: the identifiers a Package may INSERT.
export const PACKAGE_SPELL_ID_FLOOR = 6_000_000;
export const PACKAGE_SPELL_ID_CEIL = 6_999_999;

/// The seeded fixture cluster.
export const FIXTURE_SPELL_ID_FLOOR = 50_000;
export const FIXTURE_SPELL_ID_CEIL = 50_999;

/// The project-wide `509xxxx` fixture-reserved band.
export const FIXTURE_RESERVED_ID_FLOOR = 5_090_000;
export const FIXTURE_RESERVED_ID_CEIL = 5_099_999;

/// True when a Package may INSERT a spell at this identifier.
export function isPackageSpellId(spellId: number): boolean {
  return spellId >= PACKAGE_SPELL_ID_FLOOR && spellId <= PACKAGE_SPELL_ID_CEIL;
}

/// True when the identifier belongs to a seeded fixture. No Package may claim one, under any
/// operation.
export function isFixtureReservedSpellId(spellId: number): boolean {
  return (
    (spellId >= FIXTURE_SPELL_ID_FLOOR && spellId <= FIXTURE_SPELL_ID_CEIL) ||
    (spellId >= FIXTURE_RESERVED_ID_FLOOR && spellId <= FIXTURE_RESERVED_ID_CEIL)
  );
}
