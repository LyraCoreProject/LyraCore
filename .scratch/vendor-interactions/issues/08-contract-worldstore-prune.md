# 08 — Contract WorldStore and prune migration scaffolding

**What to build:** With every vendor entry path on the seam, the broad `WorldStore` interface
shrinks through real migration: vendor-only operations (stock read, buy, sell, buyback,
repair, buyback-ring read) leave the broad trait, the production adapter, and the shared
in-memory store wherever no other action family still calls them; operations shared with
other families (player item reads, interaction-refusal test, slot-by-guid) remain only where
another caller exists. The legacy vendor handler and the old socket-writing buyback helper
are deleted; any temporary delegation shims from the migration disappear so no second seam
survives. Duplicate socket tests whose behavior is now pinned at the seam are consolidated —
keep representative session tests for packet delivery and session continuity, drop the rest.

**Blocked by:** 02, 03, 05, 06, 07 (all vendor actions and entry paths migrated).

**Status:** ready-for-agent

- [ ] No vendor-only operation remains on the broad store interface or its adapters.
- [ ] The legacy vendor handler and socket-writing buyback helper no longer exist; the
      dispatcher names no vendor reducer, resolves no slots, and reads no rows directly.
- [ ] No migration scaffolding (delegation shims, dead arms, dead codec callers) remains.
- [ ] Behavior coverage lives at the seam; at least one encrypted-session test per entry path
      (direct open, gossip open, world-entry replay, one durable action) remains.
- [ ] Full workspace test suite and `cargo build` are green.
