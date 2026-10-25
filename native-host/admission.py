import socket
import struct
import time

from check import KEY, World, creates


def run(world_port, call, sql, await_empty, idle_seconds=0):
    clients = []
    accounts = {}
    guids = {}
    for index, suffix in enumerate('ABCDEFGH'):
        username, name = 'QUEUE' + suffix, 'Queued' + suffix.lower()
        call('provision_account', [username, [0] * 32, [1] * 32])
        account = sql(f"SELECT id FROM game_account WHERE username = '{username}'")[0]['rows'][0][0]
        call('establish_session', [account, list(KEY), [f'0x{index + 7000:064x}']])
        call('create_character', [account, name, 1, 1, 0, 0, 0, 0, 0, 0])
        accounts[suffix] = account
        guids[suffix] = sql(f"SELECT guid FROM game_character WHERE name = '{name}'")[0]['rows'][0][0]

    def open_client(suffix, position=0):
        client = World(world_port, username='QUEUE' + suffix)
        clients.append(client)
        expect_auth(client, position)
        return client

    def expect_auth(client, position=0):
        opcode, body = client.read()
        assert opcode == 0x1ee, hex(opcode)
        if position:
            assert body == b'\x1b' + struct.pack('<I', position), body.hex()
        else:
            assert body == b'\x0c' + bytes(9), body.hex()

    def admitted(client, timeout=5):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            opcode, body = client.read()
            assert opcode == 0x1ee
            if body[0] == 0x0c:
                return time.monotonic()
            assert body[0] == 0x1b
        raise AssertionError('Login Queue did not admit the next World Session')

    def closed(client):
        client.socket.settimeout(3)
        try:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                client.read()
            raise AssertionError('World Session continued sending after its Gate failed')
        except EOFError:
            pass
        except socket.timeout:
            raise AssertionError('World Session was not closed') from None

    def reset():
        for client in clients:
            client.close()
        clients.clear()
        await_empty('game_native_admission')
        await_empty('game_native_player')
        call('native_set_admission_limits', [0, 0, 0])

    try:
        call('native_set_admission_limits', [1, 0, 0])
        first = open_client('A')
        cancelled = open_client('B', 1)
        next_client = open_client('C', 2)
        cancelled.close()
        deadline = time.monotonic() + max(1.1, idle_seconds)
        while time.monotonic() < deadline:
            expect_auth(next_client, 1)
            assert not first.collect()
        first.close()
        admitted(next_client)
        next_client.send(0x3d, struct.pack('<Q', guids['C']))
        assert guids['C'] in creates(next_client.collect())
        reset()

        call('native_set_admission_limits', [1, 0, 0])
        first = open_client('A')
        refused = open_client('B', 1)
        refused.send(0x3d, struct.pack('<Q', guids['B']))
        closed(refused)
        assert not sql(f"SELECT guid FROM game_world_entity WHERE guid = {guids['B']}")[0]['rows']
        revoked = open_client('D', 1)
        sql(f"DELETE FROM game_session WHERE account_id = {accounts['D']}")
        closed(revoked)
        replacement = open_client('A', 0)
        closed(first)
        assert len(sql('SELECT * FROM game_native_admission')[0]['rows']) == 1
        reset()

        call('native_set_admission_limits', [1, 0, 0])
        first = open_client('A')
        older = open_client('B', 1)
        replacement = open_client('A', 1)
        closed(first)
        admitted(older)
        older.close()
        admitted(replacement)
        reset()

        call('native_set_admission_limits', [0, 1, 0])
        first = open_client('A')
        next_client = open_client('B', 1)
        first.send(0x3d, struct.pack('<Q', guids['A']))
        assert guids['A'] in creates(first.collect())
        admitted(next_client)
        started = time.monotonic()
        timeout_client = open_client('C', 1)
        admitted(timeout_client, timeout=23)
        assert time.monotonic() - started >= 19
        reset()

        time.sleep(1.1)
        call('native_set_admission_limits', [0, 0, 1])
        first = open_client('E')
        second = open_client('F', 1)
        third = open_client('G', 2)
        second_at = admitted(second)
        third_at = admitted(third)
        assert third_at - second_at >= 0.8
        assert len(sql('SELECT ticket FROM game_native_admission WHERE admitted = true')[0]['rows']) == 3
        reset()
        print('PASS Module Login Queue: FIFO, cancellation, Gates, replacement, entry timeout and admission rate')
    finally:
        reset()
