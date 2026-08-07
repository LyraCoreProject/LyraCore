<p align="center"><img src="./lyracore-icon.svg" alt="LyraCore" width="140"><h1 align="center">Lyracore</h1><h3 align="center">A vanilla WoW server built for change
</h3></p>


LyraCore is a 1.12.1 game server built on top of [SpacetimeDB](https://spacetimedb.com/), focused on providing developers with the best possible experience.

> **Not ready for deployment** 
> LyraCore is a project in an extremely early phase of development. It is not recommended that you use it for any public facing project. 

## Quickstart

### Requirements
- A World of Warcraft 1.12.1 client. None is distributed here.
- [Rust](https://rust-lang.org/tools/install/) installed
- A Linux or macOS machine/container. WSL is untested, but might work.

### Installing
```bash
curl -sSfL https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh | sh
```
Installs the LyraCore folder in your current directory alongside the lyracore cli tool. 

```bash
cd LyraCore
lyracore doctor                                                     # is this machine ready?
lyracore config set client-data [path-to-client-Data-folder]        # tell LyraCore where your client data is 
lyracore dev up                                                     # start the local realm
lyracore account create admin                                       # a login for your client
```


Point your client's `realmlist.wtf` at `127.0.0.1` and play.

### Importing data

```bash
lyracore import
```
The import tool pulls game data *you* supply: cmangos' public
`classic-db` dump and the DBCs inside your own client.

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

## Can I use my existing client?

A client never speaks to SpacetimeDB — it speaks SRP6 and WoW opcodes — so the gateway is translator and every mutation the world can undergo is a reducer call. The
realm is sharded across several databases running the *same* wasm; `dev up` brings up a sharded
fixture with a live seam you can walk across mid-session.

## Documentation

[`docs/architecture.md`](./docs/architecture.md) is the technical entrypoint, built mostly to inform agentic tools.

| Document | Purpose |
| --- | --- |
| [`docs/quickstart.md`](./docs/quickstart.md) | Clone → running realm → connected client. |
| [`docs/architecture.md`](./docs/architecture.md) | Tiers, topology, data model, sharding, packages — and the doc index. |
| [`docs/danger-zones.md`](./docs/danger-zones.md) | **Authoritative** over every other doc: traps and the exact deploy procedure. Read before any engine change. |


LyraCore is an independent implementation — **not** a fork or port of any existing emulator. It heavily relies on *knowledge* and data rather than code from other projects:
[wowdev.wiki](https://wowdev.wiki) for wire formats, the gtker crates as real dependencies under
their own licenses, and vMaNGOS/cMaNGOS/mangoszero as *behavioural* references for which packets
a client needs and in what order. Without these projects, none of this would be possible.
