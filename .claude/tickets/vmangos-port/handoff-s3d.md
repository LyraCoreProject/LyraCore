# S3d gameobjects handoff

Branch `vmangos/s3d-gameobjects`, target `vmangos/s3c-loot`. The PR head contains this report.
Source pin `464179081673cfd240f7ffe7f0daf96bca0b5a70`.

## Verified

The gameobjects family reads selected template patches and spawn ranges. It omits disabled
placements and translates source ACTIVE 0, READY 1 and alternative-active 2 to Module states
1, 0 and 2. Transports select build versions and use client TaxiPath and TaxiPathNode tables.
`--go-models` uses the selected World Source templates with client geometry.

The Module previously sent its closed/open values directly to the client, reversing READY and
ACTIVE. Native delivery now translates them at the packet boundary. The new packet assertion
fails against the previous Module artifact and passes against this change. It checks creation,
opening, closing, encounter activation and late entry. No schema changed.

The source rules were read in `src/game/GameObjectDefines.h:176` and
`src/game/ObjectDefines.h:127-134`, with placement behavior in `src/game/GameObject.cpp:969-999`
at the pinned SHA. No source code was copied.

All eight authored gather entries are selected type-3 templates and contain their expected primary
loot item. Their thresholds match client Lock entries, with zero-skill locks raised to 1 to require
the profession. They retain authored yields and gray thresholds. Real type-25 fishing holes remain
omitted because that number is the Module's synthetic gather type.

Generated full-world counts:

| Module table | Classic | vmangos |
| --- | ---: | ---: |
| GameObject spawns | 44087 | 55152 |
| GameObject templates | 10710 | 9344 |
| Traps | 280 | 327 |
| Collision models | 438 | 482 |
| Meeting stones | 25 | 22 |
| Transports | 9 | 9 |

Alliance-single has 11518 classic and 13636 vmangos spawns. Other catalogues are shared across
profiles. Selection removes 162 of 9530 template revisions, 13 of 56639 spawn rows and five of
14 transport revisions. After selection, 24 real fishing-hole templates are omitted. The census
lists added and removed entry IDs and explains catalogue, placement and mesh differences.
The model builder resolves 482 of 640 selected door/button templates, with 158 empty meshes and
no missing displays or WMO models. Both census profiles meet the gameobject map floors, including
37 classic and 75 vmangos Deadmines placements. The script's gather floor counts all GameObjects;
the report also lists actual authored gather nodes, 6602 classic and 5635 vmangos full-world.

The manifest and `unsupported.md` add animation, fishing-hole, event schedule, authored gather loot,
pool membership, respawn range, script, spawn flag, summon-only and visibility findings. The counts
are source placements or membership rows before the fishing-hole omission, and whole-catalogue
template counts. The 16 portal conditions remain unsupported.

Classic full-world and alliance-single SQL match S3-pre byte for byte. All 467 full-world reducer
batches match. Baseline SQL SHA-256 remains
`b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40`; reducer JSON SHA-256 remains
`e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098`.

Release measurements use fresh Python getrusage and monotonic-time wrappers because GNU time
is absent. All pass the performance limits. vmangos measures this family only.

| Source and profile | Peak RSS MiB | Wall seconds |
| --- | ---: | ---: |
| Classic full-world | 534.879 | 8.206 |
| Classic alliance-single | 349.902 | 7.030 |
| vmangos gameobjects full-world | 54.730 | 1.704 |
| vmangos gameobjects alliance-single | 43.957 | 1.549 |

Importer tests: 494 passed, 16 ignored. Module unit tests: 1546 passed. Build-script tests: eight
passed; Package API lint: 37 passed. Workspace clippy with all targets/all features and warnings
denied, importer clippy after final edits, fmt, diff checks, debug-reducer wasm check and release
wasm build passed. The separate reducer comparison passed. All 47 native protocol checks passed on a disposable local Realm.

Comment review passed across edited source, scripts, tests and the named fixtures, including
retained contracts. The Module comment explains the two state numberings; packet-parser comments
identify fixed protocol fields. The quaternion and type-25 collision contracts remain intact.
No documentation pointer was removed.

## Limits and next work

No stop rule triggered. No consumed gameobject column was missing. Only disposable local Realms
were used for protocol checks; no production Realm or real client was used. Collision model
counts use client geometry, but no visual collision test was run. The source import was a dry run.

Source spawn IDs reach 3998642. The GameObject-specific fixture and Package guards use their own
bands, and GUID packing preserves the source spawn ID. This is a preliminary read of those code
paths, not S4's complete identifier audit. M3 still owns that audit and creature coverage. Event
schedules and pools need later implementation; their source counts are now visible.
