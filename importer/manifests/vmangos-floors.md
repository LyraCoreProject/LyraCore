# vmangos import floors

These floors use the production-plan census at vmangos commit
`464179081673cfd240f7ffe7f0daf96bca0b5a70`, patch 10 and client build 5875.
They test for missing content. They do not establish gameplay parity.

The complete catalogue floors apply to every profile:

| Floor | Old | New | Census count and reason |
| --- | ---: | ---: | --- |
| SPELL_TOTAL | 8,000 | 22,000 | 22,381 selected spells; detect a partial catalogue. |
| SPELL_CHAIN | 300 | 1,500 | 1,586 rank links; detect missing build-selected ranks. |
| SPELL_LEARN | 5 | 7 | All seven dependent spells are required. |
| SPELL_PROC_EVENT | 100 | 190 | 199 proc rows; allow small source revisions. |
| AREA_TRIGGERS | 400 | 440 | 449 selected trigger volumes. |
| TAXI_NODES | 1 | 80 | 85 source nodes; exclude the two reserved fixture nodes. |
| GAMEOBJECT_CHEST_LOOT | 1 | 12,000 | 13,851 expanded rows; chest templates are global. |
| FISHING_LOOT | 1 | 2,200 | 2,484 rows across all selected fishing zones. |

The Deadmines checks run only when the profile includes map 36:

| Floor | Old | New | Census count and reason |
| --- | ---: | ---: | --- |
| DEADMINES_CREATURE_SPAWNS | 100 | 180 | 207 placed spawns after patch selection. |
| DEADMINES_GAMEOBJECTS | 3 | 65 | 75 placed GameObjects, including doors and chests. |

`alliance-single` and `full-world` use the following floors. Each count is from
the smaller Alliance profile. Other profiles retain their presence or corridor
floors because the census below does not measure their scope.

| Floor | Previous slice floor | New | Alliance count and reason |
| --- | ---: | ---: | --- |
| TRAINER_OFFERINGS_CLASS | 1 | 12,000 | 13,529 offerings from selected trainer links. |
| TRAINER_OFFERINGS_PROFESSION | 1 | 200 | 224 profession offerings. |
| GATHER_NODE_GO | 1 | 12,000 | The existing query counts all 13,636 GameObjects, including 2,497 gather nodes. |
| QUEST_GIVER_RELATIONS | 1 | 2,000 | 2,366 selected creature-to-quest relations. |
| QUESTS_L6_10 | 1 | 160 | 181 quests in the level band. |
| QUESTS_L10_20 | 1 | 340 | 386 quests in the level band. |
| QUESTS_CHAINED | 1 | 530 | 598 quests with a next quest. |
| CONTINENT_SPAWNS | 100 | 8,000 | 8,663 spawns on map 0, the queried primary map. |
| VENDORS | 1 | 3,500 | 4,195 vendor offerings. |
| CASTER_CAST_ROWS | 1 | 350 | 419 primary cast rows. |
| ROTATION_ROWS | 20 | 280 | 331 additional spell slots; vmangos puts the first spell in the cast table. |
| PICKPOCKET_LOOT | 1 | 1,100 | 1,266 rows linked to selected creatures. |
| SKINNING_LOOT | 1 | 450 | 515 rows linked to selected creatures. |

The single-row Geomancer and Frost Armor rotation checks became a Geomancer
Fireball cast check (476, spell 20793) and a Knuckleduster Pummel rotation check
(449, spell 12555). vmangos gives the Geomancer one spell slot and no Frost Armor
rotation. The Pillager check now requires entry 589 with Fireball 19816.

Terrain, navigation, live-creature coverage, start positions, reserved taxi
fixtures and the remaining presence checks keep their existing floors. Planned
rows cannot establish live entity counts or terrain coverage.
