import struct

from headless import creates


def check(host):
    eastern = host.add_database(guid_base=1_000_000_000)
    western = host.add_database(guid_base=2_000_000_000)
    worlds = (eastern, western)
    for shard in worlds:
        host.call('debug_seed_scenario_fixtures', database=shard)
        host.sql('DELETE FROM game_character WHERE guid = 1', shard)
        host.call('operator_add_peer_shard', [host.identity(shard), shard])
        host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=shard)
    host.sql('UPDATE game_start_position SET map_id = 1 WHERE race_class = 257', western)
    host.call('operator_set_shard_role', [{'some': {'realm': host.identity(eastern)}}])
    for shard in worlds:
        host.call('operator_set_shard_role', [{'some': {'world': host.identity()}}], database=shard)
        for peer in worlds:
            if peer != shard:
                host.call('operator_add_peer_shard', [host.identity(peer), peer], database=shard)
    for map_id, shard in enumerate(worlds):
        for owner in (None, *worlds):
            host.call('operator_set_map_owner', [map_id, host.identity(shard)], database=owner)

    def enter(client, guid):
        client.send(0x3d, struct.pack('<Q', guid))
        packets = []
        while guid not in creates(packets):
            packets.append(client.read())
        client.collect()

    def character(name, map_id):
        account = host.account(name.upper())
        host.sql(f'UPDATE game_account SET gm_level = 1 WHERE id = {account.id}')
        client = host.connect(name.upper())
        assert client.read()[0] == 0x1ee
        host.call('operator_set_character_start_map', [1, 1, map_id])
        client.send(0x36, name.encode() + b'\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0]))
        assert client.read() == (0x3a, b'\x2e')
        guid = host.rows(f"SELECT guid FROM game_character WHERE name = '{name}'", worlds[map_id])[0][0]
        host.sql(f'UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {guid}', worlds[map_id])
        enter(client, guid)
        return account, client, guid

    account, client, guid = character('Eastern', 0)
    peer_account, peer, peer_guid = character('Western', 1)

    def received(connection, opcode, count=1, auto_ack=True):
        packets = []
        host.wait_until(lambda: packets.extend(connection.collect(auto_ack=auto_ack))
            or sum(code == opcode for code, _ in packets) >= count,
            f'no complete response for opcode {opcode:#x}')
        return [packet for packet in packets if packet[0] == opcode]

    def send(connection, kind, message, language=7, target=None):
        body = struct.pack('<II', kind, language)
        if target is not None:
            body += target.encode() + b'\0'
        connection.send(0x95, body + message.encode() + b'\0')

    def line(kind, speaker, message, tag=0):
        encoded = message.encode() + b'\0'
        return 0x96, struct.pack('<BIQI', kind, 0, speaker, len(encoded)) + encoded + bytes([tag])

    def who(expected, name='', zones=()):
        host.sql('UPDATE game_native_world_connection SET who_until_micros = 0', eastern)
        body = struct.pack('<II', 0, 100) + name.encode() + b'\0\0'
        body += struct.pack('<III', 0xffffffff, 0xffffffff, len(zones))
        body += b''.join(struct.pack('<I', zone) for zone in zones) + struct.pack('<I', 0)
        client.send(0x62, body)
        if expected is None:
            assert received(client, 0x1cb) == [(0x1cb, b'Unable to complete that request. Please try again.\0')]
            return
        [(_, response)] = received(client, 0x63)
        listed, total = struct.unpack_from('<II', response)
        assert listed == total == len(expected), (listed, total, expected)
        names, rest = [], response[8:]
        for _ in range(listed):
            name, rest = rest.split(b'\0', 1)
            _, rest = rest.split(b'\0', 1)
            names.append(name.decode())
            rest = rest[16:]
        assert not rest and sorted(names) == sorted(expected), names

    def whisper(message, replies=(), tag=0, delivered=True, auto_ack=True):
        send(client, 6, message, target='wEsTeRn')
        assert received(client, 0x96, 1 + len(replies)) == [line(7, peer_guid, message), *replies]
        if delivered:
            assert received(peer, 0x96, auto_ack=auto_ack) == [line(6, guid, message, tag)]
        else:
            assert not any(opcode == 0x96 for opcode, _ in peer.collect(auto_ack=auto_ack))

    who(['Eastern', 'Western'])
    who(['Western'], name='wEsT')
    who([], zones=(0xffffff,))
    whisper('across World Shards')
    send(peer, 6, 'return whisper', target='Eastern')
    assert received(peer, 0x96) == [line(7, guid, 'return whisper')]
    assert received(client, 0x96) == [line(6, peer_guid, 'return whisper')]
    send(client, 6, 'missing Character', target='Missing')
    assert received(client, 0x2a9) == [(0x2a9, b'Missing\0')]
    send(client, 6, 'Universal whisper', language=1, target='Western')
    assert received(client, 0x96) == [line(7, peer_guid, 'Universal whisper')]
    assert received(peer, 0x96) == [line(6, guid, 'Universal whisper')]

    send(peer, 20, 'Back soon')
    peer.collect()
    whisper('away', [line(20, peer_guid, 'Back soon')])
    host.sql(f"INSERT INTO game_character_contact (id,owner_guid,owner_identity,target_guid,is_ignore) "
        f"VALUES (5090901,{peer_guid},0x{'00' * 32},{guid},true)", western)
    whisper('ignored', [line(20, peer_guid, 'Back soon'), line(22, peer_guid, 'Western')], delivered=False)
    host.sql('DELETE FROM game_character_contact WHERE id = 5090901', western)
    send(peer, 21, 'Busy')
    peer.collect()
    whisper('do not disturb', [line(21, peer_guid, 'Busy')])
    send(peer, 6, 'busy speaker', target='Eastern')
    assert received(peer, 0x96) == [line(7, guid, 'busy speaker')]
    assert received(client, 0x96) == [line(6, peer_guid, 'busy speaker', tag=2)]
    send(peer, 21, '')
    peer.collect()

    connection = host.rows(f'SELECT connection_id FROM game_native_player WHERE character_guid = {peer_guid}', western)
    host.call('debug_teleport', [peer_guid, 0, 20, 0, 0, 0], database=western)
    departure = peer.collect(auto_ack=False)
    assert any(opcode == 0x3e for opcode, _ in departure)
    who(['Eastern'])
    whisper('while loading', auto_ack=False)
    peer.send(0xdc)
    arrival = []
    while peer_guid not in creates(arrival):
        arrival.append(peer.read())
    peer.collect()
    host.wait_until(lambda: not host.rows('SELECT connection_id FROM game_native_transfer_pending', western),
        'Transfer did not finish')
    assert host.rows(f'SELECT connection_id FROM game_native_player WHERE character_guid = {peer_guid}', eastern) == connection
    who(['Eastern', 'Western'])
    whisper('after Transfer')

    def logout():
        peer.send(0x4b)
        received(peer, 0x4d)
        host.wait_until(lambda: not host.rows(f'SELECT account_id FROM game_native_away WHERE account_id = {peer_account.id}'),
            'logout retained the away World Session')

    logout()
    who(['Eastern'])
    send(client, 6, 'logged out', target='Western')
    assert received(client, 0x2a9) == [(0x2a9, b'Western\0')]
    host.sql(f'UPDATE game_character SET race = 2 WHERE guid = {peer_guid}', eastern)
    enter(peer, peer_guid)
    who(['Eastern'])
    send(client, 6, 'opposing faction', target='Western')
    assert received(client, 0x219) == [(0x219, b'')]
    assert not any(opcode == 0x96 for opcode, _ in peer.collect())

    host.call('operator_remove_peer_shard', [host.identity()], database=western)
    who(None)
    send(client, 6, 'unavailable World Shard', target='Western')
    assert received(client, 0x1cb) == [(0x1cb, b'Unable to complete that request. Please try again.\0')]
    assert not any(opcode == 0x96 for opcode, _ in peer.collect())
    host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=western)
    for reducer, args, shard in (
        ('idc_realm_communication', [[]], None),
        ('idc_world_query_answer', [1, []], None),
        ('idc_world_query', [1, []], eastern),
        ('idc_world_delivery', [1, []], eastern),
    ):
        try:
            host.call(reducer, args, database=shard)
        except RuntimeError as error:
            assert 'Peer Shard' in str(error), str(error)
        else:
            raise AssertionError(f'{reducer} admitted a non-Peer Shard')
    host.wait_until(lambda: not host.rows('SELECT id FROM game_realm_query')
        and not host.rows('SELECT id FROM game_realm_delivery'), 'Realm communication retained completed work')

    # The IDC test caller substitutes for the Realm Shard; native connections stay real.
    [operator_hex] = host.rows('SELECT identity FROM game_operator')[0][0]
    operator = {'__identity__': f'0x{int(operator_hex, 16):064x}'}
    original_connection = host.rows(f'SELECT connection_id FROM game_native_player WHERE character_guid = {guid}', eastern)[0][0]
    [connection_number] = original_connection
    recipient = struct.pack('<Q', account.id) + connection_number.to_bytes(16, 'little') + struct.pack('<Q', guid)
    notification = b'delayed Realm reply\0'
    reply = struct.pack('<IHI', 1, 0x1cb, len(notification)) + notification
    delivery = recipient + reply + b'\x01' + struct.pack('<q', 0x7fffffffffffffff)

    def deliver(payload, refused=False):
        host.call('operator_add_peer_shard', [operator, 'delivery fixture'], database=eastern)
        host.call('operator_set_shard_role', [{'some': {'world': operator}}], database=eastern)
        try:
            try:
                host.call('idc_world_delivery', [1, list(payload)], database=eastern)
            except RuntimeError as error:
                assert refused and 'recipient World Session' in str(error), str(error)
            else:
                assert not refused, 'a stale recipient received a Realm reply'
        finally:
            host.call('operator_set_shard_role', [{'some': {'world': host.identity()}}], database=eastern)
            host.call('operator_remove_peer_shard', [operator], database=eastern)

    deliver(delivery)
    assert received(client, 0x1cb) == [(0x1cb, notification)]
    deliver(delivery[:-8] + struct.pack('<q', 0))
    assert not any(opcode == 0x1cb for opcode, _ in client.collect())
    client.send(0x4b)
    received(client, 0x4d)
    host.wait_until(lambda: not host.rows(f'SELECT account_id FROM game_native_away WHERE account_id = {account.id}'),
        'logout did not return the sender to selection')
    host.call('operator_set_character_start_map', [1, 1, 0])
    client.send(0x36, b'Replacement\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0]))
    assert client.read() == (0x3a, b'\x2e')
    replacement_guid = host.rows("SELECT guid FROM game_character WHERE name = 'Replacement'", eastern)[0][0]
    enter(client, replacement_guid)
    assert host.rows(f'SELECT connection_id FROM game_native_player WHERE character_guid = {replacement_guid}', eastern) == [[original_connection]]
    deliver(delivery, refused=True)
    assert not any(opcode == 0x1cb for opcode, _ in client.collect())
    client.close()
    host.wait_until(lambda: not host.rows(f'SELECT account_id FROM game_native_away WHERE account_id = {account.id}'),
        'the closed connection retained its away World Session')
    replacement = host.connect('EASTERN')
    assert replacement.read()[0] == 0x1ee
    enter(replacement, guid)
    deliver(delivery, refused=True)
    assert not any(opcode == 0x1cb for opcode, _ in replacement.collect())
