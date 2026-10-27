# Milestone 4 handoff

Implementation base: `f60424dbb6043cbbe80e58243af81accd6467dca`. Source behavior was independently derived from vmangos `464179081673cfd240f7ffe7f0daf96bca0b5a70`, using individual source files. No source code or world rows were copied into the change. The default remains classic-db. Milestone 5 has not started. Source references include [CreatureEventAI](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/AI/CreatureEventAI.cpp), [ScriptCommands](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Maps/ScriptCommands.cpp), [GameObject](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Objects/GameObject.cpp), [Pet](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Objects/Pet.cpp) and [ObjectMgr](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/ObjectMgr.h).

S6a is split into runtime and reader PRs. S6b is split into relay runtime and reader PRs. S6c separates the gate, Guardians, dungeon import, Package verification, immunity, encounter bindings, flag gates, speech, census and completion audit so each behavior has a reviewable diff.

| PR | Branch | Head | Target | Change |
| --- | --- | --- | --- | --- |
| [#827](https://github.com/LyraCoreProject/LyraCore/pull/827) | `vmangos/s6a1-action-lists-m4` | `95540b3f8daca6f18ee7f221dc37b8e141dd9a2d` | `feat/vmangos-world-data` | Three command lists and event policy; migration-safe policy storage. |
| [#829](https://github.com/LyraCoreProject/LyraCore/pull/829) | `vmangos/s6a2-events-m4` | `80b0ea93d4d04e096af70ef0d5270b23791925ec` | `vmangos/s6a1-action-lists-m4` | Named vmangos events, commands, conditions, target refusals and findings. |
| [#830](https://github.com/LyraCoreProject/LyraCore/pull/830) | `vmangos/s6b1-script-runtime-m4` | `96cc73390bd4bc44b98a02b145da32d47a46322c` | `vmangos/s6a2-events-m4` | Delayed-script failure policy, weighted child scripts and existing execution budgets. |
| [#832](https://github.com/LyraCoreProject/LyraCore/pull/832) | `vmangos/s6b2-scripts-m4` | `87856f89a3f311ab3fb10873e3a62dc8c366d901` | `vmangos/s6b1-script-runtime-m4` | Seven script families, generic-script reachability and GUID-keyed GameObject audit. |
| [#834](https://github.com/LyraCoreProject/LyraCore/pull/834) | `vmangos/s6c1-source-profile-m4` | `efc3fe3696fd698eb275e1e171d18bc8c60ae916` | `vmangos/s6b2-scripts-m4` | Column contract, approved value gate and actual snapshot provenance. |
| [#836](https://github.com/LyraCoreProject/LyraCore/pull/836) | `vmangos/s6c2-guardian-runtime-m4` | `d0c553a01620f36134d0463a999a45af22cc7a86` | `vmangos/s6c1-source-profile-m4` | NPC Guardians for VanCleef spell 5200, with combat and lifetime handling. |
| [#838](https://github.com/LyraCoreProject/LyraCore/pull/838) | `vmangos/s6c3-dungeon-import-m4` | `cc4ad3e3c5ee48f1988310722062af51a36be066` | `vmangos/s6c2-guardian-runtime-m4` | VanCleef imports, template dependencies and voice packets. |
| [#839](https://github.com/LyraCoreProject/LyraCore/pull/839) | `vmangos/s6c4-deadmines-verification-m4` | `b6b90b2b62fc978f08cd688ff5571bbfdda0a463` | `vmangos/s6c3-dungeon-import-m4` | Deadmines speech subscription verification. |
| [#840](https://github.com/LyraCoreProject/LyraCore/pull/840) | `vmangos/s6c5-scripted-immunity-m4` | `5f99bc3e23e697b550561416d4bdbf53f46a6ecc` | `vmangos/s6c4-deadmines-verification-m4` | Scripted immunity restoration and retained NPC-flag overrides. |
| [#841](https://github.com/LyraCoreProject/LyraCore/pull/841) | `vmangos/s6c6-encounter-bindings-m4` | `d2bc75c6dac4403c67ec061adaa31d7152d6ccf5` | `vmangos/s6c5-scripted-immunity-m4` | vmangos encounter fields and Rajaxx map predicates. |
| [#843](https://github.com/LyraCoreProject/LyraCore/pull/843) | `vmangos/s6c7-command-flag-gate-m4` | `4c6afdc7794d1d8997f484f4e3de25f864b21549` | `vmangos/s6c6-encounter-bindings-m4` | Command-specific field and target flag masks. |
| [#844](https://github.com/LyraCoreProject/LyraCore/pull/844) | `vmangos/s6c8-broadcast-gender-m4` | `f1ac1397227e1d3a6b1e417a6bd112549cefa185` | `vmangos/s6c7-command-flag-gate-m4` | Gendered broadcast replies and SQL migration defaults. |
| [#845](https://github.com/LyraCoreProject/LyraCore/pull/845) | `vmangos/s6c9-eventai-census-m4` | `f206bc39023695299d8b377a8e2988ec63c8c621` | `vmangos/s6c8-broadcast-gender-m4` | Generated EventAI and script comparisons for both profiles. |
| [Packages #88](https://github.com/LyraCoreProject/packages/pull/88) | `vmangos/s6c-deadmines-verification-m4` | `ee6100871e9006690ebeb5ef144af9753432074c` | `feat/vmangos-world-data` | Dungeons verifier, maintained entity writes and test-suite lockfile. |

This report is on `vmangos/s6c10-completion-audit-m4`, targeting `vmangos/s6c9-eventai-census-m4`. Its additional code change checks every alternative spawn entry when validating encounter maps. The census is byte-identical before and after that fix. The final response and local verification receipt record this slice's PR and final head.

## Verified results

The [full-world census](census/creature-ai.md) and [alliance-single census](census/creature-ai-alliance-single.md) are generated from production plans. They include source command frequencies for each script family, added and removed subjects, and Compatibility Manifest findings. Counts do not establish player-visible parity.

| Profile and source | Subjects | Rules | Instructions | Relays | Relay steps |
| --- | ---: | ---: | ---: | ---: | ---: |
| Full-world classic | 4,334 | 10,815 | 13,031 | 121 | 449 |
| Full-world vmangos | 3,056 | 4,798 | 5,330 | 27 | 69 |
| Alliance-single classic | 769 | 1,750 | 2,122 | 27 | 108 |
| Alliance-single vmangos | 585 | 917 | 1,039 | 9 | 21 |

The seven delayed-script families contain 8,247 vmangos rows and 2,329 keys, versus 6,292 classic rows and 1,580 keys. Only generic scripts have translated EventAI roots. Full-world drops 1,071 unsupported EventAI rules and excludes 310 non-EventAI rules. Alliance-single drops 237 and excludes 5,025 outside its scope or AI selection. The complete final program loaded on a Disposable Host in eleven batches, with eight dependency-only spell headers. That check establishes loader acceptance, not spell execution.

The [approved sets](approved-sets-m4.md) cover 75 tables and 1,028 consumed columns. Event flags permit mask `0xf`, command flags `0x1f` and condition flags `0x3`. The report lists exact event, command, target and condition sets, plus command-specific and target-specific masks. Unknown values refuse import. All thirteen Import Families and the DBC comparison passed the Source Profile during verification. Counts are reported without gating.

The [event-credit ledger](event-credit-m4.md) assigns an unsupported reason to each of the 176 event-credit quests. None has an emitted completion instruction. Seventy-four have SQL command-7 references; 102 have none. The raw selected catalogue has 212 flagged quests, of which 35 have area-trigger completion; quest 9378 has no creature or GameObject giver relation and is outside the imported catalogue. Quests 8353-8360 instead passed actual emote-driven creature-objective credit and NPC replies. Wrong emotes left their objectives pending. Acceptance was staged, and reward flows were not exercised.

Only Telonis 4500 regains player immunity through its verified unconditional spawn clear. Its NPC immunity remains omitted. Archaedas 2748 and Arugal 4275 stay unflagged for their unsupported C++ AI and GUID-conditioned clear respectively. Full-world still reports 3,407 templates with at least one omitted immunity; alliance-single reports 1,022. The player-immunity list now has 1,167 entries and the NPC-immunity list 3,264.

Actual VanCleef rows produced the aggro and threshold yells, voice packets and two attacking Blackguards at 50% health. The player-kill yell is imported but unexercised. Rajaxx uses source map-509 spawn and evade rules; instance counters and zone-wide voice remain unsupported. All five source EventAI display commands use explicit display ids; none emits a template-display instruction. Authored fixtures cover template dependency closure and fixture-band refusals. Checked source tables have no ids in 5,099,000 through 5,099,999.

| Verification | Result |
| --- | --- |
| `cargo test -p lyracore-importer` | 568 passed, 18 ignored. |
| `cargo test -p lyracore-shared` | 325 passed. |
| `cargo test -p lyracore-module --lib --features debug_reducers` | 1,567 passed without Official Packages installed. |
| Package delta, protocol and test support suites | Every result line passed; one protocol test ignored. |
| Module `build_script` / `package_api_lint` | 8 / 37 passed. All Official Packages also passed the 37 API tests. |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed without Official Packages installed. |
| `cargo fmt --all -- --check` | Passed. |
| Release Wasm, `debug_reducers,package_test_fixture` | Built for Core and with all Official Packages. |
| Shared native suite, explicit Host and `--module` | All 51 passed with final Core Wasm. |
| Affected durable tests | 14 passed across seven targets. |
| `module/tests/deadmines_package.rs --ignored` | Passed with the companion dungeons Package; final rerun 9.34 seconds. |
| Dungeons door-persistence test | Passed through two Standalone restarts, 103.52 seconds. Locked recheck passed. |
| Python SQL-default tests | Three passed. |
| Existing-data publish | M3 populated display, presentation, EventAI and Character data survived; default gender zero and NPC override None; Character reconnected. |

Durable targets: `eventai_quest_credit`, `eventai_revision`, `eventai_runtime_boundaries`, `full_world_eventai`, `full_world_population`, `guardians`, `scripted_immunity`. Each ran with `cargo test -p lyracore-module --test <target> -- --ignored`. They used private Standalone nodes.

The native command was `python3 native-host/check.py --host /home/computer/LyraCore/.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone --module <release-wasm>`. Final Core Wasm SHA-256 is `ed0d356fc1a00a7a8166dc3373fb93c78313d1df2e683d68aa30190b028b7008`. Publishing used the Host migration preview and normal non-clearing update over M3 Wasm `6f9ebb255a7f5776b189d95ff2d0f7da95a3f861ad25d91768948a7548fe52bc`. Persisted variants are appended; fieldless rule policies retain their representation through separate private policy storage.

## Byte parity and performance

| Artifact | SHA-256 |
| --- | --- |
| Classic full-world SQL | `fd06e44cb7a4bd3f8c529caeebc01d0f1db327448d048bb6d2e45c7a5ebaf980` |
| Classic alliance-single SQL | `cdd4c4461307d230da5f2c71ae859ab668798cd18fdc9ac49e9bc50f1f2be244` |
| All 467 classic reducer batches | `e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098` |

All three match the required baseline byte for byte. No reader-defect exception was needed.

| Release import | Seconds | Peak MiB | Limit, seconds / MiB |
| --- | ---: | ---: | --- |
| classic, full-world | 9.903 | 539.430 | 14.375 / 587.5 |
| classic, alliance-single | 10.047 | 349.012 | 12.125 / 475 |
| vmangos-ai, full-world | 1.021 | 34.344 | 14.375 / 587.5 |
| vmangos-ai, alliance-single | 1.734 | 49.125 | 12.125 / 475 |

Measurements used `.import/vmangos-m4/measure.py`, a Python `time.monotonic` and `resource.getrusage(RUSAGE_CHILDREN).ru_maxrss` wrapper. `/usr/bin/time` was absent. Each measurement started a fresh wrapper, wrote output to disk and ran after task builds stopped. These vmangos measurements cover the creature-ai family, not a completed all-family M5 import.

## Limits and CI state

All PRs are drafts, with no submitted reviews at the recorded status snapshot. No PR was watched. Core slice checks report Official Package failures because the workflow falls back to Packages `main` when it cannot find the exact slice branch. That checkout lacks M3 spell fields. Local Package verification used companion head `ee6100871e9006690ebeb5ef144af9753432074c`.

Some earlier slices also have red durable jobs. The later #844 and #845 snapshots have all durable shards green. #845 has a red shared-native progression assertion, `native-host/check.py:4694`, after an invalid watched-faction request. The local full suite passed, and isolated progression checks passed on both M3 and final Wasm. The CI failure remains unreproduced; it is not classified as pre-existing.

Package #88 has red Module and Datascript jobs. The logs identify a playerbots test-suite lockfile update and a Core checkout without `GameObject::delete`. These CI jobs did not validate the intended final Core/Packages pair. They remain red; this handoff does not claim CI is green.

With every Official Package installed, local Clippy fails with 70 library and 73 library-test errors. The same counts and diagnostics were reproduced on Core M3 `f60424db` with frozen Packages `7aff766c`. The all-Package Module library also has two reproduced baseline failures, the playerbots corpse scanner and native-write scanner. Its native addon check has the same malformed-command-response failure on the baseline. Core-only suites pass. These unrelated failures remain outside the changed files.

No real build-5875 client visual check was performed. NPC spell presentation and full-world capacity are not established by these checks. Missing Package encounter handlers, source C++ encounters, unsupported credit paths and explicit display-id commands are recorded in [unsupported.md](unsupported.md). Fixed-duration flee has 1,563 full-world and 293 alliance-single limitations. Guardian creator/spell client fields and alternate-gender model choice remain unsupported.

The 17 SEND_CHANNEL_VISUAL and 30 TRIGGER_WEAPON_PROCS spells remain held. No new table was held back for a missing column. Map remains on its M3 source in this implementation. The maintainer added its named exception in `e9056a2b`; M5 S7-pre owns the gameplay columns and separate Map.dbc archive-directory lookup. No Package API stop rule triggered.

## Comment review and Milestone 5

Comment review passed on each slice and on the combined diff, including changed and retained source, tests, scripts and configuration comments. The fieldless policy-storage contract points to the Command lists section of `docs/data-ingestion.md`. Opcode/flag facts and migration defaults were checked against their implementations. Removed Package comments repeated mechanics; setup, spawn-hook and documentation contracts were retained. No comment-review findings remain.

M5 must retain the unsupported quest and script ledger when it flips the default. Resolve CI branch pairing for the Core/Packages feature branches, and distinguish the remaining unrelated Package failures from this port. Implement the newly approved Map exception in S7-pre. Keep both held spell flags and unsupported scripted immunities in the later conformance ledger. The final reports and raw verification logs are under `.claude/tickets/vmangos-port/` and ignored `.import/vmangos-m4/` respectively. Task-owned scratch checkouts and build caches are removed after handoff artifacts are saved.
