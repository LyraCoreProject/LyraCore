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

// Each kind names a `DeltaError` variant in `crates/lyracore-package-delta/src/error.rs`.
export type RefusalKind =
  | "InvalidPackageId"
  | "InvalidSourceHash"
  | "MalformedKey"
  | "IdOutsidePackageRange"
  | "IdFixtureReserved"
  | "EffectIndexOutOfRange"
  | "UnknownField"
  | "ValueOutOfRange"
  | "KeyColumnClaimed"
  | "IncompleteInsert"
  | "EmptyUpdate"
  | "DuplicateClaim";

export class DeltaRefusal extends Error {
  readonly kind: RefusalKind;

  constructor(kind: RefusalKind, message: string) {
    super(message);
    this.name = "DeltaRefusal";
    this.kind = kind;
  }
}

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
    "IdOutsidePackageRange",
    `spell ${spellId} is outside the Package spell range ${PACKAGE_SPELL_ID_FLOOR}..=${PACKAGE_SPELL_ID_CEIL}; ` +
      `an inserted spell must use an identifier no client and no import can already own`,
  );
}

export function spellIdFixtureReserved(spellId: number): DeltaRefusal {
  return new DeltaRefusal(
    "IdFixtureReserved",
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
