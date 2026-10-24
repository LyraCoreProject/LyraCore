#!/usr/bin/env python3
import argparse
import json
import os
from pathlib import Path
import select
import signal
import socket
import struct
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent


def free_port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--host-checkout', type=Path, required=True)
    parser.add_argument('--variant', choices=['cell', 'edge', 'direct-cell', 'direct-edge', 'shared-edge'], required=True)
    parser.add_argument('--count', type=int, default=32)
    parser.add_argument('--layout', choices=['dense', 'spread'], default='spread')
    parser.add_argument('--ingress', choices=['batch', 'native'], default='batch')
    parser.add_argument('--seconds', type=int, default=10)
    parser.add_argument('--crossing', action='store_true')
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    if not 1 <= args.count <= 5000 or args.seconds <= 0:
        parser.error('count must be 1..5000 and seconds must be positive')
    if args.check and args.count < 2:
        parser.error('--check requires at least two Characters')
    if args.crossing and args.ingress != 'batch':
        parser.error('--crossing requires batch ingress')
    run = ROOT / 'runs' / f'{time.time_ns()}-{args.variant}-{args.count}-{args.layout}'
    run.mkdir(parents=True)
    private = run / 'jwt-private.pem'
    public = run / 'jwt-public.pem'
    subprocess.run(['openssl', 'genpkey', '-algorithm', 'EC', '-pkeyopt', 'ec_paramgen_curve:P-256', '-out', str(private)], check=True)
    private.chmod(0o600)
    subprocess.run(['openssl', 'ec', '-in', str(private), '-pubout', '-out', str(public)], check=True, capture_output=True)
    http_port, world_port = free_port(), free_port()
    while world_port == http_port:
        world_port = free_port()
    database = 'disposable-native-' + str(time.time_ns())
    address = f'http://127.0.0.1:{http_port}'
    env = dict(os.environ, LYRACORE_EXPERIMENT_WOW_ADDR=f'127.0.0.1:{world_port}',
               LYRACORE_EXPERIMENT_DATABASE=database, LYRACORE_EXPERIMENT_AOI=args.variant,
               RUST_LOG='warn')
    host_log = (run / 'host.log').open('w')
    host = subprocess.Popen([str(args.host_checkout / 'target/release/spacetimedb-standalone'), 'start',
        '--listen-addr', f'127.0.0.1:{http_port}', '--data-dir', str(run / 'data'), '--non-interactive',
        '--jwt-priv-key-path', str(private), '--jwt-pub-key-path', str(public), '--page_pool_max_size', '1073741824'],
        env=env, stdout=host_log, stderr=subprocess.STDOUT)
    driver = None
    token = None
    result = {'implementation_revision':3, 'variant':args.variant, 'count':args.count, 'layout':args.layout, 'crossing':args.crossing,
              'seconds_requested':args.seconds, 'ingress':args.ingress, 'host_pid':host.pid, 'run':str(run)}

    def request(path, body=None, method='GET', content_type='application/json'):
        headers = {'Content-Type':content_type}
        if token:
            headers['Authorization'] = 'Bearer ' + token
        req = urllib.request.Request(address + path, body, headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=120) as response:
                return response.read()
        except urllib.error.HTTPError as error:
            raise RuntimeError(f'{path}: HTTP {error.code}: {error.read().decode()}') from error

    def call(name, arguments):
        return request(f'/v1/database/{database}/call/{name}', json.dumps(arguments).encode(), 'POST')

    def command(op, **kwargs):
        driver.stdin.write(json.dumps(dict(op=op, **kwargs)) + '\n')
        driver.stdin.flush()
        return response()

    def response():
        if not select.select([driver.stdout], [], [], 120)[0]:
            raise RuntimeError('Headless Client timed out')
        line = driver.stdout.readline()
        if not line:
            raise RuntimeError('Headless Client stopped: ' + (run / 'driver.log').read_text())
        return json.loads(line)

    def sample():
        stat = Path(f'/proc/{host.pid}/stat').read_text().split()
        status = Path(f'/proc/{host.pid}/status').read_text().splitlines()
        rss = next(int(line.split()[1]) for line in status if line.startswith('VmRSS:'))
        return {'cpu_seconds':sum(int(stat[i]) for i in (13, 14))/os.sysconf('SC_CLK_TCK'), 'rss_kib':rss,
                'disk_bytes':sum(p.stat().st_size for p in (run / 'data').rglob('*') if p.is_file())}

    def await_state(predicate):
        deadline = time.monotonic() + 5
        while True:
            state = command('snapshot')['entities']
            if predicate(state):
                return
            if time.monotonic() > deadline:
                raise AssertionError(json.dumps(state))
            time.sleep(0.02)

    try:
        for _ in range(200):
            if host.poll() is not None:
                raise RuntimeError('host stopped: ' + (run / 'host.log').read_text())
            try:
                token = json.loads(request('/v1/identity', b'', 'POST'))['token']
                break
            except (OSError, RuntimeError):
                time.sleep(0.1)
        assert token, 'host startup timed out'
        wasm = ROOT / 'target/wasm32-unknown-unknown/release/native_host_experiment_module.wasm'
        request(f'/v1/database/{database}?host_type=Wasm', wasm.read_bytes(), 'PUT', 'application/wasm')
        call('seed', [args.count, args.layout == 'dense', args.variant.endswith('edge')])
        if args.check:
            try:
                call('enter', [1])
                raise AssertionError('foreign identity acquired a Character')
            except RuntimeError as error:
                assert 'native Account does not own' in str(error)
            with socket.create_connection(('127.0.0.1', world_port), timeout=5) as invalid:
                assert invalid.recv(8, socket.MSG_WAITALL)[:4] == bytes([0, 6, 0xec, 1])
                body = struct.pack('<II', 5875, 0) + b'E1\0' + bytes(28)
                invalid.sendall(struct.pack('>H', len(body) + 4) + struct.pack('<I', 0x1ed) + body)
                assert invalid.recv(1) == b'', 'incorrect world proof accepted'

        driver_log = (run / 'driver.log').open('w')
        driver = subprocess.Popen([str(ROOT / 'target/release/native-host-experiment-driver'),
            f'127.0.0.1:{world_port}', str(args.count), args.layout], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=driver_log, text=True, bufsize=1)
        result['startup'] = response()
        print(json.dumps({'ready':args.count, 'variant':args.variant, 'run':str(run)}), flush=True)
        if args.check:
            assert args.count >= 2
            duplicate = subprocess.run([str(ROOT / 'target/release/native-host-experiment-driver'),
                f'127.0.0.1:{world_port}', '1', 'dense'], input='', text=True, capture_output=True, timeout=10)
            assert duplicate.returncode != 0, 'duplicate World Session accepted'
            call('set_space', [2, 1])
            await_state(lambda state: '2' not in state['1'] and set(state['2']) == {'2'})
            call('set_space', [2, 0])
            await_state(lambda state: '2' in state['1'] and '1' in state['2'])
            call('set_hidden', [2, True, False])
            await_state(lambda state: all('2' not in entities for entities in state.values()))
            call('set_hidden', [2, False, True])
            await_state(lambda state: all('2' not in entities for entities in state.values()))
            call('set_viewer_ghost', [1, True])
            await_state(lambda state: '2' in state['1'] and '2' not in state['2'])
            call('set_hidden', [2, False, False])
            await_state(lambda state: '2' in state['1'] and '2' in state['2'])
            command('move', guid=1, x=-400.0, y=20.0)
            await_state(lambda state: set(state['1']) == {'1'} and '1' not in state['2'])
            command('move', guid=1, x=21.0, y=21.0)
            await_state(lambda state: '2' in state['1'] and '1' in state['2'])
            result['checks'] = ['duplicate-session-refused', 'foreign-identity-refused', 'wrong-world-proof-refused', 'space-isolation', 'hidden', 'ghost-only', 'viewer-ghost-change', 'cell-exit', 'cell-entry', 'create-before-move', 'native-movement']
        for tick in range(10):
            warmup_start = time.monotonic()
            for first in range(1, args.count + 1, 128):
                call('step', [first, min(128, args.count - first + 1), int(time.time()*1000) & 0xffffffff, False])
            if time.monotonic() - warmup_start > 0.3:
                break
            time.sleep(0.1)
        time.sleep(0.25)
        command('reset')
        (run / 'metrics-before.txt').write_bytes(request('/v1/metrics'))
        before = sample()
        start = time.monotonic()
        start_wall = time.time()
        batches = []
        completed_ticks = 0
        for tick in range(args.seconds * 10):
            deadline = start + tick / 10
            time.sleep(max(0, deadline - time.monotonic()))
            timestamp = int((start_wall + tick / 10)*1000) & 0xffffffff
            for first in (range(1, args.count + 1, 128) if args.ingress == 'batch' else [1]):
                began = time.monotonic()
                if args.ingress == 'batch':
                    call('step', [first, min(128, args.count - first + 1), timestamp, args.crossing])
                else:
                    command('tick', tick=timestamp)
                batches.append({'tick':tick, 'first':first, 'call_ms':(time.monotonic()-began)*1000,
                                'finished_late_ms':(time.monotonic()-deadline)*1000})
            completed_ticks += 1
            if time.monotonic() - deadline > 3:
                break
        result['completed_ticks'] = completed_ticks
        result['kept_up'] = completed_ticks == args.seconds * 10 and batches[-1]['finished_late_ms'] < 100
        elapsed = time.monotonic() - start
        time.sleep(1)
        result['observed'] = command('stats')
        result['elapsed_seconds'] = elapsed
        result['before'] = before
        result['expected_movements'] = completed_ticks * (args.count**2 if args.layout == 'dense' else
            sum(min(32, args.count-first)**2 for first in range(0, args.count, 32)))
        drain_deadline = time.monotonic() + 10
        while result['observed']['movements'] < result['expected_movements'] and time.monotonic() < drain_deadline:
            time.sleep(0.2)
            result['observed'] = command('stats')
        result['all_updates_received'] = result['observed']['movements'] == result['expected_movements']
        result['offered_schedule_kept'] = result['kept_up']
        result['kept_up'] = result['kept_up'] and result['all_updates_received'] and result['observed']['p99_ms'] < 100
        result['after'] = sample()
        result['measurement_with_drain_seconds'] = time.monotonic() - start
        result['host_cpu_cores'] = (result['after']['cpu_seconds'] - before['cpu_seconds']) / result['measurement_with_drain_seconds']
        (run / 'metrics-after.txt').write_bytes(request('/v1/metrics'))
        (run / 'batches.json').write_text(json.dumps(batches, indent=2) + '\n')
        result['row_counts'] = {table:json.loads(request(f'/v1/database/{database}/sql',
            f'SELECT COUNT(*) AS n FROM {table}'.encode(), 'POST')) for table in ['viewer_cell', 'visible_entity']}
        command('stop')
        driver.wait(timeout=10)
        if args.check:
            time.sleep(0.5)
            driver = subprocess.Popen([str(ROOT / 'target/release/native-host-experiment-driver'),
                f'127.0.0.1:{world_port}', str(args.count), args.layout], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                stderr=driver_log, text=True, bufsize=1)
            assert response()['ready'] == args.count
            command('stop')
            driver.wait(timeout=10)
            result['checks'].append('reconnect-snapshot')
        result['success'] = True
    except Exception as error:
        result['success'] = False
        result['error'] = str(error)
        raise
    finally:
        if driver and driver.poll() is None:
            driver.terminate()
            driver.wait(timeout=10)
        host.terminate()
        host.wait(timeout=30)
        (run / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result), flush=True)


if __name__ == '__main__':
    main()
