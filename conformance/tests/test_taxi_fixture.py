import importlib.util
from pathlib import Path
import tempfile
import unittest


spec = importlib.util.spec_from_file_location('taxi_fixture', Path(__file__).parents[1] / 'taxi-fixture.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class TaxiFixtureTests(unittest.TestCase):
    def source(self):
        nodes = "INSERT INTO game_taxi_node (id,client_node_id,name) VALUES (2,2,'O''Brien, (node); text'),(4,4,'Destination'),(5,5,'Unrelated');"
        path = 'INSERT INTO game_taxi_path (id,source_node_id,destination_node_id,fare) VALUES (6,2,4,110),(7,4,2,110);'
        points = 'INSERT INTO game_taxi_path_node (id,path_id,node_index,x) VALUES ' + ','.join(f'({35+i},6,{i},{i}.125)' for i in range(28)) + ';'
        creature = "INSERT INTO game_creature_template (entry,name) VALUES (352,'Flight master'),(541,'Mount');"
        return '\n'.join([nodes, path, points, creature]) + '\n'

    def select(self, source):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'import.sql'
            path.write_text(source)
            return fixture.select_rows([path])

    def test_preserves_imported_tuples_and_excludes_other_routes(self):
        result = self.select(self.source())
        self.assertIn("(2,2,'O''Brien, (node); text')", result)
        self.assertIn('(62,6,27,27.125)', result)
        self.assertNotIn('Unrelated', result)
        self.assertNotIn('(7,4,2,110)', result)
        self.assertNotIn("(541,'Mount')", result)

    def test_missing_or_duplicate_route_vertices_fail(self):
        source = self.source()
        for changed in [source.replace(',(62,6,27,27.125)', ''), source.replace('(62,6,27,27.125)', '(62,6,27,27.125),(62,6,27,27.125)')]:
            with self.assertRaisesRegex(ValueError, 'expected 28 imported rows'):
                self.select(changed)

    def test_malformed_selected_input_fails(self):
        with self.assertRaisesRegex(ValueError, 'malformed'):
            self.select(self.source().replace('(7,4,2,110)', '(bad'))
