#!/bin/sh
set -eu

fail() { printf 'movement batch acceptance: %s\n' "$*" >&2; exit 2; }

realm=
driver=
movers=500
duration=120
visual=NOT_RUN
while [ "$#" -gt 0 ]; do
    case $1 in
        --realm) [ "$#" -ge 2 ] || fail "--realm needs a value"; realm=$2; shift 2 ;;
        --load-driver) [ "$#" -ge 2 ] || fail "--load-driver needs a value"; driver=$2; shift 2 ;;
        --movers) [ "$#" -ge 2 ] || fail "--movers needs a value"; movers=$2; shift 2 ;;
        --duration) [ "$#" -ge 2 ] || fail "--duration needs a value"; duration=$2; shift 2 ;;
        --visual-verdict) [ "$#" -ge 2 ] || fail "--visual-verdict needs a value"; visual=$2; shift 2 ;;
        *) fail "unknown argument: $1" ;;
    esac
done

case $realm in disposable:*) ;; *) fail "--realm must explicitly start with disposable:" ;; esac
[ -n "$driver" ] || fail "--load-driver is required"
[ -x "$driver" ] || fail "load driver is not executable: $driver"

output=$($driver --realm "$realm" --movers "$movers" --duration "$duration") || fail "load driver failed"
value() { printf '%s\n' "$output" | awk -F= -v key="$1" '$1 == key { print $2; found=1 } END { if (!found) exit 1 }'; }

entries=$(value submitted_entries) || fail "driver omitted submitted_entries"
calls=$(value reducer_calls) || fail "driver omitted reducer_calls"
failed_entries=$(value failed_entries) || fail "driver omitted failed_entries"
failed_calls=$(value failed_calls) || fail "driver omitted failed_calls"
sizes=$(value batch_size_buckets) || fail "driver omitted batch_size_buckets"
seated=$(value seated_movers) || fail "driver omitted seated_movers"
txn=$(value transaction_p95_ms) || fail "driver omitted transaction_p95_ms"
action=$(value action_latency_p95_ms) || fail "driver omitted action_latency_p95_ms"

case $entries:$calls:$failed_entries:$failed_calls:$seated in *[!0-9:]*|'') fail "integer fields must be non-negative integers" ;; esac
if [ "$calls" -eq 0 ]; then factor=NA; else factor=$(awk -v e="$entries" -v c="$calls" 'BEGIN { printf "%.2f", e/c }'); fi
accepted=yes
[ "$seated" -ge 500 ] || accepted=no
[ "$calls" -gt 0 ] || accepted=no
[ "$failed_entries" -eq 0 ] || accepted=no
[ "$failed_calls" -eq 0 ] || accepted=no

printf '%s\n' \
    "movement_batch_acceptance=$accepted" \
    "realm=$realm" \
    "load_driver=$driver" \
    "configured_movers=$movers" \
    "seated_movers=$seated" \
    "duration_seconds=$duration" \
    "submitted_entries=$entries" \
    "reducer_calls=$calls" \
    "batch_factor=$factor" \
    "failed_entries=$failed_entries" \
    "failed_calls=$failed_calls" \
    "batch_size_buckets=$sizes" \
    "transaction_p95_ms=$txn" \
    "action_latency_p95_ms=$action" \
    "client_1_12_1_visual_verdict=$visual"

[ "$accepted" = yes ]
