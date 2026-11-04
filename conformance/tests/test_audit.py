import copy
import importlib.util
import io
import json
import os
import re
from pathlib import Path
import shutil
import sys
import tempfile
import threading
import unittest
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
SPEC = importlib.util.spec_from_file_location('audit', os.environ.get(
    'LYRACORE_AUDIT_TEST_SOURCE', Path(__file__).parents[1] / 'audit.py'))
audit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(audit)
audit.SCRATCH = Path(os.environ.get("LYRACORE_AUDIT_SCRATCH", audit.SCRATCH))


class WorldReadiness(unittest.TestCase):
    def test_closed_listener_can_be_reused_with_connections_in_time_wait(self):
        with audit.socket.socket() as listener:
            listener.setsockopt(audit.socket.SOL_SOCKET, audit.socket.SO_REUSEADDR, 1)
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
            listener.listen(1)
            with audit.socket.create_connection(('127.0.0.1', port)) as client:
                connection, _ = listener.accept()
                connection.close()
                self.assertEqual(client.recv(1), b'')
        audit.free_port(port)

    def test_live_listener_is_never_considered_free(self):
        with audit.socket.socket() as listener:
            listener.setsockopt(audit.socket.SOL_SOCKET, audit.socket.SO_REUSEADDR, 1)
            listener.bind(('127.0.0.1', 0))
            listener.listen(1)
            with self.assertRaisesRegex(RuntimeError, 'unavailable'):
                audit.free_port(listener.getsockname()[1])

    def test_accept_without_an_auth_challenge_is_not_ready(self):
        early = MagicMock()
        early.__enter__.return_value.recv.return_value = b''
        ready = MagicMock()
        ready.__enter__.return_value.recv.side_effect = [b'\x00', b'\x06\xec\x01']
        with patch.object(audit.socket, 'create_connection', side_effect=[early, ready]) as connect, \
                patch.object(audit.time, 'sleep'):
            audit.wait_port(18085, world=True)
        self.assertEqual(connect.call_count, 2)


class ClientDataPath(unittest.TestCase):
    def test_relative_client_data_is_resolved_before_the_audit_changes_directory(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            client_data = Path(directory) / 'client/Data'
            client_data.mkdir(parents=True)
            relative = os.path.relpath(client_data, Path.cwd())
            arguments = ['--date', '2099-01-01', '--output', str(Path(directory) / 'reports'),
                         '--client-data', relative]
            with patch.dict(audit.os.environ, LYRACORE_AUDIT_ARGS=json.dumps(arguments)), \
                    patch.object(audit, 'scenario_main', return_value=0) as run, \
                    patch.object(audit, 'check_spacetime_version'):
                self.assertEqual(audit.main(), 0)
            self.assertEqual(run.call_args.args[0].client_data, client_data.resolve())


class ModuleSchema(unittest.TestCase):
    def schema(self):
        return {'V10': {'sections': [
            {'Typespace': {'types': [
                {'Product': {'elements': [{'name': {'some': 'id'}, 'algebraic_type': {'U64': []}},
                                         {'name': {'some': 'value'}, 'algebraic_type': {'U32': []}}]}},
                {'Sum': {'variants': [{'name': {'some': 'row'}, 'algebraic_type': {'Ref': 0}},
                                     {'name': {'some': 'empty'}, 'algebraic_type': {'U8': []}}]}}]}},
            {'Types': [{'source_name': {'scope': [], 'source_name': 'Row'}, 'ty': 0},
                       {'source_name': {'scope': [], 'source_name': 'Choice'}, 'ty': 1}]},
            {'Tables': [{'source_name': 'rows', 'product_type_ref': 0, 'primary_key': [0],
                         'default_values': [{'col_id': 1, 'value': '00000000'}],
                         'table_access': {'Private': []}, 'is_event': False}]},
            {'Reducers': [{'source_name': 'put', 'params': {'elements': [
                {'name': {'some': 'row'}, 'algebraic_type': {'Ref': 0}}]}}]},
            {'Schedules': []}, {'LifeCycleReducers': []}, {'ExplicitNames': {'entries': []}}]}}

    def test_default_access_and_event_changes_invalidate_snapshot(self):
        original = self.schema()
        before = audit.schema_fingerprint(original)
        for field, value in [('default_values', [{'col_id': 1, 'value': '01000000'}]),
                             ('table_access', {'Public': []}), ('is_event', True)]:
            with self.subTest(field=field):
                changed = copy.deepcopy(original)
                changed['V10']['sections'][2]['Tables'][0][field] = value
                self.assertNotEqual(before, audit.schema_fingerprint(changed))

    def test_definition_order_and_type_numbers_do_not_change_schema(self):
        original = self.schema()
        changed = copy.deepcopy(original)
        sections = changed['V10']['sections']
        sections[0]['Typespace']['types'].reverse()
        sections[0]['Typespace']['types'][0]['Sum']['variants'][0]['algebraic_type']['Ref'] = 1
        for item in sections[1]['Types']:
            item['ty'] = 1 - item['ty']
        sections[1]['Types'].reverse()
        sections[2]['Tables'][0]['product_type_ref'] = 1
        sections[3]['Reducers'][0]['params']['elements'][0]['algebraic_type']['Ref'] = 1
        sections.reverse()
        self.assertEqual(audit.schema_fingerprint(original), audit.schema_fingerprint(changed))

    def test_column_and_variant_order_remain_part_of_schema(self):
        original = self.schema()
        for index, kind, ordered in [(0, 'Product', 'elements'), (1, 'Sum', 'variants')]:
            with self.subTest(kind=kind):
                changed = copy.deepcopy(original)
                changed['V10']['sections'][0]['Typespace']['types'][index][kind][ordered].reverse()
                self.assertNotEqual(audit.schema_fingerprint(original), audit.schema_fingerprint(changed))

    def test_only_new_private_tables_are_compatible(self):
        previous = audit.canonical_schema(self.schema())
        current = copy.deepcopy(previous)
        current['Tables'].append(dict(current['Tables'][0], source_name='companion'))
        self.assertTrue(audit.compatible_schema(previous, current))
        for table_access, is_event in [({'Public': []}, False), ({'Private': []}, True)]:
            with self.subTest(table_access=table_access, is_event=is_event):
                changed = copy.deepcopy(current)
                changed['Tables'][-1].update(table_access=table_access, is_event=is_event)
                self.assertFalse(audit.compatible_schema(previous, changed))

    def test_stored_layout_and_initialization_changes_are_incompatible(self):
        previous = audit.canonical_schema(self.schema())
        for section, replacement in [
                ('Tables', []), ('Typespace', {}),
                ('Schedules', [{'table_name': 'rows'}]),
                ('LifeCycleReducers', [{'function_name': 'another_init'}])]:
            with self.subTest(section=section):
                current = copy.deepcopy(previous)
                current[section] = replacement
                self.assertFalse(audit.compatible_schema(previous, current))
        for field, value in [('default_values', []), ('table_access', {'Public': []}),
                             ('is_event', True)]:
            with self.subTest(field=field):
                current = copy.deepcopy(previous)
                current['Tables'][0][field] = value
                self.assertFalse(audit.compatible_schema(previous, current))
        current = copy.deepcopy(previous)
        row = next(value for value in current['Typespace'].values() if 'Product' in value)
        row['Product']['elements'].append(dict(name={'some': 'cell'}, algebraic_type={'I64': []}))
        self.assertFalse(audit.compatible_schema(previous, current))


class ImportModuleCoverage(unittest.TestCase):
    def test_reachable_modules_are_hashed(self):
        hashed = set()
        for path in audit.import_contract_paths():
            hashed.update(path.rglob('*') if path.is_dir() else [path])
        pending = [audit.ROOT / name for name in audit.IMPORT_CONTRACT
                   if name.startswith('module/src/') and name.endswith('.rs')
                   and name != 'module/src/lib.rs']
        pending.extend(path for path in hashed if path.suffix == '.rs'
                       and path.is_relative_to(audit.ROOT / 'module/src')
                       and path.name != 'lib.rs')
        visited = set()
        while pending:
            path = pending.pop()
            if path in visited:
                continue
            visited.add(path)
            source = re.sub(r'/\*.*?\*/|//[^\n]*', '', path.read_text(), flags=re.DOTALL)
            for declaration in re.finditer(
                    r'((?:#\[[^\n]+\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;',
                    source):
                attributes, name = declaration.groups()
                explicit = re.search(r'path\s*=\s*"([^"]+)"', attributes)
                directory = path.parent if path.name == 'mod.rs' else path.with_suffix('')
                candidates = ([path.parent / explicit[1]] if explicit else
                              [directory / f'{name}.rs', directory / name / 'mod.rs'])
                children = [child for child in candidates if child.is_file()]
                self.assertEqual(len(children), 1, f'{path}: module {name}')
                child = children[0].resolve()
                self.assertIn(child, hashed, f'unhashed child module of {path}')
                pending.append(child)

    def test_coverage_check_detects_an_unhashed_seed_child(self):
        paths = [path for path in audit.import_contract_paths()
                 if path != audit.ROOT / 'module/src/seed']
        with patch.object(audit, 'import_contract_paths', return_value=paths):
            with self.assertRaisesRegex(AssertionError, 'unhashed child module'):
                self.test_reachable_modules_are_hashed()

    def test_child_directories_and_external_module_paths_are_hashed_recursively(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            (root / 'seed.rs').write_text('mod fixtures;')
            (root / 'seed').mkdir()
            (root / 'seed/fixtures.rs').write_text('#[path = "../shared.rs"]\nmod shared;')
            (root / 'shared.rs').write_text('#[path = "external/mod.rs"]\nmod external;')
            (root / 'external').mkdir()
            (root / 'external/mod.rs').write_text('mod leaf;')
            (root / 'external/leaf.rs').write_text('original')
            (root / 'shared').mkdir()
            nested = root / 'shared/nested.rs'
            nested.write_text('original')
            with patch.object(audit, 'ROOT', root), \
                    patch.object(audit, 'IMPORT_CONTRACT', ('seed.rs',)):
                before = audit.tree_hash(audit.import_contract_paths())
                for path in [nested, root / 'external/leaf.rs']:
                    path.write_text('changed')
                    self.assertNotEqual(before, audit.tree_hash(audit.import_contract_paths()))
                    path.write_text('original')


class ScenarioKey(unittest.TestCase):
    def test_only_import_inputs_invalidate_snapshot(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            harness = root / 'harness'
            adapter = harness / 'adapters/vmangos'
            adapter.mkdir(parents=True)
            pins = {'database_commit': 'a' * 40}
            (adapter / 'pins.json').write_text(json.dumps(pins))
            (root / '.lyracore-cli-rev').write_text('cli')
            importer = root / 'importer/src'
            importer.mkdir(parents=True)
            (importer / 'lib.rs').write_text('first')
            (root / 'importer/scripts').mkdir()
            (root / 'importer/manifests').mkdir()
            (root / 'importer/Cargo.toml').write_text('')
            gameplay = root / 'module/src/taxi.rs'
            gameplay.parent.mkdir(parents=True)
            gameplay.write_text('first gameplay commit')
            for name in ['Cargo.lock', 'crates/lyracore-shared/src/nav.rs',
                         'module/src/seed.rs', 'module/src/seed/fixtures.rs',
                         'module/src/operations.rs', 'module/src/package_import/creatures.rs']:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('original')
            client = root / 'Data'
            client.mkdir()
            (client / 'dbc.MPQ').write_bytes(b'first')
            for name in getattr(audit, 'IMPORT_CONTRACT', []):
                path = root / name
                if not path.exists():
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text('contract input')
            with patch.object(audit, 'ROOT', root), patch.object(audit, 'SCENARIO_CACHE', root):
                first, _ = audit.scenario_key(harness, client)
                self.assertEqual(first, audit.scenario_key(harness, client)[0])
                gameplay.write_text('second gameplay commit')
                self.assertEqual(first, audit.scenario_key(harness, client)[0])
                for name in ['Cargo.lock', 'crates/lyracore-shared/src/nav.rs',
                             'module/src/seed.rs', 'module/src/seed/fixtures.rs',
                             'module/src/operations.rs', 'module/src/package_import/creatures.rs',
                             'importer/src/game_quest_flags.rs']:
                    with self.subTest(import_input=name):
                        path = root / name
                        path.write_text('changed')
                        self.assertNotEqual(first, audit.scenario_key(harness, client)[0])
                        if name == 'importer/src/game_quest_flags.rs':
                            path.unlink()
                        else:
                            path.write_text('original')
                (client / 'dbc.MPQ').write_bytes(b'other')
                changed_client, _ = audit.scenario_key(harness, client)
                pins['database_commit'] = 'b' * 40
                (adapter / 'pins.json').write_text(json.dumps(pins))
                changed_pin, _ = audit.scenario_key(harness, client)
                (importer / 'lib.rs').write_text('changed')
                changed_importer, _ = audit.scenario_key(harness, client)
                (root / '.lyracore-cli-rev').write_text('new cli')
                changed_cli, _ = audit.scenario_key(harness, client)
            self.assertNotEqual(first, changed_client)
            self.assertNotEqual(changed_client, changed_pin)
            self.assertNotEqual(changed_pin, changed_importer)
            self.assertNotEqual(changed_importer, changed_cli)


class ImportedWorld(unittest.TestCase):
    def test_restore_discards_previous_scenario_changes_before_restart(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            realm = root / 'core'
            native_data = realm / '.lyracore/native-data'
            native_data.mkdir(parents=True)
            seed = root / 'imported-world'
            seed.mkdir()
            (seed / 'world').write_text('imported')
            (native_data / 'world').write_text('scenario changed')
            calls = []

            def run(args, **_):
                calls.append(args[1:])

            with patch.object(audit, 'run', side_effect=run), patch.object(audit, 'free_port'):
                audit.restore_imported_world(realm, seed, realm / 'lyracore', {})
            self.assertEqual((native_data / 'world').read_text(), 'imported')
            self.assertEqual(calls, [['dev', 'down'], ['dev', 'up', '--single']])


class IncompleteScenarioCache(unittest.TestCase):
    def test_stop_removes_owned_interrupted_worktree_without_cli(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            cache = Path(directory)
            (cache / 'realm/core/.lyracore').mkdir(parents=True)
            (cache / 'realm/state.json').write_text(json.dumps(dict(phase='starting', key='test',
                                                                  core_commit='test')))
            with patch.object(audit, 'SCENARIO_CACHE', cache), patch.object(audit, 'free_port'):
                audit.stop_scenario_cache()
            self.assertFalse((cache / 'realm').exists())


class NamedCharacters(unittest.TestCase):
    def test_capture_passes_one_account_per_character(self):
        scenario = {'id': 'two_character_visibility', 'characters': ['Watchera', 'Arrivera']}
        with patch.object(audit, 'run') as run:
            audit.capture('wire', Path('/scratch'), scenario, 'lyracore', [3724, 8085],
                          b'first\nsecond\n', {})
        self.assertEqual(run.call_args.kwargs['input'], b'first\nsecond\n')
        self.assertEqual(run.call_args.kwargs['env']['WIRE_PEER_ACCOUNTS'], 'CONFORMANCE2')
        self.assertEqual(run.call_args.args[0][-2:], ['CONFORMANCE', '--password-stdin'])


class CargoPin(unittest.TestCase):
    def setUp(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix='audit-pin-', dir=audit.SCRATCH)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.workspace = self.root / 'workspace'
        self.member = self.workspace / 'member'
        self.home = self.root / 'cargo-home'
        self.checkout = self.home / 'git/checkouts/harness/revision'
        self.member.mkdir(parents=True)
        self.checkout.mkdir(parents=True)
        for path in [self.workspace, self.member, self.checkout]:
            (path / 'Cargo.toml').write_text('')
        self.git('init', '--quiet')
        self.git('add', '.')
        self.git('-c', 'commit.gpgsign=false', 'commit', '--quiet', '-m', 'fixture')
        self.revision = self.git('rev-parse', 'HEAD')
        self.metadata = dict(workspace_root=str(self.workspace), workspace_members=['member'],
            resolve=dict(nodes=[dict(id='harness')]), packages=[
                dict(id='member', name='member', manifest_path=str(self.member / 'Cargo.toml')),
                dict(id='harness', name='wire-client', manifest_path=str(self.checkout / 'Cargo.toml'),
                     source=f'git+https://github.com/LyraCoreProject/wire-harness?rev={self.revision}#{self.revision}')])

    def git(self, *args):
        env = {key: value for key, value in audit.os.environ.items()
               if key not in {'GIT_DIR', 'GIT_INDEX_FILE', 'GIT_WORK_TREE'}}
        env.update(GIT_AUTHOR_NAME='Fixture', GIT_COMMITTER_NAME='Fixture',
                   GIT_AUTHOR_EMAIL='fixture@example.invalid', GIT_COMMITTER_EMAIL='fixture@example.invalid')
        return audit.run(['git', *args], cwd=self.checkout, env=env, capture_output=True, text=True).stdout.strip()

    def check(self):
        return audit.pinned_harness(self.metadata, self.revision, self.home)

    def test_every_config_location_refuses_sources_and_replace_with(self):
        self.assertEqual(self.check(), self.checkout)
        for directory in [self.root / '.cargo', self.workspace / '.cargo', self.member / '.cargo', self.home]:
            directory.mkdir(exist_ok=True)
            for filename in ['config', 'config.toml', 'config.backup']:
                path = directory / filename
                for contents in ["[source.vend]\ndirectory = 'vendor'\n",
                                 "[source.pinned]\nreplace-with = 'vend'\n",
                                 "[build]\nreplace-with = 'vend'\n", "paths = ['fork']\n"]:
                    with self.subTest(path=path, contents=contents):
                        path.write_text(contents)
                        with self.assertRaisesRegex(RuntimeError, 'forbidden'):
                            self.check()
                path.unlink()

    def test_pinned_metadata_cannot_hide_another_checkout_head(self):
        self.git('-c', 'commit.gpgsign=false', 'commit', '--quiet', '--allow-empty', '-m', 'changed')
        with self.assertRaisesRegex(RuntimeError, 'HEAD differs'):
            self.check()

    def test_pinned_metadata_cannot_hide_a_directory_source_or_symlink(self):
        manifest = self.root / 'vendored.toml'
        manifest.write_text('')
        link = self.checkout / 'symlink.toml'
        link.symlink_to(manifest)
        for path in [manifest, link]:
            with self.subTest(path=path):
                self.metadata['packages'][1]['manifest_path'] = str(path)
                with self.assertRaisesRegex(RuntimeError, 'outside Cargo git checkouts'):
                    self.check()


class HostExit(unittest.TestCase):
    def setUp(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=audit.SCRATCH)
        self.addCleanup(temporary.cleanup)
        self.realm = Path(temporary.name)
        (self.realm / '.lyracore').mkdir()
        (self.realm / '.lyracore/state.json').write_text(json.dumps(
            {'spacetime': {'pid': 42, 'identity': 'started host'}, 'gateway': None}))

    def test_restore_waits_for_owned_host_exit_before_copy(self):
        seed = self.realm / 'seed'
        seed.mkdir()
        (seed / 'world').write_text('clean')
        native = self.realm / '.lyracore/native-data'
        native.mkdir()
        (native / 'world').write_text('changed')
        identities = iter(['started host', 'started host', None])
        with patch.object(audit, 'process_identity', side_effect=lambda _: next(identities)), \
                patch.object(audit, 'free_port'), patch.object(audit.time, 'sleep'), \
                patch.object(audit, 'run') as run:
            audit.restore_imported_world(self.realm, seed, self.realm / 'lyracore', {})
        self.assertEqual((native / 'world').read_text(), 'clean')
        self.assertEqual([call.args[0][2] for call in run.call_args_list], ['down', 'up'])

    def test_timed_out_host_keeps_data_untouched(self):
        seed = self.realm / 'seed'
        seed.mkdir()
        (seed / 'world').write_text('clean')
        native = self.realm / '.lyracore/native-data'
        native.mkdir()
        (native / 'world').write_text('changed')
        with patch.object(audit, 'process_identity', return_value='started host'), \
                patch.object(audit.time, 'monotonic', side_effect=[0, 121]), \
                patch.object(audit, 'run'), patch.object(audit.time, 'sleep'):
            with self.assertRaisesRegex(RuntimeError, 'did not exit'):
                audit.restore_imported_world(self.realm, seed, self.realm / 'lyracore', {})
        self.assertEqual((native / 'world').read_text(), 'changed')

    def test_retry_retains_pid_after_cli_forgets_a_slow_host(self):
        with patch.object(audit, 'process_identity', return_value='started host'), \
                patch.object(audit.time, 'monotonic', side_effect=[0, 121, 0, 121]), \
                patch.object(audit, 'run'), patch.object(audit.time, 'sleep'):
            for _ in range(2):
                with self.assertRaisesRegex(RuntimeError, 'did not exit'):
                    audit.stop_realm(self.realm, self.realm / 'lyracore', {})
                (self.realm / '.lyracore/state.json').write_text('{}')
        self.assertEqual(json.loads((self.realm / '.lyracore/audit-stopping.json').read_text()),
                         [{'pid': 42, 'identity': 'started host'}])

    def test_shutdown_marker_failure_does_not_signal_host(self):
        with patch.object(audit, 'process_identity', return_value='started host'), \
                patch.object(audit, 'write_state', side_effect=OSError('disk full')), \
                patch.object(audit, 'run') as run:
            with self.assertRaisesRegex(OSError, 'disk full'):
                audit.stop_realm(self.realm, self.realm / 'lyracore', {})
        run.assert_not_called()


class ReferenceOwnership(unittest.TestCase):
    def setUp(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=audit.SCRATCH)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        adapter = self.root / 'adapters/vmangos'
        adapter.mkdir(parents=True)
        pins = dict(binary_sha256='a' * 64, binary_tag='latest', binary_asset='binary.tar.gz',
                    source_commit='b' * 40, database_commit='c' * 40)
        (adapter / 'pins.json').write_text(json.dumps(pins))
        (adapter / 'setup.py').write_text('pinned setup')
        (adapter / 'compose.yml').write_text('pinned compose')
        (self.root / 'runtime-pins.json').write_text(json.dumps(pins))
        (self.root / 'binaries').mkdir()
        (self.root / 'binaries/.verified').write_text(pins['binary_sha256'])
        (self.root / 'provenance.json').write_text(json.dumps(dict(
            server='vmangos', binary_release='latest/binary.tar.gz', binary_sha256='a' * 64,
            source_commit='b' * 40, database_commit='c' * 40)))
        self.cache = self.root / 'cache'
        self.cache.mkdir()
        self.store = self.root / 'snapshots'
        self.store.mkdir()
        (self.root / 'credentials').mkdir()
        for name in ['player-password', 'gm-password']:
            (self.root / 'credentials' / name).write_text('fake')
        self.ids = {}
        self.running = set()
        self.world = 'clean world'
        self.restores = 0
        self.fail_restore = False
        self.fail_start = False
        self.project = None
        self.captures = []
        self.marker = self.cache / 'reference-state.json'
        for name, value in [('REFERENCE', self.root), ('SCENARIO_CACHE', self.cache),
                            ('SNAPSHOT_STORE', self.store)]:
            patched = patch.object(audit, name, value)
            patched.start()
            self.addCleanup(patched.stop)
        for patched in [patch.object(audit, 'run', side_effect=self.fake_run),
                        patch.object(audit, 'free_port'), patch.object(audit, 'wait_port')]:
            patched.start()
            self.addCleanup(patched.stop)

    def fake_run(self, args, **options):
        args = [str(arg) for arg in args]
        command = args[4:] if args[:2] == ['docker', 'compose'] else args[1:]
        output = ''
        if command[0].endswith('reference_baseline.py'):
            Path(command[2]).write_text('clean world')
        elif command[0] == 'ps':
            if '--services' in command:
                output = '\n'.join(self.running)
            else:
                output = self.ids.get(command[-1], '')
        elif command[0] == 'up':
            self.assertTrue(self.marker.is_file())
            self.assertIn('--no-start', command)
            self.project = options['env']['COMPOSE_PROJECT_NAME']
            self.ids = dict(zip(['auth', 'database', 'world'], ['a' * 64, 'b' * 64, 'c' * 64]))
        elif command[0] == 'inspect':
            service = next(name for name, identity in self.ids.items() if identity == command[-1])
            output = json.dumps({'com.docker.compose.project': self.project,
                                 'com.docker.compose.service': service})
        elif command[0] == 'start':
            saved = json.loads(self.marker.read_text())
            self.assertEqual(saved['containers'], self.ids)
            self.running = {'database'} if command[-1] == 'database' else set(self.ids)
            if self.fail_start:
                raise RuntimeError('start failed after database started')
        elif command[0] == 'stop':
            self.assertEqual(set(command[1:]), set(self.ids.values()))
            self.running.clear()
        elif command[0] == 'down':
            self.running.clear()
            self.ids.clear()
        elif command[0] == 'candidate':
            env = options['env']
            self.captures.append(env['WIRE_ADAPTER'])
            if env['WIRE_ADAPTER'] == 'vmangos':
                self.assertEqual(env.get('COMPOSE_PROJECT_NAME'), self.project)
        elif command[0] == 'exec':
            self.assertEqual(self.running, {'database'})
            if 'stdout' in options:
                options['stdout'].write(self.world.encode())
                return audit.subprocess.CompletedProcess(args, 0, stdout='')
            if self.fail_restore:
                self.world = 'partly restored'
                raise RuntimeError('restore failed')
            self.world = options['stdin'].read().decode()
            self.restores += 1
        return audit.subprocess.CompletedProcess(args, 0, stdout=output)

    def context(self, key='key'):
        return audit.scenario_reference_server(self.root, key)

    def test_first_ownership_restores_pristine_preparation_over_dirty_volume(self):
        self.world = 'orphan fixture and changed content'
        with self.context():
            self.assertEqual(self.world, 'clean world')
        self.assertEqual(Path(json.loads(self.marker.read_text())['dump']).read_text(), 'clean world')

    def test_provenance_failure_invalidates_previously_ready_reference(self):
        with self.context():
            self.world = 'fixture changed'
        provenance = self.root / 'provenance.json'
        value = json.loads(provenance.read_text())
        value['database_commit'] = 'd' * 40
        provenance.write_text(json.dumps(value))
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            with self.context():
                self.fail('provenance changed')
        self.assertEqual(json.loads(self.marker.read_text())['phase'], 'invalid')
        self.assertFalse(self.running)

    def test_failed_and_timed_out_captures_restore_clean_world_before_reuse(self):
        for failure in [RuntimeError('capture failed'), audit.subprocess.TimeoutExpired(['wire'], 240)]:
            with self.subTest(failure=failure):
                with self.assertRaises(type(failure)):
                    with self.context():
                        self.world = 'fixture changed'
                        raise failure
                self.assertEqual(json.loads(self.marker.read_text())['phase'], 'invalid')
                self.assertFalse(self.running)
                with self.context():
                    self.assertEqual(self.world, 'clean world')
                self.assertEqual(json.loads(self.marker.read_text())['phase'], 'ready')

    def test_ready_cache_reuses_ids_and_changed_key_restores_world(self):
        with self.context():
            pass
        identities = self.ids.copy()
        with self.context():
            self.world = 'fixture changed'
        self.assertEqual(self.restores, 2)
        with self.context('another key'):
            self.assertEqual(self.world, 'clean world')
        self.assertEqual(self.ids, identities)
        self.assertEqual(self.restores, 3)

    def test_interrupted_restore_is_retryable_with_the_same_container_ids(self):
        with self.assertRaises(RuntimeError):
            with self.context():
                raise RuntimeError('capture failed')
        identities = self.ids.copy()
        self.fail_restore = True
        with self.assertRaisesRegex(RuntimeError, 'restore failed'):
            with self.context():
                self.fail('cannot capture a partial restore')
        self.assertFalse(self.running)
        self.assertEqual(json.loads(self.marker.read_text())['phase'], 'restoring')
        self.fail_restore = False
        with self.context():
            self.assertEqual(self.world, 'clean world')
        self.assertEqual(self.ids, identities)

    def test_partial_start_and_readiness_failures_stop_owned_ids(self):
        self.fail_start = True
        with self.assertRaisesRegex(RuntimeError, 'start failed'):
            with self.context():
                self.fail('startup failed')
        self.assertFalse(self.running)
        self.fail_start = False
        with self.context():
            pass
        with patch.object(audit, 'wait_port', side_effect=RuntimeError('not ready')):
            with self.assertRaisesRegex(RuntimeError, 'not ready'):
                with self.context():
                    self.fail('readiness failed')
        self.assertEqual(json.loads(self.marker.read_text())['phase'], 'invalid')
        self.assertFalse(self.running)

    def test_ready_marker_failure_stops_services_and_keeps_recoverable_state(self):
        write = audit.write_state
        def fail_ready(path, state):
            if state.get('phase') == 'ready':
                raise OSError('disk full')
            write(path, state)
        with patch.object(audit, 'write_state', side_effect=fail_ready):
            with self.assertRaisesRegex(OSError, 'disk full'):
                with self.context():
                    self.world = 'changed'
        self.assertFalse(self.running)
        self.assertEqual(json.loads(self.marker.read_text())['phase'], 'invalid')
        with self.context():
            self.assertEqual(self.world, 'clean world')

    def test_changed_container_identity_refuses_reuse_and_stop(self):
        with self.context():
            pass
        for phase in ['ready', 'starting']:
            with self.subTest(phase=phase):
                state = json.loads(self.marker.read_text())
                state['phase'] = phase
                self.marker.write_text(json.dumps(state))
                self.ids['world'] = 'd' * 64
                with self.assertRaisesRegex(RuntimeError, 'container identities changed'):
                    with self.context():
                        self.fail('foreign container')
                with self.assertRaisesRegex(RuntimeError, 'container identities changed'):
                    audit.stop_scenario_cache()
                self.assertEqual(self.running, {'auth', 'database', 'world'})
                self.assertTrue(self.marker.exists())

    def test_full_audit_restores_world_and_stops_after_capture_failure(self):
        with self.assertRaisesRegex(RuntimeError, 'capture failed'):
            with audit.scenario_reference_server(self.root, 'full-audit', keep=False):
                self.world = 'changed'
                raise RuntimeError('capture failed')
        self.assertEqual(self.world, 'clean world')
        self.assertFalse(self.ids)
        self.assertFalse(self.marker.exists())

    def test_provenance_mismatch_starts_nothing(self):
        provenance = self.root / 'provenance.json'
        value = json.loads(provenance.read_text())
        value['database_commit'] = 'd' * 40
        provenance.write_text(json.dumps(value))
        with self.assertRaisesRegex(RuntimeError, 'provenance'):
            with self.context():
                self.fail('changed provenance')
        self.assertFalse(self.ids)

    def test_running_reference_is_refused_without_mutation(self):
        self.running = {'auth', 'database', 'world'}
        with self.assertRaisesRegex(RuntimeError, 'already running'):
            with self.context():
                self.fail('unowned Reference Server')
        self.assertEqual(self.running, {'auth', 'database', 'world'})
        self.assertFalse(self.marker.exists())

    def test_catalogue_restores_both_worlds_before_reassigning_a_name(self):
        self.capture_catalogue(rehearse=False)

    def test_rehearsal_continues_past_differences_without_importing(self):
        self.capture_catalogue(rehearse=True)

    def test_rehearsal_starts_independent_servers_together(self):
        self.capture_catalogue(rehearse=True, parallel_start=True)

    def capture_catalogue(self, *, rehearse, parallel_start=False):
        realm = self.root / 'realm'
        current = realm / '.import/vmangos/current.json'
        current.parent.mkdir(parents=True)
        current.write_text(json.dumps({'commit': 'pin'}))
        host, module, token = [realm / name for name in ['host', 'module', 'token']]
        for path in [host, module, token]:
            path.write_text('fake')
        state = self.cache / 'realm/state.json'
        state.parent.mkdir()
        state.write_text(json.dumps(dict(native_host_sha256=audit.digest(host),
                                        module_sha256=audit.digest(module), database_commit='pin',
                                        world_origin='snapshot')))
        scenarios = [dict(id='actor_first', characters=['Banktester', 'Spawnwatch']),
                     dict(id='viewer_first', characters=['Spawnwatch', 'Banktester'])]
        (self.root / 'catalogue.json').write_text(json.dumps(scenarios))
        (self.root / 'credentials/peer2-password').write_text('fake')
        realm_state = {}
        captured = []
        preparations = []
        startup = threading.Barrier(2, timeout=5)
        def wait_port(port, **_options):
            if parallel_start and port == 18085:
                startup.wait()
        @audit.contextmanager
        def realm_server(*_args, **options):
            preparations.append(options)
            if parallel_start and options.get('require_snapshot'):
                startup.wait()
            realm_state.clear()
            yield 'password', token, host, module, realm
        def capture(_wire, _work, scenario, adapter, *_args, **options):
            if adapter == 'vmangos':
                self.assertEqual(_args[-1]['COMPOSE_PROJECT_NAME'], self.project)
                self.assertEqual(self.world, 'clean world')
                self.world = 'Characters, creatures, vendor stock and restock timers changed'
            else:
                self.assertEqual(realm_state, {})
                realm_state.update(names=scenario['characters'], money=0, quests=[7], items=[25],
                                   reputation=10, creatures=[523], objects=[1], stock=0, restock=60)
            captured.append((scenario['id'], adapter))
        with patch.object(audit, 'scenario_realm', realm_server), \
                patch.object(audit, 'prepare_scenario_realm', realm_server), \
                patch.object(audit, 'scenario_key', return_value=('key', {'database_commit': 'pin'})), \
                patch.object(audit, 'capture', side_effect=capture), \
                patch.object(audit, 'rehearse_comparison', return_value=dict(rule=1, data=2)), \
                patch.object(audit.sys, 'stdout', io.StringIO()) as output, \
                patch.object(audit, 'wait_port', side_effect=wait_port), \
                patch.object(audit.os, 'sched_getaffinity', return_value=range(4), create=True):
            audit.capture_servers(self.root, 'wire', self.root, self.root, rehearse=rehearse)
        if rehearse:
            self.assertTrue(all(call == {'require_snapshot': True} for call in preparations))
            self.assertEqual([json.loads(line)['scenario'] for line in output.getvalue().splitlines()],
                             ['actor_first', 'viewer_first'])
        else:
            self.assertEqual(preparations[0], {'use_snapshot': False})
        self.assertCountEqual(captured, [(entry['id'], adapter) for entry in scenarios
                                        for adapter in ['vmangos', 'lyracore']])
        self.assertFalse(self.ids)
        self.assertFalse(self.marker.exists())

    def test_reference_comparison_failure_keeps_ownership_until_invalidated(self):
        realm = self.root / 'realm'
        current = realm / '.import/vmangos/current.json'
        current.parent.mkdir(parents=True)
        current.write_text(json.dumps({'commit': 'pin'}))
        host, module, token = [realm / name for name in ['host', 'module', 'token']]
        for path in [host, module, token]:
            path.write_text('fake')
        state = self.cache / 'realm/state.json'
        state.parent.mkdir()
        state.write_text(json.dumps(dict(native_host_sha256=audit.digest(host),
                                        module_sha256=audit.digest(module), database_commit='pin',
                                        world_origin='snapshot')))
        scenario = {'id': 'taxi_single_hop', 'characters': ['Taxi']}
        (self.root / 'catalogue.json').write_text(json.dumps([scenario]))
        @audit.contextmanager
        def realm_context(*_args, **_kwargs):
            yield 'password', token, host, module, realm
        with patch.object(audit, 'scenario_realm', realm_context):
            with self.assertRaisesRegex(RuntimeError, 'Reference Server differs'):
                with audit.capture_scenario(self.root, 'wire', self.root, scenario['id'], 'key', {}, self.root):
                    self.assertEqual(json.loads(self.marker.read_text())['phase'], 'invalid')
                    self.world = 'comparison drift'
                    raise RuntimeError('Reference Server differs')
        self.assertFalse(self.running)
        self.assertEqual(json.loads(self.marker.read_text())['phase'], 'invalid')
        with self.context():
            self.assertEqual(self.world, 'clean world')

    def test_reference_stops_even_when_realm_shutdown_fails(self):
        with self.context():
            self.world = 'changed'
        realm = self.cache / 'realm/core'
        (realm / '.lyracore').mkdir(parents=True)
        (realm / 'lyracore').write_text('CLI')
        (realm.parent / 'state.json').write_text(json.dumps({'phase': 'ready'}))
        def run(args, **options):
            if list(args[1:3]) == ['dev', 'down']:
                raise audit.subprocess.CalledProcessError(1, args)
            return self.fake_run(args, **options)
        with patch.object(audit, 'run', side_effect=run), \
                self.assertRaises(audit.subprocess.CalledProcessError):
            audit.stop_scenario_cache()
        self.assertFalse(self.ids)
        self.assertFalse(self.marker.exists())
        self.assertTrue(realm.exists())

    def test_stop_restores_clean_world_and_removes_owned_containers(self):
        with self.context():
            self.world = 'changed'
        audit.stop_scenario_cache()
        self.assertEqual(self.world, 'clean world')
        self.assertFalse(self.ids)
        self.assertFalse(self.marker.exists())


class ScenarioOwnership(unittest.TestCase):
    def test_second_check_cannot_replace_deployment_artifacts(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            cache = Path(directory)
            module = cache / 'deployment.wasm'
            module.write_text('first check')
            with (cache / '.check.lock').open('w') as ownership, \
                    patch.object(audit, 'SCENARIO_CACHE', cache), \
                    patch.object(audit, 'git', return_value=''), \
                    patch.object(audit.shutil, 'which', return_value='/usr/bin/docker'), \
                    patch.object(audit.subprocess, 'run', return_value=SimpleNamespace(returncode=0)):
                audit.lock_owned_state(ownership, 'first check')
                with self.assertRaisesRegex(RuntimeError, 'scenario check is busy'):
                    audit.scenario_main(SimpleNamespace())
            self.assertEqual(module.read_text(), 'first check')


class Tier1BuildDirectory(unittest.TestCase):
    def test_agreement_overrides_the_deployment_target_directory(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            cache = Path(directory)
            work = MagicMock()
            work.__enter__.return_value = cache
            with patch.object(audit, 'SCENARIO_CACHE', cache), \
                    patch.object(audit, 'git', return_value=''), \
                    patch.object(audit, 'module_schema_hash', return_value='schema'), \
                    patch.object(audit, 'scenario_key', return_value=('key', {})), \
                    patch.object(audit, 'scenario_work', return_value=work), \
                    patch.dict(os.environ, CARGO_TARGET_DIR=str(cache / 'deployment')), \
                    patch.object(audit.subprocess, 'run', side_effect=RuntimeError('child boundary')) as run:
                args = SimpleNamespace(client_data=cache, agreement=True, scenario='taxi_single_hop')
                with self.assertRaisesRegex(RuntimeError, 'child boundary'):
                    audit.run_scenario(args, cache, cache, 'core')
            self.assertEqual(run.call_args.kwargs['env']['CARGO_TARGET_DIR'],
                             str(cache / 'tier1-target'))


class ComparisonOutput(unittest.TestCase):
    def test_agreement_names_signatures_only_in_each_tier(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            def write(name, rule):
                (root / name).write_text(json.dumps({
                    'schema_version': 2, 'tier': 'tier1', 'core_commit': 'core',
                    'harness_revision': 'harness', 'scenarios': [{
                        'id': 'taxi_single_hop', 'behavior': 'taxi', 'scenario_sha256': 'sha',
                        'comparison': {'differences': [
                            dict(classification='rule', rule=rule, character=None, checkpoint='step',
                                 stream=None, opcode=1, field_path=['x'])]}}]}))
            write('tier1.json', 'tier1')
            write('audit.json', 'audit')
            result = audit.agreement_result(root / 'tier1.json', root / 'audit.json', 'taxi_single_hop')
            self.assertEqual(result['tier1_only'][0]['signature']['rule'], 'tier1')
            self.assertEqual(result['audit_only'][0]['signature']['rule'], 'audit')

    def test_agreement_refuses_different_scenario_revision(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            result = {'schema_version': 2, 'tier': 'tier1', 'core_commit': 'core',
                      'harness_revision': 'harness', 'scenarios': [{
                          'id': 'taxi_single_hop', 'behavior': 'taxi',
                          'scenario_sha256': 'first', 'comparison': {'differences': []}}]}
            (root / 'tier1.json').write_text(json.dumps(result))
            result['scenarios'][0]['scenario_sha256'] = 'second'
            (root / 'audit.json').write_text(json.dumps(result))
            with self.assertRaisesRegex(RuntimeError, 'scenario_sha256'):
                audit.agreement_result(root / 'tier1.json', root / 'audit.json', 'taxi_single_hop')

    def test_comparisons_survive_work_directory_cleanup(self):
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            with patch.object(audit, 'SCENARIO_CACHE', root), patch.object(audit, 'SNAPSHOT_STORE', root / 'store'):
                with self.assertRaisesRegex(RuntimeError, 'drift'):
                    with audit.scenario_work() as work:
                        (work / 'result.json').write_text('{"lyracore":"comparison"}')
                        (work / 'reference-comparison.json').write_text('{"reference":"comparison"}')
                        (work / 'private-password').write_text('secret')
                        raise RuntimeError('drift')
                self.assertFalse(work.exists())
                retained = root / 'store/diagnostics' / work.name
                self.assertEqual((retained / 'result.json').read_text(), '{"lyracore":"comparison"}')
                self.assertTrue((retained / 'reference-comparison.json').is_file())
                self.assertFalse((retained / 'private-password').exists())

    def test_failed_rust_command_preserves_stderr(self):
        error = audit.subprocess.CalledProcessError(1, ['audit'], stderr=b'bad candidate trace')
        with patch.object(audit, 'run', side_effect=error):
            with self.assertRaisesRegex(RuntimeError, 'bad candidate trace'):
                audit.run_with_diagnostic(['audit', 'check'])


class SnapshotStore(unittest.TestCase):
    def test_sealed_world_is_validated_and_old_entries_are_collected(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            realm = root / 'realm'
            native = realm / '.lyracore'
            (native / 'native-data').mkdir(parents=True)
            (native / 'native-data/world').write_text('imported')
            (native / 'native-jwt').mkdir()
            (native / 'native-jwt/id_ecdsa').write_text('private')
            (native / 'native-owner-token').write_text('token')
            (native / 'native-cli.toml').write_text('publisher')
            (realm / '.import/vmangos').mkdir(parents=True)
            (realm / '.import/vmangos/current.json').write_text('{"commit":"pin"}')
            store = root / 'store'
            (root / 'deployment-schema.json').write_text('{}')
            with patch.object(audit, 'SNAPSHOT_STORE', store), patch.object(audit, 'SCENARIO_CACHE', root):
                for key in ['first', 'second', 'third']:
                    audit.seal_snapshot(key, realm, 'password')
                    self.assertTrue(audit.validate_snapshot(audit.snapshot_path(key), key))
                self.assertFalse((store / 'first').exists())
                self.assertTrue((store / 'second').exists())
                changed = store / 'third/native-data/world'
                changed.chmod(0o600)
                changed.write_text('changed')
                with self.assertRaisesRegex(RuntimeError, 'changed after sealing'):
                    audit.validate_snapshot(store / 'third', 'third')
                audit.seal_snapshot('catalogue-first', realm, 'password', base_key='second')
                self.assertTrue((store / 'second').exists())
                audit.seal_snapshot('catalogue-second', realm, 'password', base_key='second')
                self.assertTrue(audit.validate_snapshot(store / 'second', 'second'))
                self.assertFalse((store / 'catalogue-first').exists())
                self.assertTrue(audit.validate_snapshot(store / 'catalogue-second', 'catalogue-second'))

    def test_abandoned_pending_worlds_are_removed_and_copy_checks_free_space(self):
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            abandoned = root / 'old-key.pending'
            abandoned.mkdir()
            (abandoned / 'partial').write_text('partial import')
            source = root / 'world'
            source.mkdir()
            (source / 'rows').write_text('imported')
            with patch.object(audit, 'SNAPSHOT_STORE', root):
                audit.clean_pending_snapshots()
                self.assertFalse(abandoned.exists())
                with patch.object(audit.shutil, 'disk_usage', return_value=shutil._ntuple_diskusage(1, 1, 0)):
                    with self.assertRaisesRegex(RuntimeError, 'insufficient free space'):
                        audit.require_copy_space(source, root)
                self.assertEqual((source / 'rows').read_text(), 'imported')

    def test_incomplete_snapshot_is_not_reimported(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            (root / 'key').mkdir()
            with self.assertRaisesRegex(RuntimeError, 'no seal'):
                audit.validate_snapshot(root / 'key', 'key')


class ScenarioRealmReuse(unittest.TestCase):
    def setUp(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=audit.SCRATCH)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.cache = self.root / 'cache'
        self.cache.mkdir()
        self.store = self.root / 'store'
        self.realm = self.cache / 'realm/core'
        self.commit = 'first-gameplay'
        self.running = False
        self.imports = 0
        self.fail_publish = False
        self.fail_schema = False
        self.schema = ModuleSchema().schema()
        self.pins = {'database_commit': 'pin'}
        (self.cache / 'deployment-schema.json').write_text(
            json.dumps(audit.canonical_schema(self.schema)))
        (self.root / '.lyracore-cli-rev').write_text('30814fe814edbfa3a143102f93008080482df6a6')
        for name, value in [('ROOT', self.root), ('SCENARIO_CACHE', self.cache),
                            ('SNAPSHOT_STORE', self.store)]:
            patched = patch.object(audit, name, value)
            patched.start()
            self.addCleanup(patched.stop)
        for patched in [patch.object(audit, 'run', side_effect=self.fake_run),
                        patch.object(audit, 'free_port'),
                        patch.object(audit.shutil, 'which', return_value='/usr/bin/spacetime')]:
            patched.start()
            self.addCleanup(patched.stop)

    def fake_run(self, args, **options):
        args = [str(arg) for arg in args]
        output = ''
        if args[:2] == ['git', 'rev-parse']:
            output = self.commit
        elif args[:3] == ['git', 'worktree', 'add']:
            self.realm.mkdir()
            (self.realm / '.git').write_text('gitdir: fake')
            (self.realm / 'lyracore').write_text('CLI')
        elif args[:3] == ['git', 'worktree', 'remove']:
            self.assertFalse(self.running)
            shutil.rmtree(self.realm)
        elif 'describe' in args:
            output = json.dumps((ModuleSchema().schema() if self.fail_schema else self.schema)['V10'])
        elif args[1:3] == ['dev', 'up']:
            if self.fail_publish and (self.realm / '.lyracore/native-data/world').exists():
                self.fail_publish = False
                self.running = True
                raise audit.subprocess.CalledProcessError(1, args, stderr=b'upgrade initialization failed')
            self.assertTrue((self.cache / 'realm/state.json').is_file())
            self.running = True
            native = self.realm / '.lyracore'
            native.mkdir(exist_ok=True)
            for directory in ['native-data', 'native-jwt']:
                (native / directory).mkdir(exist_ok=True)
            (native / 'native-jwt/key').write_text('key')
            (native / 'native-owner-token').write_text('token')
            (native / 'native-cli.toml').write_text('publisher')
            module = self.realm / 'target/wasm32-unknown-unknown/release/lyracore_module.wasm'
            module.parent.mkdir(parents=True, exist_ok=True)
            module.write_text(self.commit)
            (self.cache / 'deployment.wasm').write_text(self.commit)
            host = native / 'native-host/SpacetimeDB/target/release/spacetimedb-standalone'
            host.parent.mkdir(parents=True, exist_ok=True)
            host.write_text('host')
        elif args[1:3] == ['dev', 'down']:
            self.running = False
        elif args[1:3] == ['account', 'create']:
            native = self.realm / '.lyracore/native-data'
            (native / args[3]).write_bytes(options['input'])
        elif args[1] == 'import':
            self.imports += 1
            (self.realm / '.lyracore/native-data/world').write_text('imported')
            current = self.realm / '.import/vmangos/current.json'
            current.parent.mkdir(parents=True)
            current.write_text(json.dumps({'commit': 'pin'}))
        return audit.subprocess.CompletedProcess(args, 0, stdout=output)

    def add_private_table(self):
        self.schema['V10']['sections'][2]['Tables'].append(dict(
            source_name='companion', product_type_ref=0, primary_key=[0],
            table_access={'Private': []}, is_event=False))
        (self.cache / 'deployment-schema.json').write_text(
            json.dumps(audit.canonical_schema(self.schema)))

    def test_private_table_reuses_imported_rows_and_records_upgrade(self):
        with audit.scenario_realm('key', self.pins, self.root, 0):
            pass
        self.add_private_table()
        self.commit = 'additive-schema'
        with audit.scenario_realm('key', self.pins, self.root, 0) as (*_, realm):
            self.assertEqual((realm / '.lyracore/native-data/world').read_text(), 'imported')
            state = json.loads((self.cache / 'realm/state.json').read_text())
            self.assertEqual(state['world_origin'], 'upgraded_snapshot')
        self.assertEqual(self.imports, 1)

    def test_failed_upgrade_imports_again_and_records_fresh_world(self):
        with audit.scenario_realm('key', self.pins, self.root, 0):
            pass
        self.add_private_table()
        self.fail_publish = True
        with audit.scenario_realm('key', self.pins, self.root, 0):
            state = json.loads((self.cache / 'realm/state.json').read_text())
            self.assertEqual(state['world_origin'], 'fresh_import')
        self.assertEqual(self.imports, 2)

    def test_failed_upgrade_with_required_snapshot_never_imports(self):
        with audit.scenario_realm('key', self.pins, self.root, 0):
            pass
        self.add_private_table()
        self.fail_publish = True
        with self.assertRaisesRegex(RuntimeError, 'forbids cold import'):
            with audit.scenario_realm('key', self.pins, self.root, 0, require_snapshot=True):
                self.fail('failed upgrade cannot supply evidence')
        self.assertEqual(self.imports, 1)
        self.assertFalse(self.running)

    def test_unexpected_published_schema_cannot_supply_evidence(self):
        with audit.scenario_realm('key', self.pins, self.root, 0):
            pass
        self.add_private_table()
        self.fail_schema = True
        with self.assertRaisesRegex(RuntimeError, 'published Module schema differs'):
            with audit.scenario_realm('key', self.pins, self.root, 0):
                self.fail('schema mismatch cannot supply evidence')
        self.assertEqual(self.imports, 1)
        self.assertFalse(self.running)

    def test_existing_table_change_imports_again(self):
        with audit.scenario_realm('key', self.pins, self.root, 0):
            pass
        self.schema['V10']['sections'][2]['Tables'][0]['default_values'] = []
        (self.cache / 'deployment-schema.json').write_text(
            json.dumps(audit.canonical_schema(self.schema)))
        with audit.scenario_realm('key', self.pins, self.root, 0):
            pass
        self.assertEqual(self.imports, 2)

    def test_gameplay_commit_copies_snapshot_and_publishes_current_module(self):
        with audit.scenario_realm('key', self.pins, self.root, 0) as (_, _, _, module, realm):
            self.assertEqual(module.read_text(), self.commit)
            (realm / '.lyracore/native-data/world').write_text('changed by scenario')
        self.commit = 'second-gameplay'
        with audit.scenario_realm('key', self.pins, self.root, 0) as (_, _, _, module, realm):
            self.assertEqual(module.read_text(), self.commit)
            self.assertEqual((realm / '.lyracore/native-data/world').read_text(), 'imported')
        self.assertEqual(self.imports, 1)
        self.assertTrue(audit.validate_snapshot(self.store / 'key', 'key'))

    def test_catalogue_growth_creates_accounts_after_each_restore(self):
        with audit.scenario_realm('key', self.pins, self.root, 0) as (password, *_):
            pass
        for commit in [self.commit, 'another-gameplay']:
            self.commit = commit
            with audit.scenario_realm('key', self.pins, self.root, 2) as (restored, *_, realm):
                self.assertEqual(restored, password)
                for account in ['CONFORMANCE', 'CONFORMANCE2', 'CONFORMANCE3']:
                    self.assertEqual((realm / '.lyracore/native-data' / account).read_text(), password + '\n')
        self.assertEqual(self.imports, 1)

    def test_failed_seal_retains_stopped_import_and_retries_without_importing(self):
        with patch.object(audit, 'seal_snapshot', side_effect=OSError('disk full')):
            with self.assertRaisesRegex(OSError, 'disk full'):
                with audit.scenario_realm('key', self.pins, self.root, 0):
                    self.fail('unsealed import')
        self.assertFalse(self.running)
        self.assertEqual((self.realm / '.lyracore/native-data/world').read_text(), 'imported')
        audit.stop_scenario_cache(preserve_import=True)
        with audit.scenario_realm('key', self.pins, self.root, 0):
            pass
        self.assertEqual(self.imports, 1)
        self.assertTrue(audit.validate_snapshot(self.store / 'key', 'key'))

    def test_fresh_audit_does_not_resume_an_earlier_unsealed_import(self):
        with patch.object(audit, 'seal_snapshot', side_effect=OSError('disk full')):
            with self.assertRaisesRegex(OSError, 'disk full'):
                with audit.scenario_realm('key', self.pins, self.root, 0):
                    self.fail('unsealed import')
        with audit.prepare_scenario_realm('key', self.pins, self.root, 0, use_snapshot=False):
            pass
        self.assertEqual(self.imports, 2)

    def test_ready_marker_failure_stops_realm_and_keeps_snapshot(self):
        write = audit.write_state
        def fail_ready(path, state):
            if state.get('phase') == 'ready':
                raise OSError('disk full')
            write(path, state)
        with patch.object(audit, 'write_state', side_effect=fail_ready):
            with self.assertRaisesRegex(OSError, 'disk full'):
                with audit.scenario_realm('key', self.pins, self.root, 0):
                    self.fail('marker failed')
        self.assertFalse(self.running)
        self.assertFalse(self.realm.exists())
        self.assertTrue(audit.validate_snapshot(self.store / 'key', 'key'))

    def test_required_snapshot_never_falls_back_to_an_import(self):
        with self.assertRaisesRegex(RuntimeError, 'forbids an import'):
            with audit.scenario_realm('missing', self.pins, self.root, 0, require_snapshot=True):
                self.fail('snapshot missing')
        self.assertEqual(self.imports, 0)
        self.assertFalse(self.running)
        self.assertFalse(self.realm.exists())

    def test_rehearsal_without_snapshot_never_imports(self):
        (self.root / 'catalogue.json').write_text(json.dumps([
            dict(id='bank_slot_purchase', characters=['Banktester'])]))
        with patch.object(audit, 'scenario_key', return_value=('missing', self.pins)), \
                self.assertRaisesRegex(RuntimeError, 'forbids an import'):
            audit.capture_servers(self.root, 'wire', self.root, self.root, rehearse=True)
        self.assertEqual(self.imports, 0)
        self.assertFalse(self.running)
        self.assertFalse((self.root / 'reports').exists())


class CatalogueSnapshot(unittest.TestCase):
    def test_creature_requirements_are_independent_of_catalogue_order(self):
        first = [dict(creatures=[9499, 11188]), dict(creatures=[9499])]
        second = list(reversed(first))
        key, creatures = audit.catalogue_snapshot_key('base', first)
        self.assertEqual(creatures, [9499, 11188])
        self.assertEqual(audit.catalogue_snapshot_key('base', second), (key, creatures))
        self.assertNotEqual(audit.catalogue_snapshot_key('other', first)[0], key)
        self.assertNotEqual(audit.catalogue_snapshot_key('base', [dict(creatures=[9499])])[0], key)

    def test_full_audit_rebuilds_catalogue_content_from_its_new_base(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            (root / 'catalogue.json').write_text(json.dumps([dict(creatures=[9499], characters=['Vendor'])]))
            (root / 'deployment-schema.json').write_text('{}')
            with patch.object(audit, 'SCENARIO_CACHE', root), \
                    patch.object(audit, 'validate_snapshot', return_value=True), \
                    patch.object(audit, 'snapshot_origin', return_value='snapshot'), \
                    patch.object(audit, 'scenario_realm', side_effect=RuntimeError('new base required')), \
                    self.assertRaisesRegex(RuntimeError, 'new base required'):
                audit.prepare_catalogue_snapshot(root, 'base', {'database_commit': 'pin'}, root,
                                                 require_snapshot=False, fresh=True)

    def test_rehearsal_prepares_fixture_content_with_a_strict_cached_base(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            (root / 'catalogue.json').write_text(json.dumps([dict(creatures=[9499], characters=['Vendor'])]))
            with patch.object(audit, 'validate_snapshot', side_effect=lambda _path, key: key == 'base'), \
                    patch.object(audit, 'scenario_realm', side_effect=RuntimeError('cached base entered')) as realm, \
                    self.assertRaisesRegex(RuntimeError, 'cached base entered'):
                audit.prepare_catalogue_snapshot(root, 'base', {'database_commit': 'pin'}, root,
                                                 require_snapshot=True)
            self.assertTrue(realm.call_args.kwargs['require_snapshot'])

    def test_rehearsal_cannot_import_without_a_base_world(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            (root / 'catalogue.json').write_text(json.dumps([dict(creatures=[9499])]))
            with patch.object(audit, 'SNAPSHOT_STORE', root / 'snapshots'), \
                    patch.object(audit, 'run') as run, \
                    self.assertRaisesRegex(RuntimeError, 'base world snapshot missing'):
                audit.prepare_catalogue_snapshot(root, 'base', {'database_commit': 'pin'}, root,
                                                 require_snapshot=True)
            run.assert_not_called()


class Rehearsal(unittest.TestCase):
    def test_rehearsal_cannot_select_report_output(self):
        for option in [['--output', '/unused'], ['--date', '2099-01-01'],
                       ['--check', '--scenario', 'bank_slot_purchase']]:
            with patch.dict(audit.os.environ, LYRACORE_AUDIT_ARGS=json.dumps(['--rehearse', *option])), \
                    self.assertRaises(SystemExit) as error:
                audit.main()
            self.assertEqual(error.exception.code, 2)

    def test_rehearsal_routes_to_capture_without_the_report_writer(self):
        with patch.dict(audit.os.environ, LYRACORE_AUDIT_ARGS=json.dumps([
                '--rehearse', '--client-data', str(audit.ROOT)])), \
                patch.object(audit, 'check_spacetime_version'), \
                patch.object(audit, 'scenario_main', return_value=0) as capture, \
                patch.object(audit, 'run') as run:
            self.assertEqual(audit.main(), 0)
        self.assertTrue(capture.call_args.args[0].rehearse)
        run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
