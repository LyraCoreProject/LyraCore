#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
npm ci
npm run build:54
cargo run --manifest-path piccolo-harness/Cargo.toml -- generated/lua54/representative.lua
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
  cargo build --manifest-path piccolo-harness/Cargo.toml --target wasm32-unknown-unknown
echo 'PASS wasm32-unknown-unknown build'
