# T2: The Runtime Script authoring project

Parent: issue #320. The tracer. Runs after T0 and T1. Blocks T3, T4, T6.
Model: opus. Repo: engine.

## Maintainer decision required before this starts

**Where the authoring project lives. Recommendation: a sibling `runtimescripts/` project at the
repository root, mirroring `datascripts/` structure, not a second tree inside it.**

The forcing fact is a TypeScript major-version collision. `typescript-to-lua@1.37.1` peer-depends on
`typescript@6.0.2` exactly (read from `spikes/tstl-runtime/package-lock.json`). `datascripts/` pins
`typescript@7.0.2`. TypeScript 7 is a different compiler; TSTL 1.37.1 drives the TypeScript 6
compiler API and will not load against it. One project cannot hold both, and one `tsc --noEmit` step
cannot typecheck both.

Given the trees are already separate in the way that matters, separating them fully is the honest
reading:

- `CONTEXT.md` keeps **Datascript** and **Runtime Script** as distinct terms. A Runtime Script tree
  nested under `datascripts/` invites exactly the confusion the glossary exists to prevent.
- The two share no authoring surface. A Datascript writes against the Module schema projection in
  `datascripts/generated/`; a Runtime Script writes against `event`, Entity Handles and Host
  Operations, and needs no schema projection at all.
- The two do different things with the TypeScript. A Datascript is **executed** to emit JSON. A
  Runtime Script handler is **compiled** and never executed at author time.

What is reused is the *pattern*, and that is what matters: the same Bun pin, the same
committed-lockfile discipline, the same `src/<package>/` convention, the same canonical writer, the
same `data/.generated/` destination, the same Build Identity mechanism.

**The alternative**, if the maintainer prefers one Bun install: a Bun workspace member at
`datascripts/runtime/` with its own `package.json` pinning TypeScript 6, sharing the root `bun.lock`
and the single `bun install --frozen-lockfile` the CLI already runs. It works, and it costs one
hoisting question and a name that reads wrong. Take it only if the maintainer asks.

The rest of this ticket is written for the recommendation. Rename throughout if the alternative wins.

## Problem

A Package author can ship Runtime Scripts today only by hand-writing Lua into an artifact by hand.
There is no typed authoring surface, no compiler, no emit path, and no place for the sources to live.

## Delivery

### The project

```text
runtimescripts/
  package.json     the pins: Bun 1.3.7, typescript 6.0.2, typescript-to-lua 1.37.1
  bun.lock         committed
  tsconfig.json    the TSTL configuration
  lib/             the authoring library and the ambient typed surface
  src/<package>/   the sources, one folder per Package
  generated/       NOT committed: the event-name union, written from T1's catalogue
```

Sources live outside `packages/`, matching the rule `packages/README.md` already states for
Datascripts: *"only artifacts belong inside a Package folder"*. The issue text says `scripts/*.ts`
inside a Package; follow the repository's established rule instead, and note the departure in T6's
documentation. `scripts/` at the repository root is also already taken by shell scripts.

### The typed surface: `lib/runtime.d.ts`

Ambient declarations, no imports, no exports. It declares exactly what
`module/src/runtime_script.rs` gives an Invocation and nothing more:

```ts
interface Entity {
  readonly name: string;
  readonly is_player: boolean;
  readonly level: number;
  readonly health: number;
  readonly max_health: number;
  readonly map_id: number;
  readonly x: number; readonly y: number; readonly z: number;
}

interface ScriptEvent {
  readonly name: EventName;          // from generated/events.d.ts, T1's catalogue
  readonly actor: Entity | undefined;
  readonly target: Entity | undefined;
}

declare const event: ScriptEvent;
declare function heal(entity: Entity, amount: number): void;
declare function send_chat(player: Entity, text: string): void;
declare function grant_xp(player: Entity, amount: number): void;
```

`undefined`, not `null`: TSTL maps `undefined` to `nil`.

Restrict the standard library to the Host's allowlist. Handler files must not typecheck a call to
anything outside `ALLOWED_GLOBALS` and `ALLOWED_LIBRARY_MEMBERS`, so a name the Host nils out fails
at `tsc` rather than silently at Invocation. Add a Rust test asserting `lib/runtime.d.ts` declares
exactly the three Host Operation names, mirroring the cross-check precedent
`HOOK_EVENT_NAMES` already sets. Do not attempt to cross-check the whole allowlist from Rust; the
event catalogue is the list that grows, and T1 already generates that one.

### Handler files are bare chunks

The Host runs a Runtime Script's whole chunk to completion. There is no exported entry point it
calls: `RuntimeScriptHost::invoke` loads the source and steps it, and the chunk reads the global
`event` and calls the global Host Operations.

So a handler `.ts` file must have **no imports and no exports**. It is a TypeScript *script*, not a
module, which is what makes TSTL emit a bare chunk rather than a module wrapper ending in
`return ____exports`. The ambient `lib/runtime.d.ts` is what gives it types without an import.

```ts
// runtimescripts/src/sharper_fireballs/greet.ts
if (event.actor) {
  send_chat(event.actor, `welcome back, ${event.actor.name}`);
}
```

### The manifest carries the metadata, and is the one Package contract

Because a handler file has no exports, its `script_id`, name, Event Binding, priority and enabled
flag cannot come from it. They come from one manifest per Package, executed under Bun exactly the
way a Datascript is:

```ts
// runtimescripts/src/sharper_fireballs/scripts.ts
import { run } from "../../lib/index.ts";

await run("sharper_fireballs", (pkg) => {
  pkg.compiled(100_001, "sharper-fireballs.greet", "on_login", "greet.ts");
  pkg.lua(100_002, "sharper-fireballs.legacy", "on_death", "legacy.lua");
});
```

This is the answer to *"hand-written Lua and generated Lua reconcile through one Package contract"*.
`pkg.compiled` and `pkg.lua` differ only in whether the source is transpiled first. Both land in the
same `Script[]`, the same ordering, the same duplicate checks, the same artifact. There is no second
path and no second file format.

Signature detail: take priority and enabled as an options argument defaulting to `0` and `true`, so
the common declaration stays one line. Refuse a missing source file, and a `.ts` passed to `pkg.lua`
or a `.lua` passed to `pkg.compiled`, by naming the file.

### The compiler configuration

`tsconfig.json`, from the spike's verified `tsconfig.lua54.json`:

```json
{
  "compilerOptions": { "strict": true, "target": "ES2020", "rootDir": "src", "outDir": "generated/lua" },
  "tstl": { "luaTarget": "5.4", "luaLibImport": "inline", "noHeader": true }
}
```

- `luaTarget: "5.4"` is what the spike ran on piccolo and recorded as
  `PASS piccolo=0.3.3 fuel_per_tick=64 ticks=19`. `run-lua50.sh` and `tsconfig.lua51.json` belong to
  a closed alternative spike; ignore them.
- `noHeader: true` removes TSTL's version banner, which would otherwise move the artifact bytes on
  every TSTL bump that changes nothing else.
- `luaLibImport: "inline"` puts only the used helpers in the chunk. The Host loads one chunk with no
  `require`, so nothing else is possible.
- Confirmed by inspection of `spikes/tstl-runtime/generated/lua54/representative.lua`: the output
  carries no absolute path and no timestamp.
- If T0 landed on branch B, this is also where the parameter-prologue plugin is configured. Read T0
  before starting.

### The artifact

Emit `packages/<package>/data/.generated/script.json` in the shape
`crates/lyracore-package-delta/src/script.rs` documents: `kind: "script"`, `version: 1`, `package`,
`source_hash`, `scripts[]`. Port `datascripts/lib/canonical.ts` rather than reinventing a writer;
byte-identical output is an acceptance criterion, and that file already fixes every byte.

Refuse everything the Rust parser refuses, at the point the author can act on it: a `script_id`
outside the Package Script Range, a name outside `a-z 0-9 - _ .`, an event outside the catalogue, an
empty source, a duplicate identifier or name. The Rust parser stays the authority; refusing early
just moves the message next to the line that caused it, the way `ClaimRegistry.insert` already does
for spell identifiers.

`source_hash` follows `datascripts/lib/index.ts`: the entry manifest's own bytes, hashed whole. Same
honest promise, same limit.

Write nothing on any refusal, so a Package folder never holds a half-built artifact.

### The worked example

One example Package, the Runtime Script counterpart to `fire_nova`: a manifest, one compiled
handler, one hand-written `.lua`. It is what T4's fixture is generated from and what T6 documents.
Keep it small and real.

## Files owned

- `runtimescripts/**` (new)
- `packages/<example>/` (new, the worked example's folder and README)
- `module/src/runtime_script.rs`, the Host Operation name cross-check test ONLY. Coordinate with
  T0, T4 and T5, which also touch this file's test module.

## Out of scope

- Anything in the CLI repo. T3 owns `packages build`, `packages check` and Build Identity. This
  ticket's emit must run standalone under `bun run` so T3 has something to drive, exactly as a
  Datascript does.
- `crates/lyracore-package-delta/**`. The Script Artifact contract is closed.
- The Host's Lua contract: Fuel Budget, allowlists, Host Operations, Entity Handles, the shim.
- Fuel measurement and fixtures (T4). Diagnostics (T5). Documentation (T6).
- Source maps. Client addon TypeScript. Raw uploads.

## Acceptance tests

- A typed event handler compiles to Lua the Host accepts and runs. Prove it end to end, not by
  inspection: `bun run` the example manifest, then feed the emitted `source` to a `module` test.
- `bun ./node_modules/typescript/bin/tsc --noEmit` passes over `lib/`, `src/` and `generated/`.
- An event name outside the catalogue fails the typecheck.
- A call to a name outside the Host's allowlist fails the typecheck.
- Running the same manifest twice over the same sources and the same lockfile produces
  byte-identical `script.json`. Compare bytes, not parsed values.
- A hand-written `.lua` declared through `pkg.lua` reaches the artifact through the same code path
  and the same checks as a compiled handler, and the two sort together by `script_id`.
- `ScriptArtifact::parse` accepts the emitted artifact, and `lyracore-delta-check` validates it.
- Every refusal the Rust parser raises is raised by the emit first, naming the manifest line.
- A refusal writes no file.
- A handler file that declares an import fails the build with a message saying why a handler is a
  bare chunk.
