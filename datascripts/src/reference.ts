// The reference Datascript: the maintained, minimal example of author-time code written against
// the Module schema, and the standing check for schema drift.
//
// Every name below comes from `generated/`, which `lyracore packages build` regenerates from the
// Module wasm before it typechecks. That is the whole point of this file: rename or retype a field
// in the Module and `tsc --noEmit` fails HERE, at author time, instead of at apply time. Keep it
// referencing real columns for exactly that reason.
//
// `Spell` is a core table; `SunkenTempleSuppression` is an enabled Package's own table
// (`packages/dungeons`). The Module builds every enabled Package into the same wasm, so
// `generated/` carries both, and a Datascript typechecks against a Package table the same way it
// does against a core one.
//
// It deliberately writes nothing and applies nothing. Building a Package Delta from a Datascript is
// separate work; this file only proves that the generated schema binds.

import type { Spell, SunkenTempleSuppression } from "../generated/types";

/// One derived rank of a Spell, in the shape the schema actually declares.
///
/// Ranks in 1.12.1 share a name and differ by id, cost and effect magnitude, so a rank is the base
/// row with those columns replaced. Returning `Spell` rather than a loose object is what makes the
/// typecheck meaningful: a column this function forgets, misspells or mistypes is an error.
export function spellRank(base: Spell, rank: number): Spell {
  return {
    ...base,
    spellId: base.spellId + rank,
    name: `${base.name} (Rank ${rank})`,
    cost: base.cost * rank,
    spellLevel: base.spellLevel,
    cooldownMs: base.cooldownMs,
    rangeYd: base.rangeYd,
  };
}

/// The reading a rank ladder is built from, kept as a plain summary so the return type names the
/// generated columns it reads rather than restating them as loose strings.
export function describeRank(spell: Spell): string {
  return `${spell.name} #${spell.spellId}: ${spell.cost} power, ${spell.cooldownMs}ms cooldown`;
}

/// The same reading, done against `packages/dungeons`' own table instead of a core one — the
/// generated typings cover an installed Package exactly as they cover the Module it ships in.
export function describeSuppressionWindow(row: SunkenTempleSuppression): string {
  return `suppression #${row.scheduledId} armed for instance ${row.instanceId}`;
}
