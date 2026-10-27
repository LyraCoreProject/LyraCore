import argparse
from contextlib import contextmanager
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parent.parent
SCRATCH = Path.home() / '.cache/lyracore-scratch/conformance'
REFERENCE = Path.home() / '.cache/lyracore-scratch/reference-server'
sys.path.insert(0, str(ROOT / 'native-host'))
from host import Host


def run(args, **options):
    return subprocess.run([str(arg) for arg in args], check=True, **options)


def git(*args):
    return run(['git', *args], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


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
def reference_server(harness):
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
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if run([*compose, 'ps', '--status', 'running', '-q'], capture_output=True, text=True).stdout.strip():
            raise RuntimeError('Reference Server is already running; this audit only owns servers it starts')
        for port in [13724, 18085]:
            with socket.socket() as listener:
                listener.bind(('127.0.0.1', port))
        try:
            run([*compose, 'up', '-d', '--wait', '--wait-timeout', '180'])
            wait_port(13724)
            wait_port(18085, world=True)
            run([sys.executable, adapter / 'setup.py', '--verify-account', 'CONFORMANCE'])
            yield provenance
        finally:
            run([*compose, 'down'])


def capture(wire, work, scenario, adapter, endpoints, password, env):
    print(f"Capturing {scenario['id']} on {adapter}", flush=True)
    run([wire, 'candidate', work / f"{scenario['id']}.toml", work / f"{scenario['id']}-{adapter}.json",
         '--host', '127.0.0.1', '--logon-port', endpoints[0], '--world-port', endpoints[1],
         '--account', 'CONFORMANCE', '--password-stdin'], input=password, env=dict(env, WIRE_ADAPTER=adapter),
        stdout=subprocess.DEVNULL, timeout=240)


def delete_reference_character(wire, name, password, env):
    command = [wire, 'scenario', 'char-delete', '--host', '127.0.0.1', '--logon-port', '13724',
               '--world-port', '18085', '--account', 'CONFORMANCE', '--character', name,
               '--password-stdin']
    for attempt in range(2):
        try:
            run(command, input=password, env=env, capture_output=True, timeout=60)
            return
        except subprocess.CalledProcessError as error:
            if attempt or not error.stderr or error.stderr.splitlines()[-1] not in [
                    b'Error: logon proof rejected', b'Error: server proof mismatch']:
                raise RuntimeError(f'Reference Character cleanup failed: {error.stderr.decode(errors="replace").strip()}') from error
            print('Retrying Reference Server cleanup after an SRP proof rejection.', flush=True)


def capture_servers(host_binary, module, harness, wire, work):
    env = dict(os.environ, WIRE_CRASH_DIR=str(work / 'diagnostics'))
    scenarios = json.loads((work / 'catalogue.json').read_text())
    with reference_server(harness) as provenance:
        passwords = b''.join((REFERENCE / 'credentials' / name).read_bytes().rstrip(b'\n') + b'\n'
                             for name in ['player-password', 'gm-password'])
        reference_env = dict(env, WIRE_GM_ACCOUNT='CONFORMANCEGM', WIRE_GM_CHARACTER='Conformsetup')
        for scenario in scenarios:
            delete_reference_character(wire, scenario['character'], passwords.splitlines()[0] + b'\n', env)
            capture(wire, work, scenario, 'vmangos', [13724, 18085], passwords, reference_env)
        with Host(host_binary, module=module) as host:
            try:
                host.call('debug_repair_after_publish')
                for statement in json.loads((work / 'taxi-fixture.json').read_text()):
                    host.sql(statement)
                host.call('provision_realm', [1, 'Conformance', f'127.0.0.1:{host.world_port}', 0, 1])
                password = secrets.token_hex(8)
                host.account('CONFORMANCE', password=password)
                token = work / 'operator-token'
                token.touch(mode=0o600)
                token.write_text(host.token)
                env.update(WIRE_REALM_URL=f'http://127.0.0.1:{host.http_port}',
                           WIRE_OPERATOR_TOKEN_FILE=str(token), WIRE_SHARD=host.database)
                for scenario in scenarios:
                    capture(wire, work, scenario, 'lyracore', [host.logon_port, host.world_port],
                            (password + '\n').encode(), env)
            except BaseException:
                print('Native Host capture failed; both local servers will stop.', file=sys.stderr)
                raise
        return provenance


def main():
    parser = argparse.ArgumentParser(description='Run every catalogue scenario on two owned local servers.')
    parser.add_argument('--host', type=Path, required=True, help='Native Host binary built from this core revision')
    parser.add_argument('--date', default=datetime.date.today().isoformat())
    parser.add_argument('--output', type=Path, default=ROOT / 'conformance/reports')
    args = parser.parse_args(json.loads(os.environ.get('LYRACORE_AUDIT_ARGS', '[]')))
    if datetime.date.fromisoformat(args.date).isoformat() != args.date:
        parser.error('--date must be YYYY-MM-DD')
    args.host = args.host.resolve(strict=True)
    if any((args.output / f'{args.date}.{suffix}').exists() for suffix in ['md', 'json']):
        parser.error('report already exists; choose a new date or archive it first')
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
    metadata = json.loads(run(['cargo', 'metadata', '--locked', '--format-version', '1'],
                              capture_output=True, text=True).stdout)
    package = next(package for package in metadata['packages'] if package['name'] == 'wire-client')
    if not package['source'].endswith('#' + revision):
        raise RuntimeError('wire-client source differs from .wire-harness-rev')
    harness = Path(package['manifest_path']).parent
    run(['cargo', 'build', '--locked', '-p', 'lyracore-conformance', '--example', 'audit'])
    run(['cargo', 'build', '--locked', '--manifest-path', harness / 'Cargo.toml', '--bin', 'vanilla-wire'])
    run(['cargo', 'build', '--locked', '--release', '--target', 'wasm32-unknown-unknown',
         '-p', 'lyracore-module', '--features=debug_reducers'])
    module = target / 'wasm32-unknown-unknown/release/lyracore_module.wasm'
    audit = target / 'debug/examples/audit'
    with tempfile.TemporaryDirectory(prefix='audit-', dir=SCRATCH) as directory:
        work = Path(directory)
        tempfile.tempdir = str(work)
        os.environ['TMPDIR'] = str(work)
        run([audit, 'export', work])
        captured_module = work / 'lyracore_module.wasm'
        shutil.copyfile(module, captured_module)
        host_hash = digest(args.host)
        provenance = capture_servers(args.host, captured_module, harness, target / 'debug/vanilla-wire', work)
        if digest(args.host) != host_hash:
            raise RuntimeError('Native Host binary changed during capture')
        if core_commit != git('rev-parse', 'HEAD') or dirty != git('status', '--porcelain', '--untracked-files=normal'):
            raise RuntimeError('core changed during capture; rerun the audit')
        (work / 'run.json').write_text(json.dumps(dict(
            core_commit=core_commit, working_tree_dirty=bool(dirty), harness_revision=revision,
            recorded_at_unix_ms=time.time_ns() // 1_000_000, native_host_sha256=host_hash,
            module_sha256=digest(captured_module), reference=provenance)))
        run([audit, 'report', work, args.output, args.date])


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    main()
