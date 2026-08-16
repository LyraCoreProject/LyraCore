# Issue #32 — movement input batching verification, broken into tickets

Source: https://github.com/LyraCoreProject/LyraCore/issues/32

## State of the world

The issue predates the current implementation. The repository already has the full runtime path:

```text
socket coalescer
  ├─ state edge / other input → flush pending heartbeat → blocking single reducer
  └─ steady heartbeat        → non-blocking per-shard buffer
                                  ↓ every 40 ms
                             chunks of at most 128 GwMove
                                  ↓
                             gw_movement_batch
                                  ↓
                    per-entry apply_movement_update + publish_staged
```

The code also rejects non-increasing non-zero movement timestamps, independently skips bad batch
entries, suppresses taxi movement, and has socket tests for immediate flush ordering. Do not build a
second batching mechanism. This slice makes the existing mechanism deterministic to test, proves the
module contract at the reducer seam, adds acceptance-grade measurement, and then reconciles the union.

The private work items referenced by the migrated issue are unavailable. Their non-negotiable
requirements are fully repeated in issue #32 and its specification comment, so those requirements are
the source of truth.

## Shared vocabulary and conventions

- **steady heartbeat**: a coalesced `MSG_MOVE_HEARTBEAT` that does not represent a movement state edge.
- **immediate movement**: start, stop, turn, jump, fall, facing, or a heartbeat flushed before another
  opcode. It remains on the single-update path.
- **movement batch**: one bounded `gw_movement_batch` reducer call carrying several `GwMove` entries
  for one world shard.
- **batch factor**: accepted/submitted movement entries divided by movement batch reducer calls over
  the same interval. Report both operands; never report the ratio alone.
- Tests assert observable entries, reducer submissions, committed state, relay payload and latency.
  They do not assert mutexes, Tokio task internals, or source text.
- Match current naming and idiom. Do not add issue numbers to comments.
- No schema or vanilla protocol changes. No adaptive cadence and no production deployment.
- Preserve the existing socket anti-lag tests unchanged.

## Execution DAG

```text
T1 gateway batching seam (serial tracer)
 ├── T2 module batch behavior ──┐
 └── T3 acceptance measurement ─┤
                               └── T4 union verification (serial)
```

| # | Ticket | Model | Estimate | File ownership |
|---|--------|-------|----------|----------------|
| T1 | Make the per-shard batch boundary deterministic | strongest | ~170k | gateway coordinator/batch code and focused tests |
| T2 | Prove independent reducer behavior and stale ordering | strongest | ~170k | module gateway reducer tests only |
| T3 | Add reproducible batch-factor acceptance measurement | mid | ~170k | metrics/load driver/docs, no movement runtime |
| T4 | Reconcile and verify the complete capacity lever | strongest | ~150k | union cleanup and acceptance documentation |

Every implementation agent reads this file and its own ticket. T2 and T3 branch from integrated T1.
T4 starts only after both parallel branches are integrated.
