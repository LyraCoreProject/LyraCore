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

## Building source

`lyracore packages build` runs Package-local `datascripts/*.ts` as well as Core's legacy
`datascripts/src/<package>/*.ts`. Datascripts use the Authoring Library and a Base Snapshot from
your own client data. Their Package Deltas and Build Identities stay local under `data/.generated/`.
Never commit a Package Delta to the collection.

Runtime Scripts use `scripts/*.ts` or `scripts/*.lua`. The build compiles them into a Script
Artifact and records its Build Identity. These two files may be committed to the collection.
Each source starts with an event and a durable Script ID:

```ts
// @event on_login
// @id 100300
```

Optional `@priority` defaults to `0`, and `@enabled` defaults to `true`. TypeScript declares
`function script(): number | void`. A numeric Script Answer is what a Package reads through
`ask()`. Choose distinct IDs before installing multiple copies of a rung. After scaffolding,
run `packages build` to produce artifacts for the renamed source.

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
`packages enable` moves it back. Folder location is the enabled state.

See the [CLI commands](https://github.com/LyraCoreProject/lyracore-cli/blob/main/docs/commands.md)
for build, replay, client installation and Package management.
