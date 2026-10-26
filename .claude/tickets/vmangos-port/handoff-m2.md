# Milestone 2 handoff

Milestone 2 is complete through S3g. Milestone 3 was not started. The stack contains eight draft
PRs. None was merged, and none targets main. No production Realm was touched. No vmangos data or
emulator code is committed. Source reads used the SQLite snapshot and GitHub contents API at
`464179081673cfd240f7ffe7f0daf96bca0b5a70`.

## Stack

| Slice | PR | Branch | Target | Head | Change |
| --- | --- | --- | --- | --- | --- |
| S3-pre | [#809](https://github.com/LyraCoreProject/LyraCore/pull/809) | `vmangos/s3-pre-reader-hardening` | `feat/vmangos-world-data` | `dbcb15f854d436e43a735e95b2095673cf563045` | Reader errors, fixture contracts, UTF-8 and fetch-lock recovery |
| S3a | [#810](https://github.com/LyraCoreProject/LyraCore/pull/810) | `vmangos/s3a-globals` | `vmangos/s3-pre-reader-hardening` | `52e2df6105570542c08ee460f82e71c1e22c0946` | Globals and area-before-zone graveyards |
| S3b | [#811](https://github.com/LyraCoreProject/LyraCore/pull/811) | `vmangos/s3b-items` | `vmangos/s3a-globals` | `ce2665b60b082adefd06732323ddfbd300638e2f` | Items, vendors, drinks and property weights |
| S3c | [#812](https://github.com/LyraCoreProject/LyraCore/pull/812) | `vmangos/s3c-loot` | `vmangos/s3b-items` | `b60e4b28e1b9cf4ae2c0bb495796bb9d08b18270` | Loot groups, references and forbidden items |
| S3d | [#813](https://github.com/LyraCoreProject/LyraCore/pull/813) | `vmangos/s3d-gameobjects` | `vmangos/s3c-loot` | `0a9613c5a7955191b330647f9f7d9f0aa59bf188` | GameObjects, transports, models and client state mapping |
| S3e | [#814](https://github.com/LyraCoreProject/LyraCore/pull/814) | `vmangos/s3e-trainers` | `vmangos/s3d-gameobjects` | `51fd4ced034435870c9fc743df62f1d00466c91f` | Trainer lists and source skill steps |
| S3f | [#815](https://github.com/LyraCoreProject/LyraCore/pull/815) | `vmangos/s3f-quests` | `vmangos/s3e-trainers` | `65d5d4c250440459445c0af21fbe661cadd911ac` | Quests, reward mail and explicit event gates |
| S3g | This PR | `vmangos/s3g-gossip` | `vmangos/s3f-quests` | `This PR head` | Gossip catalogues and owned condition kinds |

Each slice has its own report: [reader hardening](handoff-s3-pre.md), [globals](handoff-s3a.md),
[items](handoff-s3b.md), [loot](handoff-s3c.md), [gameobjects](handoff-s3d.md),
[trainers](handoff-s3e.md), [quests](handoff-s3f.md) and [gossip](handoff-s3g.md).
Those reports contain per-slice commands, row changes, unsupported findings and comment reviews.
The final reply supplies the S3g SHA and a single read of PR check/review state. No PR watcher ran.

## Verified baseline and reader repairs

S3-pre makes `parse_table` and `field` return errors through family builders, gives fixtures the
production numeric checks, moves nine test-only quest constants out of production, removes
`parse_complete_table` and the duplicate classic reader, and permits explicit lock replacement
when GitHub cannot reach the old lock. Fetch digest and migration checks remain in force.

UTF-8 decoding changes 19 cells in `fishing_loot_template.comments`, reducing their stored UTF-8
size by 66 bytes. The exact row keys are in the reader report. These comments are not emitted,
so no SQL or reducer row changes. Two rebuilt baseline runs match each other and M1.

The ignored Alliance failure was a stale expectation after population alternatives were widened
in `89c2663f`: seven additional talk rules make 307 emote dependencies, and one more action-56 rule
makes 15. The repaired pinned test passes. The reader report names the extra rules and evidence.

All eight slices match these rebuilt artifacts. Full-world SQL and all 467 reducer batches were
compared for each slice; alliance-single SQL was also compared for every slice.

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| Full-world SQL | 71151072 | `b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40` |
| Full-world reducer JSON | 9718119 | `e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098` |
| Alliance-single SQL | 19488045 | `f1e070079f8c984f7c124cadcd49eceb657428d3b551164290b6018079644d92` |

Local evidence remains in ignored `.import/vmangos-m2/`, including baseline bytes, per-slice
outputs, measurements, test logs, UTF-8 row impact and aggregate hash verification JSON. Temporary
reducer export instrumentation was removed. Scratch checkouts, builds and source downloads under
`$HOME/.cache/lyracore-scratch/vmangos-port/` were removed after validation.

## Census and compatibility

The [census directory](census/) contains production-plan reports, added/removed entry keys and
one-line explanations. Each family has both profile reports, except globals whose rows are
profile-independent. Its two profile measurements emit the same SQL.

| Family | Full-world classic to vmangos | Explanation |
| --- | --- | --- |
| Globals | Graveyards 191 to 189; portals 103 to 115; create actions 215 to 214 | Patch/build selection and source links differ. All 40 added and 42 removed graveyard links, 24 faction changes, 13 added and one removed portal, and 100 name-only portal changes are explained individually. |
| Items | Templates 17718 to 17707; vendors 13113 to 13653; property members 27892 to 27686 | Selected catalogue membership, direct/shared vendor joins, forbidden items and source property weights differ. VM drink flags follow selected mana-regeneration auras. |
| Loot | Fishing 1198 to 2484; chest 4792 to 13834; pickpocket 6910 to 6911; skinning 2799 to 2968 | Source drops, reference expansion and equal shares for zero-chance groups differ. The final items census includes 1595123 creature drops after the loot fix, compared with 1316655 classic. |
| GameObjects | Spawns 44087 to 55152; templates 10710 to 9344; models 438 to 482; transports 9 to 9 | Source placements, patch selection, disabled flags, omitted type-25 fishing holes and empty client meshes explain changes. The Module maps internal open/closed state to client ACTIVE/READY correctly. |
| Trainers | Offers 28216 to 34484; skill grants 907 to 564 | Direct/shared source offers and source skill-step wrappers replace the synthetic profession list. Three absent authored targets are book-taught at the pin. |
| Quests | Templates 4164 to 4253; objectives 3886 to 4059; creature relations 7702 to 7852; event requirements 10 to 176 | Catalogue membership, collectible availability, rewards and explicit source event flags differ. All four quest count floors pass. |
| Gossip | Creature greetings 456 to 1168; options 2641 to 3539; profiles 4 to 2218; conditions 1133 to 1882 | Source menu links/default options and the complete profile catalogue explain the increase. Named owned predicates replace raw source kinds in runtime evaluation. |

Alliance-single has 1546 vmangos quest templates, 13636 GameObject spawns, 13753 trainer offers,
396 creature greetings and 1095 gossip options. All consumed count floors that this milestone can
prove pass. Floors requiring spawned creature coverage remain for S4. The gather-node script's
floor counts all GameObject spawns; the census separately reports actual gather nodes.

[unsupported.md](unsupported.md) contains all emitted Compatibility Manifest categories and exact
counts. Additions cover vendor conditions/flags/restock; loot conditions, count ranges and
reference limits; GameObject events/pools/respawn/script metadata and fishing holes; trainer
skill/rank and multiple-target limits; quest prerequisites, scripts, flags, reputation, zero-XP
fallback and unavailable collectible targets; gossip conditions, greetings, scripts, navigation,
POIs, text metadata and profile NPC flag masks. Counts overlap. The 16 condition-gated portals
remain emitted and unsupported throughout the stack.

## Release performance

Each invocation used a fresh Python getrusage/monotonic wrapper because `/usr/bin/time` is absent.
Each cell is peak RSS in MiB / wall time in seconds. Classic measures the complete import;
vmangos measures the named family only. Full-world limits are 587.5 MiB / 14.375 s and
alliance-single limits are 475 MiB / 12.125 s. Every recorded run is within those limits.

| Slice | Classic full | Classic alliance | vmangos full | vmangos alliance |
| --- | ---: | ---: | ---: | ---: |
| S3-pre | 535.246 / 8.502 | 350.523 / 7.245 | not yet ported | not yet ported |
| S3a | 534.336 / 8.304 | 348.891 / 7.306 | 18.980 / 0.069 | 19.117 / 0.069 |
| S3b | 536.922 / 8.306 | 349.145 / 7.412 | 284.676 / 5.367 | 152.277 / 5.277 |
| S3c | 534.551 / 8.992 | 347.910 / 7.989 | 59.926 / 0.935 | 61.465 / 1.458 |
| S3d | 534.879 / 8.206 | 349.902 / 7.030 | 54.730 / 1.704 | 43.957 / 1.549 |
| S3e | 535.922 / 8.985 | 349.387 / 8.002 | 26.566 / 0.521 | 36.691 / 0.990 |
| S3f | 536.508 / 8.905 | 349.492 / 8.006 | 124.172 / 6.677 | 108.145 / 7.045 |
| S3g | 536.141 / 8.711 | 349.875 / 7.337 | 23.184 / 0.164 | 36.602 / 0.715 |

## Validation and comment review

Importer suites passed at each slice: 483, 485, 489, 491, 494, 495, 500 and 502 tests respectively,
with 16 ignored real-source tests. The pinned Alliance test ran separately in S3-pre and passed.
Every slice passed fmt, diff checks and applicable clippy. The final workspace clippy includes all
targets and features with warnings denied.

Final unit coverage also passed: Module 1546, shared 320, Package Delta 289, protocol 436 including
its examples/integration target, and test-support 15. Module build-script tests passed eight,
notification-table checks one and Package API lint 37. Script unit tests passed 16 with resource
warnings denied. Debug-reducer wasm checks and release wasm builds passed for Module changes.

S3a's graveyard durable test passed on a disposable local Realm. S3g's eight gossip durable tests
passed. S3d's 47 native protocol checks passed; S3g repeated all 47 successfully. No schema or native reducer interface
changed. No vmangos world was applied to a Realm.

Comment review passed per slice and for the combined diff. Reviewed source, tests, scripts and
configuration retain the numeric units, SQL NULL contract, reducer ordering, identifier ranges,
GameObject state mapping, strict condition failure rule and stable stored-code contract where
needed. Documentation pointers resolve. New fixture strings are invented test data. No source
attribution, tool annotation or safety contract was removed.

## Verified limits and next milestone

No brief stop rule triggered. No consumed vmangos column was missing. No identifier collision was
claimed resolved without the S4 audit. No Package API tag, main update or merge was attempted.

M3 must port creatures and spells, finish spawn ID/Package range auditing, and verify spawned
vendor, trainer, questgiver and loot coverage. Some scoped catalogue candidates include disabled
spawn choices until creature enablement is reconciled in S4; full profiles intentionally carry
all selected templates. S6 must supply EventAI dependency closure, script/event delivery and the
approved-value Source Profile gate. Existing quest event requirements must be reconciled with
script-derived requirements. The 13 vmangos script menu targets are already in the gossip catalogue.

Performance of a complete vmangos import, real-client presentation and production behavior remain
unverified. Family plans and local fixture tests do not establish those results. The preliminary
GameObject ID-range reading in S3d is an inference; the complete audit belongs to S4. Unsupported
behavior is recorded for later gameplay work rather than treated as implemented.
