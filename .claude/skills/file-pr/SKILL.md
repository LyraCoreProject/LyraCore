---
name: file-pr
description: File a concise pull request. Use when the user asks to file, open or create a PR.
metadata:
    harness: [claude, codex]
---

# File PR

Before filing, read `CONTRIBUTING.md`, rebase onto the latest `main`, and check whether a PR for this
branch already exists. Review the diff locally against `origin/main` to make sure its contents match
the goal.

Complete the [comment review](../../../CODING_STANDARDS.md#comment-review) on that final diff and
include its scope and result under "What you verified".

## Title

PR titles become commit messages. Follow the repository's conventional commits, and look at recent
merged PRs for examples. Write a plain title that says what changes for a player, a developer or an
operator, for example "fix(player): movement no longer updates twice".

BAD
> perf(server): negotiate permessage-deflate on the websocket

GOOD
> perf(server): cut websocket frame size by 70% with gzipping

## Description

Fill `.github/PULL_REQUEST_TEMPLATE.md` and delete the sections that do not apply. Write in ASD-STE100
Simplified Technical English with the glossary terms.

"What and why" opens with a simple explanation of the problem, based on the user's original prompt,
then briefly explains the solution with a little context. Do not lead with an implementation
inventory:

BAD
> The starter aura families were hand-authored and unverified, and the slot-cap and diminishing-return policy had never been observed on a live database.

GOOD
> An aura is a buff or a debuff on a unit. Three rules govern them. A stacking family holds spells that must not stack, so only the correct member stays active. The module gives each unit 32 buff slots and 16 debuff slots, and refuses a new aura when its range is full. Diminishing returns cut crowd-control duration on a player through 100%, 50%, 25%, then immune.
>
> All three rules already existed. Nobody had checked the family data, and nobody had observed the slot caps or diminishing returns on a live database.

End the description with the model and harness that made the change.
