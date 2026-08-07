#!/usr/bin/env bash
# import-world-sql-test.sh — OFFLINE behavioural test for import-world.sh's SQL chokepoint (#440).
#
# Run it from the checkout root, any time, with nothing set up:
#   bash importer/scripts/import-world-sql-test.sh     # prints "[test] OK" when it passes
#
# In the style of scripts/install-script-test.sh: everything runs against STUBBED tools on PATH in a
# scratch directory. `spacetime` and `cargo` are shell shims; `target/debug/lyracore-importer` (the
# importer binary import-world.sh execs directly by relative path) is a shim too. No real ETL runs,
# no network, no live node — see that script's own header for why a full import can't be exercised
# here (tens of minutes, needs a real dump/client/node).
#
# WHAT THIS PINS: #440 was three defects that together turned a SUCCESSFUL import into ~50 false
# "table is empty" FAILs — (a) every `spacetime sql`/`spacetime call` queried the CLI's ambient
# default server instead of the node the import actually targets, (b) a FAILED query and a
# LEGITIMATE zero-row result both read as a count of 0, so one dead connection produced a wall of
# fake FAILs instead of one honest error, and (c) an undeclared `python3` dependency crashed the
# service-coverage audit mid-output on a minimal host. This test stubs `spacetime` to fail (proving a
# single "cannot reach" abort, not a FAIL wall), stubs it to succeed with canned counts (proving the
# assertions actually read them, not always 0), checks every logged invocation carries --server, and
# exercises the python3-missing degrade path.
set -uo pipefail
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"

passed=0
failed=0
ok() { passed=$((passed + 1)); printf '  ok    %s\n' "$1"; }
no() { failed=$((failed + 1)); printf '  FAIL  %s\n' "$1"; }
check() { # check <description> <command...>
    local desc="$1"
    shift
    if "$@"; then ok "$desc"; else
        no "$desc"
        return 0
    fi
}
contains() { grep -q -- "$2" "$1"; }
not_contains() { ! grep -q -- "$2" "$1"; }
count_matches() { grep -c -- "$2" "$1" 2>/dev/null || true; } # never fails the -e-less caller on "0 matches"

sandbox="$(mktemp -d "${TMPDIR:-/tmp}/lyracore-import-world-sql-test.XXXXXX")"
trap 'rm -rf "$sandbox"' EXIT

stubs="$sandbox/stubs"
mkdir -p "$stubs"
stub_log="$sandbox/stub.log"
: >"$stub_log"

# --- the scratch checkout: import-world.sh's own `cd "$(dirname "$0")/../.."` lands here -----------
checkout="$sandbox/checkout"
mkdir -p "$checkout/importer/scripts" "$checkout/target/debug"
cp "$repo_root/importer/scripts/import-world.sh" "$checkout/importer/scripts/import-world.sh"
cp "$repo_root/importer/scripts/import-manifest.sh" "$checkout/importer/scripts/import-manifest.sh"
cp "$repo_root/importer/scripts/import-class-spells.sh" "$checkout/importer/scripts/import-class-spells.sh"
chmod +x "$checkout/importer/scripts/import-world.sh" "$checkout/importer/scripts/import-class-spells.sh"
under_test="$checkout/importer/scripts/import-world.sh"

# --- stubs ----------------------------------------------------------------------------------------
# The importer binary import-world.sh execs directly by relative path (`./target/debug/lyracore-importer`,
# never resolved through PATH) — no real ETL, no dump, no client data; every family "imports" instantly.
cat >"$checkout/target/debug/lyracore-importer" <<'STUB'
#!/bin/sh
echo "lyracore-importer $*" >>"$STUB_LOG"
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

# `spacetime`, driven by two env knobs the test sets per scenario:
#   STUB_MODE=fail     — every `sql`/`call` fails (a dead/unreachable/wrong server)
#   STUB_MODE=succeed  — `sql` returns STUB_ROW_COUNT canned numeric rows (mimics a real table dump);
#                         a query that is plainly the one-continent map_id/COUNT(*) probe answers with
#                         a single map-0 row instead, so the preflight/re-assert guards don't misread
#                         the filler as foreign-continent contamination and abort somewhere this test
#                         isn't exercising.
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
        echo "spacetimedb tool version 2.7.1"
        ;;
esac
exit 0
STUB
chmod +x "$stubs/spacetime"

out="$sandbox/out.txt"

run_import() { # run_import [extra env assignments already exported by the caller]
    (
        cd "$sandbox" || exit 99
        env -i \
            HOME="$sandbox/home" \
            PATH="$stubs:/usr/bin:/bin" \
            STUB_LOG="$stub_log" \
            STUB_MODE="${STUB_MODE:-succeed}" \
            STUB_ROW_COUNT="${STUB_ROW_COUNT:-20}" \
            SPACETIME_SERVER="${SPACETIME_SERVER:-}" \
            bash "$under_test"
    ) >"$out" 2>&1
    code=$?
}

# =====================================================================================================
# 1. spacetime UNREACHABLE — the exact #440 repro: a query that FAILS must produce ONE clear abort,
#    never a wall of per-table FAILs that blame the data.
# =====================================================================================================
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

# =====================================================================================================
# 2. spacetime REACHABLE, canned counts — the assertions must actually READ what the query returned,
#    not silently collapse a real answer to 0.
# =====================================================================================================
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

# =====================================================================================================
# 3. every spacetime sql/call invocation carries --server (defect a) --------------------------------
# =====================================================================================================
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

# =====================================================================================================
# 4. python3 absent — the coverage audit must degrade explicitly, never crash mid-output (defect c) --
# =====================================================================================================
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
check "the run still completes (python3 is an audit, not a gate)" test -n "$(cat "$out")"
check "it prints the explicit SKIP line" contains "$out" "SKIP: service-coverage audit needs python3"
check "it never lets a bare 'command not found' leak into the output" \
    not_contains "$out" "python3: command not found"
check "the coverage floors are not silently reported as achieved-0 FAILs" \
    not_contains "$out" "trainer coverage (spawned trainers that teach): got 0"

echo
printf '%d passed, %d failed\n' "$passed" "$failed"
[ "$failed" -eq 0 ]
