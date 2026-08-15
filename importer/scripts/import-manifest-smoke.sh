#!/usr/bin/env bash
# import-manifest-smoke.sh — source-smoke test for importer/scripts/import-manifest.sh.
#
# Run it from the checkout root, any time, with nothing set up:
#   bash importer/scripts/import-manifest-smoke.sh     # prints "[smoke] OK" when it passes
#
# Offline: no live node, no dump, no built importer. Run it after editing the manifest.
#
# WHAT IT ASSERTS: every KEY the two real consumers read is actually DEFINED after sourcing the
# manifest. Those consumers are importer/scripts/import-world.sh's `chk` floors, and the data-gate
# probes of the wire test harness, which lives in its own repository and is pinned here by release.
# The failure it exists to catch is quiet: a renamed or deleted manifest key reads as an EMPTY STRING
# in a `chk`/gate comparison rather than as an error, so the assertion it backs stops enforcing
# anything while the import still prints OK.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1

# shellcheck source=importer/scripts/import-manifest.sh
source importer/scripts/import-manifest.sh

fail=0
need() { # key-name
  if [ -z "${!1:-}" ]; then echo "  FAIL  $1 is unset/empty"; fail=1; else echo "  ok    $1=${!1}"; fi
}

echo "[smoke] FLOOR_* keys consumed by importer/scripts/import-world.sh:"
for k in FLOOR_CLASS_TRAINERS FLOOR_TRAINER_OFFERINGS_CLASS FLOOR_TRAINER_OFFERINGS_PROFESSION \
         FLOOR_GATHER_NODE_GO FLOOR_TERRAIN_CHUNKS FLOOR_NAV_CHUNKS FLOOR_TRAINER_COVERAGE FLOOR_VENDOR_COVERAGE FLOOR_QUESTGIVER_COVERAGE FLOOR_INNKEEPER_FARLEY FLOOR_QUEST_GIVER_RELATIONS \
         FLOOR_QUESTS_L6_10 FLOOR_QUESTS_L10_20 FLOOR_QUESTS_CHAINED FLOOR_LIVE_CREATURES FLOOR_CONTINENT_SPAWNS FLOOR_VENDORS \
         FLOOR_SENTINEL_HILL_VENDOR FLOOR_START_ITEMS_CLASSES FLOOR_CASTER_CAST_ROWS \
         FLOOR_GEOMANCER_CAST_SPELL FLOOR_ROGUE_WIZARD_CAST_SPELL FLOOR_IMP_FIREBOLT_CAST_ROW \
         FLOOR_IMP_FIREBOLT_SPELL FLOOR_DEFIAS_CASTER_CAST_ROW FLOOR_ROTATION_ROWS \
         FLOOR_GEOMANCER_ROTATION_NUKE FLOOR_FROST_ARMOR_ROTATION FLOOR_AREAS FLOOR_AREA_TRIGGERS \
         FLOOR_GRAVEYARDS FLOOR_TAXI_NODES FLOOR_TAXI_PATHS FLOOR_TAXI_PATH_NODES \
         FLOOR_GRAVEYARD_ZONE_RESOLVE FLOOR_AREATRIGGER_TELEPORTS \
         FLOOR_DEADMINES_PORTAL_ROUNDTRIP FLOOR_DEADMINES_CREATURE_SPAWNS FLOOR_DEADMINES_GAMEOBJECTS \
         FLOOR_DEADMINES_BOSSES FLOOR_DEADMINES_NAMED_LOOT FLOOR_PICKPOCKET_LOOT FLOOR_SKINNING_LOOT \
         FLOOR_GAMEOBJECT_CHEST_LOOT FLOOR_FISHING_LOOT FLOOR_CREATURES_WITH_PICKPOCKET \
         FLOOR_CREATURES_WITH_SKIN FLOOR_SPELL_TOTAL FLOOR_SPELL_CHAIN FLOOR_SPELL_LEARN; do
  need "$k"
done

echo "[smoke] GATE_* keys consumed by the wire suite's data-gate probes:"
for k in GATE_SPELL_PALADIN GATE_SPELL_SHADOWBOLT GATE_CREATURE_TEST GATE_HEALER_ENTRY; do
  need "$k"
done

# Cross-check: every FLOOR_/GATE_ key the manifest itself DEFINES is covered by the lists above (a new
# key added to the manifest without updating this smoke test would otherwise go unchecked silently).
defined_keys="$(grep -oE '^(FLOOR|GATE)_[A-Z0-9_]+=' importer/scripts/import-manifest.sh | tr -d '=')"
checked_keys="FLOOR_CLASS_TRAINERS FLOOR_TRAINER_OFFERINGS_CLASS FLOOR_TRAINER_OFFERINGS_PROFESSION \
FLOOR_GATHER_NODE_GO FLOOR_TERRAIN_CHUNKS FLOOR_NAV_CHUNKS FLOOR_TRAINER_COVERAGE FLOOR_VENDOR_COVERAGE FLOOR_QUESTGIVER_COVERAGE FLOOR_INNKEEPER_FARLEY FLOOR_QUEST_GIVER_RELATIONS \
FLOOR_QUESTS_L6_10 FLOOR_QUESTS_L10_20 FLOOR_QUESTS_CHAINED FLOOR_LIVE_CREATURES FLOOR_CONTINENT_SPAWNS FLOOR_VENDORS \
FLOOR_SENTINEL_HILL_VENDOR FLOOR_START_ITEMS_CLASSES FLOOR_CASTER_CAST_ROWS \
FLOOR_GEOMANCER_CAST_SPELL FLOOR_ROGUE_WIZARD_CAST_SPELL FLOOR_IMP_FIREBOLT_CAST_ROW \
FLOOR_IMP_FIREBOLT_SPELL FLOOR_DEFIAS_CASTER_CAST_ROW FLOOR_ROTATION_ROWS \
FLOOR_GEOMANCER_ROTATION_NUKE FLOOR_FROST_ARMOR_ROTATION FLOOR_AREAS FLOOR_AREA_TRIGGERS \
FLOOR_GRAVEYARDS FLOOR_TAXI_NODES FLOOR_TAXI_PATHS FLOOR_TAXI_PATH_NODES \
FLOOR_GRAVEYARD_ZONE_RESOLVE FLOOR_AREATRIGGER_TELEPORTS FLOOR_DEADMINES_PORTAL_ROUNDTRIP \
FLOOR_DEADMINES_CREATURE_SPAWNS FLOOR_DEADMINES_GAMEOBJECTS FLOOR_DEADMINES_BOSSES FLOOR_DEADMINES_NAMED_LOOT \
FLOOR_PICKPOCKET_LOOT FLOOR_SKINNING_LOOT \
FLOOR_GAMEOBJECT_CHEST_LOOT FLOOR_FISHING_LOOT FLOOR_CREATURES_WITH_PICKPOCKET \
FLOOR_CREATURES_WITH_SKIN FLOOR_SPELL_TOTAL FLOOR_SPELL_CHAIN FLOOR_SPELL_LEARN \
GATE_SPELL_PALADIN GATE_SPELL_SHADOWBOLT GATE_CREATURE_TEST GATE_HEALER_ENTRY"
for k in $defined_keys; do
  case " $checked_keys " in
    *" $k "*) ;;
    *) echo "  FAIL  manifest defines $k but this smoke test doesn't check it — update the lists above"; fail=1 ;;
  esac
done

# The manifest's MAP!=0 tail re-points the BOX-DEPENDENT floors for a continent shard other
# than map 0. Re-source it with MAP=1 in a SUBSHELL (so the map-0 values above stay intact for anything
# sourcing this script) and assert (a) every key is still defined — an override block that typo'd a key
# name would otherwise leave a `chk` comparing against an empty string — (b) a box-dependent floor
# actually dropped, and (c) a map-INDEPENDENT floor did NOT move (those must match on every shard).
echo "[smoke] MAP=1 continent-shard floor overrides:"
map1_out="$(
  MAP=1 bash -c '
    set -uo pipefail
    source importer/scripts/import-manifest.sh
    for k in FLOOR_TERRAIN_CHUNKS FLOOR_LIVE_CREATURES FLOOR_CONTINENT_SPAWNS FLOOR_VENDORS \
             FLOOR_TRAINER_COVERAGE FLOOR_QUEST_GIVER_RELATIONS FLOOR_SPELL_TOTAL FLOOR_AREAS \
             FLOOR_TAXI_NODES FLOOR_START_ITEMS_CLASSES FLOOR_CLASS_TRAINERS; do
      [ -z "${!k:-}" ] && { echo "UNSET $k"; continue; }
      echo "$k=${!k}"
    done'
)"
echo "$map1_out" | grep -q '^UNSET' && { echo "$map1_out" | grep '^UNSET' | sed 's/^/  FAIL  /'; fail=1; }
m1() { echo "$map1_out" | grep "^$1=" | cut -d= -f2; }
chk_eq() { # label expected actual
  if [ "$2" != "$3" ]; then echo "  FAIL  $1: got $3, want $2"; fail=1; else echo "  ok    $1: $3"; fi
}
chk_eq "FLOOR_TERRAIN_CHUNKS drops for a non-map-0 shard" 100 "$(m1 FLOOR_TERRAIN_CHUNKS)"
chk_eq "FLOOR_CONTINENT_SPAWNS drops to a presence floor"  100 "$(m1 FLOOR_CONTINENT_SPAWNS)"
chk_eq "FLOOR_SPELL_TOTAL is map-INDEPENDENT (unchanged)" "$FLOOR_SPELL_TOTAL" "$(m1 FLOOR_SPELL_TOTAL)"
chk_eq "FLOOR_AREAS is map-INDEPENDENT (unchanged)"        "$FLOOR_AREAS" "$(m1 FLOOR_AREAS)"
chk_eq "FLOOR_TAXI_NODES is map-INDEPENDENT"                "$FLOOR_TAXI_NODES" "$(m1 FLOOR_TAXI_NODES)"
chk_eq "FLOOR_START_ITEMS_CLASSES is map-INDEPENDENT"      "$FLOOR_START_ITEMS_CLASSES" "$(m1 FLOOR_START_ITEMS_CLASSES)"
# And the map-0 defaults are untouched when MAP is unset/0 (the byte-parity guarantee for the canonical run).
chk_eq "MAP unset keeps the map-0 terrain floor" 2500 "$FLOOR_TERRAIN_CHUNKS"
chk_eq "MAP unset keeps the map-0 live-creature floor" 2500 "$FLOOR_LIVE_CREATURES"

# import-world.sh's `chk0`/`chk36` FENCE the assertions that name a specific map-0 or map-36 fixture.
# Their failure mode is the dangerous direction — a wrapper that stops delegating to `chk` does not go
# red, it silently stops enforcing Elwynn/Deadmines and the canonical run still prints "[world] OK".
# Nothing else runs those two lines offline, so pin their semantics here by extracting the definitions
# verbatim out of import-world.sh and exercising both arms.
echo "[smoke] import-world.sh chk0/chk36 map fences:"
fence_out="$(
  bash -c '
    set -uo pipefail
    eval "$(grep -E "^chk(0|36)\(\) \{" importer/scripts/import-world.sh)"
    chk() { echo "RAN:$1"; }
    MAP=0; INCLUDE_MAPS=36; chk0 map0-run; chk36 map36-in-run
    MAP=1; INCLUDE_MAPS=;   chk0 map1-run; chk36 map36-absent
    # SLICE: a caller-chosen map-0 box is a region shard, not the canonical corridor, so the
    # named Elwynn/Westfall fixture checks must NOT run. Unset must still mean canonical — this line
    # runs under `set -u`, which is why chk0 reads ${SLICE:-0} rather than $SLICE.
    MAP=0; INCLUDE_MAPS=36; SLICE=1; chk0 map0-slice
    MAP=0; INCLUDE_MAPS=36; SLICE=0; chk0 map0-corridor
  '
)"
ran() { case "$fence_out" in *"RAN:$1"*) echo yes ;; *) echo no ;; esac; }
chk_eq "chk0 RUNS its check on a map-0 run"            yes "$(ran map0-run)"
chk_eq "chk36 RUNS its check when 36 is in INCLUDE_MAPS" yes "$(ran map36-in-run)"
chk_eq "chk0 SKIPS its check off map 0"                no  "$(ran map1-run)"
chk_eq "chk36 SKIPS its check when 36 is absent"        no  "$(ran map36-absent)"
chk_eq "chk0 SKIPS the corridor fixtures on a map-0 SLICE"  no  "$(ran map0-slice)"
chk_eq "chk0 RUNS them on the canonical map-0 corridor"     yes "$(ran map0-corridor)"

# The fresh-shard one-continent guard helpers (db_spatial_probe/db_imported_probe/db_never_imported/
# foreign_spatial_maps, marker-delimited in import-world.sh so they can be extracted verbatim here,
# same trick as the chk0/chk36 pin above). Exercising them for real needs a live node and two
# differently-populated databases, so this pins BOTH directions offline instead, with a stub
# `spacetime`: a never-imported shard whose only spatial rows are `init`'s map-0 demo fixtures must NOT
# read as foreign content, while a database that genuinely holds real map-0 terrain/nav content must
# still be flagged for a map-1 run.
echo "[smoke] import-world.sh one-continent guard helpers:"
guard_out="$(
  bash -c '
    set -uo pipefail
    eval "$(sed -n "/# GUARD HELPERS BEGIN/,/# GUARD HELPERS END/p" importer/scripts/import-world.sh)"
    DB=probe-db
    EXPECTED_MAPS=1

    # Fresh shard: creature_spawn/gameobject carry initʼs map-0 fixtures (chicken/wolf/trainer/flight-master +
    # chest/goober/gather-node/pool-armed rows); terrain/nav are EMPTY — exactly a brand-new shard
    # about to import map 1 for the first time, before this issue was fixed.
    spacetime() { # sql <db> "SELECT map_id FROM <table>"
      case "$3" in
        *game_creature_spawn*|*game_gameobject*) echo " 0" ;;
        *game_terrain_chunk*|*game_nav_chunk*) : ;; # never imported — no rows at all
      esac
    }
    echo "fresh_never_imported=$(db_never_imported && echo yes || echo no)"
    echo "fresh_pre_foreign=$(foreign_spatial_maps "$(db_spatial_probe)")"

    # Populated foreign-continent shard: a database that already went through a REAL map-0 import, so
    # terrain/nav genuinely carry map-0 rows too — the case the guard must keep refusing.
    spacetime() {
      case "$3" in
        *game_creature_spawn*|*game_gameobject*|*game_terrain_chunk*|*game_nav_chunk*) echo " 0" ;;
      esac
    }
    echo "contaminated_never_imported=$(db_never_imported && echo yes || echo no)"
    echo "contaminated_pre_foreign=$(foreign_spatial_maps "$(db_spatial_probe)")"

    # Family-reload blind spot, live-reproduced against a throwaway node: a
    # `./target/debug/lyracore-importer --family creatures --apply` run landed 113 real map-0 spawns DIRECTLY —
    # bypassing this script and its terrain/nav ETL entirely, which is a supported way to reload a
    # single family into a dev node — so terrain/nav STILL read empty (the never-imported
    # signal alone says "fresh") even though this database plainly holds real, deliberately-imported
    # content. The row-count fallback in db_never_imported must catch this.
    spacetime() { # sql <db> "<query>"
      case "$3" in
        *"COUNT(*)"*game_creature_spawn*) echo " 113" ;; # far past initʼs 4-row fixture ceiling
        *"COUNT(*)"*game_gameobject*) echo " 0" ;;
        *game_creature_spawn*|*game_gameobject*) echo " 0" ;; # map_id probe: still reads as map 0 only
        *game_terrain_chunk*|*game_nav_chunk*) : ;;            # no --terrain/--nav ETL ever ran
      esac
    }
    echo "family_reload_never_imported=$(db_never_imported && echo yes || echo no)"
  '
)"
g() { echo "$guard_out" | grep "^$1=" | cut -d= -f2 | sed 's/ *$//'; } # foreign_spatial_maps trails a space
chk_eq "fresh shard: db_never_imported is TRUE (terrain/nav empty)"        yes "$(g fresh_never_imported)"
chk_eq "fresh shard: map-0 fixture rows still show up in the raw probe"   0   "$(g fresh_pre_foreign)"
chk_eq "contaminated shard: db_never_imported is FALSE (real terrain/nav)" no  "$(g contaminated_never_imported)"
chk_eq "contaminated shard: foreign map 0 still reported"                 0   "$(g contaminated_pre_foreign)"
chk_eq "family-reload blind spot: db_never_imported is FALSE (row-count fallback catches it)" no "$(g family_reload_never_imported)"

# import-world.sh does not enable `set -e`; a raw importer|grep|tail pipeline therefore used to keep
# running after the importer failed. Exercise the marker-delimited status-capturing helper with both
# outcomes, then pin the standalone DBC call site's explicit exit.
echo "[smoke] standalone DBC import failure propagation:"
checked_step_out="$(
  bash -c '
    set -uo pipefail
    eval "$(sed -n "/# CHECKED IMPORT HELPERS BEGIN/,/# CHECKED IMPORT HELPERS END/p" importer/scripts/import-world.sh)"
    good_step() { echo "dbc: loaded fixture"; return 0; }
    bad_step() { echo "partial taxi batch applied before failure"; return 23; }
    run_checked_step "smoke good" "dbc: loaded" 1 good_step >/dev/null
    echo "good_status=$?"
    bad_log="$(run_checked_step "smoke bad" "dbc: loaded" 1 bad_step 2>&1)"
    echo "bad_status=$?"
    case "$bad_log" in *"ABORT — smoke bad failed"*) echo "bad_abort=yes" ;; *) echo "bad_abort=no" ;; esac
  '
)"
cs() { echo "$checked_step_out" | grep "^$1=" | cut -d= -f2; }
chk_eq "successful checked step stays successful" 0 "$(cs good_status)"
chk_eq "failed checked step preserves importer exit status" 23 "$(cs bad_status)"
chk_eq "failed checked step emits an abort" yes "$(cs bad_abort)"
dbc_callsite="$(sed -n '/run_checked_step "standalone DBC catalogue import"/,/--apply || exit 1/p' importer/scripts/import-world.sh)"
case "$dbc_callsite" in *"--apply || exit 1"*) dbc_callsite_fatal=yes ;; *) dbc_callsite_fatal=no ;; esac
chk_eq "standalone DBC call exits import-world.sh on failure" yes "$dbc_callsite_fatal"

# The map-0 restore is equally load-bearing: a denied reducer call must stop the import before the
# verification stage. Its postcondition is independently checked across the catalogue and all three
# spatial anchors (template, spawn, live entity).
echo "[smoke] taxi fixture restore failure propagation and anchor checks:"
taxi_restore_out="$(
  bash -c '
    set -uo pipefail
    eval "$(sed -n "/# TAXI RESTORE HELPERS BEGIN/,/# TAXI RESTORE HELPERS END/p" importer/scripts/import-world.sh)"
    call_q() { return 0; }
    restore_taxi_fixture_or_fail >/dev/null
    echo "good_status=$?"
    call_q() { echo "operator denied"; return 17; }
    bad_log="$(restore_taxi_fixture_or_fail 2>&1)"
    echo "bad_status=$?"
    case "$bad_log" in *"ABORT — restore_taxi_fixture failed"*) echo "bad_abort=yes" ;; *) echo "bad_abort=no" ;; esac
  '
)"
trs() { echo "$taxi_restore_out" | grep "^$1=" | cut -d= -f2; }
chk_eq "successful taxi restore stays successful" 0 "$(trs good_status)"
chk_eq "failed taxi restore preserves reducer exit status" 17 "$(trs bad_status)"
chk_eq "failed taxi restore emits an abort" yes "$(trs bad_abort)"
restore_callsite="$(sed -n '/echo "\[world\] restore taxi fixture"/,/restore_taxi_fixture_or_fail || exit 1/p' importer/scripts/import-world.sh)"
case "$restore_callsite" in *"restore_taxi_fixture_or_fail || exit 1"*) restore_callsite_fatal=yes ;; *) restore_callsite_fatal=no ;; esac
chk_eq "taxi restore call exits import-world.sh on failure" yes "$restore_callsite_fatal"

taxi_anchor_out="$(
  bash -c '
    set -uo pipefail
    eval "$(sed -n "/# TAXI FIXTURE VERIFY BEGIN/,/# TAXI FIXTURE VERIFY END/p" importer/scripts/import-world.sh)"
    n() { case "$1" in *game_taxi_path_node*) echo 3 ;; *) echo 1 ;; esac; }
    checked=0; fail=0
    chk() { checked=$((checked + 1)); [ "$3" -ge "$2" ] || fail=1; }
    verify_taxi_fixture_anchors
    echo "success_checked=$checked"
    echo "success_fail=$fail"
    n() { case "$1" in *game_taxi_path_node*) echo 3 ;; *game_world_entity*) echo 0 ;; *) echo 1 ;; esac; }
    checked=0; fail=0
    verify_taxi_fixture_anchors
    echo "missing_live_checked=$checked"
    echo "missing_live_fail=$fail"
  '
)"
ta() { echo "$taxi_anchor_out" | grep "^$1=" | cut -d= -f2; }
chk_eq "taxi verification checks catalogue + template/spawn/live anchors" 7 "$(ta success_checked)"
chk_eq "complete taxi fixture satisfies every anchor" 0 "$(ta success_fail)"
chk_eq "taxi verification still runs every anchor after one miss" 7 "$(ta missing_live_checked)"
chk_eq "missing live flight master fails verification" 1 "$(ta missing_live_fail)"

if [ "$fail" = 0 ]; then echo "[smoke] OK — every consumed manifest key is defined"; else echo "[smoke] FAIL"; exit 1; fi
