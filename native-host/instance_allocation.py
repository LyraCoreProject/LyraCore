import struct

from headless import creates


def check(host):
    continent, first, second = [host.add_database(guid_base=base) for base in (1_000_000_000, 2_000_000_000, 3_000_000_000)]
    worlds = (continent, first, second)
    for shard in worlds:
        host.call('debug_seed_scenario_fixtures', database=shard)
        host.sql('DELETE FROM game_character WHERE guid = 1', shard)
        host.call('operator_add_peer_shard', [host.identity(shard), shard])
        host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=shard)
    host.call('operator_set_shard_role', [{'some': {'realm': host.identity(continent)}}])
    for shard in worlds:
        host.call('operator_set_shard_role', [{'some': {'world': host.identity()}}], database=shard)
        for peer in worlds:
            if peer != shard:
                host.call('operator_add_peer_shard', [host.identity(peer), peer], database=shard)
    for owner in (None, *worlds):
        for map_id, shard in ((0, continent), (36, first), (43, first)):
            host.call('operator_set_map_owner', [map_id, host.identity(shard)], database=owner)
        for shard in (first, second):
            host.call('operator_set_instance_host', [36, host.identity(shard), True], database=owner)
        host.call('operator_set_instance_host', [43, host.identity(first), True], database=owner)
    host.call('operator_set_character_start_map', [1, 1, 0])
    entrance, leave, inside, side_exit, rejoin = 5090632, 5090633, 5090634, 5090635, 5090636
    for shard in worlds:
        for trigger, origin, target in ((entrance, 0, 36), (leave, 36, 0), (inside, 36, 43), (side_exit, 43, 0), (rejoin, 36, 36)):
            host.sql(f'INSERT INTO game_area_trigger (id,map_id,x,y,z,radius,box_length,box_width,box_height,box_yaw) '
                     f'VALUES ({trigger},{origin},1000,1000,0,10,0,0,0,0)', shard)
            host.sql(f"INSERT INTO game_areatrigger_teleport (trigger_id,target_map,x,y,z,o,name) "
                     f"VALUES ({trigger},{target},1000,1000,0,0,'Instance allocation check')", shard)

    def received(client, opcode):
        packets = []
        host.wait_until(lambda: packets.extend(client.collect()) or any(op == opcode for op, _ in packets),
                        f'missing packet {opcode:#x}')
        return [body for op, body in packets if op == opcode]

    def login(client, guid):
        client.send(0x3d, struct.pack('<Q', guid))
        packets = []
        while guid not in creates(packets):
            packets.append(client.read())
        client.collect()

    def character(name):
        host.account(name.upper())
        client = host.connect(name.upper())
        assert client.read()[0] == 0x1ee
        client.send(0x36, name.encode() + b'\0' + bytes([1, 1, 0, 0, 0, 0, 0, 0, 0]))
        assert client.read() == (0x3a, b'\x2e')
        guid = host.rows(f"SELECT guid FROM game_character WHERE name = '{name}'", continent)[0][0]
        host.sql(f'UPDATE game_character SET first_login = false, x = 1000, y = 1000, z = 0 WHERE guid = {guid}', continent)
        login(client, guid)
        return client, guid

    def cross(client, guid, target, trigger):
        client.collect()
        client.send(0xb4, struct.pack('<I', trigger))
        packets = []
        while guid not in creates(packets):
            packet = client.read()
            packets.append(packet)
            if packet[0] == 0x3e:
                client.send(0xdc)
        client.collect()
        host.wait_until(lambda: host.rows(f'SELECT character_guid FROM game_native_player WHERE character_guid = {guid}', target),
                        'Instance placement did not adopt the Character')
        return host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {guid}', target)[0][0]

    leader, leader_guid = character('Allocleader')
    member, member_guid = character('Allocmember')
    solo, solo_guid = character('Allocsolo')
    later, later_guid = character('Alloclater')
    leader.send(0x6e, b'Allocmember\0')
    received(member, 0x6f)
    member.send(0x72)
    received(leader, 0x7d)
    group = host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {leader_guid}')[0][0]
    party_run = cross(leader, leader_guid, first, entrance)
    host.call('operator_adopt_instance_placement', [group, 36, host.identity(first)])
    try:
        host.call('operator_adopt_instance_placement', [group, 36, host.identity(second)])
        raise AssertionError('existing Group placement moved to another host')
    except RuntimeError as error:
        assert 'cannot move an existing run' in str(error)
    solo_run = cross(solo, solo_guid, second, entrance)
    assert party_run == solo_run != 0
    assert cross(member, member_guid, first, entrance) == party_run
    assert host.rows(f'SELECT leader_guid FROM game_group WHERE group_id = {group}', second) == [[leader_guid]]
    solo.send(0x95, struct.pack('<II', 6, 7) + b'Allocleader\0Across allocated Instances\0')
    assert any(b'Across allocated Instances\0' in body for body in received(leader, 0x96))

    solo.send(0x4b)
    received(solo, 0x4d)
    login(solo, solo_guid)
    assert host.rows(f'SELECT instance_id FROM game_world_entity WHERE guid = {solo_guid}', second) == [[solo_run]]
    side_run = cross(solo, solo_guid, first, inside)
    assert side_run != party_run
    assert host.rows(f'SELECT instance_id FROM game_instance_binding WHERE character_guid = {solo_guid} AND map_id = 36', first) == [[solo_run]]
    for client, guid in ((leader, leader_guid), (member, member_guid)):
        assert cross(client, guid, continent, leave) == 0
    leader.send(0x31d)
    assert received(leader, 0x31e) == [struct.pack('<I', 36)]
    if host.rows(f'SELECT instance_id FROM game_instance WHERE instance_id = {party_run}', first):
        host.call('debug_reap_instance', [party_run], database=first)
    assert host.rows(f'SELECT instance_id FROM game_instance_binding WHERE character_guid = {solo_guid} AND map_id = 36', first) == [[solo_run]]
    assert host.rows(f'SELECT reset_requested FROM game_instance WHERE instance_id = {solo_run}', second) == [[False]]
    assert cross(solo, solo_guid, continent, side_exit) == 0
    assert cross(solo, solo_guid, second, entrance) == solo_run

    for owner in (None, *worlds):
        host.call('operator_set_instance_host', [36, host.identity(first), False], database=owner)
    later_run = cross(later, later_guid, second, entrance)
    assert later_run != solo_run
    renewed = cross(leader, leader_guid, first, entrance)
    assert renewed != party_run
    assert cross(member, member_guid, first, entrance) == renewed
    assert cross(leader, leader_guid, first, inside) != 0
    assert not host.rows(f'SELECT id FROM game_instance_entry_lease WHERE guid = {leader_guid}', first)
    assert cross(leader, leader_guid, continent, side_exit) == 0
    assert cross(member, member_guid, continent, leave) == 0
    host.call('operator_remove_peer_shard', [host.identity()], database=first)
    member.send(0xb4, struct.pack('<I', entrance))
    received(member, 0x1cb)
    assert host.rows(f'SELECT map_id FROM game_world_entity WHERE guid = {member_guid}', continent) == [[0]]
    assert not host.rows(f'SELECT instance_id FROM game_instance WHERE party_id = {group} AND map_id = 36', second)
    host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=first)
    assert cross(member, member_guid, first, entrance) == renewed

    for owner in (None, *worlds):
        host.call('operator_set_instance_host', [36, host.identity(second), False], database=owner)
    refused, refused_guid = character('Allocrefused')
    refused.send(0xb4, struct.pack('<I', entrance))
    received(refused, 0x1cb)
    assert host.rows(f'SELECT map_id FROM game_world_entity WHERE guid = {refused_guid}', continent) == [[0]]
    assert cross(solo, solo_guid, continent, leave) == 0
    solo.send(0x31d)
    replies = []
    host.wait_until(lambda: replies.extend(solo.collect()) or (0x31e, struct.pack('<I', 36)) in replies,
                    'solo reset did not reach its hosting Shard')
    assert host.rows(f'SELECT reset_requested FROM game_instance WHERE instance_id = {renewed}', first) == [[False]]
    leader.send(0x6e, b'Alloclater\0')
    received(later, 0x6f)
    later.send(0x72)
    received(leader, 0x7d)
    assert cross(later, later_guid, first, rejoin) == renewed
    assert not host.rows(f'SELECT character_guid FROM game_native_player WHERE character_guid = {later_guid}', second)
    for client in (leader, member, solo, later, refused):
        client.close()
