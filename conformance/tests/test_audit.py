import importlib.util
import json
from pathlib import Path
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
        return audit.reference_server(self.root)

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
            calls.append(args[1:])
            if args[1:3] == ['dev', 'status']:
                return type('Result', (), {'stdout': 'Native Host: Stopped'})()
            if args[1:3] == ['import', '--accept']:
                raise RuntimeError('import failed')

        with tempfile.TemporaryDirectory() as directory, patch.object(audit, 'ROOT', Path(directory)), \
                patch.object(audit, 'run', run), patch.object(audit.socket, 'socket'):
            with self.assertRaisesRegex(RuntimeError, 'import failed'):
                with audit.local_realm(Path(directory)):
                    self.fail('capture must not start')
        self.assertEqual(calls, [['dev', 'status'], ['dev', 'up', '--single'],
                                 ['import', '--accept'], ['dev', 'down']])


class CleanupAuthentication(unittest.TestCase):
    def test_only_pre_authentication_proof_errors_retry_once(self):
        proof = audit.subprocess.CalledProcessError(1, ['wire'], stderr=b'Error: logon proof rejected\n')
        deletion = audit.subprocess.CalledProcessError(1, ['wire'], stderr=b'Error: character deletion failed\n')
        for errors, calls, succeeds in [([proof, None], 2, True), ([proof, proof], 2, False),
                                        ([deletion], 1, False)]:
            with self.subTest(succeeds=succeeds, calls=calls), patch.object(audit, 'run', side_effect=errors) as run:
                if succeeds:
                    audit.delete_reference_character('wire', 'Fixture', b'password', {})
                else:
                    with self.assertRaises(RuntimeError):
                        audit.delete_reference_character('wire', 'Fixture', b'password', {})
                self.assertEqual(run.call_count, calls)


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


if __name__ == '__main__':
    unittest.main()
