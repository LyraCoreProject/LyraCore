---
name: lyracore-operator
description: Operate a LyraCore production realm through guarded updates and read-only diagnosis. Use when deploying merged changes, restarting or verifying the gateway, or diagnosing topology, SpacetimeDB, listener, realm-address, or capacity failures. Not for ordinary source implementation.
---

# LyraCore operator

Treat `docs/danger-zones.md` as authoritative. For either branch, first read
[`references/production-contract.md`](references/production-contract.md) completely; it defines the
independent production authority, target, topology, redaction, and health proof.

## Choose one branch

- **Update:** require an explicit request to change a named host, then read
  [`references/update.md`](references/update.md) completely before acting.
- **Diagnose:** read [`references/diagnose.md`](references/diagnose.md) completely and keep the run
  read-only.
- **Update failure:** preserve the mutation boundary already authorized, switch to the diagnostic
  evidence sequence, and stop at the failed gate. A failure does not authorize a repair outside the
  update workflow.

Use the exact result contract below. Finish when every gate is accounted for, including skipped
checks and unresolved warnings.

## Result contract

Emit concise Markdown with these headings in this order:

```markdown
## Target
- Host / checkout:
- Approved configuration source:
- SpacetimeDB node:
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
