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
        self.running = set()
        self.world = 'clean world'
        self.restores = 0
        self.fail_restore = False
        self.fail_start = False
        self.project = None
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
        if command[0] == 'ps':
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
        elif command[0] == 'exec':
            self.assertEqual(self.running, {'database'})
            if self.fail_restore:
                self.world = 'partly restored'
                raise RuntimeError('restore failed')
            self.world = options['stdin'].read().decode()
            self.restores += 1
        return audit.subprocess.CompletedProcess(args, 0, stdout=output)

    def context(self, key='key'):
        return audit.scenario_reference_server(self.root, key)

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
        self.assertEqual(self.restores, 0)
        with self.context('another key'):
            self.assertEqual(self.world, 'clean world')
        self.assertEqual(self.ids, identities)
        self.assertEqual(self.restores, 1)

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
            with audit.reference_server(self.root, 0):
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

    def test_stop_restores_clean_world_and_removes_owned_containers(self):
        with self.context():
            self.world = 'changed'
        audit.stop_scenario_cache()
        self.assertEqual(self.world, 'clean world')
        self.assertFalse(self.ids)
        self.assertFalse(self.marker.exists())


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
        self.pins = {'database_commit': 'pin'}
        (self.root / '.lyracore-cli-rev').write_text('a016c82c6848a098ecbbb07ed53704c2b7dfdff1')
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
        elif args[1:3] == ['dev', 'up']:
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
            host = native / 'native-host/SpacetimeDB/target/release/spacetimedb-standalone'
            host.parent.mkdir(parents=True, exist_ok=True)
            host.write_text('host')
        elif args[1:3] == ['dev', 'down']:
            self.running = False
        elif args[1] == 'import':
            self.imports += 1
            (self.realm / '.lyracore/native-data/world').write_text('imported')
            current = self.realm / '.import/vmangos/current.json'
            current.parent.mkdir(parents=True)
            current.write_text(json.dumps({'commit': 'pin'}))
        return audit.subprocess.CompletedProcess(args, 0, stdout=output)

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
