# LyraCore quickstart — from `git clone` to a connected client

This is the whole path: an anonymous checkout, one command to start a seeded realm, one command to
make an account, and a stock 1.12.1 client logged into it. No repo knowledge assumed, no client-data
import, no world database to source. What you get is a **seed fixture** — enough world to prove every
layer works. [Getting the real world](#getting-the-real-world) is the separate, optional step that
turns it into Elwynn.

**Linux and macOS.** Windows/WSL is not covered (and not supported today).

**Read this first, it decides what you need:**

- **You supply the game client.** LyraCore distributes **no game client, no game assets, and no game
  data** — you must supply your own legally obtained **1.12.1, build 5875** client. This project is
  not affiliated with, endorsed by, or sponsored by Blizzard Entertainment, Inc.; *World of Warcraft*
  is their trademark, used here only to name the protocol this software interoperates with.
- **This is a local development fixture, not a server you host.** Everything below binds to
  `127.0.0.1` (or, opt-in, one private-LAN address). It is not hardened for the public internet and
  must never be exposed to it — see [Security posture](#security-posture-local-development-only)
  before you open any port.

**The short path.** One line clones the repository into `./LyraCore`, checks everything §1 lists, and
installs the `lyracore` launcher — read §1 anyway if you want to know what it decided:

```bash
curl -sSfL https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh | sh
cd LyraCore && lyracore doctor
```

It never runs `sudo`, and it refuses to touch an existing `./LyraCore`. Everything below also works
without it: [§2](#2-clone-and-check) has the plain `git clone` form.

---

## 1. Prerequisites

| Thing | Version | Why that version |
| --- | --- | --- |
| **Rust** | **1.93.0** | The workspace declares `rust-version = "1.93.0"` and `rust-toolchain.toml` pins the same. **Nothing lower builds** — `spacetimedb`/`spacetimedb-sdk` 2.7.1 declare `rust-version = "1.93.0"` themselves, so there is no lower MSRV to fall back to. You do **not** install it by hand: rustup reads `rust-toolchain.toml` and fetches/switches to 1.93.0 (plus the `wasm32-unknown-unknown` target) for any `cargo` command run inside the checkout. |
| **SpacetimeDB CLI** | **exactly 2.7.1** | `module/Cargo.toml` pins `spacetimedb = "=2.7.1"`. `./lyracore preflight` and CI require an exact match. `./lyracore doctor` only checks for 2.7.1 or newer, so a newer CLI passes `doctor` and then fails `preflight`. Install 2.7.1. |
| **git**, **curl**, a C toolchain | any current | Cloning, the SpacetimeDB installer, and linking the Native Host. |
| **openssl** command | OpenSSL 3 or newer | Native startup creates and checks its persistent signing keys with this command. LibreSSL is not supported. |
| **wasm-opt** (`install.sh` offers a user-local install; otherwise Ubuntu/Debian package `binaryen`, macOS `brew install binaryen`) | any current, *optional* | `spacetime publish` runs it to optimise the module WASM. It is **not required** — publish still succeeds without it — but the module is the server's authoritative game logic, running every reducer on every shard, so skipping it means shipping that logic unoptimised, silently, on every future publish too. Without it you'll see `Could not find wasm-opt to optimise the module` mid-publish; `lyracore doctor` reports it as an optional `⚠` rather than a blocking `✗`. |

### Ubuntu 24.04

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev openssl git curl
```

Install `libssl-dev` and `pkg-config` for the native Rust dependencies that link against OpenSSL.

```bash
# Rust (rustup — it will pick up the pinned 1.93.0 toolchain from the checkout automatically)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
```

### macOS

```bash
xcode-select --install          # Command Line Tools: git, clang, make

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
```

On Apple platforms, `native-tls` uses Security.framework. Native startup requires genuine OpenSSL
for its signing keys. With Homebrew installed, use [the OpenSSL 3 formula](https://formulae.brew.sh/formula/openssl@3):

```bash
brew install openssl@3
export PATH="$(brew --prefix openssl@3)/bin:$PATH"
openssl version
```

Keep that PATH entry in your shell configuration. `openssl version` must report OpenSSL 3 or newer.
The CLI refuses LibreSSL before writing key files.
The installer and `./lyracore doctor` also check this requirement before Native Host startup.

### SpacetimeDB 2.7.1 (both platforms)

The installer is a **version manager**: it always installs the latest `spacetime` shim, and you then
pin the version side by side. (An older `--version <x.y.z>` installer flag exists in some docs — it was
removed upstream and now errors with "unexpected argument".)

```bash
curl -sSf https://install.spacetimedb.com | sh -s -- -y
export PATH="$HOME/.local/bin:$PATH"        # add this to your shell profile too
spacetime version install 2.7.1
spacetime version use 2.7.1
spacetime --version                          # expect: spacetimedb tool version 2.7.1
```

This is the same sequence the maintainers' build pipeline runs on current Ubuntu and macOS, and the
same one `install.sh` runs (after asking) when it does not find 2.7.1. That `$HOME/.local/bin` on
`PATH` is a prerequisite of the documented install either way — the installer offers to append it
for you, once, as a single marked line.

### You do NOT need

- a game client on *this* machine (it can live on another machine on your LAN — see
  [§6](#6-playing-from-another-machine-on-your-lan));
- any MPQ/DBC extraction, any third-party world database, or a content import. The seeded
  fixture is playable as-is. (An import turns it into a full zone later; it needs client data you
  extract yourself, it is maintainer-run for the alpha, and it is not a prerequisite for logging in.)
- **a spacetimedb.com account.** `spacetime login` is *optional*: if you already use SpacetimeDB,
  `dev up` reuses that identity; if you do not, it mints one from your own local node instead
  ([§3](#3-start-the-stack)). Nothing in this guide signs you up for anything.

---

## 2. Clone and check

### With the installer

```bash
curl -sSfL https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh | sh
```

or, to read it first — the right instinct for anything you pipe into a shell:

```bash
curl -sSfLO https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh
less install.sh && sh install.sh          # --yes answers every prompt with "yes"
```

It does §1 and this section in one pass: checks the prerequisites (**never** running `sudo` — a
missing system package prints the exact `apt-get` line for *you* to run), offers to install rustup
and SpacetimeDB 2.7.1 if they are absent, clones into `./LyraCore`, and installs the `lyracore`
launcher into `$HOME/.local/bin`. It refuses a directory that already exists rather than merging into
it, and everything it writes is: `./LyraCore`, `$HOME/.local/bin/lyracore`, the two upstream
installers' own roots, and — only if you say yes — **one** marked line in **one** shell profile.

A piped run cannot prompt, so `curl … | sh` needs `sh -s -- --yes` if anything is still missing:

```bash
curl -sSfL https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh | sh -s -- --yes
```

### Without it

```bash
git clone https://github.com/LyraCoreProject/LyraCore.git lyracore
cd lyracore
./lyracore doctor
```

`./lyracore` is a small POSIX shim. On its first run it installs the exact development-CLI commit
pinned in `.lyracore-cli-rev` into the git-ignored `.lyracore/cli/<rev>/` and runs it — so the CLI
version is a property of *this checkout*, not of whatever is on your `PATH`. That first run compiles
the CLI (a minute or two); every later run execs the cached binary.

### `lyracore` vs `./lyracore`

They run the same thing. `$HOME/.local/bin/lyracore` — the ten-line launcher the installer writes —
walks up from your current directory for the checkout containing `.lyracore-cli-rev` and `exec`s
**that** checkout's `./lyracore` by path. So:

- `lyracore doctor` works from the checkout root **and from any subdirectory of it**;
- every clone still runs the CLI revision *it* pins, so several checkouts coexist;
- outside any checkout it exits `2` with `not inside a LyraCore checkout` rather than guessing;
- the launcher itself never needs updating — the pin it honours lives in the checkout.

`./lyracore` keeps working verbatim, and is the form used for the rest of this document: it is what
CI runs, and what to use if you skipped the installer or declined the `PATH` line.

A healthy `doctor` looks like this:

```text
  ✓ project layout   /home/you/lyracore
  ✓ Rust             1.93.0 (requires 1.93.0)
  ✓ Cargo            cargo 1.93.0 (083ac5135 2025-12-15)
  ✓ SpacetimeDB      2.7.1 (requires 2.7.1)
  ✓ WASM target      wasm32-unknown-unknown installed
  ✓ ports            3000, 3724, 8085 free

doctor: ready for `lyracore dev up`.
```

`doctor` exits **non-zero only for launch-blocking failures**. A `⚠` line is informational — a busy
port in particular is usually your own stack, which is why it is a warning and not a failure.

---

## 3. Start the Realm

```bash
./lyracore dev up
```

The CLI builds the pinned Native Host and starts one SpacetimeDB process. That process owns the
HTTP endpoint on `127.0.0.1:3000`, logon on `127.0.0.1:3724` and world on `127.0.0.1:8085`.

Startup runs preflight, publishes one Shard named `lyracore`, claims the Operator, installs the
GUID range and sets the Realm address. It reads a WoW authentication challenge from the world
listener before reporting success. Import world content before play with `./lyracore import`.

All maps, Instances, Accounts and Characters live in this Shard. Maps and Instances still separate
gameplay and AOI. Moving between them no longer requires cross-Shard coordination. `--single`
remains accepted and has no effect on native startup.

The first build uses the host's pinned Rust 1.97.1 toolchain, in addition to Rust 1.93.0 for the
Module. Install it with `rustup toolchain install 1.97.1` if it is missing.

The CLI stores the new local Realm in `.lyracore/native-data`. It does not merge data from a legacy
multi-Shard Realm. Stop a recorded legacy runtime with `./lyracore dev down` before starting the
native runtime. Startup refuses an unrelated process on a required port.
[Migrating to the Native Host](./migrating-to-native-host.md) gives the full steps.

Repeating `dev up` reuses a healthy native runtime. `dev down` stops the process without deleting
its data. Changing listener addresses requires a restart.

### The Operator identity

Publishing and native requests must use the same identity. Startup uses the retained credential
in `.lyracore/native-owner-token`, an existing SpacetimeDB login, or a new identity minted by the
local node. It saves the credential and publisher configuration in `.lyracore/native-owner-token`
and `.lyracore/native-cli.toml`, with mode `0600`.

To publish to a native node using a different HTTP port, keep that node's publisher configuration
in `.lyracore/native-cli.toml` and run `./lyracore publish --server http://127.0.0.1:13000`.
The explicit target must use a numeric loopback address and a nonzero port. An SSH tunnel can
connect that address to a remote test Realm. `publish` does not start the Host or change its
listener addresses. Provision the Realm's advertised world address separately.

Keep both files with the Realm's data. A new identity cannot replace the existing publisher or
Operator. No spacetimedb.com account is required.

---

## 4. Create an account

The seeded `TEST` account exists but has **no usable password** until you set one — SRP6 credentials
are computed locally and written by this command:

```bash
./lyracore account create TEST
# Password for the new account:      (hidden prompt)
```

or non-interactively:

```bash
printf 'test123' | ./lyracore account create TEST --password-stdin
```

Notes that will save you a round trip:

- The password is read from a hidden TTY prompt or one stdin line — **never** as a command-line
  argument, so `ps` shows only the username. It is held in a zeroized buffer and never reaches logs
  or `.lyracore/state.json`.
- **Max 16 bytes** (the 1.12.1 client's own limit). Longer is rejected:
  `the password is 18 bytes; the 1.12.1 client allows at most 16`.
- Vanilla SRP6 normalises both halves to **uppercase ASCII**, so `./lyracore account create test`
  provisions `TEST`, and the password is case-insensitive. Non-ASCII characters are rejected.
- Any name works, not just `TEST`: `./lyracore account create ALICE` creates a fresh account (with no
  characters — make one on the client's character-creation screen).

### Alpha Test Tools enrollment

New Accounts receive Alpha Test Tools by default. These commands inspect or change that setting and
grant or revoke Alpha Test Tools for an existing Account:

```bash
./lyracore account alpha-test-tools enrollment lyracore-realm
./lyracore account alpha-test-tools enrollment lyracore-realm false
./lyracore account alpha-test-tools grant lyracore-realm TEST
./lyracore account alpha-test-tools revoke lyracore-realm TEST
```

Use `lyracore` instead of `lyracore-realm` after `dev up --single`. The commands use the coordinator
credential created or selected by `dev up`. They refuse a missing Realm-core target, invalid action,
or missing Account name before changing state.

### Optional: prove it over the wire before touching a client

```bash
printf 'test123\n' | ./lyracore dev smoke TEST Tester --password-stdin
```

This hands off to the pinned headless wire harness and drives a **real build-5875 login** against the
running fixture: logon → encrypted world handshake → character enumerate → enter world. It signs in
as the fixture account, so provision `TEST` first (`test123` is the conventional fixture password).
It refuses to run against a stack that is not up.

---

## 5. Point your client at it

You need an unmodified **1.12.1, build 5875** client. The exact build matters — the native Module speaks
that build's wire format (logon `protocol_version` 3) and nothing else. Other builds are not rejected
with a friendly message; they fail at the logon challenge or drop mid-handshake.

1. In the client folder, edit **`realmlist.wtf`** (it is in the client root, or under
   `Data/<locale>/`) so it contains exactly:

   ```text
   set realmlist "127.0.0.1"
   ```

   The client connects to the **logon** server on TCP **3724**; it learns the world address
   (`8085`) from the realm list.

2. Launch the client and log in with the account and password from §4.

3. Pick the realm, then either play the seeded **`Tester`** character or create your own on the
   character-creation screen.

Running the client on the same Linux box is fine — recent **wine** runs build 5875 (PE WoW64, no
32-bit prefix needed). If frames dip, DXVK plus the client's Direct3D 9 backend is the combination
that holds up here. No macOS client setup is tested; if you are on macOS, the reliable path is the
LAN one below.

---

## Getting the real world

What you just logged into is a **fixture**: a few demo creatures, one questgiver, a gather node or
two, and a flight master with one short directed test route. It exists so that every layer — spawn,
combat, quest, loot, movement, and taxi catalogue lookup — has something to act on without anybody
needing a world database. The synthetic taxi ids fit the vanilla wire mask for headless tests, but
the unmodified client has no matching map art for invented nodes. Use the locally imported
`TaxiNodes.dbc` catalogue for visual client flight testing. It is not a zone.

Opening the fixture discovers its source. Once its destination is known, direct activation charges
the imported fare and selects the source node's race-appropriate Horde or Alliance mount display in
one authoritative transaction. A duplicate request cannot create a second active flight or charge
the purse twice. The Module queues the success reply before it arms authoritative scheduled
movement. Supported direct routes then advance to an exact durable landing; routes with delays or
nonzero point flags are refused until those semantics are implemented.

The real thing is the Alliance early game: the Human Elwynn/Westfall/Redridge corridor, Dun Morogh
and Loch Modan, Teldrassil and Darkshore, plus Deadmines. Each World Shard receives its bounded
continent slices, terrain and navigation. The Instance Pool receives map 36 without a duplicate
open-world pass. **That data is not in this repository and never will be.** It is reconstructed on
your machine, from two sources that are yours and not ours to hand out:

- **cmangos' [`classic-db`](https://github.com/cmangos/classic-db)** — a community-maintained vanilla
  world database, licensed GPL-3.0. Its *content* describes Blizzard's copyrighted game world
  (creature stats, quest text, spawn coordinates); cmangos offers it as non-commercial fair-use demo
  content. `lyracore import` pulls it from cmangos' own public repository, at a commit this repo
  pins, straight to your disk. We do not host it, redistribute it, or ship anything built from it.
- **your own 1.12.1 client.** Spells, talents, areas, factions, terrain and model geometry are read
  out of the `Data/` directory of the client you already have. There is no substitute and we do not
  supply one.

One command, and it asks before it does anything:

```bash
./lyracore import --client-data /path/to/WoW-1.12.1/Data
```

It prints the notice above in full and waits for you to type `yes`. Then it pulls `classic-db`
(checksum-verified against the pinned commit), confirms the client path, and runs the world ETL plus
the curated class-spell trainer offerings on `lyracore`. Its `alliance-single` profile includes
both continent corridors, Deadmines on map 36 and Deeprun Tram on map 369. Pass `--accept` to answer the consent question in advance for a
scripted run, and leave `--client-data` off to be prompted for the path.

**What to expect.** The pull is a few hundred megabytes of git history. Each ETL is the long
part: **tens of minutes**, most of it in the terrain and navigation passes, which rasterize each
bounded slice cell by cell. The native import selects the combined content profile in one Shard. It ends with global catalogue checks, named Human, Dwarf/Gnome and Night Elf
corridor checks, caster-spell completeness, and distinct Deadmines checks. A failed stage stops later
work and names the destination and mode. Fix the cause and rerun: each owned family clears and
reloads, so a rerun repairs a partial family. This is not multi-process atomicity. The stack must be
up (`./lyracore dev up`) because the ETL writes through the running node.

**It is optional, and it is one-way-ish.** Everything in §§1–5 works without it, and the wire smoke
tests run against the fixture. The import clears and reloads whole content families, so running it
points that database at the imported world instead of the fixture; publishing to a fresh database
gives you the fixture back.

**The line we do not cross:** we ship the tools, you fetch the data, and no database produced by
this pipeline is ever distributed or hosted by this project. What you do with the one on your disk
is your call and your responsibility. [`docs/data-ingestion.md`](./data-ingestion.md) is the full
statement, and `importer/scripts/` holds the scripts `lyracore import` drives if you would rather
run a stage at a time.

Exact model/WMO rays are a separate step. `./lyracore import vmaps` imports matching profile
geometry on each populated World Shard and skips the Instance Pool. It does not enable exact rays.
`debug_set_vmap_enabled true` remains an explicit Operator decision after the checks in
[`docs/verification/vmap-rollout.md`](./verification/vmap-rollout.md).

The direct importer can dry-run the bounded Map 36 entry and exit route with
`--vmap <client Data/ dir> --world-profile instances`. This is an archive-derived geometry check.
It reports source identities, floor support, height changes, headroom and collision for each retained
route sample. It does not stage data or enable collision without `--apply`, and `--apply` refuses
unsuitable static geometry. This dry run does not prove ordinary Core movement along the route.

---

## 6. Playing from another machine on your LAN

`dev up` is loopback-only, so a client on a *different* machine cannot reach it. Bind the two
client-facing listeners to one of **this machine's own private addresses** instead:

```bash
./lyracore dev down                       # restart before changing listener addresses
./lyracore dev up --lan 192.168.1.50      # `ip addr` (Linux) / `ifconfig` (macOS) to find yours
```

```text
✓ dev stack is up.
  LAN mode: clients on this network use realmlist 192.168.1.50. SpacetimeDB stays on
  127.0.0.1 — only the logon and world ports are reachable from the LAN.
```

Then on the client machine:

```text
set realmlist "192.168.1.50"
```

Two halves of this matter, and `--lan` does both: it binds logon `3724` and world `8085` to that
address, **and** it makes the realm list advertise it (`LYRACORE_REALM_ADDRESS`). Without the second
half a LAN client would authenticate and then be told to open its world connection to *its own*
loopback — a realm that logs you in and goes nowhere.

Rules `--lan` enforces for you:

- **SpacetimeDB never moves.** It stays on `127.0.0.1:3000` in every mode. `--lan` is for running a
  2004 game client from a second machine, not for reaching the database.
- **Only RFC1918 addresses are accepted** — `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`. A public
  address or `0.0.0.0` is a **usage error, not a wildcard bind**:

  ```text
  lyracore: --lan refuses 0.0.0.0: it is not a private-LAN address. ...
  ```

- Changing listener addresses on a running Native Host requires `dev down` first.

This is a LAN convenience for a contributor fixture, **not a deployment mode**: no TLS, 2004-era
password hashing, and a fixture account whose password is written in this document.
Only put it on a network you trust, and read the next section.

---

## Security posture: local development only

**Never expose ports 3000, 3724, or 8085 to the internet, and never port-forward them.** The reasons
are concrete, not precautionary:

- Keep the HTTP endpoint on port 3000 private. Native clients use the WoW listeners; the host
  submits their requests through in-process Core connections. HTTP remains an administrative endpoint.
- Local startup publishes the Module with `debug_reducers`. Debug operations can change Character
  and world state, so this development build needs the private HTTP boundary.
- **No TLS.** The logon port has the Logon Limiter (three attempts per connection, ten failed
  logons per address per minute, eight open logon connections per address). The stored `game_session`
  key expires one hour after logon and prevents later world handshakes. Existing World Sessions
  continue. The SRP6 exchange itself is 2004-era and unencrypted.

If you ever want to run this for real players, understand that the localhost → public-VPS delta
is real hardening work (a plain build, a firewalled `:3000`, DoS guards, the realm address, launch
order, backups). None of that is what `lyracore dev up` gives you.

---

## 7. Everyday commands

```bash
./lyracore dev status                # what is running, where, and is the database actually published
./lyracore dev logs                  # both components' logs
./lyracore dev logs spacetime        # Native Host output
./lyracore dev down                  # stop what this CLI started
./lyracore dev down && ./lyracore dev up   # restart (rebuild + republish; nothing is wiped)
./lyracore import                    # replace the seed fixture with the real world (see above)
./lyracore config                    # show the remembered client-data path (unset by default)
./lyracore config set client-data PATH   # remember one, so `import` stops asking
./lyracore character gm NAME true    # flip GM commands on for a character (false to revoke)
./lyracore account alpha-test-tools enrollment lyracore-realm   # inspect automatic enrollment
./lyracore update                    # pull the latest LyraCore and restart the stack (see below)
```

`dev status` checks three things that fail independently — the **process identity** (the recorded PID
is still *our* process: start time + command name, not just "a PID exists"), the **endpoint** (it
answers where it was bound), and the **database** (`lyracore` is really published on that node). A
stack whose PIDs and ports are both perfect is still broken if nothing was ever published, and that
is the state the other two checks cannot see.

Runtime files all live in the git-ignored `.lyracore/`:

| Path | What |
| --- | --- |
| `.lyracore/state.json` | the processes this CLI started (PID + identity), and the recorded bind |
| `.lyracore/native-owner-token` | Retained publisher and Operator credential, mode `0600` |
| `.lyracore/native-cli.toml` | Private publisher configuration, mode `0600` |
| `.lyracore/native-data/` | Durable Realm data; keep it across restarts |
| `.lyracore/native-host/SpacetimeDB/` | Pinned host source and build |
| `.lyracore/logs/spacetime.log` | SpacetimeDB node output |
| `.lyracore/cli/<rev>/` | the pinned development CLI |
| `.lyracore/wire-harness/<sha>/` | the pinned wire harness, once `dev smoke` has fetched it |

Exit codes: `0` success (including "already up", "already down", and a `doctor` with only warnings),
`1` operational failure (missing prerequisite, failed subprocess, refused foreign PID), `2` bad
invocation or not inside a checkout.

---

## Updating

### Development reload

For Module code changes, start the local development Realm and watcher:

```bash
./lyracore dev watch
```

The first run builds the Native Host and development WASM. Connect the client, edit Module or
Package Rust source, and save. Later edits rebuild only the WASM and publish it to the running
Host. The command prints build and publish times. It combines rapid saves and rebuilds again if
source changes during a build. Each successful build publishes its artifact; the Host skips
replacement when that artifact is already installed.

If a normal `dev up` Host is already running, stop it with `./lyracore dev down` before starting
watch. This one-time mode switch ends existing World Sessions. Ctrl+C stops the watcher and leaves
the development Host running. `./lyracore dev down` stops that Host and keeps its durable data.

A failed Rust build leaves the current Module running. Fix the source and save again. The
development Host also refuses any change to the Module definition, including private tables and
reducer arguments. Handle those changes through a planned update. A compatible code edit keeps
the same client connection, but it can still change gameplay or contain a bug. This workflow does
not replace `./lyracore preflight` or the tests required for a PR.

For a schema change that SpacetimeDB can migrate automatically, stop watch with Ctrl+C, then run:

```bash
./lyracore dev down
./lyracore dev up
./lyracore dev down
./lyracore dev watch
```

The normal `dev up` publishes through the usual migration checks. If they refuse the change,
resolve the migration before returning to watch. This sequence ends existing World Sessions.
Package changes that alter the Module definition need the same transition.

The build cache lives in `.lyracore/dev-reload-target/`. Keep it between edits. Changing the Native
Host requires a restart; the watcher only publishes the Module.
Development reload builds only the WASM library and optimizes build scripts and procedural macros.
The Host keeps a bounded function cache between edits. After installing a Host change, stop it once
with `./lyracore dev down` and start `./lyracore dev watch` to use that change.

For a value already exposed as Package Config, use `./lyracore packages config NAME KEY VALUE`.
That changes a row through the running Module without compiling or publishing WASM. Rust constants
still require a code edit. Package Deltas and Runtime Scripts use `packages apply`, which can also
publish Rust Packages; it is a separate workflow from watching Rust source.

Each replacement writes the old and new WASM to the local commit log, about 66 MB in total with
the current 33 MB development artifact. The development Host rotates log segments at 256 MiB
and takes a snapshot after rotation. Once two snapshots are complete and their transactions are
durable, it verifies both snapshots and removes older snapshots and unneeded log segments.
Accounts, Characters, items and the installed Module stay in the retained state. Cleanup keeps
enough history to recover from either retained snapshot; it does not reset the Realm.

This is a recovery window, not a fixed disk quota. Large transactions, snapshot work and cleanup
errors can leave more history on disk. The Host logs cleanup failures and keeps the recovery files.
Normal Hosts keep their existing history policy. The build cache is separate, and Wasmtime manages
its compiled-code disk cache separately. Do not delete individual commit-log files from
`.lyracore/native-data/` to reclaim space from a Realm you intend to keep.

Native startup requires OpenSSL 3 or newer. The CLI keeps local signing keys in the private
`.lyracore/native-jwt/` directory and reuses them after a restart. If a key is damaged or the pair
does not match, startup stops without replacing it. An existing certificate authority in the
Host's configuration takes precedence over these fallback keys.

### Checkout updates

```bash
./lyracore update
```

It fetches `origin`, then refuses if your checkout has **local tracked edits** — anything other than
untracked files — naming them and telling you to commit or stash first, because the next step is a
`git reset --hard origin/main` and it will not silently discard work for you. With a clean tree it
resets to `origin/main` and prints the old and new commit, and — if `.lyracore-cli-rev` moved — a note
that the next `lyracore` invocation installs the new pinned CLI automatically. It does **not** restart
anything itself; run the restart it tells you to:

```bash
./lyracore dev down && ./lyracore dev up
```

That rebuilds the Native Host and republishes the Realm's Module. It does not delete Realm data.

The manual equivalent, if you would rather not use the CLI for this:

```bash
git pull
./lyracore dev down && ./lyracore dev up
```

Either way, the `lyracore` **binary** itself is a property of your checkout, not something `update`
fetches directly: `./lyracore` reads `.lyracore-cli-rev`, and if that pin changed, installs the new
revision into `.lyracore/cli/<rev>/` on its very next invocation — the same way it does on a first
clone ([§2](#2-clone-and-check)).

---

## 8. Troubleshooting

### `lyracore: not inside a LyraCore checkout`

```text
lyracore: not inside a LyraCore checkout (no .lyracore-cli-rev here or in any parent of /home/you)
```

Exit code `2`, from the launcher in `$HOME/.local/bin`: you are outside every checkout. `cd` into
one (its root is the directory holding `.lyracore-cli-rev`) and run the command again. This is
deliberate — bare `lyracore` never picks a checkout for you, because which CLI revision runs is a
property of the checkout you are standing in.

### `command not found: lyracore`

The installer appended the `PATH` line to a profile your shell has not read yet, or you declined it.
Either start a new shell, or run `export PATH="$HOME/.local/bin:$PATH"` — or just use `./lyracore`
from the checkout root, which needs nothing on `PATH`.

### `doctor` says a version is wrong

```text
  ✗ Rust             found 1.90.0, but this checkout needs 1.93.0 — `rustup update` ...
  ✗ SpacetimeDB      found 2.5.0, but 2.7.1 is required — upgrade with `spacetime version upgrade` ...
```

- **Rust:** the pinned toolchain in `rust-toolchain.toml` is installed automatically by any cargo
  command run inside the checkout — so this usually means your `rustc` is **not managed by rustup**
  (a distro package, or a Homebrew `rust`). Install rustup and make sure its shims come first on
  `PATH`.
- **SpacetimeDB:** `doctor` accepts 2.7.1 *or newer*, but the repo is pinned to **exactly 2.7.1** and
  `./lyracore preflight` fails on anything else (`spacetime CLI reports 2.5.0, this repo is pinned to
  2.7.1`). Versions install side by side, so this is not destructive:

  ```bash
  spacetime version install 2.7.1
  spacetime version use 2.7.1
  ```

- **WASM target** (a warning): `rustup target add wasm32-unknown-unknown`.

### `doctor` warns that ports are in use

```text
  ⚠ ports            3000, 3724, 8085 already in use — `lyracore dev status` will say whether that is your own stack
```

Run `./lyracore dev status` to inspect the recorded process and listeners. Startup refuses an
unrelated process on ports 3000, 3724 or 8085. Use `ss -ltnp` on Linux or `lsof -nP -iTCP -sTCP:LISTEN`
on macOS to identify the process before stopping it.

### `dev up` fails while starting the Native Host

Read `.lyracore/logs/spacetime.log` with `./lyracore dev logs`. A missing build toolchain, an address
already in use or a Module publish failure can stop startup. The CLI records the host before
publishing, so `dev down` can stop it after a later failure.

If the world listener does not return an authentication challenge, check the log and the retained
publisher credential. The Module's Operator must be the identity that published it.

### `dev status` says the Shard is unreachable

The HTTP listener can be running before the Module has been published. Rerun `dev up` to complete
startup. It retains the host and does not clear data.

### The client says "Unable to connect"

In order of likelihood: wrong `realmlist.wtf` (it must be the one the client actually reads — check
both the client root and `Data/<locale>/`); the stack is not up (`./lyracore dev status`); port 3724
is firewalled between client and server; or the client is not build 5875.

### The client is not build 5875

Symptoms are protocol-shaped, not friendly: the client rejects the logon challenge, or disconnects
partway through the handshake. There is no version-check error message, because the whole logon tier
speaks 1.12.1/5875's `version_3` message set and nothing else — another build's messages simply do
not decode. The client's login screen shows its build number; it must read **5875**. Other 1.12.x
builds and modified/launcher-patched clients are not supported.

### The realm list appears, but selecting the realm hangs or drops

This is the **advertised-address** mismatch. The realm list hands the client a *world* address; if
that address is `127.0.0.1` and the client is on another machine, it dials its own loopback and gets
nowhere. Fix: start the stack with `./lyracore dev up --lan <this machine's private IP>` (which sets
the advertised address as well as the bind), and make sure port **8085** is open between the two
machines. A `dev up` without `--lan` is loopback-only by design.

### Login rejected / wrong password

Provision the account again — `./lyracore account create <NAME>` rotates the credentials of an
existing account rather than failing. Remember the name is uppercased, and the password is capped at
16 bytes. If you never ran `account create`, the seeded `TEST` account has no usable password: its
verifier is a placeholder until you provision one.

Account creation computes an SRP salt and verifier in the CLI and sends them directly to the
Module. It requires a running Realm and its retained Operator credential.

### `account create` says there is no coordinator credential

Run `./lyracore dev up` first. Account creation checks for the credential before reading a password.
It does not mint a replacement identity. For native Realms, the retained credential is
`.lyracore/native-owner-token`.

### `dev up` says `operator already claimed`

The retained credential does not match the Shard's Operator. Restore the credential that published
and claimed this Realm. Keep `.lyracore/native-owner-token` and `.lyracore/native-cli.toml` with the
Realm's data; deleting them does not reset its ownership.

### `dev down` refuses to stop something

```text
lyracore: refusing to stop spacetime: PID 12346 is no longer the process this CLI started
(recorded "…", found "…"). Clear it with `lyracore dev down --forget`.
```

PIDs get reused, and signalling a recycled one kills a stranger's process — so every recorded PID is
stored with its start time and command name, and `dev down` compares that identity before signalling
anything. `dev down --forget` drops the record **without** signalling.

### Restart after a failure

```bash
./lyracore dev down
./lyracore doctor
./lyracore dev up
```

If `dev down` reports a reused PID, `dev down --forget` can clear that stale process record without
signalling it. Keep `.lyracore/native-data` and the native credential files. They hold the Realm's
data and ownership, and are not build caches.

---

## Where to go next

- The [CLI command reference](https://github.com/LyraCoreProject/lyracore-cli/blob/main/docs/commands.md) in the `lyracore-cli` repository: every command and its safety
  rules.
- [GitHub Issues](https://github.com/LyraCoreProject/LyraCore/issues) — the work queue. Run the
  checks in [`docs/testing.md`](./testing.md) before proposing a change.
- [`docs/danger-zones.md`](./danger-zones.md) records deployment and migration requirements.
  Local native startup does not update an existing deployment or consolidate its Shards.
- [`docs/architecture.md`](./architecture.md#native-host-migration) explains the Module and
  Native Host responsibilities and current verification coverage.

If you are here to contribute rather than to play:

- [`CONTRIBUTING.md`](../CONTRIBUTING.md): where to start, what gets in, and how a pull request
  is judged.
- [`SUPPORT.md`](../SUPPORT.md) — where to ask a question, and what is in and out of scope.
- **Security reports** go through GitHub's private
  [vulnerability reporting](https://github.com/LyraCoreProject/LyraCore/security/advisories/new),
  never a public issue; the accepted weaknesses of a local-development-only fixture are in the
  warning banner at the top of [`README.md`](../README.md).
