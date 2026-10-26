# S3c loot handoff

Branch `vmangos/s3c-loot`, target `vmangos/s3b-items`. The PR head contains this report.
Source pin `464179081673cfd240f7ffe7f0daf96bca0b5a70`.

## Verified

The loot Import Family reads selected pickpocketing, skinning, chest and fishing rows. Creature
loot remains in items; mail remains in quests. All nine source loot tables are audited. Forbidden
items are removed before reference weights are expanded. Signed reference IDs use unsigned
absolute values, including the minimum signed value. Named fixtures cover nested references,
patch endpoints, quest-only signs, forbidden direct and referenced drops, and equal-chance groups.

vmangos zero-chance group members share the chance left by explicit members. This follows
`LootGroup::Roll` in `src/game/LootMgr.cpp:1092-1128` at the pinned SHA. A 25% explicit member
and two equal members produce 25%, 37.5%, 37.5% after a forbidden member is removed. No upstream
code was copied. Classic-db keeps its old omission of direct zero-chance rows to preserve the
required baseline; changing that behavior needs a separate decision.

Generated full-world counts:

| Module table | Classic | vmangos |
| --- | ---: | ---: |
| Fishing loot | 1,198 | 2,484 |
| Chest loot | 4,792 | 13,834 |
| Pickpocket loot | 6,910 | 6,911 |
| Skinning loot | 2,799 | 2,968 |

`census/loot.md` and `census/loot-alliance-single.md` list entry differences, selected source counts
and explanations. Fishing has more source zones. Chest loot includes 8,204 selected zero-chance
source rows before template filtering. Skinning follows distinct selected skin IDs; pickpocketing
follows creature links. The four row floors pass. Creature-dependent floors remain pending S4.
Both items censuses were regenerated because the common loot reader now retains equal-chance
members. Full-world vmangos creature loot grows from 1,594,336 to 1,595,123 rows.

`unsupported.md` adds source-row findings across all nine selected tables: 4,652 conditions,
5,961 count ranges, 14 repeated references, 1,845 reference group selectors and 35 quest-only
fishing or skinning rows. It also records 2,662 item-container rows and 81 disenchant rows with no
world import path. No selected reference pool contains multiple group IDs, but the manifest checks
that shape too. These are source counts before profile filtering, not emitted or affected-player
counts. Items imports report the creature/reference findings. The 16 portal conditions remain.

Classic full-world and alliance-single SQL match S3-pre byte for byte. The full-world reducer
comparison passed for all 467 batches. Baseline SQL SHA-256 remains
`b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40`; reducer JSON SHA-256 remains
`e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098`.

Release measurements use fresh Python `resource.getrusage` / `time.monotonic` wrappers because
GNU time is absent:

| Source and profile | Peak RSS MiB | Wall seconds |
| --- | ---: | ---: |
| Classic full-world | 534.551 | 8.992 |
| Classic alliance-single | 347.910 | 7.989 |
| vmangos loot full-world | 59.926 | 0.935 |
| vmangos loot alliance-single | 61.465 | 1.458 |

All meet the brief's limits. vmangos measurements cover the loot family only.

`cargo test -p lyracore-importer`: 491 passed, 16 ignored after removing temporary parity
instrumentation. The separate reducer parity test passed. Importer clippy with all targets and
warnings denied, fmt and diff checks passed. No Module or script changes; earlier checks carry
forward. This slice does not require another wasm check.

Comment review passed over the final diff and new fixtures. The retained reference-zero comment
now names its classic-db contract; the basis-point comment remains next to the conversion.
Reference recursion and reducer ordering contracts are intact. Documentation pointers resolve.

## Limits and next work

No stop rule triggered and no consumed loot column was missing. The census and performance runs
were dry runs. No deployed Realm or client was used. Unsupported conditions and reference details
need later gameplay work. S3f must validate reward mail and its selected loot. M3 must check
creature-dependent floors. The vmangos Source Profile gate remains due in S6c.
