#!/usr/bin/env bash
# shellcheck disable=SC2034 # file-wide: this file's entire job is to `source`-export FLOOR_*/GATE_*
# constants for import-world.sh and wire-suite.sh to read (see below) — nothing in THIS file reads
# them, so ShellCheck sees every single one as "unused" when it's the intended, documented shape.
# import-manifest.sh — the single source of truth for the numeric constants that decide whether a
# world import SUCCEEDED. Nothing here runs anything: it is a data file that other scripts `source`.
#
# YOU DO NOT RUN THIS FILE. You edit it, when a floor below is wrong for YOUR dump. It is sourced by
# importer/scripts/import-world.sh (which turns each FLOOR_* into a post-import assertion) and by the
# wire test harness, which lives in its own repository and is pinned here by release.
#
# TUNING IT IS EXPECTED, NOT EXCEPTIONAL. These numbers were measured against one operator's dump and
# one operator's client. Yours will differ. When an import prints a FAIL for a family that plainly did
# land, lower the floor here; when your real counts are far above a floor, raise it, because a floor
# far below reality catches nothing.
#
# [V] on a value below means: measured or guessed against ONE operator's dump/client, not verified
# against yours. Confirm it against your own data before relying on it; correct a wrong id rather than
# loosening the check around it.
#
# NOT a dedup of one idea — TWO DISJOINT CONSTANT CLASSES live here, kept in their own sections:
#
#   FLOOR_*  row-count MINIMUMS `import-world.sh`'s `chk` assertions compare an ETL run's actual
#            counts against (e.g. "at least 100 gather-node GOs landed"). One FLOOR_* per `chk` line
#            in `importer/scripts/import-world.sh`. The surrounding prose/rationale for each floor
#            stays in `import-world.sh` next to its `chk` call; this file carries only the numbers.
#
#   GATE_*   PRESENCE PROBE ids the wire suite's data-gate section uses to detect
#            "is this a mock-seed sandbox or a fully-imported node" (e.g. "does game_spell contain id
#            635 at all") — a specific row's EXISTENCE, not a count minimum. Different question,
#            different shape (an id to look up, not a threshold to compare against) — hence the
#            separate section, not a merge into FLOOR_*.
#
# KEY=value only (shell-sourceable, no command substitution) — `source importer/scripts/import-manifest.sh`
# from either consumer. See the source-smoke test (import-manifest-smoke.sh) for the "every consumed
# key is defined here" check both consumers rely on.
#
# PROFILE VOCABULARY: WORLD_PROFILE selects one canonical WorldImportProfile in the importer. These
# names and planned maps intentionally carry no rectangles or samples: world_import_scope.rs is the
# sole catalogue for spatial scope. The map lists are only the script's post-import ownership check.
WORLD_PROFILES="alliance-eastern alliance-kalimdor alliance-single instances"
PROFILE_ALLIANCE_EASTERN_MAPS="0"
PROFILE_ALLIANCE_KALIMDOR_MAPS="1"
PROFILE_ALLIANCE_SINGLE_MAPS="0 1 36"
PROFILE_INSTANCES_MAPS="36"

# MAP-AWARENESS: every count floor below is tuned to the MAP-0 Elwynn/Westfall box. A continent import
# for another map (`MAP=1` — Kalimdor on its own world shard) would fail ~40 of them for content that
# legitimately isn't there, which is noise, not a regression signal — so the tail of the FLOOR_*
# section re-points the BOX-DEPENDENT floors at conservative presence values when `MAP` is set to
# anything but 0. Map-INDEPENDENT floors (the DBC tables, Spell.dbc, the world-wide dump globals) are
# NOT overridden: they must land identically on every shard.
set -uo pipefail

# =====================================================================================================
#  FLOOR_* — one per `chk` line in importer/scripts/import-world.sh
# =====================================================================================================

FLOOR_CLASS_TRAINERS=6                    # 6 class trainers spawned {328,377,906,913,917,927}
FLOOR_TRAINER_OFFERINGS_CLASS=2000        # class trainer offerings (learn_skill_line=0) — the dump's own class trees are the primary source; a re-wipe regression fails this
FLOOR_TRAINER_OFFERINGS_PROFESSION=80     # profession learn offerings (learn_skill_line>0)
FLOOR_GATHER_NODE_GO=100                  # gather-node GO spawns
FLOOR_TERRAIN_CHUNKS=2500                 # [V] terrain chunks (ground-z heightmap) — tune after your first real Westfall import
FLOOR_NAV_CHUNKS=3000                     # nav chunks (walkability/LoS grid) — the full canonical box rasterized 4482 on a real import
FLOOR_INNKEEPER_FARLEY=1                  # innkeeper Farley(295) spawned
FLOOR_QUEST_GIVER_RELATIONS=450           # [V] quest-giver relations — an estimate; tighten against your own dump's count
FLOOR_QUESTS_L6_10=20                     # L6-10 quests (quest_level band)
FLOOR_QUESTS_L10_20=1                     # L10-20 quests (Westfall quest_level band) — presence only
FLOOR_QUESTS_CHAINED=1                    # [V] chained quests (next_quest_id>0) — presence only;
                                           # the McBride 783->7->15->21->54 chain (already verified via the
                                           # working prev_quest_id import) should populate this symmetrically.
                                           # NOTE: no FLOOR_QUESTS_TIMED — timed (limit_time>0) quests are only
                                           # coverage-printed, not floor-gated: plausible this Elwynn/Westfall
                                           # slice has zero (escort quests, the usual timed pairing, are
                                           # deliberately out of scope), so a floor here would risk a false
                                           # failure on a legitimate import.
FLOOR_LIVE_CREATURES=2500                 # [V] live creature entities — an estimate; tighten against your own dump's count
FLOOR_CONTINENT_SPAWNS=1500               # [V] game_creature_spawn rows carrying THIS run's primary map id:
                                           # proves the continent's spatial rows landed under the RIGHT map — a map-1
                                           # shard whose spawns all read map_id 0 would pass every other floor. 1500 is
                                           # deliberately well under the ~2200-3500 the widened map-0 box yields (a floor
                                           # that fails on a legitimate import is the worse failure) — tighten to your dump.
FLOOR_VENDORS=280                         # [V] vendors (npc_vendor) — an estimate; tighten against your own dump's count
FLOOR_TRAINER_COVERAGE=70                 # spawned trainer-flagged NPCs that actually teach (measured 72 of 75; 3 are annotated known-empty)
FLOOR_VENDOR_COVERAGE=125                 # spawned vendor-flagged NPCs with vendor rows (measured 127 of 128 via npc_vendor_template flattening; the Brigadier General is annotated)
FLOOR_QUESTGIVER_COVERAGE=190             # spawned questgiver-flagged NPCs with quest relations (measured 194 of 228; the quiet ones may be off-level content)
FLOOR_SENTINEL_HILL_VENDOR=1              # Sentinel Hill trainer/vendor spawned [V SENTINEL_HILL_VENDOR_ENTRY, set separately]
FLOOR_START_ITEMS_CLASSES=6               # Human start-items (all 6 classes)
FLOOR_HUMAN_START_POSITIONS=6             # playercreateinfo: Human Warrior/Paladin/Rogue/Priest/Mage/Warlock
FLOOR_DWARF_START_POSITIONS=5             # playercreateinfo: Dwarf Warrior/Paladin/Hunter/Rogue/Priest
FLOOR_GNOME_START_POSITIONS=4             # playercreateinfo: Gnome Warrior/Rogue/Mage/Warlock
FLOOR_NIGHT_ELF_START_POSITIONS=5         # playercreateinfo: Night Elf Warrior/Hunter/Rogue/Priest/Druid
FLOOR_DWARF_START_ITEMS_CLASSES=5         # Dwarf creation loadout families; no Human fallback
FLOOR_GNOME_START_ITEMS_CLASSES=4         # Gnome creation loadout families; no Human fallback
FLOOR_NIGHT_ELF_START_ITEMS_CLASSES=5     # Night Elf creation loadout families; no Human fallback
FLOOR_DUN_MOROGH_START=1                  # Sten Stoutarm (658), map 0
FLOOR_LOCH_MODAN_CONTENT=1                # Mountaineer Kadrell (1340), map 0
FLOOR_TELDRASSIL_START=1                  # Conservator Ilthalaine (2079), map 1
FLOOR_DARKSHORE_CONTENT=1                 # Gwennyth Bly'Leggonde (10219), map 1
FLOOR_CORRIDOR_QUEST_LEVEL_BAND=1         # each named corridor giver has a level 1-20 quest
FLOOR_CORRIDOR_GAMEOBJECTS=1              # each planned open-world map retains GameObjects
FLOOR_CASTER_CAST_ROWS=60                 # [V] caster-mob cast rows — an estimate; tighten against your own dump's count
FLOOR_GEOMANCER_CAST_SPELL=1              # 476 Geomancer cast spell present
FLOOR_ROGUE_WIZARD_CAST_SPELL=1           # 474 Rogue Wizard cast spell present
FLOOR_IMP_FIREBOLT_CAST_ROW=1             # Imp(416) Firebolt cast row
FLOOR_IMP_FIREBOLT_SPELL=1                # Imp Firebolt(3110) in game_spell
FLOOR_DEFIAS_CASTER_CAST_ROW=1            # [V] Defias caster cast row (Pillager 589?/Conjurer 449?)
FLOOR_ROTATION_ROWS=20                    # rotation rows (game_creature_spell)
FLOOR_GEOMANCER_ROTATION_NUKE=1           # 476 Geomancer rotation nuke row
FLOOR_FROST_ARMOR_ROTATION=1              # 474/476 Frost Armor rotation row
FLOOR_AREAS=1000                          # areas imported (game_area) — tuned against a real 1.12.1 client: AreaTable imports 1081 rows
FLOOR_AREA_TRIGGERS=400                   # area triggers imported (game_area_trigger) — tuned against a real 1.12.1 client: 432 rows
FLOOR_GRAVEYARDS=50                       # [V] graveyards imported (game_graveyard) — same caveat as FLOOR_AREAS
FLOOR_TAXI_NODES=1                        # client-derived rows below the reserved 5090000 fixture floor
FLOOR_TAXI_PATHS=1                        # directed client-derived routes below the fixture floor
FLOOR_TAXI_PATH_NODES=1                   # ordered client-derived path points below the fixture floor
FLOOR_GRAVEYARD_ZONE_RESOLVE=1            # zone-12 graveyard_zone links resolve to a real game_graveyard row
FLOOR_AREATRIGGER_TELEPORTS=50            # [V] areatrigger_teleport portals imported (game_areatrigger_teleport)
                                           # — an unverified estimate;
                                           # this is a WORLD-WIDE table (every dungeon/instance/battleground portal in
                                           # the dump, not just Deadmines), so 50 is a conservative floor well under a
                                           # real vanilla dump's count — tighten/widen against your own dump.
FLOOR_DEADMINES_PORTAL_ROUNDTRIP=2        # Deadmines' entrance(78)+exit(119) — read off a real dump, areatrigger_teleport rows BOTH
                                           # present — proves the round-trip (walk in AND walk back out) imports, not
                                           # just one direction. Confirm both trigger ids against YOUR dump before
                                           # trusting them; correct a wrong id rather than lowering this floor.
FLOOR_DEADMINES_CREATURE_SPAWNS=100       # [V] map-36 creature spawns (game_creature_spawn.map_id=36, from the
                                           # --include-map 36 slice) — a real classic-db Deadmines carries
                                           # several hundred placed spawns; 100 is a conservative floor so a
                                           # scope/parse regression (or forgetting --include-map 36) fails loud
                                           # without flaking on a legitimately leaner dump. Tighten to your dump.
FLOOR_DEADMINES_GAMEOBJECTS=3             # [V] map-36 gameobject spawns (game_gameobject.map_id=36) — the
                                           # instance doors (Rhahk'Zor's door, the iron clad door), the Defias
                                           # Cannon, ship objects, chests; 3 is a bare-presence floor (doors +
                                           # cannon alone clear it). Tighten to your dump.
FLOOR_DEADMINES_BOSSES=6                  # [V] DISTINCT boss template entries among the map-36 PLACED spawns —
                                           # Rhahk'Zor 644, Sneed's Shredder 642, Gilnid 1763, Mr. Smite 646,
                                           # Captain Greenskin 647, Edwin VanCleef 639. Sneed 643 is deliberately
                                           # NOT counted: cmangos-era dumps script-summon him when the Shredder
                                           # (642) dies, so a CORRECT dump typically has no placed 643 row — whether
                                           # he actually shows up is an encounter-scripting concern, not an import
                                           # floor. If your dump DOES place 643 the count reads 7 and still
                                           # passes. A wrong [V] id here FAILS LOUD — deliberate, the
                                           # SENTINEL_HILL_VENDOR precedent: correct the id, don't loosen the floor.
FLOOR_DEADMINES_NAMED_LOOT=2              # [V] BOTH named Deadmines drops present in game_creature_loot —
                                           # Cruel Barb ~5191 (VanCleef) AND Smite's Mighty Hammer ~5196 (Smite);
                                           # both item ids are 1.12-from-knowledge and UNVERIFIED — confirm them
                                           # against your dump's creature_loot_template before trusting them, then
                                           # tighten. Floor 2 = both
                                           # (the DEADMINES_PORTAL_ROUNDTRIP precedent — 1 would pass vacuously).
FLOOR_PICKPOCKET_LOOT=1                   # [V] pickpocket loot rows (game_pickpocket_loot)
FLOOR_SKINNING_LOOT=1                     # [V] skinning loot rows (game_skinning_loot)
FLOOR_GAMEOBJECT_CHEST_LOOT=1             # [V] gameobject (chest) loot rows (game_gameobject_loot)
FLOOR_FISHING_LOOT=1                      # [V] fishing loot rows (game_fishing_loot)
FLOOR_CREATURES_WITH_PICKPOCKET=1         # [V] creatures with a pickpocket table (creature_template.pickpocket_loot_id>0)
FLOOR_CREATURES_WITH_SKIN=1               # [V] creatures with a skin table (creature_template.skin_loot_id>0)
FLOOR_SPELL_TOTAL=8000                    # [V] total game_spell rows after the full Spell.dbc import — the true
                                           # count is client-build-dependent (~10-12k non-zero rows); 8000
                                           # is a conservative minimum so a partial-parse regression fails loud without being
                                           # so tight it flakes on a legitimately smaller client build.
FLOOR_SPELL_CHAIN=300                     # [V] game_spell_chain rows — an unverified estimate;
                                           # a real classic-db dump's spell_chain table
                                           # covers every multi-rank spell family (weapon skills, class ranked spells,
                                           # ranked talents-as-spells) — a conservative floor well under the real count so
                                           # a parse/column regression fails loud without flaking on a smaller client build.
FLOOR_SPELL_LEARN=5                       # game_spell_learn rows — measured on a real import: the
                                           # reduced-scope generator emits 9 today, and widening that
                                           # generator is separate work, not an import regression.
                                           # spell_learn_spell (auto-taught dependents) is a much
                                           # smaller table than spell_chain, so 5 is a presence floor
                                           # — tighten it against your own dump's real count.
FLOOR_SPELL_PROC_EVENT=100                # [V] game_spell_proc_event rows — the classic-db proc overlay
                                           # (PPM, internal cooldowns, procEx, school/family filters).
                                           # An unverified estimate: cmangos hand-authors one row per
                                           # scripted proc, which in 1.12 is the talent procs, the class
                                           # shields and the weapon procs — a few hundred. 100 is a
                                           # conservative floor that still fails loud if the table is
                                           # absent from the dump or its columns drifted. Tighten it
                                           # against your own dump's real count.

# -----------------------------------------------------------------------------------------------------
#  MAP != 0 — the continent-shard overrides
# -----------------------------------------------------------------------------------------------------
# Every floor re-pointed here is BOX-DEPENDENT: its number was measured against (or estimated for) the
# Elwynn/Westfall rectangle, so on a `MAP=1` Kalimdor shard it measures content that was never in the
# slice. Left alone, a legitimate map-1 import reports ~40 FAILs for absent Elwynn content and the real
# signal drowns — so they drop to PRESENCE floors: enough to catch "a whole family silently imported
# nothing" (the thing floors exist for), not enough to pretend we know a Durotar spawn count.
#
# `[V]` all of it, and deliberately loose: the FIRST real map-1 import is the measurement. Record its
# counts and tighten these — the map-0 floors got their numbers exactly that way. Floors NOT overridden
# (on purpose, they must match on every shard): FLOOR_START_ITEMS_CLASSES, FLOOR_SPELL_*, FLOOR_AREAS,
# FLOOR_AREA_TRIGGERS, FLOOR_GRAVEYARDS, FLOOR_AREATRIGGER_TELEPORTS, FLOOR_DEADMINES_PORTAL_ROUNDTRIP
# and the four loot-family presence floors — all DBC-sourced or world-wide dump globals, imported
# regardless of `--map`/`--box`. The map-0-ONLY named-fixture floors (Goldshire trainers, Farley,
# Sentinel Hill, the Elwynn casters, the map-36 slice) aren't overridden either: `import-world.sh`
# doesn't RUN those assertions off map 0.
# SLICE covers both cases: another continent, and a caller-chosen map-0 box (which is what a region
# shard is). Either way the corridor-tuned counts below are wrong. MAP!=0 is kept as a fallback so
# sourcing this file without import-world.sh still behaves sanely.
if [ "${SLICE:-0}" = 1 ] || [ "${MAP:-0}" != 0 ]; then
  FLOOR_TRAINER_OFFERINGS_CLASS=1         # [V] the curated non-Human bindings alone land ~30; a starting-zone box adds its own trainers
  FLOOR_TRAINER_OFFERINGS_PROFESSION=1    # [V]
  FLOOR_GATHER_NODE_GO=1                  # [V]
  FLOOR_TERRAIN_CHUNKS=100                # [V] a starting-zone box rasterizes hundreds of cells; 100 catches a dead terrain pass
  FLOOR_NAV_CHUNKS=1                      # [V] only cells WITH obstruction emit a row, so this is presence-only
  FLOOR_QUEST_GIVER_RELATIONS=1           # [V]
  FLOOR_QUESTS_L6_10=1                    # [V] every starting zone has 6-10 content; count is box-width dependent
  FLOOR_LIVE_CREATURES=100                # [V]
  FLOOR_CONTINENT_SPAWNS=100              # [V] map-scoped spawn rows — the "landed under the right map id" check
  FLOOR_VENDORS=1                         # [V]
  FLOOR_TRAINER_COVERAGE=1                # [V]
  FLOOR_VENDOR_COVERAGE=1                 # [V]
  FLOOR_QUESTGIVER_COVERAGE=1             # [V]
  FLOOR_CASTER_CAST_ROWS=1                # [V]
fi

# =====================================================================================================
#  GATE_* — presence-probe ids the wire suite's data-gate section keys on (existence, not a count
#  floor). Verified against the wire suite's CURRENT data-gate block — that suite lives in its own
#  repository, which this checkout pins by release, so nothing here can edit it. If you change an id
#  below, the harness's matching probe has to move with it.
# =====================================================================================================

GATE_SPELL_PALADIN=635      # curated paladin kit imported? (the suite's HAS_SPELL_635 probe)
GATE_SPELL_SHADOWBOLT=686   # Shadow Bolt — the general "world import ran" probe (HAS_SPELL_686)
GATE_CREATURE_TEST=103      # creature_template entry 103 (HAS_CREATURE_103)
GATE_HEALER_ENTRY=6491      # spirit healer creature entry (HAS_HEALER)
