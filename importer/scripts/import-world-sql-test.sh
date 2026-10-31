#!/usr/bin/env bash
set -uo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"

passed=0
failed=0
ok() { passed=$((passed + 1)); printf '  ok    %s\n' "$1"; }
no() { failed=$((failed + 1)); printf '  FAIL  %s\n' "$1"; }
check() {
    local desc="$1"
    shift
    if "$@"; then ok "$desc"; else
        no "$desc"
        return 0
    fi
}
contains() { grep -Fq -- "$2" "$1"; }
not_contains() { ! grep -Fq -- "$2" "$1"; }
count_matches() { grep -c -- "$2" "$1" 2>/dev/null || true; }

sandbox="$(mktemp -d "${TMPDIR:-/tmp}/lyracore-import-world-sql-test.XXXXXX")"
trap 'rm -rf "$sandbox"' EXIT

stubs="$sandbox/stubs"
mkdir -p "$stubs"
stub_log="$sandbox/stub.log"
: >"$stub_log"

checkout="$sandbox/checkout"
mkdir -p "$checkout/importer/scripts" "$checkout/target/debug"
cp "$repo_root/importer/scripts/import-world.sh" "$checkout/importer/scripts/import-world.sh"
cp "$repo_root/importer/scripts/import-manifest.sh" "$checkout/importer/scripts/import-manifest.sh"
cp "$repo_root/importer/scripts/vmangos-source.sh" "$checkout/importer/scripts/vmangos-source.sh"
touch "$checkout/source.sqlite"
cp "$repo_root/importer/scripts/import-class-spells.sh" "$checkout/importer/scripts/import-class-spells.sh"
chmod +x "$checkout/importer/scripts/import-world.sh" "$checkout/importer/scripts/import-class-spells.sh"
under_test="$checkout/importer/scripts/import-world.sh"

cat >"$checkout/target/debug/lyracore-importer" <<'STUB'
#!/bin/sh
echo "lyracore-importer $*" >>"$STUB_LOG"
case " $* " in
    *" --print-scope-maps "*)
        [ "${STUB_SCOPE_MODE:-succeed}" = fail ] && exit 1
        for map_id in ${STUB_SCOPE_MAPS:-0}; do echo "$map_id"; done
        exit 0
        ;;
esac
echo "mapped 1 applied 1"
exit 0
STUB
chmod +x "$checkout/target/debug/lyracore-importer"

cat >"$stubs/cargo" <<'STUB'
#!/bin/sh
echo "cargo $*" >>"$STUB_LOG"
exit 0
STUB
chmod +x "$stubs/cargo"

cat >"$stubs/spacetime" <<'STUB'
#!/bin/sh
echo "spacetime $*" >>"$STUB_LOG"
case "${1:-}" in
    sql)
        if [ "${STUB_MODE:-succeed}" = fail ]; then
            echo "Error: failed to find database" >&2
            exit 1
        fi
        query=""
        for a in "$@"; do
            case "$a" in *SELECT*|*select*) query="$a" ;; esac
        done
        case "$query" in
            *"SELECT map_id FROM "*)
                for map_id in ${STUB_MAP_IDS:-0}; do echo " $map_id"; done
                ;;
            *"SELECT quest_entry FROM "*|*"SELECT safe_loc_id FROM "*) echo " 42" ;;
            *map_id*|*"COUNT(*)"*) echo " 0" ;;
            *)
                i=0
                while [ "$i" -lt "${STUB_ROW_COUNT:-20}" ]; do
                    echo " 42"
                    i=$((i + 1))
                done
                ;;
        esac
        exit 0
        ;;
    call)
        [ "${STUB_MODE:-succeed}" = fail ] && exit 1
        exit 0
        ;;
    --version)
        echo "spacetimedb tool version 2.14.0"
        ;;
esac
exit 0
STUB
chmod +x "$stubs/spacetime"

out="$sandbox/out.txt"

run_import() {
    (
        cd "$sandbox" || exit 99
        env -i \
            HOME="$sandbox/home" \
            TMPDIR="$sandbox" \
            PATH="$stubs:/usr/bin:/bin" \
            DUMP="$checkout/source.sqlite" \
            SOURCE_SHA="0123456789012345678901234567890123456789" \
            STUB_LOG="$stub_log" \
            STUB_MODE="${STUB_MODE:-succeed}" \
            STUB_ROW_COUNT="${STUB_ROW_COUNT:-20}" \
            STUB_MAP_IDS="${STUB_MAP_IDS:-0}" \
            STUB_SCOPE_MAPS="${STUB_SCOPE_MAPS:-0}" \
            STUB_SCOPE_MODE="${STUB_SCOPE_MODE:-succeed}" \
            SPACETIME_SERVER="${SPACETIME_SERVER:-}" \
            WORLD_PROFILE="${WORLD_PROFILE:-}" \
            ALLOW_MAP_SWITCH="${ALLOW_MAP_SWITCH:-}" \
            bash "$under_test"
    ) >"$out" 2>&1
    code=$?
}

echo "import-world.sh — spacetime unreachable (#440 repro)"
: >"$stub_log"
STUB_MODE=fail run_import
check "the run exits non-zero" test "$code" -ne 0
abort_lines="$(count_matches "$out" '^\[world\] ABORT —')"
check "exactly one ABORT line is printed" test "${abort_lines:-0}" -eq 1
fail_lines="$(count_matches "$out" '^  FAIL  ')"
check "no per-table FAIL lines accompany it (the #440 wall of ~50)" test "${fail_lines:-0}" -eq 0
check "the abort names the unreachable database" contains "$out" "'lyracore'"
check "the abort names the server it tried" contains "$out" "http://127.0.0.1:3000"
check "the abort explains a failure is not a zero-row result" contains "$out" "not the same as zero rows"
[ "$code" -ne 0 ] || cat "$out"

echo
echo "import-world.sh — spacetime reachable, canned row counts"
: >"$stub_log"
STUB_MODE=succeed STUB_ROW_COUNT=150 run_import
check "the run reaches the assertion stage (no ABORT)" not_contains "$out" '[world] ABORT'
check "a count-based assertion reports the CANNED count, not 0" \
    contains "$out" "gather-node GO spawns: 150"
check "  ...and floors it clears print ok, not FAIL" \
    contains "$out" "ok    gather-node GO spawns: 150"
check "the run reaches its own final verdict line" contains "$out" '[world]'

echo
echo "import-world.sh — every spacetime invocation is server-pinned"
unserved="$(grep -E '^spacetime (sql|call) ' "$stub_log" | grep -vc -- '--server' || true)"
check "no logged spacetime sql/call is missing --server" test "${unserved:-0}" -eq 0
check "the default target is the loopback node dev-up runs, not the CLI's ambient default" \
    grep -q -- '--server http://127.0.0.1:3000' "$stub_log"

echo
echo "import-world.sh — SPACETIME_SERVER is overridable (the by-hand advanced path)"
: >"$stub_log"
STUB_MODE=succeed SPACETIME_SERVER="http://example.invalid:9000" run_import
check "an overridden SPACETIME_SERVER reaches spacetime verbatim" \
    grep -q -- '--server http://example.invalid:9000' "$stub_log"

echo
echo "import-world.sh — canonical profile argument"
: >"$stub_log"
STUB_MODE=succeed STUB_ROW_COUNT=150 WORLD_PROFILE=alliance-single STUB_SCOPE_MAPS="0 1 36 369" run_import
check "the profile run reaches the assertion stage" not_contains "$out" '[world] ABORT'
check "the profile is displayed with all of its planned maps" \
    contains "$out" 'canonical profile alliance-single (planned maps: 0 1 36 369)'
check "the standalone DBC catalogue receives the full taxi mount dump" \
    grep -q -- '--dbc .*--taxi-mounts .*source.sqlite --source-sha 0123456789012345678901234567890123456789 --apply' "$stub_log"
check "the dump ETL receives the canonical profile name" \
    grep -q -- 'lyracore-importer .*--world-profile alliance-single' "$stub_log"
check "the dump ETL reads the client spell catalogue before world AI" \
    grep -q -- 'lyracore-importer .*--dump .*--dbc .*--world-profile alliance-single' "$stub_log"
plain_spell_passes="$(count_matches "$stub_log" '^lyracore-importer .*--spells --apply$')"
check "the profile skips the duplicate standalone spell catalogue pass" test "$plain_spell_passes" -eq 0
check "the dump ETL does not recreate a profile as legacy spatial flags" \
    bash -c '! grep "lyracore-importer .*--world-profile alliance-single" "$0" | grep -qE -- "--map|--box|--include-map|--include-creatures"' "$stub_log"
unserved_importers="$(grep '^lyracore-importer ' "$stub_log" | grep -v -- '--print-scope-maps' | grep -vc -- '--server http://127.0.0.1:3000' || true)"
check "every profile importer invocation names the loopback server" \
    test "${unserved_importers:-0}" -eq 0
: >"$stub_log"
STUB_MODE=succeed STUB_ROW_COUNT=150 WORLD_PROFILE=alliance-eastern STUB_SCOPE_MAPS="0 369" run_import
check "the eastern profile runs its Dun Morogh named check" \
    contains "$out" 'Dun Morogh start correct-map quest-giver spawn'
check "the eastern profile runs its Loch Modan named check" \
    contains "$out" 'Loch Modan follow-on correct-map quest-giver spawn'
: >"$stub_log"
STUB_MODE=succeed STUB_ROW_COUNT=150 WORLD_PROFILE=alliance-kalimdor STUB_SCOPE_MAPS=1 ALLOW_MAP_SWITCH=1 run_import
check "the Kalimdor profile runs its Teldrassil named check" \
    contains "$out" 'Teldrassil start correct-map quest-giver spawn'
check "the Kalimdor profile runs its Darkshore named check" \
    contains "$out" 'Darkshore follow-on correct-map quest-giver spawn'

echo
echo "import-world.sh: full profile ownership and catalogue passes"
for profile in full-world full-eastern full-kalimdor full-instances; do
    case "$profile" in
        full-world) owned_maps="0 1 30 33 34 36 43 47 48 70 90 109 129 189 209 229 230 249 269 289 309 329 349 369 389 409 429 449 450 451 469 489 509 529 531 533" ;;
        full-eastern) owned_maps="0 369 449" ;;
        full-kalimdor) owned_maps="1 450" ;;
        full-instances) owned_maps="30 33 34 36 43 47 48 70 90 109 129 189 209 229 230 249 289 309 329 349 389 409 429 469 489 509 529 531 533" ;;
    esac
    : >"$stub_log"
    STUB_MODE=succeed STUB_ROW_COUNT=150 WORLD_PROFILE="$profile" STUB_MAP_IDS="$owned_maps" STUB_SCOPE_MAPS="$owned_maps" run_import
    check "$profile accepts all of its spatial maps" not_contains "$out" '[world] ABORT'
    check "$profile retains the final map ownership assertion" contains "$out" 'ok    one continent per database: spatial rows only on map(s)'
    check "$profile does not report its own maps as contamination" not_contains "$out" 'cross-continent contamination'
    check "$profile reads its map scope from the source" \
        grep -q -- "lyracore-importer --db .* --dump .*--world-profile $profile --print-scope-maps$" "$stub_log"
    check "$profile passes the name to the dump importer" \
        grep -q -- "lyracore-importer .*--dump .*--world-profile $profile .*--apply" "$stub_log"
    dump_passes="$(grep -v -- '--print-scope-maps' "$stub_log" | grep -c -- "^lyracore-importer .*--dump .*--world-profile $profile ")"
    check "$profile runs exactly one dump and client catalogue pass" test "$dump_passes" -eq 1
    check "$profile reads client catalogues during that dump pass" \
        grep -q -- "lyracore-importer .*--dump .*--dbc .*--world-profile $profile " "$stub_log"
    check "$profile skips the duplicate standalone client catalogue pass" not_contains "$stub_log" '--taxi-mounts'
    plain_spell_passes="$(count_matches "$stub_log" '^lyracore-importer .*--spells --apply$')"
    check "$profile skips the duplicate standalone spell catalogue pass" test "$plain_spell_passes" -eq 0
    check "$profile keeps curated spell and trainer overlays" grep -q -- 'lyracore-importer .*--dbc .*--spells --only .*--trainer ' "$stub_log"
    check "$profile keeps the talent pass" grep -q -- 'lyracore-importer .*--dbc .*--talents --apply' "$stub_log"
    check "$profile does not recreate the scope with legacy flags" \
        bash -c '! grep "lyracore-importer .*--world-profile $1 " "$0" | grep -qE -- "--map |--box |--include-map |--include-creatures "' "$stub_log" "$profile"
    if [ "$profile" = full-instances ]; then
        check "$profile skips continent terrain" not_contains "$stub_log" '--terrain '
        check "$profile skips continent navigation" not_contains "$stub_log" '--nav '
        check "$profile skips open-world corridor assertions" contains "$out" 'open-world corridor checks SKIPPED'
    else
        check "$profile imports terrain with its scope" grep -q -- "lyracore-importer .*--terrain .*--world-profile $profile --apply" "$stub_log"
        check "$profile imports navigation with its scope" grep -q -- "lyracore-importer .*--nav .*--world-profile $profile --apply" "$stub_log"
    fi
done

for profile in full-eastern full-kalimdor full-instances; do
    case "$profile" in
        full-eastern) foreign_map=1 scope_maps="0 369 449" ;;
        full-kalimdor) foreign_map=0 scope_maps="1 450" ;;
        full-instances) foreign_map=1 scope_maps="36" ;;
    esac
    : >"$stub_log"
    STUB_MODE=succeed STUB_ROW_COUNT=150 WORLD_PROFILE="$profile" STUB_MAP_IDS="$foreign_map" \
        STUB_SCOPE_MAPS="$scope_maps" run_import
    check "$profile stops before importing over a foreign map" test "$code" -ne 0
    check "$profile does not call the importer after an ownership failure" not_contains "$stub_log" '--apply'
done

echo
echo "import-world.sh: full-world map scope comes from the source"
full_world_source_maps="0 1 36 269 451"
: >"$stub_log"
STUB_MODE=succeed STUB_ROW_COUNT=150 WORLD_PROFILE=full-world STUB_MAP_IDS="0 1 269 451" \
    STUB_SCOPE_MAPS="$full_world_source_maps" run_import
check "full-world accepts source maps outside the supported list" not_contains "$out" '[world] ABORT'
check "  ...in the final map ownership assertion" \
    contains "$out" 'ok    one continent per database: spatial rows only on map(s) 0 1 269 36 451 '
: >"$stub_log"
STUB_MODE=succeed STUB_ROW_COUNT=150 WORLD_PROFILE=full-world STUB_MAP_IDS="0 1 269 451 13" \
    STUB_SCOPE_MAPS="$full_world_source_maps" ALLOW_MAP_SWITCH=1 run_import
check "full-world rejects a map the source does not select" \
    contains "$out" 'cross-continent contamination: '"'lyracore'"' holds spatial rows for map(s) 13 after importing'
check "  ...and fails the run" test "$code" -ne 0
for scope_mode in fail empty; do
    : >"$stub_log"
    if [ "$scope_mode" = fail ]; then
        STUB_MODE=succeed WORLD_PROFILE=full-world STUB_SCOPE_MODE=fail run_import
    else
        STUB_MODE=succeed WORLD_PROFILE=full-world STUB_SCOPE_MAPS=" " run_import
    fi
    check "a $scope_mode map scope aborts the run" contains "$out" "[world] ABORT — could not read this run's map scope"
    check "  ...before any import is applied" not_contains "$stub_log" '--apply'
done
: >"$stub_log"
STUB_MODE=succeed STUB_ROW_COUNT=150 run_import
check "a legacy run reads its map scope with its slice flags" \
    grep -q -- 'lyracore-importer --db .* --dump .* --map 0 --box -11400,-8000,-3100,2000 --include-map 36 .*--print-scope-maps$' "$stub_log"

echo
echo "import-world.sh — python3 absent from PATH"
no_python="$sandbox/no-python-path"
mkdir -p "$no_python"
for u in bash env grep sed awk sort uniq tr wc cut head tail comm mktemp cat rm mkdir dirname printf true false sh cargo spacetime; do
    p="$(PATH="$stubs:/usr/bin:/bin" command -v "$u" 2>/dev/null)" && ln -sf "$p" "$no_python/$u" 2>/dev/null
done
: >"$stub_log"
(
    cd "$sandbox" || exit 99
    env -i \
        HOME="$sandbox/home" \
        PATH="$no_python" \
        STUB_LOG="$stub_log" \
        STUB_MODE=succeed \
        STUB_ROW_COUNT=20 \
        bash "$under_test"
) >"$out" 2>&1
code=$?
check "missing python3 refuses snapshot validation" test "$code" -ne 0
check "the refusal names the prerequisite" contains "$out" "vmangos source: python3 is required"
check "the refusal does not leak a command-not-found error" not_contains "$out" "python3: command not found"

echo
printf '%d passed, %d failed\n' "$passed" "$failed"
[ "$failed" -eq 0 ]
