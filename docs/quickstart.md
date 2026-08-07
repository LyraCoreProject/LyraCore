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
| **SpacetimeDB CLI** | **exactly 2.7.1** | `module/Cargo.toml` pins `spacetimedb = "=2.7.1"` and `gateway/Cargo.toml` pins the SDK to `=2.7.1`. `./lyracore preflight` enforces an **exact** match and fails on anything else; `./lyracore doctor` reports the version it found. |
| **git**, **curl**, a C toolchain | any current | Cloning, the SpacetimeDB installer, and linking the gateway. |
| **wasm-opt** (`install.sh` offers a user-local install; otherwise Ubuntu/Debian package `binaryen`, macOS `brew install binaryen`) | any current, *optional* | `spacetime publish` runs it to optimise the module WASM. It is **not required** — publish still succeeds without it — but the module is the server's authoritative game logic, running every reducer on every shard, so skipping it means shipping that logic unoptimised, silently, on every future publish too. Without it you'll see `Could not find wasm-opt to optimise the module` mid-publish; `lyracore doctor` reports it as an optional `⚠` rather than a blocking `✗`. |

### Ubuntu 24.04

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libssl-dev git curl
```

`libssl-dev` + `pkg-config` are not optional on Linux: the gateway's SpacetimeDB SDK pulls in
`native-tls`, which builds against system OpenSSL on Linux.

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

No Homebrew packages and no OpenSSL are needed — on Apple platforms `native-tls` uses
Security.framework, not OpenSSL.

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

## 3. Start the stack

```bash
./lyracore dev up
```

It prints its progress as it goes:

```text
· starting SpacetimeDB on 127.0.0.1:3000...
· building the gateway...
· publishing lyracore, lyracore-elwynn, lyracore-kalimdor, lyracore-realm...
· no SpacetimeDB login found — minting a local identity from http://127.0.0.1:3000 (no spacetimedb.com account needed)...
· claiming the operator identity...
· starting the gateway on 127.0.0.1:8085...
✓ dev stack is up.
  spacetime  healthy   (PID 12345, 127.0.0.1:3000)
  gateway    healthy   (PID 12346, 127.0.0.1:8085)
  databases  lyracore, lyracore-elwynn, lyracore-kalimdor, lyracore-realm published on http://127.0.0.1:3000
```

(Illustrative — the exact wording is the CLI's, and it lives in
[its own repository](https://github.com/LyraCoreProject/lyracore-cli).)

What that actually did:

1. Started SpacetimeDB on `127.0.0.1:3000` — **or reused one already listening there.** A node this
   CLI did not start is never recorded and never stopped by `dev down`.
2. Built the gateway (`cargo build -p lyracore-gateway`).
3. Published the **four fixture databases** — `lyracore`, `lyracore-elwynn`, `lyracore-kalimdor`
   and `lyracore-realm` — always through `./lyracore publish`. Nothing in this path runs a bare
   `spacetime publish`, and the command **refuses** to forward the destructive `-c` clear-publish.
   (`./lyracore dev up --single` publishes and runs `lyracore` alone; see below.)
4. Got itself a **coordinator identity** — see below. The `minting` line appears only on a host with
   no SpacetimeDB login; if you have one, it is reused and that line is absent.
5. Called `claim_operator` as that identity (idempotent — repeating `dev up` is not an error).
6. Started the gateway, with the same identity, and with its logon (`3724`) and world (`8085`)
   listeners bound to loopback.

Three ports are involved, and only these three: **3000** SpacetimeDB, **3724** logon, **8085** world.

Re-running `dev up` on a healthy stack does nothing (`dev stack already up — nothing to do.`); on a
half-up stack it starts only the missing part. It never clears a database, so **your characters
survive** an up/down cycle.

The realm it brings up is **playable with no client-data import**: the module's `init` reducer seeds
the realm row, the Human-Warrior start position in Elwynn Forest, the graveyards, a `TEST` account
with a pre-made character (`Tester`, Human Warrior, level 1), and a small demo population.

### Your local realm has a live seam — go and cross it

This is the part worth doing before anything else. Your realm is **sharded**: Northshire Valley —
where your character starts and where all the seeded content is — is on `lyracore`, the rest of
Elwynn is on a second database (`lyracore-elwynn`), and map 1 (Kalimdor) is on a third. Those are
separate SpacetimeDB databases with separate writers, not zones in one process.

**Walk the road out of Northshire Valley, toward Goldshire.** Somewhere past the valley's mouth you
cross the seam, and two System chat lines appear:

```text
Crossing the seam from lyracore into lyracore-elwynn...
You are now on lyracore-elwynn. Seam crossed in 84 ms — no loading screen.
```

There is no loading screen, no character reload, and no reconnect — your session is handed over to
the other database mid-walk. The messages are there because a correct handoff is *invisible* by
construction, and for the alpha it is worth being told it happened; export `LYRACORE_SEAM_NOTIFY=0`
for silent seams. The Kalimdor database is the *other* seam, but it is topology only — map 1 has no
content at all, so it is a routing demonstration and a `dev status` line rather than a place to go.

⚠ **Expect empty ground on the far side.** Everything the fixture seeds lives in Northshire Valley,
on the near side of the seam; region 2 stays unpopulated until you run `./lyracore import` (which
needs a cmangos world dump and your own client MPQs). Goldshire is not populated out of the box. The
crossing itself is real and announces itself — that is what this walk is for.

The seam's geometry is plain content data you can edit:
[`content/regions/fixture.regions`](../content/regions/fixture.regions), explained in
[`docs/region-sharding.md`](./region-sharding.md). Every coordinate in it is committed in this
repository, so the seam lands where the file says it does — a single line of constant world x
(≈ -9183.3) between the valley and the Goldshire basin.

> **What does not work across a seam yet.** Buffing, healing, melee and trade between two players on
> opposite sides are **not built** (#75/#76), and an AoE clips at the boundary. You *can* see each
> other, move, fight mobs, chat and emote across it. If you and a friend are grouping on the road out
> of the valley and a heal will not land, this is why — stand on the same side.

If you would rather not deal with four databases (debugging something unrelated, or RAM is tight),
`./lyracore dev up --single` brings up `lyracore` alone with no seams at all.

⚠ Because the fixture is sharded, **a schema change means republishing every fixture database**, not
just `lyracore` — `./lyracore publish` covers the set. A partial publish tends to present as an
unrelated mid-session hang rather than a loud error
([`docs/danger-zones.md`](./danger-zones.md) §3).

### The coordinator identity (why there is no account signup)

The gateway's own connection to the database is privileged: `game_account` and `game_session` are
private module tables, and account provisioning is gated on a single **operator** identity that
`dev up` claims on first run. So the stack needs one real credential — and getting it must not
require an account with anybody.

It does not. `dev up` takes the first of these that exists:

1. `.lyracore/coordinator-token` — an identity a previous `dev up` minted for this checkout. It wins,
   because it is the one that already claimed the operator (a *different* identity would be refused).
2. Your `spacetime login`, if you have one. Nothing is copied into the checkout in that case.
3. Otherwise a **server-issued identity minted from your own local node** (`POST /v1/identity`), saved
   at `.lyracore/coordinator-token` with mode `0600` inside the git-ignored `.lyracore/`.

That third rung is what keeps `git clone && ./lyracore dev up` anonymous: `spacetime login` only
offers the spacetimedb.com browser flow, and this fixture never needs it. Treat the file as a
credential — it is the operator of your local realm — and note that deleting it and re-running
`dev up` mints a *new* identity that the database will refuse to accept as operator (see
[Troubleshooting](#dev-up-says-operator-already-claimed)).

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

### Optional: prove it over the wire before touching a client

```bash
./lyracore dev smoke
```

This hands off to the pinned headless wire harness and drives a **real build-5875 login** against the
running fixture: logon → encrypted world handshake → character enumerate → enter world. It signs in
as the fixture account, so provision `TEST` first (`test123` is the conventional fixture password).
It refuses to run against a stack that is not up.

---

## 5. Point your client at it

You need an unmodified **1.12.1, build 5875** client. The exact build matters — the gateway speaks
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
two. It exists so that every layer — spawn, combat, quest, loot, movement — has something to act on
without anybody needing a world database. It is not a zone.

The real thing is the Elwynn/Westfall corridor: roughly 950 creature templates, 640 quests, the
trainer/vendor/loot tables behind them, a ground-height map and a navigation grid. **That data is not
in this repository and never will be.** It is reconstructed on your machine, from two sources that
are yours and not ours to hand out:

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

It prints the notice above in full and waits for you to type `yes`. Then it runs four stages: pull
`classic-db` (checksum-verified against the pinned commit), confirm the client path, run the world
ETL, and import the curated class-spell trainer offerings. Pass `--accept` to answer the consent
question in advance for a scripted run, and leave `--client-data` off to be prompted for the path.

**What to expect.** The pull is a few hundred megabytes of git history. The ETL itself is the long
part — **tens of minutes**, most of it in the terrain and navigation passes, which rasterize the box
cell by cell. It ends by printing an assertion per content family (`ok  live creature entities:
2731`, and so on); a `FAIL` line names the family that came up short, which is the diagnostic you
want. The stack must be up (`./lyracore dev up`) — the ETL writes through the running node.

**It is optional, and it is one-way-ish.** Everything in §§1–5 works without it, and the wire smoke
tests run against the fixture. The import clears and reloads whole content families, so running it
points that database at the imported world instead of the fixture; publishing to a fresh database
gives you the fixture back.

**The line we do not cross:** we ship the tools, you fetch the data, and no database produced by
this pipeline is ever distributed or hosted by this project. What you do with the one on your disk
is your call and your responsibility. [`docs/data-ingestion.md`](./data-ingestion.md) is the full
statement, and `importer/scripts/` holds the scripts `lyracore import` drives if you would rather
run a stage at a time.

---

## 6. Playing from another machine on your LAN

`dev up` is loopback-only, so a client on a *different* machine cannot reach it. Bind the two
client-facing listeners to one of **this machine's own private addresses** instead:

```bash
./lyracore dev down                       # a running gateway cannot be rebound
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

- A **running** gateway is never silently rebound — switching modes is refused with the `dev down`
  to run first, rather than reported as "already up".

This is a LAN convenience for a contributor fixture, **not a deployment mode**: no rate limiting, no
TLS, 2004-era password hashing, and a fixture account whose password is written in this document.
Only put it on a network you trust, and read the next section.

---

## Security posture: local development only

**Never expose ports 3000, 3724, or 8085 to the internet, and never port-forward them.** The reasons
are concrete, not precautionary:

- **`:3000` is the real trust boundary.** The gateway's per-player connections to SpacetimeDB are
  **anonymous and tokenless**: the node mints a fresh identity for each. Anyone who can reach `:3000`
  can mint their own identity the same way and call **any reducer that is not operator-gated** —
  from exactly the footing a player has. The whole model's safety rests on `:3000` being unreachable.
- **The fixture publishes the debug build.** `dev up` goes through `./lyracore publish`, which
  bakes in `--features=debug_reducers` — that is **124 extra reducers**, most with no
  identity gate at all (`debug_set_health(any_guid, 0)`, `set_level`, `kill_nearest`, `teleport`, …).
  One anonymous call owns any character. This build is for the loopback harness; it must never reach
  a reachable node.
- **No rate limiting, no TLS, no brute-force lockout.** SRP6 proof attempts are unlimited, and the
  logon tier has no per-IP cap. That is acceptable on loopback and nowhere else.

If you ever want to run this for real players, understand that the localhost → public-VPS delta
is real hardening work (a plain build, a firewalled `:3000`, DoS guards, the realm address, launch
order, backups). None of that is what `lyracore dev up` gives you.

---

## 7. Everyday commands

```bash
./lyracore dev status                # what is running, where, and is the database actually published
./lyracore dev logs                  # both components' logs
./lyracore dev logs gateway          # just one (spacetime | gateway)
./lyracore dev down                  # stop what this CLI started
./lyracore dev down && ./lyracore dev up   # restart (rebuild + republish; nothing is wiped)
./lyracore import                    # replace the seed fixture with the real world (see above)
./lyracore config                    # show the remembered client-data path (unset by default)
./lyracore config set client-data PATH   # remember one, so `import` stops asking
./lyracore character gm NAME true    # flip GM commands on for a character (false to revoke)
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
| `.lyracore/coordinator-token` | **a credential** (mode `0600`) — the identity `dev up` minted for this checkout, if this host had no `spacetime login`. Never committed, never logged; keep it, or the database stops accepting a new operator |
| `.lyracore/logs/spacetime.log` | SpacetimeDB node output |
| `.lyracore/logs/gateway.log` | gateway output |
| `.lyracore/cli/<rev>/` | the pinned development CLI |
| `.lyracore/wire-harness/<sha>/` | the pinned wire harness, once `dev smoke` has fetched it |

Exit codes: `0` success (including "already up", "already down", and a `doctor` with only warnings),
`1` operational failure (missing prerequisite, failed subprocess, refused foreign PID), `2` bad
invocation or not inside a checkout.

---

## Updating

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

That rebuilds the gateway and republishes every fixture database against whatever schema the update
brought — nothing is wiped.

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

Run `./lyracore dev status`. It distinguishes the two cases precisely:

- `healthy (PID …, 127.0.0.1:8085)` — your own stack. Nothing to do.
- `external (127.0.0.1:3000 answers; not started by this CLI)` — **someone else's** process on that
  port. A foreign **SpacetimeDB** node is reused (and deliberately left running by `dev down`); a
  foreign **gateway** is *not* adopted, because we cannot know its build or topology:

  ```text
  lyracore: process error: port 8085 is already served by a gateway this CLI did not start —
  stop it first, or run `lyracore dev down --forget` if it is stale state from an earlier run
  ```

  Find the offender with `ss -ltnp | grep -E '3000|3724|8085'` (Linux) or
  `lsof -nP -iTCP:3000,3724,8085 -sTCP:LISTEN` (macOS) and stop it. There is no flag to move these
  ports.

### `dev up` fails while starting a component

Either component can fail to come up, and the message always names both the component and its log:

```text
lyracore: process error: spacetime exited immediately (PID 12345); see .lyracore/logs/spacetime.log
lyracore: process error: gateway exited during startup; see .lyracore/logs/gateway.log
lyracore: process error: gateway did not answer on 127.0.0.1:8085 within 60s; see .lyracore/logs/gateway.log
```

Read the named log — `./lyracore dev logs spacetime` or `./lyracore dev logs gateway`.

**SpacetimeDB won't start:** the usual causes are a `spacetime` that is not really on `PATH` for a
non-login shell, a half-installed version (`spacetime version list` should show 2.7.1), or something
already bound to `:3000` that is not a SpacetimeDB node at all (`dev up` reuses a node it finds
there, so a *different* service on that port produces a confusing downstream failure rather than a
clean one).

**The gateway won't start:** two lines worth recognising in `gateway.log` —

- `coordinator subscriptions not applied within 15s (node down, or token lacks access to the private
  game_account/game_session tables?)` — **the node went away, or the gateway is not the identity that
  claimed the operator.** `dev up` gives the gateway a credential or refuses to start it, so a
  *missing* one no longer produces this. What still does: SpacetimeDB died after `dev up` finished
  (`./lyracore dev status`, then `./lyracore dev logs spacetime`); the database line says
  `UNREACHABLE`, i.e. nothing was published where the gateway is looking (see the next entry); or a
  gateway you launched by hand with a `LYRACORE_COORDINATOR_TOKEN` from some *other* identity than
  the one that claimed this database's operator.
- an address-in-use error — something grabbed 3724/8085 between the port check and the bind.

### `dev status` says the database is UNREACHABLE

```text
  database   lyracore UNREACHABLE on http://127.0.0.1:3000 (... failed to find database `lyracore`.)
             — run `lyracore dev up` to publish it
```

Both processes can be perfectly healthy in this state — it means nothing was ever published to the
node they are pointed at (a common shape: a SpacetimeDB node that was already running for some other
project got reused). `./lyracore dev up` publishes it.

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

`./lyracore account create` needs a built gateway binary; on a fresh checkout run `dev up` first, or
you will get:

```text
lyracore: prerequisite missing: target/debug/lyracore-gateway is not built — run `lyracore dev up` first
```

### `account create` says there is no coordinator credential

```text
lyracore: prerequisite missing: no coordinator credential — /home/you/lyracore/.lyracore/coordinator-token
does not exist and `spacetime login show --token` printed no token. Run `lyracore dev up` first: it
mints a server-issued identity from the local node and claims it as the operator (no spacetimedb.com
account needed).
```

(One line in the terminal; wrapped here.) Rare, because `dev up` is what creates that file — you see
it if you run `account create` on a checkout where `dev up` has never completed, or if you deleted
`.lyracore/` by hand.

Provisioning writes through the **operator** identity, and `account create` deliberately will not
mint a new one: a fresh identity has claimed nothing, so the module would refuse the write *after*
your password had been read. `./lyracore dev up` mints and claims in one step; then re-run
`account create`. Nothing here needs a spacetimedb.com account.

### `dev up` says `operator already claimed`

```text
lyracore: process error: http://127.0.0.1:3000/v1/database/lyracore/call/claim_operator answered HTTP 400: operator already claimed
  `claim_operator` was called with the server-issued identity in /home/you/lyracore/.lyracore/coordinator-token. A refusal there almost always means this database was claimed by a DIFFERENT identity (an earlier `spacetime login`, or another checkout): delete that file and re-run `lyracore dev up` to fall back to that login.
```

(Two lines, both long; the wording after `HTTP 400:` is the node's, so it may differ slightly with
the SpacetimeDB version — the second line is the CLI's and is exact.)

The operator is trust-on-first-use: the first identity to claim it owns the database, and a second
one is refused for as long as that database exists. So the two identities have to be the same one
across runs, which is exactly why the minted token is persisted.

- **You logged in to SpacetimeDB after the first `dev up`, or published this database from another
  checkout.** Delete `.lyracore/coordinator-token` and re-run `dev up`: it falls back to your login,
  which is the identity that already claimed the operator.
- **You deleted `.lyracore/coordinator-token` and the database was claimed by the identity it held.**
  That credential was the only copy — restore it from a backup if you have one. Otherwise the fixture
  database has to be recreated from scratch; it holds nothing you cannot rebuild with `dev up` +
  `account create`, but do not reach for `spacetime publish -c` (see [Clean retry](#clean-retry)).

### `dev down` refuses to stop something

```text
lyracore: refusing to stop gateway: PID 12346 is no longer the process this CLI started
(recorded "…", found "…"). Clear it with `lyracore dev down --forget`.
```

PIDs get reused, and signalling a recycled one kills a stranger's process — so every recorded PID is
stored with its start time and command name, and `dev down` compares that identity before signalling
anything. `dev down --forget` drops the record **without** signalling.

### Clean retry

```bash
./lyracore dev down --forget     # drop the recorded processes, signalling nothing stale
./lyracore doctor                # confirm the ports are actually free now
./lyracore dev up
```

That is the whole reset: `.lyracore/` holds only state, logs and caches, and `dev up` rebuilds and
republishes. **Never** run `spacetime publish -c` to "start clean" — that is the destructive wipe;
it destroys the data *and* the claimed operator identity, and login breaks until you re-claim it.
`./lyracore publish` refuses to forward it for exactly this reason.

### If you hand-manage the gateway process

You normally never need to — but if you do, the kill pattern is
**`pkill -x lyracore-gatewa`**. The missing trailing `y` is deliberate: `pkill -x` matches
`/proc/<pid>/comm`, which the kernel truncates to 15 characters, and the binary name is 16. The full
name matches nothing and still exits 1 — indistinguishable from "already stopped". And never
`pkill -f`, which self-matches the launching shell. Prefer `./lyracore dev down`.

---

## Where to go next

- [`docs/development-cli.md`](./development-cli.md) — the CLI's full command and safety contract.
- The **Contributing** section of [`README.md`](../README.md#contributing) — the offline checks
  (`cargo test` per crate) to run before proposing a change, and where the work queue lives.
- [`docs/danger-zones.md`](./danger-zones.md) — authoritative traps, tooling gotchas, and the
  production (five-database) deploy procedure. `dev up` runs its own smaller four-database fixture
  topology, which is the deliberate exception to §3; do not use it to launch or repair a production
  realm.
- [`docs/region-sharding.md`](./region-sharding.md) — the seam you just crossed: the content-data
  format, the shipped fixture menu, and how routing uses it.
- [`docs/architecture.md`](./architecture.md) and [`docs/schema.md`](./schema.md) — how the module
  and gateway are put together, if you are here to write game logic.

If you are here to contribute rather than to play:

- The **Contributing** section of [`README.md`](../README.md#contributing) — the offline checks
  that must be green, and where to start (`good first issue` / `help wanted` labels).
- [`SUPPORT.md`](../SUPPORT.md) — where to ask a question, and what is in and out of scope.
- **Security reports** go through GitHub's private
  [vulnerability reporting](https://github.com/LyraCoreProject/LyraCore/security/advisories/new),
  never a public issue; the accepted weaknesses of a local-development-only fixture are in the
  warning banner at the top of [`README.md`](../README.md).
