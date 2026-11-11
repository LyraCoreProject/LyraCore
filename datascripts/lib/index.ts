import { dirname, join, resolve } from "node:path";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";

import { ClaimRegistry, type SpellAuthor } from "./authoring.ts";
import { buildDelta, validatePackageId } from "./delta.ts";
import { BaseSnapshot } from "./snapshot.ts";

export { DeltaRefusal, BaseDataError } from "./refusals.ts";
export type { EffectIndex, SpellAuthor, SpellEffectAuthor } from "./authoring.ts";

export interface Data {
  spell(spellId: number): SpellAuthor;
}

const ARTIFACT_NAME = "spell.json";

const GENERATED_DIR = "data/.generated";

export async function run(
  packageName: string,
  script: (data: Data) => void,
): Promise<string> {
  validatePackageId(packageName);

  const snapshotPath = baseSnapshotPath();
  const snapshot = BaseSnapshot.parse(
    await readFile(snapshotPath, "utf8").catch((cause) => {
      throw new Error(
        `cannot read the Base Snapshot at ${snapshotPath}: ${cause}. Generate it with ` +
          `\`lyracore-importer --dump <vmangos.sqlite> --dbc <client Data/ dir> --spell-snapshot ` +
          `${snapshotPath}\`.`,
      );
    }),
    snapshotPath,
  );

  const registry = new ClaimRegistry(snapshot);
  script({ spell: (spellId: number) => registry.spell(spellId) });

  const artifact = buildDelta(
    packageName,
    await sourceHash(),
    registry.claims(),
  );

  const path = join(packagesRoot(), packageName, GENERATED_DIR, ARTIFACT_NAME);
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, artifact);
  return path;
}

// Hash only the entry file; imported helpers are outside this source hash.
async function sourceHash(): Promise<string> {
  const source = await readFile(entryScript());
  return createHash("sha256").update(source).digest("hex");
}

function entryScript(): string {
  const main = (globalThis as { Bun?: { main?: string } }).Bun?.main;
  if (main) return main;
  const [, script] = process.argv;
  if (!script) {
    throw new Error("cannot tell which Datascript is running: no entry script");
  }
  return script;
}

function repoRoot(): string {
  return resolve(dirname(new URL(import.meta.url).pathname), "..", "..");
}

function baseSnapshotPath(): string {
  return (
    process.env.LYRACORE_BASE_SNAPSHOT ??
    join(repoRoot(), "datascripts", "generated", "base-snapshot.json")
  );
}

function packagesRoot(): string {
  return process.env.LYRACORE_PACKAGES_ROOT ?? join(repoRoot(), "packages");
}
