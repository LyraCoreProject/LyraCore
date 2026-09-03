#!/usr/bin/env bash
set -euo pipefail

project_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$project_root"

expect_ignored() {
    local path=$1
    if ! git check-ignore --no-index -q -- "$path"; then
        echo "$path: expected Git to ignore this path" >&2
        exit 1
    fi
}

expect_visible() {
    local path=$1
    if git check-ignore --no-index -q -- "$path"; then
        echo "$path: expected Git to show this path" >&2
        git check-ignore --no-index -v -- "$path" >&2
        exit 1
    fi
}

# The generated-directory rule must cover a Package Delta, both Build Identity sidecars, and a
# filename that looks like a Script Artifact. A future committed Script Artifact needs a narrower
# Package-specific negation because Git cannot inspect its JSON kind.
expect_ignored packages/fire_nova/data/.generated/spell.json
expect_ignored packages/fire_nova/data/.generated/spell.identity
expect_ignored packages/fire_nova/data/.generated/fire_nova.script.json
expect_ignored packages/fire_nova/data/.generated/script.identity

# The allowlisted first-party Package directories still expose source, while an installed Package
# remains local to the checkout.
expect_visible packages/example/src/new.rs
expect_visible packages/fire_nova/src/new.rs
expect_ignored packages/local_only/src/mod.rs

echo "Package gitignore cases passed."
