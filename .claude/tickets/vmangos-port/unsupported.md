# Unsupported vmangos behavior

This list covers the ported Import Families. The family imports and generated censuses report
these Compatibility Manifest findings. The complete vmangos Source Profile gate is due in S6c.

| Finding | Selected rows | Current limit | Next slice |
| --- | ---: | --- | --- |
| `globals.portal-condition` | 16 | `areatrigger_teleport.required_condition` is nonzero, but the Module's portal catalogue has no condition field. The current classic-db import also omits portal conditions. The globals import retains these rows and reports their trigger ids as unsupported. | S3g translates condition kinds. Portal gameplay support remains unimplemented. |

Verified against vmangos commit `464179081673cfd240f7ffe7f0daf96bca0b5a70`, selected for patch 10,
and the current `build_areatrigger_teleport_sql` output columns. No source rows are included here.

Items findings count emitted offers, keyed by creature and item. Full-world has 403 runtime-conditioned
offers, 208 offers with item flags, and 951 offers with a restock interval. Alliance-single has
21, 36 and 276 respectively. The Module stores `max_count`, but no restock interval, source item
flags or offer condition. The items import retains the offers and reports these fields as
`items.vendor-condition`, `items.vendor-item-flags` and `items.vendor-restock`. The generated
items censuses include these counts. The loot audit below includes source conditions. The 16 portal conditions above remain unsupported.

The loot audit covers all nine selected source tables, before profile-specific owner filtering.
These counts are source rows after patch and static-condition selection.
The items manifest also reports findings from creature and reference loot. Quest mail uses the
single-item, unconditional validation added in S3f.

| Finding | Occurrences | Current limit |
| --- | ---: | --- |
| `loot.condition` | 4085 | The loot tables have no runtime condition field. Static WOW_PATCH conditions are applied before expansion. |
| `loot.count-range` | 5960 | The current import stores the maximum count. |
| `loot.quest-only` | 35 | Fishing and skinning tables have no quest-only flag. |
| `loot.unconsumed-table` | 2741 | Item-container and disenchant source tables have no world import path. |

The generated loot censuses break these findings down by source table. Item-container loot has
2,660 selected rows and disenchant loot has 81. The 16 portal conditions remain unsupported.

The pre-existing small-chance clamp remains. A positive independent chance below one basis point
can emit one basis point. Fishing and skinning quest-only flags remain unsupported.

Gameobject findings count selected enabled source placements inside the profile, before the
fishing-hole omission. Template findings count the complete selected catalogue. Event and pool
counts are membership rows joined to those placements. Placement timing and population selection
are not implemented by this family import.

| Finding | Full-world | Alliance-single | Current limit |
| --- | ---: | ---: | --- |
| `gameobjects.animation` | 4325 | 1414 | The catalogue does not carry source animation progress. |
| `gameobjects.fishinghole` | 24 | 24 | Real type 25 conflicts with the Module's synthetic gather type and is omitted. |
| `gameobjects.game-event` | 12102 | 4844 | Event schedules do not control these placements. |
| `gameobjects.gather-loot` | 8 | 8 | Authored gather nodes keep their fixed primary item. |
| `gameobjects.pool` | 29465 | 5874 | Pool membership does not select these placements. |
| `gameobjects.respawn-range` | 10903 | 2054 | Source spawn respawn ranges are not represented. |
| `gameobjects.script` | 207 | 207 | Source script names have no importer behavior. |
| `gameobjects.spawn-flags` | 440 | 123 | Active, random and dynamic flags are not represented. Disabled spawns are omitted. |
| `gameobjects.summoned-spawn` | 467 | 52 | Negative respawn times do not restrict placement to a summon. |
| `gameobjects.visibility` | 4 | 0 | Per-spawn visibility distance is not represented. |

The 16 portal conditions remain unsupported. Gameobject source states are translated; they are
not an unsupported finding.

Trainer findings count emitted offers after shared-list precedence and profile filtering.

| Finding | Full-world | Alliance-single | Current limit |
| --- | ---: | ---: | --- |
| `trainers.skill-requirement` | 4879 | 1839 | Trainer offers have no required skill/rank columns. |
| `trainers.multiple-learn-targets` | 14 | 10 | The Module grants the first learn target. |
| `trainers.racial-riding` | 7 | 0 | The Module's mount gates use skill 762, not the older racial riding lines. |

The import retains source skill lines and marks the older riding behavior unsupported. Missing or
non-learning source spells would be omitted with findings; neither occurs at this pin. The 16
portal conditions remain unsupported.

Quest findings count emitted quest templates, except unavailable collect objectives, which count
objective slots. Counts overlap. Flags count nonzero raw fields; they do not mean every behavior
of each counted quest is absent. Used mail loot passes validation and adds no loot findings.

| Finding | Full-world | Alliance-single | Current limit |
| --- | ---: | ---: | --- |
| `quests.breadcrumb` | 23 | 9 | Breadcrumb prerequisites are not represented. |
| `quests.complete-script` | 252 | 96 | Completion script execution awaits S6. |
| `quests.condition` | 76 | 35 | Quest acceptance has no imported condition reference. |
| `quests.event-credit` | 176 | 53 | The completion gate is imported; source event delivery awaits S6. |
| `quests.exclusive-group` | 427 | 151 | Exclusive quest groups are not represented. |
| `quests.explicit-zero-xp` | 725 | 228 | The Module treats zero as a fallback request and can grant XP. |
| `quests.extra-reputation-rewards` | 37 | 6 | Only the first two reputation reward slots are stored. |
| `quests.flags` | 3048 | 1034 | Raw QuestFlags and SpecialFlags beyond repeatable/event are not carried. |
| `quests.maximum-level` | 89 | 22 | Maximum acceptance levels below 60 are not represented. |
| `quests.maximum-reputation` | 38 | 22 | Maximum reputation gates are not represented. |
| `quests.money-requirement` | 69 | 15 | Negative reward money does not charge the quest cost. |
| `quests.negative-prerequisite` | 44 | 16 | Negative prerequisite semantics are not represented. |
| `quests.reputation-requirement` | 264 | 16 | Minimum reputation gates are not represented. |
| `quests.reputation-spillover-mask` | 723 | 338 | Per-quest reputation spillover masks are not represented. |
| `quests.reward-cast` | 202 | 72 | Only a single-effect learn wrapper produces a reward spell. |
| `quests.skill-requirement` | 143 | 51 | Required skill/rank gates are not represented. |
| `quests.start-script` | 103 | 39 | Acceptance script execution awaits S6. |
| `quests.start-spell` | 4 | 2 | Acceptance spell casts are not represented. |
| `quests.unavailable-collect-objective` | 937 | 415 | Objectives for items absent from the current catalogue availability set are omitted. |

The 16 portal conditions remain unsupported. Source event gates keep quests pending while their
script delivery is still being ported.

Gossip findings count the complete source catalogue in both profiles. Condition graphs and menu
profiles are shared by scoped and full imports. The broadcast metadata count covers all broadcast
rows; option-gender counts distinct broadcast references. Other option findings count menu/option
keys, greeting findings count menu/text keys, and condition findings count condition IDs. Counts
overlap and do not count distinct broken interactions.

| Finding | Occurrences in each profile | Current limit |
| --- | ---: | --- |
| `gossip.broadcast-metadata` | 5720 | Language, chat type, sound and emote metadata are absent from the text tables. |
| `gossip.condition-flags` | 57 | Only reverse-result flags are represented; swapped targets stay unavailable. |
| `gossip.condition-kind` | 735 | Unsupported source kinds become an unavailable sentinel. |
| `gossip.conditional-greeting` | 1280 | The Module cannot select greeting text by current condition state. It uses the last unconditional greeting, or source default 0xffffff. |
| `gossip.menu-script` | 8 | Source greeting scripts are not dispatched. |
| `gossip.option-action` | 14 | Unsupported service actions stay unavailable. |
| `gossip.option-condition` | 521 | The referenced condition graph contains unavailable predicates, operands or flags. |
| `gossip.option-confirmation` | 5 | The option table has no confirmation text. |
| `gossip.option-gender` | 7 | Option labels store one string; male text wins when both genders are present. |
| `gossip.option-poi` | 351 | Source points of interest are not sent. |
| `gossip.option-script` | 526 | Source option scripts are not dispatched. |
| `gossip.profile-npc-flags` | 2548 | Profile options cannot carry the source NPC flag mask. Creature entry options apply it during import. |
| `gossip.spawn-greeting` | 8 | Greeting tables are keyed by creature entry, not spawn GUID. |

Payment and text-input options would stay unavailable; neither occurs at this pin. The 16 portal
conditions remain unsupported. Compiling patch predicates and owning condition kinds does not
add a condition field to the portal catalogue. Script delivery and the complete Source Profile
gate remain for S6.

Creature and cast findings count selected templates, source paths, or retained placements as indicated by the finding. The census includes all source IDs in the local manifest.

| Finding | Full-world | Alliance-single |
| --- | ---: | ---: |
| `creatures.alternate-path` | 2 | 2 |
| `creatures.auras` | 610 | 81 |
| `creatures.formation` | 5781 | 5781 |
| `creatures.game-event` | 2907 | 2907 |
| `creatures.level-zero` | 5 | 0 |
| `creatures.nested-pool` | 2 | 2 |
| `creatures.respawn-range` | 18662 | 3448 |
| `creatures.script` | 664 | 49 |
| `creatures.spawn-distance` | 41935 | 8784 |
| `creatures.spawn-flags` | 349 | 23 |
| `creatures.spawn-resources` | 48456 | 10391 |
| `creatures.static-behavior` | 8681 | 1841 |
| `creatures.spell-slot-detail` | 4577 | 610 |
| `creatures.template-behavior` | 5338 | 1354 |
| `creatures.waypoint-detail` | 60222 | 60222 |

Nested pools with unequal child limits remain inactive with their definitions and placements retained. The five unplaced level-zero templates have no defined class-level stat lookup and are omitted. The cast model keeps the first spell and an existing secondary rotation; source slot probabilities, target parameters, flags, initial/repeat delays and scripts remain unsupported. Formations and scheduled game events have no runtime. Waypoint orientation, delay, wander radius and scripts remain unrepresented, as do alternate paths. Spawn resource percentages, respawn ranges, active/random flags and per-spawn distances remain unsupported. Source template auras, scripts, immunities, extra flags and static behavior beyond the translated client flags remain unsupported. These limits are not evidence that the Source Profile gate passes; S6 owns that gate.
