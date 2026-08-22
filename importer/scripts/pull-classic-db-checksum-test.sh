#!/usr/bin/env bash
# pull-classic-db-checksum-test.sh — OFFLINE fixture test for pull-classic-db.sh's `verify_sha256`.
#
# Run it from the checkout root, any time, with nothing set up:
#   bash importer/scripts/pull-classic-db-checksum-test.sh     # prints "[test] OK" when it passes
#
# No network, no git clone, no database: it sources the pull script (whose `main` only runs when the
# file is EXECUTED, not sourced — see that file's tail) and exercises `verify_sha256` directly against
# a known temp file, asserting the right/wrong-checksum exit codes. Safe to run in CI.
#
# WHY IT EXISTS: `verify_sha256` is the guard that stops a silently-changed upstream dump from being
# imported, and its only real-world exercise is a multi-gigabyte network pull nobody runs on a whim.
# An untested guard is an assumed guard, so both arms get pinned here instead.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1

# shellcheck source=importer/scripts/pull-classic-db.sh
source importer/scripts/pull-classic-db.sh

fail=0
chk() { # label  expected-exit  actual-exit
  if [ "$3" != "$2" ]; then echo "  FAIL  $1: exit $3, want $2"; fail=1; else echo "  ok    $1: exit $3"; fi
}

tmpfile="$(mktemp)"
fixture_dir="$(mktemp -d)"
trap 'rm -f "$tmpfile"; rm -rf "$fixture_dir"' EXIT
printf 'known fixture bytes for the checksum test' > "$tmpfile"
good_sha256="$(sha256sum "$tmpfile" | awk '{print $1}')"
bad_sha256="0000000000000000000000000000000000000000000000000000000000000000"

echo "[test] verify_sha256 against a known-good digest:"
verify_sha256 "$tmpfile" "$good_sha256" >/dev/null 2>&1
chk "matching sha256 exits 0" 0 "$?"

echo "[test] verify_sha256 against a wrong digest:"
verify_sha256 "$tmpfile" "$bad_sha256" >/dev/null 2>&1
chk "mismatched sha256 exits nonzero" 1 "$?"

echo "[test] verify_sha256 against a missing file:"
verify_sha256 "/nonexistent/path/that/cannot/exist" "$good_sha256" >/dev/null 2>&1
chk "missing file exits nonzero" 1 "$?"

echo "[test] assemble preserves the exact decompressed bytes:"
mkdir -p "$fixture_dir/source/Full_DB"
printf 'exact profile bytes;\n' | gzip >"$fixture_dir/source/Full_DB/profile.sql.gz"
CLONE_DIR="$fixture_dir/source"
OUT_FILE="$fixture_dir/output.sql"
ASSEMBLE_GLOB="Full_DB/*.sql*"
assemble >/dev/null 2>&1
if cmp -s <(printf 'exact profile bytes;\n') "$OUT_FILE"; then
  chk "assembly adds no delimiter" 0 0
else
  chk "assembly adds no delimiter" 0 1
fi

if [ "$fail" = 0 ]; then echo "[test] OK — pull-classic-db.sh checksum fixture passed"; else echo "[test] FAIL"; exit 1; fi
