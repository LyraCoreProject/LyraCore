// The Datascript authoring library: what a Datascript imports.
//
// A Datascript is one call:
//
// ```ts
// import { run } from "../../lib/index.ts";
//
// await run("fire_nova", (data) => {
//   const fireball = data.spell(133);
//   const nova = fireball.clone(6_000_100);
//   nova.set("name", "Fire Nova");
//   nova.effect(0).set("radiusYd", 8);
// });
// ```
//
// `run` reads the Base Snapshot, runs the script, and writes ONE canonical Package Delta artifact to
// `packages/<package>/data/.generated/spell.json`. On any refusal it throws and writes nothing, so a
// Package folder never holds a half-built artifact and the build exits non-zero.
//
// # Determinism
//
// Nothing here reads a clock, a random source or the environment beyond the two paths below, and the
// canonical writer fixes every byte. The same Datascript over the same Base Snapshot produces the
// same artifact, which is what lets an artifact be compared, hashed and diffed.
//
// # Where the paths come from
//
// Defaults are the repository's own layout, resolved from this file. `LYRACORE_BASE_SNAPSHOT` and
// `LYRACORE_PACKAGES_ROOT` override them; the build harness sets them when it runs a Datascript
// somewhere other than a checkout, and the library's own tests use them to work on a fixture.

import { dirname, join, resolve } from "node:path";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";

import { ClaimRegistry, type SpellAuthor } from "./authoring.ts";
import { buildDelta, validatePackageId } from "./delta.ts";
import { BaseSnapshot } from "./snapshot.ts";

export { DeltaRefusal, BaseDataError } from "./refusals.ts";
export type { EffectIndex, SpellAuthor, SpellEffectAuthor } from "./authoring.ts";

/// The base data a Datascript reads. `spell` is the whole surface on purpose: there is no reader for
/// another Package's generated Delta, so one Package cannot observe another's claims.
export interface Data {
  spell(spellId: number): SpellAuthor;
}

/// The artifact file one Datascript writes, named after the Import Family it claims rows in.
const ARTIFACT_NAME = "spell.json";

/// Where a Package's generated artifacts live, relative to the Package folder. The same path the
/// importer's Package Delta stage reads.
const GENERATED_DIR = "data/.generated";

/// Run a Datascript and write its Package Delta.
///
/// `packageName` names the Package folder under the Packages root AND is the Package identity the
/// artifact carries, so there is no second name to keep in step.
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
          `\`lyracore-importer --dbc <client Data/ dir> --spell-snapshot ${snapshotPath}\`.`,
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

/// The digest of the Datascript that is running: the entry file's bytes, hashed whole.
///
/// It records WHICH source produced an artifact, so a replay can tell that a Package's source moved
/// without diffing every claim. It covers the entry file rather than the whole import graph, which
/// is the honest promise: a change to a helper the Datascript imports does not move it.
async function sourceHash(): Promise<string> {
  const source = await readFile(entryScript());
  return createHash("sha256").update(source).digest("hex");
}

/// The Datascript file the runtime was started with.
function entryScript(): string {
  const main = (globalThis as { Bun?: { main?: string } }).Bun?.main;
  if (main) return main;
  const [, script] = process.argv;
  if (!script) {
    throw new Error("cannot tell which Datascript is running: no entry script");
  }
  return script;
}

/// The repository root, from this file's own location.
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
