import struct
import time

from check import World, creates


def check(data, world_port, sql, restart):
    def files():
        snapshots = sorted(data.glob('replicas/*/snapshots/*.snapshot_dir'))
        segments = sorted(data.glob('replicas/*/clog/*.log'))
        return snapshots, segments

    for _ in range(300):
        snapshots, segments = files()
        if len(snapshots) == 2 and segments and int(segments[0].name.split('.')[0]) > 0:
            break
        time.sleep(0.1)
    else:
        raise AssertionError(f'cleanup did not retain two snapshots and a shortened log: {snapshots}, {segments}')
    older = int(snapshots[0].name.split('.')[0])
    starts = [int(path.name.split('.')[0]) for path in segments]
    assert starts[0] <= older + 1
    assert len(starts) == 1 or starts[1] > older + 1, (older, starts)
    assert len(starts) <= 4, starts
    queries = [
        'SELECT program_hash FROM st_module',
        'SELECT id, username FROM game_account',
        'SELECT guid, name, x, y, z, money FROM game_character',
        'SELECT guid, owner_guid, entry, stack_count FROM game_item_instance',
    ]
    expected = [sorted(sql(query)[0]['rows']) for query in queries]
    assert expected[2] and expected[3]

    def verify():
        for query, rows in zip(queries, expected):
            assert sorted(sql(query)[0]['rows']) == rows, query
        guid = sql("SELECT guid FROM game_character WHERE name = 'Reloadalpha'")[0]['rows'][0][0]
        client = World(world_port, username='RELOADALPHA')
        try:
            assert client.read()[0] == 0x1ee
            client.send(0x3d, struct.pack('<Q', guid))
            assert guid in creates(client.collect())
            text = b'Recovered after history cleanup'
            client.send(0x95, struct.pack('<II', 0, 7) + text + b'\0')
            assert any(opcode == 0x96 and text in body for opcode, body in client.collect())
        finally:
            client.close()
        for _ in range(100):
            if not sql('SELECT connection_id FROM game_native_player')[0]['rows']:
                break
            time.sleep(0.05)
        else:
            raise AssertionError('recovery client did not leave the world')

    restart()
    verify()

    def damage_latest_snapshot():
        current, _ = files()
        assert len(current) >= 2
        object_file = next(path for path in (current[-1] / 'objects').rglob('*') if path.is_file())
        damaged = object_file.with_suffix('.damaged')
        damaged.write_bytes(b'damaged snapshot object')
        damaged.replace(object_file)

    restart(damage_latest_snapshot)
    verify()
    assert list(data.glob('replicas/*/snapshots/*.invalid_snapshot'))
    print('PASS automatic history cleanup retains state after a killed Host and fallback from a damaged snapshot')


def check_normal(data, publish, wasm):
    old_segments = set(data.glob('replicas/*/clog/*.log'))
    old_snapshots = set(data.glob('replicas/*/snapshots/*.snapshot_dir'))
    for iteration in range(21, 27):
        section = b'\x13native-reload-check' + bytes([iteration])
        publish(wasm + b'\x00' + bytes([len(section)]) + section)
    for _ in range(300):
        snapshots = set(data.glob('replicas/*/snapshots/*.snapshot_dir'))
        compressed = []
        for path in old_segments:
            if path.exists():
                with path.open('rb') as segment:
                    if segment.read(4) == b'\x28\xb5\x2f\xfd':
                        compressed.append(path)
        if len(snapshots) > 2 and compressed:
            break
        time.sleep(0.1)
    else:
        raise AssertionError('normal Host did not retain and compress history after rotation')
    assert old_snapshots <= snapshots
    assert all(path.exists() for path in old_segments)
    print('PASS normal Host retains old snapshots and log segments and still compresses history')
