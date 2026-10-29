import json
import math
import os
from pathlib import Path
import statistics
import sys
import time
import urllib.error
import urllib.request


def request(path, body=None):
    headers = {'Authorization': f'Bearer {token}', 'Content-Type': 'application/json'}
    req = urllib.request.Request(server + path, body, headers)
    try:
        with urllib.request.urlopen(req, timeout=60) as response:
            return response.read().decode(), response.headers
    except urllib.error.HTTPError as error:
        raise RuntimeError(f'{path}: HTTP {error.code}: {error.read().decode()}') from error


def call(reducer, args):
    _, headers = request(f'/v1/database/{shard}/call/{reducer}', json.dumps(args).encode())
    return int(headers['spacetime-execution-duration-micros']) / 1000


def sql(query):
    body, _ = request(f'/v1/database/{shard}/sql', query.encode())
    return json.loads(body)[0]['rows']


def ticks():
    body, _ = request('/v1/metrics')
    result = {}
    for line in body.splitlines():
        if line.startswith('spacetime_reducer_plus_query_duration_sec_') and 'reducer="tick_creatures"' in line:
            name, value = line.rsplit(' ', 1)
            result[name] = float(value)
    assert result, 'Standalone did not expose tick duration metrics'
    return result


def tick_cost(before, after):
    delta = {key: value - before.get(key, 0) for key, value in after.items()}
    count = sum(v for k, v in delta.items() if '_count{' in k)
    total_ms = sum(v for k, v in delta.items() if '_sum{' in k) * 1000
    bounds = [float(k.split('le="')[1].split('"')[0]) * 1000
              for k, v in delta.items() if '_bucket{' in k and v == count]
    return count, total_ms / count, min(bounds)


def wait_live(entry, count, health):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        rows = sql(f'SELECT guid FROM game_world_entity WHERE entry = {entry} AND health = {health}')
        if len(rows) == count:
            return
        time.sleep(0.05)
    raise AssertionError(f'entry {entry} did not recover {count} creatures')


def benchmark(client=None, visible=0):
    sizes = [int(n) for n in os.environ.get('LYRACORE_BENCH_SIZES', '256,1000,1129,2000').split(',')]
    repeats = int(os.environ.get('LYRACORE_BENCH_REPEATS', '5'))
    assert repeats >= 1 and all(n > 0 for n in sizes)
    call('operator_apply_creature_edits', [[{'createCreature': {'fields': fields}}]])
    entry = sql('SELECT entry FROM game_creature_template WHERE entry = 5200500')[0][0]
    placement = dict(map_id=0, x=10000.0, y=10000.0, z=0.0, orientation=0.0,
                     movement_type=0, respawn_secs=120)
    count = 0
    print('| Live spawns | Empty median ms | Edit median / max ms | Recovery median s | Ticks | Tick mean / upper bound ms |', flush=True)
    print('| ---: | ---: | ---: | ---: | ---: | ---: |', flush=True)
    for size in sizes:
        assert size >= count
        while count < size:
            batch = min(256, size - count)
            edits = [{'createCreature': {'entry': entry, 'placement':
                      dict(placement, x=24.0, y=0.0) if i < visible else placement}}
                     for i in range(count, count + batch)]
            call('operator_apply_spawn_edits', [edits])
            count += batch
        wait_live(entry, size, fields['health'])
        baseline, edits, recovery, means, bounds, counts, packet_bytes = [], [], [], [], [], [], []
        for _ in range(repeats):
            if client is not None:
                client.collect()
            baseline.append(call('operator_apply_creature_edits', [[]]))
            before = ticks()
            fields['health'] += 1
            fields['max_level_health'] = fields['health']
            edits.append(call('operator_apply_creature_edits', [[{'updateCreature': {'entry': entry, 'fields': fields}}]]))
            started = time.monotonic()
            wait_live(entry, size, fields['health'])
            recovery.append(time.monotonic() - started)
            if client is not None:
                from headless import creates, destroys
                packets = client.collect()
                expected = min(visible, size)
                own = lambda guid: (guid >> 24) & 0xffffff == entry
                assert sum(map(own, creates(packets))) == expected
                assert sum(map(own, destroys(packets))) == expected
                assert sum(opcode == 0x61 for opcode, _ in packets) == 1
                packet_bytes.append(sum(4 + len(body) for _, body in packets))
            n, mean, bound = tick_cost(before, ticks())
            assert n >= math.ceil(size / 256), (size, n)
            counts.append(n)
            means.append(mean)
            bounds.append(bound)
        print(f'| {size} | {statistics.median(baseline):.3f} | '
              f'{statistics.median(edits):.3f} / {max(edits):.3f} | '
              f'{statistics.median(recovery):.3f} | {min(counts):g}-{max(counts):g} | '
              f'{statistics.mean(means):.3f} / <={max(bounds):.3f} |', flush=True)
        if packet_bytes:
            print(f'Native: {min(visible, size)} destroy/create pairs and one template response; '
                  f'median {statistics.median(packet_bytes):g} wire bytes per edit', flush=True)


if __name__ == '__main__':
    if sys.argv[1] == '--native-host':
        sys.path.insert(0, str(Path(__file__).resolve().parents[2] / 'native-host'))
        from host import Host
        with Host(sys.argv[2], seeded=True) as host:
            server, shard, token = f'http://127.0.0.1:{host.http_port}', host.database, host.token
            template = host.sql('SELECT * FROM game_creature_template WHERE entry = 620')[0]
            fields = dict(zip([column['name']['some'] for column in template['schema']['elements']],
                              template['rows'][0]))
            del fields['entry']
            host.sql('INSERT INTO game_faction_template (id,faction,faction_group,friend_group,enemy_group,enemy_0,enemy_1,enemy_2,enemy_3,friend_0,friend_1,friend_2,friend_3) VALUES (50909,0,0,0,0,0,0,0,0,0,0,0,0)')
            fields['faction_template'] = 50909
            fields['max_level'] = fields['level']
            fields['max_level_health'] = fields['health']
            client = host.enter_world('Creaturebench', at=(0, 20, 0, 0))
            benchmark(client, int(os.environ.get('LYRACORE_BENCH_VISIBLE', '32')))
    else:
        server, shard = sys.argv[1:]
        token = os.environ['LYRACORE_BENCH_TOKEN']
        fields = json.loads(os.environ['LYRACORE_BENCH_FIELDS'])
        benchmark()
