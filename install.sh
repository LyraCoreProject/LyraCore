#!/bin/sh
# POSIX sh is required before any toolchain is installed.
set -eu

REPO_URL=https://github.com/LyraCoreProject/LyraCore.git
REPO_URL_RAW=https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh
TARGET_DIR=LyraCore
SPACETIME_VERSION=2.7.1
BINARYEN_VERSION=131
BIN_DIR=$HOME/.local/bin
LAUNCHER=$BIN_DIR/lyracore
PROFILE_MARKER='# added by the LyraCore installer'
LAUNCHER_MARKER='# LyraCore launcher (installed by install.sh)'

assume_yes=0
# Retain the PATH inherited by this installer for the final shell instructions.
original_path=${PATH:-}
os=$(uname -s)
arch=$(uname -m)

say() { printf '%s\n' "$*"; }
note() { printf '  %s\n' "$*"; }
fail() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}

have() { command -v "$1" >/dev/null 2>&1; }

usage() {
    cat <<'USAGE'
Usage: sh install.sh [--yes] [--help]

Clones LyraCore into ./LyraCore, checks its prerequisites, and installs the `lyracore` launcher
into $HOME/.local/bin. Never runs sudo; never overwrites an existing ./LyraCore.

  --yes, -y   assume yes to every prompt (required for a non-interactive `curl | sh` run that
              has anything left to install)
  --help, -h  print this and exit
USAGE
}

confirm() {
    if [ "$assume_yes" -eq 1 ]; then
        printf '%s [y/N] y (--yes)\n' "$1"
        return 0
    fi
    # Opening /dev/tty detects a controlling terminal; a readable device node does not.
    if ! (exec </dev/tty) 2>/dev/null; then
        fail "no terminal to prompt on (a piped \`curl | sh\` run) and --yes was not given.
  Re-run as: curl -sSfL $REPO_URL_RAW | sh -s -- --yes
  ...or download it first and run \`sh install.sh\`, which can prompt."
    fi
    printf '%s [y/N] ' "$1"
    read -r reply </dev/tty || reply=n
    case $reply in
        [yY] | [yY][eE][sS]) return 0 ;;
        *) return 1 ;;
    esac
}

confirm_optional() {
    if [ "$assume_yes" -eq 1 ]; then
        printf '%s [y/N] y (--yes)\n' "$1"
        return 0
    fi
    if ! (exec </dev/tty) 2>/dev/null; then
        printf '%s [y/N] n (no terminal to prompt on — skipping)\n' "$1"
        return 1
    fi
    printf '%s [y/N] ' "$1"
    read -r reply </dev/tty || reply=n
    case $reply in
        [yY] | [yY][eE][sS]) return 0 ;;
        *) return 1 ;;
    esac
}

while [ $# -gt 0 ]; do
    case $1 in
        -y | --yes) assume_yes=1 ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            printf 'install.sh: unknown option %s\n' "$1" >&2
            usage >&2
            exit 2
            ;;
    esac
    shift
done

# Refuse an existing checkout before installing prerequisites.
if [ -e "$TARGET_DIR" ]; then
    fail "$(pwd)/$TARGET_DIR already exists — nothing was changed.
  Move or remove it, or run the installer from another directory."
fi

say "LyraCore installer — checking prerequisites (nothing here runs sudo)."

missing=''
need_xcode=0
apt_pkgs=''
want() {
    missing="$missing $1"
    case " $apt_pkgs " in
        *" $2 "*) ;;
        *) apt_pkgs="$apt_pkgs $2" ;;
    esac
}

recommended=''
recommended_pkgs=''
want_recommended() {
    recommended="$recommended $1"
    case " $recommended_pkgs " in
        *" $2 "*) ;;
        *) recommended_pkgs="$recommended_pkgs $2" ;;
    esac
}

have git || want git git
have curl || want curl curl

need_openssl=1
openssl_banner='not found'
if have openssl && openssl_banner=$(openssl version 2>/dev/null); then
    case "$openssl_banner" in
        'OpenSSL '*)
            openssl_version=${openssl_banner#OpenSSL }
            openssl_version=${openssl_version%% *}
            openssl_core=${openssl_version%%[-+]*}
            openssl_suffix=${openssl_version#"$openssl_core"}
            openssl_major=${openssl_core%%.*}
            openssl_rest=${openssl_core#*.}
            openssl_minor=${openssl_rest%%.*}
            openssl_patch=${openssl_rest#*.}
            case "$openssl_major:$openssl_minor:$openssl_patch" in
                *[!0-9:]* | :* | *::* | *:) ;;
                *)
                    openssl_suffix_ok=1
                    if [ -n "$openssl_suffix" ]; then
                        case "${openssl_suffix#?}" in
                            '' | *[!abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._~+-]*)
                                openssl_suffix_ok=0
                                ;;
                        esac
                    fi
                    if [ "$openssl_suffix_ok" -eq 1 ] &&
                       [ "$openssl_core" != "$openssl_rest" ] &&
                       [ "$openssl_rest" != "$openssl_patch" ] &&
                       [ "$openssl_major" -ge 3 ] 2>/dev/null; then
                        need_openssl=0
                    fi
                    ;;
            esac
            ;;
    esac
fi
if [ "$need_openssl" -eq 1 ]; then
    want "OpenSSL 3 or newer command" openssl
    note "OpenSSL command: ${openssl_banner:-unavailable}"
fi

if [ "$os" = Linux ]; then
    if ! have cc && ! have gcc && ! have clang; then
        want "a C compiler" build-essential
    fi
    if have pkg-config; then
        if ! pkg-config --exists openssl; then
            want "OpenSSL development headers" libssl-dev
        fi
    else
        want pkg-config pkg-config
        want "OpenSSL development headers" libssl-dev
    fi
elif [ "$os" = Darwin ]; then
    if ! have cc && ! have clang; then
        need_xcode=1
        missing="$missing the-Xcode-Command-Line-Tools"
    fi
else
    fail "unsupported platform \"$os\" — LyraCore supports Linux and macOS only."
fi

if [ -n "$missing" ]; then
    printf 'install.sh: missing prerequisite(s):%s\n' "$missing" >&2
    printf 'install.sh: this installer never runs sudo — install them yourself, then re-run it:\n' >&2
    if [ "$os" = Darwin ]; then
        printf '\n' >&2
        if [ "$need_xcode" -eq 1 ]; then
            printf '  xcode-select --install\n' >&2
        fi
        if [ "$need_openssl" -eq 1 ]; then
            printf '%s\n' '  brew install openssl@3' \
                '  export PATH="$(brew --prefix openssl@3)/bin:$PATH"' \
                '  Keep that PATH entry in your shell configuration; LibreSSL is not supported.' >&2
        fi
        printf '\n' >&2
    elif have apt-get; then
        printf '\n  sudo apt-get update\n  sudo apt-get install -y%s\n\n' "$apt_pkgs" >&2
    else
        printf '\n  (your package manager'\''s equivalent of:%s )\n\n' "$apt_pkgs" >&2
    fi
    exit 1
fi
note "✓ system packages"

# rustup reads the checkout's rust-toolchain.toml on the first cargo command.
cargo_needs_reload=0
if ! have cargo && [ -f "$HOME/.cargo/env" ]; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
    cargo_needs_reload=1
fi
if have cargo; then
    note "✓ Rust ($(cargo --version 2>/dev/null || echo cargo))"
else
    say ""
    say "Rust is not installed. The official rustup installer would be run, unmodified:"
    note "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y"
    note "(it installs into ~/.rustup and ~/.cargo, needs no root, and adds its own PATH line to"
    note " your shell profile — that part is rustup's, not this script's)"
    confirm "Install rustup now?" || fail "Rust is required. Install it and re-run this script."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y ||
        fail "the rustup installer failed"
    if [ -f "$HOME/.cargo/env" ]; then
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
        cargo_needs_reload=1
    fi
    have cargo || fail "rustup installed, but cargo is still not on PATH — open a new shell and re-run"
    note "✓ Rust ($(cargo --version 2>/dev/null || echo cargo))"
fi

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) PATH="$BIN_DIR:$PATH" ;;
esac
export PATH

if ! have spacetime; then
    say ""
    say "SpacetimeDB is not installed. The official installer would be run, unmodified:"
    note "curl -sSf https://install.spacetimedb.com | sh -s -- -y"
    note "(it installs into $BIN_DIR and needs no root)"
    confirm "Install SpacetimeDB now?" || fail "SpacetimeDB is required. Install it and re-run this script."
    curl -sSf https://install.spacetimedb.com | sh -s -- -y || fail "the SpacetimeDB installer failed"
    have spacetime || fail "SpacetimeDB installed, but \`spacetime\` is not in $BIN_DIR"
fi

found=$(spacetime --version 2>/dev/null | sed -n 's/.*version[ ]*\([0-9][0-9.]*\).*/\1/p' | head -n 1)
if [ "$found" = "$SPACETIME_VERSION" ]; then
    note "✓ SpacetimeDB $found"
else
    say ""
    say "This repository is pinned to SpacetimeDB exactly $SPACETIME_VERSION (found: ${found:-unknown})."
    say "Versions install side by side, so this is not destructive:"
    note "spacetime version install $SPACETIME_VERSION"
    note "spacetime version use $SPACETIME_VERSION"
    confirm "Pin SpacetimeDB to $SPACETIME_VERSION now?" ||
        fail "SpacetimeDB $SPACETIME_VERSION is required (\`lyracore preflight\` enforces an exact match)."
    spacetime version install "$SPACETIME_VERSION" || fail "spacetime version install $SPACETIME_VERSION failed"
    spacetime version use "$SPACETIME_VERSION" || fail "spacetime version use $SPACETIME_VERSION failed"
    note "✓ SpacetimeDB $SPACETIME_VERSION"
fi

# Keep the Binaryen lib/ directory beside wasm-opt for its runtime library lookup.
if have wasm-opt; then
    note "✓ wasm-opt ($(wasm-opt --version 2>/dev/null || echo wasm-opt))"
else
    binaryen_asset=''
    case "$os-$arch" in
        Linux-x86_64) binaryen_asset=x86_64-linux ;;
        Linux-aarch64 | Linux-arm64) binaryen_asset=aarch64-linux ;;
        Darwin-x86_64) binaryen_asset=x86_64-macos ;;
        Darwin-arm64 | Darwin-aarch64) binaryen_asset=arm64-macos ;;
    esac

    if [ -z "$binaryen_asset" ]; then
        say ""
        say "wasm-opt: no prebuilt Binaryen $BINARYEN_VERSION release for $os/$arch — skipping the local install."
        want_recommended wasm-opt binaryen
    else
        binaryen_url="https://github.com/WebAssembly/binaryen/releases/download/version_$BINARYEN_VERSION/binaryen-version_$BINARYEN_VERSION-$binaryen_asset.tar.gz"
        binaryen_dir="$HOME/.local/share/lyracore/binaryen-version_$BINARYEN_VERSION"
        say ""
        say "wasm-opt is not installed. spacetime publish uses it to optimise the module wasm — it still"
        say "works without it, just unoptimised. This would be downloaded and extracted, user-local:"
        note "$binaryen_url"
        note "-> $binaryen_dir  (symlinked as $BIN_DIR/wasm-opt)"
        if confirm_optional "Install wasm-opt now?"; then
            binaryen_tmp=$(mktemp -d "${TMPDIR:-/tmp}/lyracore-binaryen.XXXXXX") || binaryen_tmp=''
            if [ -n "$binaryen_tmp" ] &&
                curl -sSfL -o "$binaryen_tmp/binaryen.tar.gz" "$binaryen_url" &&
                tar -xzf "$binaryen_tmp/binaryen.tar.gz" -C "$binaryen_tmp"; then
                mkdir -p "$HOME/.local/share/lyracore" "$BIN_DIR"
                rm -rf "$binaryen_dir"
                mv "$binaryen_tmp/binaryen-version_$BINARYEN_VERSION" "$binaryen_dir"
                ln -sf "$binaryen_dir/bin/wasm-opt" "$BIN_DIR/wasm-opt"
                rm -rf "$binaryen_tmp"
                if have wasm-opt; then
                    note "✓ wasm-opt ($(wasm-opt --version 2>/dev/null || echo installed))"
                else
                    note "wasm-opt was extracted but is not runnable here — continuing without it"
                    want_recommended wasm-opt binaryen
                fi
            else
                [ -n "$binaryen_tmp" ] && rm -rf "$binaryen_tmp"
                note "download or extract failed — continuing without wasm-opt"
                want_recommended wasm-opt binaryen
            fi
        else
            note "skipping — re-run this installer later, or install by hand from $binaryen_url"
            want_recommended wasm-opt binaryen
        fi
    fi
fi

if [ -n "$recommended" ]; then
    say ""
    printf 'install.sh: recommended (optional) prerequisite(s) missing:%s\n' "$recommended"
    printf 'install.sh: everything still works without them, just slower — install when convenient:\n'
    if [ "$os" = Darwin ]; then
        printf '\n  brew install%s\n\n' "$recommended_pkgs"
    elif have apt-get; then
        printf '\n  sudo apt-get install -y%s\n\n' "$recommended_pkgs"
    else
        printf '\n  (your package manager'\''s equivalent of:%s )\n\n' "$recommended_pkgs"
    fi
fi

say ""
say "Cloning $REPO_URL into $(pwd)/$TARGET_DIR ..."
git clone "$REPO_URL" "$TARGET_DIR" || fail "git clone failed"

if [ -e "$LAUNCHER" ] && ! grep -Fq "$LAUNCHER_MARKER" "$LAUNCHER" 2>/dev/null; then
    fail "$LAUNCHER already exists and was not written by this installer — leaving it alone.
  Remove it yourself if you want the launcher, or just use ./lyracore inside the checkout."
fi

mkdir -p "$BIN_DIR"
{
    printf '#!/bin/sh\n'
    printf '%s — https://github.com/LyraCoreProject/LyraCore\n' "$LAUNCHER_MARKER"
    cat <<'LAUNCHER_EOF'
# Walks up from $PWD for the checkout you are standing in and execs THAT checkout's ./lyracore by
# path (never by PATH, so there is no recursion) — every clone therefore runs the CLI revision
# pinned in its own .lyracore-cli-rev, and this file never needs updating.
set -eu
dir=$(pwd -P)
while [ ! -f "$dir/.lyracore-cli-rev" ]; do
    if [ "$dir" = / ]; then
        printf 'lyracore: not inside a LyraCore checkout (no .lyracore-cli-rev here or in any parent of %s)\n' "$(pwd -P)" >&2
        exit 2
    fi
    dir=$(dirname -- "$dir")
done
if [ ! -x "$dir/lyracore" ]; then
    printf 'lyracore: %s is missing or not executable\n' "$dir/lyracore" >&2
    exit 2
fi
exec "$dir/lyracore" "$@"
LAUNCHER_EOF
} >"$LAUNCHER"
chmod +x "$LAUNCHER"
say ""
note "✓ launcher installed at $LAUNCHER"

path_export='export PATH="$HOME/.local/bin:$PATH"'
path_line="$path_export  $PROFILE_MARKER"
path_ready=0
profile=''

case ":$original_path:" in
    *":$BIN_DIR:"*)
        path_ready=1
        note "✓ $BIN_DIR is already on your PATH — nothing appended"
        ;;
    *)
        case ${SHELL:-} in
            */zsh) profile=$HOME/.zshrc ;;
            */bash) profile=$HOME/.bashrc ;;
            *) profile=$HOME/.profile ;;
        esac
        if [ -f "$profile" ] && grep -Fq "$PROFILE_MARKER" "$profile"; then
            path_ready=1
            note "✓ $profile already has the installer's PATH line — not appending a second one"
        elif [ -f "$profile" ] && grep -Fq '.local/bin' "$profile"; then
            path_ready=1
            note "✓ $profile already puts $BIN_DIR on PATH its own way — leaving it untouched"
        else
            say ""
            say "$BIN_DIR is not on your PATH. This line would be appended to $profile:"
            note "$path_line"
            if confirm "Append it?"; then
                printf '%s\n' "$path_line" >>"$profile"
                path_ready=1
                note "✓ appended to $profile (one line, marked; re-running never duplicates it)"
            else
                note "nothing appended — use ./lyracore inside the checkout instead"
            fi
        fi
        ;;
esac

say ""
say "✓ LyraCore is installed in $(pwd)/$TARGET_DIR"
say ""
if [ "$cargo_needs_reload" -eq 1 ]; then
    say 'This shell doesn'"'"'t have cargo on PATH yet (rustup only wires up new shells) — run: . "$HOME/.cargo/env"'
    say "(or start a new shell, which picks this up too)"
    say ""
fi
if [ "$path_ready" -eq 1 ]; then
    case ":$original_path:" in
        *":$BIN_DIR:"*) ;;
        *) say "Start a new shell first (or run: $path_export)" ;;
    esac
    say "  cd $TARGET_DIR"
    say "  lyracore doctor        # check prerequisites"
    say "  lyracore dev up        # start the seeded local realm"
else
    say "  cd $TARGET_DIR"
    say "  ./lyracore doctor      # check prerequisites"
    say "  ./lyracore dev up      # start the seeded local realm"
fi
say ""
say "The first CLI command compiles the pinned development CLI (a minute or two); later runs are"
say "instant. Then: docs/quickstart.md — account creation, your 1.12.1 client, and troubleshooting."
