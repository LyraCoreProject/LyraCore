from contextlib import closing
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sqlite3
import stat
import tempfile
import unittest
from unittest.mock import patch
import warnings
import zipfile

SPEC = importlib.util.spec_from_file_location("pull_vmangos", Path(__file__).parents[1] / "pull-vmangos.py")
FETCH = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(FETCH)
COMMIT = "abcdef0" + "1" * 33
MIGRATIONS = {"highest_id": "20010102000000", "row_count": 2}


class FetchTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.lock = self.root / "vmangos.lock"
        self.output = self.root / "downloads"
        FETCH.write_json(self.lock, FETCH.snapshot_lock(COMMIT, MIGRATIONS))
        database = self.root / "fixture.sqlite"
        with closing(sqlite3.connect(database)) as connection, connection:
            connection.execute("CREATE TABLE migrations (id TEXT PRIMARY KEY)")
            connection.executemany("INSERT INTO migrations (id) VALUES (?)",
                                   [("20010101000000",), ("20010102000000",)])
        self.sqlite = database.read_bytes()
        self.archive = self.archive_bytes()
        self.release = {
            "id": 42, "tag_name": "db_latest", "assets": [{
                "id": 99, "name": "db-sqlite-abcdef0.zip", "size": len(self.archive),
                "digest": "sha256:" + hashlib.sha256(self.archive).hexdigest(),
            }],
        }

    def archive_bytes(self, extra=None):
        result = io.BytesIO()
        with zipfile.ZipFile(result, "w", zipfile.ZIP_DEFLATED) as archive:
            archive.writestr(FETCH.SQLITE_MEMBER, self.sqlite)
            if extra is not None:
                archive.writestr(extra, b"must not be extracted")
        return result.getvalue()

    def network(self, archive=None, commit=COMMIT):
        payloads = {
            f"{FETCH.API}/releases/tags/db_latest": json.dumps(self.release).encode(),
            f"{FETCH.API}/commits/abcdef0": json.dumps({"sha": commit}).encode(),
            f"{FETCH.API}/releases/assets/99": self.archive if archive is None else archive,
        }
        return patch.object(FETCH, "request", side_effect=lambda url, _accept: io.BytesIO(payloads[url]))

    def test_fetch_checks_digest_and_records_commit_and_migrations(self):
        with self.network():
            result = FETCH.fetch(self.output, self.lock)
        self.assertEqual(Path(result["sqlite_path"]).read_bytes(), self.sqlite)
        self.assertEqual(result["migrations"], MIGRATIONS)
        self.assertEqual(result["commit"], COMMIT)
        self.assertTrue(result["matches_accepted_lock"])
        self.assertEqual(result["source_profile_gate"], "not evaluated by fetch")
        self.assertEqual(json.loads((self.output / "current.json").read_text()), result)
        self.assertFalse(list(self.output.glob(".fetch-*")))
        with self.network():
            self.assertEqual(FETCH.fetch(self.output, self.lock), result)

    def test_a_changed_snapshot_is_reported_without_accepting_it(self):
        old = FETCH.snapshot_lock("0" * 40, {"highest_id": "20010101000000", "row_count": 1})
        FETCH.write_json(self.lock, old)
        with self.network():
            report = FETCH.fetch(self.output, self.lock)
        self.assertFalse(report["matches_accepted_lock"])
        self.assertEqual(FETCH.read_lock(self.lock), old)
        with self.network():
            FETCH.fetch(self.output, self.lock, accept_lock=True)
        accepted = FETCH.read_lock(self.lock)
        self.assertEqual(accepted, FETCH.snapshot_lock(COMMIT, MIGRATIONS))
        self.assertNotIn("asset_digest", accepted)
        self.assertNotIn("sqlite_sha256", accepted)

    def test_repacked_bytes_do_not_change_snapshot_acceptance(self):
        self.archive = self.archive_bytes(extra="unused.txt")
        self.release["assets"][0].update(size=len(self.archive), digest="sha256:" + hashlib.sha256(self.archive).hexdigest())
        with self.network():
            result = FETCH.fetch(self.output, self.lock)
        self.assertTrue(result["matches_accepted_lock"])
        self.assertFalse((Path(result["sqlite_path"]).parent / "unused.txt").exists())

    def test_digest_mismatch_keeps_previous_metadata_and_lock(self):
        self.output.mkdir()
        current = self.output / "current.json"
        current.write_text("previous snapshot")
        lock_before = self.lock.read_bytes()
        corrupt = self.archive[:-1] + bytes([self.archive[-1] ^ 1])
        with self.network(archive=corrupt), self.assertRaisesRegex(ValueError, "SHA-256 digest"):
            FETCH.fetch(self.output, self.lock, accept_lock=True)
        self.assertEqual(current.read_text(), "previous snapshot")
        self.assertEqual(self.lock.read_bytes(), lock_before)
        self.assertEqual(list(self.output.iterdir()), [current])

    def test_verified_archive_still_requires_a_readable_sqlite_snapshot(self):
        self.sqlite = b"not a SQLite database"
        self.archive = self.archive_bytes()
        self.release["assets"][0].update(size=len(self.archive), digest="sha256:" + hashlib.sha256(self.archive).hexdigest())
        accepted = self.lock.read_bytes()
        with self.network(), self.assertRaises(sqlite3.Error):
            FETCH.fetch(self.output, self.lock, accept_lock=True)
        self.assertEqual(self.lock.read_bytes(), accepted)
        self.assertEqual(list(self.output.iterdir()), [])

    def test_truncated_and_oversized_inputs_fail_without_publishing(self):
        with self.network(archive=self.archive[:-1]), self.assertRaisesRegex(ValueError, "shorter"):
            FETCH.fetch(self.output, self.lock)
        self.assertEqual(list(self.output.iterdir()), [])
        with self.network(), patch.object(FETCH, "MAX_SQLITE_BYTES", 1), self.assertRaisesRegex(ValueError, "exceeds"):
            FETCH.fetch(self.output, self.lock)
        self.assertEqual(list(self.output.iterdir()), [])

    def test_missing_digest_and_mismatched_commit_fail_before_publishing(self):
        with self.network(commit="0" * 40), self.assertRaisesRegex(ValueError, "matching full commit"):
            FETCH.fetch(self.output, self.lock)
        self.release["assets"][0]["digest"] = None
        with self.network(), self.assertRaisesRegex(ValueError, "no GitHub SHA-256"):
            FETCH.fetch(self.output, self.lock)
        self.assertFalse(self.output.exists())

    def test_only_the_named_sqlite_member_is_extracted(self):
        archive = self.root / "input.zip"
        archive.write_bytes(self.archive_bytes(extra="../../outside"))
        destination = self.root / "result.sqlite"
        FETCH.extract_sqlite(archive, destination)
        self.assertEqual(destination.read_bytes(), self.sqlite)
        self.assertFalse((self.root.parent / "outside").exists())
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            archive.write_bytes(self.archive_bytes(extra=FETCH.SQLITE_MEMBER))
        with self.assertRaisesRegex(ValueError, "exactly one"):
            FETCH.extract_sqlite(archive, self.root / "duplicate.sqlite")

    def test_a_sqlite_member_cannot_be_a_symlink(self):
        archive = self.root / "link.zip"
        member = zipfile.ZipInfo(FETCH.SQLITE_MEMBER)
        member.create_system = 3
        member.external_attr = (stat.S_IFLNK | 0o777) << 16
        with zipfile.ZipFile(archive, "w") as output:
            output.writestr(member, "../../outside")
        with self.assertRaisesRegex(ValueError, "not a file"):
            FETCH.extract_sqlite(archive, self.root / "result.sqlite")

    def test_migrations_must_be_a_real_table_with_timestamp_ids(self):
        view = self.root / "view.sqlite"
        with closing(sqlite3.connect(view)) as connection, connection:
            connection.execute("CREATE TABLE source (id TEXT)")
            connection.execute("CREATE VIEW migrations AS SELECT id FROM source")
        with self.assertRaisesRegex(ValueError, "ordinary migrations table"):
            FETCH.migration_fingerprint(view)
        malformed = self.root / "malformed.sqlite"
        with closing(sqlite3.connect(malformed)) as connection, connection:
            connection.execute("CREATE TABLE migrations (id INTEGER)")
            connection.execute("INSERT INTO migrations (id) VALUES (1)")
        with self.assertRaisesRegex(ValueError, "14-digit text"):
            FETCH.migration_fingerprint(malformed)
        missing = self.root / "missing.sqlite"
        with self.assertRaises(sqlite3.Error):
            FETCH.migration_fingerprint(missing)
        self.assertFalse(missing.exists())

    def test_lock_refuses_extra_fields_and_invalid_shapes(self):
        valid = FETCH.snapshot_lock(COMMIT, MIGRATIONS)
        for invalid in [[], {}, {**valid, "sha256": "0" * 64}, {**valid, "commit": None},
                        {**valid, "format_version": True}, {**valid, "migrations": []}]:
            with self.subTest(invalid=invalid):
                FETCH.write_json(self.lock, invalid)
                with self.assertRaisesRegex(ValueError, "invalid vmangos snapshot lock"):
                    FETCH.read_lock(self.lock)


if __name__ == "__main__":
    unittest.main()
