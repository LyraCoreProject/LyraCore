# What depends on classic-db outside the importer's parser

Research done on 2026-10-26, at these commits:

- core `4ae2618f` (`origin/main` is now `4a47e397`)
- `lyracore-cli` `0cf0077`
- `LyraCoreProject/packages` `5248c228`

Check line numbers again before relying on them.

## 1. The CLI and the launcher pin

All of this is in `lyracore-cli`.

- **`src/cmd/import.rs`**:
  - `CONSENT` (157-190) names cmangos `classic-db`, GPL-3.0 and the lock. `consent()` is at 698,
    and the test at about 2992 requires "classic-db", "cmangos", "GPL-3.0" and "Blizzard".
  - `DUMP_PATH = .import/classic-db-full.sql` (204) is passed as `--dump` (1025).
  - Stage 1 (358-385) runs `bash importer/scripts/pull-classic-db.sh`. The error text at 367-368
    names `CLASSIC_DB_REF=master … --skip-verify`. The tests at 3116, 3390 and 3692 match the
    script and the wording.
  - `WorldProfile` (150) and its defaults (606-630). The CLI passes neither `--eventai-profile`
    nor `--source-sha`.
  - `CONSUMED_FLOORS` (1182-1241) reads `importer/scripts/import-manifest.sh`.
  - Content queries pin classic-db entries, so changing them needs a CLI release:
    - trainers and vendors 926, 836, 1681, 3593 and 4200
    - 328, 377, 906, 913, 917 and 927
    - Farley (295) and the Sentinel Hill vendor (491)
    - spells 133, 13322, 3110 and 12544, and entries 416, 589, 449 and 476
    - Deadmines: triggers 78 and 119, the bosses, and loot 5191 and 5196
    - `KNOWN_EMPTY_TRAINERS`
- **`src/project.rs:334`** holds `PULL_CLASSIC_DB_SCRIPT`.
- **`docs/commands.md`** (106, 112).
- **Core's `importer/scripts/import-world.sh`** (361-553) repeats the content checks. It also has a
  reserved flight-master spawn guid at line 416.
- **The pin:** `.lyracore-cli-rev` holds a full SHA, and the launcher installs that commit. The
  procedure for changing it is in `docs/agents/cross-repo-cli.md`.
- **The 0.1 release plan** reworks the CLI's import flow so that release users talk HTTP to the
  Host. Do the port's CLI work after that lands.

## 2. Module runtime semantics

**Owned by LyraCore, not by either source:**

- EventAI model in `module/src/creatures/eventai/model.rs`: `EventCondition`, `EventKind`,
  `CreatureInstruction`, `InstructionTarget`, `EventAiRule` and `RecurrencePolicy`. They're stored
  in SpacetimeDB enum order, so reordering variants changes stored data.
- EventAI tables in `eventai/tables.rs`: definition, Rule State, state and summon.
- Relay model in `eventai/relay.rs`: `RelayInstruction`, `RelayDefinition`, `RelayRun` and
  `RelayArrival`, with a LyraCore text grammar. Its only link to the source is `relay_id`, which is
  the `dbscripts_on_relay` id.
- `game_creature_ai_event` is legacy, kept for migration safety.

**Encodes classic-db or cmangos values directly:**

- **Gossip conditions.**
  - `module/src/gossip.rs:12-22` stores raw `kind: i32`.
  - `crates/lyracore-shared/src/gossip.rs`: source kinds -3/-2/-1 for NOT/OR/AND,
    `evaluate_option` maps to 9, 8, 15, 6, 14 and 22, and the team ids 67 and 469 are hard-coded.
  - `module/tests/gossip_conditions.rs` pins kinds 8, 9, 22, -3 and 99.
- **Creature population.** `module/src/creatures/population.rs` mirrors cmangos `spawn_group`
  and pools: `max_count`, world-state condition and expression, flags, `is_pool`, and members
  keyed by `source_guid` with `event_ids`. There's no runtime for game events.
- **Package Delta spatial keys.** `{map_id, entry, spawn_id}`, where `spawn_id` is the source
  spawn guid (`docs/package-delta-format.md:77-88`).
- **Rajaxx.** `module/src/creatures/eventai/loader.rs:273-274` pins the rule id 1_534_108 and the
  guid for entry 15341 with spawn guid 155940. It's mirrored in
  `importer/src/eventai_presentation.rs:13` and `eventai/presentation.rs`.
- **Broadcast text.** `game_creature_ai_broadcast_text` is keyed by the source broadcast_text id.
- **Seed constants.** `module/src/seed.rs:1417` and `seed.rs:60-100` are fixtures and fallbacks.
- **Deadmines bootstrap.** `module/src/instance.rs:23-29`.

## 3. The official Packages

- **No Package ships a Delta with spawn claims.**
- **`dungeons`** uses creature, gameobject and item entries only (`dungeons/src/deadmines.rs`),
  tagged "classic-db z2815". It should survive the switch if the entries match, which is likely
  but unverified.
  - The README and `deadmines_verify.rs` comments name z2815.
  - VanCleef's yells and his ally summon come from imported EventAI rows.
- **`playerbots`** pins the classic-db digest:
  - `src/quest_catalog.rs:12-13` has
    `CATALOG_BLUEPRINT_REVISION = "classicdb:d2083bcd…"` and hard-codes 22 quests with their
    objectives.
  - `fixtures/quest-catalog.json` and `fixtures/actions.json` carry the same digest.
  - `src/imported_fixture.rs:37-80` requires `game_import_meta` rows for 11 families.
  - `src/provisioning.rs` hard-codes item entries.
  - The testsuite hard-codes areatriggers 78 and 119 and the digest.
- **Core still names the dungeons Package** (issue #633): `module/src/tripwires.rs`,
  `module/tests/deadmines_package.rs`, and Deadmines map logic in the instance, graveyard, world
  and importer code.

## 4. Core tests and fixtures

- **Most are synthetic:** entries 51000/51001 and the 5,090,000–5,099,999 band.
- **Real classic-db pins:** Rajaxx, the gossip condition kind numbers and the seed constants.
- **The importer's own tests** are listed in `importer-inventory.md`.

## 5. Docs and verification text

- `README.md:43`, `CONTEXT.md:24` and `:82-84`, `CHANGELOG.md:133` (historical)
- `docs/quickstart.md:322-336`
- `docs/data-ingestion.md` (many lines)
- `docs/adr/0001`-`0003`
- `docs/schema.md:308,471`
- `docs/testing.md:65-66,173,186`
- `docs/recipes/add-a-spell-effect.md:9`
- `.claude/skills/verify-lyracore/checks.md:92`

## 6. Production

- `deploy/` doesn't mention world data.
- `docs/native-operations.md:105-107` forbids rerunning world imports on an existing Realm.
- **Inferred, not traced:** switching sources changes spawn guids, which strands every row keyed by
  the old guids. Moving production over is a separate Operator task and outside the port.
