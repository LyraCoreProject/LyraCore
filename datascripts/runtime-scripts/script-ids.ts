import { createHash } from "node:crypto";
import { readFile, readdir, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { readDirectives } from "./directives.ts";

export const SCRIPT_IDS_FILE = "script-ids.json";
export const SCRIPT_ID_FLOOR = 100_000;
export const SCRIPT_ID_CEIL = 999_999;

interface ScriptOwner {
  readonly package: string;
  readonly stem: string;
}

interface ScriptIds {
  version: 1;
  package: string;
  ids: Record<string, number>;
  reservations: Map<number, ScriptOwner>;
}

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function refusal(path: string, message: string): never {
  throw new Error(`${path}: ${message}`);
}

async function readJson(path: string): Promise<unknown | undefined> {
  let contents: string;
  try {
    contents = await readFile(path, "utf8");
  } catch (error) {
    if (object(error) && error.code === "ENOENT") return undefined;
    throw error;
  }
  try {
    return JSON.parse(contents);
  } catch {
    return refusal(path, "invalid JSON; restore the recorded script identities before building");
  }
}

async function readPackageIds(packageDir: string, packageName: string): Promise<ScriptIds> {
  const path = join(packageDir, SCRIPT_IDS_FILE);
  const saved = await readJson(path);
  const ledger: ScriptIds = { version: 1, package: packageName, ids: Object.create(null), reservations: new Map() };
  if (saved !== undefined) {
    if (!object(saved) || saved.version !== 1 || saved.package !== packageName || !object(saved.ids)
      || Object.keys(saved).some((key) => !["version", "package", "ids"].includes(key))) {
      refusal(path, `expected version 1 identities owned by Package ${packageName}; use packages new for an independent copy`);
    }
    for (const [stem, id] of Object.entries(saved.ids)) recordId(path, ledger, stem, id);
  }

  const generated = join(packageDir, "data", ".generated");
  const canonicalName = `${packageName}.script.json`;
  const entries = await readdir(generated, { withFileTypes: true }).catch((error: unknown) => {
    if (object(error) && error.code === "ENOENT") return [];
    throw error;
  });
  for (const entry of entries.sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0)) {
    const artifactPath = join(generated, entry.name);
    const canonical = entry.name === canonicalName;
    if (!entry.isFile()) {
      if (canonical) refusal(artifactPath, "expected a regular Script Artifact file");
      continue;
    }
    if (!entry.name.endsWith(".json")) continue;
    const contents = await readFile(artifactPath, "utf8");
    let artifact: unknown;
    try {
      artifact = JSON.parse(contents);
    } catch {
      if (canonical) refusal(artifactPath, "invalid JSON; restore the recorded script identities before building");
      continue;
    }
    if (!canonical && (!object(artifact) || artifact.kind !== "script")) continue;
    if (!object(artifact) || artifact.kind !== "script" || artifact.version !== 1
      || artifact.package !== packageName || !Array.isArray(artifact.scripts)) {
      refusal(artifactPath, "cannot recover script identities from this Script Artifact");
    }
    for (const row of artifact.scripts) {
      if (!object(row) || typeof row.name !== "string" || !row.name.startsWith(`${packageName}.`)) {
        refusal(artifactPath, "a script name does not belong to this Package");
      }
      recordId(artifactPath, ledger, row.name.slice(packageName.length + 1), row.script_id);
    }
  }

  const scriptsDir = join(packageDir, "scripts");
  const sources = await readdir(scriptsDir, { withFileTypes: true }).catch((error: unknown) => {
    if (object(error) && error.code === "ENOENT") return [];
    throw error;
  });
  for (const entry of sources.sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0)) {
    if (!entry.isFile() || (!entry.name.endsWith(".ts") && !entry.name.endsWith(".lua"))) continue;
    const sourcePath = join(scriptsDir, entry.name);
    const id = readDirectives(sourcePath, await readFile(sourcePath, "utf8")).get("id");
    if (id !== undefined) recordId(sourcePath, ledger, entry.name.slice(0, entry.name.lastIndexOf(".")), Number(id));
  }
  return ledger;
}

/** New IDs avoid every installed Package's saved identities, artifacts and legacy headers. */
export async function scriptIds(packageDir: string, packageName: string): Promise<ScriptIds> {
  const ledger = await readPackageIds(packageDir, packageName);
  const packagesDir = dirname(packageDir);
  const entries = await readdir(packagesDir, { withFileTypes: true });
  for (const entry of entries.sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0)) {
    if (entry.name === packageName || entry.name.startsWith(".")) continue;
    const siblingDir = join(packagesDir, entry.name);
    // Collection checks install Package directories as symlinks. Disabled Packages live elsewhere.
    const directory = entry.isDirectory() || (entry.isSymbolicLink() && await stat(siblingDir).then(
      (info) => info.isDirectory(),
      (error: unknown) => {
        if (object(error) && error.code === "ENOENT") return false;
        throw error;
      },
    ));
    if (!directory) continue;
    const sibling = await readPackageIds(siblingDir, entry.name);
    for (const [stem, id] of Object.entries(sibling.ids)) {
      reserveId(siblingDir, ledger, { package: sibling.package, stem }, id);
    }
  }
  return ledger;
}

function reserveId(path: string, ledger: ScriptIds, owner: ScriptOwner, id: number): void {
  const reserved = ledger.reservations.get(id);
  if (reserved && (reserved.package !== owner.package || reserved.stem !== owner.stem)) {
    refusal(path, `${owner.package}.${owner.stem} collides with ${reserved.package}.${reserved.stem} on recorded script ID ${id}; restore the published identities before building`);
  }
  ledger.reservations.set(id, owner);
}

function recordId(path: string, ledger: ScriptIds, stem: string, id: unknown): number {
  if (!/^[a-z0-9_.-]+$/.test(stem) || typeof id !== "number" || !Number.isInteger(id) || id < SCRIPT_ID_FLOOR || id > SCRIPT_ID_CEIL) {
    refusal(path, `invalid identity for ${stem}: expected a script ID in ${SCRIPT_ID_FLOOR}..=${SCRIPT_ID_CEIL}`);
  }
  const recorded = ledger.ids[stem];
  if (recorded !== undefined && recorded !== id) {
    refusal(path, `${stem} already has script ID ${recorded}; refusing to replace it with ${id}`);
  }
  reserveId(path, ledger, { package: ledger.package, stem }, id);
  ledger.ids[stem] = id;
  return id;
}

export function allocateScriptId(path: string, ledger: ScriptIds, stem: string, legacyId?: number): number {
  const recorded = ledger.ids[stem];
  if (legacyId !== undefined || recorded !== undefined) {
    return recordId(path, ledger, stem, legacyId ?? recorded);
  }
  const digest = createHash("sha256").update(ledger.package).update("\0").update(stem).digest();
  const first = SCRIPT_ID_FLOOR + digest.readUInt32BE(0) % (SCRIPT_ID_CEIL - SCRIPT_ID_FLOOR + 1);
  let id = first;
  do {
    if (!ledger.reservations.has(id)) return recordId(path, ledger, stem, id);
    id = id === SCRIPT_ID_CEIL ? SCRIPT_ID_FLOOR : id + 1;
  } while (id !== first);
  return refusal(path, `the Package Script Range ${SCRIPT_ID_FLOOR}..=${SCRIPT_ID_CEIL} has no unused script ID`);
}

export function renderScriptIds(ledger: ScriptIds): string {
  return `${JSON.stringify({ version: ledger.version, package: ledger.package, ids: Object.fromEntries(Object.entries(ledger.ids).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) }, null, 2)}\n`;
}
