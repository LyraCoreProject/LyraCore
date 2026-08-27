// The authoring surface, tested against a SYNTHETIC Base Snapshot.
//
// `fixtures/base-snapshot.json` is hand-written and every value in it is invented. It carries the
// same column names and the same canonical spellings a real snapshot would, and no client-derived
// value at all, so these tests never touch the operator's game data.

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { ClaimRegistry } from "../lib/authoring.ts";
import { buildClaim, spellKey } from "../lib/delta.ts";
import { DeltaRefusal, BaseDataError } from "../lib/refusals.ts";
import { BaseSnapshot } from "../lib/snapshot.ts";
import { SPELL_COLUMNS, SPELL_EFFECT_COLUMNS } from "../lib/schema.ts";

const SNAPSHOT_PATH = join(import.meta.dir, "fixtures", "base-snapshot.json");

const BOLT = 133;
const FIXTURE_SPELL = 50_000;
const PACKAGE_SPELL = 6_000_100;

function registry(): ClaimRegistry {
  const snapshot = BaseSnapshot.parse(
    readFileSync(SNAPSHOT_PATH, "utf8"),
    SNAPSHOT_PATH,
  );
  return new ClaimRegistry(snapshot);
}

function refusalKind(run: () => void): string {
  try {
    run();
  } catch (error) {
    if (error instanceof DeltaRefusal) return error.kind;
    throw error;
  }
  throw new Error("expected a refusal");
}

test("an assignment on an existing spell claims that column and no other", () => {
  const claims = registry();
  claims.spell(BOLT).set("cooldownMs", 1500);

  const emitted = claims.claims();

  expect(emitted).toHaveLength(1);
  expect(emitted[0]!.operation).toBe("update");
  expect([...emitted[0]!.fields.keys()]).toEqual(["cooldown_ms"]);
});

test("a spell read but never assigned contributes no claim", () => {
  const claims = registry();
  claims.spell(BOLT);

  expect(claims.claims()).toHaveLength(0);
});

test("a clone carries the whole row and every effect the source has", () => {
  const claims = registry();
  claims.spell(BOLT).clone(PACKAGE_SPELL);

  const emitted = claims.claims();

  expect(emitted).toHaveLength(3); // the header plus the source's two effects
  const header = emitted.find((c) => c.key.kind === "spell")!;
  expect(header.operation).toBe("insert");
  expect([...header.fields.keys()].sort()).toEqual(
    Object.values(SPELL_COLUMNS)
      .map((column) => column.column)
      .sort(),
  );
  for (const effect of emitted.filter((c) => c.key.kind === "spellEffect")) {
    expect(effect.operation).toBe("insert");
    expect(effect.fields.size).toBe(Object.keys(SPELL_EFFECT_COLUMNS).length);
  }
});

test("a cloned effect keeps the source's effect index, so its packed key follows", () => {
  const claims = registry();
  claims.spell(BOLT).clone(PACKAGE_SPELL);

  const indexes = claims
    .claims()
    .filter((c) => c.key.kind === "spellEffect")
    .map((c) => (c.key.kind === "spellEffect" ? c.key.effectIndex : -1));

  expect(indexes.sort()).toEqual([0, 1]);
});

test("a clone reads the source's values and an assignment overwrites just that column", () => {
  const claims = registry();
  const nova = claims.spell(BOLT).clone(PACKAGE_SPELL);
  nova.set("cost", 40);

  const header = claims.claims().find((c) => c.key.kind === "spell")!;

  expect(header.fields.get("cost")).toEqual({ type: "u32", value: 40 });
  expect(header.fields.get("cast_time_ms")).toEqual({ type: "u32", value: 2500 });
  expect(nova.get("name")).toBe("Test Bolt");
});

test("an inserted spell outside the Package Spell Range is refused", () => {
  expect(refusalKind(() => registry().spell(BOLT).clone(200_000))).toBe(
    "SpellIdNotClientSafe",
  );
});

test("a fixture-reserved spell is refused even though the base data holds it", () => {
  expect(refusalKind(() => registry().spell(FIXTURE_SPELL))).toBe(
    "SpellIdFixtureReserved",
  );
});

test("an effect index above the third slot is refused", () => {
  expect(
    refusalKind(() => registry().spell(BOLT).effect(3 as 0 | 1 | 2)),
  ).toBe("EffectIndexOutOfRange");
});

test("a primary-key column is never settable", () => {
  expect(
    refusalKind(() =>
      registry()
        .spell(BOLT)
        .set("spellId" as never, 1 as never),
    ),
  ).toBe("KeyColumnClaimed");
});

test("a column the table does not have is refused", () => {
  expect(
    refusalKind(() =>
      registry()
        .spell(BOLT)
        .set("sharpness" as never, 1 as never),
    ),
  ).toBe("UnknownField");
});

test("a value the column cannot hold is refused", () => {
  expect(refusalKind(() => registry().spell(BOLT).set("powerType", 300))).toBe(
    "ValueOutOfRange",
  );
  expect(refusalKind(() => registry().spell(BOLT).set("cost", -1))).toBe(
    "ValueOutOfRange",
  );
  expect(
    refusalKind(() => registry().spell(BOLT).set("familyFlags", -1n)),
  ).toBe("ValueOutOfRange");
});

test("two clones onto one identifier are refused", () => {
  expect(
    refusalKind(() => {
      const claims = registry();
      claims.spell(BOLT).clone(PACKAGE_SPELL);
      claims.spell(BOLT).clone(PACKAGE_SPELL);
    }),
  ).toBe("DuplicateClaim");
});

test("one spell read twice is one handle, so its columns merge into one claim", () => {
  const claims = registry();
  claims.spell(BOLT).set("cooldownMs", 1500);
  claims.spell(BOLT).set("gcdMs", 1000);

  const emitted = claims.claims();

  expect(emitted).toHaveLength(1);
  expect([...emitted[0]!.fields.keys()].sort()).toEqual(["cooldown_ms", "gcd_ms"]);
});

test("an insert that does not carry the whole row is refused, and names what is absent", () => {
  let refusal: DeltaRefusal | undefined;
  try {
    buildClaim(spellKey(PACKAGE_SPELL), "insert", new Map());
  } catch (error) {
    refusal = error as DeltaRefusal;
  }

  expect(refusal?.kind).toBe("IncompleteInsert");
  expect(refusal?.message).toContain("proc_charges");
  expect(refusal?.message).toContain("name");
});

test("an update that states no change is refused", () => {
  expect(
    refusalKind(() => buildClaim(spellKey(BOLT), "update", new Map())),
  ).toBe("EmptyUpdate");
});

/// The AC in one test: a Package cannot see what another Package invents, because the only rows a
/// Datascript can read are the base ones, and a Package spell is never in the base data.
test("a spell another Package would have invented is not readable", () => {
  expect(() => registry().spell(PACKAGE_SPELL)).toThrow(BaseDataError);
});

test("an effect slot the base row does not fill cannot be claimed", () => {
  expect(() => registry().spell(BOLT).effect(2)).toThrow(BaseDataError);
});
