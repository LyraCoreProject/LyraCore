# AGENTS.md

We build complex things as simply as possible, and we look for ways to remove complexity.

## Code

- Read `CODING_STANDARDS.md` before you implement, refactor or review code.
- Boy scout rule: fix defects in code you already change. Exceptions: a fix that spreads to other
  files, or that changes behaviour the user must weigh. Name those in one line and carry on. Fix
  everything else, even when it feels too big. That feeling reads high.
- Apply [the comment policy](CODING_STANDARDS.md#comments-and-documentation) when writing or reviewing
  source, tests, scripts and configuration. Complete its
  [comment review](CODING_STANDARDS.md#comment-review) before handoff.
- [`docs/README.md`](docs/README.md) lists every document and when to read it. Open a document when
  your change reaches its subject.
- Keep cargo target dirs, worktrees, clones and other large scratch output out of `/tmp` and
  `/dev/shm`, which are often RAM-backed. A throwaway, baseline or review build uses
  `CARGO_TARGET_DIR="$HOME/.cache/lyracore-scratch/<task>/target"` and puts its checkout under
  `$HOME/.cache/lyracore-scratch/<task>/`. Delete that directory when the task is done. Builds in
  the main checkout or a normal worktree keep the default `./target`.

## Words

- Apply the `unslop` skill (`.claude/skills/unslop/SKILL.md`) to all prose: docs, PR text, comments,
  reports.
- Use the glossary terms in identifiers, comments, commits, docs and PR text. Each term is defined
  in one file: `CORE_TERMS.md` holds the terms a new contributor meets first, `CONTEXT.md` every
  other term, and the index in `CONTEXT.md` lists both. Keep the `_Avoid_` words out of new names
  and prose. Existing identifiers, schema names, filenames and pinned artifacts keep their names.
  When you add or change a term, edit its one entry in the same change.

## Working with the user

- A question is a request for an answer. When a message asks rather than instructs ("how hard would
  it be", "what are your thoughts", "why does", "should we", "is it possible", "can X do Y"), answer
  it and edit nothing. When the change is obvious and trivial, answer first, offer the change, and
  ask before you make it.
- Spawn subagents only for breadth or adversarial review. When agents work in parallel, state file
  ownership up front. Choose each subagent's model and effort for its subtask: a cheaper model for
  scoped work, the top tier for hard design, concurrency or adversarial review. The spawning agent
  owns that choice.
- Require implementation and review subagents to read `AGENTS.md` in their checkout and return the
  comment-review result. Review the combined diff after integrating parallel work.
- Touch production or a live database only when the user tells you to or confirms it. Before you
  touch anything adjacent to one, name it.

## Issues and pull requests

- File and rewrite issues in the shape `CONTRIBUTING.md` gives under "The issue shape". Check every
  claim against the code at the commit you name.
- Open a PR with the `file-pr` skill (`.claude/skills/file-pr/SKILL.md`). It holds the title and
  description rules.

## Maintainers

Realm operations, visual and design work, and changes to the `./lyracore` CLI follow
[`docs/agents/maintainer.md`](docs/agents/maintainer.md). It is for maintainers only.
