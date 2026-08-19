# LyraCore development CLI

> Looking for the walkthrough rather than the contract? **[`docs/quickstart.md`](./quickstart.md)**
> is the clone → running realm → connected client path, with prerequisites and troubleshooting.
> This page is the CLI's command and safety contract.

`lyracore` runs the local developer fixture — since #327 a **sharded** one, four databases split
along the continental divide and the open-world/instance one
(§"Sharded out of the box, on purpose"; `--single` collapses it back to one). It
deliberately does not manage production realms, backups, system services, or the installation of
Rust and SpacetimeDB.

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
lyracore import [--accept] [--client-data PATH]
lyracore config
lyracore config set client-data PATH
lyracore character gm NAME true|false
lyracore production status --server SERVER --gateway-log PATH --realm-core DB DATABASE ...
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
| `import` | replace the seed fixture with the real world — consent notice, then the ETL on every database the fixture populates |
| `config` | show, or set, the client-data path `import` and `doctor` remember |
| `character gm` | flip GM commands on or off for a character, on whichever world shard has it |
| `production status` | read-only checks for an explicitly named production topology and the latest gateway start |
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
./lyracore publish                                    # the fixture database
./lyracore publish lyracore lyracore-world-1 realm-core   # several shards, in order
```

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
3. Runs `preflight`, then publishes **only** `lyracore`, through the same internal command
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
