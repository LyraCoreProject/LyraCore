import struct

from realm_petition import enter, remove, settled, transfer


def check(host, worlds, character, received):
    stone, other = 0xf110000000000962, 0xf110000000000963
    for map_id, shard in enumerate(worlds):
        host.call('debug_stage_meeting_stone', [5090622, stone, 1, 60, 1581, map_id, 1, 0, 0], database=shard)
        host.call('debug_stage_meeting_stone', [5090623, other, 1, 60, 717, map_id, 2, 0, 0], database=shard)
    players = []
    for index, (name, character_class, race) in enumerate([
        ('Stoneeast', 1, 1), ('Stonewest', 5, 1), ('Stonemage', 8, 1),
        ('Stonerogue', 4, 1), ('Stonewarlock', 9, 1), ('Stonehorde', 1, 2),
    ]):
        shard_index = index % 2
        account, client, guid = character(name, shard_index, online=False)
        host.sql(f'UPDATE game_character SET class = {character_class}, race = {race} WHERE guid = {guid}', worlds[shard_index])
        enter(client, guid)
        settled(host, worlds[shard_index], guid)
        players.append([client, guid, shard_index])
    first, owner, _ = players[0]
    second, _, _ = players[1]
    fifth, fifth_guid, _ = players[4]
    horde, horde_guid, _ = players[5]

    def queue(area, status):
        return 0x295, struct.pack('<IB', area, status)

    def status(client, area, state):
        assert queue(area, state) in received(client, 0x295)

    def join(client, target=stone):
        client.send(0x292, struct.pack('<Q', target))

    def seeker(guid):
        return host.rows(f'SELECT area_id,team,class,group_id FROM game_meeting_stone_seeker WHERE character_guid = {guid}')

    first.send(0x296)
    status(first, 0, 5)
    join(first, 0)
    assert not any(op == 0x295 for op, _ in first.collect())
    assert not seeker(owner)
    host.sql('UPDATE game_meeting_stone SET min_level = 2 WHERE entry = 5090622', worlds[0])
    join(first)
    assert not any(op == 0x295 for op, _ in first.collect())
    assert not seeker(owner)
    host.sql('UPDATE game_meeting_stone SET min_level = 1 WHERE entry = 5090622', worlds[0])
    join(first, other)
    status(first, 717, 1)
    first.send(0x293)
    status(first, 0, 0)
    join(first)
    status(first, 1581, 1)
    transfer(host, first, owner, worlds[0], 1, received)
    players[0][2] = 1
    first.send(0x296)
    status(first, 1581, 1)
    assert seeker(owner) == [[1581, 469, 1, 0]]
    first.send(0x4b)
    received(first, 0x4d)
    assert not seeker(owner)
    enter(first, owner)
    first.send(0x296)
    status(first, 0, 5)

    host.call('operator_remove_peer_shard', [host.identity()], database=worlds[1])
    join(first)
    host.wait_until(lambda: seeker(owner), 'Realm did not queue the admitted Seeker')
    host.wait_until(lambda: host.rows('SELECT id FROM game_party_projection_retry'), 'refused queue delivery did not retry')
    host.call('operator_add_peer_shard', [host.identity(), 'realm'], database=worlds[1])
    status(first, 1581, 1)
    host.wait_until(lambda: not host.rows('SELECT id FROM game_party_projection_retry'), 'queue retries did not drain')
    join(horde)
    status(horde, 1581, 1)
    horde.close()
    host.wait_until(lambda: not seeker(horde_guid), 'disconnected solo Seeker remained queued')
    horde = host.connect('STONEHORDE')
    assert horde.read()[0] == 0x1ee
    players[5][0] = horde
    enter(horde, horde_guid)
    horde.send(0x296)
    status(horde, 0, 5)
    join(horde)
    status(horde, 1581, 1)
    for client, _, _ in players[1:4]:
        join(client)
        status(client, 1581, 1)
    assert not host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {owner}')
    join(fifth)
    for client, _, _ in players[:5]:
        assert (0x297, b'') in received(client, 0x297)
    group = host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {owner}')[0][0]
    guids = {guid for _, guid, _ in players[:5]}
    for shard in [None, *worlds]:
        host.wait_until(lambda: {row[0] for row in host.rows(f'SELECT character_guid FROM game_group_member WHERE group_id = {group}', shard)} == guids, 'Stone Add roster did not reach a World')
    assert not host.rows('SELECT group_id FROM game_meeting_stone_party')
    assert seeker(horde_guid) == [[1581, 67, 1, 0]]
    assert all(not seeker(guid) for _, guid, _ in players[:5])
    assert all(not host.rows('SELECT character_guid FROM game_meeting_stone_seeker', shard) for shard in worlds)
    join(second)
    assert received(second, 0x2bb) == [(0x2bb, b'\x01')]
    join(first)
    assert received(first, 0x2bb) == [(0x2bb, b'\x02')]

    first.send(0x75, b'Stonewarlock\0')
    host.wait_until(lambda: not host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {fifth_guid}'), 'kick did not reach Realm')
    for client, _, _ in players:
        client.collect()
    join(first)
    for client, _, _ in players[:4]:
        status(client, 1581, 1)
    second.send(0x293)
    status(second, 0, 5)
    assert seeker(players[1][1])[0][3] == group
    host.call('debug_backdate_meeting_stone_party', [group, 301])
    for client, _, _ in players[:4]:
        packets = []
        host.wait_until(lambda: packets.extend(client.collect()) or (0x298, b'') in packets, 'queued Party reminder did not cross Shards', timeout=8)
    assert not any(op == 0x298 for op, _ in fifth.collect())
    join(fifth)
    for client, _, _ in players[:5]:
        assert (0x297, b'') in received(client, 0x297)
    assert not seeker(fifth_guid)

    first.send(0x75, b'Stonewarlock\0')
    host.wait_until(lambda: not host.rows(f'SELECT group_id FROM game_group_member WHERE character_guid = {fifth_guid}'), 'second kick did not reach Realm')
    join(first)
    status(first, 1581, 1)
    first.send(0x75, b'Stonemage\0')
    kicked = players[2][1]
    host.wait_until(lambda: seeker(kicked) and seeker(kicked)[0][3] == 0, 'kicked Seeker did not requeue on Realm')
    assert not host.rows(f'SELECT group_id FROM game_meeting_stone_party WHERE group_id = {group}')
    for client, guid, _ in players:
        client.collect()
        client.send(0x293)
        client.send(0x7b)
    host.wait_until(lambda: not host.rows('SELECT character_guid FROM game_meeting_stone_seeker'), 'Meeting Stone Queue did not drain')
    host.wait_until(lambda: not host.rows(f'SELECT group_id FROM game_group WHERE group_id = {group}'), 'Party did not disband')
    for client, guid, _ in players:
        client.collect()
        remove(client, guid, received)
