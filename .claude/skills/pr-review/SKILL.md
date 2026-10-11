---
name: pr-review
description: Finish a contributor's feature or fix from a PR, with adversarial review, evidence-backed confidence, workflow decisions, and a contributor reply. Use when asked to take a PR through completion. For a read-only review, report findings without edits or publication.
metadata:
    harness: [claude, codex]
---

# Finish the change

Own the behavior the contributor is trying to deliver. The PR supplies a starting point and
evidence. Follow the request through the code, fix the remaining gaps, and leave the contributor
with a concrete account of what their work made possible.

## Establish the outcome

Read `AGENTS.md`, `CONTRIBUTING.md`, `CODING_STANDARDS.md`, and `CORE_TERMS.md`. Reach the architecture,
testing, danger-zone, and maintainer documents when the affected code calls for them.

Read the PR body, commits, discussion, review threads, and originating issue. Fetch the current
base and head and record their SHAs. Inspect the implementation and callers beyond the diff.
Turn the request into observable acceptance criteria, grounded in the Spec or reference behavior.
Separate the contributor's claims from evidence you can inspect or reproduce. Check whether the
base already supplies any part of the fix.

Use an isolated worktree when the current branch or working changes belong to another task.
Keep the contributor's commits and authorship intact. Track additions separately so the final
reply can credit each contribution. Register the PR with the thread when the app supports it.

Establish the session's authorized scope before acting. A read-only review ends with findings.
A local trial keeps edits and its reply local. A request to finish the PR includes the necessary
local fixes and verification. Push, comment, merge, or close only within the user's authorization;
retain authorization already given. Prepare the concrete result before asking for any missing
permission. Production access remains subject to the repository's rules.

## Challenge the solution

Use an independent adversarial subagent. State file ownership first: the implementing agent owns
edits; the reviewer reads and reports. Select an available top-tier model with high reasoning
effort for design, concurrency, and adversarial review. Use the environment's live model catalog
when available. If delegation is unavailable, report that limit and perform a separate review
pass yourself without claiming independence.

Give the reviewer the acceptance criteria and their sources, checkout path, base and head SHAs,
and scope of local changes. Let it inspect the code before sharing your preferred conclusion.
Ask it to try to disprove that the complete feature or fix works, including behavior outside the
changed lines. It must assess both the Spec and repository standards, and return:

- Concrete failure cases with file locations and supporting evidence. Separate demonstrated
  defects from hypotheses that need a check.
- The smallest checks that could settle disputed behavior or missing coverage.
- Whether it is confident the outcome is complete, what supports that judgment, and what could
  still make the solution wrong. A clean diff or passing unit suite alone does not establish this.

Resolve findings by fixing them, disproving them with evidence, or naming the unresolved blocker.
Implement justified changes yourself within scope instead of returning a repair list to the
contributor. Preserve useful parts of the PR; replace an approach when a simpler one meets the
same behavior. Avoid manufacturing extra changes for the thank-you message.

Return the finished implementation and new evidence to an independent review round. Include the
original brief, prior findings, your responses, and unresolved objections. Review the combined
result, including your additions. With T3 delegation, create a new delegated task per round and
retain its task ID. After two repair rounds without convergence, stop and explain the concrete
disagreement or missing evidence. Further rounds need new evidence or direction.

## Choose and run the checks

Read `docs/testing.md` and the actual workflows under `.github/workflows/`. Inspect the PR's
checks and Actions runs. An empty check list does not mean there are no workflows: fork runs may
be waiting for approval. Record each relevant workflow's run URL, tested revision, result, and
whether it exercised the behavior in question. A PR workflow may test GitHub's synthetic merge
commit; relate it to the recorded base and head.

Decide which workflows need to run, and say why:

- Reuse successful runs that cover the current revision and required configuration. Do not
  rerun them merely to produce a fresh green badge.
- Use focused local checks while repairing. Then run the applicable repository checks on the
  final code. Add a regression test when it preserves required behavior. For an ordering bug,
  exercise the real ordering boundary and show the test can detect the broken behavior.
  Inspect cited tests' setup and assertions: they can bypass the changed path or wait away the
  failure they are supposed to detect.
- New commits, a changed base, missing coverage, or a diagnosed infrastructure failure can
  justify a new run. Fix code failures before retrying. Use the supported PR trigger or rerun;
  check that `workflow_dispatch` exists before trying to dispatch it.
- Before approving a fork run, inspect what it will execute, including workflow changes,
  scripts, dependency hooks, token permissions, and access to secrets or deployment targets.
  Approve ordinary isolated CI within existing authorization. Resolve any additional authority
  needed for secrets, paid resources, or live systems before triggering those actions.
- If checks are pending, blocked, skipped, or unavailable, name that state and its consequence.
  Explain checks that do not apply. Repository-required checks remain required even when a
  targeted local test passes. Do not retry unchanged failures indefinitely.

Keep test results tied to the code that ran. A passing PR run does not cover unpushed additions.
Record local commands, outcomes, and material gaps. Use the verification ladder in
`docs/architecture.md`; a lower rung does not prove a higher one. If asked to monitor CI and the
app provides a PR watcher, use it instead of polling.

## Decide whether it is ready

State your own confidence as well as the reviewer's. Explain the decisive evidence and remaining
uncertainty in plain words; avoid invented confidence percentages. Ready means the acceptance
criteria hold, material objections are resolved, and applicable checks cover the final code.
Otherwise state exactly what prevents that conclusion and the next check or decision needed.

Readiness and merge permission are separate. A protected branch can block a correct change.
Inspect the actual rules and report the restriction. Preserve protections and use the authorized
merge path. Re-read the PR head and base before publishing or merging; new code invalidates any
conclusion it affects. Never overwrite a contributor's concurrent work.

Follow `CONTRIBUTING.md` when choosing how to land: retain the contributor's PR when appropriate;
credit them as a co-author if most of the final implementation is ours. Use `file-pr` if a new PR
is needed. Merge or close only when authorized, then verify the resulting GitHub state before
describing it as merged or closed.

## Reply to the contributor

Apply `unslop`. Use the tone of
[samwhosung's reply on benilla #803](https://github.com/samwhosung/benilla/pull/803#issuecomment-6049493781):
specific credit, a short account of the additions, and the observable result. Read that example
when calibrating the voice; its technical details and merged status belong to that PR alone.

Thank the contributor for the concrete diagnosis, implementation, or evidence that helped.
Explain what their work established. Describe the changes you added and why they matter, with
links to published commits or the follow-up PR. State what was checked and any material gap.
End with the actual status and resulting behavior. If nothing needed changing, say that plainly.

Keep this a short maintainer reply, not a transcript of agent rounds or a list of demands.
Distinguish local additions from published code. Use a draft when publication is outside scope.
Before posting, check for an existing reply to avoid duplicates. Preserve actual newlines with
structured comment tools or `gh pr comment --body-file`. Report the posted comment's URL, or the
draft's location, along with any remaining blocker.
