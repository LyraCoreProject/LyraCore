#!/usr/bin/env bash
set -euo pipefail

: "${LYRACORE_NATIVE_DATABASE_SERVER:?set the experimental standalone binary}"
: "${LYRACORE_NATIVE_DATABASE_CLI:?set the unmodified 2.7.1 CLI}"

prototype_args=("$@")
if [[ "${1:-}" == --config-path ]]; then
    shift 2
fi
if [[ "${1:-}" == start ]]; then
    exec "$LYRACORE_NATIVE_DATABASE_SERVER" "$@"
fi
exec "$LYRACORE_NATIVE_DATABASE_CLI" "${prototype_args[@]}"
