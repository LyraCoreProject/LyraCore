<p align="center"><img src="./lyracore-icon.svg" alt="LyraCore" width="140"></p>

# LyraCore

An independent server implementing the 1.12.1 (build 5875) game-client network
protocol, where **all game state and all game logic live inside SpacetimeDB**, and a thin,
stateless **gateway** tier translates that protocol to and from it. Not affiliated with,
endorsed by, or sponsored by Blizzard Entertainment, Inc. *World of Warcraft* is a trademark
of Blizzard Entertainment, Inc., used here only to identify the protocol this software
interoperates with. No game client, game assets, or game data are distributed with this
project — you must supply your own legally obtained 1.12.1 client.

This is **not another emulator**. Mature C++ cores (TrinityCore, cMaNGOS, vMaNGOS,
mangoszero) already serve 1.12 well. The goal here is **developer experience and
openness**: contributors write game logic as **reducers** against **clean relational
tables**, iteration is publish-and-hot-reload, and the protocol's quirks are sealed inside
one tier. Extension packages are **trusted module code**: they compile into the module's
WebAssembly binary and run with the module's full privileges. Installing one is
equivalent to accepting a patch to the server — review it as such. The WASM sandbox
constrains the *host* surface (no filesystem, no ambient clock, no global RNG), which is a
determinism and blast-radius property, **not** a safety guarantee for untrusted code. Safe
untrusted modding is not built. The package mechanism itself ships in `module/`; the
`packages/` directory, its contract, and a worked example are maintained outside this public
tree today and are not yet part of what a clone of this repository gives you.

> ### ⚠ Local development only — this is not a hostable server
>
> Everything here binds to `127.0.0.1`, or opt-in to one private-LAN address. There is **no
> supported public deployment** in this alpha, and the maintainers' own production-readiness
> verdict for a public VPS is **NO-GO**: the SpacetimeDB node's `:3000` port is the real trust
> boundary and nothing in the repo enforces that it stays closed, the documented deploy path
> publishes a debug build with most of its 124 debug-only reducers carrying no identity gate at
> all, and there is no rate limiting, no TLS, no brute-force lockout, and no anti-cheat. **Never
> expose ports 3000, 3724, or 8085 to the internet.** To report a security problem, use GitHub's
> private vulnerability reporting on this repository — never a public issue.

## Status

**Alpha.** The supported promise is exactly one thing, and this repository is built to keep it:

> Clone anonymously on Linux or macOS, start a safe local seeded realm, provision an account, and
> connect your own stock 1.12.1 build-5875 client — with no client-data import and no third-party
> world database.

What that realm is today: five SpacetimeDB databases (default world shard, two further world
shards, an instance pool, and a realm-core holding accounts, sessions, groups, whispers and loot
rolls) behind one stateless gateway tier, all running the **same** wasm. 151 tables and 240 reducers
in the core module — 116 of them in a default build, the rest gated behind the `debug_reducers`
feature the local dev harness enables (extension packages add more when installed) — and every state
transition — combat, movement, quests, spells and auras, loot, items and vendors, professions,
instances, and cross-shard character transfer — is a reducer inside SpacetimeDB. The gateway holds no
durable game state. `./lyracore dev up` brings **four fixture databases** up on loopback by default —
a sharded Elwynn/Kalimdor demo plus realm-core, with a live seam you walk across mid-session — and
`./lyracore dev up --single` collapses that to one database with no seams. The multi-database topology
is the production shape and is described in [`docs/architecture.md`](./docs/architecture.md).

What it is not: production-ready, hostable, Windows-supported, or content-complete. The seeded
fixture is a starting zone with a demo population, not a finished 1–60 world. Known limitations are
tracked in the release notes on this repository's GitHub releases.

## Quickstart

**[`docs/quickstart.md`](./docs/quickstart.md) is the clone-to-connected-client path** — Linux and
macOS prerequisites, the fixture, `realmlist.wtf`, private-LAN play, and troubleshooting. From a cold
machine, one line does the clone and the prerequisite check:

```bash
curl -sSfL https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh | sh
```

It clones into `./LyraCore`, checks what §1 of the quickstart documents, and installs the `lyracore`
launcher into `$HOME/.local/bin`. It **never runs `sudo`** — a missing system package prints the
exact `apt-get` line for you to run yourself — and it refuses to touch an existing `./LyraCore`. To
read it before you pipe it into a shell (you should):

```bash
curl -sSfLO https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh
less install.sh && sh install.sh          # --yes to skip the prompts
```

Then, from the checkout or any directory inside it:

```bash
cd LyraCore
lyracore doctor        # check prerequisites
lyracore dev up        # start SpacetimeDB, publish, claim operator, start the gateway
lyracore account create TEST
lyracore dev smoke     # log in over the real 5875 wire protocol and enter the world
lyracore import        # OPTIONAL: build the real world from cmangos' classic-db + your own client
```

`lyracore` is a launcher that resolves the checkout you are standing in and runs **that** checkout's
pinned CLI, so any number of clones coexist. Skipped the installer, or declined the `PATH` change?
Then `./lyracore doctor` from the checkout root does exactly the same thing — that form works
everywhere in this documentation, and it is what CI uses.

Then point your own 1.12.1 build-5875 client's `realmlist.wtf` at `127.0.0.1` and play. The realm is
seeded and playable **without a client-data import**. `./lyracore dev up --lan 192.168.1.50`
additionally serves clients elsewhere on your network — the logon and world listeners only;
SpacetimeDB stays on loopback in every mode.

This is a **local development fixture**: loopback by default, no rate limiting, no TLS, and the debug
module build. Never expose ports 3000/3724/8085 to the internet — the operative rule is above:
loopback and private-LAN only.

The CLI itself lives in [`LyraCoreProject/lyracore-cli`](https://github.com/LyraCoreProject/lyracore-cli);
the shim installs and runs the commit pinned in `.lyracore-cli-rev`. See
[`docs/development-cli.md`](./docs/development-cli.md) for the full command and safety contract.

## Two worlds: the seeded fixture and your own data

These are distinct, and which one you are running changes what you should expect:

- **The seeded fixture** — written by the module's `init` reducer (`module/src/seed.rs`) and shipped
  in this repository: the realm row, the Human-Warrior start position in Elwynn Forest, graveyards, a
  `TEST` account with a level-1 character, and a small demo population. **No import, no imported
  world data, no MPQ/DBC extraction, no third-party world database.** A handful of its rows use real
  vanilla ids, names, and coordinates, hand-typed rather than bulk-imported — see the carve-out in
  [`docs/data-ingestion.md`](./docs/data-ingestion.md#what-the-seeded-fixture-knowingly-contains).
  This is what `./lyracore dev up` gives you, and what the alpha supports.
- **Imported content** — the full Elwynn/Westfall corridor (~950 creature templates, ~640 quests,
  terrain and navigation), built on your machine by `./lyracore import` from data **you** supply:
  cmangos' `classic-db` dump, pulled from cmangos' own public repository, plus the DBCs in your own
  legally obtained 1.12.1 client. **We ship the tools, you fetch the data, and no database produced
  by that pipeline is ever distributed or hosted by this project.** The command states this and
  requires an explicit `yes` before it fetches or reads anything. Licensing firewall and reasoning:
  [`docs/data-ingestion.md`](./docs/data-ingestion.md); the walkthrough is
  [`docs/quickstart.md`](./docs/quickstart.md#getting-the-real-world).

## Documentation

[`docs/architecture.md`](./docs/architecture.md) is the technical entrypoint, and its **§9 is the
full document index** — every other document in `docs/`, grouped, with a line on what it is. The
handful you are most likely to want first:

| Document | Purpose |
| --- | --- |
| [`docs/quickstart.md`](./docs/quickstart.md) | **Clone → running realm → connected client.** Prerequisites, `./lyracore dev up`, account creation, `realmlist.wtf`, LAN play, troubleshooting. |
| [`docs/architecture.md`](./docs/architecture.md) | The current system: tiers, the five-database topology, the data model, the read plane, sharding, packages — and the document index. |
| [`docs/danger-zones.md`](./docs/danger-zones.md) | **Authoritative** over every other document: traps, tooling gotchas, and the exact deploy/verify procedure. Read before any engine change. |
| [`docs/development-cli.md`](./docs/development-cli.md) | The `./lyracore` command surface and safety contract. |

**The work queue is [GitHub Issues](https://github.com/LyraCoreProject/LyraCore/issues).**

## Contributing

Work is tracked as [GitHub Issues](https://github.com/LyraCoreProject/LyraCore/issues); questions go
to the repository's [Discussions](https://github.com/LyraCoreProject/LyraCore/discussions). Get a
stack running with [`docs/quickstart.md`](./docs/quickstart.md), and run the offline checks
(`cargo test` per crate) before proposing a change. Community conduct follows the Contributor
Covenant 2.1.

Good places to start are the [`good first issue`](https://github.com/LyraCoreProject/LyraCore/labels/good%20first%20issue)
and [`help wanted`](https://github.com/LyraCoreProject/LyraCore/labels/help%20wanted) labels.

Security problems do **not** go in a public issue — use GitHub's private vulnerability reporting on
this repository. The warning banner above already names the accepted weaknesses of a
local-development-only posture.

## Architecture at a glance

```
            unmodified 1.12.1 clients (build 5875)
                      │  raw TCP · SRP6 · header-encrypted opcodes
                      ▼
   ┌────────────────────────────────────────┐
   │  GATEWAY  (edge / protocol tier)       │  TRUSTED · does ALL socket IO
   │  SRP6 logon :3724 + realm list         │  holds NO durable game state
   │  world :8085 · header cipher · codec   │  routes across databases
   │  stdb/ — subscriptions · AOI · relays  │  stateless, restartable
   └────────────────────────────────────────┘
                      │  SpacetimeDB client:  reducer calls ↑   subscription deltas ↓
                      ▼
   ┌────────────────────────────────────────┐
   │  SPACETIMEDB  (authority)              │  ALL state + ALL logic
   │  world shards · instance pool          │  transactional reducers
   │  realm-core: accounts · sessions ·     │  same wasm on every database
   │    groups · whispers · loot rolls      │  scheduled work drives the world
   └────────────────────────────────────────┘
```

A client never speaks SpacetimeDB: it speaks SRP6 + TCP + WoW opcodes, so the gateway is the only
legitimate SpacetimeDB client and every mutation the world can undergo is a reducer call. A shard is
a database *name* — a gateway routing fact that module game logic never reads, enforced by a
source-scan test that fails the build if it starts to.
[`docs/architecture.md`](./docs/architecture.md) has the real diagram and the exceptions.

## Chosen stack

- **Language:** Rust for both the SpacetimeDB module and the gateway.
- **Codec:** `wow_messages` / `wow_world_messages` (gtker) — generated 1.12 packet codecs.
- **Auth:** `wow_srp` (gtker) — SRP6 + the vanilla header cipher. Do not hand-roll.
- **Identity:** one SpacetimeDB connection per logged-in player, authorized via `ctx.sender`.
- **Toolchain:** Rust 1.93.0 (pinned in `rust-toolchain.toml`), SpacetimeDB 2.7.1 exactly.

## Workspace layout

```
LyraCore/
├── install.sh            # the one-line installer: clone + prereq check + `lyracore` launcher
├── lyracore              # POSIX shim → the pinned development CLI (.lyracore-cli-rev)
├── Cargo.toml            # workspace (all crates `publish = false`)
├── crates/lyracore-shared/  # protocol-agnostic constants/helpers, consumed by both tiers
├── module/               # crate `lyracore-module` (compiles to wasm) — ALL state + logic
│   └── src/              # one crate organized by domain: auth, character, world, combat/,
│                         #   creatures/, items/, spell/, quest, loot, group, instance,
│                         #   region, transfer, realm_core, seed/, debug (feature-gated)
├── gateway/              # crate + binary `lyracore-gateway` — ALL socket IO, NO game state
│   └── src/              # logon/ (SRP6 + realm list), world/ (handshake, cipher, session),
│                         #   codec/ (wow_messages + the UpdateMask encoder), stdb/ (subs, AOI)
├── importer/             # crate + binary `lyracore-importer` — the BYO-data ETL
└── docs/
```

The headless wire test client is **not** in this workspace: it is server-agnostic and lives in
[`LyraCoreProject/wire-harness`](https://github.com/LyraCoreProject/wire-harness).

## Provenance and prior art

LyraCore is an independent implementation. It is **not** a fork or a port of any existing emulator,
and no code from a GPL-licensed core is present: the one function that had been transcribed from
cMaNGOS was rewritten from a written behavioural specification and proven equivalent over a dense
input grid, and every remaining reference to the mangos family was classified and, where its wording
implied derivation, reworded.

What the project does use, deliberately and openly, is *knowledge* rather than code:

- **wowdev.wiki** for the protocol's wire formats.
- The **gtker** crates (`wow_srp`, `wow_login_messages`, `wow_world_messages`) for the SRP6
  implementation and the generated 1.12 packet codecs — real dependencies, under their own licenses.
- **vMaNGOS / cMaNGOS-classic / mangoszero** as *behavioural* references: which packets a 1.12.1
  client needs, in what order, and what the game's observable mechanics are. Mechanics and formulas
  are facts about a 2004 game, and citing them is not derivation. Where a source comment names one of
  these projects, it is citing behaviour or an imported data column, not code.

World content is never redistributed here. The importer reads DBCs from a client **you** own and a
world-database dump **you** source; see [`docs/data-ingestion.md`](./docs/data-ingestion.md) for the
licensing firewall this maintains.

## License

This project's own source is dual-licensed under your choice of the
[MIT License](./LICENSE-MIT) or the [Apache License, Version 2.0](./LICENSE-APACHE).

**Running it also requires accepting SpacetimeDB's license.** Seven of the `spacetimedb-*`
crates this workspace depends on (`spacetimedb-sdk`, `-schema`, `-sql-parser`,
`-client-api-messages`, `-data-structures`, `-metrics`, `-memory-usage`) are licensed by
Clockwork Laboratories, Inc. under the **Business Source License 1.1**, not MIT/Apache — this
is separate from, and does not affect, the license on our own code. The BSL's Additional Use
Grant permits production use with **no more than one SpacetimeDB instance**, and excludes
offering the software as a "Database Service" to third parties. Its Change Date is
**2031-07-26** (as of SpacetimeDB 2.7.1), after which it converts to **AGPL-3.0 with a
linking exception**. This project stays inside that grant: the five-database realm runs on a single
SpacetimeDB instance.

*World of Warcraft* is a trademark of Blizzard Entertainment, Inc. This project is not
affiliated with, endorsed by, or sponsored by Blizzard Entertainment, Inc.; no game client,
game assets, or game data are distributed with it.
