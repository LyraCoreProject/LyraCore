#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
npm ci
npm run build:51
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
archive="$tmp_dir/lua-5.0.3.tar.gz"
curl -fsSL https://www.lua.org/ftp/lua-5.0.3.tar.gz -o "$archive"
echo '1193a61b0e08acaa6eee0eecf29709179ee49c71baebc59b682a25c3b5a45671  '"$archive" | sha256sum -c -
mkdir "$tmp_dir/source"
tar -xzf "$archive" -C "$tmp_dir/source" --strip-components=1
make -C "$tmp_dir/source"
mkdir -p generated/lua50
sed -E -f downlevel-lua50.sed generated/lua51/representative.lua > generated/lua50/representative.lua
"$tmp_dir/source/bin/lua" -e \
  'dofile("generated/lua50/representative.lua"); assert(SPIKE_RESULT == "HOOK:18:6-12|HOOK:9:9|27", SPIKE_RESULT); print("PASS lua=5.0.3 result=" .. SPIKE_RESULT)'
