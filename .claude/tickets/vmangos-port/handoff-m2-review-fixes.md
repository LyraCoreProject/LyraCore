# Milestone 2 review-fix handoff

All twelve findings are addressed. The eight draft slices remain in order on `feat/vmangos-world-data`, rebased onto the brief update `0b6361c8`. Each branch was pushed with `--force-with-lease`. No merge, main-branch change, upstream post, deployed Realm or Milestone 3 work occurred.

The final review compares the original M2 head `4eb090e4` with the repaired stack. vmangos source observations use commit `464179081673cfd240f7ffe7f0daf96bca0b5a70`. The local SQLite SHA-256 is `a8ce8ecc8e9c9d00dfe23321d0763cf8aee19677c836f1e1b9acc15fb1fbd5c9`. No downloaded data or emulator code is committed.

## Findings and owner commits

| Finding | Slice | Commit | Change and evidence |
| --- | --- | --- | --- |
| 1 | S3e / #814 | `85c43612dc9c8e2760a7b8df0d13a9dea9fcd8bc` | Profession purchases grant the resolved learned spell and the skill cap. Durable tests cover all four Alchemy, Fishing and Mining tiers and both classic offer orders. |
| 2 | S3c / #812 | `cc67a2b5837b8568b14175b66f0cd1d072cc11f6` | Referenced group-zero children remain independent, scaled by the reference chance. Outcome tests cover a guaranteed herb with an independent bonus. |
| 3 | S3c / #812 | `cc67a2b5837b8568b14175b66f0cd1d072cc11f6` | Reference maxcount repeats the selected group or whole template, including nested references. Each repeated group gets its own roll. The reference-repeat unsupported entry is removed. |
| 4 | S3c / #812 | `cc67a2b5837b8568b14175b66f0cd1d072cc11f6` | Largest-remainder allocation preserves each group share. The 631-member fixture proves that every item can drop and all chances sum to one. |
| 5 | S3b / #811 | `cf31dc6b8606f3e024ec30ee73a5ce46839b2661` | Static WOW_PATCH and fully constant AND/OR/NOT trees filter loot and vendor rows before expansion. True conditions clear to zero. Runtime predicates remain unsupported. The negative runtime-operand follow-up is 5bab5472ab18d94f16d84b1b4e06ce616a327a9a. |
| 6 | S3g / #816 | `486550a9faf1425176eff6afbb0dca64308c5f2a` | Native gossip follows submenu targets and filters their options through current conditions. A stored root menu rejects stale clicks after a Runtime Script menu change. Wire tests cover nested/back navigation and the talent-reset confirmation. Classic inline reset still works. |
| 7 | S3g / #816 | `486550a9faf1425176eff6afbb0dca64308c5f2a` | Talent reset requires level 10 and a class trainer of the Character's class. Durable and wire checks hide or refuse it for profession, riding, pet and other-class trainers. |
| 8 | S3g / #816 | `486550a9faf1425176eff6afbb0dca64308c5f2a` | The stale navigation comment is removed. Positive action-1 targets now open imported profiles. The pinned-data audit found profiles for all 913 such targets. |
| 9 | S3d / #813 | `bd31cddd1fbc681437e42864f2732acc033d9243` | Source ACTIVE chest placements start lootable. Doors keep their separate state mapping. The durable fixture opens an imported chest and receives one item. Follow-up b899b3e95147552b8f4df88f4b3188b1c8404368 preserves unused negative Goober fields and corrects fixture initialization. |
| 10 | S3g / #816 | `486550a9faf1425176eff6afbb0dca64308c5f2a` | The last unconditional greeting wins. Conditional-only menus use source default 0xffffff. Importer fixtures check the selected greetings; the native submenu test checks both greeting IDs on the wire. Dynamic greeting selection remains unsupported. |
| 11 | S3a, S3b, S3c, S3d, S3f | `cf208669a026447034cfd6eaed43f3c1e6d3cc37` | Checked row.number readers replace silent numeric defaults on the ported vmangos paths. Owner commits are listed below. Invalid creation spells, item/vendor values, loot groups, GameObject values and quest rewards fail before emitting broken gameplay rows. |
| 12 | S3b / #811 | `cf31dc6b8606f3e024ec30ee73a5ce46839b2661` | Removed the unused vmangos drink classifier and emits restores_power=false. Item spell IDs remain intact. Module drink behavior already reads spell effects. Durable test eefd72ea10a230c65be0a2ff0d8ab067f05ac2b0 proves mana restoration, rage refusal and item consumption without the flag. |

Numeric-reader changes are owned by S3a `cf208669a026447034cfd6eaed43f3c1e6d3cc37`, S3b `cf31dc6b8606f3e024ec30ee73a5ce46839b2661`, S3c `cc67a2b5837b8568b14175b66f0cd1d072cc11f6`, S3d `bd31cddd1fbc681437e42864f2732acc033d9243` and `b899b3e95147552b8f4df88f4b3188b1c8404368`, and S3f `9880be4f8a25f2376aa9e3f361fe5aab7de69c0b`. Existing classic-only and unported paths retain their old readers.

The final production plan omits creature 10819/item 16999, Thulbek 5814/item 2511 and Evie Whirlbrew 11188/item 14526. Reference 30254 no longer contributes items 13148 or 13164 to creature 9568. At this pin, the obsolete item 13178 row belongs to reference 30248, whose creature 9237 output also omits it. Its patch-8-through-10 row in reference 30297 remains valid.

## Loot odds

[The generated comparison](census/loot-odds-review.md) contains every named herb, boss and chest, example-item chances and expected total counts. It compares the original M2 output with the final production plans. These are calculated Module distributions with uniform basis-point rolls. Expected totals include quest-only drops and assume the Character is eligible and item templates and properties are available. They are not observed random samples or a full vmangos Realm conformance test.

Golden Sansam 13945, Dreamfoil 13946/17203 and Mountain Silversage 13947/17204 rise from 50% to 100% for their primary herb. Sungrass 6151/13965/17201 stays at 100%, while its bonus rises from 0% to 50%. At this pin that bonus is Fel Creep 11514 with runtime condition 80, not Swiftthistle 2452. That runtime condition remains unsupported. Purple Lotus 17200/6142 rises from 90.4% to 100%; Wildvine stays at 9.6%, and Wildvine without Lotus falls from 9.6% to 0%.

Chest 2283 group 1 changes from 631 members totaling 10096 bp with six unreachable members to exactly 10000 bp with none unreachable. It has 535 members at 16 bp and 96 at 15 bp.

| Boss or chest | Example item | Chance before → after | Expected total item count before → after |
| --- | --- | ---: | ---: |
| Ragnaros | Legplates of Wrath | 23.4375% → 41.3818% | 11.8800 → 13.8800 |
| Nefarian | Breastplate of Wrath | 11.1100% → 21.0035% | 9.0086 → 10.0120 |
| Colossus of Zora | Hand of Edward the Odd | 33.3300% → 86.8379% | 4.9998 → 15.0000 |
| Ras Frostwhisper | Shadowy Mail Greaves | 8.3300% → 15.9844% | 3.7996 → 4.8000 |
| Lokholar | Frozen Steel Vambraces | 19.7400% → 35.5833% | 2.1040 → 3.1050 |
| Ivus | Frozen Steel Vambraces | 19.7400% → 35.5833% | 2.4323 → 3.4328 |
| Dathrohan/Balnazzar | Wyrmtongue Shoulders | 10.0000% → 19.0000% | 2.9337 → 3.9337 |
| Four Horsemen Chest | Desecrated Robe | 33.3300% → 55.5644% | 2.2999 → 3.3000 |
| Knot Thimblejack's Cache | Thick Leather | 7.6900% → 14.8071% | 9.6205 → 17.5476 |
| Magenta Cap Clusters | Magenta Fungus Cap | 100% → 100% | 2.0000 → 3.0000 |

## Classic parity

All three artifacts match the retained baseline byte for byte. No classic row changes need an exception.

| Artifact | SHA-256 |
| --- | --- |
| Full-world SQL | `b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40` |
| All 467 full-world reducer batches | `e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098` |
| Alliance-single SQL | `f1e070079f8c984f7c124cadcd49eceb657428d3b551164290b6018079644d92` |

Reducer capture used temporary test instrumentation, removed before handoff. The classic two-row profession purchase path also passed a durable test; output hashes alone do not prove that runtime behavior.

## Performance and censuses

Fresh release processes ran sequentially with getrusage peak RSS and monotonic wall time. GNU time was unavailable. All 16 runs pass the existing full-world limit of 587.5 MiB / 14.375 seconds and alliance-single limit of 475 MiB / 12.125 seconds. vmangos rows measure individual Import Families, not a complete world import.

| Source/family | Full-world MiB / seconds | Alliance-single MiB / seconds |
| --- | ---: | ---: |
| classic-db complete | 534.594 / 8.902 | 349.320 / 7.835 |
| vmangos globals | 21.277 / 0.070 | 20.613 / 0.072 |
| vmangos items | 283.648 / 5.966 | 139.871 / 5.810 |
| vmangos loot | 62.309 / 1.050 | 62.230 / 1.583 |
| vmangos gameobjects | 55.086 / 1.647 | 43.852 / 1.525 |
| vmangos trainers | 26.777 / 0.463 | 36.750 / 1.506 |
| vmangos quests | 102.578 / 6.770 | 82.852 / 7.083 |
| vmangos gossip | 23.695 / 0.166 | 36.609 / 0.694 |

All seven family censuses were regenerated for both profiles with the pinned source SHA. Items and loot reports reflect static filtering and repeated references. Gameobject reports describe lootable ACTIVE chests. Gossip reports now include 1367 full-world entry greetings and 491 Alliance entry greetings, plus the last-unconditional/default behavior. Globals, trainers and quests match their existing reports. The extra Alliance globals run is retained locally because globals are profile-independent.

## Verification

| Check | Result |
| --- | --- |
| Importer | 511 passed, 17 ignored; all affected ignored tests ran separately |
| Shared | 320 passed |
| Module lib with debug_reducers | 1546 passed |
| Package Delta | 289 passed across its targets |
| Protocol | 436 passed, 1 ignored |
| Test support | 15 passed |
| Module build script | 8 passed |
| Notification tables | 1 passed |
| Package API lint | 37 passed |
| Fetch script | 16 passed |
| Workspace clippy, all targets/features, -D warnings | Passed after final source edits |
| cargo fmt --all -- --check | Passed |
| Wasm check with debug_reducers | Passed |
| Release Wasm with debug_reducers and package_test_fixture | Built and used by local tests |
| Native protocol suite | All 48 checks passed |
| Native crash recovery / slow-reader isolation | Both passed |
| Durable graveyard | 1 passed |
| Durable gossip | 8 passed |
| Durable profession trainers | 2 passed |
| Durable imported chest | 1 passed |
| Durable drink behavior | 1 passed |
| Additive migration from original M2 Module | Passed with an existing Character and reconnect |

The first chest durable attempt collided with seeded GameObjects. The fixture now replaces the first batch and appends later batches, matching the normal importer path. The corrected test passed twice, including after the final Goober-field change. No unresolved test failure remains.

The new `game_native_player.gossip_root_menu_id` column is end-appended with `#[default(0u32)]`. The migration preflight contained that column addition only. The local HTTP publish used the CLI-equivalent BreakClients migration policy and its preflight token, without a data wipe. SpacetimeDB disconnected the client for the schema update. The Character survived and reconnect succeeded. No stored enum variant, reducer argument or Host delivery layout changed.

The native suite uses protocol clients, not a real build-5875 game client. No real-client session or complete vmangos world import was performed. Applying these fixture results to the full source world remains an inference until S4 and S6 complete the port and conformance work.

## Comment review

Passed on every slice and on the combined final diff against `4eb090e4`, including new source and tests, the native Python script, generated report text and retained comments around changed code. The trusted-menu caller contract, filtered-option ordering, classic-only zero-chance rule and probability units remain accurate. The stale navigation claim was removed without losing an interface contract. No required tool annotation, safety constraint or documentation pointer was lost. Local documentation pointers resolve.

[unsupported.md](unsupported.md) still lists the small-chance clamp, fishing/skinning quest-only flags and all 176 event-credit quests assigned to S6. Runtime loot/vendor conditions, dynamic greetings, source scripts and profile NPC flag masks also remain reported. No Milestone 3 work was started.

Local evidence is preserved under the ignored `.import/vmangos-m2-review/` directory in this checkout. It contains the final logs, performance JSON, parity artifacts, generated censuses, pinned source reads and odds/migration scripts. Downloaded data and generated SQL remain local. Scratch build/test output was removed.

Made with GPT-6 Astra in Codex through T3 Code.
