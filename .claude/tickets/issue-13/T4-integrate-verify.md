# T4 — Integrate and verify the complete Duel flow

Parent: issue #13. **Runs alone after T2 and T3.**
Model: strongest tier. Estimated size: ~155k tokens.

## Problem

The parallel combat and lifecycle slices can overlap at completion events, generated bindings, and
gateway relay wiring. The combined result needs one consistent lifecycle, broad verification at the
highest available seam, and a clean shipping diff.

## Delivery

Reconcile the two slices around one completion API and event shape. Audit every acceptance point in
the issue spec, remove duplicates and orphaned code, regenerate/check gateway bindings, and run the
focused and workspace-level tests appropriate for the touched crates. Add or finish a two-session
headless protocol flow if the current harness can express two characters without inventing a second
test framework. Document a concise local two-client manual check for flag visuals and UI-only behavior
that automation cannot prove. Remove this local ticket directory from the shipping diff.

## Acceptance criteria

- Request -> accept -> countdown -> same-faction fight -> first-to-1-health -> winner works as one
  coherent server-authoritative flow.
- Decline, active surrender, out-of-bounds return, out-of-bounds expiry, logout/removal, and transfer
  all use one idempotent teardown and emit no duplicate terminal messages.
- A post-completion same-faction attack is refused, proving authorization did not leak.
- Packet and sparse descriptor updates match the typed vanilla protocol and clear on every terminal
  path.
- Generated bindings match the module schema and no hand-edited generated drift remains.
- Focused module/gateway tests and full relevant crate/workspace checks pass, or any unrelated known
  failure is captured with evidence.
- The local live-client verification steps cover two unmodified 1.12.1 clients, flag appearance,
  request/countdown UI, boundary warning/recovery, surrender, winner text, health floor, and combat
  exit. Do not claim the graphical check was run unless it actually was.
- The final diff contains implementation, focused tests, and useful verification docs only. It does
  not contain `.claude/tickets/issue-13/`.
- The integration result is committed and ready for local review against `origin/main`.
