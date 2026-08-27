// The whole path, end to end: run the Fire Nova Datascript the way `lyracore packages build` runs
// it, and compare the bytes it writes with the frozen artifact.
//
// The frozen artifact is `fixtures/fire-nova.expected.json`. The Rust interop test in
// `crates/lyracore-package-delta/tests/datascript_interop.rs` parses the SAME file, so these bytes
// are the contract between the two languages. Regenerate it whenever the Datascript changes:
//
//   LYRACORE_BASE_SNAPSHOT=datascripts/tests/fixtures/base-snapshot.json \
//   LYRACORE_PACKAGES_ROOT=<scratch dir> \
//   bun run datascripts/src/fire_nova/spells.ts
//
// The Datascript is run as a SUBPROCESS rather than imported, because the source hash it records is
// the entry script's own digest — which only means what it says when the entry script is the
// Datascript.

import { expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const REPO = join(import.meta.dir, "..", "..");
const DATASCRIPT = join(REPO, "datascripts", "src", "fire_nova", "spells.ts");
const SNAPSHOT = join(import.meta.dir, "fixtures", "base-snapshot.json");
const FROZEN = join(import.meta.dir, "fixtures", "fire-nova.expected.json");

/// Run the Datascript against the synthetic snapshot, into a scratch Packages root.
function emit(): { code: number; stderr: string; artifact?: string } {
  const root = mkdtempSync(join(tmpdir(), "lyracore-datascript-"));
  try {
    const result = Bun.spawnSync({
      cmd: [process.execPath, "run", DATASCRIPT],
      env: {
        ...process.env,
        LYRACORE_BASE_SNAPSHOT: SNAPSHOT,
        LYRACORE_PACKAGES_ROOT: root,
      },
    });
    const path = join(root, "fire_nova", "data", ".generated", "spell.json");
    return {
      code: result.exitCode,
      stderr: result.stderr.toString(),
      artifact: result.exitCode === 0 ? readFileSync(path, "utf8") : undefined,
    };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test("the Fire Nova Datascript emits the frozen artifact, byte for byte", () => {
  const { code, stderr, artifact } = emit();

  expect(stderr).toBe("");
  expect(code).toBe(0);
  expect(artifact).toBe(readFileSync(FROZEN, "utf8"));
});

test("emitting twice produces the same bytes", () => {
  expect(emit().artifact).toBe(emit().artifact);
});

test("the artifact claims five ranks, each with its cloned effects", () => {
  const artifact = JSON.parse(readFileSync(FROZEN, "utf8"));

  const spells = artifact.claims.filter((c: { table: string }) => c.table === "game_spell");
  const effects = artifact.claims.filter(
    (c: { table: string }) => c.table === "game_spell_effect",
  );

  expect(spells).toHaveLength(5);
  expect(effects).toHaveLength(10); // the source's two effects, cloned onto each rank
  expect(spells.every((c: { operation: string }) => c.operation === "insert")).toBe(true);
  expect(
    spells.map((c: { key: { spell_id: number } }) => c.key.spell_id),
  ).toEqual([6_000_100, 6_000_101, 6_000_102, 6_000_103, 6_000_104]);
});

test("a Datascript that is refused writes nothing and exits non-zero", () => {
  const root = mkdtempSync(join(tmpdir(), "lyracore-datascript-"));
  const script = join(root, "refused.ts");
  Bun.write(
    script,
    `import { run } from ${JSON.stringify(join(REPO, "datascripts", "lib", "index.ts"))};\n` +
      `await run("fire_nova", (data) => { data.spell(133).clone(200000); });\n`,
  );

  try {
    const result = Bun.spawnSync({
      cmd: [process.execPath, "run", script],
      env: {
        ...process.env,
        LYRACORE_BASE_SNAPSHOT: SNAPSHOT,
        LYRACORE_PACKAGES_ROOT: root,
      },
    });

    expect(result.exitCode).not.toBe(0);
    expect(result.stderr.toString()).toContain("Package spell range");
    expect(() =>
      readFileSync(join(root, "fire_nova", "data", ".generated", "spell.json")),
    ).toThrow();
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
