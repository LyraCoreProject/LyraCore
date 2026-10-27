# S3e trainers handoff

Branch `vmangos/s3e-trainers`, target `vmangos/s3d-gameobjects`. The PR head contains this report.
Source pin `464179081673cfd240f7ffe7f0daf96bca0b5a70`.

## Verified

Trainer offers select build ranges and join `npc_trainer_template` through selected creature
`trainer_id` values. Shared offers take precedence over duplicate direct spells. Only templates
with the trainer flag receive offers. Zero required level falls back to the selected wrapper's
spell level. The rules follow `LoadTrainers` in `src/game/ObjectMgr.cpp:10600-10744` at the pin.

The import keeps source teach-spell wrappers and derives skill grants from their skill-step
effects. Constant steps 1 through 4 grant 75 through 300 ranks, following
`src/game/Spells/SpellEffects.cpp:2959-2975`. No upstream code was copied. No client spell fields
are mixed into source rows, and vmangos receives no synthesized profession offers.

The full-world audit compares 42 `PROFESSION_LEARN` targets with 51 offered source skill targets.
It keeps 39 of the authored targets, adds 12 source targets and omits three. The omissions are
Expert Cooking 3413, Expert Fishing 7732 and Expert First Aid 7924. Selected item rows 16072,
16083 and 16084 teach those targets through spells 19886, 19889 and 19903. The additions include
Artisan gathering and riding targets. Seven old racial riding offers are reported unsupported
because the Module uses skill 762 for mount gates.

The named fixture covers a selected creature patch, shared-list precedence, inclusive build
endpoints, a future spell revision, an expired offer, the trainer flag, source level fallback,
a 150-rank grant and a multi-target finding.

| Profile | Classic offers | vmangos offers | Classic skill grants | vmangos skill grants |
| --- | ---: | ---: | ---: | ---: |
| full-world | 28216 | 34484 | 907 | 564 |
| alliance-single | 13723 | 13753 | 369 | 224 |

Generated censuses list 61 added and nine removed trainer entries in full-world, and two removed
entries in alliance-single. Build selection keeps 4671 of 4676 direct rows and 1382 of 2404 shared
rows. Offer differences follow those selected lists and their joins. Skill-grant differences follow
the source tier availability and removal of the classic-db synthetic list. Both offer floors pass;
spawned trainer coverage remains for S4.

The Compatibility Manifest and `unsupported.md` add skill/rank prerequisites, 4879 full-world
and 1839 alliance-single offers; multiple learn targets, 14 and 10; and seven old racial riding
offers in full-world. No missing or non-learning spell appears at this pin. The 16 portal conditions
remain unsupported.

Classic full-world and alliance-single SQL match S3-pre byte for byte. All 467 full-world reducer
batches match. The SQL SHA-256 is
`b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40`; reducer JSON SHA-256 is
`e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098`.

Release measurements use fresh Python getrusage and monotonic-time wrappers because GNU time
is absent. All pass the limits. vmangos measures this family only.

| Source and profile | Peak RSS MiB | Wall seconds |
| --- | ---: | ---: |
| Classic full-world | 535.922 | 8.985 |
| Classic alliance-single | 349.387 | 8.002 |
| vmangos trainers full-world | 26.566 | 0.521 |
| vmangos trainers alliance-single | 36.691 | 0.990 |

Importer tests passed, 495 tests and 16 ignored. The separate reducer comparison passed. Importer
clippy with all targets and warnings denied, fmt and diff checks passed. No Module or script changes;
the preceding slice's wasm and native checks carry forward.

Comment review passed over the final source, fixtures, census changes and retained comments.
The single new comment states the source skill-step contract and points to its documented source.
No contract or documentation pointer was removed.

## Limits and next work

No stop rule triggered and no consumed trainer column was missing. Source imports were dry runs;
no Realm or real client was used for this slice. M3 must supply the selected creature trainer
metadata and verify spawned trainer coverage. Missing/non-learning spells produce findings and
are omitted. Nonconstant or ambiguous skill-step grants return an error. The full Source Profile
gate remains for S6c.
