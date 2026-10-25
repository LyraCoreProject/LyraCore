import socket
import struct
import threading
import time

from check import KEY, World, creates, packed_guid


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


def run(world_port, call, sql, publish, wasm):
    clients = []
    guids = []
    try:
        for index, name in enumerate(['Reloadalpha', 'Reloadbeta']):
            username = name.upper()
            call('provision_account', [username, [0] * 32, [1] * 32])
            account = sql(f"SELECT id FROM game_account WHERE username = '{username}'")[0]['rows'][0][0]
            call('establish_session', [account, list(KEY), [f'0x{index + 8000:064x}']])
            call('create_character', [account, name, 1, 1, 0, 0, 0, 0, 0, 0])
            guid = sql(f"SELECT guid FROM game_character WHERE name = '{name}'")[0]['rows'][0][0]
            sql(f'UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {guid}')
            client = World(world_port, username=username)
            client.socket.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            clients.append(client)
            guids.append(guid)
            client.socket.settimeout(120)
            assert client.read()[0] == 0x1ee
            client.send(0x3d, struct.pack('<Q', guid))
            assert guid in creates(client.collect())
        client, peer = clients
        guid = guids[0]
        assert guids[1] in creates(client.collect())
        connections = sql('SELECT connection_id, character_guid FROM game_native_player')[0]['rows']
        items = sql(f'SELECT guid, entry, stack_count FROM game_item_instance WHERE owner_guid = {guid}')[0]['rows']
        assert items
        call('debug_set_money', [guid, 123456])
        client.collect()
        peer.collect()
        sequence = 0
        traffic_seconds = 0
        previous_hash = sql('SELECT program_hash FROM st_module')[0]['rows']
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
                publish(candidate)
            finally:
                stopped.set()
                worker.join(timeout=10)
            traffic_seconds += time.monotonic() - started
            assert not worker.is_alive(), 'native requests stalled during replacement'
            assert not failures, failures
            assert relayed.is_set(), 'movement was not relayed during replacement'
            current_hash = sql('SELECT program_hash FROM st_module')[0]['rows']
            assert current_hash != previous_hash, 'publish did not replace the Module'
            previous_hash = current_hash
            assert sql('SELECT connection_id, character_guid FROM game_native_player')[0]['rows'] == connections
            state = sql(f'SELECT x, money FROM game_world_entity WHERE guid = {guid}')[0]['rows']
            assert state == [[iteration, 123456]], (iteration, state)
            assert sorted(sql(f'SELECT guid, entry, stack_count FROM game_item_instance WHERE owner_guid = {guid}')[0]['rows']) == sorted(items)
            text = f'Reload {iteration}'.encode()
            client.send(0x95, struct.pack('<II', 0, 7) + text + b'\0')
            assert any(opcode == 0x96 and text in body for opcode, body in client.collect())
            assert any(opcode == 0x96 and text in body for opcode, body in peer.collect())
        try:
            publish(b'not wasm')
        except RuntimeError:
            pass
        else:
            raise AssertionError('malformed WASM was published')
        for client in clients:
            client.collect()
        try:
            publish(rename_private_table(wasm))
        except RuntimeError as error:
            assert 'Development reload requires an unchanged Module definition' in str(error), error
        else:
            raise AssertionError('development Host accepted a private table rename')
        assert sql('SELECT program_hash FROM st_module')[0]['rows'] == previous_hash
        for client in clients:
            client.collect()
        assert sql('SELECT connection_id, character_guid FROM game_native_player')[0]['rows'] == connections
        print(f'PASS 20 development reloads with {sequence} movement requests ({sequence / traffic_seconds:.1f}/s) preserve World Sessions, ciphers, AOI and durable items; schema changes are refused')
    finally:
        for client in clients:
            client.close()
