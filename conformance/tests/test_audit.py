import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import MagicMock, patch

SPEC = importlib.util.spec_from_file_location('audit', Path(__file__).parents[1] / 'audit.py')
audit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(audit)


class ReferenceLifetime(unittest.TestCase):
    def setUp(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        self.directory = tempfile.TemporaryDirectory(dir=audit.SCRATCH)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.adapter = self.root / 'adapters/vmangos'
        self.adapter.mkdir(parents=True)
        (self.root / 'binaries').mkdir()
        pins = dict(binary_sha256='a' * 64, binary_tag='latest', binary_asset='binary.tar.gz',
                    source_commit='b' * 40, database_commit='c' * 40)
        for path in [self.adapter / 'pins.json', self.root / 'runtime-pins.json']:
            path.write_text(json.dumps(pins))
        (self.root / 'binaries/.verified').write_text(pins['binary_sha256'])
        (self.root / 'provenance.json').write_text(json.dumps(dict(
            server='vmangos', binary_release='latest/binary.tar.gz', binary_sha256='a' * 64,
            source_commit='b' * 40, database_commit='c' * 40)))
        self.running = False
        self.fail_start = False
        self.mutations = []

    def docker(self, args, **_):
        if args[1] == 'ps':
            class Output:
                stdout = 'owned-elsewhere' if self.running else ''
            return Output()
        if args[1] == 'up':
            self.running = True
            self.mutations.append('start')
            if self.fail_start:
                raise RuntimeError('compose failed after starting the database')
        if args[1] == 'down':
            self.running = False
            self.mutations.append('stop')

    def reference(self):
        return audit.reference_server(self.root, 0)

    def test_partial_start_and_capture_failures_stop_owned_services(self):
        for fail_start in [False, True]:
            with self.subTest(fail_start=fail_start), patch.object(audit, 'REFERENCE', self.root), \
                    patch.object(audit, 'run', self.docker), patch.object(audit, 'wait_port'), \
                    patch.object(audit.socket, 'socket'):
                self.fail_start = fail_start
                with self.assertRaises(RuntimeError):
                    with self.reference():
                        self.assertTrue(self.running)
                        raise RuntimeError('capture failed')
                self.assertFalse(self.running)

    def test_running_reference_is_refused_without_mutation(self):
        self.running = True
        with patch.object(audit, 'REFERENCE', self.root), patch.object(audit, 'run', self.docker):
            with self.assertRaisesRegex(RuntimeError, 'already running'):
                with self.reference():
                    self.fail('must refuse an unowned Reference Server')
        self.assertTrue(self.running)
        self.assertEqual(self.mutations, [])

    def test_provenance_mismatch_starts_nothing(self):
        provenance = self.root / 'provenance.json'
        value = json.loads(provenance.read_text())
        value['database_commit'] = 'd' * 40
        provenance.write_text(json.dumps(value))
        with patch.object(audit, 'REFERENCE', self.root), patch.object(audit, 'run', self.docker):
            with self.assertRaisesRegex(RuntimeError, 'provenance'):
                with self.reference():
                    self.fail('must refuse changed provenance')
        self.assertEqual(self.mutations, [])

    def test_scenario_reference_reuses_matching_pin_and_restarts_for_changed_key(self):
        cache = self.root / 'scenario-cache'
        cache.mkdir()
        store = self.root / 'snapshots'
        store.mkdir()

        def docker(args, **_):
            if args[1] == 'ps':
                if '--services' in args:
                    value = 'auth\ndatabase\nworld' if self.running else ''
                else:
                    value = {'auth': 'a' * 64, 'database': 'b' * 64,
                             'world': 'c' * 64}[args[-1]] if self.running else ''
                return type('Output', (), {'stdout': value})()
            if args[1] == 'up':
                self.running = True
                self.mutations.append('start')
            if args[1] == 'down':
                self.running = False
                self.mutations.append('stop')

        def dump(_compose, _env, path):
            path.write_text('clean world')

        with patch.object(audit, 'REFERENCE', self.root), patch.object(audit, 'SCENARIO_CACHE', cache), \
                patch.object(audit, 'SNAPSHOT_STORE', store), patch.object(audit, 'reference_dump', side_effect=dump), \
                patch.object(audit, 'run', side_effect=docker), patch.object(audit, 'wait_port'), \
                patch.object(audit.socket, 'socket'), \
                patch.object(audit, 'restore_reference_world',
                             side_effect=lambda *_: (setattr(self, 'running', False),
                                                     self.mutations.append('stop'))):
            for key in ['first', 'first', 'second']:
                with audit.scenario_reference_server(self.root, key):
                    self.assertTrue(self.running)
        self.assertEqual(self.mutations, ['start', 'stop', 'start'])


class WorldReadiness(unittest.TestCase):
    def test_accept_without_an_auth_challenge_is_not_ready(self):
        early = MagicMock()
        early.__enter__.return_value.recv.return_value = b''
        ready = MagicMock()
        ready.__enter__.return_value.recv.side_effect = [b'\x00', b'\x06\xec\x01']
        with patch.object(audit.socket, 'create_connection', side_effect=[early, ready]) as connect, \
                patch.object(audit.time, 'sleep'):
            audit.wait_port(18085, world=True)
        self.assertEqual(connect.call_count, 2)


class LocalRealmLifetime(unittest.TestCase):
    def test_import_failure_stops_the_realm_started_by_the_audit(self):
        calls = []

        def run(args, **_):
            calls.append([str(arg) for arg in args])
            if args[:3] == ['git', 'worktree', 'add']:
                args[4].mkdir()
            if args[1:3] == ['import', '--accept']:
                raise RuntimeError('import failed')
            if args[:3] == ['git', 'worktree', 'remove']:
                shutil.rmtree(args[4])

        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory, \
                patch.object(audit, 'ROOT', Path(directory)), patch.object(audit, 'git', return_value=''), \
                patch.object(audit, 'run', run), patch.object(audit.shutil, 'which', return_value='/usr/bin/spacetime'), \
                patch.object(audit.socket, 'socket'):
            work = Path(directory)
            (work / '.lyracore-cli-rev').write_text('a016c82c6848a098ecbbb07ed53704c2b7dfdff1')
            with self.assertRaisesRegex(RuntimeError, 'import failed'):
                with audit.local_realm(work, work / 'client/Data', 0):
                    self.fail('capture must not start')
            self.assertFalse((work / 'core').exists())
        self.assertEqual([call[1:3] for call in calls],
                         [['worktree', 'add'], ['dev', 'up'], ['import', '--accept'],
                          ['dev', 'down'], ['worktree', 'remove']])
        self.assertEqual(calls[2][-2:], ['--client-data', str(work / 'client/Data')])


class ClientDataPath(unittest.TestCase):
    def test_relative_client_data_is_resolved_before_the_audit_changes_directory(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            client_data = Path(directory) / 'client/Data'
            client_data.mkdir(parents=True)
            relative = os.path.relpath(client_data, Path.cwd())
            arguments = ['--date', '2099-01-01', '--output', str(Path(directory) / 'reports'),
                         '--client-data', relative]
            captured = []

            def capture_servers(_harness, _wire, _work, path):
                captured.append(path)
                return {}, 'host', 'module', 'content'

            def run(args, **_):
                return type('Result', (), {'stdout': '{}'})()

            original_cwd = Path.cwd()
            self.addCleanup(os.chdir, original_cwd)
            original_tempdir = tempfile.tempdir
            self.addCleanup(setattr, tempfile, 'tempdir', original_tempdir)
            with patch.dict(audit.os.environ, LYRACORE_AUDIT_ARGS=json.dumps(arguments)), \
                    patch.object(audit, 'git', side_effect=lambda *args: 'core' if args[0] == 'rev-parse' else ''), \
                    patch.object(audit, 'run', run), patch.object(audit, 'check_cargo_config'), \
                    patch.object(audit, 'pinned_harness', return_value=Path(directory)), \
                    patch.object(audit, 'capture_servers', side_effect=capture_servers):
                audit.main()
            self.assertEqual(captured, [str(client_data.resolve())])


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
            client = root / 'Data'
            client.mkdir()
            (client / 'dbc.MPQ').write_bytes(b'first')
            with patch.object(audit, 'ROOT', root), patch.object(audit, 'SCENARIO_CACHE', root):
                first, _ = audit.scenario_key(harness, client, 'schema')
                self.assertEqual(first, audit.scenario_key(harness, client, 'schema')[0])
                gameplay.write_text('second gameplay commit')
                self.assertEqual(first, audit.scenario_key(harness, client, 'schema')[0])
                changed_schema, _ = audit.scenario_key(harness, client, 'new schema')
                (client / 'dbc.MPQ').write_bytes(b'other')
                changed_client, _ = audit.scenario_key(harness, client, 'schema')
                pins['database_commit'] = 'b' * 40
                (adapter / 'pins.json').write_text(json.dumps(pins))
                changed_pin, _ = audit.scenario_key(harness, client, 'schema')
                (importer / 'lib.rs').write_text('changed')
                changed_importer, _ = audit.scenario_key(harness, client, 'schema')
            self.assertNotEqual(first, changed_schema)
            self.assertNotEqual(first, changed_client)
            self.assertNotEqual(changed_client, changed_pin)
            self.assertNotEqual(changed_pin, changed_importer)


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

            with patch.object(audit, 'run', side_effect=run):
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
            with patch.object(audit, 'SCENARIO_CACHE', cache):
                audit.stop_scenario_cache()
            self.assertFalse((cache / 'realm').exists())


class CleanupAuthentication(unittest.TestCase):
    def test_only_pre_authentication_proof_errors_retry_once(self):
        proof = audit.subprocess.CalledProcessError(1, ['wire'], stderr=b'Error: logon proof rejected\n')
        deletion = audit.subprocess.CalledProcessError(1, ['wire'], stderr=b'Error: character deletion failed\n')
        for errors, calls, succeeds in [([proof, None], 2, True), ([proof, proof], 2, False),
                                        ([deletion], 1, False)]:
            with self.subTest(succeeds=succeeds, calls=calls), patch.object(audit, 'run', side_effect=errors) as run:
                if succeeds:
                    audit.delete_reference_character('wire', 'Fixture', 'CONFORMANCE', b'password', {})
                else:
                    with self.assertRaises(RuntimeError):
                        audit.delete_reference_character('wire', 'Fixture', 'CONFORMANCE', b'password', {})
                self.assertEqual(run.call_count, calls)


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
        (self.store / ('reference-' + pins['database_commit'] + '.sql')).write_text('clean world')
        self.ids = {}

    def fake_run(self, args, **_):
        if args[1] == 'up':
            self.assertTrue((self.cache / 'reference-state.json').is_file())
            self.ids = {'auth': 'auth-id', 'database': 'db-id', 'world': 'world-id'}
        elif args[1] == 'down':
            self.ids = {}
        return type('Output', (), {'stdout': ''})()

    def context(self):
        return audit.scenario_reference_server(self.root, 'key')

    def test_failed_capture_restores_database_before_reuse(self):
        with patch.object(audit, 'REFERENCE', self.root), \
                patch.object(audit, 'SCENARIO_CACHE', self.cache), \
                patch.object(audit, 'SNAPSHOT_STORE', self.store), \
                patch.object(audit, 'reference_ids', side_effect=lambda *_: self.ids.copy()), \
                patch.object(audit, 'reference_running', side_effect=lambda *_: set(self.ids)), \
                patch.object(audit, 'run', side_effect=self.fake_run), \
                patch.object(audit, 'free_port'), patch.object(audit, 'wait_port'), \
                patch.object(audit, 'restore_reference_world') as restore:
            with self.assertRaisesRegex(RuntimeError, 'capture failed'):
                with self.context():
                    raise RuntimeError('capture failed')
            self.assertEqual(json.loads((self.cache / 'reference-state.json').read_text())['phase'],
                             'invalid')
            with self.context():
                pass
            restore.assert_called_once()
            self.assertEqual(json.loads((self.cache / 'reference-state.json').read_text())['phase'],
                             'ready')

    def test_timed_out_capture_marks_reference_unusable(self):
        with patch.object(audit, 'REFERENCE', self.root), \
                patch.object(audit, 'SCENARIO_CACHE', self.cache), \
                patch.object(audit, 'SNAPSHOT_STORE', self.store), \
                patch.object(audit, 'reference_ids', side_effect=lambda *_: self.ids.copy()), \
                patch.object(audit, 'reference_running', side_effect=lambda *_: set(self.ids)), \
                patch.object(audit, 'run', side_effect=self.fake_run), \
                patch.object(audit, 'free_port'), patch.object(audit, 'wait_port'):
            with self.assertRaises(audit.subprocess.TimeoutExpired):
                with self.context():
                    raise audit.subprocess.TimeoutExpired(['wire'], 240)
        self.assertEqual(json.loads((self.cache / 'reference-state.json').read_text())['phase'],
                         'invalid')

    def test_changed_container_identity_refuses_reuse_and_stop(self):
        for phase in ['ready', 'starting']:
            with self.subTest(phase=phase):
                marker = self.cache / 'reference-state.json'
                marker.write_text(json.dumps(dict(phase=phase, key='key', project='audit-owned',
                                                  adapter=str(self.root / 'adapters/vmangos/reference.sh'),
                                                  containers={'world': 'old-id'}, dump='clean.sql')))
                self.ids = {'world': 'new-id'}
                with patch.object(audit, 'REFERENCE', self.root), \
                        patch.object(audit, 'SCENARIO_CACHE', self.cache), \
                        patch.object(audit, 'reference_ids', side_effect=lambda *_: self.ids.copy()), \
                        patch.object(audit, 'run', side_effect=self.fake_run) as run:
                    with self.assertRaisesRegex(RuntimeError, 'container identities changed'):
                        with self.context():
                            pass
                    with self.assertRaisesRegex(RuntimeError, 'container identities changed'):
                        audit.stop_scenario_cache()
                    run.assert_not_called()
                    self.assertTrue(marker.exists())


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
            with patch.object(audit, 'SNAPSHOT_STORE', store):
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

    def test_incomplete_snapshot_is_not_reimported(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            root = Path(directory)
            (root / 'key').mkdir()
            with self.assertRaisesRegex(RuntimeError, 'no seal'):
                audit.validate_snapshot(root / 'key', 'key')


class FullAuditCleanup(unittest.TestCase):
    def test_startup_marker_keeps_worktree_when_host_exit_is_unconfirmed(self):
        audit.SCRATCH.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=audit.SCRATCH) as directory:
            work = Path(directory) / 'audit'
            work.mkdir()
            (work / 'realm-starting.json').write_text('{}')
            audit.cleanup_audit_work(work)
            self.assertTrue(work.exists())
            (work / 'realm-starting.json').unlink()
            audit.cleanup_audit_work(work)
            self.assertFalse(work.exists())


if __name__ == '__main__':
    unittest.main()
