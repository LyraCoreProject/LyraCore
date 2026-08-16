# Issue #37 — spellbook rank replacement verification

Source: [issue #37](https://github.com/LyraCoreProject/LyraCore/issues/37) — learning a non-stacking spell rank upgrade must replace the old rank in the vanilla spellbook.

## State of the world

The latest `origin/main` already implements the production behavior. The trainer handler resolves
the granted rank, sends `SMSG_SUPERCEDED_SPELL` when `WorldStore::superseded_old_rank` returns the
known predecessor, and otherwise sends `SMSG_LEARNED_SPELL`. The SpacetimeDB spell read uses the
same stack-in-book rule when preparing login initial spells. Persisted learned spell rows remain
unchanged.

Mana-cost spells stack in the book so players can downrank them. Passive and non-mana rank chains
collapse only when both the old and new ranks are known. The wire encoding is cMaNGOS-compatible:
the old rank occupies the first u16 slot and the new rank the second, regardless of generated
field names.

There is a socket test for the identical talent rank-upgrade packet. There is not an equivalent
trainer-rank test. The real 1.12.1 client remains the only acceptance seam for client packet
acceptance and relog display.

## Execution order

```
T1 (tracer, serial) -> T2 (integration and manual verification, serial)
```

| # | Ticket | Model | Est. tokens | File ownership |
|---|---|---|---:|---|
| T1 | Trainer rank-upgrade protocol coverage — **DONE** | gpt-5.6-terra | ~80k | `gateway/src/world/tests.rs` |
| T2 | Verify the completed behavior and record the client result — **BLOCKED: no interactive 5875 client attached** | gpt-5.6-terra | ~40k | no production files; issue #37 verification record only if live test can be performed |

## Shared rules

- Do not change schema, persisted learned-spell rows, or rank-stacking behavior.
- Test packets visible at the encrypted world-socket seam, not private helper calls.
- The test double may gain only the smallest configuration needed to model a known predecessor.
- Do not use Holy Light as the collapse case. Choose a passive or non-mana trainer chain.
- Run `cargo fmt` and the focused gateway test; T2 runs the relevant gateway suite.
- Do not close issue #37 without an operator-confirmed 1.12.1 client purchase and relog result.
