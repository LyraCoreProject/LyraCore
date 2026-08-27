// The canonical writer has to agree with the Rust one byte for byte, and the float spelling is the
// only part where two languages could plausibly disagree. `fixtures/f32-literals.json` pins that
// agreement: the literals in it were produced by Rust, and both sides assert against them.

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { f32Literal, writeDelta, type Claim } from "../lib/canonical.ts";

const FIXTURES = join(import.meta.dir, "fixtures");

const HASH = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

function f32FromBits(bits: number): number {
  const view = new DataView(new ArrayBuffer(4));
  view.setUint32(0, bits);
  return view.getFloat32(0);
}

test("a float is spelled exactly as Rust's f32 writer spells it", () => {
  const cases: { bits: number; literal: string }[] = JSON.parse(
    readFileSync(join(FIXTURES, "f32-literals.json"), "utf8"),
  );

  for (const { bits, literal } of cases) {
    expect(f32Literal(f32FromBits(bits))).toBe(literal);
  }
});

test("an unsigned 64-bit value travels as a decimal string", () => {
  const claim: Claim = {
    key: { kind: "spell", spellId: 133 },
    operation: "update",
    fields: new Map([["family_flags", { type: "u64", value: 18446744073709551615n }]]),
  };

  expect(writeDelta("example.bolt", HASH, [claim])).toContain(
    '"family_flags":{"type":"u64","value":"18446744073709551615"}',
  );
});

test("claims are ordered by table, then spell, then effect index", () => {
  const claim = (key: Claim["key"]): Claim => ({
    key,
    operation: "update",
    fields: new Map([["cooldown_ms", { type: "u32", value: 1 }]]),
  });

  const written = writeDelta("example.bolt", HASH, [
    claim({ kind: "spellEffect", spellId: 133, effectIndex: 2 }),
    claim({ kind: "spell", spellId: 400 }),
    claim({ kind: "spellEffect", spellId: 133, effectIndex: 0 }),
    claim({ kind: "spell", spellId: 133 }),
  ]);

  const order = [...written.matchAll(/"key":(\{[^}]*\})/g)].map((m) => m[1]);
  expect(order).toEqual([
    '{"spell_id":133}',
    '{"spell_id":400}',
    '{"spell_id":133,"effect_index":0}',
    '{"spell_id":133,"effect_index":2}',
  ]);
});

test("fields are ordered by name, whatever order they were assigned in", () => {
  const claim: Claim = {
    key: { kind: "spell", spellId: 133 },
    operation: "update",
    fields: new Map([
      ["gcd_ms", { type: "u32", value: 1000 }],
      ["cooldown_ms", { type: "u32", value: 1500 }],
      ["attributes", { type: "u32", value: 0 }],
    ]),
  };

  const written = writeDelta("example.bolt", HASH, [claim]);

  const names = [...written.matchAll(/"(\w+)":\{"type"/g)].map((m) => m[1]);
  expect(names).toEqual(["attributes", "cooldown_ms", "gcd_ms"]);
});

test("the artifact carries no whitespace and no trailing newline", () => {
  const written = writeDelta("example.bolt", HASH, []);

  expect(written).toBe(
    `{"version":1,"package":"example.bolt","source_hash":"${HASH}","claims":[]}`,
  );
});
