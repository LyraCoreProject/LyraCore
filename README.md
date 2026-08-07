<p align="center"><img src="./lyracore-icon.svg" alt="LyraCore" width="140"></p>

# LyraCore

**Run a vanilla World of Warcraft server on your own machine, and write game logic as
database transactions.**

LyraCore speaks the 1.12.1 (build 5875) client protocol, so your own stock client connects to
it unmodified. What makes it different from the mature C++ cores is where the game lives: **all
state and all logic sit inside [SpacetimeDB](https://spacetimedb.com)** as relational tables and
reducers, with a thin stateless gateway translating the wire protocol. Combat, quests, loot,
spells, movement — every one is a transaction against a clean schema, and changing one is
publish-and-reload rather than a rebuild-and-restart.

You supply your own legally obtained 1.12.1 client. None is distributed here.

> **Local development only.** Everything binds to loopback, or opt-in to one private-LAN
> address. There is no supported public deployment in this alpha, and no rate limiting, TLS or
> anti-cheat. Never expose ports 3000, 3724 or 8085 to the internet — `:3000` is the real trust
> boundary, and the concrete reasons are in
> [`docs/quickstart.md`](./docs/quickstart.md#security-posture-local-development-only). Report
> security problems through GitHub's private vulnerability reporting, never a public issue.

## Quickstart

From a cold Linux or macOS machine:

```bash
curl -sSfL https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh | sh -s -- --yes
```

That clones into `./LyraCore`, checks prerequisites, and installs the `lyracore` launcher. It
**never runs `sudo`** — a missing system package prints the exact `apt-get` line for you to run.
To read it before piping it into a shell (you should), download it first and run `sh install.sh`,
which also lets it prompt.

```bash
cd LyraCore
lyracore doctor              # is this machine ready?
lyracore dev up              # start the local realm
lyracore account create TEST # a login for your client
```

Point your client's `realmlist.wtf` at `127.0.0.1` and play. The realm is seeded and playable
with **no client-data import and no third-party world database**.

Two more worth knowing:

```bash
lyracore dev smoke   # log in over the real 5875 protocol and enter the world, headless
lyracore import      # OPTIONAL: build the real Elwynn/Westfall from your own client's data
```

`lyracore` resolves whichever checkout you are standing in and runs *that* checkout's pinned CLI,
so any number of clones coexist. `./lyracore` from the checkout root works identically and needs
no `PATH` change.

**[`docs/quickstart.md`](./docs/quickstart.md) is the full clone-to-connected-client path** —
prerequisites, LAN play, and troubleshooting.

## What you get

**Out of the box**, `lyracore dev up` seeds a starting zone in Elwynn Forest with a demo
population — enough to log in, take quests, fight, loot and level. It is a fixture, not a
finished 1–60 world.

**Optionally**, `lyracore import` builds the real Elwynn/Westfall corridor — around 950 creature
templates, 640 quests, terrain and navigation — from data *you* supply: cmangos' public
`classic-db` dump plus the DBCs inside your own client. We ship the tools; you fetch the data;
nothing produced by that pipeline is distributed or hosted by this project. The command asks for
explicit consent before it reads or fetches anything. See
[`docs/data-ingestion.md`](./docs/data-ingestion.md).

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

A client never speaks SpacetimeDB — it speaks SRP6 and WoW opcodes — so the gateway is the only
legitimate SpacetimeDB client, and every mutation the world can undergo is a reducer call. The
realm is sharded across several databases running the *same* wasm; `dev up` brings up a sharded
fixture with a live seam you can walk across mid-session, and `dev up --single` collapses it to
one database.

Written in Rust, using gtker's `wow_srp` and `wow_world_messages` for SRP6 and the 1.12 packet
codecs. Pinned to Rust 1.93.0 and SpacetimeDB 2.7.1 exactly.

```
module/    ALL state + logic, compiles to wasm      gateway/   ALL socket IO, no game state
importer/  the bring-your-own-data ETL             crates/    shared, protocol-agnostic helpers
```

## Documentation

[`docs/architecture.md`](./docs/architecture.md) is the technical entrypoint, and its §9 indexes
every other document.

| Document | Purpose |
| --- | --- |
| [`docs/quickstart.md`](./docs/quickstart.md) | Clone → running realm → connected client. |
| [`docs/architecture.md`](./docs/architecture.md) | Tiers, topology, data model, sharding, packages — and the doc index. |
| [`docs/danger-zones.md`](./docs/danger-zones.md) | **Authoritative** over every other doc: traps and the exact deploy procedure. Read before any engine change. |
| [`docs/development-cli.md`](./docs/development-cli.md) | The `lyracore` command surface and its safety contract. |

## Contributing

Work is tracked as [GitHub Issues](https://github.com/LyraCoreProject/LyraCore/issues) —
[`good first issue`](https://github.com/LyraCoreProject/LyraCore/labels/good%20first%20issue) and
[`help wanted`](https://github.com/LyraCoreProject/LyraCore/labels/help%20wanted) are good places
to start. Questions go to
[Discussions](https://github.com/LyraCoreProject/LyraCore/discussions). Get a stack running with
the quickstart and run `cargo test` before proposing a change. Conduct follows the Contributor
Covenant 2.1.

## Provenance

LyraCore is an independent implementation — **not** a fork or port of any existing emulator, and
no code from a GPL-licensed core is present. What it uses is *knowledge* rather than code:
[wowdev.wiki](https://wowdev.wiki) for wire formats, the gtker crates as real dependencies under
their own licenses, and vMaNGOS/cMaNGOS/mangoszero as *behavioural* references for which packets
a client needs and in what order. Mechanics of a 2004 game are facts, and citing them is not
derivation.

## License

This project's own source is dual-licensed under your choice of the
[MIT License](./LICENSE-MIT) or [Apache License 2.0](./LICENSE-APACHE).

**Running it also requires accepting SpacetimeDB's license.** Seven `spacetimedb-*` crates this
workspace depends on are licensed by Clockwork Laboratories, Inc. under the **Business Source
License 1.1**, not MIT/Apache — separate from, and with no effect on, the license of our own
code. The BSL's Additional Use Grant permits production use with **no more than one SpacetimeDB
instance** and excludes offering the software as a "Database Service" to third parties. Its
Change Date is **2031-07-26** (as of SpacetimeDB 2.7.1), after which it converts to AGPL-3.0 with
a linking exception. This project stays inside that grant: the sharded realm runs on a single
SpacetimeDB instance.

*World of Warcraft* is a trademark of Blizzard Entertainment, Inc. This project is not affiliated
with, endorsed by, or sponsored by Blizzard Entertainment, Inc. No game client, game assets, or
game data are distributed with it.
