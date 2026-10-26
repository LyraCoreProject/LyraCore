import socket
import struct
import threading
import time

from headless import creates, packed_guid


def rename_private_table(wasm):
    candidate = bytearray(wasm)
    offset = 8
    replacements = 0
    while offset < len(candidate):
        section = candidate[offset]
        offset += 1
        size = shift = 0
        while True:
            value = candidate[offset]
            offset += 1
            size |= (value & 0x7f) << shift
            if value < 0x80:
                break
            shift += 7
        if section == 11:
            # Equal-length names keep the WASM data section's offsets and lengths intact.
            body = candidate[offset:offset + size]
            replacements += body.count(b'game_native_player')
            candidate[offset:offset + size] = body.replace(b'game_native_player', b'game_native_plaier')
        offset += size
    assert replacements, 'private table name missing from WASM data section'
    return bytes(candidate)


def commitlog_bytes(host):
    return sum(path.stat().st_size for path in host.data.glob('replicas/*/clog/*.log'))


def run(host):
    """Replaces the Module 20 times while two World Sessions move and speak, then checks that a
    changed Module definition is refused. Leaves the Characters Reloadalpha and Reloadbeta."""
    wasm = host.module.read_bytes()
    before = commitlog_bytes(host)
    clients = []
    guids = []
    try:
        for name in ['Reloadalpha', 'Reloadbeta']:
            client = host.enter_world(name, at=(0, 0, 0, 0))
            client.socket.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            client.socket.settimeout(120)
            clients.append(client)
            guids.append(client.guid)
        client, peer = clients
        guid = guids[0]
        assert guids[1] in creates(client.collect())
        connections = host.rows('SELECT connection_id, character_guid FROM game_native_player')
        items = host.rows(f'SELECT guid, entry, stack_count FROM game_item_instance WHERE owner_guid = {guid}')
        assert items
        host.call('debug_set_money', [guid, 123456])
        client.collect()
        peer.collect()
        sequence = 0
        traffic_seconds = 0
        previous_hash = host.rows('SELECT program_hash FROM st_module')
        for iteration in range(1, 21):
            # A WASM custom section changes the program hash without changing the Module definition.
            section = b'\x13native-reload-check' + bytes([iteration])
            candidate = wasm + b'\x00' + bytes([len(section)]) + section
            stopped = threading.Event()
            relayed = threading.Event()
            failures = []

            def traffic():
                nonlocal sequence
                try:
                    while not stopped.is_set():
                        sequence += 1
                        client.send(0xb7, struct.pack('<IIffffI', 0, sequence * 10, iteration, 0, 0, 0, 0))
                        client.collect()
                        if any(opcode == 0xb7 and packed_guid(body) == guid for opcode, body in peer.collect()):
                            relayed.set()
                        stopped.wait(0.01)
                except Exception as error:
                    failures.append(error)

            worker = threading.Thread(target=traffic, daemon=True)
            started = time.monotonic()
            worker.start()
            try:
                host.publish(candidate)
            finally:
                stopped.set()
                worker.join(timeout=10)
            traffic_seconds += time.monotonic() - started
            assert not worker.is_alive(), 'native requests stalled during replacement'
            assert not failures, (iteration, failures)
            assert relayed.is_set(), 'movement was not relayed during replacement'
            current_hash = host.rows('SELECT program_hash FROM st_module')
            assert current_hash != previous_hash, 'publish did not replace the Module'
            previous_hash = current_hash
            assert host.rows('SELECT connection_id, character_guid FROM game_native_player') == connections
            state = host.rows(f'SELECT x, money FROM game_world_entity WHERE guid = {guid}')
            assert state == [[iteration, 123456]], (iteration, state)
            assert sorted(host.rows(f'SELECT guid, entry, stack_count FROM game_item_instance WHERE owner_guid = {guid}')) == sorted(items)
            text = f'Reload {iteration}'.encode()
            client.send(0x95, struct.pack('<II', 0, 7) + text + b'\0')
            assert any(opcode == 0x96 and text in body for opcode, body in client.collect())
            assert any(opcode == 0x96 and text in body for opcode, body in peer.collect())
        try:
            host.publish(b'not wasm')
        except RuntimeError:
            pass
        else:
            raise AssertionError('malformed WASM was published')
        for client in clients:
            client.collect()
        try:
            host.publish(rename_private_table(wasm))
        except RuntimeError as error:
            assert 'Development reload requires an unchanged Module definition' in str(error), error
        else:
            raise AssertionError('development Host accepted a private table rename')
        assert host.rows('SELECT program_hash FROM st_module') == previous_hash
        for client in clients:
            client.collect()
        assert host.rows('SELECT connection_id, character_guid FROM game_native_player') == connections
        print(f'{sequence} movement requests ({sequence / traffic_seconds:.1f}/s) during 20 development reloads')
        print(f'Reload storage: WASM {len(wasm)} bytes; commit log grew {commitlog_bytes(host) - before} bytes')
    finally:
        for client in clients:
            client.close()
