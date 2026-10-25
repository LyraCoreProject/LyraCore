# Getting help

LyraCore is an alpha, maintained by a few people in their spare time. There is no support contract
and no hosted service.

## Look here first

| Your situation | Where to look |
|---|---|
| You cloned it and want a running realm | [`docs/quickstart.md`](./docs/quickstart.md): prerequisites, `./lyracore dev up`, accounts, `realmlist.wtf` and LAN play. |
| The stack does not start, a port is busy, `doctor` complains, or the client does not connect | [`docs/quickstart.md` §8](./docs/quickstart.md#8-troubleshooting), one entry per failure with the message it prints. |
| What a CLI command does, and whether it is safe | The [CLI command reference](https://github.com/LyraCoreProject/lyracore-cli/blob/main/docs/commands.md) in the `lyracore-cli` repository. |
| How the server works | [`docs/architecture.md`](./docs/architecture.md). |
| You want to change something | [`CONTRIBUTING.md`](./CONTRIBUTING.md). |
| Where world content comes from | [`docs/data-ingestion.md`](./docs/data-ingestion.md). |

Before you report a problem, run:

```bash
./lyracore doctor
./lyracore dev status
```

`doctor` finds a wrong toolchain version or a port held by another process. `dev status` finds a
running stack whose node has no published Module.

If you use a coding agent, start it in the checkout and ask it to read and follow
[`docs/triage.md`](./docs/triage.md). It diagnoses the problem on your machine, searches existing
issues, and drafts a report for you to approve.

## Where to ask

Use [GitHub Issues](https://github.com/LyraCoreProject/LyraCore/issues) for bugs, questions and
proposals. For a bug, give the quickstart step you were on, your `./lyracore doctor` output and the
log that shows the failure. For a question, say what you expected and what you saw.

Report a security problem in a private message to a maintainer on the
[LyraCore Discord](https://discord.gg/wxWHS3Tbj). Do not post it in a channel or a public issue.
Report harassment or abuse the same way.

This repository and that Discord server are the only official channels.

## What is in scope

We support:

- The local development fixture on Linux and macOS: `./lyracore` and everything
  [`docs/quickstart.md`](./docs/quickstart.md) describes.
- WSL2 with Ubuntu on Windows, as [`docs/windows.md`](./docs/windows.md) describes. The maintainers
  do not test it on every change.
- An unmodified 1.12.1 build 5875 client over loopback or a private LAN.
- The seeded world that the Module's `init` reducer writes. It needs no client data import.
- Errors in this repository, such as a broken command, a stale document or a dead link.

We close these as out of scope:

- Public or VPS hosting. The [`README.md`](./README.md) says why.
- Native Windows, Git Bash, and WSL 1. Use WSL2.
- A client that is not build 5875, or a modified or launcher-patched client.
- Requests for game data: a client, MPQ or DBC files, or a world database dump. This project
  distributes none of them and does not say where to find them.

A problem that appears only after your own content import is still worth a report. Say so in it,
because the test suites run against the seeded world, not yours.

## Response times

We work through [issues](https://github.com/LyraCoreProject/LyraCore/issues) by priority label, `P1`
(blocks play, or a live regression) first and `P4` last. A quiet issue is not a rejected one. An
issue with **How to check** waits for a person with a real game client, and we have fewer of those
than engineers.

A pull request is the fastest way to get a fix. [`CONTRIBUTING.md`](./CONTRIBUTING.md) says how we
judge one.
