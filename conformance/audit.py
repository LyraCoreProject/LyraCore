import argparse
from contextlib import contextmanager
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parent.parent
SCRATCH = Path.home() / '.cache/lyracore-scratch/m5c'
REFERENCE = Path.home() / '.cache/lyracore-scratch/reference-server'
SCENARIO_CACHE = SCRATCH / 'scenario-audit'
SNAPSHOT_STORE = Path.home() / '.cache/lyracore/conformance-worlds'


def run(args, **options):
    return subprocess.run([str(arg) for arg in args], check=True, **options)


def run_with_diagnostic(args, **options):
    try:
        return run(args, capture_output=True, **options)
    except subprocess.CalledProcessError as error:
        diagnostic = (error.stderr or b'').decode(errors='replace').strip()
        raise RuntimeError(f'{args[0]} failed: {diagnostic or error}') from error


def git(*args):
    return run(['git', *args], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


@contextmanager
def timed(label):
    start = time.monotonic()
    try:
        yield
    finally:
        print(f'[audit] {label}: {time.monotonic() - start:.2f}s', file=sys.stderr, flush=True)


def client_fingerprint(directory):
    cache = SCENARIO_CACHE / 'client-data-state.json'
    manifest = file_manifest(directory)
    if cache.is_file():
        saved = json.loads(cache.read_text())
        if saved['directory'] == str(directory) and saved['manifest'] == manifest:
            return saved['sha256']
    fingerprint = hashlib.sha256()
    for path in sorted(directory.rglob('*')):
        if path.is_file():
            fingerprint.update(str(path.relative_to(directory)).encode())
            fingerprint.update(bytes.fromhex(digest(path)))
    if file_manifest(directory) != manifest:
        raise RuntimeError('client data changed while hashing')
    checksum = fingerprint.hexdigest()
    write_state(cache, {'directory': str(directory), 'manifest': manifest, 'sha256': checksum})
    return checksum


def file_manifest(directory):
    entries = []
    for path in sorted(directory.rglob('*')):
        if path.is_symlink():
            raise RuntimeError(f'cached data contains a symlink: {path}')
        if path.is_file():
            stat = path.stat()
            entries.append([str(path.relative_to(directory)), stat.st_dev, stat.st_ino,
                            stat.st_size, stat.st_mtime_ns, stat.st_ctime_ns])
    return entries


def tree_hash(paths):
    checksum = hashlib.sha256()
    for root in paths:
        checksum.update(str(root.relative_to(ROOT)).encode())
        for path in sorted(root.rglob('*')) if root.is_dir() else [root]:
            if path.is_symlink():
                raise RuntimeError(f'import input contains a symlink: {path}')
            if path.is_file():
                checksum.update(str(path.relative_to(root) if root.is_dir() else root.name).encode())
                checksum.update(bytes.fromhex(digest(path)))
    return checksum.hexdigest()


def module_schema_hash():
    SCENARIO_CACHE.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='schema-', dir=SCENARIO_CACHE) as directory:
        output = Path(directory)
        run(['spacetime', 'generate', '--lang', 'rust', '--module-path', ROOT / 'module',
             '--out-dir', output, '--build-options=--features=debug_reducers',
             '--include-private', '--no-config', '-y'], cwd=ROOT, stdout=subprocess.DEVNULL)
        checksum = hashlib.sha256()
        for path in sorted(output.rglob('*')):
            if path.is_file():
                source = path.read_text()
                # SpacetimeDB 2.7.1 emits event lookup markers nondeterministically.
                source = re.sub(r'^impl __sdk::__query_builder::CanBeLookupTable for \w+ \{\}\n?',
                                '', source, flags=re.MULTILINE)
                checksum.update(str(path.relative_to(output)).encode())
                checksum.update(source.strip().encode())
        return checksum.hexdigest()


def scenario_key(harness, client_data, schema_hash):
    pins = json.loads((harness / 'adapters/vmangos/pins.json').read_text())
    with timed('client data fingerprint'):
        client_hash = client_fingerprint(client_data)
    importer = tree_hash([ROOT / 'importer/src', ROOT / 'importer/scripts',
                          ROOT / 'importer/manifests', ROOT / 'importer/Cargo.toml'])
    key = dict(importer_sha256=importer, module_schema_sha256=schema_hash,
               vmangos_database_commit=pins['database_commit'],
               cli_revision=(ROOT / '.lyracore-cli-rev').read_text().strip(),
               client_sha256=client_hash)
    return hashlib.sha256(json.dumps(key, sort_keys=True).encode()).hexdigest(), pins


def free_port(port):
    try:
        with socket.socket() as listener:
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind(('127.0.0.1', port))
            listener.listen(1)
    except OSError as error:
        raise RuntimeError(f'loopback port {port} is unavailable: {error}') from error


def write_state(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    pending = path.with_suffix('.pending')
    pending.write_text(json.dumps(value))
    pending.chmod(0o600)
    pending.replace(path)


def process_identity(pid):
    result = subprocess.run(['ps', '-p', str(pid), '-o', 'lstart=,comm='],
                            capture_output=True, text=True)
    return ' '.join(result.stdout.split()) if result.returncode == 0 else None


def realm_processes(realm_root):
    path = realm_root / '.lyracore/state.json'
    if not path.exists():
        return []
    state = json.loads(path.read_text())
    return [record for name in ['gateway', 'spacetime']
            if (record := state.get(name)) is not None]


def stop_realm(realm_root, cli, env):
    marker = realm_root / '.lyracore/audit-stopping.json'
    owned = json.loads(marker.read_text()) if marker.is_file() else realm_processes(realm_root)
    for record in owned:
        live = process_identity(record['pid'])
        if live and live != record['identity']:
            raise RuntimeError(f"audit Realm PID {record['pid']} changed identity")
    # The CLI clears its PID records even when its 15-second shutdown wait expires.
    if owned:
        write_state(marker, owned)
    run([cli, 'dev', 'down'], env=env, cwd=realm_root, stdout=subprocess.DEVNULL)
    deadline = time.monotonic() + 120
    for record in owned:
        # An exec may change the command name while the Host retains its PID and start time.
        started = record['identity'].rsplit(' ', 1)[0]
        while (live := process_identity(record['pid'])) and live.rsplit(' ', 1)[0] == started:
            if time.monotonic() >= deadline:
                raise RuntimeError(f"audit Realm PID {record['pid']} did not exit after dev down")
            time.sleep(0.2)
    for port in [3000, 3724, 8085]:
        free_port(port)
    marker.unlink(missing_ok=True)


def writable_tree(root):
    for path in [root, *root.rglob('*')]:
        if path.is_symlink():
            raise RuntimeError(f'cached data contains a symlink: {path}')
        path.chmod(0o700 if path.is_dir() else 0o600)


def snapshot_path(key):
    return SNAPSHOT_STORE / key


def validate_snapshot(path, key):
    if path.exists() and not path.is_dir():
        raise RuntimeError('imported-world snapshot is not a directory')
    marker = path / 'state.json'
    if not marker.is_file():
        if path.exists():
            raise RuntimeError('imported-world snapshot has no seal')
        return False
    if not (path / 'native-data').is_dir():
        raise RuntimeError('imported-world snapshot has no native data')
    if not (path / 'native-jwt').is_dir():
        raise RuntimeError('imported-world snapshot has no Native Host JWT keys')
    state = json.loads(marker.read_text())
    manifest = dict(data=file_manifest(path / 'native-data'),
                    jwt=file_manifest(path / 'native-jwt'))
    if state['key'] != key or state['manifest'] != manifest:
        raise RuntimeError('imported-world snapshot changed after sealing')
    if path.stat().st_mode & 0o222:
        raise RuntimeError('imported-world snapshot is writable')
    for item in path.rglob('*'):
        if item.stat().st_mode & 0o222:
            raise RuntimeError(f'imported-world snapshot is writable: {item}')
    for name in ['native-owner-token', 'native-cli.toml', 'password', 'current.json']:
        if not (path / name).is_file():
            raise RuntimeError(f'imported-world snapshot is missing {name}')
    return True


def copy_snapshot(path, realm_root):
    native = realm_root / '.lyracore'
    native.mkdir(exist_ok=True)
    shutil.copytree(path / 'native-data', native / 'native-data')
    writable_tree(native / 'native-data')
    shutil.copytree(path / 'native-jwt', native / 'native-jwt')
    writable_tree(native / 'native-jwt')
    for name in ['native-owner-token', 'native-cli.toml']:
        shutil.copy2(path / name, native / name)
        (native / name).chmod(0o600)
    current = realm_root / '.import/vmangos/current.json'
    current.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(path / 'current.json', current)


def seal_snapshot(key, realm_root, password):
    SNAPSHOT_STORE.mkdir(parents=True, exist_ok=True, mode=0o700)
    SNAPSHOT_STORE.chmod(0o700)
    pending = SNAPSHOT_STORE / (key + '.pending')
    if pending.exists():
        writable_tree(pending)
        shutil.rmtree(pending)
    pending.mkdir(mode=0o700)
    native = realm_root / '.lyracore'
    shutil.copytree(native / 'native-data', pending / 'native-data')
    shutil.copytree(native / 'native-jwt', pending / 'native-jwt')
    for name in ['native-owner-token', 'native-cli.toml']:
        shutil.copy2(native / name, pending / name)
    shutil.copy2(realm_root / '.import/vmangos/current.json', pending / 'current.json')
    (pending / 'password').write_text(password + '\n')
    for item in reversed([*pending.rglob('*')]):
        item.chmod(0o500 if item.is_dir() else 0o400)
    manifest = dict(data=file_manifest(pending / 'native-data'),
                    jwt=file_manifest(pending / 'native-jwt'))
    write_state(pending / 'state.json', dict(key=key, manifest=manifest))
    (pending / 'state.json').chmod(0o400)
    pending.chmod(0o500)
    pending.rename(snapshot_path(key))
    for old in sorted((p for p in SNAPSHOT_STORE.iterdir() if p.is_dir() and p.name != key),
                      key=lambda p: p.stat().st_mtime, reverse=True)[1:]:
        writable_tree(old)
        shutil.rmtree(old)


def restore_imported_world(realm_root, seed, cli, env):
    stop_realm(realm_root, cli, env)
    native_data = realm_root / '.lyracore/native-data'
    shutil.rmtree(native_data)
    shutil.copytree(seed, native_data)
    writable_tree(native_data)
    run([cli, 'dev', 'up', '--single'], env=env, cwd=realm_root,
        stdout=subprocess.DEVNULL)


def retain_host_build(realm_root):
    native = realm_root / '.lyracore/native-host'
    cached = SCENARIO_CACHE / 'native-host'
    if native.is_dir() and not native.is_symlink():
        if cached.exists():
            raise RuntimeError('two Native Host build caches exist; inspect before cleanup')
        native.rename(cached)
        native.symlink_to(cached, target_is_directory=True)


def lock_owned_state(file, name, *, wait=False):
    try:
        fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as error:
        if wait:
            print(f'[audit] waiting for {name} ownership lock', file=sys.stderr, flush=True)
            fcntl.flock(file, fcntl.LOCK_EX)
            return
        raise RuntimeError(f'{name} is busy with another local capture') from error


def check_cargo_config(directory):
    def contains_replace_with(value):
        if isinstance(value, dict):
            return 'replace-with' in value or any(contains_replace_with(v) for v in value.values())
        return isinstance(value, list) and any(contains_replace_with(v) for v in value)

    for path in directory.glob('config*'):
        if not path.is_file():
            continue
        config = tomllib.loads(path.read_text())
        if 'source' in config or contains_replace_with(config):
            raise RuntimeError(f'Cargo source replacement is forbidden: {path}')
        if 'paths' in config:
            raise RuntimeError(f'Cargo paths overrides are forbidden: {path}')
        check_cargo_patch(config)


def check_cargo_patch(manifest):
    for entries in manifest.get('patch', {}).values():
        if any(name == 'wire-client' or entry.get('package') == 'wire-client'
               for name, entry in entries.items()):
            raise RuntimeError('wire-client patch overrides are forbidden')


def check_workspace_configs(metadata, cargo_home):
    manifests = {Path(metadata['workspace_root']) / 'Cargo.toml'}
    manifests.update(Path(p['manifest_path']) for p in metadata['packages']
                     if p['id'] in metadata['workspace_members'])
    directories = {directory / '.cargo' for manifest in manifests for directory in manifest.parents}
    for manifest in manifests:
        check_cargo_patch(tomllib.loads(manifest.read_text()))
    for directory in directories | {cargo_home}:
        check_cargo_config(directory)


def pinned_harness(metadata, revision, cargo_home):
    check_workspace_configs(metadata, cargo_home)
    resolved = {node['id'] for node in metadata['resolve']['nodes']}
    packages = [package for package in metadata['packages']
                if package['name'] == 'wire-client' and package['id'] in resolved]
    expected = f'git+https://github.com/LyraCoreProject/wire-harness?rev={revision}#{revision}'
    if len(packages) != 1 or packages[0]['source'] != expected:
        raise RuntimeError('wire-client source differs from .wire-harness-rev')
    manifest = Path(packages[0]['manifest_path']).resolve(strict=True)
    checkouts = (cargo_home / 'git/checkouts').resolve(strict=True)
    if not manifest.is_relative_to(checkouts):
        raise RuntimeError('wire-client manifest is outside Cargo git checkouts')
    env = {key: value for key, value in os.environ.items()
           if key not in {'GIT_DIR', 'GIT_INDEX_FILE', 'GIT_WORK_TREE'}}
    def checkout_git(*args):
        return run(['git', *args], cwd=manifest.parent, env=env,
                   capture_output=True, text=True).stdout.strip()
    checkout = Path(checkout_git('rev-parse', '--show-toplevel')).resolve(strict=True)
    if not checkout.is_relative_to(checkouts):
        raise RuntimeError('wire-client is not in a Cargo git checkout')
    if checkout_git('rev-parse', 'HEAD') != revision:
        raise RuntimeError('wire-client checkout HEAD differs from the harness pin')
    return manifest.parent


def wait_port(port, world=False):
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(('127.0.0.1', port), timeout=1) as connection:
                if not world:
                    return
                header = b''
                while len(header) < 4:
                    packet = connection.recv(4 - len(header))
                    if not packet:
                        raise OSError('world listener is not ready')
                    header += packet
                if int.from_bytes(header[2:], 'little') != 0x01EC:
                    raise RuntimeError('Reference Server did not send SMSG_AUTH_CHALLENGE')
                return
        except OSError:
            time.sleep(0.25)
    raise RuntimeError(f'local Reference Server port {port} did not start')


@contextmanager
def reference_server(harness, peer_count):
    with scenario_reference_server(harness, 'full-audit', peer_count, keep=False) as provenance:
        yield provenance


def peer_accounts(scenario):
    return [f'CONFORMANCE{index}' for index in range(2, len(scenario['characters']) + 1)]


def reference_ids(compose, env):
    ids = {}
    for service in ['auth', 'database', 'world']:
        value = run([*compose, 'ps', '-a', '-q', service], env=env,
                    capture_output=True, text=True).stdout.strip()
        if value:
            if len(value.splitlines()) != 1 or not re.fullmatch(r'[0-9a-f]{12,64}', value):
                raise RuntimeError(f'Reference Server {service} has an invalid container identity')
            ids[service] = value
    return ids


def reference_running(compose, env):
    return set(run([*compose, 'ps', '--status', 'running', '--services'],
                   env=env, capture_output=True, text=True).stdout.split())


def reference_database_command(compose, env):
    configured = dict(env, REFERENCE_ROOT=str(REFERENCE),
                      REFERENCE_UID=str(os.getuid()), REFERENCE_GID=str(os.getgid()))
    return ['docker', 'compose', '-f', str(Path(compose[0]).with_name('compose.yml'))], configured


def reference_dump(compose, env, path):
    path.parent.mkdir(parents=True, exist_ok=True)
    pending = path.with_suffix('.pending')
    database, configured = reference_database_command(compose, env)
    try:
        with pending.open('wb') as output:
            run([*database, 'exec', '-T', 'database', 'sh', '-c',
                 'MYSQL_PWD="$MARIADB_ROOT_PASSWORD" mariadb-dump -uroot '
                 '--single-transaction --add-drop-database '
                 '--databases realmd mangos characters logs'],
                env=configured, stdout=output)
        if pending.stat().st_size == 0:
            raise RuntimeError('Reference Server clean world dump is empty')
        pending.chmod(0o400)
        pending.replace(path)
    finally:
        pending.unlink(missing_ok=True)


def validate_reference_ownership(owned, ids):
    if ids != owned['containers']:
        raise RuntimeError('Reference Server container identities changed')


def owned_reference_ids(marker, owned, compose, env):
    ids = reference_ids(compose, env)
    if ids and not owned['containers'] and owned['phase'] == 'starting':
        for service, identity in ids.items():
            labels = json.loads(run(['docker', 'inspect', '--format', '{{json .Config.Labels}}', identity],
                                    env=env, capture_output=True, text=True).stdout)
            if (labels.get('com.docker.compose.project') != owned['project']
                    or labels.get('com.docker.compose.service') != service):
                raise RuntimeError('Reference Server startup container has another owner')
        owned['containers'] = ids
        write_state(marker, owned)
    validate_reference_ownership(owned, ids)
    return ids


def stop_reference(marker, owned, compose, env):
    ids = owned_reference_ids(marker, owned, compose, env)
    if ids:
        run(['docker', 'stop', *ids.values()], env=env, capture_output=True)


def restore_reference_world(marker, owned, compose, env):
    dump = Path(owned['dump'])
    if not dump.is_file():
        raise RuntimeError('Reference Server has no clean database snapshot')
    owned['phase'] = 'restoring'
    write_state(marker, owned)
    stop_reference(marker, owned, compose, env)
    database, configured = reference_database_command(compose, env)
    try:
        run([*database, 'start', '--wait', '--wait-timeout', '180', 'database'],
            env=configured, capture_output=True)
        with dump.open('rb') as source:
            run(['docker', 'exec', '-i', owned['containers']['database'], 'sh', '-c',
                 'MYSQL_PWD="$MARIADB_ROOT_PASSWORD" mariadb -uroot'],
                env=configured, stdin=source, capture_output=True)
    finally:
        stop_reference(marker, owned, compose, env)


@contextmanager
def scenario_reference_server(harness, key, peer_count=0, *, keep=True):
    adapter = harness / 'adapters/vmangos'
    pins = json.loads((adapter / 'pins.json').read_text())
    runtime = json.loads((REFERENCE / 'runtime-pins.json').read_text())
    if pins != runtime or (REFERENCE / 'binaries/.verified').read_text() != pins['binary_sha256']:
        raise RuntimeError('prepare the Reference Server with the pinned adapter first')
    provenance = json.loads((REFERENCE / 'provenance.json').read_text())
    expected = dict(server='vmangos', binary_release=pins['binary_tag'] + '/' + pins['binary_asset'],
                    **{name: pins[name] for name in ['binary_sha256', 'source_commit', 'database_commit']})
    if provenance != expected:
        raise RuntimeError('Reference Server provenance does not match its runtime pins')
    marker = SCENARIO_CACHE / 'reference-state.json'
    compose = [adapter / 'reference.sh']
    with (REFERENCE / '.audit.lock').open('w') as lock:
        lock_owned_state(lock, 'Reference Server', wait=True)
        owned = json.loads(marker.read_text()) if marker.is_file() else None
        if owned:
            env = dict(os.environ, COMPOSE_PROJECT_NAME=owned['project'])
            owned_reference_ids(marker, owned, compose, env)
            if owned['phase'] != 'ready' or owned['key'] != key:
                if Path(owned['dump']).is_file():
                    restore_reference_world(marker, owned, compose, env)
                elif owned['phase'] != 'starting':
                    raise RuntimeError('Reference Server has no clean database snapshot')
        else:
            if reference_running(compose, os.environ):
                raise RuntimeError('Reference Server is already running outside this scenario cache')
            for port in [13724, 18085]:
                free_port(port)
            owned = dict(phase='starting', key=key, project='lyracore-audit-' + secrets.token_hex(16),
                         adapter=str(compose[0]), containers={},
                         dump=str(SNAPSHOT_STORE / ('reference-' + pins['database_commit'] + '.sql')))
            env = dict(os.environ, COMPOSE_PROJECT_NAME=owned['project'])
            write_state(marker, owned)
        try:
            with timed('Reference Server start'):
                if not owned['containers']:
                    run([*compose, 'up', '--no-start'], env=env, capture_output=True)
                    owned_reference_ids(marker, owned, compose, env)
                if set(owned['containers']) != {'auth', 'database', 'world'}:
                    raise RuntimeError('Reference Server startup did not create all containers')
                database, configured = reference_database_command(compose, env)
                if not Path(owned['dump']).is_file():
                    run([*database, 'start', '--wait', '--wait-timeout', '180', 'database'],
                        env=configured, capture_output=True)
                    reference_dump(compose, env, Path(owned['dump']))
                # Persist dirtiness before starting world timers or allowing fixture writes.
                owned.update(phase='invalid', key=key)
                write_state(marker, owned)
                run([*database, 'start', '--wait', '--wait-timeout', '180'],
                    env=configured, capture_output=True)
                wait_port(13724)
                wait_port(18085, world=True)
                run([sys.executable, adapter / 'setup.py', '--verify-account', 'CONFORMANCE'],
                    env=env, capture_output=True)
                if peer_count:
                    run([sys.executable, adapter / 'setup.py', '--ensure-peers', str(peer_count)],
                        env=env, capture_output=True)
            yield provenance
            owned['phase'] = 'ready'
            write_state(marker, owned)
        except BaseException:
            # A failed marker write must still stop the containers whose IDs we already own.
            stop_reference(marker, owned, compose, env)
            raise
        finally:
            if not keep:
                release_reference(marker, owned, compose, env)


def release_reference(marker, owned, compose, env):
    owned_reference_ids(marker, owned, compose, env)
    if Path(owned['dump']).is_file():
        restore_reference_world(marker, owned, compose, env)
    elif owned['phase'] != 'starting':
        raise RuntimeError('Reference Server has no clean database snapshot')
    stop_reference(marker, owned, compose, env)
    run([*compose, 'down'], env=env, capture_output=True)
    marker.unlink()


def capture(wire, work, scenario, adapter, endpoints, password, env, progress=sys.stdout):
    print(f"Capturing {scenario['id']} on {adapter}", file=progress, flush=True)
    run([wire, 'candidate', work / f"{scenario['id']}.toml", work / f"{scenario['id']}-{adapter}.json",
         '--host', '127.0.0.1', '--logon-port', endpoints[0], '--world-port', endpoints[1],
         '--account', 'CONFORMANCE', '--password-stdin'], input=password,
        env=dict(env, WIRE_ADAPTER=adapter, WIRE_PEER_ACCOUNTS=','.join(peer_accounts(scenario))),
        stdout=subprocess.DEVNULL, timeout=240)


def delete_reference_character(wire, name, account, password, env, endpoints=(13724, 18085)):
    command = [wire, 'scenario', 'char-delete', '--host', '127.0.0.1',
               '--logon-port', str(endpoints[0]), '--world-port', str(endpoints[1]),
               '--account', account, '--character', name,
               '--password-stdin']
    for attempt in range(2):
        try:
            run(command, input=password, env=env, capture_output=True, timeout=60)
            return
        except subprocess.CalledProcessError as error:
            if attempt or not error.stderr or error.stderr.splitlines()[-1] not in [
                    b'Error: logon proof rejected', b'Error: server proof mismatch']:
                raise RuntimeError(f'Reference Character cleanup failed: {error.stderr.decode(errors="replace").strip()}') from error
            print('Retrying fixture Character cleanup after an SRP proof rejection.',
                  file=sys.stderr, flush=True)


@contextmanager
def local_realm(work, client_data, peer_count):
    if git('status', '--porcelain', '--untracked-files=normal'):
        raise RuntimeError('commit core changes before starting an isolated audit Realm')
    revision = (ROOT / '.lyracore-cli-rev').read_text().strip()
    if revision != 'a016c82c6848a098ecbbb07ed53704c2b7dfdff1':
        raise RuntimeError('audit Realm isolation needs review for this lyracore CLI revision')
    for port in [3000, 3724, 8085]:
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', port))
    spacetime = shutil.which('spacetime')
    if spacetime is None:
        raise RuntimeError('spacetime CLI is missing')
    realm_root = work / 'core'
    run(['git', 'worktree', 'add', '--detach', realm_root, 'HEAD'], cwd=ROOT,
        stdout=subprocess.DEVNULL)
    try:
        if (realm_root / '.lyracore').exists():
            raise RuntimeError('new audit worktree already has Realm state')
        (realm_root / 'target').symlink_to(SCRATCH / 'target', target_is_directory=True)
        cli = realm_root / 'lyracore'
        bin_dir = work / 'bin'
        bin_dir.mkdir()
        isolated = bin_dir / 'spacetime'
        isolated.write_text('#!/bin/sh\n'
                            'if [ "$1" = login ] && [ "$2" = show ] && [ "$3" = --token ]; then\n'
                            '    exit 1\n'
                            'fi\n'
                            'exec ' + shlex.quote(spacetime) + ' "$@"\n')
        isolated.chmod(0o700)
        cli_env = dict(os.environ)
        cli_env.pop('CARGO_TARGET_DIR', None)
        cli_env['PATH'] = str(bin_dir) + ':' + cli_env['PATH']
    except BaseException:
        run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
            stdout=subprocess.DEVNULL)
        raise
    stopped = False
    try:
        try:
            write_state(work / 'realm-starting.json', {'worktree': str(realm_root)})
            with timed('Realm start'):
                run([cli, 'dev', 'up', '--single'], env=cli_env, cwd=realm_root)
            with timed('import'):
                run([cli, 'import', '--accept', '--client-data', client_data],
                    env=cli_env, cwd=realm_root)
            token = realm_root / '.lyracore/native-owner-token'
            if not token.is_file():
                raise RuntimeError('isolated Realm has no Operator token')
            password = secrets.token_hex(8)
            run([cli, 'account', 'create', 'CONFORMANCE', '--password-stdin'],
                input=(password + '\n').encode(), env=cli_env, cwd=realm_root)
            for peer in range(2, peer_count + 2):
                run([cli, 'account', 'create', f'CONFORMANCE{peer}', '--password-stdin'],
                    input=(password + '\n').encode(), env=cli_env, cwd=realm_root)
            copied_token = work / 'operator-token'
            copied_token.write_bytes(token.read_bytes())
            copied_token.chmod(0o600)
            host = realm_root / '.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone'
            module = realm_root / 'target/wasm32-unknown-unknown/release/lyracore_module.wasm'
            yield password, copied_token, host, module, realm_root
        finally:
            stop_realm(realm_root, cli, cli_env)
            stopped = True
            (work / 'realm-starting.json').unlink(missing_ok=True)
    finally:
        if stopped:
            run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
                stdout=subprocess.DEVNULL)


@contextmanager
def scenario_realm(key, pins, client_data, peer_count, *, require_snapshot=False):
    if git('status', '--porcelain', '--untracked-files=normal'):
        raise RuntimeError('clean core working tree required; commit changes before scenario capture')
    if (ROOT / '.lyracore-cli-rev').read_text().strip() != 'a016c82c6848a098ecbbb07ed53704c2b7dfdff1':
        raise RuntimeError('audit Realm isolation needs review for this lyracore CLI revision')
    spacetime = shutil.which('spacetime')
    if spacetime is None:
        raise RuntimeError('spacetime CLI is missing')
    SCENARIO_CACHE.mkdir(parents=True, exist_ok=True)
    with (SCENARIO_CACHE / '.realm.lock').open('w') as lock:
        lock_owned_state(lock, 'scenario Realm')
        cache = SCENARIO_CACHE / 'realm'
        realm_root = cache / 'core'
        marker = cache / 'state.json'
        cli = realm_root / 'lyracore'
        bin_dir = cache / 'bin'
        cli_env = dict(os.environ)
        cli_env.pop('CARGO_TARGET_DIR', None)
        cli_env['PATH'] = str(bin_dir) + ':' + cli_env['PATH']
        snapshot = snapshot_path(key)
        saved = validate_snapshot(snapshot, key)
        if require_snapshot and not saved:
            raise RuntimeError(f'no imported-world snapshot for {key}; --require-snapshot forbids an import')
        current = git('rev-parse', 'HEAD')
        state = json.loads(marker.read_text()) if marker.is_file() else None
        if cache.exists() and state is None:
            raise RuntimeError('scenario Realm has no ownership marker; inspect it before cleanup')
        if state and (not saved or state['key'] != key or state['core_commit'] != current
                      or state['phase'] != 'ready'):
            if (realm_root / '.lyracore').exists():
                stop_realm(realm_root, cli, cli_env)
                retain_host_build(realm_root)
            if (realm_root / '.git').exists():
                run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
                    stdout=subprocess.DEVNULL)
            shutil.rmtree(cache)
            state = None
        if state is None:
            for port in [3000, 3724, 8085]:
                free_port(port)
            cache.mkdir()
            write_state(marker, dict(phase='starting', key=key, core_commit=current))
            try:
                run(['git', 'worktree', 'add', '--detach', realm_root, 'HEAD'], cwd=ROOT,
                    stdout=subprocess.DEVNULL)
                if (realm_root / '.lyracore').exists():
                    raise RuntimeError('new scenario worktree already has Realm state')
                (SCENARIO_CACHE / 'target').mkdir(exist_ok=True)
                (realm_root / 'target').symlink_to(SCENARIO_CACHE / 'target', target_is_directory=True)
                bin_dir.mkdir()
                isolated = bin_dir / 'spacetime'
                isolated.write_text('#!/bin/sh\n'
                                    'if [ "$1" = login ] && [ "$2" = show ] && [ "$3" = --token ]; then\n'
                                    '    exit 1\n'
                                    'fi\n'
                                    'exec ' + shlex.quote(spacetime) + ' "$@"\n')
                isolated.chmod(0o700)
                if saved:
                    copy_snapshot(snapshot, realm_root)
                native = realm_root / '.lyracore'
                native.mkdir(exist_ok=True)
                (SCENARIO_CACHE / 'native-host').mkdir(exist_ok=True)
                (native / 'native-host').symlink_to(SCENARIO_CACHE / 'native-host', target_is_directory=True)
                with timed('Realm start'):
                    run([cli, 'dev', 'up', '--single'], env=cli_env, cwd=realm_root,
                        stdout=subprocess.DEVNULL)
                if not saved:
                    with timed('import'):
                        run([cli, 'import', '--accept', '--client-data', client_data], env=cli_env,
                            cwd=realm_root, stdout=subprocess.DEVNULL)
                    imported = json.loads((realm_root / '.import/vmangos/current.json').read_text())
                    if imported['commit'] != pins['database_commit']:
                        raise RuntimeError('imported vmangos data differs from Reference Server pin')
                    password = secrets.token_hex(8)
                    for account in ['CONFORMANCE', *(f'CONFORMANCE{peer}' for peer in range(2, peer_count + 2))]:
                        run([cli, 'account', 'create', account, '--password-stdin'],
                            input=(password + '\n').encode(), env=cli_env, cwd=realm_root,
                            stdout=subprocess.DEVNULL)
                    with timed('imported world snapshot'):
                        stop_realm(realm_root, cli, cli_env)
                        seal_snapshot(key, realm_root, password)
                        run([cli, 'dev', 'up', '--single'], env=cli_env, cwd=realm_root,
                            stdout=subprocess.DEVNULL)
                else:
                    print('[audit] import: 0.00s (cached snapshot)', file=sys.stderr, flush=True)
                state = dict(phase='ready', key=key, core_commit=current,
                             native_host_sha256=digest(realm_root / '.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone'),
                             module_sha256=digest(realm_root / 'target/wasm32-unknown-unknown/release/lyracore_module.wasm'),
                             database_commit=pins['database_commit'])
                write_state(marker, state)
            except BaseException:
                if (realm_root / '.lyracore').exists():
                    stop_realm(realm_root, cli, cli_env)
                if (realm_root / '.git').exists():
                    run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
                        stdout=subprocess.DEVNULL)
                shutil.rmtree(cache)
                raise
        else:
            with timed('Realm start'):
                restore_imported_world(realm_root, snapshot / 'native-data', cli, cli_env)
            print('[audit] import: 0.00s (cached snapshot)', file=sys.stderr, flush=True)
            state['native_host_sha256'] = digest(realm_root / '.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone')
            state['module_sha256'] = digest(realm_root / 'target/wasm32-unknown-unknown/release/lyracore_module.wasm')
            write_state(marker, state)
        host = realm_root / '.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone'
        module = realm_root / 'target/wasm32-unknown-unknown/release/lyracore_module.wasm'
        if digest(host) != state['native_host_sha256'] or digest(module) != state['module_sha256']:
            raise RuntimeError('cached Realm binaries changed')
        token = realm_root / '.lyracore/native-owner-token'
        if not token.is_file():
            raise RuntimeError('cached Realm has no Operator token')
        password = (snapshot / 'password').read_text().strip()
        yield password, token, host, module, realm_root


def capture_servers(harness, wire, work, client_data):
    env = dict(os.environ, WIRE_CRASH_DIR=str(work / 'diagnostics'))
    scenarios = json.loads((work / 'catalogue.json').read_text())
    peer_count = max(len(scenario['characters']) - 1 for scenario in scenarios)
    with local_realm(work, client_data, peer_count) as (password, token, host, module, realm_root):
        host_hash = digest(host)
        module_hash = digest(module)
        with reference_server(harness, peer_count) as provenance:
            reference_env = dict(env, WIRE_GM_ACCOUNT='CONFORMANCEGM', WIRE_GM_CHARACTER='Conformsetup')
            for scenario in scenarios:
                with timed(f"{scenario['id']} Reference Server run"):
                    accounts = ['CONFORMANCE', *peer_accounts(scenario)]
                    passwords = []
                    for index, account in enumerate(accounts):
                        name = 'player-password' if index == 0 else f'peer{index + 1}-password'
                        secret = (REFERENCE / 'credentials' / name).read_bytes().rstrip(b'\n') + b'\n'
                        passwords.append(secret)
                        character = scenario['characters'][index]
                        delete_reference_character(wire, character, account, secret, env)
                    passwords.append((REFERENCE / 'credentials/gm-password').read_bytes().rstrip(b'\n') + b'\n')
                    capture(wire, work, scenario, 'vmangos', [13724, 18085], b''.join(passwords), reference_env)
        env.update(WIRE_REALM_URL='http://127.0.0.1:3000',
                   WIRE_OPERATOR_TOKEN_FILE=str(token), WIRE_SHARD='lyracore')
        for scenario in scenarios:
            with timed(f"{scenario['id']} Realm run"):
                capture(wire, work, scenario, 'lyracore', [3724, 8085],
                        (password + '\n').encode() * len(scenario['characters']), env)
        if digest(host) != host_hash or digest(module) != module_hash:
            raise RuntimeError('local Realm binaries changed during capture')
        snapshot = json.loads((realm_root / '.import/vmangos/current.json').read_text())
        content = (f"lyracore import alliance-single, vmangos db_latest {snapshot['commit']}, "
                   'client data from the supplied path; imported creature spawns retained')
        return provenance, host_hash, module_hash, content


def capture_scenario(harness, wire, work, scenario, key, pins, client_data, *, require_snapshot=False):
    env = dict(os.environ, WIRE_CRASH_DIR=str(work / 'diagnostics'))
    realm_env = dict(env)
    catalogue = json.loads((work / 'catalogue.json').read_text())
    entry = next((item for item in catalogue if item['id'] == scenario), None)
    if entry is None:
        raise RuntimeError(f'unknown catalogue scenario {scenario}')
    characters = entry['characters']
    peer_count = len(characters) - 1
    max_peer_count = max(len(item['characters']) - 1 for item in catalogue)
    with scenario_realm(key, pins, client_data, max_peer_count,
                        require_snapshot=require_snapshot) as (password, token, host, module, realm_root):
        realm_env.update(WIRE_REALM_URL='http://127.0.0.1:3000',
                         WIRE_OPERATOR_TOKEN_FILE=str(token), WIRE_SHARD='lyracore')
        with scenario_reference_server(harness, key, peer_count) as provenance:
            (work / 'reference.json').write_text(json.dumps(provenance))
            reference_env = dict(env, WIRE_GM_ACCOUNT='CONFORMANCEGM', WIRE_GM_CHARACTER='Conformsetup')
            with timed(f'{scenario} Reference Server run'):
                accounts = ['CONFORMANCE', *peer_accounts(entry)]
                passwords = []
                for index, account in enumerate(accounts):
                    name = 'player-password' if index == 0 else f'peer{index + 1}-password'
                    secret = (REFERENCE / 'credentials' / name).read_bytes().rstrip(b'\n') + b'\n'
                    passwords.append(secret)
                    delete_reference_character(wire, characters[index], account, secret, env)
                passwords.append((REFERENCE / 'credentials/gm-password').read_bytes().rstrip(b'\n') + b'\n')
                capture(wire, work, entry, 'vmangos', [13724, 18085], b''.join(passwords),
                        reference_env, progress=sys.stderr)
        with timed(f'{scenario} Realm run'):
            for index, character in enumerate(characters):
                account = 'CONFORMANCE' if index == 0 else f'CONFORMANCE{index + 1}'
                delete_reference_character(wire, character, account, (password + '\n').encode(),
                                           realm_env, endpoints=(3724, 8085))
            capture(wire, work, entry, 'lyracore', [3724, 8085],
                    (password + '\n').encode() * len(characters), realm_env, progress=sys.stderr)
        state = json.loads((SCENARIO_CACHE / 'realm/state.json').read_text())
        snapshot = json.loads((realm_root / '.import/vmangos/current.json').read_text())
        if (digest(host) != state['native_host_sha256'] or digest(module) != state['module_sha256']
                or snapshot['commit'] != state['database_commit']):
            raise RuntimeError('cached Realm Module, Native Host or imported data changed during capture')


def setup_failure(scenario, json_mode, diagnostic):
    if json_mode:
        print(json.dumps({'schema_version': 1, 'scenario': scenario,
                          'error': {'kind': 'setup_failed', 'diagnostic': diagnostic}}))
    else:
        print(diagnostic, file=sys.stderr)
    return 2


def comparison_signatures(path, tier, scenario):
    result = json.loads(path.read_text())
    if result['schema_version'] != 2 or result['tier'] != tier or len(result['scenarios']) != 1:
        raise RuntimeError(f'{tier} did not produce one schema-2 scenario comparison')
    [entry] = result['scenarios']
    if entry['id'] != scenario:
        raise RuntimeError(f'{tier} compared another scenario')
    signatures = set()
    for difference in entry['comparison']['differences']:
        if difference['classification'] not in ['rule', 'data']:
            raise RuntimeError(f'{tier} comparison has an unknown classification')
        signature = {name: difference.get(name) for name in
                     ['rule', 'character', 'checkpoint', 'stream', 'opcode', 'field_path']}
        signatures.add(json.dumps(dict(classification=difference['classification'],
                                       signature=signature), sort_keys=True))
    return result, entry, signatures


def agreement_result(tier1, audit, scenario):
    tier1_result, tier1_entry, first = comparison_signatures(tier1, 'tier1', scenario)
    audit_result, audit_entry, second = comparison_signatures(audit, 'tier1', scenario)
    for name in ['core_commit', 'harness_revision']:
        if tier1_result[name] != audit_result[name]:
            raise RuntimeError(f'tier 1 and audit have different {name}')
    for name in ['behavior', 'scenario_sha256']:
        if tier1_entry[name] != audit_entry[name]:
            raise RuntimeError(f'tier 1 and audit have different {name}')
    return dict(schema_version=1, scenario=scenario,
                tier1_only=[json.loads(item) for item in sorted(first - second)],
                audit_only=[json.loads(item) for item in sorted(second - first)])


def run_scenario(args, harness, target, core_commit):
    if git('status', '--porcelain', '--untracked-files=normal'):
        raise RuntimeError('clean core working tree required; commit changes before scenario capture')
    with timed('Module schema fingerprint'):
        schema_hash = module_schema_hash()
    key, pins = scenario_key(harness, args.client_data, schema_hash)
    with tempfile.TemporaryDirectory(prefix='scenario-', dir=SCENARIO_CACHE) as directory:
        work = Path(directory)
        os.environ['TMPDIR'] = str(work)
        audit = target / 'debug/examples/audit'
        wire = target / 'debug/vanilla-wire'
        if args.agreement:
            with timed('tier 1 agreement run'):
                tier1 = subprocess.run([str(ROOT / 'conformance/run'), args.scenario, '--json',
                                        '--result', str(work / 'tier1.json')],
                                       cwd=ROOT, env=dict(os.environ,
                                                          LYRACORE_CONFORMANCE_SCRATCH=str(SCRATCH / 'tier1')),
                                       capture_output=True, text=True)
            if tier1.returncode not in [0, 1] or not (work / 'tier1.json').is_file():
                raise RuntimeError('tier 1 did not produce a comparison: '
                                   + (tier1.stderr + tier1.stdout).strip())
        run_with_diagnostic([audit, 'export', work])
        capture_scenario(harness, wire, work, args.scenario, key, pins, str(args.client_data),
                         require_snapshot=args.require_snapshot)
        if core_commit != git('rev-parse', 'HEAD') or git('status', '--porcelain', '--untracked-files=normal'):
            raise RuntimeError('core changed during scenario capture; commit and rerun')
        with timed('comparison and output'):
            run_with_diagnostic([audit, 'check', work, args.scenario])
            environment = dict(os.environ, LYRACORE_CONFORMANCE_RESULT=str(work / 'result.json'),
                               LYRACORE_CONFORMANCE_AUDIT_BASELINE=str(work / 'baseline.json'))
            command = [target / 'debug/examples/run', args.scenario]
            if args.json:
                command.append('--json')
            rendered = subprocess.run([str(arg) for arg in command], cwd=ROOT, env=environment,
                                      capture_output=True, text=True)
            if rendered.returncode not in [0, 1] or not rendered.stdout:
                raise RuntimeError('could not render the completed scenario comparison: '
                                   + rendered.stderr.strip())
            if args.agreement:
                agreement = agreement_result(work / 'tier1.json', work / 'result.json', args.scenario)
                if args.json:
                    print(json.dumps(dict(audit=json.loads(rendered.stdout), agreement=agreement),
                                     indent=2))
                else:
                    print(rendered.stdout, end='')
                    for label, explanation in [('tier1_only', 'likely fixture artefact'),
                                               ('audit_only', 'fixture blind spot')]:
                        print(f"{label} ({explanation}): {len(agreement[label])}")
                        for item in agreement[label]:
                            print('  ' + json.dumps(item, sort_keys=True))
                return int(bool(tier1.returncode or rendered.returncode
                                or agreement['tier1_only'] or agreement['audit_only']))
            print(rendered.stdout, end='')
            return rendered.returncode


def scenario_main(args):
    if git('status', '--porcelain', '--untracked-files=normal'):
        raise RuntimeError('clean core working tree required; commit changes before scenario capture')
    if shutil.which('docker') is None:
        raise RuntimeError('Docker CLI is missing')
    docker = subprocess.run(['docker', 'info', '--format', '{{.ServerVersion}}'],
                            capture_output=True, text=True)
    if docker.returncode:
        raise RuntimeError('Docker is unavailable or docker group access is missing: '
                           + docker.stderr.strip())
    SCENARIO_CACHE.mkdir(parents=True, exist_ok=True)
    temporary = SCENARIO_CACHE / 'tmp'
    temporary.mkdir(exist_ok=True)
    os.environ['TMPDIR'] = str(temporary)
    target = SCENARIO_CACHE / 'target'
    os.environ['CARGO_TARGET_DIR'] = str(target)
    os.environ['PATH'] = str(Path.home() / '.cargo/bin') + ':' + os.environ['PATH']
    os.chdir(ROOT)
    core_commit = git('rev-parse', 'HEAD')
    revision = next(line.split()[1] for line in (ROOT / '.wire-harness-rev').read_text().splitlines()
                    if line.strip() and not line.startswith('#'))
    for directory in [ROOT, *ROOT.parents]:
        check_cargo_config(directory / '.cargo')
    cargo_home = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))
    check_cargo_config(cargo_home)
    metadata = json.loads(run(['cargo', 'metadata', '--locked', '--format-version', '1'],
                              capture_output=True, text=True).stdout)
    harness = pinned_harness(metadata, revision, cargo_home)
    with timed('build'):
        run(['cargo', 'build', '--locked', '-p', 'lyracore-conformance',
             '--example', 'audit', '--example', 'run'], stdout=subprocess.DEVNULL)
        run(['cargo', 'build', '--locked', '--manifest-path', harness / 'Cargo.toml',
             '--bin', 'vanilla-wire'], stdout=subprocess.DEVNULL)
    return run_scenario(args, harness, target, core_commit)


def stop_scenario_cache():
    SCENARIO_CACHE.mkdir(parents=True, exist_ok=True)
    with (SCENARIO_CACHE / '.realm.lock').open('w') as lock:
        lock_owned_state(lock, 'scenario Realm')
        cache = SCENARIO_CACHE / 'realm'
        realm_root = cache / 'core'
        if cache.exists():
            marker = cache / 'state.json'
            if not marker.is_file():
                raise RuntimeError('scenario Realm has no ownership marker; inspect it before cleanup')
            cli_env = dict(os.environ)
            cli_env.pop('CARGO_TARGET_DIR', None)
            cli_env['PATH'] = (str(cache / 'bin') + ':' + str(Path.home() / '.cargo/bin')
                               + ':' + cli_env['PATH'])
            if (realm_root / 'lyracore').is_file() and (realm_root / '.lyracore').exists():
                stop_realm(realm_root, realm_root / 'lyracore', cli_env)
            elif realm_processes(realm_root):
                raise RuntimeError('scenario Realm has owned PIDs but no CLI to stop them')
            else:
                for port in [3000, 3724, 8085]:
                    free_port(port)
            if (realm_root / '.git').exists():
                run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
                    stdout=subprocess.DEVNULL)
            shutil.rmtree(cache)
        marker = SCENARIO_CACHE / 'reference-state.json'
        if marker.is_file():
            with (REFERENCE / '.audit.lock').open('w') as reference_lock:
                lock_owned_state(reference_lock, 'Reference Server', wait=True)
                owned = json.loads(marker.read_text())
                env = dict(os.environ, COMPOSE_PROJECT_NAME=owned['project'])
                compose = [owned['adapter']]
                release_reference(marker, owned, compose, env)


def cleanup_audit_work(work):
    marker = work / 'realm-starting.json'
    if marker.exists():
        print(f'[audit] keeping {work}: Realm shutdown did not finish',
              file=sys.stderr, flush=True)
        return
    shutil.rmtree(work)


def main():
    parser = argparse.ArgumentParser(description='Compare catalogue scenarios on two local servers.')
    parser.add_argument('--date', default=datetime.date.today().isoformat())
    parser.add_argument('--output', type=Path, default=ROOT / 'conformance/reports')
    parser.add_argument('--client-data', type=Path)
    parser.add_argument('--scenario')
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--json', action='store_true')
    parser.add_argument('--agreement', action='store_true')
    parser.add_argument('--require-snapshot', action='store_true',
                        help='fail instead of importing when no matching snapshot exists')
    parser.add_argument('--stop', action='store_true')
    args = parser.parse_args(json.loads(os.environ.get('LYRACORE_AUDIT_ARGS', '[]')))
    if args.stop:
        if args.scenario or args.check or args.json or args.agreement or args.require_snapshot:
            parser.error('--stop cannot be combined with scenario check options')
        stop_scenario_cache()
        return 0
    if args.check != bool(args.scenario):
        parser.error('--scenario ID requires --check, and --check requires --scenario ID')
    if args.json and not args.scenario:
        parser.error('--json requires --scenario ID --check')
    if args.agreement and not args.scenario:
        parser.error('--agreement requires --scenario ID --check')
    if args.require_snapshot and not args.scenario:
        parser.error('--require-snapshot requires --scenario ID --check')
    if args.client_data is None:
        config = ROOT / '.lyracore/config.json'
        if config.is_file():
            args.client_data = json.loads(config.read_text()).get('client_data')
    if args.scenario:
        if not args.client_data or not Path(args.client_data).is_dir():
            return setup_failure(args.scenario, args.json,
                                 'client data missing; pass --client-data with an existing 1.12.1 client Data directory')
        args.client_data = Path(args.client_data).resolve()
        try:
            with timed('total scenario check'):
                return scenario_main(args)
        except Exception as error:
            return setup_failure(args.scenario, args.json, str(error))
    if not re.fullmatch(r'\d{4}-\d{2}-\d{2}(?:-[a-z0-9]+)?', args.date):
        parser.error('--date must be YYYY-MM-DD or YYYY-MM-DD-suffix')
    try:
        calendar_date = datetime.date.fromisoformat(args.date[:10])
    except ValueError:
        parser.error('--date is not a calendar date')
    if calendar_date.isoformat() != args.date[:10]:
        parser.error('--date is not a calendar date')
    if any((args.output / f'{args.date}.{suffix}').exists() for suffix in ['md', 'json', 'traces']):
        parser.error('report already exists; choose a new date or suffix')
    if not args.client_data or not Path(args.client_data).is_dir():
        parser.error('pass --client-data with an existing 1.12.1 client Data directory')
    args.client_data = Path(args.client_data).resolve()
    SCRATCH.mkdir(parents=True, exist_ok=True)
    temporary = SCRATCH / 'tmp'
    temporary.mkdir(exist_ok=True)
    os.environ['TMPDIR'] = str(temporary)
    target = SCRATCH / 'target'
    os.environ['CARGO_TARGET_DIR'] = str(target)
    os.environ['PATH'] = str(Path.home() / '.cargo/bin') + ':' + os.environ['PATH']
    os.chdir(ROOT)
    core_commit = git('rev-parse', 'HEAD')
    dirty = git('status', '--porcelain', '--untracked-files=normal')
    revision = next(line.split()[1] for line in (ROOT / '.wire-harness-rev').read_text().splitlines()
                    if line.strip() and not line.startswith('#'))
    for directory in [ROOT, *ROOT.parents]:
        check_cargo_config(directory / '.cargo')
    check_cargo_config(Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo')))
    metadata = json.loads(run(['cargo', 'metadata', '--locked', '--format-version', '1'],
                              capture_output=True, text=True).stdout)
    cargo_home = Path(os.environ.get('CARGO_HOME', Path.home() / '.cargo'))
    harness = pinned_harness(metadata, revision, cargo_home)
    run(['cargo', 'build', '--locked', '-p', 'lyracore-conformance', '--example', 'audit'])
    run(['cargo', 'build', '--locked', '--manifest-path', harness / 'Cargo.toml', '--bin', 'vanilla-wire'])
    audit = target / 'debug/examples/audit'
    work = Path(tempfile.mkdtemp(prefix='audit-', dir=SCRATCH))
    try:
        tempfile.tempdir = str(work)
        os.environ['TMPDIR'] = str(work)
        run([audit, 'export', work])
        provenance, host_hash, module_hash, content = capture_servers(
            harness, target / 'debug/vanilla-wire', work, str(args.client_data))
        if core_commit != git('rev-parse', 'HEAD') or dirty != git('status', '--porcelain', '--untracked-files=normal'):
            raise RuntimeError('core changed during capture; rerun the audit')
        (work / 'run.json').write_text(json.dumps(dict(
            core_commit=core_commit, working_tree_dirty=bool(dirty), harness_revision=revision,
            recorded_at_unix_ms=time.time_ns() // 1_000_000, native_host_sha256=host_hash,
            module_sha256=module_hash, content=content, reference=provenance)))
        with timed('report'):
            run([audit, 'report', work, args.output, args.date])
    finally:
        cleanup_audit_work(work)


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    sys.exit(main())
