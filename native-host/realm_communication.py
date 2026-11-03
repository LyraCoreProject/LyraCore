import struct

from headless import creates
import realm_guild
import realm_petition
import realm_mail
import realm_auction


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

    def character(name, map_id, online=True):
        account = host.account(name.upper())
        host.sql(f'UPDATE game_account SET gm_level = 1 WHERE id = {account.id}')
        client = host.connect(name.upper())
        assert client.read()[0] == 0x1ee
        host.call('operator_set_character_start_map', [1, 1, map_id])
        client.send(0x36, name.encode() + b'\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0]))
        assert client.read() == (0x3a, b'\x2e')
        guid = host.rows(f"SELECT guid FROM game_character WHERE name = '{name}'", worlds[map_id])[0][0]
        host.sql(f'UPDATE game_character SET x = 0, y = 0, z = 0 WHERE guid = {guid}', worlds[map_id])
        if online:
            enter(client, guid)
        return account, client, guid

    def received(connection, opcode, count=1, auto_ack=True):
        packets = []
        host.wait_until(lambda: packets.extend(connection.collect(auto_ack=auto_ack))
            or sum(code == opcode for code, _ in packets) >= count,
            f'no complete response for opcode {opcode:#x}')
        return [packet for packet in packets if packet[0] == opcode]

    realm_guild.check_import(host, worlds, character, received)
    realm_petition.check_import(host, worlds, character, received)
    realm_auction.check_import(host, worlds, character, received)
    account, client, guid = character('Eastern', 0)
    peer_account, peer, peer_guid = character('Western', 1)

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

    realm_guild.check(host, worlds, character, received)
    realm_petition.check(host, worlds, character, received)
    realm_mail.check(host, worlds, character, received)
    realm_auction.check(host, worlds, character, received)
    check_contacts(host, worlds, character, enter, received)

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
        ('idc_character_profiles', [[]], None),
        ('idc_character_profiles', [[]], eastern),
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
    check_profile_ordering(host, eastern)


def check_contacts(host, worlds, character, enter, received):
    eastern, western = worlds
    account, owner, guid = character('Contactowner', 0)
    target_account, target, target_guid = character('Contacttarget', 1, online=False)

    def profile(race):
        host.wait_until(lambda: host.rows(f'SELECT race FROM game_character_profile WHERE guid = {target_guid}', eastern) == [[race]],
            'remote Character profile was not published')

    def status(code, online=None):
        body = struct.pack('<BQ', code, target_guid)
        if online is not None:
            body += struct.pack('<BIII', *online)
        return 0x68, body

    def command(opcode, body, expected):
        owner.send(opcode, body)
        assert received(owner, 0x68) == [expected]

    def lists(friend_status):
        owner.send(0x66)
        packets = owner.collect()
        assert [packet for packet in packets if packet[0] in (0x67, 0x6b)] == [
            (0x67, b'\1' + struct.pack('<Q', target_guid) + friend_status),
            (0x6b, b'\1' + struct.pack('<Q', target_guid))]

    profile(1)
    host.sql(f'UPDATE game_character SET race = 2 WHERE guid = {target_guid}', western)
    host.call('operator_report_character_profiles', database=western)
    profile(2)
    command(0x69, b'Contacttarget\0', status(10))
    assert not host.rows(f'SELECT id FROM game_character_contact WHERE owner_guid = {guid}', eastern)
    host.sql(f'UPDATE game_character SET race = 1 WHERE guid = {target_guid}', western)
    host.call('operator_report_character_profiles', database=western)
    profile(1)
    command(0x69, b'cOnTaCtTaRgEt\0', status(7))
    command(0x69, b'Contacttarget\0', status(8))
    command(0x6c, b'Contacttarget\0', status(15))
    lists(b'\0')
    owner.send(0x50, struct.pack('<Q', target_guid))
    assert received(owner, 0x51) == [(0x51, struct.pack('<Q', target_guid) + b'Contacttarget\0\0' + struct.pack('<III', 1, 0, 1))]

    enter(target, target_guid)
    level, zone = host.rows(f'SELECT level, zone_id FROM game_world_entity WHERE guid = {target_guid}', western)[0]
    assert received(owner, 0x68) == [status(2, (1, zone, level, 1))]
    target.send(0x95, struct.pack('<II', 20, 7) + b'away\0')
    target.collect()
    host.wait_until(lambda: host.rows(f'SELECT chat_tag FROM game_party_character WHERE guid = {target_guid}', eastern) == [[1]],
        'remote away status did not refresh')
    lists(struct.pack('<BIII', 2, zone, level, 1))
    target.send(0x95, struct.pack('<II', 6, 7) + b'Contactowner\0blocked whisper\0')
    replies = received(target, 0x96, 2)
    assert sorted(body[0] for _, body in replies) == [7, 22]
    assert not any(opcode == 0x96 for opcode, _ in owner.collect())

    owner_level, owner_zone = host.rows(f'SELECT level, zone_id FROM game_world_entity WHERE guid = {guid}', eastern)[0]
    target.send(0x69, b'Contactowner\0')
    assert received(target, 0x68) == [(0x68, struct.pack('<BQBIII', 6, guid, 1, owner_zone, owner_level, 1))]
    host.call('debug_teleport', [guid, 1, 20, 0, 0, 0], database=eastern)
    packets = []
    while guid not in creates(packets):
        opcode, body = owner.read()
        packets.append((opcode, body))
        if opcode == 0x3e:
            owner.send(0xdc)
    owner.collect()
    host.wait_until(lambda: not host.rows(f'SELECT id FROM game_character_contact WHERE owner_guid = {guid}', eastern),
        'Transfer retained the source contact list')
    assert len(host.rows(f'SELECT id FROM game_character_contact WHERE owner_guid = {guid}', western)) == 2
    assert host.rows(f'SELECT target_guid FROM game_character_contact WHERE owner_guid = {target_guid}', western) == [[guid]], 'Transfer import removed a destination Character contact'
    lists(struct.pack('<BIII', 2, zone, level, 1))
    owner.close()
    host.wait_until(lambda: not host.rows(f'SELECT account_id FROM game_native_away WHERE account_id = {account.id}'),
        'contact owner did not disconnect')
    owner = host.connect('CONTACTOWNER')
    assert owner.read()[0] == 0x1ee
    enter(owner, guid)
    lists(struct.pack('<BIII', 2, zone, level, 1))

    host.call('debug_teleport', [target_guid, 0, 20, 0, 0, 0], database=western)
    packets = []
    while target_guid not in creates(packets):
        opcode, body = target.read()
        packets.append((opcode, body))
        if opcode == 0x3e:
            target.send(0xdc)
    target.collect()
    host.wait_until(lambda: host.rows(f'SELECT online FROM game_party_character WHERE guid = {target_guid}', western) == [[True]]
        and not host.rows('SELECT connection_id FROM game_native_transfer_pending', western),
        'contact target did not finish Transfer')
    contacts = host.rows(f'SELECT target_guid, is_ignore FROM game_character_contact WHERE owner_guid = {guid}', western)
    assert sorted(contacts) == [[target_guid, False], [target_guid, True]], f'target Transfer removed another Character\'s contacts: {contacts}'
    owner.collect()
    target.send(0x4b)
    received(target, 0x4d)
    assert received(owner, 0x68) == [status(3)]
    lists(b'\0')
    host.wait_until(lambda: not host.rows(f'SELECT account_id FROM game_native_away WHERE account_id = {target_account.id}'),
        'contact target did not return to selection')
    target.send(0x38, struct.pack('<Q', target_guid))
    assert received(target, 0x3c) == [(0x3c, b'\x39')]
    assert sorted(received(owner, 0x68, 2)) == sorted([status(5), status(16)])
    assert not host.rows(f'SELECT id FROM game_character_contact WHERE owner_guid = {guid}', western)
    owner.send(0x66)
    assert [packet for packet in owner.collect() if packet[0] in (0x67, 0x6b)] == [(0x67, b'\0'), (0x6b, b'\0')]
    owner.send(0x4b)
    received(owner, 0x4d)
    host.wait_until(lambda: not host.rows(f'SELECT account_id FROM game_native_away WHERE account_id = {account.id}'),
        'contact owner did not return to selection')
    owner.send(0x38, struct.pack('<Q', guid))
    assert received(owner, 0x3c) == [(0x3c, b'\x39')]
    owner.close()
    target.close()


def check_profile_ordering(host, world):
    [operator_hex] = host.rows('SELECT identity FROM game_operator')[0][0]
    operator = {'__identity__': f'0x{int(operator_hex, 16):064x}'}
    guid, account_id = 9_000_000_001, 9_000_001

    def selection(revision, present):
        changes = struct.pack('<IQB', 1, guid, present)
        host.call('idc_selection_result', [0, 0x3a, [], 0, account_id, list(changes), revision])

    def profile(revision, name, deleted=False):
        encoded = name.encode()
        payload = struct.pack('<IQQ', 1, guid, account_id) + int(operator_hex, 16).to_bytes(32, 'little')
        payload += struct.pack('<QI', revision, len(encoded)) + encoded + bytes([1, 1, 0, deleted])
        host.call('idc_character_profiles', [list(payload)])

    def stored(database=None):
        return host.rows(f'SELECT name, deleted FROM game_character_profile WHERE guid = {guid}', database)

    host.call('operator_add_peer_shard', [operator, 'Character profile ordering fixture'])
    host.call('operator_set_shard_role', [{'some': {'realm': operator}}])
    try:
        selection(20, True)
        profile(10, 'Beforeselection')
        assert stored() == [['Beforeselection', False]], 'selection revision discarded an unseen Character profile'
        profile(15, 'Currentprofile')
        profile(10, 'Beforeselection')
        assert stored() == [['Currentprofile', False]], 'older profile replaced newer facts'
        destination = host.identity(world)['__identity__']
        host.sql(f'UPDATE game_character_directory SET world = {destination} WHERE guid = {guid}')
        host.sql(f'UPDATE game_character_directory_version SET world = {destination}, revision = 30 WHERE guid = {guid}')
        profile(25, 'Retiredowner')
        assert stored() == [['Currentprofile', False]], 'former World owner replaced current facts'
        source = operator['__identity__']
        host.sql(f'UPDATE game_character_directory SET world = {source} WHERE guid = {guid}')
        host.sql(f'UPDATE game_character_directory_version SET world = {source}, revision = 35 WHERE guid = {guid}')
        selection(40, False)
        profile(30, 'Beforedeletion')
        assert stored() == [['Currentprofile', False]], 'a delayed profile revived a deleted Character'
        profile(35, '', deleted=True)
        profile(30, 'Beforedeletion')
        assert stored() == [['', True]], 'selection revision discarded the deletion tombstone'
        host.wait_until(lambda: stored(world) == [['', True]], 'deletion tombstone was not projected')
    finally:
        host.call('operator_set_shard_role', [{'some': {'realm': host.identity(world)}}])
        host.call('operator_remove_peer_shard', [operator])
