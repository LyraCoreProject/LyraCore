// Compile one Package's Runtime Scripts into its Script Artifact.
//
//   bun run datascripts/runtime-scripts/build-scripts.ts <package>
//
// `lyracore packages build` runs this once per enabled Package that carries a `scripts/` folder,
// the same way it runs a Datascript: one Bun subprocess, one artifact written, non-zero on any
// refusal and nothing written. Run it by hand the same way.
//
// # What a Package ships
//
// `packages/<package>/scripts/` holds the sources. A `.ts` file is compiled by the pinned
// `typescript-to-lua` under `tsconfig.json` next to this file; a `.lua` file is shipped unchanged,
// for the author who would rather write Lua. Both carry the same directive header:
//
//     // @event on_login          -- @event on_login
//     // @id 100200               -- @id 100201
//     // @priority 10             -- @priority 10
//     // @enabled false           -- @enabled false
//
// `@event` and `@id` are required; `@priority` defaults to 0 and `@enabled` to true. The directives
// stop at the first line that is neither blank nor a comment, so ordinary comments below them are
// just comments.
//
// # Why the identifier is written down
//
// A `script_id` is DURABLE: it is what `game_script` keys on and what a Package's own rows will
// point at. Deriving it from a sorted file index would renumber every later script the moment an
// author adds one alphabetically earlier, and two Packages would both start at the bottom of the
// band. So the author writes it, once, and a collision is a refusal that names both Packages
// instead of a silent renumber. The band is 100000..=999999.
//
// The script NAME is not written down: it is `<package>.<file stem>`, which is the one part of the
// identity that has an obviously right answer.
//
// # What checks what
//
// This builder refuses what it can name a FILE and a directive for. `lyracore-delta-check` is the
// authority on the artifact itself and runs afterwards over every Package at once, because a
// collision between Packages cannot be seen from inside one. The event catalogue arrives through
// `LYRACORE_HOOK_EVENTS` (the CLI fills it from `lyracore-delta-check --print-events`); without it
// this builder does not check event names and says so, and the validator still refuses an unknown
// one.

import { createHash } from "node:crypto";
import { existsSync, mkdtempSync, readdirSync, rmSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

/// Where a Package keeps its Runtime Script sources, relative to the Package folder.
const SCRIPTS_DIR = "scripts";

/// Where a Package's generated artifacts live, relative to the Package folder.
const GENERATED_DIR = "data/.generated";

/// The Package script band. Mirrors `is_package_script_id` in `lyracore-package-delta`.
const SCRIPT_ID_FLOOR = 100_000;
const SCRIPT_ID_CEIL = 999_999;

/// One script, as its directives and its Lua describe it.
interface RuntimeScript {
  readonly script_id: number;
  readonly name: string;
  readonly event: string;
  readonly priority: number;
  readonly enabled: boolean;
  readonly source: string;
}

/// A refusal an author can act on: it always names the file.
class ScriptRefusal extends Error {}

function refuse(file: string, what: string): never {
  throw new ScriptRefusal(`${file}: ${what}`);
}

// ---- directives ----

/// The `@key value` lines at the top of a script file, before the first line that is neither blank
/// nor a comment. Both comment markers are accepted so one reader serves `.ts` and `.lua`.
function readDirectives(file: string, source: string): Map<string, string> {
  const directives = new Map<string, string>();
  for (const line of source.split("\n")) {
    const text = line.trim();
    if (text.length === 0) continue;
    const comment = text.startsWith("//") ? text.slice(2) : text.startsWith("--") ? text.slice(2) : undefined;
    if (comment === undefined) break;
    const match = /^\s*@([a-z]+)\s+(\S+)\s*$/.exec(comment);
    if (!match) continue;
    const [, key, value] = match as unknown as [string, string, string];
    if (directives.has(key)) refuse(file, `\`@${key}\` is declared twice`);
    directives.set(key, value);
  }
  return directives;
}

function requiredDirective(file: string, directives: Map<string, string>, key: string): string {
  const value = directives.get(key);
  if (value === undefined) {
    refuse(file, `no \`@${key}\` directive. Every Runtime Script declares \`@event\` and \`@id\`.`);
  }
  return value;
}

function scriptId(file: string, directives: Map<string, string>): number {
  const raw = requiredDirective(file, directives, "id");
  const id = Number(raw);
  if (!Number.isInteger(id) || id < SCRIPT_ID_FLOOR || id > SCRIPT_ID_CEIL) {
    refuse(file, `\`@id ${raw}\` is not a whole number in ${SCRIPT_ID_FLOOR}..=${SCRIPT_ID_CEIL}, the Package script band`);
  }
  return id;
}

function priority(file: string, directives: Map<string, string>): number {
  const raw = directives.get("priority");
  if (raw === undefined) return 0;
  const value = Number(raw);
  if (!Number.isInteger(value)) refuse(file, `\`@priority ${raw}\` is not a whole number`);
  return value;
}

function enabled(file: string, directives: Map<string, string>): boolean {
  const raw = directives.get("enabled");
  if (raw === undefined) return true;
  if (raw === "true") return true;
  if (raw === "false") return false;
  return refuse(file, `\`@enabled ${raw}\` is neither \`true\` nor \`false\``);
}

/// The catalogue the Module dispatches, or `undefined` when the caller did not supply one.
function hookEvents(): Set<string> | undefined {
  const raw = process.env.LYRACORE_HOOK_EVENTS;
  if (raw === undefined) return undefined;
  return new Set(raw.split("\n").map((name) => name.trim()).filter((name) => name.length > 0));
}

/// A core event from the catalogue, or a Package Event this Package fires itself.
function checkEvent(file: string, event: string, packageName: string, catalogue: Set<string> | undefined): void {
  if (catalogue === undefined || catalogue.has(event)) return;
  const own = event.startsWith(`${packageName}.`) && /^[a-z][a-z0-9_]*$/.test(event.slice(packageName.length + 1));
  if (own) return;
  refuse(
    file,
    `\`@event ${event}\` is neither an event this build fires nor a Package Event of \`${packageName}\`. ` +
      `The catalogue is: ${[...catalogue].join(", ")}. A Package Event is spelled \`${packageName}.<name>\`.`,
  );
}

// ---- compilation ----

/// The directory this file sits in: the pinned Runtime Script toolchain.
function toolchainDir(): string {
  return dirname(new URL(import.meta.url).pathname);
}

function repoRoot(): string {
  return resolve(toolchainDir(), "..", "..");
}

function packagesRoot(): string {
  return process.env.LYRACORE_PACKAGES_ROOT ?? join(repoRoot(), "packages");
}

/// Compile ONE `.ts` under `scriptsDir` into `outDir`, with the checked-in settings.
///
/// One file per invocation, not one program per Package: every script declares `function script()`
/// at top level, and a shared program would make the second one a duplicate declaration. Separate
/// programs also match the Host, which runs each script as its own chunk with its own globals.
///
/// The per-run config is the checked-in `tsconfig.json` with three members added and the plugin path
/// made absolute. It is NOT an `extends` of it: `typescript-to-lua` resolves a plugin path against
/// the config it was invoked with, so a relative one in a base config would resolve from the wrong
/// directory. Materialising the whole config keeps that resolution the toolchain's own business.
async function compile(scriptsDir: string, source: string, outDir: string): Promise<void> {
  const toolchain = toolchainDir();
  const config = JSON.parse(await readFile(join(toolchain, "tsconfig.json"), "utf8")) as {
    compilerOptions: Record<string, unknown>;
    tstl: { luaPlugins: Array<{ name: string }> };
    files?: string[];
  };
  config.compilerOptions.rootDir = scriptsDir;
  config.compilerOptions.outDir = outDir;
  config.files = [join(toolchain, "runtime-script.d.ts"), source];
  config.tstl.luaPlugins = config.tstl.luaPlugins.map((plugin) => ({
    ...plugin,
    name: resolve(toolchain, plugin.name),
  }));

  const configPath = join(outDir, "tsconfig.json");
  await writeFile(configPath, JSON.stringify(config, null, 2));

  const tstl = join(toolchain, "node_modules", "typescript-to-lua", "dist", "tstl.js");
  const result = Bun.spawnSync({
    cmd: ["bun", tstl, "-p", configPath],
    stdout: "inherit",
    stderr: "inherit",
    cwd: repoRoot(),
  });
  if (result.exitCode !== 0) {
    throw new ScriptRefusal(
      `typescript-to-lua refused ${source}. Its own diagnostics are above.`,
    );
  }
}

// ---- the artifact ----

/// The digest of the sources this artifact was generated from: every file under `scripts/`, by
/// sorted relative name, length-prefixed so no two trees can collide.
///
/// The toolchain is deliberately NOT in it. `source_hash` answers "which revision of this Package's
/// sources is on the Shard"; whether the compiler moved is the Build Identity sidecar's question.
async function sourceHash(scriptsDir: string, files: string[]): Promise<string> {
  const digest = createHash("sha256");
  for (const file of files) {
    const bytes = await readFile(join(scriptsDir, file));
    digest.update(file);
    digest.update("\0");
    digest.update(String(bytes.length));
    digest.update("\0");
    digest.update(bytes);
  }
  return digest.digest("hex");
}

/// The artifact, as `crates/lyracore-package-delta/src/script.rs` parses it.
///
/// Indented rather than canonical: the Official Package Collection COMMITS Script Artifacts, and a
/// reviewer reads this file. The Module canonicalises it again before hashing, so how it is spelled
/// changes nothing it means. Member order and script order are still fixed, so two runs over one
/// source tree write the same bytes.
function renderArtifact(packageName: string, hash: string, scripts: RuntimeScript[]): string {
  const ordered = [...scripts].sort((a, b) => a.script_id - b.script_id);
  return `${JSON.stringify(
    {
      kind: "script",
      version: 1,
      package: packageName,
      source_hash: hash,
      scripts: ordered.map((script) => ({
        script_id: script.script_id,
        name: script.name,
        event: script.event,
        priority: script.priority,
        enabled: script.enabled,
        source: script.source,
      })),
    },
    null,
    2,
  )}\n`;
}

/// Every script source a Package ships, in file-name order.
function scriptSources(scriptsDir: string): string[] {
  const listing = existsSync(scriptsDir) ? readdirSync(scriptsDir) : [];
  return listing.filter((name) => name.endsWith(".ts") || name.endsWith(".lua")).sort();
}

/// Build one Package's Script Artifact and write it. Returns the path written.
export async function buildPackageScripts(packageName: string): Promise<string> {
  const packageDir = join(packagesRoot(), packageName);
  const scriptsDir = join(packageDir, SCRIPTS_DIR);
  const files = scriptSources(scriptsDir);
  if (files.length === 0) {
    throw new ScriptRefusal(`${scriptsDir}: holds no \`.ts\` or \`.lua\` Runtime Script`);
  }

  const catalogue = hookEvents();
  if (catalogue === undefined) {
    console.error(
      "LYRACORE_HOOK_EVENTS is unset, so event names are not checked here. " +
        "`lyracore-delta-check` still refuses an unknown one.",
    );
  }

  // Directives first, for every file, so a bad `@id` or a collision is a refusal before the first
  // compile rather than after the last one.
  const declared: Array<Omit<RuntimeScript, "source"> & { file: string }> = [];
  for (const file of files) {
    const path = join(scriptsDir, file);
    const directives = readDirectives(path, await readFile(path, "utf8"));
    const event = requiredDirective(path, directives, "event");
    checkEvent(path, event, packageName, catalogue);
    declared.push({
      file,
      script_id: scriptId(path, directives),
      name: `${packageName}.${file.slice(0, file.lastIndexOf("."))}`,
      event,
      priority: priority(path, directives),
      enabled: enabled(path, directives),
    });
  }
  for (const [index, script] of declared.entries()) {
    const clash = declared.findIndex(
      (other, at) => at < index && (other.script_id === script.script_id || other.name === script.name),
    );
    if (clash !== -1) {
      refuse(join(scriptsDir, script.file), `collides with ${declared[clash]?.file} on \`@id\` or name`);
    }
  }

  const outDir = mkdtempSync(join(tmpdir(), "lyracore-scripts-"));
  try {
    const scripts: RuntimeScript[] = [];
    for (const { file, ...script } of declared) {
      const path = join(scriptsDir, file);
      const stem = file.slice(0, file.lastIndexOf("."));
      let source: string;
      if (file.endsWith(".lua")) {
        source = await readFile(path, "utf8");
      } else {
        await compile(scriptsDir, path, outDir);
        source = await readFile(join(outDir, `${stem}.lua`), "utf8").catch(() => {
          refuse(path, "typescript-to-lua emitted no Lua for it");
        });
      }
      if (source.trim().length === 0) refuse(path, "compiles to nothing; a Runtime Script needs Lua in it");
      scripts.push({ ...script, source });
    }

    const artifact = renderArtifact(packageName, await sourceHash(scriptsDir, files), scripts);
    const path = join(packageDir, GENERATED_DIR, `${packageName}.script.json`);
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, artifact);
    return path;
  } finally {
    rmSync(outDir, { recursive: true, force: true });
  }
}

if (import.meta.main) {
  const packageName = process.argv[2];
  if (!packageName) {
    console.error("usage: bun run datascripts/runtime-scripts/build-scripts.ts <package>");
    process.exit(1);
  }
  try {
    console.log(await buildPackageScripts(packageName));
  } catch (cause) {
    console.error(cause instanceof Error ? cause.message : String(cause));
    process.exit(1);
  }
}
