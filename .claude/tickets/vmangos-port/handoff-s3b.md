# S3b items handoff

Branch `vmangos/s3b-items`, target `vmangos/s3a-globals`. The PR head is the commit containing
this report. Source pin `464179081673cfd240f7ffe7f0daf96bca0b5a70`.

## Verified

The items Import Family accepts vmangos. It reads named item columns, selects patch versions,
joins vendor templates through `vendor_id`, removes forbidden offers and creature drops, and
preserves four decimal places in Property Pool weights. The Module selects relative integer
weights, so the vmangos scale of 10,000 preserves the source probabilities. Classic-db keeps 100.
Creature loot already belongs to the items Import Family. S3c owns the other loot readers.

The selected catalogue contains items 25, 50, 51 and 52. Their diagnostic-root role remains;
all selected item templates are imported. The drink audit finds 40 name matches and 32
consumables with an on-use mana-regeneration aura. Sixteen name matches lack that aura and eight
aura matches lack those names. The vmangos flag follows its own selected spell table. The early
33-item audit included a non-consumable and was corrected by the generated census.

Projection retains primary keys and selection columns, then uses the same checked reader and
selection module. It reduced the initial 19.612-second items import to 5.367 seconds. No source
spell fields are combined with client spell fields.

Generated reports are `census/items.md` and `census/items-alliance-single.md`. Full-world counts:

| Module table | Classic | vmangos |
| --- | ---: | ---: |
| Item templates | 17,718 | 17,707 |
| Creature loot | 1,316,655 | 1,594,336 |
| Vendor offers | 13,113 | 13,653 |
| Property Pool members | 27,892 | 27,686 |
| Random Properties | 2,012 | 2,012 |
| Enchantment effects | 1,685 | 1,685 |

The reports list added and removed entry IDs and selection counts. Loot keys are creature
entries, not generated group IDs. Differences reflect selected catalogue membership, source loot
and reference membership, forbidden-item removal and vendor-template joins. The vendor-row floor
passes. Spawn-based vendor floors remain pending S4; an items-only plan cannot prove them.

`unsupported.md` adds `items.vendor-condition`, `items.vendor-item-flags` and
`items.vendor-restock`. Full-world occurrences are 424, 208 and 951. Alliance-single occurrences
are 21, 36 and 276. The 16 portal conditions remain. The full findings are printed by production
imports; generated reports omit source keys from the JSON summary.

Classic full-world and alliance-single SQL match the S3-pre baseline byte for byte. All 467
full-world reducer batches also match. SQL SHA-256 is
`b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40`.
Reducer JSON SHA-256 is `e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098`.
The temporary reducer comparison test passed and was removed after producing local evidence.

Release measurements use Python `resource.getrusage` and `time.monotonic`; `/usr/bin/time` is
absent. Each measurement starts a new wrapper process.

| Source and profile | Peak RSS MiB | Wall seconds |
| --- | ---: | ---: |
| Classic full-world | 536.922 | 8.306 |
| Classic alliance-single | 349.145 | 7.412 |
| vmangos items full-world | 284.676 | 5.367 |
| vmangos items alliance-single | 152.277 | 5.277 |

All four meet the brief's limits. vmangos measurements cover this family, not a complete import.

`cargo test -p lyracore-importer`: 489 passed, 16 ignored. Named SQLite fixtures cover version
selection, vendor joins, forbidden items, spell-based drinks, four-decimal weights and projection
with missing-column errors. `cargo clippy -p lyracore-importer --all-targets -- -D warnings`,
`cargo fmt --all -- --check` and `git diff --check` passed. The full-world reducer parity test
passed separately. No Module or script changes; their S3a and S3-pre checks carry forward.

Comment review passed across the final diff, new tests and census module, and retained comments
around edited code. Retained comments state the classic drink subclass quirk, loot chance units,
reference recursion rules and reducer ordering. The added projection contract explains that extra
selection columns may remain. Documentation links resolve; no removed contract was lost.

## Limits and next work

No stop rule triggered. No missing consumed item column was found. No deployed Realm or client
was used. The census is a production dry run, not a gameplay observation. EventAI dependency
closure and the vmangos Source Profile gate remain due in S6. The complete vmangos import cannot
run until the remaining families are ported. Loot conditions need S3c's audit. M3 must recheck
spawn-based vendor floors after importing vmangos creatures.
