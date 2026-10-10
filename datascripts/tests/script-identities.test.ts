import { expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { buildPackageScripts } from "../runtime-scripts/build-scripts.ts";

const PACKAGE = "example";
const SOURCE = "local function welcome(event) end\nevents.player.onLogin(welcome)\n";

interface Artifact {
  scripts: Array<{ script_id: number; name: string }>;
}

async function scratch(run: (dir: string, build: () => Promise<Artifact>) => Promise<void>): Promise<void> {
  const root = mkdtempSync(join(tmpdir(), "script-identities-"));
  const before = process.env.LYRACORE_PACKAGES_ROOT;
  const dir = join(root, PACKAGE);
  mkdirSync(join(dir, "scripts"), { recursive: true });
  mkdirSync(join(dir, "data/.generated"), { recursive: true });
  process.env.LYRACORE_PACKAGES_ROOT = root;
  try {
    await run(dir, async () => JSON.parse(readFileSync(await buildPackageScripts(PACKAGE), "utf8")));
  } finally {
    if (before === undefined) delete process.env.LYRACORE_PACKAGES_ROOT;
    else process.env.LYRACORE_PACKAGES_ROOT = before;
    rmSync(root, { recursive: true, force: true });
  }
}

function priorArtifact(dir: string, filename: string, ids: Record<string, number>, packageName = PACKAGE): void {
  writeFileSync(join(dir, "data/.generated", filename), JSON.stringify({
    kind: "script",
    version: 1,
    package: packageName,
    source_hash: "0".repeat(64),
    scripts: Object.entries(ids).map(([stem, script_id]) => ({
      script_id,
      name: `${packageName}.${stem}`,
      event: "on_login",
      priority: 0,
      enabled: true,
      source: "return",
    })),
  }));
}

function ledger(dir: string): Record<string, number> {
  return JSON.parse(readFileSync(join(dir, "script-ids.json"), "utf8")).ids;
}

test("migration preserves noncanonical artifact IDs and retired source reservations", async () => {
  await scratch(async (dir, build) => {
    priorArtifact(dir, "legacy.json", { welcome: 100300, retired: 941145 });
    writeFileSync(join(dir, "scripts/welcome.lua"), SOURCE);
    writeFileSync(join(dir, "scripts/s101.lua"), SOURCE);
    writeFileSync(join(dir, "data/.generated/spells.json"), JSON.stringify({ version: 1, claims: [] }));

    await build();

    expect(ledger(dir)).toEqual({ welcome: 100300, retired: 941145, s101: 941146 });
    rmSync(join(dir, "data/.generated/legacy.json"));
    expect((await build()).scripts.find((script) => script.name === "example.welcome")!.script_id).toBe(100300);
    expect(ledger(dir).retired).toBe(941145);
  });
});

test("migration ignores internal transition backups and unrelated JSON", async () => {
  await scratch(async (dir, build) => {
    priorArtifact(dir, ".lyracore-script-build-backup-test", { welcome: 100300 });
    priorArtifact(dir, "prior.json.lyracore-script-build-backup", { retired: 100301 });
    writeFileSync(join(dir, "data/.generated/.lyracore-script-build-backup-sidecar"), '{"version":1}');
    writeFileSync(join(dir, "data/.generated/notes.json"), "invalid JSON");
    writeFileSync(join(dir, "scripts/welcome.lua"), SOURCE);

    await build();

    expect(ledger(dir)).not.toHaveProperty("retired");
    expect(ledger(dir).welcome).not.toBe(100300);
  });
});

test("new identities with the same hash receive distinct durable IDs", async () => {
  await scratch(async (dir, build) => {
    writeFileSync(join(dir, "scripts/s101.lua"), SOURCE);
    writeFileSync(join(dir, "scripts/s1014.lua"), SOURCE);

    const first = await build();

    expect(ledger(dir)).toEqual({ s101: 941145, s1014: 941146 });
    expect(await build()).toEqual(first);
  });
});

test("legacy directives reserve IDs before an earlier new source allocates", async () => {
  await scratch(async (dir, build) => {
    writeFileSync(join(dir, "scripts/s101.lua"), SOURCE);
    writeFileSync(join(dir, "scripts/zlegacy.lua"), "-- @event on_login\n-- @id 941145\nreturn\n");

    await build();

    expect(ledger(dir)).toEqual({ s101: 941146, zlegacy: 941145 });
  });
});

test("two Packages with the same initial ID keep distinct recorded IDs on rebuild", async () => {
  await scratch(async (dir, build) => {
    const sibling = join(dirname(dir), "another");
    mkdirSync(join(sibling, "scripts"), { recursive: true });
    writeFileSync(join(sibling, "scripts/s1204858.lua"), SOURCE);
    writeFileSync(join(dir, "scripts/s101.lua"), SOURCE);

    await buildPackageScripts("another");
    const first = await build();

    expect(ledger(sibling)).toEqual({ s1204858: 941145 });
    expect(ledger(dir)).toEqual({ s101: 941146 });
    await buildPackageScripts("another");
    expect(await build()).toEqual(first);
    expect(ledger(sibling)).toEqual({ s1204858: 941145 });
  });
});

test("a sibling's removed source keeps its ID reserved in the ledger", async () => {
  await scratch(async (dir, build) => {
    const sibling = join(dirname(dir), "another");
    mkdirSync(sibling);
    const saved = JSON.stringify({ version: 1, package: "another", ids: { retired: 941145 } });
    writeFileSync(join(sibling, "script-ids.json"), saved);
    writeFileSync(join(dir, "scripts/s101.lua"), SOURCE);

    await build();

    expect(ledger(dir)).toEqual({ s101: 941146 });
    expect(readFileSync(join(sibling, "script-ids.json"), "utf8")).toBe(saved);
  });
});

test.each([
  ["legacy.lua", "-- @event on_login\n-- @id 941145\nreturn\n"],
  ["legacy.ts", "// @event on_login\n// @id 941145\nfunction script(): void {}\n"],
])("a sibling's %s reserves its legacy ID before compilation", async (file, source) => {
  await scratch(async (dir, build) => {
    const sibling = join(dirname(dir), "another");
    mkdirSync(join(sibling, "scripts"), { recursive: true });
    writeFileSync(join(sibling, "scripts", file), source);
    writeFileSync(join(dir, "scripts/s101.lua"), SOURCE);

    await build();

    expect(ledger(dir)).toEqual({ s101: 941146 });
    expect(readFileSync(join(sibling, "scripts", file), "utf8")).toBe(source);
  });
});

test("a source-free sibling artifact reserves IDs without a ledger", async () => {
  await scratch(async (dir, build) => {
    const sibling = join(dirname(dir), "another");
    mkdirSync(join(sibling, "data/.generated"), { recursive: true });
    priorArtifact(sibling, "personality.json", { welcome: 941145 }, "another");
    writeFileSync(join(dir, "scripts/s101.lua"), SOURCE);

    await build();

    expect(ledger(dir)).toEqual({ s101: 941146 });
  });
});

test("Package directory symlinks reserve their recorded IDs", async () => {
  await scratch(async (dir, build) => {
    const collection = join(dirname(dir), ".collection", "another");
    mkdirSync(collection, { recursive: true });
    writeFileSync(join(collection, "script-ids.json"), JSON.stringify({
      version: 1, package: "another", ids: { retired: 941145 },
    }));
    symlinkSync(collection, join(dirname(dir), "another"), "dir");
    writeFileSync(join(dir, "scripts/s101.lua"), SOURCE);

    await build();

    expect(ledger(dir)).toEqual({ s101: 941146 });
  });
});

test("contradictory recorded IDs name both Packages and leave their identities intact", async () => {
  await scratch(async (dir, build) => {
    const sibling = join(dirname(dir), "another");
    mkdirSync(sibling);
    const currentIds = JSON.stringify({ version: 1, package: PACKAGE, ids: { s101: 941145 } });
    const siblingIds = JSON.stringify({ version: 1, package: "another", ids: { retired: 941145 } });
    writeFileSync(join(dir, "script-ids.json"), currentIds);
    writeFileSync(join(sibling, "script-ids.json"), siblingIds);
    writeFileSync(join(dir, "scripts/s101.lua"), SOURCE);

    await expect(build()).rejects.toThrow(/another\.retired collides with example\.s101.*941145/);

    expect(readFileSync(join(dir, "script-ids.json"), "utf8")).toBe(currentIds);
    expect(readFileSync(join(sibling, "script-ids.json"), "utf8")).toBe(siblingIds);
  });
});

test("conflicting stored identities refuse instead of reallocating published IDs", async () => {
  await scratch(async (dir, build) => {
    const recorded = JSON.stringify({ version: 1, package: PACKAGE, ids: { s101: 941145, s1014: 941145 } });
    writeFileSync(join(dir, "script-ids.json"), recorded);
    writeFileSync(join(dir, "scripts/s101.lua"), SOURCE);

    await expect(build()).rejects.toThrow(/collides.*941145/);

    expect(readFileSync(join(dir, "script-ids.json"), "utf8")).toBe(recorded);
  });
});

test("disagreement between prior artifacts refuses migration", async () => {
  await scratch(async (dir, build) => {
    priorArtifact(dir, "first.json", { welcome: 100300 });
    priorArtifact(dir, "second.json", { welcome: 100301 });
    writeFileSync(join(dir, "scripts/welcome.lua"), SOURCE);

    await expect(build()).rejects.toThrow(/already has script ID 100300/);
  });
});

test.each([
  ["invalid JSON", "{", /invalid JSON/],
  ["a different artifact kind", '{"version":1,"claims":[]}', /cannot recover script identities/],
])("the canonical artifact refuses %s", async (_description, contents, refusal) => {
  await scratch(async (dir, build) => {
    writeFileSync(join(dir, "data/.generated/example.script.json"), contents);
    writeFileSync(join(dir, "scripts/welcome.lua"), SOURCE);

    await expect(build()).rejects.toThrow(refusal);
  });
});
