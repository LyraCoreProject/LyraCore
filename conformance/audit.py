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
SCRATCH = Path.home() / '.cache/lyracore-scratch/conformance'
REFERENCE = Path.home() / '.cache/lyracore-scratch/reference-server'
SCENARIO_CACHE = Path.home() / '.cache/lyracore-scratch/scenario-audit'


def run(args, **options):
    return subprocess.run([str(arg) for arg in args], check=True, **options)


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


def scenario_key(core_commit, revision, harness, client_data):
    pins = json.loads((harness / 'adapters/vmangos/pins.json').read_text())
    with timed('client data fingerprint'):
        client_hash = client_fingerprint(client_data)
    key = dict(core_commit=core_commit, harness_revision=revision, reference=pins,
               cli_revision=(ROOT / '.lyracore-cli-rev').read_text().strip(),
               client_data=str(client_data), client_sha256=client_hash)
    return hashlib.sha256(json.dumps(key, sort_keys=True).encode()).hexdigest(), pins


def free_port(port):
    try:
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', port))
    except OSError as error:
        raise RuntimeError(f'loopback port {port} is unavailable: {error}') from error


def write_state(path, value):
    pending = path.with_suffix('.pending')
    pending.write_text(json.dumps(value))
    pending.replace(path)


def restore_imported_world(realm_root, seed, cli, env):
    run([cli, 'dev', 'down'], env=env, cwd=realm_root, stdout=subprocess.DEVNULL)
    native_data = realm_root / '.lyracore/native-data'
    shutil.rmtree(native_data)
    shutil.copytree(seed, native_data)
    run([cli, 'dev', 'up', '--single'], env=env, cwd=realm_root,
        stdout=subprocess.DEVNULL)


def lock_owned_state(file, name):
    try:
        fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError as error:
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
    adapter = harness / 'adapters/vmangos'
    pins = json.loads((adapter / 'pins.json').read_text())
    runtime = json.loads((REFERENCE / 'runtime-pins.json').read_text())
    if pins != runtime or (REFERENCE / 'binaries/.verified').read_text() != pins['binary_sha256']:
        raise RuntimeError('prepare the Reference Server with the pinned adapter first')
    provenance = json.loads((REFERENCE / 'provenance.json').read_text())
    expected = dict(server='vmangos', binary_release=pins['binary_tag'] + '/' + pins['binary_asset'],
                    **{key: pins[key] for key in ['binary_sha256', 'source_commit', 'database_commit']})
    if provenance != expected:
        raise RuntimeError('Reference Server provenance does not match its runtime pins')
    compose = [adapter / 'reference.sh']
    with (REFERENCE / '.audit.lock').open('w') as lock:
        lock_owned_state(lock, 'Reference Server')
        if run([*compose, 'ps', '--status', 'running', '-q'], capture_output=True, text=True).stdout.strip():
            raise RuntimeError('Reference Server is already running; this audit only owns servers it starts')
        for port in [13724, 18085]:
            with socket.socket() as listener:
                listener.bind(('127.0.0.1', port))
        try:
            with timed('Reference Server start'):
                run([*compose, 'up', '-d', '--wait', '--wait-timeout', '180'])
                wait_port(13724)
                wait_port(18085, world=True)
                run([sys.executable, adapter / 'setup.py', '--verify-account', 'CONFORMANCE'])
                if peer_count:
                    run([sys.executable, adapter / 'setup.py', '--ensure-peers', str(peer_count)])
            yield provenance
        finally:
            run([*compose, 'down'])


def peer_accounts(scenario):
    return [f'CONFORMANCE{index}' for index in range(2, len(scenario['characters']) + 1)]


@contextmanager
def scenario_reference_server(harness, key):
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
        lock_owned_state(lock, 'Reference Server')
        running = set(run([*compose, 'ps', '--status', 'running', '--services'],
                          capture_output=True, text=True).stdout.split())
        owned = json.loads(marker.read_text()) if marker.is_file() else None
        if running and (not owned or running != {'auth', 'database', 'world'}):
            raise RuntimeError('Reference Server is running outside this scenario cache or has partial services')
        if running and owned['key'] != key:
            run([*compose, 'down'], capture_output=True)
            running = set()
            marker.unlink()
        if not running:
            for port in [13724, 18085]:
                free_port(port)
            try:
                with timed('Reference Server start'):
                    run([*compose, 'up', '-d', '--wait', '--wait-timeout', '180'], capture_output=True)
                    wait_port(13724)
                    wait_port(18085, world=True)
                    run([sys.executable, adapter / 'setup.py', '--verify-account', 'CONFORMANCE'],
                        capture_output=True)
            except BaseException:
                run([*compose, 'down'], capture_output=True)
                raise
            write_state(marker, {'key': key, 'adapter': str(compose[0])})
        else:
            with timed('Reference Server start'):
                wait_port(13724)
                wait_port(18085, world=True)
        yield provenance


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
    try:
        try:
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
            run([cli, 'dev', 'down'], env=cli_env, cwd=realm_root)
    finally:
        run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
            stdout=subprocess.DEVNULL)


@contextmanager
def scenario_realm(key, pins, client_data):
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
        state = json.loads(marker.read_text()) if marker.is_file() else None
        seed = cache / 'imported-world'
        cli = realm_root / 'lyracore'
        bin_dir = cache / 'bin'
        cli_env = dict(os.environ)
        cli_env.pop('CARGO_TARGET_DIR', None)
        cli_env['PATH'] = str(bin_dir) + ':' + cli_env['PATH']
        if cache.exists() and (state is None or state['key'] != key
                               or state.get('seed_manifest') != file_manifest(seed)):
            if realm_root.exists():
                if (realm_root / '.lyracore').exists():
                    run([cli, 'dev', 'down'], env=cli_env, cwd=realm_root,
                        stdout=subprocess.DEVNULL)
                run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
                    stdout=subprocess.DEVNULL)
            shutil.rmtree(cache)
            state = None
        fresh = state is None
        if state is None:
            for port in [3000, 3724, 8085]:
                free_port(port)
            cache.mkdir()
            run(['git', 'worktree', 'add', '--detach', realm_root, 'HEAD'], cwd=ROOT,
                stdout=subprocess.DEVNULL)
            try:
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
                with timed('Realm start'):
                    run([cli, 'dev', 'up', '--single'], env=cli_env, cwd=realm_root,
                        stdout=subprocess.DEVNULL)
                with timed('import'):
                    run([cli, 'import', '--accept', '--client-data', client_data], env=cli_env,
                        cwd=realm_root, stdout=subprocess.DEVNULL)
                password = secrets.token_hex(8)
                run([cli, 'account', 'create', 'CONFORMANCE', '--password-stdin'],
                    input=(password + '\n').encode(), env=cli_env, cwd=realm_root,
                    stdout=subprocess.DEVNULL)
                secret = cache / 'password'
                secret.write_text(password + '\n')
                secret.chmod(0o600)
                host = realm_root / '.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone'
                module = realm_root / 'target/wasm32-unknown-unknown/release/lyracore_module.wasm'
                snapshot = json.loads((realm_root / '.import/vmangos/current.json').read_text())
                if snapshot['commit'] != pins['database_commit']:
                    raise RuntimeError('imported vmangos data differs from Reference Server pin')
                with timed('imported world snapshot'):
                    run([cli, 'dev', 'down'], env=cli_env, cwd=realm_root,
                        stdout=subprocess.DEVNULL)
                    shutil.copytree(realm_root / '.lyracore/native-data', seed)
                    run([cli, 'dev', 'up', '--single'], env=cli_env, cwd=realm_root,
                        stdout=subprocess.DEVNULL)
                state = dict(key=key, native_host_sha256=digest(host), module_sha256=digest(module),
                             database_commit=snapshot['commit'], seed_manifest=file_manifest(seed))
                write_state(marker, state)
            except BaseException:
                if (realm_root / '.lyracore').exists():
                    run([cli, 'dev', 'down'], env=cli_env, cwd=realm_root,
                        stdout=subprocess.DEVNULL)
                run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
                    stdout=subprocess.DEVNULL)
                shutil.rmtree(cache)
                raise
        else:
            with timed('Realm start'):
                restore_imported_world(realm_root, seed, cli, cli_env)
            print('[audit] import: 0.00s (cached)', file=sys.stderr)
        host = realm_root / '.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone'
        module = realm_root / 'target/wasm32-unknown-unknown/release/lyracore_module.wasm'
        snapshot = json.loads((realm_root / '.import/vmangos/current.json').read_text())
        if (digest(host) != state['native_host_sha256'] or digest(module) != state['module_sha256']
                or snapshot['commit'] != state['database_commit']):
            raise RuntimeError('cached Realm Module, Native Host or imported data changed')
        token = realm_root / '.lyracore/native-owner-token'
        if not token.is_file():
            raise RuntimeError('cached Realm has no Operator token')
        password = (cache / 'password').read_text().strip()
        yield password, token, host, module, realm_root, fresh


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


def capture_scenario(harness, wire, work, scenario, key, pins, client_data):
    env = dict(os.environ, WIRE_CRASH_DIR=str(work / 'diagnostics'))
    realm_env = dict(env)
    entry, = json.loads((work / 'catalogue.json').read_text())
    with scenario_realm(key, pins, client_data) as (password, token, host, module, realm_root, fresh):
        realm_env.update(WIRE_REALM_URL='http://127.0.0.1:3000',
                         WIRE_OPERATOR_TOKEN_FILE=str(token), WIRE_SHARD='lyracore')
        if fresh:
            with timed(f'{scenario} Realm prime run'):
                capture(wire, work, entry, 'lyracore', [3724, 8085],
                        (password + '\n').encode(), realm_env, progress=sys.stderr)
            (work / f'{scenario}-lyracore.json').unlink()
            cli_env = dict(os.environ)
            cli_env.pop('CARGO_TARGET_DIR', None)
            cli_env['PATH'] = str(SCENARIO_CACHE / 'realm/bin') + ':' + cli_env['PATH']
            with timed('Realm reset after prime'):
                restore_imported_world(realm_root, SCENARIO_CACHE / 'realm/imported-world',
                                       realm_root / 'lyracore', cli_env)
        with scenario_reference_server(harness, key) as provenance:
            (work / 'reference.json').write_text(json.dumps(provenance))
            passwords = b''.join((REFERENCE / 'credentials' / name).read_bytes().rstrip(b'\n') + b'\n'
                                 for name in ['player-password', 'gm-password'])
            reference_env = dict(env, WIRE_GM_ACCOUNT='CONFORMANCEGM', WIRE_GM_CHARACTER='Conformsetup')
            with timed(f'{scenario} Reference Server run'):
                delete_reference_character(wire, entry['character'],
                                           passwords.splitlines()[0] + b'\n', env)
                capture(wire, work, entry, 'vmangos', [13724, 18085], passwords,
                        reference_env, progress=sys.stderr)
        with timed(f'{scenario} Realm run'):
            delete_reference_character(wire, entry['character'], (password + '\n').encode(), realm_env,
                                       endpoints=(3724, 8085))
            capture(wire, work, entry, 'lyracore', [3724, 8085],
                    (password + '\n').encode(), realm_env, progress=sys.stderr)
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


def run_scenario(args, harness, target, core_commit, revision):
    if git('status', '--porcelain', '--untracked-files=normal'):
        raise RuntimeError('clean core working tree required; commit changes before scenario capture')
    key, pins = scenario_key(core_commit, revision, harness, args.client_data)
    with tempfile.TemporaryDirectory(prefix='scenario-', dir=SCENARIO_CACHE) as directory:
        work = Path(directory)
        os.environ['TMPDIR'] = str(work)
        audit = target / 'debug/examples/audit'
        wire = target / 'debug/vanilla-wire'
        run([audit, 'export-one', work, args.scenario], capture_output=True)
        capture_scenario(harness, wire, work, args.scenario, key, pins, str(args.client_data))
        if core_commit != git('rev-parse', 'HEAD') or git('status', '--porcelain', '--untracked-files=normal'):
            raise RuntimeError('core changed during scenario capture; commit and rerun')
        with timed('comparison and output'):
            run([audit, 'check', work, args.scenario], capture_output=True)
            environment = dict(os.environ, LYRACORE_CONFORMANCE_RESULT=str(work / 'result.json'),
                               LYRACORE_CONFORMANCE_AUDIT_BASELINE=str(work / 'baseline.json'))
            command = [target / 'debug/examples/run', args.scenario]
            if args.json:
                command.append('--json')
            rendered = subprocess.run([str(arg) for arg in command], cwd=ROOT, env=environment,
                                      capture_output=True, text=True)
            if rendered.stdout:
                print(rendered.stdout, end='')
            if rendered.returncode not in [0, 1] or not rendered.stdout:
                raise RuntimeError('could not render the completed scenario comparison: '
                                   + rendered.stderr.strip())
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
    return run_scenario(args, harness, target, core_commit, revision)


def stop_scenario_cache():
    SCENARIO_CACHE.mkdir(parents=True, exist_ok=True)
    with (SCENARIO_CACHE / '.realm.lock').open('w') as lock:
        lock_owned_state(lock, 'scenario Realm')
        cache = SCENARIO_CACHE / 'realm'
        realm_root = cache / 'core'
        if cache.exists():
            cli_env = dict(os.environ)
            cli_env.pop('CARGO_TARGET_DIR', None)
            cli_env['PATH'] = (str(cache / 'bin') + ':' + str(Path.home() / '.cargo/bin')
                               + ':' + cli_env['PATH'])
            if (realm_root / 'lyracore').is_file() and (realm_root / '.lyracore').exists():
                run([realm_root / 'lyracore', 'dev', 'down'], env=cli_env, cwd=realm_root,
                    stdout=subprocess.DEVNULL)
            if (realm_root / '.git').exists():
                run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
                    stdout=subprocess.DEVNULL)
            shutil.rmtree(cache)
        marker = SCENARIO_CACHE / 'reference-state.json'
        if marker.is_file():
            with (REFERENCE / '.audit.lock').open('w') as reference_lock:
                lock_owned_state(reference_lock, 'Reference Server')
                adapter = json.loads(marker.read_text())['adapter']
                run([adapter, 'down'], capture_output=True)
                marker.unlink()


def main():
    parser = argparse.ArgumentParser(description='Compare catalogue scenarios on two local servers.')
    parser.add_argument('--date', default=datetime.date.today().isoformat())
    parser.add_argument('--output', type=Path, default=ROOT / 'conformance/reports')
    parser.add_argument('--client-data', type=Path)
    parser.add_argument('--scenario')
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--json', action='store_true')
    parser.add_argument('--stop', action='store_true')
    args = parser.parse_args(json.loads(os.environ.get('LYRACORE_AUDIT_ARGS', '[]')))
    if args.stop:
        if args.scenario or args.check or args.json:
            parser.error('--stop cannot be combined with --scenario, --check or --json')
        stop_scenario_cache()
        return 0
    if args.check != bool(args.scenario):
        parser.error('--scenario ID requires --check, and --check requires --scenario ID')
    if args.json and not args.scenario:
        parser.error('--json requires --scenario ID --check')
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
    with tempfile.TemporaryDirectory(prefix='audit-', dir=SCRATCH) as directory:
        work = Path(directory)
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


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    sys.exit(main())
