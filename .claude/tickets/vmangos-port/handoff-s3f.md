# S3f quests handoff

Branch `vmangos/s3f-quests`, target `vmangos/s3e-trainers`. The PR head contains this report.
Source pin `464179081673cfd240f7ffe7f0daf96bca0b5a70`.

## Verified

Quest templates select patches and relation tables select ranges. The plan preserves the existing
catalogue scopes: full profiles share the full catalogue, while other canonical profiles share
the starter and instance catalogue. Giver relations use enabled spawn choices inside the selected
profile. Creature loot, chest loot and vendor stock supply collectible-item availability, after
forbidden-item filtering.

Mail uses `RewMailTemplateId`, `RewMailDelaySecs` and `RewMailMoney`. Sender zero keeps the Module's
quest-ender rule. Used mail loot passes the existing one-item, unconditional 100% validation.
Quest learn rewards use single-effect wrappers from selected vmangos spell rows. No client spell
columns are mixed in. Explicit `RewXP` supplies XP, as `Quest::XPValue` in
`src/game/QuestDef.cpp:180-201` specifies at the pin. The Module's zero-XP fallback remains a
reported limitation.

Exploration/event flags add a completion-credit requirement unless an area-trigger objective
already supplies the gate. `src/game/QuestDef.h:166-167` defines that source flag. These quests
wait for credit rather than completing without their event. Source script delivery and inferred
EventAI requirements remain for S6. No source script was executed or copied.

A typed nullable-text accessor accepts SQL NULL while retaining literal text. vmangos quest text
uses literal SQL quoting, so the text `NULL` remains distinct from an empty value. Classic-db keeps
its old nullable-text output. Negative GameObject objective IDs use unsigned absolute values to
avoid overflow on the minimum signed integer; pinned output is unchanged by that edge fix.

Named fixtures cover patch/build selection, relation endpoints, area/cast/collect/GameObject
objectives, local givers, mail delay and money, forbidden mail items, explicit XP, learn rewards,
nullable Unicode text and event-credit gates. The classic nullable-text regression also passes.

Generated full-world census:

| Module table | Classic | vmangos |
| --- | ---: | ---: |
| game_creature_quest | 7702 | 7852 |
| game_gameobject_quest | 459 | 462 |
| game_mail_loot | 74 | 72 |
| game_quest_cast_objective | 19 | 21 |
| game_quest_event_requirement | 10 | 176 |
| game_quest_objective | 3886 | 4059 |
| game_quest_reward_choice | 1400 | 1386 |
| game_quest_reward_item | 1532 | 1556 |
| game_quest_reward_mail | 77 | 73 |
| game_quest_reward_spell | 90 | 95 |
| game_quest_template | 4164 | 4253 |
| game_quest_text | 4164 | 4253 |

Alliance-single has 1542 classic and 1546 vmangos quest templates, with 2358 and 2366 creature
relations. The reports list added and removed keys and explain source catalogue, objective,
reward and mail differences. The 176 full-world event requirements follow explicit source flags;
classic-db's ten requirements follow its current EventAI plan. Alliance-single has 53 and six.
All four quest row/count floors pass. Spawned coverage remains for S4.

`unsupported.md` adds the manifest counts for quest conditions and prerequisites, scripts,
explicit zero XP, extra reputation fields, flags, unsupported reward casts and unavailable collect
objectives. Full-world includes 725 explicit zero-XP quests and 937 omitted collect slots;
alliance-single includes 228 and 415. The 176/53 event-credit findings refer to delivery still due
in S6, while the requirement itself is imported. Counts overlap and do not count distinct broken
quests. The used mail templates add no loot findings.

Classic SQL for both profiles matches S3-pre byte for byte. All 467 full-world reducer batches
match. Full-world SQL SHA-256 is
`b9cef3c0ca9acae9fa1994370d5f21fd4be0eb8faac90b27dc3457004a6baa40`; reducer JSON SHA-256 is
`e950ec5870e6d6be57e2c6b83765770318360a68354538ccef18f58d81c89098`.

Release measurements use fresh Python getrusage and monotonic-time wrappers because GNU time
is absent. All pass the limits. vmangos measures this family only.

| Source and profile | Peak RSS MiB | Wall seconds |
| --- | ---: | ---: |
| Classic full-world | 536.508 | 8.905 |
| Classic alliance-single | 349.492 | 8.006 |
| vmangos quests full-world | 124.172 | 6.677 |
| vmangos quests alliance-single | 108.145 | 7.045 |

Importer tests passed, 500 tests and 16 ignored. The separate reducer parity test passed. Importer
clippy with all targets and warnings denied, fmt and diff checks passed. No Module or script changes;
S3d's wasm and native checks carry forward.

Comment review passed over changed code, new fixtures and retained comments. SQL statement
ordering and the classic nullable-text contract remain intact. The minimum signed objective ID
now has defined behavior. No documentation pointer or contract was removed.

## Limits and next work

No stop rule triggered and no consumed quest column was missing. Imports and censuses were dry
runs, without a Realm or client session. M3 must verify spawned giver coverage. S6 must add source
script/event delivery and EventAI dependency closure, and reconcile inferred event requirements
with the explicit source flags imported here. The 16 portal conditions remain unsupported.
