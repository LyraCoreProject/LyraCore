# Milestone 4 review-fix handoff

The 14 original slices were rebased onto `95396fa20ad9b36cad86a73f8c5330b5f8c30e56` and pushed with leases. The new top slice is [#859](https://github.com/LyraCoreProject/LyraCore/pull/859), `vmangos/s6c11-event-credit-rule-m4`. Only authorized slice and companion branches were pushed. No merge, main update, upstream post, production change or Milestone 5 work occurred. Source behavior was independently derived from individual files at vmangos `464179081673cfd240f7ffe7f0daf96bca0b5a70`; no upstream repository was cloned and no world-data artifact was committed or uploaded.

## Findings and player outcomes

| Finding | Owning PR | Commit | Change and evidence |
| --- | --- | --- | --- |
| 1 | 859 | `a61a00a601c05bbf4c884bee06ce9a5e89307dcd` | Event gates require emitted credit; ledger and Native normal-objective fallback test. Taming use/unlock remains unsupported as detailed below. |
| 2 | 829 | `8abfe8ae2cb1eda932240053e14319453aeb3ef2` | Broadcast 1150 accompanies flee; player-facing speech and rout test. |
| 3 | 841 | `646c43db915b725bbde8cb109128409aad30c983` | Condition 33 maps; reader fixtures and Rajaxx import/Native checks. Zone/death speech prerequisites are in #838. |
| 4 | 838 | `2f54243bfc0d86372dad9f77de38367a63b301d3` | SQL sound default introduced with the column; both affected durable tests passed on every #838–#843 head. |
| 5 | 829 / 838 | `8abfe8ae2cb1eda932240053e14319453aeb3ef2 / c9e59d29b7c629f8c8055b390da97a4e141af8c9` | Plain summons, optional target handling and scoped dependencies; placement and appearance tests. |
| 6 | 841 | `646c43db915b725bbde8cb109128409aad30c983` | State 0 emits Reset; evade notification and not-started mapping tests. Missing Package handlers are not implemented. |
| 7 | 829 | `8abfe8ae2cb1eda932240053e14319453aeb3ef2` | Random emotes translate; each choice reaches the player-facing World Fake output. |
| 8 | 827 / Packages 89 | `c308e85d480f096e404c620de25abe5baa441133 / f1e6106e10f17457e7ebe83bb9c136e4996bc9b0` | Exact/base/feature fallback in both repos; 5+6 branch-selection tests. Matching companion branch replaces closed #88. |
| 9 | 840 / 848 | `960329e7081126c136d490a6e5d3065cbbe1bd8d / f4ebf4ca4de66931686a86f6d902f6d00fe8aa3d` | Correct name, counts and transient packet effect; durable and actual Native checks. |
| 10 | 836 | `5ae80473d24d7aef91112d04be40f561b5f6a9a3` | Total Guardians capped at 16; repeated-cast durable test. |
| 11 | 829 | `c60df40a02e839342e479586287205288e7f1072` | All 13 families listed in the error; focused error-output test. |
| 12 | Packages 89 | `f1e6106e10f17457e7ebe83bb9c136e4996bc9b0` | Restore equip_swap display-id comment; checked against the implementation and Deadmines verifier. |
| 13 | 827 | `0ad2d7369117fe97b07dd1ee01c7ad2d5ce73171` | Wait for missile impact and check only watched-faction field; reproduction/widened-window checks and full Native suite pass. |

Plain summons: 157 of 197 command rows have supported shapes, including all 139 cases identified by review. Of these, 149 occur in emitted rules; the others have unsupported surrounding rules or excluded templates. Forty command shapes remain refused. Mor Grayhoof 16044 and Isalien 16045 can summon without a current opponent; Crystal Fang 10376, Defias Dockmaster 6846 and Shy-Rotam 10737 retain their summons. Thor 523 and Dungar Longdrink 352 retain speech and gryphon summons. Sethir 6909 remains refused because his unique-count/radius limit and flags are unsupported. Accepted corpse-lifetime differences are reported for 84 full-world and 17 alliance-single rules.

Twenty of 21 random-emote commands appear in emitted rules. The pin has 93 targeted-emote commands, not the review's approximate 92. Their timed movement pause, facing and restoration are not represented; all remain refused.

The condition audit checked types 0, 6, 9, 24, 33 and the -1/-2/-3 boolean operators against pinned `Conditions.h`/`Conditions.cpp`. Type 15 is level and remains refused. The five actual map-condition ids pass through the SQLite reader in tests. All seven Rajaxx rules now translate. The actual-import Native check observes AQ faction 310 and the death yell and voice for both near and distant players in the same instance. World Fake tests cover his bound-map notifications and Wailing Caverns evade Reset signals. Missing Package encounter handlers remain missing: notification translation does not itself reset an implemented encounter's state.

Overlord Mok'Morokk (4500), not Telonis, is the only template whose unconditional translated spawn clear qualifies for restoration. Native creation sends flags 320, then a values update sends 64; the committed creature already has the immunity bit cleared. This changes the packet sequence but creates no lasting server-side immunity. A real-client frame effect was not checked. Four templates have any EventAI immunity clear: 1840, 4275, 4500 and 10992. The other three do not meet the rule. Full-world still omits at least one immunity on 3,407 templates; alliance-single on 1,022. The full-world lists contain 1,167 player-immunity and 3,264 NPC-immunity omissions.

## Event-credit outcomes

The [per-quest ledger](event-credit-m4.md) is generated from the scoped production quest and EventAI plans plus pinned source references. Full-world has 176 normal-objective fallbacks, alliance-single 53, and neither keeps an event requirement without emitted credit. These are eligibility results; every quest's complete gameplay has not been exercised. Of the 176, 175 occur in the classic catalogue; the review's 165-playable subset was not independently established.

| Exclusive fallback category | Full-world quests |
| --- | ---: |
| Refused SQL | 22 |
| Script without dispatcher | 52 |
| C++ spell effect | 39 |
| C++ script reference | 60 |
| Grant source not located | 3 |

The unresolved sources are 3366, 6847 and 6848. A C++ quest constant locates a script but does not prove a reachable grant; the ledger distinguishes that evidence. The raw source has 212 event-marked quests: 35 use area-trigger objectives and 9378 has no imported giver relation, leaving 176 candidates.

The twelve Taming the Beast quests are 6061–6064, 6082–6085, 6087–6088 and 6101–6102; 6086 is Training the Beast. The committed Native check accepts imported quest 6085 as a dwarf hunter, receives and returns its supplied rod, rewards the quest and accepts 6086. Earlier prerequisites are staged. Casting the rod instead deletes the item needed for turn-in; the same failure was reproduced on pre-fix `928e3489`, and the affected item-use code is unchanged from feature base. The existing path also ignores the supplied target and does not model item charges. The C++ taming callback is unported. Reward spell 1579 teaches three spells and the existing single-spell reward importer refuses it. Successful taming and unlocking Tame Beast are therefore **not fixed or verified** by this change. The normal-objective fallback is verified.

Quest 2460's Shenthul salute sequence needs condition 806 (quest-incomplete, type 9/value2 1) and command-7 credit limited to 20 yards. Neither is supported. Quest 2608's Doc Mixilpixil event 720702 needs the same incomplete-state condition 3694, then a generic script with chat override, zero-emote clearing and relative movement options. Its credit is command 7 at 33 seconds; command 0 is speech. Both quests use normal objectives now; their authored sequences remain unsupported. No partial sequence was credited as if complete.

## Before and after behavior coverage

The generated [action comparison](behavior-parity-m4.md) compares emitted full-world plans at `928e3489` and after these fixes. “Equivalent” means all classic action keys occur; “reduced” means some occur; “lost” means none occur. GUID rules are grouped with their template and reachable relays are expanded. This is a static inference, not verified gameplay parity: timing, chance, conditions, targets and flags are outside this comparison.

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

The regenerated [full-world census](census/creature-ai.md) has 3,142 vmangos subjects, 4,917 rules, 7,155 instructions and 27 relays; 952 rules are dropped and 310 excluded. The [alliance-single census](census/creature-ai-alliance-single.md) has 605 subjects, 942 rules, 1,389 instructions and nine relays; 216 rules are dropped and 5,021 excluded. Before the fixes these were 3,056/4,798/5,330/27 and 585/917/1,039/9 respectively.

## Verification

| Check | Result on corrected code |
| --- | --- |
| Importer suite | 573 passed, 18 ignored; final summon-helper tests 20 passed |
| Core Module library with debug reducers | 1,572 passed |
| Shared, protocol, Package delta, test support | All result lines passed; one ignored protocol test |
| Build-script / Package API lint | 8 / 37 passed |
| Core workspace Clippy, all targets/features, `-D warnings` | Passed |
| Formatting and diff whitespace | Passed |
| Release Wasm with debug reducers and Package fixtures | Built |
| Full shared Native suite | All 52 passed with explicit Host and Wasm |
| Intermediate sound-default durable tests | Both tests passed on all five heads #838, #839, #840, #841 and #843 |
| Guardian cap | Durable repeated-cast test passed |
| Scripted immunity | Durable committed-state check and actual-import Native packet check passed |
| Actual Rajaxx | AQ faction and death speech/voice passed |
| Actual hunter fallback | Native acceptance, supplied rod, reward and next quest passed; rod use failed as on pre-fix Module |
| CI branch selection | Core five tests and Packages six tests passed |
| Deadmines companion | API lint 37 passed; Deadmines choreography and persistent doors passed; release Wasm built. All-Package library: 1,691 passed, two baseline failures. |

Native Host: `/home/computer/LyraCore/.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone`; the full command was `python3 native-host/check.py --host <host> --module <wasm>`. The verified Core Wasm SHA-256 is `8786d001f390547f2270760c47698b718ddae3702e0c89bbc861db5df1a5d308`. The missile test now distinguishes launch from impact and waits for damage from a successful missile. The watched-faction refusal check examines only field 1261, so an unrelated regeneration field update cannot fail it. A widened observation window reproduced the unrelated value update and passed the corrected assertion.

The corrected Core code passed the suites above; the full importer suite and release build were rerun after the final summon-helper refactor. Intermediate test receipts identify their exact tested heads; subsequent immunity documentation edits leave executable trees unchanged. Original implementation results for all eight actual Hallow's End quests, VanCleef's source encounter and existing-data migration remain historical evidence and were not repeated as part of this review-fix run. No real build-5875 client visual verification was done. No new claim of full-world capacity is made.

The all-Package library failures are `pkg_playerbots::goals::tests::a_despawn_takes_the_quest_log_and_the_corpse_with_it` and `tripwires::native_delivery_write_discipline::writes_maintain_simulation_aoi_and_native_delivery`. The supplied baseline logs reproduce both at Core `f60424db` with Packages `7aff766c`; the current feature base differs from that Core baseline only in BRIEF.md. The current scanner no longer lists the repaired dungeons fixture. Playerbots writes and its corpse assertion remain outside the changed files. All-Package Clippy and the historical addon failure were not rerun in this review-fix run; the green Clippy result above is Core-only.

## Classic byte parity and performance

| Artifact | SHA-256 |
| --- | --- |
| Full-world SQL | `fd06e44cb7a4bd3f8c529caeebc01d0f1db327448d048bb6d2e45c7a5ebaf980` |
| Alliance-single SQL | `cdd4c4461307d230da5f2c71ae859ab668798cd18fdc9ac49e9bc50f1f2be244` |
| All 467 reducer batches | `e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098` |

All three freshly generated artifacts match the required baseline.

| Release import | Final seconds | Final peak MiB | Limit seconds / MiB |
| --- | ---: | ---: | --- |
| Classic full-world | 9.52 | 530.051 | 14.375 / 587.5 |
| Classic alliance-single | 8.18 | 346.422 | 12.125 / 475 |
| vmangos creature-ai full-world | 1.37 | 34.668 | 14.375 / 587.5 |
| vmangos creature-ai alliance-single | 2.07 | 47.781 | 12.125 / 475 |

GNU `time -v` measured the final release importer after task builds finished. `/usr/bin/time` was absent, so the distribution binary was extracted under task scratch. All four final measurements pass the fixed wall-time and memory budgets. The vmangos measurements cover creature-ai, not a future all-family import.

Earlier measurements under memory pressure are retained in `performance.json`: corrected classic full-world took 35.24 s / 430.949 MiB and alliance-single 63.55 s / 346.754 MiB; the matched old binary took 60.59 s / 528.348 MiB and 72.14 s / 345.492 MiB. The machine was swapping with other builds active. Final measurements are in `performance-final.json`; the changed conditions explain why the earlier run was not used to certify the gate.

## Comment review and remaining limits

Comment review passed on the combined Core diff and companion changes, separately from fmt and tests. Changed and retained source/test/script/configuration comments were checked against the code; documentation pointers resolve. The policy-storage contract remains documented, opcode/default comments match their implementations, and the companion retains the equip_swap display-id constraint. Removed fixture narration carried no additional contract.

The 17 SEND_CHANNEL_VISUAL and 30 TRIGGER_WEAPON_PROCS spells remain held. Missing dispatchers, unsupported summon lifetime details, scripted immunities and encounter handlers remain in [unsupported.md](unsupported.md). The Map exception is acknowledged but its implementation belongs to Milestone 5, which has not started. No Package API stop rule was triggered. Exact CI results and review states are recorded per PR below; no PR was watched.

The final response and ignored `.import/m4-review/checks.md` record every check and review state at the final pushed heads. The matching Packages branch is `vmangos/s6c4-deadmines-verification-m4`, PR #89; GitHub closed #88 during its rename. Packages CI reaches the intended Core branch and its Datascripts jobs pass, but its Module jobs fail compiling playerbots test calls to the unavailable `support::gateway_binary`. Both the test calls and Core test-support source are unchanged from their feature bases. This separate compatibility failure remains unfixed; it is not the old missing GameObject/custom_flags pairing failure.
