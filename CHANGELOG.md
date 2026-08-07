# Changelog

All notable changes to LyraCore are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) — with the caveat that a `0.x` alpha
makes no compatibility promise at all: schemas, environment variables, CLI flags, and database
names may change between pre-releases without a deprecation period.

This file starts at the first public release. The work before it happened in a private repository
and is not reconstructed here.

## [Unreleased]

Nothing yet.

## [0.1.0-alpha.1] — unreleased

The first public alpha. The promise it makes, and nothing beyond it:

> Anonymously clone LyraCore on Linux or macOS, start a safe local seeded realm, provision an
> account, and connect a user-supplied 1.12.1 build-5875 client — without importing any copyrighted
> game data.

### Added

- **`./lyracore`, a source-first development CLI.** `doctor` checks the prerequisites,
  `dev up` starts a node, publishes, claims the operator identity and starts the gateway,
  `account create` writes SRP6 credentials without ever putting a password on a command line,
  `dev smoke` drives a real build-5875 login over the wire, and `dev status`/`logs`/`down` manage
  what it started — verified by process *identity*, not by a bare PID. The CLI is pinned per
  checkout in `.lyracore-cli-rev`. See [`docs/development-cli.md`](./docs/development-cli.md).
- **A private-LAN mode** — `dev up --lan <RFC1918 address>` binds the two client-facing listeners
  and advertises the matching realm address, while SpacetimeDB stays on loopback in every mode.
- **[`docs/quickstart.md`](./docs/quickstart.md)**: the whole clone-to-connected-client path,
  per-OS prerequisites, `realmlist.wtf`, LAN play, and a troubleshooting section keyed to the exact
  message each failure prints.
- **A checked build on Linux and macOS.** Formatting, strict clippy, per-crate tests on both
  operating systems, an MSRV-consistency gate, offline preflight, publish safety assertions,
  coverage floors, and `cargo-deny`/`cargo-audit` supply-chain auditing. This pipeline runs in the
  maintainers' working repository; there is no public CI in this alpha.
- **Community-health documentation**: this changelog and [`SUPPORT.md`](./SUPPORT.md).
  Security reports go through GitHub's private
  [vulnerability reporting](https://github.com/LyraCoreProject/LyraCore/security/advisories/new),
  not a standalone policy document.

### Changed

- **The project is LyraCore.** Packages are `lyracore-shared` / `-module` / `-gateway` /
  `-importer`, binaries are `lyracore-gateway` / `lyracore-importer`, the default database is
  `lyracore`, and every `GW_*` environment variable is now `LYRACORE_*`. No compatibility aliases
  were added — this is a pre-alpha contract reset.
- **The documentation describes the current server**: a rewritten
  [`docs/architecture.md`](./docs/architecture.md) covering the multi-database topology, the read
  plane, sharding and the package substrate, with a document index and dated status banners on
  point-in-time material.
- **The wire test harness is its own repository.** It is server-agnostic and consumed as a pinned
  release rather than living in this workspace, so the server's own build no longer compiles a test
  client to go green.
- **Rust 1.93.0 is the toolchain and the declared MSRV**, pinned in `rust-toolchain.toml` and
  asserted against `Cargo.toml`. There is no lower buildable version: SpacetimeDB 2.5.0's own
  crates declare the same floor.
- **Tier-1 extension packages are documented as trusted compiled module code**, not as sandboxed
  plugins — in the README, the architecture documentation, and the decision record. Installing one
  is equivalent to accepting a patch to the server.

### Removed

- **The file-based work queue.** GitHub Issues is the single source of truth for what is open.

### Security

- **Publication gate passed**: a legal, IP, secret and security scan across the working tree
  and the whole reachable history. No credential was found and no history rewrite was needed. Dual
  `MIT OR Apache-2.0` licensing is now backed by real license files, the seven BSL-1.1
  SpacetimeDB dependencies are disclosed with the recorded single-instance decision, and a
  non-affiliation / trademark notice is in place.
- **The one GPL-derived function is gone.** A verbatim cmangos port in `module/src/skill.rs`
  and three further transcription-shaped functions were rewritten from written behavioural specs and
  proven bit-identical over dense input grids; 575 mangos-family references were classified as
  citation, wording, or derivation and handled accordingly.
- **The security posture is stated up front** rather than buried, in the README's warning banner:
  local development only, no supported public deployment, and the accepted weaknesses of that
  posture — no rate limiting, no TLS, no brute-force lockout, no anti-cheat, and a debug build
  whose reducers are mostly ungated by identity.

### Known limitations

- **Not internet-safe, and not a hosted product.** The maintainers' own verdict for a public VPS is
  NO-GO — see the warning banner in [`README.md`](./README.md).
- **Linux and macOS only.** Windows and WSL are neither covered nor tested.
- **Build 5875 only.** Other 1.12.x builds fail at the logon challenge or mid-handshake, with no
  friendly version-mismatch message.
- **The seeded fixture is the supported content.** A full zone needs an import built from data you
  supply yourself, and for the alpha that import is maintainer-run; no game client, assets, or world
  database ship with this project.
- **No anti-cheat, no rate limiting, no TLS.** Acceptable on loopback and nowhere else.

[Unreleased]: https://github.com/LyraCoreProject/LyraCore/compare/v0.1.0-alpha.1...main
[0.1.0-alpha.1]: https://github.com/LyraCoreProject/LyraCore/releases/tag/v0.1.0-alpha.1
