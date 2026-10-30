import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import zipfile


def prepare(adapter, output, reference, scratch):
    spec = importlib.util.spec_from_file_location('reference_setup', adapter / 'setup.py')
    setup = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(setup)
    setup.ROOT = reference
    archive = setup.asset('database')
    with tempfile.TemporaryDirectory(prefix='reference-pristine-', dir=scratch) as directory:
        root = Path(directory)
        for name in ['imports', 'init', 'config', 'credentials']:
            (root / name).mkdir()
        (root / 'binaries').symlink_to(reference / 'binaries', target_is_directory=True)
        shutil.copy2(reference / 'db.env', root / 'db.env')
        with zipfile.ZipFile(archive) as source:
            for name in ['logon', 'mangos', 'characters', 'logs']:
                matches = [item for item in source.namelist() if Path(item).name == name + '.sql']
                if len(matches) != 1:
                    raise RuntimeError(f'verified database archive has no unique {name}.sql')
                with source.open(matches[0]) as sql, (root / 'imports' / (name + '.sql')).open('wb') as target:
                    shutil.copyfileobj(sql, target)
        override = root / 'volume.json'
        override.write_text(json.dumps({
            'services': {'database': {'volumes': [
                {'type': 'volume', 'source': 'pristine', 'target': '/var/lib/mysql'}]}},
            'volumes': {'pristine': {}},
        }))
        setup.ROOT = root
        setup.ENV = dict(os.environ, REFERENCE_ROOT=str(root),
                         COMPOSE_PROJECT_NAME='lyracore-' + root.name,
                         REFERENCE_UID=str(os.getuid()), REFERENCE_GID=str(os.getgid()))
        setup.COMPOSE = ['docker', 'compose', '-f', str(adapter / 'compose.yml'), '-f', str(override)]
        setup.configure()
        pending = output.with_suffix('.pending')
        try:
            setup.run([*setup.COMPOSE, 'up', '-d', '--wait', '--wait-timeout', '180', 'database'],
                      stdout=subprocess.DEVNULL)
            setup.account('CONFORMANCE', 0, 'player')
            setup.account('CONFORMANCEGM', 5, 'gm')
            setup.verify_configuration()
            if setup.sql('SELECT COUNT(*) FROM characters.characters;') != '0':
                raise RuntimeError('pristine Reference Server contains Characters')
            with pending.open('wb') as target:
                setup.run([*setup.COMPOSE, 'exec', '-T', 'database', 'sh', '-c',
                           'MYSQL_PWD="$MARIADB_ROOT_PASSWORD" mariadb-dump -uroot '
                           '--single-transaction --add-drop-database '
                           '--databases realmd mangos characters logs'], stdout=target)
            if not pending.stat().st_size:
                raise RuntimeError('pristine Reference Server dump is empty')
            for path in (root / 'credentials').iterdir():
                shutil.copy2(path, reference / 'credentials' / path.name)
            pending.chmod(0o400)
            pending.replace(output)
        finally:
            setup.run([*setup.COMPOSE, 'down', '--volumes'], stdout=subprocess.DEVNULL)
            pending.unlink(missing_ok=True)


if __name__ == '__main__':
    prepare(*(Path(argument) for argument in sys.argv[1:]))
