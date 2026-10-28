#!/usr/bin/env python3

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import stat
import sys
import tempfile
from urllib.error import HTTPError
from urllib.request import Request, urlopen
import zipfile

API = "https://api.github.com/repos/vmangos/core"
REPOSITORY = "vmangos/core"
SQLITE_MEMBER = "sqlite-dump/mangos.sqlite"
MAX_ARCHIVE_BYTES = 512 * 1024 * 1024
MAX_SQLITE_BYTES = 2 * 1024 * 1024 * 1024
SCRIPT_PATH = Path(__file__).resolve()
DEFAULT_LOCK = SCRIPT_PATH.with_name("vmangos.lock")
DEFAULT_OUTPUT = SCRIPT_PATH.parents[2] / ".import" / "vmangos"


def request(url, accept):
    return urlopen(Request(url, headers={
        "Accept": accept,
        "User-Agent": "LyraCore-world-source-fetch",
        "X-GitHub-Api-Version": "2022-11-28",
    }), timeout=60)


def api_json(path):
    with request(f"{API}/{path}", "application/vnd.github+json") as response:
        payload = response.read(4 * 1024 * 1024 + 1)
    if len(payload) > 4 * 1024 * 1024:
        raise ValueError("GitHub metadata exceeds 4 MiB")
    value = json.loads(payload)
    if not isinstance(value, dict):
        raise ValueError("GitHub metadata must be an object")
    return value


def sqlite_asset(release):
    if release.get("tag_name") != "db_latest" or not isinstance(release.get("assets"), list):
        raise ValueError("GitHub did not return the db_latest release")
    assets = [asset for asset in release["assets"]
              if isinstance(asset, dict) and isinstance(asset.get("name"), str)
              and re.fullmatch(r"db-sqlite-[0-9a-f]{7,40}\.zip", asset["name"])]
    if len(assets) != 1:
        raise ValueError("db_latest must contain exactly one db-sqlite-<commit>.zip asset")
    asset = assets[0]
    if (not isinstance(asset.get("digest"), str)
            or not re.fullmatch(r"sha256:[0-9a-f]{64}", asset["digest"])):
        raise ValueError("SQLite asset has no GitHub SHA-256 digest")
    if type(asset.get("id")) is not int or asset["id"] <= 0:
        raise ValueError("SQLite asset has an invalid id")
    if type(asset.get("size")) is not int or not 0 < asset["size"] <= MAX_ARCHIVE_BYTES:
        raise ValueError("SQLite asset size is invalid or exceeds 512 MiB")
    return asset


def digest(path):
    sha = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            sha.update(chunk)
    return sha.hexdigest()


def download(asset, destination):
    with request(f"{API}/releases/assets/{asset['id']}", "application/octet-stream") as source:
        with destination.open("xb") as output:
            received = 0
            while chunk := source.read(1024 * 1024):
                received += len(chunk)
                if received > asset["size"]:
                    raise ValueError("SQLite archive exceeds the GitHub asset size")
                output.write(chunk)
    if received != asset["size"]:
        raise ValueError("SQLite archive is shorter than the GitHub asset size")
    if digest(destination) != asset["digest"].removeprefix("sha256:"):
        raise ValueError("SQLite archive does not match the GitHub SHA-256 digest")


def extract_sqlite(archive_path, destination):
    with zipfile.ZipFile(archive_path) as archive:
        members = [member for member in archive.infolist() if member.filename == SQLITE_MEMBER]
        if len(members) != 1:
            raise ValueError(f"archive must contain exactly one {SQLITE_MEMBER}")
        member = members[0]
        mode = member.external_attr >> 16
        if (member.is_dir() or stat.S_ISLNK(mode) or member.flag_bits & 1
                or not 0 < member.file_size <= MAX_SQLITE_BYTES):
            raise ValueError("SQLite archive member is not a file or exceeds 2 GiB")
        with archive.open(member) as source, destination.open("xb") as output:
            shutil.copyfileobj(source, output, length=1024 * 1024)
    if destination.stat().st_size != member.file_size:
        raise ValueError("SQLite archive member is truncated")


def migration_fingerprint(path):
    connection = sqlite3.connect(f"{path.resolve().as_uri()}?mode=ro", uri=True)
    connection.row_factory = sqlite3.Row
    try:
        connection.execute("PRAGMA trusted_schema = OFF")
        connection.execute("PRAGMA query_only = ON")
        table = connection.execute(
            "SELECT type FROM pragma_table_list WHERE schema = 'main' AND name = 'migrations'"
        ).fetchone()
        if table is None or table["type"] != "table":
            raise ValueError("snapshot has no ordinary migrations table")
        highest = None
        count = 0
        for row in connection.execute("SELECT id FROM migrations"):
            value = row["id"]
            if not isinstance(value, str) or not re.fullmatch(r"[0-9]{14}", value):
                raise ValueError("migration ids must be nonempty 14-digit text values")
            highest = value if highest is None else max(highest, value)
            count += 1
        if highest is None:
            raise ValueError("migration ids must be nonempty 14-digit text values")
        return {"highest_id": highest, "row_count": count}
    finally:
        connection.close()


def snapshot_lock(commit, migrations):
    return {"format_version": 1, "repository": REPOSITORY, "commit": commit,
            "migrations": migrations}


def read_lock(path):
    value = json.loads(path.read_text())
    if not isinstance(value, dict) or not isinstance(value.get("migrations"), dict):
        raise ValueError(f"{path}: invalid vmangos snapshot lock")
    migrations = value["migrations"]
    if (type(value.get("format_version")) is not int or value["format_version"] != 1 or value.get("repository") != REPOSITORY
            or not isinstance(value.get("commit"), str)
            or not re.fullmatch(r"[0-9a-f]{40}", value["commit"])
            or not isinstance(migrations.get("highest_id"), str)
            or not re.fullmatch(r"[0-9]{14}", migrations["highest_id"])
            or type(migrations.get("row_count")) is not int or migrations["row_count"] <= 0
            or set(value) != {"format_version", "repository", "commit", "migrations"}
            or set(migrations) != {"highest_id", "row_count"}):
        raise ValueError(f"{path}: invalid vmangos snapshot lock")
    return value


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, prefix=".snapshot-",
                                         delete=False) as output:
            temporary = Path(output.name)
            json.dump(value, output, indent=2, sort_keys=True)
            output.write("\n")
        os.replace(temporary, path)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def compare_lock(accepted, actual, accept_lock=False):
    if accepted is None:
        return "unlocked"
    if accepted == actual:
        return "matches"
    if accepted["commit"] == actual["commit"]:
        return "migration fingerprint changed"
    try:
        comparison = api_json(f"compare/{accepted['commit']}...{actual['commit']}")
    except HTTPError as error:
        error.close()
        if not accept_lock or error.code != 404:
            raise
        return "previous lock unreachable"
    status = comparison.get("status")
    relations = {"ahead": "newer", "behind": "older", "diverged": "diverged"}
    if status not in relations:
        raise ValueError(f"GitHub returned an invalid snapshot comparison: {status!r}")
    return relations[status]


def fetch(output, lock_path=DEFAULT_LOCK, accept_lock=False):
    accepted = read_lock(lock_path) if lock_path.exists() else None
    if accepted is None and not accept_lock:
        raise ValueError(f"{lock_path}: missing accepted snapshot lock")
    release = api_json("releases/tags/db_latest")
    asset = sqlite_asset(release)
    short_commit = asset["name"][len("db-sqlite-"):-len(".zip")]
    commit = api_json(f"commits/{short_commit}")["sha"]
    if (not isinstance(commit, str) or not re.fullmatch(r"[0-9a-f]{40}", commit)
            or not commit.startswith(short_commit)):
        raise ValueError("asset commit did not resolve to a matching full commit")
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".fetch-", dir=output) as scratch:
        staging = Path(scratch)
        archive = staging / asset["name"]
        sqlite = staging / "mangos.sqlite"
        download(asset, archive)
        extract_sqlite(archive, sqlite)
        migrations = migration_fingerprint(sqlite)
        actual = snapshot_lock(commit, migrations)
        comparison = compare_lock(accepted, actual, accept_lock)
        sqlite_digest = digest(sqlite)
        destination = output / f"{commit}-{asset['digest'][7:23]}"
        if destination.exists():
            if (digest(destination / archive.name) != asset["digest"][7:]
                    or digest(destination / sqlite.name) != sqlite_digest):
                raise ValueError(f"{destination}: cached snapshot differs from the verified download")
        else:
            os.rename(staging, destination)
        if accept_lock:
            write_json(lock_path, actual)
            accepted = actual
        report = {
            **actual,
            "release_id": release["id"],
            "asset_id": asset["id"],
            "asset_name": asset["name"],
            "asset_digest": asset["digest"],
            "sqlite_sha256": sqlite_digest,
            "sqlite_path": str(destination / "mangos.sqlite"),
            "archive_path": str(destination / asset["name"]),
            "matches_accepted_lock": accepted == actual,
            "comparison_to_previous_lock": comparison,
            "source_profile_gate": "not evaluated by fetch",
        }
        write_json(output / "current.json", report)
        return report


def main():
    parser = argparse.ArgumentParser(description="Fetch the current vmangos SQLite snapshot without executing source SQL.")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--lock", type=Path, default=DEFAULT_LOCK)
    parser.add_argument("--accept-lock", action="store_true",
                        help="replace the accepted commit and migration fingerprint after verification")
    args = parser.parse_args()
    try:
        print(json.dumps(fetch(args.output, args.lock, args.accept_lock), indent=2, sort_keys=True))
    except (OSError, ValueError, KeyError, sqlite3.Error, zipfile.BadZipFile) as error:
        print(f"vmangos fetch failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
