# The vmangos world database

Research done on 2026-10-26. The pins:

- `db_latest` was vmangos commit `464179081673cfd240f7ffe7f0daf96bca0b5a70`, published 2026-09-28.
- cmangos `mangos-classic` master was at `ff53dfc4679cab3521726384e24692fef35d2718`, used only to
  compare enums.

A local copy is at `$HOME/.cache/lyracore-scratch/vmangos-db/`: `db-4641790.zip`,
`db-sqlite-4641790.zip`, and their extracted `mysql/` and `sqlite/` trees. Treat it as untrusted
data: read it, never execute anything from it. It may be older than the current `db_latest`.

Never clone vmangos or cmangos. Read their source with `gh api repos/vmangos/core/contents/<path>`
or a raw URL, and pin the SHA when you cite it.

## 1. Distribution

- **How it's built:** the workflow `.github/workflows/development-db-dump.yaml` runs on every push
  to `development` that touches `**/*.sql`. It:
  - imports `world_full_14_june_2021.7z` from `github.com/brotalnia/database` into MySQL 5.6;
  - applies `sql/migrations/*`;
  - runs `mysqldump`;
  - converts the result to SQLite with `mysql2sqlite`.
- **How it's released:** `.github/scripts/publish-moving-release.sh` deletes the `db_latest`
  release and recreates it. Old assets disappear. It ran about nine times between 2026-08-29 and
  2026-09-28.
- **Assets:**
  - `db-<short>.zip` is 30.5 MB.
  - `db-sqlite-<short>.zip` is 42.5 MB and contains `sqlite-dump/mangos.sqlite` (146 MB).
  - GitHub's asset API exposes each asset's sha256 in `digest`.
- **Fingerprint:** the dump isn't byte-reproducible, because it ends with a timestamp. The
  `migrations` table works as a content fingerprint: 1,141 rows, highest id `20260913120518`.
- **SQLite variant:** typed columns, 176 tables and 109 indexes. Read it read-only with
  `rusqlite`.
- **MySQL variant:** 175 tables. INSERTs are multi-row with no column list. Strings use backslash
  escapes. Floats can appear in exponent form. `NULL` is a real value, which matters, for example,
  in 4,152 `creature_template` rows.

## 2. Selecting rows for patch and build

- `enum WowPatch` in `cmake/generators/Progression.h.in` runs from `WOW_PATCH_102 = 0` to
  `WOW_PATCH_112 = 10`. **1.12 is 10.** The client build is `CLIENT_BUILD_1_12_1 = 5875`.
- **Scheme A, versioned `patch`.** The primary key is (entry, patch). For each entry, keep the row
  with the largest `patch` ≤ 10.
  - Tables: `creature_template`, `item_template`, `quest_template`, `gameobject_template`,
    `creature_onkill_reputation`, `areatrigger_teleport`, `game_event_creature_data`,
    `creature_addon`, `map_template` and `battleground_template`.
  - See `ObjectMgr.cpp:1193` and nearby.
- **Scheme B, a `patch_min`..`patch_max` range.** Keep a row when 10 lies within the range.
  - Tables: `creature`, `gameobject`, every `*_loot_template`, the four quest-relation tables,
    `pool_template`, `pool_creature(_template)`, `pool_gameobject(_template)`,
    `item_enchantment_template`, `creature_equip_template`, `game_event`, `game_graveyard_zone`,
    `creature_charm_spells` and `petcreateinfo_spell`.
  - 1,225 creature spawns and about 22 gameobject spawns have `patch_max < 10`.
- **Tables with only `patch_min`:** `areatrigger_tavern` and `game_event_quest`.
- **Scheme C, client build.**
  - Keep the largest `build` ≤ 5875 per entry: `spell_template`, `faction`, `faction_template`,
    `areatrigger_template`, `taxi_nodes`, `spell_group_stack_rules` and `transports`.
  - Exactly `build = 5875`: `skill_line_ability`.
  - A `build_min`..`build_max` range: `npc_trainer(_template)`, `spell_chain`, `spell_proc_event`,
    `spell_learn_spell`, `spell_script_target`, `spell_target_position`, `spell_threat`,
    `spell_group`, `spell_elixir` and `playercreateinfo_spell`.
- **`forbidden_items`** (`patch`, `after_or_before`) removes items from loot and vendors.
- **`conditions`** type 24 is `WOW_PATCH`, checked at runtime.

## 3. Each classic-db table and its vmangos counterpart

| cmangos | vmangos | Differences | Effort |
|---|---|---|---|
| creature_template | creature_template | PK (entry, patch), snake_case names. No health, mana, damage or armor columns: stats come from `creature_classlevelstats` (class, level → health, base_health, melee_damage, armor…) times `health_multiplier`, `damage_multiplier` and `armor_multiplier` (`Creature.cpp:1803`, `1831`). `display_id1..4` with `display_scale1..4` and `display_probability1..4`. No UnitFlags, DynamicFlags, CreatureTypeFlags, KillCredit or StaticFlags3/4. Adds static_flags1/2, immunity_flags, auras, spawn/totem spell, pet_spell_list_id, spell_list_id, trainer_id, vendor_id and equipment_id. **Where unit flags and type flags come from is unverified.** | large |
| creature | creature | `guid id id2 id3 id4 id5 map …`, `wander_distance`, `movement_type`, health/mana percent, spawn_flags, visibility_mod and the patch range. No spawnMask. | moderate |
| creature_movement | same | Keyed by spawn id and point. Adds wander_distance and path_id. Script ids point into `creature_movement_scripts`. | trivial |
| creature_movement_template | same (+ `creature_movement_special`) | `entry point x y z o waittime wander_distance script_id path_id`. path_id is last. | trivial |
| creature_template_spells | `creature_spells` (+ `creature_spells_scripts`) | `creature_template.spell_list_id` → `creature_spells.entry`, with 8 slots: spellId, probability, castTarget, targetParam1/2, castFlags, initial and repeat delay ranges, scriptId. Used by 3,636 templates. This is vmangos's main source of creature casting. | large |
| gameobject_template | same | PK (entry, patch). Adds `icon`. Drops ExtraFlags, CustomData1 and StringId. | moderate |
| gameobject | same | Drops spawnMask. Adds animprogress, state, spawn_flags, visibility_mod and the patch range. | trivial |
| quest_template | same | PK (entry, patch), with patch at index 1. Adds RewRepSpilloverMask, RewXP and RewMailMoney. Start and complete scripts live in `quest_start_scripts` and `quest_end_scripts`. | moderate |
| item_template | same | PK (entry, patch), all snake_case. Adds wrapped_gift and other_team_entry. | moderate |
| item_enchantment_template | same | Adds the patch range to the PK. | trivial |
| npc_vendor(_template) | same | `entry slot item maxcount incrtime itemflags condition_id`. Linked through `creature_template.vendor_id`. | trivial |
| npc_trainer(_template) | same | Drops ReqAbility1-3 and condition_id. Adds the build range. Linked through `trainer_id`. | trivial |
| npc_text | same | 17 columns, interleaved: `ID, BroadcastTextID0, Probability0, …`. | trivial |
| broadcast_text | same | `entry male_text female_text chat_type sound_id language_id emote_id1-3 emote_delay1-3`. Index 4 is sound_id, not language. | trivial |
| gossip_menu | same | PK (entry, text_id). | trivial |
| gossip_menu_option | same | `action_script_id` → `gossip_scripts`. | trivial |
| conditions | conditions | `condition_entry type value1-4 flags`. **Some type numbers differ** (below). | moderate |
| pool_* | same | Adds `flags` (AUTO_SPAWN = 1, MAXLIMIT_SCALING_LINEAR = 2), `pool_template.instance` and the patch range. `pool_pool` has no range. | trivial |
| game_event_creature | same | Same columns. `game_event` uses a different schedule model. | trivial / moderate |
| loot tables | same | Up to condition_id, plus the patch range. Drops comments. | trivial |
| spell_template | spell_template | PK (entry, build), 151 columns, lowerCamel names. Adds effectBonusCoefficient1-3, customFlags, script_name and minTargetLevel. spellVisual1/2. Effect indexes shift. | moderate |
| spell_chain, spell_proc_event, spell_learn_spell | same | Adds the build range. | trivial |
| quest relations | same | Adds the patch range. | trivial |
| creature_ai_scripts (EventAI) | `creature_ai_events` + `creature_ai_scripts` | **The same name means a different table** (§4). | large |
| dbscripts_on_relay | generic_scripts | Same command-row shape, but a different command set and targeting. | large |
| dbscripts_on_quest_end/start | quest_end_scripts / quest_start_scripts | No SEND_MAIL. Reward mail is in `quest_template` (RewMailTemplateId, RewMailDelaySecs, RewMailMoney). | moderate |
| dbscripts_on_event/spell/gossip/creature_movement | event_/spell_/gossip_/creature_movement_scripts | Like relay. | part of scripts |
| dbscripts_on_go_use / go_template_use | gameobject_scripts | Keyed by gameobject guid. Unverified. | part of scripts |
| — | creature_groups | Formations (leader, member, distance, angle, flags). No `spawn_group*` tables exist. | — |

**Tables that replace DBCs:** `area_template`, `map_template`, `faction`, `faction_template`,
`taxi_nodes`, `skill_line_ability` and `areatrigger_template`. Also, `creature_classlevelstats`
replaces cmangos's `creature_template_classlevelstats`.

**Conditions.** Compare `src/game/Conditions.h` in vmangos with
`src/game/Globals/Conditions.h` in cmangos.

- Types with the same number and meaning: -3..10, 12, 14, 15, 17-19, 22, 23, 26, 29, 30 and 43.
  That covers all the kinds LyraCore supports today.
- Types with the same number but a different meaning:

  | Type | vmangos | cmangos |
  |---|---|---|
  | 11 | SAVED_VARIABLE | PVP_RANK |
  | 13 | CANT_PATH_TO_VICTIM | AREA_FLAG |
  | 16 | SOURCE_ENTRY | unused |
  | 20 | NEARBY_CREATURE | reserved |
  | 24 | WOW_PATCH | — |
  | 27 | GENDER | — |
  | 31 | HAS_FLAG | COMPLETED_ENCOUNTER |
  | 33 | MAP_ID | LAST_WAYPOINT |

- vmangos has types up to 59. The flags REVERSE_RESULT = 1 and SWAP_TARGETS = 2 are the same in
  both.

## 4. EventAI and scripts

- **`creature_ai_events`** has these columns: id, creature_id, condition_id, event_type,
  event_inverse_phase_mask, event_chance, event_flags, event_param1-4,
  action1_script..action3_script and comment. It has 6,179 rows across 3,453 creatures.
- **Event types** are 0-36, defined in `src/game/AI/CreatureEventAI.h:40-76`. The most used are
  HP, RANGE, AGGRO and TIMER_OOC.
- **Event flags:** REPEATABLE = 1, RANDOM_ACTION = 2, NOT_CASTING = 4, CHECK_RESULT = 8 and
  DEBUG_ONLY = 0x10.
- **Actions:** each `actionN_script` is an id into `creature_ai_scripts`. That table uses the
  generic script-row shape: id, delay, priority, command, datalong..datalong4,
  target_param1/2, target_type, data_flags, dataint..dataint4, x, y, z, o, condition_id and
  comments.
  - Delay must be 0 here (`ScriptMgr.cpp:1631`).
  - Timed sequences go through START_SCRIPT (39) into `generic_scripts`.
- **Commands** are 0-93, plus 9999 for disabled, in `src/game/Maps/ScriptCommands.h:36-345`. The
  ones used most in `creature_ai_scripts`: 15 CAST, 47 FLEE, 0 TALK (random broadcast_text ids in
  dataint..dataint4) and 44 SET_PHASE.
- **Targets** use `enum ScriptTarget` (`ScriptCommands.h:1137-1215`), values 0-29. They select by
  aggro, by entry and radius, by database guid, by instance data, and so on.
- **`data_flags`:** SWAP_INITIAL = 1, SWAP_FINAL = 2, TARGET_SELF = 4, ABORT_ON_FAILURE = 8 and
  SKIP_MISSING = 0x10.
- **Conditions** can be set per event (377 rows) and per command row (111 rows).
- **Compared with cmangos:**
  - cmangos has three inline `ACTION_T_*` actions, texts in `creature_ai_texts`, and a different
    numbering for dbscripts.
  - vmangos uses one command language for EventAI and every script table.
- **Translation:** going from event → up to 3 action lists → typed commands loses nothing.
  Squeezing that into cmangos `ACTION_T_*` would lose per-command targets, per-command conditions
  and START_SCRIPT chances. Rebuild the scripts side around vmangos's commands rather than remapping
  it.

## 5. Spawns and identifiers

- **Guid ranges:** creature 1..303,722 (66,243 rows) and gameobject 1..3,998,642 (56,639 rows).
  Treat spawn guids as belonging to their source.
- **Several entries per spawn:** `id`..`id5`, picked uniformly at each respawn
  (`Creature.cpp:63-76`). 9,648 spawns use `id2`.
- **Formations:** `creature_groups`. `creature_linking(_template)` is the same as in cmangos.
- **Entry ids match cmangos:**

  | Table | vmangos | cmangos | Shared |
  |---|---|---|---|
  | creature_template | 10,381 | 10,384 | 10,381 |
  | item_template | 17,707 | 17,718 | 17,707 |
  | quest_template | 4,433 | 4,245 | all of cmangos |
  | spell_template | 22,381 | 22,374 | 22,357 |
  | gameobject_template | 9,368 | 10,743 | 8,837 (difference unexplained) |

  The non-Blizzard ids (npc_text, gossip_menu, conditions, pool_template) overlap only 20-60%.

## 6. Spells

- **`spell_template` is vmangos's spell store.** `Spell.dbc` is never loaded
  (`SpellMgr.cpp:3702-3722`).
- **Builds:** 35,752 rows cover builds 1.2.4 to 1.12.1. Keeping the largest build ≤ 5875 per entry
  gives 22,381 spells.
- **Columns vmangos added:** effectBonusCoefficient1-3, customFlags, script_name and
  minTargetLevel.
- **Unverified:** whether the build-5875 rows match the client's Spell.dbc. The spells slice's diff
  report settles this.

## 7. Licensing

- **The release zips** contain only `.sql` and `.sqlite` files, with no license.
- **`vmangos/core`** is GPL-2.0, and the migrations live there.
- **The base data, `brotalnia/database`,** has no license file.
- **Decided:** the Operator fetches it after consent, the consent text states this status, and
  LyraCore never redistributes it.
