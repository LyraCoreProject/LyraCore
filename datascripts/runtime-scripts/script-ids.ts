import { createHash } from "node:crypto";
import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";

export const SCRIPT_IDS_FILE = "script-ids.json";
const FLOOR = 100_000;
const CEILING = 999_999;

interface ScriptIds {
  version: 1;
  package: string;
  ids: Record<string, number>;
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

/** Recorded IDs outlive source removal. Prior Script Artifacts supply migration identities. */
export async function scriptIds(packageDir: string, packageName: string): Promise<ScriptIds> {
  const path = join(packageDir, SCRIPT_IDS_FILE);
  const saved = await readJson(path);
  const ledger: ScriptIds = { version: 1, package: packageName, ids: Object.create(null) };
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
  return ledger;
}

function recordId(path: string, ledger: ScriptIds, stem: string, id: unknown): number {
  if (!/^[a-z0-9_.-]+$/.test(stem) || typeof id !== "number" || !Number.isInteger(id) || id < FLOOR || id > CEILING) {
    refusal(path, `invalid identity for ${stem}: expected a script ID in ${FLOOR}..=${CEILING}`);
  }
  const recorded = ledger.ids[stem];
  if (recorded !== undefined && recorded !== id) {
    refusal(path, `${stem} already has script ID ${recorded}; refusing to replace it with ${id}`);
  }
  for (const [other, reserved] of Object.entries(ledger.ids)) {
    if (other !== stem && reserved === id) {
      refusal(path, `${stem} collides with ${other} on recorded script ID ${id}; restore the published identities before building`);
    }
  }
  ledger.ids[stem] = id;
  return id;
}

export function allocateScriptId(path: string, ledger: ScriptIds, stem: string, legacyId?: number): number {
  const recorded = ledger.ids[stem];
  if (legacyId !== undefined || recorded !== undefined) {
    return recordId(path, ledger, stem, legacyId ?? recorded);
  }
  const digest = createHash("sha256").update(ledger.package).update("\0").update(stem).digest();
  const first = FLOOR + digest.readUInt32BE(0) % (CEILING - FLOOR + 1);
  const reserved = new Set(Object.values(ledger.ids));
  let id = first;
  do {
    if (!reserved.has(id)) return recordId(path, ledger, stem, id);
    id = id === CEILING ? FLOOR : id + 1;
  } while (id !== first);
  return refusal(path, `the Package Script Range ${FLOOR}..=${CEILING} has no unused script ID`);
}

export function renderScriptIds(ledger: ScriptIds): string {
  return `${JSON.stringify({ ...ledger, ids: Object.fromEntries(Object.entries(ledger.ids).sort(([a], [b]) => a < b ? -1 : a > b ? 1 : 0)) }, null, 2)}\n`;
}
