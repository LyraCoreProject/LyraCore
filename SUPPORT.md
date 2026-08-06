# Getting help

LyraCore is an alpha, maintained by a very small number of people in their own time. There is no
support contract, no SLA, and no hosted service. What there is: documentation that tries to be
specific, and a tracker that gets read.

## Start here — the answer is often already written down

| Your situation | Where to look |
|---|---|
| "I cloned it, now what?" | [`docs/quickstart.md`](./docs/quickstart.md) — prerequisites, `./lyracore dev up`, account creation, `realmlist.wtf`, LAN play. |
| The stack will not start, a port is busy, `doctor` complains, the client will not connect | [`docs/quickstart.md` §8](./docs/quickstart.md#8-troubleshooting) — one entry per failure mode, with the exact message each one prints. |
| What does this CLI command do / is it safe | [`docs/development-cli.md`](./docs/development-cli.md). |
| How does the thing actually work | [`docs/architecture.md`](./docs/architecture.md), whose §9 indexes the rest of the documentation. |
| I want to change something | [`docs/danger-zones.md`](./docs/danger-zones.md) — traps, tooling gotchas, and the exact deploy/verify procedure to read before any change. |
| Is it safe to expose this | No — see the warning banner in [`README.md`](./README.md). |
| Where does world content come from | [`docs/data-ingestion.md`](./docs/data-ingestion.md). |

Before reporting anything, run:

```bash
./lyracore doctor
./lyracore dev status
```

Between them they distinguish most of the common failures — a wrong toolchain version, a port held
by someone else's process, a healthy pair of processes pointed at a node where nothing was ever
published.

## Where to ask

**Questions and discussion → GitHub Discussions**, on this repository. Setup questions, "is this
supposed to work like that", ideas you want to sanity-check before writing code, and anything where
you are not yet sure whether you have found a bug.

**Bugs and concrete feature proposals → [GitHub Issues](https://github.com/LyraCoreProject/LyraCore/issues).**
Whichever you are filing, include the quickstart step you were on, your `./lyracore doctor` output,
and which log the failure appeared in. Those three answers are what turns a report into a fix rather
than a thread.

**Security reports → not here.** Use GitHub's private vulnerability reporting on this repository; do
not open a public issue.

There is no chat server, no forum, no mailing list, and no social account. If you find something
claiming to be an official LyraCore channel, it is not one.

## What is in scope

Supported, and worth reporting:

- The **local development fixture** on Linux and macOS: `./lyracore` and everything
  [`docs/quickstart.md`](./docs/quickstart.md) describes.
- A stock, unmodified **1.12.1 build 5875** client connecting over loopback or a private LAN.
- The **seeded** world — the fixture written by the module's `init` reducer, which needs no
  client-data import.
- Anything in this repository being wrong: a broken command, a stale document, a dead link.

Out of scope, and closed as such:

- **Public or VPS hosting.** Not supported in this alpha, at all. See the warning banner in
  [`README.md`](./README.md).
- **Windows and WSL.** Not covered and not tested.
- **Any client that is not build 5875**, and any modified or launcher-patched client.
- **Requests for game data** — a client, MPQ/DBC files, or a world-database dump. This project
  distributes none of those and will not point you at somewhere that does.
- **Problems that only appear after your own content import.** Say so in the report; it is still
  worth filing, but it is a different world from the seeded fixture the test suites run against.
- Anything the [Code of Conduct](./CODE_OF_CONDUCT.md) covers.

## Response expectations

Best effort, by people with day jobs. `P1` (blocks play, or a live regression) gets looked at first;
everything else is queued by priority label (`P1`-`P4`) on [GitHub Issues](https://github.com/LyraCoreProject/LyraCore/issues).
A quiet issue has not been rejected, and note that anything labelled `needs-live-eyeball` is waiting
on a human sitting in front of a real game client, which is a scarcer resource here than engineering
time.

The fastest way to get something fixed is a pull request. `good first issue` and `help wanted` are
the labels to start from.
