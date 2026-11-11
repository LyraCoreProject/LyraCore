import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import * as ids from "../lib/ids.ts";
import { packedSpellEffectId } from "../lib/ids.ts";

test("a spell effect id is the spell shifted left by two plus the effect index", () => {
  expect(packedSpellEffectId(133, 0)).toBe(532n);
  expect(packedSpellEffectId(133, 1)).toBe(533n);
  expect(packedSpellEffectId(133, 2)).toBe(534n);
});

test("a package spell effect id keeps its full value", () => {
  expect(packedSpellEffectId(6_000_001, 2)).toBe(24_000_006n);
});

test("the identifier bounds match the Rust Package Identifier Ranges", () => {
  const source = readFileSync(
    join(import.meta.dir, "..", "..", "crates", "lyracore-package-delta", "src", "ids.rs"),
    "utf8",
  );
  const names = [
    "MAX_SPELL_EFFECT_INDEX",
    "PACKAGE_SPELL_ID_FLOOR",
    "PACKAGE_SPELL_ID_CEIL",
    "FIXTURE_SPELL_ID_FLOOR",
    "FIXTURE_SPELL_ID_CEIL",
    "FIXTURE_RESERVED_ID_FLOOR",
    "FIXTURE_RESERVED_ID_CEIL",
  ] as const;
  for (const name of names) {
    const match = source.match(new RegExp(`^pub const ${name}: \\w+ = ([\\d_]+);$`, "m"));
    expect(match, name).not.toBeNull();
    expect(ids[name], name).toBe(Number(match![1].replaceAll("_", "")));
  }
});
