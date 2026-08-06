#!/usr/bin/env bash
# import-world.sh — THE world import. It takes a cmangos world dump plus your extracted client data
# and turns an empty LyraCore database into a populated, playable world, then ASSERTS that it worked.
#
# WHEN YOU RUN IT: once per database, after publishing the module — and again whenever you change the
# box, bump the dump, or reprovision destructively. NEEDS, all four:
#   1. a running SpacetimeDB node with the target database published (and claimed by you)
#   2. the world dump at $DUMP — get it with importer/scripts/pull-classic-db.sh
#   3. an extracted client Data directory at $DBC (the 1.12.1 DBCs and terrain MPQs)
#   4. a Rust toolchain — it builds the importer itself
# `lyracore import` drives this, and the puller and the class-spell pass, for you. Running this
# script by hand is the advanced path: several shards, a non-default box, or a second continent.
#
# WHAT IT IMPORTS. Creature templates/spawns, quests, loot, vendors and gameobjects for ONE widened
# box covering ALL of Elwynn + the Goldshire/Eastvale/Westbrook/Fargodeep hubs + the Stormwind
# approach + ALL of Westfall (Sentinel Hill, the coast, the Deadmines overworld half) — the 1-20
# levelling corridor. Then terrain, nav, the DBC tables, the full Spell.dbc, the curated class spells
# and the caster-mob spells. Deliberately GENEROUS — adjacent-zone spillover (Stormwind/Redridge/
# Duskwood edges) is fine; it's more content and the AOI scopes the per-player relay, so the extra
# spawns cost a connected client nothing.
#
# WHY THE BOX MATTERS (a real regression this box guards against): the 6 Human class trainers are in
# GOLDSHIRE at x=-9461.85; a box starting at x=-9460 excludes them by ~1.85 units, so they never
# import and no class can learn past its starter kit. Keep X0 well WEST of Goldshire.
#
# ONE BOX, NOT TWO PASSES: the ETL is clear+reload PER FAMILY (creature/quest/loot/vendor/gameobject
# tables are wiped and reloaded whole on every run), so a second `--dump --box` pass covering Westfall
# would WIPE the Elwynn rows the first pass just loaded, not union with them — there is no additive
# two-pass path. Widening the single box is therefore the only correct way to cover both zones in one
# importer run.
#
# After the ETL: re-run the curated spell-import. The ETL's own npc_trainer class trees are the
# PRIMARY offerings (the full Spell.dbc import makes every offered id resolvable, so the safe policy
# is to offer everything and let the level gate red-out the high ranks), and the curated pass only
# overlays the "specials" the dump never sells. Then re-arm the creature tick and ASSERT the 1-20
# blockers landed — loudly, so a box or ETL regression fails here instead of surfacing in play.
#
# [V] on a value below means: measured or guessed against ONE operator's dump/client, not verified
# against yours. Confirm it against your own data before relying on it; correct a wrong id rather
# than loosening the check around it. Expect to retune the [V] count floors (they live in
# importer/scripts/import-manifest.sh) once you have real numbers from your own first import.
#
# Run from the checkout root.
# Usage: importer/scripts/import-world.sh            (the canonical map-0 box)
#        BOX="X0,X1,Y0,Y1" importer/scripts/import-world.sh   (override)
#        DB=lyracore-world-1 MAP=1 BOX="…" CENTER="X,Y,Z" importer/scripts/import-world.sh   (a second CONTINENT)
#
# BEFORE YOUR FIRST IMPORT: derive YOUR real box from YOUR dump instead of trusting the ballpark
# below —
#   cargo build -q --bin lyracore-importer && ./target/debug/lyracore-importer --dump "$DUMP" --map 0 --print-extents
# — and compare its suggested --box against BOX below; widen or replace as needed.
#
# ONE SCRIPT, ONE CONTINENT PER DATABASE —
# `MAP` picks which continent this run imports. The importer already has `--map`, `--box`, `--db` and
# `--include-map`, so a second continent is this script threading `MAP` through them plus a map fence
# and map-aware assertions — NOT a second pipeline. `MAP=0` (the default) is the canonical run.
#
#   DB=lyracore-world-1 MAP=1 BOX="-1400,600,-5400,-3600" CENTER="-618.52,-4251.67,38.72" \
#     bash importer/scripts/import-world.sh
#
# WHY NEITHER DATABASE EVER CARRIES BOTH CONTINENTS' SPATIAL DATA — three layers, none of them
# "the boxes happen not to overlap":
#   1. The importer's MAP FENCE (`creature_row_kept`/`row_in_slice`): a spawn/GO row on a map this run
#      does not own can never enter the slice — including via `--include-creatures`, which used to
#      match on entry alone and so could drag a foreign map's row in.
#   2. The ETL is clear+reload PER FAMILY and the clears are WHOLESALE (module `import_terrain_chunks`
#      / `import_nav_chunks` / `import_creature_spawns` delete EVERY row before loading batch 0). So a
#      map-1 run against a map-0 database does not merge the two continents — it REPLACES one with the
#      other. Structurally, one database can only ever hold the LAST run's maps.
#   3. …which is a footgun, not a safety feature: pointing a map-1 run at the live world-0 database
#      would silently swap Elwynn for Kalimdor. So the PREFLIGHT below reads the map ids already
#      present in the target database's spatial tables and REFUSES when they aren't this run's maps
#      (`ALLOW_MAP_SWITCH=1` to overwrite deliberately), and the assertion block re-checks the same
#      invariant after the run — no foreign-map spatial row may exist when the ETL finishes.
# The non-spatial families (Spell.dbc, the DBC character-creation/faction/area/graveyard tables, the
# world-wide `areatrigger_teleport` portal table, quest/item/loot templates) are deliberately on BOTH
# shards: a quest template is not spatial data, and every shard needs the full spell/item catalogue.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1
# Which CONTINENT this run imports. 0 = the canonical Elwynn/Westfall + Deadmines corridor.
# Exported so the manifest's MAP!=0 floor overrides see it (sourced below, and `[V]`-loose on purpose).
export MAP="${MAP:-0}"
# Is this a SLICE — a run whose box is not the canonical map-0 corridor? Two ways to be one: another
# continent (MAP!=0), or a caller-chosen map-0 box, which is what a REGION shard is.
# Both mean the same two things: the corridor's named-fixture assertions cannot apply, and the
# count floors tuned for the corridor are wrong. One flag so those two facts can never disagree.
# Computed from the environment before the manifest is sourced, because it gates the floor overrides.
export SLICE=0
{ [ "$MAP" != 0 ] || [ -n "${BOX:-}" ]; } && export SLICE=1
# shellcheck source=importer/scripts/import-manifest.sh
source importer/scripts/import-manifest.sh   # FLOOR_* assertion minimums — see that file's header

DUMP="${DUMP:-.import/classic-db-full.sql}"
DBC="${DBC:-../wowclient/Data}"
# [V] X0,X1,Y0,Y1 — full-Elwynn + full-Westfall + ALL of Redridge (Lakeshire, the natural Human L15-25
# zone) + north-Duskwood spillover. Redridge sits entirely within the X band (-11400..-8000) and was
# excluded ONLY by the old Y-floor -1600; dropping it to -3100 admits Lakeshire/Camp Everstill/Stonewatch
# so the 15-18 solo-quest stretch (~59 Redridge quest relations) becomes completable.
# Loch Modan is DEFERRED — off the Human corridor, needs a big eastward X extension that sweeps in noise.
# NOTE: this widens the box ~28% → +~1286 spawns and a ~30% longer terrain+nav ETL, both measured on a
# real import; the RUNTIME cost is ~0, because creature ticking is scoped to cells that have players in
# them. CONFIRM against `--print-extents` on your own dump before importing; tighten or widen as your
# real spawn extents dictate.
# The canonical map-0 rectangle is ONLY a default for map 0 — see the MAP!=0 branch below, which
# refuses to guess a second continent's box.
[ "$MAP" = 0 ] && BOX="${BOX:--11400,-8000,-3100,2000}"
# Out-of-box quest givers force-imported by ENTRY (--include-creatures): 10 NPCs whose
# spawns sit OUTSIDE the box (Westfall/Stormwind/Dun Morogh) but whose sibling giver is in-band, so
# importing them recovers 20 fully-completable broken 1-20 quests (measured with the module's
# debug_audit_quest_chains reducer on a real import). Ranked
# by completable-quest yield: 344 Magistrate Solomon (+6 couriers), 11406 High Priest Rohan (+3 priest
# ability quests), 266 Wiley the Black (+2 Defias), 415 Verner Osgood (+2), 1343 Mountaineer Stormpike
# (+2), 6966 Lucius (+1), 5165 Hulfdan (+1), 6166 Yorus Barleybrew (+1 warrior), 6569 Gnoarn (+1),
# 5149 Brandur Ironhammer (+1 paladin Tome of Divinity entry).
# …and they are all EASTERN KINGDOMS (map 0) entries, so the default is map-0-only: force-including
# them on another continent's run would ask the importer for foreign-map spawns (the map fence in
# `creature_row_kept` now refuses those outright, but shipping the flag would still be nonsense).
[ "$MAP" = 0 ] && INCLUDE_CREATURES="${INCLUDE_CREATURES-344,11406,266,415,1343,6966,5165,6166,6569,5149}"
INCLUDE_CREATURES="${INCLUDE_CREATURES-}"
# Overridable so the SAME ETL can load a second shard: the instances database spawns Deadmines itself
# from map-36 `game_creature_spawn` rows, so it needs its own copy of them —
# `DB=lyracore-instances bash importer/scripts/import-world.sh`. Defaults to the world/realm database.
DB="${DB:-lyracore}"
# Whole extra maps folded into the SAME clear+reload run (--include-map). Map 0's canonical run carries
# the Deadmines interior (36); another continent gets none by default — its own instanced maps belong
# to the instances database, not to a continent shard.
[ "$MAP" = 0 ] && INCLUDE_MAPS="${INCLUDE_MAPS-36}"
INCLUDE_MAPS="${INCLUDE_MAPS-}"
# --center: the point the terrain interpolate self-check and the nav walkability self-check sample. It
# MUST lie inside $BOX and its Z must be the real ground height there (the terrain check hard-fails past
# 2 yd). Empty = the importer's own Northshire default, which is why map 0's command line is unchanged.
CENTER="${CENTER:-}"
# A zone id whose `game_graveyard_zone` links must resolve (see the check far below). 12 = Elwynn.
[ "$MAP" = 0 ] && GRAVEYARD_ZONE="${GRAVEYARD_ZONE:-12}"
GRAVEYARD_ZONE="${GRAVEYARD_ZONE:-}"

# ---- MAP != 0: a second continent must be DERIVED, never guessed ---------------------------------
# There is no canonical rectangle for a continent nobody here has imported, and a wrong box is the
# expensive kind of wrong (the Goldshire-trainer regression in this file's header cost a full working
# session). So rather than shipping a remembered Durotar number, this refuses to run without BOX +
# CENTER and prints the exact derivation. `--print-extents` takes a candidate `--box`, so the loop is:
#   1. ./target/debug/lyracore-importer --dump "$DUMP" --map 1 --print-extents
#      → the whole continent's spawn extents AND the dump's own playercreateinfo start positions for
#        map 1 (Valley of Trials / Camp Narache / Shadowglen — the starting-zone anchors).
#   2. Pick an anchor, draw a rough rectangle around it, and re-run with `--box "X0,X1,Y0,Y1"`:
#      → the REAL spawn extents + count inside your candidate, plus a `suggested --center` pinned to an
#        actual spawn in the slice (a real cmangos ground point — exactly what the self-checks need).
#   3. Iterate until the count/extents cover the zone you want; feed the printed values in as BOX/CENTER.
if [ "$MAP" != 0 ]; then
  DERIVE="cargo build -q --bin lyracore-importer && ./target/debug/lyracore-importer --db \"$DB\" --dump $DUMP --map $MAP --print-extents"
  : "${BOX:?MAP=$MAP needs an explicit BOX=X0,X1,Y0,Y1 — derive it, do not guess: $DERIVE (then re-run it with a candidate --box to narrow; see this script header)}"
  : "${CENTER:?MAP=$MAP needs an explicit CENTER=X,Y,Z inside BOX — the terrain/nav self-checks sample it; use the suggested --center that $DERIVE prints for your box}"
fi

# A CALLER-SUPPLIED map-0 box needs CENTER for exactly the same reason a second continent does.
# The importer's default --center is Northshire (-8949.95,-132.49),
# which sits inside the canonical corridor and OUTSIDE any narrower slice of it — e.g. a region shard
# holding only the Goldshire side, whose box ends at x=-9433. The terrain pass then dies with
# "slice center's cell not among collected rows" partway through, after the dump families have already
# been cleared and reloaded. Failing here instead costs nothing and says what to do.
if [ "$MAP" = 0 ] && [ "$SLICE" = 1 ] && [ -z "${CENTER:-}" ]; then
  echo "A caller-supplied map-0 BOX needs an explicit CENTER=X,Y,Z inside it." >&2
  echo "The default --center is Northshire (-8949.95,-132.49,83.53), which is outside any box that" >&2
  echo "does not contain Northshire — the terrain self-check would hard-fail mid-run." >&2
  echo "Derive one (read-only):" >&2
  echo "  cargo build -q --bin lyracore-importer && ./target/debug/lyracore-importer --db \"$DB\" --dump $DUMP \\" >&2
  echo "    --map 0 --box \"$BOX\" --print-extents" >&2
  echo "…then pass the printed 'suggested --center' as CENTER." >&2
  exit 2
fi

# Flag arrays. For MAP=0 with the defaults these expand to exactly the canonical flag set
# (--map 0 --box … --include-map 36 --include-creatures … , no --center) — every MAP/BOX/CENTER knob
# above adds flags rather than changing the default run.
INC_MAP_FLAGS=(); for m in ${INCLUDE_MAPS//,/ }; do INC_MAP_FLAGS+=(--include-map "$m"); done
INC_CREATURE_FLAGS=(); [ -n "$INCLUDE_CREATURES" ] && INC_CREATURE_FLAGS=(--include-creatures "$INCLUDE_CREATURES")
CENTER_FLAGS=(); [ -n "$CENTER" ] && CENTER_FLAGS=(--center "$CENTER")
# Every map whose spatial rows this run is ALLOWED to leave in $DB. Sorted LEXICALLY, not numerically:
# `comm` below merges as text, and a numeric sort ({36,209}) is not a lexical one ({209,36}) — mixing the
# two makes comm silently report a wrong difference. Both sides of the comparison use `sort -u`.
EXPECTED_MAPS="$(printf '%s\n' "$MAP" ${INCLUDE_MAPS//,/ } | sort -u)"

# The map ids whose SPATIAL rows $DB currently holds — the four map-keyed content tables (empty on a
# freshly published shard). Non-spatial tables (spells/items/quests/DBC/areatrigger_teleport) are
# deliberately NOT consulted: every shard carries those.
#
# A FAILED query is NOT the same as "no rows", and conflating them makes this whole guard vacuous: a
# renamed table/column, a shard published from a module that predates one of these four tables, or an
# unreachable node all print nothing a number-grep can match, so the preflight below AND the post-run
# re-assert would BOTH read "no foreign maps" and pass forever — the wrong-column-name-reads-as-0-rows
# trap this repo has been bitten by (docs/danger-zones.md). So each query's EXIT STATUS is checked and an
# unreadable table rides the output as a `!<table>` line, which the two callers split back out below.
# One probe, two consumers, three lines of splitting — cheaper than a second query pass.
# GUARD HELPERS BEGIN — extracted verbatim (marker-delimited) by importer/scripts/import-manifest-smoke.sh's
# offline pin; keep every guard function between these two markers.
db_spatial_probe() {
  for t in game_creature_spawn game_gameobject game_terrain_chunk game_nav_chunk; do
    out="$(spacetime sql "$DB" "SELECT map_id FROM $t" 2>&1)" || { echo "!$t"; continue; }
    printf '%s\n' "$out"
  done | grep -oE '^ *[0-9]+ *$|^!.+' | tr -d ' ' | sort -u
}
probe_unreadable() { printf '%s\n' "$1" | grep '^!' | tr -d '!' | tr '\n' ' '; }
probe_maps()       { printf '%s\n' "$1" | grep -E '^[0-9]+$'; }
foreign_spatial_maps() { comm -23 <(probe_maps "$1") <(printf '%s\n' "$EXPECTED_MAPS") | tr '\n' ' '; }

# Two of the four families above are ones `init` NEVER seeds: only a real `importer --apply`
# run writes to `game_terrain_chunk`/`game_nav_chunk` (module/src/terrain.rs, module/src/nav.rs doc
# comments; module/src/gameobject.rs's `imported_terrain_maps` reads the identical signal for its own
# pool-arming fence). `init` DOES seed 3 map-0 creature rows and up to 5 map-0 gameobject rows into
# EVERY freshly published database (the demo chicken/wolf/trainer + chest/goober/gather-node fixtures),
# so `game_creature_spawn`/`game_gameobject` alone can't tell a never-imported shard from a genuinely
# contaminated one — but `game_terrain_chunk`/`game_nav_chunk` can: empty means nothing has ever been
# really imported here, so ANY foreign map `db_spatial_probe` reports can only be `init`'s own fixtures.
db_imported_probe() {
  for t in game_terrain_chunk game_nav_chunk; do
    out="$(spacetime sql "$DB" "SELECT map_id FROM $t" 2>&1)" || { echo "!$t"; continue; }
    printf '%s\n' "$out"
  done | grep -oE '^ *[0-9]+ *$|^!.+' | tr -d ' ' | sort -u
}
# Second, independent fail-safe signal: `init` seeds AT MOST 3 `game_creature_spawn`
# rows (chicken/wolf/trainer) and AT MOST 5 `game_gameobject` rows (chest/goober/2 standalone gather
# nodes/1 armed tier-pool point) into EVERY freshly published database — module/src/seed.rs. Terrain/nav
# alone is not sufficient: `./target/debug/lyracore-importer --family creatures` (or `gameobjects`) run DIRECTLY —
# a supported way to reload one family into a dev node, which bypasses this script's terrain/nav ETL
# entirely — loads real spatial rows while leaving game_terrain_chunk/game_nav_chunk
# permanently empty. A row count over the fixture ceiling in EITHER table means real content landed here
# by SOME path, terrain/nav probe notwithstanding, so `db_never_imported` below ANDs this in too. Prints a
# bare integer, or nothing (→ caller fails safe) if the count can't be read or parsed.
INIT_MAX_CREATURE_FIXTURES=3
INIT_MAX_GAMEOBJECT_FIXTURES=5
db_row_count() { # table
  out="$(spacetime sql "$DB" "SELECT COUNT(*) AS n FROM $1" 2>&1)" || return 1
  printf '%s\n' "$out" | grep -oE '^ *[0-9]+ *$' | tr -d ' ' | tail -1
}
# True iff $DB has NEVER received a real import. An unreadable terrain/nav table, or an unreadable/
# unparseable row count, returns FALSE (not fresh) — the same fail-safe direction as the WARNING above:
# when this can't be determined, fall through to the strict foreign-map check rather than silently
# waving the run through.
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

# ---- PREFLIGHT: one continent per database ------------------------------------------------------
# The family clears are WHOLESALE, so this run does not ADD to whatever $DB already holds — it REPLACES
# it. Pointed at the wrong database that silently swaps a live continent for another one, which is
# exactly the failure a shard split invites. Refuse; ALLOW_MAP_SWITCH=1 is the deliberate override
# (re-importing the same continent is not a switch and never trips this).
PRE_PROBE="$(db_spatial_probe)"
PRE_UNREADABLE="$(probe_unreadable "$PRE_PROBE")"
if [ -n "${PRE_UNREADABLE// /}" ]; then
  # Loud, but NOT an abort: a guard that refuses a legitimate import is worse than one that does not
  # fire, and `spacetime sql`'s exit status is not something this script can pin. The hard gate is the
  # post-run re-assert at the bottom, which fails the run outright on the same condition (and cannot be
  # satisfied by an empty answer — it demands to SEE this run's own map).
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
  echo "          spacetime call \"$DB\" claim_operator"
  echo "        Deliberately overwriting this database anyway: re-run with ALLOW_MAP_SWITCH=1."
  [ "${ALLOW_MAP_SWITCH:-0}" = 1 ] || exit 1
  echo "[world] ALLOW_MAP_SWITCH=1 — overwriting '$DB' with map(s) $(echo "$EXPECTED_MAPS" | tr '\n' ' ') anyway"
fi

cargo build -q --bin lyracore-importer || exit 1
# --include-map 36: the Deadmines interior rides the SAME single clear+reload run as
# the Elwynn+Westfall box — a WHOLE extra map keyed by id, no geometry (an instance map has no --box
# worth drawing). Its creatures/GOs/waypoints/loot/quest relations all flow through the same families.
# NO terrain for map 36 — deliberately. An instance interior is WMO geometry, not ADT floors, so there
# is no heightmap to sample: the decided design is ground_z None plus the spawn Zs verbatim from the
# dump, and `terrain::map_dir` hard-refuses map 36 to keep anyone from trying.
echo "[world] ETL  --map $MAP  --box $BOX  include-maps [${INCLUDE_MAPS:-none}]  include-creatures [${INCLUDE_CREATURES:-none}]  → $DB"
./target/debug/lyracore-importer --db "$DB" --dump "$DUMP" --dbc "$DBC" --map "$MAP" --box "$BOX" \
  "${INC_MAP_FLAGS[@]}" "${INC_CREATURE_FLAGS[@]}" "${CENTER_FLAGS[@]}" --apply 2>&1 \
  | grep -iE "filter:|mapped|applied|error|abort|WARNING" | tail -12
# Terrain heightmap ETL: --terrain honors --box (terrain.rs::slice_cell_range),
# so this covers the SAME widened Elwynn+Westfall rectangle as the creature ETL above — continuous
# ground-z across the zone border. --center stays the Northshire default (inside the box), which
# feeds the importer's own interpolate self-check (hard-fails on an axis-mapping regression, not a
# silent hole); it does not narrow the box. Idempotent clear+reload, like the other passes.
#
# MAP 1 (Kalimdor) TERRAIN + NAV WORK — verified, not assumed: `terrain::map_dir` maps 1 → "Kalimdor"
# (the same two-arm continent match that maps 0 → "Azeroth"), so both passes read
# `World\Maps\Kalimdor\Kalimdor_<a>_<b>.adt` out of the operator's own terrain.MPQ through the same
# code path, with the same filename-axis arbitration and the same self-checks. This is NOT the map-36
# situation: 36 is refused outright because a WMO instance has no ADT floors at all. The one extra
# operator obligation on another continent is $CENTER (above) — the self-checks sample it.
echo "[world] terrain heightmap ETL  --map $MAP  --box $BOX${CENTER:+  --center $CENTER}"
./target/debug/lyracore-importer --db "$DB" --terrain "$DBC" --map "$MAP" --box "$BOX" "${CENTER_FLAGS[@]}" --apply 2>&1 \
  | grep -iE "terrain:|self-check|error|abort" | tail -8
# Nav grid ETL: WMO/M2 collision + slope rasterized into
# game_nav_chunk (walkability + LoS obstruction). Same box; --center feeds its own walkable
# self-check, and the rotation-convention calibration hard-fails on transform drift. M2 parse
# warnings (a handful of decorative props) are expected; the grep keeps summary lines only.
echo "[world] nav grid ETL  --map $MAP  --box $BOX${CENTER:+  --center $CENTER}"
./target/debug/lyracore-importer --db "$DB" --nav "$DBC" --map "$MAP" --box "$BOX" "${CENTER_FLAGS[@]}" --apply 2>&1 \
  | grep -iE "nav: [0-9]|calibration|self-check|applied|error|abort" | tail -8
# Character-creation + faction DBC tables. The combined --dump --dbc pass above uses the DBCs ONLY for
# Scale==0 resolution; dbc::run() — the SOLE loader of game_start_item / game_char_base_info /
# game_race_info / game_faction[_template] — is gated on --dump being ABSENT, so a --dump pass SKIPS it.
# Run --dbc ALONE here so a fresh reprovision + this script populates those tables; otherwise non-Warrior
# classes fall back to the Warrior loadout, Human Female renders Male, and create_character skips
# race/class validation (each is an idempotent clear+reload, independent of the cmangos creature slice).
echo "[world] character-creation + faction DBC tables (start items / race+class combos / race display / factions)"
./target/debug/lyracore-importer --db "$DB" --dbc "$DBC" --apply 2>&1 \
  | grep -iE "dbc: loaded|error|abort" | tail -6
# Real talent trees (TalentTab.dbc + Talent.dbc → game_talent_tab/game_talent).
# WITHOUT this the tables hold only the 8 demo talents, whose ids don't match what the 5875 client
# sends on a talent click → talent selection silently fails (the client renders the real trees from its
# OWN Talent.dbc). This clear+reloads 27 tabs + 432 talents so the client's real talent ids resolve.
echo "[world] real talent trees (TalentTab.dbc + Talent.dbc → game_talent)"
./target/debug/lyracore-importer --db "$DB" --dbc "$DBC" --talents --apply 2>&1 \
  | grep -iE "loaded [0-9]+ tabs|error|abort" | tail -2
echo "[world] full Spell.dbc import (every non-zero row)"
# Full de-curated import: EVERY non-zero Spell.dbc row (header+effects), no --only allowlist. The
# long tail (raid/other-class/PvP spells with no Rust kind mapping yet) lands as E_SCRIPTED no-ops —
# that's the intended end state, NOT a gap; mapping the long tail of spell effects to typed kinds is
# separate, later work. The
# wholesale clear in spell_delete_statements (importer/src/spell.rs) is guarded at
# SYNTHETIC_SPELL_ID_FLOOR (50000) so the module's synthetic/test fixture spells (Combat Insight
# 50000, Test PW:Shield 50072, Minor Healing 50110, Test Regeneration 50137 — none re-created by any
# import script) survive this reload. The curated class import + caster --only import below still run
# AFTER this and re-apply their own rows — this full import SUBSUMES their spell rows (byte-identical,
# same kind-resolution code path) but they're kept for their `--trainer` offering bindings.
./target/debug/lyracore-importer --db "$DB" --dbc "$DBC" --spells --apply 2>&1 \
  | grep -iE "imported [0-9]|WARNING|error|abort" | tail -3
# NOTE the explicit `DB=` prefix, and do not drop it: `DB` is a plain shell variable here (not
# exported) and the child script defaults to `lyracore`, so without the prefix a
# `DB=<other> import-world.sh` run writes its curated spells into the DEFAULT database instead of the
# target one — silently, on a shard that otherwise imported correctly.
echo "[world] curated spell-import (functional 1-20 trainer offerings + castable ranks) → $DB"
DB="$DB" importer/scripts/import-class-spells.sh "$DBC" 2>&1 | grep -iE "imported [0-9]|error" | tail -1
# Caster-mob spell-import (Elwynn+Westfall 1-20): the spells the in-box caster mobs reference,
# additively imported into game_spell/game_spell_effect via the --only allowlist (preserves the
# curated class kit + test fixtures). game_creature_cast + game_creature_spell (populated by the ETL
# above) point at these; an un-imported id would silently no-op. SCOPE: only the canonical-box
# casters — caster mobs OUTSIDE Elwynn+Westfall are DEFERRED (widen this allowlist + import their
# zone box later). spell2+ rotation rows (e.g. Frost Armor 12544 as a self-buff on entries 474/476)
# are now populated by the ETL above; all referenced spell2+ spell IDs must be
# present in this --only list.
#
# EXTEND-HERE (Westfall Defias casters): these ids were never read off a real dump, and a FABRICATED
# spell id is the dangerous kind of wrong — it can silently resolve to some unrelated REAL spell in
# Spell.dbc instead of failing loudly. So none was added to the --only list below sight-unseen; this
# list is placeholder-free on purpose. Instead, on YOUR dump:
# for each Defias caster creature_template entry below, read its creature_template_spells.spell1 (+
# any spell2-4 rotation ids) and append them to the --only list below, mirroring how 474/476 (Elwynn
# casters) already ride it.
#   589?  [V] Defias Pillager — CONFIRM this entry id against your dump before trusting it
#   449?  [V] Defias Conjurer — CONFIRM this entry id against your dump before trusting it
# The net: `spell.rs`'s "--only requested N ids but matched M in Spell.dbc" WARNING (a wrong/typo'd
# id) plus this script's "Defias caster cast row" assertion below (an entry with no cast row at all)
# both fail loudly rather than silently shipping a mute caster.
echo "[world] caster-mob spell-import (Elwynn+Westfall 1-20)"
./target/debug/lyracore-importer --db "$DB" --dbc "$DBC" --spells --apply \
  --only "53,133,143,1776,145,744,745,3149,3150,3238,3248,5416,5708,6016,6268,6524,6660,6730,7159,7357,8014,8260,8646,8873,9080,10101,10277,12023,12024,12170,12544,13322,13342,13375,13443,14030,15572,15652,15657,15661,16144,16244,20712,20714,20720,20746,20793,20808,23114,23260,23504,28265" \
  2>&1 | grep -iE "imported [0-9]|WARNING|error" | tail -1
echo "[world] repair (re-arm creature tick + re-seed fixtures/schedules)"
# #378: debug_rearm_creature_tick was collapsed into debug_repair_after_publish, along with the
# other 16 debug_seed_*/debug_ensure_*/debug_rearm_* reducers — this now also re-seeds the fixture
# families the ETL truncates/leaves untouched (this precedent is why debug_seed_scenario_fixtures
# used to bolt extra seeders onto itself, per #378's history). Preconditions above (this database is
# already published + claimed) satisfy debug_repair_after_publish's `require_operator` gate.
spacetime call "$DB" debug_repair_after_publish >/dev/null 2>&1
# ARM the synthesized gather pools → max_active live game_gameobject rows per pool (init does NOT re-run
# on an auto-migrate publish, and the importer writes pool members via SQL but no live rows). Idempotent.
# This call is UNCONDITIONAL (every MAP) on purpose — `arm_pool` itself map-fences each
# candidate against `game_terrain_chunk`/`game_nav_chunk` (module/src/gameobject.rs's
# `imported_terrain_maps`), so a map-1 run can no longer resurrect `init`'s map-0 Northshire tier pool;
# it simply arms 0 rows for a pool none of whose members' map this database actually hosts.
echo "[world] arm gather pools"
spacetime call "$DB" arm_all_pools >/dev/null 2>&1

# ---- ASSERT the 1-20 blockers landed (a box/ETL regression must fail here, not in play) ----
fail=0
chk() { # label  expected-min  actual
  if [ "${3:-0}" -lt "$2" ]; then echo "  FAIL  $1: got ${3:-0}, want >= $2"; fail=1; else echo "  ok    $1: $3"; fi
}
# MAP-FENCED assertions. Most floors below are box-dependent COUNTS and stay live for any
# continent (the manifest re-points them for MAP!=0); these two wrappers fence the checks that name a
# SPECIFIC Elwynn/Westfall or Deadmines fixture — a Goldshire trainer entry, Farley, Mr. Smite's hammer.
# Those cannot pass on another continent and must not be reported as failures there: a map-1 run drowning
# in ~40 FAILs about absent Elwynn content is noise, and noise is how a real regression gets missed.
#   chk0  — only on the map-0 corridor run
#   chk36 — only when the Deadmines interior is actually in this run's --include-map set
# The wrappers gate the COMPARISON, not the query — a fenced-out check still runs its
# `spacetime sql` (arguments evaluate before the call). A handful of extra SELECTs returning zero rows
# is cheaper than restructuring 200 lines of interleaved assertions into per-map blocks.
# Named Elwynn/Westfall fixtures (Goldshire trainers, Farley, Sentinel Hill, the Elwynn casters):
# only meaningful on the canonical corridor. A narrower map-0 box is a slice and must not report them
# as failures — that wall of FAILs is exactly what this wrapper was invented to prevent for map 1.
# `${SLICE:-0}` deliberately, not `$SLICE`: import-manifest-smoke.sh pins this line's semantics by
# grepping the definition out of this file and running it under `set -u` with only MAP/INCLUDE_MAPS
# set, so a bare $SLICE aborts that whole check. Unset means the canonical corridor, same default the
# manifest uses.
chk0() { [ "$MAP" = 0 ] && [ "${SLICE:-0}" = 0 ] && chk "$@"; return 0; }
chk36() { case " ${INCLUDE_MAPS//,/ } " in *" 36 "*) chk "$@" ;; esac; return 0; }
n() { spacetime sql "$DB" "$1" 2>&1 | grep -cE "${2:-^ *[0-9]}"; }
echo "[world] assertions (map $MAP → $DB):"
[ "$SLICE" = 0 ] || echo "  ..    map-0 corridor fixture checks (Goldshire trainers / Farley / Sentinel Hill / Elwynn caster cast+rotation rows / Deadmines) SKIPPED — not this continent's content"
chk0 "Goldshire anchor trainers spawned {328,377,906,913,917,927} (NOT total coverage — see the service-coverage audit below)" "$FLOOR_CLASS_TRAINERS" \
    "$(n "SELECT guid FROM game_world_entity WHERE owner_guid=0 AND (entry=328 OR entry=377 OR entry=906 OR entry=913 OR entry=917 OR entry=927)" '[0-9]{6,}')"
chk "class trainer offerings (learn_skill_line=0)"   "$FLOOR_TRAINER_OFFERINGS_CLASS" "$(n "SELECT spell_id FROM game_trainer_spell WHERE learn_skill_line = 0")"
chk "profession learn offerings (learn_skill_line>0)" "$FLOOR_TRAINER_OFFERINGS_PROFESSION" "$(n "SELECT spell_id FROM game_trainer_spell WHERE learn_skill_line > 0")"
chk "gather-node GO spawns"                          "$FLOOR_GATHER_NODE_GO" "$(n "SELECT guid FROM game_gameobject" '[0-9]')"
# [V] tune after the first real Westfall import: Elwynn-only imported ~2500 chunks; the widened box
# must land well past that. A terrain::run failure (axis self-check bail, no-cells-in-slice) must
# fail HERE, not surface as missing ground-z in play (the review's silent-terrain-skip gap).
chk "terrain chunks (ground-z heightmap)"           "$FLOOR_TERRAIN_CHUNKS" "$(n "SELECT key FROM game_terrain_chunk" '[0-9]')"
chk "nav chunks (walkability/LoS grid)"             "$FLOOR_NAV_CHUNKS" "$(n "SELECT key FROM game_nav_chunk" '[0-9]')"
# Gather nodes spawn FLAT (in-place respawn) — vanilla Elwynn is not spatially pooled. Gather pools are
# armed by the arm_all_pools call above; they're not part of the ETL, so no pool-count assertion here.
# (The old pool-2 tier-variety demonstrator rode a hand-written profession seed file, retired along
# with the rest of the synthetic profession data; professions import from the real npc_trainer + DBC.)
chk0 "innkeeper Farley(295) spawned"      "$FLOOR_INNKEEPER_FARLEY" "$(n "SELECT guid FROM game_world_entity WHERE entry=295 AND owner_guid=0" '[0-9]{6,}')"
# The continent's OWN spatial rows: spawn rows carrying THIS run's primary map id. Every
# other count check would pass on a shard whose spawns all landed under the wrong map — this is the one
# that says "the Kalimdor content is on the Kalimdor shard, as map 1".
chk "spawn rows on this run's map ($MAP)" "$FLOOR_CONTINENT_SPAWNS" "$(n "SELECT guid FROM game_creature_spawn WHERE map_id = $MAP" '[0-9]')"
# [V] tune-to-dump: raised 300→450 — the widened Westfall box adds Sentinel Hill's quest hubs + the
# Deadmines chain's overworld half on top of the Elwynn 1-10 relations. 450 is an unverified estimate
# — tighten or widen it against your real dump's count.
chk "quest-giver relations"              "$FLOOR_QUEST_GIVER_RELATIONS" "$(n "SELECT quest_entry FROM game_creature_quest")"
chk "L6-10 quests (quest_level band)"    "$FLOOR_QUESTS_L6_10" "$(spacetime sql "$DB" "SELECT quest_level FROM game_quest_template" 2>&1 | grep -oE '^ *[0-9]+ *$' | tr -d ' ' | awk '$1>=6 && $1<=10' | wc -l)"
# ≥1 Westfall-band quest present (quest_level 10-20) — no entry id needed, just the level band; a
# widened box with zero quests in this band means Westfall's quest_template rows never landed.
chk "L10-20 quests (Westfall quest_level band)" "$FLOOR_QUESTS_L10_20" "$(spacetime sql "$DB" "SELECT quest_level FROM game_quest_template" 2>&1 | grep -oE '^ *[0-9]+ *$' | tr -d ' ' | awk '$1>=10 && $1<=20' | wc -l)"
# Chained quests: McBride's 783->7->15->21->54 chain should populate next_quest_id symmetrically to
# the already-working prev_quest_id side. A single-column inequality filter in SQL is itself the
# "can return 0 rows wrongly" trap (docs/danger-zones.md §2) — select the raw column and filter in the
# shell instead, like the quest_level bands above.
chk "chained quests (next_quest_id>0) [V]" "$FLOOR_QUESTS_CHAINED" "$(spacetime sql "$DB" "SELECT next_quest_id FROM game_quest_template" 2>&1 | grep -oE '^ *[0-9]+ *$' | tr -d ' ' | awk '$1>0' | wc -l)"
# Timed quests (limit_time>0): coverage-printed only, no floor — see FLOOR_QUESTS_CHAINED's comment in
# import-manifest.sh for why (plausibly zero in this Elwynn/Westfall slice).
echo "  ..  timed quests (limit_time>0) [V]: $(spacetime sql "$DB" "SELECT limit_time FROM game_quest_template" 2>&1 | grep -oE '^ *[0-9]+ *$' | tr -d ' ' | awk '$1>0' | wc -l)"
# [V] tune-to-dump: raised 1500→2500 — Westfall's own creature density added to Elwynn's. 2500 is an
# unverified estimate — tighten or widen it against your real dump's count.
chk "live creature entities"           "$FLOOR_LIVE_CREATURES" "$(n "SELECT guid FROM game_world_entity WHERE entry>0 AND owner_guid=0" '[0-9]{6,}')"
# [V] tune-to-dump: raised 200→280 — Sentinel Hill + Westfall's other vendors added. 280 is an
# unverified estimate — tighten or widen it against your real dump's count.
chk "vendors (npc_vendor)"              "$FLOOR_VENDORS" "$(n "SELECT item_entry FROM game_npc_vendor")"
# --- Service coverage: an NPC that ADVERTISES a service via npc_flags must actually PROVIDE it.
# This audit exists because the row-count assertions above once certified an import "OK" while every
# spawned Northshire class trainer taught NOTHING — it joins spawned+flagged templates against the provider tables
# shell-side (spacetime sql has no JOIN) and names the dead NPCs. Floors guard non-regression on
# the COVERED counts; the DEAD lists are the honest gap ledger (fix = give them rows, not silence).
echo "[world] service coverage (flagged NPCs that actually provide their service):"
COV=$(python3 - "$DB" <<'PYEOF'
import subprocess, sys
db = sys.argv[1]
def rows(q):
    out = subprocess.run(["spacetime","sql",db,q],capture_output=True,text=True).stdout
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

# [V] — set this to the real Sentinel Hill trainer/vendor creature_template.entry from YOUR dump if
# the default below does not match it. An entry that matches nothing never matches a live spawn, so
# this check FAILS LOUD rather than passing quietly — that's deliberate: a guessed wrong id could
# silently "pass" on an unrelated creature, which is worse than an honest, visible FAIL demanding the
# real value.
# Find it via: grep the dump's creature_template INSERT for a name near Sentinel Hill (Westfall,
# around x≈-10650,y≈1180 — see module/src/world.rs SENTINEL_HILL) with npc_flags & (0x4|0x10) set
# (UNIT_NPC_FLAG_VENDOR|TRAINER).
SENTINEL_HILL_VENDOR_ENTRY="${SENTINEL_HILL_VENDOR_ENTRY:-491}"  # Quartermaster Lewis — verified on a real import: spawned, with 15 vendor rows
chk0 "Sentinel Hill vendor spawned (Quartermaster Lewis 491)" "$FLOOR_SENTINEL_HILL_VENDOR" \
    "$(n "SELECT guid FROM game_world_entity WHERE owner_guid=0 AND entry=${SENTINEL_HILL_VENDOR_ENTRY}" '[0-9]{6,}')"
# All 6 Human classes have a creation loadout (race_class = (race<<8)|class: Warrior 257, Paladin 258,
# Rogue 260, Priest 261, Mage 264, Warlock 265). Asserts the standalone --dbc pass above ran — a --dump-only
# run leaves game_start_item empty and non-Warrior classes silently inherit the Warrior kit. Count DISTINCT
# present race_class values (the WHERE caps it at 6) — want all 6.
chk "Human start-items (all 6 classes)"   "$FLOOR_START_ITEMS_CLASSES" \
    "$(spacetime sql "$DB" "SELECT race_class FROM game_start_item WHERE race_class=257 OR race_class=258 OR race_class=260 OR race_class=261 OR race_class=264 OR race_class=265" 2>&1 | grep -oE '^ *[0-9]+ *$' | tr -d ' ' | sort -u | wc -l)"
# [V] tune-to-dump: raised 40→60 — Westfall's Defias casters (Pillager/Conjurer, see the EXTEND-HERE
# block above) add to Elwynn's caster count. 60 is an unverified estimate — tighten or widen it
# against your real dump's count.
chk "caster-mob cast rows"               "$FLOOR_CASTER_CAST_ROWS" "$(n "SELECT creature_entry FROM game_creature_cast")"
chk "total game_spell rows (full Spell.dbc import)" "$FLOOR_SPELL_TOTAL" "$(n "SELECT spell_id FROM game_spell")"
# Rank-chain + auto-learn dependent tables, populated by the same --dump pass above (family
# "spellmeta" — no --family flag needed here, a plain --dump run activates every family). A 0 here
# means the spell_chain/spell_learn_spell column indices ([V] importer/src/main.rs `sc`/`sls`) drifted
# against your dump, or those tables are absent from it.
chk "spell chain rows (game_spell_chain) [V]"            "$FLOOR_SPELL_CHAIN" "$(n "SELECT spell_id FROM game_spell_chain")"
chk "spell auto-learn dependents (game_spell_learn) [V]" "$FLOOR_SPELL_LEARN" "$(n "SELECT id FROM game_spell_learn")"
# Map fencing, drawn precisely: a `game_spell` PRESENCE check is map-INDEPENDENT (the full
# Spell.dbc import + the curated `--only` passes run on every shard), so 133/13322/3110 stay live on any
# continent — free coverage. What IS map-0 content is the CREATURE-derived side: a `game_creature_cast`
# or `game_creature_spell` row exists only because an Elwynn/Westfall creature entered THIS slice, so
# those are `chk0`-fenced (the Imp 416 pet template included — it rides the map-0 corridor import).
chk "476 Geomancer cast spell present"    "$FLOOR_GEOMANCER_CAST_SPELL" "$(n "SELECT spell_id FROM game_spell WHERE spell_id=133")"
chk "474 Rogue Wizard cast spell present" "$FLOOR_ROGUE_WIZARD_CAST_SPELL" "$(n "SELECT spell_id FROM game_spell WHERE spell_id=13322")"
chk0 "Imp(416) Firebolt cast row"         "$FLOOR_IMP_FIREBOLT_CAST_ROW" "$(n "SELECT spell_id FROM game_creature_cast WHERE creature_entry=416")"
chk "Imp Firebolt(3110) in game_spell"    "$FLOOR_IMP_FIREBOLT_SPELL" "$(n "SELECT spell_id FROM game_spell WHERE spell_id=3110")"
# ≥1 Defias caster cast row — same [V] entries as the EXTEND-HERE block above (Pillager 589? /
# Conjurer 449?): this only proves the ETL saw a creature_template_spells row for one of them (the
# main --dump pass populates game_creature_cast independent of the caster --only spell list above),
# NOT that the referenced spell header is imported — confirm both ids against your dump.
chk0 "Defias caster cast row (Pillager 589?/Conjurer 449? [V])" "$FLOOR_DEFIAS_CASTER_CAST_ROW" \
    "$(n "SELECT creature_entry FROM game_creature_cast WHERE creature_entry=589 OR creature_entry=449")"
# Rotation table populated (rank 20): multi-spell in-box casters now have entries in game_creature_spell.
# 474/476 each carry 2 rows (spell1 ALWAYS + spell2 SELF_MISSING_AURA); 10 entries × ≥2 rows = ≥20.
chk0 "rotation rows (game_creature_spell)" "$FLOOR_ROTATION_ROWS" "$(n "SELECT id FROM game_creature_spell")"
chk0 "476 Geomancer rotation nuke row"     "$FLOOR_GEOMANCER_ROTATION_NUKE" "$(n "SELECT id FROM game_creature_spell WHERE creature_entry=476 AND condition=0")"
chk0 "474/476 Frost Armor rotation row"    "$FLOOR_FROST_ARMOR_ROTATION" "$(n "SELECT id FROM game_creature_spell WHERE spell_id=12544")"

# --- AreaTable/AreaTrigger/WorldSafeLocs import counts ------------------------------------------
# [V] tune-to-client: these three land from the standalone --dbc pass above (dbc.rs::run), NOT the
# --dump pass, so they're populated regardless of --box. The thresholds are deliberately conservative
# ballparks — vanilla 1.12's real AreaTable/AreaTrigger/WorldSafeLocs DBCs run to the thousands and
# hundreds — so tighten them against your own client's real counts.
chk "areas imported (game_area) [V]"                 "$FLOOR_AREAS" "$(n "SELECT id FROM game_area")"
chk "area triggers imported (game_area_trigger) [V]"  "$FLOOR_AREA_TRIGGERS" "$(n "SELECT id FROM game_area_trigger")"
chk "graveyards imported (game_graveyard) [V]"         "$FLOOR_GRAVEYARDS" "$(n "SELECT id FROM game_graveyard")"
# Zone graveyard_zone → game_graveyard "join" check: `spacetime sql` has no JOIN, so this is done
# shell-side (fetch each table separately, intersect in bash) — at least one link for $GRAVEYARD_ZONE
# must resolve to a real game_graveyard row, proving `graveyard::resolve_graveyard`'s zone-linked path
# actually has something to resolve in the zone players are levelling in, not just an orphaned
# link. The zone id is a PARAMETER: 12 = Elwynn on the map-0 corridor; on another continent
# set it to a zone in YOUR box — e.g. Durotar 14, Mulgore 215, Teldrassil 141 ([V] AreaTable ids, read
# yours from the imported game_area rather than trusting these). Unset on a non-map-0 run = skipped, not
# failed: an unset zone would search zone 0 and report a bogus FAIL.
if [ -n "$GRAVEYARD_ZONE" ]; then
  echo "[world] zone-$GRAVEYARD_ZONE graveyard_zone→game_graveyard resolve check (shell-side, no SQL JOIN):"
  zone_ids="$(spacetime sql "$DB" "SELECT safe_loc_id FROM game_graveyard_zone WHERE zone_id = $GRAVEYARD_ZONE" 2>&1 | grep -oE '^ *[0-9]+ *$' | tr -d ' ')"
  zone_resolved=0
  for id in $zone_ids; do
    hit="$(spacetime sql "$DB" "SELECT id FROM game_graveyard WHERE id = $id" 2>&1 | grep -cE '^ *'"$id"' *$')"
    if [ "${hit:-0}" -gt 0 ]; then zone_resolved=$((zone_resolved + 1)); fi
  done
  chk "zone-$GRAVEYARD_ZONE graveyard_zone links resolve to a real game_graveyard row" "$FLOOR_GRAVEYARD_ZONE_RESOLVE" "$zone_resolved"
else
  echo "  ..    graveyard_zone resolve check SKIPPED — set GRAVEYARD_ZONE=<a zone id inside your box> to enable it"
fi

# --- areatrigger_teleport (dungeon entrance/exit portals) import counts --------------------------
# Unlike the DBC-sourced trio above, this lands from the --dump pass's "globals" family (NOT --box
# scoped — a world-wide portal table, see build_areatrigger_teleport_sql's doc comment), so it's
# populated by the SAME --dump --dbc invocation at the top of this script. `[V]` floor + ids: neither
# the column layout nor the Deadmines trigger ids has been confirmed against a second dump — tighten,
# widen or correct them against your own.
chk "areatrigger_teleport portals imported (game_areatrigger_teleport) [V]" "$FLOOR_AREATRIGGER_TELEPORTS" \
    "$(n "SELECT trigger_id FROM game_areatrigger_teleport")"
# Both directions of the Deadmines portal must import — the Moonbrook ENTRANCE (map 0 -> map 36) and
# the interior EXIT (map 36 -> map 0) are two separate areatrigger_teleport rows; a floor of 1 would
# pass vacuously on only one direction importing (walk in but can never walk back out). Both trigger
# ids are `[V]` — CONFIRM them against your own dump.
chk "Deadmines portal round-trip: entrance(78)+exit(119) both present" "$FLOOR_DEADMINES_PORTAL_ROUNDTRIP" \
    "$(n "SELECT trigger_id FROM game_areatrigger_teleport WHERE trigger_id=78 OR trigger_id=119")"

# --- the Deadmines (map 36) content slice — from --include-map 36 above --------------------------
# All floors/ids here are [V] — see each key's comment in import-manifest.sh for what to confirm.
# Spawn/GO counts read the IMPORT tables (game_creature_spawn/game_gameobject), not live entities —
# they're populated synchronously by the reducer batches, independent of the creature-tick timing.
chk36 "Deadmines creature spawns (map 36) [V]" "$FLOOR_DEADMINES_CREATURE_SPAWNS" \
    "$(n "SELECT guid FROM game_creature_spawn WHERE map_id = 36" '[0-9]')"
chk36 "Deadmines gameobjects (doors/cannon/chests, map 36) [V]" "$FLOOR_DEADMINES_GAMEOBJECTS" \
    "$(n "SELECT guid FROM game_gameobject WHERE map_id = 36" '[0-9]')"
# Placed bosses spawned, DISTINCT entries — floor 6 (Rhahk'Zor 644 / Shredder 642 / Gilnid 1763 /
# Smite 646 / Greenskin 647 / VanCleef 639); Sneed 643 stays in the query but NOT the floor: he is
# script-summoned on the Shredder's death in cmangos-era dumps (so a correct import has no placed row
# for him), and a dump that does place him reads 7 and still passes. A wrong [V] entry id FAILS
# LOUD here; correct the id against your dump (the SENTINEL_HILL_VENDOR precedent), don't loosen.
chk36 "Deadmines bosses spawned (>=6 distinct placed entries) [V]" "$FLOOR_DEADMINES_BOSSES" \
    "$(spacetime sql "$DB" "SELECT entry FROM game_creature_spawn WHERE map_id = 36 AND (entry=644 OR entry=643 OR entry=642 OR entry=1763 OR entry=646 OR entry=647 OR entry=639)" 2>&1 | grep -oE '^ *[0-9]+ *$' | tr -d ' ' | sort -u | wc -l)"
# BOTH named drops in the loot pools: Cruel Barb ~5191 (VanCleef) + Smite's Mighty
# Hammer ~5196 (Smite). DISTINCT item ids so two rows of one item can't pass vacuously.
chk36 "Deadmines named drops (Cruel Barb ~5191 + Smite's Mighty Hammer ~5196) [V]" "$FLOOR_DEADMINES_NAMED_LOOT" \
    "$(spacetime sql "$DB" "SELECT item_entry FROM game_creature_loot WHERE item_entry=5191 OR item_entry=5196" 2>&1 | grep -oE '^ *[0-9]+ *$' | tr -d ' ' | sort -u | wc -l)"
# The Defias Brotherhood chain (the next_quest_id machinery, ending on VanCleef's head) is covered by
# the FLOOR_QUESTS_CHAINED check above. Its in-instance kill/collect credit is NOT asserted here —
# that needs a character actually inside the instance, so it is a live check, not an import one.
# Quest RELATIONS for map-36 creatures import via the same entries set as the box slice, so no
# separate floor is needed here either.

# --- loot-family completeness (pickpocket/skinning/gameobject-chest/fishing) ---------------------
# [V] every floor below is an UNVERIFIED presence floor — tighten them against your own dump once you
# have real counts. A floor of 1 just proves the family imported AT ALL: a parse or scope regression
# that silently drops a whole family must fail HERE, not surface as an empty loot window in play.
chk "pickpocket loot rows (game_pickpocket_loot) [V]"   "$FLOOR_PICKPOCKET_LOOT" "$(n "SELECT id FROM game_pickpocket_loot")"
chk "skinning loot rows (game_skinning_loot) [V]"        "$FLOOR_SKINNING_LOOT" "$(n "SELECT id FROM game_skinning_loot")"
# The two curated TREASURE_CHESTS (Battered Chest lootId 2277, Solid Chest lootId 9932) both carry
# real rows in every known cmangos classic-db dump — a 0 here means the CHEST data1 scoping (or the
# gameobject_loot_template parse) regressed, not that the dump lacks rows.
chk "gameobject (chest) loot rows (game_gameobject_loot) [V]" "$FLOOR_GAMEOBJECT_CHEST_LOOT" "$(n "SELECT id FROM game_gameobject_loot")"
chk "fishing loot rows (game_fishing_loot) [V]"          "$FLOOR_FISHING_LOOT" "$(n "SELECT id FROM game_fishing_loot")"
# Creature-template loot ids actually resolved (PickpocketLootId/SkinLootId nonzero) — proves the [V]
# ct::PICKPOCKET_LOOT_ID/ct::SKIN_LOOT_ID column positions actually land on real data, not on a
# neighboring always-zero column (which would otherwise pass the row-count checks above vacuously via
# an unrelated in-slice creature that DOES have a table).
chk "creatures with a pickpocket table (creature_template.pickpocket_loot_id>0) [V]" "$FLOOR_CREATURES_WITH_PICKPOCKET" \
    "$(n "SELECT entry FROM game_creature_template WHERE pickpocket_loot_id > 0")"
chk "creatures with a skin table (creature_template.skin_loot_id>0) [V]" "$FLOOR_CREATURES_WITH_SKIN" \
    "$(n "SELECT entry FROM game_creature_template WHERE skin_loot_id > 0")"

# --- ONE CONTINENT PER DATABASE, re-checked after the run ----------------------------------------
# The preflight refused to START against a database holding another continent; this is the same
# invariant asserted on the RESULT — every map id in the four map-keyed spatial tables must be one this
# run imports. It catches what the preflight cannot: a family whose clear+reload left foreign rows
# behind (a clear that stopped scoping wholesale, a hand-run partial import, a `--family` refresh that
# dropped `--include-map`) — i.e. "neither continent's database carries the other's spatial data"
# stated as a check rather than as a convention.
#
# …and it carries its OWN POSITIVE CONTROL, because "no foreign map ids" is exactly what an unreadable
# probe returns: this run's PRIMARY map must be visible in the same probe's output. After a successful
# import $DB necessarily holds map-$MAP spawn and terrain rows, so an empty answer means the probe read
# nothing at all and its verdict is worthless — not that the invariant holds. Without this, a renamed
# table/column silently downgrades BOTH the preflight and this re-assert to permanent `ok`s.
#
# This re-assert uses the SAME `db_never_imported` fresh-shard exemption as the preflight
# above (so the two can never disagree about what counts as "content"). In practice it's a no-op here:
# a genuinely successful import always leaves game_terrain_chunk/game_nav_chunk non-empty for $MAP, so
# `db_never_imported` is false by the time this runs and the positive control below still catches a
# run whose terrain/nav load silently failed.
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
