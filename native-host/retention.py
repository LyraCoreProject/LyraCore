import os
import struct
import time

from check import World, creates


def check(data, world_port, sql, restart):
    def files():
        snapshots = sorted(path for path in data.glob('replicas/*/snapshots/*.snapshot_dir')
            if (path / (path.stem + '.snapshot_bsatn')).is_file() and not path.with_suffix('.lock').exists())
        segments = sorted(data.glob('replicas/*/clog/*.log'))
        return snapshots, segments

    for _ in range(300):
        snapshots, segments = files()
        if len(snapshots) == 2 and 1 <= len(segments) <= 4:
            older = int(snapshots[0].name.split('.')[0])
            starts = [int(path.name.split('.')[0]) for path in segments]
            if 0 < starts[0] <= older + 1 and (len(starts) == 1 or starts[1] > older + 1):
                break
        time.sleep(0.1)
    else:
        raise AssertionError(f'cleanup did not retain two snapshots and a shortened log: {snapshots}, {segments}')
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
        newest = current[-1] / 'objects'
        older = current[-2] / 'objects'
        common = [(path, older / path.relative_to(newest)) for path in newest.rglob('*')
            if path.is_file() and (older / path.relative_to(newest)).is_file()]
        assert common, 'snapshots have no unchanged objects to test'
        assert all(not os.path.samefile(a, b) for a, b in common), 'retained snapshots share object files'
        object_file, fallback_file = common[0]
        expected = fallback_file.read_bytes()
        with object_file.open('r+b') as damaged:
            damaged.write(b'damaged snapshot object')
            damaged.flush()
            os.fsync(damaged.fileno())
        assert fallback_file.read_bytes() == expected

    restart(damage_latest_snapshot)
    verify()
    assert list(data.glob('replicas/*/snapshots/*.invalid_snapshot'))
    print('PASS automatic history cleanup retains state after a killed Host and in-place snapshot corruption')


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
