# S3g gossip and conditions handoff

Branch `vmangos/s3g-gossip`, target `vmangos/s3f-quests`. The PR head contains this report.
Source pin `464179081673cfd240f7ffe7f0daf96bca0b5a70`.

## Verified

The gossip family reads vmangos broadcast text, interleaved NPC text slots, menu rows, options
and conditions by column name. Broadcast references supply male and female text. Slot probabilities,
literal `NULL` text and Unicode survive the import. Numeric raw option labels remain text unless
an explicit broadcast reference replaces them.

Creature options use selected template versions, source NPC flag masks and menu-zero fallback.
Questgiver placeholders are omitted and indexes stay dense. Payment and text-input gates are
unavailable. Close-menu sentinel -1 maps to the Module's close behavior; positive menu navigation,
POIs and source scripts remain unsupported. The reader reports eight spawn-specific greetings
and five menus with multiple unconditional greetings. It selects the first unconditional greeting
in primary-key order; dynamic greeting choices remain unsupported.

The old relay menu list does not describe vmangos. Menu 6623 exists, while 6687, 6688 and 6949 do
not. The reader carries all 2218 source menu IDs as profiles. A read-only audit of script command
84 found these 13 targets, all present: 5710, 5711, 5712, 5713, 6629, 6929, 6930, 6931, 6932, 6933,
6934, 7167 and 7228. Source script execution remains for S6. Source references are
`src/game/Maps/ScriptCommands.h:316`, `src/game/Objects/Player.cpp:11972-12021` and
`src/game/Objects/Player.cpp:12319-12363` at the pin, read through `gh api`. No source code was copied.

Decision 9 is implemented through `lyracore-shared::gossip::ConditionKind`. The importer maps source
numbers into named runtime meanings. The Module decodes owned persistent codes and evaluates the
enum. Existing stored codes remain stable for classic-db parity; no schema change or migration is
needed. Sixteen source patch predicates become constants at patch 10, with inversion preserved.
Unsupported vmangos kinds use an unavailable sentinel, including source rows with negative operands.
The classic reader retains opaque unsupported codes for byte parity. Neither NOT nor OR can turn
an unavailable predicate into permission.

Named fixtures cover reordered columns, selected creature revisions, weighted gendered slots,
explicit broadcast references, literal numeric labels, source flags, default options, payment
gates, patch comparisons, reverse flags, negative unsupported operands, quest predicates and
unavailable compound references. Durable tests now use owned kinds rather than source numbers.

## Census and unsupported behavior

| Module table | Classic full | vmangos full | Classic alliance | vmangos alliance |
| --- | ---: | ---: | ---: | ---: |
| game_gossip_condition | 1133 | 1882 | 1133 | 1882 |
| game_gossip_menu | 456 | 1168 | 146 | 396 |
| game_gossip_menu_profile | 4 | 2218 | 4 | 2218 |
| game_gossip_menu_profile_option | 2 | 2560 | 2 | 2560 |
| game_gossip_option | 2641 | 3539 | 1096 | 1095 |
| game_npc_text | 326 | 1813 | 123 | 1813 |
| game_npc_text_slot | 352 | 2322 | 135 | 2322 |

The generated reports list every added and removed entry key. Creature rows follow selected
menu links and default options. Profiles, their text and conditions use the complete source
catalogue in both profiles, which explains the larger scoped text count. Slot counts follow
resolved source broadcast references. There is no gossip-specific floor in the import manifest.
Spawned service coverage remains for S4.

The manifest and `unsupported.md` record broadcast metadata, unsupported condition kinds and
flags, conditional and spawn-specific greetings, ambiguous unconditional greetings, menu scripts,
unsupported option actions and conditions, confirmations, gendered option labels, positive menu
navigation, POIs, option scripts and profile NPC flag masks. These are source/catalogue counts,
identical for both profiles. Counts overlap. The 16 portal conditions remain unsupported because
the portal table still has no condition field.

## Parity, measurements and checks

Classic SQL for both profiles matches S3-pre byte for byte. All 467 full-world reducer batches
match. SQL SHA-256:
`b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40`.
Reducer JSON SHA-256:
`e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098`.
The temporary reducer capture test passed and was removed.

Release measurements use fresh Python `resource.getrusage(RUSAGE_CHILDREN)` and `time.monotonic`
wrappers because `/usr/bin/time` is absent. All pass the limits. vmangos measures gossip only.

| Source and profile | Peak RSS MiB | Wall seconds |
| --- | ---: | ---: |
| Classic full-world | 536.141 | 8.711 |
| Classic alliance-single | 349.875 | 7.337 |
| vmangos gossip full-world | 23.184 | 0.164 |
| vmangos gossip alliance-single | 36.602 | 0.715 |


The final importer suite passed 502 tests with 16 ignored. Shared tests passed 320; Package Delta
passed 289; protocol tests and examples passed 436; test-support passed 15. Module units passed
1546, build-script tests eight, notification checks one and Package API lint 37. Workspace clippy
with all targets/all features and warnings denied, fmt and diff checks passed. Script tests passed
16 with resource warnings denied. The debug-reducer wasm check and release wasm build passed.
All eight durable gossip tests and all 47 native protocol checks passed on disposable local Realms.
The durable suite also published the Module with test fixture support. No table layout changed.

Comment review passed over the shared enum and evaluator, importer readers, SQL statement
extraction, Module decoder, durable tests, named fixtures and census code. Retained comments
state the stored-code compatibility contract, strict Boolean failure rule, playable-mask rule
and import-owned ID ranges. The source-mapping comment and menu-catalogue comment explain the
source boundary and catalogue scope. Documentation pointers resolve. No contract was removed.

## Limits and next work

No stop rule triggered and no consumed column was missing. Data imports were dry runs. Durable
and native tests use disposable local Realms with authored fixtures, not a complete vmangos world.
No production Realm or real client was used. Performance numbers measure this family, not a full
vmangos import. The larger catalogue does not prove script behavior or client presentation.

M3 must port creatures and spells, complete the identifier audit and verify spawned service
coverage. Scoped catalogue candidates include selected spawn choices, forced entries and pets;
creature enablement and EventAI dependency closure must be reconciled in S4/S6. S6 must implement
source script dispatch and the complete Source Profile gate. S3g supplies the profile catalogue
and condition meanings they will consume. Milestone 3 was not started.
