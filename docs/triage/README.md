# Issue order and delivery

The [published report](https://1u1gh8ximdga.postplan.dev) is Codex's working dispatch list for
the GitHub issue queue. GitHub remains authoritative for issue state, review comments and CI.

`issues.json` holds the assessments, owners, next actions and review state. `report.html` is generated
from it. The report explains the scoring rule. Treat scores as estimates and revise them when
evidence changes.

## Update the report

1. Read current open issues and PRs with `gh`. Read comments before deciding that an issue needs
   implementation. Check linked PRs and current code for work that has already shipped.
2. Update `issues.json`. Give each new issue one assessment. Preserve unresolved acceptance checks
   after a code PR merges. Record the current PR head, check results and unresolved review findings
   in its entry. Name the implementer and next action.
3. Update `updated_at`, `base_revision`, `focus` and `history`. Keep the monitoring note accurate
   when work pauses or resumes.
4. Render and update the existing publication:

   ```bash
   python3 scripts/render-issue-triage.py --publish
   ```

The saved Postplan draft identifier keeps the URL stable when this work moves to another checkout.
Rendering without `--publish` only changes the local HTML. Check upload success before reporting an
update as published.

## Drive an assigned issue

Read its assessment and GitHub comments, then write local Tickets under `.claude/tickets/issue-N/`.
Use the fan-out skill for dependent slices. Independent implementations use separate worktrees with
declared file ownership. Local Tickets stay out of commits.

The implementer owns its PR after publication. Follow the file-pr skill, rebase before opening, and
record actual verification. After each push, inspect checks, CodeRabbit reviews, human comments and
unresolved threads. Fix valid findings; explain disagreement with evidence. The coordinating agent
reviews the finished diff against the Spec and coding standards and checks the current PR head
before merge. Update the report at assignment, PR publication, review changes, merge and any blocker.

Follow `docs/danger-zones.md` for human review requirements. Real-client acceptance and named-host
operations keep their separate completion criteria. Monitoring runs during the active work session;
this report does not install an unattended scheduler.
