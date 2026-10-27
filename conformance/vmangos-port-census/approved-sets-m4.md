# Milestone 4 approved source values

The `vmangos-4641790` Source Profile checks 75 tables and 1,028 consumed columns. `importer/manifests/vmangos-4641790.json` holds the exact column names and affinities. The gate checks raw source values before patch/build selection. Approval means the importer has classified a value; it does not mean every behavior using it is implemented. Counts are reported and never gate import.

Generated from the profile JSON by `audit-approved-sets.py`. Values are listed in decimal; masks are hexadecimal. A value mask permits combinations of its bits. Command and target masks apply only to the stated selector and optional destination field.

## `area_template`

- `flags` mask: 0x3ff.

## `conditions`

- `type` values: -3, -2, -1, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 27, 28, 29, 30, 31, 33, 34, 35, 36, 37, 38, 39, 40, 41, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59.
- `flags` mask: 0x3.

## `creature`

- `spawn_flags` mask: 0xff.

## `creature_ai_events`

- `event_type` values: 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 21, 22, 23, 24, 25, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36.
- `event_flags` mask: 0xf.

## `creature_ai_scripts`

- `command` values: 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 14, 15, 16, 18, 19, 20, 22, 23, 24, 25, 26, 27, 28, 29, 33, 34, 35, 37, 38, 39, 42, 43, 44, 46, 47, 48, 49, 50, 51, 52, 54, 55, 56, 59, 60, 61, 62, 65, 67, 68, 69, 71, 72, 73, 74, 77, 78, 79, 80, 81, 83, 84, 85, 88, 90, 91.
- `target_type` values: 0, 1, 4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 22, 23, 24, 25, 28, 29.
- `data_flags` mask: 0x1f.
- command 10, `dataint3` values: -1, 0, 1, 4.
- command 29, `datalong` values: 0, 1, 8.
- command 4, `datalong` values: 9, 46, 147.
- command 3, `datalong3` mask: 0xc5.
- command 3, `datalong4` mask: 0x3.
- command 4, destination field 9, `datalong2` mask: 0x10.
- command 4, destination field 46, `datalong2` mask: 0x20603c2.
- command 4, destination field 147, `datalong2` mask: 0x3.
- command 6, `datalong2` mask: 0x0.
- command 10, `dataint` mask: 0x9.
- command 15, `datalong2` mask: 0x27.
- command 16, `datalong2` mask: 0x2.
- command 22, `datalong2` mask: 0x7.
- command 74, `datalong2` mask: 0x8.
- command 78, `datalong` mask: 0x3.
- target 4, `target_param1` mask: 0x303.
- target 5, `target_param1` mask: 0x103.
- target 7, `target_param1` mask: 0x1.

## `creature_display_info_addon`

- `gender` values: 0, 1, 2.

## `creature_movement_scripts`

- `command` values: 0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 13, 14, 15, 16, 18, 19, 20, 22, 23, 24, 25, 26, 27, 28, 34, 35, 37, 39, 41, 44, 51, 59, 60, 61, 62, 63, 66, 67, 68, 69, 76, 77, 78, 79, 80, 81, 82, 84, 87, 90, 92.
- `target_type` values: 0, 10, 11, 13, 14, 23, 24, 25, 27.
- `data_flags` mask: 0x1f.
- command 10, `dataint3` values: -1, 0.
- command 4, `datalong` values: 9, 46, 147.
- command 2, destination field 9, `datalong2` mask: 0x0.
- command 3, `datalong3` mask: 0x147.
- command 3, `datalong4` mask: 0x3.
- command 4, destination field 9, `datalong2` mask: 0x10.
- command 4, destination field 46, `datalong2` mask: 0x2000302.
- command 4, destination field 147, `datalong2` mask: 0x3.
- command 6, `datalong2` mask: 0x0.
- command 10, `dataint` mask: 0x3.
- command 15, `datalong2` mask: 0x27.
- command 16, `datalong2` mask: 0x2.
- command 22, `datalong2` mask: 0x3.
- command 78, `datalong` mask: 0x1.

## `creature_template`

- `flags_extra` mask: 0xfffff.
- `immunity_flags` mask: 0x7f.
- `npc_flags` mask: 0x12087fff.
- `static_flags1` mask: 0xffffffff.
- `static_flags2` mask: 0x7e.

## `event_scripts`

- `command` values: 0, 1, 3, 4, 9, 10, 11, 13, 15, 18, 20, 22, 25, 26, 28, 31, 35, 37, 38, 39, 41, 44, 61, 68, 69, 76, 80, 81, 83, 86.
- `target_type` values: 0, 10, 11, 13, 14, 22.
- `data_flags` mask: 0x1f.
- command 10, `dataint3` values: -1, 0.
- command 4, `datalong` values: 46, 147.
- command 3, `datalong3` mask: 0x1.
- command 3, `datalong4` mask: 0x2.
- command 4, destination field 46, `datalong2` mask: 0x300.
- command 4, destination field 147, `datalong2` mask: 0x3.
- command 10, `dataint` mask: 0x1d.
- command 15, `datalong2` mask: 0x0.
- command 22, `datalong2` mask: 0x1.
- command 31, `datalong3` mask: 0x1.

## `faction`

- `reputation_flags1` mask: 0x1f.
- `reputation_flags2` mask: 0x1e.
- `reputation_flags3` mask: 0x11.
- `reputation_flags4` mask: 0x11.

## `faction_template`

- `faction_flags` mask: 0x1c7f.

## `gameobject`

- `spawn_flags` mask: 0xf.

## `gameobject_scripts`

- `command` values: 3, 9, 10, 11, 12, 15, 20, 28, 31, 34, 37, 39, 41, 60, 76, 77, 82.
- `target_type` values: 0, 10, 11, 13.
- `data_flags` mask: 0x17.
- command 10, `dataint3` values: -1, 0.
- command 3, `datalong3` mask: 0x4.
- command 3, `datalong4` mask: 0x2.
- command 10, `dataint` mask: 0x19.
- command 15, `datalong2` mask: 0x2.
- command 31, `datalong3` mask: 0x1.

## `gameobject_template`

- `flags` mask: 0x7f.

## `generic_scripts`

- `command` values: 0, 1, 3, 4, 6, 7, 9, 10, 11, 13, 14, 15, 16, 18, 19, 20, 22, 24, 25, 26, 27, 28, 33, 34, 35, 37, 39, 41, 42, 43, 44, 48, 49, 51, 52, 55, 59, 60, 61, 62, 63, 65, 66, 68, 69, 70, 71, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 87, 88, 89, 90, 91, 92, 93.
- `target_type` values: 0, 9, 10, 11, 13, 14, 22, 23, 24, 25, 28.
- `data_flags` mask: 0x1f.
- command 10, `dataint3` values: -1, 0, 8.
- command 4, `datalong` values: 9, 46, 147.
- command 3, `datalong3` mask: 0x14f.
- command 3, `datalong4` mask: 0x3.
- command 4, destination field 9, `datalong2` mask: 0x11.
- command 4, destination field 46, `datalong2` mask: 0x2040382.
- command 4, destination field 147, `datalong2` mask: 0x3.
- command 6, `datalong2` mask: 0x0.
- command 10, `dataint` mask: 0xf.
- command 15, `datalong2` mask: 0x27.
- command 16, `datalong2` mask: 0x6.
- command 22, `datalong2` mask: 0x3.
- command 74, `datalong2` mask: 0x0.
- command 78, `datalong` mask: 0x7.

## `gossip_scripts`

- `command` values: 0, 1, 2, 3, 4, 6, 7, 8, 9, 10, 13, 14, 15, 16, 17, 18, 20, 22, 25, 26, 28, 30, 34, 35, 36, 37, 39, 41, 48, 60, 61, 74, 80, 83, 84, 85, 87, 93.
- `target_type` values: 0, 10, 13, 14.
- `data_flags` mask: 0xf.
- command 10, `dataint3` values: -1, 0.
- command 4, `datalong` values: 46, 147.
- command 2, destination field 147, `datalong2` mask: 0x3.
- command 3, `datalong3` mask: 0x45.
- command 3, `datalong4` mask: 0x3.
- command 4, destination field 46, `datalong2` mask: 0x2000200.
- command 4, destination field 147, `datalong2` mask: 0x3.
- command 6, `datalong2` mask: 0x0.
- command 10, `dataint` mask: 0xc.
- command 15, `datalong2` mask: 0x7.
- command 16, `datalong2` mask: 0x0.
- command 22, `datalong2` mask: 0x3.
- command 74, `datalong2` mask: 0x0.

## `item_template`

- `flags` mask: 0x3eff7.

## `npc_vendor`

- `itemflags` mask: 0x3.

## `npc_vendor_template`

- `itemflags` mask: 0x0.

## `pool_creature`

- `flags` mask: 0x0.

## `pool_creature_template`

- `flags` mask: 0x0.

## `pool_pool`

- `flags` mask: 0x0.

## `pool_template`

- `flags` mask: 0x0.

## `quest_end_scripts`

- `command` values: 0, 1, 3, 4, 5, 9, 10, 13, 14, 15, 16, 18, 19, 20, 22, 25, 26, 27, 28, 31, 32, 35, 37, 39, 41, 44, 60, 61, 62, 66, 68, 69, 76, 80, 81, 82, 89.
- `target_type` values: 0, 10, 11, 13, 14, 28.
- `data_flags` mask: 0x1f.
- command 10, `dataint3` values: -1, 0.
- command 4, `datalong` values: 9, 46, 147.
- command 3, `datalong3` mask: 0x147.
- command 3, `datalong4` mask: 0x3.
- command 4, destination field 9, `datalong2` mask: 0x10.
- command 4, destination field 46, `datalong2` mask: 0x300.
- command 4, destination field 147, `datalong2` mask: 0x17.
- command 10, `dataint` mask: 0xf.
- command 15, `datalong2` mask: 0x2.
- command 16, `datalong2` mask: 0x2.
- command 22, `datalong2` mask: 0x1.
- command 31, `datalong3` mask: 0x1.
- command 32, `datalong3` mask: 0x0.

## `quest_start_scripts`

- `command` values: 0, 1, 3, 4, 5, 7, 9, 10, 11, 15, 17, 20, 22, 25, 26, 27, 28, 31, 32, 35, 39, 41, 44, 52, 55, 60, 61, 69, 71, 74, 76, 82, 93.
- `target_type` values: 0, 10, 11, 14.
- `data_flags` mask: 0xf.
- command 10, `dataint3` values: -1.
- command 4, `datalong` values: 46, 147.
- command 3, `datalong3` mask: 0x47.
- command 3, `datalong4` mask: 0x3.
- command 4, destination field 46, `datalong2` mask: 0x302.
- command 4, destination field 147, `datalong2` mask: 0x3.
- command 10, `dataint` mask: 0xf.
- command 15, `datalong2` mask: 0x20.
- command 22, `datalong2` mask: 0x7.
- command 31, `datalong3` mask: 0x1.
- command 32, `datalong3` mask: 0x1.
- command 74, `datalong2` mask: 0x0.

## `quest_template`

- `QuestFlags` mask: 0x76f.
- `SpecialFlags` mask: 0x3.

## `spell_proc_event`

- `procFlags` mask: 0xff7f8.

## `spell_template`

- `auraDescriptionFlags` mask: 0x3f007e.
- `auraInterruptFlags` mask: 0x7fbdff.
- `channelInterruptFlags` mask: 0x3ffff.
- `customFlags` mask: 0x3fb7.
- `descriptionFlags` mask: 0x7f007e.
- `interruptFlags` mask: 0x1f.
- `nameFlags` mask: 0x7f007e.
- `nameSubtextFlags` mask: 0x7f007e.
- `procFlags` mask: 0x3fffff.
- `spellFamilyFlags` mask: 0xfffffffff.
