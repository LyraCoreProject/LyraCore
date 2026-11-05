import argparse
from contextlib import ExitStack, contextmanager
from concurrent.futures import ThreadPoolExecutor
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

from preflight import check_spacetime_version

ROOT = Path(__file__).resolve().parent.parent
SCRATCH = Path(os.environ.get('LYRACORE_AUDIT_SCRATCH', Path.home() / '.cache/lyracore-scratch/m5c'))
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


def canonical_schema(schema):
    if set(schema) != {'V10'}:
        raise RuntimeError('audit snapshots require a V10 Module definition')
    sections = {name: value for section in schema['V10']['sections'] for name, value in section.items()}
    types = sections.pop('Typespace')['types']
    names = {item['ty']: item['source_name'] for item in sections['Types']}
    if set(names) != set(range(len(types))):
        raise RuntimeError('Module definition contains unnamed types; review snapshot schema hashing')

    def normalize(value):
        if isinstance(value, dict):
            result = {}
            for name, item in value.items():
                if name in ['Ref', 'product_type_ref', 'ty']:
                    result[name] = names[item]
                else:
                    normalized = normalize(item)
                    # Collections are unordered; columns and enum variants keep their wire order.
                    if name in ['indexes', 'constraints', 'sequences', 'default_values', 'entries']:
                        normalized.sort(key=lambda entry: json.dumps(entry, sort_keys=True))
                    result[name] = normalized
            return result
        if isinstance(value, list):
            return [normalize(item) for item in value]
        return value

    canonical = {'Typespace': {json.dumps(names[index], sort_keys=True): normalize(value)
                               for index, value in enumerate(types)}}
    for name, value in sections.items():
        canonical[name] = normalize(value)
        if isinstance(value, list):
            canonical[name].sort(key=lambda entry: json.dumps(entry, sort_keys=True))
    return canonical


def schema_fingerprint(schema):
    return hashlib.sha256(json.dumps(canonical_schema(schema), sort_keys=True).encode()).hexdigest()


def compatible_schema(previous, current):
    old_tables = {table['source_name']: table for table in previous['Tables']}
    new_tables = {table['source_name']: table for table in current['Tables']}
    if any(new_tables.get(name) != table for name, table in old_tables.items()):
        return False
    if any(table['table_access'] != {'Private': []} or table.get('is_event', False)
           for name, table in new_tables.items() if name not in old_tables):
        return False
    if any(current['Typespace'].get(name) != value
           for name, value in previous['Typespace'].items()):
        return False
    for section in previous.keys() | current.keys():
        if section in ['Tables', 'Typespace']:
            continue
        old, new = previous.get(section), current.get(section)
        if section == 'ExplicitNames':
            old, new = old['entries'], new['entries']
        if section in ['Types', 'Reducers', 'ExplicitNames']:
            if any(entry not in new for entry in old):
                return False
        elif old != new:
            return False
    return True


def snapshot_origin(path, schema, *, strict_world=False):
    state = json.loads((path / 'state.json').read_text())
    previous = state.get('module_schema')
    if 'gameplay_manifest' not in state:
        return None
    if previous == schema:
        origin = 'snapshot'
    elif previous is not None and compatible_schema(previous, schema):
        origin = 'upgraded_snapshot'
    else:
        print('[audit] snapshot schema is incompatible or missing; cold import required',
              file=sys.stderr, flush=True)
        return None
    changed = snapshot_gameplay_changes(path)
    if changed and strict_world:
        print('[audit] strict world requires an import after Tier B changes: ' + ', '.join(changed),
              file=sys.stderr, flush=True)
        return None
    if changed:
        origin += '_gameplay_drift'
    return origin


def module_schema_hash():
    SCENARIO_CACHE.mkdir(parents=True, exist_ok=True)
    host_checkout = SCENARIO_CACHE / 'native-host/SpacetimeDB'
    build_env = dict(os.environ)
    build_env.pop('CARGO_TARGET_DIR', None)
    with timed('Native Host build'):
        run([sys.executable, ROOT / 'native-host/prepare-host.py', '--checkout', host_checkout,
             '--build'], env=build_env, cwd=ROOT, stdout=subprocess.DEVNULL)
    deployment_env = dict(os.environ)
    for name in ['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'CARGO_BUILD_RUSTFLAGS',
                 'CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS']:
        deployment_env.pop(name, None)
    with timed('deployment Module build'):
        run(['cargo', 'build', '--locked', '--release', '--target', 'wasm32-unknown-unknown',
             '-p', 'lyracore-module', '--features=debug_reducers'], cwd=ROOT, env=deployment_env,
            stdout=subprocess.DEVNULL)
    module = SCENARIO_CACHE / 'deployment.wasm'
    shutil.copy2(Path(os.environ['CARGO_TARGET_DIR']) / 'wasm32-unknown-unknown/release/lyracore_module.wasm', module)
    with timed('Module schema extraction'):
        extracted = run_with_diagnostic([host_checkout / 'target/release/spacetimedb-standalone',
                                         'extract-schema', module], timeout=120)
    schema = json.loads(extracted.stdout)
    write_state(SCENARIO_CACHE / 'deployment-schema.json', canonical_schema(schema))
    return schema_fingerprint(schema)


# Read README.md's import contract before extending the import or initialization call paths.
IMPORT_CONTRACT = (
    'Cargo.toml',
    'Cargo.lock',
    'rust-toolchain.toml',
    'importer/src',
    'importer/scripts',
    'importer/manifests',
    'importer/Cargo.toml',
    'crates/lyracore-shared',
    'crates/lyracore-package-delta',
    'crates/lyracore-protocol',
    'module/Cargo.toml',
    'module/build.rs',
    'module/build_support/source.rs',
    'module/src/lib.rs',
    'module/src/seed.rs',
    'module/src/config.rs',
    'module/src/operations.rs',
    'module/src/package_import.rs',
    'module/src/import_meta.rs',
    'module/src/terrain.rs',
    'module/src/nav.rs',
    'module/src/vmap.rs',
    'module/src/go_model.rs',
    'module/src/go_collider.rs',
    'module/src/region.rs',
    'module/src/creatures/catalogue.rs',
    'module/src/creatures/spawn.rs',
    'module/src/creatures/eventai/fixtures.rs',
    'module/src/creatures/eventai/loader.rs',
    'module/src/creatures/eventai/model.rs',
    'module/src/creatures/eventai/stored.rs',
    'module/src/creatures/eventai/tables.rs',
    'module/src/creatures/eventai/relay.rs',
    'module/src/gameobject.rs',
    'module/src/items/tables.rs',
    'module/src/items/properties.rs',
    'module/src/spell/tables.rs',
    'module/src/spell/spellbook.rs',
    'module/src/spell/stacking.rs',
    'module/src/spell/proc.rs',
    'module/src/action_bar.rs',
    'module/src/xp.rs',
    'module/src/talent.rs',
    'module/src/weather.rs',
    'module/src/skill.rs',
    'module/src/helpers.rs',
    'module/src/time.rs',
    'module/src/instance.rs',
    'module/src/world.rs',
    'module/src/world/simulation.rs',
    'module/src/world/placement.rs',
    'module/src/breath.rs',
    'module/src/gc.rs',
    'module/src/combat/engage.rs',
    'module/src/creatures/tick/mod.rs',
    'module/src/creatures/pet_care.rs',
    'module/src/creatures/eventai/presentation.rs',
    'module/src/duel.rs',
    'module/src/motion.rs',
    'module/src/gw.rs',
    'module/src/meeting_stone/mod.rs',
    'module/src/quest.rs',
    'module/src/loot/mod.rs',
    'module/src/trainer.rs',
    'module/src/auth.rs',
    'module/src/seed/fixtures.rs',
    'module/src/tables.rs',
    'module/src/character.rs',
    'module/src/faction.rs',
    'module/src/graveyard.rs',
    'module/src/skilldata.rs',
    'module/src/auction.rs',
    'module/src/mail_catalogue.rs',
    'module/src/stats.rs',
    'module/src/operator_content/spells.rs',
    'module/src/gossip.rs',
    'module/src/creatures/population.rs',
    'module/src/items/temporary_enchantment.rs',
    'module/src/spell/chance_on_hit.rs',
    'module/src/transport/mod.rs',
    'datascripts/runtime-scripts/event-names.rs',
    'datascripts/runtime-scripts/events.json',
    'module/src/package_import',
)


def import_contract_paths():
    paths = {ROOT / path for path in IMPORT_CONTRACT}
    missing = [str(path) for path in sorted(paths) if not path.exists()]
    if missing:
        raise RuntimeError('import contract paths are missing: ' + ', '.join(missing))
    return sorted(paths)


def gameplay_manifest():
    strict = {ROOT / path for path in IMPORT_CONTRACT}
    return {str(path.relative_to(ROOT)): digest(path)
            for path in sorted((ROOT / 'module/src').rglob('*.rs'))
            if not any(path == entry or entry in path.parents for entry in strict)}


def snapshot_gameplay_changes(path):
    saved = json.loads((path / 'state.json').read_text())['gameplay_manifest']
    current = gameplay_manifest()
    return sorted(name for name in saved.keys() | current.keys()
                  if saved.get(name) != current.get(name))


def scenario_key(harness, client_data):
    pins = json.loads((harness / 'adapters/vmangos/pins.json').read_text())
    with timed('client data fingerprint'):
        client_hash = client_fingerprint(client_data)
    contract = tree_hash(import_contract_paths())
    key = dict(snapshot_version=3, import_contract_sha256=contract,
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
    require_copy_space(path / 'native-data', realm_root)
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


def clean_pending_snapshots():
    if SNAPSHOT_STORE.exists():
        for pending in SNAPSHOT_STORE.glob('*.pending'):
            if pending.is_dir():
                writable_tree(pending)
                shutil.rmtree(pending)


def require_copy_space(source, destination):
    needed = sum(path.stat().st_size for path in source.rglob('*') if path.is_file())
    reserve = max(needed // 10, 256 * 1024 * 1024)
    if shutil.disk_usage(destination).free < needed + reserve:
        raise RuntimeError(f'insufficient free space to copy {source}: need {needed + reserve} bytes')


def seal_snapshot(key, realm_root, password, *, base_key=None):
    SNAPSHOT_STORE.mkdir(parents=True, exist_ok=True, mode=0o700)
    SNAPSHOT_STORE.chmod(0o700)
    clean_pending_snapshots()
    if base_key and not validate_snapshot(snapshot_path(base_key), base_key):
        raise RuntimeError('catalogue snapshot requires a sealed base world')
    require_copy_space(realm_root / '.lyracore/native-data', SNAPSHOT_STORE)
    pending = SNAPSHOT_STORE / (key + '.pending')
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
    schema = json.loads((SCENARIO_CACHE / 'deployment-schema.json').read_text())
    gameplay = (json.loads((snapshot_path(base_key) / 'state.json').read_text())['gameplay_manifest']
                if base_key else gameplay_manifest())
    state = dict(key=key, manifest=manifest, module_schema=schema, gameplay_manifest=gameplay)
    if base_key:
        state['base_world'] = base_key
    write_state(pending / 'state.json', state)
    (pending / 'state.json').chmod(0o400)
    pending.chmod(0o500)
    existing = snapshot_path(key)
    if existing.exists():
        writable_tree(existing)
        shutil.rmtree(existing)
    pending.rename(existing)
    previous = sorted((p for p in SNAPSHOT_STORE.iterdir()
                       if p.is_dir() and p.name != key and (p / 'state.json').is_file()),
                      key=lambda p: p.stat().st_mtime, reverse=True)
    retained = {base_key} if base_key else {path.name for path in previous[:1]}
    for old in previous:
        if old.name not in retained:
            writable_tree(old)
            shutil.rmtree(old)


class SnapshotUpgradeError(RuntimeError):
    pass


def start_realm(realm_root, cli, env, *, restored=False):
    try:
        run_with_diagnostic([cli, 'dev', 'up', '--single'], env=env, cwd=realm_root)
    except (RuntimeError, subprocess.TimeoutExpired) as error:
        if restored:
            raise SnapshotUpgradeError(str(error)) from error
        raise


def verify_realm_schema(realm_root, env, *, restored=False):
    try:
        schema = run_with_diagnostic(
            [realm_root.parent / 'bin/spacetime', '--config-path',
             realm_root / '.lyracore/native-cli.toml', 'describe', '--json',
             '-s', 'http://127.0.0.1:3000', 'lyracore'], env=env, cwd=realm_root)
        expected = json.loads((SCENARIO_CACHE / 'deployment-schema.json').read_text())
        if canonical_schema({'V10': json.loads(schema.stdout)}) != expected:
            raise RuntimeError('published Module schema differs from deployment Module')
    except (RuntimeError, ValueError, subprocess.TimeoutExpired) as error:
        if restored:
            raise SnapshotUpgradeError(str(error)) from error
        raise


def restore_imported_world(realm_root, seed, cli, env):
    stop_realm(realm_root, cli, env)
    native_data = realm_root / '.lyracore/native-data'
    require_copy_space(seed, realm_root)
    shutil.rmtree(native_data)
    shutil.copytree(seed, native_data)
    writable_tree(native_data)
    start_realm(realm_root, cli, env, restored=True)


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


def reference_baseline(harness):
    adapter = harness / 'adapters/vmangos'
    checksum = hashlib.sha256()
    for path in [adapter / 'pins.json', adapter / 'setup.py', adapter / 'compose.yml',
                 ROOT / 'conformance/reference_baseline.py']:
        checksum.update(bytes.fromhex(digest(path)))
    return SNAPSHOT_STORE / ('reference-pristine-' + checksum.hexdigest() + '.sql')


def prepare_reference_baseline(harness, path):
    seal = path.with_suffix('.json')
    credentials = ['player-password', 'gm-password']
    if path.is_file() and seal.is_file():
        saved = json.loads(seal.read_text())
        if (saved['sha256'] == digest(path)
                and saved['credentials'] == {name: digest(REFERENCE / 'credentials' / name)
                                             for name in credentials}):
            return
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with timed('pristine Reference Server preparation'):
        run([sys.executable, ROOT / 'conformance/reference_baseline.py',
             harness / 'adapters/vmangos', path, REFERENCE, SCENARIO_CACHE],
            stdout=subprocess.DEVNULL)
    write_state(seal, dict(sha256=digest(path), credentials={
        name: digest(REFERENCE / 'credentials' / name) for name in credentials}))


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
    marker = SCENARIO_CACHE / 'reference-state.json'
    compose = [adapter / 'reference.sh']
    with (REFERENCE / '.audit.lock').open('w') as lock:
        lock_owned_state(lock, 'Reference Server', wait=True)
        owned = json.loads(marker.read_text()) if marker.is_file() else None
        env = dict(os.environ, COMPOSE_PROJECT_NAME=owned['project']) if owned else dict(os.environ)
        if owned:
            owned_reference_ids(marker, owned, compose, env)
        try:
            runtime = json.loads((REFERENCE / 'runtime-pins.json').read_text())
            if pins != runtime or (REFERENCE / 'binaries/.verified').read_text() != pins['binary_sha256']:
                raise RuntimeError('prepare the Reference Server with the pinned adapter first')
            provenance = json.loads((REFERENCE / 'provenance.json').read_text())
            expected = dict(server='vmangos', binary_release=pins['binary_tag'] + '/' + pins['binary_asset'],
                            **{name: pins[name] for name in ['binary_sha256', 'source_commit', 'database_commit']})
            if provenance != expected:
                raise RuntimeError('Reference Server provenance does not match its runtime pins')
        except BaseException:
            if owned:
                owned['phase'] = 'invalid'
                try:
                    write_state(marker, owned)
                finally:
                    stop_reference(marker, owned, compose, env)
            raise
        if owned is None:
            if reference_running(compose, env):
                raise RuntimeError('Reference Server is already running outside this scenario cache')
            for port in [13724, 18085]:
                free_port(port)
            owned = dict(phase='starting', key=key, project='lyracore-audit-' + secrets.token_hex(16),
                         adapter=str(compose[0]), containers={})
            env['COMPOSE_PROJECT_NAME'] = owned['project']
        owned['dump'] = str(reference_baseline(harness))
        write_state(marker, owned)
        try:
            with timed('Reference Server start'):
                if not owned['containers']:
                    run([*compose, 'up', '--no-start'], env=env, capture_output=True)
                    owned_reference_ids(marker, owned, compose, env)
                if set(owned['containers']) != {'auth', 'database', 'world'}:
                    raise RuntimeError('Reference Server startup did not create all containers')
                database, configured = reference_database_command(compose, env)
                stop_reference(marker, owned, compose, env)
                prepare_reference_baseline(harness, Path(owned['dump']))
                restore_reference_world(marker, owned, compose, env)
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
            yield provenance, owned['project']
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
    run_with_diagnostic([wire, 'candidate', work / f"{scenario['id']}.toml", work / f"{scenario['id']}-{adapter}.json",
         '--host', '127.0.0.1', '--logon-port', endpoints[0], '--world-port', endpoints[1],
         '--account', 'CONFORMANCE', '--password-stdin'], input=password,
        env=dict(env, WIRE_ADAPTER=adapter, WIRE_PEER_ACCOUNTS=','.join(peer_accounts(scenario))),
        timeout=240)


@contextmanager
def scenario_realm(key, pins, client_data, peer_count, *, require_snapshot=False, strict_world=False):
    with ExitStack() as lifetime:
        try:
            realm = lifetime.enter_context(prepare_scenario_realm(
                key, pins, client_data, peer_count, require_snapshot=require_snapshot, strict_world=strict_world))
        except SnapshotUpgradeError as error:
            print(f'[audit] snapshot publish or initialization failed: {error}', file=sys.stderr, flush=True)
            if require_snapshot:
                raise RuntimeError('--require-snapshot forbids cold import after snapshot upgrade failure') from error
            print('[audit] falling back to a fresh world import', file=sys.stderr, flush=True)
            realm = lifetime.enter_context(prepare_scenario_realm(
                key, pins, client_data, peer_count, use_snapshot=False, strict_world=strict_world))
        yield realm


@contextmanager
def prepare_scenario_realm(key, pins, client_data, peer_count, *, require_snapshot=False,
                           use_snapshot=True, strict_world=False):
    if git('status', '--porcelain', '--untracked-files=normal'):
        raise RuntimeError('clean core working tree required; commit changes before scenario capture')
    if (ROOT / '.lyracore-cli-rev').read_text().strip() != '30814fe814edbfa3a143102f93008080482df6a6':
        raise RuntimeError('audit Realm isolation needs review for this lyracore CLI revision')
    spacetime = shutil.which('spacetime')
    if spacetime is None:
        raise RuntimeError('spacetime CLI is missing')
    SCENARIO_CACHE.mkdir(parents=True, exist_ok=True)
    SNAPSHOT_STORE.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (SCENARIO_CACHE / '.realm.lock').open('w') as lock, (SNAPSHOT_STORE / '.lock').open('w') as store_lock:
        lock_owned_state(lock, 'scenario Realm')
        lock_owned_state(store_lock, 'imported-world snapshot store', wait=True)
        clean_pending_snapshots()
        cache = SCENARIO_CACHE / 'realm'
        realm_root = cache / 'core'
        marker = cache / 'state.json'
        cli = realm_root / 'lyracore'
        bin_dir = cache / 'bin'
        cli_env = dict(os.environ)
        cli_env.pop('CARGO_TARGET_DIR', None)
        cli_env['PATH'] = str(bin_dir) + ':' + cli_env['PATH']
        snapshot = snapshot_path(key)
        schema = json.loads((SCENARIO_CACHE / 'deployment-schema.json').read_text())
        origin = (snapshot_origin(snapshot, schema, strict_world=strict_world)
                  if use_snapshot and validate_snapshot(snapshot, key) else None)
        saved = origin is not None
        if require_snapshot and not saved:
            raise RuntimeError(f'no imported-world snapshot for {key}; --require-snapshot forbids an import')
        current = git('rev-parse', 'HEAD')
        state = json.loads(marker.read_text()) if marker.is_file() else None
        if cache.exists() and state is None:
            raise RuntimeError('scenario Realm has no ownership marker; inspect it before cleanup')
        if state and state['phase'] == 'importing':
            raise RuntimeError(f'interrupted import retained at {realm_root}; inspect it before cleanup')
        if state and state['phase'] == 'imported' and use_snapshot:
            if state['key'] != key or state['core_commit'] != current:
                raise RuntimeError(f'unsealed imported world retained at {realm_root}; seal it before changing inputs')
            stop_realm(realm_root, cli, cli_env)
            seal_snapshot(key, realm_root, (cache / 'password').read_text().strip())
            saved = True
            origin = 'fresh_import'
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
                wrapper = [sys.executable, str(ROOT / 'conformance/audit_spacetime.py'),
                           spacetime, str(SCENARIO_CACHE / 'deployment.wasm')]
                isolated.write_text('#!/bin/sh\n'
                                    'if [ "$1" = login ] && [ "$2" = show ] && [ "$3" = --token ]; then\n'
                                    '    exit 1\n'
                                    'fi\n'
                                    'exec ' + shlex.join(wrapper) + ' "$@"\n')
                isolated.chmod(0o700)
                if saved:
                    copy_snapshot(snapshot, realm_root)
                native = realm_root / '.lyracore'
                native.mkdir(exist_ok=True)
                (SCENARIO_CACHE / 'native-host').mkdir(exist_ok=True)
                (native / 'native-host').symlink_to(SCENARIO_CACHE / 'native-host', target_is_directory=True)
                with timed('Realm start'):
                    start_realm(realm_root, cli, cli_env, restored=saved)
                    verify_realm_schema(realm_root, cli_env, restored=saved)
                if not saved:
                    password = secrets.token_hex(8)
                    (cache / 'password').write_text(password + '\n')
                    (cache / 'password').chmod(0o600)
                    state = dict(phase='importing', key=key, core_commit=current)
                    write_state(marker, state)
                    with timed('import'):
                        run([cli, 'import', '--accept', '--client-data', client_data], env=cli_env,
                            cwd=realm_root, stdout=subprocess.DEVNULL)
                    imported = json.loads((realm_root / '.import/vmangos/current.json').read_text())
                    if imported['commit'] != pins['database_commit']:
                        raise RuntimeError('imported vmangos data differs from Reference Server pin')
                    state = dict(phase='imported', key=key, core_commit=current)
                    write_state(marker, state)
                    with timed('imported world snapshot'):
                        stop_realm(realm_root, cli, cli_env)
                        seal_snapshot(key, realm_root, password)
                        start_realm(realm_root, cli, cli_env, restored=True)
                        verify_realm_schema(realm_root, cli_env, restored=True)
                else:
                    print('[audit] import: 0.00s (cached snapshot)', file=sys.stderr, flush=True)
                state = dict(phase='ready', key=key, core_commit=current,
                             world_origin=origin or 'fresh_import',
                             changed_gameplay_files=snapshot_gameplay_changes(snapshot) if saved else [],
                             native_host_sha256=digest(realm_root / '.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone'),
                             module_sha256=digest(SCENARIO_CACHE / 'deployment.wasm'),
                             database_commit=pins['database_commit'])
                write_state(marker, state)
            except BaseException:
                if (realm_root / '.lyracore').exists():
                    stop_realm(realm_root, cli, cli_env)
                if state and state['phase'] in ['importing', 'imported']:
                    print(f'[audit] retaining stopped imported world at {realm_root}', file=sys.stderr)
                    raise
                if (realm_root / '.git').exists():
                    run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
                        stdout=subprocess.DEVNULL)
                shutil.rmtree(cache)
                raise
        else:
            try:
                with timed('Realm start'):
                    restore_imported_world(realm_root, snapshot / 'native-data', cli, cli_env)
                    verify_realm_schema(realm_root, cli_env, restored=True)
            except SnapshotUpgradeError:
                stop_realm(realm_root, cli, cli_env)
                raise
            print('[audit] import: 0.00s (cached snapshot)', file=sys.stderr, flush=True)
            state['world_origin'] = origin
            state['changed_gameplay_files'] = snapshot_gameplay_changes(snapshot)
            state['native_host_sha256'] = digest(realm_root / '.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone')
            state['module_sha256'] = digest(SCENARIO_CACHE / 'deployment.wasm')
            write_state(marker, state)
        host = realm_root / '.lyracore/native-host/SpacetimeDB/target/release/spacetimedb-standalone'
        module = SCENARIO_CACHE / 'deployment.wasm'
        if digest(host) != state['native_host_sha256'] or digest(module) != state['module_sha256']:
            raise RuntimeError('cached Realm binaries changed')
        token = realm_root / '.lyracore/native-owner-token'
        if not token.is_file():
            raise RuntimeError('cached Realm has no Operator token')
        imported = json.loads((realm_root / '.import/vmangos/current.json').read_text())
        if imported['commit'] != pins['database_commit']:
            raise RuntimeError('restored world differs from Reference Server data pin')
        print(f"[audit] world origin: {state['world_origin']}", file=sys.stderr, flush=True)
        for path in state['changed_gameplay_files']:
            print(f'[audit] changed Tier B file: {path}', file=sys.stderr, flush=True)
        password = (snapshot / 'password').read_text().strip()
        for account in ['CONFORMANCE', *(f'CONFORMANCE{peer}' for peer in range(2, peer_count + 2))]:
            run([cli, 'account', 'create', account, '--password-stdin'],
                input=(password + '\n').encode(), env=cli_env, cwd=realm_root,
                stdout=subprocess.DEVNULL)
        yield password, token, host, module, realm_root


def rehearse_comparison(wire, work, scenario, adapter):
    golden = work / f"{scenario['id']}-golden.json"
    expected = json.loads(golden.read_text())['header']['origin']['provenance']
    if json.loads((work / 'reference.json').read_text()) != expected:
        raise RuntimeError('Reference Server provenance differs from the Golden Trace')
    result = subprocess.run([str(arg) for arg in [
        wire, 'compare', work / f"{scenario['id']}.toml", golden,
        work / f"{scenario['id']}-{adapter}.json"]], capture_output=True, text=True)
    if result.returncode not in [0, 1] or not result.stdout:
        raise RuntimeError('rehearsal comparison failed: ' + result.stderr.strip())
    comparison = json.loads(result.stdout)
    differences = comparison['differences']
    if any(item['classification'] not in ['rule', 'data'] for item in differences):
        raise RuntimeError('rehearsal comparison has an unknown classification')
    (work / f"{scenario['id']}-{adapter}-comparison.json").write_text(result.stdout)
    return {kind: sum(item['classification'] == kind for item in differences)
            for kind in ['rule', 'data']}


def catalogue_snapshot_key(base_key, catalogue):
    creatures = sorted({entry for scenario in catalogue for entry in scenario.get('creatures', [])})
    if not creatures:
        return base_key, creatures
    contract = {path: digest(ROOT / path) for path in
                ['conformance/audit.py', 'conformance/examples/audit.rs']}
    key = dict(base_world=base_key, creatures=creatures, preparation=contract)
    return hashlib.sha256(json.dumps(key, sort_keys=True).encode()).hexdigest(), creatures


def prepare_catalogue_snapshot(work, base_key, pins, client_data, *, require_snapshot, fresh=False,
                               strict_world=False):
    catalogue = json.loads((work / 'catalogue.json').read_text())
    key, creatures = catalogue_snapshot_key(base_key, catalogue)
    if not creatures:
        return key
    if not fresh and validate_snapshot(snapshot_path(key), key):
        schema = json.loads((SCENARIO_CACHE / 'deployment-schema.json').read_text())
        if snapshot_origin(snapshot_path(key), schema, strict_world=strict_world) is not None:
            return key
    if require_snapshot and not validate_snapshot(snapshot_path(base_key), base_key):
        raise RuntimeError('base world snapshot missing; --require-snapshot forbids an import; '
                           'run a single --check once to prepare the base world')
    peer_count = max(len(scenario['characters']) - 1 for scenario in catalogue)
    with scenario_realm(base_key, pins, client_data, peer_count,
                        require_snapshot=require_snapshot, strict_world=strict_world) as (password, _token, _host, _module, realm):
        cli_env = dict(os.environ)
        cli_env.pop('CARGO_TARGET_DIR', None)
        cli_env['PATH'] = str(realm.parent / 'bin') + ':' + cli_env['PATH']
        try:
            source_root = SNAPSHOT_STORE / 'sources' / pins['database_commit']
            marker = source_root / 'current.json'
            source = json.loads(marker.read_text()) if marker.is_file() else None
            if (not source or source['commit'] != pins['database_commit']
                    or not Path(source['sqlite_path']).is_file()
                    or digest(Path(source['sqlite_path'])) != source['sqlite_sha256']):
                with timed('catalogue World Source preparation'):
                    fetched = run_with_diagnostic([sys.executable, ROOT / 'importer/scripts/pull-vmangos.py',
                                                   '--output', source_root])
                    source = json.loads(fetched.stdout)
            if source['commit'] != pins['database_commit']:
                raise RuntimeError('catalogue World Source differs from the Reference Server pin')
            run(['cargo', 'build', '--locked', '-p', 'lyracore-importer', '--bin', 'lyracore-importer'],
                cwd=ROOT, stdout=subprocess.DEVNULL)
            env = dict(os.environ, LYRACORE_SPACETIME_CONFIG_PATH=str(realm / '.lyracore/native-cli.toml'))
            with timed('catalogue fixture content'):
                run([SCENARIO_CACHE / 'target/debug/lyracore-importer', '--dump', source['sqlite_path'],
                     '--dbc', client_data, '--world-profile', 'alliance-single',
                     '--include-creatures', ','.join(map(str, creatures)),
                     '--source-sha', pins['database_commit'], '--server', 'http://127.0.0.1:3000',
                     '--db', 'lyracore', '--apply'], cwd=ROOT, env=env, stdout=subprocess.DEVNULL)
        finally:
            stop_realm(realm, realm / 'lyracore', cli_env)
        seal_snapshot(key, realm, password, base_key=base_key)
    return key


def capture_servers(harness, wire, work, client_data, *, rehearse=False):
    scenarios = json.loads((work / 'catalogue.json').read_text())
    peer_count = max(len(scenario['characters']) - 1 for scenario in scenarios)
    key, pins = scenario_key(harness, Path(client_data))
    try:
        if not rehearse:
            with prepare_scenario_realm(key, pins, client_data, peer_count, use_snapshot=False):
                pass
        key = prepare_catalogue_snapshot(work, key, pins, client_data,
                                         require_snapshot=rehearse, fresh=not rehearse,
                                         strict_world=rehearse)
        for scenario in scenarios:
            started = time.monotonic()
            with capture_scenario(harness, wire, work, scenario['id'], key, pins, client_data,
                                  require_snapshot=True, strict_world=rehearse):
                state = json.loads((SCENARIO_CACHE / 'realm/state.json').read_text())
                if rehearse:
                    comparisons = {adapter: rehearse_comparison(wire, work, scenario, adapter)
                                   for adapter in ['vmangos', 'lyracore']}
                    print(json.dumps(dict(scenario=scenario['id'], setup='passed',
                                          seconds=round(time.monotonic() - started, 2),
                                          comparisons=comparisons)), flush=True)
        provenance = json.loads((work / 'reference.json').read_text())
        content = (f"lyracore import alliance-single, vmangos db_latest {pins['database_commit']}, "
                   'client data from the supplied path; imported creature spawns retained; '
                   'catalogue creature and vendor content included from the same World Source')
        return provenance, state['native_host_sha256'], state['module_sha256'], content
    finally:
        stop_scenario_cache(preserve_import=True)


@contextmanager
def capture_scenario(harness, wire, work, scenario, key, pins, client_data, *, require_snapshot=False,
                     strict_world=False):
    env = dict(os.environ, WIRE_CRASH_DIR=str(work / 'diagnostics'))
    realm_env = dict(env)
    catalogue = json.loads((work / 'catalogue.json').read_text())
    entry = next((item for item in catalogue if item['id'] == scenario), None)
    if entry is None:
        raise RuntimeError(f'unknown catalogue scenario {scenario}')
    characters = entry['characters']
    max_peer_count = max(len(item['characters']) - 1 for item in catalogue)
    cores = len(os.sched_getaffinity(0)) if hasattr(os, 'sched_getaffinity') else (os.cpu_count() or 1)
    with ExitStack() as realm_lifetime, ExitStack() as reference_lifetime:
        with ThreadPoolExecutor(max_workers=2 if cores >= 4 else 1) as captures:
            realm_start = captures.submit(realm_lifetime.enter_context, scenario_realm(
                key, pins, client_data, max_peer_count, require_snapshot=require_snapshot,
                strict_world=strict_world))
            reference_start = captures.submit(reference_lifetime.enter_context,
                                             scenario_reference_server(harness, key, len(characters) - 1))
            password, token, host, module, realm_root = realm_start.result()
            provenance, project = reference_start.result()
            realm_env.update(WIRE_REALM_URL='http://127.0.0.1:3000',
                             WIRE_OPERATOR_TOKEN_FILE=str(token), WIRE_SHARD='lyracore')
            (work / 'reference.json').write_text(json.dumps(provenance))
            reference_env = dict(env, COMPOSE_PROJECT_NAME=project,
                                 WIRE_GM_ACCOUNT='CONFORMANCEGM', WIRE_GM_CHARACTER='Conformsetup')
            def capture_reference():
                with timed(f'{scenario} Reference Server run'):
                    passwords = []
                    for index in range(len(characters)):
                        name = 'player-password' if index == 0 else f'peer{index + 1}-password'
                        secret = (REFERENCE / 'credentials' / name).read_bytes().rstrip(b'\n') + b'\n'
                        passwords.append(secret)
                    passwords.append((REFERENCE / 'credentials/gm-password').read_bytes().rstrip(b'\n') + b'\n')
                    capture(wire, work, entry, 'vmangos', [13724, 18085], b''.join(passwords),
                            reference_env, progress=sys.stderr)
            def capture_realm():
                with timed(f'{scenario} Realm run'):
                    capture(wire, work, entry, 'lyracore', [3724, 8085],
                            (password + '\n').encode() * len(characters), realm_env, progress=sys.stderr)
            if cores >= 4:
                futures = [captures.submit(capture_reference), captures.submit(capture_realm)]
                for future in futures:
                    future.result()
            else:
                capture_reference()
                capture_realm()
        state = json.loads((SCENARIO_CACHE / 'realm/state.json').read_text())
        snapshot = json.loads((realm_root / '.import/vmangos/current.json').read_text())
        if (digest(host) != state['native_host_sha256'] or digest(module) != state['module_sha256']
                or snapshot['commit'] != state['database_commit']):
            raise RuntimeError('cached Realm Module, Native Host or imported data changed during capture')
        yield {name: state[name] for name in ['world_origin', 'changed_gameplay_files']}


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


@contextmanager
def scenario_work():
    work = Path(tempfile.mkdtemp(prefix='scenario-', dir=SCENARIO_CACHE))
    previous = os.environ.get('TMPDIR')
    try:
        yield work
    finally:
        if previous is None:
            os.environ.pop('TMPDIR', None)
        else:
            os.environ['TMPDIR'] = previous
        evidence = SNAPSHOT_STORE / 'diagnostics' / work.name
        evidence.mkdir(parents=True, mode=0o700)
        for path in work.glob('*.json'):
            shutil.copy2(path, evidence / path.name)
        if (work / 'diagnostics').exists():
            shutil.copytree(work / 'diagnostics', evidence / 'wire')
        print(f'[audit] diagnostics: {evidence}', file=sys.stderr, flush=True)
        shutil.rmtree(work)


def run_scenario(args, harness, target, core_commit):
    if git('status', '--porcelain', '--untracked-files=normal'):
        raise RuntimeError('clean core working tree required; commit changes before scenario capture')
    with timed('Module schema fingerprint'):
        module_schema_hash()
    key, pins = scenario_key(harness, args.client_data)
    with scenario_work() as work:
        os.environ['TMPDIR'] = str(work)
        audit = target / 'debug/examples/audit'
        wire = target / 'debug/vanilla-wire'
        if args.agreement:
            with timed('tier 1 agreement run'):
                tier1 = subprocess.run([str(ROOT / 'conformance/run'), args.scenario, '--json',
                                        '--result', str(work / 'tier1.json')],
                                       cwd=ROOT, env=dict(os.environ,
                                                          CARGO_TARGET_DIR=str(SCENARIO_CACHE / 'tier1-target'),
                                                          LYRACORE_CONFORMANCE_SCRATCH=str(SCRATCH / 'tier1')),
                                       capture_output=True, text=True)
            if tier1.returncode not in [0, 1] or not (work / 'tier1.json').is_file():
                raise RuntimeError('tier 1 did not produce a comparison: '
                                   + (tier1.stderr + tier1.stdout).strip())
        run_with_diagnostic([audit, 'export', work])
        key = prepare_catalogue_snapshot(work, key, pins, str(args.client_data),
                                         require_snapshot=args.require_snapshot, strict_world=args.strict_world)
        with capture_scenario(harness, wire, work, args.scenario, key, pins, str(args.client_data),
                              require_snapshot=True, strict_world=args.strict_world) as world:
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
                if args.json:
                    output = dict(json.loads(rendered.stdout), **world)
                    rendered.stdout = json.dumps(output, indent=2) + '\n'
                else:
                    rendered.stdout += f"World origin: {world['world_origin']}\n"
                    if world['changed_gameplay_files']:
                        rendered.stdout += 'Changed Tier B files:\n' + ''.join(
                            f'  {path}\n' for path in world['changed_gameplay_files'])
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


def run_catalogue(args, harness, target, core_commit):
    with timed('Module schema fingerprint'):
        module_schema_hash()
    with scenario_work() as work:
        audit = target / 'debug/examples/audit'
        run_with_diagnostic([audit, 'export', work])
        provenance, host_hash, module_hash, content = capture_servers(
            harness, target / 'debug/vanilla-wire', work, str(args.client_data),
            rehearse=args.rehearse)
        if core_commit != git('rev-parse', 'HEAD') or git('status', '--porcelain', '--untracked-files=normal'):
            raise RuntimeError('core changed during catalogue capture; commit and rerun')
        if not args.rehearse:
            revision = next(line.split()[1] for line in (ROOT / '.wire-harness-rev').read_text().splitlines()
                            if line.strip() and not line.startswith('#'))
            (work / 'run.json').write_text(json.dumps(dict(
                core_commit=core_commit, working_tree_dirty=False, harness_revision=revision,
                recorded_at_unix_ms=time.time_ns() // 1_000_000, native_host_sha256=host_hash,
                module_sha256=module_hash, content=content, reference=provenance)))
            with timed('report'):
                run_with_diagnostic([audit, 'report', work, args.output, args.date])
    return 0


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
    with (SCENARIO_CACHE / '.check.lock').open('w') as ownership:
        lock_owned_state(ownership, 'scenario check')
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
        if args.scenario:
            return run_scenario(args, harness, target, core_commit)
        return run_catalogue(args, harness, target, core_commit)


def stop_scenario_cache(*, preserve_import=False):
    SCENARIO_CACHE.mkdir(parents=True, exist_ok=True)
    with (SCENARIO_CACHE / '.realm.lock').open('w') as lock:
        lock_owned_state(lock, 'scenario Realm')
        try:
            cache = SCENARIO_CACHE / 'realm'
            realm_root = cache / 'core'
            if cache.exists():
                marker = cache / 'state.json'
                if not marker.is_file():
                    raise RuntimeError('scenario Realm has no ownership marker; inspect it before cleanup')
                state = json.loads(marker.read_text())
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
                if not (preserve_import and state.get('phase') in ['importing', 'imported']):
                    if (realm_root / '.git').exists():
                        run(['git', 'worktree', 'remove', '--force', realm_root], cwd=ROOT,
                            stdout=subprocess.DEVNULL)
                    shutil.rmtree(cache)
        finally:
            marker = SCENARIO_CACHE / 'reference-state.json'
            if marker.is_file():
                with (REFERENCE / '.audit.lock').open('w') as reference_lock:
                    lock_owned_state(reference_lock, 'Reference Server', wait=True)
                    owned = json.loads(marker.read_text())
                    env = dict(os.environ, COMPOSE_PROJECT_NAME=owned['project'])
                    compose = [owned['adapter']]
                    release_reference(marker, owned, compose, env)


def main():
    parser = argparse.ArgumentParser(description='Compare catalogue scenarios on two local servers.')
    parser.add_argument('--date')
    parser.add_argument('--output', type=Path)
    parser.add_argument('--client-data', type=Path)
    parser.add_argument('--scenario')
    parser.add_argument('--check', action='store_true')
    parser.add_argument('--rehearse', action='store_true',
                        help='capture the full catalogue from cached worlds without writing a report')
    parser.add_argument('--json', action='store_true')
    parser.add_argument('--agreement', action='store_true')
    parser.add_argument('--require-snapshot', action='store_true',
                        help='fail instead of a fresh world import when no matching base snapshot exists')
    parser.add_argument('--strict-world', action='store_true',
                        help='treat reachable gameplay changes as cold import inputs for a local check')
    parser.add_argument('--stop', action='store_true')
    args = parser.parse_args(json.loads(os.environ.get('LYRACORE_AUDIT_ARGS', '[]')))
    if args.stop:
        if args.scenario or args.check or args.json or args.agreement or args.require_snapshot or args.rehearse or args.strict_world:
            parser.error('--stop cannot be combined with scenario check options')
        stop_scenario_cache()
        return 0
    if args.rehearse and (args.scenario or args.check or args.json or args.agreement or args.require_snapshot):
        parser.error('--rehearse cannot be combined with scenario check options')
    if args.rehearse and (args.date is not None or args.output is not None):
        parser.error('--rehearse cannot write a dated report or select an output directory')
    if args.check != bool(args.scenario):
        parser.error('--scenario ID requires --check, and --check requires --scenario ID')
    if args.json and not args.scenario:
        parser.error('--json requires --scenario ID --check')
    if args.agreement and not args.scenario:
        parser.error('--agreement requires --scenario ID --check')
    if args.strict_world and not args.scenario:
        parser.error('--strict-world requires --scenario ID --check')
    if args.require_snapshot and not args.scenario:
        parser.error('--require-snapshot requires --scenario ID --check')
    try:
        check_spacetime_version(ROOT, executable='spacetime')
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        return setup_failure(args.scenario, args.json, str(error))
    if args.client_data is None:
        config = ROOT / '.lyracore/config.json'
        if config.is_file():
            args.client_data = json.loads(config.read_text()).get('client_data')
    if args.scenario or args.rehearse:
        if not args.client_data or not Path(args.client_data).is_dir():
            return setup_failure(args.scenario, args.json,
                                 'client data missing; pass --client-data with an existing 1.12.1 client Data directory')
        args.client_data = Path(args.client_data).resolve()
        try:
            with timed('total rehearsal' if args.rehearse else 'total scenario check'):
                return scenario_main(args)
        except Exception as error:
            return setup_failure(args.scenario, args.json, str(error))
    args.date = args.date if args.date is not None else datetime.date.today().isoformat()
    args.output = args.output or ROOT / 'conformance/reports'
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
    try:
        with timed('total audit'):
            return scenario_main(args)
    except Exception as error:
        return setup_failure(None, False, str(error))


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    sys.exit(main())
