import { expect, test } from "bun:test";
import { packedSpellEffectId } from "../lib/ids.ts";

test("a spell effect id is the spell shifted left by two plus the effect index", () => {
  expect(packedSpellEffectId(133, 0)).toBe(532n);
  expect(packedSpellEffectId(133, 1)).toBe(533n);
  expect(packedSpellEffectId(133, 2)).toBe(534n);
});

test("a package spell effect id keeps its full value", () => {
  expect(packedSpellEffectId(6_000_001, 2)).toBe(24_000_006n);
});
