<p align="center"><img src="./lyracore-icon-light.svg" alt="LyraCore" width="140"><h1 align="center">LyraCore</h1><h3 align="center">A vanilla server built for change
</h3></p>


LyraCore is a World of Warcraft 1.12.1 server written in Rust on [SpacetimeDB](https://spacetimedb.com/).
All game state and rules live in the database. You add content with Packages, without editing core
files, and players connect with an unmodified client.

> **Not ready for deployment.**
> LyraCore is in an early phase of development and is not secure. Do not use it for a public realm.

## Quickstart

### Requirements
- A World of Warcraft 1.12.1 client, build 5875. None is distributed here.
- A Linux or macOS machine or container. WSL is untested.

### Installing
```bash
curl -sSfL https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh | sh
cd LyraCore
```
The script puts the LyraCore folder in your current directory and a launcher for the
[lyracore CLI](https://github.com/LyraCoreProject/lyracore-cli) in `~/.local/bin`. It offers to
install Rust, SpacetimeDB 2.7.1 and `wasm-opt` if they are missing. Nothing needs root.

```bash
lyracore doctor                  # is this machine ready?
lyracore dev up                  # start the local realm
lyracore account create admin    # a login for your client
```
Point your client's `realmlist.wtf` at `127.0.0.1` and log in. The realm starts with a small seed
world. Import the real one next.

### Importing data
```bash
lyracore config set client-data <path-to-client-Data-folder>
lyracore import
```
The import reads data you supply: the DBCs inside your own client and cMaNGOS' public
`classic-db` dump.

## Can I use my existing client?

Yes. An unmodified 1.12.1 client, build 5875, is the only client LyraCore supports. Nothing is
patched, no launcher is replaced, and no addon is required. Other 1.12.x builds fail at the logon
challenge or during the handshake.

## Packages

A Package is a folder under `packages/` that adds content to the realm. It can hold Runtime Scripts
in TypeScript or Lua, data changes, client addons, Rust code, or any mix of these.

```bash
lyracore packages new my-package    # start your own
lyracore packages add playerbots    # install an official one
```

The official Packages live in [LyraCoreProject/packages](https://github.com/LyraCoreProject/packages).
[`docs/package-api.md`](./docs/package-api.md) lists what a Package's Rust code may call.

## Architecture

A client never talks to SpacetimeDB. The gateway speaks the WoW protocol and turns each client
action into a reducer call. The realm runs on several databases that all run the same module.
[`docs/architecture.md`](./docs/architecture.md) has the details.

```
            unmodified 1.12.1 clients (build 5875)
                      │  raw TCP · SRP6 · header-encrypted opcodes
                      ▼
   ┌────────────────────────────────────────┐
   │  GATEWAY  (edge / protocol tier)       │  TRUSTED · does ALL socket IO
   │  SRP6 logon :3724 + realm list         │  holds NO durable game state
   │  world :8085 · header cipher · codec   │  routes across databases
   │  stdb/ · subscriptions · AOI · relays  │  stateless, restartable
   └────────────────────────────────────────┘
                      │  SpacetimeDB client:  reducer calls ↑   subscription deltas ↓
                      ▼
   ┌────────────────────────────────────────┐
   │  SPACETIMEDB  (authority)              │  ALL state + ALL logic
   │  world shards · instance pool          │  transactional reducers
   │  realm-core: accounts · sessions ·     │  same wasm on every database
   │    groups · guilds · chat · loot rolls │  scheduled work drives the world
   └────────────────────────────────────────┘
```

## Contributing

Anyone can open an issue or a pull request. Start with [`CONTRIBUTING.md`](./CONTRIBUTING.md). It
says what gets in, how to file an issue, and how a maintainer judges a pull request.

## Credits

LyraCore is an independent implementation, not a fork or port of another emulator. It relies on
knowledge and data from other projects: [wowdev.wiki](https://wowdev.wiki) for wire formats, the
gtker crates as dependencies, and vMaNGOS, cMaNGOS and mangoszero as references for which packets
a client needs and in what order. Without these projects, none of this would be possible.

## License

Dual-licensed under [MIT](./LICENSE-MIT) or [Apache 2.0](./LICENSE-APACHE), at your option.
