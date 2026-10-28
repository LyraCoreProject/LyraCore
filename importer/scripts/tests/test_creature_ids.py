from contextlib import closing
import importlib.util
from pathlib import Path
import sqlite3
import unittest

spec = importlib.util.spec_from_file_location(
    'audit_creature_ids', Path(__file__).resolve().parents[1] / 'audit-creature-ids.py')
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class IdentifierBands(unittest.TestCase):
    def test_report_counts_collisions_and_uses_current_maximum(self):
        with closing(sqlite3.connect(':memory:')) as connection:
            connection.executescript('CREATE TABLE creature (guid INTEGER);'
                'CREATE TABLE gameobject (guid INTEGER);'
                'INSERT INTO creature VALUES (10),(5090000);'
                'INSERT INTO gameobject VALUES (3998642),(16000000);')
            report = audit.identifier_bands(connection)
        self.assertIn('| creature | 5090000 | Fixture | 5090000..5099999 | 1 |', report)
        self.assertIn('| gameobject | 16000000 | Package Gameobject | 16000000..16999999 | 1 |', report)
        self.assertIn('| gameobject | 16000000 | Curated, separate key spaces | 1000000..5200499 | 1 |', report)
