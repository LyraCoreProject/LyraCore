---
name: fan-out
description: Slice a GitHub issue into local ~150-200k-token tickets, then fan out agents to implement them.
disable-model-invocation: true
argument-hint: <github issue url>
metadata:
    harness: [claude, codex]
---

# To Tickets

Slice a GitHub issue into **tracer-bullet** tickets sized for one agent context window each, write them locally, then fan out one agent per ticket to `/implement` it. The tickets exist to brief the agents; nothing gets published to GitHub.

## 1. Read the issue and the state of the world

Fetch the issue with `gh issue view <number> --comments`, then chase every issue and PR it references until you know what has actually shipped. Issues go stale: sibling slices may have landed since it was written, leaving a pattern to copy. Ground every ticket in the code as it is today, not as the issue assumed.

## 2. Slice

Cut vertical slices — each a complete, verifiable path through every layer it touches, never one layer spread across tickets.

- Size each ticket at **150–200k tokens of agent work** (one fresh context window). If a slice estimates bigger, slice again.
- The first ticket is usually a **tracer** that establishes the seam or pattern, runs alone, and blocks the rest.
- Middle tickets copy the tracer's pattern and can run in parallel.
- Close with an integration ticket that runs last: verify the union of the parallel work, reconcile duplicates, delete what the slices orphaned.
- Give each ticket its blocking edges; a ticket with no blockers can start immediately.

## 3. Write the tickets

Write to `.claude/tickets/issue-<N>/`, copying the shape of the exemplar in `.claude/tickets/issue-212/`:

- `README.md` — the shared brief every agent reads first: source issue, state of prior slices, the pattern to copy with its code shape inlined, shared conventions (error classification, naming), and the execution DAG.
- One `T<n>-<slug>.md` per ticket:

  ```
  # T<n> — <title>

  Parent: issue #<N>. **<runs alone | parallel with T…>**
  Model: <tier>. Estimated size: ~<n>k tokens.

  ## Problem
  ## Delivery
  ## Acceptance criteria
  ```

Tickets stay local: publish nothing to GitHub and leave the parent issue untouched.

## 4. Fan out

Match model to ticket difficulty: the strongest tier for the tracer and the integration ticket, a mid tier for pattern-copy tickets. Record the choice on each ticket's `Model:` line.

Work the **frontier** — the tickets whose blockers are all done:

1. Run the tracer first, alone, and integrate it.
2. Spawn one agent per unblocked ticket, concurrently, each in its own worktree so they cannot collide. Brief each agent to read the folder's `README.md` plus its own ticket file, then `/implement` it.
3. Integrate the parallel branches, then run the integration ticket serially on the combined result.

If the harness cannot spawn agents, work the frontier yourself in the same order.

Finish by reporting the DAG, each ticket's model, and each agent's outcome.
