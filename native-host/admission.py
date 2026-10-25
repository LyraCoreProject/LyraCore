import socket
import struct
import time

from headless import creates


def run(host, idle_seconds=0):
    clients = []
    accounts = {}
    guids = {}
    for suffix in 'ABCDEFGH':
        account = host.account('QUEUE' + suffix)
        accounts[suffix] = account.id
        guids[suffix] = host.character('Queued' + suffix.lower(), account=account)

    def open_client(suffix, position=0):
        client = host.connect('QUEUE' + suffix)
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
        host.await_empty('game_native_admission')
        host.await_empty('game_native_player')
        host.call('native_set_admission_limits', [0, 0, 0])

    try:
        host.call('native_set_admission_limits', [1, 0, 0])
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

        host.call('native_set_admission_limits', [1, 0, 0])
        first = open_client('A')
        refused = open_client('B', 1)
        refused.send(0x3d, struct.pack('<Q', guids['B']))
        closed(refused)
        assert not host.rows(f"SELECT guid FROM game_world_entity WHERE guid = {guids['B']}")
        revoked = open_client('D', 1)
        host.sql(f"DELETE FROM game_session WHERE account_id = {accounts['D']}")
        closed(revoked)
        replacement = open_client('A', 0)
        closed(first)
        assert len(host.rows('SELECT * FROM game_native_admission')) == 1
        reset()

        host.call('native_set_admission_limits', [1, 0, 0])
        first = open_client('A')
        older = open_client('B', 1)
        replacement = open_client('A', 1)
        closed(first)
        admitted(older)
        older.close()
        admitted(replacement)
        reset()

        host.call('native_set_admission_limits', [0, 1, 0])
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
        host.call('native_set_admission_limits', [0, 0, 1])
        first = open_client('E')
        second = open_client('F', 1)
        third = open_client('G', 2)
        second_at = admitted(second)
        third_at = admitted(third)
        assert third_at - second_at >= 0.8
        assert len(host.rows('SELECT ticket FROM game_native_admission WHERE admitted = true')) == 3
        reset()
    finally:
        reset()
