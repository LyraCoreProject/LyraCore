#!/usr/bin/env bash
# Each import clears whole content families; combine all slices in one run.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1
WORLD_PROFILE="${WORLD_PROFILE:-}"
export MAP="${MAP:-0}"
# SLICE must be set before sourcing the manifest because it selects the count floors.
export SLICE=0
{ [ "$MAP" != 0 ] || [ -n "${BOX:-}" ]; } && export SLICE=1
# shellcheck source=importer/scripts/import-manifest.sh
source importer/scripts/import-manifest.sh
# shellcheck source=importer/scripts/vmangos-source.sh
source importer/scripts/vmangos-source.sh || exit 1

PROFILE_FLAGS=()
PROFILE_MAPS=""
if [ -n "$WORLD_PROFILE" ]; then
  case "$WORLD_PROFILE" in
    full-world) PROFILE_MAPS="$PROFILE_FULL_WORLD_MAPS"; MAP=0 ;;
    full-eastern) PROFILE_MAPS="$PROFILE_FULL_EASTERN_MAPS"; MAP=0 ;;
    full-kalimdor) PROFILE_MAPS="$PROFILE_FULL_KALIMDOR_MAPS"; MAP=1 ;;
    full-instances) PROFILE_MAPS="$PROFILE_FULL_INSTANCES_MAPS"; MAP=36 ;;
    alliance-eastern) PROFILE_MAPS="$PROFILE_ALLIANCE_EASTERN_MAPS"; MAP=0 ;;
    alliance-kalimdor) PROFILE_MAPS="$PROFILE_ALLIANCE_KALIMDOR_MAPS"; MAP=1 ;;
    alliance-single) PROFILE_MAPS="$PROFILE_ALLIANCE_SINGLE_MAPS"; MAP=0 ;;
    instances) PROFILE_MAPS="$PROFILE_INSTANCES_MAPS"; MAP=36 ;;
    *) echo "WORLD_PROFILE=$WORLD_PROFILE is unknown; valid profiles: $WORLD_PROFILES" >&2; exit 2 ;;
  esac
  export MAP SLICE=1
  source importer/scripts/import-manifest.sh
  PROFILE_FLAGS=(--world-profile "$WORLD_PROFILE")
  echo "[world] canonical profile $WORLD_PROFILE (planned maps: $PROFILE_MAPS) → ${DB:-lyracore}"
fi

DBC="${DBC:-../wowclient/Data}"
EVENTAI_PROFILE="${EVENTAI_PROFILE:-vmangos-4641790}"
[ -z "$WORLD_PROFILE" ] && [ "$MAP" = 0 ] && BOX="${BOX:--11400,-8000,-3100,2000}"
[ -z "$WORLD_PROFILE" ] && [ "$MAP" = 0 ] && INCLUDE_CREATURES="${INCLUDE_CREATURES-344,11406,266,415,1343,6966,5165,6166,6569,5149}"
INCLUDE_CREATURES="${INCLUDE_CREATURES-}"
DB="${DB:-lyracore}"
# Bare spacetime commands can inherit a different node from the CLI configuration.
SPACETIME_SERVER="${SPACETIME_SERVER:-http://127.0.0.1:3000}"
q() { spacetime sql --server "$SPACETIME_SERVER" "$DB" "$1"; }
call_q() { spacetime call --server "$SPACETIME_SERVER" "$DB" "$@"; }
# CHECKED IMPORT HELPERS BEGIN
# Capture the importer status before grep/tail can hide a failed apply.
run_checked_step() {
  local label="$1" pattern="$2" lines="$3" output status
  shift 3
  output="$("$@" 2>&1)"; status=$?
  if [ "$status" -ne 0 ]; then
    echo "[world] ABORT — $label failed (exit $status)" >&2
    [ -z "$output" ] || printf '%s\n' "$output" >&2
    return "$status"
  fi
  printf '%s\n' "$output" | grep -iE "$pattern" | tail -"$lines" || true
}
# CHECKED IMPORT HELPERS END

# TAXI RESTORE HELPERS BEGIN
restore_taxi_fixture_or_fail() {
  local output status
  output="$(call_q restore_taxi_fixture 2>&1)"; status=$?
  if [ "$status" -ne 0 ]; then
    echo "[world] ABORT — restore_taxi_fixture failed against '${DB:-lyracore}' at ${SPACETIME_SERVER:-http://127.0.0.1:3000} (exit $status)" >&2
    echo "        A failed query is not the same as zero rows; refusing to verify partial import data." >&2
    [ -z "$output" ] || printf '%s\n' "$output" >&2
    return "$status"
  fi
}
# TAXI RESTORE HELPERS END
[ -z "$WORLD_PROFILE" ] && [ "$MAP" = 0 ] && INCLUDE_MAPS="${INCLUDE_MAPS-36}"
INCLUDE_MAPS="${INCLUDE_MAPS-}"
# CENTER must be inside the scope, with Z within 2 yards of the real ground height.
CENTER="${CENTER:-}"
[ -z "$WORLD_PROFILE" ] && [ "$MAP" = 0 ] && GRAVEYARD_ZONE="${GRAVEYARD_ZONE:-12}"
GRAVEYARD_ZONE="${GRAVEYARD_ZONE:-}"

if [ -z "$WORLD_PROFILE" ] && [ "$MAP" != 0 ]; then
  DERIVE="cargo build -q --bin lyracore-importer && ./target/debug/lyracore-importer --db \"$DB\" --dump $DUMP --map $MAP --print-extents"
  : "${BOX:?MAP=$MAP needs an explicit BOX=X0,X1,Y0,Y1 — derive it, do not guess: $DERIVE (then re-run it with a candidate --box to narrow; see this script header)}"
  : "${CENTER:?MAP=$MAP needs an explicit CENTER=X,Y,Z inside BOX — the terrain/nav self-checks sample it; use the suggested --center that $DERIVE prints for your box}"
fi

if [ -z "$WORLD_PROFILE" ] && [ "$MAP" = 0 ] && [ "$SLICE" = 1 ] && [ -z "${CENTER:-}" ]; then
  echo "A caller-supplied map-0 BOX needs an explicit CENTER=X,Y,Z inside it." >&2
  echo "The default --center is Northshire (-8949.95,-132.49,83.53), which is outside any box that" >&2
  echo "does not contain Northshire — the terrain self-check would hard-fail mid-run." >&2
  echo "Derive one (read-only):" >&2
  echo "  cargo build -q --bin lyracore-importer && ./target/debug/lyracore-importer --db \"$DB\" --dump $DUMP \\" >&2
  echo "    --map 0 --box \"$BOX\" --print-extents" >&2
  echo "…then pass the printed 'suggested --center' as CENTER." >&2
  exit 2
fi

INC_MAP_FLAGS=(); for m in ${INCLUDE_MAPS//,/ }; do INC_MAP_FLAGS+=(--include-map "$m"); done
INC_CREATURE_FLAGS=(); [ -n "$INCLUDE_CREATURES" ] && INC_CREATURE_FLAGS=(--include-creatures "$INCLUDE_CREATURES")
CENTER_FLAGS=(); [ -n "$CENTER" ] && CENTER_FLAGS=(--center "$CENTER")
if [ -n "$WORLD_PROFILE" ]; then
  SCOPE_FLAGS=("${PROFILE_FLAGS[@]}")
else
  SCOPE_FLAGS=(--map "$MAP" --box "$BOX" ${INC_MAP_FLAGS[@]+"${INC_MAP_FLAGS[@]}"} ${INC_CREATURE_FLAGS[@]+"${INC_CREATURE_FLAGS[@]}"} ${CENTER_FLAGS[@]+"${CENTER_FLAGS[@]}"})
fi
cargo build -q --bin lyracore-importer || exit 1
# The source decides which maps an all-maps scope such as full-world owns.
if ! EXPECTED_MAPS="$(./target/debug/lyracore-importer --dump "$DUMP" "${SCOPE_FLAGS[@]}" --print-scope-maps)" \
  || [ -z "$EXPECTED_MAPS" ]; then
  echo "[world] ABORT — could not read this run's map scope from $DUMP" >&2
  exit 1
fi
# comm requires both map lists in lexical order.
EXPECTED_MAPS="$(printf '%s\n' $EXPECTED_MAPS | sort -u)"

# GUARD HELPERS BEGIN
db_spatial_probe() {
  for t in game_creature_spawn game_gameobject game_terrain_chunk game_nav_chunk; do
    # Offline tests extract these functions without q() or SPACETIME_SERVER.
    out="$(spacetime sql "$DB" "SELECT map_id FROM $t" --server "${SPACETIME_SERVER:-http://127.0.0.1:3000}" 2>&1)" || { echo "!$t"; continue; }
    printf '%s\n' "$out"
  done | grep -oE '^ *[0-9]+ *$|^!.+' | tr -d ' ' | sort -u
}
probe_unreadable() { printf '%s\n' "$1" | grep '^!' | tr -d '!' | tr '\n' ' '; }
probe_maps()       { printf '%s\n' "$1" | grep -E '^[0-9]+$'; }
foreign_spatial_maps() { comm -23 <(probe_maps "$1") <(printf '%s\n' "$EXPECTED_MAPS") | tr '\n' ' '; }

# init seeds neither terrain nor navigation rows.
db_imported_probe() {
  for t in game_terrain_chunk game_nav_chunk; do
    out="$(spacetime sql "$DB" "SELECT map_id FROM $t" --server "${SPACETIME_SERVER:-http://127.0.0.1:3000}" 2>&1)" || { echo "!$t"; continue; }
    printf '%s\n' "$out"
  done | grep -oE '^ *[0-9]+ *$|^!.+' | tr -d ' ' | sort -u
}
# init seeds at most four creature spawns and five GameObjects.
INIT_MAX_CREATURE_FIXTURES=4
INIT_MAX_GAMEOBJECT_FIXTURES=5
db_row_count() {
  out="$(spacetime sql "$DB" "SELECT COUNT(*) AS n FROM $1" --server "${SPACETIME_SERVER:-http://127.0.0.1:3000}" 2>&1)" || return 1
  printf '%s\n' "$out" | grep -oE '^ *[0-9]+ *$' | tr -d ' ' | tail -1
}
# Unreadable tables or counts must fail the fresh-shard exemption.
db_never_imported() {
  imp_probe="$(db_imported_probe)"
  [ -z "$(probe_unreadable "$imp_probe")" ] || return 1
  [ -z "$(probe_maps "$imp_probe")" ] || return 1
  cs="$(db_row_count game_creature_spawn)" || return 1
  go="$(db_row_count game_gameobject)" || return 1
  [ -n "$cs" ] && [ "$cs" -le "$INIT_MAX_CREATURE_FIXTURES" ] || return 1
  [ -n "$go" ] && [ "$go" -le "$INIT_MAX_GAMEOBJECT_FIXTURES" ] || return 1
}
# GUARD HELPERS END

PRE_PROBE="$(db_spatial_probe)"
PRE_UNREADABLE="$(probe_unreadable "$PRE_PROBE")"
if [ -n "${PRE_UNREADABLE// /}" ]; then
  # The post-import check treats unreadable spatial tables as a failure.
  echo "[world] WARNING — could not read map_id from '$DB': ${PRE_UNREADABLE% }"
  echo "        The one-continent preflight is therefore INCONCLUSIVE, not clean. If this"
  echo "        database holds another continent, this run REPLACES it. Node up? database published?"
fi
PRE_FOREIGN="$(foreign_spatial_maps "$PRE_PROBE")"
if [ -n "${PRE_FOREIGN// /}" ] && db_never_imported; then
  echo "[world] fresh shard — map(s) ${PRE_FOREIGN% } are init's own demo/test fixtures"
  echo "        game_terrain_chunk/game_nav_chunk are both empty, so nothing has been really imported"
  echo "        here yet), not real content; proceeding without ALLOW_MAP_SWITCH"
  PRE_FOREIGN=""
fi
if [ -n "${PRE_FOREIGN// /}" ]; then
  echo "[world] ABORT — '$DB' already holds spatial rows for map(s): ${PRE_FOREIGN% }"
  echo "        this run imports map(s): $(echo "$EXPECTED_MAPS" | tr '\n' ' ')"
  echo "        The ETL is clear+reload per family with WHOLESALE clears, so continuing would REPLACE"
  echo "        that continent's terrain/nav/spawns with this one's — not merge them. One continent per"
  echo "        database is the one-continent-per-database invariant; publish a separate database for this map:"
  echo "          spacetime publish -s local -p module --build-options='--features=debug_reducers' <db>"
  echo "          spacetime call --server \"$SPACETIME_SERVER\" \"$DB\" claim_operator"
  echo "        Deliberately overwriting this database anyway: re-run with ALLOW_MAP_SWITCH=1."
  [ "${ALLOW_MAP_SWITCH:-0}" = 1 ] || exit 1
  echo "[world] ALLOW_MAP_SWITCH=1 — overwriting '$DB' with map(s) $(echo "$EXPECTED_MAPS" | tr '\n' ' ') anyway"
fi

if [ -n "$WORLD_PROFILE" ]; then
  echo "[world] ETL  --world-profile $WORLD_PROFILE  → $DB"
else
  echo "[world] ETL  --map $MAP  --box $BOX  include-maps [${INCLUDE_MAPS:-none}]  include-creatures [${INCLUDE_CREATURES:-none}]  → $DB"
fi
run_checked_step "world content for $WORLD_PROFILE into $DB" \
  "filter:|mapped|applied|error|abort|WARNING" 12 \
  ./target/debug/lyracore-importer --db "$DB" --server "$SPACETIME_SERVER" \
  --dump "$DUMP" --dbc "$DBC" "${SCOPE_FLAGS[@]}" \
  --eventai-profile "$EVENTAI_PROFILE" --source-sha "$SOURCE_SHA" --apply || exit 1
# Map 36 has stacked floors that a single terrain height cannot represent.
if { [ "$WORLD_PROFILE" = instances ] || [ "$WORLD_PROFILE" = full-instances ]; }; then
  echo "[world] terrain heightmap ETL SKIPPED — map 36 needs a floor-preserving representation"
else
  if [ -n "$WORLD_PROFILE" ]; then terrain_scope="--world-profile $WORLD_PROFILE"; else terrain_scope="--map $MAP --box $BOX${CENTER:+  --center $CENTER}"; fi
  echo "[world] terrain heightmap ETL $terrain_scope"
  run_checked_step "terrain for $WORLD_PROFILE into $DB" \
    "terrain:|self-check|error|abort" 8 \
    ./target/debug/lyracore-importer --db "$DB" --server "$SPACETIME_SERVER" \
    --terrain "$DBC" "${SCOPE_FLAGS[@]}" --apply || exit 1
fi
if { [ "$WORLD_PROFILE" = instances ] || [ "$WORLD_PROFILE" = full-instances ]; }; then
  echo "[world] nav grid ETL SKIPPED — instance profiles use WMO maps"
else
  if [ -n "$WORLD_PROFILE" ]; then nav_scope="--world-profile $WORLD_PROFILE"; else nav_scope="--map $MAP --box $BOX${CENTER:+  --center $CENTER}"; fi
  echo "[world] nav grid ETL $nav_scope"
  run_checked_step "navigation for $WORLD_PROFILE into $DB" \
    "nav: [0-9]|calibration|self-check|applied|error|abort" 8 \
    ./target/debug/lyracore-importer --db "$DB" --server "$SPACETIME_SERVER" \
    --nav "$DBC" "${SCOPE_FLAGS[@]}" --apply || exit 1
fi
case "$WORLD_PROFILE" in
  full-*) echo "[world] client catalogues loaded with the world content" ;;
  *)
    echo "[world] character-creation + faction DBC tables"
    run_checked_step "standalone DBC catalogue import" "dbc: loaded|error|abort" 6 \
      ./target/debug/lyracore-importer --db "$DB" --server "$SPACETIME_SERVER" --dbc "$DBC" --taxi-mounts "$DUMP" --source-sha "$SOURCE_SHA" --apply || exit 1
    ;;
esac
echo "[world] real talent trees (TalentTab.dbc + Talent.dbc → game_talent)"
run_checked_step "talents into $DB" "loaded [0-9]+ tabs|error|abort" 2 \
  ./target/debug/lyracore-importer --db "$DB" --server "$SPACETIME_SERVER" \
  --dbc "$DBC" --talents --apply || exit 1
# An unexported DB shell variable does not reach the child without env.
echo "[world] curated spell-import (functional 1-20 trainer offerings + castable ranks) → $DB"
run_checked_step "curated class spells into $DB" "imported [0-9]|error" 1 \
  env DB="$DB" SPACETIME_SERVER="$SPACETIME_SERVER" \
  importer/scripts/import-class-spells.sh "$DBC" || exit 1
echo "[world] repair (re-arm creature tick + re-seed fixtures/schedules)"
call_q debug_repair_after_publish >/dev/null 2>&1
# Restore map-0 fixtures only when this shard owns map 0.
if [ "$MAP" = 0 ]; then
  echo "[world] restore taxi fixture"
  restore_taxi_fixture_or_fail || exit 1
fi
# arm_pool fences candidates against the imported terrain and navigation maps.
echo "[world] arm gather pools"
call_q arm_all_pools >/dev/null 2>&1

fail=0
# This file carries SQL failures out of command substitutions to the parent shell.
SQL_ABORT_FLAG="$(mktemp "${TMPDIR:-/tmp}/lyracore-import-sql-abort.XXXXXX")"
rm -f "$SQL_ABORT_FLAG"
trap 'rm -f "$SQL_ABORT_FLAG"' EXIT
sql_note_failure() { printf '%s\n' "$1" >>"$SQL_ABORT_FLAG"; }
sql_check_abort() {
  [ -s "$SQL_ABORT_FLAG" ] || return 0
  echo "[world] ABORT — a verification query against '$DB' at $SPACETIME_SERVER failed:" >&2
  head -1 "$SQL_ABORT_FLAG" >&2
  echo "        A failed query is not the same as zero rows, and reporting it as one is how a single" >&2
  echo "        dead connection turns into ~50 fake \"table is empty\" FAILs (#440) — refusing instead" >&2
  echo "        of reporting any of the assertions below. Check the node is up and '$DB' is published" >&2
  echo "        there (docs/native-operations.md); set SPACETIME_SERVER=<url> if your node isn't on the" >&2
  echo "        default loopback address." >&2
  exit 1
}
chk() {
  sql_check_abort
  if [ "${3:-0}" -lt "$2" ]; then echo "  FAIL  $1: got ${3:-0}, want >= $2"; fail=1; else echo "  ok    $1: $3"; fi
}
# PROFILE ASSERTION HELPERS BEGIN
chk0() {
  case "$WORLD_PROFILE" in
    alliance-eastern|alliance-single|full-eastern|full-world) chk "$@" ;;
    "") [ "$MAP" = 0 ] && [ "${SLICE:-0}" = 0 ] && chk "$@" ;;
  esac
  return 0
}
chk36() {
  case "$WORLD_PROFILE" in
    alliance-single|instances|full-world|full-instances) chk "$@" ;;
    "") case " ${INCLUDE_MAPS//,/ } " in *" 36 "*) chk "$@" ;; esac ;;
  esac
  return 0
}
chk_open_world() { [ "$WORLD_PROFILE" != instances ] && [ "$WORLD_PROFILE" != full-instances ] && chk "$@"; return 0; }
# PROFILE ASSERTION HELPERS END
n() {
  local out status
  out="$(q "$1" 2>&1)"; status=$?
  if [ "$status" -ne 0 ]; then sql_note_failure "$out"; printf '0'; return 0; fi
  printf '%s\n' "$out" | grep -cE "${2:-^ *[0-9]}"
}
q_list() {
  local out status
  out="$(q "$1" 2>&1)"; status=$?
  if [ "$status" -ne 0 ]; then sql_note_failure "$out"; return 0; fi
  printf '%s\n' "$out" | grep -oE '^ *[0-9]+ *$' | tr -d ' '
}
# SQL TABLE PULL BEGIN
q_table() {
  local out status
  out="$(q "$1" 2>&1)"; status=$?
  if [ "$status" -ne 0 ]; then sql_note_failure "$out"; return 0; fi
  printf '%s\n' "$out" | awk -F'|' '
    { sub(/^[[:space:]|]+/, ""); sub(/[[:space:]|]+$/, "") }
    NF == 0 { next }
    {
      numeric = 1
      row = ""
      for (i = 1; i <= NF; i++) {
        gsub(/[[:space:]]/, "", $i)
        if ($i !~ /^[0-9]+$/) { numeric = 0; break }
        row = row (i > 1 ? " " : "") $i
      }
      if (numeric) print row
    }'
}
# SQL TABLE PULL END
quest_giver_levels_in_band() {
  local entry="$1" low="$2" high="$3" quest level matched=0
  for quest in $(q_list "SELECT quest_entry FROM game_creature_quest WHERE creature_entry = $entry"); do
    for level in $(q_list "SELECT quest_level FROM game_quest_template WHERE entry = $quest"); do
      [ "$level" -ge "$low" ] && [ "$level" -le "$high" ] && matched=$((matched + 1))
    done
  done
  printf '%s' "$matched"
}
verify_caster_spell_catalogue() {
  local table spell entries missing=0
  for table in game_creature_cast game_creature_spell; do
    for spell in $(q_list "SELECT spell_id FROM $table" | sort -u); do
      [ "$(n "SELECT spell_id FROM game_spell WHERE spell_id = $spell")" -gt 0 ] && continue
      entries="$(q_list "SELECT creature_entry FROM $table WHERE spell_id = $spell" | sort -u | tr '\n' ' ')"
      echo "  FAIL  caster spell catalogue: $table creature entry/entries ${entries% } reference missing spell $spell"
      missing=1
    done
  done
  sql_check_abort
  if [ "$missing" = 0 ]; then
    echo "  ok    caster spell catalogue: every imported caster spell resolves"
  else
    fail=1
  fi
}
# EVENTAI VERIFY BEGIN
verify_eventai_catalogue() {
  local definitions count malformed duplicate
  definitions="$(q_table "SELECT id, creature_entry, creature_guid, definition_revision FROM game_creature_ai_definition")"
  count="$(printf '%s\n' "$definitions" | awk 'NF && $1 > 0 { rows++ } END { print rows + 0 }')"
  if [ "$count" -eq 0 ]; then
    echo "  ..    EventAI: no native definitions in this World Import Scope"
    return 0
  fi
  chk "EventAI native definitions" 1 "$count"
  malformed="$(printf '%s\n' "$definitions" | awk 'NF && (($2 == 0) == ($3 == 0)) { rows++ } END { print rows + 0 }')"
  duplicate="$(printf '%s\n' "$definitions" | awk '
    NF {
      subject = ($2 > 0 ? "entry:" $2 : "guid:" $3)
      if (++seen[subject] == 2) rows++
    }
    END { print rows + 0 }')"
  sql_check_abort
  if [ "$malformed" -ne 0 ] || [ "$duplicate" -ne 0 ]; then
    echo "  FAIL  EventAI definitions: malformed-subjects=$malformed duplicate-subjects=$duplicate"
    fail=1
  else
    echo "  ok    EventAI native definitions have unique entry or guid subjects"
  fi
}
# EVENTAI VERIFY END
verify_alliance_creation_data() {
  chk "Human start positions (all 6 classes, map 0)" "$FLOOR_HUMAN_START_POSITIONS" "$(n "SELECT race_class FROM game_start_position WHERE race=1 AND map_id=0")"
  chk "Dwarf start positions (all 5 classes, map 0)" "$FLOOR_DWARF_START_POSITIONS" "$(n "SELECT race_class FROM game_start_position WHERE race=3 AND map_id=0")"
  chk "Gnome start positions (all 4 classes, map 0)" "$FLOOR_GNOME_START_POSITIONS" "$(n "SELECT race_class FROM game_start_position WHERE race=7 AND map_id=0")"
  chk "Night Elf start positions (all 5 classes, map 1)" "$FLOOR_NIGHT_ELF_START_POSITIONS" "$(n "SELECT race_class FROM game_start_position WHERE race=4 AND map_id=1")"
  chk "Dwarf start-item families (no Human fallback)" "$FLOOR_DWARF_START_ITEMS_CLASSES" "$(q_list "SELECT race_class FROM game_start_item WHERE race_class=769 OR race_class=770 OR race_class=771 OR race_class=772 OR race_class=773" | sort -u | wc -l)"
  chk "Gnome start-item families (no Human fallback)" "$FLOOR_GNOME_START_ITEMS_CLASSES" "$(q_list "SELECT race_class FROM game_start_item WHERE race_class=1793 OR race_class=1796 OR race_class=1800 OR race_class=1801" | sort -u | wc -l)"
  chk "Night Elf start-item families (no Human fallback)" "$FLOOR_NIGHT_ELF_START_ITEMS_CLASSES" "$(q_list "SELECT race_class FROM game_start_item WHERE race_class=1025 OR race_class=1027 OR race_class=1028 OR race_class=1029 OR race_class=1035" | sort -u | wc -l)"
}
verify_named_corridor() {
  local label="$1" map="$2" giver="$3" floor="$4"
  chk "$label correct-map quest-giver spawn ($giver, map $map)" "$floor" "$(n "SELECT guid FROM game_creature_spawn WHERE entry=$giver AND map_id=$map")"
  chk "$label quest-giver relation ($giver)" "$floor" "$(n "SELECT quest_entry FROM game_creature_quest WHERE creature_entry=$giver")"
  chk "$label quest level band (1-20)" "$FLOOR_CORRIDOR_QUEST_LEVEL_BAND" "$(quest_giver_levels_in_band "$giver" 1 20)"
}
verify_graveyard_zone() {
  local label="$1" zone="$2" id resolved=0
  for id in $(q_list "SELECT safe_loc_id FROM game_graveyard_zone WHERE zone_id = $zone"); do
    [ "$(n "SELECT id FROM game_graveyard WHERE id = $id")" -gt 0 ] && resolved=$((resolved + 1))
  done
  chk "$label graveyard-zone links resolve (zone $zone)" "$FLOOR_GRAVEYARD_ZONE_RESOLVE" "$resolved"
}
verify_profile_corridors() {
  case "$WORLD_PROFILE" in
    alliance-eastern|alliance-single|full-eastern|full-world)
      verify_named_corridor "Dun Morogh start" 0 658 "$FLOOR_DUN_MOROGH_START"
      chk "Dun Morogh Paladin trainer (Bromos Grummner 926)" 1 "$(n "SELECT spell_id FROM game_trainer_spell WHERE trainer_entry=926")"
      chk "Dun Morogh vendor (Durnan Furcutter 836)" 1 "$(n "SELECT item_entry FROM game_npc_vendor WHERE creature_entry=836")"
      verify_named_corridor "Loch Modan follow-on" 0 1340 "$FLOOR_LOCH_MODAN_CONTENT"
      chk "Loch Modan Mining trainer (Brock Stoneseeker 1681)" 1 "$(n "SELECT spell_id FROM game_trainer_spell WHERE trainer_entry=1681")"
      chk "Eastern profile GameObjects (map 0)" "$FLOOR_CORRIDOR_GAMEOBJECTS" "$(n "SELECT guid FROM game_gameobject WHERE map_id=0")"
      verify_graveyard_zone "Dun Morogh" 1
      verify_graveyard_zone "Loch Modan" 38
      ;;
    alliance-kalimdor)
      ;;
  esac
  case "$WORLD_PROFILE" in
    alliance-kalimdor|alliance-single|full-kalimdor|full-world)
      verify_named_corridor "Teldrassil start" 1 2079 "$FLOOR_TELDRASSIL_START"
      chk "Teldrassil Warrior trainer (Alyissia 3593)" 1 "$(n "SELECT spell_id FROM game_trainer_spell WHERE trainer_entry=3593")"
      verify_named_corridor "Darkshore follow-on" 1 10219 "$FLOOR_DARKSHORE_CONTENT"
      chk "Darkshore vendor (Laird 4200)" 1 "$(n "SELECT item_entry FROM game_npc_vendor WHERE creature_entry=4200")"
      chk "Kalimdor profile GameObjects (map 1)" "$FLOOR_CORRIDOR_GAMEOBJECTS" "$(n "SELECT guid FROM game_gameobject WHERE map_id=1")"
      verify_graveyard_zone "Teldrassil" 141
      verify_graveyard_zone "Darkshore" 148
      ;;
  esac
}
# TAXI FIXTURE VERIFY BEGIN
verify_taxi_fixture_anchors() {
  chk "reserved taxi source storage/wire node" 1 "$(n "SELECT id FROM game_taxi_node WHERE id=5090100 AND client_node_id=255 AND map_id=0")"
  chk "reserved taxi destination storage/wire node" 1 "$(n "SELECT id FROM game_taxi_node WHERE id=5090101 AND client_node_id=256 AND map_id=0")"
  chk "reserved directed taxi route" 1 "$(n "SELECT id FROM game_taxi_path WHERE id=5090102 AND source_node_id=5090100 AND destination_node_id=5090101 AND fare=25")"
  chk "reserved ordered taxi geometry" 3 "$(n "SELECT id FROM game_taxi_path_node WHERE path_id=5090102 AND map_id=0")"
  chk "reserved flight-master template" 1 "$(n "SELECT entry FROM game_creature_template WHERE entry=51006 AND npc_flags=9")"
  chk "reserved flight-master spawn" 1 "$(n "SELECT guid FROM game_creature_spawn WHERE guid=17379391817761423361 AND entry=51006 AND map_id=0")"
  chk "reserved live flight-master entity" 1 "$(n "SELECT guid FROM game_world_entity WHERE guid=17379391817761423361 AND entry=51006 AND map_id=0")"
}
# TAXI FIXTURE VERIFY END
echo "[world] assertions (map $MAP → $DB):"
if [ -z "$WORLD_PROFILE" ] && [ "$SLICE" != 0 ]; then
  echo "  ..    named Human and Deadmines checks SKIPPED — custom legacy slice"
elif [ "$WORLD_PROFILE" = alliance-kalimdor ]; then
  echo "  ..    named Human and Deadmines checks SKIPPED — alliance-kalimdor does not own them"
elif { [ "$WORLD_PROFILE" = instances ] || [ "$WORLD_PROFILE" = full-instances ]; }; then
  echo "  ..    open-world corridor checks SKIPPED — instance profiles have no continent terrain"
fi
[ "$MAP" != 0 ] || verify_taxi_fixture_anchors
verify_alliance_creation_data
[ -z "$WORLD_PROFILE" ] || verify_profile_corridors
if [ "$WORLD_PROFILE" != instances ] && [ "$WORLD_PROFILE" != full-instances ]; then
chk0 "Goldshire anchor trainers spawned {328,377,906,913,917,927} (NOT total coverage — see the service-coverage audit below)" "$FLOOR_CLASS_TRAINERS" \
    "$(n "SELECT guid FROM game_world_entity WHERE owner_guid=0 AND (entry=328 OR entry=377 OR entry=906 OR entry=913 OR entry=917 OR entry=927)" '[0-9]{6,}')"
chk "class trainer offerings (learn_skill_line=0)"   "$FLOOR_TRAINER_OFFERINGS_CLASS" "$(n "SELECT spell_id FROM game_trainer_spell WHERE learn_skill_line = 0")"
chk "profession learn offerings (learn_skill_line>0)" "$FLOOR_TRAINER_OFFERINGS_PROFESSION" "$(n "SELECT spell_id FROM game_trainer_spell WHERE learn_skill_line > 0")"
chk "gather-node GO spawns"                          "$FLOOR_GATHER_NODE_GO" "$(n "SELECT guid FROM game_gameobject" '[0-9]')"
chk_open_world "terrain chunks (ground-z heightmap)" "$FLOOR_TERRAIN_CHUNKS" "$(n "SELECT key FROM game_terrain_chunk" '[0-9]')"
chk_open_world "nav chunks (walkability/LoS grid)"   "$FLOOR_NAV_CHUNKS" "$(n "SELECT key FROM game_nav_chunk" '[0-9]')"
chk0 "innkeeper Farley(295) spawned"      "$FLOOR_INNKEEPER_FARLEY" "$(n "SELECT guid FROM game_world_entity WHERE entry=295 AND owner_guid=0" '[0-9]{6,}')"
chk "spawn rows on this run's map ($MAP)" "$FLOOR_CONTINENT_SPAWNS" "$(n "SELECT guid FROM game_creature_spawn WHERE map_id = $MAP" '[0-9]')"
chk "quest-giver relations"              "$FLOOR_QUEST_GIVER_RELATIONS" "$(n "SELECT quest_entry FROM game_creature_quest")"
chk "L6-10 quests (quest_level band)"    "$FLOOR_QUESTS_L6_10" "$(q_list "SELECT quest_level FROM game_quest_template" | awk '$1>=6 && $1<=10' | wc -l)"
chk "L10-20 quests (Westfall quest_level band)" "$FLOOR_QUESTS_L10_20" "$(q_list "SELECT quest_level FROM game_quest_template" | awk '$1>=10 && $1<=20' | wc -l)"
# SpacetimeDB single-column inequality filters can return zero rows incorrectly.
chk "chained quests (next_quest_id>0) [V]" "$FLOOR_QUESTS_CHAINED" "$(q_list "SELECT next_quest_id FROM game_quest_template" | awk '$1>0' | wc -l)"
echo "  ..  timed quests (limit_time>0) [V]: $(q_list "SELECT limit_time FROM game_quest_template" | awk '$1>0' | wc -l)"
sql_check_abort   # No chk follows this query to check the abort flag.
chk "live creature entities"           "$FLOOR_LIVE_CREATURES" "$(n "SELECT guid FROM game_world_entity WHERE entry>0 AND owner_guid=0" '[0-9]{6,}')"
chk "vendors (npc_vendor)"              "$FLOOR_VENDORS" "$(n "SELECT item_entry FROM game_npc_vendor")"
echo "[world] service coverage (flagged NPCs that actually provide their service):"
if ! command -v python3 >/dev/null 2>&1; then
  echo "  SKIP: service-coverage audit needs python3 (not found on PATH) — install it and re-run this"
  echo "        script to get trainer/vendor/questgiver dead-NPC coverage; this does not fail the import."
else
COV=$(python3 - "$DB" "$SPACETIME_SERVER" <<'PYEOF'
import subprocess, sys
db, server = sys.argv[1], sys.argv[2]
def rows(q):
    out = subprocess.run(["spacetime","sql","--server",server,db,q],capture_output=True,text=True).stdout
    return [l.strip().strip('|').split('|') for l in out.splitlines() if l.startswith(' ')]
tmpl = {}
for r in rows("SELECT entry, npc_flags, name FROM game_creature_template"):
    try: tmpl[int(r[0])] = (int(r[1]), r[2].strip().strip('"'))
    except (ValueError, IndexError): pass
spawned = {int(r[0]) for r in rows("SELECT entry FROM game_world_entity WHERE owner_guid = 0") if r[0].strip().isdigit()}
teach = {int(r[0]) for r in rows("SELECT trainer_entry FROM game_trainer_spell") if r[0].strip().isdigit()}
sell  = {int(r[0]) for r in rows("SELECT creature_entry FROM game_npc_vendor") if r[0].strip().isdigit()}
give  = {int(r[0]) for r in rows("SELECT creature_entry FROM game_creature_quest") if r[0].strip().isdigit()}
# Trainers with NO offerings BY DESIGN (each has a system that isn't the class-spell path):
KNOWN_EMPTY = {2485: "portal trainer (teleports not curated yet)",
               4732: "riding trainer (189 mounts)",
               2879: "pet trainer (188 pet system)",
               15351: "PvP-rank reward vendor (no honor system yet)"}
def audit(label, bit, providers):
    flagged = {e for e in spawned if e in tmpl and tmpl[e][0] & bit}
    ok = flagged & providers
    dead = sorted(flagged - providers - set(KNOWN_EMPTY))
    known = sorted((flagged - providers) & set(KNOWN_EMPTY))
    names = ", ".join(f"{tmpl[e][1]}({e})" for e in dead[:12]) + (" …" if len(dead) > 12 else "")
    mark = "ok   " if not dead else "GAP  "
    extra = f" known-empty={len(known)} ({', '.join(KNOWN_EMPTY[e] for e in known)})" if known else ""
    print(f"  {mark} {label}: flagged+spawned={len(flagged)} providing={len(ok)} dead={len(dead)}{extra}" + (f": {names}" if dead else ""))
    return len(ok)
t = audit("trainers (npc_flags&0x10 vs game_trainer_spell)", 0x10, teach)
v = audit("vendors (npc_flags&0x4 vs game_npc_vendor)", 0x4, sell)
q = audit("questgivers (npc_flags&0x2 vs game_creature_quest; quiet ones may be off-level content)", 0x2, give)
print(f"COUNTS {t} {v} {q}")
PYEOF
)
echo "$COV" | grep -v ^COUNTS
read -r COV_TRAINERS COV_VENDORS COV_QUESTGIVERS <<<"$(echo "$COV" | grep ^COUNTS | cut -d' ' -f2-)"
chk "trainer coverage (spawned trainers that teach)"    "$FLOOR_TRAINER_COVERAGE" "$COV_TRAINERS"
chk "vendor coverage (spawned vendors that sell)"       "$FLOOR_VENDOR_COVERAGE" "$COV_VENDORS"
chk "questgiver coverage (spawned givers with quests)"  "$FLOOR_QUESTGIVER_COVERAGE" "$COV_QUESTGIVERS"
fi
fi

SENTINEL_HILL_VENDOR_ENTRY="${SENTINEL_HILL_VENDOR_ENTRY:-491}"
chk0 "Sentinel Hill vendor spawned (Quartermaster Lewis 491)" "$FLOOR_SENTINEL_HILL_VENDOR" \
    "$(n "SELECT guid FROM game_world_entity WHERE owner_guid=0 AND entry=${SENTINEL_HILL_VENDOR_ENTRY}" '[0-9]{6,}')"
# race_class encodes (race << 8) | class.
chk "Human start-items (all 6 classes)"   "$FLOOR_START_ITEMS_CLASSES" \
    "$(q_list "SELECT race_class FROM game_start_item WHERE race_class=257 OR race_class=258 OR race_class=260 OR race_class=261 OR race_class=264 OR race_class=265" | sort -u | wc -l)"
chk "caster-mob cast rows"               "$FLOOR_CASTER_CAST_ROWS" "$(n "SELECT creature_entry FROM game_creature_cast")"
chk "total game_spell rows (vmangos spell catalogue)" "$FLOOR_SPELL_TOTAL" "$(n "SELECT spell_id FROM game_spell")"
chk "spell chain rows (game_spell_chain) [V]"            "$FLOOR_SPELL_CHAIN" "$(n "SELECT spell_id FROM game_spell_chain")"
chk "spell auto-learn dependents (game_spell_learn) [V]" "$FLOOR_SPELL_LEARN" "$(n "SELECT id FROM game_spell_learn")"
chk "proc overlay rows (game_spell_proc_event) [V]"      "$FLOOR_SPELL_PROC_EVENT" "$(n "SELECT spell_id FROM game_spell_proc_event")"
chk "476 Geomancer cast spell present"    "$FLOOR_GEOMANCER_CAST_SPELL" "$(n "SELECT spell_id FROM game_spell WHERE spell_id=20793")"
chk "474 Rogue Wizard cast spell present" "$FLOOR_ROGUE_WIZARD_CAST_SPELL" "$(n "SELECT spell_id FROM game_spell WHERE spell_id=13322")"
chk0 "Imp(416) Firebolt cast row"         "$FLOOR_IMP_FIREBOLT_CAST_ROW" "$(n "SELECT spell_id FROM game_creature_cast WHERE creature_entry=416")"
chk "Imp Firebolt(3110) in game_spell"    "$FLOOR_IMP_FIREBOLT_SPELL" "$(n "SELECT spell_id FROM game_spell WHERE spell_id=3110")"
chk0 "Defias Pillager Fireball cast row" "$FLOOR_DEFIAS_CASTER_CAST_ROW" \
    "$(n "SELECT creature_entry FROM game_creature_cast WHERE creature_entry=589 AND spell_id=19816")"
chk0 "rotation rows (game_creature_spell)" "$FLOOR_ROTATION_ROWS" "$(n "SELECT id FROM game_creature_spell")"
chk0 "476 Geomancer Fireball cast row"     "$FLOOR_GEOMANCER_CAST_ROW" "$(n "SELECT spell_id FROM game_creature_cast WHERE creature_entry=476 AND spell_id=20793")"
chk0 "449 Knuckleduster Pummel rotation row"    "$FLOOR_KNUCKLEDUSTER_ROTATION" "$(n "SELECT id FROM game_creature_spell WHERE creature_entry=449 AND spell_id=12555")"
verify_caster_spell_catalogue
verify_eventai_catalogue

chk "areas imported (game_area) [V]"                 "$FLOOR_AREAS" "$(n "SELECT id FROM game_area")"
chk "area triggers imported (game_area_trigger) [V]"  "$FLOOR_AREA_TRIGGERS" "$(n "SELECT id FROM game_area_trigger")"
chk "graveyards imported (game_graveyard) [V]"         "$FLOOR_GRAVEYARDS" "$(n "SELECT id FROM game_graveyard")"
# IDs at or above 5090000 belong to the reserved taxi fixture.
chk "taxi nodes imported from vmangos"            "$FLOOR_TAXI_NODES" "$(n "SELECT id FROM game_taxi_node WHERE id < 5090000")"
chk "directed taxi paths imported from TaxiPath.dbc"    "$FLOOR_TAXI_PATHS" "$(n "SELECT id FROM game_taxi_path WHERE id < 5090000")"
chk "taxi path points imported from TaxiPathNode.dbc"   "$FLOOR_TAXI_PATH_NODES" "$(n "SELECT id FROM game_taxi_path_node WHERE id < 5090000")"
if [ -n "$GRAVEYARD_ZONE" ]; then
  echo "[world] zone-$GRAVEYARD_ZONE graveyard_zone→game_graveyard resolve check (shell-side, no SQL JOIN):"
  zone_ids="$(q_list "SELECT safe_loc_id FROM game_graveyard_zone WHERE zone_id = $GRAVEYARD_ZONE")"
  zone_resolved=0
  for id in $zone_ids; do
    hit="$(n "SELECT id FROM game_graveyard WHERE id = $id" "^ *${id} *\$")"
    if [ "${hit:-0}" -gt 0 ]; then zone_resolved=$((zone_resolved + 1)); fi
  done
  chk "zone-$GRAVEYARD_ZONE graveyard_zone links resolve to a real game_graveyard row" "$FLOOR_GRAVEYARD_ZONE_RESOLVE" "$zone_resolved"
else
  echo "  ..    graveyard_zone resolve check SKIPPED — set GRAVEYARD_ZONE=<a zone id inside your box> to enable it"
fi

chk "areatrigger_teleport portals imported (game_areatrigger_teleport) [V]" "$FLOOR_AREATRIGGER_TELEPORTS" \
    "$(n "SELECT trigger_id FROM game_areatrigger_teleport")"
chk "Deadmines portal round-trip: entrance(78)+exit(119) both present" "$FLOOR_DEADMINES_PORTAL_ROUNDTRIP" \
    "$(n "SELECT trigger_id FROM game_areatrigger_teleport WHERE trigger_id=78 OR trigger_id=119")"

# Imported spawn rows are independent of the creature tick timing.
chk36 "Deadmines creature spawns (map 36) [V]" "$FLOOR_DEADMINES_CREATURE_SPAWNS" \
    "$(n "SELECT guid FROM game_creature_spawn WHERE map_id = 36" '[0-9]')"
chk36 "Deadmines gameobjects (doors/cannon/chests, map 36) [V]" "$FLOOR_DEADMINES_GAMEOBJECTS" \
    "$(n "SELECT guid FROM game_gameobject WHERE map_id = 36" '[0-9]')"
chk36 "Deadmines bosses spawned (>=6 distinct placed entries) [V]" "$FLOOR_DEADMINES_BOSSES" \
    "$(q_list "SELECT entry FROM game_creature_spawn WHERE map_id = 36 AND (entry=644 OR entry=643 OR entry=642 OR entry=1763 OR entry=646 OR entry=647 OR entry=639)" | sort -u | wc -l)"
chk36 "Deadmines named drops (Cruel Barb ~5191 + Smite's Reaver ~5196) [V]" "$FLOOR_DEADMINES_NAMED_LOOT" \
    "$(q_list "SELECT item_entry FROM game_creature_loot WHERE item_entry=5191 OR item_entry=5196" | sort -u | wc -l)"

chk "pickpocket loot rows (game_pickpocket_loot) [V]"   "$FLOOR_PICKPOCKET_LOOT" "$(n "SELECT id FROM game_pickpocket_loot")"
chk "skinning loot rows (game_skinning_loot) [V]"        "$FLOOR_SKINNING_LOOT" "$(n "SELECT id FROM game_skinning_loot")"
chk "gameobject (chest) loot rows (game_gameobject_loot) [V]" "$FLOOR_GAMEOBJECT_CHEST_LOOT" "$(n "SELECT id FROM game_gameobject_loot")"
chk "fishing loot rows (game_fishing_loot) [V]"          "$FLOOR_FISHING_LOOT" "$(n "SELECT id FROM game_fishing_loot")"
chk "creatures with a pickpocket table (creature_template.pickpocket_loot_id>0) [V]" "$FLOOR_CREATURES_WITH_PICKPOCKET" \
    "$(n "SELECT entry FROM game_creature_template WHERE pickpocket_loot_id > 0")"
chk "creatures with a skin table (creature_template.skin_loot_id>0) [V]" "$FLOOR_CREATURES_WITH_SKIN" \
    "$(n "SELECT entry FROM game_creature_template WHERE skin_loot_id > 0")"

# Seeing this run's primary map prevents an empty query from proving ownership.
POST_PROBE="$(db_spatial_probe)"
POST_UNREADABLE="$(probe_unreadable "$POST_PROBE")"
POST_FOREIGN="$(foreign_spatial_maps "$POST_PROBE")"
if [ -n "${POST_FOREIGN// /}" ] && db_never_imported; then
  POST_FOREIGN=""
fi
if [ -n "${POST_UNREADABLE// /}" ]; then
  echo "  FAIL  one-continent re-assert could not read map_id from: ${POST_UNREADABLE% } — the check is INCONCLUSIVE, not clean"
  fail=1
elif [ -n "${POST_FOREIGN// /}" ]; then
  echo "  FAIL  cross-continent contamination: '$DB' holds spatial rows for map(s) ${POST_FOREIGN% } after importing map(s) $(echo "$EXPECTED_MAPS" | tr '\n' ' ')"
  fail=1
elif ! probe_maps "$POST_PROBE" | grep -qx -- "$MAP"; then
  echo "  FAIL  one-continent re-assert saw NO spatial row on this run's own map ($MAP) — the probe read nothing, so 'no foreign maps' proves nothing"
  fail=1
else
  echo "  ok    one continent per database: spatial rows only on map(s) $(echo "$EXPECTED_MAPS" | tr '\n' ' ')"
fi

if [ "$fail" = 0 ]; then echo "[world] OK — all 1-20 blockers present"; else echo "[world] REGRESSION — see FAILs above"; exit 1; fi
