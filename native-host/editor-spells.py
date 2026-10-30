#!/usr/bin/env python3
import json
import os
from pathlib import Path
import shutil
import sys


def main():
    root = Path(__file__).resolve().parent.parent
    args = sys.argv[1:]
    if '--read-spell-source' in args and '--dump' not in args:
        source = os.environ.get('MELD_LYRACORE_WORLD_SOURCE')
        if not source:
            manifest = root / '.import/vmangos/current.json'
            try:
                source = json.loads(manifest.read_text())['sqlite_path']
            except (OSError, ValueError, KeyError) as error:
                sys.exit(f'cannot read the World Source from {manifest}: {error}; set MELD_LYRACORE_WORLD_SOURCE')
        source_path = Path(source).expanduser()
        if not source_path.is_absolute():
            source_path = root / source_path
        args += ['--dump', str(source_path.resolve())]
    cargo = shutil.which('cargo') or str(Path.home() / '.cargo/bin/cargo')
    os.chdir(root)
    os.execv(cargo, [cargo, 'run', '--quiet', '--locked', '-p', 'lyracore-importer',
                    '--bin', 'lyracore-importer', '--', *args])


if __name__ == '__main__':
    main()
