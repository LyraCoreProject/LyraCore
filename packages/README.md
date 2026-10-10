# Packages

Each folder here is an enabled Package. The build compiles its `src/` into the Module, and
`lyracore client sync` installs its `client/` half. A Package can also contain Runtime Script
sources in `scripts/`, Datascript sources in `datascripts/`, or generated artifacts in
`data/.generated/`. Any one of these is enough. Core ships no enabled Packages.

Start with the Reference Package ladder in the
[Official Package Collection](https://github.com/LyraCoreProject/packages#packages):

| Rung | Example |
| --- | --- |
| `example-script` | TypeScript welcome on login and Lua welcome on level-up |
| `example-client` | A client addon and a UI Transform |
| `example-data` | A Datascript that clones one spell |
| `example-rust` | A Rust hook, Package Config and a Package test |
| `example-all` | Rust asks a Runtime Script, with Package Config as the fallback |

```bash
./lyracore packages new my-welcome
./lyracore packages new my-welcome --from example-rust
./lyracore packages add example-script
```

`new` defaults to `example-script`. Both `new` and a bare-name `add` fetch the collection tag
matching the Package API version in [`docs/package-api.md`](../docs/package-api.md). A missing tag
refuses the operation. The Provenance Stamp records the collection revision; a scaffold also
records its rung and remains the author's own copy. `packages update` can advance an installed
Official Package Source at the compatible tag, but does not replace a scaffold.

A folder or Git URL passed to `packages add` keeps its own source rules. Review a Package as you
would a Core patch. Its Rust is trusted Module code.

## Applying packages

After creating, installing or changing a Package, apply the enabled Packages to your Shards:

```bash
./lyracore packages apply
```

With no Shard names, the command uses the recorded development topology. Pass Shard names to
choose other Shards. It builds missing or stale artifacts from enabled sources, then applies
spell Package Deltas and Script Artifacts. Current Script Artifacts need neither Bun nor client data.
A Datascript build needs Bun and a Base Snapshot. If the snapshot is missing, `apply` extracts one
from your own client data. Applying spell Package Deltas also reads client data. Use
`--client-data PATH` to select the client's Data directory.

If an enabled or disabled Package contains `src/mod.rs`, or a Shard records pending Package
Teardown, `apply` publishes the Module built from the enabled Packages. Including disabled Rust
Packages in this decision ensures the publish removes their code. After each publish it calls
`debug_repair_after_publish` to restore Module schedules and fixtures. Each Shard completes its
publish, repair and artifact application before the next Shard starts.

The command asks once before changing the Realm. Use `--check` to prepare local artifacts and
validate the plan without Realm writes. Client content uses `lyracore client sync` separately.
For flags and retry behavior, see the
[CLI commands](https://github.com/LyraCoreProject/lyracore-cli/blob/main/docs/commands.md).

## Building source

Use `lyracore packages build` to build artifacts without applying them. It runs Package-local
`datascripts/*.ts` as well as Core's legacy `datascripts/src/<package>/*.ts`. Datascripts use the
Authoring Library and a Base Snapshot from your own client data. Their Package Deltas and Build
Identities stay local under `data/.generated/`.
Never commit a Package Delta to the collection.

Runtime Scripts use `scripts/*.ts` or `scripts/*.lua`. Each file declares one named function and
one Event Binding. TypeScript checks the event payload:

```ts
function welcome(event: PlayerLoginEvent): void {
  send_chat(event.player, "Welcome!");
}

events.player.onLogin(welcome);
```

Lua uses the same function and Event Binding shape:

```lua
local function ding(event)
    send_chat(event.player, "You reached level " .. event.newLevel)
end

events.player.onLevelUp(ding)
```

The event catalogue generates TypeScript declarations and Lua editor definitions. The checkout's
editor configuration loads them for installed Packages. Login and level-up carry a Character in
`event.player`; level-up also carries `event.newLevel`, even while the Character's level snapshot
still holds the old value. Other events expose their declared payload fields and optional
`event.actor` and `event.target` Entity Handles.

A second argument sets options, for example `events.player.onLogin(welcome, { priority: 10 })`.
Priority defaults to `0` and enabled state to `true`. Lua uses `{ priority = 10 }`. An Event Binding
must be an unconditional top-level call to a named function in the file. Put shared helpers in that
file too. A function can return a numeric Script Answer for a Package that calls `ask()`.
Bind a Package Event with `events.package.on("welcome", welcome)`; the builder supplies the
shipping Package's prefix.

`packages build` records stable Script Identities in the Package-root `script-ids.json` and emits
a Script Artifact with its Build Identity. Commit all three files with the sources. The Runtime
Script name remains `<package>.<file stem>`, so changing the function name keeps its identity.
Deleted entries stay reserved. `packages new` omits the copied identities and artifacts so the
new Package gets its own IDs. A filename change creates a new script identity.

Existing `@event` and `@id` Script Directives still build. To migrate, build once to record the
identities, then replace the directives with a named function and Event Binding. An existing
Script Artifact also supplies its original IDs. The builder refuses a disagreement between
recorded identities, legacy directives and an existing artifact.

After creating or editing a Package, run `packages apply` to build and activate it.

## Client content

Put addons under `client/addons/<Name>/`. A UI Transform in `client/ui-transforms.json` inserts
text at one unique `before`, `after` or `replace` anchor in a FrameXML or GlueXML Baseline.
`client sync` composes it against your own client's bytes. `client pack` refuses to distribute
that derived output. The client rung supplies an insertion, never a copied Baseline.

## Config and location

Rust Packages seed Package Config with `ensure_package_config_default` and read it through
`game_package_config`. Seeding preserves a value the Operator already changed. On a development
topology, `lyracore packages config NAME KEY VALUE` changes it without a republish.

`packages disable` runs Package Teardown and moves the folder to `.lyracore/packages-disabled/`.
`packages enable` moves it back. Folder location is the enabled state. Run `packages apply` after
either command to apply the enabled set.

See the [CLI commands](https://github.com/LyraCoreProject/lyracore-cli/blob/main/docs/commands.md)
for build, apply, client installation and Package management.
