import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";
import { buildPackageScripts } from "../runtime-scripts/build-scripts.ts";

const PACKAGE = "example.scripts";
const WELCOME = `function welcome(event: PlayerLoginEvent): void {
  send_chat(event.player, "Welcome!");
}
events.player.onLogin(welcome);
`;

interface Artifact {
  source_hash: string;
  scripts: Array<{ script_id: number; name: string; event: string; source: string; priority: number; enabled: boolean }>;
}

async function scratch(run: (dir: string, build: () => Promise<Artifact>) => Promise<void>): Promise<void> {
  const root = mkdtempSync(join(tmpdir(), "typed-scripts-"));
  const before = process.env.LYRACORE_PACKAGES_ROOT;
  const dir = join(root, PACKAGE);
  mkdirSync(join(dir, "scripts"), { recursive: true });
  process.env.LYRACORE_PACKAGES_ROOT = root;
  try {
    await run(dir, async () => JSON.parse(readFileSync(await buildPackageScripts(PACKAGE), "utf8")));
  } finally {
    if (before === undefined) delete process.env.LYRACORE_PACKAGES_ROOT;
    else process.env.LYRACORE_PACKAGES_ROOT = before;
    rmSync(root, { recursive: true, force: true });
  }
}

test("named TS and Lua handlers bind the same event without directives", async () => {
  await scratch(async (dir, build) => {
    writeFileSync(join(dir, "scripts/welcome.ts"), WELCOME);
    writeFileSync(join(dir, "scripts/ding.lua"), `local function ding(event)
  send_chat(event.player, "Ding " .. event.newLevel)
end
events.player.onLevelUp(ding, {priority = -3, enabled = false})
`);
    const first = await build();
    const welcome = first.scripts.find((script) => script.event === "on_login")!;
    const ding = first.scripts.find((script) => script.event === "on_levelup")!;
    expect(welcome.source).toContain("welcome(event)");
    expect(welcome.source).not.toContain("return script()");
    expect(ding.priority).toBe(-3);
    expect(ding.enabled).toBe(false);
    expect(await build()).toEqual(first);
    const ids = JSON.parse(readFileSync(join(dir, "script-ids.json"), "utf8"));
    expect(ids.package).toBe(PACKAGE);
    expect(ids.ids.welcome).toBe(welcome.script_id);
    expect(ids.ids.ding).toBe(ding.script_id);
  });
});

test("a handler's type must match its event and its available payload", async () => {
  await scratch(async (dir, build) => {
    const path = join(dir, "scripts/welcome.ts");
    writeFileSync(path, WELCOME.replace("PlayerLoginEvent", "PlayerLevelUpEvent"));
    await expect(build()).rejects.toThrow(/typescript-to-lua refused/);
    writeFileSync(path, WELCOME.replace('"Welcome!"', "event.newLevel"));
    await expect(build()).rejects.toThrow(/typescript-to-lua refused/);
    writeFileSync(path, WELCOME.replace(": void", ": string").replace('send_chat(event.player, "Welcome!");', 'return "wrong";'));
    await expect(build()).rejects.toThrow(/typescript-to-lua refused/);
  });
});

test("Package Events bind a local name and preserve numeric Script Answers", async () => {
  await scratch(async (dir, build) => {
    writeFileSync(join(dir, "scripts/answer.ts"), `function answer(event: PackageEvent): number { return 0; }
events.package.on("greeting", answer, {priority: 2});`);
    const result = await build();
    expect(result.scripts[0]!.event).toBe(`${PACKAGE}.greeting`);
    expect(result.scripts[0]!.priority).toBe(2);
    expect(result.scripts[0]!.source).toContain("return 0");
    expect(result.scripts[0]!.source.trimEnd()).toMatch(/return ____lyracore_handler\(event\)$/);
  });
});

test("Lua comments and multiline strings do not create Event Bindings", async () => {
  await scratch(async (dir, build) => {
    writeFileSync(join(dir, "scripts/welcome.lua"), `-- events.player.onLevelUp(wrong)
local message = [=[events.player.onLogin(fake)]=]
local function welcome(event) send_chat(event.player, message) end
events.player.onLogin(welcome)
`);
    expect((await build()).scripts[0]!.event).toBe("on_login");
  });
});

test("Lua handlers preserve UTF-8 messages and ordinary events fields", async () => {
  await scratch(async (dir, build) => {
    const source = `local messages = { events = "你好，世界 🌍" }
local function welcome(event) send_chat(event.player, messages.events) end
events.player.onLogin(welcome)
`;
    writeFileSync(join(dir, "scripts/welcome.lua"), source);
    expect((await build()).scripts[0]!.source).toContain(source);
  });
});

test("typed TS handlers can use fields named events", async () => {
  await scratch(async (dir, build) => {
    writeFileSync(join(dir, "scripts/welcome.ts"), `function welcome(event: PlayerLoginEvent): void {
  const counts: { events: number } = { events: 1 };
  const { events: count } = counts;
  send_chat(event.player, String(counts.events + count));
}
events.player.onLogin(welcome);
`);
    expect((await build()).scripts[0]!.event).toBe("on_login");
  });
});

test("legacy scripts retain local events names and UTF-8 source", async () => {
  await scratch(async (dir, build) => {
    const lua = `-- @event on_login
-- @id 100300
local events = { greet = function() return "你好" end }
events.greet()
return 1
`;
    writeFileSync(join(dir, "scripts/welcome.lua"), lua);
    writeFileSync(join(dir, "scripts/other.ts"), `// @event on_login
// @id 100301
const events = { count: 1 };
function script(): number {
  return events.count;
}
`);
    const scripts = (await build()).scripts;
    expect(scripts.find((script) => script.script_id === 100300)!.source).toBe(lua);
    expect(scripts.find((script) => script.script_id === 100301)!.source).toContain("return script()");
  });
});

test("conditional, duplicate, aliased and unknown bindings are refused", async () => {
  await scratch(async (dir, build) => {
    const path = join(dir, "scripts/welcome.lua");
    const handler = "local function welcome(event) end\n";
    for (const source of [
      handler + "if true then events.player.onLogin(welcome) end",
      handler + "events.player.onLogin(welcome)\nevents.player.onLevelUp(welcome)",
      handler + "local subscribe = events.player.onLogin\nsubscribe(welcome)",
      handler + "events.player.onSneeze(welcome)",
      handler + "events.player.onLogin(welcome, {priority = 2147483648})",
      handler + "events.player.onLogin(welcome, {enabled = 1})",
      handler + "events.player.onLogin(welcome, {priority = 1, priority = 2})",
      handler + "events.player.onLogin(welcome)\nreturn 7",
    ]) {
      writeFileSync(path, source);
      await expect(build()).rejects.toThrow();
    }
  });
});

test("migration adopts existing artifact IDs and refuses a changed durable ID", async () => {
  await scratch(async (dir, build) => {
    const path = join(dir, "scripts/welcome.ts");
    writeFileSync(path, "// @event on_login\n// @id 100300\nfunction script(): void {}\n");
    await build();
    rmSync(join(dir, "script-ids.json"));
    writeFileSync(path, WELCOME);
    expect((await build()).scripts[0]!.script_id).toBe(100300);
    writeFileSync(join(dir, "script-ids.json"), JSON.stringify({ version: 1, package: PACKAGE, ids: { welcome: 100301 } }));
    await expect(build()).rejects.toThrow(/already has script ID/);
  });
});

test("adding and removing sources or renaming a function keeps recorded IDs", async () => {
  await scratch(async (dir, build) => {
    writeFileSync(join(dir, "scripts/welcome.ts"), WELCOME);
    const id = (await build()).scripts[0]!.script_id;
    writeFileSync(join(dir, "scripts/aaa.ts"), WELCOME.replaceAll("welcome", "other"));
    writeFileSync(join(dir, "scripts/welcome.ts"), WELCOME.replaceAll("welcome", "renamed"));
    expect((await build()).scripts.find((script) => script.name.endsWith(".welcome"))!.script_id).toBe(id);
    rmSync(join(dir, "scripts/welcome.ts"));
    await build();
    expect(JSON.parse(readFileSync(join(dir, "script-ids.json"), "utf8")).ids.welcome).toBe(id);
    writeFileSync(join(dir, "scripts/welcome.ts"), WELCOME);
    expect((await build()).scripts.find((script) => script.name.endsWith(".welcome"))!.script_id).toBe(id);
  });
});

test("an identity file copied from another Package is refused", async () => {
  await scratch(async (dir, build) => {
    writeFileSync(join(dir, "scripts/welcome.ts"), WELCOME);
    writeFileSync(join(dir, "script-ids.json"), JSON.stringify({ version: 1, package: "someone.else", ids: { welcome: 100300 } }));
    await expect(build()).rejects.toThrow(/owned by Package/);
  });
});

test("the artifact source hash includes the exact recorded identity bytes", async () => {
  await scratch(async (dir, build) => {
    writeFileSync(join(dir, "scripts/welcome.ts"), WELCOME);
    const artifact = await build();
    const digest = createHash("sha256");
    for (const [name, contents] of [
      ["script-ids.json", readFileSync(join(dir, "script-ids.json"))],
      ["welcome.ts", Buffer.from(WELCOME)],
    ] as const) {
      digest.update(name).update("\0").update(String(contents.length)).update("\0").update(contents);
    }
    expect(artifact.source_hash).toBe(digest.digest("hex"));
  });
});

test("the Runtime Script Host runs the exact Lua this toolchain emits", async () => {
  await scratch(async (dir, build) => {
    const fixtures = join(import.meta.dir, "fixtures/typed");
    for (const file of ["welcome.ts", "ding.lua", "answer.lua"]) {
      writeFileSync(join(dir, "scripts", file), readFileSync(join(fixtures, file)));
    }
    for (const script of (await build()).scripts) {
      const stem = script.name.slice(PACKAGE.length + 1);
      expect(script.source).toBe(readFileSync(join(fixtures, `${stem}.generated.lua`), "utf8"));
    }
  });
});
