# Brief: replace classic-db with the vmangos world database

You are porting LyraCore's world-data import from cmangos classic-db to the vmangos world database.
The decisions below were settled with the maintainer on 2026-10-26. Don't reopen them. If the code
contradicts one, stop and report.

Read these first, in this order:

1. `AGENTS.md`, `CODING_STANDARDS.md` and `CORE_TERMS.md`, and the parts of `CONTEXT.md` your slice
   touches.
2. `.claude/skills/unslop/SKILL.md` and `.claude/skills/file-pr/SKILL.md`. They apply to every
   comment, doc, commit and PR you write.
3. The research in this directory:
   - `research/importer-inventory.md`: what the importer reads today;
   - `research/vmangos-schema.md`: the vmangos database and how it differs;
   - `research/downstream.md`: everything outside the importer that depends on classic-db.

   The research was done at core `4ae2618f`. Check every file:line against the code before you
   rely on it.
4. `docs/data-ingestion.md`, `docs/adr/0001` to `0003`, `docs/danger-zones.md` (schema changes) and
   `docs/testing.md`.

## Why

vmangos's world data is considered more accurate than classic-db, because much of it comes from
captured retail packets. After this port, LyraCore builds a 1.12 conformance system that records
vmangos packet traces as the reference. Switching sources first means those traces and the world
data come from the same project.

## Settled decisions

1. **One source.** At the end of the port, vmangos supplies the world data, the server spell
   catalogue, and every table vmangos uses in place of a DBC: `faction`, `faction_template`,
   `area_template`, `map_template`, `taxi_nodes`, `skill_line_ability` and
   `areatrigger_template`. The client's DBCs remain the source for everything vmangos doesn't
   replace, such as terrain, TaxiPath and TaxiPathNode, SpellIcon, and the UI files. classic-db is
   deleted at the end.
2. **Read the SQLite variant** (`db-sqlite-<short>.zip`, `sqlite-dump/mangos.sqlite`) read-only,
   with `rusqlite`, and **by column name**. Never read by position. Never execute vmangos SQL.
   ADR 0003's principle (parse, never execute) stays and gets amended to cover SQLite.
3. **Select for patch and build in one place.** Use constants for patch 10 (1.12) and client build
   5875. Don't add an option for other patches. Implement every selection scheme in
   `research/vmangos-schema.md` §2, including `forbidden_items` and condition type 24 (WOW_PATCH).
4. **Pin a moving snapshot.**
   - vmangos replaces its `db_latest` release in place, and the maintainer accepts that risk.
   - The lock records the accepted vmangos commit and the migration fingerprint (the highest id in
     the `migrations` table, plus the row count), not exact file bytes.
   - The fetch downloads the current `db_latest` SQLite asset and verifies the asset's GitHub
     digest.
   - If the snapshot is newer than the lock, the Source Profile gate decides (decision 8). On a
     pass, the import goes ahead and `game_import_meta` records the actual commit.
   - The downloaded file stays in `.import/`, which git ignores.
5. **Licensing and consent.** The Operator's consent text states the status honestly: GPL-2.0
   migrations from `vmangos/core`, applied on top of base data from `brotalnia/database`, which has
   no license file. LyraCore never redistributes the data, and ADR 0001 stays as it is.
6. **The bar is parity.** Everything that works on classic-db today must work on vmangos. Anything
   vmangos expresses that the Module can't goes into the Compatibility Manifest as unsupported, and
   into `unsupported.md` in this directory. That covers at least:
   - the detail in `creature_spells` slots (chances, delays, targets, flags);
   - formations from `creature_groups`;
   - per-command conditions and targets the Module lacks;
   - patch-aware or scheduled game events;
   - spells with a `script_name`.

   Those become behaviors in the later conformance ledger. Change the Module only where parity
   needs it, or where decision 7 or 9 says so.
7. **The extra spell columns are built now.** Carry `effectBonusCoefficient1-3`, `customFlags` and
   `minTargetLevel` into the Module's spell catalogue as new columns at the end of the table (see
   `docs/danger-zones.md` for safe schema changes), and implement their gameplay meaning, with
   tests. Reimplement the behavior from reading vmangos's source. **Never copy vmangos code.**
   LyraCore is an independent MIT/Apache implementation. Don't port `script_name` spells; list them
   as unsupported.
8. **The Source Profile for a moving snapshot.**
   - A new profile, named like `vmangos-<short commit>`, replaces `cmangos-classic-z2815`.
   - The gate checks two things: a column contract (every table and column the importer reads
     exists, with the expected type) and approved value sets (no event type, command, target type,
     flag bit or condition type outside what the profile approves).
   - Counts compared with the locked snapshot are reported, but they don't decide whether the
     import runs.
   - An unapproved value stops the import with findings, as it does today.
9. **Condition kinds belong to LyraCore.** The importer translates source condition types into a
   LyraCore enum. The Module and `crates/lyracore-shared/src/gossip.rs` stop knowing any source's
   numbering. Update `module/tests/gossip_conditions.rs` to match.
10. **Spells and the Base Snapshot.** The server spell catalogue comes from vmangos `spell_template`
    (the largest `build` ≤ 5875 for each entry). The Datascript Base Snapshot is built from the same
    rows. The client half doesn't change: Package spells still reach the client through the
    player's own `Spell.dbc`.
11. **Mixing sources.** Never take some columns of one table from vmangos and the rest from a DBC.
    Different tables may come from different sources. If a vmangos table lacks a column LyraCore
    uses today, stop work on **that table only**: leave it on its current source, finish every other
    table and slice in the milestone, and report the column, where it's used, and your options in
    the handoff. Don't end the run early for it. This
    rule covers only vmangos data that is missing. A column that only one source has and LyraCore
    doesn't read (for example `link_kind`) isn't a stop. Neither is a defect in today's classic-db
    reader (see the standing rule in S1).

    **The one named exception: `Map`** (decided by the maintainer on 2026-10-27). vmangos
    `map_template` supplies every gameplay column of the Map catalogue. The client's `Map.dbc`
    supplies only `internal_name`, looked up by map id, because that column is the archive
    directory the importer uses to find a map's WDT and ADT files. That's client-asset lookup, like
    terrain, not world data. No other column of `Map` comes from `Map.dbc`.

## Rules

- **Branches.**
  - Base everything on `feat/vmangos-world-data`. Name each slice branch `vmangos/<slice-id>`.
  - Within a milestone, stack the slices: each slice branches from the previous one's head. Its PR
    targets the previous slice's branch; the first slice's PR targets `feat/vmangos-world-data`.
  - You may push these branches and open these PRs without asking. Write PR text with the
    `file-pr` skill.
  - Never merge, never touch `main`, never open a PR into `main`. The orchestrator merges the stack
    after each milestone review. The maintainer merges the final PR into `main`.
- **Other repos.** For the same work in `lyracore-cli` and `LyraCoreProject/packages`, use branches
  named `feat/vmangos-world-data` in those repos, under the same rules. Follow
  `docs/agents/cross-repo-cli.md`, and point this branch's `.lyracore-cli-rev` at the CLI branch's
  SHA.
- **Data.**
  - Never commit, push or upload vmangos data, or any extract of it larger than a test fixture.
    Test fixtures are small, hand-written rows.
  - Keep downloads in `.import/` or in `$HOME/.cache/lyracore-scratch/vmangos-port/`. Treat them as
    untrusted: read them, never execute them.
  - A copy from 2026-09-28 is at `$HOME/.cache/lyracore-scratch/vmangos-db/`.
- **vmangos and cmangos sources.**
  - Never clone either repository, not even a sparse clone, not even into `/tmp`.
  - Read files with `gh api repos/vmangos/core/contents/<path>?ref=<sha>` or raw URLs, and cite the
    SHA.
  - Downloading the `db_latest` release asset is allowed.
- **No outward actions.** Don't post on vmangos, cmangos or brotalnia repositories. Don't file
  issues. Don't touch production or any deployed Realm.
- **Verifying.**
  - Use only a local Realm (`./lyracore dev up`), on your own ports, and only one you started
    yourself.
  - Follow `docs/testing.md`. Before you call a failing test pre-existing, show that it fails the
    same way on the base commit.
  - Build scratch output goes under `$HOME/.cache/lyracore-scratch/vmangos-port/`, never `/tmp`.
    Delete it when you're done.
- **Words.**
  - Use glossary terms. When you add or change a term (for example the source's name, or the
    renamed Source Profile), edit its one entry in `CONTEXT.md` in the same change.
  - Pinned artifacts, such as `cmangos-classic-z2815.json` and `classic-db.lock`, keep their names
    until they're deleted.
- **Comment review.** Complete the comment review in `CODING_STANDARDS.md` on every slice, and
  report its result.
- **Stopping.** Stop and report, rather than guess, when:
  - a decision above doesn't fit what you find;
  - decision 11 triggers;
  - an identifier band collides (slice S4);
  - a change would break the Package API contract (slice S7).

## Measuring parity

Build a census report early (in S1) and keep it current. For each Import Family, compare a
classic-db import with a vmangos import:

- row counts for each Module table;
- the set of entry ids, with the ids added and removed listed;
- the families' consumed floors from `importer/scripts/import-manifest.sh`;
- Compatibility Manifest findings.

Write it to `.claude/tickets/vmangos-port/census/<family>.md`, generated by a command, not by hand.
Every difference larger than a few rows needs a one-line explanation, such as "vmangos drops 1,225
spawns with patch_max < 10".

## Performance limit

Added on 2026-10-26, after the Milestone 1 review measured a 5.4x jump in peak memory. A classic-db
import, and later a vmangos import of the same profile, must stay within **1.25 times** the base
commit's peak memory and wall-clock time, for both `alliance-single` and `full-world`. The base is
470 MiB and 11.5 s for full-world, and 380 MiB and 9.7 s for alliance-single, measured on release
builds. Measure with `/usr/bin/time -v` on release builds, and put the numbers in every milestone
handoff.

## Slices

Each slice is one PR with its tests. Blocking edges are listed per slice. Milestones end with a
handoff to the orchestrator and an adversarial review. **Stop at each milestone.** Don't start the
next milestone in the same run.

### Milestone 1: the seam and the fetch

**S0. Setup.**

- Draft ADR 0004, "Import world data from vmangos". It supersedes ADR 0002 and amends ADR 0003 for
  the SQLite reader. Update it as the slices teach you things.
- Decide the source's term with the `CONTEXT.md` rules, for example a **World Source** entry, and
  add it.

**S1. Read tables by column name.** Blocks everything after it.

- Add `rusqlite` (with the `bundled` feature, unless you find a reason not to) and a vmangos source
  module that opens the SQLite file read-only.
- Rows are read by column name, with typed accessors that fail on a missing column or a type
  mismatch.
- Make the classic-db text reader expose rows by column name too, taking the names from the
  dump's `CREATE TABLE` statements, so both sources share one interface.
- First, as its own slice (S1-pre, `vmangos/s1-deterministic-output`), make the classic-db
  import deterministic. Two runs of the same binary on the same input must produce identical
  dry-run SQL and reducer batches. Decided on 2026-10-26: the gossip text slot ids came from
  `HashSet` iteration (`needed_text_ids` in `world_import.rs`). Sort before assigning ids, and
  check every other hash-ordered loop that feeds output. That deterministic revision is the
  baseline.
- **Standing rule for defects in the current reader** (decided on 2026-10-26, after
  `game_graveyard_zone` was found reading `link_kind` as `faction`). Moving onto column names will
  expose places where the classic-db reader reads the wrong column. These are defects, not
  decision-11 cases, so don't stop for them:
  - Fix each one in its own commit, with a test, on a prerequisite slice
    (`vmangos/s1-classic-reader-fixes`, stacked before the named-column refactor).
  - Rebuild the deterministic baseline after the fixes, and run S1's byte comparison against that.
  - List every fix in the handoff, with the column, the cause and how many rows change.

  Stop only if a fix needs a Module change, or touches something the brief already marks as a
  decision.
- Move every positional index in `dump.rs` and its callers onto names. A classic-db import must
  produce identical output before and after, compared with the S1-pre baseline, so compare the
  dry-run SQL and the reducer batches byte for byte.
- Add the patch and build selection module (decision 3) and the `--source vmangos|classic-db`
  switch, which defaults to classic-db for now.
- Add a test helper that builds a small SQLite database from named rows, so vmangos fixtures stay
  readable.
- Build the census command.

**S2. Fetch and lock.**

- An importer-side fetch script, or a Rust subcommand. Choose whichever fits the CLI's later work
  better, and explain the choice. It downloads the current `db_latest` SQLite asset through the
  GitHub API, verifies the digest, extracts it into `.import/`, and reads the commit and the
  migration fingerprint.
- Write the new lock format. The lock records the accepted snapshot.
- The CLI's consent text belongs to S7, not here.

**Milestone 1 handoff.** Include:

- the classic-db before/after comparison from S1;
- the selection module and its tests;
- the fetch running against the live release;
- a first census of the globals family on vmangos.

### Milestone 2: the simple families

Do one slice per family. Each moves its family's readers to vmangos behind `--source vmangos`,
rewrites the family's tests to use vmangos-shaped named fixtures (keep the classic-db tests until
S8), and updates the family's census. On the vmangos path, never parse a value with a silent
default such as `.parse().unwrap_or(0)`. Use the checked `row.number` accessors, which fail with a
named error. Check parity by the odds and behavior a player would see, not only by row counts.

**S3a. globals.**

- `player_classlevelstats`, `player_levelstats`, the `playercreateinfo*` tables (with build ranges),
  `game_graveyard_zone` (with patch ranges) and `areatrigger_teleport` (versioned by patch).
- **Graveyards link to areas as well as zones.** vmangos's `ghost_zone` can hold a subzone (area)
  id. vmangos looks up the area first and falls back to the zone (`GetClosestGraveYard`,
  `ObjectMgr.cpp:7512-7524` at `4641790`). The Module's `zone_linked` (`module/src/graveyard.rs`)
  matches the zone only.
  - Change the Module for parity (decision 6 allows it): an area match first, then the zone, with
    tests. This is decided, so don't stop for it.
  - Add a one-line explanation in the census for each graveyard and portal difference.

**S3b. items.**

- `item_template` (versioned), `item_enchantment_template`, vendors through
  `creature_template.vendor_id` and the new `npc_vendor` layout, and `forbidden_items`.
- Recheck the drink heuristic (`is_drink_consumable`) and the hard-coded items 25, 50, 51 and 52
  against vmangos data.

**S3c. loot.**

- Every loot table, with patch ranges and references, and `forbidden_items`.

**S3d. gameobjects.**

- `gameobject_template` (versioned) and `gameobject` (patch range, `spawn_flags`, `state`).
- Recheck `GATHER_NODES` and the type 25 rule.
- `transports` (versioned by build) for `transport.rs`, and `go_model.rs`.

**S3e. trainers.**

- `npc_trainer(_template)`, with build ranges, linked through `creature_template.trainer_id`.
- Recheck `PROFESSION_LEARN` against vmangos. vmangos may teach profession tiers differently.

**S3f. quests.**

- `quest_template` (versioned), the relation tables (patch ranges) and
  `areatrigger_involvedrelation`.
- Reward mail moves to `RewMailTemplateId`, `RewMailDelaySecs` and `RewMailMoney`.
- `mail_loot_template`.
- The quest learn spell, from vmangos `spell_template`.

**S3g. gossip and conditions.**

- `broadcast_text` (new layout), `npc_text` (interleaved layout), `gossip_menu` and
  `gossip_menu_option`.
- `conditions`, with decision 9: LyraCore-owned kinds. This is a Module and `lyracore-shared`
  change, with its tests.
- Recheck `RELAY_GOSSIP_MENUS`.

**Milestone 2 handoff:** the census for each family, with explanations.

### Milestone 3: creatures and spells

**S4. creatures.**

- `creature_template` (versioned):
  - Compute health, damage and armor from `creature_classlevelstats` times the template
    multipliers. Reimplement the formula from vmangos `Creature.cpp`, and cite the SHA.
  - Display ids, with scale and probability.
  - Trainer, vendor and gossip links.
  - Find where LyraCore's unit flags and creature type flags come from now. If vmangos has no
    equivalent, decision 11 applies.
- Spawns:
  - patch range and `spawn_flags`;
  - `id`..`id5`, which become population members with equal chance, the way `creature_spawn_entry`
    feeds population today;
  - waypoints from `creature_movement` and `creature_movement_template`;
  - pools with their `flags`;
  - `game_event_creature`.
- `creature_groups` is unsupported.
- Casts: map `creature_spells` onto today's `game_creature_cast` and `game_creature_spell` model at
  parity, and list the slot detail as unsupported.
- **Identifier bands.** vmangos gameobject guids reach 3,998,642, inside the curated reserved band
  1,000,000–5,200,499 (`crates/lyracore-package-delta/src/ids.rs`). Find what that band guards
  and whether spawn ids are subject to it. If they collide, stop and report the options.
- Recheck the hard-coded `INCLUDE_CREATURES` defaults and every creature id the importer hard-codes.

**S5. Spells and the other DBC-replacing tables.** Read decisions 1, 7, 10 and 11 again first.

- **S5a. Diff report.**
  - Compare vmangos `spell_template` (largest build ≤ 5875) with the client's `Spell.dbc`, field
    by field. Do the same for every other vmangos table that replaces a DBC: `faction`,
    `faction_template`, `area_template` with AreaTable, `map_template` with Map,
    `taxi_nodes`, `skill_line_ability` and `areatrigger_template`.
  - Report the counts of differing rows and fields, with examples.
  - Write it to `.claude/tickets/vmangos-port/census/dbc-diff.md`.
  - This report is part of the Milestone 3 handoff, so the maintainer sees what the switch changes.
- **S5b. The spell catalogue moves to vmangos.**
  - `spell.rs` and the code feeding `game_spell`, `game_spell_effect` and `game_spell_reagent`
    read vmangos rows.
  - `spell_snapshot.rs` builds the Base Snapshot from the same rows.
  - Package spell Deltas and `lyracore packages apply` keep working.
  - Run the Datascript checks in `datascripts/` and the spell recipe tests.
- **S5c. spellmeta.** `spell_chain`, `spell_proc_event` and `spell_learn_spell`, all with build
  ranges.
- **S5d. The extra spell columns (decision 7).**
  - Add the new columns.
  - Implement `effectBonusCoefficient` in spell damage and healing.
  - Implement each `customFlags` bit vmangos defines. List the bits, cite their definitions, and
    implement each with a test. A bit that would need a large new system follows decision 11's
    stop rule.
  - Implement `minTargetLevel` as a cast Gate with its Refusal.
  - List the `script_name` spells as unsupported.
- **S5e. The other DBC-replacing tables.** Move each one to vmangos, under decision 11.

**Milestone 3 handoff:**

- the creature census;
- `dbc-diff.md`;
- the spell catalogue switch, and what changed for Datascripts;
- the customFlags list and its tests;
- `unsupported.md` so far.

### Milestone 4: EventAI and scripts

This is the largest piece. Read `module/src/creatures/eventai/model.rs`, `relay.rs` and the
importer's `eventai.rs` first, then `research/vmangos-schema.md` §4.

**S6a. Events.**

- `creature_ai_events` maps onto the Module's EventAI model: an event with up to three action
  lists.
- Each action is a `creature_ai_scripts` command list, with delay 0, mapped onto
  `CreatureInstruction` and `InstructionTarget`.
- Talk uses `broadcast_text` ids.
- Event flags, chance, phase mask, and conditions per event and per command.
- Where the Module's model lacks a command or a target, add it if parity needs it. Otherwise list
  it as unsupported.
- Persisted enums are stored in variant order, so only append variants. Never reorder them.

**S6b. Scripts.**

- Rebuild the relay import around vmangos's command set. START_SCRIPT (39) leads into
  `generic_scripts`, which allows delays.
- Also import `quest_start_scripts`, `quest_end_scripts`, `gossip_scripts`, `event_scripts` and
  `creature_movement_scripts`, wherever today's classic-db import uses the cmangos equivalent.
- `gameobject_scripts` is keyed by gameobject guid. Check that in vmangos source.
- Respect today's limits on nesting depth and step budget.

**S6c. The Source Profile and gate (decision 8).**

- The new profile, its column contract, the approved sets, and the census report.
- Re-derive every hard-coded source id: Rajaxx (in `importer/src/eventai_presentation.rs` and
  `module/src/creatures/eventai/loader.rs`), `encounter_binding`, the display templates and the
  fixture-id refusals.
- Update the Deadmines content the dungeons Package relies on: VanCleef's yells and ally summon.
- **Event-credit quest rule** (decided by the orchestrator on 2026-10-27 under decision 6, after the
  Milestone 4 review). The vmangos quests family adds an event requirement only when the import
  emits a working credit source for that quest: a translated EventAI or script command. Otherwise
  the quest completes by its normal objectives, as it does on classic, and is listed in
  `unsupported.md` with where vmangos grants the credit (SQL that was refused, a script with no
  dispatcher, or C++). No quest that classic can complete may become impossible to complete.
- **Event-credit quests** (from the Milestone 2 review). On vmangos, every quest with
  `SpecialFlags & 2` (176 quests, for example 28/29, 6001/6002 and 2460) needs event credit, and
  only EventAI or scripts can grant it. S6 must give each one a working credit source, or list it
  as unsupported with the reason, before S7a flips the default. Also check the Hallow's End emote
  quests 8353-8360, which lost their requirement.

- **Scripted immunity** (from the Milestone 3 review). vmangos spawns some creatures immune to
  players or NPCs (static flags 0x20 and 0x40), and scripts clear that later. Milestone 3 left those
  flags off for parity, and listed every template in `unsupported.md`. Where the translated vmangos
  scripts set and clear the immunity (for example through MODIFY_FLAGS or a phase change), apply
  the static flags and let the scripts clear them. Templates with no script that clears the flag
  stay unflagged and listed. Test with Archaedas 2748 and Arugal 4275, or similar.
- **Held spell flags.** SEND_CHANNEL_VISUAL (0x0800, 17 spells) and TRIGGER_WEAPON_PROCS (0x2000,
  30 spells) stay listed in `unsupported.md`. Milestone 4 doesn't own them.

**Milestone 4 handoff:**

- the EventAI and scripts census, compared with classic-db's (rules, subjects, relays);
- the approved sets;
- `unsupported.md`;
- a local-Realm run of the Deadmines Package test (`module/tests/deadmines_package.rs`, which needs
  the dungeons Package; see `docs/testing.md`).

### Milestone 5: flip the default, then everything downstream

**S7-pre. Map from vmangos (the decision-11 exception).**

- Import `map_template` (versioned by patch) for every Map column LyraCore reads today. Take
  `internal_name` from the client `Map.dbc` by map id, and refuse any vmangos map that has no
  client row.
- Carry the vmangos-only fields the dbc-diff report found as additive Module columns: `player_limit`
  (26 maps differ from the client), `reset_delay` (7 maps) and the ghost entrance
  (`ghost_entrance_map/x/y`, about 30 maps). Using them in gameplay (instance player limits, raid
  lockouts, issue #533, ghost entrances) is later ledger work. List it in `unsupported.md`, unless
  it's a small parity fix.
- Census, plus a test that terrain lookup still finds every map's archive files.

**S7-ledger. Creature behavior vmangos doesn't hold as data** (decided by the maintainer on
2026-10-27). Some creatures lose classic behavior on vmangos because vmangos runs them on C++ AI
(about 146, for example the Alterac Valley lords, Lady Jaina Proudmoore 4968, The Beast 10430,
Gahz'ranka 15114 and Alzzin 11492), or has no EventAI for them at all (about 414, mostly ambient
speech). These are accepted and don't block the flip.

- Generate a per-creature list in `unsupported.md` from the behavior-parity comparison. For each
  creature give the entry, the name, the behavior lost (speech, casts, summons, phase or HP rules,
  flee or help, despawn or emote, quest credit) and the cause: vmangos C++ AI (name the script),
  no vmangos EventAI, or a refused translation.
- Mark creatures in content that's playable today, starting with dungeons.
- These become Feature Ledger behaviors, rebuilt later from reading vmangos's C++ and never copied.

**S7a. Flip the default to vmangos in the importer.**

- `importer/scripts/import-world.sh`.
- `import-manifest.sh`: retune the floors from the census and explain each change.
- `import-manifest-smoke.sh`.
- The content checks in `import-world.sh` that pin classic-db entries.

**S7b. The CLI**, in `lyracore-cli` on `feat/vmangos-world-data`.

- **Check first:** the 0.1 release work also changes the CLI's import flow. If it hasn't merged
  into the CLI's `main`, stop and report.
- Otherwise:
  - the consent text (decision 5), and update its test;
  - the fetch step;
  - `DUMP_PATH` and the pull command;
  - the content queries that pin classic-db entries;
  - `docs/commands.md`;
  - then bump this branch's `.lyracore-cli-rev`.

**S7c. The Packages**, in `LyraCoreProject/packages` on `feat/vmangos-world-data`.

- playerbots: the catalogue blueprint revision, the 22-quest catalogue (check it against vmangos
  quest data), `imported_fixture.rs`, the fixtures and the testsuite digests.
- dungeons: the README and comments that name z2815.
- **Package API.** Package Delta spatial claims are keyed by source spawn id, and those change. That
  breaks the contract in `docs/package-api.md`. Draft the version bump and the `CHANGELOG.md`
  entry, but stop before any collection tag. Tagging is the maintainer's call.

**S7d. Docs.**

- `docs/data-ingestion.md`, `docs/quickstart.md`, `README.md`, `docs/testing.md` (the
  environment variable), `docs/schema.md`, `docs/recipes/add-a-spell-effect.md` and
  `.claude/skills/verify-lyracore/checks.md`.
- Finish ADR 0004 and settle `CONTEXT.md`.

**S8. Delete classic-db.**

- Remove the text parser in `dump.rs`, `classic-db.lock`, `pull-classic-db.sh` and its test,
  `cmangos-classic-z2815.json`, the `--source` switch, every classic-db test and fixture, and
  `LYRACORE_CLASSIC_DB_SQL`.
- Rename the remaining ignored real-source tests to a vmangos environment variable.
- Move `unsupported.md` to `conformance/unsupported-vmangos.md`. It seeds the Feature Ledger that
  comes after the port. Then delete `.claude/tickets/vmangos-port/` from the branch, so it never
  reaches `main`. The directory is gitignored, so it was force-added.

- **CI pairing cleanup.** Remove the temporary `vmangos/*` → `feat/vmangos-world-data` fallback
  from Core's CI (`.github/workflows/rust.yml`) and the Packages `core-ref` action before the
  final merge. Unpaired PRs into `main` keep the pinned Core revision.
- **Event-requirement ownership.** The vmangos quests family deletes rows in
  `game_quest_event_requirement`, which the creature-ai family owns. With classic-db gone this
  can't erase another source's rows, but give the table one owning family.

**Final handoff.** Include:

- a full vmangos import on a local dev Realm: `./lyracore dev up`, `./lyracore import`,
  `./lyracore dev smoke`, `python3 native-host/check.py`, and the durable tests;
- the final census and `unsupported.md`;
- the list of real-client checks in `verify-lyracore/checks.md` the change affects, for people to
  run;
- the final PR from `feat/vmangos-world-data` into `main`, written with `file-pr`, which the
  maintainer merges.

## Each handoff

Return:

- the PRs, with branch, head SHA and target;
- what you built;
- test commands and their results;
- census changes;
- new entries in `unsupported.md`;
- every stop-rule trigger, with your options;
- the comment review result;
- anything the next milestone must know.

Keep it factual. Separate what you verified from what you inferred.
