import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

from conformance.audit_spacetime import deployment_arguments
from conformance import reference_baseline


class DeploymentArtifact(unittest.TestCase):
    def test_publish_and_schema_use_the_same_wasm_with_private_configuration(self):
        artifact = Path('/audit/deployment.wasm')
        for arguments, expected in [
            (['--config-path', '/audit/native-cli.toml', 'publish', '-s', 'http://127.0.0.1:3000',
              '-p', '/audit/core/module', '--build-options=--features=debug_reducers', '--yes', 'lyracore'],
             ['--config-path', '/audit/native-cli.toml', 'publish', '-s', 'http://127.0.0.1:3000', '--yes', 'lyracore']),
            (['generate', '--lang', 'rust', '--module-path', '/audit/core/module', '--out-dir', '/audit/schema',
              '--build-options=--features=debug_reducers', '--include-private', '--no-config', '-y'],
             ['generate', '--lang', 'rust', '--out-dir', '/audit/schema', '--include-private', '--no-config', '-y']),
        ]:
            self.assertEqual(deployment_arguments(arguments, artifact), [*expected, '--bin-path', str(artifact)])
        self.assertEqual(deployment_arguments(['sql', 'lyracore', 'SELECT 1'], artifact),
                         ['sql', 'lyracore', 'SELECT 1'])


class PristinePreparation(unittest.TestCase):
    def test_dump_comes_from_verified_archive_in_a_new_disposable_volume(self):
        scratch = Path(os.environ.get('LYRACORE_AUDIT_SCRATCH',
                                      Path.home() / '.cache/lyracore-scratch/m5c'))
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as directory:
            root = Path(directory)
            reference = root / 'reference'
            (reference / 'credentials').mkdir(parents=True)
            (reference / 'database').mkdir()
            (reference / 'database/world').write_text('orphan fixture')
            (reference / 'db.env').write_text('private credentials')
            archive = root / 'verified.zip'
            with zipfile.ZipFile(archive, 'w') as sql:
                for name in ['logon', 'mangos', 'characters', 'logs']:
                    sql.writestr(name + '.sql', 'pristine ' + name)
            volumes = {}
            setup = SimpleNamespace()

            def run(arguments, **options):
                override = json.loads(Path(setup.COMPOSE[-1]).read_text())
                mount = override['services']['database']['volumes'][0]
                self.assertEqual(mount, {'type': 'volume', 'source': 'pristine', 'target': '/var/lib/mysql'})
                self.assertNotEqual(setup.ROOT, reference)
                if 'up' in arguments:
                    volumes['pristine'] = (setup.ROOT / 'imports/mangos.sql').read_text()
                elif 'exec' in arguments:
                    options['stdout'].write(volumes['pristine'].encode())
                elif 'down' in arguments:
                    self.assertIn('--volumes', arguments)
                    volumes.clear()
                return subprocess.CompletedProcess(arguments, 0)

            def account(_username, _level, name):
                (setup.ROOT / 'credentials' / (name + '-password')).write_text('saved password')

            setup.asset = lambda kind: archive if kind == 'database' else self.fail(kind)
            setup.configure = lambda: None
            setup.account = account
            setup.verify_configuration = lambda: None
            setup.sql = lambda _statement: '0'
            setup.run = run
            spec = SimpleNamespace(loader=SimpleNamespace(exec_module=lambda _module: None))
            output = root / 'baseline.sql'
            with patch.object(importlib.util, 'spec_from_file_location', return_value=spec), \
                    patch.object(importlib.util, 'module_from_spec', return_value=setup):
                reference_baseline.prepare(root / 'adapter', output, reference, root)
            self.assertEqual(output.read_text(), 'pristine mangos')
            self.assertEqual((reference / 'database/world').read_text(), 'orphan fixture')
            self.assertEqual((reference / 'credentials/player-password').read_text(), 'saved password')
            self.assertFalse(volumes)
            self.assertFalse(list(root.glob('reference-pristine-*')))
