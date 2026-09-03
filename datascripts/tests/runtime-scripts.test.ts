// The Runtime Script build, end to end: sources under a Package's `scripts/`, one Script Artifact
// out, and the same bytes every time.
//
// The committed `runtime-scripts/fuel-workload.lua` is where this suite meets the Module:
// `module/src/runtime_script.rs` runs those exact bytes on the Runtime Script Host and sizes the
// Fuel Budget against them. So the first test recompiles the workload and refuses a stale copy —
// otherwise the Module would be measuring Lua the toolchain no longer emits.

import { expect, test } from "bun:test";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

import { buildPackageScripts } from "../runtime-scripts/build-scripts.ts";

const TOOLCHAIN = join(import.meta.dir, "..", "runtime-scripts");
const PACKAGE = "example.scripts";
const EVENT = "on_login";

interface Built {
  readonly artifact: string;
  readonly scripts: Array<{ script_id: number; name: string; event: string; source: string }>;
}

/// Build a scratch Package holding `files`, and return its artifact.
///
/// A scratch root rather than `packages/`: the artifact is what is under test, and a build that
/// wrote into the checkout would make one test's output another test's input.
async function build(
  files: Record<string, string>,
  events?: string[],
  packageName = PACKAGE,
): Promise<Built> {
  const root = mkdtempSync(join(tmpdir(), "lyracore-runtime-scripts-"));
  const previousRoot = process.env.LYRACORE_PACKAGES_ROOT;
  const previousEvents = process.env.LYRACORE_HOOK_EVENTS;
  try {
    const scripts = join(root, packageName, "scripts");
    mkdirSync(scripts, { recursive: true });
    for (const [name, source] of Object.entries(files)) {
      const path = join(scripts, name);
      mkdirSync(dirname(path), { recursive: true });
      writeFileSync(path, source);
    }
    process.env.LYRACORE_PACKAGES_ROOT = root;
    if (events === undefined) delete process.env.LYRACORE_HOOK_EVENTS;
    else process.env.LYRACORE_HOOK_EVENTS = events.join("\n");

    const artifact = readFileSync(await buildPackageScripts(packageName), "utf8");
    return { artifact, scripts: JSON.parse(artifact).scripts };
  } finally {
    if (previousRoot === undefined) delete process.env.LYRACORE_PACKAGES_ROOT;
    else process.env.LYRACORE_PACKAGES_ROOT = previousRoot;
    if (previousEvents === undefined) delete process.env.LYRACORE_HOOK_EVENTS;
    else process.env.LYRACORE_HOOK_EVENTS = previousEvents;
    rmSync(root, { recursive: true, force: true });
  }
}

/// The two required directives, for the tests that are about something else.
function directives(event = EVENT, id = 100_200): string {
  return `// @event ${event}\n// @id ${id}\n`;
}

// ---- the workload the Module's Fuel Budget is sized against ----

test("the committed fuel workload Lua is what the pinned toolchain emits today", async () => {
  const source = readFileSync(join(TOOLCHAIN, "fuel-workload.ts"), "utf8");
  const committed = readFileSync(join(TOOLCHAIN, "fuel-workload.lua"), "utf8");

  const built = await build({ "fuel-workload.ts": source });

  expect(built.scripts[0]?.source).toBe(committed);
});

test("every checked-in Package Runtime Script passes through the pinned compiler", async () => {
  const packagesRoot = join(import.meta.dir, "..", "..", "packages");
  const packageScripts = readdirSync(packagesRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((entry) => {
      const scriptsDir = join(packagesRoot, entry.name, "scripts");
      let sources;
      try {
        sources = readdirSync(scriptsDir, { withFileTypes: true });
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
        throw error;
      }
      const files = Object.fromEntries(
        sources
          .filter(
            (source) =>
              source.isFile() && (source.name.endsWith(".ts") || source.name.endsWith(".lua")),
          )
          .sort((a, b) => a.name.localeCompare(b.name))
          .map((source) => [source.name, readFileSync(join(scriptsDir, source.name), "utf8")]),
      );
      return Object.keys(files).length === 0 ? [] : [{ package: entry.name, files }];
    });

  expect(packageScripts.length).toBeGreaterThan(0);
  for (const checkedIn of packageScripts) {
    await build(checkedIn.files, undefined, checkedIn.package);
  }
});

// ---- determinism ----

test("two builds of one source tree write the same bytes", async () => {
  const files = { "greet.ts": `${directives()}function script(): number { return 7; }\n` };

  expect((await build(files)).artifact).toBe((await build(files)).artifact);
});

test("the source digest covers only immediate regular TypeScript and Lua files", async () => {
  const built = await build({
    "alpha.ts": `${directives(EVENT, 100_201)}function script(): void {}\n`,
    "zeta.lua": "-- @event on_login\n-- @id 100202\nreturn 2\n",
    "README.md": "ignored\n",
    "nested/hidden.ts": `${directives(EVENT, 100_203)}function script(): void {}\n`,
  });

  expect(JSON.parse(built.artifact).source_hash).toBe(
    "8395ead00aad341a7daa23658447385da94dabf6932b406cbfbdb5e2fd664002",
  );
  expect(built.scripts.map((script) => script.name)).toEqual([
    `${PACKAGE}.alpha`,
    `${PACKAGE}.zeta`,
  ]);
});

test("the scripts are ordered by identifier, whatever order the files sort in", async () => {
  const built = await build({
    "zeta.ts": `${directives(EVENT, 100_300)}function script(): void {}\n`,
    "alpha.ts": `${directives(EVENT, 100_900)}function script(): void {}\n`,
  });

  expect(built.scripts.map((s) => s.script_id)).toEqual([100_300, 100_900]);
  expect(built.scripts.map((s) => s.name)).toEqual([`${PACKAGE}.zeta`, `${PACKAGE}.alpha`]);
});

// ---- the piccolo call shape ----

test("a table constructor passed to a call is hoisted through the guard", async () => {
  // piccolo 0.3.3 leaks a trailing constructor's element count as an extra argument. The Module
  // pins both that fault and this fix; here we prove the emitter applies it.
  const built = await build({
    "guard.ts":
      `${directives()}function take(o: { a: number }): number { return o.a; }\n` +
      "function script(): number { return take({ a: 1 }); }\n",
  });

  const lua = built.scripts[0]?.source ?? "";
  expect(lua).toContain("local function ____tbl(t)");
  expect(lua).toMatch(/____tbl\(\{/);
});

test("every emitted file ends by calling its entry point, so the chunk returns the Script Answer", async () => {
  const built = await build({
    "answer.ts": `${directives()}function script(): number { return 3; }\n`,
  });

  expect(built.scripts[0]?.source.trimEnd().endsWith("return script()")).toBe(true);
});

// ---- hand-written Lua ----

test("a hand-written .lua script ships unchanged", async () => {
  const lua = "-- @event on_login\n-- @id 100400\ngrant_xp(event.actor, 5)\n";

  const built = await build({ "bonus.lua": lua });

  expect(built.scripts[0]?.source).toBe(lua);
  expect(built.scripts[0]?.name).toBe(`${PACKAGE}.bonus`);
});

// ---- refusals, each naming the file ----

test("a script declaring no entry point is refused", async () => {
  expect(build({ "bare.ts": `${directives()}const x = 1;\n` })).rejects.toThrow(
    /typescript-to-lua refused/,
  );
});

test("the entry point takes no parameters and returns only a number or nothing", async () => {
  expect(
    build({
      "parameter.ts":
        `${directives()}function script(required: number): number { return required; }\n`,
    }),
  ).rejects.toThrow(/typescript-to-lua refused/);
  expect(
    build({
      "string.ts": `${directives()}function script(): string { return "wrong"; }\n`,
    }),
  ).rejects.toThrow(/typescript-to-lua refused/);
  expect(
    build({
      "optional.ts": `${directives()}function script(value?: number): void {}\n`,
    }),
  ).rejects.toThrow(/typescript-to-lua refused/);
  expect(
    build({
      "rest.ts": `${directives()}function script(...values: number[]): void {}\n`,
    }),
  ).rejects.toThrow(/typescript-to-lua refused/);
  expect(
    build({
      "overload.ts":
        `${directives()}function script(): number;\n` +
        `function script(value?: number): number | string { return value ?? "wrong"; }\n`,
    }),
  ).rejects.toThrow(/typescript-to-lua refused/);
});

test("a missing @event or @id is refused by name", async () => {
  expect(
    build({ "no-id.ts": "// @event on_login\nfunction script(): void {}\n" }),
  ).rejects.toThrow(/`@id`/);
  expect(build({ "no-event.ts": "// @id 100200\nfunction script(): void {}\n" })).rejects.toThrow(
    /`@event`/,
  );
});

test("an identifier outside the Package script band is refused", async () => {
  expect(
    build({ "low.ts": `${directives(EVENT, 42)}function script(): void {}\n` }),
  ).rejects.toThrow(/100000\.\.=999999/);
});

test("two scripts claiming one identifier are refused", async () => {
  expect(
    build({
      "a.ts": `${directives()}function script(): void {}\n`,
      "b.ts": `${directives()}function script(): void {}\n`,
    }),
  ).rejects.toThrow(/collides/);
});

test("an unknown event is refused and a Package Event of the own Package is not", async () => {
  const catalogue = ["on_login", "on_death"];

  expect(
    build({ "sneeze.ts": `${directives("on_sneeze")}function script(): void {}\n` }, catalogue),
  ).rejects.toThrow(/on_sneeze/);

  const own = await build(
    { "own.ts": `${directives(`${PACKAGE}.ember_spent`)}function script(): void {}\n` },
    catalogue,
  );
  expect(own.scripts[0]?.event).toBe(`${PACKAGE}.ember_spent`);
});

test("a Package folder with no script sources is refused rather than writing an empty artifact", async () => {
  expect(build({})).rejects.toThrow(/no `\.ts` or `\.lua`/);
});

test("a failed artifact write leaves the prior artifact unchanged", async () => {
  const root = mkdtempSync(join(tmpdir(), "lyracore-runtime-scripts-write-"));
  const previousRoot = process.env.LYRACORE_PACKAGES_ROOT;
  const packageDir = join(root, PACKAGE);
  const scripts = join(packageDir, "scripts");
  const generated = join(packageDir, "data", ".generated");
  const artifact = join(generated, `${PACKAGE}.script.json`);
  try {
    mkdirSync(scripts, { recursive: true });
    mkdirSync(generated, { recursive: true });
    writeFileSync(
      join(scripts, "greet.lua"),
      "-- @event on_login\n-- @id 100200\nreturn 1\n",
    );
    writeFileSync(artifact, "prior artifact\n");
    chmodSync(generated, 0o555);
    process.env.LYRACORE_PACKAGES_ROOT = root;

    await expect(buildPackageScripts(PACKAGE)).rejects.toThrow();

    chmodSync(generated, 0o755);
    expect(readFileSync(artifact, "utf8")).toBe("prior artifact\n");
    expect(readdirSync(generated)).toEqual([`${PACKAGE}.script.json`]);
  } finally {
    chmodSync(generated, 0o755);
    if (previousRoot === undefined) delete process.env.LYRACORE_PACKAGES_ROOT;
    else process.env.LYRACORE_PACKAGES_ROOT = previousRoot;
    rmSync(root, { recursive: true, force: true });
  }
});

test("a failed atomic rename removes its sibling temporary directory", async () => {
  const root = mkdtempSync(join(tmpdir(), "lyracore-runtime-scripts-rename-"));
  const previousRoot = process.env.LYRACORE_PACKAGES_ROOT;
  const packageDir = join(root, PACKAGE);
  const scripts = join(packageDir, "scripts");
  const generated = join(packageDir, "data", ".generated");
  const artifact = join(generated, `${PACKAGE}.script.json`);
  try {
    mkdirSync(scripts, { recursive: true });
    mkdirSync(artifact, { recursive: true });
    writeFileSync(
      join(scripts, "greet.lua"),
      "-- @event on_login\n-- @id 100200\nreturn 1\n",
    );
    writeFileSync(join(artifact, "prior"), "unchanged\n");
    process.env.LYRACORE_PACKAGES_ROOT = root;

    await expect(buildPackageScripts(PACKAGE)).rejects.toThrow();

    expect(readFileSync(join(artifact, "prior"), "utf8")).toBe("unchanged\n");
    expect(readdirSync(generated)).toEqual([`${PACKAGE}.script.json`]);
  } finally {
    if (previousRoot === undefined) delete process.env.LYRACORE_PACKAGES_ROOT;
    else process.env.LYRACORE_PACKAGES_ROOT = previousRoot;
    rmSync(root, { recursive: true, force: true });
  }
});
