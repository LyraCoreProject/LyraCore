# EventAI action coverage before and after the M4 fixes

This compares the full-world emitted plans: classic-db, vmangos at `928e3489`, and the corrected vmangos plan. A creature is an entry; GUID rules are grouped with their creature entry, and reachable relay actions are expanded. No source rows are reproduced.

“Equivalent” means every classic action key remains available; “reduced” means some remain; “lost” means none remain. Keys are spell, text, summon or emote ids, quest ids reached by event or creature-objective credit, and typed operands for other actions. This is static action coverage, not verified gameplay equivalence: it does not compare event timing, chance, conditions, targets, command flags or source-only extra actions. Flee text is a separate expected behavior (broadcast 1150). Native and durable test results are reported separately.

| Behavior | Classic creatures | Before equivalent / reduced / lost | After equivalent / reduced / lost |
| --- | ---: | --- | --- |
| All behaviors | 4315 | 499 / 2100 / 1716 | 1047 / 1606 / 1662 |
| Emotes | 66 | 10 / 9 / 47 | 18 / 10 / 38 |
| Encounter notifications | 20 | 3 / 7 / 10 | 3 / 7 / 10 |
| Flags and appearance | 102 | 3 / 0 / 99 | 4 / 0 / 98 |
| Flee | 1284 | 1248 / 0 / 36 | 1248 / 0 / 36 |
| Flee text | 1284 | 1 / 0 / 1283 | 1248 / 0 / 36 |
| Movement and posture | 736 | 8 / 0 / 728 | 8 / 0 / 728 |
| Other actions | 370 | 46 / 22 / 302 | 46 / 22 / 302 |
| Phase control | 145 | 7 / 61 / 77 | 7 / 61 / 77 |
| Quest credit | 12 | 8 / 0 / 4 | 8 / 0 / 4 |
| Speech | 1938 | 292 / 190 / 1456 | 1513 / 105 / 320 |
| Spells | 3556 | 637 / 797 / 2122 | 637 / 797 / 2122 |
| Summons | 26 | 0 / 0 / 26 | 11 / 0 / 15 |
