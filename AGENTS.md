# AGENTS.md

In this project we focus on building complex things as simple as possible. We love to find ways to reduce complexity when solving problems.

## Coding standards

- Before implementing, refactoring, or reviewing code, read `CODING_STANDARDS.md`. Apply it to
  changed code without churning unrelated legacy code.
- Boy scout rule: fix defects in code you are already changing. Exceptions: the fix spreads to other
  files, or changes behaviour the user must weigh. Name those in one line and carry on. Fix
  everything else, even when it feels too big. That feeling reads high.

## Prose

- Apply the `unslop` skill (`.claude/skills/unslop/SKILL.md`) to all prose you write: docs, PR text, comments, reports.

## Domain language

- `CONTEXT.md` is the glossary. Use its terms in identifiers, comments, commit messages, docs and PR text. Do not use the `_Avoid_` words in new prose or new names. Existing identifiers, schema names, filenames, and pinned artifacts keep their names. When you introduce or change a term, update `CONTEXT.md` in the same change.

## Questions are read-only

- A question from a user is a request for an answer, not for changes. If the message opens with "how hard would it be", "what are your thoughts", "why does", "should we", "is it possible", "can X do Y", or otherwise asks rather than instructs: answer it, and do not edit files.
- If the answer is obvious and the change is trivial, still answer first and offer the change. Ask before making it.

## Match ceremony to the task

- Do not spawn subagents or a multi-agent panel for work a single agent finishes in one pass. Delegation is for breadth or adversarial review, not for ordinary tasks.
- When several agents work in parallel, state file ownership up front so they do not collide.
- Pick each subagent's model and effort for its subtask; inheriting the parent's is usually overkill. Scoped, well-guided work goes to a cheaper model; reserve the top tier for hard design, concurrency, or adversarial review. The spawning agent owns this call.

## Visual and design work

- Do not edit real components first. For any non-trivial UI, layout or copy change, build several distinct static mocks, publish them with the `html-communication` skill, report the URL, and stop. Wait for a pick before implementing.
- Standing constraints: Information-dense, no decorative card/pill chrome, no light-gray subtitle lines above sections. Minimal copy. No em dashes.

## Blast radius

- Never touch production or live databases unless explicitly told to or confirmed by the user. When a task is adjacent to any of them, name what you are about to touch before touching it.

## Realm operations

- For a production realm update or read-only realm diagnosis, use `$lyracore-operator`. Its
  authoritative safety boundary is `docs/danger-zones.md`; require a named host before mutation.
- Before changing `./lyracore` behaviour or its pin, read `docs/agents/cross-repo-cli.md`. The CLI
  source is the sibling `lyracore-cli` repository, not `.lyracore/cli/`'s installed cache.

## Pull Requests

- File PRs with the `file-pr` skill (`.claude/skills/file-pr/SKILL.md`).
- Make sure titles follow conventions from the repo. They should be simple and easy to understand. Conventional commit styles in projects that use them, i.e. "fix(player): movement no longer updates twice".
- PR descriptions should aim for simplicity. Open with a minimal, clear description of the problem. Follow up with how you solved it. Give a little bit of context, talk in ASD-STE100 Simplified Technical English, and use the ubiquitous language from `CONTEXT.md` if available.
- Add a blurb to the end of the PR description about what model and harness is making the changes.
- **Rebase onto latest `main` before opening**. Stale branches conflict and waste a review round.

## Legacy code style

Much of the existing codebase carries essay comments and issue references. That style is legacy, not the template: match the surrounding code's naming and idiom, not its comment density.
