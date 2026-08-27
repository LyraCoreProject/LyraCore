// Why a Datascript's Package Delta was refused.
//
// A STRICT MIRROR of `DeltaError` in `crates/lyracore-package-delta/src/error.rs`. Every rule the
// Rust parser enforces is enforced here too, under the same name, so a Datascript that emits fails
// at author time rather than in the importer. Nothing here is a rule the parser does not have: an
// author who works around a refusal here would only meet it again at import.
//
// # The variants that cannot happen here
//
// `Malformed`, `UnsupportedVersion`, `MissingMember`, `UnexpectedMember` and `WrongJsonType` are the
// parser's answers to untrusted JSON; this library writes the JSON, so it cannot produce them.
// `UnknownTable` and `UnknownOperation` cannot happen because neither is authored — the table
// follows from the object an author holds, and the operation from whether it was cloned.
// `DeleteNotSupported` cannot happen because there is no delete surface to call.
// `FieldTypeMismatch` cannot happen because a claim always declares the column's OWN type: a value
// that does not fit it is `ValueOutOfRange`, exactly as the parser would rule on the same artifact.
//
// # What is NOT a refusal
//
// Missing base data is a different failure: it is about the snapshot an author is reading, not about
// the artifact they are writing, and the Rust parser has no opinion on it because it never sees base
// data. It raises `BaseDataError` instead, so the mirror stays exact.

import {
  FIXTURE_RESERVED_ID_CEIL,
  FIXTURE_RESERVED_ID_FLOOR,
  FIXTURE_SPELL_ID_CEIL,
  FIXTURE_SPELL_ID_FLOOR,
  MAX_SPELL_EFFECT_INDEX,
  PACKAGE_SPELL_ID_CEIL,
  PACKAGE_SPELL_ID_FLOOR,
} from "./ids.ts";
import type { FieldType } from "./schema.ts";

/// The `DeltaError` variant a refusal mirrors.
export type RefusalKind =
  | "InvalidPackageId"
  | "InvalidSourceHash"
  | "MalformedKey"
  | "SpellIdNotClientSafe"
  | "SpellIdFixtureReserved"
  | "EffectIndexOutOfRange"
  | "UnknownField"
  | "ValueOutOfRange"
  | "KeyColumnClaimed"
  | "IncompleteInsert"
  | "EmptyUpdate"
  | "DuplicateClaim";

/// A Package Delta this Datascript may not emit. Thrown, never returned: a refused Datascript
/// writes nothing at all, so a half-built artifact can never reach a Package folder.
export class DeltaRefusal extends Error {
  readonly kind: RefusalKind;

  constructor(kind: RefusalKind, message: string) {
    super(message);
    this.name = "DeltaRefusal";
    this.kind = kind;
  }
}

/// The Base Snapshot does not hold what the Datascript asked for. Distinct from a `DeltaRefusal`:
/// the artifact would have been valid, the base data it was to be derived from is not there.
export class BaseDataError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "BaseDataError";
  }
}

export function invalidPackageId(found: string): DeltaRefusal {
  return new DeltaRefusal(
    "InvalidPackageId",
    `\`${found}\` is not a Package identity: expected 1 to 64 characters of a-z, 0-9, \`-\`, \`_\` or \`.\``,
  );
}

export function invalidSourceHash(found: string): DeltaRefusal {
  return new DeltaRefusal(
    "InvalidSourceHash",
    `\`${found}\` is not a source hash: expected 64 lowercase hexadecimal characters`,
  );
}

export function malformedKey(table: string, detail: string): DeltaRefusal {
  return new DeltaRefusal("MalformedKey", `malformed \`${table}\` key: ${detail}`);
}

export function spellIdNotClientSafe(spellId: number): DeltaRefusal {
  return new DeltaRefusal(
    "SpellIdNotClientSafe",
    `spell ${spellId} is outside the Package spell range ${PACKAGE_SPELL_ID_FLOOR}..=${PACKAGE_SPELL_ID_CEIL}; ` +
      `an inserted spell must use an identifier no client and no import can already own`,
  );
}

export function spellIdFixtureReserved(spellId: number): DeltaRefusal {
  return new DeltaRefusal(
    "SpellIdFixtureReserved",
    `spell ${spellId} is fixture-reserved (${FIXTURE_SPELL_ID_FLOOR}..=${FIXTURE_SPELL_ID_CEIL} and ` +
      `${FIXTURE_RESERVED_ID_FLOOR}..=${FIXTURE_RESERVED_ID_CEIL}); no Package may claim it`,
  );
}

export function effectIndexOutOfRange(effectIndex: number): DeltaRefusal {
  return new DeltaRefusal(
    "EffectIndexOutOfRange",
    `effect index ${effectIndex} is out of range; a spell has effects 0..=${MAX_SPELL_EFFECT_INDEX}`,
  );
}

export function unknownField(table: string, field: string): DeltaRefusal {
  return new DeltaRefusal(
    "UnknownField",
    `\`${table}\` has no claimable column \`${field}\``,
  );
}

export function valueOutOfRange(
  field: string,
  type: FieldType,
  literal: string,
): DeltaRefusal {
  return new DeltaRefusal(
    "ValueOutOfRange",
    `column \`${field}\`: \`${literal}\` does not fit \`${type}\``,
  );
}

export function keyColumnClaimed(table: string, field: string): DeltaRefusal {
  return new DeltaRefusal(
    "KeyColumnClaimed",
    `\`${table}\`.\`${field}\` is part of the primary key; the claim's \`key\` already names it`,
  );
}

export function incompleteInsert(table: string, missing: string[]): DeltaRefusal {
  return new DeltaRefusal(
    "IncompleteInsert",
    `an \`insert\` into \`${table}\` must carry the whole row; ${missing.length} column(s) absent: ${missing.join(", ")}`,
  );
}

export function emptyUpdate(table: string): DeltaRefusal {
  return new DeltaRefusal(
    "EmptyUpdate",
    `an \`update\` on \`${table}\` must claim at least one column`,
  );
}

export function duplicateClaim(table: string, key: string): DeltaRefusal {
  return new DeltaRefusal(
    "DuplicateClaim",
    `\`${table}\` row ${key} is claimed twice by one Package; merge the two claims`,
  );
}
