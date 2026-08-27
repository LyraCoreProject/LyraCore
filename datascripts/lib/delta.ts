// Building a Package Delta from claims, with the refusals `crates/lyracore-package-delta` applies
// to the same artifact.
//
// Every check here has a counterpart in `delta.rs`: `spellKey` / `spellEffectKey` mirror
// `PrimaryKey`'s constructors, `narrowValue` mirrors `parse_field_value`'s range narrowing,
// `buildClaim` mirrors `Claim::new`, and `buildDelta` mirrors `PackageDelta::new`. An artifact this
// module produces parses; an artifact it refuses would have been refused at import.

import {
  isFixtureReservedSpellId,
  isPackageSpellId,
  MAX_SPELL_EFFECT_INDEX,
} from "./ids.ts";
import {
  duplicateClaim,
  emptyUpdate,
  incompleteInsert,
  invalidPackageId,
  invalidSourceHash,
  keyColumnClaimed,
  malformedKey,
  spellIdFixtureReserved,
  spellIdNotClientSafe,
  unknownField,
  valueOutOfRange,
  effectIndexOutOfRange,
} from "./refusals.ts";
import {
  SPELL_COLUMNS,
  SPELL_EFFECT_COLUMNS,
  SPELL_EFFECT_KEY_COLUMNS,
  SPELL_EFFECT_TABLE,
  SPELL_KEY_COLUMNS,
  SPELL_TABLE,
  type Column,
  type FieldType,
} from "./schema.ts";
import {
  describeKey,
  tableOf,
  writeDelta,
  type Claim,
  type FieldValue,
  type PrimaryKey,
} from "./canonical.ts";

const U64_CEIL = (1n << 64n) - 1n;

/// Accepts 1 to 64 characters of `a-z`, `0-9`, `-`, `_` or `.`. Lowercase on purpose: two Packages
/// differing only in case would look like the same Package in a conflict report.
export function validatePackageId(id: string): string {
  if (!/^[a-z0-9._-]{1,64}$/.test(id)) throw invalidPackageId(id);
  return id;
}

/// Accepts exactly 64 lowercase hexadecimal characters.
export function validateSourceHash(hex: string): string {
  if (!/^[0-9a-f]{64}$/.test(hex)) throw invalidSourceHash(hex);
  return hex;
}

/// Names a spell header row.
export function spellKey(spellId: number): PrimaryKey {
  checkClaimableSpellId(SPELL_TABLE, spellId);
  return { kind: "spell", spellId };
}

/// Names one effect of a spell. The packed durable key follows from the two components.
export function spellEffectKey(spellId: number, effectIndex: number): PrimaryKey {
  checkClaimableSpellId(SPELL_EFFECT_TABLE, spellId);
  if (
    !Number.isInteger(effectIndex) ||
    effectIndex < 0 ||
    effectIndex > MAX_SPELL_EFFECT_INDEX
  ) {
    throw effectIndexOutOfRange(effectIndex);
  }
  return { kind: "spellEffect", spellId, effectIndex };
}

/// A spell identifier is refused the same way under every operation when it is 0 or fixture-owned.
function checkClaimableSpellId(table: string, spellId: number): void {
  if (!Number.isInteger(spellId) || spellId < 0 || spellId > 0xffff_ffff) {
    throw malformedKey(
      table,
      `\`spell_id\` must be a whole number in 0..=4294967295, found \`${spellId}\``,
    );
  }
  if (spellId === 0) throw malformedKey(table, "`spell_id` 0 is not a spell");
  if (isFixtureReservedSpellId(spellId)) throw spellIdFixtureReserved(spellId);
}

/// The claimable columns of a table, by their generated names.
export function columnsOf(table: string): Record<string, Column> {
  return table === SPELL_TABLE ? SPELL_COLUMNS : SPELL_EFFECT_COLUMNS;
}

/// Resolve a generated field name to its column, refusing a name the table does not have and a name
/// the primary key already carries.
export function resolveColumn(table: string, field: string): Column {
  const column = columnsOf(table)[field];
  if (column) return column;

  const keyColumns =
    table === SPELL_TABLE ? SPELL_KEY_COLUMNS : SPELL_EFFECT_KEY_COLUMNS;
  const durable = camelToSnake(field);
  if (keyColumns.includes(durable)) throw keyColumnClaimed(table, durable);
  throw unknownField(table, field);
}

/// The durable spelling of a generated name, used only to recognise a primary-key column so the
/// refusal can name it the way the schema does.
function camelToSnake(field: string): string {
  return field.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
}

/// Narrow an author's value to the column's type, refusing anything that does not fit — the same
/// bounds the parser applies to the same claim.
export function narrowValue(column: Column, value: unknown): FieldValue {
  const { type } = column;
  switch (type) {
    case "u8":
      return unsignedValue(column, value, 0xffn);
    case "u16":
      return unsignedValue(column, value, 0xffffn);
    case "u32":
      return unsignedValue(column, value, 0xffff_ffffn);
    case "u64":
      return unsignedValue(column, value, U64_CEIL);
    case "i32":
      if (!Number.isInteger(value) || (value as number) < -2_147_483_648 || (value as number) > 2_147_483_647) {
        throw outOfRange(column, value);
      }
      return { type, value: value as number };
    case "f32": {
      if (typeof value !== "number" || !Number.isFinite(Math.fround(value))) {
        throw outOfRange(column, value);
      }
      return { type, value: Math.fround(value) };
    }
    case "bool":
      if (typeof value !== "boolean") throw outOfRange(column, value);
      return { type, value };
    case "string":
      if (typeof value !== "string") throw outOfRange(column, value);
      return { type, value };
  }
}

/// An unsigned column takes a whole number or a `bigint`. `u64` carries a `bigint` because a JSON
/// number loses precision above 2^53; the narrower widths carry a number, which is what the
/// generated types declare.
function unsignedValue(column: Column, value: unknown, ceiling: bigint): FieldValue {
  let wide: bigint;
  if (typeof value === "bigint") {
    wide = value;
  } else if (typeof value === "number" && Number.isSafeInteger(value)) {
    wide = BigInt(value);
  } else {
    throw outOfRange(column, value);
  }
  if (wide < 0n || wide > ceiling) throw outOfRange(column, value);
  return column.type === "u64"
    ? { type: "u64", value: wide }
    : { type: column.type, value: Number(wide) };
}

function outOfRange(column: Column, value: unknown) {
  return valueOutOfRange(column.column, column.type as FieldType, describe(value));
}

/// A value as a refusal quotes it. Mirrors the parser quoting the raw JSON it read.
function describe(value: unknown): string {
  if (typeof value === "string") return JSON.stringify(value);
  if (typeof value === "bigint") return value.toString();
  if (value === undefined) return "undefined";
  return JSON.stringify(value) ?? String(value);
}

/// Build a claim, refusing everything the artifact promises it will refuse.
///
/// The identifier policy is operation-dependent, and this is the only place it is applied: an
/// `insert` invents a row, so its spell must sit in the Package Spell Range; an `update` changes a
/// row someone else owns, so any identifier is allowed except a fixture-reserved one.
export function buildClaim(
  key: PrimaryKey,
  operation: "insert" | "update",
  fields: Map<string, FieldValue>,
): Claim {
  const table = tableOf(key);
  if (operation === "insert" && !isPackageSpellId(key.spellId)) {
    throw spellIdNotClientSafe(key.spellId);
  }

  if (operation === "insert") {
    const missing = Object.values(columnsOf(table))
      .map((column) => column.column)
      .filter((name) => !fields.has(name));
    if (missing.length > 0) throw incompleteInsert(table, missing);
  } else if (fields.size === 0) {
    throw emptyUpdate(table);
  }

  return { key, operation, fields };
}

/// The whole artifact, in canonical form. Refuses a Package that names one row twice: a Package
/// states each row once and lists every column it sets there.
export function buildDelta(
  packageId: string,
  sourceHash: string,
  claims: Claim[],
): string {
  const seen = new Set<string>();
  for (const claim of claims) {
    const key = `${tableOf(claim.key)}${describeKey(claim.key)}`;
    if (seen.has(key)) {
      throw duplicateClaim(tableOf(claim.key), describeKey(claim.key));
    }
    seen.add(key);
  }
  return writeDelta(
    validatePackageId(packageId),
    validateSourceHash(sourceHash),
    claims,
  );
}
