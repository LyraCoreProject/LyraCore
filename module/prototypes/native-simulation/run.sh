#!/usr/bin/env bash
set -euo pipefail

prototype_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)
cd -- "$prototype_root"

if command -v taskset >/dev/null 2>&1; then
    prototype_affinity=$(taskset -pc $$)
    prototype_affinity=${prototype_affinity##*: }
    prototype_cpu=${prototype_affinity%%[,-]*}
    taskset -pc "$prototype_cpu" $$ >/dev/null
fi

export CARGO_BUILD_JOBS=1
export CARGO_PROFILE_DEV_CODEGEN_UNITS=1
export CARGO_PROFILE_DEV_DEBUG=0
export CARGO_PROFILE_TEST_CODEGEN_UNITS=1
export CARGO_PROFILE_TEST_DEBUG=0
export TOKIO_WORKER_THREADS=1
export RAYON_NUM_THREADS=1

prototype_mode=${1:-demo}
if (($#)); then shift; fi
case "$prototype_mode" in
    demo)
        cargo run --locked -q -p lyracore-module --features native_simulation_prototype \
            --example native_simulation_prototype -- "$@"
        ;;
    check)
        cargo test --locked -p lyracore-module --features native_simulation_prototype \
            --test native_simulation_prototype -- --test-threads=1
        ;;
    reference)
        cargo build --locked -p lyracore-module --features native_simulation_prototype \
            --target wasm32-unknown-unknown
        prototype_target=$(cd -- "${CARGO_TARGET_DIR:-target}" && pwd)
        export LYRACORE_NATIVE_REFERENCE_WASM="$prototype_target/wasm32-unknown-unknown/debug/lyracore_module.wasm"
        export LYRACORE_NATIVE_REFERENCE_TRACE="$prototype_target/native-simulation-reference.jsonl"
        timeout 180s cargo test --locked -p lyracore-module --features native_simulation_prototype \
            --test native_simulation_prototype table_backed_module_matches_the_native_trace \
            -- --ignored --test-threads=1 --nocapture
        ;;
    benchmark)
        export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
        export CARGO_PROFILE_RELEASE_DEBUG=0
        cargo build --release --locked -p lyracore-module --features native_simulation_prototype \
            --target wasm32-unknown-unknown
        cargo build --release --locked -p lyracore-module --features native_simulation_prototype \
            --example native_simulation_benchmark
        prototype_target=$(cd -- "${CARGO_TARGET_DIR:-target}" && pwd)
        export LYRACORE_NATIVE_REFERENCE_WASM="$prototype_target/wasm32-unknown-unknown/release/lyracore_module.wasm"
        timeout 300s cargo test --release --locked -p lyracore-module --features native_simulation_prototype \
            --test native_simulation_prototype -- --include-ignored --test-threads=1
        prototype_results=${1:-"$prototype_target/native-simulation-benchmark-$(date -u +%Y%m%dT%H%M%SZ).jsonl"}
        if (($#)); then shift; fi
        timeout 900s "$prototype_target/release/examples/native_simulation_benchmark" \
            "$LYRACORE_NATIVE_REFERENCE_WASM" "$prototype_results" "$@"
        python3 module/prototypes/native-simulation/summarize.py "$prototype_results"
        ;;
    *)
        echo 'Usage: run.sh [demo [scenario|--list]|check|reference|benchmark [output.jsonl [creatures ...]]]' >&2
        exit 2
        ;;
esac
