# AGENTS.md

We build complex things as simply as possible, and we look for ways to remove complexity.

## Code

- Read `CODING_STANDARDS.md` before you implement, refactor or review code. Apply it to the code you
  change, and leave unrelated legacy code alone.
- Boy scout rule: fix defects in code you already change. Exceptions: a fix that spreads to other
  files, or that changes behaviour the user must weigh. Name those in one line and carry on. Fix
  everything else, even when it feels too big. That feeling reads high.
- Legacy code carries essay comments and issue references. Match its naming and idiom, not its
  comment density.

## Words

- Apply the `unslop` skill (`.claude/skills/unslop/SKILL.md`) to all prose: docs, PR text, comments,
  reports.
- Use the glossary terms in identifiers, comments, commits, docs and PR text. `CORE_TERMS.md` holds
  the common ones, `CONTEXT.md` all of them. Keep the `_Avoid_` words out of new names and prose.
  Existing identifiers, schema names, filenames and pinned artifacts keep their names. When you add
  or change a term, update `CONTEXT.md` in the same change. Update `CORE_TERMS.md` when it includes
  that term; add a term there only when new contributors need it.

## Working with the user

- A question is a request for an answer. When a message asks rather than instructs ("how hard would
  it be", "what are your thoughts", "why does", "should we", "is it possible", "can X do Y"), answer
  it and edit nothing. When the change is obvious and trivial, answer first, offer the change, and
  ask before you make it.
- Spawn subagents only for breadth or adversarial review. When agents work in parallel, state file
  ownership up front. Choose each subagent's model and effort for its subtask: a cheaper model for
  scoped work, the top tier for hard design, concurrency or adversarial review. The spawning agent
  owns that choice.
- Touch production or a live database only when the user tells you to or confirms it. Before you
  touch anything adjacent to one, name it.

## Issues and pull requests

- File and rewrite issues in the shape `CONTRIBUTING.md` gives under "The issue shape". Check every
  claim against the code at the commit you name.
- Before you open a PR, read `CONTRIBUTING.md`, rebase onto the latest `main`, and file it with the
  `file-pr` skill (`.claude/skills/file-pr/SKILL.md`).
- PR titles are plain and follow the repo's conventional commits, for example
  "fix(player): movement no longer updates twice".
- A PR description opens with the problem, then says how you solved it with a little context, in
  ASD-STE100 Simplified Technical English with the glossary terms. End it with the model and
  harness that made the change.

## Maintainers

Realm operations, visual and design work, and changes to the `./lyracore` CLI follow
[`docs/agents/maintainer.md`](docs/agents/maintainer.md). It is for maintainers only.
