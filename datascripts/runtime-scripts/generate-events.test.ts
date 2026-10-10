import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { EVENTS, luaDeclarations, rustEventNames, typeScriptDeclarations } from "./generate-events";

const ROOT = dirname(fileURLToPath(import.meta.url));

test("checked-in editor declarations match the core event catalogue", () => {
  expect(readFileSync(join(ROOT, "runtime-script.d.ts"), "utf8")).toBe(typeScriptDeclarations());
  expect(readFileSync(join(ROOT, "runtime-script.lua"), "utf8")).toBe(luaDeclarations());
  expect(readFileSync(join(ROOT, "event-names.rs"), "utf8")).toBe(rustEventNames());
});

test("level-up payload reads the attained level from the core hook", () => {
  const levelup = EVENTS.find(definition => definition.event === "on_levelup");
  expect(levelup?.fields).toContainEqual({ name: "newLevel", type: "number", source: "payload.new_level" });
});

test("login and level-up declare a required Character handle", () => {
  for (const name of ["on_login", "on_levelup"]) {
    const definition = EVENTS.find(definition => definition.event === name);
    expect(definition?.fields).toContainEqual({ name: "player", type: "PlayerEntity", source: "actor", required: true });
  }
});
