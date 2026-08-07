#!/bin/sh
# LyraCore installer (#302) — one line from a cold machine to a prereq-checked checkout:
#
#   curl -sSfL https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh | sh
#
# Or, to read it before you run it (recommended for anything you pipe to a shell):
#
#   curl -sSfLO https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh
#   less install.sh && sh install.sh
#
# What it does, in the order docs/quickstart.md §1 documents:
#
#   1. refuses immediately if ./LyraCore already exists — a re-run touches nothing;
#   2. checks the prerequisites. It NEVER runs sudo: a missing system package prints the exact
#      apt-get line for you to run yourself and stops;
#   3. offers to install rustup and SpacetimeDB exactly 2.7.1 (both user-local, no root);
#   4. clones the repository into ./LyraCore;
#   5. installs a ten-line checkout-resolving launcher at $HOME/.local/bin/lyracore, so the CLI is
#      typed as bare `lyracore` from anywhere inside any checkout while EVERY checkout keeps the CLI
#      revision pinned in its own .lyracore-cli-rev. `./lyracore` keeps working verbatim;
#   6. if $HOME/.local/bin is not already on PATH, prints the export line and offers to append one
#      marked, idempotent line to one shell profile.
#
# Flags: --yes (-y) assume yes to every prompt; --help.
#
# Everything it writes: ./LyraCore, $HOME/.local/bin/lyracore, the rustup and SpacetimeDB installers'
# own roots (only if you consent to running them), and — only if you consent — one line in one shell
# profile. It writes nothing else, and it never needs root.
#
# POSIX sh, Linux and macOS, matching the ./lyracore shim's style: this runs before anything at all
# is installed, so it may not assume bash.
set -eu

REPO_URL=https://github.com/LyraCoreProject/LyraCore.git
REPO_URL_RAW=https://raw.githubusercontent.com/LyraCoreProject/LyraCore/main/install.sh
TARGET_DIR=LyraCore
SPACETIME_VERSION=2.7.1
BIN_DIR=$HOME/.local/bin
LAUNCHER=$BIN_DIR/lyracore
# Both greppable and load-bearing: the profile line is only ever appended when this marker is absent,
# and a pre-existing ~/.local/bin/lyracore is only overwritten when it carries the launcher's.
PROFILE_MARKER='# added by the LyraCore installer'
LAUNCHER_MARKER='# LyraCore launcher (installed by install.sh)'

# The prompt reads from the terminal, not from stdin: in a `curl | sh` run stdin IS the script.
assume_yes=0
# PATH as the user's shell will have it — the checks below may prepend $BIN_DIR for their own use.
original_path=${PATH:-}
os=$(uname -s)

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
    # Not `[ -r /dev/tty ]`: the device node usually exists and is readable even when the process
    # has no controlling terminal — only opening it tells you, and the failed open would otherwise
    # leak a raw shell error over this prompt.
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

# ---------------------------------------------------------------------------
# 1. Refuse an existing checkout FIRST, before installing anything.
# ---------------------------------------------------------------------------
# This is deliberately the very first thing that happens: a second run in the same directory must
# exit non-zero having changed nothing at all, not after half an hour of toolchain installs.
if [ -e "$TARGET_DIR" ]; then
    fail "$(pwd)/$TARGET_DIR already exists — nothing was changed.
  Move or remove it, or run the installer from another directory."
fi

say "LyraCore installer — checking prerequisites (nothing here runs sudo)."

# ---------------------------------------------------------------------------
# 2. Prerequisites we cannot install without root: report, never sudo.
# ---------------------------------------------------------------------------
missing=''
apt_pkgs=''
want() {
    # want <human name> <apt package>
    missing="$missing $1"
    case " $apt_pkgs " in
        *" $2 "*) ;;
        *) apt_pkgs="$apt_pkgs $2" ;;
    esac
}

have git || want git git
have curl || want curl curl

if [ "$os" = Linux ]; then
    if ! have cc && ! have gcc && ! have clang; then
        want "a C compiler" build-essential
    fi
    if have pkg-config; then
        # The gateway's SpacetimeDB SDK pulls in native-tls, which links system OpenSSL on Linux.
        if ! pkg-config --exists openssl; then
            want "OpenSSL development headers" libssl-dev
        fi
    else
        want pkg-config pkg-config
        want "OpenSSL development headers" libssl-dev
    fi
elif [ "$os" = Darwin ]; then
    if ! have cc && ! have clang; then
        missing="$missing the-Xcode-Command-Line-Tools"
    fi
else
    fail "unsupported platform \"$os\" — LyraCore supports Linux and macOS only."
fi

if [ -n "$missing" ]; then
    printf 'install.sh: missing prerequisite(s):%s\n' "$missing" >&2
    printf 'install.sh: this installer never runs sudo — install them yourself, then re-run it:\n' >&2
    if [ "$os" = Darwin ]; then
        printf '\n  xcode-select --install\n\n' >&2
    elif have apt-get; then
        printf '\n  sudo apt-get update\n  sudo apt-get install -y%s\n\n' "$apt_pkgs" >&2
    else
        printf '\n  (your package manager'\''s equivalent of:%s )\n\n' "$apt_pkgs" >&2
    fi
    exit 1
fi
note "✓ system packages"

# ---------------------------------------------------------------------------
# 3. rustup — user-local, no root.
# ---------------------------------------------------------------------------
# rustup reads the checkout's rust-toolchain.toml and fetches the pinned toolchain itself on the
# first cargo command, so there is deliberately no version check or `rustup update` here.
# Tracks whether THIS shell only sees cargo because we just sourced ~/.cargo/env for our own use —
# rustup's PATH line applies to shells started after the install, so the shell that ran this script
# needs the same reminder in step 8 below, whether cargo was already there or installed just now.
cargo_needs_reload=0
if ! have cargo && [ -f "$HOME/.cargo/env" ]; then
    # Installed already, just not exported into this non-login shell.
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

# ---------------------------------------------------------------------------
# 4. SpacetimeDB — exactly 2.7.1.
# ---------------------------------------------------------------------------
# The installer lands `spacetime` in $HOME/.local/bin, which is exactly why the launcher below can
# piggyback on that PATH entry rather than inventing a new one. Prepend it for the rest of THIS
# script only; $original_path is what the user's own shell will see.
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

# `spacetime --version` prints e.g. "spacetimedb tool version 2.7.1; ...".
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

# ---------------------------------------------------------------------------
# 5. Clone.
# ---------------------------------------------------------------------------
say ""
say "Cloning $REPO_URL into $(pwd)/$TARGET_DIR ..."
git clone "$REPO_URL" "$TARGET_DIR" || fail "git clone failed"

# ---------------------------------------------------------------------------
# 6. The checkout-resolving launcher.
# ---------------------------------------------------------------------------
# Putting a checkout's own ./lyracore on PATH would defeat the shim: one global CLI would shadow
# every checkout's pin and only one clone would work. This launcher instead resolves the checkout
# you are standing in, so N clones with N different pins coexist and it never needs updating.
if [ -e "$LAUNCHER" ] && ! grep -Fq "$LAUNCHER_MARKER" "$LAUNCHER" 2>/dev/null; then
    fail "$LAUNCHER already exists and was not written by this installer — leaving it alone.
  Remove it yourself if you want the launcher, or just use ./lyracore inside the checkout."
fi

mkdir -p "$BIN_DIR"
# The body goes through a QUOTED heredoc so it is written verbatim — no shell expansion, no
# backslash escaping to get subtly wrong. Only the marker line is interpolated, so the string this
# script greps for and the string it writes cannot drift apart.
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

# ---------------------------------------------------------------------------
# 7. PATH: append nothing if $HOME/.local/bin is already there.
# ---------------------------------------------------------------------------
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

# ---------------------------------------------------------------------------
# 8. What to do next.
# ---------------------------------------------------------------------------
say ""
say "✓ LyraCore is installed in $(pwd)/$TARGET_DIR"
say ""
if [ "$cargo_needs_reload" -eq 1 ]; then
    # rustup's PATH line only applies to shells started after it ran, so this one — the one running
    # `lyracore doctor` next — still can't see cargo even though Rust is installed. Same fix either way.
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
