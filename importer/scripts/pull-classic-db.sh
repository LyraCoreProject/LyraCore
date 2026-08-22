#!/usr/bin/env bash
# pull-classic-db.sh — STEP ONE of a world import: fetch the cmangos `classic-db` world dump that
# importer/scripts/import-world.sh then parses.
#
# WHEN YOU RUN IT: once, before your first import, and again whenever you bump the pinned upstream
# commit. NEEDS: `git`, `gzip`, `sha256sum`, and a few GB of disk under .import/. No database, no
# node, no built importer — this step is pure download.
#
# `lyracore import` drives this for you as part of the normal import flow; running this script by
# hand is the advanced path, for when you want to control the ref, float master, or pull once and
# import many times.
#
# OPT-IN BY DESIGN. This is the mechanism docs/data-ingestion.md's "Decisions taken" section calls
# for: "an explicit opt-in pull-script, not a git submodule" — nothing auto-fetches Blizzard-derived
# content for a casual cloner, and the association stays out of the default repo graph until an
# operator explicitly runs this.
#
# LICENSING FIREWALL (docs/data-ingestion.md — read that file first if this is unfamiliar): classic-db
# is GPLv3 and its CONTENT is asserted to remain Blizzard's, offered only as non-commercial "fair-use
# demo content". This repo ships ZERO of it:
#   - this script NEVER executes any cmangos SQL/code — it only clones the repo and decompresses
#     the SQL it ships into one dump for the importer to PARSE (importer/src/main.rs's own doc:
#     "never executes it" — same rule, same reason: running GPL code is a materially different act
#     than reading its data as text).
#   - everything this script writes lands under .import/, which is gitignored (see .gitignore:
#     "Operator-pulled third-party data (cmangos classic-db) + generated bundles — NEVER committed").
#   - the ONLY things this repository ships are: this script, importer/scripts/classic-db.lock (our
#     own metadata — a commit SHA + checksum, not their data), and the importer that reads the result.
#
# PIN-BY-LOCKFILE, NOT FLOATING (reconciles docs/data-ingestion.md's earlier "track latest master, not
# pinned by default" decision — that decision was about strategy, not this script's default): classic-
# db has no release tags, so bare "master" churns silently. This script defaults to the SHA recorded in
# importer/scripts/classic-db.lock for a REPRODUCIBLE out-of-the-box pull, with sha256 verification that ABORTS
# on mismatch (never silently import a changed shape at the same pinned commit). Override with
# CLASSIC_DB_REF to track master (or any ref) instead — pass --skip-verify too, since the checksum only
# means something for the locked SHA.
#
# Usage (run from the checkout root):
#   importer/scripts/pull-classic-db.sh                        # clone/fetch, checkout the locked SHA, verify
#   CLASSIC_DB_REF=master importer/scripts/pull-classic-db.sh --skip-verify   # float master, skip the pinned checksum
#
# Output: .import/classic-db-full.sql, importer/scripts/import-world.sh's default $DUMP path.
# classic-db ships its world database as ONE gzipped mysqldump under Full_DB/ (today:
# Full_DB/ClassicDB_1_12_1_z<rev>.sql.gz), so "assemble" means: decompress every file the glob
# matches, in glob order, into one plain-text file the importer can parse. The script adds no bytes,
# so the profile digest covers the exact decompressed source. If upstream splits or moves the dump,
# the profile must change before import.
#
# Bump procedure for importer/scripts/classic-db.lock (one line): run
#   CLASSIC_DB_REF=master importer/scripts/pull-classic-db.sh --skip-verify
# then copy its printed "resolved commit" + "sha256" into that lockfile and commit it.
set -uo pipefail
cd "$(dirname "$0")/../.." || exit 1

LOCKFILE="importer/scripts/classic-db.lock"
# shellcheck source=importer/scripts/classic-db.lock
source "$LOCKFILE"

CLASSIC_DB_REPO="${CLASSIC_DB_REPO:-https://github.com/cmangos/classic-db.git}"
CLASSIC_DB_REF="${CLASSIC_DB_REF:-$CLASSIC_DB_SHA}"
CLONE_DIR=".import/classic-db"
OUT_FILE=".import/classic-db-full.sql"
# Where the world dump lives inside the cloned repo — see the header note above. Confirmed against
# cmangos/classic-db at the locked commit: one gzipped mysqldump, Full_DB/ClassicDB_1_12_1_z*.sql.gz.
ASSEMBLE_GLOB="Full_DB/*.sql*"

# --- sha256 verify, factored out so an offline test can exercise it without a network clone --------
# Compares `file`'s sha256 against `expected`; prints OK/FAIL and returns 0/1. NEVER used to decide
# whether to import — that decision belongs to the caller (main, below), which aborts loudly on a
# mismatch rather than silently proceeding with unverified data.
verify_sha256() {
  local file="$1" expected="$2" actual
  if [ ! -f "$file" ]; then
    echo "[pull-classic-db] verify_sha256: no such file: $file" >&2
    return 1
  fi
  actual="$(sha256sum "$file" | awk '{print $1}')"
  if [ "$actual" != "$expected" ]; then
    echo "[pull-classic-db] sha256 MISMATCH for $file" >&2
    echo "  expected: $expected" >&2
    echo "  actual:   $actual" >&2
    return 1
  fi
  echo "[pull-classic-db] sha256 OK: $actual"
  return 0
}

# --- clone/fetch + checkout ---------------------------------------------------------------------
clone_or_fetch() {
  if [ -d "$CLONE_DIR/.git" ]; then
    git -C "$CLONE_DIR" fetch --all --tags
  else
    mkdir -p .import
    git clone "$CLASSIC_DB_REPO" "$CLONE_DIR"
  fi
  git -C "$CLONE_DIR" checkout "$CLASSIC_DB_REF"
}

# --- assemble the single dump file the importer reads --------------------------------------------
# NEVER executes any cmangos SQL. It only concatenates files the importer will parse.
assemble() {
  : > "$OUT_FILE"
  shopt -s nullglob
  # shellcheck disable=SC2206 # intentional glob expansion (ASSEMBLE_GLOB is a pattern, not a
  # literal path) — nullglob is set around it so a no-match yields an empty array, not the pattern.
  local sql_files=("$CLONE_DIR"/$ASSEMBLE_GLOB)
  shopt -u nullglob
  if [ "${#sql_files[@]}" -eq 0 ]; then
    echo "[pull-classic-db] FATAL: nothing matched '$ASSEMBLE_GLOB' under $CLONE_DIR (upstream layout moved? adjust ASSEMBLE_GLOB)" >&2
    return 1
  fi
  local f
  for f in "${sql_files[@]}"; do
    case "$f" in
      *.gz) gzip -dc "$f" >> "$OUT_FILE" ;;
      *) cat "$f" >> "$OUT_FILE" ;;
    esac
  done
  echo "[pull-classic-db] assembled $OUT_FILE from ${#sql_files[@]} file(s)"
}

main() {
  local skip_verify=0
  if [ "${1:-}" = "--skip-verify" ]; then
    skip_verify=1
  fi

  echo "[pull-classic-db] repo=$CLASSIC_DB_REPO ref=$CLASSIC_DB_REF"
  clone_or_fetch
  local resolved_sha
  resolved_sha="$(git -C "$CLONE_DIR" rev-parse HEAD)"
  echo "[pull-classic-db] resolved commit: $resolved_sha"

  assemble || exit 1

  if [ "$skip_verify" = 1 ]; then
    echo "[pull-classic-db] --skip-verify given (expected when CLASSIC_DB_REF floats past the lock) — sha256 not checked"
  elif [ "$CLASSIC_DB_REF" != "$CLASSIC_DB_SHA" ]; then
    echo "[pull-classic-db] CLASSIC_DB_REF ($CLASSIC_DB_REF) differs from the locked SHA — pass --skip-verify to acknowledge (the lock's checksum only means something for the locked SHA)" >&2
    exit 1
  else
    verify_sha256 "$OUT_FILE" "$CLASSIC_DB_SHA256" || {
      echo "[pull-classic-db] ABORT: the assembled dump does not match the locked checksum — the classic-db repo's" >&2
      echo "  output changed shape at the SAME pinned commit. Do not import this file; investigate before trusting" >&2
      echo "  it (this is the exact drift guard the licensing firewall's 'never silently import a changed shape'" >&2
      echo "  promise requires — see docs/data-ingestion.md)." >&2
      exit 1
    }
  fi

  echo "[pull-classic-db] done: $OUT_FILE ready for importer/scripts/import-world.sh (its default \$DUMP path)"
  echo "[pull-classic-db] resolved SHA for provenance: $resolved_sha (pass to \`importer --source-sha $resolved_sha\`)"
}

# Only run `main` when EXECUTED, not when sourced — lets an offline test `source` this file (e.g. to
# call `verify_sha256` directly against a fixture) without triggering a network clone.
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
  main "$@"
fi
