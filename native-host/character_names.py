import struct


def check(host):
    eastern, western = [host.add_database(guid_base=base) for base in (1_000_000_000, 2_000_000_000)]
    worlds = (eastern, western)
    for shard in worlds:
        host.call('debug_seed_scenario_fixtures', database=shard)
        host.sql('DELETE FROM game_character WHERE guid = 1', shard)
        host.call('operator_add_peer_shard', [host.identity(shard), shard])
        host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=shard)
    host.sql('UPDATE game_start_position SET map_id = 1 WHERE race_class = 257', western)
    host.sql('INSERT INTO game_start_position (race_class,race,class,map_id,zone_id,x,y,z,orientation,display_id) '
             'VALUES (513,2,1,1,14,0,0,0,0,51)', western)
    host.call('create_character', [1, 'Legacyname', 1, 1, 0, 0, 0, 0, 0, 0], database=western)
    host.call('operator_set_shard_role', [{'some': {'realm': host.identity(eastern)}}])
    for map_id, shard in enumerate(worlds):
        host.call('operator_set_map_owner', [map_id, host.identity(shard)])
    host.call('operator_set_character_start_map', [1, 1, 0])
    host.call('operator_set_character_start_map', [2, 1, 1])
    host.call('operator_set_shard_role', [{'some': {'world': host.identity()}}], database=eastern)
    account = host.account('NAMESONE')
    other = host.account('NAMESTWO')

    def connect(username):
        client = host.connect(username)
        assert client.read()[0] == 0x1ee
        return client

    def send(client, name, race=1):
        client.send(0x36, name.encode() + b'\0' + bytes([race, 1, 0, 0, 0, 0, 0, 0, 0]))

    def create(client, name, result=0x2e):
        send(client, name)
        assert client.read() == (0x3a, bytes([result]))

    one, two = connect(account.username), connect(other.username)
    create(one, 'Beforeimport', 0x2f)
    assert not host.rows('SELECT name FROM game_character_name')
    host.call('operator_set_shard_role', [{'some': {'world': host.identity()}}], database=western)
    host.wait_until(lambda: len(host.rows('SELECT world FROM game_character_names_ready')) == 2,
                    'existing World names did not arrive')
    create(one, 'LEGACYNAME', 0x31)
    assert not host.rows("SELECT guid FROM game_character WHERE name = 'LEGACYNAME'", eastern)
    try:
        host.call('create_character', [1, 'Bypassed', 1, 1, 0, 0, 0, 0, 0, 0], database=eastern)
        raise AssertionError('direct World creation bypassed the Realm reservation')
    except RuntimeError as error:
        assert 'Realm name reservation' in str(error)

    send(one, 'Sharedname')
    send(two, 'sHAREDNAME', race=2)
    answers = [one.read(), two.read()]
    assert sorted(answers) == [(0x3a, b'\x2e'), (0x3a, b'\x31')]
    winner = answers.index((0x3a, b'\x2e'))
    winning_name = ('Sharedname', 'sHAREDNAME')[winner]
    guid = host.rows(f"SELECT guid FROM game_character WHERE name = '{winning_name}'", worlds[winner])[0][0]
    assert not host.rows(f"SELECT guid FROM game_character WHERE name = '{('sHAREDNAME', 'Sharedname')[winner]}'", worlds[1 - winner])
    winning_client = (one, two)[winner]
    winning_client.send(0x38, struct.pack('<Q', guid))
    assert winning_client.read() == (0x3c, b'\x39')
    host.call('operator_set_character_start_map', [1, 1, 1])
    create(two, 'SHAREDNAME')

    host.sql('UPDATE game_start_position SET map_id = 0 WHERE race_class = 257', western)
    host.call('operator_set_map_owner', [1, host.identity(western)], database=western)
    create(two, 'Retryname', 0x2f)
    assert not host.rows("SELECT name FROM game_character_name WHERE name = 'retryname'")
    host.sql('UPDATE game_start_position SET map_id = 1 WHERE race_class = 257', western)
    create(two, 'Retryname')

    for index in range(7):
        host.call('operator_set_character_start_map', [1, 1, index % 2])
        create(two, f'Capacity{chr(97 + index)}')
    host.call('operator_set_character_start_map', [1, 1, 1])
    host.call('operator_remove_peer_shard', [host.identity(western)])
    send(two, 'Aftertimeout')
    host.wait_until(lambda: host.rows("SELECT guid FROM game_character WHERE name = 'Aftertimeout'", western),
                    'creation did not commit on the World Shard')
    host.wait_until(lambda: not host.rows('SELECT id FROM game_pending_selection'),
                    'selection did not expire', timeout=15)
    held = host.rows("SELECT * FROM game_character_name WHERE name = 'aftertimeout'")
    assert held and held[0][-1] == 0
    host.restart()
    for shard in worlds:
        host.rows('SELECT guid FROM game_character', shard)
    assert host.rows("SELECT * FROM game_character_name WHERE name = 'aftertimeout'") == held
    one, two = connect(account.username), connect(other.username)
    host.call('operator_set_character_start_map', [1, 1, 0])
    create(one, 'AFTERTIMEOUT', 0x31)
    create(two, 'Pendingcapacity', 0x34)
    host.call('operator_add_peer_shard', [host.identity(western), western])
    host.wait_until(lambda: host.rows("SELECT guid FROM game_character_name WHERE name = 'aftertimeout'")[0][0] != 0,
                    'replayed profile did not complete the reservation', timeout=15)
    two.send(0x37)
    opcode, body = two.read()
    assert opcode == 0x3b and b'Aftertimeout\0' in body
    create(two, 'Overcapacity', 0x34)
    assert len(host.rows(f'SELECT guid FROM game_character_directory WHERE account_id = {other.id}')) == 10

    added = host.add_database(guid_base=3_000_000_000)
    host.call('debug_seed_scenario_fixtures', database=added)
    host.sql('DELETE FROM game_character WHERE guid = 1', added)
    host.sql('UPDATE game_start_position SET map_id = 2 WHERE race_class = 257', added)
    host.call('create_character', [1, 'LEGACYNAME', 1, 1, 0, 0, 0, 0, 0, 0], database=added)
    host.call('operator_add_peer_shard', [host.identity(added), added])
    host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=added)
    host.call('operator_set_map_owner', [2, host.identity(added)])
    host.call('operator_set_shard_role', [{'some': {'world': host.identity()}}], database=added)
    host.wait_until(lambda: host.rows('SELECT id FROM game_character_profiles_retry', added),
                    'conflicting existing names were not refused')
    create(one, 'Importblocked', 0x2f)
    assert len(host.rows('SELECT world FROM game_character_names_ready')) == 2
    host.sql("UPDATE game_character SET name = 'Distinctname' WHERE name = 'LEGACYNAME'", added)
    host.call('operator_report_character_profiles', database=added)
    host.wait_until(lambda: len(host.rows('SELECT world FROM game_character_names_ready')) == 3,
                    'corrected names did not complete the import')
    create(one, 'Importblocked')
    create(one, 'Distinctname', 0x31)
    for client in (one, two):
        client.close()
