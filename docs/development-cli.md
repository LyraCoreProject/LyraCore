# LyraCore development CLI

> Looking for the walkthrough rather than the contract? **[`docs/quickstart.md`](./quickstart.md)**
> is the clone → running realm → connected client path, with prerequisites and troubleshooting.
> This page is the CLI's command and safety contract.

`lyracore` runs the local developer fixture — since #327 a **sharded** one, four databases split
along the continental divide and the open-world/instance one
(§"Sharded out of the box, on purpose"; `--single` collapses it back to one). It
deliberately does not manage production realms, backups, or the installation of Rust and
SpacetimeDB. The one system service it can touch is the Standalone Supervisor tracked in this
checkout, and only when [`service reconcile`](#service-reconcile--make-a-host-match-the-tracked-unit)
asks it to.

**The CLI lives in its own repository**, [`LyraCoreProject/lyracore-cli`](https://github.com/LyraCoreProject/lyracore-cli).
This repo does not vendor it.

## Running it

From the repository root:

```bash
./lyracore doctor      # are the prerequisites present?
./lyracore dev up      # start (or reuse) the local stack
```

If you installed with [`install.sh`](../install.sh), the same commands are `lyracore doctor` /
`lyracore dev up` from anywhere inside the checkout: `$HOME/.local/bin/lyracore` resolves the
checkout you are standing in and runs *its* pinned CLI. `./lyracore` is the form used throughout this
document; the two are the same program.

`./lyracore` installs the exact CLI commit pinned in **`.lyracore-cli-rev`** into the git-ignored
`.lyracore/cli/<rev>/`, then runs it. The first run for a new revision builds it; later runs exec
the cached binary. So the CLI version is a property of the checkout, not of whatever is on your
PATH.

The install fetches the CLI over anonymous **HTTPS** — no SSH key, no credential helper, nothing to
configure. (It used an `ssh://` remote with `CARGO_NET_GIT_FETCH_WITH_CLI=true` while that repository
was private, which meant a fresh clone's very first command failed for anyone but a maintainer.)

**To bump the CLI:** put the new commit SHA in `.lyracore-cli-rev` and commit it.

## Commands

```text
lyracore doctor
lyracore preflight
lyracore publish [DATABASE ...] [--skip-preflight]
lyracore dev up [--lan <IP>]
lyracore dev status
lyracore dev logs [spacetime|gateway]
lyracore dev smoke
lyracore dev down [--forget]
lyracore account create USER [--password-stdin]
lyracore account alpha-test-tools enrollment REALM_CORE [true|false]
lyracore account alpha-test-tools grant|revoke REALM_CORE ACCOUNT
lyracore import [--accept] [--client-data PATH]
lyracore config
lyracore config set client-data PATH
lyracore client sync
lyracore client pack --out DIR [--zip]
lyracore packages add FOLDER|GIT-URL|NAME [--yes]
lyracore packages build
lyracore packages check
lyracore packages config NAME [KEY [VALUE]] [--new]
lyracore packages disable NAME
lyracore packages enable NAME
lyracore packages list
lyracore packages new NAME
lyracore packages remove NAME [--yes]
lyracore packages replay [DATABASE ...] [--check] [--yes] [--force-all] [--client-data PATH]
lyracore packages update [NAME] [--yes]
lyracore character gm NAME true|false
lyracore production status --server SERVER --gateway-log PATH --realm-core DB DATABASE ...
lyracore service reconcile
lyracore update
```

| Command | What it is |
| --- | --- |
| `doctor` | are the prerequisites for `dev up` present? |
| `preflight` | the OFFLINE deploy gate — the same five checks the core repo's own pre-publish gate runs, natively |
| `publish` | the ONE correct `spacetime publish`, with the two mandatory flags and no path to a wipe |
| `dev up` | start (or reuse) the loopback fixture — four databases: two world shards, the instance pool, realm-core (`--single` for one) |
| `dev status` | process identity, endpoint, and whether the database is actually published |
| `dev logs` | tail the components this CLI started |
| `dev smoke` | the pinned wire harness's generic login smoke against the running fixture |
| `dev down` | stop only the processes this CLI started, and only if the PID is still ours |
| `account create` | provision an account's SRP6 credentials without a password in `argv` |
| `account alpha-test-tools` | read or set automatic enrollment, or grant or revoke one Account |
| `import` | replace the seed fixture with the real world — consent notice, then the ETL on every database the fixture populates |
| `config` | show, or set, the client-data path `import` and `doctor` remember |
| `client sync` | pack `patch-3.MPQ` and every enabled Package's addons, then install them into the configured client |
| `client pack` | build the Client Artifact a player installs: package-authored content only, into a directory of your choosing |
| `packages add` | install a Package from a folder on this machine, from a Git URL, or by bare name from the Official Package Collection, after a trust review and a confirmation |
| `packages build` | regenerate the Module schema typings, typecheck every Datascript against them, then emit and validate each enabled Package's Datascript-generated Package Delta |
| `packages check` | verify every enabled Package's generated artifact against its recorded Build Identity, regenerating the Module typings fresh |
| `packages config` | read a Package's key-values, or write one to every Shard of the fixture topology |
| `packages disable` | move an enabled Package out of the build's sight, keeping it on disk |
| `packages enable` | move a disabled Package back into the build |
| `packages list` | every installed Package: enabled or disabled, where it came from, and whether it has drifted |
| `packages new` | scaffold a new Package offline, by copying and renaming the reference Package this checkout ships |
| `packages remove` | delete a disabled Package, after a confirmation and a check for local changes |
| `packages replay` | reapply every enabled Package's claims and Runtime Scripts onto the named Shards, or the whole recorded fixture topology by default |
| `packages update` | advance a Git-backed Package, or every one of them, to the repository's current commit |
| `character gm` | flip GM commands on or off for a character, on whichever world shard has it |
| `production status` | read-only checks for an explicitly named production topology and the latest gateway start |
| `service reconcile` | make a production host's Standalone Supervisor match the unit tracked in this checkout. Root only |
| `update` | pull the latest LyraCore into this checkout and tell you how to restart it |

**The CLI shells out to nothing in `scripts/`.** It drives this checkout through its *files* —
`Cargo.toml`, `rust-toolchain.toml`, `module/`, `scripts/*.sh` (read, not run), `.wire-harness-rev`.
The shell scripts remain the authority for CI and for anyone driving the repo directly; the CLI's
job is to be usable in a checkout that has neither `scripts/` nor `adapters/`.

`import` is the one deliberate exception, and it is a different thing: `importer/scripts/` is not
private repo plumbing but **shipped tooling**, part of the importer the same way `module/src` is
part of the module. Those scripts carry post-ETL assertions tuned against real dumps over months;
`import` is a façade that adds ordering, a consent gate and per-stage diagnosis on top of them, on
the same path `publish` took before it was absorbed into Rust. A checkout without
`importer/scripts/` gets a named prerequisite error, not a silent no-op.

## `import` — the real world, on your machine

```bash
./lyracore import                                        # prompts for consent and for the client path
./lyracore import --client-data /games/WoW-1.12.1/Data   # prompts for consent only
./lyracore import --accept --client-data /games/…/Data   # scripted: consent answered in advance
```

**The consent notice is printed in full before any network access, client archive read, or database
write.** The CLI may first check that a supplied path and its required filenames exist; it does not
open those archives. The notice names cmangos' `classic-db` and its GPL-3.0 licence,
states that the content describes Blizzard's copyrighted game world and that this project never
distributes it or anything built from it, and states that the DBC half comes from the user's own
1.12.1 client. Only a typed `yes` or `--accept` proceeds; anything else exits 2 having run nothing.
There is no terminal-less default: `import` with no TTY and no `--accept` refuses.

The command has three setup stages: fetch and checksum the pinned dump, resolve the client `Data/`
directory, and build `lyracore-importer`. Each content destination then has three fail-fast stages:
run its importer modes, re-arm its schedules and gather pools, and run profile-aware Verification.
That is six displayed stages under `--single` and twelve on the sharded fixture.

The sharded destination plan is fixed:

| Destination | Profile | Spatial result |
| --- | --- | --- |
| `lyracore` | `alliance-eastern` | Human corridor, Dun Morogh and Loch Modan on map 0 |
| `lyracore-kalimdor` | `alliance-kalimdor` | Teldrassil and Darkshore on map 1 |
| `lyracore-instances` | `instances` | whole map 36, with no open-world terrain, navigation or vmap pass |

The single topology uses `alliance-single`, the union of those bounded continent slices and map 36.
Every importer child names its destination and loopback SpacetimeDB endpoint. The curated
`import-class-spells.sh` pass receives both through explicit environment variables. The full
Spell.dbc catalogue already supplies caster spells, so Verification derives every referenced
caster spell from `game_creature_cast` and `game_creature_spell` instead of maintaining an
independent allowlist.

If a mode fails, later modes and later destinations do not start. The error names the destination,
profile and mode. Fix the cause and rerun the full command. Import families use clear-and-reload, so
the rerun repairs a partial family; the complete multi-destination operation is not atomic.

After `import world`, run `./lyracore import vmaps` when exact model/WMO collision data is needed.
It follows the World Shard profiles and skips the Instance Pool. Importing vmaps does not enable
exact rays. Enabling them is a separate Operator decision after `docs/vmap-rollout.md` Verification.

The automated repository checks cover destination plans, profile fences, canned SQL failures, and
synthetic importer rows. They do not read a real pinned dump or client archives. A lawful real-data
profile dry run, terrain and navigation extraction, vmap generation, and stock 1.12.1 client
playthrough remain Operator Verification; a successful plan or unit test is not evidence that those
checks passed.

One Verification runs from `cargo test` when you point it at your own client: with
`LYRACORE_CLIENT_DATA=<client Data/ dir>`, `cargo test -p lyracore-importer alliance_eastern_terrain`
runs the `alliance-eastern` terrain dry run and asserts every Bounded Map Slice self-check is within
tolerance, Loch Modan included. Run it before an `--apply` after changing a profile anchor; without
the variable it reports itself skipped.

A `--client-data` path and its required filenames are checked **before** the consent notice is
answered, so a typo costs nothing; archive reads still start only after consent. The flagless run is
prompted for at stage 2, in order. Every stage runs from the checkout
root regardless of the directory you invoked `lyracore` from. Every stage's target database is
passed explicitly, never left to a script's default — that default is `lyracore`, and a silent one
is how a shard once had its spells written to a different database entirely.

## `config` — remembering your client-data path

```bash
./lyracore config                          # show what's remembered (or "(unset)")
./lyracore config set client-data PATH     # remember one
```

Stored in the git-ignored `.lyracore/config.json`, repo-local to this checkout — it is not your
`spacetime` login, and it is not shared across clones. `config set client-data` runs the same
validation `import` does (the install-root-vs-`Data/` check, the TBC-or-later rejection) before
writing anything, so a bad path is refused with the same diagnosis either command would give you.

This is also `import`'s fallback chain, not just a separate command: stage 2 tries `--client-data`
first, then the value in `config.json` if one is set and still valid, and only then falls back to
the interactive prompt — which, once you type a path that validates, is **saved to `config.json`
for you**, so a plain `./lyracore import` never asks twice.

## `packages` — install, scaffold, and see what is installed

```bash
./lyracore packages add ~/src/my-package       # asks before it copies anything
./lyracore packages add ~/src/my-package --yes # answer the confirmation in advance
./lyracore packages add https://host/greeter.git   # clone a repository whose root is one Package
./lyracore packages add greeter                # bare name: resolve from the Official Package Collection
./lyracore packages list
./lyracore packages new my-package             # scaffold one from nothing but this checkout
```

A Package is a drop-in folder under `packages/<name>/`. `module/build.rs` discovers it and compiles
its `src/` into the module wasm; `--pack-client` picks up its `client/` half. Installing one adds
code that runs inside the module with full access to every table in the database, so `add` is built
around saying so.

**Everything that can refuse the install happens before anything is copied.** The name must be one
the build accepts (`[a-zA-Z][a-zA-Z0-9_-]*` — the build maps `my-package` onto the module
`pkg_my_package` and panics on anything else). The shape must be one the build accepts: client-only
and Rust-only are both valid, and when `src/` exists, `src/mod.rs` is required. The name must
collide with neither the enabled nor the disabled inventory, compared on the Rust identifier rather
than the folder name — `my-package` and `my_package` fold onto the same module.

Then it prints a **Trust Review** and asks. The review is a deterministic, read-only scan of the
candidate folder using a port of the build's own marker scan, so a commented-out or quoted marker
registers nothing here either. It reports tables, reducers, hooks, tick passes, character-owned
sweeps, addons and client overrides. A Package cannot yet ship a Datascript of its own — the
authoring toolchain lives in `datascripts/` and is described under `packages build` below — so
Datascripts and Runtime Scripts get an explicit "none detected" row rather than silence. The review states
plainly that everything else in the Package's Rust is trusted code and that it is an inventory, not
a security guarantee.

On confirmation the folder is **copied, never symlinked**: a linked Package would compile from a
folder outside the checkout, so `preflight`, `publish` and `client sync` would each read whatever
that folder said at the time. A symlink anywhere inside the candidate is refused for the same
reason. A `.git` directory is skipped.

The install then writes a **Provenance Stamp** — `packages/<name>/.lyracore-package.toml`, holding
the Package Source, the Content Identity of what was copied, and the install time — and runs
`preflight`. A Git Package Source records one key more: the exact commit that was installed.

**An argument that looks like a URL is a Git Package Source.** `https://`, `http://`, `ssh://`,
`git://` and the scp-style `git@host:path` are cloned. Everything else is a path on this machine, as
it always was, other than a bare word that resolves as neither (see below). The repository's root is
the Package, so the Package takes the repository's name without the `.git` suffix, and a repository
whose name the build would refuse is refused here. The clone lands in scratch space under
`.lyracore/`, and what gets installed is a copy of its tree without the `.git`. An installed Package
is a fixed tree, never a working copy. The clone needs credentials or it fails; it never sits
waiting on a hidden prompt.

**A bare word that is not a path on this machine is an Official Package Source.** `packages add
greeter` resolves `greeter` against the one Official Package Collection this CLI knows,
`LyraCoreProject/packages`, which holds several first-party Packages side by side, one top-level
directory each. The collection is cloned the same way a Git Package Source is, the named directory
goes through the same Trust Review and consent question as any other install, and the rest of the
clone is discarded. An unknown name is refused before anything is copied; a name that only differs
from one already in the collection by hyphen/underscore folding is named in the refusal instead of
installed in its place. The Provenance Stamp records the collection's URL and the exact commit the
directory was resolved at. That commit is pinned at install time: `packages update` refuses this
kind by name (see below), so a later commit to the collection can never silently change what is
installed. Picking up a newer one means removing the Package and adding it again.

**It publishes nothing.** The two remaining steps are printed for you to run:

```bash
./lyracore publish        # compile the Package in and publish it to every database
./lyracore client sync    # if the Package ships addons or client overrides
```

A failed `preflight` leaves the copy in place, says the module on the node is unchanged, and names
the exact `rm -rf` that undoes the install — so you can fix the Package where it sits and re-run
`preflight` instead of starting over.

`packages list` reports, per Package: enabled or disabled, its Package Source, its recorded Content
Identity, whether the tree on disk still matches it (`clean` or `LOCALLY DRIFTED`), and what it
registers. A Package with no stamp — dropped into `packages/` by hand, or installed before this
command existed — renders as unrecorded rather than failing the listing.

**`packages new NAME` scaffolds a Package with no network access and nothing external to review.**
It copies `packages/example/` — the maintained reference Package every LyraCore checkout ships,
including a fresh public clone — to `packages/NAME/`, renaming the reference's own identifiers into
the new name, then writes a Provenance Stamp recording a **scaffold** origin rather than a Package
Source (nothing external was installed, so there is nothing to name) and runs `preflight`. The same
name and shape refusals as `add` apply before anything is written. The reference Package is
Rust-only and inert. Its `src/mod.rs` carries one commented hook pattern, and the scaffold has no
`client/` directory. The printed next steps say to add `client/addons/<Name>/` for addons or `client/mpq/`
for client-file overrides, and that `client sync` will pack them in once you do. Growing the Rust
half means wiring more hooks from the catalogue in `module/src/hooks.rs`, following the pattern
`packages/NAME/src/mod.rs` already shows. A scaffold ships no Datascript: the authoring toolchain
in `datascripts/` is checkout-wide today, not per-Package.

## `packages enable`, `disable`, `remove` — taking a Package out of the build

```bash
./lyracore packages disable my-package        # out of the build, still on disk
./lyracore packages enable my-package         # back into the build
./lyracore packages remove my-package         # delete a disabled Package, asks first
./lyracore packages remove my-package --yes   # answer the deletion question in advance
```

**Enabled is a location, not a recorded flag.** `packages/` is what the build discovers.
`.lyracore/packages-disabled/` is git-ignored local state the build cannot see. `enable` and
`disable` rename one folder between the two. Nothing can disagree with the filesystem about which
Packages the next build compiles.

Both directories are on the same filesystem, so the rename is atomic and each verb is the other's
undo. That is why neither asks for confirmation. The Provenance Stamp lives inside the folder, so it
travels with the move and is never rewritten: a re-enabled Package still reports its Package Source
and still reads as `clean` rather than drifted.

Name collisions fail before anything moves, on the Rust identifier rather than the folder name. A
disabled `foo_bar` cannot be enabled next to an enabled `foo-bar`, because both fold onto
`pkg_foo_bar`.

**Before `disable` moves a Package, it reports the Module tables that Package registers.** Disabling
takes those tables out of the schema, so the next publish is a schema change that removes them.
`lyracore publish` never passes SpacetimeDB's destructive wipe flag, so a publish that would drop a
table still holding rows stops instead of deleting them. The report tells you that before you commit
to the move. It does not block it.

**`packages remove NAME` deletes, so it has gates.** It requires the Package to be disabled already,
and points at `packages disable` when it is not: the build has to stop compiling a Package before
the folder goes. It refuses a folder whose Content Identity no longer matches its Provenance Stamp,
and a folder with no readable stamp at all. Both are the same rule. This command may only delete
content that is recorded somewhere else, and local edits to an installed copy are recorded nowhere.
Save them outside the checkout first, or delete the folder by hand.

**None of the three publishes or synchronizes a client.** Each prints the steps it did not run.

## `packages update` — advancing a Git-backed Package

```bash
./lyracore packages update my-package        # advance one Package, asks first
./lyracore packages update                   # advance every Git-backed Package
./lyracore packages update --yes             # answer the questions in advance
```

**Only a Git-backed Package can be updated.** With a name, anything else is refused by name and told
why: a Package installed from a local folder has no newer revision to fetch, a scaffold has no
Package Source at all, an Official Package Source has its commit pinned at install time on purpose,
and a Package Source kind this CLI does not otherwise know is not cloned on the chance that it might
be a repository. With no name, `update` walks both inventories and takes the Git-backed Packages,
disabled ones included. A disabled Package that comes back later should not bring an old revision
with it.

Each update clones the recorded repository and compares the commit it finds against the recorded
one. Same commit, nothing to do. A newer commit gets the same Trust Review and the same question as
an install, and the question names both commits.

**A folder that has drifted from its Provenance Stamp is refused, and nothing is discarded.** An
update replaces the whole folder, so it may only run when every byte in that folder is recorded
somewhere else. That is `packages remove`'s rule, for the same reason: local edits to an installed
copy are recorded nowhere, and neither command can get them back.

**The previous revision is kept until the new one is proven.** The old folder moves out of the
inventory, the new revision installs in its place, and `preflight` runs with it compiled in. Only
then is the old folder deleted. If anything fails, the previous revision goes back byte for byte,
the candidate is discarded, and the error names both commits. `update` publishes nothing and
synchronizes no client; it prints the steps it did not run.

Applying and replaying Package Deltas is `packages replay`, below. Advancing an Official Package
Source through `packages update` remains separate work.

## `packages config` — a Package's key-values, on every Shard

```bash
./lyracore packages config greeter                       # every key and its value
./lyracore packages config greeter greeting              # one value
./lyracore packages config greeter greeting "Hi there"   # write it to every Shard
./lyracore packages config greeter volume 3 --new        # create a key the Package never seeded
```

A Package Config row is one durable value a Package reads and the Operator edits. A Package seeds
its own defaults when it initialises, so the list shows real keys with live values.

**The rows are per-Shard state.** Every database of the fixture topology holds its own copy, and the
Module coordinates none of them. A write therefore goes to every Shard of the recorded topology, the
same set `packages replay` uses when no database is named. A read visits every Shard too: when they
do not agree on a key, the command names each Shard's answer instead of printing one of them. A
Shard with no row for the key reads as `(unset)`, which is the same kind of disagreement.

**The Module owns which keys exist.** Writing a key the Package never seeded needs `--new`, which is
the `allow_new` argument of `set_package_config`. Without it the Module refuses the write and names
the keys the Package does have; the command prints that refusal back unchanged. A Package name that
is not installed is refused before any Shard is read, with the installed list.

Reads go through `spacetime sql`, because `game_package_config` is a public table. The write calls
the Operator-gated `set_package_config` reducer over the same bearer-token path `dev up` uses to
claim the Operator, so the local realm has to be up and claimed.

**A failed write is not rolled back.** The command stops at the Shard that refused and names what
was written, what stopped it, and what was never touched. Re-running the same command after the
cause is fixed rewrites the Shards that already took the value, which changes nothing on them.

## `packages build` — Datascript typings, the typecheck gate, and Package Delta emission

```bash
./lyracore packages build
```

A **Datascript** is author-time TypeScript that describes game data. It is written against the
Module's own schema, so the names and types in it cannot drift from the Module. `packages build`
enforces that and, once a Package has a Datascript, turns it into a validated Package Delta. A
version gate, then up to seven steps, in this order:

0. `bun --version` must match the checkout's pin. A hard failure, not a warning: the next two steps
   run `bun install` and the locked `tsc` for real, against whatever Bun is on `PATH`.
1. `spacetime generate --lang typescript` extracts the schema **through the module wasm** and writes
   it to `datascripts/generated/`. Offline: it builds the module and reads it, and touches no
   database.
2. `bun install --frozen-lockfile` installs exactly what `datascripts/bun.lock` records. Frozen, so
   a build never silently resolves a newer dependency than the next author will get.
3. `tsc --noEmit` is the typecheck gate. Nothing is emitted; the answer is the exit code.

Steps 4 to 7 run only when an enabled Package carries a Datascript. A checkout with none builds
exactly as it did before those steps existed:

4. The Base Snapshot must already exist at `datascripts/generated/base-snapshot.json`, or the build
   fails fast with the exact `lyracore-importer --spell-snapshot` command to build one, once, rather
   than letting every Datascript fail with the same "cannot read" error in turn.
5. Every enabled Package's Datascripts run, one `bun run` subprocess per file, in name order. The
   first script to throw stops the build; later scripts and later Packages never run.
6. `lyracore-delta-check` traces every enabled Package's generated artifacts together, in one
   invocation. This is the same authoritative Rust-side check `packages replay` runs before it
   writes to a Shard, so a Claim Conflict between two Packages is caught by the one implementation
   that also decides whether it may apply, not by a second, looser one.
7. A **Build Identity** sidecar is written next to each artifact that just validated: the hashes
   `packages check` and preflight later recompute to tell whether the artifact is still current.
   Written only after step 6 succeeds, so a sidecar never describes an artifact this build itself
   would have refused.

A Package Delta this command emits is never committed: it is regenerated author-side on every
build and installed from source, the same way `datascripts/generated/` itself is git-ignored. A
Script Artifact is the one exception, since it is package-authored Lua with no client-derived
data, so a Package may commit it under `data/.generated/` to ship its Runtime Scripts.

Generating **first** is what gives the gate teeth. Rename a column in the Module, run
`packages build`, and a Datascript still using the old name fails with the file and line to fix —
at author time, rather than at apply time.

Typechecking **before emission** carries the same reasoning one step further: a Datascript that
fails to typecheck should not run at all, so step 3 gates step 5 the way step 1 gates step 3.

The typings cover **core and installed Package tables alike**, and by construction rather than by a
second mechanism: `module/build.rs` compiles every enabled Package into the same module wasm, so a
table a Package registers is in the schema `spacetime generate` reads. Install a Package that
declares a table, re-run `packages build`, and its row type is in `datascripts/generated/types.ts`.

### The Datascript project

```text
datascripts/
  package.json     the pins: Bun 1.3.7, spacetimedb 2.7.1, typescript 7.0.2
  bun.lock         committed — a fresh checkout resolves exactly these dependencies
  tsconfig.json    strict, noEmit
  src/             the Datascripts. `src/reference.ts` is the maintained reference Datascript
  generated/       written by step 1. NOT committed
```

`generated/` is git-ignored on purpose. It is a ~400-file, 2 MB projection of the module wasm that
`packages build` reproduces on every run. Committing it would put a large mechanical diff in every
schema change and create a second source of truth that can disagree with the Module. The Module is
the schema's authority.

`src/reference.ts` is the standing schema check. It names real columns, so it is the file that fails
when the schema moves under it. Keep it referencing real columns.

### Bun is author-side only

`packages build` is the only command that needs Bun, and authoring Datascripts is the only reason
to run it. **An Operator applying a prebuilt Package needs no Bun and no Node.** Nothing in
`dev up`, `preflight`, `publish` or `client sync` invokes a JavaScript toolchain. That is why
`doctor` reports a missing or different Bun version as a warning and never as a launch blocker.

Install the pinned version with
`curl -fsSL https://bun.sh/install | bash -s "bun-v1.3.7"`.

Datascripts are **trusted author-time code**, run from this checkout by the person who wrote them.
They are not sandboxed and are not described as sandboxed. `packages build` above is what turns one
into a Package Delta; `packages replay`, below, is what applies it to a Shard.

## `packages replay` — reapply a Package's artifacts across the Realm

A Package's artifacts are not a one-shot edit. Its catalogues live on every World Shard and
Instance Pool that owns a copy, so every edit has to reach all of them.

    lyracore packages replay [DATABASE ...] [--check] [--yes] [--force-all] [--client-data PATH]

With no names it targets every Shard of the recorded fixture topology. Named Shards are used
exactly as given. It takes Shard names only; anything flag-shaped is refused before a process
starts, and it never infers a production Shard list.

### The two Import Families it carries

**spell.** The artifact is a Package Delta: claims on columns of rows a base import owns. A base
import replaces the whole family, so the claims replay as the last stage of that family's import.
The Shard therefore reimports `Spell.dbc` first, through the importer, and the claims go on top.

**script.** The artifact is a Script Artifact: whole `game_script` rows. No DBC and no dump holds a
Runtime Script, so this family has no base import and nothing to reload first. The whole enabled
plan goes straight to `apply_package_deltas` in one transaction, and applying it IS the
reconciliation: the Shard ends up holding exactly the scripts the enabled Packages ship and nothing
else.

### The run

1. Preflight. Every enabled Package's artifact is read and digested once, and every target's
   provenance is read, per family. An unreadable artifact, a Package named twice, a Claim Conflict,
   a Runtime Script collision, or an unreachable Shard fails the run before the first write.
2. Apply, Shard by Shard in order. Each Shard takes the spell family through the importer, then the
   script family through the reducer.
3. Stop at the first failure and name the Shard and family. The report lists completion per family,
   including a spell apply that finished on the Shard before its script apply refused the plan. It
   also lists untouched Shards and prints the command to resume.

Resume is the default, and it is decided per family. A Shard is reported complete for a family and
skipped when every enabled Package is recorded in `game_package_import` with the digest this
checkout produces, and no Package is recorded that is no longer enabled. The spell family
additionally requires every row to sit on the Shard's current base import stamp; the script family
has no base import to sit on. Re-running after a failure therefore costs nothing on completed
families, and a Shard already holding this checkout's Package Deltas is still replayed for its
Runtime Scripts. `--force-all` replays anyway.

`--check` prints both plans, writes nothing, and asks nothing. It runs the spell importer when the
checkout has a Package Delta plan, or when a Shard needs an empty spell reconciliation. When only
script-family work remains, a check needs no client Data directory.

Disabling a Package is a replay, not a deletion. Its folder leaves `packages/`, so its artifacts
leave both payloads: the reducer clears the Package Spell Range as it applies the spell family, and
the empty script plan is still sent, which is what takes the Package's Runtime Scripts off every
Shard. Both are destructive and the confirmation names them.

The other claim families a Package may claim in (items, quests, loot, casts and trainers) are NOT
replayed here. Their base import is the world dump rather than a DBC, which is a much larger and
more destructive reload than this verb owns. Reapply those with the importer's own dump modes.

## `packages check` — is every Package Delta still current?

```bash
./lyracore packages check
```

`packages build` writes a Build Identity next to each artifact it emits. `packages check`
recomputes every recorded input from the checkout on disk right now and refuses, naming the
specific input, the moment one no longer matches. `preflight` folds the same report into its own
gate on `publish`'s behalf, so a stale artifact never reaches a Shard.

`datascripts/generated/` is regenerated fresh, every run, with the same `spacetime generate`
invocation `packages build`'s typegen step uses, so a Module schema change makes a committed
artifact stale even on a clean checkout that never ran `packages build` itself. Nothing else is
regenerated: this command never runs Bun and never re-emits a Datascript, so it needs neither Bun
nor a Base Snapshot to do its job.

A missing Base Snapshot is reported and does not fail the check: the snapshot is the Operator's own
client-derived data, and a CI machine holding none cannot regenerate one to compare against. A Base
Snapshot that is present and no longer matches its recorded hash is a real mismatch and fails like
any other input. A missing sidecar is treated as stale; it predates identity tracking, so there is
nothing to compare against.

A checkout with no Packages at all, or none carrying a generated artifact, is a clean no-op.

`packages check` runs against a Package Delta sitting uncommitted in your own checkout: only a
Script Artifact is ever committed, so that is where the drift responsibility lives too — an
author regenerates and re-checks locally, and the Official Package Collection's CI refuses a
Package Delta it finds committed rather than checking it.

## UI Transforms — a Package's edit inside a stock UI file

A Package's `client/mpq/` tree replaces a stock file whole. When two Packages need the same file,
that does not work: one of them has to own it. A UI Transform is the other way in. It is an
anchored edit, declared in `packages/<name>/client/ui-transforms.json`, and several Packages may
edit one file as long as their anchors do not overlap. `client-patch/ui-transforms.json` works the
same way for a checkout-wide edit.

```json
[
  { "path": "Interface/FrameXML/LootFrame.lua",
    "after": "function LootFrame_OnLoad()",
    "insert": "\tPkgLoot_OnLoad();\n" },
  { "path": "Interface/FrameXML/FrameXML.toc",
    "before": "LootFrame.xml",
    "insert": "PkgLoot.lua\nPkgLoot.xml\n" },
  { "path": "Interface/GlueXML/GlueXML.toc",
    "replace": "AccountLogin.xml",
    "insert": "AccountLogin.xml\nPkgGlue.lua\n" }
]
```

Each entry names one `path`, one anchor, and the `insert` text. `path` accepts either slash
direction, must sit under `Interface/FrameXML/` or `Interface/GlueXML/`, and must end in `.lua`,
`.xml` or `.toc`. The anchor is exactly one of `before`, `after` or `replace`. Its text must occur
exactly once in the Baseline: zero occurrences refuses as "anchor not found", several as an
ambiguous anchor, and both name the Package, the file and the anchor text.

`client sync` resolves every anchor against the untouched Baseline before it applies anything, then
applies the edits in the order their anchors appear in the file. The composed result therefore does
not depend on which Package the walk reached first. Two edits whose anchor ranges intersect have no
correct merge and refuse, naming both Packages. A path one Package overrides whole from its `mpq/`
tree while another edits it by transform refuses the same way: a file is replaced or patched, never
both.

The Baseline comes out of your own client's UI archives, read in load order: `interface.MPQ`,
`<locale>/locale-<locale>.MPQ`, `patch.MPQ`, `patch-2.MPQ`, `<locale>/patch-<locale>.MPQ`,
`<locale>/patch-2-<locale>.MPQ`. Archives your client does not have are skipped, and the not-found
message lists every one searched. `patch-3.MPQ` is never read: it is the packer's own previous
output, so composing against it would apply each edit again on every run. The composed file carries
a header comment naming the Baseline hash and the transform hash, so the same client and the same
declarations rebuild byte-identical output.

That output is your client's own bytes with the edits in them, which makes it baseline-derived. It
reaches your client through `client sync` alone. `client pack` refuses a checkout that declares any
UI Transform at all, and names the Package and the file it declared.

## `client sync` — push client content to your own client

```bash
./lyracore config set client-data /games/WoW-1.12.1/Data   # once, if `import` hasn't already
./lyracore client sync
```

A thin wrapper around `lyracore-importer --pack-client <client Data/ dir> --apply` (core repo,
`importer/src/pack_client.rs`): it builds `patch-3.MPQ` from `client-patch/` plus every enabled
Package's `client/` directory, installs the addons into `Interface/AddOns/`, and clears the `WDB/`
cache — so a change to a Package's client-side UI reaches your own client in one command. Refuses
before touching anything if `config set client-data` was never run, naming that command as the fix.

Collision and licensing-firewall failures — two Packages shipping the same archive path or the same
addon name, or a raw `.dbc`/`.MPQ` committed where only our own assets belong — are caught before
any file is written to your client, and name both sources.

There is no managed-content ledger and nothing here deletes an addon. When a Package that used to
ship an addon is disabled or removed, `client sync` warns (best-effort) that the addon it installed
earlier is still sitting in your `Interface/AddOns/` and names the Package — removing it is your
call, by hand. An addon `client sync` never installed (yours, or a third party's) is never flagged.

This is also where a declared UI Transform is composed and packed, against the Baseline in your own
UI archives.

Packaging any of this for someone other than the Operator running the command is `client pack`'s
job, below. It packs strictly less, because a baseline-derived file never leaves this machine.

## `client pack` — build the artifact a player installs

```bash
./lyracore client pack --out ./client-pack
./lyracore client pack --out ./client-pack --zip
```

`client sync` fills your own client. `client pack` builds the Client Artifact instead: a directory
tree a player copies over a stock 1.12.1 install.

```text
<DIR>/
  lyracore-client-pack.json     the manifest, written last
  Data/patch-3.MPQ              only when a source ships at least one mpq/ file
  Interface/AddOns/<Name>/
```

Under it sits `lyracore-importer --pack-out <DIR>` (core repo, `importer/src/pack_client.rs`),
which collects the same sources `client sync` collects and opens no client at all.

The licensing firewall holds by provenance. Only package-authored bytes, the ones an author
committed under `client-patch/` or `packages/<name>/client/`, may enter the artifact. A DBC overlay
and a UI Transform output are both computed from the Operator's own client, so `--pack-out` refuses
each by name and writes nothing. That is why an artifact's `Data/patch-3.MPQ` carries no DBC while
the one `client sync` installs does.

`--out` resolves against the checkout root when it is relative. It is refused inside the configured
client-data path and inside `packages/`. A directory that already holds files and no
`lyracore-client-pack.json` is refused as well, because this command did not create it. A directory
that does hold that manifest is a prior artifact, and it is cleared before the repack.

The manifest is written last, so its presence means the artifact is complete. It records `format`
(`1`), `packed_at` (UTC, RFC 3339), `core_revision` (`git rev-parse HEAD`, or `unknown` when that
fails), one `packages` entry per enabled Package (`name`, `source_kind`, `source`, `revision` from
its Provenance Stamp, and a `content_identity` computed fresh at pack time), and `contents`, every
packed file's relative path, sorted.

`--zip` also writes `<DIR>.zip`, by running the system `zip` binary with `<DIR>` as its working
directory. Neither repo carries a zip library, so a machine without the binary gets that named as
the reason, and keeps the directory that was already built. A `<DIR>.zip` this command did not
write is refused rather than overwritten.

## `preflight` — the offline deploy gate

```bash
./lyracore preflight
```

The same five checks the maintainers' internal preflight script runs, in the same order, with the
same fail-every-check-then-report behaviour, and it touches **no node** — no publish, no call, no
sql, so it is safe against a live stack:

0. `rustc` and the `spacetime` CLI **exactly** match the versions this checkout pins
   (`rust-toolchain.toml`, `module/Cargo.toml`). Exact, not a floor — `doctor` asks for a minimum,
   a deploy gate cannot, because a CLI *ahead* of the pin publishes a schema this repo never tested
   against. A missing or drifted CLI is a hard failure but does **not** skip checks 2–3.
1. the module builds with `--features=debug_reducers` — the feature a publish bakes in and the
   default test config never compiles.
2. real offline wasm schema extraction (`spacetime generate` into a scratch directory that deletes
   itself). `PREFLIGHT_SKIP_SCHEMA=1` still skips it where `spacetimedb-standalone` is unavailable.
3. every `#[client_visibility_filter]` names real tables and columns. Ported to Rust from an
   internal Python script, so **`python3` is no longer a prerequisite**; the port was verified
   differentially against the Python over the real module (same verdict and same message on every
   mutation tried).
4. a script with a configurable `DB` target threads it into every tool it drives.

## `publish` — the one correct deploy

```bash
./lyracore publish                                    # every database of the active fixture topology
./lyracore publish lyracore lyracore-world-1 realm-core   # several shards, in order
```

With no names, this publishes every database of the fixture topology `dev up` and `dev status`
already read from `.lyracore/state.json` — one database for an active single fixture, all of them
for an active sharded fixture, or the default sharded topology if nothing has been recorded yet (a
fresh clone that has never run `dev up`). Naming databases explicitly still publishes exactly those,
in the order given.

Renders exactly `spacetime publish -s local -p <checkout>/module
--build-options=--features=debug_reducers --yes <DATABASE>`, runs `preflight` first, and publishes
several databases sequentially, stopping at the first failure.

Its arguments are database **NAMES**. **Every flag-shaped argument is refused** with exit 2, before
any process starts — `-c`, `--delete-data`, `--clear-database` and `--clear` with a message naming
it as the destructive wipe, anything else with the general refusal. Nothing is forwarded to
`spacetime publish` that this CLI did not put there itself. The only recognised option is
`--skip-preflight`, which announces on stdout that nothing validated the schema.

This is the same contract the maintainers' internal deploy script carries — each of those cases is a
unit test in the CLI too. [`docs/danger-zones.md`](./danger-zones.md) §1 remains authoritative.

## `production status` — read-only evidence for a named topology

```bash
./lyracore production status \
  --server local \
  --gateway-log /tmp/gw.log \
  --realm-core lyracore-realm \
  lyracore lyracore-world-1 lyracore-instances lyracore-realm
```

This command does not infer production from the contributor fixture. The server, log path,
realm-core, and complete database set are required. The server value is forwarded unchanged to
SpacetimeDB inventory and schema probes. The command checks that every named database is reachable,
isolates the latest gateway-start segment, compares configured and expected topology, requires a
distinct coordinator connection per database, and verifies realm-core plus logon/world listener markers.
Address and missing-occupancy signals are warnings; unreachable databases, missing connections,
startup errors, or missing listeners fail the command. It performs no publish, reducer call, SQL
write, or service action. It is the canonical log parser for the production runbook; verify the
operating system's actual sockets separately.

Runtime files live in the git-ignored `.lyracore/` — `state.json` for the processes the CLI started,
`logs/{spacetime,gateway}.log`, and `coordinator-token` (mode `0600`) if this host had no
`spacetime login` and the CLI minted a local identity for it.

## What `dev up` actually does

1. Starts SpacetimeDB on `127.0.0.1:3000`, **or reuses one already listening there.** A node the CLI
   did not start is never recorded and never stopped by `dev down`.
2. Builds the gateway.
3. Runs `preflight`, then publishes **every database in the recorded topology** — all four of the
   default sharded fixture, or just `lyracore` under `--single` — through the same internal command
   `lyracore publish` uses — which is what guarantees `--features=debug_reducers`, `--yes`,
   `-s local`, and the unreachability of a `-c` wipe. No path here renders a `spacetime publish` any
   other way, clears a database, or re-selects the SpacetimeDB server. A checkout the gate rejects
   is not published and no gateway is started against it.
4. Resolves the **coordinator credential** (below), minting one from the local node if this host has
   no SpacetimeDB login.
5. Calls `claim_operator` as that identity (idempotent for the same identity, so repeating `dev up`
   is not an error).
6. Starts the gateway, with the same credential, bound to loopback.

Re-running `dev up` on a healthy stack does nothing. On a partially-up stack it starts only the
missing part.

### The coordinator credential — no spacetimedb.com account required

The gateway's coordinator connection reads the **private** `game_account`/`game_session` tables and
calls the operator-gated `provision_account`, so it must authenticate as the identity that claimed
the operator. Without that it starts, warns, and dies ~15s later on `coordinator subscriptions not
applied within 15s`, which reads like a node fault rather than a credential one.

`dev up` takes the first credential that exists (#297):

1. **`.lyracore/coordinator-token`** — one this CLI minted for this checkout earlier. It wins,
   because it is the identity that already claimed the operator; `claim_operator` is TOFU and
   refuses a different one, so preferring anything else here would lock the checkout out of its own
   database.
2. **`spacetime login show --token`** — an existing SpacetimeDB login is reused, and nothing is
   minted or copied into the checkout.
3. **`POST /v1/identity` on the local node** — a *server-issued* identity, persisted at mode `0600`
   for rung 1.

Rung 3 is what keeps `git clone && ./lyracore dev up` anonymous: `spacetime login` offers only the
spacetimedb.com browser flow, and this fixture never needs it. The claim then follows the
credential — a login token *is* the `spacetime` CLI's identity, so that path still shells out to
`spacetime call`; a server-issued one is sent to the node's HTTP API with a bearer header, because
shelling out would claim the operator for the CLI's identity while the gateway ran as the minted
one.

`account create` walks the same ladder **without** rung 3: a freshly minted identity has claimed
nothing, so provisioning would be refused *after* the password had been read. It refuses first and
names `dev up`.

The credential reaches a child as an environment variable, never argv, and the node as an
`Authorization` header, never a URL — so it is absent from rendered commands, logs, errors and
`state.json`. Its only on-disk copy is the `0600` file above.

The realm it brings up is **playable without a client-data import**: the module's `init` reducer
seeds the realm row, the Human-Warrior start position, the graveyards, the `TEST` account and its
character, and a small demo population. `./lyracore import` (which needs a world-database dump it
pulls for you and client data you already own) is what turns that into the Alliance Human,
Dwarf/Gnome and Night Elf early-game corridors, and it is not a prerequisite for logging in.

### `--lan <IP>` — let another machine on your network in

```bash
./lyracore dev up --lan 192.168.1.50     # one of YOUR machine's private addresses
```

This binds the two **client-facing** listeners — logon 3724 and world 8085 — to that address, and
makes the realm list advertise it (`LYRACORE_REALM_ADDRESS`, read by `gateway/src/config.rs`). Both
halves matter: the seeded `game_realm` row says `127.0.0.1:8085`, so a client that logged in over
the LAN would otherwise be told to open its world connection to *its own* loopback — a realm that
authenticates and then goes nowhere.

**SpacetimeDB does not move.** It stays on `127.0.0.1:3000` in every mode. The address must be
private (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`); a public address or `0.0.0.0` is a usage
error, not a wildcard bind. A running gateway cannot be rebound — switching modes is refused, with
the `dev down` to run first, rather than reported as "already up".

This is a LAN convenience for a contributor fixture, not a deployment mode: no rate limiting, no
TLS, 2004-era password hashing, and a `TEST` account whose password is in this document.

### `dev smoke`

```bash
printf 'test123' | ./lyracore account create TEST --password-stdin   # once
./lyracore dev smoke
```

Runs the pinned wire harness's generic login smoke — logon → world handshake → character enumerate
→ enter world — against the running fixture.

**Where the harness comes from:** the CLI resolves it itself, from the release pinned in
`.wire-harness-rev` — the same semantics the maintainers' internal `wire-harness.sh` script
implements, and the same cache directory. It clones the pinned **tag** (a branch is refused as a
pin) into the git-ignored `.lyracore/wire-harness/<sha>/`, then **verifies HEAD is the recorded
sha**; a tag that has been re-pointed is reported as a supply-chain event, not a stale cache. It
then builds `vanilla-wire` from the harness's own manifest and runs the harness's adapter seam out
of that checkout.

That is the one behavioural change here: `dev smoke` used to hand off to an internal
`adapters/lyracore/run-suite.sh` wrapper, and now it does not touch it at all. That wrapper still
exists, in the maintainers' own working tree, for running the **full** suite by hand; it is only
`dev smoke` that no longer needs it.

`LYRACORE_WIRE_HARNESS_DIR=/path/to/wire-harness` still overrides the pin with a local working tree.
The CLI validates it and announces it on stderr every single time, because a stale local checkout
silently substituted for the pin is a measurement nobody can reproduce.

It refuses to run against a stack that is not up, and in `--lan` mode it connects to the LAN
address rather than loopback.

### Sharded out of the box, on purpose

**This reverses a documented decision** (#327). This section used to read "One database, on purpose",
and the fixture used to *unset* the topology variables so a contributor could not end up with a
gateway pointed at databases the CLI never published. That was the right call for "get a client
connected" and the wrong call for "show what this project is": a visitor who followed the quickstart
never met the sharded topology at all, and sharding is the thing that makes LyraCore different.

> **The fixture shrank on 2026-08-08 (#471):** the region tier — and with it the `lyracore-elwynn`
> region shard and the Northshire Valley | rest-of-Elwynn seam the quickstart used to walk — was
> removed from the codebase. The fixture keeps the broad splits: the continental shard map and
> realm-core. The seam design is preserved in `docs/region-sharding.md` (retired).

> **The instance pool joined on 2026-08-11 (#108):** for the reason the region shard left. It is a
> production tier a fresh clone could not exercise at all, so instance routing was the one split
> nobody developed against.

`dev up` brings up a **four-database** fixture, one per production tier in
[`architecture.md`](./architecture.md) §3.1, all of it published and health-checked by the CLI
itself:

| Database | Role |
| --- | --- |
| `lyracore` | the default world shard — Eastern Kingdoms, where `init` seeds the fixture content (Northshire Valley) and a new character spawns |
| `lyracore-kalimdor` | world shard for map 1, reached via a `LYRACORE_SHARD_MAP` rule |
| `lyracore-instances` | the instance pool — every dungeon run (map 36, the Deadmines), reached by a second rule of the same shape |
| `lyracore-realm` | realm-core — accounts and sessions, the character→shard index, load samples |

So a fresh clone has two live splits: the **Eastern Kingdoms | Kalimdor** continental divide,
crossed by the escrowed cross-database transfer rather than by walking, and the **open world |
instance** one, crossed at a dungeon portal.

Both shard-map rules are the same shape (`<map>:*=<db>`), because an instance map routes exactly
like a continent one. The bucket half of a rule exists to spread ONE map's instances over a pool of
several databases; a one-database pool does not need it, and map 0 is named by neither rule —
Eastern Kingdoms stays whole on the default database.

Three of those four names are production's own, and deliberately: what keeps a fixture off a
production node is the node it is published to (`-s local`, loopback:3000), never the name.

The fixture does not provide the wider world. **Elwynn beyond Northshire and all of the Kalimdor
World Shard are empty** until you run `./lyracore import` with a cmangos dump and your own client
MPQs. The import assigns `alliance-eastern` to `lyracore`, `alliance-kalimdor` to
`lyracore-kalimdor`, and the instance-only `instances` profile to `lyracore-instances`. The single
topology receives their union in `lyracore`. Each destination is verified independently after its
clear-and-reload stages, so rerun the command after fixing a failure.

⚠ **Dungeon populations still spawn on the world shard until you say otherwise.** Routing map 36 off
`lyracore` does not move the spawning with it: `game_config.hosts_instances` is a per-database flag
that defaults to on everywhere, so the gateway logs one WARNING per start saying every entry spawns
~207 creatures on the world writer and evicts them again after the transfer. The run works. To model
the production split, turn the flag off on the world shard once — it survives a republish:

```bash
spacetime sql -s local lyracore "UPDATE game_config SET hosts_instances = false WHERE id = 0"
```

Leave it alone under `dev up --single`, where the one database has to host its own dungeons.

The CLI still *owns* the topology rather than inheriting it: the variables above are set to the
fixture's own values for the child gateway, so a contributor with the production recipe exported in
their shell gets the fixture, not a four-database gateway pointed at production database names. And
`dev status` / `doctor` report **every** fixture database, not just the default — a partial publish
presents as an unrelated mid-session hang (`docs/danger-zones.md` §3), which is a brutal first
experience to debug.

> ⚠ **A schema change now means republishing every fixture database, not one.** `./lyracore publish`
> covers the set; the same rule the production realm has always had (`docs/danger-zones.md` §1.2)
> now applies locally, because the local realm is now genuinely sharded. Republishing only
> `lyracore` after a migration leaves the other three on the old schema.

**`dev up --single` is the escape hatch.** It publishes and runs `lyracore` alone, with
`LYRACORE_SHARD_MAP`, `LYRACORE_SHARD_MAP_FILE` and `LYRACORE_REALM_CORE`
unset — per `gateway/src/config.rs`, an unconfigured shard map collapses every lookup to
`LYRACORE_DATABASE`, so the result is byte-identical to a single-database build. Reach for it when
you are debugging something that is not about sharding, when RAM is tight (four databases cost more
than one), or to establish whether a bug is a sharding bug at all.

## `account alpha-test-tools` controls

```bash
./lyracore account alpha-test-tools enrollment lyracore-realm
./lyracore account alpha-test-tools enrollment lyracore-realm true
./lyracore account alpha-test-tools enrollment lyracore-realm false
./lyracore account alpha-test-tools grant lyracore-realm ACCOUNT
./lyracore account alpha-test-tools revoke lyracore-realm ACCOUNT
```

The first command reads whether genuinely new Accounts receive Alpha Test Tools. Adding `true` or
`false` enables or disables that automatic enrollment. It does not change existing Accounts.
`grant` and `revoke` change one existing Account, with the name normalized to uppercase as it is
during provisioning.

Every form requires the Realm-core database name. Use `lyracore-realm` for the default sharded
fixture and `lyracore` under `dev up --single`. The CLI refuses a missing target, invalid boolean or
action, and a missing Account name before it sends a request. It authenticates the read and every
Operator-only change with the coordinator credential that `dev up` resolved. The credential stays
out of arguments, rendered commands, and output.

## `character gm` — grant or revoke GM commands

```bash
./lyracore character gm NAME true    # level 3 — unlocks the .commands dot-command kit
./lyracore character gm NAME false   # level 0 — revokes it
```

Calls the operator-gated `set_gm_level` reducer, authenticated with the same coordinator credential
`dev up` resolved (§"The coordinator credential" above) — never `spacetime call`, because that
shells out as the `spacetime` CLI's own identity, which is not necessarily the one that claimed the
operator. If the checkout has no coordinator credential yet, it refuses and names `dev up`, the same
remedy `account create` gives for the same problem.

Characters live on **world shards**, never realm-core, so it walks the shards in topology order
(just `lyracore` under `--single`) and calls each in turn, stopping at the first one that has a
player by that name. A miss on every shard is reported once, aggregated, rather than once per shard.

## What `dev status` verifies

Three things that fail independently:

- **process identity** — the recorded PID is still the process this CLI started (start time +
  command name, not just "a PID exists");
- **the endpoint** — it answers where it was bound, which in `--lan` mode is the LAN address, not
  loopback;
- **the databases** — **every** fixture database is actually published on the node
  (`spacetime describe`: schema only, no rows, no writes) — all four in the default topology, just
  `lyracore` under `--single`. A stack whose PIDs and ports are both perfect is still broken if
  nothing was ever published to the node it is pointed at, and that is the one state the other two
  checks cannot see. Checking only the default would miss the *partial* publish, which is worse:
  it presents as an unrelated mid-session hang rather than a loud failure
  (`docs/danger-zones.md` §3).

`doctor` covers the prerequisites for getting there: the project layout, `rustc` against the
version this checkout declares, Cargo, SpacetimeDB 2.7.1, the WASM target, and the three ports. It
asks for a **minimum** version; `preflight` asks for an **exact** match against the pins, because
one is "can you build this at all" and the other is "is it safe to deploy this".

`doctor` also reports the **client data** path from `config.json` (§"`config`" above): unset, or set
to something that no longer validates, is a `⚠` naming the problem and the `config set client-data`
fix — never a failure, because a client is only needed for `lyracore import`, not for `dev up`, and
`doctor` gates the latter.

## Stopping things safely

A bare PID is not an identity — PIDs get reused, and signalling a recycled one kills a stranger's
process. Each recorded PID is stored with the process start time and command name (read via POSIX
`ps`; no `/proc`, no GNU-only flags, so Linux and macOS behave the same).

`dev down` compares that identity before signalling anything. If the PID now belongs to something
else it **refuses and kills nothing**, telling you to run `dev down --forget`, which drops the
record without signalling.

## Passwords

`account create` reads the password from a hidden terminal prompt, or from one bounded stdin line
with `--password-stdin`. It is handed to `gateway provision USER --password-stdin` over the child's
stdin and never becomes a command-line argument, so `ps` shows only the username. It is held in a
zeroized buffer, and is absent from rendered commands, logs, error messages, and `state.json`.

```bash
printf 'hunter2' | ./lyracore account create TEST --password-stdin
```

## `service reconcile` — make a host match the tracked unit

```bash
sudo ./lyracore service reconcile
```

For a **production host** only, and the encoded form of the manual install in
[`docs/danger-zones.md`](./danger-zones.md) §3. It makes the host's Standalone Supervisor match
`deploy/systemd/spacetimedb-standalone.service` in this checkout. Service Reconciliation is one
job, so the verb owns the git steps too:

1. `id -u`, before the fetch. The plan resets the checkout and then writes to
   `/etc/systemd/system`, so it asks for the privilege once rather than stopping halfway for a
   password.
2. The same checkout update `update` does. A tracked local edit still refuses everything, the
   service change included.
3. The host prerequisites the unit names: its `User=` account, its `ExecStart` binary, its
   `--data-dir`, and the directory holding its `StandardError=append:` log. Each missing one is a
   refusal naming the command that fixes it. None is created for you.
4. Conflicting-service detection. Every active unit whose `ExecStart` or `WorkingDirectory` claims
   the same data directory or listen address. A hand-rolled legacy `spacetimedb.service` is named
   and refused, never migrated and never stopped on your behalf, so two nodes cannot race for one
   port and one data directory.
5. `install -o root -g root -m 0644` into `/etc/systemd/system/`, then `systemctl daemon-reload`,
   `enable`, `restart`.
6. Verification. `systemctl show` must report `ActiveState=active` plus the `LimitNOFILE` and
   `StandardError` the tracked unit declares. A node that came back with the inherited
   1024-descriptor ceiling is reported as NOT reconciled instead of passing.

Every expected value is read out of the tracked unit rather than duplicated in the CLI, so it
cannot certify a host against a contract this checkout no longer ships. The node's persistent
database directory is only ever checked for existence: never created, moved or deleted. Two runs
converge on the same end state, and it runs even when the checkout already sits on `origin/main`,
because deployment drift is independent of git drift. It restarts the node every time, so every run
costs a short outage.

Steps 3 to 6 read the host before they change it, so a refusal there leaves the checkout on
`origin/main` and the host as it was. The reset in step 2 comes first on purpose: the unit to
install, and the contract to check the host against, are read out of the updated checkout.

No gateway rebuild, module publish or schema migration is implied. Those stay operator decisions.

## `update` — pull and restart

```bash
./lyracore update
```

Fetches `origin`, then refuses — listing the files — if the checkout has any **tracked** edit;
untracked files are fine. The next step is a `git reset --hard origin/main`, and the refusal exists
so that step never has anything of yours to discard. A clean tree at the same commit as `origin/main`
prints "already up to date" and does nothing else. Otherwise it reads `.lyracore-cli-rev` before
resetting, resets, and prints the old and new commit — plus a note if the CLI pin itself moved,
since that revision installs on the *next* `lyracore` invocation, not this one.

It does not restart anything for you: it prints `./lyracore dev down && ./lyracore dev up` as the
following step, the same rebuild-and-republish restart §"Everyday commands" in the quickstart
documents, because a schema change coming in on the pull needs every fixture database republished,
not just a process restart.

## Exit codes

| Code | Meaning |
| ---: | --- |
| `0` | Success — including "already up", "already down", and a `doctor` with only warnings |
| `1` | Operational failure: missing prerequisite, failed subprocess, or a refused foreign PID |
| `2` | Invalid invocation, or not inside a checkout |

`doctor` exits nonzero only for launch-blocking failures. A busy port is a warning, not a failure —
it is usually your own running stack.
