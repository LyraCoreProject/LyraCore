#!/usr/bin/env bash
# Delete SpacetimeDB history that no restart can need. SpacetimeDB 2.7.1 never deletes commit-log
# segments, snapshots or module logs by itself. A restart loads the newest valid snapshot and replays
# the segments after it, and falls back to an older snapshot if the newest fails to load. So this
# keeps the two newest valid snapshots, keeps every segment from the one that holds S2 + 1 (S2 is the
# older kept snapshot), and deletes module logs from before today (UTC). The rules come from the
# v2.7.1 source and clockworklabs/SpacetimeDB#5542.
#
# Usage: spacetimedb-prune.sh [--apply] [DATA_DIR]
# Without --apply it only reports. DATA_DIR defaults to /var/lib/lyracore/spacetimedb.
set -euo pipefail

apply=0
if [ "${1:-}" = --apply ]; then
  apply=1
  shift
fi
data=${1:-/var/lib/lyracore/spacetimedb}
# The segment compressor rewrites files after each snapshot; leave anything it may still own.
min_age=${PRUNE_MIN_AGE_MINUTES:-30}
today=$(date -u +%F)

[ -d "$data/replicas" ] || {
  echo "$data has no replicas/ directory; not a SpacetimeDB data dir" >&2
  exit 1
}
cd "$data"

offset() { echo $((10#${1%%.*})); }
settled() { [ -z "$(find "$1" -maxdepth 0 -mmin "-$min_age")" ]; }
bytes() { du -sb "$@" | awk '{s += $1} END {print s + 0}'; }
drop() { [ "$apply" = 1 ] && rm -rf -- "$@"; return 0; }

total=0
for replica in "$data"/replicas/*/; do
  replica=${replica%/}
  snaps=$replica/snapshots
  clog=$replica/clog
  [ -d "$snaps" ] && [ -d "$clog" ] || continue

  # A valid snapshot is N.snapshot_dir holding N.snapshot_bsatn, with no N.lock beside it.
  valid=()
  for dir in "$snaps"/*.snapshot_dir; do
    [ -d "$dir" ] || continue
    n=$(basename "$dir" .snapshot_dir)
    [ -f "$dir/$n.snapshot_bsatn" ] && [ ! -e "$snaps/$n.lock" ] && valid+=("$n")
  done
  if [ "${#valid[@]}" -lt 2 ]; then
    echo "$replica: ${#valid[@]} valid snapshot(s); nothing pruned"
    continue
  fi
  # Names are zero-padded to 20 digits, so a text sort is a numeric sort.
  mapfile -t valid < <(printf '%s\n' "${valid[@]}" | sort)
  s2=$(offset "${valid[${#valid[@]} - 2]}")

  old_snaps=()
  for n in "${valid[@]}"; do
    [ "$(offset "$n")" -lt "$s2" ] && settled "$snaps/$n.snapshot_dir" && old_snaps+=("$snaps/$n.snapshot_dir")
  done

  # Only a contiguous run of the oldest segments may go: a gap between kept segments is fatal on restart.
  mapfile -t segs < <(find "$clog" -maxdepth 1 -name '*.stdb.log' -printf '%f\n' | sort)
  old_segs=()
  for ((i = 0; i + 1 < ${#segs[@]}; i++)); do
    [ "$(offset "${segs[i + 1]}")" -le $((s2 + 1)) ] && settled "$clog/${segs[i]}" || break
    base=$clog/${segs[i]%.stdb.log}
    old_segs+=("$base.stdb.log")
    [ -e "$base.stdb.ofs" ] && old_segs+=("$base.stdb.ofs")
  done

  old_logs=()
  for log in "$replica"/module_logs/*.log; do
    [ -f "$log" ] && [[ "$(basename "$log" .log)" < "$today" ]] && old_logs+=("$log")
  done

  freed=0
  for group in old_snaps old_segs old_logs; do
    declare -n files=$group
    if [ "${#files[@]}" -gt 0 ]; then
      freed=$((freed + $(bytes "${files[@]}")))
      drop "${files[@]}"
    fi
    unset -n files
  done
  total=$((total + freed))
  echo "$replica: keep snapshots from $s2; $([ "$apply" = 1 ] && echo deleted || echo would delete)" \
    "${#old_snaps[@]} snapshot(s), $(( ${#old_segs[@]} )) segment file(s), ${#old_logs[@]} module log(s):" \
    "$((freed / 1000000)) MB"
done
echo "total: $((total / 1000000)) MB $([ "$apply" = 1 ] && echo freed || echo reclaimable; true)"
