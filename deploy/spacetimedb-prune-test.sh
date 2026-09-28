#!/usr/bin/env bash
# Self-check for spacetimedb-prune.sh on a synthetic data dir.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
fail=0
old=$(date -d '-2 hours' +%Y%m%d%H%M)

pad() { printf '%020d' "$1"; }
snapshot() { # offset [nofile|locked]
  local d
  d=$root/replicas/1/snapshots/$(pad "$1").snapshot_dir
  mkdir -p "$d/objects"
  [ "${2:-}" = nofile ] || echo x > "$d/$(pad "$1").snapshot_bsatn"
  [ "${2:-}" = locked ] && touch "$root/replicas/1/snapshots/$(pad "$1").lock"
  touch -t "$old" "$d"
}
segment() {
  local b
  b=$root/replicas/1/clog/$(pad "$1")
  mkdir -p "$root/replicas/1/clog"
  echo x > "$b.stdb.log"
  echo x > "$b.stdb.ofs"
  touch -t "$old" "$b.stdb.log" "$b.stdb.ofs"
}
exists() { if [ -e "$1" ]; then echo "  ok    kept $2"; else echo "  FAIL  deleted $2"; fail=1; fi; }
gone() { if [ -e "$1" ]; then echo "  FAIL  kept $2"; fail=1; else echo "  ok    deleted $2"; fi; }

for n in 0 100 200 300; do snapshot "$n"; done
snapshot 250 nofile
snapshot 350 locked
for n in 0 90 180 260 320; do segment "$n"; done
mkdir -p "$root/replicas/1/module_logs"
echo x > "$root/replicas/1/module_logs/2020-01-01.log"
echo x > "$root/replicas/1/module_logs/$(date -u +%F).log"
S=$root/replicas/1/snapshots C=$root/replicas/1/clog L=$root/replicas/1/module_logs

echo "[prune] dry run deletes nothing:"
bash "$here/spacetimedb-prune.sh" "$root" > /dev/null
exists "$S/$(pad 0).snapshot_dir" "snapshot 0"
exists "$C/$(pad 0).stdb.log" "segment 0"

echo "[prune] apply keeps the two newest valid snapshots and the log from S2 + 1:"
bash "$here/spacetimedb-prune.sh" --apply "$root" > /dev/null
gone "$S/$(pad 0).snapshot_dir" "snapshot 0"
gone "$S/$(pad 100).snapshot_dir" "snapshot 100"
exists "$S/$(pad 200).snapshot_dir" "snapshot 200 (S2)"
exists "$S/$(pad 300).snapshot_dir" "snapshot 300 (newest)"
exists "$S/$(pad 250).snapshot_dir" "incomplete snapshot 250"
exists "$S/$(pad 350).snapshot_dir" "locked snapshot 350"
gone "$C/$(pad 0).stdb.log" "segment 0"
gone "$C/$(pad 90).stdb.ofs" "segment 90 index"
exists "$C/$(pad 180).stdb.log" "segment 180 (holds S2 + 1)"
exists "$C/$(pad 260).stdb.log" "segment 260"
gone "$L/2020-01-01.log" "old module log"
exists "$L/$(date -u +%F).log" "today's module log"

echo "[prune] a recent segment stops the run, so no gap opens:"
rm -rf "$root/replicas"
for n in 0 100 200 300; do snapshot "$n"; done
for n in 0 90 180 260; do segment "$n"; done
touch "$C/$(pad 90).stdb.log"
bash "$here/spacetimedb-prune.sh" --apply "$root" > /dev/null
gone "$C/$(pad 0).stdb.log" "segment 0"
exists "$C/$(pad 90).stdb.log" "recent segment 90"
exists "$C/$(pad 180).stdb.log" "segment 180"

exit "$fail"
