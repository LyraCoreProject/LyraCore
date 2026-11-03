import struct
import time

from headless import creates


def roster(body):
    count = struct.unpack_from('<I', body)[0]
    offset = 4

    def string():
        nonlocal offset
        end = body.index(0, offset)
        result = body[offset:end].decode()
        offset = end + 1
        return result

    motd, info = string(), string()
    ranks = struct.unpack_from('<I', body, offset)[0]
    offset += 4 + 4 * ranks
    members = {}
    for _ in range(count):
        guid, online = struct.unpack_from('<QB', body, offset)
        offset += 9
        name = string()
        rank, level, cls, zone = struct.unpack_from('<IBBI', body, offset)
        offset += 10
        days = None if online else struct.unpack_from('<f', body, offset)[0]
        offset += 0 if online else 4
        members[guid] = (name, online, rank, level, cls, zone, days, string(), string())
    assert offset == len(body)
    return motd, info, members


def check(host, worlds, character, received):
    eastern, western = worlds
    _, client, guid = character('Guildalpha', 0)
    _, peer, peer_guid = character('Guildbeta', 1)
    host.wait_until(lambda: len(host.rows('SELECT guid FROM game_guild_character')) >= 4,
        'guild presence did not reach the Realm')

    def settled(shard):
        host.wait_until(lambda: not host.rows('SELECT id FROM game_realm_guild_command', shard)
            and not host.rows('SELECT id FROM game_realm_guild_presence', shard),
            'guild requests did not settle')

    def command(kind, name, result):
        return 0x93, struct.pack('<I', kind) + name.encode() + b'\0' + struct.pack('<I', result)

    def event(kind, strings=(), subject=0):
        body = bytes([kind, len(strings)]) + b''.join(s.encode() + b'\0' for s in strings)
        return 0x92, body + (struct.pack('<Q', subject) if subject else b'')

    def membership(expected):
        for shard in (None, *worlds):
            host.wait_until(lambda shard=shard: sorted(host.rows(
                f'SELECT character_guid FROM game_guild_member WHERE guild_id = {guild_id}', shard)) == sorted([[member] for member in expected]),
                f'guild membership differs on {shard}')

    client.send(0x95, struct.pack('<II', 0, 7) + b'.guild create "Across Guild"\0')
    host.wait_until(lambda: host.rows(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {guid}'),
        'Realm did not create the guild')
    guild_id = host.rows(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {guid}')[0][0]
    membership([guid])
    client.collect()
    peer.collect()

    peer.send(0x6c, b'Guildalpha\0')
    received(peer, 0x68)
    settled(western)
    client.send(0x82, b'Guildbeta\0')
    client.collect()
    settled(eastern)
    assert not host.rows(f'SELECT target_guid FROM game_guild_invite WHERE target_guid = {peer_guid}')
    assert not any(op == 0x83 for op, _ in peer.collect())
    peer.send(0x6d, struct.pack('<Q', guid))
    received(peer, 0x68)
    settled(western)
    client.send(0x82, b'gUiLdBeTa\0')
    assert received(peer, 0x83) == [(0x83, b'Guildalpha\0Across Guild\0')]
    client.collect()
    client.send(0x82, b'Guildbeta\0')
    assert command(1, 'Guildbeta', 5) in received(client, 0x93)
    peer.send(0x84)
    membership([guid, peer_guid])
    client.collect()
    peer.collect()

    peer.send(0x91, b'Forbidden MOTD\0')
    assert command(1, '', 8) in received(peer, 0x93)
    client.send(0x91, b'Across continents\0')
    assert event(2, ['Across continents']) in received(peer, 0x92)
    client.collect()
    client.send(0x235, b'Guildbeta\0Private note\0')
    assert roster(received(client, 0x8a)[-1][1])[2][peer_guid][-1] == 'Private note'
    peer.collect()
    peer.send(0x89)
    motd, _, members = roster(received(peer, 0x8a)[-1][1])
    assert motd == 'Across continents'
    assert members[peer_guid][-1] == ''
    assert members[guid][:2] == ('Guildalpha', 1)
    assert members[peer_guid][:2] == ('Guildbeta', 1)

    client.send(0x95, struct.pack('<II', 3, 7) + b'Guild across Shards\0')
    assert b'Guild across Shards\0' in received(peer, 0x96)[-1][1]
    client.collect()
    client.send(0x95, struct.pack('<II', 4, 7) + b'Officers only\0')
    received(client, 0x96)
    assert not any(op == 0x96 and b'Officers only\0' in body for op, body in peer.collect())

    client.send(0x8b, b'Guildbeta\0')
    assert event(0, ['Guildalpha', 'Guildbeta', 'Member']) in received(peer, 0x92)
    membership([guid, peer_guid])
    client.collect()
    host.call('debug_teleport', [peer_guid, 0, 10, 0, 0, 0], database=western)
    received(peer, 0x3e, auto_ack=False)
    peer.send(0xdc)
    packets = []
    while peer_guid not in creates(packets):
        packets.append(peer.read())
    peer.collect()
    membership([guid, peer_guid])
    peer.send(0x89)
    assert roster(received(peer, 0x8a)[-1][1])[2][peer_guid][2] == 3

    peer.send(0x4b)
    received(peer, 0x4d)
    client.collect()
    peer.send(0x38, struct.pack('<Q', peer_guid))
    assert received(peer, 0x3c) == [(0x3c, b'\x39')]
    membership([guid])
    client.send(0x8f)
    received(client, 0x92)
    membership([])
    for shard in (None, *worlds):
        host.wait_until(lambda shard=shard: not host.rows(f'SELECT guild_id FROM game_guild WHERE guild_id = {guild_id}', shard),
            'disbanded guild projection remained')
    client.send(0x4b)
    received(client, 0x4d)
    client.send(0x38, struct.pack('<Q', guid))
    assert received(client, 0x3c) == [(0x3c, b'\x39')]
    client.close()
    peer.close()
    check_authority_ordering(host)


def check_authority_ordering(host):
    shard = host.add_database(guid_base=3_000_000_000)
    [operator_hex] = host.rows('SELECT identity FROM game_operator')[0][0]
    operator = {'__identity__': f'0x{int(operator_hex, 16):064x}'}
    host.call('operator_add_peer_shard', [operator, 'guild ordering fixture'], database=shard)
    host.call('operator_set_shard_role', [{'some': {'world': operator}}], database=shard)
    guid = 9_000_000_020

    def string(text):
        encoded = text.encode()
        return struct.pack('<I', len(encoded)) + encoded

    def membership(revision, guild_id, present):
        record = struct.pack('<QQIB', guid, revision, guild_id, 0 if present else 1)
        if present:
            record += struct.pack('<QII', guid, guild_id, 0) + string('Member') + string('') + string('')
            record += struct.pack('<Qq', 7, 0)
        return record

    def update(revision, members=(), empty_guild=None):
        snapshots = b'' if empty_guild is None else struct.pack('<IBII', empty_guild, 1, 0, 0)
        payload = struct.pack('<QI', revision, int(empty_guild is not None)) + snapshots
        payload += struct.pack('<I', len(members)) + b''.join(members)
        host.call('idc_world_guild_update', [list(payload)], database=shard)

    def stored():
        return host.rows(f'SELECT guild_id FROM game_guild_member WHERE character_guid = {guid}', shard)

    update(20, [membership(20, 22, True)])
    assert stored() == [[22]]
    update(30, empty_guild=11)
    assert stored() == [[22]], 'a former guild roster erased a newer membership'
    update(40, [membership(10, 11, False)])
    assert stored() == [[22]], 'an old membership removal used its newer enclosing roster revision'
    update(41, [membership(21, 22, False)])
    assert stored() == []
    update(50, [membership(20, 22, True)])
    assert stored() == [], 'a late membership revived a departed member'

    host.sql(f"INSERT INTO game_guild (guild_id,name_key,name,leader_guid,team,motd,info,emblem_style,emblem_color,border_style,border_color,background_color,created_micros) VALUES (33,'legacy guild','Legacy Guild',{guid},469,'','',0,0,0,0,0,0)", shard)
    try:
        update(51, empty_guild=33)
        raise AssertionError('a projection removed unmigrated guild state')
    except RuntimeError as error:
        assert 'projection conflicts with unmigrated local guild state' in str(error)
    assert host.rows('SELECT name FROM game_guild WHERE guild_id = 33', shard) == [['Legacy Guild']]
    assert not host.rows('SELECT revision FROM game_guild_projection WHERE guild_id = 33', shard)
    host.sql('DELETE FROM game_guild WHERE guild_id = 33', shard)

    host.call('operator_set_shard_role', [{'some': {'realm': operator}}], database=shard)
    profile = operator['__identity__']
    host.sql(f"INSERT INTO game_character_profile (guid,account_id,world,revision,name,race,class,gender,deleted) VALUES ({guid},7,{profile},1,'Member',1,1,0,false)", shard)
    guild = dict(guild_id=500, name_key='imported guild', name='Imported Guild', leader_guid=guid,
        team=469, motd='Imported MOTD', info='', emblem_style=0, emblem_color=0, border_style=0,
        border_color=0, background_color=0, created_micros=0)
    ranks = [dict(id=rank + 1, guild_id=500, rank_id=rank, name=name, rights=0xff1ff if rank < 2 else 0x43)
        for rank, name in enumerate(('Guild Master', 'Officer', 'Veteran', 'Member', 'Initiate'))]
    member = dict(character_guid=guid, guild_id=500, rank_id=0, name='Member', public_note='',
        officer_note='', realm_account_id=7, joined_micros=0)
    state = dict(guild=guild, ranks=ranks, members=[member], invites=[])
    invalid = dict(state, ranks=ranks[:-1])
    try:
        host.call('operator_import_guild', [invalid], database=shard)
        raise AssertionError('incomplete ranks were imported')
    except RuntimeError as error:
        assert 'guild ranks are inconsistent' in str(error)
    assert not host.rows('SELECT guild_id FROM game_guild', shard)
    host.call('operator_import_guild', [state], database=shard)
    assert host.rows('SELECT guild_id, motd FROM game_guild', shard) == [[500, 'Imported MOTD']]
    assert host.rows('SELECT last_id FROM game_guild_sequence', shard) == [[500]]
    assert stored() == [[500]]
    try:
        host.call('operator_import_guild', [state], database=shard)
        raise AssertionError('duplicate import was accepted')
    except RuntimeError as error:
        assert 'guild id or name conflicts' in str(error)
    assert len(host.rows('SELECT id FROM game_guild_rank', shard)) == 5


def check_import(host, worlds, character, received):
    eastern, western = worlds
    leader_account, leader, leader_guid = character('Guildoldlead', 0, online=False)
    member_account, member, member_guid = character('Guildoldpeer', 1, online=False)
    logout = int((time.time() - 2 * 86400) * 1_000_000)
    host.sql(f'UPDATE game_character SET level = 37, zone_id = 14, last_logout_micros = {logout} WHERE guid = {member_guid}', western)
    guild = dict(guild_id=500, name_key='imported guild', name='Imported Guild', leader_guid=leader_guid,
        team=469, motd='Imported MOTD', info='', emblem_style=0, emblem_color=0, border_style=0,
        border_color=0, background_color=0, created_micros=0)
    ranks = [dict(id=rank + 1, guild_id=500, rank_id=rank, name=name, rights=0xff1ff if rank < 2 else 0x43)
        for rank, name in enumerate(('Guild Master', 'Officer', 'Veteran', 'Member', 'Initiate'))]
    members = [dict(character_guid=guid, guild_id=500, rank_id=rank, name=name, public_note='',
        officer_note='', realm_account_id=account.id, joined_micros=0)
        for guid, account, rank, name in [(leader_guid,leader_account,0,'Guildoldlead'),
            (member_guid,member_account,4,'Guildoldpeer')]]
    for shard in worlds:
        for table, rows in [('game_guild',[guild]),('game_guild_rank',ranks),('game_guild_member',members)]:
            for row in rows:
                values = ','.join("'" + value + "'" if isinstance(value,str) else str(value) for value in row.values())
                host.sql(f"INSERT INTO {table} ({','.join(row)}) VALUES ({values})",shard)
        host.call('operator_report_character_profiles',database=shard)
    host.wait_until(lambda: len(host.rows('SELECT guid FROM game_character_profile WHERE deleted = false')) == 2,
        'imported members have no Character profiles')
    for shard in worlds:
        host.call('operator_report_guild_characters',database=shard)
    host.wait_until(lambda: len(host.rows('SELECT guid FROM game_guild_character')) == 2,
        'offline guild roster backfill did not reach the Realm')
    host.call('operator_import_guild',[dict(guild=guild,ranks=ranks,members=members,invites=[])])
    for shard in worlds:
        host.wait_until(lambda shard=shard: host.rows('SELECT guild_id FROM game_guild_projection WHERE guild_id = 500',shard),
            'imported guild projection did not settle')
    leader.send(0x3d,struct.pack('<Q',leader_guid))
    packets=[]
    while leader_guid not in creates(packets):
        packets.append(leader.read())
    leader.collect()
    try:
        host.call('operator_report_guild_characters',database=eastern)
        raise AssertionError('roster backfill accepted an active World Session')
    except RuntimeError as error:
        assert 'gameplay closed' in str(error)
    leader.send(0x89)
    motd, _, roster_members = roster(received(leader,0x8a)[-1][1])
    offline = roster_members[member_guid]
    assert motd == 'Imported MOTD'
    assert offline[:6] == ('Guildoldpeer',0,4,37,1,14), offline
    assert 1.99 <= offline[6] <= 2.01, offline
    leader.send(0x8f)
    received(leader,0x92)
    host.wait_until(lambda: not host.rows('SELECT guild_id FROM game_guild WHERE guild_id = 500'),
        'imported guild was not disbanded')
    leader.send(0x4b)
    received(leader,0x4d)
    for client,guid in [(leader,leader_guid),(member,member_guid)]:
        client.send(0x38,struct.pack('<Q',guid))
        assert received(client,0x3c) == [(0x3c,b'\x39')]
        client.close()
    host.wait_until(lambda: not host.rows('SELECT guid FROM game_guild_character'),
        'deleted imported members retained roster facts')
