---
name: lyracore-operator
description: Operate a LyraCore realm through guarded production updates and read-only diagnosis. Use for deploying or publishing merged LyraCore changes, restarting a production gateway, checking shard or realm-core connectivity, investigating SpacetimeDB or gateway startup failures, diagnosing listener or realm-address problems, and producing an operator health report. Applies to realm operations, not ordinary source implementation.
---

# LyraCore operator

Treat `docs/danger-zones.md` as authoritative. Work against an explicit host and checkout; a realm
name alone is not a target.

## Choose one branch

- **Update:** require an explicit request to change the realm, then read
  [`references/update.md`](references/update.md) completely before acting.
- **Diagnose:** read [`references/diagnose.md`](references/diagnose.md) completely and keep the run
  read-only.
- **Update failure:** preserve the mutation boundary already authorized, switch to the diagnostic
  evidence sequence, and stop at the failed gate. A failure does not authorize a repair outside the
  update workflow.

## Operator contract

1. **Lock the target.** Record hostname, checkout, branch, commit, node URI, gateway manager, and
   gateway log source. Finish when every command can be tied to that target.
2. **Resolve topology.** Compare the expected production database list, `spacetime list`, and the
   gateway's sanitized `LYRACORE_DATABASE`, `LYRACORE_SHARD_MAP`, and `LYRACORE_REALM_CORE` values.
   Finish when the three views agree or a mismatch is a blocker.
3. **Keep secrets remote.** Read only named environment keys, render every token as `[redacted]`,
   and pass credentials from a remote file directly into a remote process. Finish when no command
   output or report contains a credential or full process environment.
4. **Prove connectivity.** A configured database list proves intent; one distinct
   `coordinator connected to shard <db>` line per expected database proves connectivity. A listener
   alone is not health.
5. **Report the run.** Use the exact headings and outcome vocabulary below. Finish when every gate
   is accounted for, including skipped checks and unresolved warnings.

Use only `./lyracore publish` for module deployment. Supply the complete production database list
in one invocation. The destructive clear-publish family is a hard stop. The contributor fixture's
`dev up` / `dev down` lifecycle is outside the production path. Use
`./lyracore production status` as the canonical latest-start evidence parser, then inspect actual
sockets separately.

## Result contract

Emit concise Markdown with these headings in this order:

```markdown
## Target
- Host / checkout:
- Commit: <before> -> <after or unchanged>
- Mode: update | diagnose

## Topology
- Expected:
- Discovered:
- Gateway configured:

## Checks
| Check | Outcome | Evidence |
|---|---|---|
| ... | PASS / WARN / FAIL / SKIPPED | bounded fact or log marker |

## Service state
- SpacetimeDB:
- Gateway:
- Listeners:
- Connected databases:

## Warnings
- Impact -> remedy

## Blockers
- Failed gate -> required next decision

## Next actions
1. Smallest safe next step
```

`SKIPPED` is a failed deploy gate in update mode. In diagnose mode it means evidence was unavailable;
state what prevented collection. Keep every heading: write `- None` in empty Warnings and Blockers
sections.
