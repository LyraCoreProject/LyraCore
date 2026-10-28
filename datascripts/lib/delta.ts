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

// Lowercase Package identities avoid case-only conflicts.
export function validatePackageId(id: string): string {
  if (!/^[a-z0-9._-]{1,64}$/.test(id)) throw invalidPackageId(id);
  return id;
}

export function validateSourceHash(hex: string): string {
  if (!/^[0-9a-f]{64}$/.test(hex)) throw invalidSourceHash(hex);
  return hex;
}

export function spellKey(spellId: number): PrimaryKey {
  checkClaimableSpellId(SPELL_TABLE, spellId);
  return { kind: "spell", spellId };
}

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

export function columnsOf(table: string): Record<string, Column> {
  return table === SPELL_TABLE ? SPELL_COLUMNS : SPELL_EFFECT_COLUMNS;
}

export function resolveColumn(table: string, field: string): Column {
  const column = columnsOf(table)[field];
  if (column) return column;

  const keyColumns =
    table === SPELL_TABLE ? SPELL_KEY_COLUMNS : SPELL_EFFECT_KEY_COLUMNS;
  const durable = camelToSnake(field);
  if (keyColumns.includes(durable)) throw keyColumnClaimed(table, durable);
  throw unknownField(table, field);
}

function camelToSnake(field: string): string {
  return field.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
}

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

// u64 requires bigint because number loses integer precision above 2^53.
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

function describe(value: unknown): string {
  if (typeof value === "string") return JSON.stringify(value);
  if (typeof value === "bigint") return value.toString();
  if (value === undefined) return "undefined";
  return JSON.stringify(value) ?? String(value);
}

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
      .filter((column) => column.defaultValue === undefined)
      .map((column) => column.column)
      .filter((name) => !fields.has(name));
    if (missing.length > 0) throw incompleteInsert(table, missing);
  } else if (fields.size === 0) {
    throw emptyUpdate(table);
  }

  return { key, operation, fields };
}

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
