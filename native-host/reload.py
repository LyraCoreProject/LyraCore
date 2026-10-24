import struct
import time

from check import KEY, World, creates, packed_guid


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
        for iteration in range(1, 4):
            # A WASM custom section changes the program hash without changing the Module definition.
            section = b'\x13native-reload-check' + bytes([iteration])
            candidate = wasm + b'\x00' + bytes([len(section)]) + section
            client.send(0xb7, struct.pack('<IIffffI', 0, iteration * 1000, iteration, 0, 0, 0, 0))
            publish(candidate)
            client.collect()
            observed = peer.collect()
            for _ in range(100):
                if any(opcode == 0xb7 and packed_guid(body) == guid for opcode, body in observed):
                    break
                time.sleep(0.02)
                observed += peer.collect()
            assert any(opcode == 0xb7 and packed_guid(body) == guid for opcode, body in observed)
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
        assert sql('SELECT connection_id, character_guid FROM game_native_player')[0]['rows'] == connections
        print('PASS development reload preserves World Sessions, ciphers, AOI and durable items')
    finally:
        for client in clients:
            client.close()
