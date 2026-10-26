# What the importer reads from classic-db

Research done on 2026-10-26 at core `4ae2618f`. `origin/main` has since moved to `4a47e397`, so
check line numbers again before you rely on them. Paths are under `importer/` unless stated.

## 1. Pipeline

**Fetch and pin**

- `scripts/pull-classic-db.sh` fetches the source:
  - It clones `cmangos/classic-db` at `CLASSIC_DB_REF`, which defaults to the lock.
  - It concatenates `Full_DB/*.sql*` into `.import/classic-db-full.sql`. Each part is gunzipped and
    no delimiter is added.
  - It verifies SHA-256 against `scripts/classic-db.lock`. A ref other than the lock needs
    `--skip-verify`.
- The lock holds `CLASSIC_DB_SHA=cd0c426a…` and `CLASSIC_DB_SHA256=d2083bcd…`, a hash over the
  decompressed bytes.
- `scripts/import-world.sh` runs the stages:
  - default `DUMP=.import/classic-db-full.sql` (line 35)
  - the main import: `--dump --dbc … --eventai-profile --source-sha "$CLASSIC_DB_SHA" --apply`
    (186-188)
  - `--terrain` (197), `--nav` (207), `--taxi-mounts` (215), `--talents` (220)
  - `scripts/import-manifest.sh` holds the map lists and count floors for each profile.
- Shell tests: `pull-classic-db-checksum-test.sh`, `import-world-sql-test.sh` and
  `import-manifest-smoke.sh`.

**CLI flags** (`src/main.rs`; the importer split in #774 moved code since the line numbers were taken)

- `--dump`, `--dbc`, `--taxi-mounts`, `--go-models` and `--print-extents`
- `--world-profile`, `--include-map`, `--include-creatures`, `--family`, `--apply`,
  `--source-sha`, `--eventai-profile` and `--packages`
- `--dump` is required for `--family`, `--include-*`, `--print-extents` and `--go-models`.

**Parsing** (`src/dump.rs`)

- `read_dump` reads the whole file into memory, using lossy UTF-8.
- `parse_table` and `parse_complete_table` find ``INSERT INTO `<table>` VALUES `` and scan tuples.
  Both rescan the whole dump on every call.
- Every field is read by position, and the column names are never read. Index constants for each
  table live in `dump.rs`, and other raw indexes are scattered through the code.
- `NULL` comes through as the string `"NULL"`. The standard-SQL `''` escape is not handled.
- A missing table gives zero rows and no error.
- Many readers adapt to the column count:
  - `npc_text`: 81 or 17 columns
  - `gossip_menu_option`: 11, 14 or 15
  - `creature_movement_template`: 9 or 8
  - `spell_proc_event`: 9 or 11
  - `broadcast_text`: 12 or 17
  - `conditions`: 4, 5 or 7 and more
- `spell_proc_event` and `conditions` drop rows that are too short instead of failing.

**Assembly and write** (`src/world_import.rs`)

- `run` builds the plan. A dry run prints the SQL.
- `--apply` does four things:
  - runs the SQL through `shard.run_sql_statements`;
  - calls the reducers `import_gameobjects(_append)`, `import_creature_ai_relay_definitions`,
    `import_creature_ai_definitions(_append)` and `import_creature_spawns(_append)`;
  - stamps each family through `stamp_import_meta(family, source_sha, sha256(dump), row_count)`;
  - reapplies the Package Deltas.
- `FAMILIES`: creatures, items, loot, gossip, quests, gameobjects, trainers, casts, creature-ai,
  globals and spellmeta. Each family is cleared, then inserted in batches.

## 2. Each family

Column numbers below are 0-based positions in classic-db.

### creatures

- **`creature_template`** (`dump.rs` `ct`):
  - Columns read: entry, name, subname, min/max level, model_id1, faction, scale, family, type,
    npc_flags, unit_flags, type_flags, rank, min/max_level_health, min/max melee damage, armor,
    base attack time, loot gold, loot/pickpocket/skin loot ids, trainer type and class,
    vendor_template_id (raw index 76) and gossip_menu_id.
  - Writes `game_creature_template`.
  - `dbc.rs` also reads model_id1..4 for taxi mounts, and `eventai.rs` reads raw column 80
    (spell_list_id).
- **`creature`** (`cr`): guid, id, map, x/y/z/o, spawn time and movement type.
  - Rows go to `import_creature_spawns` with a packed guid.
  - The import fails on a guid of 0, a guid above 0xFFFFFF, or a duplicate.
- **`creature_movement`** and **`creature_movement_template`** write `game_creature_waypoint`.
- **Population** (`creature_population.rs`):
  - Tables read: `spawn_group`, `spawn_group_entry`, `spawn_group_spawn`, `creature_spawn_entry`,
    `game_event_creature`, `pool_template`, `pool_creature(_template)`, `pool_pool` and
    `pool_gameobject(_template)`. The gameobject pools are read only to refuse mixed pools.
  - It fails on bad groups, chances above 100, a spawn in both a group and a pool, cyclic pools,
    pools spanning maps, and limit 0.
  - Writes `game_creature_population`, `_member` and `_selection`.

### items

- **`item_template`**, read by position:
  - Columns: entry, class/subclass, name, display, quality, flags, buy/sell, inventory type,
    allowed class/race, item level, required level/skill/reputation, max count, stackable, slots,
    10 stat pairs, damage, armor and resistances, delay, 5 spell/trigger pairs, bonding, page
    text, start quest, sheath, random property, block, durability, bag family and food type.
  - `is_drink_consumable` is a name heuristic keyed to classic-db.
  - Items 25, 50, 51 and 52 are always kept.
- **`creature_loot_template`** and **`reference_loot_template`**.
- **`npc_vendor`** and **`npc_vendor_template`**.
- **`item_enchantment_template`**, checked against the client's Random Properties.
- Writes `game_item_template`, `game_creature_loot`, `game_npc_vendor` and
  `game_item_property_weight`.

### loot

- Pickpocketing, skinning, gameobject and fishing loot templates, all with references resolved.

### gossip

- `broadcast_text`, `npc_text`, `gossip_menu`, `gossip_menu_option` and `conditions`.
- `gossip_conditions.rs` supports kinds 0, 6, 8, 9, 14, 15 and 22. Any other kind becomes NEVER.
- `RELAY_GOSSIP_MENUS = [6623, 6687, 6688, 6949]` is hard-coded.

### quests

- `quest_template`, the four quest-relation tables, and `areatrigger_involvedrelation`.
- From `dbscripts_on_quest_end`, only SEND_MAIL (38) is used.
- `mail_loot_template`, with strict validation.
- `spell_template`, read by position: effects 61-63 and the learned spell at 109-111, for
  quest-reward learn spells.

### gameobjects

- `gameobject_template` and `gameobject`.
- Type 25 (FISHINGHOLE) is dropped, because the Module reuses 25 as its GATHER marker.
- `GATHER_NODES` is hard-coded.

### trainers

- `npc_trainer`.
- `PROFESSION_LEARN` is hard-coded and synthesizes profession tiers, checked against SkillTiers.

### casts

- `creature_template_spells`. set_id must be 0. Rows are filtered against the Spell.dbc id set.

### globals

- `player_classlevelstats`, `player_levelstats`, `playercreateinfo`, `game_graveyard_zone`,
  `playercreateinfo_spell`, `playercreateinfo_action`, `playercreateinfo_item` and
  `areatrigger_teleport`.

### Other passes

- `transport.rs` reads `transports` joined to type 15 gameobject templates.
- `go_model.rs` reads `gameobject_template`.
- `extents.rs` reads `creature` and `playercreateinfo`.

## 3. EventAI translation

`src/eventai.rs` has about 6,000 production lines and 2,740 test lines (58 tests).
`eventai_presentation.rs` adds 196.

- **The Source Profile** is `manifests/cmangos-classic-z2815.json` (459 lines), embedded with
  `include_str!`. It pins:
  - the cmangos and classic-db commits and the SQL SHA-256;
  - the loader contract "creature-ai-scripts-24-columns-ordered-by-id-v1";
  - rule counts (10,843 source rules, 39 guid rules);
  - the approved event, action, target, flag and dependency sets;
  - per-value censuses.
- **`CompatibilityManifest::build`** compares what the dump contains against the profile. The gate
  runs in `resolve_scope_entries` and is enforced on `--apply`. Any change to the dump bytes fails
  it.
- **Tables read:**
  - `creature_ai_scripts`: 24 columns, three inline actions of 4 columns each
  - `broadcast_text` and `script_texts` (negative ids)
  - `creature_ai_summons`
  - `conditions`: only (6,469), (6,67) and (9,quest)
  - quest and creature sets
  - `creature_template_spells` overlaps
- **Relay tables:** `dbscripts_on_relay` (22 columns) and `dbscript_random_templates`, plus
  existence sets from many tables.
- **Events supported:** 0-18, 21-25, 27-30, 32-34 and 36.
- **Actions supported:** 49 values from 1 to 64, grouped into speech, emote, cast, threat, phase,
  summon, movement, state, encounter, quest and presentation. Action 4 (sound) is excluded.
- **Relay commands supported:** 0, 1, 3, 10, 13, 15, 18, 20-22, 25, 26, 28, 29, 31, 32, 35-37,
  40, 42, 44, 45, 48, 52 and 53. Nesting depth is 16 and the step budget 4,096.
- **Output:** rules are encoded as LyraCore text rules, grouped per subject, revisioned with BLAKE3
  and sent in 64 KB batches through the creature-ai reducers.

## 4. Identifiers specific to classic-db

- **Creature guids:** `0xF130<<48 | entry<<24 | source_guid`, with `source_guid` at most 0xFFFFFF.
  **Gameobject guids:** `0xF110<<48 | spawn_id`. Both are in
  `crates/lyracore-package-delta/src/ids.rs`.
- **Reserved bands:**
  - Operator Spawn Range: 14,000,000–14,999,999
  - fixture-reserved: 5,090,000–5,099,999
  - curated-reserved: 1,000,000–5,200,499

  Nothing checks that source guids stay out of these bands. vmangos gameobject guids go up to
  3,998,642, so check whether the curated band applies to spawn ids.
- **EventAI:** a negative `creature_id` is a source guid; the profile counts 39 such rules.
  `encounter_binding` hard-codes 14 cmangos (entry, field) pairs.
  `eventai_presentation.rs` hard-codes `RAJAXX_RULE_ID = 1_534_108` and
  `RAJAXX_SOURCE_GUID = -155_940`, plus 19 display templates.
- **Other hard-coded ids:**
  - `SPELL_TEMPLATE_ENTRIES = [416, 1860, 1863, 5676, 5677, 12861]`
  - `RELAY_GOSSIP_MENUS`, `GATHER_NODES`, items 25/50/51/52 and `PROFESSION_LEARN`
  - `INCLUDE_CREATURES` defaults in `import-world.sh`
  - `CT_VENDOR_TEMPLATE_ID = 76`, duplicated
- **Population ids:** a pool root is `(2<<32)+root`, a direct spawn `(1<<32)+guid`. Event ids are
  stored verbatim.

## 5. Spell data: DBC or classic-db

- **From DBC:** `spell.rs`, `spell_dbc.rs`, `spell_snapshot.rs` and `talent.rs` read only DBCs.
  They produce `game_spell`, `game_spell_effect`, `game_spell_reagent` and the creature-AI spell
  metadata.
- **From classic-db, the spellmeta family:**
  - `spell_chain`, with DBC rank links filling the gaps;
  - `spell_learn_spell`, with two armor-proficiency rows added;
  - `spell_proc_event`, with family masks resolved against Spell.dbc. Ids from 50,000 up are
    skipped.
- **Other classic-db spell uses:** the `spell_template` existence set and quest learn spells,
  `creature_template_spells`, `npc_trainer`, item spell ids, and the gameobject trap spell and
  cooldown.

## 6. Tests

- All importer tests are in-crate:

  | File | Tests |
  |---|---|
  | `world_import.rs` | 110 |
  | `dbc.rs` | 34 |
  | `dump.rs` | 12 |
  | `eventai.rs` | 58 |
  | `creature_population.rs` | 9 |
  | `item_property.rs` | 9 |
  | `go_model.rs` | 7 |
  | `transport.rs` | 4 |
  | `extents.rs` | 3 |
  | `gossip_conditions.rs` | 2 |

- About 272 embedded ``INSERT INTO ` `` rows follow cmangos's column order. Helpers build full
  cmangos rows, for example `creature_template_row*`, `quest_template_row` and
  `dbscripts_send_mail_row`.
- Ignored tests use the real dump through `LYRACORE_CLASSIC_DB_SQL`: in `world_import.rs`, in
  `creature_population.rs`, and `eventai.rs` `pinned_full_dump_source_census_matches_the_profile`.

## 7. Docs and terms

- **Docs:**
  - `README.md` (43, 103)
  - `CHANGELOG.md` (133-135)
  - `docs/data-ingestion.md`, which has the most
  - `docs/quickstart.md` (322-333)
  - `docs/schema.md` (308, 471)
  - `docs/testing.md` (173, 186)
  - `docs/adr/0001`, `0002` and `0003`
  - `docs/architecture.md`
  - `docs/benilla-compatibility.md`
  - `docs/recipes/add-a-spell-effect.md`
  - `.claude/skills/verify-lyracore/checks.md` (92)
- **`CONTEXT.md` entries:** EventAI Source Profile, Compatibility Manifest, Encounter Binding,
  Relay Definition, World Import Scope, Creature-AI Family, Import Family, Operator Spawn Range,
  Base Snapshot and Package EventAI Range.
- Pinned artifacts such as `cmangos-classic-z2815` keep their names until they are deleted.

## Risks

1. Reading by position means a reordered vmangos table is misread without any error.
2. The EventAI gate pins exact bytes and a 24-column loader contract.
3. Several hard-coded id tables are specific to classic-db.
4. `sha256_hex` shells out to `sha256sum`.
