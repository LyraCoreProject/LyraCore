// The canonical form of a Package Delta, written to byte-match
// `crates/lyracore-package-delta/src/canonical.rs`.
//
// Byte equality is the interop contract, not a nicety: the importer re-canonicalizes every artifact
// it reads and the Module hashes what it applies, so an artifact this library writes and the same
// artifact round-tripped through Rust must be the same bytes.
//
// The rules, all of them, in the same order the Rust writer states them:
//
//  * No whitespace anywhere, and no trailing newline.
//  * Object members appear in a fixed declared order; `fields` members appear sorted by name.
//  * Claims appear sorted by table, then spell, then effect index.
//  * Integers are plain decimal.
//  * An unsigned 64-bit value is a decimal string with no sign, no padding and no separators.
//  * A float is the shortest decimal that reads back as the same `f32`, always with a decimal
//    point.
//  * A string escapes only what JSON requires, using the short escape where one exists.

import { SPELL_EFFECT_TABLE, SPELL_TABLE, type FieldType } from "./schema.ts";

/// The artifact version this library writes.
export const DELTA_VERSION = 1;

/// One claimed column value, already narrowed to the column's type.
export interface FieldValue {
  readonly type: FieldType;
  readonly value: number | bigint | boolean | string;
}

/// The row a claim names, typed by its table.
export type PrimaryKey =
  | { readonly kind: "spell"; readonly spellId: number }
  | {
      readonly kind: "spellEffect";
      readonly spellId: number;
      readonly effectIndex: number;
    };

/// One Package's claim on one row. The table follows from the key, so a claim cannot name a table
/// its key does not describe.
export interface Claim {
  readonly key: PrimaryKey;
  readonly operation: "insert" | "update";
  readonly fields: Map<string, FieldValue>;
}

/// The durable table a key's row lives in.
export function tableOf(key: PrimaryKey): string {
  return key.kind === "spell" ? SPELL_TABLE : SPELL_EFFECT_TABLE;
}

/// The row as a conflict report writes it, which is also how a duplicate-claim refusal names it.
export function describeKey(key: PrimaryKey): string {
  return key.kind === "spell"
    ? `{spell_id=${key.spellId}}`
    : `{spell_id=${key.spellId}, effect_index=${key.effectIndex}}`;
}

/// The whole artifact in canonical form. `claims` is sorted here rather than by the caller, so the
/// order is a property of the writer and not of the order an author happened to write things in.
export function writeDelta(
  packageId: string,
  sourceHash: string,
  claims: Claim[],
): string {
  const ordered = [...claims].sort(compareKeys);
  const parts = ordered.map(writeClaim).join(",");
  return `{"version":${DELTA_VERSION},"package":${writeString(packageId)},"source_hash":${writeString(sourceHash)},"claims":[${parts}]}`;
}

/// The canonical claim order: every `game_spell` row before every `game_spell_effect` row, then by
/// spell, then by effect index. Mirrors the derived `Ord` on the Rust `PrimaryKey`.
export function compareKeys(a: Claim, b: Claim): number {
  const rank = (claim: Claim) => (claim.key.kind === "spell" ? 0 : 1);
  if (rank(a) !== rank(b)) return rank(a) - rank(b);
  if (a.key.spellId !== b.key.spellId) return a.key.spellId - b.key.spellId;
  if (a.key.kind === "spellEffect" && b.key.kind === "spellEffect") {
    return a.key.effectIndex - b.key.effectIndex;
  }
  return 0;
}

function writeClaim(claim: Claim): string {
  const names = [...claim.fields.keys()].sort();
  const fields = names
    .map((name) => `${writeString(name)}:${writeValue(claim.fields.get(name)!)}`)
    .join(",");
  return `{"table":${writeString(tableOf(claim.key))},"key":${writeKey(claim.key)},"operation":${writeString(claim.operation)},"fields":{${fields}}}`;
}

function writeKey(key: PrimaryKey): string {
  return key.kind === "spell"
    ? `{"spell_id":${key.spellId}}`
    : `{"spell_id":${key.spellId},"effect_index":${key.effectIndex}}`;
}

function writeValue(value: FieldValue): string {
  return `{"type":${writeString(value.type)},"value":${scalarLiteral(value)}}`;
}

/// One claimed value in its canonical JSON spelling, without the surrounding type tag.
export function scalarLiteral(value: FieldValue): string {
  switch (value.type) {
    case "u64":
      // A JSON number loses precision above 2^53, so an unsigned 64-bit value travels as a string.
      return `"${value.value}"`;
    case "f32":
      return f32Literal(value.value as number);
    case "bool":
      return value.value ? "true" : "false";
    case "string":
      return writeString(value.value as string);
    default:
      return String(value.value);
  }
}

/// A float as Rust's `f32` `Display` writes it, plus the decimal point Rust's writer appends: the
/// shortest decimal that reads back as the same `f32`, never in exponent form.
export function f32Literal(input: number): string {
  const text = shortestF32(Math.fround(input));
  return text.includes(".") ? text : `${text}.0`;
}

/// The shortest decimal that round-trips through `f32`. `toPrecision` is correctly rounded, so the
/// first precision that survives the round trip is the same digit string Rust's shortest-repr writer
/// produces.
function shortestF32(value: number): string {
  if (!Number.isFinite(value)) {
    throw new RangeError(`${value} is not a finite f32`);
  }
  if (Object.is(value, -0)) return "-0";
  for (let precision = 1; precision <= 9; precision++) {
    const candidate = value.toPrecision(precision);
    if (Math.fround(Number(candidate)) === value) return plainDecimal(candidate);
  }
  return plainDecimal(value.toPrecision(9));
}

/// The same value with the exponent multiplied out and no padding zeros. Rust's float `Display`
/// never uses exponent form, so neither may this.
function plainDecimal(text: string): string {
  let sign = "";
  let body = text;
  if (body.startsWith("-")) {
    sign = "-";
    body = body.slice(1);
  }

  let exponent = 0;
  const marker = body.indexOf("e");
  if (marker >= 0) {
    exponent = Number(body.slice(marker + 1));
    body = body.slice(0, marker);
  }

  const dot = body.indexOf(".");
  let digits = dot >= 0 ? body.slice(0, dot) + body.slice(dot + 1) : body;
  let pointAt = (dot >= 0 ? dot : body.length) + exponent;

  while (pointAt <= 0) {
    digits = `0${digits}`;
    pointAt += 1;
  }
  while (pointAt > digits.length) {
    digits = `${digits}0`;
  }

  const whole = digits.slice(0, pointAt).replace(/^0+(?=\d)/, "");
  const fraction = digits.slice(pointAt).replace(/0+$/, "");
  return sign + whole + (fraction ? `.${fraction}` : "");
}

/// A JSON string escaping only what JSON requires, with the short escape where one exists — the
/// exact set the Rust writer emits.
function writeString(text: string): string {
  return JSON.stringify(text);
}
