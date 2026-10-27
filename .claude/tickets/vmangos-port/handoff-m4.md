# Milestone 4 re-check handoff

The 15 Core slices are based on `e75851923fa0e9b114af16f4482f52e4238c6f24`. Each fix is in its owning slice, and later slices were rebased in order. Only the authorized `vmangos/*` branches and Packages companion branch were pushed, with leases. No main update, merge, upstream post, live Realm operation or Milestone 5 work occurred. Disposable local Realms supplied the runtime evidence.

Source behavior was derived from individual vmangos files at `464179081673cfd240f7ffe7f0daf96bca0b5a70`, fetched through the GitHub API. No vmangos or cmangos repository was cloned, no source code was copied, and no world-data artifact was committed or uploaded. Import plans, SQL, source references and logs remain local under `.import/m4-recheck/`.

## Findings

| Finding | Owning slice | Implementation commit | Outcome |
| --- | --- | --- | --- |
| 1: Quest gates and command 7 | #859 | `f477b2bf0efca9fb317fcb3b206f950a50ce8f3e`, `b2f04725ce2f377b5148cb63ecbf848543b8e84c`, `9fa0466943a14a56d34b4ba4b1697f41a5f2c387` | 2460 now grants credit after Shenthul receives a salute. 2608 retains its event gate and is accept-only. Distance, group credit and script speech overrides translate; runtime and Native checks pass. |
| 1: Translation prerequisites | #841 / #844 | `8ae94521ee4a57d95ef74f0e5317ae44ace56528` / `91a948da200d29438408e4248d398e213065a1c4` | Conditions 806 and 3694 use the incomplete quest state. Model bounding radii preserve source distance checks. |
| 2: Packages Core pin | packages#89 | `718af9160c191b9073d642501f07c658ccd8e41b` | A PR into Packages main keeps the pinned Core revision, including when its head has a paired Core branch. Pairing applies to non-main bases. Seven branch-selection tests pass. |
| 3: Guardian threshold | #836 | `a9db041810097f26a3bbab5dbcf5a516936e80b1`, `d4f4d83170d5fab9116f4c6fe58bb4ef1d572051` | More than 15 existing Guardians prevents another cast; otherwise the full amount is summoned. Durable tests check 15→16, 15→17 and refusal after each threshold is crossed. |
| 4: Flee text | #829 | `5fd03d2831f0830480a92f754dad0a847f103116`, `b46eaa51d97fd29ed466c2a129a4cc738df51e05` | Broadcast 1150 follows successful flee. A refused flee produces no speech. Both player-facing World Fake cases pass. |
| 5: Stored variants and encounter map | #841 | `8ae94521ee4a57d95ef74f0e5317ae44ace56528`, `2bdbbf2860d4ac6d6afd3e851b9a214d3084ff52` | M3 stored rules survive the current Module publish, execute after reconnect, and survive restart alongside new Reset/Rajaxx variants. The map rule now accepts any entry with a spawn on the bound map; runtime notifications still require that map and instance. No Rajaxx/map-1 exception remains. |
| 6: Summon census | #845 | `96b8d1e8c13e1d62a2713d4bab6845cdca78f2a9` | Production translation determines the count: 154 supported shapes of 197, 149 in emitted rules, 43 refused and five supported shapes without an emitted rule. The census lists every script id and zero-based step. |
| 7: Naming and tests | #838 / #829 / #841 / #859 | `055d2a6dc51a34114f1db0cdf5789882f88ce6ec` / `5fd03d2831f0830480a92f754dad0a847f103116` / `8ae94521ee4a57d95ef74f0e5317ae44ace56528` / `fcf4fb2c95850eaa52a696baa64c51adf38b14b1` | `insert_defaults` replaces `spell_defaults`; the family test requires a separate `spell` token; Reset uses the production dispatch test, with the test-only helper removed; status and template counts live in the ledger. |
| 8: CI | #829 / all slices | `b46eaa51d97fd29ed466c2a129a4cc738df51e05` | An intermediate #829 Clippy failure was real: the dispatcher exceeded 200 lines. A focused flee operation fixes it in that slice. The cache test failure reproduces on the feature base; current per-head checks are recorded separately below. |

The failed reward-request path also returned an offer screen for an incomplete quest. #859 now returns the incomplete quest screen and withholds the reward. The shared Native quest-chain check and actual imported 2460/2608 checks cover this behavior.

## Quest outcomes and recount

Quest 2460, The Shattered Salute: accept and an unrelated emote leave it incomplete. Salute 78 to Shenthul 3401 passes condition 806, emits the distance-limited command-7 credit, and allows reward. The actual-import Native test passed. The 20-yard limit includes both units' bounding radii and uses three-dimensional distance. Durable cases cover the radius boundary, an out-of-range failure, group and single-player credit, and unrestricted distance zero.

Quest 2608, The Touch of Zanzil: accepting and flexing cannot complete it. Condition 3694, chat overrides and command-7 distance/group fields are implemented, but the complete scene remains refused. Persistent emote state 69 and clearing state 0 need a stored animation field and client updates. The four-second move is relative to the player; moves at 14, 20 and 30 seconds require point-movement control, movement options and final orientation. These are absent systems. Consequently its command-7 credit at 33 seconds is not emitted. This is a known completion regression against classic, permitted by the task's accept-only fallback and recorded in [unsupported.md](unsupported.md). No free completion is granted.

The prior ledger had 15 quests refused at command 7, rather than 14: **219, 452, 590, 648, 836, 1173, 2767, 3367, 3382, 3982, 4265, 4901, 5713, 5944, 6523**. All 15 now pass distance/group translation. Nine still have no translated dispatcher: 219, 452, 590, 1173, 3367, 3982, 4265, 4901, 6523. Other commands refuse six: command 34 for 648, 836 and 2767; command flags for 3382; command 20 for 5713; command 71 for 5944. Quest 4265 also has a separate start script without a classic dispatcher. None of these 15 gains a complete emitted grant sequence from this change alone; 2460 is the new working grant.

The regenerated [event-credit report](event-credit-m4.md) covers 176 full-world candidates: one emitted grant, one retained unsupported gate, and 174 normal-objective fallbacks. The exclusive fallback categories are 11 refused SQL, 61 without dispatch, 39 C++ spell effects, 60 C++ scripts and three unresolved sources (3366, 6847, 6848). Alliance-single has one retained unsupported gate and 52 normal-objective fallbacks. These are production-plan classifications, not proof that all those quests can be played through.

## EventAI and summon census

| Profile | Subjects | Rules | Instructions | Relays | Dropped rules | Excluded rules |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Full-world | 3,143 | 4,923 | 5,609 | 32 | 946 | 310 |
| Alliance-single | 605 | 945 | 1,105 | 12 | 213 | 5,021 |

The regenerated [full-world census](census/creature-ai.md) and [alliance census](census/creature-ai-alliance-single.md) contain the exact summon list. The former count of 157 wrongly classified three target-10 commands in script 1128901, steps 0, 1 and 2. Those commands were already refused by production translation; the count now uses that translator. The 154 supported shapes do not imply 154 emitted commands. Flee and its speech now form one instruction, which also changes instruction totals.

## Verification

| Check | Current result |
| --- | --- |
| Importer | 576 passed, 18 ignored |
| Core Module library | 1,572 passed |
| Build-script / notification tables / Package API lint | 8 / 1 / 37 passed |
| Shared, protocol, Package-delta and test-support | Passed; one protocol test ignored |
| Core workspace Clippy, all targets and features, `-D warnings` | Passed after the speech-parser and flee refactors |
| Formatting and diff whitespace | Passed |
| Python SQL defaults and importer scripts | Passed |
| Release Wasm with debug reducers and Package fixtures | Built for Core and with the companion Packages |
| Shared Native suite | All 52 passed using the explicit Host and `--module` |
| Actual imported rogue quests | 2460 accept/wrong emote/salute/reward passed; 2608 retained gate passed |
| Affected durable tests | All 21 passed: quest credit 1, EventAI boundaries 4, full-world EventAI 3, Guardians 3, Package-delta references 10 |
| Deadmines with packages#89 | Choreography passed; doors and completion survived restart; Package API lint 37 passed |
| Packages branch-selection tests | Seven passed |
| Existing-data publish | Passed from populated M3 Module to current Module, without clearing data |

The Native Host was `/home/computer/LyraCore/.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone`. The current Core Wasm used by the final publish check is SHA-256 `7a865ed9a38333958990eebd383f9a4218fa8c398f4d2581be36b6bf8a38fd2a`. Native protocol checks ran before the final behavior-preserving flee/speech parser refactors; the final Module library, Clippy and durable checks cover those refactors. Results at the stack tip are not separate full-suite runs at each intermediate head.

The publish check populated an M3 Realm with legacy `StoredEventAiRule` rows and a character balance. Host preflight returned an automatic migration with `BreakClients` acknowledgement required for appended variants. Publishing with that exact preflight plan retained rows and balance. Reconnected Native clients executed the stored emote rule. New Reset/Rajaxx variants then survived Host restart and another reconnect. No `publish -c`, table clearing or production Realm was used. The acknowledgement contract is documented in `docs/data-ingestion.md`.

The all-Package library run passed 1,691 tests and reproduced two existing playerbot failures: `a_despawn_takes_the_quest_log_and_the_corpse_with_it` and `writes_maintain_simulation_aoi_and_native_delivery`. The prior handoff supplies matching baseline results at Core `f60424db` and Packages `7aff766c`. This run independently confirmed the failure names, unchanged playerbot files, and that Core `f60424db..e7585192` changes only BRIEF.md. The green Clippy result is Core-only. The companion's playerbot integration tests still refer to removed `support::gateway_binary`; branch-selection repair does not repair that separate compatibility issue.

## Classic byte parity and performance

| Artifact | SHA-256 |
| --- | --- |
| Full-world SQL | `fd06e44cb7a4bd3f8c529caeebc01d0f1db327448d048bb6d2e45c7a5ebaf980` |
| Alliance-single SQL | `cdd4c4461307d230da5f2c71ae859ab668798cd18fdc9ac49e9bc50f1f2be244` |
| All 467 reducer batches | `e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098` |

All three freshly generated artifacts match the required hashes.

| Release import | Seconds | Peak MiB | Limit seconds / MiB |
| --- | ---: | ---: | --- |
| Classic full-world | 9.74 | 535.414 | 14.375 / 587.5 |
| Classic alliance-single | 8.59 | 351.426 | 12.125 / 475 |
| vmangos creature-ai full-world | 1.21 | 34.387 | 14.375 / 587.5 |
| vmangos creature-ai alliance-single | 1.79 | 47.859 | 12.125 / 475 |

GNU `time -v` measured the release importer after compilation finished. All four measurements pass the fixed budgets. The vmangos measurements cover the creature-ai family. Earlier measurements during compilation exceeded the classic wall-time budget (21.91 and 16.22 seconds); both sets remain in the local logs so the changed load is explicit.

## CI classification and limits

The old #844 `host-tests` failure at `compile_cache.rs:145` (5 versus 6) is a cache insertion-count assertion; its fuel assertion had already passed. The exact failure reproduced using the unchanged feature-base test source and Host lockfile dependency versions in an isolated release test crate. Another run failed at the later insertion-count assertion (8 versus 9). This establishes a pre-existing flaky test, not a new M4 fuel regression. Logs are `compile-cache-base-reproduced-exact.log` and `compile-cache-exact-repetitions.json`. The full Host suite on the base was not rerun.

An initial #829 Clippy failure was caused by this work and fixed in #829 before the final restack. Pending checks are not treated as passes. Current CI and review states, plus failure logs and any one-time #844 retry, are recorded in `.import/m4-recheck/checks.md` and the final response. No PR is watched.

No real build-5875 client visual check was performed. Missing script dispatchers, encounter handlers, targeted-emote choreography, summon lifetime differences and hunter taming/Tame Beast unlock remain documented in [unsupported.md](unsupported.md). Historical checks of the eight Hallow's End quests and actual Rajaxx speech are prior evidence, not fresh runs in this re-check. The newly executed Native and durable checks above are the verified player outcomes; broader parity remains an inference limited by the ledger.

## Comment review

Passed separately from tests and formatting. The combined re-check diff and companion action/tests were reviewed with surrounding code, including added, changed and retained comments in source, tests, scripts and configuration. Retained SQL-default and classic quest-gate comments state compatibility constraints; tool annotations and the companion display-id constraint remain intact. Removed fixture comments and the test-only Reset helper carried no lost contract. Documentation pointers resolve. Later speech-parser/flee refactors and this handoff were included in the final review.
