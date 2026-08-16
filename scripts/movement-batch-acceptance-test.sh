#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

make_driver() {
    name=$1 seated=$2 entries=$3 calls=$4 failed_entries=$5 failed_calls=$6
    sed -e "s/@SEATED@/$seated/" -e "s/@ENTRIES@/$entries/" -e "s/@CALLS@/$calls/" \
        -e "s/@FAILED_ENTRIES@/$failed_entries/" -e "s/@FAILED_CALLS@/$failed_calls/" \
        "$root/scripts/fixtures/movement-load-driver.sh" >"$tmp/$name"
    chmod +x "$tmp/$name"
}

make_driver normal 500 12000 100 0 0
out=$($root/scripts/movement-batch-acceptance.sh --realm disposable:test --load-driver "$tmp/normal" --movers 500 --duration 60)
printf '%s\n' "$out" | grep -q '^movement_batch_acceptance=yes$'
printf '%s\n' "$out" | grep -q '^batch_factor=120.00$'
printf '%s\n' "$out" | grep -q '^batch_size_buckets=1:0,2-32:0,33-64:0,65-127:4,128:96$'

make_driver insufficient 499 1000 10 0 0
if $root/scripts/movement-batch-acceptance.sh --realm disposable:test --load-driver "$tmp/insufficient" >/dev/null; then
    echo "insufficient population unexpectedly passed" >&2; exit 1
fi

make_driver failed 500 1000 10 12 1
if $root/scripts/movement-batch-acceptance.sh --realm disposable:test --load-driver "$tmp/failed" >/dev/null; then
    echo "failed submissions unexpectedly passed" >&2; exit 1
fi

make_driver idle 500 0 0 0 0
idle=$($root/scripts/movement-batch-acceptance.sh --realm disposable:test --load-driver "$tmp/idle" 2>/dev/null || true)
printf '%s\n' "$idle" | grep -q '^batch_factor=NA$'
printf '%s\n' "$idle" | grep -q '^movement_batch_acceptance=no$'

if $root/scripts/movement-batch-acceptance.sh --realm lyracore-deploy --load-driver "$tmp/normal" >/dev/null 2>&1; then
    echo "non-disposable realm unexpectedly accepted" >&2; exit 1
fi
echo "movement batch acceptance script tests passed"
