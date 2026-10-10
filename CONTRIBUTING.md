# Contributing

LyraCore is a vanilla 1.12.1 server built for developers. A change is accepted when it makes the
server behave more like 1.12.1, fixes a bug, or makes LyraCore easier to build on. It needs evidence
and green checks, and it must be one small piece. Gameplay that 1.12.1 does not have belongs in a
Package, not in core.

## Read first

Three documents are required reading:

1. This file: what gets in and how a pull request is judged.
2. [`CODING_STANDARDS.md`](CODING_STANDARDS.md): how new and changed code is written.
3. [`CORE_TERMS.md`](CORE_TERMS.md): the terms the code, the comments and the reviews use.

Look everything else up when a change needs it:

- [`docs/quickstart.md`](docs/quickstart.md): from a clone to a connected client.
- [`docs/testing.md`](docs/testing.md): the commands CI runs, tier by tier.
- The recipes for [a new opcode](docs/recipes/add-an-opcode.md) and
  [a new spell effect](docs/recipes/add-a-spell-effect.md): every file and step, in order.
- [`CONTEXT.md`](CONTEXT.md): the full glossary, with an alphabetical index.
- [`docs/architecture.md`](docs/architecture.md): how the tiers fit together, and the index of the
  other documents.
- [`docs/danger-zones.md`](docs/danger-zones.md): schema migrations, publishing, and the traps that
  have broken a Realm.

## Picking an issue

The maintainers file issues in one shape (see "The issue shape"). Each says what 1.12.1 does,
what LyraCore does instead and where in the code, and how to fix it.

- `ready-for-agent` means the issue holds everything you need to build it, whether you work alone
  or with an agent. Start here.
- An issue with **How to check** in place of **Fix direction** has its code done. It needs a person
  at a real 1.12.1 build 5875 client. If you have one, these are welcome.
- `ready-for-human` needs a maintainer: a decision, a maintainer's Realm, or coordination. Leave
  these to the maintainers.
- `in progress` is taken. Pick another issue.

When you start an issue, open a draft pull request that says `Closes #N` as soon as you have a
first commit. A maintainer then marks the issue `in progress`.

## What gets in

- A bug fix, with how to see the bug before and after.
- A step closer to 1.12.1: a missing opcode, spell effect, mechanic or window, done the way the
  1.12.1 client and server do it.
- A correction where LyraCore and 1.12.1 disagree, with the 1.12.1 fact stated.
- A better developer experience: the CLI, the docs, the tests, and the tools for building on
  LyraCore.
- A Package API addition that a real Package needs.

## What does not

- Gameplay that 1.12.1 does not have, in core. It goes in a Package (see "Building on top").
- Anything from a WoW client or a world database: MPQ or DBC files, models, maps, or a database
  dump. You import your own at run time.
- Code copied from another emulator. Their behaviour is a reference. Their code is not.
- Big or mixed changes. Send one change per pull request, small enough to read in one sitting.

## Building on top

New content and behaviour beyond 1.12.1 live in a Package. `lyracore packages new <name>`
scaffolds one from the Reference Package ladder in the collection. Use `--from` to choose a rung. [`docs/package-api.md`](docs/package-api.md) lists what
a Package may call, and the build fails on anything else. The Official Package Collection,
[LyraCoreProject/packages](https://github.com/LyraCoreProject/packages), takes pull requests under
its own contributing guide.

If your Package needs a core path that the Package API does not list, open an issue. Name the path
and say why you need it. The surface grows by addition. LyraCore has no releases yet, so a Package
targets core `main`.

## How a change is judged

1. The checks pass. CI runs the Rust workflow and the durable suite on every pull request,
   including one from a fork. The pull request template lists the commands CI runs, and
   [`docs/testing.md`](docs/testing.md) explains them.
2. The evidence is stated: what 1.12.1 does, and how you know. A packet capture, a DBC field, a
   real client, or the behaviour of an established emulator all count.
3. The code follows [`CODING_STANDARDS.md`](CODING_STANDARDS.md) and uses the glossary terms.
4. A schema change follows [`docs/danger-zones.md`](docs/danger-zones.md): append new columns at
   the end with a default, and regenerate the Gateway bindings for a new table.
5. The title follows the repository's convention, for example
   `fix(loot): taking an item from the loot window adds it to the bag`.

## After you open it

Anyone can open a pull request. Only maintainers merge. A maintainer reads it, checks it against
1.12.1 and runs it. The maintainer may finish it on your branch with a rebase, a fix or a test, so
leave "Allow edits by maintainers" ticked. An automated reviewer also comments on each pull request.
Fix what it finds, or reply with the reason you disagree.

When most of what would land is ours, we land our own version with you as a co-author and close
yours with a note. When `main` already has the fix, we close yours and say where. A pull request
that is out of scope is closed with the reason.

## Setting up

You need Linux or macOS, Rust, and your own 1.12.1 build 5875 client. [`README.md`](README.md) has
the install, and [`docs/quickstart.md`](docs/quickstart.md) goes from clone to a connected client.
`./lyracore doctor` says what your machine is missing.

The seeded world needs no client data, and most tests run against it. Tests that need your own
client data are ignored by default, so a green run does not cover them.
[`docs/testing.md`](docs/testing.md) names the variables that run them.

## The issue shape

Every issue a maintainer or an agent files uses this shape. A bug report from the template is
welcome as it is, and a maintainer rewrites it into this shape.

- **Title:** one plain sentence that states the defect or the gap, for example "A Warlock cannot
  place a Soulstone on another player".
- **Expected (1.12.1).** What vanilla does, and how that is known. Use **Expected.** for tooling.
- **Actual** (at a commit). What LyraCore does now, with permalinks to the code at that commit.
- **How it shows.** What a player, a developer or an operator sees.
- **Fix direction.** The likely change, in a few sentences.

When the code is done and only a check on a real client remains, **How to check** and **Done
when** replace **Fix direction**.

## Reporting a bug

Open an issue with the bug template: what you did, what you saw, and your `./lyracore doctor`
output. If you know what 1.12.1 does instead, say so. Report a security problem privately through
[GitHub's vulnerability reporting](https://github.com/LyraCoreProject/LyraCore/security/advisories/new),
never in a public issue. [`SUPPORT.md`](SUPPORT.md) says what is in scope.

## Working with an AI agent

Working with an AI agent is expected. The agent reads [`AGENTS.md`](AGENTS.md), and the same rules
bind it. Say in the pull request which model and harness made the change.
