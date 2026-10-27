# World data import

This how-to covers the importer: how world content and client data reach a Realm's Shard, what each
pass checks, and what the seeded fixture contains. The everyday path is `./lyracore import`; the
[quickstart](./quickstart.md#getting-the-real-world) covers it. Use the commands below to run one
stage at a time or to inspect a dry run. Why the repository ships tools and no data is recorded in
[ADR 0001](./adr/0001-ship-tools-not-data.md), [ADR 0002](./adr/0002-opt-in-pinned-classic-db-pull.md)
and [ADR 0003](./adr/0003-parse-world-data-never-execute.md). [Danger zones](./danger-zones.md) §2
lists the traps a content import can spring.

Content comes from two places. The Operator's own 1.12.1 client supplies the DBC tables, terrain,
navigation and model geometry, read in memory through `wow-mpq` and `wow_dbc`. cmangos `classic-db`
supplies the world content: creature templates and spawns, items, gameobjects, loot, quests,
gossip text, vendors, trainers and level stats. `importer/scripts/pull-classic-db.sh` fetches it
into the gitignored `.import/`.

Exact collision geometry has its own live-acceptance contract in
[`vmap-rollout.md`](./operations/vmap-rollout.md). Importing it never turns on collision.

## Transitional World Source readers

The vmangos port keeps `--source classic-db` as the default. The classic-db reader takes column
names and types from `CREATE TABLE`, checks its column contract before planning, and never executes
the source SQL. The SQLite reader opens the file read-only, disables trusted schemas, refuses views
and virtual tables, and checks requested field types. Both readers return the same named row type.
The full classic-db importer requires the tables listed in `importer/src/classic_columns.rs`.
`WorldSource::ClassicDb` uses that same contract-checked reader. Family builders return reader
errors before they emit a plan. Classic fixtures check the same numeric values and declarations.

`--source vmangos --family creatures|casts|globals|items|loot|gameobjects|trainers|quests|gossip` imports the selected SQLite rows for those families.
Other families are still being ported. Both world profiles use the same globals and item catalogues.
Vendor offers and creature loot follow the selected profile. Generate the comparison
with both local sources and your client:

```sh
cargo run -p lyracore-importer -- \
  --source vmangos --dump ".import/vmangos/<commit>-<digest-prefix>/mangos.sqlite" \
  --census .import/classic-db-full.sql --family globals --dbc /path/to/client/Data \
  > .claude/tickets/vmangos-port/census/globals.md
```

The report checks Module counts against the production globals plan and lists added and removed
natural keys. It compares the globals count floors from `import-manifest.sh`. Starter extras exclude client
outfits and repeated item keys. These are import-plan measurements, not observations of a Realm.
The existing EventAI Compatibility Manifest remains classic-db-only until S6c.

The vmangos globals Compatibility Manifest reports portal conditions that the Module cannot apply.
Those portals remain in the catalogue at current parity. The Module resolves graveyards from area
links first, then zone links when no area candidate meets its faction and destination checks. This
follows `GetClosestGraveYard` in [ObjectMgr.cpp at vmangos commit
464179081673cfd240f7ffe7f0daf96bca0b5a70](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/ObjectMgr.cpp#L7512-L7524).
The existing dungeon entrance and static fallbacks remain after the imported links.

The `creatures` and `casts` vmangos families use selected creature templates, spawn ranges,
entry choices, paths and spell lists. The creature plan preserves signed event membership and
pool limits. Nested pools that the current population model cannot express stay inactive and
appear in the Compatibility Manifest. Game-event schedules and formations remain unsupported.

Creature health is rounded after the class-level health and template multiplier are multiplied.
Melee damage uses class-level damage, its multiplier and variance; armor uses class-level armor
and its multiplier. The importer stores every level in the template's range in
`game_creature_level_stats`. The Module uses those final values without its classic rank scaling.
The source's default rank rates are all one. Levels above the source stat range use the source's
linear extrapolation from level one to its highest level. Five unplaced level-zero templates at
the pin have no defined stat lookup and are omitted with findings. A placed level-zero template
stops the plan.

These rules follow `InitStatsForLevel` in
[Creature.cpp](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Objects/Creature.cpp#L1791),
`LoadCreatureClassLevelStats` in
[ObjectMgr.cpp](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/ObjectMgr.cpp#L2090)
and the rank-rate defaults in
[World.cpp](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/World.cpp#L514).
The integer Module damage columns round the two derived damage bounds. Display rows retain source
weights and scales, resolving zero scales through CreatureDisplayInfo. Unit and client creature
type flags are derived from the source static flags. Trainer, vendor and gossip joins retain their
existing family ownership.

The cast plan joins `creature_template.spell_list_id` to `creature_spells`. It uses the first
available source spell as the primary cast and the existing rotation model for the remaining
spells. Slot probability, timing, target, flags and script detail remain unsupported. S6 supplies
EventAI dependency closure; a complete vmangos world import is still unavailable at this stage.

The items import joins shared vendor offers through `creature_template.vendor_id` and removes
selected `forbidden_items` from vendor offers and creature loot. The unused `restores_power` flag is false for vmangos; food and drink behavior comes from item
on-use spells. Property Pool weights preserve four
decimal places with a common integer scale of 10,000. The Module uses relative weights.

Use `--family items --world-profile full-world` or `alliance-single` with the census command above.
It reports production plan counts and entry membership. Static WOW_PATCH conditions remove false offers and clear true conditions. Vendor stock timing, runtime offer conditions and
item flags are unsupported and appear in the Compatibility Manifest. The reader projects the
columns needed for narrow joins and spell checks, retaining the keys and fields needed for the
same patch/build selection. It never substitutes client spell fields into vmangos rows.

The loot family imports pickpocketing, skinning, chest and fishing tables after patch selection.
Creature loot stays in items and mail loot stays in quests. Forbidden items are removed before
reference expansion. vmangos zero-chance group members share the chance left by explicit members,
as described by `LootGroup::Roll` in [LootMgr.cpp at commit
464179081673cfd240f7ffe7f0daf96bca0b5a70](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/LootMgr.cpp#L1092-L1128).
Group-zero reference children roll independently. Each reference processes its selected group or
whole template `maxcount` times, with separate groups for each roll. Largest-remainder allocation
keeps each group's basis-point total equal to its intended share.
The classic-db output remains unchanged. Runtime conditions, variable counts and quest-only skinning or fishing drops
appear in the Compatibility Manifest. Item-container and disenchant tables have no current world
import path; the manifest counts them as unsupported.

The gameobjects import selects template patches and spawn ranges, skips disabled spawns, and
translates source READY/ACTIVE states to the Module's closed/open states. A source ACTIVE chest
starts lootable because vmangos tracks its loot state separately from its animation state. Native packets convert
these back to the client's numbering. The eight authored gather entries retain their skill and
item rules; real type-25 fishing holes remain unsupported. Game-event schedules, pool selection,
remaining spawn flags, animation, visibility, respawn ranges and script names appear in the
Compatibility Manifest. The import retains those placements at current parity.

Transports use selected build rows and template versions with the client's TaxiPath and
TaxiPathNode tables. `--go-models <client/Data> --source vmangos --dump <sqlite>` resolves door and
button meshes through the same template reader. The gameobjects census includes these two
companion catalogues, plus planned spawn counts for each consumed map floor.

The trainers family joins shared offers through `creature_template.trainer_id`. Shared offers
win when the same spell also appears on the creature's direct list. It keeps source wrapper spell
IDs and uses the selected wrapper's spell level when the offer has no required level. This follows
`LoadTrainers` in [ObjectMgr.cpp at the pinned commit](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/ObjectMgr.cpp#L10600-L10744).

vmangos profession offers come from its skill-step effects. Constant steps 1 through 4 grant
75 through 300 ranks, following [EffectLearnSkill at the same commit](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Spells/SpellEffects.cpp#L2959-L2975).
Buying a profession offer grants both its skill cap and its resolved learned spell.
The vmangos import adds no synthetic profession offers. Its manifest compares the selected skill
targets with the classic-db `PROFESSION_LEARN` list and reports source skill prerequisites and
multiple learn targets that the current Module cannot represent. Classic-db keeps its existing
trainer output.

The quests family selects quest patches and relation ranges. Full profiles share the full quest
catalogue; other canonical profiles share the existing starter and instance catalogue. Giver
relations use the selected profile's enabled creature choices and GameObjects. Source exploration
or event flags add an event-credit requirement unless an area-trigger objective already supplies
the gate. This follows [QuestSpecialFlags at the pin](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/QuestDef.h#L166-L167).
Source script delivery and EventAI dependency closure remain part of S6.

Reward mail uses `RewMailTemplateId`, `RewMailDelaySecs` and `RewMailMoney`. Sender zero retains the
Module's quest-ender sender rule. Used mail loot must be one unconditional item at 100% chance.
Quest learn rewards use single-effect wrappers from the selected vmangos spell table. `RewXP`
supplies explicit rewards, following [Quest::XPValue at the pin](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/QuestDef.cpp#L180-L201).
The Module treats zero XP as a fallback request, so explicit zero-XP quests remain unsupported.

The Compatibility Manifest also reports unavailable collect objectives, non-goober GameObject
targets, extra prerequisites, scripts, flags, reward casts and reputation fields that the Module
does not carry. Classic-db retains its current SQL and reducer output, including empty strings
for nullable quest text.

`importer/src/source_selection.rs` owns patch 10 and build 5875 selection. Versioned tables keep the
greatest eligible patch or build per primary key, ordered by numeric key. Ranges include both
endpoints; patch-minimum-only and build-minimum-only tables have no upper bound. `skill_line_ability` requires build 5875. `forbidden_items` includes the
boundary in both directions. WOW_PATCH condition type 24 supports equality, at-least and at-most,
and refuses values outside patches 0 through 10 or comparison codes 0 through 2. Family ports use
the selected forbidden-item rows to remove vendor and loot entries; condition translation is S3g.

The gossip reader resolves interleaved NPC text slots through `broadcast_text`, keeping gendered
strings and probabilities. It imports all source menus as profiles instead of the classic-db four-menu
relay list. At the pin, 13 script-command-84 targets exist in that catalogue. The command's meaning is
specified in [ScriptCommands.h at the pin](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Maps/ScriptCommands.h#L316).
Importing these profiles does not import the scripts that select them. That work remains for S6.
Creature options use source NPC flag masks and menu-zero fallback, following
[Player::PrepareGossipMenu at the pin](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Objects/Player.cpp#L11972-L12021).
Profile options cannot express those masks. Spawn-specific greetings, dynamic greeting choices,
confirmations, POIs, scripts and broadcast metadata are reported in the Compatibility Manifest. Payment and text
input options are unavailable because the Module cannot enforce those gates.

The native gossip handler follows positive submenu targets on gossip actions and evaluates the
new menu's conditions before it sends options. Talent reset requires a class trainer of the
Character's class and level 10 or above. Classic-db's inline talent-reset option still works.
Greeting fallback follows [the pinned source](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Objects/Player.cpp#L12330-L12360):
the last unconditional greeting wins; a menu with none uses `0xffffff`. Conditional greetings
remain unsupported.

`lyracore-shared::gossip::ConditionKind` owns the runtime meanings and persistent encoding. Its codes
retain existing Realm values so classic-db SQL, reducer batches and stored rows stay compatible.
Only the importer translates source numbers. Both sources map their supported Boolean, quest, level,
team and race/class predicates into this enum. vmangos patch predicates become constant predicates
at import time, with inversion preserved. Other vmangos kinds become an unavailable sentinel;
unsupported classic-db codes remain opaque for byte parity. Unknown flags, missing references and
unsupported predicates cannot become eligible through inversion or OR. Condition rows keep their schema.
See [Conditions.h at the pin](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Conditions.h#L30-L138)
when extending the importer mapping, and add a named runtime kind before supporting a new meaning.

The forbidden-item rules follow [ObjectMgr.cpp at vmangos commit
464179081673cfd240f7ffe7f0daf96bca0b5a70](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/ObjectMgr.cpp#L10789).
WOW_PATCH follows [Conditions.cpp at the same commit](https://github.com/vmangos/core/blob/464179081673cfd240f7ffe7f0daf96bca0b5a70/src/game/Conditions.cpp#L316).
The Rust implementation is independent. Tests cover selection boundaries, composite keys, duplicate
revisions, invalid ranges, each WOW_PATCH comparison and the read-only SQLite boundary.

## Fetching the transitional vmangos snapshot

Run `python3 importer/scripts/pull-vmangos.py`. Its default output path resolves from the script
location to the checkout's ignored `.import/vmangos/`, regardless of the working directory. It reads
the current `db_latest` release through GitHub's API, resolves the commit in the SQLite asset name to
a full SHA, downloads that asset through the asset API, and checks GitHub's SHA-256 digest before
extraction. It reads only
`sqlite-dump/mangos.sqlite` from the zip. Other members are ignored, and a duplicate or symlink
SQLite member fails the fetch. The migrations query opens SQLite read-only with trusted schemas
disabled.

The fetch uses Python's standard library so the later CLI can call one importer-side script without
requiring `gh`, a git clone, or new HTTP and archive dependencies in the Rust importer. It replaces
the old fetch step when S7 changes the CLI. The CLI's consent text is also an S7 change.

`importer/scripts/vmangos.lock` records format version 1, the repository, accepted full commit, and
migration fingerprint. The fingerprint is the highest migration id plus the row count. File hashes
are download-integrity evidence, not snapshot acceptance. To accept a reviewed snapshot, run the
same script with `--accept-lock` and review the lock diff.

The archive and SQLite file stay under `.import/vmangos/<commit>-<digest-prefix>/`. The script writes
`.import/vmangos/current.json` with their paths, the actual commit, migration fingerprint, GitHub
digest and whether the accepted lock matches. For a changed commit, GitHub ancestry determines
whether the snapshot is newer, older or diverged from the previous lock. A changed migration
fingerprint at the same commit is reported without a direction. The lock stays unchanged unless
`--accept-lock` is supplied. Fetch does not evaluate the Source Profile or permit apply. S6c must
check the actual snapshot's column contract and approved values before a later CLI imports it, then
record its actual commit. If GitHub cannot compare an unreachable accepted commit and returns
404, `--accept-lock` can replace that lock after the archive and migrations checks pass.

Use the `sqlite_path` and `commit` from `current.json` as the census's `--dump` and `--source-sha`.
The source switch remains classic-db by default. No downloaded data belongs in a commit or PR.

## Full source import

`--world-profile` selects the World Import Scope. `lyracore import` uses `alliance-single` on
`lyracore`; `full-world` imports every source map into one database. `full-eastern`,
`full-kalimdor` and `full-instances` still parse; they are the legacy multi-Shard scopes. Each full profile imports
every creature definition and builds a shared quest catalogue from the full source. Runnable AI
follows the imported creature populations and their summon dependencies, and quest-giver relations
follow the imported spawns. Reimports replace each selected family; two runs do not combine
spatial scopes.

For a complete source import, supply the dump and client data together:

```sh
cargo run -p lyracore-importer -- \
  --dump .import/classic-db-full.sql --dbc /path/to/client/Data \
  --world-profile full-world
```

This prints the plan. Add `--apply --server <local-node> --db <database>` to load it. Full profiles
load the client reference catalogues from the same dump and client inputs, including taxi mount
models. They do not require a separate `--taxi-mounts` argument. A complete import with `--dbc`
loads the full Spell catalogue before world content on every profile, because the Module refuses
EventAI relay definitions that name a missing spell. The script also loads talents, terrain and
navigation:

```sh
WORLD_PROFILE=full-world DUMP=.import/classic-db-full.sql DBC=/path/to/client/Data \
  DB=local-full-world SPACETIME_SERVER=http://127.0.0.1:3000 \
  bash importer/scripts/import-world.sh
```

All authored creature positions are retained, including nonselectable creatures and population
alternatives. Population metadata retains entry choices, count limits, chances and event links.
Legacy creature pools retain fractional chances and template-based membership. Equal-sized,
equal-chance nested pools with single-selection limits flatten to the same initial distribution.
Other nesting and mixed creature/GameObject pools stop the import before it writes data.
The Module selects an initial population from that metadata. Selections last until reimport or
Instance teardown; source pool rotation after deaths is not yet implemented. Positive game-event
members and groups with world-state conditions remain dormant until an event system can evaluate
them. Negative event members use the inactive-event state. Conditional squad positions with no
SQL entry choices remain inspectable source rows with entry zero and never become live entities.

Dungeon, raid and battleground source rows stay dormant in Instance zero. Creating a dungeon or
raid Instance copies its selected creatures and patrols into that Instance. The copies respawn
and are removed with the Instance. This supplies map populations; it does not supply missing
encounter scripts, raid lockouts or battleground rules. Continent terrain covers the full client
grid; imported spawn coverage does not imply complete collision coverage for every interior.
Zone-wide creature speech requires imported terrain and area metadata to resolve its audience.
Reimport clears creature copies in existing Instances but keeps their Instance leases. Recreate
those Instances before play resumes.

The EventAI compatibility gate still applies to the full catalogue. A larger scope does not
approve dropped rules or unknown behavior. Existing smaller profiles retain their shared starting
quest catalogue and their local quest-giver relations.

## Creature EventAI family

`--family creature-ai` compiles source rows into Module-owned native definitions. The selected
EventAI Source Profile defaults to `cmangos-classic-z2815` and is committed at
`importer/manifests/cmangos-classic-z2815.json`. That profile pins:

- CMaNGOS Classic commit `ca0775fe352fb67a0e82dc6051f0563187d569a5` and its 24-column,
  source-id-ordered EventAI loader contract;
- classic-db commit `cd0c426a3b2ff56dd518bf009025299468e60fdb` and
  `Full_DB/ClassicDB_1_12_1_z2815.sql.gz`;
- SHA-256 `d2083bcd2670451279cbf93af138eadae04c6d183a4cd0ff0357047e4a565de6` over the exact
  decompressed SQL bytes;
- 10,843 source rules, including 39 negative source-guid rules.

The pull lock uses the same classic-db snapshot and digest. The pull script writes the decompressed
bytes without adding a delimiter. A later dump with the same z2815 filename is not the same profile
unless its digest and censuses match.

Run a report without `--apply`:

```sh
./target/debug/lyracore-importer \
  --dump .import/classic-db-full.sql \
  --family creature-ai \
  --eventai-profile cmangos-classic-z2815
```

The dry run prints the complete scoped Compatibility Manifest, including every unapproved result.
It records source, emitted instruction, normalized, excluded, and dropped counts separately. Event,
action, target, cast-flag, EventAI-flag, and dependency results remain grouped by source value and
reason. Dependency results retain the path from source rule and instruction slot to the referenced
catalogue value.

Apply constructs and checks that same plan before it returns any work to the caller. A wrong digest,
loader contract, census, source value, dependency, or classification is a Refusal before any EventAI
SQL statement or definition reducer call. This is intentional. A profile can remain useful for dry
run while later compatibility Tickets approve more of the pinned source.

World Import Scope still controls compatibility claims. Positive subjects must name imported entry
templates. Negative subjects resolve through an imported spawn guid and become guid subjects.
Accepted summon dependencies enlarge the scope to a fixpoint. An out-of-scope rule is reported as
such and is not claimed as compatible.

One durable definition row owns one entry or guid subject. It contains source-ordered
`EventAiRule` values, and each rule contains an ordered `Vec<CreatureInstruction>`. The source parser
still reads three source action slots. The native definition has no three-instruction limit. Entry
and guid definitions compose by source rule id, with entry order retained for an equal id.

The family replaces native definitions through the operator-only
`import_creature_ai_definitions` and `import_creature_ai_definitions_append` reducers. Broadcast text
and summon locations remain family-only SQL catalogues. The old flat `game_creature_ai_event` table
is kept for migration safety and is not an evaluator boundary.

Each subject revision is derived from its normalized native definition. A creature compares its
durable revision on its next behavior visit. A mismatch clears that creature's phase, Ranged
Posture, and Rule State before evaluation, then stamps the new revision. Cleanup does not scan the
Shard, touch another creature, replay an edge event, or undo a committed one-shot outcome.
Unchanged definitions keep their revision and Rule State across scheduled Module calls.

The current instruction compiler preserves the speech, emote, cast, phase, flee, call-for-help,
summon, and Ranged Posture behavior already supported by the Module. Its source mappings follow the
pinned CMaNGOS declarations and loader:
[event, action, target, and flag values](https://github.com/cmangos/mangos-classic/blob/ca0775fe352fb67a0e82dc6051f0563187d569a5/src/game/AI/EventAI/CreatureEventAI.h) and
[source tuple load behavior](https://github.com/cmangos/mangos-classic/blob/ca0775fe352fb67a0e82dc6051f0563187d569a5/src/game/AI/EventAI/CreatureEventAIMgr.cpp).

After import, `import-world.sh` verifies that native definition subjects are well formed and unique.
The Compatibility Manifest remains the authority for source values and dependency paths. Real-client
Verification should cover one source-backed rule and one creature with no native definition.

## Client catalogues and Transports

The standalone DBC pass reads all three taxi files through the same in-memory MPQ patch chain as
the other client tables. It validates every endpoint/path reference and every `(path, node_index)`
ordinal, client id against the vanilla 1..=256 mask range, reserved wire-id collisions, and negative
delays before issuing SQL. Apply is a recoverable clear-and-reload: points are cleared before paths
and nodes, then imported rows and the reserved fixture are inserted deterministically. A database
failure during those statements can leave a partial family, but `import-world.sh` stops on that
failure and the next run clears the family again before rebuilding it. No DBC bytes or generated
catalogue artifact is written into the repository.

TaxiNodes mount fields name creature-template entries, despite the `wow_dbc` field name
`mount_creature_display_info`. The standalone DBC pass requires `--taxi-mounts <dump.sql[.gz]>`
and resolves them through the full dump, not the spatially filtered creature catalogue, so import
order does not depend on which mount creatures have spawns in scope. The world import script
passes its existing `DUMP` input.

For each faction, a zero creature entry selects the other faction's entry before lookup. A missing
template, a template with no model, or a selected model absent from `CreatureDisplayInfo` produces
display zero and a warning. The Module then refuses that faction's flight before charging the fare.
An explicit missing entry does not fall back to the other faction. Empty template input, unfinished
INSERT statements, duplicate entries and invalid entry or model fields fail before any DBC catalogue
SQL runs.

The catalogue stores the first nonzero model in the template's four model slots. This deterministic
choice omits the reference's random model and gender selection. The creature-entry lookup and
faction fallback follow [CMaNGOS Classic's taxi mount selection](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Globals/ObjectMgr.cpp#L6500-L6534).

The same pass builds the Transport catalogue, `game_transport`. Each row of the dump's `transports`
table names a type-15 `gameobject_template`. Its `data0`, `data1` and `data2` give the TaxiPath,
speed and acceleration, and `transports.period` gives the client's cycle in milliseconds. `map_id`
is the map of the path's first node. A missing template, a different GameObject type, a path
without imported points, or a zero speed, acceleration or period is omitted with a warning.

Existing catalogues need the standalone DBC pass again with the same client and full dump. Updating
the binary alone does not repair stored displays. With flights stopped, an Operator can rebuild
them on the Shard with explicit targets:

```bash
cargo run -p lyracore-importer -- --server "$SPACETIME_SERVER" --db "$DB" \
  --dbc "$DBC" --taxi-mounts "$DUMP"
# Review the dry run, then repeat with --apply during the import maintenance window.
```

This reloads all standalone DBC catalogues, including taxi nodes, paths and points. It preserves the
reserved taxi fixture. Flights already active retain their captured mount display until they end.

## What the seeded fixture contains

`init` never reads a world-database dump, but not every id in the seeded fixture is invented. The
1.12.1 client renders items, spells and NPCs by numeric id against its own DBCs and item cache. A
fixture with fake ids would show blank tooltips, or collide with a real id that a later import
brings in. So the fixture reuses real vanilla ids and, for a few rows, real names and coordinates:

- Items: a few dozen real ids so names, icons and tooltips render, such as 25 Worn Shortsword,
  6948 Hearthstone, 769 Chunk of Boar Meat and 2318 Light Leather. The starter-kit and
  profession-item blocks in `module/src/seed.rs` list them.
- Spells: about a dozen real ids used by the starter kit and profession recipes, such as 772 Rend
  and 133 Fireball.
- Gameobjects: the Goldshire gather-node demo, entries 1731 and 1732 (Copper and Tin Vein).
- Graveyards: five real Elwynn and Westfall world-safe-locations with their names and coordinates,
  in `module/src/seed.rs` and as fallback constants in `module/src/world.rs`.
- The start position: the Northshire Valley coordinates where a new Human Warrior spawns.
- The taxi fixture: two reserved nodes, one one-way route with three points and a fare, and a
  nearby flight master. [Schema](./schema.md#7-the-seeded-fixture) gives its id rules.
- A few quest and NPC display strings used by the test fixtures.

Each row is typed by hand, from facts about the 1.12.1 client and the public game world, not copied
from a third-party database. Content at the scale of a `classic-db` import, hundreds of creature
templates, quests or loot rows, exists only where an Operator ran `./lyracore import` against their
own client and dump.

## Starter aura families: provenance

`module/src/seed.rs`'s `seed_spell_groups` hand-authors the mutually exclusive aura families. This
records what its rows were checked against, because "hand-authored" is not the same as "unverified"
and the two must not be conflated.

**Reference source.** The Operator supplies build 5875 client archives. The importer reads
`DBFilesClient/Spell.dbc` in memory through `wow-mpq`. `importer/src/spell_dbc.rs` checks the 173-field,
692-byte layout and reads the columns shared by spell and item enchantment import. Other DBC tables
still use `wow_dbc`. No client bytes enter the repository.

The Spell layout follows the pinned [SpellEntry definition](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Server/DBCStructure.h).
`wow_dbc` 0.3 omits column 21, InterruptFlags. Its named fields stay shifted through the effect arrays.
An extra field restores alignment at the localized name, column 120.

| Consumed fields | Build 5875 columns |
|---|---|
| ID, school, dispel, mechanic | 0, 1, 4, 5 |
| Attributes, AttributesEx1, AttributesEx2, stance mask | 6, 7, 8, 11 |
| Explicit target flags | 13 |
| Required spell focus, cast time, recovery times | 15, 18, 19, 20 |
| Aura interrupt, Proc mask, chance, charges | 22, 24, 25, 26 |
| Maximum level, spell level, duration, power, cost, range, stack count | 27, 29, 30, 31, 32, 36, 39 |
| Reagents and counts, equipment class and masks | 42..49, 50..57, 58..60 |
| Effects, die sides, base dice, dice scaling, level scaling, base points | 61..63, 64..66, 67..69, 70..72, 73..75, 76..78 |
| Mechanics, target A, target B, radii, auras, amplitudes | 79..81, 82..84, 85..87, 88..90, 91..93, 94..96 |
| Multipliers, chain targets, item references, miscellaneous values, triggered spells, combo scaling | 97..99, 100..102, 103..105, 106..108, 109..111, 112..114 |
| English name, family, complete 64-bit family mask | 120, 160, 161..162 |

Spell output retains its existing magnitude convention, base points plus one, and auxiliary-table
range, radius, duration and cast-time resolution. Item enchantments retain base points plus base dice.
The normalized input preserves all three slots and raw aura numbers. Unsupported effects remain
`E_SCRIPTED`, and unsupported item enchantment effects remain Stat Kind 0.

[Target definitions](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/SpellTargetDefines.h)
define 6 as enemy and 21 and 57 as friendly units. The pinned
[target classification](https://github.com/cmangos/mangos-classic/blob/8ec338a1704e7dcb1c0213eb7ed58f9231ade40f/src/game/Spells/SpellTargets.cpp)
marks codes 9, 10, 17, 18, 22, 32, 39, 41 through 44, 46 through 50, 53, 55 and 63 as locations.
When a location in target A has a companion target B, target B decides the Module's unit routing.
Code 45 is a friendly chain unit and code 26 is a lock, so neither uses that rule. Code 0 means no
implicit target. For a `(0, 0)` pair, the importer uses compatible Spell header target flags, then
the source effect's default target. An unsupported effect keeps the scripted fallback. The importer
translates vanilla damage and movement interrupt bits into the Module's compact mask. Gouge's control
and Stealth's speed penalty come from the third source effect. Their former synthetic additions are
removed. The Human Spirit maps directly to its percentage aura. Evocation's existing periodic-mana
correction now addresses source slot 0.
The authored seal marker and Power Word: Shield's linked Weakened Soul rule remain because their
Module behavior is not an equivalent DBC effect.

The binding summon correction follows the
[pinned ClassicDB snapshot](https://github.com/cmangos/classic-db/blob/cd0c426a3b2ff56dd518bf009025299468e60fdb/Full_DB/ClassicDB_1_12_1_z2815.sql.gz).
Spell 7728 sends event 1131 for creature 5676; spell 8674 sends event 1134 for creature 5677.
Both require spell focus 83, and their event scripts set a 360000 ms despawn delay.
[Their EventAI rows](https://github.com/cmangos/classic-db/blob/cd0c426a3b2ff56dd518bf009025299468e60fdb/ACID/acid_classic.sql#L18699-L18705)
set faction 14 and attack the summoner. The importer maps these source events to `E_SUMMON_HOSTILE`
and preserves the focus requirement.

Every World Import Profile loads the same full Spell catalogue. Publishing the Module alone does not
correct stored Spell rows; that needs the Spell reimport and the curated class-spell pass.

**Not used, deliberately:** cmangos `spell_group` / `spell_group_stack_rules`. That dataset would
answer the rule questions directly, but it is GPL-licensed content, and transcribing its group assignments into this repository is the
copy [ADR 0001](./adr/0001-ship-tools-not-data.md) forbids. Operators who run the importer against their own dump get it locally; the shipped
starter set does not.

**Verified against the client (membership):** every id in all nine families, by spell name and rank
subtext. Corrections this produced:

| Family | Correction |
|---|---|
| Mark of the Wild | Rank 4 (`5234`) was missing from an otherwise complete 7-rank chain. |
| Paladin Blessings | Added Might rank 7 (`25291`), Wisdom rank 6 (`25290`), Sanctuary ranks 2-4 (`20912`/`20913`/`20914`), and the eight Greater Blessings (a Greater Blessing is the same buff as its single-target form). |
| Armor debuffs | Added Faerie Fire (Feral) ranks 1-4 (`16857`/`17390`/`17391`/`17392`), the druid's form-shifted cast of the same debuff. |
| Well Fed | Added the real food-buff ids alongside the sandbox's synthetic `50116`. |
| Fortitude, Battle Shout, Intellect, Spirit, Shadow Protection | No membership change. The existing ids matched the client. |

**Verified by reasoning over the client's own magnitudes (comparability):** `rank_is_comparable` was
`true` for eight of nine families and is now `true` only for Battle Shout, the sole family built from a
single rank chain. The client makes the bug concrete: Prayer of Fortitude rank 1 grants the same
stamina as Power Word: Fortitude rank 5, and rank 2 the same as rank 6, so comparing the two chains'
rank numbers would have refused a correct Prayer of Fortitude rank 2 in favour of a Power Word rank 6
of identical strength. Gift of the Wild ranks 1-2 sit at Mark of the Wild ranks 6-7 the same way. Those
families now compare effect magnitude, with an existing aura's stack count folded in.

**Unverified, and marked as such:**

- **Rule assignment per family.** `Spell.dbc` does not encode aura exclusivity; that lives in
  server-side data. The assignments (EXCLUSIVE_STRONGER everywhere except per-caster Blessings and
  latest-wins Well Fed) rest on observed 1.12.1 behaviour, not on a reference table.
- **Whether Blessing of Freedom, Protection, and Sacrifice share the Blessing exclusivity.** They are
  omitted rather than guessed at.
- **Expose Armor's magnitude**, which the client stores as a combo-point coefficient rather than a flat
  value. The policy tests use its 5-combo-point value.

The policy tests in `module/src/spell/stacking.rs` and `module/tests/aura_tick.rs` cover the families.
